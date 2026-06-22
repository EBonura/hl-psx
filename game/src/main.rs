//! hl-psx -- render a real Half-Life BSP map (cooked to `.hlm` by `tools/hl-bsp`)
//! with a GTE-projected, ordering-table-sorted, pad-driven fly-camera.
//!
//! M1 geometry, M2 textures (4-bit CLUT), M3 per-face lightmap shading, M4 PVS:
//! each frame we find the camera's BSP leaf, decompress its potentially-visible
//! set, and draw only the faces of visible leaves.
//!
//! Controls (digital pad): D-pad up/down = forward/back, left/right = turn,
//! L1/R1 = down/up, Triangle/Cross = look up/down.

#![no_std]
#![no_main]

extern crate psx_rt;

mod map;
mod phys;
mod vram;

use psx_gpu::ot::OrderingTable;
use psx_gpu::prim::TriTexturedGouraud;
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_gte::math::{Mat3I16, Vec3I32};
use psx_gte::scene;
use psx_pad::{button, poll_port1};
use psx_rt::tty;

use map::Map;
use vram::{TexSlot, EMPTY_SLOT};

// Cooked at build time from the user's own Half-Life install (git-ignored).
static MAP_BYTES: &[u8] = include_bytes!("../../data/maps/c1a0.hlm");

const OT_LEN: usize = 1024;
const MAX_VERTS: usize = 8192;
const MAX_PRIMS: usize = 12000;
const MAX_TEX_SLOTS: usize = 512;
const MAX_FACES: usize = 8192; // c1a0 has 3695
const MAX_LEAVES: usize = 8192; // c1a0 has 1438
const NEAR: u16 = 32;
// ponytail: backface cull off; flip on once winding is confirmed from a capture.
const CULL: bool = false;
const H_PROJ: u16 = 160; // ~90 deg horizontal FOV at 320px

// Camera control rates. Angles are Q0.12 (4096 = one revolution) for `sincos`;
// the GTE matrix builders take 256/rev, hence `>> 4`.
const YAW_STEP: u16 = 48;
const PITCH_STEP: i16 = 32;
const PITCH_MAX: i16 = 1000;
const VIEW_HEIGHT: i32 = 28; // eye above the player origin (world units)

static mut OT: OrderingTable<OT_LEN> = OrderingTable::new();
const EMPTY_TRI: TriTexturedGouraud = TriTexturedGouraud::new(
    [(0, 0), (0, 0), (0, 0)],
    [(0, 0), (0, 0), (0, 0)],
    [(128, 128, 128), (128, 128, 128), (128, 128, 128)],
    0,
    0,
);
static mut PRIMS: [TriTexturedGouraud; MAX_PRIMS] = [EMPTY_TRI; MAX_PRIMS];
static mut TEX_SLOTS: [TexSlot; MAX_TEX_SLOTS] = [EMPTY_SLOT; MAX_TEX_SLOTS];
// PVS scratch: decompressed visible-leaf bits + per-face draw-once dedup.
static mut VIS_BITS: [u8; MAX_LEAVES / 8] = [0; MAX_LEAVES / 8];
static mut FACE_FRAME: [u16; MAX_FACES] = [0; MAX_FACES];

/// World->view rotation: rotY(yaw)*rotX(pitch), rows 0/1 negated for the GPU's
/// Y-down screen (same convention as oot-psx).
fn view_rotation(yaw: u16, pitch: i16) -> Mat3I16 {
    let look = Mat3I16::rotate_y((yaw >> 4) as u16).mul(&Mat3I16::rotate_x((pitch >> 4) as u16));
    let mut r = look;
    let mut j = 0;
    while j < 3 {
        r.m[0][j] = -r.m[0][j];
        r.m[1][j] = -r.m[1][j];
        j += 1;
    }
    r
}

fn dot12(row: [i16; 3], e: [i32; 3]) -> i32 {
    ((row[0] as i32 * e[0]) + (row[1] as i32 * e[1]) + (row[2] as i32 * e[2])) >> 12
}

/// Walk the BSP tree to the leaf containing `eye` (world space). Returns 0 (the
/// solid/outside leaf) if the tree is empty or the point falls outside.
fn camera_leaf(m: &Map, eye: [i32; 3]) -> i32 {
    if m.n_nodes == 0 {
        return 0;
    }
    let mut idx = 0i32;
    let mut guard = 0;
    loop {
        if idx < 0 || idx as usize >= m.n_nodes || guard > 256 {
            return 0;
        }
        guard += 1;
        let nd = m.node(idx as usize);
        let side = ((nd.n[0] as i64 * eye[0] as i64
            + nd.n[1] as i64 * eye[1] as i64
            + nd.n[2] as i64 * eye[2] as i64)
            >> 12) as i32
            - nd.dist;
        let next = if side >= 0 { nd.c0 } else { nd.c1 };
        if next < 0 {
            return -next - 1; // leaf index
        }
        idx = next;
    }
}

/// Quake run-length vis decompression into `out` (bit i -> leaf i+1).
fn decompress_vis(m: &Map, visofs: i32, out: &mut [u8]) {
    let row = ((m.n_leaves.saturating_sub(1)) + 7) / 8;
    let row = row.min(out.len());
    for b in out[..row].iter_mut() {
        *b = 0;
    }
    if visofs < 0 {
        for b in out[..row].iter_mut() {
            *b = 0xFF; // no vis info -> everything visible
        }
        return;
    }
    let vis = m.vis();
    let mut v = visofs as usize;
    let mut c = 0usize;
    while c < row {
        if v >= vis.len() {
            break;
        }
        if vis[v] != 0 {
            out[c] = vis[v];
            v += 1;
            c += 1;
        } else {
            v += 1;
            if v >= vis.len() {
                break;
            }
            let mut cnt = vis[v];
            v += 1;
            while cnt > 0 && c < row {
                out[c] = 0;
                c += 1;
                cnt -= 1;
            }
        }
    }
}

/// Project one triangle, cull, and insert it into the OT. `np` is the running
/// primitive-pool cursor.
unsafe fn emit_tri(m: &Map, t: usize, nv: usize, np: &mut usize) {
    let (a, b, c) = m.tri_idx(t);
    if a >= nv || b >= nv || c >= nv {
        return;
    }
    let slot = TEX_SLOTS[m.tri_tex(t).min(MAX_TEX_SLOTS - 1)];
    if !slot.valid {
        return;
    }
    let p = scene::project_triangle(m.vert(a), m.vert(b), m.vert(c));
    let (pa, pb, pc) = (p[0], p[1], p[2]);
    if pa.sz < NEAR || pb.sz < NEAR || pc.sz < NEAR {
        return;
    }
    if CULL {
        let area = (pb.sx as i32 - pa.sx as i32) * (pc.sy as i32 - pa.sy as i32)
            - (pc.sx as i32 - pa.sx as i32) * (pb.sy as i32 - pa.sy as i32);
        if area <= 0 {
            return;
        }
    }
    let avgz = ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3;
    let mut otz = (avgz >> 6) as usize;
    if otz == 0 {
        otz = 1;
    } else if otz >= OT_LEN {
        otz = OT_LEN - 1;
    }
    if *np >= MAX_PRIMS {
        return;
    }
    let (sr, sg, sb) = m.tri_rgb(t);
    PRIMS[*np] = TriTexturedGouraud::with_material(
        [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
        m.tri_uv(t),
        [(sr, sg, sb), (sr, sg, sb), (sr, sg, sb)],
        slot.material,
    );
    OT.add(otz, &mut PRIMS[*np], TriTexturedGouraud::WORDS);
    *np += 1;
}

#[no_mangle]
fn main() {
    tty::println("hl-psx: booting M4 (PVS) map renderer");

    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    scene::set_screen_offset(160 << 16, 120 << 16);
    scene::set_projection_plane(H_PROJ);

    let m = Map::load(MAP_BYTES);
    let nv = if m.n_verts < MAX_VERTS { m.n_verts } else { MAX_VERTS };

    let failed = unsafe { vram::upload_textures(&m, &mut TEX_SLOTS) };
    if failed > 0 {
        tty::println("hl-psx: some textures did not fit VRAM");
    }

    let mut player = phys::Player::new(m.spawn_pos);
    let mut yaw: u16 = (m.spawn_yaw as u16) & 0xFFF;
    let mut pitch: i16 = 0;
    let mut frame_no: u16 = 0;

    loop {
        let held = poll_port1().buttons;
        if held.is_held(button::LEFT) {
            yaw = yaw.wrapping_sub(YAW_STEP) & 0xFFF;
        }
        if held.is_held(button::RIGHT) {
            yaw = yaw.wrapping_add(YAW_STEP) & 0xFFF;
        }
        if held.is_held(button::TRIANGLE) {
            pitch = (pitch + PITCH_STEP).min(PITCH_MAX);
        }
        if held.is_held(button::CROSS) {
            pitch = (pitch - PITCH_STEP).max(-PITCH_MAX);
        }
        let fwd = if held.is_held(button::UP) {
            1
        } else if held.is_held(button::DOWN) {
            -1
        } else {
            0
        };
        let strafe = if held.is_held(button::R1) {
            1
        } else if held.is_held(button::L1) {
            -1
        } else {
            0
        };
        let jump = held.is_held(button::CIRCLE);
        player.update(&m, fwd, strafe, jump, yaw);
        let eye = [player.pos[0], player.pos[1] + VIEW_HEIGHT, player.pos[2]];

        let rot = view_rotation(yaw, pitch);
        scene::load_rotation(&rot);
        let t = [-dot12(rot.m[0], eye), -dot12(rot.m[1], eye), -dot12(rot.m[2], eye)];
        scene::load_translation(Vec3I32::new(t[0], t[1], t[2]));

        frame_no = frame_no.wrapping_add(1);

        unsafe {
            if frame_no == 0 {
                for f in FACE_FRAME.iter_mut() {
                    *f = 0;
                }
                frame_no = 1;
            }
            OT.clear();
            let mut np = 0usize;
            let cam_leaf = camera_leaf(&m, eye);
            if cam_leaf > 0 && (cam_leaf as usize) < m.n_leaves {
                let (visofs, _, _) = m.leaf(cam_leaf as usize);
                decompress_vis(&m, visofs, &mut VIS_BITS);
                let leaf_bits = m.n_leaves.saturating_sub(1);
                for i in 0..leaf_bits {
                    if VIS_BITS[i >> 3] & (1 << (i & 7)) == 0 {
                        continue;
                    }
                    let (_, m0, mc) = m.leaf(i + 1);
                    for mj in m0..m0 + mc {
                        if mj >= m.n_marks {
                            break;
                        }
                        let face = m.mark(mj);
                        if face >= MAX_FACES || FACE_FRAME[face] == frame_no {
                            continue;
                        }
                        FACE_FRAME[face] = frame_no;
                        let (first, cnt) = m.face_tris(face);
                        for tt in first..first + cnt {
                            if tt < m.n_tris {
                                emit_tri(&m, tt, nv, &mut np);
                            }
                        }
                    }
                }
            } else {
                // Camera outside the world hull: draw everything.
                for tt in 0..m.n_tris {
                    emit_tri(&m, tt, nv, &mut np);
                }
            }

            fb.clear(0, 0, 0);
            OT.submit();
        }

        gpu::vsync();
        fb.swap();
    }
}

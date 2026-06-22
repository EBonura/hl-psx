//! hl-psx Milestone 1 -- render a real Half-Life BSP map (cooked to `.hlm` by
//! `tools/hl-bsp`) with a GTE-projected, ordering-table-sorted, pad-driven
//! fly-camera. Flat per-triangle colour (each triangle painted with its source
//! texture's average colour); textures, PVS, and entities are later milestones.
//!
//! Controls (digital pad): D-pad up/down = forward/back, left/right = turn,
//! L1/R1 = down/up, Triangle/Cross = look up/down.

#![no_std]
#![no_main]

extern crate psx_rt;

mod map;
mod vram;

use psx_gpu::ot::OrderingTable;
use psx_gpu::prim::TriTexturedGouraud;
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_gte::math::{Mat3I16, Vec3I32};
use psx_gte::scene::{self, Projected};
use psx_math::sincos;
use psx_pad::{button, poll_port1};
use psx_rt::tty;

use map::Map;
use vram::{TexSlot, EMPTY_SLOT};

// Cooked at build time from the user's own Half-Life install (git-ignored).
// `make cook MAP=c1a0` regenerates it.
static MAP_BYTES: &[u8] = include_bytes!("../../data/maps/c1a0.hlm");

const OT_LEN: usize = 1024;
const MAX_VERTS: usize = 8192; // c1a0 has 5203; headroom for other campaign maps
const MAX_PRIMS: usize = 12000; // worst case ~ all in-front triangles
const MAX_TEX_SLOTS: usize = 512; // c1a0 has 164 unique textures
const NEAR: u16 = 32; // drop triangles touching/behind the near plane
// ponytail: backface cull off for the first light-up so geometry is visible
// regardless of winding; flip on once orientation is confirmed from a capture.
const CULL: bool = false;

const H_PROJ: u16 = 160; // ~90° horizontal FOV at 320px wide

// Per-frame camera control rates. Angles are Q0.12 (4096 = one revolution) to
// match `sincos`; the GTE matrix builders take 256/rev, hence the `>> 4`.
const YAW_STEP: u16 = 48;
const PITCH_STEP: i16 = 32;
const PITCH_MAX: i16 = 1000; // ~88°
const MOVE: i32 = 48; // world units/frame (HL units ~ inches)
const VMOVE: i32 = 32;

static mut OT: OrderingTable<OT_LEN> = OrderingTable::new();
static mut SCRATCH: [Projected; MAX_VERTS] = [Projected { sx: 0, sy: 0, sz: 0 }; MAX_VERTS];
const EMPTY_TRI: TriTexturedGouraud = TriTexturedGouraud::new(
    [(0, 0), (0, 0), (0, 0)],
    [(0, 0), (0, 0), (0, 0)],
    [(128, 128, 128), (128, 128, 128), (128, 128, 128)],
    0,
    0,
);
static mut PRIMS: [TriTexturedGouraud; MAX_PRIMS] = [EMPTY_TRI; MAX_PRIMS];
static mut TEX_SLOTS: [TexSlot; MAX_TEX_SLOTS] = [EMPTY_SLOT; MAX_TEX_SLOTS];

/// World->view rotation: look = rotY(yaw)·rotX(pitch), with rows 0 and 1
/// negated to map world Y-up into the GPU's Y-down screen and keep winding.
/// (Same convention as the proven oot-psx camera.)
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

/// `(R·eye) >> 12` for one matrix row (1.3.12 matrix × world-unit eye).
fn dot12(row: [i16; 3], e: [i32; 3]) -> i32 {
    ((row[0] as i32 * e[0]) + (row[1] as i32 * e[1]) + (row[2] as i32 * e[2])) >> 12
}

#[no_mangle]
fn main() {
    tty::println("hl-psx: booting M1 map renderer");

    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    scene::set_screen_offset(160 << 16, 120 << 16);
    scene::set_projection_plane(H_PROJ);

    let m = Map::load(MAP_BYTES);
    let nv = if m.n_verts < MAX_VERTS { m.n_verts } else { MAX_VERTS };

    // Upload every cooked texture to VRAM once.
    let failed = unsafe { vram::upload_textures(&m, &mut TEX_SLOTS) };
    if failed > 0 {
        tty::println("hl-psx: some textures did not fit VRAM");
    }

    // Spawn at the map's centre, raised a little, looking along +yaw.
    let (mn, mx) = m.bounds();
    let mut eye = [(mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2 + 40, (mn[2] + mx[2]) / 2];
    let mut yaw: u16 = 0; // Q0.12
    let mut pitch: i16 = 0; // Q0.12

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
        // Forward = (sin yaw, 0, cos yaw) in the world XZ plane.
        let fx = sincos::sin_q12(yaw);
        let fz = sincos::sin_q12((yaw + 1024) & 0xFFF);
        if held.is_held(button::UP) {
            eye[0] += (fx * MOVE) >> 12;
            eye[2] += (fz * MOVE) >> 12;
        }
        if held.is_held(button::DOWN) {
            eye[0] -= (fx * MOVE) >> 12;
            eye[2] -= (fz * MOVE) >> 12;
        }
        if held.is_held(button::R1) {
            eye[1] += VMOVE;
        }
        if held.is_held(button::L1) {
            eye[1] -= VMOVE;
        }

        let rot = view_rotation(yaw, pitch);
        scene::load_rotation(&rot);
        let t = [-dot12(rot.m[0], eye), -dot12(rot.m[1], eye), -dot12(rot.m[2], eye)];
        scene::load_translation(Vec3I32::new(t[0], t[1], t[2]));

        unsafe {
            OT.clear();
            for i in 0..nv {
                SCRATCH[i] = scene::project_vertex(m.vert(i));
            }
            let mut np = 0usize;
            for tix in 0..m.n_tris {
                let (a, b, c) = m.tri_idx(tix);
                if a >= nv || b >= nv || c >= nv {
                    continue;
                }
                let slot = TEX_SLOTS[m.tri_tex(tix).min(MAX_TEX_SLOTS - 1)];
                if !slot.valid {
                    continue;
                }
                let (pa, pb, pc) = (SCRATCH[a], SCRATCH[b], SCRATCH[c]);
                if pa.sz < NEAR || pb.sz < NEAR || pc.sz < NEAR {
                    continue;
                }
                if CULL {
                    let area = (pb.sx as i32 - pa.sx as i32) * (pc.sy as i32 - pa.sy as i32)
                        - (pc.sx as i32 - pa.sx as i32) * (pb.sy as i32 - pa.sy as i32);
                    if area <= 0 {
                        continue;
                    }
                }
                let avgz = ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3;
                let mut otz = (avgz >> 6) as usize;
                if otz == 0 {
                    otz = 1;
                } else if otz >= OT_LEN {
                    otz = OT_LEN - 1;
                }
                if np >= MAX_PRIMS {
                    break;
                }
                let (sr, sg, sb) = m.tri_rgb(tix);
                PRIMS[np] = TriTexturedGouraud::with_material(
                    [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
                    m.tri_uv(tix),
                    [(sr, sg, sb), (sr, sg, sb), (sr, sg, sb)],
                    slot.material,
                );
                OT.add(otz, &mut PRIMS[np], TriTexturedGouraud::WORDS);
                np += 1;
            }

            fb.clear(0, 0, 0);
            OT.submit();
        }

        gpu::vsync();
        fb.swap();
    }
}

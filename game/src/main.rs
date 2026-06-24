//! hl-psx -- render a real Half-Life BSP map (cooked to `.hlm` by `tools/hl-bsp`)
//! with a GTE-projected, ordering-table-sorted player walking Black Mesa.
//!
//! Pipeline: M1 geometry, M2 textures (4-bit CLUT), M3/M7 per-vertex lightmap
//! shading, M4 PVS leaf culling, M5 player collision, M8 brush entities/doors.
//! Triangles project once into a per-frame cache (RTPT batched); those that
//! straddle the near plane are clipped + software-reprojected (render.rs) rather
//! than dropped.
//!
//! Controls (DualShock analog only): left stick = move/strafe, right stick =
//! look (X turn, Y pitch), Cross = jump.

#![no_std]
#![no_main]

extern crate psx_rt;

mod cdstream;
mod hud;
mod map;
mod menu;
mod model;
mod phys;
mod render;
mod telemetry;
mod vram;

use psx_gpu::material::TextureMaterial;
use psx_gpu::ot::OrderingTable;
use psx_gpu::prim::TriTexturedGouraud;
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_gte::math::{Mat3I16, Vec3I32};
use psx_gte::scene::{self, Projected};
use psx_pad::{button, enable_analog_port1, poll_port1};
use psx_rt::tty;

use map::Map;
use model::Model;
use psx_gpu::prim::QuadTexturedMaterial;
use vram::{TexSlot, EMPTY_SLOT};

// Maps stream from the disc's WORLD.PAK at runtime (no longer baked into the
// EXE). MAP_BUF holds one cooked `.hlm`; sized for the largest packed map
// (~900 KB). `make rooms` cooks the selectable maps; chunk id == room_<N>.
const MAP_WORDS: usize = 235_520; // 942,080 bytes
static mut MAP_BUF: [u32; MAP_WORDS] = [0; MAP_WORDS];
static SCI_BYTES: &[u8] = include_bytes!("../../data/models/scientist.hlmdl");
static WPN_BYTES: &[u8] = include_bytes!("../../data/models/v_9mmhandgun.hlmdl");

const OT_LEN: usize = 1024;
const WEAPON_OT_LEN: usize = 64;
const HUD_OT_LEN: usize = 1;
const MAX_VERTS: usize = 8192;
const MAX_MODEL_VERTS: usize = 1024;
const MAX_PRIMS: usize = 12000;
const MAX_TEX_SLOTS: usize = 512;
const MAX_FACES: usize = 8192;
const MAX_LEAVES: usize = 8192;
const MAX_ENTS: usize = 256;
const DOOR_SPEED: i32 = 120;
const NEAR: u16 = 2; // GTE depth: only verts at/behind the near plane take the soft-clip path
const SUBDIV_PX: i32 = 96; // split near-clipped triangles wider than this (affine fix)
const SUBDIV_DEPTH: u8 = 0; // ponytail: subdivision off (perf); affine warp accepted
const CULL: bool = true; // backface cull (keep area > 0; winding verified)
const H_PROJ: u16 = 160; // ~90 deg horizontal FOV at 320px

const PITCH_MAX: i16 = 1000;
const YAW_RATE: i32 = 130; // yaw units/frame at full stick (Q0.12)
const PITCH_RATE: i32 = 95; // pitch units/frame at full stick
const DEADZONE: i32 = 28; // radial stick deadzone
const VIEW_HEIGHT: i32 = 28;
const FAR_VIEW: i32 = 6000; // leaf cull distance (world units); generous to avoid pop
const MODEL_CULL: bool = false; // backface-cull studio models (flip if inside-out)
const MODEL_SHADE: u8 = 110; // flat model tint (dimmer than 128 to match the lit world)
const TRAM_STEP_DIV: i32 = 15; // tram units/sec -> units/frame (demo pace)

static mut OT: OrderingTable<OT_LEN> = OrderingTable::new();
static mut WEAPON_OT: OrderingTable<WEAPON_OT_LEN> = OrderingTable::new();
static mut HUD_OT: OrderingTable<HUD_OT_LEN> = OrderingTable::new();
const EMPTY_TRI: TriTexturedGouraud = TriTexturedGouraud::new(
    [(0, 0), (0, 0), (0, 0)],
    [(0, 0), (0, 0), (0, 0)],
    [(128, 128, 128), (128, 128, 128), (128, 128, 128)],
    0,
    0,
);
static mut PRIMS: [TriTexturedGouraud; MAX_PRIMS] = [EMPTY_TRI; MAX_PRIMS];
static mut HUD_PRIMS: [QuadTexturedMaterial; hud::DRAW_CAP] = [hud::EMPTY_QUAD; hud::DRAW_CAP];
static mut TEX_SLOTS: [TexSlot; MAX_TEX_SLOTS] = [EMPTY_SLOT; MAX_TEX_SLOTS];
static mut MODEL_SLOTS: [TexSlot; 16] = [EMPTY_SLOT; 16];
static mut WEAPON_SLOTS: [TexSlot; 12] = [EMPTY_SLOT; 12];
static mut SCRATCH: [Projected; MAX_VERTS] = [Projected {
    sx: 0,
    sy: 0,
    sz: 0,
}; MAX_VERTS];
static mut WEAPON_SCRATCH: [Projected; MAX_MODEL_VERTS] = [Projected {
    sx: 0,
    sy: 0,
    sz: 0,
}; MAX_MODEL_VERTS];
static mut WEAPON_CACHE_FRAME: usize = usize::MAX;
static mut WEAPON_CACHE_RECOIL: i32 = i32::MIN;
static mut WEAPON_CACHE_VERTS: usize = 0;
static mut VIS_BITS: [u8; MAX_LEAVES / 8] = [0; MAX_LEAVES / 8];
static mut FACE_FRAME: [u16; MAX_FACES] = [0; MAX_FACES];
static mut VERT_FRAME: [u16; MAX_VERTS] = [0; MAX_VERTS]; // project-once-per-frame cache marker
static mut ENT_PHASE: [i32; MAX_ENTS] = [0; MAX_ENTS];
static mut CLIP_CV: [render::CVert; 4] = [render::EMPTY_CV; 4]; // near-clip scratch (reused)

/// World->view rotation: rotX(pitch)*rotY(yaw) -- pitch in camera space so
/// looking up/down while turned doesn't roll the horizon. Rows 0/1 negated for
/// the GPU's Y-down screen.
fn view_rotation(yaw: u16, pitch: i16) -> Mat3I16 {
    let look = Mat3I16::rotate_x((pitch >> 4) as u16).mul(&Mat3I16::rotate_y((yaw >> 4) as u16));
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

/// Integer square root (for path-segment lengths). Verified by the tram ride
/// playing back at the right pace.
fn isqrt(n: i64) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut res = 0i64;
    let mut bit = 1i64 << 62;
    while bit > x {
        bit >>= 2;
    }
    while bit != 0 {
        if x >= res + bit {
            x -= res + bit;
            res = (res >> 1) + bit;
        } else {
            res >>= 1;
        }
        bit >>= 2;
    }
    res as i32
}

/// Length of a world-space segment.
#[inline]
fn seg_len(a: [i32; 3], b: [i32; 3]) -> i32 {
    let d = [
        (b[0] - a[0]) as i64,
        (b[1] - a[1]) as i64,
        (b[2] - a[2]) as i64,
    ];
    isqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).max(1)
}

/// Cull when the screen triangle isn't front-facing (area <= 0). Matches the
/// cook's reversed winding.
#[inline]
fn culled(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> bool {
    (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1) <= 0
}

#[inline]
fn sphere_visible(center: [i32; 3], radius: i32, rot: &Mat3I16, base_t: [i32; 3]) -> bool {
    let vz = dot12(rot.m[2], center) + base_t[2];
    if vz + radius < render::NEAR_Z || vz - radius > FAR_VIEW {
        return false;
    }
    let z = vz.max(render::NEAR_Z);
    let vx = dot12(rot.m[0], center) + base_t[0];
    if vx.abs() > z + radius * 2 {
        return false;
    }
    let vy = dot12(rot.m[1], center) + base_t[1];
    vy.abs() * 4 <= z * 3 + radius * 8
}

#[inline]
fn clamp_otz(z: usize) -> usize {
    z.clamp(1, OT_LEN - 1)
}

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
            return -next - 1;
        }
        idx = next;
    }
}

fn decompress_vis(m: &Map, visofs: i32, out: &mut [u8]) {
    let row = ((m.n_leaves.saturating_sub(1)) + 7) / 8;
    let row = row.min(out.len());
    for b in out[..row].iter_mut() {
        *b = 0;
    }
    if visofs < 0 {
        for b in out[..row].iter_mut() {
            *b = 0xFF;
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

/// Project vertex `i` into the cache once per frame (base view matrix).
#[inline]
unsafe fn proj_vert(m: &Map, i: usize, frame: u16) {
    if VERT_FRAME[i] != frame {
        SCRATCH[i] = scene::project_vertex(m.vert(i));
        VERT_FRAME[i] = frame;
    }
}

#[inline]
unsafe fn push_tri_to<const N: usize>(
    ot: &mut OrderingTable<N>,
    np: &mut usize,
    screen: [(i16, i16); 3],
    uv: [(u8, u8); 3],
    rgb: [(u8, u8, u8); 3],
    mat: TextureMaterial,
    otz: usize,
) {
    if *np >= MAX_PRIMS {
        return;
    }
    PRIMS[*np] = TriTexturedGouraud::with_material(screen, uv, rgb, mat);
    ot.add(otz, &mut PRIMS[*np], TriTexturedGouraud::WORDS);
    *np += 1;
}

#[inline]
unsafe fn push_tri(
    np: &mut usize,
    screen: [(i16, i16); 3],
    uv: [(u8, u8); 3],
    rgb: [(u8, u8, u8); 3],
    mat: TextureMaterial,
    otz: usize,
) {
    push_tri_to(&mut OT, np, screen, uv, rgb, mat, otz);
}

#[inline]
unsafe fn push_weapon_tri(
    np: &mut usize,
    screen: [(i16, i16); 3],
    uv: [(u8, u8); 3],
    rgb: [(u8, u8, u8); 3],
    mat: TextureMaterial,
    otz: usize,
) {
    push_tri_to(&mut WEAPON_OT, np, screen, uv, rgb, mat, otz);
}

/// Emit triangle `t` from its three projected screen verts `p`. Small in-front
/// triangles emit straight from the cache. Anything large (affine warp) or
/// near-straddling drops to the view-space path: near-clip, then recursively
/// split at view-space midpoints while it's big on screen, then guard-clip.
unsafe fn emit_projected(m: &Map, t: usize, p: [Projected; 3], nv: usize, np: &mut usize) {
    let (a, b, c) = m.tri_idx(t);
    if a >= nv || b >= nv || c >= nv {
        return;
    }
    let slot = TEX_SLOTS[m.tri_tex(t).min(MAX_TEX_SLOTS - 1)];
    if !slot.valid {
        return;
    }
    let uv = m.tri_uv(t);
    let rgb = m.tri_rgb(t);
    let (pa, pb, pc) = (p[0], p[1], p[2]);
    let clamped = |q: &Projected| q.sx <= -1023 || q.sx >= 1023 || q.sy <= -1023 || q.sy >= 1023;

    // Fast path: fully in front and on-screen -> emit straight from the cache.
    // (Affine warp on big near surfaces is accepted; the view-space path is for
    // near-plane straddlers only -- routing the whole scene through it tanked fps.)
    if pa.sz >= NEAR
        && pb.sz >= NEAR
        && pc.sz >= NEAR
        && !clamped(&pa)
        && !clamped(&pb)
        && !clamped(&pc)
    {
        let (sa, sb, sc) = (
            (pa.sx as i32, pa.sy as i32),
            (pb.sx as i32, pb.sy as i32),
            (pc.sx as i32, pc.sy as i32),
        );
        if CULL && culled(sa, sb, sc) {
            return;
        }
        let avgz = ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3;
        push_tri(
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            uv,
            rgb,
            slot.material,
            clamp_otz((avgz >> 6) as usize),
        );
        return;
    }
    if pa.sz == 0 && pb.sz == 0 && pc.sz == 0 {
        return; // entirely behind the camera
    }

    // View-space path (near-plane straddlers): rebuild, near-clip, emit.
    let cvv = |idx: usize, k: usize| {
        let v = scene::transform_vertex(m.vert(idx));
        render::CVert {
            v: [v.x, v.y, v.z],
            rgb: (rgb[k].0 as i32, rgb[k].1 as i32, rgb[k].2 as i32),
            uv: (uv[k].0 as i32, uv[k].1 as i32),
        }
    };
    let cv = [cvv(a, 0), cvv(b, 1), cvv(c, 2)];
    let n = render::near_clip(&cv, &mut CLIP_CV);
    if n < 3 {
        return;
    }
    for k in 1..n - 1 {
        emit_cv(
            &[CLIP_CV[0], CLIP_CV[k], CLIP_CV[k + 1]],
            SUBDIV_DEPTH,
            slot.material,
            np,
        );
    }
}

/// Recursively split a view-space triangle at its midpoints while it's larger
/// than SUBDIV_PX on screen (affine perspective correction), then guard-clip and
/// emit. ponytail: depth 1 (<=4 sub-tris per big tri); raise SUBDIV_DEPTH if warp
/// is still visible, at the cost of more triangles.
unsafe fn emit_cv(cv: &[render::CVert; 3], depth: u8, mat: TextureMaterial, np: &mut usize) {
    let pa = render::project_soft(&cv[0]);
    let pb = render::project_soft(&cv[1]);
    let pc = render::project_soft(&cv[2]);
    let spanx = pa.x.max(pb.x).max(pc.x) - pa.x.min(pb.x).min(pc.x);
    let spany = pa.y.max(pb.y).max(pc.y) - pa.y.min(pb.y).min(pc.y);
    if depth > 0 && (spanx > SUBDIV_PX || spany > SUBDIV_PX) {
        let ab = render::mid_cv(&cv[0], &cv[1]);
        let bc = render::mid_cv(&cv[1], &cv[2]);
        let ca = render::mid_cv(&cv[2], &cv[0]);
        emit_cv(&[cv[0], ab, ca], depth - 1, mat, np);
        emit_cv(&[ab, cv[1], bc], depth - 1, mat, np);
        emit_cv(&[ca, bc, cv[2]], depth - 1, mat, np);
        emit_cv(&[ab, bc, ca], depth - 1, mat, np);
        return;
    }
    if CULL && culled((pa.x, pa.y), (pb.x, pb.y), (pc.x, pc.y)) {
        return;
    }
    let cl = |x: i32| x.clamp(0, 255) as u8;
    // Common case: fully on-screen -> draw directly, no guard-clip buffer.
    if render::in_band(&pa) && render::in_band(&pb) && render::in_band(&pc) {
        let avgz = ((pa.z + pb.z + pc.z) / 3).max(1);
        push_tri(
            np,
            [
                (pa.x as i16, pa.y as i16),
                (pb.x as i16, pb.y as i16),
                (pc.x as i16, pc.y as i16),
            ],
            [
                (pa.uv.0 as u8, pa.uv.1 as u8),
                (pb.uv.0 as u8, pb.uv.1 as u8),
                (pc.uv.0 as u8, pc.uv.1 as u8),
            ],
            [
                (cl(pa.rgb.0), cl(pa.rgb.1), cl(pa.rgb.2)),
                (cl(pb.rgb.0), cl(pb.rgb.1), cl(pb.rgb.2)),
                (cl(pc.rgb.0), cl(pc.rgb.1), cl(pc.rgb.2)),
            ],
            mat,
            clamp_otz((avgz >> 4) as usize),
        );
        return;
    }
    // Off-screen span: guard-clip (rare).
    let mut g = [render::EMPTY_SV; 8];
    let gn = render::guard_clip(&[pa, pb, pc], 3, &mut g);
    if gn < 3 {
        return;
    }
    for j in 1..gn - 1 {
        let (s0, s1, s2) = (g[0], g[j], g[j + 1]);
        let avgz = ((s0.z + s1.z + s2.z) / 3).max(1);
        push_tri(
            np,
            [
                (s0.x as i16, s0.y as i16),
                (s1.x as i16, s1.y as i16),
                (s2.x as i16, s2.y as i16),
            ],
            [
                (s0.uv.0 as u8, s0.uv.1 as u8),
                (s1.uv.0 as u8, s1.uv.1 as u8),
                (s2.uv.0 as u8, s2.uv.1 as u8),
            ],
            [
                (cl(s0.rgb.0), cl(s0.rgb.1), cl(s0.rgb.2)),
                (cl(s1.rgb.0), cl(s1.rgb.1), cl(s1.rgb.2)),
                (cl(s2.rgb.0), cl(s2.rgb.1), cl(s2.rgb.2)),
            ],
            mat,
            clamp_otz((avgz >> 4) as usize),
        );
    }
}

/// Draw a model at world `pos`, rotated by `yaw` (Q0.12), at animation `frame`.
unsafe fn draw_model(
    md: &Model,
    slots: &[TexSlot],
    pos: [i32; 3],
    yaw: u16,
    frame: usize,
    eye: [i32; 3],
    rot: &Mat3I16,
    np: &mut usize,
) {
    // GTE rotation = view ∘ model-yaw; translation places the origin at `pos`.
    let mr = rot.mul(&Mat3I16::rotate_y((yaw >> 4) as u16));
    scene::load_rotation(&mr);
    let es = [eye[0] - pos[0], eye[1] - pos[1], eye[2] - pos[2]];
    let et = [
        -dot12(rot.m[0], es),
        -dot12(rot.m[1], es),
        -dot12(rot.m[2], es),
    ];
    scene::load_translation(Vec3I32::new(et[0], et[1], et[2]));
    for t in 0..md.n_tris {
        let slot = slots[md.tri_tex(t).min(slots.len() - 1)];
        if !slot.valid {
            continue;
        }
        let (a, b, c) = md.tri_idx(t);
        let p = scene::project_triangle(md.vert(frame, a), md.vert(frame, b), md.vert(frame, c));
        let (pa, pb, pc) = (p[0], p[1], p[2]);
        if pa.sz < NEAR || pb.sz < NEAR || pc.sz < NEAR {
            continue;
        }
        if MODEL_CULL
            && culled(
                (pa.sx as i32, pa.sy as i32),
                (pb.sx as i32, pb.sy as i32),
                (pc.sx as i32, pc.sy as i32),
            )
        {
            continue;
        }
        let avgz = ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3;
        push_tri(
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            md.tri_uv(t),
            [(MODEL_SHADE, MODEL_SHADE, MODEL_SHADE); 3],
            slot.material,
            clamp_otz((avgz >> 6) as usize),
        );
    }
}

// First-person viewmodel transform. GoldSrc attaches the model to the camera:
// view.cpp copies the camera angles to the viewmodel and uses the predicted
// view origin, while the MDL vertices carry the actual first-person placement.
// Keep that authored placement here; only map HL's local axes into PSX view
// space and scale to this port's cooked-world/projection units.
const VM_BASE: Mat3I16 = Mat3I16 {
    m: [[0, 0, -4096], [0, -4096, 0], [4096, 0, 0]],
};
const VM_SCALE: i32 = 5;
const VM_VIEW_SHIFT: [i32; 3] = [30, 30, 40]; // PS1 viewport fit: right, down, deeper
const VM_CULL: bool = true;
const VM_OT_Z0: u32 = 48;
const VM_OT_STEP: u32 = 2;
const VM_CULL_POS: bool = true; // winding sign that is the backface
const VM_TWO_SIDED_TEX: usize = 0; // GLOVED_sleeve: avoid punched gaps in the orange arm
const VM_SHADE: u8 = 255;
const VM_FRAME: usize = 0; // authored idle pose
const SHOW_VIEWMODEL: bool = true;
const ANIM_DIV: usize = 4; // game-frames per baked animation frame

/// Build the viewmodel's GTE matrix: authored HL viewmodel axes, scale baked in.
fn viewmodel_rot() -> Mat3I16 {
    let r = VM_BASE;
    let mut m = r;
    for i in 0..3 {
        for j in 0..3 {
            m.m[i][j] = (r.m[i][j] as i32 * VM_SCALE) as i16;
        }
    }
    m
}

#[inline]
fn viewmodel_otz(avgz: u32) -> usize {
    let rel = avgz.saturating_sub(VM_OT_Z0) / VM_OT_STEP;
    1 + (rel as usize).min(WEAPON_OT_LEN - 2)
}

/// Draw the held weapon in view space (attached to the camera), flat-shaded, on
/// top of the world. `recoil_y` is a small screen-space kick layered over the
/// source-authored origin.
unsafe fn draw_viewmodel(
    md: &Model,
    slots: &[TexSlot],
    frame: usize,
    recoil_y: i32,
    np: &mut usize,
) {
    let r = viewmodel_rot();
    scene::load_rotation(&r);
    scene::load_translation(Vec3I32::new(
        VM_VIEW_SHIFT[0],
        VM_VIEW_SHIFT[1] + recoil_y,
        VM_VIEW_SHIFT[2],
    ));
    let nv = md.n_verts.min(MAX_MODEL_VERTS);
    if WEAPON_CACHE_FRAME != frame || WEAPON_CACHE_RECOIL != recoil_y || WEAPON_CACHE_VERTS != nv {
        for i in 0..nv {
            WEAPON_SCRATCH[i] = scene::project_vertex(md.vert(frame, i));
        }
        WEAPON_CACHE_FRAME = frame;
        WEAPON_CACHE_RECOIL = recoil_y;
        WEAPON_CACHE_VERTS = nv;
    }
    // HMDL keeps texture groups in source order (sleeve/glove before gun).
    // It submits to a dedicated weapon OT, drawn after the world, so its own
    // depth can sort normally without room polygons cutting through it.
    for t in 0..md.n_tris {
        let tex_id = md.tri_tex(t);
        let slot = slots[tex_id.min(slots.len() - 1)];
        if !slot.valid {
            continue;
        }
        let (a, b, c) = md.tri_idx(t);
        if a >= nv || b >= nv || c >= nv {
            continue;
        }
        let (pa, pb, pc) = (WEAPON_SCRATCH[a], WEAPON_SCRATCH[b], WEAPON_SCRATCH[c]);
        if pa.sz < NEAR || pb.sz < NEAR || pc.sz < NEAR {
            continue;
        }
        // Backface cull (toggle/sign tunable while dialling in the transform).
        if VM_CULL && tex_id != VM_TWO_SIDED_TEX {
            let area = (pb.sx as i32 - pa.sx as i32) * (pc.sy as i32 - pa.sy as i32)
                - (pc.sx as i32 - pa.sx as i32) * (pb.sy as i32 - pa.sy as i32);
            if (area >= 0) == VM_CULL_POS {
                continue;
            }
        }
        let avgz = ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3;
        push_weapon_tri(
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            md.tri_uv(t),
            [(VM_SHADE, VM_SHADE, VM_SHADE); 3],
            slot.material,
            viewmodel_otz(avgz),
        );
    }
}

#[no_mangle]
fn main() {
    tty::println("hl-psx: booting renderer");

    gpu::init(VideoMode::Ntsc, Resolution::R320X240);
    let mut fb = FrameBuffer::new(320, 240);
    gpu::set_draw_area(0, 0, 319, 239);
    gpu::set_draw_offset(0, 0);
    scene::set_screen_offset(160 << 16, 120 << 16);
    scene::set_projection_plane(H_PROJ);
    let sci = Model::load(SCI_BYTES);

    // Boot flow: pick a map in the menu, stream + play it, return on Select.
    // (Analog is enabled inside play(); the menu runs on the digital pad.)
    loop {
        let sel = menu::run(&mut fb);
        play(&mut fb, &sci, sel as u32);
    }
}

/// Stream map `chunk_id` from WORLD.PAK, upload its textures, and run the
/// renderer + physics loop until the player presses Select (back to menu).
fn play(fb: &mut FrameBuffer, sci: &Model, chunk_id: u32) {
    let _ = enable_analog_port1();
    telemetry::frame_begin(0);
    telemetry::task_begin(telemetry::task::FIXED_UPDATE);
    telemetry::stage_begin(telemetry::stage::CD_WORLD_PACK_STREAM);
    let map_len = cdstream::load_chunk(chunk_id, unsafe { &mut MAP_BUF }).unwrap_or(0);
    telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
    telemetry::counter(telemetry::counter::CD_WORLD_PACK_CHUNKS, 1);
    telemetry::counter(telemetry::counter::CD_WORLD_PACK_BYTES, map_len as u32);
    telemetry::counter(
        telemetry::counter::CD_WORLD_PACK_SECTORS,
        ((map_len as u32) + 2047) / 2048,
    );
    telemetry::counter(
        telemetry::counter::CD_WORLD_PACK_STATUS,
        if map_len == 0 { 0 } else { 1 },
    );
    telemetry::task_end(telemetry::task::FIXED_UPDATE);
    if map_len == 0 {
        tty::println("hl-psx: WORLD.PAK stream failed");
        telemetry::debug_log("hl-psx: WORLD.PAK stream failed");
        return;
    }
    telemetry::debug_log("hl-psx: WORLD.PAK chunk loaded");
    let map_bytes = unsafe { core::slice::from_raw_parts(MAP_BUF.as_ptr() as *const u8, map_len) };
    let m = Map::load(map_bytes);
    let nv = if m.n_verts < MAX_VERTS {
        m.n_verts
    } else {
        MAX_VERTS
    };

    telemetry::stage_begin(telemetry::stage::VRAM_UPLOAD);
    let _ = unsafe { vram::upload_textures(&m, &mut TEX_SLOTS) };
    let wpn = Model::load(WPN_BYTES);
    unsafe {
        vram::upload_tex_blob(sci.tex_blob(), sci.n_texs, &mut MODEL_SLOTS);
        vram::upload_tex_blob(wpn.tex_blob(), wpn.n_texs, &mut WEAPON_SLOTS);
    }
    let hud_mat = hud::upload(); // real HUD sprite sheet -> free gameplay tpage
    telemetry::stage_end(telemetry::stage::VRAM_UPLOAD);

    let mut player = phys::Player::new(m.spawn_pos);
    let mut yaw: u16 = (m.spawn_yaw as u16) & 0xFFF;
    let mut pitch: i16 = 0;
    let mut frame_no: u16 = 0;
    let mut telemetry_frame: u32 = 1;

    // Tram ride: carry the player along the path_track chain, then hand back
    // control. ride_off is the tram's displacement from its parked start.
    let wp0 = if m.n_way > 0 {
        m.waypoint(0)
    } else {
        [0, 0, 0]
    };
    let tram_step = (m.tram_speed / TRAM_STEP_DIV).max(3);
    // Tram cinematic OFF by default: it locks the player on rails (no walking),
    // which read as "movement broken" on c0a0. Re-enable once walk-on-platform
    // works. Flip TRAM_RIDE to restore the on-rails intro.
    const TRAM_RIDE: bool = false;
    let mut riding = TRAM_RIDE && m.n_way >= 2;
    let mut recoil = 0i32; // viewmodel kick when firing
    let mut seg = 0usize;
    let mut seg_dist = 0i32;
    let mut ride_off = [0i32; 3];

    loop {
        telemetry::frame_begin(telemetry_frame);
        telemetry::task_begin(telemetry::task::FIXED_UPDATE);
        telemetry::stage_begin(telemetry::stage::UPDATE);
        // Modern twin-stick FPS: left stick moves/strafes, right stick looks
        // (X = turn, Y = pitch), Cross = jump. Analog only.
        let pad = poll_port1();
        if pad.buttons.is_held(button::SELECT) {
            telemetry::stage_end(telemetry::stage::UPDATE);
            telemetry::task_end(telemetry::task::FIXED_UPDATE);
            return; // back to the map menu
        }
        // Analog is required: if the pad ever isn't in analog mode, re-assert it.
        if !pad.is_analog() {
            let _ = enable_analog_port1();
        }
        // R2 fires: kick the viewmodel; re-kicks while held (rapid fire feel).
        recoil = (recoil - 3).max(0);
        if pad.buttons.is_held(button::R2) && recoil == 0 {
            recoil = 16;
        }
        let (mut fwd, mut strafe, mut turn, mut look) = (0i32, 0i32, 0i32, 0i32);
        if pad.is_analog() {
            let (lx, ly) = pad.sticks.left_centered();
            let (rx, ry) = pad.sticks.right_centered();
            let dz2 = DEADZONE * DEADZONE;
            // Radial deadzone per stick (avoids axis drift / diagonal bias).
            if (lx as i32) * (lx as i32) + (ly as i32) * (ly as i32) > dz2 {
                fwd = -(ly as i32); // stick up = forward
                strafe = lx as i32;
            }
            if (rx as i32) * (rx as i32) + (ry as i32) * (ry as i32) > dz2 {
                turn = rx as i32;
                look = -(ry as i32); // stick up = look up
            }
        }
        yaw = (((yaw as i32) + (turn * YAW_RATE) / 128) & 0xFFF) as u16;
        pitch = (pitch + ((look * PITCH_RATE) / 128) as i16).clamp(-PITCH_MAX, PITCH_MAX);

        // Collision movers: every brush entity at its current offset (doors at
        // their open amount, statics at origin) + the tram at its ride offset.
        let mut movers = [phys::NO_MOVER; MAX_ENTS + 1];
        let mut nmov = 0;
        unsafe {
            for ei in 0..m.n_ents.min(MAX_ENTS) {
                let e = m.entity(ei);
                let off = if e.kind == 1 {
                    let ph = ENT_PHASE[ei] as i64;
                    [
                        ((e.mv[0] as i64 * ph) >> 12) as i32,
                        ((e.mv[1] as i64 * ph) >> 12) as i32,
                        ((e.mv[2] as i64 * ph) >> 12) as i32,
                    ]
                } else {
                    e.origin
                };
                movers[nmov] = phys::Mover { head: e.head, off };
                nmov += 1;
            }
            if m.tram_submodel > 0 && nmov < movers.len() {
                let toff = [
                    ride_off[0] + m.tram_base[0],
                    ride_off[1] + m.tram_base[1],
                    ride_off[2] + m.tram_base[2],
                ];
                movers[nmov] = phys::Mover {
                    head: m.tram_head,
                    off: toff,
                };
                nmov += 1;
            }
        }
        let movers = &movers[..nmov];

        if riding {
            // Advance along the path, possibly crossing several waypoints.
            let mut rem = tram_step;
            while rem > 0 && seg + 1 < m.n_way {
                let len = seg_len(m.waypoint(seg), m.waypoint(seg + 1));
                if seg_dist + rem >= len {
                    rem -= len - seg_dist;
                    seg += 1;
                    seg_dist = 0;
                } else {
                    seg_dist += rem;
                    rem = 0;
                }
            }
            let pos = if seg + 1 < m.n_way {
                let a = m.waypoint(seg);
                let b = m.waypoint(seg + 1);
                let len = seg_len(a, b);
                let f = (seg_dist * 4096 / len).clamp(0, 4096);
                [
                    a[0] + ((b[0] - a[0]) * f >> 12),
                    a[1] + ((b[1] - a[1]) * f >> 12),
                    a[2] + ((b[2] - a[2]) * f >> 12),
                ]
            } else {
                riding = false; // reached the end of the line
                m.waypoint(m.n_way - 1)
            };
            ride_off = [pos[0] - wp0[0], pos[1] - wp0[1], pos[2] - wp0[2]];
            // Locked to the tram (walking on a moving platform desyncs gravity).
            player.pos = [
                m.spawn_pos[0] + ride_off[0],
                m.spawn_pos[1] + ride_off[1],
                m.spawn_pos[2] + ride_off[2],
            ];
            player.vel = [0, 0, 0];
        } else {
            // On foot: full physics, colliding with the world + brush movers.
            player.update(
                &m,
                movers,
                fwd,
                strafe,
                pad.buttons.is_held(button::CROSS),
                yaw,
            );
        }
        let eye = [player.pos[0], player.pos[1] + VIEW_HEIGHT, player.pos[2]];

        let rot = view_rotation(yaw, pitch);
        scene::load_rotation(&rot);
        let base_t = [
            -dot12(rot.m[0], eye),
            -dot12(rot.m[1], eye),
            -dot12(rot.m[2], eye),
        ];
        scene::load_translation(Vec3I32::new(base_t[0], base_t[1], base_t[2]));

        telemetry::stage_end(telemetry::stage::UPDATE);
        telemetry::counter(telemetry::counter::SIM_TICKS, 1);
        telemetry::task_end(telemetry::task::FIXED_UPDATE);

        frame_no = frame_no.wrapping_add(1);
        telemetry::task_begin(telemetry::task::VISUAL_RENDER);
        telemetry::stage_begin(telemetry::stage::RENDER);

        unsafe {
            if frame_no == 0 {
                for f in FACE_FRAME.iter_mut() {
                    *f = 0;
                }
                for f in VERT_FRAME.iter_mut() {
                    *f = 0;
                }
                frame_no = 1;
            }
            OT.clear();
            WEAPON_OT.clear();
            HUD_OT.clear();
            let mut np = 0usize;
            let mut room_cells_drawn = 0u32;
            let mut room_surfaces_considered = 0u32;

            // World (model 0) via PVS, drawing each visible leaf's faces once.
            // Vertices are projected lazily (only those actually drawn).
            telemetry::stage_begin(telemetry::stage::ROOM);
            let cam_leaf = camera_leaf(&m, eye);
            if cam_leaf > 0 && (cam_leaf as usize) < m.n_leaves {
                let (visofs, _, _) = m.leaf(cam_leaf as usize);
                decompress_vis(&m, visofs, &mut VIS_BITS);
                for i in 0..m.n_leaves.saturating_sub(1) {
                    if VIS_BITS[i >> 3] & (1 << (i & 7)) == 0 {
                        continue;
                    }
                    // Frustum cull the leaf's bounding sphere: behind the near
                    // plane, beyond the far distance, or outside the ~45deg
                    // horizontal FOV (conservative 2*r slack -> never culls a
                    // visible leaf). H_PROJ=160 over a 160px half-width = 45deg.
                    let (lc, lr) = m.leaf_bounds(i + 1);
                    let vz = dot12(rot.m[2], lc) + base_t[2];
                    if vz + lr < render::NEAR_Z || vz - lr > FAR_VIEW {
                        continue;
                    }
                    let vx = dot12(rot.m[0], lc) + base_t[0];
                    if vx.abs() > vz + lr * 2 {
                        continue;
                    }
                    // Vertical FOV ~37deg (120px half-height / H_PROJ 160): cull
                    // if |vy| > 0.75*vz, with 2*r slack -> 4|vy| > 3vz + 8r.
                    let vy = dot12(rot.m[1], lc) + base_t[1];
                    if vy.abs() * 4 > vz * 3 + lr * 8 {
                        continue;
                    }
                    room_cells_drawn = room_cells_drawn.saturating_add(1);
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
                        room_surfaces_considered = room_surfaces_considered.saturating_add(1);
                        // Backface cull the whole face before touching its tris.
                        let (fnrm, fd) = m.face_plane(face);
                        if dot12(fnrm, eye) <= fd {
                            continue;
                        }
                        let (first, cnt) = m.face_tris(face);
                        for tt in first..first + cnt {
                            if tt >= m.n_tris {
                                continue;
                            }
                            let (a, b, c) = m.tri_idx(tt);
                            if a < nv && b < nv && c < nv {
                                proj_vert(&m, a, frame_no);
                                proj_vert(&m, b, frame_no);
                                proj_vert(&m, c, frame_no);
                                emit_projected(
                                    &m,
                                    tt,
                                    [SCRATCH[a], SCRATCH[b], SCRATCH[c]],
                                    nv,
                                    &mut np,
                                );
                            }
                        }
                    }
                }
            } else {
                for tt in 0..m.n_tris {
                    let (a, b, c) = m.tri_idx(tt);
                    if a < nv && b < nv && c < nv {
                        proj_vert(&m, a, frame_no);
                        proj_vert(&m, b, frame_no);
                        proj_vert(&m, c, frame_no);
                        emit_projected(&m, tt, [SCRATCH[a], SCRATCH[b], SCRATCH[c]], nv, &mut np);
                    }
                }
            }
            telemetry::stage_end(telemetry::stage::ROOM);
            telemetry::counter(telemetry::counter::ROOM_CELLS_DRAWN, room_cells_drawn);
            telemetry::counter(
                telemetry::counter::ROOM_SURFACES_CONSIDERED,
                room_surfaces_considered,
            );

            telemetry::stage_begin(telemetry::stage::MODEL_INSTANCES);
            let model_prims0 = np;
            let mut model_draws = 0u32;
            let mut model_bounds_tests = 0u32;
            let mut model_bounds_culled = 0u32;
            let mut model_culled_tris = 0u32;

            // Brush entities: doors slide open near the player. Each renders with
            // a per-entity GTE translation (base view shifted by the offset);
            // its few tris are projected fresh (not from the world cache).
            for ei in 0..m.n_ents.min(MAX_ENTS) {
                let e = m.entity(ei);
                let off = if e.kind == 1 {
                    let dx = (player.pos[0] - e.center[0]) as i64;
                    let dy = (player.pos[1] - e.center[1]) as i64;
                    let dz = (player.pos[2] - e.center[2]) as i64;
                    let near = dx * dx + dy * dy + dz * dz < e.r2 as i64;
                    let ph = &mut ENT_PHASE[ei];
                    *ph = if near {
                        (*ph + DOOR_SPEED).min(4096)
                    } else {
                        (*ph - DOOR_SPEED).max(0)
                    };
                    [
                        ((e.mv[0] as i64 * *ph as i64) >> 12) as i32,
                        ((e.mv[1] as i64 * *ph as i64) >> 12) as i32,
                        ((e.mv[2] as i64 * *ph as i64) >> 12) as i32,
                    ]
                } else {
                    e.origin
                };
                if e.r2 > 0 {
                    model_bounds_tests = model_bounds_tests.saturating_add(1);
                    let radius = isqrt(e.r2 as i64);
                    let center = [
                        e.center[0] + off[0],
                        e.center[1] + off[1],
                        e.center[2] + off[2],
                    ];
                    if !sphere_visible(center, radius, &rot, base_t) {
                        model_bounds_culled = model_bounds_culled.saturating_add(1);
                        continue;
                    }
                }
                model_draws = model_draws.saturating_add(1);
                let es = [eye[0] - off[0], eye[1] - off[1], eye[2] - off[2]];
                let et = [
                    -dot12(rot.m[0], es),
                    -dot12(rot.m[1], es),
                    -dot12(rot.m[2], es),
                ];
                scene::load_translation(Vec3I32::new(et[0], et[1], et[2]));
                let (ff, nf) = m.submodel(e.submodel);
                for f in ff..ff + nf {
                    let (first, cnt) = m.face_tris(f);
                    let (fnrm, fd) = m.face_plane(f);
                    if dot12(fnrm, es) <= fd {
                        model_culled_tris = model_culled_tris.saturating_add(cnt as u32);
                        continue;
                    }
                    for tt in first..first + cnt {
                        if tt >= m.n_tris {
                            continue;
                        }
                        let (a, b, c) = m.tri_idx(tt);
                        if a < nv && b < nv && c < nv {
                            let p = scene::project_triangle(m.vert(a), m.vert(b), m.vert(c));
                            emit_projected(&m, tt, p, nv, &mut np);
                        }
                    }
                }
            }

            // Tram car: render its submodel at the current ride offset.
            if m.tram_submodel > 0 && m.tram_submodel < m.n_models {
                model_draws = model_draws.saturating_add(1);
                let toff = [
                    ride_off[0] + m.tram_base[0],
                    ride_off[1] + m.tram_base[1],
                    ride_off[2] + m.tram_base[2],
                ];
                let es = [eye[0] - toff[0], eye[1] - toff[1], eye[2] - toff[2]];
                let et = [
                    -dot12(rot.m[0], es),
                    -dot12(rot.m[1], es),
                    -dot12(rot.m[2], es),
                ];
                scene::load_translation(Vec3I32::new(et[0], et[1], et[2]));
                let (ff, nf) = m.submodel(m.tram_submodel);
                for f in ff..ff + nf {
                    let (first, cnt) = m.face_tris(f);
                    let (fnrm, fd) = m.face_plane(f);
                    if dot12(fnrm, es) <= fd {
                        model_culled_tris = model_culled_tris.saturating_add(cnt as u32);
                        continue;
                    }
                    for tt in first..first + cnt {
                        if tt >= m.n_tris {
                            continue;
                        }
                        let (a, b, c) = m.tri_idx(tt);
                        if a < nv && b < nv && c < nv {
                            let p = scene::project_triangle(m.vert(a), m.vert(b), m.vert(c));
                            emit_projected(&m, tt, p, nv, &mut np);
                        }
                    }
                }
            }

            // Studio models placed at point entities (scientists). Frustum-cull
            // by the prop's forward depth + horizontal FOV so off-screen ones
            // don't burn primitives.
            for pi in 0..m.n_props {
                let (ty, org, yaw) = m.prop(pi);
                if ty != 0 {
                    continue; // only scientists included for now
                }
                model_bounds_tests = model_bounds_tests.saturating_add(1);
                let vz = dot12(rot.m[2], org) + base_t[2];
                if vz < render::NEAR_Z - 72 || vz > FAR_VIEW {
                    model_bounds_culled = model_bounds_culled.saturating_add(1);
                    continue;
                }
                let vx = dot12(rot.m[0], org) + base_t[0];
                if vx.abs() > vz + 128 {
                    model_bounds_culled = model_bounds_culled.saturating_add(1);
                    continue;
                }
                model_draws = model_draws.saturating_add(1);
                let sf = (frame_no as usize / ANIM_DIV) % sci.n_frames.max(1);
                draw_model(&sci, &MODEL_SLOTS, org, yaw as u16, sf, eye, &rot, &mut np);
            }
            telemetry::stage_end(telemetry::stage::MODEL_INSTANCES);
            telemetry::counter(telemetry::counter::MODEL_INSTANCE_DRAWS, model_draws);
            telemetry::counter(
                telemetry::counter::MODEL_INSTANCE_BOUNDS_TESTS,
                model_bounds_tests,
            );
            telemetry::counter(
                telemetry::counter::MODEL_INSTANCE_BOUNDS_CULLED,
                model_bounds_culled,
            );
            telemetry::counter(
                telemetry::counter::MODEL_INSTANCE_CULLED_TRIS,
                model_culled_tris,
            );
            telemetry::counter(
                telemetry::counter::MODEL_INSTANCE_SUBMITTED_TRIS,
                np.saturating_sub(model_prims0) as u32,
            );

            let world_prims = np;
            if SHOW_VIEWMODEL {
                telemetry::stage_begin(telemetry::stage::EQUIPMENT);
                draw_viewmodel(&wpn, &WEAPON_SLOTS, VM_FRAME, -recoil, &mut np);
                telemetry::stage_end(telemetry::stage::EQUIPMENT);
                telemetry::counter(
                    telemetry::counter::EQUIPMENT_SUBMITTED_TRIS,
                    np.saturating_sub(world_prims) as u32,
                );
            }
            let _ = hud::draw(hud_mat, 100, 17, &mut HUD_OT, &mut HUD_PRIMS);

            telemetry::stage_begin(telemetry::stage::FRAME_CLEAR);
            fb.clear(0, 0, 0);
            telemetry::stage_end(telemetry::stage::FRAME_CLEAR);
            telemetry::stage_begin(telemetry::stage::WORLD_FLUSH);
            OT.submit();
            telemetry::stage_end(telemetry::stage::WORLD_FLUSH);
            telemetry::stage_begin(telemetry::stage::OT_SUBMIT);
            WEAPON_OT.submit();
            HUD_OT.submit();
            telemetry::stage_end(telemetry::stage::OT_SUBMIT);
            telemetry::counter(telemetry::counter::TRI_PRIMITIVES, np as u32);
            telemetry::counter(telemetry::counter::WORLD_COMMANDS, world_prims as u32);
        }

        telemetry::stage_end(telemetry::stage::RENDER);
        telemetry::stage_begin(telemetry::stage::PRESENT);
        gpu::vsync();
        fb.swap();
        telemetry::stage_end(telemetry::stage::PRESENT);
        telemetry::counter(telemetry::counter::VISUAL_FRAMES, 1);
        telemetry::task_end(telemetry::task::VISUAL_RENDER);
        telemetry_frame = telemetry_frame.wrapping_add(1);
    }
}

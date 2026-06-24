//! hl-psx -- render a real Half-Life BSP map (cooked to `.hlm` by `tools/hl-bsp`)
//! with a GTE-projected, ordering-table-sorted player walking Black Mesa.
//!
//! Pipeline: M1 geometry, M2 textures (4-bit CLUT), M3/M7 per-vertex lightmap
//! shading, M4 PVS leaf culling, M5 player collision, M8 brush entities/doors.
//! Vertices project once into per-frame/draw caches; triangles that straddle the
//! near plane are clipped + software-reprojected (render.rs) rather
//! than dropped.
//!
//! Controls (DualShock analog only): left stick = move/strafe, right stick =
//! look (X turn, Y pitch), Cross = jump, R2 = fire.

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

mod room_budget {
    include!(concat!(env!("OUT_DIR"), "/room_budget.rs"));
}

use psx_gpu::material::TexturedGouraudPacketMaterial;
use psx_gpu::ot::OrderingTable;
use psx_gpu::prim::{QuadTexturedGouraud, TriTexturedGouraud};
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_gte::math::{Mat3I16, Vec3I32};
use psx_gte::scene::{self, Projected};
use psx_pad::{button, enable_analog_port1, poll_port1};
use psx_rt::{interrupts, tty};

use map::Map;
use model::{Model, RenderFace as ModelRenderFace};
use psx_engine::{PrimitivePacketArena, PrimitivePacketScratch, PrimitiveSink};
use psx_gpu::prim::QuadTexturedMaterial;
use vram::{TexSlot, EMPTY_SLOT};

// Maps stream from the disc's WORLD.PAK at runtime (no longer baked into the
// EXE). MAP_BUF holds either one temporary texture chunk or one resident map
// chunk; build.rs sizes it from data/rooms. `make rooms` cooks menu room N as
// room_<2N>.psxc (resident HLMA) and room_<2N+1>.psxc (temporary HLTX textures).
const MAP_WORDS: usize = room_budget::MAP_WORDS;
const MODEL_WORDS: usize = room_budget::MODEL_WORDS;
static mut MAP_BUF: [u32; MAP_WORDS] = [0; MAP_WORDS];
static mut MODEL_BUF: [u32; MODEL_WORDS] = [0; MODEL_WORDS];
static SCI_BYTES: &[u8] = include_bytes!("../../data/models/scientist.hlmdl");
static BARNEY_BYTES: &[u8] = include_bytes!("../../data/models/barney.hlmdl");
static HEADCRAB_BYTES: &[u8] = include_bytes!("../../data/models/headcrab.hlmdl");

const OT_LEN: usize = 1024;
const WEAPON_OT_LEN: usize = 64;
const HUD_OT_LEN: usize = 1;
const MAX_VERTS: usize = 8192;
const MAX_MODEL_VERTS: usize = 1024;
const SCI_FACE_CAP: usize = 768;
const BARNEY_FACE_CAP: usize = 800;
const HEADCRAB_FACE_CAP: usize = 512;
const MAX_RENDER_PACKETS: usize = 3328;
const MAX_WEAPON_CACHE_TRIS: usize = 320;
const MAX_TEX_SLOTS: usize = room_budget::MAX_TEX_SLOTS;
const MAX_WORLD_TRIS: usize = room_budget::MAX_TRIS;
const MAX_FACES: usize = room_budget::MAX_FACES;
const MAX_LEAVES: usize = room_budget::MAX_LEAVES;
const MAX_ENTS: usize = room_budget::MAX_ENTS;
const MAX_PROPS: usize = 128;
const DOOR_SPEED: i32 = 120;
const NEAR: u16 = 2; // GTE depth: only verts at/behind the near plane take the soft-clip path
const SUBDIV_PX: i32 = 96; // split near-clipped triangles wider than this (affine fix)
const SUBDIV_DEPTH: u8 = 0; // ponytail: subdivision off (perf); affine warp accepted
const CULL: bool = true; // backface cull (keep area > 0; winding verified)
const H_PROJ: u16 = 160; // ~90 deg horizontal FOV at 320px
const WORLD_QUAD_PAIRING: bool = true; // pair two-triangle BSP quads when safe
const WORLD_BOUNDS_CULL: bool = true; // face AABB frustum test before projection
const WORLD_BOUNDS_PAD: i32 = 256; // guard band for rotated face AABBs near screen edges

const PITCH_MAX: i16 = 1000;
const YAW_RATE: i32 = 130; // yaw units/frame at full stick (Q0.12)
const PITCH_RATE: i32 = 95; // pitch units/frame at full stick
const DEADZONE: i32 = 28; // radial stick deadzone
const VIEW_HEIGHT: i32 = 28;
const FAR_VIEW: i32 = 6000; // leaf cull distance (world units); generous to avoid pop
const MODEL_CULL: bool = true; // backface-cull studio models
const MODEL_SHADE: u8 = 110; // flat model tint (dimmer than 128 to match the lit world)
const MODEL_HIT_SHADE: u8 = 180; // brief flash when the player lands a shot
const TRAM_STEP_DIV: i32 = 15; // tram units/sec -> units/frame (demo pace)
const SIM_VBLANKS: u32 = 3; // 60 Hz NTSC / 3 = 20 Hz gameplay tick
const ROOM_WORLD_CHUNK_MUL: u32 = 2;
const ROOM_TEXTURE_CHUNK_ADD: u32 = 1;
const MODEL_CHUNK_V_9MMHANDGUN: u32 = 1000;
const GLOCK_MAX_CLIP: u16 = 17;
const GLOCK_DAMAGE: u8 = 8;
const GLOCK_RANGE: i32 = 8192;
const GLOCK_AIM_PIX_X: i32 = 22;
const GLOCK_AIM_PIX_Y: i32 = 34;
const PROP_TARGET_HEIGHT: i32 = 40;
const PROP_TYPE_SCIENTIST: u8 = 0;
const PROP_TYPE_BARNEY: u8 = 1;
const PROP_TYPE_HEADCRAB: u8 = 2;
const PROP_STATE_IDLE: u8 = 0;
const PROP_STATE_MOVE: u8 = 1;
const PROP_STATE_ATTACK: u8 = 2;
const PROP_STATE_DEAD: u8 = 3;
const PROP_CLIP_IDLE: usize = 0;
const PROP_CLIP_MOVE: usize = 1;
const PROP_CLIP_ATTACK: usize = 2;
const PROP_CLIP_DEAD: usize = 3;
const PLAYER_START_HEALTH: u16 = 100;
const SCIENTIST_HEALTH: u8 = 24;
const BARNEY_HEALTH: u8 = 35;
const HEADCRAB_HEALTH: u8 = 16;
const HEADCRAB_SPEED: i32 = 5;
const HEADCRAB_WAKE_RANGE2: i32 = 1300 * 1300;
const HEADCRAB_STOP_RANGE: i32 = 34;
const HEADCRAB_ATTACK_RANGE2: i32 = HEADCRAB_STOP_RANGE * HEADCRAB_STOP_RANGE;
const HEADCRAB_ATTACK_DAMAGE: u16 = 6;
const HEADCRAB_ATTACK_COOLDOWN: u8 = 18;
const HEADCRAB_TARGET_HEIGHT: i32 = 12;
const PROP_HIT_FLASH_TICKS: u8 = 4;

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
static mut PRIMITIVE_PACKETS: PrimitivePacketScratch<MAX_RENDER_PACKETS> =
    PrimitivePacketScratch::ZERO;
static mut HUD_PRIMS: [QuadTexturedMaterial; hud::DRAW_CAP] = [hud::EMPTY_QUAD; hud::DRAW_CAP];
static mut TEX_SLOTS: [TexSlot; MAX_TEX_SLOTS] = [EMPTY_SLOT; MAX_TEX_SLOTS];
static mut SCI_SLOTS: [TexSlot; 24] = [EMPTY_SLOT; 24];
static mut BARNEY_SLOTS: [TexSlot; 24] = [EMPTY_SLOT; 24];
static mut HEADCRAB_SLOTS: [TexSlot; 8] = [EMPTY_SLOT; 8];
static mut WEAPON_SLOTS: [TexSlot; 12] = [EMPTY_SLOT; 12];
static mut SCI_FACES: [ModelRenderFace; SCI_FACE_CAP] = [ModelRenderFace::ZERO; SCI_FACE_CAP];
static mut BARNEY_FACES: [ModelRenderFace; BARNEY_FACE_CAP] =
    [ModelRenderFace::ZERO; BARNEY_FACE_CAP];
static mut HEADCRAB_FACES: [ModelRenderFace; HEADCRAB_FACE_CAP] =
    [ModelRenderFace::ZERO; HEADCRAB_FACE_CAP];
static mut SCI_FACE_COUNT: usize = 0;
static mut BARNEY_FACE_COUNT: usize = 0;
static mut HEADCRAB_FACE_COUNT: usize = 0;
static mut WORLD_UV_WORDS: [[u16; 3]; MAX_WORLD_TRIS] = [[0; 3]; MAX_WORLD_TRIS];
static mut WORLD_UV_COUNT: usize = 0;
const EMPTY_PROJECTED: Projected = Projected {
    sx: 0,
    sy: 0,
    sz: 0,
};
static mut SCRATCH: [Projected; MAX_VERTS] = [EMPTY_PROJECTED; MAX_VERTS];
static mut MODEL_SCRATCH: [Projected; MAX_MODEL_VERTS] = [EMPTY_PROJECTED; MAX_MODEL_VERTS];
static mut WEAPON_CACHE_FRAME: usize = usize::MAX;
static mut WEAPON_CACHE_RECOIL: i32 = i32::MIN;
static mut WEAPON_CACHE_VERTS: usize = 0;
static mut WEAPON_TRI_CACHE: [TriTexturedGouraud; MAX_WEAPON_CACHE_TRIS] =
    [EMPTY_TRI; MAX_WEAPON_CACHE_TRIS];
static mut WEAPON_TRI_OTZ: [u8; MAX_WEAPON_CACHE_TRIS] = [0; MAX_WEAPON_CACHE_TRIS];
static mut WEAPON_TRI_COUNT: usize = 0;
static mut SUBMODEL_VERT_TOKEN: [u16; MAX_VERTS] = [0; MAX_VERTS];
static mut SUBMODEL_DRAW_TOKEN: u16 = 1;
const PVS_LINK_END: u16 = u16::MAX;
static mut VIS_BITS: [u8; MAX_LEAVES / 8] = [0; MAX_LEAVES / 8];
static mut PVS_LEAF_COUNT: usize = 0;
static mut PVS_FACE_FIRST: [u16; MAX_FACES] = [0; MAX_FACES];
static mut PVS_FACE_TRI_COUNT: [u16; MAX_FACES] = [0; MAX_FACES];
static mut PVS_FACE_CENTER: [[i16; 3]; MAX_FACES] = [[0; 3]; MAX_FACES];
static mut PVS_FACE_EXTENT: [[u16; 3]; MAX_FACES] = [[0; 3]; MAX_FACES];
static mut PVS_FACE_NEXT: [u16; MAX_FACES] = [PVS_LINK_END; MAX_FACES];
static mut PVS_FACE_COUNT: usize = 0;
static mut PVS_FACE_MARK: [u16; MAX_FACES] = [0; MAX_FACES];
static mut PVS_FACE_MARK_TOKEN: u16 = 1;
static mut PVS_GROUP_FIRST: [u16; MAX_FACES] = [PVS_LINK_END; MAX_FACES];
static mut PVS_GROUP_MARK: [u16; MAX_FACES] = [0; MAX_FACES];
static mut PVS_GROUP_ACTIVE: [u16; MAX_FACES] = [0; MAX_FACES];
static mut PVS_GROUP_NRM: [[i16; 3]; MAX_FACES] = [[0; 3]; MAX_FACES];
static mut PVS_GROUP_DIST: [i32; MAX_FACES] = [0; MAX_FACES];
static mut PVS_GROUP_COUNT: usize = 0;
static mut PVS_TRI_REF_COUNT: usize = 0;
static mut PVS_ENTS: [u16; MAX_ENTS] = [0; MAX_ENTS];
static mut PVS_ENT_COUNT: usize = 0;
static mut PVS_CAM_LEAF: i32 = -1;
static mut DRAW_FACE_MARK: [u16; MAX_FACES] = [0; MAX_FACES];
static mut DRAW_FACE_MARK_TOKEN: u16 = 1;
static mut VERT_FRAME: [u16; MAX_VERTS] = [0; MAX_VERTS]; // project-once-per-frame cache marker
const EMPTY_ENT: map::Ent = map::Ent {
    submodel: 0,
    kind: 2,
    origin: [0, 0, 0],
    mv: [0, 0, 0],
    center: [0, 0, 0],
    r2: 0,
    head: 0,
    leaf_start: 0,
    leaf_count: 0,
};
static mut ENT_CACHE: [map::Ent; MAX_ENTS] = [EMPTY_ENT; MAX_ENTS];
static mut ENT_RADIUS: [i32; MAX_ENTS] = [0; MAX_ENTS];
static mut ENT_PHASE: [i32; MAX_ENTS] = [0; MAX_ENTS];
static mut PROP_COUNT: usize = 0;
static mut PROP_KIND: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_POS: [[i32; 3]; MAX_PROPS] = [[0; 3]; MAX_PROPS];
static mut PROP_YAW: [u16; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_LEAF: [i16; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_STATE: [u8; MAX_PROPS] = [PROP_STATE_IDLE; MAX_PROPS];
static mut PROP_ATTACK_COOLDOWN: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_HEALTH: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_HIT_FLASH: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut CLIP_CV: [render::CVert; 4] = [render::EMPTY_CV; 4]; // near-clip scratch (reused)

/// World->view rotation. `yaw` is stored in player-space convention, where
/// positive yaw turns the forward vector toward +world X. A view matrix is the
/// inverse of that camera rotation, so negate yaw before building rotY. Pitch is
/// still camera-space so looking up/down while turned doesn't roll the horizon.
/// Rows 0/1 are negated for the GPU's Y-down screen.
fn view_rotation(yaw: u16, pitch: i16) -> Mat3I16 {
    let view_yaw = 0u16.wrapping_sub(yaw >> 4);
    let look = Mat3I16::rotate_x((pitch >> 4) as u16).mul(&Mat3I16::rotate_y(view_yaw));
    let mut r = look;
    let mut j = 0;
    while j < 3 {
        r.m[0][j] = -r.m[0][j];
        r.m[1][j] = -r.m[1][j];
        j += 1;
    }
    r
}

#[inline(always)]
fn dot12(row: [i16; 3], e: [i32; 3]) -> i32 {
    ((row[0] as i32 * e[0]) + (row[1] as i32 * e[1]) + (row[2] as i32 * e[2])) >> 12
}

#[inline(always)]
fn scale12(x: i32, s: i32) -> i32 {
    (x * s) >> 12
}

#[inline]
fn scale12_vec(v: [i32; 3], s: i32) -> [i32; 3] {
    [scale12(v[0], s), scale12(v[1], s), scale12(v[2], s)]
}

#[inline]
fn vblank_reached(now: u32, target: u32) -> bool {
    now.wrapping_sub(target) < 0x8000_0000
}

#[inline]
fn wait_until_vblank(target: u32) {
    while !vblank_reached(interrupts::vblank_count(), target) {}
}

fn wait_vblank_edge() -> u32 {
    let entry = interrupts::vblank_count();
    loop {
        let now = interrupts::vblank_count();
        if now != entry {
            return now;
        }
    }
}

/// Integer square root (for path-segment lengths). Verified by the tram ride
/// playing back at the right pace.
fn isqrt(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n as u32;
    let mut res = 0u32;
    let mut bit = 1u32 << 30;
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

#[inline]
fn dist2_3(a: [i32; 3], b: [i32; 3]) -> i32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

#[inline]
fn dist2_3_lt(a: [i32; 3], b: [i32; 3], limit: i32) -> bool {
    dist2_3(a, b) < limit
}

/// Length of a world-space segment.
#[inline]
fn seg_len(a: [i32; 3], b: [i32; 3]) -> i32 {
    isqrt(dist2_3(a, b)).max(1)
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
    if vx.abs() * 2 > z * 2 + radius * 3 {
        return false;
    }
    let vy = dot12(rot.m[1], center) + base_t[1];
    vy.abs() * 4 <= z * 3 + radius * 5
}

#[inline]
fn prop_start_health(ty: u8) -> u8 {
    match ty {
        PROP_TYPE_SCIENTIST => SCIENTIST_HEALTH,
        PROP_TYPE_BARNEY => BARNEY_HEALTH,
        PROP_TYPE_HEADCRAB => HEADCRAB_HEALTH,
        _ => 0,
    }
}

#[inline]
fn prop_target(ty: u8, org: [i32; 3]) -> [i32; 3] {
    let h = if ty == PROP_TYPE_HEADCRAB {
        HEADCRAB_TARGET_HEIGHT
    } else {
        PROP_TARGET_HEIGHT
    };
    [org[0], org[1] + h, org[2]]
}

fn prop_clip(ty: u8, state: u8) -> usize {
    if ty != PROP_TYPE_HEADCRAB {
        return PROP_CLIP_IDLE;
    }
    match state {
        PROP_STATE_MOVE => PROP_CLIP_MOVE,
        PROP_STATE_ATTACK => PROP_CLIP_ATTACK,
        PROP_STATE_DEAD => PROP_CLIP_DEAD,
        _ => PROP_CLIP_IDLE,
    }
}

fn prop_anim_frame(md: &Model, ty: u8, state: u8, sim_frame_no: u32, pi: usize) -> usize {
    let clip = prop_clip(ty, state);
    let len = md.clip_len(clip);
    if state == PROP_STATE_DEAD {
        return md.clip_frame(clip, len.saturating_sub(1));
    }
    let phase = sim_frame_no as usize + pi.wrapping_mul(3);
    let div = if ty == PROP_TYPE_HEADCRAB && state != PROP_STATE_IDLE {
        2
    } else {
        ANIM_DIV
    };
    md.clip_frame(clip, (phase / div.max(1)) % len)
}

#[inline]
fn dist2_xz(a: [i32; 3], b: [i32; 3]) -> i32 {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    dx * dx + dz * dz
}

fn yaw_from_vec(dx: i32, dz: i32) -> u16 {
    let ax = dx.abs();
    let az = dz.abs();
    if ax * 2 < az {
        if dz >= 0 {
            0
        } else {
            2048
        }
    } else if az * 2 < ax {
        if dx >= 0 {
            1024
        } else {
            3072
        }
    } else if dx >= 0 && dz >= 0 {
        512
    } else if dx >= 0 {
        1536
    } else if dz < 0 {
        2560
    } else {
        3584
    }
}

unsafe fn init_prop_state(m: &Map) {
    let mut i = 0usize;
    while i < MAX_PROPS {
        PROP_KIND[i] = 0;
        PROP_POS[i] = [0, 0, 0];
        PROP_YAW[i] = 0;
        PROP_LEAF[i] = 0;
        PROP_STATE[i] = PROP_STATE_IDLE;
        PROP_ATTACK_COOLDOWN[i] = 0;
        PROP_HEALTH[i] = 0;
        PROP_HIT_FLASH[i] = 0;
        i += 1;
    }

    PROP_COUNT = 0;
    let nprops = m.n_props.min(MAX_PROPS);
    let mut pi = 0usize;
    while pi < nprops {
        let (ty, org, yaw, leaf) = m.prop(pi);
        let kind = ty as u8;
        PROP_KIND[pi] = kind;
        PROP_POS[pi] = org;
        PROP_YAW[pi] = yaw as u16;
        PROP_LEAF[pi] = leaf;
        PROP_HEALTH[pi] = prop_start_health(kind);
        PROP_COUNT += 1;
        pi += 1;
    }
}

unsafe fn tick_props(m: &Map, movers: &[phys::Mover], player_pos: [i32; 3], health: &mut u16) {
    let nprops = PROP_COUNT.min(MAX_PROPS);
    let mut pi = 0usize;
    while pi < nprops {
        if PROP_HIT_FLASH[pi] > 0 {
            PROP_HIT_FLASH[pi] -= 1;
        }
        if PROP_ATTACK_COOLDOWN[pi] > 0 {
            PROP_ATTACK_COOLDOWN[pi] -= 1;
        }

        let ty = PROP_KIND[pi];
        if PROP_HEALTH[pi] == 0 {
            PROP_STATE[pi] = PROP_STATE_DEAD;
            pi += 1;
            continue;
        }

        if ty == PROP_TYPE_HEADCRAB {
            let pos = PROP_POS[pi];
            let d2 = dist2_xz(pos, player_pos);
            if d2 < HEADCRAB_WAKE_RANGE2 {
                let dx = player_pos[0] - pos[0];
                let dz = player_pos[2] - pos[2];
                PROP_YAW[pi] = yaw_from_vec(dx, dz);
                if d2 <= HEADCRAB_ATTACK_RANGE2 {
                    PROP_STATE[pi] = PROP_STATE_ATTACK;
                    if PROP_ATTACK_COOLDOWN[pi] == 0 {
                        *health = health.saturating_sub(HEADCRAB_ATTACK_DAMAGE);
                        PROP_ATTACK_COOLDOWN[pi] = HEADCRAB_ATTACK_COOLDOWN;
                    }
                } else {
                    PROP_STATE[pi] = PROP_STATE_MOVE;
                    let len = isqrt(d2).max(1);
                    let step = HEADCRAB_SPEED.min(len.saturating_sub(HEADCRAB_STOP_RANGE));
                    if step > 0 {
                        let np = [pos[0] + dx * step / len, pos[1], pos[2] + dz * step / len];
                        let from = prop_target(ty, pos);
                        let to = prop_target(ty, np);
                        if phys::line_clear_world(m, from, to)
                            && phys::line_clear_movers(m, movers, from, to)
                        {
                            PROP_POS[pi] = np;
                        }
                    }
                }
            } else {
                PROP_STATE[pi] = PROP_STATE_IDLE;
            }
        }
        pi += 1;
    }
}

unsafe fn fire_glock(
    m: &Map,
    movers: &[phys::Mover],
    eye: [i32; 3],
    rot: &Mat3I16,
    base_t: [i32; 3],
) -> Option<usize> {
    let mut best = usize::MAX;
    let mut best_z = GLOCK_RANGE + 1;
    let mut best_score = i32::MAX;
    let mut pi = 0usize;
    let nprops = PROP_COUNT.min(MAX_PROPS);
    while pi < nprops {
        let ty = PROP_KIND[pi];
        if prop_start_health(ty) == 0 || PROP_HEALTH[pi] == 0 {
            pi += 1;
            continue;
        }

        let target = prop_target(ty, PROP_POS[pi]);
        let vz = dot12(rot.m[2], target) + base_t[2];
        if !(render::NEAR_Z..=GLOCK_RANGE).contains(&vz) {
            pi += 1;
            continue;
        }

        let vx = dot12(rot.m[0], target) + base_t[0];
        let vy = dot12(rot.m[1], target) + base_t[1];
        if vx.abs() * H_PROJ as i32 > vz * GLOCK_AIM_PIX_X
            || vy.abs() * H_PROJ as i32 > vz * GLOCK_AIM_PIX_Y
        {
            pi += 1;
            continue;
        }
        if !phys::line_clear_world(m, eye, target)
            || !phys::line_clear_movers(m, movers, eye, target)
        {
            pi += 1;
            continue;
        }

        let score = (vx.abs() * 2) + vy.abs();
        if vz < best_z || (vz == best_z && score < best_score) {
            best = pi;
            best_z = vz;
            best_score = score;
        }
        pi += 1;
    }

    if best == usize::MAX {
        None
    } else {
        PROP_HEALTH[best] = PROP_HEALTH[best].saturating_sub(GLOCK_DAMAGE);
        PROP_HIT_FLASH[best] = PROP_HIT_FLASH_TICKS;
        Some(best)
    }
}

#[inline]
fn abs_dot12(row: [i16; 3], ext: [i32; 3]) -> i32 {
    let ax = if row[0] < 0 {
        -(row[0] as i32)
    } else {
        row[0] as i32
    };
    let ay = if row[1] < 0 {
        -(row[1] as i32)
    } else {
        row[1] as i32
    };
    let az = if row[2] < 0 {
        -(row[2] as i32)
    } else {
        row[2] as i32
    };
    ((ax * ext[0]) + (ay * ext[1]) + (az * ext[2])) >> 12
}

#[inline]
fn box_visible(center: [i32; 3], ext: [i32; 3], rot: &Mat3I16, base_t: [i32; 3]) -> bool {
    let vz = dot12(rot.m[2], center) + base_t[2];
    let ez = abs_dot12(rot.m[2], ext);
    if vz + ez < render::NEAR_Z || vz - ez > FAR_VIEW {
        return false;
    }
    let zmax = (vz + ez).max(render::NEAR_Z);

    let vx = dot12(rot.m[0], center) + base_t[0];
    let ex = abs_dot12(rot.m[0], ext);
    if (vx - ex) * 2 > zmax * 2 + WORLD_BOUNDS_PAD || (-vx - ex) * 2 > zmax * 2 + WORLD_BOUNDS_PAD {
        return false;
    }

    let vy = dot12(rot.m[1], center) + base_t[1];
    let ey = abs_dot12(rot.m[1], ext);
    (vy - ey) * 4 <= zmax * 3 + WORLD_BOUNDS_PAD && (-vy - ey) * 4 <= zmax * 3 + WORLD_BOUNDS_PAD
}

#[inline]
fn clamp_otz(z: usize) -> usize {
    z.clamp(1, OT_LEN - 1)
}

#[inline]
fn farthest3_u16(a: u16, b: u16, c: u16) -> u32 {
    a.max(b).max(c) as u32
}

#[inline]
fn farthest4_u16(a: u16, b: u16, c: u16, d: u16) -> u32 {
    a.max(b).max(c).max(d) as u32
}

#[inline]
fn farthest3_i32(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).max(c).max(1)
}

#[inline]
fn world_otz_from_gte3(a: &Projected, b: &Projected, c: &Projected) -> usize {
    // PSoXide's renderer has a farthest-depth policy for depth-spanning world
    // surfaces. Average depth lets long sloped BSP triangles draw over nearer
    // geometry in a painter's-algorithm OT.
    clamp_otz((farthest3_u16(a.sz, b.sz, c.sz) >> 6) as usize)
}

#[inline]
fn world_otz_from_gte4(a: &Projected, b: &Projected, c: &Projected, d: &Projected) -> usize {
    clamp_otz((farthest4_u16(a.sz, b.sz, c.sz, d.sz) >> 6) as usize)
}

#[inline]
fn world_otz_from_view3(a: i32, b: i32, c: i32) -> usize {
    clamp_otz((farthest3_i32(a, b, c) >> 4) as usize)
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
        let side = dot12(nd.n, eye) - nd.dist;
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

unsafe fn next_draw_face_mark_token() -> u16 {
    let next = DRAW_FACE_MARK_TOKEN.wrapping_add(1);
    if next == 0 {
        for mark in DRAW_FACE_MARK.iter_mut() {
            *mark = 0;
        }
        DRAW_FACE_MARK_TOKEN = 1;
    } else {
        DRAW_FACE_MARK_TOKEN = next;
    }
    DRAW_FACE_MARK_TOKEN
}

unsafe fn next_pvs_face_mark_token() -> u16 {
    let next = PVS_FACE_MARK_TOKEN.wrapping_add(1);
    if next == 0 {
        for mark in PVS_FACE_MARK.iter_mut() {
            *mark = 0;
        }
        for mark in PVS_GROUP_MARK.iter_mut() {
            *mark = 0;
        }
        PVS_FACE_MARK_TOKEN = 1;
    } else {
        PVS_FACE_MARK_TOKEN = next;
    }
    PVS_FACE_MARK_TOKEN
}

unsafe fn rebuild_pvs_cache(m: &Map, cam_leaf: i32, nents: usize) {
    let (visofs, _, _) = m.leaf(cam_leaf as usize);
    decompress_vis(m, visofs, &mut VIS_BITS);
    PVS_LEAF_COUNT = 0;
    PVS_FACE_COUNT = 0;
    PVS_GROUP_COUNT = 0;
    PVS_TRI_REF_COUNT = 0;
    PVS_ENT_COUNT = 0;
    let mark_token = next_pvs_face_mark_token();

    for i in 0..m.n_leaves.saturating_sub(1).min(MAX_LEAVES) {
        if VIS_BITS[i >> 3] & (1u8 << (i & 7)) == 0 {
            continue;
        }
        let leaf = i + 1;
        PVS_LEAF_COUNT += 1;

        let (_, m0, mc) = m.leaf(leaf);
        for mj in m0..m0 + mc {
            if mj >= m.n_marks {
                break;
            }
            let face = m.mark(mj);
            if face >= MAX_FACES || PVS_FACE_MARK[face] == mark_token {
                continue;
            }
            PVS_FACE_MARK[face] = mark_token;
            let (first, cnt) = m.face_tris(face);
            if cnt == 0 || first > u16::MAX as usize || PVS_FACE_COUNT >= MAX_FACES {
                continue;
            }

            let group = m.face_group(face);
            if group >= MAX_FACES {
                continue;
            }
            if PVS_GROUP_MARK[group] != mark_token {
                if PVS_GROUP_COUNT >= MAX_FACES {
                    break;
                }
                PVS_GROUP_MARK[group] = mark_token;
                PVS_GROUP_FIRST[group] = PVS_LINK_END;
                let (n, d) = m.face_plane(face);
                PVS_GROUP_NRM[group] = n;
                PVS_GROUP_DIST[group] = d;
                PVS_GROUP_ACTIVE[PVS_GROUP_COUNT] = group as u16;
                PVS_GROUP_COUNT += 1;
            }

            let entry = PVS_FACE_COUNT;
            PVS_FACE_FIRST[entry] = first as u16;
            PVS_FACE_TRI_COUNT[entry] = cnt as u16;
            PVS_TRI_REF_COUNT += cnt;
            let (bc, be) = m.face_bounds(face);
            PVS_FACE_CENTER[entry] = [bc[0] as i16, bc[1] as i16, bc[2] as i16];
            PVS_FACE_EXTENT[entry] = [be[0] as u16, be[1] as u16, be[2] as u16];
            PVS_FACE_NEXT[entry] = PVS_GROUP_FIRST[group];
            PVS_GROUP_FIRST[group] = entry as u16;
            PVS_FACE_COUNT += 1;
        }
    }
    let mut ei = 0usize;
    while ei < nents {
        let e = ENT_CACHE[ei];
        if entity_touches_pvs(m, &e) && PVS_ENT_COUNT < MAX_ENTS {
            PVS_ENTS[PVS_ENT_COUNT] = ei as u16;
            PVS_ENT_COUNT += 1;
        }
        ei += 1;
    }
    PVS_CAM_LEAF = cam_leaf;
}

#[inline]
fn pvs_leaf_visible(m: &Map, leaf: usize) -> bool {
    if leaf == 0 || leaf >= m.n_leaves {
        return false;
    }
    let bit = leaf - 1;
    if bit >= m.n_leaves.saturating_sub(1) || bit >= MAX_LEAVES {
        return false;
    }
    unsafe { (VIS_BITS[bit >> 3] & (1u8 << (bit & 7))) != 0 }
}

#[inline]
fn entity_touches_pvs(m: &Map, e: &map::Ent) -> bool {
    if e.leaf_count == 0 {
        return true;
    }
    let end = e.leaf_start.saturating_add(e.leaf_count);
    let mut i = e.leaf_start;
    while i < end {
        if pvs_leaf_visible(m, m.ent_leaf(i)) {
            return true;
        }
        i += 1;
    }
    false
}

/// Project vertex `i` into the cache once per frame (base view matrix).
#[inline]
unsafe fn proj_vert(m: &Map, i: usize, frame: u16) {
    if VERT_FRAME[i] != frame {
        SCRATCH[i] = scene::project_vertex_scheduled(m.vert(i));
        VERT_FRAME[i] = frame;
    }
}

unsafe fn next_submodel_draw_token() -> u16 {
    let next = SUBMODEL_DRAW_TOKEN.wrapping_add(1);
    if next == 0 {
        for mark in SUBMODEL_VERT_TOKEN.iter_mut() {
            *mark = 0;
        }
        SUBMODEL_DRAW_TOKEN = 1;
    } else {
        SUBMODEL_DRAW_TOKEN = next;
    }
    SUBMODEL_DRAW_TOKEN
}

#[inline]
unsafe fn proj_submodel_vert(m: &Map, i: usize, token: u16) {
    if SUBMODEL_VERT_TOKEN[i] != token {
        SCRATCH[i] = scene::project_vertex_scheduled(m.vert(i));
        SUBMODEL_VERT_TOKEN[i] = token;
    }
}

#[inline]
const fn uv_word(uv: (u8, u8)) -> u16 {
    (uv.0 as u16) | ((uv.1 as u16) << 8)
}

#[inline]
const fn uv_word_pair_i32(word: u16) -> (i32, i32) {
    ((word & 0xff) as i32, ((word >> 8) & 0xff) as i32)
}

#[inline]
unsafe fn cached_world_uv_words(m: &Map, tri_index: usize) -> [u16; 3] {
    if tri_index < WORLD_UV_COUNT {
        WORLD_UV_WORDS[tri_index]
    } else {
        m.tri_uv_words(tri_index)
    }
}

#[inline]
unsafe fn push_tri(
    packets: &mut PrimitivePacketArena<'_>,
    np: &mut usize,
    screen: [(i16, i16); 3],
    uv: [(u8, u8); 3],
    rgb: [(u8, u8, u8); 3],
    mat: TexturedGouraudPacketMaterial,
    otz: usize,
) {
    push_tri_uv_words(
        packets,
        np,
        screen,
        [uv_word(uv[0]), uv_word(uv[1]), uv_word(uv[2])],
        rgb,
        mat,
        otz,
    );
}

#[inline]
unsafe fn push_tri_uv_words(
    packets: &mut PrimitivePacketArena<'_>,
    np: &mut usize,
    screen: [(i16, i16); 3],
    uv_words: [u16; 3],
    rgb: [(u8, u8, u8); 3],
    mat: TexturedGouraudPacketMaterial,
    otz: usize,
) {
    let prim = TriTexturedGouraud::with_packet_material_packed_uv_words(screen, uv_words, rgb, mat);
    let Some(packet) = packets.push(prim) else {
        return;
    };
    OT.add(otz, packet, TriTexturedGouraud::WORDS);
    *np += 1;
}

/// Emit triangle `t` from its three projected screen verts `p`. Small in-front
/// triangles emit straight from the cache. Anything large (affine warp) or
/// near-straddling drops to the view-space path: near-clip, then recursively
/// split at view-space midpoints while it's big on screen, then guard-clip.
unsafe fn emit_projected(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tri: map::RenderTri,
    p: [Projected; 3],
    nv: usize,
    np: &mut usize,
) {
    let (a, b, c) = (
        tri.idx[0] as usize,
        tri.idx[1] as usize,
        tri.idx[2] as usize,
    );
    if a >= nv || b >= nv || c >= nv {
        return;
    }
    let rgb = tri.rgb;
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
        if tri.tex >= m.n_texs || tri.tex >= MAX_TEX_SLOTS {
            return;
        }
        let slot = TEX_SLOTS[tri.tex];
        if !slot.valid {
            return;
        }
        push_tri_uv_words(
            packets,
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            tri.uv_words,
            rgb,
            slot.packet,
            world_otz_from_gte3(&pa, &pb, &pc),
        );
        return;
    }
    if pa.sz == 0 && pb.sz == 0 && pc.sz == 0 {
        return; // entirely behind the camera
    }
    if tri.tex >= m.n_texs || tri.tex >= MAX_TEX_SLOTS {
        return;
    }
    let slot = TEX_SLOTS[tri.tex];
    if !slot.valid {
        return;
    }

    // View-space path (near-plane straddlers): rebuild, near-clip, emit.
    let cvv = |idx: usize, k: usize| {
        let v = scene::transform_vertex_scheduled(m.vert(idx));
        let uv = uv_word_pair_i32(tri.uv_words[k]);
        render::CVert {
            v: [v.x, v.y, v.z],
            rgb: (rgb[k].0 as i32, rgb[k].1 as i32, rgb[k].2 as i32),
            uv,
        }
    };
    let cv = [cvv(a, 0), cvv(b, 1), cvv(c, 2)];
    let n = render::near_clip(&cv, &mut CLIP_CV);
    if n < 3 {
        return;
    }
    for k in 1..n - 1 {
        emit_cv(
            packets,
            &[CLIP_CV[0], CLIP_CV[k], CLIP_CV[k + 1]],
            SUBDIV_DEPTH,
            slot.packet,
            np,
        );
    }
}

/// Recursively split a view-space triangle at its midpoints while it's larger
/// than SUBDIV_PX on screen (affine perspective correction), then guard-clip and
/// emit. ponytail: depth 1 (<=4 sub-tris per big tri); raise SUBDIV_DEPTH if warp
/// is still visible, at the cost of more triangles.
unsafe fn emit_cv(
    packets: &mut PrimitivePacketArena<'_>,
    cv: &[render::CVert; 3],
    depth: u8,
    mat: TexturedGouraudPacketMaterial,
    np: &mut usize,
) {
    let pa = render::project_soft(&cv[0]);
    let pb = render::project_soft(&cv[1]);
    let pc = render::project_soft(&cv[2]);
    let spanx = pa.x.max(pb.x).max(pc.x) - pa.x.min(pb.x).min(pc.x);
    let spany = pa.y.max(pb.y).max(pc.y) - pa.y.min(pb.y).min(pc.y);
    if depth > 0 && (spanx > SUBDIV_PX || spany > SUBDIV_PX) {
        let ab = render::mid_cv(&cv[0], &cv[1]);
        let bc = render::mid_cv(&cv[1], &cv[2]);
        let ca = render::mid_cv(&cv[2], &cv[0]);
        emit_cv(packets, &[cv[0], ab, ca], depth - 1, mat, np);
        emit_cv(packets, &[ab, cv[1], bc], depth - 1, mat, np);
        emit_cv(packets, &[ca, bc, cv[2]], depth - 1, mat, np);
        emit_cv(packets, &[ab, bc, ca], depth - 1, mat, np);
        return;
    }
    if CULL && culled((pa.x, pa.y), (pb.x, pb.y), (pc.x, pc.y)) {
        return;
    }
    let cl = |x: i32| x.clamp(0, 255) as u8;
    // Common case: fully on-screen -> draw directly, no guard-clip buffer.
    if render::in_band(&pa) && render::in_band(&pb) && render::in_band(&pc) {
        push_tri(
            packets,
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
            world_otz_from_view3(pa.z, pb.z, pc.z),
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
        push_tri(
            packets,
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
            world_otz_from_view3(s0.z, s1.z, s2.z),
        );
    }
}

unsafe fn try_emit_tri_pair_quad_values(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    t0: map::RenderTri,
    t1: map::RenderTri,
    nv: usize,
    frame: u16,
    nq: &mut usize,
) -> bool {
    if t0.tex >= m.n_texs || t0.tex >= MAX_TEX_SLOTS {
        return false;
    }

    let a = t0.idx[0] as usize;
    let c = t0.idx[1] as usize;
    let b = t0.idx[2] as usize;
    let d = t1.idx[1] as usize;
    if t0.tex != t1.tex || t1.idx[0] as usize != a || t1.idx[2] as usize != c {
        return false;
    }
    if a >= nv || b >= nv || c >= nv || d >= nv {
        return true;
    }

    let slot = TEX_SLOTS[t0.tex];
    if !slot.valid {
        return true;
    }

    proj_vert(m, a, frame);
    proj_vert(m, b, frame);
    proj_vert(m, c, frame);
    proj_vert(m, d, frame);
    let (pa, pb, pc, pd) = (SCRATCH[a], SCRATCH[b], SCRATCH[c], SCRATCH[d]);
    let clamped = |q: &Projected| q.sx <= -1023 || q.sx >= 1023 || q.sy <= -1023 || q.sy >= 1023;
    if pa.sz < NEAR
        || pb.sz < NEAR
        || pc.sz < NEAR
        || pd.sz < NEAR
        || clamped(&pa)
        || clamped(&pb)
        || clamped(&pc)
        || clamped(&pd)
    {
        return false;
    }
    if CULL
        && culled(
            (pa.sx as i32, pa.sy as i32),
            (pc.sx as i32, pc.sy as i32),
            (pb.sx as i32, pb.sy as i32),
        )
    {
        return true;
    }

    let prim = QuadTexturedGouraud::with_packet_material_packed_uv_words(
        [
            (pb.sx, pb.sy),
            (pa.sx, pa.sy),
            (pc.sx, pc.sy),
            (pd.sx, pd.sy),
        ],
        [
            t0.uv_words[2],
            t0.uv_words[0],
            t0.uv_words[1],
            t1.uv_words[1],
        ],
        [t0.rgb[2], t0.rgb[0], t0.rgb[1], t1.rgb[1]],
        slot.packet,
    );
    let Some(packet) = packets.push(prim) else {
        return false;
    };
    OT.add(
        world_otz_from_gte4(&pa, &pb, &pc, &pd),
        packet,
        QuadTexturedGouraud::WORDS,
    );
    *nq += 1;
    true
}

#[derive(Clone, Copy)]
struct WorldCounters {
    cells_considered: u32,
    cells_drawn: u32,
    surfaces_considered: u32,
    emit_calls: u32,
}

impl WorldCounters {
    const fn new() -> WorldCounters {
        WorldCounters {
            cells_considered: 0,
            cells_drawn: 0,
            surfaces_considered: 0,
            emit_calls: 0,
        }
    }
}

unsafe fn emit_world_render_tri(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tri: map::RenderTri,
    nv: usize,
    frame: u16,
    np: &mut usize,
    counts: &mut WorldCounters,
) {
    let (a, b, c) = (
        tri.idx[0] as usize,
        tri.idx[1] as usize,
        tri.idx[2] as usize,
    );
    if a < nv && b < nv && c < nv {
        proj_vert(m, a, frame);
        proj_vert(m, b, frame);
        proj_vert(m, c, frame);
        counts.emit_calls += 1;
        emit_projected(
            packets,
            m,
            tri,
            [SCRATCH[a], SCRATCH[b], SCRATCH[c]],
            nv,
            np,
        );
    }
}

unsafe fn emit_world_face_tris(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    first: usize,
    cnt: usize,
    nv: usize,
    frame: u16,
    np: &mut usize,
    nq: &mut usize,
    counts: &mut WorldCounters,
) {
    if cnt == 2 && first + 1 < m.n_tris {
        let t0 = m.render_tri(first, cached_world_uv_words(m, first));
        let t1 = m.render_tri(first + 1, cached_world_uv_words(m, first + 1));
        if WORLD_QUAD_PAIRING && try_emit_tri_pair_quad_values(packets, m, t0, t1, nv, frame, nq) {
            counts.emit_calls += 2;
            return;
        }
        emit_world_render_tri(packets, m, t0, nv, frame, np, counts);
        emit_world_render_tri(packets, m, t1, nv, frame, np, counts);
        return;
    }
    let end = first + cnt;
    let mut tt = first;
    while tt < end {
        if tt >= m.n_tris {
            tt += 1;
            continue;
        }
        let tri = m.render_tri(tt, cached_world_uv_words(m, tt));
        emit_world_render_tri(packets, m, tri, nv, frame, np, counts);
        tt += 1;
    }
}

unsafe fn emit_world_face(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    face: usize,
    nv: usize,
    frame: u16,
    eye: [i32; 3],
    rot: &Mat3I16,
    base_t: [i32; 3],
    np: &mut usize,
    nq: &mut usize,
    counts: &mut WorldCounters,
    draw_token: u16,
) {
    if face >= m.n_faces || face >= MAX_FACES || DRAW_FACE_MARK[face] == draw_token {
        return;
    }
    DRAW_FACE_MARK[face] = draw_token;
    counts.surfaces_considered += 1;

    let (fnrm, fd) = m.face_plane(face);
    if dot12(fnrm, eye) <= fd {
        return;
    }
    let (first, cnt) = m.face_tris(face);
    if cnt == 0 {
        return;
    }
    let (bc, be) = m.face_bounds(face);
    if WORLD_BOUNDS_CULL && !box_visible(bc, be, rot, base_t) {
        return;
    }
    emit_world_face_tris(packets, m, first, cnt, nv, frame, np, nq, counts);
}

/// Draw a model at world `pos`, rotated by `yaw` (Q0.12), at animation `frame`.
unsafe fn draw_model(
    packets: &mut PrimitivePacketArena<'_>,
    md: &Model,
    slots: &[TexSlot],
    faces: *const ModelRenderFace,
    face_count: usize,
    pos: [i32; 3],
    yaw: u16,
    frame: usize,
    shade: u8,
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
    let nv = md.n_verts.min(MAX_MODEL_VERTS);
    for i in 0..nv {
        MODEL_SCRATCH[i] = scene::project_vertex_scheduled(md.vert(frame, i));
    }
    let nfaces = face_count.min(md.n_tris);
    for t in 0..nfaces {
        let render_face = *faces.add(t);
        let (a, b, c) = (
            render_face.face.vertex_indices[0] as usize,
            render_face.face.vertex_indices[1] as usize,
            render_face.face.vertex_indices[2] as usize,
        );
        if a >= nv || b >= nv || c >= nv {
            continue;
        }
        let (pa, pb, pc) = (MODEL_SCRATCH[a], MODEL_SCRATCH[b], MODEL_SCRATCH[c]);
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
        let slot = slots[(render_face.tex as usize).min(slots.len() - 1)];
        if !slot.valid {
            continue;
        }
        let avgz = ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3;
        push_tri_uv_words(
            packets,
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            render_face.face.uv_words,
            [(shade, shade, shade); 3],
            slot.packet,
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
    packets: &mut PrimitivePacketArena<'_>,
    md: &Model,
    slots: &[TexSlot],
    frame: usize,
    recoil_y: i32,
    np: &mut usize,
) {
    let nv = md.n_verts.min(MAX_MODEL_VERTS);
    if WEAPON_CACHE_FRAME != frame || WEAPON_CACHE_RECOIL != recoil_y || WEAPON_CACHE_VERTS != nv {
        let r = viewmodel_rot();
        scene::load_rotation(&r);
        scene::load_translation(Vec3I32::new(
            VM_VIEW_SHIFT[0],
            VM_VIEW_SHIFT[1] + recoil_y,
            VM_VIEW_SHIFT[2],
        ));
        for i in 0..nv {
            MODEL_SCRATCH[i] = scene::project_vertex_scheduled(md.vert(frame, i));
        }
        WEAPON_CACHE_FRAME = frame;
        WEAPON_CACHE_RECOIL = recoil_y;
        WEAPON_CACHE_VERTS = nv;

        WEAPON_TRI_COUNT = 0;
        // HMDL keeps texture groups in source order (sleeve/glove before gun).
        // The viewmodel is camera-locked, so cache the already-cullled packet
        // stream until the authored frame or recoil offset changes.
        for t in 0..md.n_tris {
            if WEAPON_TRI_COUNT >= MAX_WEAPON_CACHE_TRIS {
                break;
            }
            let tri = md.tri(t);
            let tex_id = tri.tex;
            let slot = slots[tex_id.min(slots.len() - 1)];
            if !slot.valid {
                continue;
            }
            let (a, b, c) = (
                tri.idx[0] as usize,
                tri.idx[1] as usize,
                tri.idx[2] as usize,
            );
            if a >= nv || b >= nv || c >= nv {
                continue;
            }
            let (pa, pb, pc) = (MODEL_SCRATCH[a], MODEL_SCRATCH[b], MODEL_SCRATCH[c]);
            if pa.sz < NEAR || pb.sz < NEAR || pc.sz < NEAR {
                continue;
            }
            if VM_CULL && tex_id != VM_TWO_SIDED_TEX {
                let area = (pb.sx as i32 - pa.sx as i32) * (pc.sy as i32 - pa.sy as i32)
                    - (pc.sx as i32 - pa.sx as i32) * (pb.sy as i32 - pa.sy as i32);
                if (area >= 0) == VM_CULL_POS {
                    continue;
                }
            }
            let avgz = ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3;
            WEAPON_TRI_CACHE[WEAPON_TRI_COUNT] =
                TriTexturedGouraud::with_packet_material_packed_uv_words(
                    [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
                    [uv_word(tri.uv[0]), uv_word(tri.uv[1]), uv_word(tri.uv[2])],
                    [(VM_SHADE, VM_SHADE, VM_SHADE); 3],
                    slot.packet,
                );
            WEAPON_TRI_OTZ[WEAPON_TRI_COUNT] = viewmodel_otz(avgz) as u8;
            WEAPON_TRI_COUNT += 1;
        }
    }

    for i in 0..WEAPON_TRI_COUNT {
        let mut prim = EMPTY_TRI;
        prim.copy_payload_from(&WEAPON_TRI_CACHE[i]);
        let Some(packet) = packets.push(prim) else {
            break;
        };
        WEAPON_OT.add(
            WEAPON_TRI_OTZ[i] as usize,
            packet,
            TriTexturedGouraud::WORDS,
        );
        *np += 1;
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
    let barney = Model::load(BARNEY_BYTES);
    let headcrab = Model::load(HEADCRAB_BYTES);
    unsafe {
        SCI_FACE_COUNT = sci.fill_render_faces_raw(
            core::ptr::addr_of_mut!(SCI_FACES).cast::<ModelRenderFace>(),
            SCI_FACE_CAP,
        );
        BARNEY_FACE_COUNT = barney.fill_render_faces_raw(
            core::ptr::addr_of_mut!(BARNEY_FACES).cast::<ModelRenderFace>(),
            BARNEY_FACE_CAP,
        );
        HEADCRAB_FACE_COUNT = headcrab.fill_render_faces_raw(
            core::ptr::addr_of_mut!(HEADCRAB_FACES).cast::<ModelRenderFace>(),
            HEADCRAB_FACE_CAP,
        );
    }

    // Boot flow: pick a map in the menu, stream + play it, return on Select.
    // (Analog is enabled inside play(); the menu runs on the digital pad.)
    loop {
        let sel = menu::run(&mut fb);
        play(&mut fb, &sci, &barney, &headcrab, sel as u32);
    }
}

#[inline(always)]
fn room_world_chunk_id(room_id: u32) -> u32 {
    room_id.saturating_mul(ROOM_WORLD_CHUNK_MUL)
}

#[inline(always)]
fn room_texture_chunk_id(room_id: u32) -> u32 {
    room_world_chunk_id(room_id).saturating_add(ROOM_TEXTURE_CHUNK_ADD)
}

/// Stream menu room `chunk_id` from WORLD.PAK, upload its textures, and run the
/// renderer + physics loop until the player presses Select (back to menu).
fn play(fb: &mut FrameBuffer, sci: &Model, barney: &Model, headcrab: &Model, room_id: u32) {
    let _ = enable_analog_port1();
    telemetry::frame_begin(0);
    telemetry::task_begin(telemetry::task::FIXED_UPDATE);
    let texture_chunk_id = room_texture_chunk_id(room_id);
    let world_chunk_id = room_world_chunk_id(room_id);

    telemetry::stage_begin(telemetry::stage::CD_WORLD_PACK_STREAM);
    let tex_len = cdstream::load_chunk(texture_chunk_id, unsafe { &mut MAP_BUF }).unwrap_or(0);
    telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
    let mut stream_bytes = tex_len as u32;
    let mut stream_chunks = if tex_len == 0 { 0 } else { 1 };
    let mut stream_sectors = if tex_len == 0 {
        0
    } else {
        ((tex_len as u32) + 2047) / 2048
    };
    if tex_len == 0 {
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_CHUNKS, 0);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_BYTES, 0);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_SECTORS, 0);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_STATUS, 0);
        telemetry::task_end(telemetry::task::FIXED_UPDATE);
        tty::println("hl-psx: WORLD.PAK texture stream failed");
        telemetry::debug_log("hl-psx: WORLD.PAK texture stream failed");
        return;
    }
    telemetry::debug_log("hl-psx: WORLD.PAK texture chunk loaded");

    telemetry::stage_begin(telemetry::stage::CD_WORLD_PACK_STREAM);
    let weapon_len =
        cdstream::load_chunk(MODEL_CHUNK_V_9MMHANDGUN, unsafe { &mut MODEL_BUF }).unwrap_or(0);
    telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
    if weapon_len > 0 {
        stream_chunks += 1;
        stream_bytes = stream_bytes.saturating_add(weapon_len as u32);
        stream_sectors = stream_sectors.saturating_add(((weapon_len as u32) + 2047) / 2048);
    } else {
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_CHUNKS, stream_chunks);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_BYTES, stream_bytes);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_SECTORS, stream_sectors);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_STATUS, 0);
        telemetry::task_end(telemetry::task::FIXED_UPDATE);
        tty::println("hl-psx: WORLD.PAK weapon stream failed");
        telemetry::debug_log("hl-psx: WORLD.PAK weapon stream failed");
        return;
    }
    telemetry::debug_log("hl-psx: WORLD.PAK weapon chunk loaded");

    let weapon_bytes: &'static [u8] =
        unsafe { core::slice::from_raw_parts(MODEL_BUF.as_ptr() as *const u8, weapon_len) };
    let wpn = Model::load(weapon_bytes);
    let tex_bytes = unsafe { core::slice::from_raw_parts(MAP_BUF.as_ptr() as *const u8, tex_len) };
    telemetry::stage_begin(telemetry::stage::VRAM_UPLOAD);
    let (room_texs, tex_failed) = match unsafe {
        vram::upload_tex_chunk_raw(
            tex_bytes,
            core::ptr::addr_of_mut!(TEX_SLOTS).cast::<TexSlot>(),
            MAX_TEX_SLOTS,
        )
    } {
        Some(v) => v,
        None => {
            telemetry::stage_end(telemetry::stage::VRAM_UPLOAD);
            telemetry::counter(telemetry::counter::CD_WORLD_PACK_CHUNKS, stream_chunks);
            telemetry::counter(telemetry::counter::CD_WORLD_PACK_BYTES, stream_bytes);
            telemetry::counter(telemetry::counter::CD_WORLD_PACK_SECTORS, stream_sectors);
            telemetry::counter(telemetry::counter::CD_WORLD_PACK_STATUS, 0);
            telemetry::task_end(telemetry::task::FIXED_UPDATE);
            tty::println("hl-psx: WORLD.PAK texture chunk invalid");
            telemetry::debug_log("hl-psx: WORLD.PAK texture chunk invalid");
            return;
        }
    };
    telemetry::counter(
        telemetry::counter::ROOM_TEXTURE_UPLOADS,
        room_texs.saturating_sub(tex_failed) as u32,
    );
    telemetry::counter(
        telemetry::counter::ROOM_MATERIAL_TEXTURE_DROPS,
        tex_failed as u32,
    );
    unsafe {
        vram::upload_tex_blob_raw(
            sci.tex_blob(),
            sci.n_texs,
            core::ptr::addr_of_mut!(SCI_SLOTS).cast::<TexSlot>(),
            24,
        );
        vram::upload_tex_blob_raw(
            barney.tex_blob(),
            barney.n_texs,
            core::ptr::addr_of_mut!(BARNEY_SLOTS).cast::<TexSlot>(),
            24,
        );
        vram::upload_tex_blob_raw(
            headcrab.tex_blob(),
            headcrab.n_texs,
            core::ptr::addr_of_mut!(HEADCRAB_SLOTS).cast::<TexSlot>(),
            8,
        );
        vram::upload_tex_blob_raw(
            wpn.tex_blob(),
            wpn.n_texs,
            core::ptr::addr_of_mut!(WEAPON_SLOTS).cast::<TexSlot>(),
            12,
        );
        WEAPON_CACHE_FRAME = usize::MAX;
        WEAPON_CACHE_RECOIL = i32::MIN;
        WEAPON_CACHE_VERTS = 0;
        WEAPON_TRI_COUNT = 0;
    }
    let hud_mat = hud::upload(); // real HUD sprite sheet -> free gameplay tpage
    telemetry::stage_end(telemetry::stage::VRAM_UPLOAD);

    telemetry::stage_begin(telemetry::stage::CD_WORLD_PACK_STREAM);
    let map_len = cdstream::load_chunk(world_chunk_id, unsafe { &mut MAP_BUF }).unwrap_or(0);
    telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
    if map_len > 0 {
        stream_chunks += 1;
        stream_bytes = stream_bytes.saturating_add(map_len as u32);
        stream_sectors = stream_sectors.saturating_add(((map_len as u32) + 2047) / 2048);
    }
    telemetry::counter(telemetry::counter::CD_WORLD_PACK_CHUNKS, stream_chunks);
    telemetry::counter(telemetry::counter::CD_WORLD_PACK_BYTES, stream_bytes);
    telemetry::counter(telemetry::counter::CD_WORLD_PACK_SECTORS, stream_sectors);
    telemetry::counter(
        telemetry::counter::CD_WORLD_PACK_STATUS,
        if map_len == 0 { 0 } else { 1 },
    );
    telemetry::task_end(telemetry::task::FIXED_UPDATE);
    if map_len == 0 {
        tty::println("hl-psx: WORLD.PAK world stream failed");
        telemetry::debug_log("hl-psx: WORLD.PAK world stream failed");
        return;
    }
    telemetry::debug_log("hl-psx: WORLD.PAK world chunk loaded");

    let map_bytes = unsafe { core::slice::from_raw_parts(MAP_BUF.as_ptr() as *const u8, map_len) };
    let m = Map::load(map_bytes);
    unsafe {
        WORLD_UV_COUNT = m.fill_uv_words_raw(
            core::ptr::addr_of_mut!(WORLD_UV_WORDS).cast::<[u16; 3]>(),
            MAX_WORLD_TRIS,
        );
    }
    if room_texs != m.n_texs {
        tty::println("hl-psx: texture/world count mismatch");
        telemetry::debug_log("hl-psx: texture/world count mismatch");
    }
    unsafe {
        PVS_CAM_LEAF = -1;
        PVS_LEAF_COUNT = 0;
        PVS_ENT_COUNT = 0;
        init_prop_state(&m);
    }
    let nents = m.n_ents.min(MAX_ENTS);
    unsafe {
        for ei in 0..nents {
            let e = m.entity(ei);
            ENT_CACHE[ei] = e;
            ENT_RADIUS[ei] = isqrt(e.r2);
        }
    }
    let nv = if m.n_verts < MAX_VERTS {
        m.n_verts
    } else {
        MAX_VERTS
    };

    let mut player = phys::Player::new(m.spawn_pos);
    let mut yaw: u16 = (m.spawn_yaw as u16) & 0xFFF;
    let mut pitch: i16 = 0;
    let mut frame_no: u16 = 0;
    let mut telemetry_frame: u32 = 1;
    let mut sim_frame_no: u32 = 0;
    let mut ammo: u16 = GLOCK_MAX_CLIP;
    let mut health: u16 = PLAYER_START_HEALTH;

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

    telemetry::stage_begin(telemetry::stage::ROOM_SURFACE_CACHE);
    unsafe {
        let initial_eye = [player.pos[0], player.pos[1] + VIEW_HEIGHT, player.pos[2]];
        let initial_leaf = camera_leaf(&m, initial_eye);
        if initial_leaf > 0 && (initial_leaf as usize) < m.n_leaves {
            rebuild_pvs_cache(&m, initial_leaf, nents);
            telemetry::counter(telemetry::counter::ROOM_SURFACE_CACHE_BUILDS, 1);
            telemetry::counter(
                telemetry::counter::ROOM_SURFACE_CACHE_BUILD_SURFACES,
                PVS_FACE_COUNT as u32,
            );
            telemetry::counter(
                telemetry::counter::ROOM_SURFACE_CACHE_BUILD_VERTICES,
                PVS_TRI_REF_COUNT as u32,
            );
        }

        WEAPON_OT.clear();
        let mut warm_packets = PrimitivePacketArena::new(&mut PRIMITIVE_PACKETS);
        let mut warm_np = 0usize;
        draw_viewmodel(
            &mut warm_packets,
            &wpn,
            &WEAPON_SLOTS,
            VM_FRAME,
            -recoil,
            &mut warm_np,
        );
        WEAPON_OT.clear();
    }
    telemetry::stage_end(telemetry::stage::ROOM_SURFACE_CACHE);

    gpu::configure_vsync_timer();
    interrupts::install_vblank_counter();
    let mut next_sim_vblank = interrupts::vblank_count();

    loop {
        wait_until_vblank(next_sim_vblank);
        let mut ticks_this_visual = 0u16;
        while vblank_reached(interrupts::vblank_count(), next_sim_vblank) {
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
            // R2 fires at the Glock cadence; the actual hit-test runs after
            // movement/movers update so it uses the current camera and doors.
            recoil = (recoil - 3).max(0);
            let want_fire = pad.buttons.is_held(button::R2) && recoil == 0 && ammo > 0;
            let (mut fwd, mut strafe, mut turn, mut look) = (0i32, 0i32, 0i32, 0i32);
            if pad.is_analog() {
                let (lx, ly) = pad.sticks.left_centered();
                let (rx, ry) = pad.sticks.right_centered();
                let dz2 = DEADZONE * DEADZONE;
                // Radial deadzone per stick (avoids axis drift / diagonal bias).
                if (lx as i32) * (lx as i32) + (ly as i32) * (ly as i32) > dz2 {
                    fwd = -(ly as i32); // stick up = forward
                    strafe = -(lx as i32);
                }
                if (rx as i32) * (rx as i32) + (ry as i32) * (ry as i32) > dz2 {
                    turn = -(rx as i32);
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
                for ei in 0..nents {
                    let e = ENT_CACHE[ei];
                    let off = if e.kind == 1 {
                        let near = dist2_3_lt(player.pos, e.center, e.r2);
                        let ph = &mut ENT_PHASE[ei];
                        *ph = if near {
                            (*ph + DOOR_SPEED).min(4096)
                        } else {
                            (*ph - DOOR_SPEED).max(0)
                        };
                        scale12_vec(e.mv, *ph)
                    } else {
                        e.origin
                    };
                    if e.kind != 2 && nmov < movers.len() {
                        movers[nmov] = phys::Mover {
                            head: e.head,
                            off,
                            center: e.center,
                            radius: ENT_RADIUS[ei],
                        };
                        nmov += 1;
                    }
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
                        center: [0, 0, 0],
                        radius: 0,
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
                telemetry::stage_begin(telemetry::stage::SIM_COLLISION);
                player.update(
                    &m,
                    movers,
                    fwd,
                    strafe,
                    pad.buttons.is_held(button::CROSS),
                    yaw,
                );
                telemetry::stage_end(telemetry::stage::SIM_COLLISION);
            }
            let eye = [player.pos[0], player.pos[1] + VIEW_HEIGHT, player.pos[2]];
            if want_fire {
                ammo = ammo.saturating_sub(1);
                recoil = 16;
                let fire_rot = view_rotation(yaw, pitch);
                let fire_base_t = [
                    -dot12(fire_rot.m[0], eye),
                    -dot12(fire_rot.m[1], eye),
                    -dot12(fire_rot.m[2], eye),
                ];
                unsafe {
                    let _ = fire_glock(&m, movers, eye, &fire_rot, fire_base_t);
                }
            }
            unsafe {
                tick_props(&m, movers, player.pos, &mut health);
            }
            telemetry::counter(
                telemetry::counter::ROOM_CAMERA_GLOBAL_X_BIASED,
                (eye[0] + 32768).max(0) as u32,
            );
            telemetry::counter(
                telemetry::counter::ROOM_CAMERA_GLOBAL_Y_BIASED,
                (eye[1] + 32768).max(0) as u32,
            );
            telemetry::counter(
                telemetry::counter::ROOM_CAMERA_GLOBAL_Z_BIASED,
                (eye[2] + 32768).max(0) as u32,
            );
            telemetry::counter(telemetry::counter::ROOM_PLAYER_VIEW_YAW_Q12, yaw as u32);

            telemetry::stage_end(telemetry::stage::UPDATE);
            telemetry::counter(telemetry::counter::SIM_TICKS, 1);
            telemetry::counter(telemetry::counter::VISUAL_INTERVAL_VBLANKS, SIM_VBLANKS);
            telemetry::task_end(telemetry::task::FIXED_UPDATE);

            telemetry_frame = telemetry_frame.wrapping_add(1);
            sim_frame_no = sim_frame_no.wrapping_add(1);
            ticks_this_visual = ticks_this_visual.saturating_add(1);
            next_sim_vblank = next_sim_vblank.wrapping_add(SIM_VBLANKS);
        }
        if ticks_this_visual > 1 {
            telemetry::counter(
                telemetry::counter::VISUAL_SKIPPED_VBLANKS,
                ticks_this_visual.saturating_sub(1) as u32,
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

        frame_no = frame_no.wrapping_add(1);
        telemetry::task_begin(telemetry::task::VISUAL_RENDER);
        telemetry::stage_begin(telemetry::stage::RENDER);

        unsafe {
            if frame_no == 0 {
                for f in VERT_FRAME.iter_mut() {
                    *f = 0;
                }
                frame_no = 1;
            }
            OT.clear();
            WEAPON_OT.clear();
            HUD_OT.clear();
            let mut packets = PrimitivePacketArena::new(&mut PRIMITIVE_PACKETS);
            let mut np = 0usize;
            let mut nq = 0usize;

            // World (model 0): PVS-visible faces cached by leaf. Runtime does
            // one backface test per cooked plane group, cheap face-bounds
            // rejection, then emits triangle fans with PS1 quad pairing.
            telemetry::stage_begin(telemetry::stage::ROOM);
            let cam_leaf = camera_leaf(&m, eye);
            let have_pvs = cam_leaf > 0 && (cam_leaf as usize) < m.n_leaves;
            if have_pvs {
                if PVS_CAM_LEAF != cam_leaf {
                    rebuild_pvs_cache(&m, cam_leaf, nents);
                }
                let mut room_counts = WorldCounters::new();
                room_counts.cells_considered = PVS_LEAF_COUNT as u32;
                room_counts.cells_drawn = PVS_LEAF_COUNT as u32;
                room_counts.surfaces_considered = PVS_FACE_COUNT as u32;

                for gi in 0..PVS_GROUP_COUNT {
                    let group = PVS_GROUP_ACTIVE[gi] as usize;
                    if dot12(PVS_GROUP_NRM[group], eye) <= PVS_GROUP_DIST[group] {
                        continue;
                    }
                    let mut entry = PVS_GROUP_FIRST[group];
                    while entry != PVS_LINK_END {
                        let e = entry as usize;
                        let bc16 = PVS_FACE_CENTER[e];
                        let be16 = PVS_FACE_EXTENT[e];
                        let bc = [bc16[0] as i32, bc16[1] as i32, bc16[2] as i32];
                        let be = [be16[0] as i32, be16[1] as i32, be16[2] as i32];
                        if !WORLD_BOUNDS_CULL || box_visible(bc, be, &rot, base_t) {
                            let cnt = PVS_FACE_TRI_COUNT[e] as usize;
                            emit_world_face_tris(
                                &mut packets,
                                &m,
                                PVS_FACE_FIRST[e] as usize,
                                cnt,
                                nv,
                                frame_no,
                                &mut np,
                                &mut nq,
                                &mut room_counts,
                            );
                        }
                        entry = PVS_FACE_NEXT[e];
                    }
                }

                telemetry::counter(
                    telemetry::counter::ROOM_CELLS_CONSIDERED,
                    room_counts.cells_considered,
                );
                telemetry::counter(
                    telemetry::counter::ROOM_CELLS_DRAWN,
                    room_counts.cells_drawn,
                );
                telemetry::counter(telemetry::counter::ROOM_CELLS_CULLED, 0);
                telemetry::counter(
                    telemetry::counter::ROOM_SURFACES_CONSIDERED,
                    room_counts.surfaces_considered,
                );
                telemetry::counter(
                    telemetry::counter::ROOM_SURF_PROFILED,
                    room_counts.emit_calls,
                );
            } else {
                let draw_token = next_draw_face_mark_token();
                let mut room_counts = WorldCounters::new();
                let mut face = 0usize;
                while face < m.n_faces {
                    emit_world_face(
                        &mut packets,
                        &m,
                        face,
                        nv,
                        frame_no,
                        eye,
                        &rot,
                        base_t,
                        &mut np,
                        &mut nq,
                        &mut room_counts,
                        draw_token,
                    );
                    face += 1;
                }
                telemetry::counter(telemetry::counter::ROOM_CELLS_CONSIDERED, 0);
                telemetry::counter(telemetry::counter::ROOM_CELLS_DRAWN, 0);
                telemetry::counter(telemetry::counter::ROOM_CELLS_CULLED, 0);
                telemetry::counter(
                    telemetry::counter::ROOM_SURFACES_CONSIDERED,
                    room_counts.surfaces_considered,
                );
                telemetry::counter(
                    telemetry::counter::ROOM_SURF_PROFILED,
                    room_counts.emit_calls,
                );
            }
            telemetry::stage_end(telemetry::stage::ROOM);

            telemetry::stage_begin(telemetry::stage::MODEL_INSTANCES);
            let model_prims0 = np;
            let mut model_draws = 0u32;
            let mut model_bounds_tests = 0u32;
            let mut model_bounds_culled = 0u32;
            let mut model_culled_tris = 0u32;
            let mut model_projected_vertices = 0u32;

            // Brush entities: doors slide open near the player. Each renders with
            // a per-entity GTE translation (base view shifted by the offset);
            // its few tris are projected fresh (not from the world cache).
            let brush_iter_count = if have_pvs { PVS_ENT_COUNT } else { nents };
            for bi in 0..brush_iter_count {
                let ei = if have_pvs { PVS_ENTS[bi] as usize } else { bi };
                let e = ENT_CACHE[ei];
                let off = if e.kind == 1 {
                    scale12_vec(e.mv, ENT_PHASE[ei])
                } else {
                    e.origin
                };
                if e.r2 > 0 {
                    model_bounds_tests = model_bounds_tests.saturating_add(1);
                    let radius = ENT_RADIUS[ei];
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
                let submodel_token = next_submodel_draw_token();
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
                        let tri = m.render_tri(tt, cached_world_uv_words(&m, tt));
                        let (a, b, c) = (
                            tri.idx[0] as usize,
                            tri.idx[1] as usize,
                            tri.idx[2] as usize,
                        );
                        if a < nv && b < nv && c < nv {
                            proj_submodel_vert(&m, a, submodel_token);
                            proj_submodel_vert(&m, b, submodel_token);
                            proj_submodel_vert(&m, c, submodel_token);
                            emit_projected(
                                &mut packets,
                                &m,
                                tri,
                                [SCRATCH[a], SCRATCH[b], SCRATCH[c]],
                                nv,
                                &mut np,
                            );
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
                let submodel_token = next_submodel_draw_token();
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
                        let tri = m.render_tri(tt, cached_world_uv_words(&m, tt));
                        let (a, b, c) = (
                            tri.idx[0] as usize,
                            tri.idx[1] as usize,
                            tri.idx[2] as usize,
                        );
                        if a < nv && b < nv && c < nv {
                            proj_submodel_vert(&m, a, submodel_token);
                            proj_submodel_vert(&m, b, submodel_token);
                            proj_submodel_vert(&m, c, submodel_token);
                            emit_projected(
                                &mut packets,
                                &m,
                                tri,
                                [SCRATCH[a], SCRATCH[b], SCRATCH[c]],
                                nv,
                                &mut np,
                            );
                        }
                    }
                }
            }

            // Studio actors placed by map entities plus any runtime fallback
            // enemies. Models are baked to posed frames by the host cooker; the
            // PS1 path only chooses the current actor frame.
            let actor_count = PROP_COUNT.min(MAX_PROPS);
            for pi in 0..actor_count {
                let ty = PROP_KIND[pi];
                let org = PROP_POS[pi];
                let yaw = PROP_YAW[pi];
                let cooked_leaf = PROP_LEAF[pi];
                let (md, slots, faces, face_count) = match ty {
                    PROP_TYPE_SCIENTIST => (
                        sci,
                        &SCI_SLOTS[..],
                        core::ptr::addr_of!(SCI_FACES).cast::<ModelRenderFace>(),
                        SCI_FACE_COUNT,
                    ),
                    PROP_TYPE_BARNEY => (
                        barney,
                        &BARNEY_SLOTS[..],
                        core::ptr::addr_of!(BARNEY_FACES).cast::<ModelRenderFace>(),
                        BARNEY_FACE_COUNT,
                    ),
                    PROP_TYPE_HEADCRAB => (
                        headcrab,
                        &HEADCRAB_SLOTS[..],
                        core::ptr::addr_of!(HEADCRAB_FACES).cast::<ModelRenderFace>(),
                        HEADCRAB_FACE_COUNT,
                    ),
                    _ => continue,
                };
                model_bounds_tests = model_bounds_tests.saturating_add(1);
                if have_pvs {
                    let prop_leaf = if ty == PROP_TYPE_HEADCRAB {
                        camera_leaf(&m, org)
                    } else if cooked_leaf > 0 {
                        cooked_leaf as i32
                    } else {
                        camera_leaf(&m, org)
                    };
                    if prop_leaf <= 0 || !pvs_leaf_visible(&m, prop_leaf as usize) {
                        model_bounds_culled = model_bounds_culled.saturating_add(1);
                        continue;
                    }
                }
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
                let vy = dot12(rot.m[1], org) + base_t[1];
                if vy.abs() * 4 > vz * 3 + 360 {
                    model_bounds_culled = model_bounds_culled.saturating_add(1);
                    continue;
                }
                model_draws = model_draws.saturating_add(1);
                let sf = prop_anim_frame(md, ty, PROP_STATE[pi], sim_frame_no, pi);
                let shade = if pi < MAX_PROPS && PROP_HIT_FLASH[pi] > 0 {
                    MODEL_HIT_SHADE
                } else {
                    MODEL_SHADE
                };
                model_projected_vertices =
                    model_projected_vertices.saturating_add(md.n_verts.min(MAX_MODEL_VERTS) as u32);
                draw_model(
                    &mut packets,
                    md,
                    slots,
                    faces,
                    face_count,
                    org,
                    yaw,
                    sf,
                    shade,
                    eye,
                    &rot,
                    &mut np,
                );
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
            telemetry::counter(
                telemetry::counter::MODEL_INSTANCE_PROJECTED_VERTICES,
                model_projected_vertices,
            );

            let world_prims = np;
            let world_quads = nq;
            if SHOW_VIEWMODEL {
                telemetry::stage_begin(telemetry::stage::EQUIPMENT);
                draw_viewmodel(
                    &mut packets,
                    &wpn,
                    &WEAPON_SLOTS,
                    VM_FRAME,
                    -recoil,
                    &mut np,
                );
                telemetry::stage_end(telemetry::stage::EQUIPMENT);
                telemetry::counter(
                    telemetry::counter::EQUIPMENT_SUBMITTED_TRIS,
                    np.saturating_sub(world_prims) as u32,
                );
            }
            let _ = hud::draw(hud_mat, health, ammo, &mut HUD_OT, &mut HUD_PRIMS);

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
            telemetry::counter(telemetry::counter::TRI_PRIMITIVES, (np + nq) as u32);
            telemetry::counter(
                telemetry::counter::WORLD_COMMANDS,
                (world_prims + world_quads) as u32,
            );
            telemetry::counter(
                telemetry::counter::ROOM_SURF_SPLIT_TRIS,
                model_prims0 as u32,
            );
            telemetry::counter(
                telemetry::counter::ROOM_SURF_WHOLE_QUADS,
                world_quads as u32,
            );
            telemetry::counter(
                telemetry::counter::TRI_PRIMITIVE_REMAINING,
                packets.remaining() as u32,
            );
            telemetry::counter(
                telemetry::counter::ROOM_SUBMIT_PRIMITIVE_OVERFLOWS,
                (packets.remaining() == 0) as u32,
            );
        }

        telemetry::stage_end(telemetry::stage::RENDER);
        telemetry::stage_begin(telemetry::stage::PRESENT);
        let present_vblank = wait_vblank_edge();
        fb.swap();
        telemetry::stage_end(telemetry::stage::PRESENT);
        let lateness_vblanks = if vblank_reached(present_vblank, next_sim_vblank) {
            present_vblank
                .wrapping_sub(next_sim_vblank)
                .min(u16::MAX as u32) as u16
        } else {
            0
        };
        telemetry::counter(telemetry::counter::VISUAL_FRAMES, 1);
        if lateness_vblanks > 0 {
            telemetry::counter(telemetry::counter::VISUAL_DEADLINE_MISSES, 1);
        }
        telemetry::counter(
            telemetry::counter::VISUAL_MAX_LATENESS_VBLANKS,
            lateness_vblanks as u32,
        );
        telemetry::task_end(telemetry::task::VISUAL_RENDER);
    }
}

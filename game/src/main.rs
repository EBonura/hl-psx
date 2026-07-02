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
mod hltext;
mod hud;
mod map;
mod menu;
mod model;
mod phys;
mod render;
mod sfx;
mod telemetry;
mod vram;

mod room_budget {
    include!(concat!(env!("OUT_DIR"), "/room_budget.rs"));
}

use psx_fx::{LcgRng, ParticlePool};
use psx_gpu::material::{BlendMode, TexturedGouraudPacketMaterial};
use psx_gpu::ot::OrderingTable;
use psx_gpu::prim::{QuadTexturedGouraud, RectFlat, TriTexturedGouraud};
use psx_gpu::{self as gpu, framebuf::FrameBuffer, Resolution, VideoMode};
use psx_gte::math::{Mat3I16, Vec3I32};
use psx_gte::scene::{self, Projected};
use psx_pad::{button, enable_analog_port1, poll_port1};
use psx_rt::{interrupts, tty};

use map::{Map, SKY_TEX_NONE};
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
// NPC/item geometry is no longer baked into the EXE: every model type streams
// from WORLD.PAK per-map into MODEL_BUF via the model pool (see stream_map_models).

// World ordering table. sz (view depth) tops out near FAR_VIEW; otz = sz>>OT_SHIFT
// indexes the table, back-to-front. OT_SHIFT=4 (16-unit buckets) keeps the depth
// sort fine (at >>6 only ~10% of the OT was used and far geometry tie-broke
// arbitrarily). The GPU submit DMA walks the WHOLE chain every frame -- every
// empty slot is a forwarding link it still reads -- so an oversized table is pure
// per-frame DMA cost. With FAR_VIEW=1000, otz tops out at 1000>>4=62; OT_LEN=512
// covers that ~8x over (otz<512 => sz<8176, far beyond anything emitted) while
// dropping the empty-slot walk ~4x and saving 6 KB RAM vs the old 2048. Raise it
// only if FAR_VIEW grows past 512<<OT_SHIFT.
const OT_LEN: usize = 512;
const OT_SHIFT: u32 = 4;
const WEAPON_OT_LEN: usize = 64;
const HUD_OT_LEN: usize = 1;
const FX_OT_LEN: usize = 1;
const MAX_VERTS: usize = 12288; // covers the biggest campaign map (room_140 = 11664 verts)
const MAX_MODEL_VERTS: usize = 1024;
const SCI_FACE_CAP: usize = 768;
const BARNEY_FACE_CAP: usize = 800;
const HEADCRAB_FACE_CAP: usize = 512;
const SUIT_ITEM_FACE_CAP: usize = 448;
const BATTERY_ITEM_FACE_CAP: usize = 160;
#[cfg(not(feature = "emulator-telemetry"))]
const MAX_RENDER_PACKETS: usize = 2560;
#[cfg(feature = "emulator-telemetry")]
const MAX_RENDER_PACKETS: usize = 2304;
const MAX_WEAPON_CACHE_TRIS: usize = 320;
const MAX_TEX_SLOTS: usize = room_budget::MAX_TEX_SLOTS;
const MAX_FACES: usize = room_budget::MAX_FACES;
const MAX_FACE_GROUPS: usize = room_budget::MAX_FACE_GROUPS;
const MAX_LEAVES: usize = room_budget::MAX_LEAVES;
const MAX_ENTS: usize = room_budget::MAX_ENTS;
const MAX_PVS_FACE_RECS: usize = 2048;
const MAX_PROPS: usize = 128;
const MAX_NAV_NODES: usize = 255;
const MAX_LOGIC: usize = 384;
const MAX_LOGIC_EVENTS: usize = 64;
const NEAR: u16 = 2; // GTE depth: only verts at/behind the near plane take the soft-clip path
const SUBDIV_PX: i32 = 96; // split near-clipped triangles wider than this (affine fix)
const SUBDIV_DEPTH: u8 = 0; // ponytail: subdivision off (perf); affine warp accepted
const CULL: bool = true; // backface cull (keep area > 0; winding verified)
const H_PROJ: u16 = 160; // ~90 deg horizontal FOV at 320px
const WORLD_QUAD_PAIRING: bool = true; // pair two-triangle BSP quads when safe
const WORLD_BOUNDS_CULL: bool = true; // face AABB frustum test before projection
// Push solid backdrop walls (the `black` texture) this many OT buckets toward the
// back so foreground detail that is nearly coplanar with them always wins the
// painter's-order tie (no Z-buffer). A backdrop has nothing behind it, so the
// bias is one-directional and safe.
const BACKDROP_OTZ_BIAS: usize = 4;

const PITCH_MAX: i16 = 1000;
const YAW_RATE: i32 = 130; // yaw units/frame at full stick (Q0.12)
const PITCH_RATE: i32 = 95; // pitch units/frame at full stick
const DEADZONE: i32 = 28; // radial stick deadzone
const VIEW_HEIGHT: i32 = 28;
const PLAYER_USE_REACH: i32 = 96;
const PLAYER_TOUCH_HALF_XZ: i32 = 16;
const PLAYER_TOUCH_HEIGHT: i32 = 56;
// Distance cull on world face centers (sphere_visible). HL maps are enclosed, so
// every spawn sightline terminates (corner/door/dark) well within 2000 units; the
// conservative PVS still flags far geometry as potentially visible and the renderer
// processes it for no visible pixel. Culling past 2000 reclaims that: ~30%
// room_surface_draw at the c0a0 spawn, pixel-identical to the old 16000 across
// c0a0/c1a0/c1a1/c1a2/c1a3/c1a4 spawns. The depth fog below dissolves the cut
// edge, so this can sit closer than the old 2000/1400 for fps on open maps.
// VISUAL/FPS KNOB: 1000 measured 14.3->17.9 fps on c2a5 (Surface Tension,
// GPU-fill-bound) vs 1400, with indoor maps unchanged (short sightlines) and
// the vista still faithful (sky is fog-exempt). Raise toward 1400+ for longer
// sightlines, lower toward 800 for more open-map fps (hazier).
const FAR_VIEW: i32 = 1000;
// Distance fog. World geometry fades to black between FOG_START and FAR_VIEW so
// the far-cull edge dissolves instead of popping -- which lets FAR_VIEW sit much
// closer than the old 2000 (distant geometry is a large share of the per-frame
// triangle work; pulling the cull in is the biggest fps lever on open maps).
// Sky/backdrop faces are exempt so the horizon stays. Reciprocal is compile-time
// (no runtime divide). Raise FOG_START toward FAR_VIEW for a lighter haze (more
// visible cull), lower it for more fps. Kept at ~0.65*FAR_VIEW.
const FOG_START: i32 = 650;
const FOG_INV: i32 = (256i32 << 12) / (FAR_VIEW - FOG_START); // compile-time
// Studio models (enemies/NPCs/items) cull no farther than the world: each is
// hundreds of textured-gouraud tris (project + emit), and an actor beyond the
// world cull would float against culled void. Capped at FAR_VIEW (was a stale
// 1600 that exceeded the pulled-in FAR_VIEW, drawing enemies past the world for
// wasted CPU). On enemy-laden maps the far roster is the dominant per-frame CPU
// cost for little visible detail. Tunable: lower for more headroom, but not
// above FAR_VIEW.
const MODEL_FAR: i32 = 1000;
const MODEL_CULL: bool = true; // backface-cull studio models
const MODEL_OCCLUSION_CULL: bool = true; // skip actors fully hidden by static BSP
const MODEL_SHADE: u8 = 110; // flat model tint (dimmer than 128 to match the lit world)
// Blend liquid surfaces (water/toxic/fluid) instead of drawing them opaque. OFF:
// with no underwater scene drawn behind the surface, an Average blend goes near-
// black over the void below a liquid brush. Needs real underwater rendering to
// look right; opaque liquids stay visible in the meantime.
const LIQUID_TRANSPARENCY: bool = false;
const DBG_MODEL_SHOWCASE: bool = false; // debug: line up loaded enemy models in front of the camera
const DBG_PAD_BOOT: bool = cfg!(feature = "debug-map-boot"); // hold L1 | map_index to boot any map headlessly
// Debug: pin the camera to a fixed pose (to reproduce a specific view headlessly).
const DBG_CAM: bool = false;
const DBG_CAM_POS: [i32; 3] = [-624, -184, -160];
const DBG_CAM_YAW: u16 = 1024;
const DBG_CAM_PITCH: i16 = 0;
const MODEL_HIT_SHADE: u8 = 180; // brief flash when the player lands a shot
const SIM_VBLANKS: u32 = 3; // 60 Hz NTSC / 3 = 20 Hz gameplay tick
const ROOM_WORLD_CHUNK_MUL: u32 = 2;
const ROOM_TEXTURE_CHUNK_ADD: u32 = 1;
const LANDMARK_NAME_MAX: usize = 31;
const SKY_FACE_COUNT: usize = 6;
const SKY_TEX_SIZE: usize = 128;
const MODEL_CHUNK_V_9MMHANDGUN: u32 = 1000;
const MODEL_CHUNK_SCIENTIST_TEX: u32 = 1100;
const MODEL_CHUNK_BARNEY_TEX: u32 = 1101;
const MODEL_CHUNK_HEADCRAB_TEX: u32 = 1102;
const MODEL_CHUNK_SUIT_ITEM_TEX: u32 = 1200;
const MODEL_CHUNK_BATTERY_ITEM_TEX: u32 = 1201;
const MODEL_CHUNK_V_9MMHANDGUN_TEX: u32 = 2000;
const GLOCK_MAX_CLIP: u16 = 17; // glock magazine; also the initial-launch clip cap
const GLOCK_START_RESERVE: u16 = 35; // 9mm reserve at game start
const GLOCK_RANGE: i32 = 8192; // shared hitscan reach for the ballistic weapons
const GLOCK_AIM_PIX_X: i32 = 22; // hitscan aim-cone half-width (screen px)
const GLOCK_AIM_PIX_Y: i32 = 34;
const GLOCK_EMPTY_COOLDOWN_TICKS: u8 = 4; // dry-click cadence (0.2s)
const PROP_TARGET_HEIGHT: i32 = 40;
const SCIENTIST_RENDER_RADIUS: i32 = 72;
const BARNEY_RENDER_RADIUS: i32 = 72;
const HEADCRAB_RENDER_RADIUS: i32 = 36;
const ITEM_RENDER_RADIUS: i32 = 36;
const PROP_TYPE_SCIENTIST: u8 = 0;
const PROP_TYPE_BARNEY: u8 = 1;
const PROP_TYPE_HEADCRAB: u8 = 2;
const PROP_TYPE_ITEM_SUIT: u8 = 3;
const PROP_TYPE_ITEM_BATTERY: u8 = 4;
const PROP_TYPE_CONTROLLER: u8 = 11; // flies: exempt from walker floor checks
const PROP_TYPE_SITTING_SCI: u8 = 25; // seated pose, keeps its authored chair height
const PROP_DEAD_BIT: u16 = 0x8000; // cook flag: spawn as a corpse (death pose, 0 hp)
const PROP_TYPE_WEAPON_FIRST: u8 = 26; // weapon pickups 26..=39 (index - 26 = weapon id)
const PROP_TYPE_WEAPON_LAST: u8 = 39;
const PROP_TYPE_AMMO_FIRST: u8 = 40; // ammo pickups 40..=47
const PROP_TYPE_AMMO_LAST: u8 = 47;
const PROP_TYPE_MEDKIT: u8 = 48;
// (ammo pool, rounds) per ammo pickup type 40..=47.
const AMMO_PICKUPS: [(usize, u16); 8] = [
    (AMMO_9MM, 17),
    (AMMO_9MM, 50),
    (AMMO_BUCK, 12),
    (AMMO_357, 6),
    (AMMO_BOLT, 5),
    (AMMO_ROCKET, 1),
    (AMMO_URANIUM, 20),
    (AMMO_GREN, 2),
];
const MEDKIT_HEAL: u16 = 15;
const PROP_STATE_IDLE: u8 = 0;
const PROP_STATE_MOVE: u8 = 1;
const PROP_STATE_ATTACK: u8 = 2;
const PROP_STATE_DEAD: u8 = 3;

// ---- per-map model pool registry ----
// 26 model types (the cook's collect_props ids). Each streams from WORLD.PAK:
// geometry chunk `1300+id`, texture chunk `1100+id`. The runtime keeps only the
// types a map places resident (TYPE_TO_SLOT -> LOADED_MODELS).
const N_MODEL_TYPES: usize = 49;
const MAX_LOADED_MODELS: usize = 22; // distinct model types resident per map (enemies + pickups)
const POOL_TEX_SLOTS: usize = 240; // shared TexSlot pool across loaded models
const POOL_FACE_CAP: usize = 5248; // shared RenderFace pool (worst per-map tri sum, c4a3)
const MODEL_SLOT_NONE: u8 = 0xFF;
const MODEL_GEOM_CHUNK_BASE: u32 = 1300;
const MODEL_TEX_CHUNK_BASE: u32 = 1100;
const AI_ITEM: u8 = 0; // static pickup
const AI_FLEE: u8 = 1; // scientist
const AI_ALLY: u8 = 2; // barney
const AI_MELEE: u8 = 3; // approach + bite (headcrab and friends)
const AI_IDLE: u8 = 4; // render only (flyers/ceiling/bosses until they get real AI)
const AI_RANGED: u8 = 5; // approach to range, then fire (grunts, vorts, agrunt, controller)
const AI_TURRET: u8 = 6; // stationary: rotate to face + fire (sentry/turret/miniturret)

#[derive(Clone, Copy)]
struct ModelDef {
    health: u8,
    target_h: i32,
    radius: i32,
    ai: u8,
    speed: u8,        // move speed (world units/tick); 0 = stationary
    atk_range: u16,   // attack engage distance (world units)
    atk_damage: u8,   // HP per hit
    atk_cooldown: u8, // ticks between attacks (20 Hz)
}
const fn mdef(health: u8, target_h: i32, radius: i32, ai: u8) -> ModelDef {
    ModelDef {
        health,
        target_h,
        radius,
        ai,
        speed: 0,
        atk_range: 0,
        atk_damage: 0,
        atk_cooldown: 0,
    }
}
const fn mdef_atk(
    health: u8,
    target_h: i32,
    radius: i32,
    ai: u8,
    speed: u8,
    atk_range: u16,
    atk_damage: u8,
    atk_cooldown: u8,
) -> ModelDef {
    ModelDef {
        health,
        target_h,
        radius,
        ai,
        speed,
        atk_range,
        atk_damage,
        atk_cooldown,
    }
}
// Combat params (speed/range/damage/cooldown) for AI_RANGED + AI_TURRET come from
// the per-enemy MDL/Half-Life survey; melee/idle/passive types ignore them.
const MODEL_DEFS: [ModelDef; N_MODEL_TYPES] = [
    mdef(SCIENTIST_HEALTH, 40, SCIENTIST_RENDER_RADIUS, AI_FLEE), // 0 scientist
    mdef(BARNEY_HEALTH, 40, BARNEY_RENDER_RADIUS, AI_ALLY),       // 1 barney
    mdef(HEADCRAB_HEALTH, 12, HEADCRAB_RENDER_RADIUS, AI_MELEE),  // 2 headcrab
    mdef(0, 16, ITEM_RENDER_RADIUS, AI_ITEM),                     // 3 item_suit
    mdef(0, 16, ITEM_RENDER_RADIUS, AI_ITEM),                     // 4 item_battery
    mdef(50, 40, 90, AI_MELEE),                                  // 5 zombie
    mdef(20, 20, 70, AI_MELEE),                                  // 6 houndeye
    mdef(40, 32, 90, AI_MELEE),                                  // 7 bullsquid
    mdef_atk(50, 40, 90, AI_RANGED, 16, 1000, 5, 8),            // 8 hgrunt (mp5 bursts)
    mdef_atk(30, 40, 90, AI_RANGED, 15, 800, 10, 24),           // 9 alien_slave (zap)
    mdef_atk(60, 48, 100, AI_RANGED, 16, 1000, 8, 16),          // 10 alien_grunt (hornets)
    mdef_atk(60, 40, 100, AI_RANGED, 16, 1024, 3, 14),          // 11 alien_controller (energy)
    mdef(40, 32, 90, AI_IDLE),                                   // 12 barnacle (ceiling: render only)
    mdef(16, 8, 40, AI_IDLE),                                    // 13 leech (flyer: render only)
    mdef(6, 4, 30, AI_IDLE),                                     // 14 cockroach (passive)
    mdef(30, 48, 90, AI_IDLE),                                   // 15 gman (passive)
    mdef(200, 90, 220, AI_IDLE),                                 // 16 gargantua (boss: render only)
    mdef(200, 90, 240, AI_IDLE),                                 // 17 nihilanth (boss: render only)
    mdef(150, 70, 200, AI_IDLE),                                 // 18 bigmomma (boss: render only)
    mdef(40, 20, 90, AI_MELEE),                                  // 19 ichthyosaur
    mdef_atk(40, 40, 80, AI_TURRET, 0, 1000, 7, 8),            // 20 sentry
    mdef_atk(50, 40, 80, AI_TURRET, 0, 1200, 8, 7),            // 21 turret
    mdef_atk(30, 30, 60, AI_TURRET, 0, 1000, 5, 3),            // 22 miniturret
    mdef(80, 60, 150, AI_IDLE),                                 // 23 apache (flyer: render only)
    mdef(10, 20, 60, AI_IDLE),                                  // 24 flyer_flock (passive)
    mdef(SCIENTIST_HEALTH, 25, SCIENTIST_RENDER_RADIUS, AI_IDLE), // 25 sitting scientist
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 26 weapon_crowbar
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 27 weapon_9mmhandgun
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 28 weapon_357
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 29 weapon_9mmAR
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 30 weapon_shotgun
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 31 weapon_crossbow
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 32 weapon_rpg
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 33 weapon_gauss
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 34 weapon_egon
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 35 weapon_hornetgun
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 36 weapon_handgrenade
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 37 weapon_snark
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 38 weapon_tripmine
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 39 weapon_satchel
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 40 ammo_9mmclip
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 41 ammo_9mmAR
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 42 ammo_buckshot
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 43 ammo_357
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 44 ammo_crossbow
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 45 ammo_rpgclip
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 46 ammo_gaussclip
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 47 ammo_ARgrenades
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM), // 48 item_healthkit
];

#[inline]
fn model_def(ty: u8) -> ModelDef {
    MODEL_DEFS[(ty as usize).min(N_MODEL_TYPES - 1)]
}

#[derive(Clone, Copy)]
struct LoadedModel {
    valid: bool,
    type_id: u8,
    geom_off: usize, // byte offset into MODEL_BUF
    geom_len: usize,
    face_start: usize, // index into POOL_FACES
    n_faces: usize,
    tex_start: usize, // index into POOL_TEX
    n_tex: usize,
}
impl LoadedModel {
    const ZERO: Self = Self {
        valid: false,
        type_id: 0,
        geom_off: 0,
        geom_len: 0,
        face_start: 0,
        n_faces: 0,
        tex_start: 0,
        n_tex: 0,
    };
}
const PROP_CLIP_IDLE: usize = 0;
const PROP_CLIP_MOVE: usize = 1;
const PROP_CLIP_ATTACK: usize = 2;
const PROP_CLIP_PAIN: usize = 3;
const PROP_CLIP_DEAD: usize = 4;
const PLAYER_START_HEALTH: u16 = 100;
const DEATH_TICKS: u8 = 60; // frozen "you died" window before respawn (3s at 20 Hz)
const PLAYER_START_ARMOR: u16 = 0;
const HEV_MAX_ARMOR: u16 = 100;
const CHARGER_RATE: u16 = 4; // health/armor points per use pulse (8-tick cadence)
const HEV_BATTERY_ARMOR: u16 = 15;
const HEV_PICKUP_TICKS: u8 = 36;
const ITEM_TOUCH_RANGE2: i32 = 38 * 38;
const ITEM_TOUCH_HEIGHT: i32 = 56;
const PROP_LINK_MATCH_XZ_EPS: i32 = 24;
const PROP_LINK_MATCH_Y_EPS: i32 = 96;
const PROP_GROUND_PROBE_UP: i32 = 24;
const PROP_GROUND_PROBE_DOWN: i32 = 160;
const GROUND_SCAN_STEP: i32 = 8;
const SCIENTIST_HEALTH: u8 = 24;
const BARNEY_HEALTH: u8 = 35;
const HEADCRAB_HEALTH: u8 = 16;
const HEADCRAB_SPEED: i32 = 5;
const HEADCRAB_WAKE_RANGE2: i32 = 1300 * 1300;
const HEADCRAB_STOP_RANGE: i32 = 34;
const HEADCRAB_LEAP_RANGE2: i32 = 256 * 256;
const HEADCRAB_ATTACK_DAMAGE: u16 = 6;
const HEADCRAB_ATTACK_COOLDOWN: u8 = 40;
const HEADCRAB_ATTACK_TICKS: u8 = 12;
const HEADCRAB_ATTACK_IMPACT_TICK: u8 = 5;
const HEADCRAB_LEAP_SPEED: i32 = 18;
const HEADCRAB_BITE_RANGE2: i32 = 48 * 48;
const HEADCRAB_TARGET_HEIGHT: i32 = 12;
const BARNEY_ATTACK_RANGE2: i32 = 1024 * 1024;
const BARNEY_ATTACK_COOLDOWN: u8 = 9;
const BARNEY_ATTACK_TICKS: u8 = 5;
const BARNEY_DAMAGE: u8 = 8;
const BARNEY_SPEED: i32 = 7;
const BARNEY_FOLLOW_RANGE2: i32 = 640 * 640;
const BARNEY_STOP_RANGE2: i32 = 128 * 128;
const SCIENTIST_FEAR_RANGE2: i32 = 896 * 896;
const SCIENTIST_FEAR_TICKS: u8 = 80;
const SCIENTIST_FLEE_SPEED: i32 = 7;
const SCIENTIST_FACE_RANGE2: i32 = 192 * 192;
const PROP_HIT_FLASH_TICKS: u8 = 4;
const PROP_TARGET_NONE: u8 = 254;
const PROP_TARGET_PLAYER: u8 = 255;
const NAV_NODE_NONE: u8 = 255;
const NAV_NEAREST_RANGE2: i32 = 1024 * 1024;
const NAV_NODE_REACHED_RANGE2: i32 = 48 * 48;
const NAV_VERTICAL_MAX: i32 = 160;
const NAV_FLEE_RANGE2: i32 = 1200 * 1200;
const MAX_IMPACT_MARKS: usize = 24;
const MAX_IMPACT_PARTICLES: usize = 64;
const IMPACT_MARK_TICKS: u8 = 180;
const IMPACT_KIND_WORLD: u8 = 0;
const IMPACT_KIND_BLOOD: u8 = 1;

static mut OT: OrderingTable<OT_LEN> = OrderingTable::new();
static mut WEAPON_OT: OrderingTable<WEAPON_OT_LEN> = OrderingTable::new();
static mut HUD_OT: OrderingTable<HUD_OT_LEN> = OrderingTable::new();
static mut FX_OT: OrderingTable<FX_OT_LEN> = OrderingTable::new();
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
static mut IMPACT_PARTICLES: ParticlePool<MAX_IMPACT_PARTICLES> = ParticlePool::new();
static mut IMPACT_RNG: LcgRng = LcgRng::new(0x484c_5058);
static mut IMPACT_PARTICLE_RECTS: [RectFlat; MAX_IMPACT_PARTICLES] =
    [const { RectFlat::new(0, 0, 0, 0, 0, 0, 0) }; MAX_IMPACT_PARTICLES];
static mut IMPACT_MARK_RECTS: [RectFlat; MAX_IMPACT_MARKS] =
    [const { RectFlat::new(0, 0, 0, 0, 0, 0, 0) }; MAX_IMPACT_MARKS];
static mut DEATH_OVERLAY: RectFlat = RectFlat::new(0, 0, 0, 0, 0, 0, 0);
static mut TEX_SLOTS: [TexSlot; MAX_TEX_SLOTS] = [EMPTY_SLOT; MAX_TEX_SLOTS];
// Resident viewmodel pool: ALL weapons load at map start so switching is instant
// and every weapon shows its real model. Geometry occupies the head of MODEL_BUF
// (the lightmap + face-loop compression freed the map budget to afford this big a
// reserve); enemies stream after it, trading a slice of the enemy pool for the
// full visible arsenal. Textures go in VM_SLOTS. The loader stops if the pool
// fills; any weapon that doesn't fit falls back to the glock viewmodel.
const VM_POOL_WORDS: usize = 46_720; // ~183 KB head reserve of MODEL_BUF (all 14)
const VM_SLOTS_TOTAL: usize = 176; // 14 viewmodels x up to ~24 skins (rpg)
static mut VM_SLOTS: [TexSlot; VM_SLOTS_TOTAL] = [EMPTY_SLOT; VM_SLOTS_TOTAL];
// Load order = the HL1 slot order; the whole arsenal is resident.
const VM_RESIDENT: [usize; N_WEAPONS] = [
    W_CROWBAR, W_GLOCK, W_357, W_MP5, W_SHOTGUN, W_CROSSBOW, W_RPG, W_GAUSS, W_EGON, W_HORNET,
    W_GRENADE, W_SNARK, W_TRIPMINE, W_SATCHEL,
];

#[derive(Clone, Copy)]
struct VmEntry {
    valid: bool,
    geom_off: usize,
    geom_len: usize,
    slot_start: usize,
    n_slots: usize,
}
impl VmEntry {
    const NONE: VmEntry = VmEntry {
        valid: false,
        geom_off: 0,
        geom_len: 0,
        slot_start: 0,
        n_slots: 0,
    };
}
static mut VM_ENTRY: [VmEntry; N_WEAPONS] = [VmEntry::NONE; N_WEAPONS];
// Fill cursor into the viewmodel pool. Only the spawn weapon streams at map
// load; the rest append here on first switch (stream_one_viewmodel), so a load
// streams ~1 viewmodel instead of all 14. No eviction -- the pool is sized for
// the whole arsenal, so every weapon still ends up resident once first drawn.
static mut VM_FILL_WORD: usize = 0;
static mut VM_FILL_SLOT: usize = 0;

// Shared per-map model pool: streamed geometry lives in MODEL_BUF (after the
// viewmodel); textures, render faces, and slot bookkeeping live in these pools.
static mut POOL_TEX: [TexSlot; POOL_TEX_SLOTS] = [EMPTY_SLOT; POOL_TEX_SLOTS];
static mut POOL_FACES: [ModelRenderFace; POOL_FACE_CAP] =
    [ModelRenderFace::ZERO; POOL_FACE_CAP];
static mut LOADED_MODELS: [LoadedModel; MAX_LOADED_MODELS] =
    [LoadedModel::ZERO; MAX_LOADED_MODELS];
static mut TYPE_TO_SLOT: [u8; N_MODEL_TYPES] = [MODEL_SLOT_NONE; N_MODEL_TYPES];

#[derive(Clone, Copy, PartialEq, Eq)]
enum PauseExit {
    Resume,
    MainMenu,
}

const PAUSE_ITEMS: [&str; 2] = ["Resume", "Main Menu"];
const PAUSE_WHITE: (u8, u8, u8) = (238, 238, 230);
const PAUSE_AMBER: (u8, u8, u8) = (255, 170, 0);
const PAUSE_DIM: (u8, u8, u8) = (96, 96, 92);
const PAUSE_ARMED: (u8, u8, u8) = (75, 53, 10);
// cli-spinners "dots" (the classic braille terminal spinner ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏),
// drawn as its 2x3 dot grid since the bitmap font has no braille glyphs. Each
// entry is the low 6 braille bits: bit0..2 = left column top->bottom, bit3..5 =
// right column top->bottom. A rotating subset lights each frame.
const SPINNER_DOTS: [u8; 10] = [
    0x0B, 0x19, 0x39, 0x38, 0x3C, 0x34, 0x26, 0x27, 0x07, 0x0F,
];

/// Draw one "dots" spinner frame as a 2x3 grid of small dots at (x, y).
fn draw_spinner_dots(x: i16, y: i16, frame: u8) {
    let mask = SPINNER_DOTS[(frame as usize) % SPINNER_DOTS.len()];
    const D: i16 = 3; // dot size
    const G: i16 = 5; // dot spacing
    let mut i = 0;
    while i < 6 {
        let col = (i / 3) as i16;
        let row = (i % 3) as i16;
        let dx = x + col * G;
        let dy = y + row * G;
        let (r, g, b) = if mask & (1 << i) != 0 {
            (255, 214, 96) // lit: bright HEV amber
        } else {
            (40, 34, 20) // unlit: dim, so the grid reads as a spinner
        };
        gpu::draw_quad_flat([(dx, dy), (dx + D, dy), (dx, dy + D), (dx + D, dy + D)], r, g, b);
        i += 1;
    }
}

fn loading_label_for_room(room_id: u16) -> &'static str {
    let idx = room_id as usize;
    if idx < menu::MAPS.len() {
        menu::MAPS[idx]
    } else {
        "room"
    }
}

// Fresh load from the menu: a full loading card on a cleared buffer, then swap.
fn draw_loading_card(fb: &mut FrameBuffer, label: &str, frame: u8) {
    fb.clear(0, 0, 0);
    gpu::draw_quad_flat([(0, 0), (320, 0), (0, 240), (320, 240)], 6, 6, 6);
    gpu::draw_quad_flat([(42, 64), (278, 64), (42, 176), (278, 176)], 14, 13, 11);
    gpu::draw_quad_flat([(46, 68), (274, 68), (46, 172), (274, 172)], 24, 21, 16);
    hltext::draw_centered_scaled(84, "HALF-LIFE", hltext::SMALL_Q8, PAUSE_WHITE);

    let loading = "Loading";
    let y = 118;
    hltext::draw_centered_scaled(y, loading, hltext::SMALL_Q8, PAUSE_AMBER);
    let x = 160 + hltext::text_width_scaled(loading, hltext::SMALL_Q8) / 2 + 8;
    draw_spinner_dots(x, y, frame);

    hltext::draw_centered_scaled(144, label, hltext::SMALL_Q8, PAUSE_DIM);
    gpu::draw_sync();
    gpu::vsync();
    fb.swap();
}

// Level-to-level transition (HL-style): keep the current scene frozen on screen
// and overlay only a tiny "Loading" strip, instead of clearing to a black card.
// The displayed image lives in the FRONT buffer (the one we are NOT drawing to),
// so we retarget the GPU at it, draw a small bottom strip, and DO NOT swap -- the
// previous frame stays visible while the next level streams in. Drawing just
// after vsync writes the small bottom strip before the beam scans down to it, so
// there is no tearing on the live buffer.
fn draw_loading_overlay(fb: &mut FrameBuffer, frame: u8) {
    let front_y = fb.buffer_y(fb.drawing ^ 1);
    gpu::vsync();
    gpu::set_draw_area(0, front_y, fb.width - 1, front_y + fb.height - 1);
    gpu::set_draw_offset(0, front_y as i16);

    let (y0, y1) = (210i16, 232i16);
    gpu::draw_quad_flat([(80, y0), (240, y0), (80, y1), (240, y1)], 5, 5, 7);
    gpu::draw_quad_flat([(80, y0), (240, y0), (80, y0 + 1), (240, y0 + 1)], 24, 21, 16);
    let loading = "Loading";
    let ty = y0 + 5;
    let lw = hltext::text_width_scaled(loading, hltext::SMALL_Q8);
    let spin_w = 8; // dots-grid width
    let bx = 160 - (lw + 8 + spin_w) / 2; // centre "Loading <spinner>" as a unit
    hltext::draw_text_scaled(bx, ty, loading, hltext::SMALL_Q8, PAUSE_AMBER);
    draw_spinner_dots(bx + lw + 8, ty, frame);
    gpu::draw_sync();

    // Restore the draw target to the back buffer so the level renders there.
    let back_y = fb.buffer_y(fb.drawing);
    gpu::set_draw_area(0, back_y, fb.width - 1, back_y + fb.height - 1);
    gpu::set_draw_offset(0, back_y as i16);
}

fn draw_next_loading_screen(fb: &mut FrameBuffer, label: &str, frame: &mut u8, keep_frame: bool) {
    if keep_frame {
        draw_loading_overlay(fb, *frame);
    } else {
        draw_loading_card(fb, label, *frame);
    }
    *frame = frame.wrapping_add(1);
}

fn draw_pause_menu(fb: &mut FrameBuffer, sel: usize) {
    fb.clear(0, 0, 0);
    gpu::draw_quad_flat([(44, 44), (276, 44), (44, 198), (276, 198)], 10, 10, 10);
    gpu::draw_quad_flat([(48, 48), (272, 48), (48, 194), (272, 194)], 20, 18, 14);
    hltext::draw_centered_scaled(66, "HALF-LIFE", hltext::SMALL_Q8, PAUSE_WHITE);
    hltext::draw_centered_scaled(86, "Paused", hltext::SMALL_Q8, PAUSE_DIM);

    let mut i = 0usize;
    while i < PAUSE_ITEMS.len() {
        let y = 116 + i as i16 * 22;
        if i == sel {
            gpu::draw_quad_flat(
                [(88, y - 4), (232, y - 4), (88, y + 18), (232, y + 18)],
                PAUSE_ARMED.0,
                PAUSE_ARMED.1,
                PAUSE_ARMED.2,
            );
            hltext::draw_centered_scaled(y, PAUSE_ITEMS[i], hltext::SMALL_Q8, PAUSE_WHITE);
        } else {
            hltext::draw_centered_scaled(y, PAUSE_ITEMS[i], hltext::SMALL_Q8, PAUSE_AMBER);
        }
        i += 1;
    }

    hltext::draw_centered_scaled(
        174,
        "Cross Select   Circle Back",
        hltext::SMALL_Q8,
        PAUSE_DIM,
    );
}

fn run_pause_menu(fb: &mut FrameBuffer) -> PauseExit {
    let mut sel = 0usize;
    let mut p_up = true;
    let mut p_dn = true;
    let mut p_ok = true;
    let mut p_back = true;

    loop {
        let pad = poll_port1();
        let b = pad.buttons;
        let up = b.is_held(button::UP);
        let dn = b.is_held(button::DOWN);
        let ok = b.is_held(button::CROSS) || b.is_held(button::START);
        let back = b.is_held(button::CIRCLE);

        if up && !p_up {
            sel = (sel + PAUSE_ITEMS.len() - 1) % PAUSE_ITEMS.len();
        }
        if dn && !p_dn {
            sel = (sel + 1) % PAUSE_ITEMS.len();
        }
        if back && !p_back {
            return PauseExit::Resume;
        }
        if ok && !p_ok {
            return if sel == 0 {
                PauseExit::Resume
            } else {
                PauseExit::MainMenu
            };
        }

        p_up = up;
        p_dn = dn;
        p_ok = ok;
        p_back = back;

        draw_pause_menu(fb, sel);
        gpu::draw_sync();
        gpu::vsync();
        fb.swap();
    }
}

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
static mut WEAPON_CACHE_SCALE: u16 = 0;
static mut WEAPON_TRI_CACHE: [TriTexturedGouraud; MAX_WEAPON_CACHE_TRIS] =
    [EMPTY_TRI; MAX_WEAPON_CACHE_TRIS];
static mut WEAPON_TRI_OTZ: [u8; MAX_WEAPON_CACHE_TRIS] = [0; MAX_WEAPON_CACHE_TRIS];
static mut WEAPON_TRI_COUNT: usize = 0;
static mut SUBMODEL_VERT_TOKEN: [u16; MAX_VERTS] = [0; MAX_VERTS];
static mut SUBMODEL_DRAW_TOKEN: u16 = 1;
const PVS_LINK_END: u16 = u16::MAX;
#[derive(Clone, Copy)]
struct PvsFaceRec {
    first: u16,
    count: u16,
    center: [i16; 3],
    radius: u16,
    tex: u8,       // per-face texture (loop faces store tex on the FaceRec)
    is_loop: bool, // fan a vertex loop vs iterate raw tris
}
const EMPTY_PVS_FACE_REC: PvsFaceRec = PvsFaceRec {
    first: 0,
    count: 0,
    center: [0; 3],
    radius: 0,
    tex: 0,
    is_loop: false,
};
static mut VIS_BITS: [u8; MAX_LEAVES / 8] = [0; MAX_LEAVES / 8];
static mut PVS_LEAF_COUNT: usize = 0;
static mut PVS_FACE_INDEX: [u16; MAX_FACES] = [0; MAX_FACES];
static mut PVS_FACE_NEXT: [u16; MAX_FACES] = [PVS_LINK_END; MAX_FACES];
static mut PVS_FACE_REC: [PvsFaceRec; MAX_PVS_FACE_RECS] = [EMPTY_PVS_FACE_REC; MAX_PVS_FACE_RECS];
static mut PVS_FACE_COUNT: usize = 0;
static mut PVS_FACE_MARK: [u16; MAX_FACES] = [0; MAX_FACES];
static mut PVS_FACE_MARK_TOKEN: u16 = 1;
static mut PVS_GROUP_FIRST: [u16; MAX_FACE_GROUPS] = [PVS_LINK_END; MAX_FACE_GROUPS];
static mut PVS_GROUP_FACE: [u16; MAX_FACE_GROUPS] = [PVS_LINK_END; MAX_FACE_GROUPS];
static mut PVS_GROUP_ACTIVE: [u16; MAX_FACE_GROUPS] = [0; MAX_FACE_GROUPS];
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
static mut ENT_PREV_OFF: [[i32; 3]; MAX_ENTS] = [[0; 3]; MAX_ENTS]; // ride-carry deltas
static mut ENT_BREAK_LOGIC: [u16; MAX_ENTS] = [u16::MAX; MAX_ENTS]; // ent -> breakable logic rec
static mut LOGIC_BREAK_HP: [u16; MAX_LOGIC] = [0; MAX_LOGIC]; // remaining breakable health
static mut TELEPORT_REQUEST: Option<([i32; 3], u16)> = None; // dest pos + yaw, applied post-touch
static mut PUSH_IMPULSE: [i32; 3] = [0; 3]; // per-tick trigger_push velocity add
static mut ENT_ACTIVE: [u8; MAX_ENTS] = [0; MAX_ENTS];

#[derive(Clone, Copy)]
struct LogicEvent {
    at: u16,
    target: u16,
    killtarget: u16,
    use_type: u8,
    active: u8,
}

const EMPTY_LOGIC_EVENT: LogicEvent = LogicEvent {
    at: 0,
    target: 0,
    killtarget: 0,
    use_type: map::USE_TOGGLE,
    active: 0,
};

#[derive(Clone, Copy)]
struct ImpactMark {
    pos: [i32; 3],
    ttl: u8,
    kind: u8,
}

const EMPTY_IMPACT_MARK: ImpactMark = ImpactMark {
    pos: [0; 3],
    ttl: 0,
    kind: IMPACT_KIND_WORLD,
};

static mut LOGIC_STATE: [u8; MAX_LOGIC] = [0; MAX_LOGIC];
static mut LOGIC_NEXT: [u16; MAX_LOGIC] = [0; MAX_LOGIC];
static mut LOGIC_TARGET: [u16; MAX_LOGIC] = [0; MAX_LOGIC];
static mut LOGIC_COUNTER: [i16; MAX_LOGIC] = [0; MAX_LOGIC];
static mut LOGIC_PROP_LINK: [u8; MAX_LOGIC] = [LOGIC_PROP_NONE; MAX_LOGIC];
static mut LOGIC_EVENTS: [LogicEvent; MAX_LOGIC_EVENTS] = [EMPTY_LOGIC_EVENT; MAX_LOGIC_EVENTS];
static mut TRACKTRAIN_SUBMODEL: u16 = 0;
static mut TRACKTRAIN_CMD_ACTIVE: u8 = 0;
static mut TRACKTRAIN_CMD_USE_TYPE: u8 = map::USE_TOGGLE;
static mut TRACKTRAIN_CMD_SPEED: u16 = 0;
static mut TRACKTRAIN_USE_SPEED: u16 = 60; // +use drive speed (On A Rail)
static mut LOGIC_PLAYER_POS: [i32; 3] = [0; 3];
static mut SIM_NOW: u16 = 0; // current sim tick, for fire-path logic hooks
static mut LOGIC_PLAYER_YAW: u16 = 0;
static mut LOGIC_PLAYER_PITCH: i16 = 0;
static mut LOGIC_PLAYER_HEALTH: u16 = PLAYER_START_HEALTH;
static mut LOGIC_PLAYER_SUIT: u8 = 0;
static mut LOGIC_PLAYER_ARMOR: u16 = 0;
static mut LOGIC_PLAYER_CLIP_AMMO: u16 = GLOCK_MAX_CLIP;
static mut LOGIC_PLAYER_RESERVE_AMMO: u16 = GLOCK_START_RESERVE;
static mut PROP_COUNT: usize = 0;
static mut PROP_ACTIVE: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_KIND: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_POS: [[i32; 3]; MAX_PROPS] = [[0; 3]; MAX_PROPS];
static mut PROP_YAW: [u16; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_LEAF: [i16; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_STATE: [u8; MAX_PROPS] = [PROP_STATE_IDLE; MAX_PROPS];
static mut PROP_ATTACK_COOLDOWN: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_AI_TIMER: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_AI_TARGET: [u8; MAX_PROPS] = [PROP_TARGET_NONE; MAX_PROPS];
// AI target re-acquisition is staggered: each prop re-runs the (BSP-trace-heavy)
// find_*_target only every AI_REACQUIRE_INTERVAL sim-ticks, keeping its cached
// PROP_AI_TARGET between -- cuts the per-frame line-of-sight trace count ~Nx on
// enemy-dense maps. At 20 Hz a 4-tick lag is ~200 ms (imperceptible); per-frame
// facing/firing LOS still runs every tick so combat stays accurate.
static mut AI_TICK: u32 = 0;
const AI_REACQUIRE_INTERVAL: u32 = 4;
#[inline]
unsafe fn ai_reacquire(pi: usize) -> bool {
    (pi as u32).wrapping_add(AI_TICK) % AI_REACQUIRE_INTERVAL == 0
}
static mut PROP_HEALTH: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_HIT_FLASH: [u8; MAX_PROPS] = [0; MAX_PROPS];
static mut PROP_LOGIC_LINK: [u16; MAX_PROPS] = [u16::MAX; MAX_PROPS];
static mut NAV_QUEUE: [u8; MAX_NAV_NODES] = [0; MAX_NAV_NODES];
static mut NAV_PREV: [u8; MAX_NAV_NODES] = [NAV_NODE_NONE; MAX_NAV_NODES];
static mut IMPACT_MARKS: [ImpactMark; MAX_IMPACT_MARKS] = [EMPTY_IMPACT_MARK; MAX_IMPACT_MARKS];
static mut IMPACT_MARK_CURSOR: usize = 0;
static mut CLIP_CV: [render::CVert; 4] = [render::EMPTY_CV; 4]; // near-clip scratch (reused)

// ---- DEBUG: crosshair triangle pick (find the world tri under screen centre) ----
// Toggle DEBUG_XHAIR. Each frame the nearest world triangle containing the screen
// centre is recorded into XHAIR + outlined on screen, and its leaf / PVS state is
// resolved. Press L1 to dump XHAIR + camera state to the guest debug log, so the
// same triangle can be compared between a frame where it shows and one where it
// is missing. XHAIR is also peekable in RAM (see captures/hl-psx.map).
const DEBUG_XHAIR: bool = false;
#[repr(C)]
#[derive(Clone, Copy)]
struct XhairHit {
    valid: u32,
    tt: u32,
    idx: [u32; 3],
    v: [[i32; 3]; 3],
    sv: [(i16, i16); 3],
    leaf: i32,
    pvs_visible: u32,
    depth: u32,
    tex: u32,
}
const EMPTY_XHAIR: XhairHit = XhairHit {
    valid: 0,
    tt: 0,
    idx: [0; 3],
    v: [[0; 3]; 3],
    sv: [(0, 0); 3],
    leaf: -1,
    pvs_visible: 0,
    depth: u32::MAX,
    tex: u32::MAX,
};
static mut XHAIR: XhairHit = EMPTY_XHAIR;
static mut XHAIR_DUMP_PREV: bool = false;
static mut XHAIR_DUMP_LAST: u32 = u32::MAX; // last auto-dumped key (tt, or sentinel)

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
fn model_local_to_world_q12(raw: u16) -> i32 {
    if raw == 0 {
        4096
    } else {
        raw as i32
    }
}

#[inline(always)]
fn model_local_scale(raw: u16) -> i32 {
    (4096 / model_local_to_world_q12(raw)).max(1)
}

#[inline(always)]
fn model_local_scale_and_shift(raw: u16) -> (i32, u8) {
    let s = model_local_scale(raw);
    let shift = match s {
        1 => 0,
        2 => 1,
        4 => 2,
        8 => 3,
        16 => 4,
        _ => 0xff,
    };
    (s, shift)
}

#[inline(always)]
fn model_unscale_depth(depth: u32, scale: i32, shift: u8) -> u32 {
    if shift != 0xff {
        depth >> shift
    } else {
        depth / scale as u32
    }
}

#[inline(always)]
fn canonical_ram_const<T>(p: *const T) -> *const T {
    #[cfg(target_arch = "mips")]
    {
        (((p as usize) & 0x001f_ffff) | 0x8000_0000) as *const T
    }
    #[cfg(not(target_arch = "mips"))]
    {
        p
    }
}

#[inline(always)]
unsafe fn streamed_map_bytes(len: usize) -> &'static [u8] {
    let ptr = canonical_ram_const(core::ptr::addr_of!(MAP_BUF).cast::<u8>());
    unsafe { core::slice::from_raw_parts(ptr, len) }
}

unsafe fn streamed_model_bytes_at(byte_off: usize, len: usize) -> &'static [u8] {
    let ptr = canonical_ram_const(core::ptr::addr_of!(MODEL_BUF).cast::<u8>());
    unsafe { core::slice::from_raw_parts(ptr.add(byte_off), len) }
}

#[inline(always)]
unsafe fn viewmodel_bytes_at(byte_off: usize, len: usize) -> &'static [u8] {
    let ptr = canonical_ram_const(core::ptr::addr_of!(MODEL_BUF).cast::<u8>());
    unsafe { core::slice::from_raw_parts(ptr.add(byte_off), len) }
}

/// Stream one weapon's viewmodel (geometry -> MODEL_BUF head reserve, textures
/// -> VM_SLOTS), appending at the pool's fill cursor. Idempotent: an already-
/// resident weapon returns `true` without touching the disc. Returns `false` if
/// the pool is full or the chunk is missing (the caller falls back to the
/// glock). Textures stage transiently above the reserve, in the enemy region
/// stream_map_models fills later.
unsafe fn stream_one_viewmodel(
    wid: usize,
    stream_chunks: &mut u32,
    stream_bytes: &mut u32,
    stream_sectors: &mut u32,
) -> bool {
    if wid >= N_WEAPONS {
        return false;
    }
    if VM_ENTRY[wid].valid {
        return true;
    }
    let word = VM_FILL_WORD;
    let slot = VM_FILL_SLOT;
    if word >= VM_POOL_WORDS || slot >= VM_SLOTS_TOTAL {
        return false;
    }
    let buf_ptr = core::ptr::addr_of_mut!(MODEL_BUF).cast::<u32>();
    let wm = WEAPON_DEFS[wid].wm as u32;
    let dst = core::slice::from_raw_parts_mut(buf_ptr.add(word), VM_POOL_WORDS - word);
    let glen = cdstream::load_chunk(MODEL_CHUNK_V_9MMHANDGUN + wm, dst).unwrap_or(0);
    let glen_words = glen.div_ceil(4);
    if glen == 0 || word + glen_words > VM_POOL_WORDS {
        return false;
    }
    account_streamed_chunk(glen, stream_chunks, stream_bytes, stream_sectors);
    // Stage the texture in the pool's free tail (after this geometry), bounded
    // at VM_POOL_WORDS. A mid-switch stream runs while the enemies above the
    // reserve are resident, so staging at VM_POOL_WORDS itself would clobber
    // them; keeping it inside the pool is safe (a tex too big for the remaining
    // pool is skipped -> untextured, never overflowing into the enemies).
    let (ntex, _) = stream_model_texture_chunk(
        MODEL_CHUNK_V_9MMHANDGUN_TEX + wm,
        word + glen_words,
        VM_POOL_WORDS,
        core::ptr::addr_of_mut!(VM_SLOTS).cast::<TexSlot>().add(slot),
        VM_SLOTS_TOTAL - slot,
        stream_chunks,
        stream_bytes,
        stream_sectors,
    )
    .unwrap_or((0, 0));
    VM_ENTRY[wid] = VmEntry {
        valid: true,
        geom_off: word * 4,
        geom_len: glen,
        slot_start: slot,
        n_slots: ntex,
    };
    VM_FILL_WORD = word + glen_words;
    VM_FILL_SLOT = slot + ntex;
    true
}

/// Reset the viewmodel pool and preload only the glock (the spawn weapon + the
/// fallback). Every other weapon streams in on first switch, so a map load
/// streams one viewmodel instead of all 14. Returns whether the glock loaded.
unsafe fn load_resident_viewmodels(
    stream_chunks: &mut u32,
    stream_bytes: &mut u32,
    stream_sectors: &mut u32,
) -> bool {
    for e in VM_ENTRY.iter_mut() {
        *e = VmEntry::NONE;
    }
    VM_FILL_WORD = 0;
    VM_FILL_SLOT = 0;
    stream_one_viewmodel(W_GLOCK, stream_chunks, stream_bytes, stream_sectors)
}

/// Resolve the viewmodel (Model + its VM_SLOTS sub-range) for a weapon, falling
/// back to the glock for weapons outside the resident set.
unsafe fn viewmodel_for(wid: usize) -> (Model, usize, usize) {
    let e = if wid < N_WEAPONS && VM_ENTRY[wid].valid {
        VM_ENTRY[wid]
    } else {
        VM_ENTRY[W_GLOCK]
    };
    (
        Model::load(viewmodel_bytes_at(e.geom_off, e.geom_len)),
        e.slot_start,
        e.n_slots,
    )
}

#[inline]
unsafe fn loaded_model(slot: usize) -> Model {
    let lm = LOADED_MODELS[slot];
    Model::load(streamed_model_bytes_at(lm.geom_off, lm.geom_len))
}

/// Stream the model types this map places (distinct prop kinds) into the shared
/// pool: geometry into MODEL_BUF after the viewmodel, render faces into
/// POOL_FACES, textures into VRAM (POOL_TEX). `TYPE_TO_SLOT` maps a type id to
/// its `LOADED_MODELS` entry; types that don't fit the buffers are skipped (the
/// prop simply doesn't render).
unsafe fn stream_map_models(m: &Map, weapon_len: usize) {
    for t in TYPE_TO_SLOT.iter_mut() {
        *t = MODEL_SLOT_NONE;
    }
    for lm in LOADED_MODELS.iter_mut() {
        *lm = LoadedModel::ZERO;
    }
    let buf_ptr = core::ptr::addr_of_mut!(MODEL_BUF).cast::<u32>();
    let mut geom_word = weapon_len.div_ceil(4); // viewmodel reserves the head
    let mut face_off = 0usize;
    let mut tex_off = 0usize;
    let mut slot_idx = 0usize;
    let (mut sc, mut sb, mut ss) = (0u32, 0u32, 0u32);
    let nprops = m.n_props.min(MAX_PROPS);
    // Two passes: combat/interactive types first, decorative render-only types
    // (AI_IDLE: bosses, flyers, barnacles) last. If a heavy roster overflows the
    // pool, a background statue drops instead of a fighting enemy.
    let mut pass = 0;
    let mut pi = 0usize;
    loop {
        if pi >= nprops {
            if pass == 0 {
                pass = 1;
                pi = 0;
                continue;
            }
            break;
        }
        let ty = (m.prop(pi).0 & !PROP_DEAD_BIT) as usize; // corpses stream their live model
        pi += 1;
        if ty >= N_MODEL_TYPES || TYPE_TO_SLOT[ty] != MODEL_SLOT_NONE {
            continue; // out of range, or this type is already resident
        }
        let decorative = model_def(ty as u8).ai == AI_IDLE;
        if (pass == 0) == decorative {
            continue; // wrong pass for this type
        }
        if slot_idx >= MAX_LOADED_MODELS || geom_word >= MODEL_WORDS {
            break;
        }
        let dst = core::slice::from_raw_parts_mut(buf_ptr.add(geom_word), MODEL_WORDS - geom_word);
        let glen = cdstream::load_chunk(MODEL_GEOM_CHUNK_BASE + ty as u32, dst).unwrap_or(0);
        if glen == 0 || geom_word + glen.div_ceil(4) > MODEL_WORDS {
            continue; // missing chunk or would overflow MODEL_BUF -> skip type
        }
        let md = Model::load(streamed_model_bytes_at(geom_word * 4, glen));
        let nf = md.fill_render_faces_raw(
            core::ptr::addr_of_mut!(POOL_FACES)
                .cast::<ModelRenderFace>()
                .add(face_off),
            POOL_FACE_CAP - face_off,
        );
        let (ntex, _failed) = stream_model_texture_chunk(
            MODEL_TEX_CHUNK_BASE + ty as u32,
            geom_word + glen.div_ceil(4), // stage tex in the free tail above this geom
            MODEL_WORDS,                  // ... up to the end of the pool
            core::ptr::addr_of_mut!(POOL_TEX).cast::<TexSlot>().add(tex_off),
            POOL_TEX_SLOTS - tex_off,
            &mut sc,
            &mut sb,
            &mut ss,
        )
        .unwrap_or((0, 0));
        LOADED_MODELS[slot_idx] = LoadedModel {
            valid: true,
            type_id: ty as u8,
            geom_off: geom_word * 4,
            geom_len: glen,
            face_start: face_off,
            n_faces: nf,
            tex_start: tex_off,
            n_tex: ntex,
        };
        TYPE_TO_SLOT[ty] = slot_idx as u8;
        geom_word += glen.div_ceil(4);
        face_off += nf;
        tex_off += ntex;
        slot_idx += 1;
    }
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

/// Length of a world-space segment.
#[inline]
fn seg_len(a: [i32; 3], b: [i32; 3]) -> i32 {
    isqrt(dist2_3(a, b)).max(1)
}

#[inline]
fn tram_step_for_speed(speed: i32) -> i32 {
    if speed <= 0 {
        0
    } else {
        ((speed + 19) / 20).max(1)
    }
}

#[inline]
fn tram_path_pos(m: &Map, seg: usize, seg_dist: i32) -> [i32; 3] {
    if m.n_way == 0 {
        return [0, 0, 0];
    }
    if seg + 1 >= m.n_way {
        return m.waypoint(m.n_way - 1);
    }
    let a = m.waypoint(seg);
    let b = m.waypoint(seg + 1);
    let len = seg_len(a, b);
    let f = (seg_dist * 4096 / len).clamp(0, 4096);
    [
        a[0] + ((b[0] - a[0]) * f >> 12),
        a[1] + ((b[1] - a[1]) * f >> 12),
        a[2] + ((b[2] - a[2]) * f >> 12),
    ]
}

fn tram_advance(m: &Map, seg: &mut usize, seg_dist: &mut i32, step: i32) -> bool {
    if step <= 0 || m.n_way < 2 {
        return false;
    }
    let mut rem = step;
    while rem > 0 && *seg + 1 < m.n_way {
        let len = seg_len(m.waypoint(*seg), m.waypoint(*seg + 1));
        if *seg_dist + rem >= len {
            rem -= len - *seg_dist;
            *seg += 1;
            *seg_dist = 0;
        } else {
            *seg_dist += rem;
            rem = 0;
        }
    }
    *seg + 1 < m.n_way
}

fn tram_apply_command(
    use_type: u8,
    requested_speed: i32,
    active: &mut bool,
    current_speed: &mut i32,
) -> bool {
    match use_type {
        map::USE_OFF => {
            *active = false;
            *current_speed = 0;
            false
        }
        map::USE_ON => {
            *active = requested_speed > 0;
            *current_speed = requested_speed.max(0);
            *active
        }
        _ => {
            if *active {
                *active = false;
                *current_speed = 0;
                false
            } else {
                *active = requested_speed > 0;
                *current_speed = requested_speed.max(0);
                *active
            }
        }
    }
}

#[inline]
fn tram_should_carry_player(player_pos: [i32; 3], train_pos: [i32; 3]) -> bool {
    let dx = player_pos[0] - train_pos[0];
    let dz = player_pos[2] - train_pos[2];
    let dy = player_pos[1] - train_pos[1];
    dx * dx + dz * dz <= TRAM_CARRY_RADIUS2 && dy.abs() <= TRAM_CARRY_HEIGHT
}

#[derive(Clone, Copy)]
struct LandmarkName {
    bytes: [u8; LANDMARK_NAME_MAX + 1],
    len: u8,
}

const NO_LANDMARK: LandmarkName = LandmarkName {
    bytes: [0; LANDMARK_NAME_MAX + 1],
    len: 0,
};

#[derive(Clone, Copy)]
struct RoomLaunch {
    room_id: u16,
    landmark: LandmarkName,
    landmark_offset: [i32; 3],
    yaw: u16,
    pitch: i16,
    health: u16,
    suit_equipped: bool,
    armor: u16,
    clip_ammo: u16,
    reserve_ammo: u16,
    preserve_view: bool,
}

#[derive(Clone, Copy)]
enum PlayExit {
    BackToMenu,
    ChangeLevel(RoomLaunch),
}

static mut CHANGE_REQUEST: RoomLaunch = RoomLaunch {
    room_id: 0,
    landmark: NO_LANDMARK,
    landmark_offset: [0; 3],
    yaw: 0,
    pitch: 0,
    health: PLAYER_START_HEALTH,
    suit_equipped: false,
    armor: 0,
    clip_ammo: GLOCK_MAX_CLIP,
    reserve_ammo: GLOCK_START_RESERVE,
    preserve_view: false,
};
static mut CHANGE_REQUEST_ACTIVE: u8 = 0;
// Arsenal carried across changelevel (menu launches reset it): owned mask,
// per-weapon clips, ammo pools, selected weapon.
static mut CARRY_VALID: bool = false;
static mut CARRY_OWNED: u16 = 0;
static mut CARRY_CLIPS: [u16; N_WEAPONS] = [0; N_WEAPONS];
static mut CARRY_AMMO: [u16; N_AMMO] = [0; N_AMMO];
static mut CARRY_CURRENT: u8 = 0;

#[inline]
fn standalone_room_starts_with_hev(room_id: usize) -> bool {
    // The first cooked item_suit is in c1a0d (room 9). Later standalone chapter
    // launches should behave like campaign progress so batteries/enemy damage
    // are testable, while c1a0d itself still exposes the pickup.
    room_id > 9
}

fn menu_launch(room_id: usize) -> RoomLaunch {
    RoomLaunch {
        room_id: room_id.min(u16::MAX as usize) as u16,
        landmark: NO_LANDMARK,
        landmark_offset: [0; 3],
        yaw: 0,
        pitch: 0,
        health: PLAYER_START_HEALTH,
        suit_equipped: standalone_room_starts_with_hev(room_id),
        armor: 0,
        clip_ammo: GLOCK_MAX_CLIP,
        reserve_ammo: GLOCK_START_RESERVE,
        preserve_view: false,
    }
}

fn landmark_from_str(name: &str) -> LandmarkName {
    let mut out = NO_LANDMARK;
    let src = name.as_bytes();
    let n = src.len().min(LANDMARK_NAME_MAX);
    let mut i = 0usize;
    while i < n {
        out.bytes[i] = src[i];
        i += 1;
    }
    out.len = n as u8;
    out
}

fn landmark_matches(name: LandmarkName, other: &str) -> bool {
    let n = name.len as usize;
    other.as_bytes().len() == n && &name.bytes[..n] == other.as_bytes()
}

const LOGIC_STATE_BOTTOM: u8 = 0;
const LOGIC_STATE_GOING_UP: u8 = 1;
const LOGIC_STATE_TOP: u8 = 2;
const LOGIC_STATE_GOING_DOWN: u8 = 3;
const LOGIC_STATE_WAITING: u8 = 4;
const LOGIC_STATE_REMOVED: u8 = 255;
const LOGIC_PROP_NONE: u8 = 255;
const SF_DOOR_START_OPEN: u16 = 1;
const SF_DOOR_TOGGLE: u16 = 32;
const SF_DOOR_USE_ONLY: u16 = 256;
const SF_BUTTON_DONTMOVE: u16 = 1;
const SF_BUTTON_TOGGLE: u16 = 32;
const SF_BUTTON_TOUCH_ONLY: u16 = 256;
const SF_TRIGGER_HURT_TARGET_ONCE: u16 = 1;
const SF_TRIGGER_HURT_START_OFF: u16 = 2;
const SF_BREAK_TRIGGER_ONLY: u16 = 1; // func_breakable: immune to gunfire
const TRIGGER_HURT_REPEAT_TICKS: u16 = 10;
const TRAM_CARRY_RADIUS2: i32 = 384 * 384;
const TRAM_CARRY_HEIGHT: i32 = 160;

#[inline]
fn time_reached(now: u16, at: u16) -> bool {
    now.wrapping_sub(at) < 0x8000
}

#[inline]
fn logic_valid_brush(brush: u16, nents: usize) -> Option<usize> {
    let i = brush as usize;
    if brush != map::LOGIC_BRUSH_NONE && i < nents {
        Some(i)
    } else {
        None
    }
}

/// True when the player overlaps any func_ladder volume (kind 4). Ladders are
/// invisible AABBs; expand them by the player hull so grabbing feels natural.
unsafe fn ladder_touch(m: &Map, nents: usize, pos: [i32; 3]) -> bool {
    let _ = m;
    let mut ei = 0usize;
    while ei < nents {
        let e = ENT_CACHE[ei];
        if e.kind == 4 && ENT_ACTIVE[ei] != 0 {
            let dx = (pos[0] - e.center[0]).abs();
            let dy = (pos[1] - e.center[1]).abs();
            let dz = (pos[2] - e.center[2]).abs();
            if dx <= e.mv[0] + 18 && dy <= e.mv[1] + 34 && dz <= e.mv[2] + 18 {
                return true;
            }
        }
        ei += 1;
    }
    false
}

/// Damage a brush entity; breakables shatter at 0 HP (vanish, fire targets).
unsafe fn damage_brush_ent(m: &Map, nlogic: usize, nents: usize, ei: usize, dmg: u8, now: u16) {
    if ei >= nents || ENT_ACTIVE[ei] == 0 {
        return;
    }
    let li = ENT_BREAK_LOGIC[ei];
    if li == u16::MAX || (li as usize) >= nlogic {
        return;
    }
    let li = li as usize;
    let rec = m.logic(li);
    if (rec.spawnflags & SF_BREAK_TRIGGER_ONLY) != 0 {
        return; // only breakable via its trigger, not gunfire
    }
    let hp = LOGIC_BREAK_HP[li];
    if hp == 0 {
        return;
    }
    let hp = hp.saturating_sub(dmg as u16);
    LOGIC_BREAK_HP[li] = hp;
    if hp == 0 {
        ENT_ACTIVE[ei] = 0;
        LOGIC_STATE[li] = LOGIC_STATE_REMOVED;
        // Glass tinkles, everything else crunches (material key, arg1).
        let snd = if rec.arg1 == 0 { sfx::GLASS_BREAK } else { sfx::WOOD_BREAK };
        sfx::play_world(snd, ENT_CACHE[ei].center);
        logic_sub_use_targets(m, nlogic, nents, li, rec, now, map::USE_TOGGLE, 0);
    }
}

#[inline]
unsafe fn ent_draw_offset(ei: usize) -> [i32; 3] {
    let e = ENT_CACHE[ei];
    if e.kind == 1 || e.kind == 3 {
        scale12_vec(e.mv, ENT_PHASE[ei])
    } else {
        e.origin
    }
}

#[inline]
unsafe fn logic_current_target(li: usize, rec: map::LogicEnt) -> u16 {
    if li < MAX_LOGIC {
        LOGIC_TARGET[li]
    } else {
        rec.target
    }
}

#[inline]
fn logic_item_prop_kind(kind: u8) -> Option<u8> {
    match kind {
        map::LOGIC_ITEM_SUIT => Some(PROP_TYPE_ITEM_SUIT),
        map::LOGIC_ITEM_BATTERY => Some(PROP_TYPE_ITEM_BATTERY),
        _ => None,
    }
}

unsafe fn logic_find_by_targetname(m: &Map, nlogic: usize, targetname: u16) -> Option<usize> {
    if targetname == 0 {
        return None;
    }
    let mut li = 0usize;
    while li < nlogic {
        if LOGIC_STATE[li] != LOGIC_STATE_REMOVED {
            let rec = m.logic(li);
            if rec.targetname == targetname {
                return Some(li);
            }
        }
        li += 1;
    }
    None
}

#[inline]
unsafe fn logic_queue_tracktrain_command(rec: map::LogicEnt, use_type: u8) {
    if rec.arg1 == 0 || rec.arg1 != TRACKTRAIN_SUBMODEL {
        return;
    }
    TRACKTRAIN_CMD_ACTIVE = 1;
    TRACKTRAIN_CMD_USE_TYPE = use_type;
    TRACKTRAIN_CMD_SPEED = rec.speed;
}

#[inline]
unsafe fn logic_take_tracktrain_command() -> Option<(u8, i32)> {
    if TRACKTRAIN_CMD_ACTIVE == 0 {
        return None;
    }
    TRACKTRAIN_CMD_ACTIVE = 0;
    Some((TRACKTRAIN_CMD_USE_TYPE, TRACKTRAIN_CMD_SPEED as i32))
}

unsafe fn logic_remove_entity(li: usize, rec: map::LogicEnt, nents: usize) {
    if li >= MAX_LOGIC {
        return;
    }
    LOGIC_STATE[li] = LOGIC_STATE_REMOVED;
    LOGIC_TARGET[li] = 0;
    LOGIC_COUNTER[li] = 0;
    if let Some(ei) = logic_valid_brush(rec.brush, nents) {
        ENT_ACTIVE[ei] = 0;
    }
    let pi = LOGIC_PROP_LINK[li];
    if pi != LOGIC_PROP_NONE {
        let p = pi as usize;
        if p < MAX_PROPS {
            PROP_ACTIVE[p] = 0;
            PROP_LOGIC_LINK[p] = u16::MAX;
        }
        LOGIC_PROP_LINK[li] = LOGIC_PROP_NONE;
    }
}

#[inline]
fn logic_center(rec: map::LogicEnt) -> [i32; 3] {
    [
        (rec.mins[0] + rec.maxs[0]) >> 1,
        (rec.mins[1] + rec.maxs[1]) >> 1,
        (rec.mins[2] + rec.maxs[2]) >> 1,
    ]
}

#[inline]
fn player_touches_logic(pos: [i32; 3], rec: map::LogicEnt) -> bool {
    let pmins = [
        pos[0] - PLAYER_TOUCH_HALF_XZ,
        pos[1],
        pos[2] - PLAYER_TOUCH_HALF_XZ,
    ];
    let pmaxs = [
        pos[0] + PLAYER_TOUCH_HALF_XZ,
        pos[1] + PLAYER_TOUCH_HEIGHT,
        pos[2] + PLAYER_TOUCH_HALF_XZ,
    ];
    pmins[0] <= rec.maxs[0]
        && pmaxs[0] >= rec.mins[0]
        && pmins[1] <= rec.maxs[1]
        && pmaxs[1] >= rec.mins[1]
        && pmins[2] <= rec.maxs[2]
        && pmaxs[2] >= rec.mins[2]
}

#[inline]
unsafe fn logic_enqueue_event(at: u16, target: u16, killtarget: u16, use_type: u8) {
    if target == 0 && killtarget == 0 {
        return;
    }
    let mut i = 0usize;
    while i < MAX_LOGIC_EVENTS {
        if LOGIC_EVENTS[i].active == 0 {
            LOGIC_EVENTS[i] = LogicEvent {
                at,
                target,
                killtarget,
                use_type,
                active: 1,
            };
            return;
        }
        i += 1;
    }
}

unsafe fn logic_kill_targets(m: &Map, nlogic: usize, nents: usize, target: u16) {
    if target == 0 {
        return;
    }
    let mut li = 0usize;
    while li < nlogic {
        if LOGIC_STATE[li] != LOGIC_STATE_REMOVED {
            let rec = m.logic(li);
            if rec.targetname == target {
                logic_remove_entity(li, rec, nents);
            }
        }
        li += 1;
    }
}

unsafe fn logic_landmark_origin_by_id(m: &Map, nlogic: usize, name_id: u16) -> Option<[i32; 3]> {
    if name_id == 0 {
        return None;
    }
    let mut li = 0usize;
    while li < nlogic {
        let rec = m.logic(li);
        if rec.kind == map::LOGIC_INFO_LANDMARK && rec.targetname == name_id {
            return Some(rec.origin);
        }
        li += 1;
    }
    None
}

fn launch_landmark_origin(m: &Map, nlogic: usize, name: LandmarkName) -> Option<[i32; 3]> {
    if name.len == 0 {
        return None;
    }
    let mut li = 0usize;
    while li < nlogic {
        let rec = m.logic(li);
        if rec.kind == map::LOGIC_INFO_LANDMARK
            && rec.targetname != 0
            && landmark_matches(name, m.logic_name(rec.targetname))
        {
            return Some(rec.origin);
        }
        li += 1;
    }
    None
}

unsafe fn logic_request_changelevel(m: &Map, nlogic: usize, rec: map::LogicEnt) {
    let map_name = m.logic_name(rec.arg0);
    let landmark_name = m.logic_name(rec.arg1);
    let Some(room_id) = menu::room_for_map_name(map_name) else {
        tty::println("hl-psx: changelevel target not cooked");
        telemetry::debug_log("hl-psx: changelevel target not cooked");
        telemetry::debug_log(map_name);
        return;
    };
    telemetry::debug_log("hl-psx: changelevel request");
    telemetry::debug_log(map_name);
    if !landmark_name.is_empty() {
        telemetry::debug_log("hl-psx: changelevel landmark");
        telemetry::debug_log(landmark_name);
    }
    let old_landmark = logic_landmark_origin_by_id(m, nlogic, rec.arg1);
    let offset = if let Some(origin) = old_landmark {
        [
            LOGIC_PLAYER_POS[0] - origin[0],
            LOGIC_PLAYER_POS[1] - origin[1],
            LOGIC_PLAYER_POS[2] - origin[2],
        ]
    } else {
        [0, 0, 0]
    };
    CHANGE_REQUEST = RoomLaunch {
        room_id: room_id.min(u16::MAX as usize) as u16,
        landmark: landmark_from_str(landmark_name),
        landmark_offset: offset,
        yaw: LOGIC_PLAYER_YAW,
        pitch: LOGIC_PLAYER_PITCH,
        health: LOGIC_PLAYER_HEALTH,
        suit_equipped: LOGIC_PLAYER_SUIT != 0,
        armor: LOGIC_PLAYER_ARMOR,
        clip_ammo: LOGIC_PLAYER_CLIP_AMMO,
        reserve_ammo: LOGIC_PLAYER_RESERVE_AMMO,
        preserve_view: true,
    };
    CHANGE_REQUEST_ACTIVE = 1;
}

unsafe fn logic_sub_use_targets(
    m: &Map,
    nlogic: usize,
    nents: usize,
    li: usize,
    rec: map::LogicEnt,
    now: u16,
    use_type: u8,
    depth: u8,
) {
    let target = logic_current_target(li, rec);
    if rec.delay_ticks > 0 {
        logic_enqueue_event(
            now.wrapping_add(rec.delay_ticks),
            target,
            rec.killtarget,
            use_type,
        );
        return;
    }
    logic_kill_targets(m, nlogic, nents, rec.killtarget);
    logic_fire_targets(m, nlogic, nents, target, use_type, now, depth + 1);
}

unsafe fn logic_fire_targets(
    m: &Map,
    nlogic: usize,
    nents: usize,
    target: u16,
    use_type: u8,
    now: u16,
    depth: u8,
) {
    if target == 0 || depth > 8 {
        return;
    }
    let mut li = 0usize;
    while li < nlogic {
        if LOGIC_STATE[li] != LOGIC_STATE_REMOVED {
            let rec = m.logic(li);
            if rec.targetname == target {
                logic_use_entity(m, nlogic, nents, li, use_type, now, depth + 1);
            }
        }
        li += 1;
    }
}

#[inline]
unsafe fn logic_phase_step(rec: map::LogicEnt, ei: usize) -> i32 {
    let e = ENT_CACHE[ei];
    let len = isqrt(e.mv[0] * e.mv[0] + e.mv[1] * e.mv[1] + e.mv[2] * e.mv[2]).max(1);
    let speed = (rec.speed as i32).max(1);
    ((speed * 4096) / (20 * len)).max(1).min(4096)
}

unsafe fn logic_activate_door(nents: usize, li: usize, rec: map::LogicEnt, use_type: u8) {
    let Some(_ei) = logic_valid_brush(rec.brush, nents) else {
        return;
    };
    let state = LOGIC_STATE[li];
    if state == LOGIC_STATE_REMOVED {
        return;
    }
    match use_type {
        map::USE_OFF => {
            if state == LOGIC_STATE_TOP || state == LOGIC_STATE_GOING_UP {
                LOGIC_STATE[li] = LOGIC_STATE_GOING_DOWN;
            }
        }
        map::USE_ON => {
            if state == LOGIC_STATE_BOTTOM || state == LOGIC_STATE_GOING_DOWN {
                LOGIC_STATE[li] = LOGIC_STATE_GOING_UP;
            }
        }
        _ => {
            if state == LOGIC_STATE_BOTTOM || state == LOGIC_STATE_GOING_DOWN {
                LOGIC_STATE[li] = LOGIC_STATE_GOING_UP;
            } else if (rec.spawnflags & SF_DOOR_TOGGLE) != 0 {
                LOGIC_STATE[li] = LOGIC_STATE_GOING_DOWN;
            }
        }
    }
}

unsafe fn logic_activate_button(
    m: &Map,
    nlogic: usize,
    nents: usize,
    li: usize,
    rec: map::LogicEnt,
    now: u16,
    depth: u8,
    from_touch: bool,
) {
    if !from_touch && (rec.spawnflags & SF_BUTTON_TOUCH_ONLY) != 0 {
        return;
    }
    let state = LOGIC_STATE[li];
    if state == LOGIC_STATE_REMOVED
        || state == LOGIC_STATE_GOING_UP
        || state == LOGIC_STATE_GOING_DOWN
        || state == LOGIC_STATE_WAITING
    {
        return;
    }
    sfx::play(sfx::BUTTON);
    if (rec.spawnflags & SF_BUTTON_DONTMOVE) != 0 {
        logic_sub_use_targets(m, nlogic, nents, li, rec, now, map::USE_TOGGLE, depth + 1);
        if rec.wait_ticks >= 0 {
            LOGIC_STATE[li] = LOGIC_STATE_WAITING;
            LOGIC_NEXT[li] = now.wrapping_add(rec.wait_ticks as u16);
        }
        return;
    }
    if state == LOGIC_STATE_TOP && (rec.spawnflags & SF_BUTTON_TOGGLE) != 0 {
        LOGIC_STATE[li] = LOGIC_STATE_GOING_DOWN;
    } else if state == LOGIC_STATE_BOTTOM {
        LOGIC_STATE[li] = LOGIC_STATE_GOING_UP;
    }
}

unsafe fn logic_use_entity(
    m: &Map,
    nlogic: usize,
    nents: usize,
    li: usize,
    use_type: u8,
    now: u16,
    depth: u8,
) {
    if li >= nlogic || depth > 8 || LOGIC_STATE[li] == LOGIC_STATE_REMOVED {
        return;
    }
    let rec = m.logic(li);
    match rec.kind {
        map::LOGIC_FUNC_DOOR => logic_activate_door(nents, li, rec, use_type),
        map::LOGIC_FUNC_BUTTON => {
            logic_activate_button(m, nlogic, nents, li, rec, now, depth + 1, false)
        }
        map::LOGIC_TRIGGER_RELAY => {
            logic_sub_use_targets(m, nlogic, nents, li, rec, now, rec.use_type, depth + 1)
        }
        map::LOGIC_TRIGGER_AUTO => {
            logic_sub_use_targets(m, nlogic, nents, li, rec, now, rec.use_type, depth + 1);
            logic_remove_entity(li, rec, nents);
        }
        map::LOGIC_MULTI_MANAGER => {
            logic_kill_targets(m, nlogic, nents, rec.killtarget);
            let mut ai = 0usize;
            while ai < rec.aux_count {
                let aux = m.logic_aux(rec.first_aux + ai);
                logic_enqueue_event(
                    now.wrapping_add(aux.delay_ticks),
                    aux.target,
                    0,
                    map::USE_TOGGLE,
                );
                ai += 1;
            }
        }
        map::LOGIC_TRIGGER_COUNTER => {
            if LOGIC_COUNTER[li] > 0 {
                LOGIC_COUNTER[li] -= 1;
                if LOGIC_COUNTER[li] == 0 {
                    logic_sub_use_targets(
                        m,
                        nlogic,
                        nents,
                        li,
                        rec,
                        now,
                        map::USE_TOGGLE,
                        depth + 1,
                    );
                    logic_remove_entity(li, rec, nents);
                }
            }
        }
        map::LOGIC_TRIGGER_CHANGETARGET => {
            if let Some(target_li) = logic_find_by_targetname(m, nlogic, rec.target) {
                LOGIC_TARGET[target_li] = rec.arg0;
            }
        }
        map::LOGIC_TRIGGER_HURT => match use_type {
            map::USE_ON => LOGIC_STATE[li] = LOGIC_STATE_BOTTOM,
            map::USE_OFF => LOGIC_STATE[li] = LOGIC_STATE_TOP,
            _ => {
                LOGIC_STATE[li] = if LOGIC_STATE[li] == LOGIC_STATE_TOP {
                    LOGIC_STATE_BOTTOM
                } else {
                    LOGIC_STATE_TOP
                };
            }
        },
        map::LOGIC_FUNC_TRACKTRAIN => {
            logic_queue_tracktrain_command(rec, use_type);
        }
        map::LOGIC_TRIGGER_CHANGELEVEL => {
            logic_sub_use_targets(m, nlogic, nents, li, rec, now, use_type, depth + 1);
            logic_request_changelevel(m, nlogic, rec);
        }
        map::LOGIC_TRIGGER_ONCE | map::LOGIC_TRIGGER_MULTIPLE => {
            logic_sub_use_targets(m, nlogic, nents, li, rec, now, use_type, depth + 1)
        }
        _ => {}
    }
}

unsafe fn logic_process_events(m: &Map, nlogic: usize, nents: usize, now: u16) {
    let mut i = 0usize;
    while i < MAX_LOGIC_EVENTS {
        let ev = LOGIC_EVENTS[i];
        if ev.active != 0 && time_reached(now, ev.at) {
            LOGIC_EVENTS[i].active = 0;
            logic_kill_targets(m, nlogic, nents, ev.killtarget);
            logic_fire_targets(m, nlogic, nents, ev.target, ev.use_type, now, 0);
        }
        i += 1;
    }
}

unsafe fn logic_pre_tick(m: &Map, nlogic: usize, nents: usize, now: u16) {
    logic_process_events(m, nlogic, nents, now);
    let mut li = 0usize;
    while li < nlogic {
        let rec = m.logic(li);
        match LOGIC_STATE[li] {
            LOGIC_STATE_WAITING => {
                if time_reached(now, LOGIC_NEXT[li]) {
                    LOGIC_STATE[li] = LOGIC_STATE_BOTTOM;
                }
            }
            LOGIC_STATE_GOING_UP | LOGIC_STATE_GOING_DOWN => {
                if let Some(ei) = logic_valid_brush(rec.brush, nents) {
                    let step = logic_phase_step(rec, ei);
                    if LOGIC_STATE[li] == LOGIC_STATE_GOING_UP {
                        if ENT_PHASE[ei] == 0 {
                            sfx::play_world(sfx::DOOR_MOVE, ENT_CACHE[ei].center);
                        }
                        ENT_PHASE[ei] = (ENT_PHASE[ei] + step).min(4096);
                        if ENT_PHASE[ei] >= 4096 {
                            sfx::play_world(sfx::DOOR_STOP, ENT_CACHE[ei].center);
                            LOGIC_STATE[li] = LOGIC_STATE_TOP;
                            logic_sub_use_targets(
                                m,
                                nlogic,
                                nents,
                                li,
                                rec,
                                now,
                                map::USE_TOGGLE,
                                0,
                            );
                            if rec.kind == map::LOGIC_FUNC_DOOR {
                                if rec.wait_ticks >= 0 && (rec.spawnflags & SF_DOOR_TOGGLE) == 0 {
                                    LOGIC_NEXT[li] = now.wrapping_add(rec.wait_ticks as u16);
                                }
                            } else if rec.kind == map::LOGIC_FUNC_BUTTON {
                                if rec.wait_ticks >= 0 && (rec.spawnflags & SF_BUTTON_TOGGLE) == 0 {
                                    LOGIC_NEXT[li] = now.wrapping_add(rec.wait_ticks as u16);
                                }
                            }
                        }
                    } else {
                        if ENT_PHASE[ei] == 4096 {
                            sfx::play_world(sfx::DOOR_MOVE, ENT_CACHE[ei].center);
                        }
                        ENT_PHASE[ei] = (ENT_PHASE[ei] - step).max(0);
                        if ENT_PHASE[ei] <= 0 {
                            sfx::play_world(sfx::DOOR_STOP, ENT_CACHE[ei].center);
                            LOGIC_STATE[li] = LOGIC_STATE_BOTTOM;
                            if rec.kind == map::LOGIC_FUNC_DOOR {
                                logic_sub_use_targets(
                                    m,
                                    nlogic,
                                    nents,
                                    li,
                                    rec,
                                    now,
                                    map::USE_TOGGLE,
                                    0,
                                );
                            }
                        }
                    }
                }
            }
            LOGIC_STATE_TOP => {
                if rec.kind == map::LOGIC_FUNC_DOOR
                    && rec.wait_ticks >= 0
                    && (rec.spawnflags & SF_DOOR_TOGGLE) == 0
                    && time_reached(now, LOGIC_NEXT[li])
                {
                    LOGIC_STATE[li] = LOGIC_STATE_GOING_DOWN;
                } else if rec.kind == map::LOGIC_FUNC_BUTTON
                    && rec.wait_ticks >= 0
                    && (rec.spawnflags & SF_BUTTON_TOGGLE) == 0
                    && time_reached(now, LOGIC_NEXT[li])
                {
                    LOGIC_STATE[li] = LOGIC_STATE_GOING_DOWN;
                }
            }
            _ => {}
        }
        li += 1;
    }
}

unsafe fn logic_try_use(
    m: &Map,
    nlogic: usize,
    nents: usize,
    eye: [i32; 3],
    yaw: u16,
    pitch: i16,
    movers: &[phys::Mover],
    now: u16,
    health: &mut u16,
    armor: &mut u16,
) {
    let rot = view_rotation(yaw, pitch);
    let base_t = [
        -dot12(rot.m[0], eye),
        -dot12(rot.m[1], eye),
        -dot12(rot.m[2], eye),
    ];
    let mut best = usize::MAX;
    let mut best_score = i32::MAX;
    let mut li = 0usize;
    while li < nlogic {
        if LOGIC_STATE[li] != LOGIC_STATE_REMOVED {
            let rec = m.logic(li);
            if rec.kind == map::LOGIC_FUNC_BUTTON
                || rec.kind == map::LOGIC_FUNC_DOOR
                || rec.kind == map::LOGIC_HEALTH_CHARGER
                || rec.kind == map::LOGIC_HEV_CHARGER
            {
                let c = logic_center(rec);
                let vz = dot12(rot.m[2], c) + base_t[2];
                if vz > 0 && vz <= PLAYER_USE_REACH {
                    let vx = dot12(rot.m[0], c) + base_t[0];
                    let vy = dot12(rot.m[1], c) + base_t[1];
                    if vx.abs() * 3 < vz * 2 && vy.abs() * 3 < vz * 2 {
                        let score = vz + vx.abs() + vy.abs();
                        if score < best_score
                            && phys::line_clear_world(m, eye, c)
                            && phys::line_clear_movers(m, movers, eye, c)
                        {
                            best = li;
                            best_score = score;
                        }
                    }
                }
            }
        }
        li += 1;
    }
    if best != usize::MAX {
        let rec = m.logic(best);
        match rec.kind {
            // Wall chargers drain their juice into the player per use pulse.
            map::LOGIC_HEALTH_CHARGER => {
                if LOGIC_COUNTER[best] > 0 && *health < PLAYER_START_HEALTH {
                    let give = (CHARGER_RATE as i16).min(LOGIC_COUNTER[best]) as u16;
                    let give = give.min(PLAYER_START_HEALTH - *health);
                    *health += give;
                    LOGIC_COUNTER[best] -= give as i16;
                    sfx::play(sfx::MEDSHOT);
                }
            }
            map::LOGIC_HEV_CHARGER => {
                if LOGIC_COUNTER[best] > 0 && *armor < HEV_MAX_ARMOR {
                    let give = (CHARGER_RATE as i16).min(LOGIC_COUNTER[best]) as u16;
                    let give = give.min(HEV_MAX_ARMOR - *armor);
                    *armor += give;
                    LOGIC_COUNTER[best] -= give as i16;
                    sfx::play(sfx::MEDSHOT);
                }
            }
            _ => logic_use_entity(m, nlogic, nents, best, map::USE_TOGGLE, now, 0),
        }
    }
}

unsafe fn logic_touch_triggers(
    m: &Map,
    nlogic: usize,
    nents: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    now: u16,
) {
    let mut li = 0usize;
    while li < nlogic {
        if LOGIC_STATE[li] != LOGIC_STATE_REMOVED {
            let rec = m.logic(li);
            match rec.kind {
                map::LOGIC_TRIGGER_ONCE
                | map::LOGIC_TRIGGER_MULTIPLE
                | map::LOGIC_TRIGGER_CHANGELEVEL => {
                    if LOGIC_STATE[li] != LOGIC_STATE_WAITING
                        && player_touches_logic(player_pos, rec)
                    {
                        logic_use_entity(m, nlogic, nents, li, map::USE_TOGGLE, now, 0);
                        if rec.kind == map::LOGIC_TRIGGER_ONCE {
                            LOGIC_STATE[li] = LOGIC_STATE_REMOVED;
                        } else if rec.kind == map::LOGIC_TRIGGER_MULTIPLE && rec.wait_ticks >= 0 {
                            LOGIC_STATE[li] = LOGIC_STATE_WAITING;
                            LOGIC_NEXT[li] = now.wrapping_add(rec.wait_ticks as u16);
                        }
                    }
                }
                map::LOGIC_FUNC_BUTTON => {
                    if (rec.spawnflags & SF_BUTTON_TOUCH_ONLY) != 0
                        && player_touches_logic(player_pos, rec)
                    {
                        logic_activate_button(m, nlogic, nents, li, rec, now, 0, true);
                    }
                }
                map::LOGIC_FUNC_DOOR => {
                    if rec.targetname == 0
                        && (rec.spawnflags & SF_DOOR_USE_ONLY) == 0
                        && player_touches_logic(player_pos, rec)
                    {
                        logic_activate_door(nents, li, rec, map::USE_TOGGLE);
                    }
                }
                map::LOGIC_TRIGGER_HURT => {
                    if LOGIC_STATE[li] == LOGIC_STATE_BOTTOM
                        && player_touches_logic(player_pos, rec)
                        && time_reached(now, LOGIC_NEXT[li])
                    {
                        damage_player(health, armor, rec.arg0.max(1));
                        logic_sub_use_targets(m, nlogic, nents, li, rec, now, map::USE_TOGGLE, 0);
                        LOGIC_NEXT[li] = now.wrapping_add(TRIGGER_HURT_REPEAT_TICKS);
                        if (rec.spawnflags & SF_TRIGGER_HURT_TARGET_ONCE) != 0 {
                            LOGIC_TARGET[li] = 0;
                        }
                    }
                }
                map::LOGIC_TRIGGER_TELEPORT => {
                    if rec.aux_count >= 2 && player_touches_logic(player_pos, rec) {
                        let a = m.logic_aux(rec.first_aux);
                        let b = m.logic_aux(rec.first_aux + 1);
                        TELEPORT_REQUEST = Some((
                            [
                                a.target as i16 as i32,
                                a.delay_ticks as i16 as i32 + 4, // clear the floor
                                b.target as i16 as i32,
                            ],
                            b.delay_ticks, // destination yaw (q12)
                        ));
                    }
                }
                map::LOGIC_TRIGGER_PUSH => {
                    if rec.aux_count >= 2 && player_touches_logic(player_pos, rec) {
                        let a = m.logic_aux(rec.first_aux);
                        let b = m.logic_aux(rec.first_aux + 1);
                        PUSH_IMPULSE = [
                            a.target as i16 as i32,
                            a.delay_ticks as i16 as i32,
                            b.target as i16 as i32,
                        ];
                    }
                }
                map::LOGIC_TRIGGER_GRAVITY => {
                    if player_touches_logic(player_pos, rec) {
                        phys::set_gravity_scale(rec.arg0 as i32);
                    }
                }
                _ => {}
            }
        }
        li += 1;
    }
}

unsafe fn logic_find_matching_prop(kind: u8, origin: [i32; 3]) -> u8 {
    let Some(prop_kind) = logic_item_prop_kind(kind) else {
        return LOGIC_PROP_NONE;
    };
    let nprops = PROP_COUNT.min(MAX_PROPS);
    let mut pi = 0usize;
    while pi < nprops {
        let pos = PROP_POS[pi];
        let near_origin = (pos[0] - origin[0]).abs() <= PROP_LINK_MATCH_XZ_EPS
            && (pos[1] - origin[1]).abs() <= PROP_LINK_MATCH_Y_EPS
            && (pos[2] - origin[2]).abs() <= PROP_LINK_MATCH_XZ_EPS;
        if PROP_KIND[pi] == prop_kind && near_origin && PROP_LOGIC_LINK[pi] == u16::MAX {
            return pi as u8;
        }
        pi += 1;
    }
    LOGIC_PROP_NONE
}

unsafe fn init_logic_state(m: &Map, nlogic: usize, nents: usize, now: u16) {
    TRACKTRAIN_SUBMODEL = m.tram_submodel.min(u16::MAX as usize) as u16;
    TRACKTRAIN_CMD_ACTIVE = 0;
    TRACKTRAIN_CMD_USE_TYPE = map::USE_TOGGLE;
    TRACKTRAIN_CMD_SPEED = 0;
    let mut ei = 0usize;
    while ei < MAX_ENTS {
        ENT_ACTIVE[ei] = if ei < nents { 1 } else { 0 };
        ENT_PHASE[ei] = 0;
        ENT_PREV_OFF[ei] = if ei < nents {
            ent_draw_offset(ei)
        } else {
            [0; 3]
        };
        ENT_BREAK_LOGIC[ei] = u16::MAX;
        ei += 1;
    }
    let mut li = 0usize;
    while li < MAX_LOGIC {
        LOGIC_STATE[li] = LOGIC_STATE_BOTTOM;
        LOGIC_NEXT[li] = 0;
        LOGIC_TARGET[li] = 0;
        LOGIC_COUNTER[li] = 0;
        LOGIC_BREAK_HP[li] = 0;
        LOGIC_PROP_LINK[li] = LOGIC_PROP_NONE;
        li += 1;
    }
    let mut ev = 0usize;
    while ev < MAX_LOGIC_EVENTS {
        LOGIC_EVENTS[ev] = EMPTY_LOGIC_EVENT;
        ev += 1;
    }
    li = 0;
    while li < nlogic {
        let rec = m.logic(li);
        LOGIC_TARGET[li] = rec.target;
        LOGIC_COUNTER[li] = match rec.kind {
            map::LOGIC_TRIGGER_COUNTER => (rec.arg0 as i16).max(1),
            // Chargers store their remaining juice here (never counters).
            map::LOGIC_HEALTH_CHARGER | map::LOGIC_HEV_CHARGER => rec.arg0 as i16,
            _ => 0,
        };
        match rec.kind {
            map::LOGIC_FUNC_DOOR => {
                if let Some(ei) = logic_valid_brush(rec.brush, nents) {
                    if (rec.spawnflags & SF_DOOR_START_OPEN) != 0 {
                        LOGIC_STATE[li] = LOGIC_STATE_TOP;
                        ENT_PHASE[ei] = 4096;
                    }
                }
            }
            map::LOGIC_TRIGGER_AUTO => {
                let at = now.wrapping_add(rec.delay_ticks.max(1));
                logic_enqueue_event(at, rec.target, rec.killtarget, rec.use_type);
            }
            map::LOGIC_ITEM_SUIT | map::LOGIC_ITEM_BATTERY => {
                let pi = logic_find_matching_prop(rec.kind, rec.origin);
                LOGIC_PROP_LINK[li] = pi;
                if pi != LOGIC_PROP_NONE {
                    PROP_LOGIC_LINK[pi as usize] = li as u16;
                }
            }
            map::LOGIC_TRIGGER_HURT => {
                if (rec.spawnflags & SF_TRIGGER_HURT_START_OFF) != 0 {
                    LOGIC_STATE[li] = LOGIC_STATE_TOP;
                }
            }
            map::LOGIC_FUNC_BREAKABLE => {
                LOGIC_BREAK_HP[li] = rec.arg0.max(1);
                if let Some(ei) = logic_valid_brush(rec.brush, nents) {
                    ENT_BREAK_LOGIC[ei] = li as u16;
                }
            }
            map::LOGIC_FUNC_TRACKTRAIN => {
                if rec.arg1 != 0 && rec.arg1 == TRACKTRAIN_SUBMODEL {
                    TRACKTRAIN_USE_SPEED = rec.speed.max(40); // player drive speed
                    if rec.arg0 > 0 {
                        TRACKTRAIN_CMD_ACTIVE = 1;
                        TRACKTRAIN_CMD_USE_TYPE = map::USE_ON;
                        TRACKTRAIN_CMD_SPEED = rec.arg0;
                    }
                }
            }
            _ => {}
        }
        li += 1;
    }
}

/// Cull when the screen triangle isn't front-facing (area <= 0). Matches the
/// cook's reversed winding.
#[inline]
fn culled(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> bool {
    (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1) <= 0
}

/// DEBUG: is screen point `p` inside triangle (a,b,c)? (winding-agnostic)
fn point_in_tri(p: (i32, i32), a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> bool {
    let s = |u: (i32, i32), v: (i32, i32)| (v.0 - u.0) * (p.1 - u.1) - (v.1 - u.1) * (p.0 - u.0);
    let d1 = s(a, b);
    let d2 = s(b, c);
    let d3 = s(c, a);
    let neg = d1 < 0 || d2 < 0 || d3 < 0;
    let pos = d1 > 0 || d2 > 0 || d3 > 0;
    !(neg && pos)
}

/// DEBUG: record world tri `tt` if it contains the screen centre and is nearest.
#[inline]
unsafe fn xhair_consider(m: &Map, tt: usize, pa: Projected, pb: Projected, pc: Projected) {
    if !DEBUG_XHAIR {
        return;
    }
    // Skip only near-plane crossers (garbage projection); clamped/off-screen-wide
    // tris keep valid screen coords, so they're still pickable.
    if pa.sz < NEAR || pb.sz < NEAR || pc.sz < NEAR {
        return;
    }
    let depth = (pa.sz as u32 + pb.sz as u32 + pc.sz as u32) / 3;
    if depth >= XHAIR.depth {
        return;
    }
    let a = (pa.sx as i32, pa.sy as i32);
    let b = (pb.sx as i32, pb.sy as i32);
    let c = (pc.sx as i32, pc.sy as i32);
    // Skip degenerate (zero/sub-pixel screen area) tris: point_in_tri treats them
    // as containing every point, so they'd spuriously win the pick (and there are
    // ~1000 zero-area cooked tris). Only real surfaces should be picked.
    let cross = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
    if cross.abs() < 4 {
        return;
    }
    if point_in_tri((render::OFX, render::OFY), a, b, c) {
        let idx = m.tri_idx(tt);
        XHAIR.valid = 1;
        XHAIR.tt = tt as u32;
        XHAIR.idx = [idx[0] as u32, idx[1] as u32, idx[2] as u32];
        XHAIR.sv = [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)];
        XHAIR.depth = depth;
        XHAIR.tex = m.tri_tex(tt) as u32;
    }
}

/// DEBUG: fallback crosshair pick. The in-emit pick only sees triangles that
/// reach an emit path -- quads (the dominant world path) and backface-culled
/// faces are invisible to it, so a *missing* triangle never highlights. When the
/// draw picked nothing (crosshair over a gap or a quad), sweep the PVS face set
/// -- which includes culled faces -- for the nearest tri under the crosshair, so
/// even an undrawn triangle gets highlighted and dumped. The caller must reload
/// the world GTE transform first (the entity passes leave their own loaded).
unsafe fn xhair_pick_pvs(m: &Map, nv: usize, frame: u16) {
    let mut e = 0usize;
    while e < PVS_FACE_COUNT && e < MAX_FACES {
        let face = PVS_FACE_INDEX[e] as usize;
        e += 1;
        if m.face_is_loop(face) {
            continue; // debug crosshair pick reads raw tris only (loop faces skip)
        }
        let (first, cnt) = m.face_tris(face);
        let end = first + cnt;
        let mut tt = first;
        while tt < end && tt < m.n_tris {
            let t = tt;
            tt += 1;
            let idx = m.tri_idx(t);
            let (a, b, c) = (idx[0] as usize, idx[1] as usize, idx[2] as usize);
            if a >= nv || b >= nv || c >= nv {
                continue;
            }
            proj_vert(m, a, frame);
            proj_vert(m, b, frame);
            proj_vert(m, c, frame);
            xhair_consider(m, t, SCRATCH[a], SCRATCH[b], SCRATCH[c]);
        }
    }
}

/// DEBUG: minimal no_std i32 -> decimal in a stack buffer.
fn fmt_i32(v: i32, buf: &mut [u8; 12]) -> &str {
    let mut i = buf.len();
    let neg = v < 0;
    let mut u = (v as i64).unsigned_abs();
    loop {
        i -= 1;
        buf[i] = b'0' + (u % 10) as u8;
        u /= 10;
        if u == 0 {
            break;
        }
    }
    if neg && i > 0 {
        i -= 1;
        buf[i] = b'-';
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("?")
}

/// DEBUG: append "label<value> " into `buf` at `*n` (saturating).
fn append_kv(buf: &mut [u8], n: &mut usize, label: &str, v: i32) {
    let mut nb = [0u8; 12];
    for &c in label.as_bytes().iter().chain(fmt_i32(v, &mut nb).as_bytes()) {
        if *n < buf.len() {
            buf[*n] = c;
            *n += 1;
        }
    }
    if *n < buf.len() {
        buf[*n] = b' ';
        *n += 1;
    }
}

/// DEBUG: one compact line per dump -> PSoXide's Play debug terminal. Two lines
/// (the showing frame and the missing frame) are easy to eyeball side by side.
unsafe fn xhair_dump(eye: [i32; 3], yaw: u16, pitch: i16, cam_leaf: i32, prims: i32, vis_faces: i32) {
    let mut buf = [0u8; 192];
    let mut n = 0usize;
    append_kv(&mut buf, &mut n, "XH v=", XHAIR.valid as i32);
    append_kv(&mut buf, &mut n, "tri=", XHAIR.tt as i32);
    append_kv(&mut buf, &mut n, "tex=", XHAIR.tex as i32);
    append_kv(&mut buf, &mut n, "tleaf=", XHAIR.leaf);
    append_kv(&mut buf, &mut n, "pvs=", XHAIR.pvs_visible as i32);
    append_kv(&mut buf, &mut n, "| camleaf=", cam_leaf);
    append_kv(&mut buf, &mut n, "x=", eye[0]);
    append_kv(&mut buf, &mut n, "y=", eye[1]);
    append_kv(&mut buf, &mut n, "z=", eye[2]);
    append_kv(&mut buf, &mut n, "yaw=", yaw as i32);
    append_kv(&mut buf, &mut n, "pit=", pitch as i32);
    // prims emitted this frame vs the MAX_RENDER_PACKETS arena cap; vis_faces vs
    // PVS_FACE budget. If prims is pinned at the cap, the arena overflowed and
    // geometry past it was dropped (black voids).
    append_kv(&mut buf, &mut n, "| prims=", prims);
    append_kv(&mut buf, &mut n, "/", MAX_RENDER_PACKETS as i32);
    append_kv(&mut buf, &mut n, "nf=", vis_faces);
    // Guest debug-log port: the PSoXide emulator frontend now eprintln's these to
    // the host console (drain_debug_logs), so they show in the terminal in the
    // plain library frontend too, not just the editor's Play debug terminal.
    telemetry::console(core::str::from_utf8(&buf[..n]).unwrap_or("?"));
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
    model_def(ty).health
}

#[inline]
fn prop_target(ty: u8, org: [i32; 3]) -> [i32; 3] {
    [org[0], org[1] + model_def(ty).target_h, org[2]]
}

#[inline]
fn prop_occlusion_visible(m: &Map, eye: [i32; 3], ty: u8, org: [i32; 3]) -> bool {
    if !MODEL_OCCLUSION_CULL {
        return true;
    }
    if phys::line_clear_world(m, eye, prop_target(ty, org)) {
        return true;
    }
    phys::line_clear_world(m, eye, [org[0], org[1] + (PROP_TARGET_HEIGHT / 2), org[2]])
}

fn prop_clip(state: u8, hit_flash: bool) -> usize {
    if state == PROP_STATE_DEAD {
        return PROP_CLIP_DEAD;
    }
    if hit_flash {
        return PROP_CLIP_PAIN;
    }
    match state {
        PROP_STATE_MOVE => PROP_CLIP_MOVE,
        PROP_STATE_ATTACK => PROP_CLIP_ATTACK,
        _ => PROP_CLIP_IDLE,
    }
}

fn prop_anim_frame(
    md: &Model,
    ty: u8,
    state: u8,
    hit_flash: u8,
    sim_frame_no: u32,
    pi: usize,
) -> usize {
    let clip = prop_clip(state, hit_flash > 0);
    let len = md.clip_len(clip);
    if state == PROP_STATE_DEAD {
        return md.clip_frame(clip, len.saturating_sub(1));
    }
    if hit_flash > 0 {
        let pain_frame = PROP_HIT_FLASH_TICKS.saturating_sub(hit_flash) as usize;
        return md.clip_frame(clip, pain_frame.min(len.saturating_sub(1)));
    }
    let phase = sim_frame_no as usize + pi.wrapping_mul(3);
    let div = if state == PROP_STATE_ATTACK || ty == PROP_TYPE_HEADCRAB && state == PROP_STATE_MOVE
    {
        1
    } else if state == PROP_STATE_MOVE {
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

#[inline]
fn prop_is_human(ty: u8) -> bool {
    ty == PROP_TYPE_SCIENTIST || ty == PROP_TYPE_BARNEY
}

#[inline]
fn actor_line_clear(m: &Map, movers: &[phys::Mover], from: [i32; 3], to: [i32; 3]) -> bool {
    phys::line_clear_world(m, from, to) && phys::line_clear_movers(m, movers, from, to)
}

/// Find the world floor surface Y directly under `pos` by point-tracing the BSP
/// node tree (leaf 0 == solid, the same tree `camera_leaf` walks). The cook
/// conflates `dmodel.headnode[0]` (a node index) with the clipnode array, so the
/// emitted `hull0_head` aliases the player hull and `phys::snap_to_ground`
/// always returned None -- which left every prop floating at its raw entity
/// origin (7..112 units above the floor). Models are floor-anchored (feet at
/// y==0), so dropping the origin onto the floor seats the feet.
/// ponytail: world-only -- props on moving platforms (movers) aren't tracked;
/// rare for placed NPCs/items. Scan step 8u, refined to ~1u.
fn prop_floor_y(m: &Map, pos: [i32; 3]) -> Option<i32> {
    let (x, z) = (pos[0], pos[2]);
    let top = pos[1] + PROP_GROUND_PROBE_UP;
    if camera_leaf(m, [x, top, z]) == 0 {
        return None; // headroom is solid -- no clean floor to drop onto
    }
    let bottom = pos[1] - PROP_GROUND_PROBE_DOWN;
    let mut empty_y = top;
    let mut y = top - GROUND_SCAN_STEP;
    while y >= bottom {
        if camera_leaf(m, [x, y, z]) == 0 {
            // First solid below: the floor is between y (solid) and empty_y. Refine.
            let (mut solid, mut empty) = (y, empty_y);
            for _ in 0..4 {
                let mid = (solid + empty) / 2;
                if camera_leaf(m, [x, mid, z]) == 0 {
                    solid = mid;
                } else {
                    empty = mid;
                }
            }
            return Some(empty); // lowest empty = floor surface
        }
        empty_y = y;
        y -= GROUND_SCAN_STEP;
    }
    None // no floor within probe range
}

#[inline]
fn prop_grounded_pos(m: &Map, _movers: &[phys::Mover], pos: [i32; 3]) -> [i32; 3] {
    match prop_floor_y(m, pos) {
        Some(y) => [pos[0], y, pos[2]],
        None => pos,
    }
}

unsafe fn prop_set_pos(m: &Map, movers: &[phys::Mover], pi: usize, pos: [i32; 3]) {
    let pos = prop_grounded_pos(m, movers, pos);
    PROP_POS[pi] = pos;
    let leaf = camera_leaf(m, pos);
    PROP_LEAF[pi] = if leaf > 0 && leaf <= i16::MAX as i32 {
        leaf as i16
    } else {
        0
    };
}

unsafe fn prop_face_point(pi: usize, p: [i32; 3]) {
    let pos = PROP_POS[pi];
    let dx = p[0] - pos[0];
    let dz = p[2] - pos[2];
    if dx != 0 || dz != 0 {
        PROP_YAW[pi] = yaw_from_vec(dx, dz);
    }
}

unsafe fn prop_try_step(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    dx: i32,
    dz: i32,
    speed: i32,
) -> bool {
    if speed <= 0 {
        return false;
    }
    let dirs = [
        [dx, dz],
        [dz, -dx],
        [-dz, dx],
        [dx + (dz >> 1), dz - (dx >> 1)],
        [dx - (dz >> 1), dz + (dx >> 1)],
    ];
    let ty = PROP_KIND[pi];
    let pos = PROP_POS[pi];
    let from = prop_target(ty, pos);
    let mut i = 0usize;
    while i < dirs.len() {
        let sx = dirs[i][0];
        let sz = dirs[i][1];
        let d2 = sx * sx + sz * sz;
        if d2 > 0 {
            let len = isqrt(d2).max(1);
            let step = speed.min(len);
            let cand = [pos[0] + sx * step / len, pos[1], pos[2] + sz * step / len];
            // Walkers refuse a step with no floor under it (HL CheckLocalMove):
            // accepting it froze the actor's height and sent it chasing on air
            // over pits/ledges. The flying controller keeps its altitude.
            let np = match prop_floor_y(m, cand) {
                Some(y) => [cand[0], y, cand[2]],
                None if ty == PROP_TYPE_CONTROLLER => cand,
                None => {
                    i += 1;
                    continue;
                }
            };
            let to = prop_target(ty, np);
            if actor_line_clear(m, movers, from, to) {
                prop_set_pos(m, movers, pi, np);
                return true;
            }
        }
        i += 1;
    }
    false
}

#[inline]
fn nav_node_count(m: &Map) -> usize {
    m.n_nav.min(MAX_NAV_NODES)
}

unsafe fn nav_nearest(m: &Map, pos: [i32; 3], max_d2: i32) -> u8 {
    let n = nav_node_count(m);
    let mut best = NAV_NODE_NONE;
    let mut best_d2 = max_d2;
    let mut i = 0usize;
    while i < n {
        let node = m.nav_node(i);
        let dy = (node.pos[1] - pos[1]).abs();
        if dy <= NAV_VERTICAL_MAX {
            let d2 = dist2_xz(pos, node.pos);
            if d2 < best_d2 {
                best = i as u8;
                best_d2 = d2;
            }
        }
        i += 1;
    }
    best
}

unsafe fn nav_nearest_reachable(
    m: &Map,
    movers: &[phys::Mover],
    from: [i32; 3],
    pos: [i32; 3],
    max_d2: i32,
) -> u8 {
    let n = nav_node_count(m);
    let mut best = NAV_NODE_NONE;
    let mut best_d2 = max_d2;
    let mut i = 0usize;
    while i < n {
        let node = m.nav_node(i);
        let dy = (node.pos[1] - pos[1]).abs();
        if dy <= NAV_VERTICAL_MAX {
            let d2 = dist2_xz(pos, node.pos);
            let to = [node.pos[0], from[1], node.pos[2]];
            if d2 < best_d2 && actor_line_clear(m, movers, from, to) {
                best = i as u8;
                best_d2 = d2;
            }
        }
        i += 1;
    }
    best
}

unsafe fn nav_next_node(m: &Map, src: u8, dst: u8) -> u8 {
    let n = nav_node_count(m);
    let src_i = src as usize;
    let dst_i = dst as usize;
    if src_i >= n || dst_i >= n {
        return NAV_NODE_NONE;
    }
    if src == dst {
        return src;
    }

    let mut i = 0usize;
    while i < n {
        NAV_PREV[i] = NAV_NODE_NONE;
        i += 1;
    }

    let mut head = 0usize;
    let mut tail = 0usize;
    NAV_QUEUE[tail] = src;
    tail += 1;
    NAV_PREV[src_i] = src;

    while head < tail {
        let cur = NAV_QUEUE[head];
        head += 1;
        let node = m.nav_node(cur as usize);
        let end = node.first_link.saturating_add(node.link_count);
        let mut li = node.first_link;
        while li < end {
            let next = m.nav_link(li);
            if next < n && NAV_PREV[next] == NAV_NODE_NONE {
                NAV_PREV[next] = cur;
                if next == dst_i {
                    let mut step = dst;
                    while NAV_PREV[step as usize] != src {
                        step = NAV_PREV[step as usize];
                        if step == NAV_NODE_NONE {
                            return NAV_NODE_NONE;
                        }
                    }
                    return step;
                }
                if tail < MAX_NAV_NODES {
                    NAV_QUEUE[tail] = next as u8;
                    tail += 1;
                }
            }
            li += 1;
        }
    }
    NAV_NODE_NONE
}

unsafe fn nav_waypoint_towards(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    goal: [i32; 3],
) -> Option<[i32; 3]> {
    if m.n_nav == 0 {
        return None;
    }
    let ty = PROP_KIND[pi];
    let pos = PROP_POS[pi];
    let from = prop_target(ty, pos);
    let src = nav_nearest_reachable(m, movers, from, pos, NAV_NEAREST_RANGE2);
    if src == NAV_NODE_NONE {
        return None;
    }
    let dst = nav_nearest(m, goal, NAV_NEAREST_RANGE2);
    if dst == NAV_NODE_NONE {
        return None;
    }

    let src_pos = m.nav_node(src as usize).pos;
    if dist2_xz(pos, src_pos) > NAV_NODE_REACHED_RANGE2
        || (pos[1] - src_pos[1]).abs() > NAV_VERTICAL_MAX
    {
        return Some(src_pos);
    }
    if src == dst {
        return Some(goal);
    }

    let next = nav_next_node(m, src, dst);
    if next == NAV_NODE_NONE {
        None
    } else {
        Some(m.nav_node(next as usize).pos)
    }
}

unsafe fn nav_flee_goal(m: &Map, threat: [i32; 3], pos: [i32; 3]) -> Option<[i32; 3]> {
    let n = nav_node_count(m);
    let current_threat_d2 = dist2_xz(pos, threat);
    let mut best = NAV_NODE_NONE;
    let mut best_score = current_threat_d2;
    let mut i = 0usize;
    while i < n {
        let node = m.nav_node(i);
        if (node.pos[1] - pos[1]).abs() <= NAV_VERTICAL_MAX
            && dist2_xz(pos, node.pos) <= NAV_FLEE_RANGE2
        {
            let score = dist2_xz(node.pos, threat);
            if score > best_score {
                best = i as u8;
                best_score = score;
            }
        }
        i += 1;
    }
    if best == NAV_NODE_NONE {
        None
    } else {
        Some(m.nav_node(best as usize).pos)
    }
}

unsafe fn prop_move_towards_point(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    goal: [i32; 3],
    speed: i32,
) -> bool {
    let pos = PROP_POS[pi];
    prop_face_point(pi, goal);
    if prop_try_step(m, movers, pi, goal[0] - pos[0], goal[2] - pos[2], speed) {
        return true;
    }
    if let Some(wp) = nav_waypoint_towards(m, movers, pi, goal) {
        let pos = PROP_POS[pi];
        prop_face_point(pi, wp);
        prop_try_step(m, movers, pi, wp[0] - pos[0], wp[2] - pos[2], speed)
    } else {
        false
    }
}

unsafe fn target_org(target: u8, player_pos: [i32; 3], nprops: usize) -> Option<[i32; 3]> {
    if target == PROP_TARGET_PLAYER {
        return Some(player_pos);
    }
    let ti = target as usize;
    if ti < nprops && PROP_HEALTH[ti] > 0 {
        Some(PROP_POS[ti])
    } else {
        None
    }
}

unsafe fn target_aim_point(target: u8, player_pos: [i32; 3], nprops: usize) -> Option<[i32; 3]> {
    if target == PROP_TARGET_PLAYER {
        return Some([player_pos[0], player_pos[1] + VIEW_HEIGHT, player_pos[2]]);
    }
    let ti = target as usize;
    if ti < nprops && PROP_HEALTH[ti] > 0 {
        Some(prop_target(PROP_KIND[ti], PROP_POS[ti]))
    } else {
        None
    }
}

unsafe fn damage_prop(pi: usize, dmg: u8) {
    if pi >= MAX_PROPS || PROP_ACTIVE[pi] == 0 || PROP_HEALTH[pi] == 0 {
        return;
    }
    PROP_HEALTH[pi] = PROP_HEALTH[pi].saturating_sub(dmg);
    PROP_HIT_FLASH[pi] = PROP_HIT_FLASH_TICKS;
    if PROP_HEALTH[pi] == 0 {
        PROP_STATE[pi] = PROP_STATE_DEAD;
        PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        PROP_AI_TIMER[pi] = 0;
        sfx::play_world(sfx::BODYDROP, PROP_POS[pi]);
    }
}

static mut PAIN_SFX_COOLDOWN: u8 = 0;

fn damage_player(health: &mut u16, armor: &mut u16, dmg: u16) {
    if dmg == 0 {
        return;
    }
    // Pain grunt, rate-limited so per-tick hazards (trigger_hurt) don't spam.
    unsafe {
        if PAIN_SFX_COOLDOWN == 0 {
            sfx::play(sfx::PAIN);
            PAIN_SFX_COOLDOWN = 12;
        }
    }
    if *armor > 0 {
        // GoldSrc's HEV suit keeps only 20% of generic damage on health and
        // spends half of the remaining damage as suit power.
        let health_dmg = dmg / 5;
        let armor_cost = (dmg.saturating_sub(health_dmg).saturating_add(1)) / 2;
        if armor_cost <= *armor {
            *armor -= armor_cost;
            *health = health.saturating_sub(health_dmg);
            return;
        }

        let absorbed = (*armor).saturating_mul(2);
        *armor = 0;
        *health = health.saturating_sub(dmg.saturating_sub(absorbed));
    } else {
        *health = health.saturating_sub(dmg);
    }
}

unsafe fn damage_target(target: u8, dmg: u8, health: &mut u16, armor: &mut u16) {
    if target == PROP_TARGET_PLAYER {
        damage_player(health, armor, dmg as u16);
    } else if target != PROP_TARGET_NONE {
        damage_prop(target as usize, dmg);
    }
}

unsafe fn find_headcrab_target(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    nprops: usize,
) -> u8 {
    let pos = PROP_POS[pi];
    let from = prop_target(PROP_TYPE_HEADCRAB, pos);
    let mut best_visible = PROP_TARGET_NONE;
    let mut best_visible_d2 = HEADCRAB_WAKE_RANGE2;
    let mut best_any = PROP_TARGET_NONE;
    let mut best_any_d2 = HEADCRAB_WAKE_RANGE2;

    let player_d2 = dist2_xz(pos, player_pos);
    if player_d2 < best_any_d2 {
        best_any = PROP_TARGET_PLAYER;
        best_any_d2 = player_d2;
    }
    if player_d2 < best_visible_d2 {
        let to = [player_pos[0], player_pos[1] + VIEW_HEIGHT, player_pos[2]];
        if actor_line_clear(m, movers, from, to) {
            best_visible = PROP_TARGET_PLAYER;
            best_visible_d2 = player_d2;
        }
    }

    let mut ti = 0usize;
    while ti < nprops {
        if ti != pi && PROP_HEALTH[ti] > 0 && prop_is_human(PROP_KIND[ti]) {
            let d2 = dist2_xz(pos, PROP_POS[ti]);
            if d2 < best_any_d2 {
                best_any = ti as u8;
                best_any_d2 = d2;
            }
            if d2 < best_visible_d2 {
                let to = prop_target(PROP_KIND[ti], PROP_POS[ti]);
                if actor_line_clear(m, movers, from, to) {
                    best_visible = ti as u8;
                    best_visible_d2 = d2;
                }
            }
        }
        ti += 1;
    }
    if best_visible != PROP_TARGET_NONE {
        best_visible
    } else {
        best_any
    }
}

unsafe fn find_barney_target(m: &Map, movers: &[phys::Mover], pi: usize, nprops: usize) -> u8 {
    let pos = PROP_POS[pi];
    let from = prop_target(PROP_TYPE_BARNEY, pos);
    let mut best = PROP_TARGET_NONE;
    let mut best_d2 = BARNEY_ATTACK_RANGE2;
    let mut ti = 0usize;
    while ti < nprops {
        if ti != pi && PROP_HEALTH[ti] > 0 && PROP_KIND[ti] == PROP_TYPE_HEADCRAB {
            let d2 = dist2_xz(pos, PROP_POS[ti]);
            if d2 < best_d2 {
                let to = prop_target(PROP_TYPE_HEADCRAB, PROP_POS[ti]);
                if actor_line_clear(m, movers, from, to) {
                    best = ti as u8;
                    best_d2 = d2;
                }
            }
        }
        ti += 1;
    }
    best
}

unsafe fn find_scientist_threat(m: &Map, movers: &[phys::Mover], pi: usize, nprops: usize) -> u8 {
    let pos = PROP_POS[pi];
    let from = prop_target(PROP_TYPE_SCIENTIST, pos);
    let mut best = PROP_TARGET_NONE;
    let mut best_d2 = SCIENTIST_FEAR_RANGE2;
    let mut ti = 0usize;
    while ti < nprops {
        if ti != pi && PROP_HEALTH[ti] > 0 && PROP_KIND[ti] == PROP_TYPE_HEADCRAB {
            let d2 = dist2_xz(pos, PROP_POS[ti]);
            if d2 < best_d2 {
                let to = prop_target(PROP_TYPE_HEADCRAB, PROP_POS[ti]);
                if actor_line_clear(m, movers, from, to) {
                    best = ti as u8;
                    best_d2 = d2;
                }
            }
        }
        ti += 1;
    }
    best
}

/// Target acquisition for hostile actors: nearest player-or-human within `wake2`,
/// preferring one with line of sight. Generalizes find_headcrab_target by wake
/// range and uses the actor's own eye height.
unsafe fn find_actor_target(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    nprops: usize,
    wake2: i32,
) -> u8 {
    let ty = PROP_KIND[pi];
    let pos = PROP_POS[pi];
    let from = prop_target(ty, pos);
    let mut best_visible = PROP_TARGET_NONE;
    let mut best_visible_d2 = wake2;
    let mut best_any = PROP_TARGET_NONE;
    let mut best_any_d2 = wake2;

    let player_d2 = dist2_xz(pos, player_pos);
    if player_d2 < best_any_d2 {
        best_any = PROP_TARGET_PLAYER;
        best_any_d2 = player_d2;
    }
    if player_d2 < best_visible_d2 {
        let to = [player_pos[0], player_pos[1] + VIEW_HEIGHT, player_pos[2]];
        if actor_line_clear(m, movers, from, to) {
            best_visible = PROP_TARGET_PLAYER;
            best_visible_d2 = player_d2;
        }
    }

    let mut ti = 0usize;
    while ti < nprops {
        if ti != pi && PROP_HEALTH[ti] > 0 && prop_is_human(PROP_KIND[ti]) {
            let d2 = dist2_xz(pos, PROP_POS[ti]);
            if d2 < best_any_d2 {
                best_any = ti as u8;
                best_any_d2 = d2;
            }
            if d2 < best_visible_d2 {
                let to = prop_target(PROP_KIND[ti], PROP_POS[ti]);
                if actor_line_clear(m, movers, from, to) {
                    best_visible = ti as u8;
                    best_visible_d2 = d2;
                }
            }
        }
        ti += 1;
    }
    if best_visible != PROP_TARGET_NONE {
        best_visible
    } else {
        best_any
    }
}

/// Ranged + turret AI. Acquire a target, face it, and fire a hitscan shot every
/// `atk_cooldown` ticks while it is within `atk_range` and in line of sight.
/// Movers (`can_move`) close to range first; turrets (`!can_move`) hold still.
unsafe fn tick_shooter(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
    can_move: bool,
) {
    let ty = PROP_KIND[pi];
    let def = model_def(ty);
    let range = def.atk_range as i32;
    let range2 = range.saturating_mul(range);
    // Wake a little beyond firing range so movers start closing; turrets only
    // engage once the target is actually inside range.
    let wake = if can_move { range + 384 } else { range };
    let wake2 = wake.saturating_mul(wake);

    let target = if ai_reacquire(pi) {
        find_actor_target(m, movers, pi, player_pos, nprops, wake2)
    } else {
        PROP_AI_TARGET[pi]
    };
    if target == PROP_TARGET_NONE {
        PROP_STATE[pi] = PROP_STATE_IDLE;
        PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        return;
    }
    let Some(aim) = target_aim_point(target, player_pos, nprops) else {
        PROP_STATE[pi] = PROP_STATE_IDLE;
        PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        return;
    };
    PROP_AI_TARGET[pi] = target;
    prop_face_point(pi, aim);

    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    let from = prop_target(ty, pos);
    let visible = actor_line_clear(m, movers, from, aim);

    if d2 <= range2 && visible {
        // In range + line of sight: hold and fire on the cooldown. The attack
        // clip plays while STATE_ATTACK; the hit is instant (hitscan).
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        if PROP_ATTACK_COOLDOWN[pi] == 0 {
            damage_target(target, def.atk_damage, health, armor);
            PROP_ATTACK_COOLDOWN[pi] = def.atk_cooldown;
            // Human weapons crack like an MP5; alien ranged attacks zap.
            let snd = if ty == 8 || ty >= 20 { sfx::MP5 } else { sfx::ELECTRO };
            sfx::play_world(snd, pos);
        }
    } else if can_move {
        PROP_STATE[pi] = PROP_STATE_MOVE;
        prop_move_towards_point(m, movers, pi, aim, def.speed as i32);
    } else {
        PROP_STATE[pi] = PROP_STATE_IDLE;
    }
}

unsafe fn tick_headcrab(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    if PROP_STATE[pi] == PROP_STATE_ATTACK && PROP_AI_TIMER[pi] > 0 {
        let target = PROP_AI_TARGET[pi];
        if let Some(aim) = target_aim_point(target, player_pos, nprops) {
            prop_face_point(pi, aim);
            if PROP_AI_TIMER[pi] > HEADCRAB_ATTACK_IMPACT_TICK {
                let pos = PROP_POS[pi];
                prop_try_step(
                    m,
                    movers,
                    pi,
                    aim[0] - pos[0],
                    aim[2] - pos[2],
                    HEADCRAB_LEAP_SPEED,
                );
            } else if PROP_AI_TIMER[pi] == HEADCRAB_ATTACK_IMPACT_TICK {
                let pos = PROP_POS[pi];
                let from = prop_target(PROP_TYPE_HEADCRAB, pos);
                if dist2_xz(pos, aim) <= HEADCRAB_BITE_RANGE2
                    && actor_line_clear(m, movers, from, aim)
                {
                    damage_target(target, HEADCRAB_ATTACK_DAMAGE as u8, health, armor);
                }
            }
        }
        PROP_AI_TIMER[pi] -= 1;
        if PROP_AI_TIMER[pi] == 0 {
            PROP_STATE[pi] = PROP_STATE_IDLE;
        }
        return;
    }

    let target = if ai_reacquire(pi) {
        find_headcrab_target(m, movers, pi, player_pos, nprops)
    } else {
        PROP_AI_TARGET[pi]
    };
    if target == PROP_TARGET_NONE {
        PROP_STATE[pi] = PROP_STATE_IDLE;
        PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        return;
    }

    let Some(aim) = target_aim_point(target, player_pos, nprops) else {
        PROP_STATE[pi] = PROP_STATE_IDLE;
        PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    prop_face_point(pi, aim);
    PROP_AI_TARGET[pi] = target;
    let from = prop_target(PROP_TYPE_HEADCRAB, pos);
    let visible = actor_line_clear(m, movers, from, aim);
    if d2 <= HEADCRAB_LEAP_RANGE2 && PROP_ATTACK_COOLDOWN[pi] == 0 && visible {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = HEADCRAB_ATTACK_TICKS;
        PROP_ATTACK_COOLDOWN[pi] = HEADCRAB_ATTACK_COOLDOWN;
        let snd = match PROP_KIND[pi] {
            5 => sfx::ZO_ATTACK,   // zombie swipe
            6 => sfx::HE_BLAST,    // houndeye sonic blast
            _ => sfx::HC_ATTACK,   // headcrab-family shriek
        };
        sfx::play_world(snd, pos);
    } else {
        PROP_STATE[pi] = PROP_STATE_MOVE;
        let step_d2 = d2.saturating_sub(HEADCRAB_STOP_RANGE * HEADCRAB_STOP_RANGE);
        if step_d2 > 0 {
            prop_move_towards_point(m, movers, pi, aim, HEADCRAB_SPEED);
        }
    }
}

unsafe fn tick_barney(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    if PROP_AI_TIMER[pi] > 0 {
        PROP_AI_TIMER[pi] -= 1;
    }

    let target = if ai_reacquire(pi) {
        find_barney_target(m, movers, pi, nprops)
    } else {
        PROP_AI_TARGET[pi]
    };
    if target != PROP_TARGET_NONE {
        if let Some(aim) = target_aim_point(target, player_pos, nprops) {
            prop_face_point(pi, aim);
            PROP_AI_TARGET[pi] = target;
            if PROP_ATTACK_COOLDOWN[pi] == 0 {
                PROP_STATE[pi] = PROP_STATE_ATTACK;
                PROP_AI_TIMER[pi] = BARNEY_ATTACK_TICKS;
                PROP_ATTACK_COOLDOWN[pi] = BARNEY_ATTACK_COOLDOWN;
                damage_target(target, BARNEY_DAMAGE, health, armor);
                sfx::play_world(sfx::GLOCK, PROP_POS[pi]);
            } else if PROP_AI_TIMER[pi] > 0 {
                PROP_STATE[pi] = PROP_STATE_ATTACK;
            } else {
                PROP_STATE[pi] = PROP_STATE_IDLE;
            }
            return;
        }
    }

    if PROP_AI_TIMER[pi] > 0 && PROP_STATE[pi] == PROP_STATE_ATTACK {
        return;
    }
    PROP_AI_TARGET[pi] = PROP_TARGET_NONE;

    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, player_pos);
    if d2 < BARNEY_FOLLOW_RANGE2 {
        let player_eye = [player_pos[0], player_pos[1] + VIEW_HEIGHT, player_pos[2]];
        if actor_line_clear(m, movers, prop_target(PROP_TYPE_BARNEY, pos), player_eye) {
            prop_face_point(pi, player_eye);
        } else {
            prop_face_point(pi, player_pos);
        }
        if d2 > BARNEY_STOP_RANGE2 {
            PROP_STATE[pi] = PROP_STATE_MOVE;
            prop_move_towards_point(m, movers, pi, player_pos, BARNEY_SPEED);
            return;
        }
    }
    PROP_STATE[pi] = PROP_STATE_IDLE;
}

unsafe fn tick_scientist(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    nprops: usize,
) {
    let threat = find_scientist_threat(m, movers, pi, nprops);
    if threat != PROP_TARGET_NONE {
        PROP_AI_TARGET[pi] = threat;
        PROP_AI_TIMER[pi] = SCIENTIST_FEAR_TICKS;
    }

    let remembered = PROP_AI_TARGET[pi];
    let flee_from = if PROP_AI_TIMER[pi] > 0 {
        target_org(remembered, player_pos, nprops)
    } else {
        None
    };
    if let Some(threat_pos) = flee_from {
        let pos = PROP_POS[pi];
        let mut dx = pos[0] - threat_pos[0];
        let dz = pos[2] - threat_pos[2];
        if dx == 0 && dz == 0 {
            dx = 1;
        }
        PROP_YAW[pi] = yaw_from_vec(dx, dz);
        PROP_STATE[pi] = PROP_STATE_MOVE;
        if !prop_try_step(m, movers, pi, dx, dz, SCIENTIST_FLEE_SPEED) {
            if let Some(goal) = nav_flee_goal(m, threat_pos, pos) {
                prop_move_towards_point(m, movers, pi, goal, SCIENTIST_FLEE_SPEED);
            }
        }
        if PROP_AI_TIMER[pi] > 0 {
            PROP_AI_TIMER[pi] -= 1;
        }
        if PROP_AI_TIMER[pi] == 0 {
            PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        }
        return;
    }

    let pos = PROP_POS[pi];
    if dist2_xz(pos, player_pos) < SCIENTIST_FACE_RANGE2 {
        let player_eye = [player_pos[0], player_pos[1] + VIEW_HEIGHT, player_pos[2]];
        if actor_line_clear(m, movers, prop_target(PROP_TYPE_SCIENTIST, pos), player_eye) {
            prop_face_point(pi, player_eye);
        }
    }
    PROP_STATE[pi] = PROP_STATE_IDLE;
}

unsafe fn init_prop_state(m: &Map) {
    let mut i = 0usize;
    while i < MAX_PROPS {
        PROP_ACTIVE[i] = 0;
        PROP_KIND[i] = 0;
        PROP_POS[i] = [0, 0, 0];
        PROP_YAW[i] = 0;
        PROP_LEAF[i] = 0;
        PROP_STATE[i] = PROP_STATE_IDLE;
        PROP_ATTACK_COOLDOWN[i] = 0;
        PROP_AI_TIMER[i] = 0;
        PROP_AI_TARGET[i] = PROP_TARGET_NONE;
        PROP_HEALTH[i] = 0;
        PROP_HIT_FLASH[i] = 0;
        PROP_LOGIC_LINK[i] = u16::MAX;
        i += 1;
    }

    PROP_COUNT = 0;
    let nprops = m.n_props.min(MAX_PROPS);
    let mut pi = 0usize;
    while pi < nprops {
        let (ty, org, yaw, leaf) = m.prop(pi);
        let dead = ty & PROP_DEAD_BIT != 0; // authored corpse: death pose, no AI
        let kind = (ty & !PROP_DEAD_BIT) as u8;
        // Sitting scientists are authored at seat height on chair brushes the
        // world tree can't see; snapping would drop them through the chair.
        let org = if kind == PROP_TYPE_SITTING_SCI {
            org
        } else {
            prop_grounded_pos(m, &[], org)
        };
        PROP_ACTIVE[pi] = 1;
        PROP_KIND[pi] = kind;
        PROP_POS[pi] = org;
        PROP_YAW[pi] = yaw as u16;
        let grounded_leaf = camera_leaf(m, org);
        PROP_LEAF[pi] = if grounded_leaf > 0 && grounded_leaf <= i16::MAX as i32 {
            grounded_leaf as i16
        } else {
            leaf
        };
        PROP_HEALTH[pi] = if dead { 0 } else { prop_start_health(kind) };
        if dead {
            PROP_STATE[pi] = PROP_STATE_DEAD;
        }
        PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        PROP_COUNT += 1;
        pi += 1;
    }
}

unsafe fn tick_props(
    m: &Map,
    movers: &[phys::Mover],
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
) {
    AI_TICK = AI_TICK.wrapping_add(1); // drives staggered AI target re-acquisition
    let nprops = PROP_COUNT.min(MAX_PROPS);
    let mut pi = 0usize;
    while pi < nprops {
        if PROP_ACTIVE[pi] == 0 {
            pi += 1;
            continue;
        }
        if PROP_HIT_FLASH[pi] > 0 {
            PROP_HIT_FLASH[pi] -= 1;
        }
        if PROP_ATTACK_COOLDOWN[pi] > 0 {
            PROP_ATTACK_COOLDOWN[pi] -= 1;
        }

        let ty = PROP_KIND[pi];
        if ty == PROP_TYPE_ITEM_SUIT || ty == PROP_TYPE_ITEM_BATTERY {
            pi += 1;
            continue;
        }
        if PROP_HEALTH[pi] == 0 {
            PROP_STATE[pi] = PROP_STATE_DEAD;
            PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
            PROP_AI_TIMER[pi] = 0;
            pi += 1;
            continue;
        }

        // Props use WORLD-ONLY collision (no brush-entity mover hulls). trace_all
        // is O(movers) per trace, so per-prop slide-move (x4 iters) + step + ground
        // x the mover count is O(props x movers). On dense maps this explodes:
        // c1a2 (Office Complex) has 70 props x 169 brush entities = ~100k hull-bbox
        // checks/frame, dropping it to ~0.3 fps ("freezes as soon as you enter").
        // World geometry still blocks enemies; them clipping a door/func_wall is
        // non-essential. The PLAYER keeps full mover collision (its physics runs
        // outside this loop). Restore per-prop mover collision via a spatial
        // broadphase if it's ever needed.
        let _ = movers;
        let pm: &[phys::Mover] = &[];
        match model_def(ty).ai {
            // Melee aliens (zombie/houndeye/bullsquid/ichy) reuse the headcrab
            // approach+bite AI; ranged/boss/flyer types render but don't move yet.
            AI_MELEE => tick_headcrab(m, pm, pi, player_pos, health, armor, nprops),
            AI_RANGED => tick_shooter(m, pm, pi, player_pos, health, armor, nprops, true),
            AI_TURRET => tick_shooter(m, pm, pi, player_pos, health, armor, nprops, false),
            AI_ALLY => tick_barney(m, pm, pi, player_pos, health, armor, nprops),
            AI_FLEE => tick_scientist(m, pm, pi, player_pos, nprops),
            _ => {}
        }
        pi += 1;
    }
}

#[inline]
fn player_touches_pickup(player_pos: [i32; 3], item_pos: [i32; 3]) -> bool {
    let dy = item_pos[1] - player_pos[1];
    if dy < -8 || dy > ITEM_TOUCH_HEIGHT {
        return false;
    }
    dist2_xz(player_pos, item_pos) <= ITEM_TOUCH_RANGE2
}

unsafe fn collect_pickups(
    player_pos: [i32; 3],
    suit_equipped: &mut bool,
    armor: &mut u16,
    health: &mut u16,
    weapon: &mut Arsenal,
    pickup_kind: &mut u8,
    pickup_ticks: &mut u8,
) {
    let nprops = PROP_COUNT.min(MAX_PROPS);
    let mut pi = 0usize;
    while pi < nprops {
        if PROP_ACTIVE[pi] == 0 || !player_touches_pickup(player_pos, PROP_POS[pi]) {
            pi += 1;
            continue;
        }
        match PROP_KIND[pi] {
            PROP_TYPE_ITEM_SUIT => {
                if !*suit_equipped {
                    *suit_equipped = true;
                    PROP_ACTIVE[pi] = 0;
                    let li = PROP_LOGIC_LINK[pi];
                    if li != u16::MAX && (li as usize) < MAX_LOGIC {
                        let l = li as usize;
                        LOGIC_STATE[l] = LOGIC_STATE_REMOVED;
                        LOGIC_TARGET[l] = 0;
                        LOGIC_PROP_LINK[l] = LOGIC_PROP_NONE;
                    }
                    PROP_LOGIC_LINK[pi] = u16::MAX;
                    *pickup_kind = hud::PICKUP_SUIT;
                    *pickup_ticks = HEV_PICKUP_TICKS;
                    sfx::play(sfx::SUIT);
                    telemetry::debug_log("hl-psx: HEV suit equipped");
                }
            }
            ty @ PROP_TYPE_WEAPON_FIRST..=PROP_TYPE_WEAPON_LAST => {
                let wid = (ty - PROP_TYPE_WEAPON_FIRST) as usize;
                // HL keeps the pickup if the weapon (or its default ammo) is
                // maxed; simplified: weapons always collect on first touch.
                if !weapon.owns(wid) {
                    weapon.give_weapon(wid);
                    PROP_ACTIVE[pi] = 0;
                    sfx::play(sfx::PICKUP);
                } else {
                    // Duplicate weapon = its magazine's worth of ammo.
                    let d = &WEAPON_DEFS[wid];
                    if d.ammo != AMMO_NONE {
                        weapon.give_ammo(d.ammo, d.clip.max(1));
                        PROP_ACTIVE[pi] = 0;
                        sfx::play(sfx::PICKUP);
                    }
                }
            }
            ty @ PROP_TYPE_AMMO_FIRST..=PROP_TYPE_AMMO_LAST => {
                let (pool, rounds) = AMMO_PICKUPS[(ty - PROP_TYPE_AMMO_FIRST) as usize];
                if weapon.ammo[pool] < max_reserve_for(pool) {
                    weapon.give_ammo(pool, rounds);
                    PROP_ACTIVE[pi] = 0;
                    sfx::play(sfx::PICKUP);
                }
            }
            PROP_TYPE_MEDKIT => {
                if *health < PLAYER_START_HEALTH {
                    *health = (*health + MEDKIT_HEAL).min(PLAYER_START_HEALTH);
                    PROP_ACTIVE[pi] = 0;
                    sfx::play(sfx::MEDSHOT);
                }
            }
            PROP_TYPE_ITEM_BATTERY => {
                if *suit_equipped && *armor < HEV_MAX_ARMOR {
                    *armor = armor.saturating_add(HEV_BATTERY_ARMOR).min(HEV_MAX_ARMOR);
                    PROP_ACTIVE[pi] = 0;
                    let li = PROP_LOGIC_LINK[pi];
                    if li != u16::MAX && (li as usize) < MAX_LOGIC {
                        let l = li as usize;
                        LOGIC_STATE[l] = LOGIC_STATE_REMOVED;
                        LOGIC_TARGET[l] = 0;
                        LOGIC_PROP_LINK[l] = LOGIC_PROP_NONE;
                    }
                    PROP_LOGIC_LINK[pi] = u16::MAX;
                    *pickup_kind = hud::PICKUP_BATTERY;
                    *pickup_ticks = HEV_PICKUP_TICKS;
                    sfx::play(sfx::PICKUP);
                    telemetry::debug_log("hl-psx: HEV battery picked up");
                }
            }
            _ => {}
        }
        pi += 1;
    }
}

// ---- Weapon system: data-driven HL1 arsenal ----------------------------------
// Ammo reserve pools, shared by weapons of the same type (glock + mp5 share 9mm).
const AMMO_NONE: usize = 0; // melee (crowbar): no ammo
const AMMO_9MM: usize = 1;
const AMMO_357: usize = 2;
const AMMO_BUCK: usize = 3;
const AMMO_BOLT: usize = 4;
const AMMO_ROCKET: usize = 5;
const AMMO_URANIUM: usize = 6;
const AMMO_HORNET: usize = 7;
const AMMO_GREN: usize = 8;
const AMMO_SNARK: usize = 9;
const AMMO_SATCHEL: usize = 10;
const AMMO_TRIPMINE: usize = 11;
const N_AMMO: usize = 12;

// Fire archetypes.
const FIRE_MELEE: u8 = 0; // short-range trace (crowbar)
const FIRE_SEMI: u8 = 1; // one hitscan per trigger press (glock, .357, gauss)
const FIRE_AUTO: u8 = 2; // hitscan while held (mp5, egon)
const FIRE_SPREAD: u8 = 3; // multi-pellet hitscan per press (shotgun)
const FIRE_PROJ: u8 = 4; // spawns a projectile (rpg, crossbow, grenade, hornet, ...)

// Projectile kinds (FIRE_PROJ weapons). Explosive kinds do area damage.
const PROJ_BOLT: u8 = 0;
const PROJ_ROCKET: u8 = 1;
const PROJ_GRENADE: u8 = 2;
const PROJ_HORNET: u8 = 3;
const PROJ_SNARK: u8 = 4;
const PROJ_PLACED: u8 = 5; // satchel / tripmine (lobbed explosive)

struct WeaponDef {
    #[allow(dead_code)] // documents the table; a HUD weapon label is the next use
    name: &'static str,
    ammo: usize,      // AMMO_*
    clip: u16,        // magazine size (0 = fires straight from the reserve)
    reserve_max: u16, // carry cap for this ammo type
    damage: u8,       // per hit / per pellet
    range: i32,       // hitscan reach
    pellets: u8,      // hitscan traces per press (shotgun > 1)
    spread: i32,      // per-pellet aim jitter (aim-cone pixels)
    cooldown: u8,     // ticks between shots
    reload: u8,       // reload ticks (0 = no magazine reload)
    fire: u8,         // FIRE_*
    proj: u8,         // PROJ_* (FIRE_PROJ only)
    wm: u8,           // viewmodel index: geom chunk 1000+wm, tex 2000+wm
}

// Weapon ids = index into WEAPON_DEFS. Switch order follows the HL1 slots.
const W_CROWBAR: usize = 0;
const W_GLOCK: usize = 1;
const W_357: usize = 2;
const W_MP5: usize = 3;
const W_SHOTGUN: usize = 4;
const W_CROSSBOW: usize = 5;
const W_RPG: usize = 6;
const W_GAUSS: usize = 7;
const W_EGON: usize = 8;
const W_HORNET: usize = 9;
const W_GRENADE: usize = 10;
const W_SNARK: usize = 11;
const W_TRIPMINE: usize = 12;
const W_SATCHEL: usize = 13;
const N_WEAPONS: usize = 14;

const fn wdef(
    name: &'static str,
    ammo: usize,
    clip: u16,
    reserve_max: u16,
    damage: u8,
    range: i32,
    pellets: u8,
    spread: i32,
    cooldown: u8,
    reload: u8,
    fire: u8,
    proj: u8,
    wm: u8,
) -> WeaponDef {
    WeaponDef {
        name,
        ammo,
        clip,
        reserve_max,
        damage,
        range,
        pellets,
        spread,
        cooldown,
        reload,
        fire,
        proj,
        wm,
    }
}

// Faithful-ish HL1 values (cooldown/reload in 20 Hz sim ticks). Exotic behaviours
// (gauss charge, egon beam, snark AI, satchel/tripmine placement) are mapped to
// the nearest archetype; the per-weapon stats and viewmodel are authentic.
static WEAPON_DEFS: [WeaponDef; N_WEAPONS] = [
    wdef("CROWBAR", AMMO_NONE, 0, 0, 10, 96, 1, 0, 7, 0, FIRE_MELEE, 0, 4),
    wdef("GLOCK", AMMO_9MM, 17, 250, 8, GLOCK_RANGE, 1, 0, 6, 30, FIRE_SEMI, 0, 0),
    wdef("357", AMMO_357, 6, 36, 40, GLOCK_RANGE, 1, 0, 15, 40, FIRE_SEMI, 0, 1),
    wdef("MP5", AMMO_9MM, 50, 250, 8, GLOCK_RANGE, 1, 5, 2, 30, FIRE_AUTO, 0, 2),
    wdef("SHOTGUN", AMMO_BUCK, 8, 125, 5, GLOCK_RANGE, 6, 14, 16, 24, FIRE_SPREAD, 0, 13),
    wdef("CROSSBOW", AMMO_BOLT, 5, 50, 50, GLOCK_RANGE, 1, 0, 15, 30, FIRE_PROJ, PROJ_BOLT, 3),
    wdef("RPG", AMMO_ROCKET, 1, 5, 100, 0, 1, 0, 30, 30, FIRE_PROJ, PROJ_ROCKET, 10),
    wdef("GAUSS", AMMO_URANIUM, 0, 100, 20, GLOCK_RANGE, 1, 0, 5, 0, FIRE_SEMI, 0, 7),
    wdef("EGON", AMMO_URANIUM, 0, 100, 6, GLOCK_RANGE, 1, 0, 1, 0, FIRE_AUTO, 0, 6),
    wdef("HORNET", AMMO_HORNET, 0, 8, 8, 0, 1, 0, 5, 0, FIRE_PROJ, PROJ_HORNET, 9),
    wdef("GRENADE", AMMO_GREN, 0, 10, 100, 0, 1, 0, 20, 0, FIRE_PROJ, PROJ_GRENADE, 8),
    wdef("SNARK", AMMO_SNARK, 0, 15, 10, 0, 1, 0, 10, 0, FIRE_PROJ, PROJ_SNARK, 14),
    wdef("TRIPMINE", AMMO_TRIPMINE, 0, 5, 100, 0, 1, 0, 20, 0, FIRE_PROJ, PROJ_PLACED, 15),
    wdef("SATCHEL", AMMO_SATCHEL, 0, 5, 100, 0, 1, 0, 20, 0, FIRE_PROJ, PROJ_PLACED, 11),
];

#[inline]
fn wdef_of(id: usize) -> &'static WeaponDef {
    &WEAPON_DEFS[id.min(N_WEAPONS - 1)]
}

/// The player's whole arsenal: owned set, per-weapon magazines, per-type reserve,
/// and the live firing/reload/switch timers for the selected weapon.
struct Arsenal {
    owned: u16, // bit i set => weapon i owned
    current: usize,
    clip: [u16; N_WEAPONS],
    ammo: [u16; N_AMMO],
    cooldown: u8,
    reload_ticks: u8,
    dry_ticks: u8,
    switch_ticks: u8, // brief lockout after a weapon change
}

impl Arsenal {
    fn new() -> Self {
        let mut a = Arsenal {
            owned: 0,
            current: W_GLOCK,
            clip: [0; N_WEAPONS],
            ammo: [0; N_AMMO],
            cooldown: 0,
            reload_ticks: 0,
            dry_ticks: 0,
            switch_ticks: 0,
        };
        // HL1 starts with the crowbar + glock; full clips.
        a.give_weapon(W_CROWBAR);
        a.give_weapon(W_GLOCK);
        a.clip[W_GLOCK] = WEAPON_DEFS[W_GLOCK].clip;
        a.ammo[AMMO_9MM] = GLOCK_START_RESERVE;
        a.current = W_GLOCK;
        a
    }

    #[inline]
    fn def(&self) -> &'static WeaponDef {
        wdef_of(self.current)
    }

    fn owns(&self, id: usize) -> bool {
        id < N_WEAPONS && (self.owned & (1 << id)) != 0
    }

    fn give_weapon(&mut self, id: usize) {
        if id < N_WEAPONS {
            let fresh = !self.owns(id);
            self.owned |= 1 << id;
            // First pickup of a magazine weapon arrives loaded.
            if fresh && WEAPON_DEFS[id].clip > 0 && self.clip[id] == 0 {
                self.clip[id] = WEAPON_DEFS[id].clip;
            }
        }
    }

    fn give_ammo(&mut self, ammo: usize, n: u16) {
        if ammo < N_AMMO && ammo != AMMO_NONE {
            let cap = max_reserve_for(ammo);
            self.ammo[ammo] = self.ammo[ammo].saturating_add(n).min(cap);
        }
    }

    /// Live magazine count of the selected weapon (0 for no-magazine weapons).
    fn clip_display(&self) -> u16 {
        self.clip[self.current]
    }

    /// Reserve count of the selected weapon's ammo type.
    fn reserve_display(&self) -> u16 {
        self.ammo[self.def().ammo]
    }

    /// HUD ammo display mode: 0 = melee (none), 1 = reserve only, 2 = reserve|clip.
    fn ammo_mode(&self) -> u8 {
        let d = self.def();
        if d.ammo == AMMO_NONE {
            0
        } else if d.clip == 0 {
            1
        } else {
            2
        }
    }

    fn tick(&mut self) {
        if self.cooldown > 0 {
            self.cooldown -= 1;
        }
        if self.dry_ticks > 0 {
            self.dry_ticks -= 1;
        }
        if self.switch_ticks > 0 {
            self.switch_ticks -= 1;
        }
        if self.reload_ticks > 0 {
            self.reload_ticks -= 1;
            if self.reload_ticks == 0 {
                self.finish_reload();
            }
        }
    }

    fn start_reload(&mut self) -> bool {
        let d = self.def();
        if d.reload == 0 || d.clip == 0 || self.reload_ticks != 0 {
            return false;
        }
        if self.clip[self.current] >= d.clip || self.ammo[d.ammo] == 0 {
            return false;
        }
        self.reload_ticks = d.reload;
        self.cooldown = self.cooldown.max(d.reload);
        true
    }

    fn finish_reload(&mut self) {
        let d = self.def();
        let need = d.clip.saturating_sub(self.clip[self.current]);
        let take = need.min(self.ammo[d.ammo]);
        self.clip[self.current] = self.clip[self.current].saturating_add(take);
        self.ammo[d.ammo] = self.ammo[d.ammo].saturating_sub(take);
    }

    /// Consume ammo for one shot. Returns true if the shot goes off (the caller
    /// then runs the archetype). Handles dry-click + auto-reload.
    fn try_fire(&mut self) -> bool {
        let d = self.def();
        if self.cooldown != 0 || self.reload_ticks != 0 || self.switch_ticks != 0 {
            return false;
        }
        if d.ammo == AMMO_NONE {
            self.cooldown = d.cooldown; // melee: never out of ammo
            return true;
        }
        if d.clip > 0 {
            if self.clip[self.current] == 0 {
                self.cooldown = GLOCK_EMPTY_COOLDOWN_TICKS;
                self.dry_ticks = GLOCK_EMPTY_COOLDOWN_TICKS;
                let _ = self.start_reload();
                return false;
            }
            self.clip[self.current] -= 1;
        } else {
            if self.ammo[d.ammo] == 0 {
                self.cooldown = GLOCK_EMPTY_COOLDOWN_TICKS;
                self.dry_ticks = GLOCK_EMPTY_COOLDOWN_TICKS;
                return false;
            }
            self.ammo[d.ammo] -= 1;
        }
        self.cooldown = d.cooldown;
        true
    }

    /// Cycle to the next/prev owned weapon. Returns true if the selection changed
    /// (the caller then streams the new viewmodel).
    fn cycle(&mut self, forward: bool) -> bool {
        if self.owned == 0 {
            return false;
        }
        let mut i = self.current;
        for _ in 0..N_WEAPONS {
            i = if forward {
                (i + 1) % N_WEAPONS
            } else {
                (i + N_WEAPONS - 1) % N_WEAPONS
            };
            if self.owns(i) {
                if i != self.current {
                    self.select(i);
                    return true;
                }
                break;
            }
        }
        false
    }

    fn select(&mut self, id: usize) {
        self.current = id;
        self.reload_ticks = 0;
        self.cooldown = 0;
        self.dry_ticks = 0;
        self.switch_ticks = 8; // ~0.4 s raise lockout
    }

    /// Grant the full arsenal + ammo. NB this is a demake simplification: HL1
    /// starts with crowbar+glock and you pick the rest up. Faithful weapon_* /
    /// ammo_* pickups (with w_* world models) are the next step; for now the whole
    /// system is given at spawn so every weapon is reachable.
    fn give_full_arsenal(&mut self) {
        self.owned = (1u16 << N_WEAPONS) - 1;
        let mut i = 0;
        while i < N_WEAPONS {
            if WEAPON_DEFS[i].clip > 0 {
                self.clip[i] = WEAPON_DEFS[i].clip;
            }
            i += 1;
        }
        let mut a = 0;
        while a < N_AMMO {
            self.ammo[a] = max_reserve_for(a);
            a += 1;
        }
    }
}

#[inline]
fn max_reserve_for(ammo: usize) -> u16 {
    // The reserve cap is the largest reserve_max among weapons using this ammo.
    let mut cap = 0u16;
    let mut i = 0;
    while i < N_WEAPONS {
        if WEAPON_DEFS[i].ammo == ammo && WEAPON_DEFS[i].reserve_max > cap {
            cap = WEAPON_DEFS[i].reserve_max;
        }
        i += 1;
    }
    cap
}

#[inline]
fn project_world_point(p: [i32; 3], rot: &Mat3I16, base_t: [i32; 3]) -> Option<(i16, i16, i32)> {
    let vz = dot12(rot.m[2], p) + base_t[2];
    if !(render::NEAR_Z..=FAR_VIEW).contains(&vz) {
        return None;
    }
    let vx = dot12(rot.m[0], p) + base_t[0];
    let vy = dot12(rot.m[1], p) + base_t[1];
    let sx = 160 + (vx * H_PROJ as i32) / vz;
    let sy = 120 + (vy * H_PROJ as i32) / vz;
    if sx < -16 || sx > 336 || sy < -16 || sy > 256 {
        return None;
    }
    Some((sx as i16, sy as i16, vz))
}

unsafe fn clear_combat_fx() {
    clear_projectiles();
    IMPACT_PARTICLES.clear();
    let mut i = 0usize;
    while i < MAX_IMPACT_MARKS {
        IMPACT_MARKS[i] = EMPTY_IMPACT_MARK;
        i += 1;
    }
    IMPACT_MARK_CURSOR = 0;
}

unsafe fn decay_combat_fx() {
    IMPACT_PARTICLES.update(1);
    let mut i = 0usize;
    while i < MAX_IMPACT_MARKS {
        if IMPACT_MARKS[i].ttl > 0 {
            IMPACT_MARKS[i].ttl -= 1;
        }
        i += 1;
    }
}

unsafe fn spawn_impact_mark(pos: [i32; 3], kind: u8) {
    let i = IMPACT_MARK_CURSOR % MAX_IMPACT_MARKS;
    IMPACT_MARKS[i] = ImpactMark {
        pos,
        ttl: IMPACT_MARK_TICKS,
        kind,
    };
    IMPACT_MARK_CURSOR = (IMPACT_MARK_CURSOR + 1) % MAX_IMPACT_MARKS;
}

unsafe fn spawn_impact_particles(screen: (i16, i16), kind: u8) {
    let (color, count, spread, ttl) = if kind == IMPACT_KIND_BLOOD {
        ((130, 14, 8), 5, 28, 12)
    } else {
        ((232, 186, 82), 7, 44, 10)
    };
    IMPACT_PARTICLES.spawn_burst(&mut IMPACT_RNG, screen, color, count, spread, ttl);
}

unsafe fn spawn_impact_fx(pos: [i32; 3], kind: u8, rot: &Mat3I16, base_t: [i32; 3]) {
    spawn_impact_mark(pos, kind);
    if let Some((sx, sy, _)) = project_world_point(pos, rot, base_t) {
        spawn_impact_particles((sx, sy), kind);
    }
}

unsafe fn render_impact_marks<const N: usize>(
    ot: &mut OrderingTable<N>,
    rects: &mut [RectFlat; MAX_IMPACT_MARKS],
    rot: &Mat3I16,
    base_t: [i32; 3],
) -> usize {
    let mut written = 0usize;
    let mut i = 0usize;
    while i < MAX_IMPACT_MARKS {
        let mark = IMPACT_MARKS[i];
        if mark.ttl != 0 {
            if let Some((sx, sy, vz)) = project_world_point(mark.pos, rot, base_t) {
                let size = if vz < 900 { 3 } else { 2 };
                let scale = mark.ttl as u16;
                let (r0, g0, b0) = if mark.kind == IMPACT_KIND_BLOOD {
                    (90u16, 8u16, 5u16)
                } else {
                    (18u16, 17u16, 14u16)
                };
                rects[written] = RectFlat::new(
                    sx - (size as i16 / 2),
                    sy - (size as i16 / 2),
                    size,
                    size,
                    ((r0 * scale) / IMPACT_MARK_TICKS as u16) as u8,
                    ((g0 * scale) / IMPACT_MARK_TICKS as u16) as u8,
                    ((b0 * scale) / IMPACT_MARK_TICKS as u16) as u8,
                );
                ot.add(0, &mut rects[written], RectFlat::WORDS);
                written += 1;
                if written >= MAX_IMPACT_MARKS {
                    break;
                }
            }
        }
        i += 1;
    }
    written
}

const MELEE_AIM_PIX: i32 = 70; // crowbar swing: wide forgiving cone

/// One hitscan trace. `damage`/`range` from the weapon; (`aim_x`,`aim_y`) is the
/// half-cone in screen px; (`cx_px`,`cy_px`) offsets the cone centre (shotgun
/// pellets). Returns the enemy hit, applying damage + blood; else a world decal.
#[allow(clippy::too_many_arguments)]
unsafe fn fire_hitscan(
    m: &Map,
    movers: &[phys::Mover],
    eye: [i32; 3],
    rot: &Mat3I16,
    base_t: [i32; 3],
    damage: u8,
    range: i32,
    aim_x: i32,
    aim_y: i32,
    cx_px: i32,
    cy_px: i32,
) -> Option<usize> {
    let end = [
        eye[0] + (((rot.m[2][0] as i32) * range) >> 12),
        eye[1] + (((rot.m[2][1] as i32) * range) >> 12),
        eye[2] + (((rot.m[2][2] as i32) * range) >> 12),
    ];
    let world_hit = phys::trace_line(m, movers, eye, end);
    let world_limit_z = world_hit
        .map(|hit| (range * hit.frac) >> 12)
        .unwrap_or(range + 1);
    let mut best = usize::MAX;
    let mut best_z = range + 1;
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
        if !(render::NEAR_Z..=range).contains(&vz) || vz > world_limit_z {
            pi += 1;
            continue;
        }

        let vx = dot12(rot.m[0], target) + base_t[0];
        let vy = dot12(rot.m[1], target) + base_t[1];
        // Cone centred on the pellet's screen offset (cx_px, cy_px).
        let dx = vx * H_PROJ as i32 - cx_px * vz;
        let dy = vy * H_PROJ as i32 - cy_px * vz;
        if dx.abs() > vz * aim_x || dy.abs() > vz * aim_y {
            pi += 1;
            continue;
        }
        if !phys::line_clear_world(m, eye, target)
            || !phys::line_clear_movers(m, movers, eye, target)
        {
            pi += 1;
            continue;
        }

        let score = (dx.abs() * 2 + dy.abs()) / vz.max(1);
        if vz < best_z || (vz == best_z && score < best_score) {
            best = pi;
            best_z = vz;
            best_score = score;
        }
        pi += 1;
    }

    if best == usize::MAX {
        if let Some(hit) = world_hit {
            let decal_pos = [
                hit.pos[0] + ((hit.normal[0] * 2) >> 12),
                hit.pos[1] + ((hit.normal[1] * 2) >> 12),
                hit.pos[2] + ((hit.normal[2] * 2) >> 12),
            ];
            spawn_impact_fx(decal_pos, IMPACT_KIND_WORLD, rot, base_t);
            sfx::play_world(sfx::RIC, decal_pos);
            if hit.mover >= 0 {
                damage_brush_ent(m, m.n_logic, m.n_ents, hit.mover as usize, damage, SIM_NOW);
            }
        }
        None
    } else {
        let ty = PROP_KIND[best];
        damage_prop(best, damage);
        spawn_impact_fx(
            prop_target(ty, PROP_POS[best]),
            IMPACT_KIND_BLOOD,
            rot,
            base_t,
        );
        PROP_AI_TARGET[best] = PROP_TARGET_PLAYER;
        if ty == PROP_TYPE_SCIENTIST {
            PROP_AI_TIMER[best] = SCIENTIST_FEAR_TICKS;
        }
        Some(best)
    }
}

/// Fixed spread pattern for shotgun pellets (no RNG in the render path); pellet
/// `i` lands `spread` px off-centre in a small fixed rosette.
#[inline]
fn pellet_offset(i: u8, spread: i32) -> (i32, i32) {
    const PAT: [(i32, i32); 6] = [(0, 0), (2, -1), (-2, 1), (1, 2), (-1, -2), (2, 2)];
    let (px, py) = PAT[(i as usize) % PAT.len()];
    (px * spread / 2, py * spread / 2)
}

/// Run a weapon's fire archetype for one shot already paid for by try_fire.
/// Returns true when a hitscan connected with an enemy (drives the crowbar
/// hit-vs-miss sound).
unsafe fn fire_weapon(
    d: &WeaponDef,
    m: &Map,
    movers: &[phys::Mover],
    eye: [i32; 3],
    rot: &Mat3I16,
    base_t: [i32; 3],
) -> bool {
    match d.fire {
        FIRE_MELEE => fire_hitscan(
            m, movers, eye, rot, base_t, d.damage, d.range, MELEE_AIM_PIX, MELEE_AIM_PIX, 0, 0,
        )
        .is_some(),
        FIRE_SPREAD => {
            let n = d.pellets.max(1);
            let mut i = 0u8;
            let mut hit = false;
            while i < n {
                let (cx, cy) = pellet_offset(i, d.spread);
                hit |= fire_hitscan(
                    m,
                    movers,
                    eye,
                    rot,
                    base_t,
                    d.damage,
                    d.range,
                    GLOCK_AIM_PIX_X,
                    GLOCK_AIM_PIX_Y,
                    cx,
                    cy,
                )
                .is_some();
                i += 1;
            }
            hit
        }
        FIRE_PROJ => {
            spawn_projectile(d.proj, d.damage, eye, rot);
            false
        }
        _ => {
            // FIRE_SEMI / FIRE_AUTO: single centred hitscan.
            fire_hitscan(
                m,
                movers,
                eye,
                rot,
                base_t,
                d.damage,
                d.range,
                GLOCK_AIM_PIX_X,
                GLOCK_AIM_PIX_Y,
                0,
                0,
            )
            .is_some()
        }
    }
}

/// The fire sound for a weapon id; melee picks hit vs miss.
fn weapon_fire_sfx(id: usize, hit: bool) -> u8 {
    match id {
        W_CROWBAR => {
            if hit {
                sfx::CBAR_HIT
            } else {
                sfx::CBAR_MISS
            }
        }
        W_GLOCK => sfx::GLOCK,
        W_357 => sfx::PYTHON,
        W_MP5 => sfx::MP5,
        W_SHOTGUN => sfx::SHOTGUN,
        W_CROSSBOW => sfx::XBOW,
        W_RPG => sfx::RPG,
        W_GAUSS | W_EGON => sfx::GAUSS,
        W_HORNET => sfx::ELECTRO,
        _ => sfx::CBAR_MISS, // thrown/placed: a swing whoosh
    }
}

// ---- Projectiles: rockets, bolts, grenades, hornets, lobbed explosives -------
const MAX_PROJECTILES: usize = 12;
const PROJ_GRAVITY: i32 = 12; // world units/tick^2 for arced kinds
const PROJ_HIT_RADIUS: i32 = 56; // projectile-vs-enemy contact radius

#[derive(Clone, Copy)]
struct Projectile {
    active: bool,
    pos: [i32; 3],
    vel: [i32; 3],
    kind: u8,
    damage: u8,
    life: u8,
}

impl Projectile {
    const ZERO: Projectile = Projectile {
        active: false,
        pos: [0; 3],
        vel: [0; 3],
        kind: 0,
        damage: 0,
        life: 0,
    };
}

static mut PROJECTILES: [Projectile; MAX_PROJECTILES] = [Projectile::ZERO; MAX_PROJECTILES];
static mut PROJ_RECTS: [RectFlat; MAX_PROJECTILES] =
    [const { RectFlat::new(0, 0, 0, 0, 0, 0, 0) }; MAX_PROJECTILES];

// (speed, life ticks, gravity?, AoE radius (0 = direct hit only), colour, size px)
fn proj_params(kind: u8) -> (i32, u8, bool, i32, (u8, u8, u8), u16) {
    match kind {
        PROJ_ROCKET => (90, 50, false, 220, (250, 150, 50), 6),
        PROJ_BOLT => (150, 40, false, 0, (210, 210, 170), 3),
        PROJ_GRENADE => (64, 60, true, 200, (120, 150, 90), 5),
        PROJ_HORNET => (85, 50, false, 0, (250, 230, 70), 3),
        PROJ_SNARK => (48, 80, true, 110, (190, 170, 50), 5),
        _ => (40, 100, true, 200, (170, 70, 50), 5), // PROJ_PLACED (satchel / tripmine)
    }
}

unsafe fn clear_projectiles() {
    let mut i = 0;
    while i < MAX_PROJECTILES {
        PROJECTILES[i] = Projectile::ZERO;
        i += 1;
    }
}

unsafe fn spawn_projectile(kind: u8, damage: u8, eye: [i32; 3], rot: &Mat3I16) {
    let (speed, life, gravity, ..) = proj_params(kind);
    let fwd = [rot.m[2][0] as i32, rot.m[2][1] as i32, rot.m[2][2] as i32];
    let mut slot = usize::MAX;
    let mut i = 0;
    while i < MAX_PROJECTILES {
        if !PROJECTILES[i].active {
            slot = i;
            break;
        }
        i += 1;
    }
    if slot == usize::MAX {
        slot = 0; // pool full: recycle slot 0
    }
    let mut vel = [
        (fwd[0] * speed) >> 12,
        (fwd[1] * speed) >> 12,
        (fwd[2] * speed) >> 12,
    ];
    if gravity {
        vel[1] += speed / 3; // toss it up a little for an arc
    }
    PROJECTILES[slot] = Projectile {
        active: true,
        pos: [
            eye[0] + ((fwd[0] * 24) >> 12),
            eye[1] + ((fwd[1] * 24) >> 12),
            eye[2] + ((fwd[2] * 24) >> 12),
        ],
        vel,
        kind,
        damage,
        life,
    };
}

unsafe fn explode(m: &Map, pos: [i32; 3], damage: u8, radius: i32) {
    if radius <= 0 {
        return;
    }
    sfx::play_world(sfx::EXPLODE, pos);
    // Blast breakables in range (crates, boards, grates).
    let nents = m.n_ents;
    let mut ei = 0usize;
    while ei < nents {
        if ENT_ACTIVE[ei] != 0 && ENT_BREAK_LOGIC[ei] != u16::MAX {
            let c = ENT_CACHE[ei].center;
            let (dx, dy, dz) = (c[0] - pos[0], c[1] - pos[1], c[2] - pos[2]);
            if dx.abs() < radius && dy.abs() < radius && dz.abs() < radius {
                damage_brush_ent(m, m.n_logic, nents, ei, damage, SIM_NOW);
            }
        }
        ei += 1;
    }
    let r2 = radius * radius;
    let mut pi = 0;
    let nprops = PROP_COUNT.min(MAX_PROPS);
    while pi < nprops {
        if prop_start_health(PROP_KIND[pi]) != 0 && PROP_HEALTH[pi] != 0 {
            let t = prop_target(PROP_KIND[pi], PROP_POS[pi]);
            let d2 = dist2_3(t, pos);
            if d2 < r2 {
                let dmg = (damage as i32 * (radius - isqrt(d2)) / radius).clamp(0, 255) as u8;
                damage_prop(pi, dmg);
                PROP_AI_TARGET[pi] = PROP_TARGET_PLAYER;
            }
        }
        pi += 1;
    }
}

unsafe fn tick_projectiles(m: &Map, movers: &[phys::Mover]) {
    let mut i = 0;
    while i < MAX_PROJECTILES {
        if !PROJECTILES[i].active {
            i += 1;
            continue;
        }
        let (_, _, gravity, aoe, _, _) = proj_params(PROJECTILES[i].kind);
        let old = PROJECTILES[i].pos;
        if gravity {
            PROJECTILES[i].vel[1] -= PROJ_GRAVITY;
        }
        let v = PROJECTILES[i].vel;
        let new = [old[0] + v[0], old[1] + v[1], old[2] + v[2]];
        let mut hit = false;
        let mut hit_pos = new;
        if let Some(h) = phys::trace_line(m, movers, old, new) {
            hit = true;
            hit_pos = h.pos;
        }
        // Enemy contact: nearest living prop within PROJ_HIT_RADIUS of the new pos.
        let mut best = usize::MAX;
        let mut best_d2 = PROJ_HIT_RADIUS * PROJ_HIT_RADIUS;
        let mut pi = 0;
        let nprops = PROP_COUNT.min(MAX_PROPS);
        while pi < nprops {
            if prop_start_health(PROP_KIND[pi]) != 0 && PROP_HEALTH[pi] != 0 {
                let d2 = dist2_3(prop_target(PROP_KIND[pi], PROP_POS[pi]), new);
                if d2 < best_d2 {
                    best_d2 = d2;
                    best = pi;
                }
            }
            pi += 1;
        }
        if best != usize::MAX {
            hit = true;
            hit_pos = prop_target(PROP_KIND[best], PROP_POS[best]);
        }
        PROJECTILES[i].pos = new;
        if PROJECTILES[i].life > 0 {
            PROJECTILES[i].life -= 1;
        }
        if hit || PROJECTILES[i].life == 0 {
            if aoe > 0 {
                explode(m, hit_pos, PROJECTILES[i].damage, aoe);
            } else if best != usize::MAX {
                damage_prop(best, PROJECTILES[i].damage);
                PROP_AI_TARGET[best] = PROP_TARGET_PLAYER;
            }
            PROJECTILES[i].active = false;
        }
        i += 1;
    }
}

unsafe fn render_projectiles<const N: usize>(
    ot: &mut OrderingTable<N>,
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    let mut i = 0;
    while i < MAX_PROJECTILES {
        if PROJECTILES[i].active {
            let (_, _, _, _, (r, g, b), size) = proj_params(PROJECTILES[i].kind);
            if let Some((sx, sy, _)) = project_world_point(PROJECTILES[i].pos, rot, base_t) {
                let half = (size / 2) as i16;
                PROJ_RECTS[i] = RectFlat::new(sx - half, sy - half, size, size, r, g, b);
                ot.add(0, &mut PROJ_RECTS[i], RectFlat::WORDS);
            }
        }
        i += 1;
    }
}

#[inline]
fn face_bounds_visible(center: [i32; 3], radius: i32, rot: &Mat3I16, base_t: [i32; 3]) -> bool {
    // Conservative sphere around the cooked face AABB. It is looser than the
    // full AABB test but much cheaper across large PVS face lists.
    sphere_visible(center, radius, rot, base_t)
}

#[inline]
fn cached_face_visible(rec: PvsFaceRec, rot: &Mat3I16, base_t: [i32; 3]) -> bool {
    sphere_visible(
        [
            rec.center[0] as i32,
            rec.center[1] as i32,
            rec.center[2] as i32,
        ],
        rec.radius as i32,
        rot,
        base_t,
    )
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
    // Farthest-depth policy for depth-spanning world surfaces (lets long sloped
    // BSP triangles draw behind nearer geometry in a painter's-algorithm OT).
    // OT_SHIFT keeps the bucket size small so geometry spreads across the whole
    // table -- sz tops out ~sz<FAR_VIEW, so `>> OT_SHIFT` must stay < OT_LEN.
    clamp_otz((farthest3_u16(a.sz, b.sz, c.sz) >> OT_SHIFT) as usize)
}

#[inline]
fn world_otz_from_gte4(a: &Projected, b: &Projected, c: &Projected, d: &Projected) -> usize {
    clamp_otz((farthest4_u16(a.sz, b.sz, c.sz, d.sz) >> OT_SHIFT) as usize)
}

#[inline]
fn world_otz_from_view3(a: i32, b: i32, c: i32) -> usize {
    clamp_otz((farthest3_i32(a, b, c) >> OT_SHIFT) as usize)
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

#[inline]
fn valid_pvs_leaf(m: &Map, leaf: i32) -> bool {
    leaf > 0 && (leaf as usize) < m.n_leaves
}

fn recover_camera_leaf(m: &Map, eye: [i32; 3], player_pos: [i32; 3], train_hint: [i32; 3]) -> i32 {
    let leaf = camera_leaf(m, eye);
    if valid_pvs_leaf(m, leaf) {
        return leaf;
    }

    const CANDIDATE_OFFSETS: [[i32; 3]; 10] = [
        [0, -VIEW_HEIGHT + 8, 0],
        [0, -32, 0],
        [0, 32, 0],
        [48, 0, 0],
        [-48, 0, 0],
        [0, 0, 48],
        [0, 0, -48],
        [96, -16, 0],
        [-96, -16, 0],
        [0, -16, 96],
    ];
    let mut i = 0usize;
    while i < CANDIDATE_OFFSETS.len() {
        let o = CANDIDATE_OFFSETS[i];
        let cand = [eye[0] + o[0], eye[1] + o[1], eye[2] + o[2]];
        let cand_leaf = camera_leaf(m, cand);
        if valid_pvs_leaf(m, cand_leaf) {
            return cand_leaf;
        }
        i += 1;
    }

    let player_leaf = camera_leaf(m, [player_pos[0], player_pos[1] + 8, player_pos[2]]);
    if valid_pvs_leaf(m, player_leaf) {
        return player_leaf;
    }

    let train_leaf = camera_leaf(
        m,
        [train_hint[0], train_hint[1] + VIEW_HEIGHT, train_hint[2]],
    );
    if valid_pvs_leaf(m, train_leaf) {
        return train_leaf;
    }

    leaf
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
        PVS_FACE_MARK_TOKEN = 1;
    } else {
        PVS_FACE_MARK_TOKEN = next;
    }
    PVS_FACE_MARK_TOKEN
}

unsafe fn rebuild_pvs_cache(m: &Map, cam_leaf: i32, nents: usize) {
    let (visofs, _, _) = m.leaf(cam_leaf as usize);
    decompress_vis(m, visofs, &mut VIS_BITS);
    let mut old_group = 0usize;
    while old_group < PVS_GROUP_COUNT {
        PVS_GROUP_FACE[PVS_GROUP_ACTIVE[old_group] as usize] = PVS_LINK_END;
        old_group += 1;
    }
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
            if cnt == 0
                || first > u16::MAX as usize
                || cnt > u16::MAX as usize
                || PVS_FACE_COUNT >= MAX_FACES
            {
                continue;
            }

            let group = m.face_group(face);
            if group >= MAX_FACE_GROUPS {
                continue;
            }
            if PVS_GROUP_FACE[group] == PVS_LINK_END {
                if PVS_GROUP_COUNT >= MAX_FACE_GROUPS {
                    break;
                }
                PVS_GROUP_FIRST[group] = PVS_LINK_END;
                PVS_GROUP_FACE[group] = face as u16;
                PVS_GROUP_ACTIVE[PVS_GROUP_COUNT] = group as u16;
                PVS_GROUP_COUNT += 1;
            }

            let entry = PVS_FACE_COUNT;
            PVS_FACE_INDEX[entry] = face as u16;
            if entry < MAX_PVS_FACE_RECS {
                let (bc, radius) = m.face_bounds(face);
                let radius = radius as u16;
                PVS_FACE_REC[entry] = PvsFaceRec {
                    first: first as u16,
                    count: cnt as u16,
                    center: [bc[0] as i16, bc[1] as i16, bc[2] as i16],
                    radius,
                    tex: m.face_tex(face) as u8,
                    is_loop: m.face_is_loop(face),
                };
            }
            PVS_TRI_REF_COUNT += cnt;
            PVS_FACE_NEXT[entry] = PVS_GROUP_FIRST[group];
            PVS_GROUP_FIRST[group] = entry as u16;
            PVS_FACE_COUNT += 1;
        }
    }
    let mut ei = 0usize;
    while ei < nents {
        let e = ENT_CACHE[ei];
        // kind 4 = invisible ladder volume: physics only, never drawn.
        if e.kind != 4
            && ENT_ACTIVE[ei] != 0
            && entity_touches_pvs(m, &e)
            && PVS_ENT_COUNT < MAX_ENTS
        {
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
    m.tri_uv_words(tri_index)
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
    // The 0x3C quad splits on the a-c diagonal into tri(b,a,c) + tri(a,d,c).
    // That tiles a clean quad ONLY when b and d sit on opposite sides of a-c
    // (the two halves wind the same way). At grazing angles a fan-pair can
    // project so b and d land on the SAME side -- a bowtie whose halves overlap
    // and rasterize to garbage/black. View-dependent, so it pops in and out as
    // the camera moves. When that happens, defer to the single-tri path (each
    // half is convex on its own and draws fine).
    let area = |p: &Projected, q: &Projected, r: &Projected| -> i64 {
        (q.sx as i64 - p.sx as i64) * (r.sy as i64 - p.sy as i64)
            - (q.sy as i64 - p.sy as i64) * (r.sx as i64 - p.sx as i64)
    };
    let w_bac = area(&pb, &pa, &pc);
    let w_adc = area(&pa, &pd, &pc);
    if w_bac == 0 || w_adc == 0 || (w_bac > 0) != (w_adc > 0) {
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
    let qrgb = if slot.backdrop {
        [t0.rgb[2], t0.rgb[0], t0.rgb[1], t1.rgb[1]]
    } else {
        [
            fog1(t0.rgb[2], pb.sz as i32),
            fog1(t0.rgb[0], pa.sz as i32),
            fog1(t0.rgb[1], pc.sz as i32),
            fog1(t1.rgb[1], pd.sz as i32),
        ]
    };
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
        qrgb,
        slot.packet,
    );
    let Some(packet) = packets.push(prim) else {
        return false;
    };
    let mut otz = world_otz_from_gte4(&pa, &pb, &pc, &pd);
    if slot.backdrop {
        otz = clamp_otz(otz + BACKDROP_OTZ_BIAS);
    }
    OT.add(otz, packet, QuadTexturedGouraud::WORDS);
    *nq += 1;
    true
}

#[derive(Clone, Copy)]
struct WorldCounters {
    cells_considered: u32,
    cells_drawn: u32,
    cells_culled: u32,
    surfaces_considered: u32,
    emit_calls: u32,
}

impl WorldCounters {
    const fn new() -> WorldCounters {
        WorldCounters {
            cells_considered: 0,
            cells_drawn: 0,
            cells_culled: 0,
            surfaces_considered: 0,
            emit_calls: 0,
        }
    }
}

/// Decode-after-cull fast path shared by the world and submodel triangle loops.
///
/// The caller has already projected the three verts (`pa`/`pb`/`pc`). Read only
/// the indices to get here; the heavier per-triangle data (tex, uv, per-vertex
/// rgb -- ~9 unaligned u16 reads + 3 rgb555 unpacks) is decoded only for
/// survivors, so the triangles that back-face/off-screen cull pay almost
/// nothing. Returns true when the triangle is fully handled (emitted or culled);
/// false means it straddles the near plane and the caller must run the full
/// decode + view-space clip path.
/// Distance-fog factor for a view depth, 256 = unfogged, 0 = full (black) at
/// FAR_VIEW. Compile-time reciprocal, so no runtime divide.
#[inline]
fn fog_factor(sz: i32) -> i32 {
    if sz <= FOG_START {
        256
    } else if sz >= FAR_VIEW {
        0
    } else {
        ((FAR_VIEW - sz) * FOG_INV) >> 12
    }
}

/// Fade one vertex color toward black by its view depth.
#[inline]
fn fog1(rgb: (u8, u8, u8), sz: i32) -> (u8, u8, u8) {
    let f = fog_factor(sz);
    if f >= 256 {
        rgb
    } else {
        (
            ((rgb.0 as i32 * f) >> 8) as u8,
            ((rgb.1 as i32 * f) >> 8) as u8,
            ((rgb.2 as i32 * f) >> 8) as u8,
        )
    }
}

/// Fade a triangle's three vertex colors toward black by per-vertex depth.
/// Backdrop/sky tris pass through so the horizon never darkens.
#[inline]
fn fog_world_rgb(rgb: [(u8, u8, u8); 3], sz: [i32; 3], backdrop: bool) -> [(u8, u8, u8); 3] {
    if backdrop {
        return rgb;
    }
    [
        fog1(rgb[0], sz[0]),
        fog1(rgb[1], sz[1]),
        fog1(rgb[2], sz[2]),
    ]
}

unsafe fn emit_proj_fast(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tt: usize,
    pa: Projected,
    pb: Projected,
    pc: Projected,
    np: &mut usize,
) -> bool {
    let clamped = |q: &Projected| q.sx <= -1023 || q.sx >= 1023 || q.sy <= -1023 || q.sy >= 1023;
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
        let tex = m.tri_tex(tt);
        if CULL && culled(sa, sb, sc) {
            return true;
        }
        if tex >= m.n_texs || tex >= MAX_TEX_SLOTS {
            return true;
        }
        let slot = TEX_SLOTS[tex];
        if !slot.valid {
            return true;
        }
        let mut otz = world_otz_from_gte3(&pa, &pb, &pc);
        if slot.backdrop {
            otz = clamp_otz(otz + BACKDROP_OTZ_BIAS);
        }
        let rgb = fog_world_rgb(
            m.tri_rgb(tt),
            [pa.sz as i32, pb.sz as i32, pc.sz as i32],
            slot.backdrop,
        );
        push_tri_uv_words(
            packets,
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            m.tri_uv_words(tt),
            rgb,
            slot.packet,
            otz,
        );
        return true;
    }
    // All-behind-camera counts as handled (nothing to draw); otherwise straddler.
    pa.sz == 0 && pb.sz == 0 && pc.sz == 0
}

/// Like `emit_proj_fast` but reads tex/uv/rgb from a loop-built `RenderTri`
/// (loop faces have no tri-array index to look them up by).
unsafe fn emit_proj_fast_tri(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tri: &map::RenderTri,
    pa: Projected,
    pb: Projected,
    pc: Projected,
    np: &mut usize,
) -> bool {
    let clamped = |q: &Projected| q.sx <= -1023 || q.sx >= 1023 || q.sy <= -1023 || q.sy >= 1023;
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
            return true;
        }
        if tri.tex >= m.n_texs || tri.tex >= MAX_TEX_SLOTS {
            return true;
        }
        let slot = TEX_SLOTS[tri.tex];
        if !slot.valid {
            return true;
        }
        let mut otz = world_otz_from_gte3(&pa, &pb, &pc);
        if slot.backdrop {
            otz = clamp_otz(otz + BACKDROP_OTZ_BIAS);
        }
        let rgb = fog_world_rgb(
            tri.rgb,
            [pa.sz as i32, pb.sz as i32, pc.sz as i32],
            slot.backdrop,
        );
        push_tri_uv_words(
            packets,
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            tri.uv_words,
            rgb,
            slot.packet,
            otz,
        );
        return true;
    }
    pa.sz == 0 && pb.sz == 0 && pc.sz == 0
}

/// Emit world triangle `tt` from the per-frame projected-vertex cache.
unsafe fn emit_world_tri(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tt: usize,
    nv: usize,
    frame: u16,
    np: &mut usize,
    counts: &mut WorldCounters,
) {
    if tt >= m.n_tris {
        return;
    }
    let idx = m.tri_idx(tt);
    let (a, b, c) = (idx[0] as usize, idx[1] as usize, idx[2] as usize);
    if a >= nv || b >= nv || c >= nv {
        return;
    }
    proj_vert(m, a, frame);
    proj_vert(m, b, frame);
    proj_vert(m, c, frame);
    counts.emit_calls += 1;
    let (pa, pb, pc) = (SCRATCH[a], SCRATCH[b], SCRATCH[c]);
    xhair_consider(m, tt, pa, pb, pc); // pick covers fast + soft-clip paths
    if emit_proj_fast(packets, m, tt, pa, pb, pc, np) {
        return;
    }
    // Straddler/offscreen: full decode + view-space near-clip path.
    let tri = m.render_tri(tt, cached_world_uv_words(m, tt));
    emit_projected(packets, m, tri, [pa, pb, pc], nv, np);
}

/// Emit submodel triangle `tt` (brush entity / tram), decode-after-cull, using
/// the submodel-token vertex cache (the GTE translation is the entity offset).
unsafe fn emit_submodel_tri(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tt: usize,
    nv: usize,
    token: u16,
    np: &mut usize,
) {
    if tt >= m.n_tris {
        return;
    }
    let idx = m.tri_idx(tt);
    let (a, b, c) = (idx[0] as usize, idx[1] as usize, idx[2] as usize);
    if a >= nv || b >= nv || c >= nv {
        return;
    }
    proj_submodel_vert(m, a, token);
    proj_submodel_vert(m, b, token);
    proj_submodel_vert(m, c, token);
    let (pa, pb, pc) = (SCRATCH[a], SCRATCH[b], SCRATCH[c]);
    xhair_consider(m, tt, pa, pb, pc); // pick covers fast + soft-clip paths
    if emit_proj_fast(packets, m, tt, pa, pb, pc, np) {
        return;
    }
    let tri = m.render_tri(tt, cached_world_uv_words(m, tt));
    emit_projected(packets, m, tri, [pa, pb, pc], nv, np);
}

/// Submodel version of emit_world_loop_tri: uses the entity-token vertex cache.
unsafe fn emit_submodel_loop_tri(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tri: &map::RenderTri,
    nv: usize,
    token: u16,
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
    proj_submodel_vert(m, a, token);
    proj_submodel_vert(m, b, token);
    proj_submodel_vert(m, c, token);
    let (pa, pb, pc) = (SCRATCH[a], SCRATCH[b], SCRATCH[c]);
    if emit_proj_fast_tri(packets, m, tri, pa, pb, pc, np) {
        return;
    }
    emit_projected(packets, m, *tri, [pa, pb, pc], nv, np);
}

/// Draw one brush-entity/tram face: fan its loop, or iterate its raw tris.
unsafe fn emit_submodel_face(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    f: usize,
    first: usize,
    cnt: usize,
    nv: usize,
    token: u16,
    np: &mut usize,
) {
    if m.face_is_loop(f) {
        let tex = m.face_tex(f);
        let mut k = 1;
        while k + 1 < cnt {
            let tri = m.loop_render_tri(tex, first, first + k + 1, first + k);
            emit_submodel_loop_tri(packets, m, &tri, nv, token, np);
            k += 1;
        }
    } else {
        for tt in first..first + cnt {
            emit_submodel_tri(packets, m, tt, nv, token, np);
        }
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
    // The cook fans every face as (anchor, V[k+1], V[k]), so any two consecutive
    // triangles share the anchor edge and form a planar convex quad. Pair them
    // into one PS1 quad (POLY_GT4) -- halving prim count, decode, and OT inserts
    // on the dominant world path -- and fall back to a lean single triangle when
    // a pair straddles the near plane / off-screen edge.
    let end = first + cnt;
    let mut tt = first;
    while tt < end {
        if WORLD_QUAD_PAIRING && tt + 1 < end && tt + 1 < m.n_tris {
            let t0 = m.render_tri(tt, cached_world_uv_words(m, tt));
            let t1 = m.render_tri(tt + 1, cached_world_uv_words(m, tt + 1));
            if try_emit_tri_pair_quad_values(packets, m, t0, t1, nv, frame, nq) {
                counts.emit_calls += 2;
                tt += 2;
                continue;
            }
        }
        emit_world_tri(packets, m, tt, nv, frame, np, counts);
        tt += 1;
    }
}

unsafe fn emit_world_loop_tri(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tri: &map::RenderTri,
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
    if a >= nv || b >= nv || c >= nv {
        return;
    }
    proj_vert(m, a, frame);
    proj_vert(m, b, frame);
    proj_vert(m, c, frame);
    counts.emit_calls += 1;
    let (pa, pb, pc) = (SCRATCH[a], SCRATCH[b], SCRATCH[c]);
    if emit_proj_fast_tri(packets, m, tri, pa, pb, pc, np) {
        return;
    }
    emit_projected(packets, m, *tri, [pa, pb, pc], nv, np);
}

/// Fan a loop face into triangles (tri k = anchor, loop[k+1], loop[k] -- the
/// cook's exact fan), pairing consecutive fan tris into a POLY_GT4 where they
/// form a convex on-screen quad. A 4-vertex face is one natural quad; pairs are
/// exact consecutive fan tris, so no false pairing (avoids the grazing bowtie).
unsafe fn emit_world_face_loop(
    packets: &mut PrimitivePacketArena<'_>,
    m: &Map,
    tex: usize,
    base: usize,
    count: usize,
    nv: usize,
    frame: u16,
    np: &mut usize,
    nq: &mut usize,
    counts: &mut WorldCounters,
) {
    let mut k = 1;
    while k + 1 < count {
        if WORLD_QUAD_PAIRING && k + 2 < count {
            let t0 = m.loop_render_tri(tex, base, base + k + 1, base + k);
            let t1 = m.loop_render_tri(tex, base, base + k + 2, base + k + 1);
            if try_emit_tri_pair_quad_values(packets, m, t0, t1, nv, frame, nq) {
                counts.emit_calls += 2;
                k += 2;
                continue;
            }
        }
        let tri = m.loop_render_tri(tex, base, base + k + 1, base + k);
        emit_world_loop_tri(packets, m, &tri, nv, frame, np, counts);
        k += 1;
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
    if WORLD_BOUNDS_CULL && !face_bounds_visible(bc, be, rot, base_t) {
        return;
    }
    if m.face_is_loop(face) {
        emit_world_face_loop(packets, m, m.face_tex(face), first, cnt, nv, frame, np, nq, counts);
    } else {
        emit_world_face_tris(packets, m, first, cnt, nv, frame, np, nq, counts);
    }
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
) -> u32 {
    // OoT/Crash vertex-precision trick. Vertices are baked at `s`x (cook), so
    // keep the rotation matrix FULL precision and inflate the view-space
    // translation by `s` instead of folding the 1/s down-scale into the matrix.
    // The whole model is then `s`x in view space; the perspective divide
    // (screen = H·x/z) cancels the uniform `s`, but the vertices keep their
    // sub-unit resolution through the divide -> no coarse vertex grid. (Folding
    // 1/s into the matrix -- the old path -- cancels ×s against ÷s BEFORE the
    // GTE's >>12, collapsing right back to a 1-unit grid: no gain at any s.)
    // ponytail: ceiling ~16x -- inflated view-Z fills SZ3's u16 (clips past
    // ~16383 world units) and IR1/IR3's i16; fine for s=4 at model range.
    let (s, scale_shift) = model_local_scale_and_shift(md.local_to_world_q12());
    let mr = rot.mul(&Mat3I16::rotate_y((yaw >> 4) as u16));
    scene::load_rotation(&mr);
    let es = [eye[0] - pos[0], eye[1] - pos[1], eye[2] - pos[2]];
    // Translation uses the view rotation only (model yaw spins verts about the
    // origin, not the origin itself); scale it into the same `s`x view space.
    let et = [
        -dot12(rot.m[0], es) * s,
        -dot12(rot.m[1], es) * s,
        -dot12(rot.m[2], es) * s,
    ];
    scene::load_translation(Vec3I32::new(et[0], et[1], et[2]));
    let near_s = (NEAR as i32 * s) as u16;
    let nv = md.n_verts.min(MAX_MODEL_VERTS);
    let verts = md.frame(frame);
    telemetry::stage_begin(telemetry::stage::TEXTURED_MODEL_PROJECT);
    let mut i = 0usize;
    while i + 2 < nv {
        let projected =
            scene::project_triangle_scheduled(verts.vert(i), verts.vert(i + 1), verts.vert(i + 2));
        MODEL_SCRATCH[i] = projected[0];
        MODEL_SCRATCH[i + 1] = projected[1];
        MODEL_SCRATCH[i + 2] = projected[2];
        i += 3;
    }
    while i < nv {
        MODEL_SCRATCH[i] = scene::project_vertex_scheduled(verts.vert(i));
        i += 1;
    }
    telemetry::stage_end(telemetry::stage::TEXTURED_MODEL_PROJECT);
    telemetry::stage_begin(telemetry::stage::TEXTURED_MODEL_FACES);
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
        if pa.sz < near_s || pb.sz < near_s || pc.sz < near_s {
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
        // Deflate sz by `s` back to 1x world depth, then sort by the FARTHEST
        // vertex at the world's OT_SHIFT -- exactly like world_otz_from_gte3 --
        // so models interleave correctly with world geometry. The old `>> 6`
        // predated OT_SHIFT=4: it sorted models 4x too near, so they drew on top
        // of walls/columns that should occlude them.
        let depthz = model_unscale_depth(
            (pa.sz as u32).max(pb.sz as u32).max(pc.sz as u32),
            s,
            scale_shift,
        );
        push_tri_uv_words(
            packets,
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            render_face.face.uv_words,
            [(shade, shade, shade); 3],
            slot.packet,
            clamp_otz((depthz >> OT_SHIFT) as usize),
        );
    }
    telemetry::stage_end(telemetry::stage::TEXTURED_MODEL_FACES);
    nv as u32
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

/// Build the viewmodel's GTE matrix: authored HL viewmodel axes plus the
/// first-person fit scale. The model-local precision scale is kept out of the
/// matrix and applied to translation/depth, matching the world-model path.
fn viewmodel_rot() -> Mat3I16 {
    let mut m = VM_BASE;
    let mut r = 0;
    while r < 3 {
        let mut c = 0;
        while c < 3 {
            m.m[r][c] =
                (m.m[r][c] as i32 * VM_SCALE).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            c += 1;
        }
        r += 1;
    }
    m
}

#[inline]
fn viewmodel_otz(avgz: u32) -> usize {
    let rel = avgz.saturating_sub(VM_OT_Z0) / VM_OT_STEP;
    1 + (rel as usize).min(WEAPON_OT_LEN - 2)
}

fn draw_sky(m: &Map, yaw: u16, pitch: i16) {
    if m.sky_tex_base == SKY_TEX_NONE || m.sky_tex_base + SKY_FACE_COUNT > MAX_TEX_SLOTS {
        return;
    }
    // Cooker face order: ft, rt, bk, lf, up, dn.
    let sky_yaw = ((yaw as usize) + 512) & 0xFFF;
    let side = (sky_yaw >> 10) & 3;
    let face = if pitch > 760 {
        4
    } else if pitch < -760 {
        5
    } else {
        side
    };
    let slot = unsafe { TEX_SLOTS[m.sky_tex_base + face] };
    if !slot.valid {
        return;
    }
    let u0 = if face < 4 {
        (((sky_yaw & 1023) * SKY_TEX_SIZE) >> 10) as u8
    } else {
        0
    };
    let u1 = u0.wrapping_add((SKY_TEX_SIZE - 1) as u8);
    let v1 = (SKY_TEX_SIZE - 1) as u8;
    gpu::draw_quad_textured_material(
        [(0, 0), (319, 0), (0, 239), (319, 239)],
        [(u0, 0), (u1, 0), (u0, v1), (u1, v1)],
        slot.material,
    );
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
    if slots.is_empty() {
        return; // viewmodel texture failed to upload: skip rather than index empty
    }
    let nv = md.n_verts.min(MAX_MODEL_VERTS);
    let local_to_world = md.local_to_world_q12();
    let (s, scale_shift) = model_local_scale_and_shift(local_to_world);
    if WEAPON_CACHE_FRAME != frame
        || WEAPON_CACHE_RECOIL != recoil_y
        || WEAPON_CACHE_VERTS != nv
        || WEAPON_CACHE_SCALE != local_to_world
    {
        let r = viewmodel_rot();
        scene::load_rotation(&r);
        scene::load_translation(Vec3I32::new(
            VM_VIEW_SHIFT[0] * s,
            (VM_VIEW_SHIFT[1] + recoil_y) * s,
            VM_VIEW_SHIFT[2] * s,
        ));
        let near_s = (NEAR as i32 * s) as u16;
        let verts = md.frame(frame);
        let mut i = 0usize;
        while i + 2 < nv {
            let projected = scene::project_triangle_scheduled(
                verts.vert(i),
                verts.vert(i + 1),
                verts.vert(i + 2),
            );
            MODEL_SCRATCH[i] = projected[0];
            MODEL_SCRATCH[i + 1] = projected[1];
            MODEL_SCRATCH[i + 2] = projected[2];
            i += 3;
        }
        while i < nv {
            MODEL_SCRATCH[i] = scene::project_vertex_scheduled(verts.vert(i));
            i += 1;
        }
        WEAPON_CACHE_FRAME = frame;
        WEAPON_CACHE_RECOIL = recoil_y;
        WEAPON_CACHE_VERTS = nv;
        WEAPON_CACHE_SCALE = local_to_world;

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
            if pa.sz < near_s || pb.sz < near_s || pc.sz < near_s {
                continue;
            }
            if VM_CULL && tex_id != VM_TWO_SIDED_TEX {
                let area = (pb.sx as i32 - pa.sx as i32) * (pc.sy as i32 - pa.sy as i32)
                    - (pc.sx as i32 - pa.sx as i32) * (pb.sy as i32 - pa.sy as i32);
                if (area >= 0) == VM_CULL_POS {
                    continue;
                }
            }
            let avgz = model_unscale_depth(
                ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3,
                s,
                scale_shift,
            );
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
    // Models are no longer loaded here: play() streams each map's model set into
    // the pool (stream_map_models) after the world loads.

    // SFX: stream the cooked SPU-ADPCM pack once and park it in SPU RAM (its
    // own 512 KB; no main-RAM cost). MAP_BUF is free until the first map loads,
    // so stage through it.
    let sfx_ready = {
        let len = cdstream::load_chunk(sfx::CHUNK_ID, unsafe { &mut MAP_BUF }).unwrap_or(0);
        let bytes = unsafe { streamed_map_bytes(len) };
        if len > 0 {
            unsafe { sfx::init_from_pack(bytes) }
        } else {
            0
        }
    };
    if sfx_ready == 0 {
        tty::println("hl-psx: SFX pack missing/failed (silent boot)");
    }

    // Boot flow: pick a map in the menu, stream + play it, return on Select.
    // (Analog is enabled inside play(); the menu runs on the digital pad.)
    loop {
        // Debug: the headless harness holds L1 with a map index in the pad's low
        // byte to boot any map directly (the menu only exposes 19 chapter starts),
        // for all-maps load verification. Released before gameplay.
        let mut dbg_sel = None;
        if DBG_PAD_BOOT {
            let mut i = 0;
            while i < 150 {
                gpu::vsync();
                fb.swap();
                let b = poll_port1().buttons;
                if b.is_held(button::L1) {
                    dbg_sel = Some((b.bits() & 0xFF) as usize);
                    break;
                }
                i += 1;
            }
        }
        let sel = dbg_sel.unwrap_or_else(|| menu::run(&mut fb));
        let mut launch = menu_launch(sel);
        // First load comes from the menu (fresh -> full loading card). A
        // changelevel re-enters play() with the previous frame still on screen,
        // so keep it frozen and overlay only a tiny "Loading" strip.
        let mut keep_frame = false;
        loop {
            match play(&mut fb, launch, keep_frame) {
                PlayExit::BackToMenu => break,
                PlayExit::ChangeLevel(next) => {
                    launch = next;
                    keep_frame = true;
                }
            }
        }
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

#[inline]
fn account_streamed_chunk(len: usize, chunks: &mut u32, bytes: &mut u32, sectors: &mut u32) {
    *chunks = chunks.saturating_add(1);
    *bytes = bytes.saturating_add(len as u32);
    *sectors = sectors.saturating_add(((len as u32) + 2047) / 2048);
}

fn stream_model_texture_chunk(
    chunk_id: u32,
    dst_word: usize,
    stage_end: usize,
    slots: *mut TexSlot,
    slot_len: usize,
    stream_chunks: &mut u32,
    stream_bytes: &mut u32,
    stream_sectors: &mut u32,
) -> Option<(usize, usize)> {
    telemetry::stage_begin(telemetry::stage::CD_WORLD_PACK_STREAM);
    // Stage the texture in the free tail above the geometry loaded so far, NOT
    // at offset 0 (offset 0 holds live geometry draw_model reads; a texture
    // there would clobber it and crash -- this was c1a2a). `stage_end` bounds
    // the scratch so it cannot spill into the NEXT region: a viewmodel streamed
    // mid-switch stages inside the pool (stage_end = VM_POOL_WORDS) and never
    // touches the resident enemies above it. load_chunk refuses a chunk larger
    // than its destination, so a tex that won't fit is skipped (untextured).
    let len = {
        let buf = unsafe { &mut MODEL_BUF };
        let end = stage_end.min(buf.len());
        if dst_word >= end {
            telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
            return None;
        }
        cdstream::load_chunk(chunk_id, &mut buf[dst_word..end]).unwrap_or(0)
    };
    telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
    if len == 0 {
        return None;
    }
    account_streamed_chunk(len, stream_chunks, stream_bytes, stream_sectors);
    let bytes = unsafe { streamed_model_bytes_at(dst_word * 4, len) };
    telemetry::stage_begin(telemetry::stage::VRAM_UPLOAD);
    let uploaded = unsafe { vram::upload_tex_chunk_append_raw(bytes, slots, slot_len) };
    telemetry::stage_end(telemetry::stage::VRAM_UPLOAD);
    uploaded
}

/// Stream a room from WORLD.PAK, upload its textures, and run the renderer +
/// physics loop until Select returns to menu or a trigger_changelevel requests
/// the next room.
fn play(fb: &mut FrameBuffer, launch: RoomLaunch, keep_frame: bool) -> PlayExit {
    let _ = enable_analog_port1();
    unsafe {
        CHANGE_REQUEST_ACTIVE = 0;
    }
    telemetry::frame_begin(0);
    telemetry::task_begin(telemetry::task::FIXED_UPDATE);
    let texture_chunk_id = room_texture_chunk_id(launch.room_id as u32);
    let world_chunk_id = room_world_chunk_id(launch.room_id as u32);
    telemetry::debug_log("hl-psx: loading room");
    if (launch.room_id as usize) < menu::MAPS.len() {
        telemetry::debug_log(menu::MAPS[launch.room_id as usize]);
    }
    let loading_label = loading_label_for_room(launch.room_id);
    let mut loading_frame = 0u8;
    draw_next_loading_screen(fb, loading_label, &mut loading_frame, keep_frame);

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
        return PlayExit::BackToMenu;
    }
    telemetry::debug_log("hl-psx: WORLD.PAK texture chunk loaded");

    draw_next_loading_screen(fb, loading_label, &mut loading_frame, keep_frame);
    let tex_bytes = unsafe { streamed_map_bytes(tex_len) };
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
            return PlayExit::BackToMenu;
        }
    };
    telemetry::stage_end(telemetry::stage::VRAM_UPLOAD);
    let model_tex_failed = 0usize;
    let model_texs = 0usize;
    draw_next_loading_screen(fb, loading_label, &mut loading_frame, keep_frame);
    // Stream the curated viewmodel set (geometry -> MODEL_BUF head reserve,
    // textures -> VM_SLOTS) so weapon switching is instant. Textures stage above
    // the reserve, in the enemy region stream_map_models fills afterwards.
    telemetry::stage_begin(telemetry::stage::CD_WORLD_PACK_STREAM);
    let glock_vm_ok = unsafe {
        load_resident_viewmodels(&mut stream_chunks, &mut stream_bytes, &mut stream_sectors)
    };
    telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
    if !glock_vm_ok {
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_CHUNKS, stream_chunks);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_BYTES, stream_bytes);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_SECTORS, stream_sectors);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_STATUS, 0);
        telemetry::task_end(telemetry::task::FIXED_UPDATE);
        tty::println("hl-psx: WORLD.PAK viewmodel stream failed");
        telemetry::debug_log("hl-psx: WORLD.PAK viewmodel stream failed");
        return PlayExit::BackToMenu;
    }
    telemetry::counter(
        telemetry::counter::ROOM_TEXTURE_UPLOADS,
        (room_texs.saturating_sub(tex_failed) + model_texs.saturating_sub(model_tex_failed)) as u32,
    );
    telemetry::counter(
        telemetry::counter::ROOM_MATERIAL_TEXTURE_DROPS,
        (tex_failed + model_tex_failed) as u32,
    );
    telemetry::debug_log("hl-psx: WORLD.PAK viewmodels loaded");
    unsafe {
        WEAPON_CACHE_FRAME = usize::MAX;
        WEAPON_CACHE_RECOIL = i32::MIN;
        WEAPON_CACHE_VERTS = 0;
        WEAPON_CACHE_SCALE = 0;
        WEAPON_TRI_COUNT = 0;
    }
    draw_next_loading_screen(fb, loading_label, &mut loading_frame, keep_frame);
    let hud_mat = hud::upload(); // real HUD sprite sheet -> free gameplay tpage

    draw_next_loading_screen(fb, loading_label, &mut loading_frame, keep_frame);
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
        return PlayExit::BackToMenu;
    }
    telemetry::debug_log("hl-psx: WORLD.PAK world chunk loaded");

    let map_bytes = unsafe { streamed_map_bytes(map_len) };
    let m = Map::load(map_bytes);
    if room_texs != m.n_texs {
        tty::println("hl-psx: texture/world count mismatch");
        telemetry::debug_log("hl-psx: texture/world count mismatch");
    }
    phys::set_gravity_scale(4096); // fresh map: normal gravity until a zone says otherwise
    unsafe {
        PVS_CAM_LEAF = -1;
        PVS_LEAF_COUNT = 0;
        PVS_ENT_COUNT = 0;
        // Water/liquid textures were rendered translucent (Average blend) here.
        // DISABLED: HL liquid brushes have no geometry drawn behind the surface
        // (the volume is solid, or it caps a deep/void pit), so an Average blend
        // over the dark backdrop rendered liquids near-black -- reads as missing
        // floor. Faithful transparent water needs the underwater scene drawn
        // behind the surface (a real feature); until then liquids stay opaque
        // (visible) instead of dark. The cook still flags them (face_translucent)
        // so re-enabling is a one-line flip.
        if LIQUID_TRANSPARENCY {
            for f in 0..m.n_faces {
                if m.face_translucent(f) {
                    let t = m.face_tex(f);
                    if t < MAX_TEX_SLOTS && TEX_SLOTS[t].valid {
                        let blended = TEX_SLOTS[t].material.with_blend_mode(BlendMode::Average);
                        TEX_SLOTS[t].material = blended;
                        // The emit path samples the pre-packed `packet`, so rebuild
                        // it (it carries the semi-transp bit + tpage blend).
                        TEX_SLOTS[t].packet =
                            psx_gpu::material::TexturedGouraudPacketMaterial::from_texture(blended);
                    }
                }
            }
        }
        init_prop_state(&m);
        stream_map_models(&m, VM_POOL_WORDS * 4); // enemies stream after the viewmodel reserve
        clear_combat_fx();
    }
    let nents = m.n_ents.min(MAX_ENTS);
    let nlogic = m.n_logic.min(MAX_LOGIC);
    unsafe {
        for ei in 0..nents {
            let e = m.entity(ei);
            ENT_CACHE[ei] = e;
            ENT_RADIUS[ei] = isqrt(e.r2);
        }
        init_logic_state(&m, nlogic, nents, 0);
    }
    let nv = if m.n_verts < MAX_VERTS {
        m.n_verts
    } else {
        MAX_VERTS
    };

    let spawn_pos = if let Some(origin) = launch_landmark_origin(&m, nlogic, launch.landmark) {
        telemetry::debug_log("hl-psx: spawning at landmark");
        [
            origin[0] + launch.landmark_offset[0],
            origin[1] + launch.landmark_offset[1],
            origin[2] + launch.landmark_offset[2],
        ]
    } else {
        if launch.landmark.len > 0 {
            telemetry::debug_log("hl-psx: landmark missing, using spawn");
        }
        m.spawn_pos
    };
    let mut player = phys::Player::new(spawn_pos);
    let mut yaw: u16 = if launch.preserve_view {
        launch.yaw
    } else {
        (m.spawn_yaw as u16) & 0xFFF
    };
    let mut pitch: i16 = if launch.preserve_view {
        launch.pitch
    } else {
        0
    };
    let mut frame_no: u16 = 0;
    let mut telemetry_frame: u32 = 1;
    let mut sim_frame_no: u32 = 0;
    let mut weapon = Arsenal::new();
    weapon.clip[W_GLOCK] = launch.clip_ammo.min(WEAPON_DEFS[W_GLOCK].clip);
    weapon.ammo[AMMO_9MM] = launch.reserve_ammo.min(max_reserve_for(AMMO_9MM));
    // Changelevel restores the carried arsenal; fresh/menu launches start with
    // the HL crowbar+glock baseline and pick the rest up in the world.
    unsafe {
        if launch.preserve_view && CARRY_VALID {
            weapon.owned = CARRY_OWNED;
            weapon.clip = CARRY_CLIPS;
            weapon.ammo = CARRY_AMMO;
            let cur = CARRY_CURRENT as usize;
            if cur < N_WEAPONS && weapon.owns(cur) {
                weapon.current = cur;
            }
        } else {
            CARRY_VALID = false;
        }
    }
    let mut fire_was_held = false; // rising-edge latch for non-auto weapons
    let mut switch_prev = false; // rising-edge latch for L1/R1 weapon cycling
    let mut pending_vm_switch = false; // re-stream the viewmodel after a weapon change
    let mut health: u16 = launch.health;
    let mut armor: u16 = launch.armor.min(HEV_MAX_ARMOR);
    let mut death_ticks: u8 = 0; // >0 while dead; respawns at 0
    let mut suit_equipped = launch.suit_equipped
        || standalone_room_starts_with_hev(launch.room_id as usize)
        || armor > 0;
    if !suit_equipped {
        armor = PLAYER_START_ARMOR;
    }
    let mut pickup_kind: u8 = hud::PICKUP_NONE;
    let mut pickup_ticks: u8 = 0;
    let mut use_cooldown: u8 = 0;

    // Tracktrain mover: the BSP cooker stores the train's path_track chain, and
    // the logic system toggles it through the func_tracktrain targetname.
    let wp0 = if m.n_way > 0 {
        m.waypoint(0)
    } else {
        [0, 0, 0]
    };
    let mut recoil = 0i32; // viewmodel kick when firing
    let mut tram_active = false;
    let mut tram_speed = m.tram_speed.max(0);
    let mut tram_player_attached = false;
    let mut tram_seg = 0usize;
    let mut tram_seg_dist = 0i32;
    let mut ride_off = [0i32; 3];
    unsafe {
        if let Some((use_type, speed)) = logic_take_tracktrain_command() {
            let started = tram_apply_command(use_type, speed, &mut tram_active, &mut tram_speed);
            if started {
                tram_player_attached = tram_should_carry_player(
                    player.pos,
                    tram_path_pos(&m, tram_seg, tram_seg_dist),
                );
            }
        }
    }

    telemetry::stage_begin(telemetry::stage::ROOM_SURFACE_CACHE);
    unsafe {
        let initial_eye = [player.pos[0], player.pos[1] + VIEW_HEIGHT, player.pos[2]];
        let initial_train_hint = tram_path_pos(&m, tram_seg, tram_seg_dist);
        let initial_leaf = recover_camera_leaf(&m, initial_eye, player.pos, initial_train_hint);
        if valid_pvs_leaf(&m, initial_leaf) {
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
        let (vm_model, vm_slot, vm_n) = viewmodel_for(weapon.current);
        draw_viewmodel(
            &mut warm_packets,
            &vm_model,
            &VM_SLOTS[vm_slot..vm_slot + vm_n],
            VM_FRAME,
            -recoil,
            &mut warm_np,
        );
        WEAPON_OT.clear();
    }
    telemetry::stage_end(telemetry::stage::ROOM_SURFACE_CACHE);

    // Menu loading cards are drawn directly into the double buffers, so clear
    // both pages once before the first gameplay frame -- otherwise sparse world
    // coverage could leave a stale loading panel behind. On a level->level
    // transition (keep_frame) we deliberately DON'T clear: the previous scene
    // stays frozen (with the tiny overlay) until the first rendered frame of the
    // new level swaps over it, so there is no black flash between levels.
    if !keep_frame {
        fb.clear(0, 0, 0);
        gpu::draw_sync();
        fb.swap();
        fb.clear(0, 0, 0);
        gpu::draw_sync();
        fb.swap();
    }

    gpu::configure_vsync_timer();
    interrupts::install_vblank_counter();
    let mut next_sim_vblank = interrupts::vblank_count().wrapping_add(SIM_VBLANKS);
    let mut prev_pause_button = true;

    'gameplay: loop {
        wait_until_vblank(next_sim_vblank);
        let mut ticks_this_visual = 0u16;
        while vblank_reached(interrupts::vblank_count(), next_sim_vblank) {
            telemetry::frame_begin(telemetry_frame);
            telemetry::task_begin(telemetry::task::FIXED_UPDATE);
            telemetry::stage_begin(telemetry::stage::UPDATE);
            // Modern twin-stick FPS: left stick moves/strafes, right stick looks
            // (X = turn, Y = pitch), Cross = jump. Analog only.
            let pad = poll_port1();
            let pause_button =
                pad.buttons.is_held(button::START) || pad.buttons.is_held(button::SELECT);
            if pause_button && !prev_pause_button {
                telemetry::stage_end(telemetry::stage::UPDATE);
                telemetry::task_end(telemetry::task::FIXED_UPDATE);
                match run_pause_menu(fb) {
                    PauseExit::MainMenu => return PlayExit::BackToMenu,
                    PauseExit::Resume => {
                        let _ = enable_analog_port1();
                        prev_pause_button = true;
                        next_sim_vblank = interrupts::vblank_count().wrapping_add(SIM_VBLANKS);
                        continue 'gameplay;
                    }
                }
            }
            prev_pause_button = pause_button;
            // Analog is required: if the pad ever isn't in analog mode, re-assert it.
            if !pad.is_analog() {
                let _ = enable_analog_port1();
            }
            // R2 fires at the Glock cadence; the actual hit-test runs after
            // movement/movers update so it uses the current camera and doors.
            recoil = (recoil - 3).max(0);
            weapon.tick();
            unsafe {
                decay_combat_fx();
            }
            if pickup_ticks > 0 {
                pickup_ticks -= 1;
                if pickup_ticks == 0 {
                    pickup_kind = hud::PICKUP_NONE;
                }
            }
            // Player death: freeze for DEATH_TICKS (a red death screen renders),
            // then respawn at the map spawn -- suit kept, armor + ammo reset.
            if death_ticks > 0 {
                death_ticks -= 1;
                if death_ticks == 0 {
                    player = phys::Player::new(spawn_pos);
                    yaw = (m.spawn_yaw as u16) & 0xFFF;
                    pitch = 0;
                    health = PLAYER_START_HEALTH;
                    armor = 0;
                    weapon = Arsenal::new();
                    pending_vm_switch = true; // respawn forces the glock viewmodel
                }
            }
            let dead = death_ticks > 0;
            // Auto weapons fire while held; everything else fires once per press.
            let fire_held = !dead && pad.buttons.is_held(button::R2);
            let want_fire = fire_held && (weapon.def().fire == FIRE_AUTO || !fire_was_held);
            fire_was_held = fire_held;
            let want_reload = pad.buttons.is_held(button::CIRCLE);
            // L1/R1 cycle owned weapons (rising edge so a hold steps once).
            let sw_next = pad.buttons.is_held(button::R1);
            let sw_prev = pad.buttons.is_held(button::L1);
            let sw_held = sw_next || sw_prev;
            if !dead && sw_held && !switch_prev && weapon.cycle(sw_next) {
                pending_vm_switch = true;
            }
            switch_prev = sw_held;
            if pending_vm_switch {
                pending_vm_switch = false;
                unsafe {
                    // Stream the newly-selected weapon's viewmodel if it is not
                    // resident yet (first switch to it): a brief hitch, then it
                    // stays resident for instant re-selection. Runs before the
                    // draw below, so the frame shows the real model, not a flash
                    // of the glock. Falls back to the glock if it cannot load.
                    let mut d = (0u32, 0u32, 0u32);
                    stream_one_viewmodel(weapon.current, &mut d.0, &mut d.1, &mut d.2);
                    WEAPON_CACHE_FRAME = usize::MAX; // re-cache the viewmodel for the new weapon
                }
            }
            if use_cooldown > 0 {
                use_cooldown -= 1;
            }
            let want_use = pad.buttons.is_held(button::SQUARE) && use_cooldown == 0;
            if want_use {
                use_cooldown = 8;
            }
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
            if dead {
                fwd = 0;
                strafe = 0;
                turn = 0;
                look = 0;
            }
            yaw = (((yaw as i32) + (turn * YAW_RATE) / 128) & 0xFFF) as u16;
            pitch = (pitch + ((look * PITCH_RATE) / 128) as i16).clamp(-PITCH_MAX, PITCH_MAX);
            unsafe {
                LOGIC_PLAYER_POS = player.pos;
                LOGIC_PLAYER_YAW = yaw;
                LOGIC_PLAYER_PITCH = pitch;
                LOGIC_PLAYER_HEALTH = health;
                LOGIC_PLAYER_SUIT = if suit_equipped { 1 } else { 0 };
                LOGIC_PLAYER_ARMOR = armor;
                LOGIC_PLAYER_CLIP_AMMO = weapon.clip_display();
                LOGIC_PLAYER_RESERVE_AMMO = weapon.reserve_display();
                SIM_NOW = sim_frame_no as u16;
                logic_pre_tick(&m, nlogic, nents, sim_frame_no as u16);
            }

            let prev_train_pos = tram_path_pos(&m, tram_seg, tram_seg_dist);
            unsafe {
                if let Some((use_type, speed)) = logic_take_tracktrain_command() {
                    let started =
                        tram_apply_command(use_type, speed, &mut tram_active, &mut tram_speed);
                    if !tram_active {
                        tram_player_attached = false;
                    } else if started && !tram_player_attached {
                        tram_player_attached = tram_should_carry_player(player.pos, prev_train_pos);
                    }
                }
            }
            let prev_ride_off = ride_off;
            if tram_active {
                let still_moving = tram_advance(
                    &m,
                    &mut tram_seg,
                    &mut tram_seg_dist,
                    tram_step_for_speed(tram_speed),
                );
                let train_pos = tram_path_pos(&m, tram_seg, tram_seg_dist);
                ride_off = [
                    train_pos[0] - wp0[0],
                    train_pos[1] - wp0[1],
                    train_pos[2] - wp0[2],
                ];
                if !still_moving {
                    tram_active = false;
                    tram_speed = 0;
                }
            }
            let tram_delta = [
                ride_off[0] - prev_ride_off[0],
                ride_off[1] - prev_ride_off[1],
                ride_off[2] - prev_ride_off[2],
            ];
            if tram_delta != [0, 0, 0] {
                let train_pos = tram_path_pos(&m, tram_seg, tram_seg_dist);
                if !tram_player_attached && tram_should_carry_player(player.pos, prev_train_pos) {
                    tram_player_attached = true;
                }
                if tram_player_attached {
                    if tram_should_carry_player(player.pos, prev_train_pos)
                        || tram_should_carry_player(player.pos, train_pos)
                    {
                        player.pos = [
                            player.pos[0] + tram_delta[0],
                            player.pos[1] + tram_delta[1],
                            player.pos[2] + tram_delta[2],
                        ];
                    } else {
                        tram_player_attached = false;
                    }
                }
            }

            // Collision movers: every brush entity at its current offset (doors at
            // their open amount, statics at origin) + the tram at its ride offset.
            let mut movers = [phys::NO_MOVER; MAX_ENTS + 1];
            let mut nmov = 0;
            unsafe {
                for ei in 0..nents {
                    if ENT_ACTIVE[ei] == 0 {
                        continue;
                    }
                    let e = ENT_CACHE[ei];
                    let off = ent_draw_offset(ei);
                    // kind 2 = nonsolid visual, kind 4 = ladder volume (no hull).
                    if e.kind != 2 && e.kind != 4 && nmov < movers.len() {
                        movers[nmov] = phys::Mover {
                            head: e.head,
                            off,
                            center: e.center,
                            radius: ENT_RADIUS[ei],
                            id: ei as i32,
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
                        id: -2, // the tram has its own carry path
                    };
                    nmov += 1;
                }
            }
            let movers = &movers[..nmov];

            // Ride moving brushes: if a mover we stood on last tick shifted
            // (plat/elevator door phase), carry the player by the same delta so
            // they stay planted instead of sliding off or falling through.
            unsafe {
                if player.ground_mover >= 0 && (player.ground_mover as usize) < nents {
                    let ei = player.ground_mover as usize;
                    let now_off = ent_draw_offset(ei);
                    let prev = ENT_PREV_OFF[ei];
                    let d = [
                        now_off[0] - prev[0],
                        now_off[1] - prev[1],
                        now_off[2] - prev[2],
                    ];
                    if d != [0, 0, 0] {
                        player.pos[0] += d[0];
                        player.pos[1] += d[1];
                        player.pos[2] += d[2];
                    }
                }
                let mut ei = 0usize;
                while ei < nents {
                    ENT_PREV_OFF[ei] = ent_draw_offset(ei);
                    ei += 1;
                }
            }

            // Full player physics always runs; moving trains carry the player by
            // delta before the update, then block them through their shifted hull.
            telemetry::stage_begin(telemetry::stage::SIM_COLLISION);
            let on_ladder = unsafe { ladder_touch(&m, nents, player.pos) };
            if on_ladder {
                player.update_climb(
                    &m,
                    movers,
                    fwd,
                    strafe,
                    pad.buttons.is_held(button::CROSS),
                    yaw,
                    pitch,
                );
            } else {
                player.update(
                    &m,
                    movers,
                    fwd,
                    strafe,
                    pad.buttons.is_held(button::CROSS),
                    yaw,
                );
            }
            telemetry::stage_end(telemetry::stage::SIM_COLLISION);
            let eye = [player.pos[0], player.pos[1] + VIEW_HEIGHT, player.pos[2]];
            unsafe {
                collect_pickups(
                    player.pos,
                    &mut suit_equipped,
                    &mut armor,
                    &mut health,
                    &mut weapon,
                    &mut pickup_kind,
                    &mut pickup_ticks,
                );
                LOGIC_PLAYER_POS = player.pos;
                LOGIC_PLAYER_YAW = yaw;
                LOGIC_PLAYER_PITCH = pitch;
                LOGIC_PLAYER_HEALTH = health;
                LOGIC_PLAYER_SUIT = if suit_equipped { 1 } else { 0 };
                LOGIC_PLAYER_ARMOR = armor;
                LOGIC_PLAYER_CLIP_AMMO = weapon.clip_display();
                LOGIC_PLAYER_RESERVE_AMMO = weapon.reserve_display();
                if want_use {
                    // Standing on the tracktrain + use = drive it (On A Rail);
                    // otherwise aim-use doors/buttons/chargers.
                    let train_pos = tram_path_pos(&m, tram_seg, tram_seg_dist);
                    if m.tram_submodel > 0 && tram_should_carry_player(player.pos, train_pos) {
                        TRACKTRAIN_CMD_ACTIVE = 1;
                        TRACKTRAIN_CMD_USE_TYPE = map::USE_TOGGLE;
                        TRACKTRAIN_CMD_SPEED = TRACKTRAIN_USE_SPEED;
                        sfx::play(sfx::BUTTON);
                    } else {
                        logic_try_use(
                            &m,
                            nlogic,
                            nents,
                            eye,
                            yaw,
                            pitch,
                            movers,
                            sim_frame_no as u16,
                            &mut health,
                            &mut armor,
                        );
                    }
                }
                logic_touch_triggers(
                    &m,
                    nlogic,
                    nents,
                    player.pos,
                    &mut health,
                    &mut armor,
                    sim_frame_no as u16,
                );
                // Teleport lands before this frame renders (the render eye is
                // recomputed below); push adds velocity while inside the volume.
                if let Some((dest, dyaw)) = TELEPORT_REQUEST.take() {
                    player.pos = dest;
                    player.vel = [0, 0, 0];
                    player.on_ground = false;
                    yaw = dyaw & 0xFFF;
                }
                if PUSH_IMPULSE != [0, 0, 0] {
                    player.vel[1] += PUSH_IMPULSE[1];
                    // Lateral push nudges the position directly (vel xz is
                    // recomputed from the stick every tick).
                    player.pos[0] += PUSH_IMPULSE[0];
                    player.pos[2] += PUSH_IMPULSE[2];
                    PUSH_IMPULSE = [0; 3];
                }
            }
            if want_reload {
                let _ = weapon.start_reload();
            }
            if want_fire && weapon.try_fire() {
                recoil = 16;
                let fire_rot = view_rotation(yaw, pitch);
                let fire_base_t = [
                    -dot12(fire_rot.m[0], eye),
                    -dot12(fire_rot.m[1], eye),
                    -dot12(fire_rot.m[2], eye),
                ];
                unsafe {
                    let hit = fire_weapon(weapon.def(), &m, movers, eye, &fire_rot, fire_base_t);
                    sfx::play(weapon_fire_sfx(weapon.current, hit));
                }
            }
            unsafe {
                sfx::set_ear(player.pos);
                if PAIN_SFX_COOLDOWN > 0 {
                    PAIN_SFX_COOLDOWN -= 1;
                }
                tick_projectiles(&m, movers);
                tick_props(&m, movers, player.pos, &mut health, &mut armor);
                LOGIC_PLAYER_HEALTH = health;
                LOGIC_PLAYER_ARMOR = armor;
                LOGIC_PLAYER_CLIP_AMMO = weapon.clip_display();
                LOGIC_PLAYER_RESERVE_AMMO = weapon.reserve_display();
            }
            if health == 0 && death_ticks == 0 {
                death_ticks = DEATH_TICKS; // enemies killed the player -> start the death window
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

            unsafe {
                if CHANGE_REQUEST_ACTIVE != 0 {
                    CHANGE_REQUEST_ACTIVE = 0;
                    // Carry the whole arsenal into the next map.
                    CARRY_VALID = true;
                    CARRY_OWNED = weapon.owned;
                    CARRY_CLIPS = weapon.clip;
                    CARRY_AMMO = weapon.ammo;
                    CARRY_CURRENT = weapon.current as u8;
                    return PlayExit::ChangeLevel(CHANGE_REQUEST);
                }
            }
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

        if DBG_CAM {
            player.pos = DBG_CAM_POS;
            yaw = DBG_CAM_YAW;
            pitch = DBG_CAM_PITCH;
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
            FX_OT.clear();
            XHAIR = EMPTY_XHAIR; // DEBUG: reset crosshair-tri pick for this frame
            let mut packets = PrimitivePacketArena::new(&mut PRIMITIVE_PACKETS);
            let mut np = 0usize;
            let mut nq = 0usize;

            // World (model 0): PVS-visible faces cached by leaf. Runtime does
            // one backface test per cooked plane group, cheap face-bounds
            // rejection, then emits triangle fans with PS1 quad pairing.
            telemetry::stage_begin(telemetry::stage::ROOM);
            let train_hint = tram_path_pos(&m, tram_seg, tram_seg_dist);
            let mut cam_leaf = recover_camera_leaf(&m, eye, player.pos, train_hint);
            let mut have_pvs = valid_pvs_leaf(&m, cam_leaf);
            let mut reused_last_pvs = false;
            if !have_pvs && valid_pvs_leaf(&m, PVS_CAM_LEAF) {
                cam_leaf = PVS_CAM_LEAF;
                have_pvs = true;
                reused_last_pvs = true;
            }
            if have_pvs {
                if !reused_last_pvs && PVS_CAM_LEAF != cam_leaf {
                    telemetry::stage_begin(telemetry::stage::ROOM_VISIBLE_LIST);
                    rebuild_pvs_cache(&m, cam_leaf, nents);
                    telemetry::stage_end(telemetry::stage::ROOM_VISIBLE_LIST);
                }
                let mut room_counts = WorldCounters::new();
                room_counts.cells_considered = PVS_LEAF_COUNT as u32;
                room_counts.cells_drawn = PVS_LEAF_COUNT as u32;
                room_counts.surfaces_considered = PVS_FACE_COUNT as u32;

                telemetry::stage_begin(telemetry::stage::ROOM_SURFACE_DRAW);
                // Front-to-back banded emit: when a view exposes more geometry
                // than the packet arena can hold (huge open rooms), process near
                // faces first so the arena fills with close geometry and only the
                // farthest faces drop -- the wall in front of you always draws,
                // never a black hole up close. Normal views run a single pass.
                const N_DEPTH_BANDS: i32 = 8;
                const DEPTH_BAND_SHIFT: u32 = 8; // 256 world units per band
                // Band only when the arena would overflow; otherwise one pass (the
                // common case, no cost). On overflow use just enough bands to span
                // FAR_VIEW -- no empty re-walks past the view distance, and if a band
                // is wider than FAR_VIEW this collapses to 1 on its own. Emitting near
                // bands first means an overflowing arena drops the FARTHEST faces, not
                // the floor under your feet (which is what an unbanded late-group emit
                // would drop -- the missing-near-geometry bug this fixes).
                let nbands = if PVS_TRI_REF_COUNT > MAX_RENDER_PACKETS {
                    ((FAR_VIEW >> DEPTH_BAND_SHIFT) + 1).min(N_DEPTH_BANDS)
                } else {
                    1
                };
                let mut band = 0i32;
                while band < nbands {
                    for gi in 0..PVS_GROUP_COUNT {
                        let group = PVS_GROUP_ACTIVE[gi] as usize;
                        let plane_face = PVS_GROUP_FACE[group] as usize;
                        let (plane_n, plane_d) = m.face_plane(plane_face);
                        if dot12(plane_n, eye) <= plane_d {
                            continue;
                        }
                        let mut entry = PVS_GROUP_FIRST[group];
                        while entry != PVS_LINK_END {
                            let e = entry as usize;
                            entry = PVS_FACE_NEXT[e];
                            if e < MAX_PVS_FACE_RECS {
                                let rec = PVS_FACE_REC[e];
                                // Depth-band gate only matters in multi-band mode;
                                // with a single pass every face is band 0, so skip
                                // the per-face dot12 depth entirely (faithful -- the
                                // check always passed when nbands == 1).
                                if nbands > 1 {
                                    let c = [
                                        rec.center[0] as i32,
                                        rec.center[1] as i32,
                                        rec.center[2] as i32,
                                    ];
                                    let depth = (dot12(rot.m[2], c) + base_t[2]).max(0);
                                    if (depth >> DEPTH_BAND_SHIFT).min(nbands - 1) != band {
                                        continue;
                                    }
                                }
                                if !WORLD_BOUNDS_CULL || cached_face_visible(rec, &rot, base_t) {
                                    if rec.is_loop {
                                        emit_world_face_loop(
                                            &mut packets,
                                            &m,
                                            rec.tex as usize,
                                            rec.first as usize,
                                            rec.count as usize,
                                            nv,
                                            frame_no,
                                            &mut np,
                                            &mut nq,
                                            &mut room_counts,
                                        );
                                    } else {
                                        emit_world_face_tris(
                                            &mut packets,
                                            &m,
                                            rec.first as usize,
                                            rec.count as usize,
                                            nv,
                                            frame_no,
                                            &mut np,
                                            &mut nq,
                                            &mut room_counts,
                                        );
                                    }
                                }
                            } else {
                                let face = PVS_FACE_INDEX[e] as usize;
                                let (bc, be) = m.face_bounds(face);
                                let depth = (dot12(rot.m[2], bc) + base_t[2]).max(0);
                                if (depth >> DEPTH_BAND_SHIFT).min(nbands - 1) != band {
                                    continue;
                                }
                                if !WORLD_BOUNDS_CULL || face_bounds_visible(bc, be, &rot, base_t) {
                                    let (first, cnt) = m.face_tris(face);
                                    if m.face_is_loop(face) {
                                        emit_world_face_loop(
                                            &mut packets,
                                            &m,
                                            m.face_tex(face),
                                            first,
                                            cnt,
                                            nv,
                                            frame_no,
                                            &mut np,
                                            &mut nq,
                                            &mut room_counts,
                                        );
                                    } else {
                                        emit_world_face_tris(
                                            &mut packets,
                                            &m,
                                            first,
                                            cnt,
                                            nv,
                                            frame_no,
                                            &mut np,
                                            &mut nq,
                                            &mut room_counts,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    band += 1;
                }
                telemetry::stage_end(telemetry::stage::ROOM_SURFACE_DRAW);

                telemetry::counter(
                    telemetry::counter::ROOM_CELLS_CONSIDERED,
                    room_counts.cells_considered,
                );
                telemetry::counter(
                    telemetry::counter::ROOM_CELLS_DRAWN,
                    room_counts.cells_drawn,
                );
                telemetry::counter(
                    telemetry::counter::ROOM_CELLS_CULLED,
                    room_counts.cells_culled,
                );
                telemetry::counter(
                    telemetry::counter::ROOM_SURFACES_CONSIDERED,
                    room_counts.surfaces_considered,
                );
                telemetry::counter(
                    telemetry::counter::ROOM_SURF_PROFILED,
                    room_counts.emit_calls,
                );
                telemetry::counter(
                    telemetry::counter::ROOM_VISIBILITY_FALLBACK_DRAWS,
                    if reused_last_pvs { 1 } else { 0 },
                );
            } else {
                let draw_token = next_draw_face_mark_token();
                let mut room_counts = WorldCounters::new();
                let mut face = 0usize;
                telemetry::stage_begin(telemetry::stage::ROOM_SURFACE_DRAW);
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
                telemetry::stage_end(telemetry::stage::ROOM_SURFACE_DRAW);
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
                telemetry::counter(telemetry::counter::ROOM_VISIBILITY_FALLBACK_DRAWS, 1);
            }
            telemetry::stage_end(telemetry::stage::ROOM);

            telemetry::stage_begin(telemetry::stage::MODEL_INSTANCES);
            let model_prims0 = np;
            let model_quads0 = nq;
            let mut model_draws = 0u32;
            let mut model_bounds_tests = 0u32;
            let mut model_bounds_culled = 0u32;
            let mut model_culled_tris = 0u32;
            let mut model_projected_vertices = 0u32;

            // Brush entities: doors slide open near the player. Each renders with
            // a per-entity GTE translation (base view shifted by the offset);
            // its few tris are projected fresh (not from the world cache).
            telemetry::stage_begin(telemetry::stage::MODEL_BOUNDS);
            let brush_iter_count = if have_pvs { PVS_ENT_COUNT } else { nents };
            for bi in 0..brush_iter_count {
                let ei = if have_pvs { PVS_ENTS[bi] as usize } else { bi };
                if ENT_ACTIVE[ei] == 0 {
                    continue;
                }
                let e = ENT_CACHE[ei];
                let off = ent_draw_offset(ei);
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
                if e.kind != 1 && e.kind != 3 {
                    let mut static_counts = WorldCounters::new();
                    let (ff, nf) = m.submodel(e.submodel);
                    for f in ff..ff + nf {
                        let (fnrm, fd) = m.face_plane(f);
                        if dot12(fnrm, eye) <= fd {
                            let (_, cnt) = m.face_tris(f);
                            model_culled_tris = model_culled_tris.saturating_add(cnt as u32);
                            continue;
                        }
                        let (bc, be) = m.face_bounds(f);
                        if WORLD_BOUNDS_CULL && !face_bounds_visible(bc, be, &rot, base_t) {
                            continue;
                        }
                        let (first, cnt) = m.face_tris(f);
                        if m.face_is_loop(f) {
                            emit_world_face_loop(
                                &mut packets,
                                &m,
                                m.face_tex(f),
                                first,
                                cnt,
                                nv,
                                frame_no,
                                &mut np,
                                &mut nq,
                                &mut static_counts,
                            );
                        } else {
                            emit_world_face_tris(
                                &mut packets,
                                &m,
                                first,
                                cnt,
                                nv,
                                frame_no,
                                &mut np,
                                &mut nq,
                                &mut static_counts,
                            );
                        }
                    }
                    continue;
                }
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
                    let (bc, be) = m.face_bounds(f);
                    let moved_center = [bc[0] + off[0], bc[1] + off[1], bc[2] + off[2]];
                    if WORLD_BOUNDS_CULL && !face_bounds_visible(moved_center, be, &rot, base_t) {
                        continue;
                    }
                    emit_submodel_face(&mut packets, &m, f, first, cnt, nv, submodel_token, &mut np);
                }
            }
            telemetry::stage_end(telemetry::stage::MODEL_BOUNDS);

            // Tram car: render its submodel at the current ride offset.
            telemetry::stage_begin(telemetry::stage::MODEL_DRAW);
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
                    let (bc, be) = m.face_bounds(f);
                    let moved_center = [bc[0] + toff[0], bc[1] + toff[1], bc[2] + toff[2]];
                    if WORLD_BOUNDS_CULL && !face_bounds_visible(moved_center, be, &rot, base_t) {
                        continue;
                    }
                    emit_submodel_face(&mut packets, &m, f, first, cnt, nv, submodel_token, &mut np);
                }
            }
            telemetry::stage_end(telemetry::stage::MODEL_DRAW);

            // Studio actors placed by map entities plus any runtime fallback
            // enemies. Models are baked to posed frames by the host cooker; the
            // PS1 path only chooses the current actor frame.
            telemetry::stage_begin(telemetry::stage::TEXTURED_MODEL_JOINTS);
            let actor_count = PROP_COUNT.min(MAX_PROPS);
            for pi in 0..actor_count {
                if PROP_ACTIVE[pi] == 0 {
                    continue;
                }
                let ty = PROP_KIND[pi];
                let org = PROP_POS[pi];
                let yaw = PROP_YAW[pi];
                let cooked_leaf = PROP_LEAF[pi];
                let slot = TYPE_TO_SLOT[(ty as usize).min(N_MODEL_TYPES - 1)];
                if slot == MODEL_SLOT_NONE {
                    continue; // type not resident this map (overflow or missing)
                }
                let lm = LOADED_MODELS[slot as usize];
                if !lm.valid {
                    continue;
                }
                let md_owned = loaded_model(slot as usize);
                let md = &md_owned;
                let slots = &POOL_TEX[lm.tex_start..lm.tex_start + lm.n_tex];
                let faces = core::ptr::addr_of!(POOL_FACES)
                    .cast::<ModelRenderFace>()
                    .add(lm.face_start);
                let face_count = lm.n_faces;
                let radius = model_def(ty).radius;
                model_bounds_tests = model_bounds_tests.saturating_add(1);
                // Far cull (tighter than world FAR_VIEW): skip distant detailed
                // models before the costlier PVS/frustum/occlusion tests + draw.
                if dot12(rot.m[2], org) + base_t[2] - radius > MODEL_FAR {
                    model_bounds_culled = model_bounds_culled.saturating_add(1);
                    continue;
                }
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
                if !sphere_visible(org, radius, &rot, base_t) {
                    model_bounds_culled = model_bounds_culled.saturating_add(1);
                    continue;
                }
                if !prop_occlusion_visible(&m, eye, ty, org) {
                    model_bounds_culled = model_bounds_culled.saturating_add(1);
                    continue;
                }
                model_draws = model_draws.saturating_add(1);
                let sf = if ty == PROP_TYPE_ITEM_SUIT || ty == PROP_TYPE_ITEM_BATTERY {
                    0
                } else {
                    prop_anim_frame(md, ty, PROP_STATE[pi], PROP_HIT_FLASH[pi], sim_frame_no, pi)
                };
                let shade = if ty == PROP_TYPE_ITEM_SUIT || ty == PROP_TYPE_ITEM_BATTERY {
                    128
                } else if pi < MAX_PROPS && PROP_HIT_FLASH[pi] > 0 {
                    MODEL_HIT_SHADE
                } else {
                    MODEL_SHADE
                };
                model_projected_vertices = model_projected_vertices.saturating_add(draw_model(
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
                ));
            }
            if DBG_MODEL_SHOWCASE {
                let mut k = 0i32;
                for slot in 0..MAX_LOADED_MODELS {
                    let lm = LOADED_MODELS[slot];
                    if !lm.valid || lm.type_id < 5 {
                        continue; // enemies only (skip NPCs/items)
                    }
                    let md = loaded_model(slot);
                    let slots = &POOL_TEX[lm.tex_start..lm.tex_start + lm.n_tex];
                    let faces = core::ptr::addr_of!(POOL_FACES)
                        .cast::<ModelRenderFace>()
                        .add(lm.face_start);
                    let fwd = rot.m[2];
                    let right = rot.m[0];
                    let side = k * 130 - 130;
                    let pos = [
                        eye[0] + ((fwd[0] as i32 * 240 + right[0] as i32 * side) >> 12),
                        eye[1] + ((fwd[1] as i32 * 240 + right[1] as i32 * side) >> 12) - 40,
                        eye[2] + ((fwd[2] as i32 * 240 + right[2] as i32 * side) >> 12),
                    ];
                    draw_model(
                        &mut packets,
                        &md,
                        slots,
                        faces,
                        lm.n_faces,
                        pos,
                        0,
                        (frame_no as usize / 8) % md.n_frames.max(1),
                        MODEL_SHADE,
                        eye,
                        &rot,
                        &mut np,
                    );
                    k += 1;
                }
            }
            telemetry::stage_end(telemetry::stage::TEXTURED_MODEL_JOINTS);
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
                (np.saturating_sub(model_prims0) + nq.saturating_sub(model_quads0) * 2) as u32,
            );
            telemetry::counter(
                telemetry::counter::MODEL_INSTANCE_PROJECTED_VERTICES,
                model_projected_vertices,
            );

            let world_prims = np;
            let world_quads = nq;
            if SHOW_VIEWMODEL {
                telemetry::stage_begin(telemetry::stage::EQUIPMENT);
                let (vm_model, vm_slot, vm_n) = viewmodel_for(weapon.current);
                draw_viewmodel(
                    &mut packets,
                    &vm_model,
                    &VM_SLOTS[vm_slot..vm_slot + vm_n],
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
            if death_ticks > 0 {
                // Death screen: the view reddens over the death window, then respawn.
                let r = (((DEATH_TICKS - death_ticks) as u32) * 5).min(190) as u8;
                DEATH_OVERLAY = RectFlat::new(0, 0, 320, 240, r, r / 6, r / 6);
                HUD_OT.add(0, &mut DEATH_OVERLAY, RectFlat::WORDS);
            } else {
                let _ = hud::draw(
                    hud_mat,
                    suit_equipped,
                    health,
                    armor,
                    weapon.clip_display(),
                    weapon.reserve_display(),
                    weapon.ammo_mode(),
                    pickup_kind,
                    pickup_ticks,
                    &mut HUD_OT,
                    &mut HUD_PRIMS,
                );
            }
            let _ = render_impact_marks(&mut FX_OT, &mut IMPACT_MARK_RECTS, &rot, base_t);
            let _ =
                IMPACT_PARTICLES.render_into_ot(&mut FX_OT, &mut IMPACT_PARTICLE_RECTS, 0, (0, 0));
            render_projectiles(&mut FX_OT, &rot, base_t);

            telemetry::stage_begin(telemetry::stage::FRAME_CLEAR);
            fb.clear(0, 0, 0);
            draw_sky(&m, yaw, pitch);
            telemetry::stage_end(telemetry::stage::FRAME_CLEAR);

            // DEBUG: if the draw picked nothing under the crosshair (a quad or a
            // genuinely missing/culled triangle), sweep the PVS faces so it still
            // highlights + dumps. Reload the world GTE transform first -- the
            // entity passes left their own loaded.
            if DEBUG_XHAIR && XHAIR.valid == 0 && have_pvs {
                scene::load_rotation(&rot);
                scene::load_translation(Vec3I32::new(base_t[0], base_t[1], base_t[2]));
                xhair_pick_pvs(&m, nv, frame_no);
            }

            // DEBUG: fill the crosshair tri bright magenta (a POLYGON -- the HW
            // renderer skips PS1 line prims, so an outline is invisible in the GUI)
            // at the front OT slot so it draws on top. Also resolve its world verts /
            // leaf / PVS state for the L1 dump.
            if DEBUG_XHAIR && XHAIR.valid != 0 {
                let mut cen = [0i32; 3];
                let mut k = 0;
                while k < 3 {
                    let vv = m.vert(XHAIR.idx[k] as usize);
                    XHAIR.v[k] = [vv.x as i32, vv.y as i32, vv.z as i32];
                    cen[0] += XHAIR.v[k][0];
                    cen[1] += XHAIR.v[k][1];
                    cen[2] += XHAIR.v[k][2];
                    k += 1;
                }
                XHAIR.leaf = camera_leaf(&m, [cen[0] / 3, cen[1] / 3, cen[2] / 3]);
                XHAIR.pvs_visible = pvs_leaf_visible(&m, XHAIR.leaf.max(0) as usize) as u32;
                let v = XHAIR.sv;
                let fill = psx_gpu::prim::TriFlat::new([v[0], v[1], v[2]], 255, 0, 255);
                if let Some(pk) = packets.push(fill) {
                    OT.add(1, pk, psx_gpu::prim::TriFlat::WORDS);
                }
            }

            telemetry::stage_begin(telemetry::stage::WORLD_FLUSH);
            OT.submit();
            FX_OT.submit();
            telemetry::stage_end(telemetry::stage::WORLD_FLUSH);
            telemetry::stage_begin(telemetry::stage::OT_SUBMIT);
            WEAPON_OT.submit();
            HUD_OT.submit();
            telemetry::stage_end(telemetry::stage::OT_SUBMIT);

            // DEBUG: dump the crosshair tri + camera state to PSoXide's Play debug
            // terminal. Auto-fires whenever the aimed triangle (or valid state)
            // changes -- so you just aim, no controller button needed -- plus a
            // manual L1 re-dump. Lets the same tri be compared between a frame
            // where it shows and one where it is missing.
            if DEBUG_XHAIR {
                let dump_now = poll_port1().buttons.is_held(button::L1);
                let key = if XHAIR.valid != 0 { XHAIR.tt } else { u32::MAX - 1 };
                // Fire on L1, on aimed-triangle change, AND every ~2s so steady
                // aim still produces a visible line.
                let periodic = frame_no % 16 == 0;
                if (dump_now && !XHAIR_DUMP_PREV) || key != XHAIR_DUMP_LAST || periodic {
                    xhair_dump(eye, yaw, pitch, cam_leaf, (np + nq) as i32, PVS_FACE_COUNT as i32);
                    XHAIR_DUMP_LAST = key;
                }
                XHAIR_DUMP_PREV = dump_now;
            }
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

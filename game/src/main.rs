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
mod telemetry;
mod vram;

mod room_budget {
    include!(concat!(env!("OUT_DIR"), "/room_budget.rs"));
}

use psx_fx::{LcgRng, ParticlePool};
use psx_gpu::material::TexturedGouraudPacketMaterial;
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
static SCI_BYTES: &[u8] = include_bytes!("../../data/models/scientist.hlmdl");
static BARNEY_BYTES: &[u8] = include_bytes!("../../data/models/barney.hlmdl");
static HEADCRAB_BYTES: &[u8] = include_bytes!("../../data/models/headcrab.hlmdl");
static SUIT_ITEM_BYTES: &[u8] = include_bytes!("../../data/models/w_suit.hlmdl");
static BATTERY_ITEM_BYTES: &[u8] = include_bytes!("../../data/models/w_battery.hlmdl");

// World ordering table. sz (view depth) tops out near FAR_VIEW; otz = sz>>OT_SHIFT
// must stay < OT_LEN. OT_SHIFT=4 (16-unit buckets) spreads geometry across the
// whole table -- at >>6 only ~10% of the OT was ever used (measured sz~6.9k on
// c0a0), so far geometry that differed by <64 units tie-broke arbitrarily. With
// OT_SHIFT=4, worst-case otz ~16000>>4=1000; OT_LEN=2048 leaves clamp headroom.
const OT_LEN: usize = 2048;
const OT_SHIFT: u32 = 4;
const WEAPON_OT_LEN: usize = 64;
const HUD_OT_LEN: usize = 1;
const FX_OT_LEN: usize = 1;
const MAX_VERTS: usize = 8192;
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
// Distance cull. MUST exceed the largest map's diagonal (max cooked ~13.5k) or
// distant-but-visible geometry clips -- the old 6000 chopped the back half of
// every level off. PVS already bounds visibility, so this only caps pathological
// sightlines; it does not usefully fire on the campaign maps.
const FAR_VIEW: i32 = 16000;
const MODEL_CULL: bool = true; // backface-cull studio models
const MODEL_OCCLUSION_CULL: bool = true; // skip actors fully hidden by static BSP
const MODEL_SHADE: u8 = 110; // flat model tint (dimmer than 128 to match the lit world)
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
const GLOCK_MAX_CLIP: u16 = 17;
const GLOCK_START_RESERVE: u16 = 35;
const GLOCK_DAMAGE: u8 = 8;
const GLOCK_RANGE: i32 = 8192;
const GLOCK_AIM_PIX_X: i32 = 22;
const GLOCK_AIM_PIX_Y: i32 = 34;
const GLOCK_PRIMARY_COOLDOWN_TICKS: u8 = 6; // HL primary cycle is 0.3s; game tick is 20 Hz
const GLOCK_EMPTY_COOLDOWN_TICKS: u8 = 4; // HL empty click cadence is 0.2s
const GLOCK_RELOAD_TICKS: u8 = 30; // HL Glock reload is 1.5s
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
const PROP_STATE_IDLE: u8 = 0;
const PROP_STATE_MOVE: u8 = 1;
const PROP_STATE_ATTACK: u8 = 2;
const PROP_STATE_DEAD: u8 = 3;
const PROP_CLIP_IDLE: usize = 0;
const PROP_CLIP_MOVE: usize = 1;
const PROP_CLIP_ATTACK: usize = 2;
const PROP_CLIP_PAIN: usize = 3;
const PROP_CLIP_DEAD: usize = 4;
const PLAYER_START_HEALTH: u16 = 100;
const PLAYER_START_ARMOR: u16 = 0;
const HEV_MAX_ARMOR: u16 = 100;
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
static mut TEX_SLOTS: [TexSlot; MAX_TEX_SLOTS] = [EMPTY_SLOT; MAX_TEX_SLOTS];
static mut SCI_SLOTS: [TexSlot; 24] = [EMPTY_SLOT; 24];
static mut BARNEY_SLOTS: [TexSlot; 24] = [EMPTY_SLOT; 24];
static mut HEADCRAB_SLOTS: [TexSlot; 8] = [EMPTY_SLOT; 8];
static mut SUIT_ITEM_SLOTS: [TexSlot; 8] = [EMPTY_SLOT; 8];
static mut BATTERY_ITEM_SLOTS: [TexSlot; 8] = [EMPTY_SLOT; 8];
static mut WEAPON_SLOTS: [TexSlot; 12] = [EMPTY_SLOT; 12];
static mut SCI_FACES: [ModelRenderFace; SCI_FACE_CAP] = [ModelRenderFace::ZERO; SCI_FACE_CAP];
static mut BARNEY_FACES: [ModelRenderFace; BARNEY_FACE_CAP] =
    [ModelRenderFace::ZERO; BARNEY_FACE_CAP];
static mut HEADCRAB_FACES: [ModelRenderFace; HEADCRAB_FACE_CAP] =
    [ModelRenderFace::ZERO; HEADCRAB_FACE_CAP];
static mut SUIT_ITEM_FACES: [ModelRenderFace; SUIT_ITEM_FACE_CAP] =
    [ModelRenderFace::ZERO; SUIT_ITEM_FACE_CAP];
static mut BATTERY_ITEM_FACES: [ModelRenderFace; BATTERY_ITEM_FACE_CAP] =
    [ModelRenderFace::ZERO; BATTERY_ITEM_FACE_CAP];
static mut SCI_FACE_COUNT: usize = 0;
static mut BARNEY_FACE_COUNT: usize = 0;
static mut HEADCRAB_FACE_COUNT: usize = 0;
static mut SUIT_ITEM_FACE_COUNT: usize = 0;
static mut BATTERY_ITEM_FACE_COUNT: usize = 0;

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
const LOADING_SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

fn loading_label_for_room(room_id: u16) -> &'static str {
    let idx = room_id as usize;
    if idx < menu::MAPS.len() {
        menu::MAPS[idx]
    } else {
        "room"
    }
}

fn draw_loading_screen(fb: &mut FrameBuffer, label: &str, frame: u8) {
    fb.clear(0, 0, 0);
    gpu::draw_quad_flat([(0, 0), (320, 0), (0, 240), (320, 240)], 6, 6, 6);
    gpu::draw_quad_flat([(42, 64), (278, 64), (42, 176), (278, 176)], 14, 13, 11);
    gpu::draw_quad_flat([(46, 68), (274, 68), (46, 172), (274, 172)], 24, 21, 16);
    hltext::draw_centered_scaled(84, "HALF-LIFE", hltext::SMALL_Q8, PAUSE_WHITE);

    let loading = "Loading";
    let y = 118;
    hltext::draw_centered_scaled(y, loading, hltext::SMALL_Q8, PAUSE_AMBER);
    let x = 160 + hltext::text_width_scaled(loading, hltext::SMALL_Q8) / 2 + 8;
    hltext::draw_text_scaled(
        x,
        y,
        LOADING_SPINNER[(frame as usize) & 3],
        hltext::SMALL_Q8,
        PAUSE_WHITE,
    );

    hltext::draw_centered_scaled(144, label, hltext::SMALL_Q8, PAUSE_DIM);
    gpu::draw_sync();
    gpu::vsync();
    fb.swap();
}

fn draw_next_loading_screen(fb: &mut FrameBuffer, label: &str, frame: &mut u8) {
    draw_loading_screen(fb, label, *frame);
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
}
const EMPTY_PVS_FACE_REC: PvsFaceRec = PvsFaceRec {
    first: 0,
    count: 0,
    center: [0; 3],
    radius: 0,
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
static mut LOGIC_PLAYER_POS: [i32; 3] = [0; 3];
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
const DEBUG_XHAIR: bool = true;
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

#[inline(always)]
unsafe fn streamed_model_bytes(len: usize) -> &'static [u8] {
    let ptr = canonical_ram_const(core::ptr::addr_of!(MODEL_BUF).cast::<u8>());
    unsafe { core::slice::from_raw_parts(ptr, len) }
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
                        ENT_PHASE[ei] = (ENT_PHASE[ei] + step).min(4096);
                        if ENT_PHASE[ei] >= 4096 {
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
                        ENT_PHASE[ei] = (ENT_PHASE[ei] - step).max(0);
                        if ENT_PHASE[ei] <= 0 {
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
            if rec.kind == map::LOGIC_FUNC_BUTTON || rec.kind == map::LOGIC_FUNC_DOOR {
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
        logic_use_entity(m, nlogic, nents, best, map::USE_TOGGLE, now, 0);
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
        ei += 1;
    }
    let mut li = 0usize;
    while li < MAX_LOGIC {
        LOGIC_STATE[li] = LOGIC_STATE_BOTTOM;
        LOGIC_NEXT[li] = 0;
        LOGIC_TARGET[li] = 0;
        LOGIC_COUNTER[li] = 0;
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
        LOGIC_COUNTER[li] = if rec.kind == map::LOGIC_TRIGGER_COUNTER {
            (rec.arg0 as i16).max(1)
        } else {
            0
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
            map::LOGIC_FUNC_TRACKTRAIN => {
                if rec.arg1 != 0 && rec.arg1 == TRACKTRAIN_SUBMODEL && rec.arg0 > 0 {
                    TRACKTRAIN_CMD_ACTIVE = 1;
                    TRACKTRAIN_CMD_USE_TYPE = map::USE_ON;
                    TRACKTRAIN_CMD_SPEED = rec.arg0;
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
            let np = prop_grounded_pos(
                m,
                movers,
                [pos[0] + sx * step / len, pos[1], pos[2] + sz * step / len],
            );
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
    }
}

fn damage_player(health: &mut u16, armor: &mut u16, dmg: u16) {
    if dmg == 0 {
        return;
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

    let target = find_headcrab_target(m, movers, pi, player_pos, nprops);
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

    let target = find_barney_target(m, movers, pi, nprops);
    if target != PROP_TARGET_NONE {
        if let Some(aim) = target_aim_point(target, player_pos, nprops) {
            prop_face_point(pi, aim);
            PROP_AI_TARGET[pi] = target;
            if PROP_ATTACK_COOLDOWN[pi] == 0 {
                PROP_STATE[pi] = PROP_STATE_ATTACK;
                PROP_AI_TIMER[pi] = BARNEY_ATTACK_TICKS;
                PROP_ATTACK_COOLDOWN[pi] = BARNEY_ATTACK_COOLDOWN;
                damage_target(target, BARNEY_DAMAGE, health, armor);
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
        let kind = ty as u8;
        let org = prop_grounded_pos(m, &[], org);
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
        PROP_HEALTH[pi] = prop_start_health(kind);
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

        if ty == PROP_TYPE_HEADCRAB {
            tick_headcrab(m, movers, pi, player_pos, health, armor, nprops);
        } else if ty == PROP_TYPE_BARNEY {
            tick_barney(m, movers, pi, player_pos, health, armor, nprops);
        } else if ty == PROP_TYPE_SCIENTIST {
            tick_scientist(m, movers, pi, player_pos, nprops);
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
                    telemetry::debug_log("hl-psx: HEV suit equipped");
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
                    telemetry::debug_log("hl-psx: HEV battery picked up");
                }
            }
            _ => {}
        }
        pi += 1;
    }
}

#[derive(Clone, Copy)]
struct WeaponState {
    clip: u16,
    reserve: u16,
    cooldown: u8,
    reload_ticks: u8,
    dry_ticks: u8,
}

impl WeaponState {
    fn new(clip: u16, reserve: u16) -> Self {
        Self {
            clip: clip.min(GLOCK_MAX_CLIP),
            reserve,
            cooldown: 0,
            reload_ticks: 0,
            dry_ticks: 0,
        }
    }

    fn tick(&mut self) {
        if self.cooldown > 0 {
            self.cooldown -= 1;
        }
        if self.dry_ticks > 0 {
            self.dry_ticks -= 1;
        }
        if self.reload_ticks > 0 {
            self.reload_ticks -= 1;
            if self.reload_ticks == 0 {
                self.finish_reload();
            }
        }
    }

    fn start_reload(&mut self) -> bool {
        if self.reload_ticks != 0 || self.clip >= GLOCK_MAX_CLIP || self.reserve == 0 {
            return false;
        }
        self.reload_ticks = GLOCK_RELOAD_TICKS;
        self.cooldown = self.cooldown.max(GLOCK_RELOAD_TICKS);
        true
    }

    fn finish_reload(&mut self) {
        let need = GLOCK_MAX_CLIP.saturating_sub(self.clip);
        let take = need.min(self.reserve);
        self.clip = self.clip.saturating_add(take).min(GLOCK_MAX_CLIP);
        self.reserve = self.reserve.saturating_sub(take);
    }

    fn try_fire(&mut self) -> bool {
        if self.cooldown != 0 || self.reload_ticks != 0 {
            return false;
        }
        if self.clip == 0 {
            self.cooldown = GLOCK_EMPTY_COOLDOWN_TICKS;
            self.dry_ticks = GLOCK_EMPTY_COOLDOWN_TICKS;
            let _ = self.start_reload();
            return false;
        }
        self.clip -= 1;
        self.cooldown = GLOCK_PRIMARY_COOLDOWN_TICKS;
        true
    }
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

unsafe fn fire_glock(
    m: &Map,
    movers: &[phys::Mover],
    eye: [i32; 3],
    rot: &Mat3I16,
    base_t: [i32; 3],
) -> Option<usize> {
    let end = [
        eye[0] + (((rot.m[2][0] as i32) * GLOCK_RANGE) >> 12),
        eye[1] + (((rot.m[2][1] as i32) * GLOCK_RANGE) >> 12),
        eye[2] + (((rot.m[2][2] as i32) * GLOCK_RANGE) >> 12),
    ];
    let world_hit = phys::trace_line(m, movers, eye, end);
    let world_limit_z = world_hit
        .map(|hit| (GLOCK_RANGE * hit.frac) >> 12)
        .unwrap_or(GLOCK_RANGE + 1);
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
        if !(render::NEAR_Z..=GLOCK_RANGE).contains(&vz) || vz > world_limit_z {
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
        if let Some(hit) = world_hit {
            let decal_pos = [
                hit.pos[0] + ((hit.normal[0] * 2) >> 12),
                hit.pos[1] + ((hit.normal[1] * 2) >> 12),
                hit.pos[2] + ((hit.normal[2] * 2) >> 12),
            ];
            spawn_impact_fx(decal_pos, IMPACT_KIND_WORLD, rot, base_t);
        }
        None
    } else {
        let ty = PROP_KIND[best];
        damage_prop(best, GLOCK_DAMAGE);
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

#[inline]
fn face_bounds_visible(center: [i32; 3], ext: [i32; 3], rot: &Mat3I16, base_t: [i32; 3]) -> bool {
    // Conservative sphere around the cooked face AABB. It is looser than the
    // full AABB test but much cheaper across large PVS face lists.
    sphere_visible(center, ext[0] + ext[1] + ext[2], rot, base_t)
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
                let (bc, be) = m.face_bounds(face);
                let radius = be[0]
                    .saturating_add(be[1])
                    .saturating_add(be[2])
                    .min(u16::MAX as i32) as u16;
                PVS_FACE_REC[entry] = PvsFaceRec {
                    first: first as u16,
                    count: cnt as u16,
                    center: [bc[0] as i16, bc[1] as i16, bc[2] as i16],
                    radius,
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
        if ENT_ACTIVE[ei] != 0 && entity_touches_pvs(m, &e) && PVS_ENT_COUNT < MAX_ENTS {
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
#[inline]
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
        push_tri_uv_words(
            packets,
            np,
            [(pa.sx, pa.sy), (pb.sx, pb.sy), (pc.sx, pc.sy)],
            m.tri_uv_words(tt),
            m.tri_rgb(tt),
            slot.packet,
            otz,
        );
        return true;
    }
    // All-behind-camera counts as handled (nothing to draw); otherwise straddler.
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
        // Deflate sz by `s` back to 1x world depth so models sort correctly in
        // the shared OT against the (un-inflated) world geometry.
        let avgz = model_unscale_depth(
            ((pa.sz as u32) + (pb.sz as u32) + (pc.sz as u32)) / 3,
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
            clamp_otz((avgz >> 6) as usize),
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
    let sci = Model::load(SCI_BYTES);
    let barney = Model::load(BARNEY_BYTES);
    let headcrab = Model::load(HEADCRAB_BYTES);
    let suit_item = Model::load(SUIT_ITEM_BYTES);
    let battery_item = Model::load(BATTERY_ITEM_BYTES);
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
        SUIT_ITEM_FACE_COUNT = suit_item.fill_render_faces_raw(
            core::ptr::addr_of_mut!(SUIT_ITEM_FACES).cast::<ModelRenderFace>(),
            SUIT_ITEM_FACE_CAP,
        );
        BATTERY_ITEM_FACE_COUNT = battery_item.fill_render_faces_raw(
            core::ptr::addr_of_mut!(BATTERY_ITEM_FACES).cast::<ModelRenderFace>(),
            BATTERY_ITEM_FACE_CAP,
        );
    }

    // Boot flow: pick a map in the menu, stream + play it, return on Select.
    // (Analog is enabled inside play(); the menu runs on the digital pad.)
    loop {
        let sel = menu::run(&mut fb);
        let mut launch = menu_launch(sel);
        loop {
            match play(
                &mut fb,
                &sci,
                &barney,
                &headcrab,
                &suit_item,
                &battery_item,
                launch,
            ) {
                PlayExit::BackToMenu => break,
                PlayExit::ChangeLevel(next) => launch = next,
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
    slots: *mut TexSlot,
    slot_len: usize,
    stream_chunks: &mut u32,
    stream_bytes: &mut u32,
    stream_sectors: &mut u32,
) -> Option<(usize, usize)> {
    telemetry::stage_begin(telemetry::stage::CD_WORLD_PACK_STREAM);
    let len = cdstream::load_chunk(chunk_id, unsafe { &mut MODEL_BUF }).unwrap_or(0);
    telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
    if len == 0 {
        return None;
    }
    account_streamed_chunk(len, stream_chunks, stream_bytes, stream_sectors);
    let bytes = unsafe { streamed_model_bytes(len) };
    telemetry::stage_begin(telemetry::stage::VRAM_UPLOAD);
    let uploaded = unsafe { vram::upload_tex_chunk_append_raw(bytes, slots, slot_len) };
    telemetry::stage_end(telemetry::stage::VRAM_UPLOAD);
    uploaded
}

/// Stream a room from WORLD.PAK, upload its textures, and run the renderer +
/// physics loop until Select returns to menu or a trigger_changelevel requests
/// the next room.
fn play(
    fb: &mut FrameBuffer,
    sci: &Model,
    barney: &Model,
    headcrab: &Model,
    suit_item: &Model,
    battery_item: &Model,
    launch: RoomLaunch,
) -> PlayExit {
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
    draw_next_loading_screen(fb, loading_label, &mut loading_frame);

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

    draw_next_loading_screen(fb, loading_label, &mut loading_frame);
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
    let mut model_tex_failed = 0usize;
    let mut model_texs = 0usize;
    let mut upload_model_tex = |chunk_id: u32, slots: *mut TexSlot, slot_len: usize| -> bool {
        match stream_model_texture_chunk(
            chunk_id,
            slots,
            slot_len,
            &mut stream_chunks,
            &mut stream_bytes,
            &mut stream_sectors,
        ) {
            Some((n, failed)) => {
                model_texs = model_texs.saturating_add(n);
                model_tex_failed = model_tex_failed.saturating_add(failed);
                true
            }
            None => false,
        }
    };
    draw_next_loading_screen(fb, loading_label, &mut loading_frame);
    let mut model_textures_ok = upload_model_tex(
        MODEL_CHUNK_SCIENTIST_TEX,
        core::ptr::addr_of_mut!(SCI_SLOTS).cast::<TexSlot>(),
        24,
    );
    if model_textures_ok {
        draw_next_loading_screen(fb, loading_label, &mut loading_frame);
        model_textures_ok = upload_model_tex(
            MODEL_CHUNK_BARNEY_TEX,
            core::ptr::addr_of_mut!(BARNEY_SLOTS).cast::<TexSlot>(),
            24,
        );
    }
    if model_textures_ok {
        draw_next_loading_screen(fb, loading_label, &mut loading_frame);
        model_textures_ok = upload_model_tex(
            MODEL_CHUNK_HEADCRAB_TEX,
            core::ptr::addr_of_mut!(HEADCRAB_SLOTS).cast::<TexSlot>(),
            8,
        );
    }
    if model_textures_ok {
        draw_next_loading_screen(fb, loading_label, &mut loading_frame);
        model_textures_ok = upload_model_tex(
            MODEL_CHUNK_SUIT_ITEM_TEX,
            core::ptr::addr_of_mut!(SUIT_ITEM_SLOTS).cast::<TexSlot>(),
            8,
        );
    }
    if model_textures_ok {
        draw_next_loading_screen(fb, loading_label, &mut loading_frame);
        model_textures_ok = upload_model_tex(
            MODEL_CHUNK_BATTERY_ITEM_TEX,
            core::ptr::addr_of_mut!(BATTERY_ITEM_SLOTS).cast::<TexSlot>(),
            8,
        );
    }
    if model_textures_ok {
        draw_next_loading_screen(fb, loading_label, &mut loading_frame);
        model_textures_ok = upload_model_tex(
            MODEL_CHUNK_V_9MMHANDGUN_TEX,
            core::ptr::addr_of_mut!(WEAPON_SLOTS).cast::<TexSlot>(),
            12,
        );
    }
    if !model_textures_ok {
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_CHUNKS, stream_chunks);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_BYTES, stream_bytes);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_SECTORS, stream_sectors);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_STATUS, 0);
        telemetry::task_end(telemetry::task::FIXED_UPDATE);
        tty::println("hl-psx: WORLD.PAK model texture stream failed");
        telemetry::debug_log("hl-psx: WORLD.PAK model texture stream failed");
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

    draw_next_loading_screen(fb, loading_label, &mut loading_frame);
    telemetry::stage_begin(telemetry::stage::CD_WORLD_PACK_STREAM);
    let weapon_len =
        cdstream::load_chunk(MODEL_CHUNK_V_9MMHANDGUN, unsafe { &mut MODEL_BUF }).unwrap_or(0);
    telemetry::stage_end(telemetry::stage::CD_WORLD_PACK_STREAM);
    if weapon_len > 0 {
        account_streamed_chunk(
            weapon_len,
            &mut stream_chunks,
            &mut stream_bytes,
            &mut stream_sectors,
        );
    } else {
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_CHUNKS, stream_chunks);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_BYTES, stream_bytes);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_SECTORS, stream_sectors);
        telemetry::counter(telemetry::counter::CD_WORLD_PACK_STATUS, 0);
        telemetry::task_end(telemetry::task::FIXED_UPDATE);
        tty::println("hl-psx: WORLD.PAK weapon stream failed");
        telemetry::debug_log("hl-psx: WORLD.PAK weapon stream failed");
        return PlayExit::BackToMenu;
    }
    telemetry::debug_log("hl-psx: WORLD.PAK weapon chunk loaded");

    let weapon_bytes = unsafe { streamed_model_bytes(weapon_len) };
    let wpn = Model::load(weapon_bytes);
    unsafe {
        WEAPON_CACHE_FRAME = usize::MAX;
        WEAPON_CACHE_RECOIL = i32::MIN;
        WEAPON_CACHE_VERTS = 0;
        WEAPON_CACHE_SCALE = 0;
        WEAPON_TRI_COUNT = 0;
    }
    draw_next_loading_screen(fb, loading_label, &mut loading_frame);
    let hud_mat = hud::upload(); // real HUD sprite sheet -> free gameplay tpage

    draw_next_loading_screen(fb, loading_label, &mut loading_frame);
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
    unsafe {
        PVS_CAM_LEAF = -1;
        PVS_LEAF_COUNT = 0;
        PVS_ENT_COUNT = 0;
        init_prop_state(&m);
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
    let mut weapon = WeaponState::new(launch.clip_ammo, launch.reserve_ammo);
    let mut health: u16 = launch.health;
    let mut armor: u16 = launch.armor.min(HEV_MAX_ARMOR);
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

    // Loading cards are drawn directly into the double buffers. Clear both
    // pages once before the first gameplay frame so sparse world coverage
    // cannot leave a stale loading panel behind.
    fb.clear(0, 0, 0);
    gpu::draw_sync();
    fb.swap();
    fb.clear(0, 0, 0);
    gpu::draw_sync();
    fb.swap();

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
            let want_fire = pad.buttons.is_held(button::R2);
            let want_reload = pad.buttons.is_held(button::CIRCLE);
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
            yaw = (((yaw as i32) + (turn * YAW_RATE) / 128) & 0xFFF) as u16;
            pitch = (pitch + ((look * PITCH_RATE) / 128) as i16).clamp(-PITCH_MAX, PITCH_MAX);
            unsafe {
                LOGIC_PLAYER_POS = player.pos;
                LOGIC_PLAYER_YAW = yaw;
                LOGIC_PLAYER_PITCH = pitch;
                LOGIC_PLAYER_HEALTH = health;
                LOGIC_PLAYER_SUIT = if suit_equipped { 1 } else { 0 };
                LOGIC_PLAYER_ARMOR = armor;
                LOGIC_PLAYER_CLIP_AMMO = weapon.clip;
                LOGIC_PLAYER_RESERVE_AMMO = weapon.reserve;
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

            // Full player physics always runs; moving trains carry the player by
            // delta before the update, then block them through their shifted hull.
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
            let eye = [player.pos[0], player.pos[1] + VIEW_HEIGHT, player.pos[2]];
            unsafe {
                collect_pickups(
                    player.pos,
                    &mut suit_equipped,
                    &mut armor,
                    &mut pickup_kind,
                    &mut pickup_ticks,
                );
                LOGIC_PLAYER_POS = player.pos;
                LOGIC_PLAYER_YAW = yaw;
                LOGIC_PLAYER_PITCH = pitch;
                LOGIC_PLAYER_HEALTH = health;
                LOGIC_PLAYER_SUIT = if suit_equipped { 1 } else { 0 };
                LOGIC_PLAYER_ARMOR = armor;
                LOGIC_PLAYER_CLIP_AMMO = weapon.clip;
                LOGIC_PLAYER_RESERVE_AMMO = weapon.reserve;
                if want_use {
                    logic_try_use(
                        &m,
                        nlogic,
                        nents,
                        eye,
                        yaw,
                        pitch,
                        movers,
                        sim_frame_no as u16,
                    );
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
                    let _ = fire_glock(&m, movers, eye, &fire_rot, fire_base_t);
                }
            }
            unsafe {
                tick_props(&m, movers, player.pos, &mut health, &mut armor);
                LOGIC_PLAYER_HEALTH = health;
                LOGIC_PLAYER_ARMOR = armor;
                LOGIC_PLAYER_CLIP_AMMO = weapon.clip;
                LOGIC_PLAYER_RESERVE_AMMO = weapon.reserve;
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
                const DEPTH_BAND_SHIFT: u32 = 11; // 2048 world units per band
                let nbands = if PVS_TRI_REF_COUNT > MAX_RENDER_PACKETS {
                    N_DEPTH_BANDS
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
                                let c = [
                                    rec.center[0] as i32,
                                    rec.center[1] as i32,
                                    rec.center[2] as i32,
                                ];
                                let depth = (dot12(rot.m[2], c) + base_t[2]).max(0);
                                if (depth >> DEPTH_BAND_SHIFT).min(nbands - 1) != band {
                                    continue;
                                }
                                if !WORLD_BOUNDS_CULL || cached_face_visible(rec, &rot, base_t) {
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
                            } else {
                                let face = PVS_FACE_INDEX[e] as usize;
                                let (bc, be) = m.face_bounds(face);
                                let depth = (dot12(rot.m[2], bc) + base_t[2]).max(0);
                                if (depth >> DEPTH_BAND_SHIFT).min(nbands - 1) != band {
                                    continue;
                                }
                                if !WORLD_BOUNDS_CULL || face_bounds_visible(bc, be, &rot, base_t) {
                                    let (first, cnt) = m.face_tris(face);
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
                    for tt in first..first + cnt {
                        emit_submodel_tri(&mut packets, &m, tt, nv, submodel_token, &mut np);
                    }
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
                    for tt in first..first + cnt {
                        emit_submodel_tri(&mut packets, &m, tt, nv, submodel_token, &mut np);
                    }
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
                let (md, slots, faces, face_count, radius) = match ty {
                    PROP_TYPE_SCIENTIST => (
                        sci,
                        &SCI_SLOTS[..],
                        core::ptr::addr_of!(SCI_FACES).cast::<ModelRenderFace>(),
                        SCI_FACE_COUNT,
                        SCIENTIST_RENDER_RADIUS,
                    ),
                    PROP_TYPE_BARNEY => (
                        barney,
                        &BARNEY_SLOTS[..],
                        core::ptr::addr_of!(BARNEY_FACES).cast::<ModelRenderFace>(),
                        BARNEY_FACE_COUNT,
                        BARNEY_RENDER_RADIUS,
                    ),
                    PROP_TYPE_HEADCRAB => (
                        headcrab,
                        &HEADCRAB_SLOTS[..],
                        core::ptr::addr_of!(HEADCRAB_FACES).cast::<ModelRenderFace>(),
                        HEADCRAB_FACE_COUNT,
                        HEADCRAB_RENDER_RADIUS,
                    ),
                    PROP_TYPE_ITEM_SUIT => (
                        suit_item,
                        &SUIT_ITEM_SLOTS[..],
                        core::ptr::addr_of!(SUIT_ITEM_FACES).cast::<ModelRenderFace>(),
                        SUIT_ITEM_FACE_COUNT,
                        ITEM_RENDER_RADIUS,
                    ),
                    PROP_TYPE_ITEM_BATTERY => (
                        battery_item,
                        &BATTERY_ITEM_SLOTS[..],
                        core::ptr::addr_of!(BATTERY_ITEM_FACES).cast::<ModelRenderFace>(),
                        BATTERY_ITEM_FACE_COUNT,
                        ITEM_RENDER_RADIUS,
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
            let _ = hud::draw(
                hud_mat,
                suit_equipped,
                health,
                armor,
                weapon.clip,
                weapon.reserve,
                pickup_kind,
                pickup_ticks,
                &mut HUD_OT,
                &mut HUD_PRIMS,
            );
            let _ = render_impact_marks(&mut FX_OT, &mut IMPACT_MARK_RECTS, &rot, base_t);
            let _ =
                IMPACT_PARTICLES.render_into_ot(&mut FX_OT, &mut IMPACT_PARTICLE_RECTS, 0, (0, 0));

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
                let periodic = frame_no % 64 == 0;
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

//! Deterministic, emulator-only gameplay trace for the GoldSrc differential.
//!
//! Lines intentionally use a tiny `key=value` grammar instead of formatting or
//! allocation. PSoXide prefixes them with its guest frame/cycle metadata; the
//! host normalizer strips that non-semantic prefix.

use crate::telemetry;

const LINE_CAP: usize = 384;

static mut CURRENT_MAP: &'static str = "";
static mut CURRENT_MAP_ID: u16 = 0;
static mut GLOBAL_TICK: u32 = 0;

pub struct TickState {
    pub map_tick: u32,
    pub player_pos: [i32; 3],
    pub player_vel: [i32; 3],
    pub yaw: u16,
    pub pitch: i16,
    pub health: u16,
    pub armor: u16,
    pub weapon: u8,
    pub owned: u16,
    pub clip: u16,
    pub reserve: u16,
    pub on_ground: bool,
    pub ground_mover: i32,
    pub train_pos: [i32; 3],
    pub train_yaw: u16,
    pub train_seg: u16,
    pub train_dist: i32,
    pub train_speed: i32,
    pub train_active: bool,
    pub train_attached: bool,
    pub train_pre_left: i32,
}

pub struct PropState<'a> {
    pub map_tick: u32,
    pub index: u16,
    pub name: &'a str,
    pub kind: u8,
    pub active: bool,
    pub pos: [i32; 3],
    pub yaw: u16,
    pub state: u8,
    pub health: u8,
    pub ai_target: u8,
    pub player_leaf: i32,
    pub cached_pvs_leaf: i32,
    pub pvs_current: bool,
    pub in_player_pvs: bool,
    pub script_mode: u8,
    pub script_goal: [i16; 3],
    pub nav_src: u8,
    pub nav_dst: u8,
    pub nav_next: u8,
    pub move_cooldown: u8,
    pub world_floor: i32,
    pub top_leaf: i32,
    pub top_ent_solid: bool,
}

/// Post-physics state for one authored brush/actor checkpoint. These rows are
/// emitted only by the reference build, once per simulated second, so richer
/// diagnostics do not consume production RAM or runtime.
pub struct EntityState<'a> {
    pub map_tick: u32,
    pub slot: u16,
    pub class: &'a str,
    pub targetname: &'a str,
    pub brush: i32,
    pub pos: [i32; 3],
    pub center: [i32; 3],
    pub vel: [i32; 3],
    pub yaw_q12: i32,
    pub health: u16,
    pub active: bool,
    pub state: u8,
    pub phase: i32,
    pub spawnflags: u16,
}

struct Line {
    bytes: [u8; LINE_CAP],
    len: usize,
}

impl Line {
    fn new(kind: &str) -> Self {
        let mut line = Self {
            bytes: [0; LINE_CAP],
            len: 0,
        };
        line.text("HLPSX|");
        line.text(kind);
        line
    }

    fn byte(&mut self, b: u8) {
        if self.len < self.bytes.len() {
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }

    fn text(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.byte(b);
        }
    }

    fn key(&mut self, key: &str) {
        self.byte(b'|');
        self.text(key);
        self.byte(b'=');
    }

    fn field_str(&mut self, key: &str, value: &str) {
        self.key(key);
        self.text(value);
    }

    fn field_u32(&mut self, key: &str, mut value: u32) {
        self.key(key);
        let mut digits = [0u8; 10];
        let mut n = 0usize;
        loop {
            digits[n] = b'0' + (value % 10) as u8;
            n += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            self.byte(digits[n]);
        }
    }

    fn field_i32(&mut self, key: &str, value: i32) {
        self.key(key);
        let magnitude = if value < 0 {
            self.byte(b'-');
            value.wrapping_neg() as u32
        } else {
            value as u32
        };
        let mut digits = [0u8; 10];
        let mut n = 0usize;
        let mut rest = magnitude;
        loop {
            digits[n] = b'0' + (rest % 10) as u8;
            n += 1;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            self.byte(digits[n]);
        }
    }

    fn field_hex_u16(&mut self, key: &str, value: u16) {
        self.key(key);
        self.text("0x");
        let mut shift = 12u32;
        loop {
            let digit = ((value as u32 >> shift) & 0xf) as u8;
            self.byte(if digit < 10 {
                b'0' + digit
            } else {
                b'a' + digit - 10
            });
            if shift == 0 {
                break;
            }
            shift -= 4;
        }
    }

    fn finish(self) {
        telemetry::debug_log(core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("HLPSX|bad"));
    }
}

fn common(line: &mut Line, map_tick: u32) {
    unsafe {
        line.field_str("map", CURRENT_MAP);
        line.field_u32("map_id", CURRENT_MAP_ID as u32);
        line.field_u32("global_tick", GLOBAL_TICK);
    }
    line.field_u32("tick", map_tick);
}

pub fn begin_map(map: &'static str, map_id: u16) {
    unsafe {
        CURRENT_MAP = map;
        CURRENT_MAP_ID = map_id;
    }
    let mut line = Line::new("event");
    common(&mut line, 0);
    line.field_str("event", "map_start");
    line.finish();
}

pub fn target_fire(map_tick: u16, target: &str, use_type: u8, depth: u8) {
    let mut line = Line::new("event");
    common(&mut line, map_tick as u32);
    line.field_str("event", "target_fire");
    line.field_str("target", target);
    line.field_u32("use", use_type as u32);
    line.field_u32("depth", depth as u32);
    line.finish();
}

pub fn changelevel(map_tick: u16, next_map: &str, landmark: &str) {
    let mut line = Line::new("event");
    common(&mut line, map_tick as u32);
    line.field_str("event", "changelevel");
    line.field_str("next_map", next_map);
    line.field_str("landmark", landmark);
    line.finish();
}

pub fn carry(
    map_tick: u16,
    direction: &str,
    id: u16,
    kind: u8,
    health: u8,
    state: u8,
    pos: [i32; 3],
) {
    let mut line = Line::new("event");
    common(&mut line, map_tick as u32);
    line.field_str("event", "actor_carry");
    line.field_str("direction", direction);
    line.field_u32("id", id as u32);
    line.field_u32("kind_id", kind as u32);
    line.field_u32("health", health as u32);
    line.field_u32("state", state as u32);
    line.field_i32("x", pos[0]);
    line.field_i32("y", pos[1]);
    line.field_i32("z", pos[2]);
    line.finish();
}

/// Raw semantic command consumed for this map-local fixed tick. This is logged
/// before death/mount/crouch/zoom transforms so the same HLINPUT1 rows can be
/// replayed by the original engine and aligned to the following state snapshot.
pub fn input(map_tick: u32, forward: i8, strafe: i8, turn: i8, look: i8, actions: u16) {
    let mut line = Line::new("input");
    common(&mut line, map_tick);
    line.field_u32("input_tick", map_tick);
    line.field_i32("forward", forward as i32);
    line.field_i32("strafe", strafe as i32);
    line.field_i32("turn", turn as i32);
    line.field_i32("look", look as i32);
    line.field_hex_u16("actions", actions);
    line.finish();
}

/// One-shot map-start audit for the zero-BSS prop candidate list. Keeping the
/// count and first entries in reference traces makes a corrupted tail obvious
/// before an AI scan turns it into an opaque guest bounds panic.
pub fn prop_hotlist(map_tick: u32, stage: u8, nprops: usize, count: u8, entries: [u8; 6]) {
    let mut line = Line::new("hotlist");
    common(&mut line, map_tick);
    line.field_u32("stage", stage as u32);
    line.field_u32("nprops", nprops as u32);
    line.field_u32("humans", count as u32);
    let keys = ["h0", "h1", "h2", "h3", "h4", "h5"];
    let mut i = 0usize;
    while i < entries.len() {
        line.field_u32(keys[i], entries[i] as u32);
        i += 1;
    }
    line.finish();
}

pub fn tick(state: TickState) {
    let mut line = Line::new("tick");
    common(&mut line, state.map_tick);
    line.field_i32("px", state.player_pos[0]);
    line.field_i32("py", state.player_pos[1]);
    line.field_i32("pz", state.player_pos[2]);
    line.field_i32("vx", state.player_vel[0]);
    line.field_i32("vy", state.player_vel[1]);
    line.field_i32("vz", state.player_vel[2]);
    line.field_u32("yaw", state.yaw as u32);
    line.field_i32("pitch", state.pitch as i32);
    line.field_u32("health", state.health as u32);
    line.field_u32("armor", state.armor as u32);
    line.field_u32("weapon", state.weapon as u32);
    line.field_u32("owned", state.owned as u32);
    line.field_u32("clip", state.clip as u32);
    line.field_u32("reserve", state.reserve as u32);
    line.field_u32("ground", state.on_ground as u32);
    line.field_i32("ground_mover", state.ground_mover);
    line.field_i32("train_x", state.train_pos[0]);
    line.field_i32("train_y", state.train_pos[1]);
    line.field_i32("train_z", state.train_pos[2]);
    line.field_u32("train_yaw", state.train_yaw as u32);
    line.field_u32("train_seg", state.train_seg as u32);
    line.field_i32("train_dist", state.train_dist);
    line.field_i32("train_speed", state.train_speed);
    line.field_u32("train_active", state.train_active as u32);
    line.field_u32("train_attached", state.train_attached as u32);
    line.field_i32("train_pre_left", state.train_pre_left);
    line.finish();
    unsafe {
        GLOBAL_TICK = GLOBAL_TICK.wrapping_add(1);
    }
}

pub fn prop(state: PropState<'_>) {
    let mut line = Line::new("prop");
    common(&mut line, state.map_tick);
    line.field_u32("index", state.index as u32);
    line.field_str("name", state.name);
    line.field_u32("kind_id", state.kind as u32);
    line.field_u32("active", state.active as u32);
    line.field_i32("x", state.pos[0]);
    line.field_i32("y", state.pos[1]);
    line.field_i32("z", state.pos[2]);
    line.field_u32("yaw", state.yaw as u32);
    line.field_u32("state", state.state as u32);
    line.field_u32("health", state.health as u32);
    line.field_u32("ai_target", state.ai_target as u32);
    line.field_i32("player_leaf", state.player_leaf);
    line.field_i32("cached_pvs_leaf", state.cached_pvs_leaf);
    line.field_u32("pvs_current", state.pvs_current as u32);
    line.field_u32("in_player_pvs", state.in_player_pvs as u32);
    line.field_u32("script", state.script_mode as u32);
    line.field_i32("goal_x", state.script_goal[0] as i32);
    line.field_i32("goal_y", state.script_goal[1] as i32);
    line.field_i32("goal_z", state.script_goal[2] as i32);
    line.field_u32("nav_src", state.nav_src as u32);
    line.field_u32("nav_dst", state.nav_dst as u32);
    line.field_u32("nav_next", state.nav_next as u32);
    line.field_u32("move_cooldown", state.move_cooldown as u32);
    line.field_i32("world_floor", state.world_floor);
    line.field_i32("top_leaf", state.top_leaf);
    line.field_u32("top_ent_solid", state.top_ent_solid as u32);
    line.finish();
}

/// Movement-probe result for a scripted actor. `result` uses bit 7 for a
/// successful direction index; failures OR bit0=line, bit1=monsterclip,
/// bit2=no floor. Reference builds only, so this adds no shipping state/cost.
#[cfg(feature = "deep-reference-trace")]
pub fn nav_step(map_tick: u32, index: u16, dx: i32, dz: i32, result: u8) {
    let mut line = Line::new("nav");
    common(&mut line, map_tick);
    line.field_u32("index", index as u32);
    line.field_i32("dx", dx);
    line.field_i32("dz", dz);
    line.field_u32("result", result as u32);
    line.finish();
}

#[cfg(not(feature = "deep-reference-trace"))]
#[inline(always)]
pub fn nav_step(_map_tick: u32, _index: u16, _dx: i32, _dz: i32, _result: u8) {}

/// Player active-hull diagnostic. `kind`: 0=stationary, 1=jump up,
/// 2=combined jump/forward. Deep traces only; production and ordinary
/// reference builds contain neither the call nor these rows.
#[cfg(feature = "deep-reference-trace")]
pub fn player_hull(map_tick: u32, kind: u8, probe: crate::phys::PlayerHullProbe) {
    let mut line = Line::new("hull");
    common(&mut line, map_tick);
    line.field_u32("kind", kind as u32);
    line.field_i32("frac", probe.frac);
    line.field_u32("startsolid", probe.startsolid as u32);
    line.field_i32("mover", probe.mover);
    line.field_i32("nx", probe.normal[0]);
    line.field_i32("ny", probe.normal[1]);
    line.field_i32("nz", probe.normal[2]);
    line.finish();
}

/// Flat/up/down alternatives used by PM_WalkMove. Deep traces only: this is
/// intentionally verbose enough to identify the first integer hull decision
/// that differs from GoldSrc without perturbing production RAM or timing.
#[cfg(feature = "deep-reference-trace")]
pub fn player_step(map_tick: u32, probe: crate::phys::PlayerStepProbe) {
    let mut line = Line::new("step");
    common(&mut line, map_tick);
    line.field_i32("head", probe.head);
    line.field_i32("dx", probe.move_delta[0]);
    line.field_i32("dy", probe.move_delta[1]);
    line.field_i32("dz", probe.move_delta[2]);
    line.field_i32("direct_frac", probe.direct_frac);
    line.field_i32("direct_ny", probe.direct_normal[1]);
    line.field_i32("flat_x", probe.flat_pos[0]);
    line.field_i32("flat_y", probe.flat_pos[1]);
    line.field_i32("flat_z", probe.flat_pos[2]);
    line.field_i32("up_frac", probe.up_frac);
    line.field_u32("up_solid", probe.up_startsolid as u32);
    line.field_i32("up_y", probe.up_pos[1]);
    line.field_i32("raised_direct_frac", probe.raised_direct_frac);
    line.field_i32("raised_direct_nx", probe.raised_direct_normal[0]);
    line.field_i32("raised_direct_ny", probe.raised_direct_normal[1]);
    line.field_i32("raised_direct_nz", probe.raised_direct_normal[2]);
    line.field_i32("raised_x", probe.raised_pos[0]);
    line.field_i32("raised_y", probe.raised_pos[1]);
    line.field_i32("raised_z", probe.raised_pos[2]);
    line.field_i32("down_frac", probe.down_frac);
    line.field_u32("down_solid", probe.down_startsolid as u32);
    line.field_i32("down_nx", probe.down_normal[0]);
    line.field_i32("down_ny", probe.down_normal[1]);
    line.field_i32("down_nz", probe.down_normal[2]);
    line.field_i32("step_x", probe.step_pos[0]);
    line.field_i32("step_y", probe.step_pos[1]);
    line.field_i32("step_z", probe.step_pos[2]);
    line.field_u32("landed", probe.landed as u32);
    line.field_u32("chosen", probe.chose_step as u32);
    line.finish();
}

/// Post-walk downward floor probe and retained Q6 origin residue. This is the
/// bridge between the integer PS1 hull and GoldSrc's float slope origin.
#[cfg(feature = "deep-reference-trace")]
pub fn player_ground(
    map_tick: u32,
    start_y: i32,
    end_y: i32,
    frac: i32,
    normal: [i32; 3],
    final_y: i32,
    residue_q6: i8,
) {
    let mut line = Line::new("ground");
    common(&mut line, map_tick);
    line.field_i32("start_y", start_y);
    line.field_i32("end_y", end_y);
    line.field_i32("frac", frac);
    line.field_i32("nx", normal[0]);
    line.field_i32("ny", normal[1]);
    line.field_i32("nz", normal[2]);
    line.field_i32("final_y", final_y);
    line.field_i32("residue_q6", residue_q6 as i32);
    line.finish();
}

/// Pre-sweep fixed-point player motion. This exposes the exact Q6 velocity,
/// retained origin residue, and integer hull delta chosen for one frame so a
/// GoldSrc/PSX divergence can be assigned to integration rather than collision.
/// Deep traces only; shipping/reference builds contain no call or strings.
#[cfg(feature = "deep-reference-trace")]
pub fn player_motion(
    map_tick: u32,
    start: [i32; 3],
    was_airborne: bool,
    jump: bool,
    ground_mover: i32,
    fine: [i32; 3],
    prior_carry_y: i8,
    delta: [i32; 3],
    next_carry_y: i8,
) {
    let mut line = Line::new("motion");
    common(&mut line, map_tick);
    line.field_i32("start_x", start[0]);
    line.field_i32("start_y", start[1]);
    line.field_i32("start_z", start[2]);
    line.field_u32("was_air", was_airborne as u32);
    line.field_u32("jump", jump as u32);
    line.field_i32("ground_mover", ground_mover);
    line.field_i32("fine_x", fine[0]);
    line.field_i32("fine_y", fine[1]);
    line.field_i32("fine_z", fine[2]);
    line.field_i32("carry_y", prior_carry_y as i32);
    line.field_i32("dx", delta[0]);
    line.field_i32("dy", delta[1]);
    line.field_i32("dz", delta[2]);
    line.field_i32("next_carry_y", next_carry_y as i32);
    line.finish();
}

/// Every bump considered by the integer equivalent of `PM_FlyMove`. `call`
/// distinguishes the flat and raised alternatives of `PM_WalkMove`; `bump`
/// is the collision-plane iteration inside that call. Deep traces only.
#[cfg(feature = "deep-reference-trace")]
#[allow(clippy::too_many_arguments)]
pub fn player_slide(
    map_tick: u32,
    call: u8,
    bump: u8,
    start: [i32; 3],
    velocity: [i32; 3],
    delta: [i32; 3],
    end: [i32; 3],
    time_left: i32,
    frac: i32,
    normal: [i32; 3],
    startsolid: bool,
    mover: i32,
) {
    let mut line = Line::new("slide");
    common(&mut line, map_tick);
    line.field_u32("call", call as u32);
    line.field_u32("bump", bump as u32);
    line.field_i32("time_left", time_left);
    line.field_i32("sx", start[0]);
    line.field_i32("sy", start[1]);
    line.field_i32("sz", start[2]);
    line.field_i32("vx", velocity[0]);
    line.field_i32("vy", velocity[1]);
    line.field_i32("vz", velocity[2]);
    line.field_i32("dx", delta[0]);
    line.field_i32("dy", delta[1]);
    line.field_i32("dz", delta[2]);
    line.field_i32("ex", end[0]);
    line.field_i32("ey", end[1]);
    line.field_i32("ez", end[2]);
    line.field_i32("frac", frac);
    line.field_i32("nx", normal[0]);
    line.field_i32("ny", normal[1]);
    line.field_i32("nz", normal[2]);
    line.field_u32("startsolid", startsolid as u32);
    line.field_i32("mover", mover);
    line.finish();
}

/// Exact ladder request and first collision result. GoldSrc exposes the same
/// stages through the opt-in `HLREF|pmladder` probe, so a differential can
/// distinguish view-basis/decomposition error from integer hull clipping.
pub fn player_ladder(
    map_tick: u32,
    start: [i32; 3],
    fine_q6: [i32; 3],
    move_delta: [i32; 3],
    first_frac: i32,
    first_normal: [i32; 3],
    first_startsolid: bool,
    first_mover: i32,
    final_pos: [i32; 3],
    final_vel: [i32; 3],
) {
    let mut line = Line::new("ladder");
    common(&mut line, map_tick);
    line.field_i32("sx", start[0]);
    line.field_i32("sy", start[1]);
    line.field_i32("sz", start[2]);
    line.field_i32("fine_x", fine_q6[0]);
    line.field_i32("fine_y", fine_q6[1]);
    line.field_i32("fine_z", fine_q6[2]);
    line.field_i32("dx", move_delta[0]);
    line.field_i32("dy", move_delta[1]);
    line.field_i32("dz", move_delta[2]);
    line.field_i32("first_frac", first_frac);
    line.field_i32("first_nx", first_normal[0]);
    line.field_i32("first_ny", first_normal[1]);
    line.field_i32("first_nz", first_normal[2]);
    line.field_u32("first_startsolid", first_startsolid as u32);
    line.field_i32("first_mover", first_mover);
    line.field_i32("fx", final_pos[0]);
    line.field_i32("fy", final_pos[1]);
    line.field_i32("fz", final_pos[2]);
    line.field_i32("vx", final_vel[0]);
    line.field_i32("vy", final_vel[1]);
    line.field_i32("vz", final_vel[2]);
    line.finish();
}

/// First world/brush collision for a player weapon ray. Deep traces only: this
/// makes a missed scripted shot distinguishable from a ray intercepted by the
/// wrong BSP hull without adding code or data to ordinary reference builds.
#[cfg(feature = "deep-reference-trace")]
pub fn hitscan(map_tick: u32, eye: [i32; 3], end: [i32; 3], hit: Option<crate::phys::RayHit>) {
    let mut line = Line::new("hitscan");
    common(&mut line, map_tick);
    line.field_i32("eye_x", eye[0]);
    line.field_i32("eye_y", eye[1]);
    line.field_i32("eye_z", eye[2]);
    line.field_i32("end_x", end[0]);
    line.field_i32("end_y", end[1]);
    line.field_i32("end_z", end[2]);
    if let Some(hit) = hit {
        line.field_i32("frac", hit.frac);
        line.field_i32("mover", hit.mover);
        line.field_i32("hit_x", hit.pos[0]);
        line.field_i32("hit_y", hit.pos[1]);
        line.field_i32("hit_z", hit.pos[2]);
        line.field_i32("nx", hit.normal[0]);
        line.field_i32("ny", hit.normal[1]);
        line.field_i32("nz", hit.normal[2]);
    } else {
        line.field_i32("frac", 4096);
        line.field_i32("mover", -2);
    }
    line.finish();
}

pub fn entity(state: EntityState<'_>) {
    let mut line = Line::new("entity");
    common(&mut line, state.map_tick);
    line.field_u32("map_tick", state.map_tick);
    line.field_u32("slot", state.slot as u32);
    line.field_str("class", state.class);
    line.field_str("targetname", state.targetname);
    line.field_i32("brush", state.brush);
    line.field_i32("x", state.pos[0]);
    line.field_i32("y", state.pos[1]);
    line.field_i32("z", state.pos[2]);
    line.field_i32("cx", state.center[0]);
    line.field_i32("cy", state.center[1]);
    line.field_i32("cz", state.center[2]);
    line.field_i32("vx", state.vel[0]);
    line.field_i32("vy", state.vel[1]);
    line.field_i32("vz", state.vel[2]);
    line.field_i32("yaw_q12", state.yaw_q12);
    line.field_u32("health", state.health as u32);
    line.field_u32("active", state.active as u32);
    line.field_u32("state", state.state as u32);
    line.field_i32("phase", state.phase);
    line.field_u32("spawnflags", state.spawnflags as u32);
    line.finish();
}

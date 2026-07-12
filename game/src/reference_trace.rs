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
pub fn nav_step(map_tick: u32, index: u16, dx: i32, dz: i32, result: u8) {
    let mut line = Line::new("nav");
    common(&mut line, map_tick);
    line.field_u32("index", index as u32);
    line.field_i32("dx", dx);
    line.field_i32("dz", dz);
    line.field_u32("result", result as u32);
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

//! Cooked schedule-table contract: the per-map AI section of a room, shared by the host
//! cooker (`host/hl-sched`), the census and the PS1 runtime (`game/src/ai`).
//!
//! The section is a trailer of the room blob: `... tables | u32 section_bytes | "AIT1"`, where
//! `section_bytes` counts the tables, itself and the tag. All integers are little endian.
//! Constants here use the numbering of the retail monster AI so that an oracle trace
//! (`conds`, `state`, schedule names) compares bit for bit with the port's state.

/// Trailer tag of the AI section (last four bytes of the room blob).
pub const SECTION_TAG: [u8; 4] = *b"AIT1";
/// Bytes of the trailer proper (`u32 section_bytes | tag`).
pub const TRAILER_BYTES: usize = 8;

/// Monster states (`MONSTERSTATE_*`).
pub mod state {
    pub const NONE: u8 = 0;
    pub const IDLE: u8 = 1;
    pub const COMBAT: u8 = 2;
    pub const ALERT: u8 = 3;
    pub const HUNT: u8 = 4;
    pub const PRONE: u8 = 5;
    pub const SCRIPT: u8 = 6;
    pub const PLAYDEAD: u8 = 7;
    pub const DEAD: u8 = 8;
    /// Bit of `state` in a rule's state mask.
    pub const fn bit(s: u8) -> u16 {
        1 << s
    }
}

/// Condition bits (`bits_COND_*`).
pub mod cond {
    pub const NO_AMMO_LOADED: u32 = 1 << 0;
    pub const SEE_HATE: u32 = 1 << 1;
    pub const SEE_FEAR: u32 = 1 << 2;
    pub const SEE_DISLIKE: u32 = 1 << 3;
    pub const SEE_ENEMY: u32 = 1 << 4;
    pub const ENEMY_OCCLUDED: u32 = 1 << 5;
    pub const SMELL_FOOD: u32 = 1 << 6;
    pub const ENEMY_TOOFAR: u32 = 1 << 7;
    pub const LIGHT_DAMAGE: u32 = 1 << 8;
    pub const HEAVY_DAMAGE: u32 = 1 << 9;
    pub const CAN_RANGE_ATTACK1: u32 = 1 << 10;
    pub const CAN_MELEE_ATTACK1: u32 = 1 << 11;
    pub const CAN_RANGE_ATTACK2: u32 = 1 << 12;
    pub const CAN_MELEE_ATTACK2: u32 = 1 << 13;
    pub const PROVOKED: u32 = 1 << 15;
    pub const NEW_ENEMY: u32 = 1 << 16;
    pub const HEAR_SOUND: u32 = 1 << 17;
    pub const SMELL: u32 = 1 << 18;
    pub const ENEMY_FACING_ME: u32 = 1 << 19;
    pub const ENEMY_DEAD: u32 = 1 << 20;
    pub const SEE_CLIENT: u32 = 1 << 21;
    pub const SEE_NEMESIS: u32 = 1 << 22;
    /// Species conditions (the grunt's "do not fire" verdict is `SPECIAL1`).
    pub const SPECIAL1: u32 = 1 << 28;
    pub const SPECIAL2: u32 = 1 << 29;
    pub const TASK_FAILED: u32 = 1 << 30;
    pub const SCHEDULE_DONE: u32 = 1 << 31;
}

/// Memory bits (`bits_MEMORY_*`), the low 16.
pub mod mem {
    pub const PROVOKED: u16 = 1 << 0;
    pub const IN_COVER: u16 = 1 << 1;
    pub const SUSPICIOUS: u16 = 1 << 2;
    pub const PATH_FINISHED: u16 = 1 << 3;
    pub const ON_PATH: u16 = 1 << 4;
    pub const MOVE_FAILED: u16 = 1 << 5;
    pub const FLINCHED: u16 = 1 << 6;
    pub const KILLED: u16 = 1 << 7;
}

/// Sound classes (`bits_SOUND_*`) a schedule listens for.
pub mod snd {
    pub const WORLD: u16 = 1 << 1;
    pub const PLAYER: u16 = 1 << 2;
    pub const CARCASS: u16 = 1 << 3;
    pub const MEAT: u16 = 1 << 4;
    pub const DANGER: u16 = 1 << 5;
    pub const GARBAGE: u16 = 1 << 6;
    pub const COMBAT: u16 = 1 << 0;
}

/// Task opcodes. The argument is an `i16`; its meaning is given per opcode.
pub mod op {
    pub const INVALID: u8 = 0;
    pub const WAIT: u8 = 1; // arg = ticks
    pub const WAIT_FACE_ENEMY: u8 = 2; // arg = ticks
    pub const WAIT_PVS: u8 = 3;
    pub const WAIT_RANDOM: u8 = 4; // arg = max ticks
    pub const WAIT_INDEFINITE: u8 = 5;
    pub const SUGGEST_STATE: u8 = 6; // arg = state
    pub const GET_PATH_TO_ENEMY: u8 = 7;
    pub const GET_PATH_TO_ENEMY_LKP: u8 = 8;
    pub const GET_PATH_TO_ENEMY_CORPSE: u8 = 9;
    pub const GET_PATH_TO_LEADER: u8 = 10;
    pub const GET_PATH_TO_TARGET: u8 = 11;
    pub const GET_PATH_TO_LASTPOSITION: u8 = 12;
    pub const GET_PATH_TO_BESTSOUND: u8 = 13;
    pub const RUN_PATH: u8 = 14;
    pub const WALK_PATH: u8 = 15;
    pub const WAIT_FOR_MOVEMENT: u8 = 16;
    pub const CLEAR_MOVE_WAIT: u8 = 17;
    pub const STORE_LASTPOSITION: u8 = 18;
    pub const CLEAR_LASTPOSITION: u8 = 19;
    pub const STOP_MOVING: u8 = 20;
    pub const FACE_IDEAL: u8 = 21;
    pub const FACE_ENEMY: u8 = 22;
    pub const FACE_TARGET: u8 = 23;
    pub const FACE_ROUTE: u8 = 24;
    pub const FACE_LASTPOSITION: u8 = 25;
    pub const TURN_LEFT: u8 = 26; // arg = degrees
    pub const TURN_RIGHT: u8 = 27; // arg = degrees
    pub const RANGE_ATTACK1: u8 = 28;
    pub const RANGE_ATTACK2: u8 = 29;
    pub const MELEE_ATTACK1: u8 = 30;
    pub const MELEE_ATTACK2: u8 = 31;
    pub const SPECIAL_ATTACK1: u8 = 32;
    pub const SPECIAL_ATTACK2: u8 = 33;
    pub const RELOAD: u8 = 34;
    pub const SMALL_FLINCH: u8 = 35;
    pub const SET_ACTIVITY: u8 = 36; // arg = activity
    pub const PLAY_SEQUENCE: u8 = 37; // arg = activity
    pub const PLAY_SEQUENCE_FACE_ENEMY: u8 = 38;
    pub const SET_SCHEDULE: u8 = 39; // arg = schedule type
    pub const SET_FAIL_SCHEDULE: u8 = 40; // arg = schedule type
    pub const CLEAR_FAIL_SCHEDULE: u8 = 41;
    pub const REMEMBER: u8 = 42; // arg = memory bits
    pub const FORGET: u8 = 43;
    pub const FIND_COVER_FROM_ENEMY: u8 = 44; // arg = max distance (0 = default)
    pub const FIND_COVER_FROM_BEST_SOUND: u8 = 45;
    pub const FIND_COVER_FROM_ORIGIN: u8 = 46;
    pub const FIND_FAR_NODE_COVER_FROM_ENEMY: u8 = 47; // arg = min distance
    pub const FIND_NEAR_NODE_COVER_FROM_ENEMY: u8 = 48; // arg = max distance
    pub const SOUND: u8 = 49; // arg = cue; no effect on the simulation
    pub const DIE: u8 = 50;
    pub const WAIT_FOR_SCRIPT: u8 = 51;
    pub const PLAY_SCRIPT: u8 = 52;
    /// Species task: arg = species task id, run by the species leaf (`World::custom_task`).
    pub const CUSTOM: u8 = 53;
    pub const FAIL: u8 = 54;
    pub const COUNT: u8 = 55;
}

/// Row predicates (`Pick.pred_a`, `Pick.pred_b`, `TypeRow.pred`). 0 always holds.
pub mod pred {
    pub const ALWAYS: u8 = 0;
    pub const IN_SQUAD: u8 = 1;
    pub const NOT_IN_SQUAD: u8 = 2;
    pub const IS_LEADER: u8 = 3;
    pub const NOT_LEADER: u8 = 4;
    pub const SKILL_HARD: u8 = 5;
    pub const RANDOM_HALF: u8 = 6;
    pub const RANDOM_TENTH: u8 = 7;
    /// Takes an engage slot (side effect: occupied until the schedule changes).
    pub const OCCUPY_ENGAGE: u8 = 8;
    pub const OCCUPY_GRENADE: u8 = 9;
    pub const OCCUPY_ATTACK: u8 = 10;
    pub const HAS_ENEMY: u8 = 11;
    pub const FLAG_A: u8 = 12; // species capability flag 0 (grunt: grenade launcher)
    pub const FLAG_B: u8 = 13;
    pub const SQUAD_ENEMY_ELUDED: u8 = 14;
    pub const STANDING: u8 = 15;
    pub const FIRST_ENCOUNTER: u8 = 16;
    pub const ENEMY_IS_PLAYER: u8 = 17;
    pub const LIGHT_DAMAGE_90: u8 = 18; // 90 percent roll
    pub const MOVETYPE_FLY: u8 = 19;
    /// The last damage came from roughly the monster's front.
    pub const DAMAGE_AHEAD: u8 = 20;
    pub const NOT_FLINCHED: u8 = 21;
    /// The best sound heard is a DANGER sound (a grenade).
    pub const DANGER_SOUND: u8 = 22;
    /// Squad member, skill hard, can throw a grenade, grenade slot taken.
    pub const HARD_GRENADE_COVER: u8 = 23;
    /// Can range attack 1 and an engage slot was taken.
    pub const CAN_RANGE1_ENGAGE: u8 = 24;
    /// Re-rolls the standing flag one time in ten, then holds when standing.
    pub const ROLL_STANDING: u8 = 25;
    /// The enemy is the player and this is the first meeting (clears the first-encounter flag).
    pub const FIRST_ENCOUNTER_TAKE: u8 = 26;
    pub const COUNT: u8 = 27;
}

/// Species task ids for [`op::CUSTOM`].
pub mod custom {
    pub const GRUNT_FACE_TOSS_DIR: i16 = 1;
    pub const GRUNT_SPEAK: i16 = 2;
    pub const GRUNT_CHECK_FIRE: i16 = 3;
}

/// A type row whose schedule byte has this bit set redirects to another schedule type (the low 7 bits).
pub const TYPE_REDIRECT: u8 = 0x80;

/// Row effects, run when a pick or type row is taken.
pub mod fx {
    pub const NONE: u8 = 0;
    pub const CLEAR_ELUDED: u8 = 1;
    pub const ROLL_STANDING: u8 = 2; // 1 in 10: re-roll the standing flag
    pub const CLEAR_FIRST_ENCOUNTER: u8 = 3;
    pub const COUNT: u8 = 4;
}

/// Schedule types (`SCHED_*`) a pick row asks for; a type row turns it into a concrete schedule.
/// Species add types from `FIRST_SPECIES_TYPE` up.
pub mod ty {
    pub const NONE: u8 = 0;
    pub const IDLE_STAND: u8 = 1;
    pub const IDLE_WALK: u8 = 2;
    pub const WAKE_ANGRY: u8 = 3;
    pub const ALERT_FACE: u8 = 5;
    pub const ALERT_SMALL_FLINCH: u8 = 6;
    pub const ALERT_BIG_FLINCH: u8 = 7;
    pub const ALERT_STAND: u8 = 8;
    pub const INVESTIGATE_SOUND: u8 = 9;
    pub const COMBAT_FACE: u8 = 10;
    pub const COMBAT_STAND: u8 = 11;
    pub const CHASE_ENEMY: u8 = 12;
    pub const CHASE_ENEMY_FAILED: u8 = 13;
    pub const VICTORY_DANCE: u8 = 14;
    pub const SMALL_FLINCH: u8 = 17;
    pub const TAKE_COVER_FROM_ENEMY: u8 = 18;
    pub const TAKE_COVER_FROM_BEST_SOUND: u8 = 19;
    pub const TAKE_COVER_FROM_ORIGIN: u8 = 20;
    pub const COWER: u8 = 21;
    pub const MELEE_ATTACK1: u8 = 22;
    pub const MELEE_ATTACK2: u8 = 23;
    pub const RANGE_ATTACK1: u8 = 24;
    pub const RANGE_ATTACK2: u8 = 25;
    pub const SPECIAL_ATTACK1: u8 = 26;
    pub const SPECIAL_ATTACK2: u8 = 27;
    pub const STANDOFF: u8 = 28;
    pub const ARM_WEAPON: u8 = 29;
    pub const RELOAD: u8 = 30;
    pub const DIE: u8 = 33;
    pub const WAIT_TRIGGER: u8 = 34;
    pub const FOLLOW: u8 = 35;
    pub const AISCRIPT: u8 = 39;
    pub const FAIL: u8 = 40;
    pub const FIRST_SPECIES_TYPE: u8 = 48;
}

/// Layout of the tables blob (offsets from the blob start).
pub mod layout {
    /// Header: six `(u16 offset, u16 count)` pairs for schedules, tasks, picks, type rows,
    /// species and squad groups, then 4 reserved bytes.
    pub const HEADER_BYTES: usize = 24;
    /// `u32 interrupt | u16 sound | u16 first_task | u8 task_count | u8 reserved`.
    pub const SCHED_BYTES: usize = 10;
    /// `u8 op | i16 arg`.
    pub const TASK_BYTES: usize = 3;
    /// `u8 species | u8 reserved | u8 pred_a | u8 pred_b | u8 type | u8 effect |
    /// u16 state_mask | u32 any_of | u32 none_of | u32 all_of`.
    pub const PICK_BYTES: usize = 20;
    /// `u8 species | u8 type | u8 pred | u8 schedule`.
    pub const TYPE_BYTES: usize = 4;
    /// `u8 id | u8 parent | u8 flags | u8 reserved | u16 r1_min | u16 r1_max | i16 cone_q12 |
    /// u16 r2_min | u16 r2_max | u16 melee_range`.
    pub const SPECIES_BYTES: usize = 16;
    /// `u8 prop | u8 group | u8 flags`: a placed monster's squad group (its `netname`, numbered from 1 per
    /// map) and `GROUP_LEADER` for the one flagged leader.
    pub const GROUP_BYTES: usize = 3;
    /// Group row flag: the flagged squad leader (`SF_SQUADMONSTER_LEADER`).
    pub const GROUP_LEADER: u8 = 1;
}

/// Why a table blob was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Invalid {
    Short,
    Range(&'static str),
    Index(&'static str),
}

/// A validated table blob. All reads are unchecked after [`Tables::new`] succeeds.
#[derive(Clone, Copy)]
pub struct Tables {
    d: &'static [u8],
}

/// One schedule header.
#[derive(Clone, Copy)]
pub struct Sched {
    pub interrupt: u32,
    pub sound: u16,
    pub first: u16,
    pub count: u8,
}

/// One schedule-type selection row.
#[derive(Clone, Copy)]
pub struct Pick {
    pub species: u8,
    pub pred_a: u8,
    pub pred_b: u8,
    pub ty: u8,
    pub effect: u8,
    pub states: u16,
    pub any_of: u32,
    pub none_of: u32,
    pub all_of: u32,
}

/// Per-species parameters.
#[derive(Clone, Copy)]
pub struct Species {
    pub id: u8,
    pub parent: u8,
    pub flags: u8,
    pub r1_min: u16,
    pub r1_max: u16,
    pub cone_q12: i16,
    pub r2_min: u16,
    pub r2_max: u16,
    pub melee_range: u16,
}

/// Byte `o` of a table blob.
///
/// SAFETY of the unchecked read: `Tables::new` checks every offset, count and index of a blob once on the
/// host (the cooker runs it), and the guest only wraps blobs the cooker wrote (`Tables::trusted`), so no
/// accessor reads past the end. Skipping the bounds check removes a panic call site from every read.
#[inline(always)]
fn byte(d: &[u8], o: usize) -> u8 {
    #[cfg(target_arch = "mips")]
    {
        // SAFETY: see above.
        unsafe { *d.get_unchecked(o) }
    }
    #[cfg(not(target_arch = "mips"))]
    {
        d[o]
    }
}

#[inline(always)]
fn u16le(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([byte(d, o), byte(d, o + 1)])
}

#[inline(always)]
fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([byte(d, o), byte(d, o + 1), byte(d, o + 2), byte(d, o + 3)])
}

impl Tables {
    /// Checks every offset, count and index once; the accessors then read without bounds
    /// checks of their own on the PS1 (they still index a slice, which the optimizer keeps cheap
    /// because validation bounds the offsets).
    pub fn new(d: &'static [u8]) -> Result<Tables, Invalid> {
        use layout::*;
        if d.len() < HEADER_BYTES {
            return Err(Invalid::Short);
        }
        let t = Tables { d };
        let parts = [
            ("schedules", t.sched_off(), t.n_sched(), SCHED_BYTES),
            ("tasks", t.task_off(), t.n_task(), TASK_BYTES),
            ("picks", t.pick_off(), t.n_pick(), PICK_BYTES),
            ("types", t.type_off(), t.n_type(), TYPE_BYTES),
            ("species", t.species_off(), t.n_species(), SPECIES_BYTES),
            ("groups", t.group_off(), t.n_group(), GROUP_BYTES),
        ];
        for (name, off, n, size) in parts {
            if off + n * size > d.len() {
                return Err(Invalid::Range(name));
            }
        }
        for i in 0..t.n_sched() {
            let s = t.sched(i as u8);
            if s.first as usize + s.count as usize > t.n_task() {
                return Err(Invalid::Index("schedule task range"));
            }
        }
        for i in 0..t.n_task() {
            let (o, a) = t.task_at(i);
            if o >= op::COUNT {
                return Err(Invalid::Index("task opcode"));
            }
            if (o == op::SET_SCHEDULE || o == op::SET_FAIL_SCHEDULE) && !(0..256).contains(&a) {
                return Err(Invalid::Index("task schedule type"));
            }
        }
        for i in 0..t.n_type() {
            let sched = t.type_row(i).3;
            if sched & TYPE_REDIRECT == 0 && sched as usize >= t.n_sched() {
                return Err(Invalid::Index("type row schedule"));
            }
        }
        for i in 0..t.n_pick() {
            let p = t.pick(i);
            if p.pred_a >= pred::COUNT || p.pred_b >= pred::COUNT || p.effect >= fx::COUNT {
                return Err(Invalid::Index("pick predicate or effect"));
            }
        }
        Ok(t)
    }

    /// Wraps a blob the cooker already validated with [`Tables::new`] (the PS1 never re-checks it).
    pub const fn trusted(d: &'static [u8]) -> Tables {
        Tables { d }
    }

    pub fn sched_off(&self) -> usize {
        u16le(self.d, 0) as usize
    }
    pub fn n_sched(&self) -> usize {
        u16le(self.d, 2) as usize
    }
    pub fn task_off(&self) -> usize {
        u16le(self.d, 4) as usize
    }
    pub fn n_task(&self) -> usize {
        u16le(self.d, 6) as usize
    }
    pub fn pick_off(&self) -> usize {
        u16le(self.d, 8) as usize
    }
    pub fn n_pick(&self) -> usize {
        u16le(self.d, 10) as usize
    }
    pub fn type_off(&self) -> usize {
        u16le(self.d, 12) as usize
    }
    pub fn n_type(&self) -> usize {
        u16le(self.d, 14) as usize
    }
    pub fn species_off(&self) -> usize {
        u16le(self.d, 16) as usize
    }
    pub fn n_species(&self) -> usize {
        u16le(self.d, 18) as usize
    }
    pub fn group_off(&self) -> usize {
        u16le(self.d, 20) as usize
    }
    pub fn n_group(&self) -> usize {
        u16le(self.d, 22) as usize
    }

    /// `(prop, group, flags)` of group row `i`.
    #[inline]
    pub fn group(&self, i: usize) -> (u8, u8, u8) {
        let o = self.group_off() + i * layout::GROUP_BYTES;
        (byte(self.d, o), byte(self.d, o + 1), byte(self.d, o + 2))
    }

    /// Schedule header `id` (an index; schedule 0 is the idle "no schedule" placeholder).
    #[inline]
    pub fn sched(&self, id: u8) -> Sched {
        let o = self.sched_off() + id as usize * layout::SCHED_BYTES;
        Sched {
            interrupt: u32le(self.d, o),
            sound: u16le(self.d, o + 4),
            first: u16le(self.d, o + 6),
            count: byte(self.d, o + 8),
        }
    }

    #[inline]
    fn task_at(&self, i: usize) -> (u8, i16) {
        let o = self.task_off() + i * layout::TASK_BYTES;
        (byte(self.d, o), u16le(self.d, o + 1) as i16)
    }

    /// Task `i` of schedule `s`.
    #[inline]
    pub fn task(&self, s: &Sched, i: u8) -> (u8, i16) {
        self.task_at(s.first as usize + i as usize)
    }

    #[inline]
    pub fn pick(&self, i: usize) -> Pick {
        let o = self.pick_off() + i * layout::PICK_BYTES;
        Pick {
            species: byte(self.d, o),
            pred_a: byte(self.d, o + 2),
            pred_b: byte(self.d, o + 3),
            ty: byte(self.d, o + 4),
            effect: byte(self.d, o + 5),
            states: u16le(self.d, o + 6),
            any_of: u32le(self.d, o + 8),
            none_of: u32le(self.d, o + 12),
            all_of: u32le(self.d, o + 16),
        }
    }

    /// `(species, type, predicate, schedule)` of type row `i`.
    #[inline]
    pub fn type_row(&self, i: usize) -> (u8, u8, u8, u8) {
        let o = self.type_off() + i * layout::TYPE_BYTES;
        (byte(self.d, o), byte(self.d, o + 1), byte(self.d, o + 2), byte(self.d, o + 3))
    }

    /// Parameters of species `id`, if the map carries it.
    pub fn species(&self, id: u8) -> Option<Species> {
        for i in 0..self.n_species() {
            let o = self.species_off() + i * layout::SPECIES_BYTES;
            if byte(self.d, o) == id {
                return Some(Species {
                    id,
                    parent: byte(self.d, o + 1),
                    flags: byte(self.d, o + 2),
                    r1_min: u16le(self.d, o + 4),
                    r1_max: u16le(self.d, o + 6),
                    cone_q12: u16le(self.d, o + 8) as i16,
                    r2_min: u16le(self.d, o + 10),
                    r2_max: u16le(self.d, o + 12),
                    melee_range: u16le(self.d, o + 14),
                });
            }
        }
        None
    }
}

/// The table blob of a room, found from the trailer, or `None` when the room carries none.
/// `room` is the whole room blob.
pub fn section(room: &'static [u8]) -> Option<&'static [u8]> {
    let len = room.len() & !3;
    if len < TRAILER_BYTES + layout::HEADER_BYTES || room[len - 4..len] != SECTION_TAG {
        return None;
    }
    let bytes = u32le(room, len - 8) as usize;
    if bytes < TRAILER_BYTES + layout::HEADER_BYTES || bytes > len {
        return None;
    }
    Some(&room[len - bytes..len - TRAILER_BYTES])
}

/// Length of `room` without the AI trailer (the end the older trailers are found from).
pub fn room_end(room: &[u8]) -> usize {
    let len = room.len() & !3;
    if len >= TRAILER_BYTES + layout::HEADER_BYTES && room[len - 4..len] == SECTION_TAG {
        let bytes = u32le(room, len - 8) as usize;
        if bytes >= TRAILER_BYTES + layout::HEADER_BYTES && bytes <= len {
            return len - bytes;
        }
    }
    len
}

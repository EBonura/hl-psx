//! Schedule runtime for the monsters: state, conditions, schedules and tasks, squads.
//!
//! Pure integer logic with no hardware, included by the host runner (`hl-logic-tests`) and by the
//! game. The world (movement, animation, senses, attacks) is reached through [`World`]; the tables
//! are the per-map `AIT1` section (`hl_format::ai`). Written for this port from the retail
//! monster-AI behaviour (the oracle prints schedule name, task and conditions every tick).

use hl_format::ai::{cond, fx, mem, op, pred, state, ty, Sched, Species, Tables, TYPE_REDIRECT};

/// Conditions the world's `look` recomputes at every think.
pub const LOOK_OWNED: u32 = cond::SEE_HATE
    | cond::SEE_FEAR
    | cond::SEE_DISLIKE
    | cond::SEE_ENEMY
    | cond::ENEMY_OCCLUDED
    | cond::ENEMY_TOOFAR
    | cond::CAN_RANGE_ATTACK1
    | cond::CAN_MELEE_ATTACK1
    | cond::CAN_RANGE_ATTACK2
    | cond::CAN_MELEE_ATTACK2
    | cond::ENEMY_FACING_ME
    | cond::HEAR_SOUND;

/// "No row" / "no enemy" marker in the byte fields.
pub const NONE: u8 = 0xFF;
/// Squad slot bits (engage x2, grenade x2, split), as in the retail squad slot word.
pub const SLOTS_ENGAGE: u16 = 0b11;
pub const SLOTS_GRENADE: u16 = 0b1100;
pub const SLOTS_ATTACK: u16 = 0b11_1000_0000;
pub const SLOT_SPLIT: u16 = 1 << 10;
/// Row `flags` bits.
pub const F_LAUNCHER: u8 = 1; // species flag A: carries a grenade launcher
pub const F_FLAG_B: u8 = 2;
pub const F_STANDING: u8 = 4;
pub const F_FIRST_ENCOUNTER: u8 = 8;
pub const F_FLYING: u8 = 16;

/// Task status.
const NEW: u8 = 0;
const RUNNING: u8 = 1;
const COMPLETE: u8 = 4;

/// Longest schedule run in one think (the original loops ten times).
const MAX_STEPS: u8 = 10;

/// One monster's schedule state. `repr(C)` and exactly 40 bytes: the tick log reads the rows of the static
/// pool as RAM words (`conds` at +8, `memory`/`wait_until` at +12, `slot`/`enemy`/`squad`/`fail`/`aux` at +16).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Row {
    pub prop: u8,
    pub species: u8,
    pub state: u8,
    pub ideal: u8,
    pub sched: u8,
    pub idx: u8,
    pub tstatus: u8,
    pub flags: u8,
    pub conds: u32,
    pub memory: u16,
    pub wait_until: u16,
    /// Squad slot this monster holds (0 = none).
    pub slot: u16,
    pub enemy: u8,
    /// Squad id 1..=7, 0 = not in a squad.
    pub squad: u8,
    /// Schedule type to run when the current one fails (0 = the species' fail schedule).
    pub fail: u8,
    pub aux: u8,
    /// Yaw the monster is turning to (12-bit), set by the turn tasks.
    pub yaw: u16,
    pub goal: [i16; 3],
    pub last: [i16; 3],
    /// Closest the walker has been to its goal on this route (the world's progress check).
    pub best: u16,
}

const _: () = assert!(core::mem::size_of::<Row>() == 40);

impl Row {
    pub const EMPTY: Row = Row {
        prop: NONE,
        species: 0,
        state: state::IDLE,
        ideal: state::IDLE,
        sched: 0,
        idx: 0,
        tstatus: NEW,
        flags: 0,
        conds: 0,
        memory: 0,
        wait_until: 0,
        slot: 0,
        enemy: NONE,
        squad: 0,
        fail: 0,
        aux: 0,
        yaw: 0,
        goal: [0; 3],
        last: [0; 3],
        best: 0,
    };
}

/// Squad bookkeeping, indexed by squad id.
#[derive(Clone, Copy)]
pub struct Squads {
    /// Slot word per squad (bits taken by members).
    pub slots: [u16; 8],
    /// Leader row per squad.
    pub leader: [u8; 8],
    /// Bit per squad: the squad's enemy got away (the original's `m_fEnemyEluded`).
    pub eluded: u8,
}

impl Squads {
    pub const EMPTY: Squads = Squads { slots: [0; 8], leader: [NONE; 8], eluded: 0 };
}

/// How a route progresses.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Moving,
    Done,
    Failed,
}

/// Where a path goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    Enemy,
    EnemyLkp,
    EnemyCorpse,
    Leader,
    Target,
    BestSound,
}

/// What a cover search wants.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    Enemy,
    Sound,
    Origin,
}

/// Result of a species task.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Running,
    Complete,
    Failed,
}

/// The world a monster lives in. `p` is the prop index of the monster.
pub trait World {
    fn now(&self) -> u16;
    fn rand(&mut self, n: u32) -> u32;
    fn skill_hard(&self) -> bool;
    /// Sensing for one think: returns the enemy to keep (or `NONE`) and the new value of the sensed
    /// conditions (`LOOK_OWNED`: SEE_ENEMY, ENEMY_OCCLUDED, CAN_*_ATTACK*, ENEMY_FACING_ME, HEAR_SOUND ...)
    /// plus NEW_ENEMY and ENEMY_DEAD when they apply. `sounds` is the current schedule's sound mask.
    fn look(&mut self, p: u8, enemy: u8, sounds: u16) -> (u8, u32);
    fn enemy_is_player(&self, e: u8) -> bool;
    /// The last damage came from roughly in front of the monster.
    fn damage_ahead(&self, p: u8) -> bool;
    /// The best sound heard is a DANGER sound.
    fn danger_sound(&self, p: u8) -> bool;
    fn set_activity(&mut self, p: u8, act: u8);
    fn seq_done(&self, p: u8) -> bool;
    /// Turn toward the enemy's last known position (`enemy`) or toward `ideal`; true when facing.
    /// With `enemy` the new ideal yaw is returned in `ideal`.
    fn face(&mut self, p: u8, enemy: bool, ideal: &mut u16) -> bool;
    /// The ideal yaw `deg` degrees left (positive) or right (negative) of the current yaw.
    fn turn_by(&mut self, p: u8, deg: i16) -> u16;
    fn stop(&mut self, p: u8);
    /// Build the route to `goal`; `out` receives the destination point.
    fn path_to(&mut self, p: u8, goal: Goal, out: &mut [i16; 3]) -> bool;
    /// Start moving along the built route (movement activity run or walk).
    fn begin_move(&mut self, p: u8, run: bool) -> bool;
    /// One think of the route toward `goal`.
    fn move_tick(&mut self, p: u8, goal: [i16; 3]) -> Move;
    /// Cover search; `others` are the other squad members' positions (a cover point near one is not valid).
    fn find_cover(&mut self, p: u8, from: Cover, min: i32, max: i32, others: &[[i16; 3]], out: &mut [i16; 3]) -> bool;
    fn store_position(&self, p: u8) -> [i16; 3];
    /// Begin an attack animation effect (fire, kick, throw); the activity itself is set by the runtime.
    fn attack(&mut self, p: u8, task: u8);
    /// Species task `id`; `first` is true on the first call.
    /// Returns the task result and conditions to set on the monster.
    fn custom(&mut self, p: u8, id: i16, first: bool) -> (Task, u32);
    fn die(&mut self, p: u8);
}

/// All schedule state of a map.
pub struct Ai<const N: usize> {
    pub rows: [Row; N],
    pub squads: Squads,
}

impl<const N: usize> Ai<N> {
    pub const fn new() -> Self {
        Ai { rows: [Row::EMPTY; N], squads: Squads::EMPTY }
    }

    /// Row of prop `p`, if it has one.
    #[optimize(size)]
    pub fn row_of(&self, p: u8) -> Option<usize> {
        let mut i = 0;
        while i < N {
            if self.rows[i].prop == p {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// Give prop `p` a row (idle, no schedule). `None` when every row is taken.
    #[optimize(size)]
    #[inline(never)]
    pub fn claim(&mut self, p: u8, species: u8, flags: u8) -> Option<usize> {
        let mut free = N;
        let mut i = 0;
        while i < N {
            if self.rows[i].prop == p {
                return Some(i);
            }
            if self.rows[i].prop == NONE && free == N {
                free = i;
            }
            i += 1;
        }
        if free == N {
            return None;
        }
        self.rows[free] = Row { prop: p, species, flags, ..Row::EMPTY };
        Some(free)
    }

    /// Free a monster's row (death, removal), leaving its squad.
    #[optimize(size)]
    pub fn release(&mut self, i: usize) {
        self.leave_squad(i);
        self.rows[i] = Row::EMPTY;
    }

    /// Members of squad `id` (rows holding a prop).
    #[optimize(size)]
    fn squad_size(&self, id: u8) -> usize {
        let mut n = 0;
        let mut i = 0;
        while i < N {
            if self.rows[i].prop != NONE && self.rows[i].squad == id {
                n += 1;
            }
            i += 1;
        }
        n
    }

    /// First row in squad `id`, or `NONE`.
    #[optimize(size)]
    fn first_member(&self, id: u8) -> u8 {
        let mut i = 0;
        while i < N {
            if self.rows[i].prop != NONE && self.rows[i].squad == id {
                return i as u8;
            }
            i += 1;
        }
        NONE
    }

    /// Number of monsters in the row's squad (1 when alone).
    #[optimize(size)]
    pub fn squad_count(&self, i: usize) -> usize {
        let s = self.rows[i].squad;
        if s == 0 {
            1
        } else {
            self.squad_size(s)
        }
    }

    /// Recruit unsquadded monsters of the same species within `radius2` (squared, map units) of
    /// row `i`, up to `max` members. `dist2(a, b)` gives the squared distance between two props.
    #[optimize(size)]
    #[inline(never)]
    pub fn recruit(&mut self, i: usize, max: usize, radius2: i32, dist2: impl Fn(u8, u8) -> i32) {
        if self.rows[i].squad == 0 {
            let mut id = 1;
            while id < 8 && self.squads.leader[id] != NONE {
                id += 1;
            }
            if id == 8 {
                return;
            }
            self.squads.leader[id] = i as u8;
            self.squads.slots[id] = 0;
            self.rows[i].squad = id as u8;
        }
        let id = self.rows[i].squad;
        let mut count = self.squad_size(id);
        let mut j = 0;
        while j < N && count < max {
            let (a, b) = (self.rows[i], self.rows[j]);
            if j != i && b.prop != NONE && b.squad == 0 && b.species == a.species && b.state != state::DEAD
                && dist2(a.prop, b.prop) < radius2
            {
                self.rows[j].squad = id;
                count += 1;
            }
            j += 1;
        }
        // No single-member squads.
        if count == 1 {
            self.leave_squad(i);
        }
    }

    /// Leave the squad; the next member becomes leader and the squad ends when it is empty.
    #[optimize(size)]
    #[inline(never)]
    pub fn leave_squad(&mut self, i: usize) {
        let id = self.rows[i].squad;
        if id == 0 {
            return;
        }
        self.vacate(i);
        self.rows[i].squad = 0;
        if self.squads.leader[id as usize] == i as u8 {
            let next = self.first_member(id);
            self.squads.leader[id as usize] = next;
            if next == NONE {
                self.squads.slots[id as usize] = 0;
                self.squads.eluded &= !(1 << id);
            }
        }
    }

    /// Release the slot the monster holds.
    #[optimize(size)]
    fn vacate(&mut self, i: usize) {
        let id = self.rows[i].squad as usize;
        if id != 0 {
            self.squads.slots[id] &= !self.rows[i].slot;
        }
        self.rows[i].slot = 0;
    }

    #[optimize(size)]
    fn enemy_split(&self, id: u8) -> bool {
        let lead = self.squads.leader[id as usize] as usize;
        let e = if lead < N { self.rows[lead].enemy } else { NONE };
        let mut i = 0;
        while i < N {
            let r = &self.rows[i];
            if r.prop != NONE && r.squad == id && r.enemy != NONE && r.enemy != e {
                return true;
            }
            i += 1;
        }
        false
    }

    /// Take one of the `desired` squad slots (the original's `OccupySlot`). A monster outside a squad
    /// always succeeds.
    #[optimize(size)]
    #[inline(never)]
    pub fn occupy(&mut self, i: usize, desired: u16) -> bool {
        let id = self.rows[i].squad;
        if id == 0 {
            return true;
        }
        if self.enemy_split(id) {
            self.rows[i].slot = SLOT_SPLIT;
            return true;
        }
        let taken = self.squads.slots[id as usize];
        if desired ^ taken == 0 {
            return false;
        }
        for b in 0..11 {
            let bit = 1u16 << b;
            if desired & bit != 0 && taken & bit == 0 {
                self.squads.slots[id as usize] |= bit;
                self.rows[i].slot = bit;
                return true;
            }
        }
        false
    }

    /// A member sees `e`: members that do not see an enemy yet adopt it (`SquadMakeEnemy`).
    #[optimize(size)]
    fn share_enemy(&mut self, i: usize, e: u8) {
        let id = self.rows[i].squad;
        if id == 0 {
            return;
        }
        for m in 0..N {
            let r = &mut self.rows[m];
            if r.prop != NONE && r.squad == id && r.enemy != e && r.conds & cond::SEE_ENEMY == 0 {
                r.enemy = e;
                r.conds |= cond::NEW_ENEMY;
            }
        }
    }

    #[optimize(size)]
    #[inline(never)]
    fn predicate<W: World>(&mut self, w: &mut W, i: usize, p: u8) -> bool {
        let r = self.rows[i];
        let lead = if r.squad == 0 { NONE } else { self.squads.leader[r.squad as usize] };
        match p {
            pred::ALWAYS => true,
            pred::IN_SQUAD => r.squad != 0,
            pred::NOT_IN_SQUAD => r.squad == 0,
            pred::IS_LEADER => r.squad != 0 && lead == i as u8,
            pred::NOT_LEADER => r.squad != 0 && lead != i as u8,
            pred::SKILL_HARD => w.skill_hard(),
            pred::RANDOM_HALF => w.rand(2) == 0,
            pred::RANDOM_TENTH => w.rand(10) == 0,
            pred::OCCUPY_ENGAGE => self.occupy(i, SLOTS_ENGAGE),
            pred::OCCUPY_GRENADE => self.occupy(i, SLOTS_GRENADE),
            pred::OCCUPY_ATTACK => self.occupy(i, SLOTS_ATTACK),
            pred::HAS_ENEMY => r.enemy != NONE,
            pred::FLAG_A => r.flags & F_LAUNCHER != 0,
            pred::FLAG_B => r.flags & F_FLAG_B != 0,
            pred::SQUAD_ENEMY_ELUDED => r.squad != 0 && self.squads.eluded & (1 << r.squad) != 0,
            pred::STANDING => r.flags & F_STANDING != 0,
            pred::FIRST_ENCOUNTER => r.flags & F_FIRST_ENCOUNTER != 0,
            pred::ENEMY_IS_PLAYER => r.enemy != NONE && w.enemy_is_player(r.enemy),
            pred::LIGHT_DAMAGE_90 => r.enemy != NONE && w.rand(100) <= 90,
            pred::MOVETYPE_FLY => r.flags & F_FLYING != 0,
            pred::DAMAGE_AHEAD => w.damage_ahead(r.prop),
            pred::NOT_FLINCHED => r.memory & mem::FLINCHED == 0,
            pred::DANGER_SOUND => w.danger_sound(r.prop),
            pred::HARD_GRENADE_COVER => {
                r.squad != 0 && w.skill_hard() && r.conds & cond::CAN_RANGE_ATTACK2 != 0 && self.occupy(i, SLOTS_GRENADE)
            }
            pred::CAN_RANGE1_ENGAGE => r.conds & cond::CAN_RANGE_ATTACK1 != 0 && self.occupy(i, SLOTS_ENGAGE),
            pred::ROLL_STANDING => {
                if w.rand(10) == 0 {
                    let stand = if w.rand(2) == 1 { F_STANDING } else { 0 };
                    self.rows[i].flags = (r.flags & !F_STANDING) | stand;
                }
                self.rows[i].flags & F_STANDING != 0
            }
            pred::FIRST_ENCOUNTER_TAKE => {
                let take = r.enemy != NONE && w.enemy_is_player(r.enemy) && r.flags & F_FIRST_ENCOUNTER != 0;
                if take {
                    self.rows[i].flags &= !F_FIRST_ENCOUNTER;
                }
                take
            }
            _ => false,
        }
    }

    #[optimize(size)]
    fn effect(&mut self, w: &mut impl World, i: usize, e: u8) {
        let id = self.rows[i].squad;
        match e {
            fx::CLEAR_ELUDED if id != 0 => self.squads.eluded &= !(1 << id),
            fx::CLEAR_FIRST_ENCOUNTER => self.rows[i].flags &= !F_FIRST_ENCOUNTER,
            _ => {}
        }
    }

    /// Parent of species `sp` in the table, or `sp` itself at the root.
    #[optimize(size)]
    fn parent(t: &Tables, sp: u8) -> u8 {
        match t.species(sp) {
            Some(s) => s.parent,
            None => sp,
        }
    }

    /// The original's `GetSchedule`: the schedule TYPE for the monster's state and conditions.
    #[optimize(size)]
    #[inline(never)]
    fn pick<W: World>(&mut self, w: &mut W, t: &Tables, i: usize) -> u8 {
        if self.rows[i].state == state::COMBAT && self.rows[i].conds & cond::ENEMY_DEAD != 0 {
            // The enemy died: forget it. With no other enemy the monster drops to ALERT (the next think's
            // look finds one if there is another).
            self.rows[i].enemy = NONE;
            self.rows[i].state = state::ALERT;
            self.rows[i].ideal = state::ALERT;
        }
        let (species, state, conds) = (self.rows[i].species, self.rows[i].state, self.rows[i].conds);
        let mut sp = species;
        loop {
            for k in 0..t.n_pick() {
                let p = t.pick(k);
                if p.species != sp
                    || p.states & (1 << state) == 0
                    || (p.any_of != 0 && conds & p.any_of == 0)
                    || conds & p.all_of != p.all_of
                    || conds & p.none_of != 0
                {
                    continue;
                }
                if self.predicate(w, i, p.pred_a) && self.predicate(w, i, p.pred_b) {
                    self.effect(w, i, p.effect);
                    return p.ty;
                }
            }
            let up = Self::parent(t, sp);
            if up == sp {
                return ty::IDLE_STAND;
            }
            sp = up;
        }
    }

    /// The original's `GetScheduleOfType`: the concrete schedule for a type.
    #[optimize(size)]
    #[inline(never)]
    fn of_type<W: World>(&mut self, w: &mut W, t: &Tables, i: usize, mut kind: u8) -> u8 {
        let species = self.rows[i].species;
        for _ in 0..4 {
            let mut sp = species;
            let mut redirect = NONE;
            loop {
                let mut k = 0;
                while k < t.n_type() && redirect == NONE {
                    let (s, ty_, p, sched) = t.type_row(k);
                    if s == sp && ty_ == kind && self.predicate(w, i, p) {
                        if sched & TYPE_REDIRECT == 0 {
                            return sched;
                        }
                        redirect = sched & !TYPE_REDIRECT;
                    }
                    k += 1;
                }
                let up = Self::parent(t, sp);
                if redirect != NONE || up == sp {
                    break;
                }
                sp = up;
            }
            if redirect == NONE {
                return 0;
            }
            kind = redirect;
        }
        0
    }

    #[optimize(size)]
    fn change_schedule(&mut self, i: usize, id: u8) {
        let r = &mut self.rows[i];
        r.sched = id;
        r.idx = 0;
        r.tstatus = NEW;
        r.conds = 0;
        r.fail = 0;
    }

    /// The original's `GetIdealState`.
    #[optimize(size)]
    #[inline(never)]
    fn ideal_state(&mut self, i: usize) {
        // Squad monsters hand a newly seen enemy to the whole squad before choosing a state.
        let (st, c, e) = (self.rows[i].state, self.rows[i].conds, self.rows[i].enemy);
        if (st == state::IDLE || st == state::ALERT) && c & cond::NEW_ENEMY != 0 && e != NONE {
            self.share_enemy(i, e);
        }
        let r = &mut self.rows[i];
        match r.state {
            state::IDLE => {
                if c & cond::NEW_ENEMY != 0 {
                    r.ideal = state::COMBAT;
                } else if c & (cond::LIGHT_DAMAGE | cond::HEAVY_DAMAGE | cond::HEAR_SOUND | cond::SMELL | cond::SMELL_FOOD) != 0 {
                    r.ideal = state::ALERT;
                }
            }
            state::ALERT => {
                if c & (cond::NEW_ENEMY | cond::SEE_ENEMY) != 0 {
                    r.ideal = state::COMBAT;
                } else if c & cond::HEAR_SOUND != 0 {
                    r.ideal = state::ALERT;
                }
            }
            state::COMBAT => {
                if r.enemy == NONE {
                    r.ideal = state::ALERT;
                }
            }
            state::SCRIPT => {
                if c & (cond::TASK_FAILED | cond::LIGHT_DAMAGE | cond::HEAVY_DAMAGE) != 0 {
                    r.ideal = state::IDLE;
                }
            }
            state::DEAD => r.ideal = state::DEAD,
            _ => {}
        }
    }

    /// Run one think of row `i`. `sense` is false for a monster whose sensing is gated (idle or alert
    /// outside the player's view): its conditions are left as they are.
    #[optimize(size)]
    #[inline(never)]
    pub fn think<W: World>(&mut self, w: &mut W, t: &Tables, i: usize, sense: bool) {
        let p = self.rows[i].prop;
        if p == NONE || self.rows[i].state == state::DEAD {
            return;
        }
        if sense {
            let sounds = t.sched(self.rows[i].sched).sound;
            let (enemy, conds) = w.look(p, self.rows[i].enemy, sounds);
            self.rows[i].enemy = enemy;
            self.rows[i].conds = (self.rows[i].conds & !LOOK_OWNED) | conds;
        }
        for _ in 0..MAX_STEPS {
            let r = self.rows[i];
            let s = t.sched(r.sched);
            if r.sched != 0 && r.tstatus == COMPLETE {
                self.rows[i].tstatus = NEW;
                self.rows[i].idx += 1;
                if self.rows[i].idx >= s.count {
                    self.rows[i].conds |= cond::SCHEDULE_DONE;
                }
            }
            let r = self.rows[i];
            let valid = r.sched != 0
                && r.conds & (s.interrupt | cond::SCHEDULE_DONE | cond::TASK_FAILED) == 0;
            if !valid || r.state != r.ideal {
                // `ScheduleChange`: the monster lets go of its squad slot before the new schedule picks one.
                self.vacate(i);
                let c = r.conds;
                if (c != 0 && c & cond::SCHEDULE_DONE == 0)
                    || s.interrupt & cond::SCHEDULE_DONE != 0
                    || (r.state == state::COMBAT && r.enemy == NONE)
                {
                    self.ideal_state(i);
                }
                let r = self.rows[i];
                let id = if r.conds & cond::TASK_FAILED != 0 && r.state == r.ideal {
                    let kind = if r.fail != 0 { r.fail } else { ty::FAIL };
                    self.of_type(w, t, i, kind)
                } else {
                    self.rows[i].state = self.rows[i].ideal;
                    let kind = self.pick(w, t, i);
                    self.of_type(w, t, i, kind)
                };
                self.change_schedule(i, id);
            }
            let s = t.sched(self.rows[i].sched);
            if self.rows[i].tstatus == NEW && self.rows[i].idx < s.count {
                self.rows[i].tstatus = RUNNING;
                let (o, a) = t.task(&s, self.rows[i].idx);
                self.start_task(w, t, i, o, a);
            } else if self.rows[i].idx >= s.count {
                self.rows[i].tstatus = COMPLETE;
            }
            let st = self.rows[i].tstatus;
            if st != COMPLETE && st != NEW {
                break;
            }
        }
        let s = t.sched(self.rows[i].sched);
        if self.rows[i].tstatus == RUNNING && self.rows[i].idx < s.count {
            let (o, a) = t.task(&s, self.rows[i].idx);
            self.run_task(w, i, o, a);
        }
        // Damage conditions do not outlive the think that could use them.
        self.rows[i].conds &= !(cond::LIGHT_DAMAGE | cond::HEAVY_DAMAGE);
    }

    #[optimize(size)]
    fn complete(&mut self, i: usize) {
        if self.rows[i].conds & cond::TASK_FAILED == 0 {
            self.rows[i].tstatus = COMPLETE;
        }
    }

    #[optimize(size)]
    fn fail(&mut self, i: usize) {
        self.rows[i].conds |= cond::TASK_FAILED;
    }

    #[optimize(size)]
    #[inline(never)]
    fn start_task<W: World>(&mut self, w: &mut W, t: &Tables, i: usize, o: u8, a: i16) {
        let p = self.rows[i].prop;
        let now = w.now();
        match o {
            op::TURN_LEFT => self.rows[i].yaw = w.turn_by(p, a),
            op::TURN_RIGHT => self.rows[i].yaw = w.turn_by(p, -a),
            op::REMEMBER => {
                self.rows[i].memory |= a as u16;
                self.complete(i);
            }
            op::FORGET => {
                self.rows[i].memory &= !(a as u16);
                self.complete(i);
            }
            op::STORE_LASTPOSITION => {
                self.rows[i].last = w.store_position(p);
                self.complete(i);
            }
            op::CLEAR_LASTPOSITION => {
                self.rows[i].last = [0; 3];
                self.complete(i);
            }
            op::STOP_MOVING => {
                w.stop(p);
                self.complete(i);
            }
            op::PLAY_SEQUENCE | op::PLAY_SEQUENCE_FACE_ENEMY | op::SET_ACTIVITY => {
                w.set_activity(p, a as u8);
                if o == op::SET_ACTIVITY {
                    self.complete(i);
                }
            }
            op::SET_SCHEDULE => {
                let id = self.of_type(w, t, i, a as u8);
                if id != 0 {
                    self.change_schedule(i, id);
                } else {
                    self.fail(i);
                }
            }
            op::SET_FAIL_SCHEDULE => {
                self.rows[i].fail = a as u8;
                self.complete(i);
            }
            op::CLEAR_FAIL_SCHEDULE => {
                self.rows[i].fail = 0;
                self.complete(i);
            }
            op::SUGGEST_STATE => {
                self.rows[i].ideal = a as u8;
                self.complete(i);
            }
            op::WAIT | op::WAIT_FACE_ENEMY => self.rows[i].wait_until = now.wrapping_add(a as u16),
            op::WAIT_RANDOM => self.rows[i].wait_until = now.wrapping_add(2 + w.rand(a.max(1) as u32) as u16),
            op::CLEAR_MOVE_WAIT => self.complete(i),
            op::GET_PATH_TO_LASTPOSITION => {
                self.rows[i].goal = self.rows[i].last;
                self.complete(i);
            }
            op::GET_PATH_TO_ENEMY | op::GET_PATH_TO_ENEMY_LKP | op::GET_PATH_TO_ENEMY_CORPSE
            | op::GET_PATH_TO_LEADER | op::GET_PATH_TO_TARGET | op::GET_PATH_TO_BESTSOUND => {
                let goal = match o {
                    op::GET_PATH_TO_ENEMY => Goal::Enemy,
                    op::GET_PATH_TO_ENEMY_LKP => Goal::EnemyLkp,
                    op::GET_PATH_TO_ENEMY_CORPSE => Goal::EnemyCorpse,
                    op::GET_PATH_TO_LEADER => Goal::Leader,
                    op::GET_PATH_TO_TARGET => Goal::Target,
                    _ => Goal::BestSound,
                };
                let mut out = self.rows[i].goal;
                if (goal == Goal::Enemy && self.rows[i].enemy == NONE) || !w.path_to(p, goal, &mut out) {
                    self.fail(i);
                } else {
                    self.rows[i].goal = out;
                    self.complete(i);
                }
            }
            op::RUN_PATH | op::WALK_PATH => {
                if w.begin_move(p, o == op::RUN_PATH) {
                    self.complete(i);
                } else {
                    self.fail(i);
                }
            }
            op::FIND_COVER_FROM_ENEMY | op::FIND_FAR_NODE_COVER_FROM_ENEMY | op::FIND_NEAR_NODE_COVER_FROM_ENEMY
            | op::FIND_COVER_FROM_BEST_SOUND | op::FIND_COVER_FROM_ORIGIN => {
                let from = match o {
                    op::FIND_COVER_FROM_BEST_SOUND => Cover::Sound,
                    op::FIND_COVER_FROM_ORIGIN => Cover::Origin,
                    _ => Cover::Enemy,
                };
                let (min, max) = match o {
                    op::FIND_FAR_NODE_COVER_FROM_ENEMY => (a as i32, 0),
                    op::FIND_NEAR_NODE_COVER_FROM_ENEMY => (0, a as i32),
                    _ => (0, 0),
                };
                let mut out = self.rows[i].goal;
                let mut others = [[0i16; 3]; 3];
                let mut n = 0;
                let sq = self.rows[i].squad;
                let mut j = 0;
                while sq != 0 && j < N && n < 3 {
                    if j != i && self.rows[j].prop != NONE && self.rows[j].squad == sq {
                        others[n] = w.store_position(self.rows[j].prop);
                        n += 1;
                    }
                    j += 1;
                }
                if !w.find_cover(p, from, min, max, &others[..n], &mut out) {
                    self.fail(i);
                } else {
                    self.rows[i].goal = out;
                    self.complete(i);
                }
            }
            op::FACE_ENEMY | op::FACE_IDEAL | op::FACE_TARGET | op::FACE_ROUTE | op::FACE_LASTPOSITION => {}
            op::RANGE_ATTACK1 | op::RANGE_ATTACK2 | op::MELEE_ATTACK1 | op::MELEE_ATTACK2
            | op::SPECIAL_ATTACK1 | op::SPECIAL_ATTACK2 | op::RELOAD | op::SMALL_FLINCH => {
                w.attack(p, o);
            }
            op::SOUND => self.complete(i),
            op::DIE => w.die(p),
            op::FAIL => self.fail(i),
            op::CUSTOM => {
                let (res, set) = w.custom(p, a, true);
                self.rows[i].conds |= set;
                match res {
                    Task::Complete => self.complete(i),
                    Task::Failed => self.fail(i),
                    Task::Running => {}
                }
            }
            _ => {}
        }
    }

    #[optimize(size)]
    #[inline(never)]
    fn run_task<W: World>(&mut self, w: &mut W, i: usize, o: u8, a: i16) {
        let p = self.rows[i].prop;
        let now = w.now();
        let waited = |r: &Row| now.wrapping_sub(r.wait_until) < 0x8000;
        match o {
            op::TURN_LEFT | op::TURN_RIGHT | op::FACE_IDEAL | op::FACE_TARGET | op::FACE_ROUTE
            | op::FACE_LASTPOSITION => {
                let mut yaw = self.rows[i].yaw;
                if w.face(p, false, &mut yaw) {
                    self.complete(i);
                }
            }
            op::FACE_ENEMY => {
                let mut yaw = self.rows[i].yaw;
                let done = w.face(p, true, &mut yaw);
                self.rows[i].yaw = yaw;
                if done {
                    self.complete(i);
                }
            }
            op::PLAY_SEQUENCE | op::SMALL_FLINCH => {
                if w.seq_done(p) {
                    self.complete(i);
                }
            }
            op::PLAY_SEQUENCE_FACE_ENEMY => {
                let mut yaw = self.rows[i].yaw;
                w.face(p, true, &mut yaw);
                self.rows[i].yaw = yaw;
                if w.seq_done(p) {
                    self.complete(i);
                }
            }
            op::WAIT | op::WAIT_RANDOM => {
                if waited(&self.rows[i]) {
                    self.complete(i);
                }
            }
            op::WAIT_FACE_ENEMY => {
                let mut yaw = self.rows[i].yaw;
                w.face(p, true, &mut yaw);
                self.rows[i].yaw = yaw;
                if waited(&self.rows[i]) {
                    self.complete(i);
                }
            }
            op::WAIT_FOR_MOVEMENT => match w.move_tick(p, self.rows[i].goal) {
                Move::Done => self.complete(i),
                Move::Failed => self.fail(i),
                Move::Moving => {}
            },
            op::RANGE_ATTACK1 | op::RANGE_ATTACK2 | op::MELEE_ATTACK1 | op::MELEE_ATTACK2
            | op::SPECIAL_ATTACK1 | op::SPECIAL_ATTACK2 | op::RELOAD => {
                let mut yaw = self.rows[i].yaw;
                w.face(p, true, &mut yaw);
                self.rows[i].yaw = yaw;
                if w.seq_done(p) {
                    self.complete(i);
                }
            }
            op::WAIT_PVS => self.complete(i),
            op::CUSTOM => {
                let (res, set) = w.custom(p, a, false);
                self.rows[i].conds |= set;
                match res {
                    Task::Complete => self.complete(i),
                    Task::Failed => self.fail(i),
                    Task::Running => {}
                }
            }
            _ => {}
        }
    }

    /// Schedule `id`'s header, for tests and tools.
    #[optimize(size)]
    pub fn sched_of(t: &Tables, id: u8) -> Sched {
        t.sched(id)
    }

    /// Species parameters of a row.
    #[optimize(size)]
    pub fn species_of(t: &Tables, i: &Row) -> Option<Species> {
        t.species(i.species)
    }

    /// Memory bit test.
    #[optimize(size)]
    pub fn remembers(&self, i: usize, bit: u16) -> bool {
        self.rows[i].memory & bit != 0
    }
}

/// Memory bits for callers (re-exported for convenience).
pub const MEM_IN_COVER: u16 = mem::IN_COVER;

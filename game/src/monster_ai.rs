//! Guest side of the schedule runtime (`ai`): the world the HECU grunts' rows act in.
//!
//! The runtime decides (state, schedule, task); this module senses for it, moves the actor, plays
//! the activity and delivers the attack events. A think runs every second tick per monster (the
//! retail 10 Hz); everything between thinks (sequence timers, attack events) runs every tick.
//! Rows are claimed when the map loads (placed grunts) or when a carried or made grunt first ticks.

use crate::ai::{Ai, Cover, Goal, Move, Task, World, LOOK_OWNED, NONE};
use crate::*;
use hl_format::ai::{cond, custom, snd, Tables};

const ROWS: usize = 32;
/// The species id of the HECU grunt in the tables (its prop kind).
const KIND: u8 = 8;
/// The player as an enemy in a row (`NONE` is 0xFF there, the prop table has two sentinels).
const PLAYER_ROW: u8 = 0xFE;
/// `bits_COND_*` look distances and the squad recruit radius, in map units.
const RANGE: i32 = 2048;
const SQUAD_RADIUS2: i32 = 1024 * 1024;
const FRIEND_NEAR: i32 = 128;
/// Retail `FacingIdeal` tolerance (10 degrees) in q12 yaw.
const FACING: i32 = 114;
/// Ticks a cover search start moves on (`m_iLastCoverSearch`).
static mut AI: Ai<ROWS> = Ai::new();
static mut TABLES: Option<Tables> = None;
static mut COVER_START: u8 = 0;
/// Rounds left in each grunt's weapon (`m_cAmmoLoaded`).
static mut AMMO: [u8; MAX_PROPS] = [0; MAX_PROPS];

/// Route flags.
const R_LAST: u8 = 1; // `wp` is the goal
const R_AFTER: u8 = 2; // `after` is the waypoint to resume at once the detour apex is reached
const R_AFTER_LAST: u8 = 4; // ... and it is the goal
const R_RUN: u8 = 8;
const R_BUILT: u8 = 16; // a route to the row's goal exists (made by a cover search or a path task)
const R_ENEMY: u8 = 32; // the goal is the enemy: it does not block the walk

/// A monster's route. Retail keeps up to eight waypoints; this keeps the node chain they are cut
/// from and cuts the next one when the walker gets there (the same corner cuts, one at a time).
#[derive(Clone, Copy)]
struct Route {
    /// The waypoint being walked to.
    wp: [i16; 3],
    /// Where the leg began: the last kept point (`RouteSimplify`'s `vecStart`).
    start: [i16; 3],
    after: [i16; 3],
    /// The route's end point (the row's goal it was built for).
    goal: [i16; 3],
    /// Next raw node of the chain not yet cut or kept (`NAV_NODE_NONE` once only the goal is left).
    cur: u8,
    dst: u8,
    flags: u8,
}

impl Route {
    const EMPTY: Route = Route { wp: [0; 3], start: [0; 3], after: [0; 3], goal: [0; 3], cur: NAV_NODE_NONE, dst: NAV_NODE_NONE, flags: 0 };
}

static mut ROUTES: [Route; ROWS] = [Route::EMPTY; ROWS];

#[inline]
unsafe fn routes() -> &'static mut [Route; ROWS] {
    &mut *core::ptr::addr_of_mut!(ROUTES)
}

/// `CheckLocalMove`: the walk is clear, or fails after `dist` units (`tri`: a detour may be tried).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Local {
    Valid,
    Blocked { dist: i32, tri: bool },
}

/// One move think.
enum Walked {
    Arrived,
    Failed,
    Moving(bool),
}

/// `(retail activity, prop state, ticks until the sequence reads finished)`. The ticks are the model's
/// sequence length rounded up plus the two it takes the retail think to notice (`hl-aicmp mdlseqs`).
const ACTS: [(u8, u8, u8); 16] = [
    (1, PROP_STATE_IDLE, 82),
    (3, PROP_STATE_MOVE, 58),
    (4, PROP_STATE_MOVE, 14),
    (15, PROP_STATE_IDLE, 22),
    (16, PROP_STATE_IDLE, 22),
    (17, PROP_STATE_ATTACK, 6),
    (21, PROP_STATE_IDLE, 16),
    (22, PROP_STATE_IDLE, 17),
    (25, PROP_STATE_IDLE, 45),
    (26, PROP_STATE_IDLE, 14),
    (28, PROP_STATE_ATTACK, ATTACK_TICKS),
    (30, PROP_STATE_ATTACK, 20),
    (32, PROP_STATE_IDLE, RELOAD_TICKS),
    (47, PROP_STATE_ATTACK, 16),
    (60, PROP_STATE_ATTACK, 23),
    (65, PROP_STATE_IDLE, 14),
];
/// Ticks of the burst sequence (`crouching_mp5`); the rounds go out at ticks 0, 2 and 4 of it.
const ATTACK_TICKS: u8 = 14;
/// The sequence timer: ticks left in the low six bits, what the sequence delivers in the top two.
const SEQ_LEFT: u8 = 63;
const SEQ_BURST: u8 = 0x40;
const SEQ_KICK: u8 = 0x80;
/// Rounds in a full magazine: the MP5 36, the shotgun 8.
unsafe fn clip(pi: usize) -> u8 {
    let weapons = hecu_grenades::weapons(GRUNT_GREN[pi]);
    if weapons & 1 == 0 && weapons & 8 != 0 {
        8
    } else {
        36
    }
}

/// A walker is at its goal within this many units.
const REACHED: i32 = 32;
/// Units a walker covers on each of its move ticks (every second tick): the retail run covers 30.4
/// units a think and the walk about 60 percent of that.
const RUN_STEP: i32 = 30;
const WALK_STEP: i32 = 18;
/// Failed steps (move ticks) before a route is given up.
const STUCK: u8 = 12;
/// `MONSTER_CUT_CORNER_DIST`: a waypoint this close counts as reached.
const CUT_CORNER: i32 = 8;
/// `DIST_TO_CHECK`: how far ahead each move checks its way.
const DIST_TO_CHECK: i32 = 200;
/// `CheckLocalMove` fails when the walk ends more than this far from the goal's height.
const LAND_MAX: i32 = 64;
/// The kick lands at tick 8 of `frontkick`.
const KICK_TICK: u8 = 8;
/// `reload_mp5` is 35 ticks long and refills the weapon at tick 24 (`HGRUNT_AE_RELOAD`).
const RELOAD_TICKS: u8 = 37;
const RELOAD_TICK: u8 = 24;
const SEQ_RELOAD: u8 = SEQ_BURST | SEQ_KICK;

#[inline]
fn row_enemy(target: u8) -> u8 {
    match target {
        PROP_TARGET_NONE => NONE,
        PROP_TARGET_PLAYER => PLAYER_ROW,
        t => t,
    }
}

#[inline]
fn target_of(row: u8) -> u8 {
    match row {
        NONE => PROP_TARGET_NONE,
        PLAYER_ROW => PROP_TARGET_PLAYER,
        r => r,
    }
}

#[inline]
unsafe fn ai() -> &'static mut Ai<ROWS> {
    &mut *core::ptr::addr_of_mut!(AI)
}

/// Start of a map: forget every row, load the tables, give each placed grunt a row and form squads
/// the way `StartMonster` does (in a random order, each unsquadded grunt recruits up to three
/// others within 1,024 units that it has a line to).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn reset(m: &Map, nprops: usize) {
    *ai() = Ai::new();
    TABLES = m.ai_tables();
    COVER_START = 0;
    *routes() = [Route::EMPTY; ROWS];
    if TABLES.is_none() {
        return;
    }
    let mut pi = 0usize;
    while pi < nprops {
        if PROP_KIND[pi] == KIND && PROP_HEALTH[pi] > 0 && PROP_DORMANT[pi] & PROP_RUNTIME_PRISONER == 0 {
            ai().claim(pi as u8, KIND, grenade_flags(pi));
            AMMO[pi] = clip(pi);
        }
        pi += 1;
    }
    form_groups();
    // The rest recruit by radius, in the random order the retail start-up thinks run in.
    let mut order = [0u8; ROWS];
    let mut n = 0usize;
    for i in 0..ROWS {
        let named = (0..TABLES.map_or(0, |t| t.n_group())).any(|g| TABLES.is_some_and(|t| t.group(g).0 == ai().rows[i].prop));
        if ai().rows[i].prop != NONE && ai().rows[i].squad == 0 && !named {
            order[n] = i as u8;
            n += 1;
        }
    }
    let mut k = n;
    while k > 1 {
        let j = impact_rng().below(k as u32) as usize;
        k -= 1;
        order.swap(k, j);
    }
    for k in 0..n {
        form_squad(m, order[k] as usize);
    }
}

/// Squads named in the map (`netname`): the flagged leader and every member of the group, at most four.
/// `StartMonster` lets only the leader recruit, and it takes the whole group.
#[inline(never)]
#[optimize(size)]
unsafe fn form_groups() {
    let Some(t) = TABLES else {
        return;
    };
    for g in 0..t.n_group() {
        let (prop, group, flags) = t.group(g);
        if flags & hl_format::ai::layout::GROUP_LEADER == 0 {
            continue;
        }
        let Some(lead) = ai().row_of(prop) else {
            continue;
        };
        let mut id = 1;
        while id < 8 && ai().squads.leader[id] != NONE {
            id += 1;
        }
        if id == 8 {
            return;
        }
        ai().squads.leader[id] = lead as u8;
        ai().rows[lead].squad = id as u8;
        let mut size = 1;
        for h in 0..t.n_group() {
            let (member, mg, _) = t.group(h);
            if mg == group && size < 4 {
                if let Some(r) = ai().row_of(member) {
                    if ai().rows[r].squad == 0 {
                        ai().rows[r].squad = id as u8;
                        size += 1;
                    }
                }
            }
        }
        if size == 1 {
            ai().leave_squad(lead);
        }
    }
}

unsafe fn grenade_flags(pi: usize) -> u8 {
    if hecu_grenades::weapons(GRUNT_GREN[pi]) & hecu_grenades::LAUNCHER != 0 {
        ai::F_LAUNCHER | ai::F_FIRST_ENCOUNTER
    } else {
        ai::F_FIRST_ENCOUNTER
    }
}

#[inline(never)]
#[optimize(size)]
unsafe fn form_squad(m: &Map, i: usize) {
    if ai().rows[i].squad != 0 {
        return;
    }
    let rows = &ai().rows;
    let me = rows[i].prop as usize;
    let eye = [PROP_POS[me][0], PROP_POS[me][1] + 28, PROP_POS[me][2]];
    let near = |a: u8, b: u8| -> i32 {
        let (pa, pb) = (PROP_POS[a as usize], PROP_POS[b as usize]);
        let d2 = dist2_3(pa, pb);
        if d2 < SQUAD_RADIUS2 && phys::line_clear_world(m, eye, [pb[0], pb[1] + 28, pb[2]]) {
            d2
        } else {
            i32::MAX
        }
    };
    ai().recruit(i, 4, SQUAD_RADIUS2, near);
}

/// The row of prop `pi`, claiming one (and squadding it) for a grunt that arrived after the map load.
#[inline(never)]
#[optimize(size)]
unsafe fn row_of(m: &Map, pi: usize) -> Option<usize> {
    if let Some(i) = ai().row_of(pi as u8) {
        return Some(i);
    }
    let i = ai().claim(pi as u8, KIND, grenade_flags(pi))?;
    AMMO[pi] = clip(pi);
    form_squad(m, i);
    Some(i)
}

/// A grunt died (or was removed): free its row and leave its squad.
pub(crate) unsafe fn died(pi: usize) {
    if let Some(i) = ai().row_of(pi as u8) {
        ai().release(i);
    }
}

/// A grunt took damage: the conditions the schedules interrupt on.
pub(crate) unsafe fn damaged(pi: usize, dmg: u8) {
    if let Some(i) = ai().row_of(pi as u8) {
        let r = &mut ai().rows[i];
        r.conds |= cond::LIGHT_DAMAGE;
        if dmg >= 20 {
            r.conds |= cond::HEAVY_DAMAGE;
        }
    }
}

/// One tick of a HECU grunt. `pm` are the movers near it (movement), `sight` all of them (traces).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick(
    m: &Map,
    pm: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    if advance_grunt_grenade(pi) {
        return;
    }
    let Some(t) = TABLES else {
        return;
    };
    let Some(i) = row_of(m, pi) else {
        return;
    };
    let mut w = Guest { m, pm, sight, player: player_pos, health, armor, nprops, t };
    if PROP_STATE[pi] == PROP_STATE_MOVE && (pi as u16 ^ MOVE_TICK) & 1 == 0 {
        // The walk, once per retail think: check the way ahead, then step toward the waypoint.
        match w.walk(pi, i) {
            Walked::Arrived => {
                PROP_STATE[pi] = PROP_STATE_IDLE;
                PROP_AI_TIMER[pi] = 0;
                routes()[i].flags &= !R_BUILT;
            }
            Walked::Failed => {
                PROP_AI_TIMER[pi] = STUCK;
                routes()[i].flags &= !R_BUILT;
            }
            Walked::Moving(true) => PROP_AI_TIMER[pi] = 0,
            Walked::Moving(false) => PROP_AI_TIMER[pi] = PROP_AI_TIMER[pi].saturating_add(1),
        }
    }
    let timer = PROP_AI_TIMER[pi];
    if PROP_STATE[pi] != PROP_STATE_MOVE && timer & SEQ_LEFT != 0 {
        PROP_AI_TIMER[pi] = timer - 1;
        w.events(pi, timer);
        if timer & SEQ_LEFT == 1 {
            PROP_AI_TIMER[pi] = 0;
        }
    }
    if (pi as u32).wrapping_add(AI_TICK) & 1 == 0 {
        ai().think(&mut w, &t, i, true);
    }
}

struct Guest<'a> {
    m: &'a Map,
    pm: &'a [phys::Mover],
    sight: &'a [phys::Mover],
    player: [i32; 3],
    health: &'a mut u16,
    armor: &'a mut u16,
    nprops: usize,
    t: Tables,
}

fn pos16(p: [i32; 3]) -> [i16; 3] {
    [p[0] as i16, p[1] as i16, p[2] as i16]
}

fn pos32(p: [i16; 3]) -> [i32; 3] {
    [p[0] as i32, p[1] as i32, p[2] as i32]
}

impl Guest<'_> {
    /// Attack events at their tick of the running sequence (`timer` is the value before this tick).
    #[inline(never)]
    #[optimize(size)]
    unsafe fn events(&mut self, pi: usize, timer: u8) {
        let left = (timer & SEQ_LEFT) - 1;
        if timer & SEQ_RELOAD == SEQ_RELOAD {
            if RELOAD_TICKS - left == RELOAD_TICK {
                AMMO[pi] = clip(pi);
            }
        } else if timer & SEQ_BURST != 0 {
            let elapsed = ATTACK_TICKS - left;
            // The burst sequence loops: its first frame's round goes out again at tick 12.
            if elapsed == 2 || elapsed == 4 || elapsed == 12 {
                self.shoot(pi, false);
            }
        } else if timer & SEQ_KICK != 0 && left == 20 - KICK_TICK {
            let target = PROP_AI_TARGET[pi];
            if let Some(aim) = target_aim_point(target, self.player, self.nprops) {
                if dist2_xz(PROP_POS[pi], aim) <= 100 * 100 {
                    damage_target(target, skill_hit(Sk::HgruntKick), PROP_POS[pi], self.health, self.armor);
                }
            }
        }
    }

    /// One round (MP5) or one blast (shotgun) at the enemy; `first` plays the burst's sound.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn shoot(&mut self, pi: usize, first: bool) {
        let target = PROP_AI_TARGET[pi];
        let Some(aim) = target_aim_point(target, self.player, self.nprops) else {
            return;
        };
        let pos = PROP_POS[pi];
        let from = prop_eye(self.m, pi);
        let weapons = hecu_grenades::weapons(GRUNT_GREN[pi]);
        AMMO[pi] = AMMO[pi].saturating_sub(1);
        if weapons & 1 == 0 && weapons & 8 != 0 {
            // A shotgun marine: one blast of `sk_hgrunt_pellets` pellets.
            if first {
                let mut hits = 0u8;
                let mut n = skill_value(Sk::HgruntPellets);
                while n > 0 {
                    if monster_shot_lands(self.m, from, aim, target, self.player, self.nprops, 535).is_some() {
                        hits += 1;
                    }
                    n -= 1;
                }
                if hits > 0 {
                    damage_target(target, hits.saturating_mul(skill_hit(Sk::Bullet9mm)), pos, self.health, self.armor);
                }
                push_tracer(from, aim);
                sfx::play_world(sfx::SHOTGUN, pos);
                make_noise(pos, senses::VOL_MONSTER_BURST);
            }
            return;
        }
        if monster_shot_lands(self.m, from, aim, target, self.player, self.nprops, 711).is_some() {
            damage_target(target, skill_hit(Sk::Bullet9mmAr), pos, self.health, self.armor);
            push_tracer(from, aim);
        } else {
            let off = [
                impact_rng().signed(60) as i32,
                impact_rng().signed(40) as i32,
                impact_rng().signed(60) as i32,
            ];
            push_tracer(from, [aim[0] + off[0], aim[1] + off[1], aim[2] + off[2]]);
        }
        if first {
            species::cue_or(hl_format::setpiece_audio::HG_MGUN, sfx::MP5, pos);
            make_noise(pos, senses::VOL_MONSTER_BURST);
        }
    }

    /// Some other live grunt stands in the line of fire from `from` to `aim`.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn friend_in_line(&self, pi: usize, from: [i32; 3], aim: [i32; 3]) -> bool {
        let seg = [aim[0] - from[0], aim[2] - from[2]];
        let len2 = seg[0] * seg[0] + seg[1] * seg[1];
        let len = isqrt_i32(len2).max(1);
        let mut o = 0usize;
        while o < self.nprops {
            if o != pi && PROP_KIND[o] == KIND && PROP_ACTIVE[o] != 0 && PROP_HEALTH[o] > 0 {
                let w = [PROP_POS[o][0] - from[0], PROP_POS[o][2] - from[2]];
                let along = w[0] * seg[0] + w[1] * seg[1];
                let cross = w[0] * seg[1] - w[1] * seg[0];
                if along > 0 && along < len2 && cross.abs() < 36 * len {
                    return true;
                }
            }
            o += 1;
        }
        false
    }

    /// Enemy position for facing and paths: where it is seen, else the last place it was.
    unsafe fn enemy_point(&self, pi: usize) -> Option<[i32; 3]> {
        let target = PROP_AI_TARGET[pi];
        if target == PROP_TARGET_NONE {
            return None;
        }
        let truth = target_aim_point(target, self.player, self.nprops)?;
        if prop_ai_target_visible(pi) || PROP_LKP[pi][0] == i16::MIN {
            Some(truth)
        } else {
            Some([PROP_LKP[pi][0] as i32, PROP_POS[pi][1], PROP_LKP[pi][1] as i32])
        }
    }

    /// The grenade condition: the check is due, the monster stands still, and a throw is possible.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn grenade_ok(&mut self, pi: usize, aim: [i32; 3]) -> bool {
        if PROP_STATE[pi] == PROP_STATE_MOVE
            || PROP_DORMANT[pi] & PROP_RUNTIME_AIRBORNE != 0
            || !hecu_grenades::due(GRUNT_GREN[pi], SIM_NOW)
        {
            return false;
        }
        GRUNT_GREN[pi] = hecu_grenades::reschedule(GRUNT_GREN[pi], SIM_NOW, hecu_grenades::CHECK_TICKS);
        let Some(attack) = hecu_grenades::attack(hecu_grenades::weapons(GRUNT_GREN[pi])) else {
            return false;
        };
        let launcher = attack == hecu_grenades::Attack::Launcher;
        let Some(plan) = grunt_grenade_plan(self.m, self.sight, pi, aim, launcher, self.nprops) else {
            return false;
        };
        PROP_SCRIPT_GOAL[pi] = plan.velocity_q4;
        true
    }

    /// Retail `FValidateCover`: when in a squad, no member (the monster itself included) within 128
    /// units of the point on the ground plane.
    unsafe fn valid_cover(pi: usize, point: [i32; 3], others: &[[i16; 3]]) -> bool {
        others.is_empty()
            || (dist2_xz(PROP_POS[pi], point) > FRIEND_NEAR * FRIEND_NEAR
                && others.iter().all(|o| dist2_xz(pos32(*o), point) > FRIEND_NEAR * FRIEND_NEAR))
    }

    /// Retail `CheckLocalMove` from `a` to `b`: the human hull walks the chord (world, doors and
    /// live actors; the player too unless the goal is the enemy) and ends within 64 units of `b`'s
    /// height. Moves between levels with no ramp between them fail the height rule.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn local_move(&self, pi: usize, a: [i32; 3], b: [i32; 3], actors: bool) -> Local {
        let floor = match prop_floor_y(self.m, pi, b) {
            Some(f) if (f - b[1]).abs() <= LAND_MAX => Some(f),
            _ => None,
        };
        let end = [b[0], floor.unwrap_or(b[1]), b[2]];
        let world = actor_chord_blocked(self.m, self.pm, pi, a, end);
        let other = if actors {
            script_human_actor_blocked_fraction(pi, a, end, Some((self.player, LOGIC_PLAYER_HALF_HEIGHT)))
        } else {
            None
        };
        let frac = match (world, other) {
            (Some(w), Some(o)) => Some(w.min(o)),
            (w, o) => w.or(o),
        };
        if let Some(f) = frac {
            // The walk reports where its failed 16-unit step began.
            let len = isqrt_i32(dist2_xz(a, end));
            return Local::Blocked { dist: ((len * f.clamp(0, 4096)) >> 12) & !15, tri: true };
        }
        if floor.is_none() {
            return Local::Blocked { dist: 0, tri: false };
        }
        Local::Valid
    }

    /// Retail `FTriangulate` around a blocker `dist` units along the way from `a` to `b`.
    unsafe fn detour(&self, pi: usize, a: [i32; 3], b: [i32; 3], dist: i32, actors: bool) -> Option<[i32; 3]> {
        let player = if actors { Some((self.player, LOGIC_PLAYER_HALF_HEIGHT)) } else { None };
        script_local_detour(self.m, self.pm, pi, 0, a, b, dist, player)
    }

    /// Length of the node route from `from` to `to` (a large number when there is none).
    unsafe fn route_len(&self, from: u8, to: usize) -> i32 {
        let n = nav_node_count(self.m);
        let (mut cur, mut sum, mut hops) = (from as usize, 0i32, 0);
        while cur != to {
            let next = self.m.nav_route_next(cur, to);
            if hops == 64 || next == cur || next >= n {
                return i32::MAX / 2;
            }
            sum += isqrt_i32(dist2_3(self.m.nav_node(cur).pos, self.m.nav_node(next).pos)) as i32;
            cur = next;
            hops += 1;
        }
        sum
    }

    /// Retail `RouteSimplify`, one waypoint at a time: from the last kept point, drop each node the
    /// next one can be walked to directly, cut the corner at the midpoint when only that can be walked
    /// to, else keep the node. Sets the route's next waypoint.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn cut(&self, pi: usize, r: &mut Route, goal: [i32; 3]) {
        let n = nav_node_count(self.m);
        let start = pos32(r.start);
        let actors = r.flags & R_ENEMY == 0;
        let mut guard = 0;
        while (r.cur as usize) < n && guard < 8 {
            guard += 1;
            let here = r.cur as usize;
            let at = self.m.nav_node(here).pos;
            let next = if r.cur == r.dst { NAV_NODE_NONE } else { self.m.nav_route_next(here, r.dst as usize) as u8 };
            let ahead = if (next as usize) < n && next != r.cur { self.m.nav_node(next as usize).pos } else { goal };
            let next = if ahead == goal { NAV_NODE_NONE } else { next };
            r.cur = next;
            if self.local_move(pi, start, ahead, actors) == Local::Valid {
                continue;
            }
            let mid = [(at[0] + ahead[0]) / 2, (at[1] + ahead[1]) / 2, (at[2] + ahead[2]) / 2];
            r.wp = if self.local_move(pi, start, mid, actors) == Local::Valid { pos16(mid) } else { pos16(at) };
            r.flags &= !R_LAST;
            return;
        }
        r.wp = pos16(goal);
        r.flags |= R_LAST;
    }

    /// Retail `BuildRoute` to `goal` for row `i`: walk straight there, else around the blocker, else
    /// along the node graph. False when there is no way.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn build_route(&self, pi: usize, i: usize, goal: [i32; 3], enemy: bool) -> bool {
        let pos = PROP_POS[pi];
        let keep = routes()[i].flags & R_RUN;
        let r = &mut routes()[i];
        *r = Route::EMPTY;
        r.flags = keep | R_BUILT | if enemy { R_ENEMY } else { 0 };
        r.start = pos16(pos);
        r.wp = pos16(goal);
        r.goal = r.wp;
        match self.local_move(pi, pos, goal, !enemy) {
            Local::Valid => {
                r.flags |= R_LAST;
                return true;
            }
            Local::Blocked { dist, tri: true } => {
                if let Some(apex) = self.detour(pi, pos, goal, dist, !enemy) {
                    r.wp = pos16(apex);
                    r.after = pos16(goal);
                    r.flags |= R_AFTER | R_AFTER_LAST;
                    return true;
                }
            }
            Local::Blocked { .. } => {}
        }
        let src = if nav_node_count(self.m) == 0 { NAV_NODE_NONE } else { nav_nearest(self.m, self.sight, pi, pos, NAV_NEAREST_RANGE2) };
        let dst = if src == NAV_NODE_NONE { NAV_NODE_NONE } else { nav_nearest(self.m, self.sight, pi, goal, NAV_NEAREST_RANGE2) };
        if src == NAV_NODE_NONE || dst == NAV_NODE_NONE || self.route_len(src, dst as usize) >= i32::MAX / 2 {
            r.flags &= !R_BUILT;
            return false;
        }
        r.cur = src;
        r.dst = dst;
        self.cut(pi, r, goal);
        true
    }

    /// Retail `CBaseMonster::Move`: check 200 units of the way ahead (detour or give up when it is
    /// blocked), take the next waypoint when this one is reached, and step toward it.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn walk(&mut self, pi: usize, i: usize) -> Walked {
        let r = &mut routes()[i];
        if r.flags & R_BUILT == 0 {
            return Walked::Failed;
        }
        let goal = pos32(r.goal);
        let actors = r.flags & R_ENEMY == 0;
        let pos = PROP_POS[pi];
        let mut wp = pos32(r.wp);
        let mut dist = isqrt_i32(dist2_xz(pos, wp));
        if dist > 0 {
            let reach = dist.min(DIST_TO_CHECK);
            let (dx, dy, dz) = (wp[0] - pos[0], wp[1] - pos[1], wp[2] - pos[2]);
            let len3 = isqrt_i32(dx * dx + dy * dy + dz * dz).max(1);
            let end = [pos[0] + dx * reach / len3, pos[1] + dy * reach / len3, pos[2] + dz * reach / len3];
            if let Local::Blocked { dist: blocked, .. } = self.local_move(pi, pos, end, actors) {
                let Some(apex) = self.detour(pi, pos, wp, blocked, actors) else {
                    return Walked::Failed;
                };
                // InsertWaypoint: the detour apex first, then what the route had.
                r.after = r.wp;
                r.flags = (r.flags & !(R_AFTER_LAST | R_LAST)) | R_AFTER | if r.flags & R_LAST != 0 { R_AFTER_LAST } else { 0 };
                r.wp = pos16(apex);
                wp = apex;
                dist = isqrt_i32(dist2_xz(pos, wp));
            }
        }
        if dist <= CUT_CORNER {
            if r.flags & R_LAST != 0 {
                return Walked::Arrived;
            }
            r.start = r.wp;
            if r.flags & R_AFTER != 0 {
                r.wp = r.after;
                r.flags = (r.flags & !(R_AFTER | R_AFTER_LAST)) | if r.flags & R_AFTER_LAST != 0 { R_LAST } else { 0 };
            } else {
                self.cut(pi, r, goal);
            }
            wp = pos32(r.wp);
        }
        prop_face_point(pi, wp);
        let step = if r.flags & R_RUN != 0 { RUN_STEP } else { WALK_STEP };
        let moved = prop_try_step(self.m, self.pm, pi, wp[0] - pos[0], wp[2] - pos[2], step, false);
        Walked::Moving(moved)
    }

    /// Retail `FindLateralCover`: five points 48 units apart to either side (left first) that the
    /// threat cannot see, are valid cover and can be walked to.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn lateral_cover(&self, pi: usize, i: usize, eyes: [i32; 3], others: &[[i16; 3]]) -> Option<[i32; 3]> {
        let pos = PROP_POS[pi];
        let yaw = prop_yaw_value(PROP_YAW[pi]);
        let (sx, sz) = (sincos::sin_q12((yaw + 1024) & 0x0fff), -sincos::sin_q12(yaw));
        let view = prop_eye(self.m, pi)[1] - pos[1];
        let mut n = 1;
        while n <= 5 {
            let mut side = 0;
            while side < 2 {
                let k = if side == 0 { -n * 48 } else { n * 48 };
                let test = [pos[0] + sx * k / 4096, pos[1], pos[2] + sz * k / 4096];
                let from = [test[0], test[1] + view, test[2]];
                if !actor_line_clear(self.m, self.sight, from, eyes)
                    && Self::valid_cover(pi, test, others)
                    && self.local_move(pi, pos, test, true) == Local::Valid
                {
                    let keep = routes()[i].flags & R_RUN;
                    routes()[i] = Route { wp: pos16(test), start: pos16(pos), goal: pos16(test), flags: keep | R_BUILT | R_LAST, ..Route::EMPTY };
                    return Some(test);
                }
                side += 1;
            }
            n += 1;
        }
        None
    }

    /// Retail `FindCover`: from the round-robin start, the first node in `min..max` of me whose
    /// view to the threat is blocked, that I reach no later than the threat does, that is valid
    /// cover and that a route reaches (nodes on another floor count when the route climbs there).
    #[inline(never)]
    #[optimize(size)]
    unsafe fn node_cover(
        &self,
        pi: usize,
        i: usize,
        threat: [i32; 3],
        eyes: [i32; 3],
        min: i32,
        max: i32,
        others: &[[i16; 3]],
    ) -> Option<[i32; 3]> {
        let n = nav_node_count(self.m);
        if n == 0 {
            return None;
        }
        let max = if max == 0 { 784 } else { max };
        let min = min.min(max / 2);
        let pos = PROP_POS[pi];
        let mine = nav_nearest(self.m, self.sight, pi, pos, NAV_NEAREST_RANGE2);
        let theirs = nav_nearest(self.m, self.sight, pi, threat, NAV_NEAREST_RANGE2);
        if mine == NAV_NODE_NONE {
            return None;
        }
        let theirs = if theirs == NAV_NODE_NONE { mine } else { theirs };
        let view = prop_eye(self.m, pi)[1] - pos[1];
        let mut k = 0usize;
        while k < n {
            let node = (k + COVER_START as usize) % n;
            COVER_START = (node + 1) as u8;
            k += 1;
            if !nav_actor_node(self.m, pi, node) {
                continue;
            }
            let at = self.m.nav_node(node).pos;
            let d = isqrt_i32(dist2_3(pos, at)) as i32;
            if d < min || d >= max {
                continue;
            }
            if actor_line_clear(self.m, self.sight, [at[0], at[1] + view, at[2]], eyes) {
                continue;
            }
            if (mine == theirs || self.route_len(mine, node) <= self.route_len(theirs, node))
                && Self::valid_cover(pi, at, others)
                && self.build_route(pi, i, at, false)
            {
                return Some(at);
            }
        }
        None
    }
}

impl World for Guest<'_> {
    fn now(&self) -> u16 {
        unsafe { SIM_NOW }
    }

    fn rand(&mut self, n: u32) -> u32 {
        unsafe { impact_rng().below(n) }
    }

    fn skill_hard(&self) -> bool {
        settings::skill() == 2
    }

    #[inline(never)]
    #[optimize(size)]
    fn look(&mut self, p: u8, enemy: u8, sounds: u16) -> (u8, u32) {
        unsafe {
            let pi = p as usize;
            let prev = target_of(enemy);
            // An alert from outside the runtime (damage aggro) counts while the row has no enemy.
            let mut cur = PROP_AI_TARGET[pi];
            if prev != PROP_TARGET_NONE || target_aim_point(cur, self.player, self.nprops).is_none() {
                cur = prev;
            }
            PROP_AI_TARGET[pi] = cur;
            let mut c = 0u32;
            if AMMO[pi] == 0 {
                c |= cond::NO_AMMO_LOADED;
            }
            if sounds & snd::COMBAT != 0 && (*core::ptr::addr_of!(SOUNDS)).heard(PROP_POS[pi]).is_some() {
                c |= cond::HEAR_SOUND;
            }
            if cur != PROP_TARGET_NONE && target_aim_point(cur, self.player, self.nprops).is_none() {
                // The enemy died: the schedule picker forgets it.
                return (row_enemy(cur), c | cond::ENEMY_DEAD);
            }
            let (target, visible) = species::acquire_target(self.m, self.sight, pi, self.player, self.nprops, RANGE);
            let aim = if target == PROP_TARGET_NONE {
                None
            } else {
                target_aim_point(target, self.player, self.nprops).and_then(|truth| pursue_point(pi, truth, visible))
            };
            let Some(aim) = aim else {
                PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
                prop_ai_set_target_visible(pi, false);
                return (NONE, c);
            };
            PROP_AI_TARGET[pi] = target;
            if target != prev {
                c |= cond::NEW_ENEMY;
            }
            if !visible {
                return (row_enemy(target), c | cond::ENEMY_OCCLUDED);
            }
            c |= cond::SEE_ENEMY;
            let pos = PROP_POS[pi];
            let yaw = prop_yaw_value(PROP_YAW[pi]);
            let (fx, fz) = (sincos::sin_q12(yaw), sincos::sin_q12((yaw + 1024) & 0x0fff));
            let (dx, dz) = (aim[0] - pos[0], aim[2] - pos[2]);
            let d2 = dx * dx + dz * dz;
            let from = prop_eye(self.m, pi);
            if d2 <= RANGE * RANGE && senses::within_cone(2048, fx, fz, dx, dz) && !self.friend_in_line(pi, from, aim) {
                c |= cond::CAN_RANGE_ATTACK1;
            }
            if let Some(s) = self.t.species(KIND) {
                let melee = s.melee_range as i32;
                if d2 <= melee * melee && senses::within_cone(2867, fx, fz, dx, dz) {
                    c |= cond::CAN_MELEE_ATTACK1;
                }
            }
            if self.grenade_ok(pi, aim) {
                c |= cond::CAN_RANGE_ATTACK2;
            }
            if target == PROP_TARGET_PLAYER {
                // The player looks at the monster: within 15 degrees of his view direction.
                let (px, pz) = (sincos::sin_q12(LOGIC_PLAYER_YAW), sincos::sin_q12((LOGIC_PLAYER_YAW + 1024) & 0x0fff));
                if senses::within_cone(3932, px, pz, pos[0] - self.player[0], pos[2] - self.player[2]) {
                    c |= cond::ENEMY_FACING_ME;
                }
            }
            (row_enemy(target), c)
        }
    }

    fn enemy_is_player(&self, e: u8) -> bool {
        e == PLAYER_ROW
    }

    fn damage_ahead(&self, _p: u8) -> bool {
        false
    }

    fn danger_sound(&self, _p: u8) -> bool {
        false
    }

    fn set_activity(&mut self, p: u8, act: u8) {
        unsafe {
            let pi = p as usize;
            if act == 29 {
                self.toss(pi);
                return;
            }
            let (_, state, ticks) = ACTS.iter().copied().find(|a| a.0 == act).unwrap_or((act, PROP_STATE_IDLE, 0));
            let flags = match act {
                28 => SEQ_BURST,
                30 => SEQ_KICK,
                32 => SEQ_RELOAD,
                _ => 0,
            };
            PROP_STATE[pi] = state;
            PROP_AI_TIMER[pi] = ticks.min(SEQ_LEFT) | flags;
        }
    }

    fn seq_done(&self, p: u8) -> bool {
        unsafe { PROP_AI_TIMER[p as usize] & SEQ_LEFT == 0 }
    }

    fn face(&mut self, p: u8, enemy: bool, ideal: &mut u16) -> bool {
        unsafe {
            let pi = p as usize;
            let pos = PROP_POS[pi];
            if enemy {
                if let Some(at) = self.enemy_point(pi) {
                    if at[0] != pos[0] || at[2] != pos[2] {
                        *ideal = yaw_from_vec(at[0] - pos[0], at[2] - pos[2]);
                    }
                }
            }
            let cur = prop_yaw_value(PROP_YAW[pi]);
            let yaw = turn_toward(cur, *ideal, senses::turn_rate_q12(KIND) * 2);
            PROP_YAW[pi] = prop_with_yaw(PROP_YAW[pi], yaw);
            let diff = (*ideal as i32 - yaw as i32) & 0xfff;
            diff < FACING || diff > 4096 - FACING
        }
    }

    fn turn_by(&mut self, p: u8, deg: i16) -> u16 {
        unsafe {
            let cur = prop_yaw_value(PROP_YAW[p as usize]) as i32;
            ((cur - deg as i32 * 4096 / 360) & 0xfff) as u16
        }
    }

    fn stop(&mut self, p: u8) {
        unsafe {
            let pi = p as usize;
            if PROP_STATE[pi] == PROP_STATE_MOVE {
                PROP_STATE[pi] = PROP_STATE_IDLE;
                PROP_AI_TIMER[pi] = 0;
                if let Some(i) = ai().row_of(p) {
                    routes()[i].flags &= !R_BUILT;
                }
            }
        }
    }

    fn path_to(&mut self, p: u8, goal: Goal, out: &mut [i16; 3]) -> bool {
        unsafe {
            let pi = p as usize;
            let at = match goal {
                Goal::EnemyLkp => {
                    if PROP_LKP[pi][0] == i16::MIN {
                        return false;
                    }
                    [PROP_LKP[pi][0] as i32, PROP_POS[pi][1], PROP_LKP[pi][1] as i32]
                }
                Goal::Enemy | Goal::EnemyCorpse | Goal::Target => {
                    let t = PROP_AI_TARGET[pi];
                    if t == PROP_TARGET_PLAYER {
                        self.player
                    } else if (t as usize) < self.nprops {
                        PROP_POS[t as usize]
                    } else {
                        return false;
                    }
                }
                _ => return false,
            };
            let Some(i) = ai().row_of(p) else {
                return false;
            };
            if !self.build_route(pi, i, at, true) {
                return false;
            }
            *out = pos16(at);
            true
        }
    }

    fn begin_move(&mut self, p: u8, run: bool) -> bool {
        unsafe {
            let pi = p as usize;
            let Some(i) = ai().row_of(p) else {
                return false;
            };
            let goal = ai().rows[i].goal;
            // A path task or cover search made the route; a plain last-position path has none yet.
            if routes()[i].flags & R_BUILT == 0 || routes()[i].goal != goal {
                if !self.build_route(pi, i, pos32(goal), false) {
                    return false;
                }
            }
            let r = &mut routes()[i];
            r.flags = (r.flags & !R_RUN) | if run { R_RUN } else { 0 };
            self.set_activity(p, if run { 4 } else { 3 });
            PROP_AI_TIMER[pi] = 0;
        }
        true
    }

    fn move_tick(&mut self, p: u8, goal: [i16; 3]) -> Move {
        unsafe {
            let pi = p as usize;
            if PROP_STATE[pi] != PROP_STATE_MOVE && dist2_xz(PROP_POS[pi], pos32(goal)) < REACHED * REACHED {
                Move::Done
            } else if PROP_STATE[pi] != PROP_STATE_MOVE {
                Move::Failed
            } else if PROP_AI_TIMER[pi] >= STUCK {
                PROP_STATE[pi] = PROP_STATE_IDLE;
                PROP_AI_TIMER[pi] = 0;
                Move::Failed
            } else {
                Move::Moving
            }
        }
    }

    fn find_cover(&mut self, p: u8, from: Cover, min: i32, max: i32, others: &[[i16; 3]], out: &mut [i16; 3]) -> bool {
        unsafe {
            let pi = p as usize;
            let pos = PROP_POS[pi];
            let target = PROP_AI_TARGET[pi];
            let (threat, eyes) = match from {
                Cover::Enemy if target != PROP_TARGET_NONE => match target_aim_point(target, self.player, self.nprops) {
                    Some(aim) => (target_org(target, self.player, self.nprops).unwrap_or(pos), aim),
                    None => (pos, prop_eye(self.m, pi)),
                },
                Cover::Sound => match (*core::ptr::addr_of!(SOUNDS)).heard(pos) {
                    Some(src) => (src, src),
                    None => (pos, prop_eye(self.m, pi)),
                },
                _ => (pos, prop_eye(self.m, pi)),
            };
            let Some(i) = ai().row_of(p) else {
                return false;
            };
            if min == 0 {
                if let Some(at) = self.lateral_cover(pi, i, eyes, others) {
                    *out = pos16(at);
                    return true;
                }
            }
            match self.node_cover(pi, i, threat, eyes, min, max, others) {
                Some(at) => {
                    *out = pos16(at);
                    true
                }
                None => false,
            }
        }
    }

    fn store_position(&self, p: u8) -> [i16; 3] {
        unsafe { pos16(PROP_POS[p as usize]) }
    }

    fn attack(&mut self, p: u8, task: u8) {
        use hl_format::ai::op;
        unsafe {
            let pi = p as usize;
            match task {
                op::RANGE_ATTACK1 => {
                    self.set_activity(p, 28);
                    self.shoot(pi, true);
                }
                op::MELEE_ATTACK1 => self.set_activity(p, 30),
                op::RELOAD => self.set_activity(p, 32),
                op::RANGE_ATTACK2 => self.set_activity(p, 29),
                _ => PROP_AI_TIMER[pi] = 0,
            }
        }
    }

    fn custom(&mut self, p: u8, id: i16, _first: bool) -> (Task, u32) {
        unsafe {
            let pi = p as usize;
            match id {
                custom::GRUNT_CHECK_FIRE => {
                    let target = PROP_AI_TARGET[pi];
                    let blocked = match target_aim_point(target, self.player, self.nprops) {
                        Some(aim) => self.friend_in_line(pi, prop_eye(self.m, pi), aim),
                        None => false,
                    };
                    (Task::Complete, if blocked { cond::SPECIAL1 } else { 0 })
                }
                _ => (Task::Complete, 0),
            }
        }
    }

    fn die(&mut self, _p: u8) {}
}

impl Guest<'_> {
    /// Begin the grenade toss or launch; the plan was stored by the condition check.
    unsafe fn toss(&mut self, pi: usize) {
        let launcher = hecu_grenades::attack(hecu_grenades::weapons(GRUNT_GREN[pi]))
            == Some(hecu_grenades::Attack::Launcher);
        if PROP_SCRIPT_GOAL[pi] == [0, 0, 0] {
            // No plan from the condition check (a cover schedule asked for the throw): plan now.
            let planned = target_aim_point(PROP_AI_TARGET[pi], self.player, self.nprops)
                .and_then(|aim| grunt_grenade_plan(self.m, self.sight, pi, aim, launcher, self.nprops));
            match planned {
                Some(plan) => PROP_SCRIPT_GOAL[pi] = plan.velocity_q4,
                None => return,
            }
        }
        PROP_STATE[pi] = if launcher { 7 } else { 6 };
        PROP_AI_TIMER[pi] = if launcher {
            hecu_grenades::LAUNCH_ANIMATION_TICKS
        } else {
            hecu_grenades::TOSS_ANIMATION_TICKS
        };
    }
}

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
    (32, PROP_STATE_IDLE, 37),
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
/// A walker is at its goal within this many units.
const REACHED: i32 = 32;
/// Failed steps (move ticks) before a route is given up.
const STUCK: u8 = 12;
/// The kick lands at tick 8 of `frontkick`.
const KICK_TICK: u8 = 8;

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
    if TABLES.is_none() {
        return;
    }
    let mut order = [0u8; ROWS];
    let mut n = 0usize;
    let mut pi = 0usize;
    while pi < nprops && n < ROWS {
        if PROP_KIND[pi] == KIND && PROP_HEALTH[pi] > 0 && PROP_DORMANT[pi] & PROP_RUNTIME_PRISONER == 0 {
            if let Some(i) = ai().claim(pi as u8, KIND, grenade_flags(pi)) {
                order[n] = i as u8;
                n += 1;
            }
        }
        pi += 1;
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
    let timer = PROP_AI_TIMER[pi];
    if PROP_STATE[pi] == PROP_STATE_MOVE {
        // The walk: a step toward the route's goal on the walker's move ticks, the timer counting
        // failed steps. Reaching the goal ends the movement.
        let goal = pos32(ai().rows[i].goal);
        if dist2_xz(PROP_POS[pi], goal) < REACHED * REACHED {
            PROP_STATE[pi] = PROP_STATE_IDLE;
            PROP_AI_TIMER[pi] = 0;
        } else {
            let dest = [goal[0], PROP_POS[pi][1], goal[2]];
            if prop_move_towards_point(m, pm, pi, dest, 15) {
                PROP_AI_TIMER[pi] = 0;
            } else {
                PROP_AI_TIMER[pi] = timer.saturating_add(1);
            }
        }
    } else if timer & SEQ_LEFT != 0 {
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
        if timer & SEQ_BURST != 0 {
            let elapsed = ATTACK_TICKS - left;
            if elapsed == 2 || elapsed == 4 {
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

    /// Retail `FValidateCover`: no other squad member within 128 units of the point.
    fn valid_cover(point: [i32; 3], others: &[[i16; 3]]) -> bool {
        others.iter().all(|o| dist2_3(pos32(*o), point) >= FRIEND_NEAR * FRIEND_NEAR)
    }

    /// Retail `FindLateralCover`: a point up to 224 units to either side that the threat cannot see.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn lateral_cover(&self, pi: usize, eyes: [i32; 3], others: &[[i16; 3]]) -> Option<[i32; 3]> {
        let pos = PROP_POS[pi];
        let yaw = prop_yaw_value(PROP_YAW[pi]);
        let (sx, sz) = (sincos::sin_q12((yaw + 1024) & 0x0fff), -sincos::sin_q12(yaw));
        let view = prop_eye(self.m, pi)[1] - pos[1];
        let mut i = 0;
        while i < 8 {
            let mut side = 0;
            while side < 2 {
                let k = if side == 0 { -i * 32 } else { i * 32 };
                let test = [pos[0] + sx * k / 4096, pos[1], pos[2] + sz * k / 4096];
                let from = [test[0], test[1] + view, test[2]];
                if !actor_line_clear(self.m, self.sight, from, eyes)
                    && Self::valid_cover(test, others)
                    && (i == 0
                        || (actor_line_clear(self.m, self.sight, [pos[0], pos[1] + 8, pos[2]], [test[0], test[1] + 8, test[2]])
                            && prop_floor_y(self.m, pi, test).is_some()))
                {
                    return Some(test);
                }
                side += 1;
            }
            i += 1;
        }
        None
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

    /// Retail `FindCover`: from the round-robin start, the first node in `min..max` of me whose
    /// view to the threat is blocked and that I reach no later than the threat does.
    #[inline(never)]
    #[optimize(size)]
    unsafe fn node_cover(
        &self,
        pi: usize,
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
                && Self::valid_cover(at, others)
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
            ((cur + deg as i32 * 4096 / 360) & 0xfff) as u16
        }
    }

    fn stop(&mut self, p: u8) {
        unsafe {
            let pi = p as usize;
            if PROP_STATE[pi] == PROP_STATE_MOVE {
                PROP_STATE[pi] = PROP_STATE_IDLE;
                PROP_AI_TIMER[pi] = 0;
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
            *out = pos16(at);
            true
        }
    }

    fn begin_move(&mut self, p: u8, run: bool) -> bool {
        self.set_activity(p, if run { 4 } else { 3 });
        unsafe {
            PROP_AI_TIMER[p as usize] = 0;
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
            if min == 0 {
                if let Some(at) = self.lateral_cover(pi, eyes, others) {
                    *out = pos16(at);
                    return true;
                }
            }
            match self.node_cover(pi, threat, eyes, min, max, others) {
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

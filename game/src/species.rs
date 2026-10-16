//! Combat of the individual monster species, written from measurements of the
//! retail game under the Xash3D reference (Medium skill, 20 Hz ticks):
//! the HECU grunt's MP5 bursts with distance-dependent accuracy, and the
//! vortigaunt's long zap cycle and claws.

use crate::*;
use hl_format::setpiece_audio as SP;

/// A cue from the map's monster sound bank (silent when the bank lacks it).
#[inline(always)]
unsafe fn cue(slot: u8, pos: [i32; 3]) {
    setpiece_sfx::play(slot, pos);
}

/// A cue from the monster bank, else the resident core sample.
#[inline]
pub(crate) unsafe fn cue_or(slot: u8, core: u8, pos: [i32; 3]) {
    if setpiece_sfx::has(slot) {
        setpiece_sfx::play(slot, pos);
    } else {
        sfx::play_world(core, pos);
    }
}

/// Keep or find the enemy and face it. Clears the actor's target and returns
/// `None` when there is nothing to fight.
#[inline(never)]
#[optimize(size)]
unsafe fn acquire(
    m: &Map,
    sight: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    nprops: usize,
    wake: i32,
) -> Option<(u8, bool, [i32; 3])> {
    let (target, visible) = if ai_reacquire(pi) {
        let selected =
            retained_actor_target(m, sight, pi, player_pos, nprops).unwrap_or_else(|| {
                if prop_waits_for_trigger(pi) {
                    (PROP_TARGET_NONE, false)
                } else {
                    find_actor_target(m, sight, pi, player_pos, nprops, wake * wake)
                }
            });
        prop_ai_set_target_visible(pi, selected.1);
        selected
    } else {
        (PROP_AI_TARGET[pi], prop_ai_target_visible(pi))
    };
    let aim = if target == PROP_TARGET_NONE {
        None
    } else {
        target_aim_point(target, player_pos, nprops)
    };
    let aim = aim.and_then(|truth| pursue_point(pi, truth, visible));
    let Some(aim) = aim else {
        PROP_STATE[pi] = PROP_STATE_IDLE;
        PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
        prop_ai_set_target_visible(pi, false);
        return None;
    };
    PROP_AI_TARGET[pi] = target;
    prop_face_point(pi, aim);
    Some((target, visible, aim))
}

/// monster_human_grunt with an MP5: ready stance, then bursts of three volleys
/// (one bullet, sometimes three) every 10 to 14 ticks, and a short pause after
/// five bursts. It holds its ground while it sees the enemy within 1,000 units.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_grunt(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    // CHGrunt::CheckRangeAttack1 fires out to 2048 units, the distance every
    // monster looks.
    const RANGE: i32 = 2048;
    if advance_grunt_grenade(pi) { return; }
    let Some((target, visible, aim)) =
        acquire(m, sight, pi, player_pos, nprops, RANGE)
    else {
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    // Close-quarters rules (retail Half-Life under Xash3D, profile-ranged.md 9.4): a grunt that
    // finds the enemy within 80 units runs back to 500..600 and shoots from there; the ones
    // that charge (which they are is not cooked yet: every odd actor stands in) fire one burst,
    // run in until the hulls touch (the retail kick reaches 34) and kick for 15 then 10. Timer bit 7 flags these modes, bits 5-6 pick
    // one (0 backing away, 1 charging, 2 kicking) and bits 0-4 count: pairs of ticks while
    // backing away or charging, ticks of the kick bout.
    let t = PROP_AI_TIMER[pi];
    let charger = pi & 1 != 0;
    if t & 0x80 != 0 {
        let mode = (t >> 5) & 3;
        let left = t & 31;
        PROP_STATE[pi] = PROP_STATE_MOVE;
        if mode == 2 {
            PROP_AI_TIMER[pi] = if left > 1 {
                0x80 | (2 << 5) | (left - 1)
            } else {
                0
            };
            if left == 14 || left == 8 {
                damage_target(target, if left == 14 { 15 } else { 10 }, pos, health, armor);
            }
        } else if mode == 1 {
            if d2 <= 64 * 64 {
                PROP_AI_TIMER[pi] = 0x80 | (2 << 5) | 20;
            } else if left > 0 {
                if MOVE_TICK & 1 == 0 {
                    PROP_AI_TIMER[pi] = 0x80 | (1 << 5) | (left - 1);
                }
                prop_move_towards_point(m, movers, pi, aim, 15);
            } else {
                // Could not close in (a wall or a crowd): go back to shooting.
                PROP_AI_TIMER[pi] = 0;
            }
        } else {
            let far = 500 + (pi as i32 % 101);
            if left > 0 && d2 < far * far {
                if MOVE_TICK & 1 == 0 {
                    PROP_AI_TIMER[pi] = 0x80 | (left - 1);
                }
                let (dx, dz) = (pos[0] - aim[0], pos[2] - aim[2]);
                let l = isqrt_i32(dx * dx + dz * dz).max(1);
                let goal = [pos[0] + dx * 512 / l, pos[1], pos[2] + dz * 512 / l];
                prop_move_towards_point(m, movers, pi, goal, 8);
            } else {
                PROP_AI_TIMER[pi] = 0;
            }
        }
        return;
    }
    if visible && d2 < 80 * 80 && !charger {
        PROP_AI_TIMER[pi] = 0x80 | 31;
        return;
    }
    if visible && charger && PROP_STATE[pi] == PROP_STATE_ATTACK && t >> 4 != 0 {
        PROP_AI_TIMER[pi] = 0x80 | (1 << 5) | 31;
        return;
    }
    if d2 > RANGE * RANGE || !visible {
        PROP_STATE[pi] = PROP_STATE_MOVE;
        PROP_AI_TIMER[pi] = 0;
        prop_move_towards_point(m, movers, pi, aim, 15);
        return;
    }
    if PROP_STATE[pi] != PROP_STATE_ATTACK {
        // Ready stance before the first shot.
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = 0;
        prop_attack_cooldown_set(pi, prop_attack_cooldown(pi).max(ATTACK_WINDUP + 1));
        return;
    }
    if try_grunt_grenade(m,sight,pi,aim,nprops) { return; }
    let weapons = hecu_grenades::weapons(GRUNT_GREN[pi]);
    if weapons & 1 == 0 && weapons & 8 != 0 {
        // A shotgun marine (`weapons` bit 8 with no MP5): one blast of
        // `sk_hgrunt_pellets` pellets, each drawn in a 15 degree cone and worth
        // 5 (`sk_plr_buckshot`), every 0.7 s.
        if prop_attack_cooldown(pi) == 0 {
            let from = prop_eye(m, pi);
            let mut hits = 0u8;
            let mut n = skill_value(Sk::HgruntPellets);
            while n > 0 {
                if monster_shot_lands(m, from, aim, target, player_pos, nprops, 535).is_some() {
                    hits += 1;
                }
                n -= 1;
            }
            if hits > 0 {
                damage_target(target, hits * 5, pos, health, armor);
            }
            push_tracer(from, aim);
            sfx::play_world(sfx::SHOTGUN, pos);
            make_noise(pos, senses::VOL_MONSTER_BURST);
            prop_attack_cooldown_set(pi, 14);
        }
        return;
    }
    let state = PROP_AI_TIMER[pi]; // low nibble: burst ticks left, high: bursts since the pause
    let left = state & 15;
    if left > 0 {
        PROP_AI_TIMER[pi] = state - 1;
        if left & 1 == 0 {
            let from = prop_eye(m, pi);
            let dmg = skill_hit(Sk::Bullet9mmAr);
            let bullets = if impact_rng().below(4) == 0 { 3 } else { 1 };
            let mut b = 0;
            while b < bullets {
                // FireBullets with VECTOR_CONE_10DEGREES: the bullet either
                // crosses the target's body or goes wide of it.
                if monster_shot_lands(m, from, aim, target, player_pos, nprops, 711).is_some() {
                    damage_target(target, dmg, pos, health, armor);
                    push_tracer(from, aim);
                } else {
                    let off = [
                        impact_rng().signed(60) as i32,
                        impact_rng().signed(40) as i32,
                        impact_rng().signed(60) as i32,
                    ];
                    push_tracer(from, [aim[0] + off[0], aim[1] + off[1], aim[2] + off[2]]);
                }
                b += 1;
            }
            cue_or(SP::HG_MGUN, sfx::MP5, pos);
            make_noise(pos, senses::VOL_MONSTER_BURST);
        }
    } else if prop_attack_cooldown(pi) == 0 {
        let bursts = (state >> 4) + 1;
        let gap = 10 + 2 * impact_rng().below(3) as u8;
        if bursts >= 5 {
            prop_attack_cooldown_set(pi, gap + 8);
            PROP_AI_TIMER[pi] = 6;
        } else {
            prop_attack_cooldown_set(pi, gap);
            PROP_AI_TIMER[pi] = (bursts << 4) | 6;
        }
    }
}

/// monster_alien_slave: a 46-tick zap whose two beams land 34 ticks in (20 on
/// Medium at any range up to about 550 units), repeated every 53 ticks; between
/// zaps it runs in and claws three times a 30-tick loop, at +9, +13 and +21.
///
/// The zap plays `zapattack1` from its first pose (the charge cry at +2, the
/// beams and their sounds from +33 for the 7 ticks the sequence keeps them
/// out) and the claws loop `attack1` (the clip choice is `prop_clip`'s: an
/// attack with no zap in flight).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_slave(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    // CISlave::CheckRangeAttack1: a zap from 64 to 784 units.
    const RANGE: i32 = 784;
    const CLAW: i32 = 70;
    const ZAP: u8 = 46;
    let Some((target, visible, aim)) =
        acquire(m, sight, pi, player_pos, nprops, RANGE)
    else {
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    let beam = skill_hit(Sk::SlaveZap);
    let claw = skill_hit(Sk::SlaveClaw);
    let casting = PROP_AI_TIMER[pi];
    if casting > 0 {
        // Mid-zap: stand and face the enemy; `ZAP - casting + 1` ticks have run.
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = casting - 1;
        if casting == 1 {
            // The sequence is over: do not show the claw for the tick before
            // the next decision.
            PROP_STATE[pi] = PROP_STATE_MOVE;
        }
        let k = ZAP - casting + 1;
        if k == 2 {
            cue(SP::SLV_CHARGE, pos);
        }
        if k >= 33 && k <= 40 && visible {
            let from = [pos[0], pos[1] + 30, pos[2]];
            push_tracer_styled(from, aim, TRACER_BEAM);
            push_tracer_styled([from[0], from[1] + 8, from[2]], aim, TRACER_BEAM);
        }
        if k == 33 {
            sfx::play_world(sfx::ELECTRO, pos);
            cue(SP::SLV_BEAM, pos);
        } else if k == 34 && visible && d2 <= RANGE * RANGE {
            damage_target(target, beam.saturating_mul(2), pos, health, armor);
        }
        return;
    }
    let cooldown = prop_attack_cooldown(pi);
    if d2 <= CLAW * CLAW {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        // The claw loop is 30 ticks; the cooldown counts it down.
        let left = if cooldown == 0 {
            prop_attack_cooldown_set(pi, 30);
            30
        } else {
            cooldown
        };
        if matches!(30 - left, 9 | 13 | 21) {
            damage_target(target, claw, pos, health, armor);
            cue(SP::SLV_CLAW, pos);
        }
    } else if visible && d2 <= RANGE * RANGE && d2 > 64 * 64 && cooldown == 0 {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = ZAP;
        // A far target is zapped again as the sequence ends (53 ticks a cycle);
        // a nearer one is closed on for 20 ticks first.
        prop_attack_cooldown_set(pi, if d2 < 350 * 350 { ZAP + 23 } else { ZAP + 7 });
    } else if d2 < 350 * 350 || !visible {
        PROP_STATE[pi] = PROP_STATE_MOVE;
        prop_move_towards_point(m, movers, pi, aim, 9);
    } else {
        PROP_STATE[pi] = PROP_STATE_IDLE;
    }
}

/// Types whose think lives in this module although the model table calls them
/// passive: they are the only AI_IDLE actors the actor loop does not skip.
#[inline(always)]
pub(crate) fn idle_thinks(ty: u8) -> bool {
    ty == 12 || ty == PROP_TYPE_LEECH || ty == tripmine::PROP_TYPE_TRIPMINE
}

pub(crate) unsafe fn idle_think(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    ty: u8,
    player_pos: [i32; 3],
    in_player_pvs: bool,
    health: &mut u16,
    armor: &mut u16,
) {
    if ty == 12 {
        barnacle::tick(m, pi, player_pos, health);
    } else if ty == PROP_TYPE_LEECH {
        tick_leech(m, movers, pi, player_pos, in_player_pvs, health, armor);
    } else if ty == tripmine::PROP_TYPE_TRIPMINE {
        tripmine::tick_map(m, movers, pi);
    }
}

pub(crate) const PROP_TYPE_LEECH: u8 = 13;

static mut LEECHES: [leech::Swim; leech::SLOTS] = [leech::Swim::EMPTY; leech::SLOTS];

/// A new map starts with no swimmer thinking.
pub(crate) unsafe fn reset_leeches() {
    LEECHES = [leech::Swim::EMPTY; leech::SLOTS];
}

/// The leech's view of the map for one think.
struct LeechWorld<'a> {
    m: &'a Map,
    movers: &'a [phys::Mover],
    pos: [i32; 3],
    yaw: u16,
    eye: Option<[i32; 3]>,
}

impl leech::World for LeechWorld<'_> {
    #[inline(never)]
    fn free_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> i32 {
        match phys::trace_line(self.m, self.movers, from, to) {
            Some(hit) => hit.frac,
            None => 4096,
        }
    }

    #[inline(never)]
    fn wet(&mut self, p: [i32; 3]) -> bool {
        unsafe { water_touch(self.m, self.m.n_ents, p) }
    }

    #[inline(never)]
    fn rand(&mut self, lo: i32, hi: i32) -> i32 {
        unsafe { lo + impact_rng().below((hi - lo + 1).max(1) as u32) as i32 }
    }

    #[inline(never)]
    fn look(&mut self) -> Option<leech::Enemy> {
        let eye = self.eye?;
        let (dx, dz) = (eye[0] - self.pos[0], eye[2] - self.pos[2]);
        let delta = [dx, eye[1] - self.pos[1], dz];
        if dist2_3([0; 3], delta) > leech::LOOK_RANGE * leech::LOOK_RANGE
            || !leech::in_view(self.yaw, dx, dz)
            || !actor_line_clear(self.m, self.movers, self.pos, eye)
        {
            return None;
        }
        self.enemy()
    }

    #[inline(never)]
    fn enemy(&mut self) -> Option<leech::Enemy> {
        // FVisible: a leech never sees a player standing on dry land, and
        // Look only keeps enemies whose waterlevel is not zero.
        Some(leech::Enemy {
            eye: self.eye?,
            wet: unsafe { PLAYER_WATER_LEVEL } != 0,
        })
    }
}

/// monster_leech: swim, hunt the player while he is in the water, bite.
/// Thinks at 10 Hz and only while the player can see its room (retail
/// FIND_CLIENT_IN_PVS); a leech outside the view keeps its place and
/// forgets its mood.
#[inline(never)]
#[optimize(size)]
unsafe fn tick_leech(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    in_player_pvs: bool,
    health: &mut u16,
    armor: &mut u16,
) {
    let mut slot = leech::SLOTS;
    let mut free = leech::SLOTS;
    let mut i = 0usize;
    while i < leech::SLOTS {
        let owner = LEECHES[i].prop;
        if owner == pi as u8 {
            slot = i;
        } else if owner != leech::FREE
            && (PROP_ACTIVE[owner as usize] == 0 || PROP_HEALTH[owner as usize] == 0)
        {
            LEECHES[i] = leech::Swim::EMPTY;
        }
        if LEECHES[i].prop == leech::FREE && free == leech::SLOTS {
            free = i;
        }
        i += 1;
    }
    if !in_player_pvs {
        if slot != leech::SLOTS {
            LEECHES[slot] = leech::Swim::EMPTY;
        }
        PROP_STATE[pi] = PROP_STATE_MOVE;
        return;
    }
    let pos = PROP_POS[pi];
    let yaw = prop_yaw_value(PROP_YAW[pi]);
    let eye = if *health > 0 {
        Some([player_pos[0], player_pos[1] + VIEW_HEIGHT, player_pos[2]])
    } else {
        None
    };
    let mut world = LeechWorld {
        m,
        movers,
        pos,
        yaw,
        eye,
    };
    if slot == leech::SLOTS {
        if free == leech::SLOTS {
            // Every slot is busy: this one holds its pose.
            PROP_STATE[pi] = PROP_STATE_MOVE;
            return;
        }
        slot = free;
        LEECHES[slot] = leech::Swim::spawn(pi as u8, pos, &mut world);
    }
    let wet = water_touch(m, m.n_ents, [pos[0], pos[1] + 1, pos[2]]);
    if (AI_TICK.wrapping_add(pi as u32)) % leech::THINK_TICKS == 0 {
        let motion = LEECHES[slot].think(pos, yaw, wet, &mut world);
        #[cfg(feature = "probe-fixes")]
        if SIM_NOW % 10 == 0 || motion.bite {
            reference_trace::probe_vals(
                SIM_NOW as u32,
                if motion.bite { "leech_bite" } else { "leech" },
                &[pi as i32, pos[0], pos[1], pos[2], motion.biting as i32, motion.dry as i32],
            );
        }
        if motion.bite {
            // The bite event only hurts a prey the leech is facing.
            if let Some(eye) = eye {
                if leech::bite_connects(yaw, eye[0] - pos[0], eye[2] - pos[2]) {
                    damage_target(
                        PROP_TARGET_PLAYER,
                        skill_hit(Sk::LeechBite),
                        pos,
                        health,
                        armor,
                    );
                    LEECHES[slot].bit();
                }
            }
        }
        PROP_STATE[pi] = if motion.dry {
            PROP_STATE_IDLE
        } else if motion.biting {
            PROP_STATE_ATTACK
        } else {
            PROP_STATE_MOVE
        };
        if motion.turn_q12 != 0 {
            let turned = ((yaw as i32 + motion.turn_q12) & 0x0fff) as u16;
            PROP_YAW[pi] = prop_with_yaw(PROP_YAW[pi], turned);
        }
        let mut to = pos;
        let mut axis = 0usize;
        while axis < 3 {
            // Slide: each axis moves on its own until a wall stops it.
            if motion.step[axis] != 0 {
                let mut next = to;
                next[axis] += motion.step[axis];
                // A start inside a hull reads as no hit, so also refuse any
                // step that ends in solid rock.
                if phys::trace_line(m, movers, to, next).is_none() && camera_leaf(m, next) > 0 {
                    to = next;
                }
            }
            axis += 1;
        }
        if to != pos {
            prop_set_pos_exact(m, pi, to);
        }
    }
}

/// The shooting species: each with its own cadence, the rest on the generic shooter.
#[inline(never)]
pub(crate) unsafe fn tick_ranged(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    ty: u8,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    match ty {
        6 => tick_hound(m, movers, sight, pi, player_pos, health, armor, nprops),
        7 => tick_squid(m, movers, sight, pi, player_pos, health, armor, nprops),
        8 => tick_grunt(m, movers, sight, pi, player_pos, health, armor, nprops),
        9 => tick_slave(m, movers, sight, pi, player_pos, health, armor, nprops),
        _ => tick_shooter(
            m, movers, sight, pi, player_pos, health, armor, nprops, true,
        ),
    }
}

/// Shove the player received from the last whip (units per tick), applied by
/// the player-move epilogue.
static mut KNOCK: [i32; 3] = [0; 3];

#[inline(always)]
pub(crate) unsafe fn take_knock() -> [i32; 3] {
    core::mem::take(&mut *core::ptr::addr_of_mut!(KNOCK))
}

/// monster_zombie: wakes on what it sees within 60 degrees of its facing,
/// walks to 64 units, and claws: 80% two 20-damage hits at +12 and +24 ticks of
/// a 52-tick bout, else one 40 at +6 of a 32-tick bout (Medium).
#[inline(never)]
#[optimize(size)]
unsafe fn tick_zombie(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    let Some((target, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, 700) else {
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    let t = PROP_AI_TIMER[pi]; // low 6 bits: bout ticks left, bit 6: the single heavy swing
    let left = t & 63;
    if left > 0 {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = t - 1;
        let done = if t & 64 != 0 { 32 - left } else { 52 - left };
        let hit = if t & 64 != 0 {
            done == 6
        } else {
            done == 12 || done == 24
        };
        if hit && d2 <= 76 * 76 {
            let swing = skill_hit(if t & 64 != 0 {
                Sk::ZombieBothSlash
            } else {
                Sk::ZombieSlash
            });
            damage_target(target, swing, pos, health, armor);
            if target == PROP_TARGET_PLAYER {
                // Each claw throws the player 5 units per tick away from the zombie; the
                // first claw also to the zombie's left, the second to its right.
                let (dx, dz) = (aim[0] - pos[0], aim[2] - pos[2]);
                let side = if t & 64 != 0 {
                    0
                } else if done == 12 {
                    -5
                } else {
                    5
                };
                let len = isqrt_i32(dx * dx + dz * dz).max(1);
                KNOCK = [(dx * 5 + dz * side) / len, 0, (dz * 5 - dx * side) / len];
            }
            // The claws land with claw_strike; the second swipe of a bout (or
            // the lone heavy one) adds the zombie's roar.
            if setpiece_sfx::has(SP::ZO_CLAW) {
                cue(SP::ZO_CLAW, pos);
                if t & 64 != 0 || done == 24 {
                    sfx::play_world(sfx::ZO_ATTACK, pos);
                }
            } else {
                sfx::play_world(sfx::ZO_ATTACK, pos);
            }
        }
    } else if d2 <= 64 * 64 && visible {
        let heavy = impact_rng().below(5) == 0;
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = if heavy { 64 | 32 } else { 52 };
    } else {
        PROP_STATE[pi] = PROP_STATE_MOVE;
        prop_move_towards_point(m, movers, pi, aim, 4);
    }
}

/// monster_bullchicken: spits twice from 65 to 780 units (10 damage, a 30-tick
/// stationary animation each), then runs in and bites (25, the cvar the retail
/// close attack reads) every 44 ticks or so, knocking the player back. Far targets are approached at 15 units per tick.
#[inline(never)]
#[optimize(size)]
unsafe fn tick_squid(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    let Some((target, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, 900) else {
        PROP_AI_TIMER[pi] = 0;
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    let t = PROP_AI_TIMER[pi]; // low 5 bits: animation ticks left, bit 5: whip, bits 6-7: spits so far
    let left = t & 31;
    if left > 0 {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = t - 1;
        let from = prop_eye(m, pi);
        if t & 32 == 0 && left == 25 {
            let dir = dir_q12(from, aim);
            spawn_projectile_dir(PROJ_SPIT, skill_hit(Sk::SquidSpit), from, dir, true);
            cue_or(SP::BC_SPIT, sfx::HC_ATTACK, pos);
        } else if t & 32 != 0 && left == 20 && d2 <= 85 * 85 {
            cue(SP::BC_BITE, pos);
            // The tail whip is the finisher: used when it would kill, the bite
            // otherwise (CheckMeleeAttack1 and 2).
            let victim = if target == PROP_TARGET_PLAYER {
                *health
            } else {
                PROP_HEALTH[target as usize] as u16
            };
            let whip = skill_hit(Sk::SquidWhip);
            damage_target(
                target,
                if victim <= whip as u16 { whip } else { skill_hit(Sk::SquidBite) },
                pos,
                health,
                armor,
            );
            let dir = dir_q12(pos, aim);
            KNOCK = [(dir[0] * 14) >> 12, 15, (dir[2] * 14) >> 12];
        }
    } else if visible && d2 <= 64 * 64 {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = (t & 0xC0) | 32 | 24;
        cue(SP::BC_GROWL, pos);
    } else if visible
        && d2 <= 784 * 784
        && prop_attack_cooldown(pi) == 0
        && !(PROP_STATE[pi] == PROP_STATE_MOVE && d2 >= 512 * 512)
    {
        // CBullsquid::CheckRangeAttack1: a spit between 64 and 784 units, but not
        // from a run beyond 512; the next comes half a second later when it stood,
        // five seconds when it spat on the move.
        let running = PROP_STATE[pi] == PROP_STATE_MOVE;
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = 30;
        prop_attack_cooldown_set(pi, if running { 100 } else { 10 });
    } else if d2 > 500 * 500 || !visible {
        PROP_STATE[pi] = PROP_STATE_MOVE;
        prop_move_towards_point(m, movers, pi, aim, 15);
    } else {
        PROP_STATE[pi] = PROP_STATE_IDLE;
    }
}

/// monster_houndeye: runs in to 190 units, charges for 49 ticks and lets go at
/// +45 with 15 x (1 - distance / 384), multiplied by the pack (up to three
/// alive within 150 units). A hit aborts the charge: it slides 140 units back
/// over 30 ticks and rests 54. A blast every 80 ticks otherwise.
///
/// Animation and cries follow the retail sequences: an alert cry at first
/// sight, a hunting cry as it sets off, the `attack` sequence for the charge
/// (a warm-up cry at +2, the blast at +45), and the looping `madidle2` for the rest, growling at its sound events.
#[inline(never)]
#[optimize(size)]
unsafe fn tick_hound(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    _health: &mut u16,
    _armor: &mut u16,
    nprops: usize,
) {
    let had_target = PROP_AI_TARGET[pi] != PROP_TARGET_NONE;
    let Some((_, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, 900) else {
        PROP_AI_TIMER[pi] = 0;
        return;
    };
    let pos = PROP_POS[pi];
    if !had_target {
        cue(SP::HE_ALERT, pos);
    }
    let d2 = dist2_xz(pos, aim);
    let left = PROP_AI_TIMER[pi];
    if PROP_STATE[pi] == PROP_STATE_MOVE_AWAY {
        PROP_AI_TIMER[pi] = left - 1;
        prop_try_step(m, movers, pi, pos[0] - aim[0], pos[2] - aim[2], 5, true);
        if left == 1 {
            PROP_STATE[pi] = PROP_STATE_IDLE;
            prop_attack_cooldown_set(pi, 54);
        }
    } else if PROP_STATE[pi] == PROP_STATE_ATTACK {
        if PROP_HIT_FLASH[pi] > 0 {
            PROP_STATE[pi] = PROP_STATE_MOVE_AWAY;
            PROP_AI_TIMER[pi] = 30;
            return;
        }
        PROP_AI_TIMER[pi] = left - 1;
        // `49 - left` ticks have run; the `attack` sequence is 48 ticks long.
        if left == 47 {
            // One of its two warm-up cries, picked per animal and charge.
            let pick = (pos[0] ^ pos[2] ^ SIM_NOW as i32) & 1;
            cue(
                if pick == 0 {
                    SP::HE_WARM1
                } else {
                    SP::HE_WARM3
                },
                pos,
            );
        } else if left == 4 {
            // A squad adds 1.1 of the damage for every member past the first
            // (the pack of up to four hounds within a thousand units).
            let mut pack = 0i32;
            let mut qi = 0usize;
            while qi < nprops {
                if PROP_KIND[qi] == PROP_TYPE_HOUNDEYE
                    && PROP_ACTIVE[qi] != 0
                    && PROP_HEALTH[qi] > 0
                    && dist2_3(PROP_POS[qi], pos) < 1024 * 1024
                {
                    pack += 1;
                }
                qi += 1;
            }
            let dmg = skill_hit(Sk::HoundeyeBlast) as i32;
            let squad = dmg * (10 + 11 * (pack.clamp(1, 4) - 1)) / 10;
            houndeye_blast(m, pi, 384, squad.min(255) as u8);
            sfx::play_world(sfx::HE_BLAST, pos);
        } else if left == 1 {
            // Rest: `prop_clip` plays madidle2 while the cooldown runs.
            PROP_STATE[pi] = PROP_STATE_IDLE;
            prop_attack_cooldown_set(pi, 24 + impact_rng().below(8) as u8);
        }
    } else if visible && d2 > 190 * 190 {
        // The hunting cry comes a few ticks into the run, not with the alert.
        let run = if PROP_STATE[pi] == PROP_STATE_MOVE {
            left + 1
        } else {
            1
        };
        PROP_AI_TIMER[pi] = run;
        if run == 5 {
            cue(SP::HE_HUNT, pos);
        }
        PROP_STATE[pi] = PROP_STATE_MOVE;
        prop_move_towards_point(m, movers, pi, aim, 17);
    } else if visible && prop_attack_cooldown(pi) == 0 {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = 48;
    } else {
        PROP_STATE[pi] = PROP_STATE_IDLE;
        if prop_attack_cooldown(pi) != 0 {
            // Resting: madidle2's sound events (7, 17 and 34 of 46 frames at
            // 37 fps); the timer counts the rest's ticks.
            let age = left.wrapping_add(1);
            PROP_AI_TIMER[pi] = age;
            match age % 25 {
                2 | 8 => cue(SP::HE_GROWL, pos),
                18 => cue(SP::HE_GROWL2, pos),
                _ => {}
            }
        } else {
            PROP_AI_TIMER[pi] = 0;
        }
    }
}

/// Melee species: the zombie has its own bout; everything else keeps the
/// shared lunge-and-bite.
#[inline(never)]
pub(crate) unsafe fn tick_melee(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    ty: u8,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    if ty == 5 || ty == 55 {
        tick_zombie(m, movers, sight, pi, player_pos, health, armor, nprops);
    } else {
        tick_headcrab(m, movers, sight, pi, player_pos, health, armor, nprops);
    }
}

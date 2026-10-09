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
    cone: bool,
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
    let mut aim = if target == PROP_TARGET_NONE {
        None
    } else {
        target_aim_point(target, player_pos, nprops)
    };
    if let (true, true, Some(a)) = (cone, PROP_AI_TARGET[pi] == PROP_TARGET_NONE, aim) {
        // A sleeper notices only what stands within 60 degrees of its facing.
        let yaw = prop_yaw_value(PROP_YAW[pi]);
        let dx = a[0] - PROP_POS[pi][0];
        let dz = a[2] - PROP_POS[pi][2];
        let along = (sincos::sin_q12(yaw) * dx + sincos::sin_q12((yaw + 1024) & 0x0fff) * dz) >> 12;
        if along * 2 < isqrt_i32(dx * dx + dz * dz) {
            aim = None;
        }
    }
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

/// Chance in percent that one MP5 bullet from a grunt reaches a standing
/// target `dist` units away: all of them at 100 units, 45 at 300, about 20
/// from 550 on (measured at 100, 200, 300, 550 and 700).
fn grunt_hit_pct(dist: i32) -> u32 {
    (130 - dist * 3 / 10).clamp(20, 100) as u32
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
    const RANGE: i32 = 1000;
    let Some((target, visible, aim)) =
        acquire(m, sight, pi, player_pos, nprops, RANGE + 384, false)
    else {
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
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
    let state = PROP_AI_TIMER[pi]; // low nibble: burst ticks left, high: bursts since the pause
    let left = state & 15;
    if left > 0 {
        PROP_AI_TIMER[pi] = state - 1;
        if left & 1 == 0 {
            let from = prop_eye(m, pi);
            let dist = isqrt_i32(dist2_3(from, aim));
            let pct = grunt_hit_pct(dist);
            let dmg = skill_damage(PROP_KIND[pi]).unwrap_or(4);
            let bullets = if impact_rng().below(4) == 0 { 3 } else { 1 };
            let mut b = 0;
            while b < bullets {
                if impact_rng().below(100) < pct {
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
    const RANGE: i32 = 650;
    const CLAW: i32 = 70;
    const ZAP: u8 = 46;
    let Some((target, visible, aim)) =
        acquire(m, sight, pi, player_pos, nprops, RANGE + 300, false)
    else {
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    let beam = skill_damage(PROP_KIND[pi]).unwrap_or(10);
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
            damage_target(target, beam, pos, health, armor);
            cue(SP::SLV_CLAW, pos);
        }
    } else if visible && d2 <= RANGE * RANGE && d2 > 150 * 150 && cooldown == 0 {
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
    ty == 12 || ty == tripmine::PROP_TYPE_TRIPMINE
}

pub(crate) unsafe fn idle_think(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    ty: u8,
    player_pos: [i32; 3],
    health: &mut u16,
) {
    if ty == 12 {
        barnacle::tick(m, pi, player_pos, health);
    } else if ty == tripmine::PROP_TYPE_TRIPMINE {
        tripmine::tick_map(m, movers, pi);
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
    let Some((target, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, 700, true) else {
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
            let one = skill_damage(PROP_KIND[pi]).unwrap_or(20);
            damage_target(
                target,
                if t & 64 != 0 { one * 2 } else { one },
                pos,
                health,
                armor,
            );
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
/// stationary animation each), then runs in and whips (25) every 44 ticks or so,
/// knocking the player back. Far targets are approached at 15 units per tick.
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
    let Some((target, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, 900, false) else {
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
            spawn_projectile_dir(PROJ_SPIT, skill_damage(7).unwrap_or(10), from, dir, true);
            cue_or(SP::BC_SPIT, sfx::HC_ATTACK, pos);
        } else if t & 32 != 0 && left == 20 && d2 <= 85 * 85 {
            cue(SP::BC_BITE, pos);
            damage_target(target, skill_damage(75).unwrap_or(25), pos, health, armor);
            let dir = dir_q12(pos, aim);
            KNOCK = [(dir[0] * 14) >> 12, 15, (dir[2] * 14) >> 12];
        }
    } else if visible && d2 <= 64 * 64 {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = (t & 0xC0) | 32 | 24;
        cue(SP::BC_GROWL, pos);
    } else if visible && d2 <= 780 * 780 && (t >> 6) < 2 && prop_attack_cooldown(pi) == 0 {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = ((t >> 6) + 1) << 6 | 30;
        prop_attack_cooldown_set(pi, 4);
    } else if d2 > 500 * 500 || (t >> 6) >= 2 || !visible {
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
    let Some((_, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, 900, false) else {
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
            let mut pack = 0u8;
            let mut qi = 0usize;
            while qi < nprops {
                if PROP_KIND[qi] == PROP_TYPE_HOUNDEYE
                    && PROP_ACTIVE[qi] != 0
                    && PROP_HEALTH[qi] > 0
                    && dist2_3(PROP_POS[qi], pos) < 150 * 150
                {
                    pack += 1;
                }
                qi += 1;
            }
            let dmg = skill_damage(PROP_TYPE_HOUNDEYE).unwrap_or(15);
            houndeye_blast(pos, 384, dmg.saturating_mul(pack.clamp(1, 3)));
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

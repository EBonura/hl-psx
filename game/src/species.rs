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

/// CISlave::ArmBeam, one hand a call (the hands take turns): from the hand (36
/// up, 16 to the side, 32 ahead) toward a random point out to that side, up to
/// 512 units; a beam only when that line meets a wall.
#[inline(never)]
#[optimize(size)]
unsafe fn slave_arm_beam(m: &Map, movers: &[phys::Mover], pi: usize, right: bool) {
    let pos = PROP_POS[pi];
    let yaw = prop_yaw_value(PROP_YAW[pi]);
    let (fx, fz) = (sincos::sin_q12(yaw), sincos::sin_q12((yaw + 1024) & 0x0fff));
    let side = if right { 1 } else { -1 };
    let (rx, rz) = (fz * side, -fx * side);
    let src = [
        pos[0] + ((rx * 16 + fx * 32) >> 12),
        pos[1] + 36,
        pos[2] + ((rz * 16 + fz * 32) >> 12),
    ];
    // A hash of the tick and the prop, so the shared RNG streams stay as they were.
    let h = (SIM_NOW as u32 ^ (pi as u32) << 8).wrapping_mul(0x9E37_79B1);
    let out = (h >> 23) as i32;
    let end = [
        src[0] + ((rx * out) >> 12),
        src[1] + ((h >> 7) & 0x3ff) as i32 - 512,
        src[2] + ((rz * out) >> 12),
    ];
    if let Some(hit) = phys::trace_line(m, movers, src, end) {
        push_tracer_styled(src, hit.pos, TRACER_BEAM);
    }
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
    if advance_grunt_grenade(pi) {
        return;
    }
    let Some((target, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, RANGE) else {
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    // Close-quarters rules (retail Half-Life under Xash3D, profile-ranged.md 9.4): a grunt that
    // finds the enemy within 80 units runs back to 500..600 and shoots from there; the ones
    // that charge (which they are is not cooked yet: every odd actor stands in) fire one burst,
    // run in until the hulls touch (the retail kick reaches 34) and kick once for `sk_hgrunt_kick`. Timer bit 7 flags these modes, bits 5-6 pick
    // one (0 backing away, 1 charging, 2 kicking) and bits 0-4 count: pairs of ticks while
    // backing away or charging, ticks of the kick bout.
    let mut t = PROP_AI_TIMER[pi];
    let charger = pi & 1 != 0;
    // CHGrunt::GetSchedule on light damage: TAKE_COVER_FROM_ENEMY (the 1 in
    // 10 that flinches instead is prop_flinch_ticks'). A hit flash that has
    // not run out and is no flinch is a hit that started none. One search
    // runs per tick, so a burst of hits on a squad spreads over the flash's
    // ticks; a marine whose search found nothing tries again while it lasts.
    if t & 0x80 == 0
        && PROP_HIT_FLASH[pi] != 0
        && PROP_HIT_FLASH[pi] < PROP_HIT_FLASH_TICKS
        && COVER_SEARCH_TICK != SIM_NOW
        && PROP_SCRIPT_MODE[pi] == 0
        && PROP_DORMANT[pi] & PROP_RUNTIME_AIRBORNE == 0
    {
        COVER_SEARCH_TICK = SIM_NOW;
        if let Some(cover) = nav_cover_goal(m, sight, pi, aim) {
            PROP_SCRIPT_GOAL[pi] = [cover[0] as i16, cover[1] as i16, cover[2] as i16];
            t = 0x80 | (3 << 5) | 31;
            PROP_AI_TIMER[pi] = t;
        }
    }
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
            // frontkick has one HGRUNT_AE_KICK event, frame 14 at 35 fps: the 8th
            // tick of the sequence, one hit of sk_hgrunt_kick.
            if left == 13 {
                // DMG_CLUB: the suit names only a major club hit, which a kick never is.
                hev::set_kind(hev::Kind::Generic);
                damage_target(target, skill_hit(Sk::HgruntKick), pos, health, armor);
                if target == PROP_TARGET_PLAYER {
                    set_view_punch(Some(15), None, None); // CHGrunt::HandleAnimEvent (kick)
                }
            }
        } else if mode == 3 {
            // TASK_RUN_PATH to the cover, then GRUNT_WAIT_FACE_ENEMY there
            // until the count runs out.
            let g = PROP_SCRIPT_GOAL[pi];
            let goal = [g[0] as i32, g[1] as i32, g[2] as i32];
            if left > 0 {
                if MOVE_TICK & 1 == 0 {
                    PROP_AI_TIMER[pi] = 0x80 | (3 << 5) | (left - 1);
                }
                if dist2_xz(pos, goal) > 24 * 24 {
                    prop_move_towards_point(m, movers, pi, goal, 15);
                } else {
                    PROP_STATE[pi] = PROP_STATE_IDLE;
                }
            } else {
                PROP_AI_TIMER[pi] = 0;
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
    if try_grunt_grenade(m, sight, pi, aim, nprops) {
        return;
    }
    let weapons = hecu_grenades::weapons(GRUNT_GREN[pi]);
    if weapons & 1 == 0 && weapons & 8 != 0 {
        // A shotgun marine (`weapons` bit 8 with no MP5): one blast of
        // `sk_hgrunt_pellets` pellets, each drawn in a 15 degree cone and worth
        // `sk_9mm_bullet` (5, 5, 8 in the Xash3D trace), every 0.7 s.
        if prop_attack_cooldown(pi) == 0 {
            let from = prop_eye(m, pi);
            let mut hits = 0u8;
            let mut n = skill_value(Sk::HgruntPellets);
            while n > 0 {
                // A pellet on the head counts three (sk_player_head).
                hits += monster_shot_lands(m, from, aim, target, player_pos, nprops, 535).unwrap_or(0);
                n -= 1;
            }
            if hits > 0 {
                damage_target(
                    target,
                    hits.saturating_mul(skill_hit(Sk::Bullet9mm)),
                    pos,
                    health,
                    armor,
                );
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
                if let Some(worth) = monster_shot_lands(m, from, aim, target, player_pos, nprops, 711) {
                    damage_target(target, dmg * worth, pos, health, armor);
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
    let Some((target, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, RANGE) else {
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
        // ISLAVE_AE_ZAP_POWERUP's ArmBeams: lightning from each hand to a wall the
        // hand can reach, crackling while the charge builds.
        if k >= 2 && k < 33 && k & 1 == 0 {
            slave_arm_beam(m, movers, pi, k & 2 != 0);
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
        // ISLAVE_AE_CLAW, three times in attack1 (`29 - left` ticks into it).
        if anim_event(9, hl_format::anim_event::SLAVE_CLAW, 29u8.wrapping_sub(left)) != 0 {
            // CISlave's claws are DMG_SLASH; its zap (DMG_SHOCK) names nothing.
            hev::set_kind(hev::Kind::Slash);
            damage_target(target, claw, pos, health, armor);
            if target == PROP_TARGET_PLAYER {
                set_view_punch(Some(5), None, Some(-18)); // CISlave::HandleAnimEvent (claws)
            }
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
    ty == 12
        || ty == tentacle::KIND
        || ty == PROP_TYPE_LEECH
        || ty == PROP_TYPE_BIGMOMMA
        || ty == tripmine::PROP_TYPE_TRIPMINE
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
    } else if ty == tentacle::KIND {
        tick_tentacle(m, pi, player_pos, health, armor);
    } else if ty == PROP_TYPE_LEECH {
        tick_leech(m, movers, pi, player_pos, in_player_pvs, health, armor);
    } else if ty == PROP_TYPE_BIGMOMMA {
        tick_gonarch(m, movers, pi, player_pos, health, armor);
    } else if ty == tripmine::PROP_TYPE_TRIPMINE {
        tripmine::tick_map(m, movers, pi);
    }
}

pub(crate) const PROP_TYPE_LEECH: u8 = 13;

static mut TENTACLES: [tentacle::Tentacle; tentacle::SLOTS] =
    [tentacle::Tentacle::EMPTY; tentacle::SLOTS];

/// A new map starts with no tentacle thinking.
pub(crate) unsafe fn reset_tentacles() {
    TENTACLES = [tentacle::Tentacle::EMPTY; tentacle::SLOTS];
}

/// The tentacle state of prop `pi`, taking a free slot (spawned down its pit)
/// the first time; `None` when all four are in use.
#[inline(never)]
#[optimize(size)]
unsafe fn tentacle_slot(m: &Map, pi: usize) -> Option<&'static mut tentacle::Tentacle> {
    let slots = &mut *core::ptr::addr_of_mut!(TENTACLES);
    let mut free = tentacle::SLOTS;
    let mut i = 0usize;
    while i < tentacle::SLOTS {
        if slots[i].prop == pi as u8 {
            return Some(&mut slots[i]);
        }
        if slots[i].prop == tentacle::FREE && free == tentacle::SLOTS {
            free = i;
        }
        i += 1;
    }
    if free == tentacle::SLOTS {
        return None;
    }
    // The cooker keeps the tentacle's sweep limit in its carry word.
    slots[free] =
        tentacle::Tentacle::spawn(pi as u8, prop_yaw_value(PROP_YAW[pi]), m.prop_carry_id(pi));
    Some(&mut slots[free])
}

/// The tentacle's pose for drawing: its clip slot and q12 position in it.
pub(crate) unsafe fn tentacle_clip(pi: usize) -> Option<(usize, usize)> {
    let slots = &*core::ptr::addr_of!(TENTACLES);
    let mut i = 0usize;
    while i < tentacle::SLOTS {
        if slots[i].prop == pi as u8 {
            let (seq, cycle) = slots[i].pose();
            return Some((seq as usize, cycle as usize));
        }
        i += 1;
    }
    None
}

/// A trigger used tentacle `pi` (`CTentacle::CommandUse`): on, it hears a
/// noise where the activator stands; off, it dies; toggled, it thrashes at the
/// engine.
pub(crate) unsafe fn tentacle_use(m: &Map, pi: usize, use_type: u8, activator: [i32; 3]) {
    if use_type == map::USE_ON {
        crate::make_world_noise(activator, 1024);
    } else if let Some(t) = tentacle_slot(m, pi) {
        if use_type == map::USE_OFF {
            t.die();
        } else if use_type == map::USE_TOGGLE {
            t.thrash();
        }
    }
}

/// The world as the tentacle hears it: the player's own sound, and every
/// noise still carrying, at twice the usual reach.
struct TentacleWorld {
    pi: usize,
    player: [i32; 3],
}

impl tentacle::World for TentacleWorld {
    #[inline(never)]
    fn rand(&mut self, lo: i32, hi: i32) -> i32 {
        unsafe { lo + impact_rng().below((hi - lo + 1).max(1) as u32) as i32 }
    }

    #[inline(never)]
    fn hear(&mut self, ear: [i32; 3]) -> Option<[i32; 3]> {
        // The player's own sound is weighed first, so it wins a tie.
        unsafe {
            crate::noise_heard_with(
                ear,
                tentacle::HEARING,
                self.player,
                crate::PLAYER_NOISE.volume(),
            )
        }
    }

    #[inline(never)]
    fn yaw_of(&mut self, dx: i32, dz: i32) -> u16 {
        yaw_from_vec(dx, dz)
    }

    #[inline(never)]
    fn ticks(&mut self, seq: u8) -> u16 {
        unsafe { script_clip_hold_ticks(self.pi, seq, 20) }
    }

    /// Its taps and bangs are the sample its `sound` keyvalue names (te_strike2
    /// for the silo, pl_dirt1 for dirt: the map's bank carries only its own),
    /// the taps at 0.4 volume; the song is te_sing1. All ATTN_NORM.
    #[inline(never)]
    #[optimize(size)]
    fn sound(&mut self, what: u8, height: i32) {
        unsafe {
            let mut pos = PROP_POS[self.pi];
            pos[1] += height;
            let slot = if what == tentacle::SOUND_SING {
                SP::TE_SING
            } else if setpiece_sfx::has(SP::TE_STRIKE) {
                SP::TE_STRIKE
            } else {
                SP::TE_DIRT
            };
            let volume = if what == tentacle::SOUND_TAP { 40 } else { 100 };
            crate::sfx::play_map_authored(setpiece_sfx::id(slot), pos, volume, 2);
        }
    }
}

/// monster_tentacle: hear, rise or sink to the noise's level, strike at it.
/// The claw hurts on contact with the player's hull (`HitTouch`), tested
/// against the posed studio hitboxes only while he stands inside the
/// tentacle's reach box.
#[inline(never)]
#[optimize(size)]
unsafe fn tick_tentacle(
    m: &Map,
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
) {
    let Some(t) = tentacle_slot(m, pi) else {
        return;
    };
    if PROP_SCRIPT_MODE[pi] != 0 {
        t.scripted();
        return;
    }
    let pos = PROP_POS[pi];
    let think = (AI_TICK.wrapping_add(pi as u32)) % tentacle::THINK_TICKS == 0;
    let mut world = TentacleWorld {
        pi,
        player: player_pos,
    };
    let mut hp = PROP_HEALTH[pi];
    if t.tick(pos, &mut hp, think, &mut world) {
        PROP_ACTIVE[pi] = 0;
        t.prop = tentacle::FREE;
        return;
    }
    PROP_HEALTH[pi] = hp;
    PROP_YAW[pi] = prop_with_yaw(PROP_YAW[pi], t.yaw);
    PROP_STATE[pi] = PROP_STATE_IDLE;
    #[cfg(feature = "interaction-probe")]
    if SIM_NOW % 10 == 0 {
        reference_trace::probe_vals(
            SIM_NOW as u32,
            "tentacle",
            &[
                pi as i32,
                t.seq as i32,
                t.dir as i32,
                t.cycle as i32,
                hp as i32,
                t.yaw as i32,
                t.killing() as i32,
            ],
        );
    }
    // SetObjectCollisionBox: 400 either side, 850 up.
    let d = [
        player_pos[0] - pos[0],
        player_pos[1] - pos[1],
        player_pos[2] - pos[2],
    ];
    if *health == 0 || t.dying || d[0].abs() > 400 || d[2].abs() > 400 || d[1] < -64 || d[1] > 850 {
        return;
    }
    if let Some(hitbox) = crate::prop_studio_touch(m, pi, player_pos) {
        if let Some(dmg) = t.touch(crate::tentacle_claw_box(hitbox)) {
            #[cfg(feature = "interaction-probe")]
            reference_trace::probe_vals(
                SIM_NOW as u32,
                "tentacle_hit",
                &[pi as i32, hitbox as i32, dmg as i32, t.seq as i32],
            );
            // CTentacle::HitTouch is DMG_CRUSH.
            hev::set_kind(hev::Kind::Crush);
            damage_target(PROP_TARGET_PLAYER, dmg, pos, health, armor);
        }
    }
}

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
                &[
                    pi as i32,
                    pos[0],
                    pos[1],
                    pos[2],
                    motion.biting as i32,
                    motion.dry as i32,
                ],
            );
        }
        if motion.bite {
            // The bite event only hurts a prey the leech is facing.
            if let Some(eye) = eye {
                if leech::bite_connects(yaw, eye[0] - pos[0], eye[2] - pos[2]) {
                    setpiece_sfx::play(hl_format::setpiece_audio::LEECH_BITE, pos);
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

/// Every melee and ranged monster's think, chosen by its archetype
/// (`ModelDef::arch`). The schedule engine replaces these routines one
/// archetype at a time; a type with no archetype stands idle.
#[inline(never)]
pub(crate) unsafe fn tick_monster(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    ty: u8,
    player_pos: [i32; 3],
    in_pvs: bool,
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    match model_def(ty).archetype() {
        ARCH_BITER => tick_headcrab(m, movers, sight, pi, player_pos, health, armor, nprops),
        ARCH_ZOMBIE => tick_zombie(m, movers, sight, pi, player_pos, health, armor, nprops),
        ARCH_HOUND => tick_hound(m, movers, sight, pi, player_pos, health, armor, nprops),
        ARCH_SQUID => tick_squid(m, movers, sight, pi, player_pos, health, armor, nprops),
        ARCH_GRUNT => tick_grunt(m, movers, sight, pi, player_pos, health, armor, nprops),
        ARCH_SLAVE => tick_slave(m, movers, sight, pi, player_pos, health, armor, nprops),
        ARCH_SHOOTER => tick_shooter(
            m, movers, sight, pi, player_pos, health, armor, nprops, true,
        ),
        ARCH_GARG => crate::garg::tick(m, movers, sight, pi, player_pos, nprops),
        _ => idle_think(m, movers, pi, ty, player_pos, in_pvs, health, armor),
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
/// walks to 64 units, and claws: 80% two 20-damage hits at +15 and +27 ticks of
/// a 52-tick bout, else one 40 at +11 of a 40-tick bout (Medium). The claw ticks are
/// the reference runtime's (one-claw swing at +15 and +27 ticks after the attack state
/// shows, both-arms swing at +11) and the bouts are its attack-to-attack spacing of 40 to 60 ticks.
/// `done` counts the ticks after the one that set the bout, so a claw at +N is `done == N - 1`.
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
    let yaw_before = PROP_YAW[pi];
    let Some((target, visible, aim)) = acquire(m, sight, pi, player_pos, nprops, 700) else {
        return;
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    let t = PROP_AI_TIMER[pi]; // low 6 bits: bout ticks left, bit 6: the single heavy swing
    let left = t & 63;
    if left > 0 {
        // TASK_MELEE_ATTACK1 never turns the zombie: the bout keeps the heading it started with.
        PROP_YAW[pi] = yaw_before;
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = t - 1;
        let done = if t & 64 != 0 { 40 - left } else { 52 - left };
        // The claws land on attack1's events (1 right, 2 left) `done` ticks
        // into it; the lone heavy swing (ZOMBIE_AE_BOTH_ATTACK, 3) plays
        // attack2, which is not cooked, and keeps its measured +11.
        let ev = if t & 64 != 0 {
            (done == 10) as u8 * 3
        } else {
            anim_event(PROP_KIND[pi], hl_format::anim_event::ZOMBIE_ATTACK, done)
        };
        let hit = ev != 0;
        // CheckTraceHullAttack(70): a 32-wide hull swept 70 units straight ahead; the target's
        // centre is reached within 70 + 16 in front and 16 + 16 to the side.
        let yaw = prop_yaw_value(PROP_YAW[pi]);
        let (dx, dz) = (aim[0] - pos[0], aim[2] - pos[2]);
        let fwd = (sincos::sin_q12(yaw) * dx + sincos::sin_q12((yaw + 1024) & 0x0fff) * dz) >> 12;
        let side = (sincos::sin_q12((yaw + 1024) & 0x0fff) * dx - sincos::sin_q12(yaw) * dz) >> 12;
        if hit && (0..=86).contains(&fwd) && side.abs() <= 32 {
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
                let side = match ev {
                    1 => -5,
                    2 => 5,
                    _ => 0,
                };
                let len = isqrt_i32(dx * dx + dz * dz).max(1);
                KNOCK = [(dx * 5 + dz * side) / len, 0, (dz * 5 - dx * side) / len];
                // CZombie::HandleAnimEvent: right claw, left claw, both.
                set_view_punch(
                    Some(5),
                    None,
                    match ev {
                        1 => Some(-18),
                        2 => Some(18),
                        _ => None,
                    },
                );
            }
            // The claws land with claw_strike; the second swipe of a bout (or
            // the lone heavy one) adds the zombie's roar.
            if setpiece_sfx::has(SP::ZO_CLAW) {
                cue(SP::ZO_CLAW, pos);
                if ev >= 2 {
                    sfx::play_world(sfx::ZO_ATTACK, pos);
                }
            } else {
                sfx::play_world(sfx::ZO_ATTACK, pos);
            }
        } else if hit {
            // A swing that finds nothing: claw_miss.
            cue(SP::ZO_MISS, pos);
        }
    } else if d2 <= 64 * 64 && visible {
        let heavy = impact_rng().below(5) == 0;
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = if heavy { 64 | 40 } else { 52 };
    } else {
        PROP_STATE[pi] = PROP_STATE_MOVE;
        prop_move_towards_point(m, movers, pi, aim, 4);
    }
}

/// Each bullsquid's meal (`squid_meal`).
pub(crate) static mut SQUID_MEAL: [squid_meal::Meal; LIVE_PROPS] =
    [squid_meal::Meal::NONE; LIVE_PROPS];

/// The nearest carcass scent within the squid's smell range: a monster or person that died
/// in play within the last 30 s (a body burst to gibs leaves its meat there, bits_SOUND_MEAT
/// for 25 s, taken as the same scent; a corpse placed dead leaves none), and whether it is in
/// the squid's view cone and sight.
#[inline(never)]
#[optimize(size)]
unsafe fn nearest_carcass(
    m: &Map,
    sight: &[phys::Mover],
    pi: usize,
    nprops: usize,
) -> Option<([i32; 3], bool)> {
    let pos = PROP_POS[pi];
    let mut best: Option<([i32; 3], i32)> = None;
    let mut q = 0usize;
    while q < nprops {
        if q != pi
            && PROP_ACTIVE[q] != 0
            && PROP_HEALTH[q] == 0
            && PROP_STATE[q] == PROP_STATE_DEAD
            && senses::class_of(PROP_KIND[q]) != 0
            && senses::class_of(PROP_KIND[q]) != senses::CLASS_MACHINE
            && !(q < m.n_props && m.prop(q).0 & PROP_DEAD_BIT != 0)
            && SIM_NOW.wrapping_sub(PROP_DEATH_START[q]) < squid_meal::CARCASS_TICKS
        {
            let d2 = dist2_3(PROP_POS[q], pos);
            if d2 <= squid_meal::SMELL_RANGE * squid_meal::SMELL_RANGE
                && best.is_none_or(|(_, b)| d2 < b)
            {
                best = Some((PROP_POS[q], d2));
            }
        }
        q += 1;
    }
    let (food, _) = best?;
    let yaw = prop_yaw_value(PROP_YAW[pi]);
    let in_cone = senses::in_view_cone(
        PROP_KIND[pi],
        sincos::sin_q12(yaw),
        sincos::sin_q12((yaw + 1024) & 0x0fff),
        food[0] - pos[0],
        food[2] - pos[2],
    );
    let seen = in_cone && {
        let eye = prop_eye(m, pi);
        actor_line_clear(m, sight, eye, [food[0], food[1] + 8, food[2]])
    };
    Some((food, seen))
}

/// A bullsquid with no enemy: its meal (SCHED_SQUID_EAT, SCHED_SQUID_SNIFF_AND_EAT).
#[inline(never)]
#[optimize(size)]
unsafe fn squid_forage(
    m: &Map,
    movers: &[phys::Mover],
    sight: &[phys::Mover],
    pi: usize,
    nprops: usize,
) {
    let meal = SQUID_MEAL[pi];
    let scent = if meal.phase == squid_meal::Phase::None && ai_reacquire(pi) {
        nearest_carcass(m, sight, pi, nprops)
    } else {
        None
    };
    let at_food = dist2_xz(PROP_POS[pi], meal.food) <= squid_meal::REACH * squid_meal::REACH;
    let next = squid_meal::step(meal, SIM_NOW, scent, at_food);
    SQUID_MEAL[pi] = next;
    #[cfg(feature = "interaction-probe")]
    if next.phase != meal.phase {
        crate::reference_trace::probe_vals(
            SIM_NOW as u32,
            "meal",
            &[pi as i32, next.phase as i32, next.food[0], next.food[2]],
        );
    }
    match next.phase {
        squid_meal::Phase::Walk => {
            PROP_STATE[pi] = PROP_STATE_MOVE;
            prop_move_towards_point(m, movers, pi, next.food, squid_meal::WALK_SPEED);
        }
        squid_meal::Phase::Sniff | squid_meal::Phase::Eat => {
            PROP_STATE[pi] = PROP_STATE_IDLE;
            prop_face_point(pi, next.food);
        }
        squid_meal::Phase::None => {}
    }
}

/// The clip a bullsquid's meal plays (eat, sniff and walk follow its die1 in the roster).
pub(crate) unsafe fn squid_meal_clip(pi: usize) -> Option<usize> {
    match SQUID_MEAL[pi].phase {
        squid_meal::Phase::Eat => Some(8),
        squid_meal::Phase::Sniff => Some(9),
        squid_meal::Phase::Walk => Some(10),
        squid_meal::Phase::None => None,
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
        squid_forage(m, movers, sight, pi, nprops);
        return;
    };
    // An enemy (or the damage that brings one) breaks a meal.
    SQUID_MEAL[pi] = squid_meal::Meal {
        phase: squid_meal::Phase::None,
        ..SQUID_MEAL[pi]
    };
    let pos = PROP_POS[pi];
    let d2 = dist2_xz(pos, aim);
    // Low 5 bits: animation ticks left (bit 7 adds 32 for the whip's long bout), bit 5: a
    // melee bout, bit 6: the whip rather than the bite.
    let t = PROP_AI_TIMER[pi];
    let left = (t & 31) | ((t >> 2) & 32);
    if left > 0 {
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        let next = left - 1;
        PROP_AI_TIMER[pi] = (t & 0x60) | (next & 31) | ((next & 32) << 2);
        let from = prop_eye(m, pi);
        let fwd = dir_q12(pos, aim);
        let near = d2 <= 85 * 85;
        if t & 32 == 0 && left == 25 {
            let dir = dir_q12(from, aim);
            spawn_projectile_dir(PROJ_SPIT, skill_hit(Sk::SquidSpit), from, dir, true);
            cue_or(SP::BC_SPIT, sfx::HC_ATTACK, pos);
        } else if t & 96 == 96 && left == SQUID_WHIP_TICKS - 16 && near {
            // BSQUID_AE_TAILWHIP (whip frame 23): the whip's damage, the punch
            // (x 20, z -20) and a shove 200 to the squid's right and 100 up.
            cue(SP::BC_BITE, pos);
            DMG_ALWAYSGIB = true;
            damage_target(target, skill_hit(Sk::SquidWhip), pos, health, armor);
            DMG_ALWAYSGIB = false;
            if target == PROP_TARGET_PLAYER {
                set_view_punch(Some(20), None, Some(-20));
                KNOCK = [(fwd[2] * 10) >> 12, 5, -(fwd[0] * 10) >> 12];
            }
        } else if t & 96 == 32 && left == SQUID_BITE_TICKS - 5 && near {
            // BSQUID_AE_BITE (bite frame 7): the bite pulls the prey 100 in and 100 up.
            cue(SP::BC_BITE, pos);
            damage_target(target, skill_hit(Sk::SquidBite), pos, health, armor);
            if target == PROP_TARGET_PLAYER {
                KNOCK = [-(fwd[0] * 5) >> 12, 5, -(fwd[2] * 5) >> 12];
            }
        } else if t & 96 == 32 && left == SQUID_BITE_TICKS - 7 && near {
            // BSQUID_AE_THROW (bite frame 10): it flings a player 300 away and 300 up
            // and shakes his view (UTIL_ScreenShake 25, 0.7 s).
            if target == PROP_TARGET_PLAYER {
                crate::garg::shake(LOGIC_PLAYER_POS, 25, 14, 2);
                KNOCK = [(fwd[0] * 15) >> 12, 15, (fwd[2] * 15) >> 12];
            }
        }
    } else if visible && d2 <= 64 * 64 {
        // CheckMeleeAttack1 (the whip) is the finisher, used when it would kill;
        // CheckMeleeAttack2 (the bite) otherwise.
        let victim = if target == PROP_TARGET_PLAYER {
            *health
        } else {
            PROP_HEALTH[target as usize] as u16
        };
        PROP_STATE[pi] = PROP_STATE_ATTACK;
        PROP_AI_TIMER[pi] = if victim <= skill_hit(Sk::SquidWhip) as u16 {
            32 | 64 | (SQUID_WHIP_TICKS & 31) | ((SQUID_WHIP_TICKS & 32) << 2)
        } else {
            32 | SQUID_BITE_TICKS
        };
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

/// The bullsquid's bite (29 frames at 30 fps, kept at its old 24-tick bout) and
/// its whip (55 frames at 30 fps).
const SQUID_BITE_TICKS: u8 = 24;
const SQUID_WHIP_TICKS: u8 = 37;

/// The hop back's AI timer runs from `HOUND_HOP_END + HOUND_HOP_TICKS` down to
/// `HOUND_HOP_END`; the slide's stays at 30 and under.
const HOUND_HOP_END: u8 = 100;
const HOUND_HOP_TICKS: u8 = 19;

/// The houndeye is in its hop back: `prop_clip` shows jumpback.
pub(crate) unsafe fn houndeye_hopping(pi: usize, state: u8) -> bool {
    state == PROP_STATE_MOVE_AWAY && PROP_AI_TIMER[pi] > HOUND_HOP_END
}

/// The level direction straight away from `aim`, in Q12.
#[inline(never)]
#[optimize(size)]
unsafe fn hound_back(pi: usize, aim: [i32; 3]) -> [i32; 3] {
    let pos = PROP_POS[pi];
    dir_q12([aim[0], pos[1], aim[2]], pos)
}

/// monster_houndeye: runs in to 190 units, charges for 49 ticks and lets go at
/// +45 with 15 x (1 - distance / 384), multiplied by the pack (every living
/// houndeye within 1,024 units, up to four, counted at the blast). A hit aborts
/// the charge: it slides 140 units back over 30 ticks and rests 54. A blast
/// every 80 ticks otherwise.
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
    if PROP_STATE[pi] == PROP_STATE_MOVE_AWAY && left > HOUND_HOP_END {
        // SCHED_HOUND_HOP_RETREAT: jumpback (29 frames at 30 fps, 19 ticks),
        // HOUND_AE_HOPBACK at frame 6 sets 200 u/s back and 0.3 g (240 u/s)
        // up. The airborne ticks are spent in prop_air_tick, so the clip's
        // last 3 ticks run after the landing; then it takes cover (the slide).
        PROP_AI_TIMER[pi] = left - 1;
        if left == HOUND_HOP_END + HOUND_HOP_TICKS - 4 {
            let back = hound_back(pi, aim);
            PROP_JUMP[pi].start();
            PROP_JUMP[pi].launch(
                [(back[0] * 160 >> 12) as i16, 192, (back[2] * 160 >> 12) as i16],
                true,
            );
            prop_take_off(m, pi);
            // The AI sleeps through the 12 airborne ticks; 3 are left after the landing.
            PROP_AI_TIMER[pi] = HOUND_HOP_END + 3;
        } else if left == HOUND_HOP_END + 1 {
            PROP_AI_TIMER[pi] = 30;
        }
    } else if PROP_STATE[pi] == PROP_STATE_MOVE_AWAY {
        PROP_AI_TIMER[pi] = left - 1;
        prop_try_step(m, movers, pi, pos[0] - aim[0], pos[2] - aim[2], 5, true);
        if left == 1 {
            PROP_STATE[pi] = PROP_STATE_IDLE;
            prop_attack_cooldown_set(pi, 54);
        }
    } else if PROP_STATE[pi] == PROP_STATE_ATTACK {
        if PROP_HIT_FLASH[pi] > 0 {
            // CHoundeye::GetSchedule: hurt, it hops back 40% of the time when
            // 128 units behind it are clear, else takes cover.
            PROP_STATE[pi] = PROP_STATE_MOVE_AWAY;
            // UTIL_TraceHull 128 units straight back, as a line at mid height.
            let from = prop_target(PROP_KIND[pi], pos);
            let back = hound_back(pi, aim);
            let to = [from[0] + (back[0] >> 5), from[1], from[2] + (back[2] >> 5)];
            PROP_AI_TIMER[pi] = if impact_rng().below(10) < 4 && blast_line_clear(m, from, to) {
                HOUND_HOP_END + HOUND_HOP_TICKS
            } else {
                30
            };
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
            houndeye_blast(m, pi, 384, squad.min(255) as u8, pack);
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

// ---- Gonarch (monster_bigmomma) ----

/// Her mortar clock and her brood.
static mut BROOD: crate::gonarch::Brood = crate::gonarch::Brood::NEW;

#[inline(always)]
unsafe fn brood() -> &'static mut crate::gonarch::Brood {
    &mut *core::ptr::addr_of_mut!(BROOD)
}
/// The lob worked out when the mortar attack began, in sixteenths a tick.
static mut MORTAR_VEL: [i32; 3] = [0; 3];
/// Gonarch state clears with the map.
pub(crate) unsafe fn reset_gonarch() {
    BROOD = crate::gonarch::Brood::NEW;
}

/// One of her babies died (`CBigMomma::DeathNotice`).
pub(crate) unsafe fn gonarch_child_died() {
    brood().child_died();
}

/// How the node walker (`tick_boss_walker`) holds her: 0 she is walking to a
/// node and it owns her movement, 1 she waits at a node (for its delay or for
/// the damage it asks) and fights where she stands, 2 the path is over (or
/// she has none) and she hunts.
unsafe fn gonarch_phase(pi: usize) -> u8 {
    if BOSS_WALKER == u16::MAX || LOGIC_PROP_LINK[BOSS_WALKER as usize] as usize != pi {
        return 2;
    }
    match LOGIC_STATE[BOSS_WALKER as usize] {
        LOGIC_STATE_TOP => 2,
        LOGIC_STATE_WAITING | LOGIC_STATE_GOING_DOWN => 1,
        _ => 0,
    }
}

/// CBigMomma in combat: the mortar first, then the leg slash, then laying a
/// baby, else a run at the enemy. Each action plays its sequence over
/// `PROP_AI_TIMER` ticks and does its work at the sequence's event; the
/// state names which (`PROP_STATE_ATTACK` the mortar, 6 the slash, 7 the lay).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_gonarch(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
) {
    use crate::gonarch as G;
    let nprops = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
    let phase = gonarch_phase(pi);
    let timer = PROP_AI_TIMER[pi];
    let pos = PROP_POS[pi];
    if timer > 0 {
        PROP_AI_TIMER[pi] = timer - 1;
        if let Some(aim) = target_aim_point(PROP_AI_TARGET[pi], player_pos, nprops) {
            prop_face_point(pi, aim);
        }
        let left = timer - 1;
        match PROP_STATE[pi] {
            6 if left == G::SLASH_TICKS - G::SLASH_EVENT_TICK => {
                cue(SP::GON_ATTACK, pos);
                #[cfg(feature = "interaction-probe")]
                reference_trace::probe(SIM_NOW as u32, "gonarch", "slash", *health as i32);
                gonarch_slash(pi, pos, player_pos, health, armor, nprops);
            }
            7 if left == G::LAY_TICKS - G::LAY_EVENT_TICK => {
                cue(SP::GON_BIRTH, pos);
                if lay_baby(m, pi, pos) {
                    #[cfg(feature = "interaction-probe")]
                    reference_trace::probe(
                        SIM_NOW as u32,
                        "gonarch",
                        "lay",
                        brood().children as i32 + 1,
                    );
                    brood().laid(SIM_NOW, impact_rng().below(1000) as u16);
                }
            }
            PROP_STATE_ATTACK if left == G::MORTAR_TICKS - G::MORTAR_EVENT_TICK => {
                cue(SP::GON_SACK, pos);
                #[cfg(feature = "interaction-probe")]
                reference_trace::probe(SIM_NOW as u32, "gonarch", "mortar", 0);
                launch_mortar(pos);
                brood().launched(SIM_NOW, impact_rng().below(1000) as u16);
            }
            _ => {}
        }
        if left == 0 {
            PROP_STATE[pi] = PROP_STATE_IDLE;
        }
        return;
    }
    if phase == 0 {
        return;
    }
    let Some((_, visible, aim)) = acquire(m, movers, pi, player_pos, nprops, 2048) else {
        return;
    };
    let (dx, dz) = (aim[0] - pos[0], aim[2] - pos[2]);
    let dist = isqrt_i32(dx * dx + dz * dz);
    if visible {
        let yaw = prop_yaw_value(PROP_YAW[pi]);
        let (fx, fz) = (sincos::sin_q12(yaw), sincos::sin_q12((yaw + 1024) & 0x0fff));
        let in_cone = |cos| senses::within_cone(cos, fx, fz, dx, dz);
        if brood().mortar_ready(SIM_NOW, dist, in_cone(G::MORTAR_FACING_Q12)) {
            // VecCheckSplatToss: an arc whose apex is 150 to 500 units over
            // the midpoint (the ceiling caps it) that the walls leave clear.
            let start = [pos[0], pos[1] + G::MORTAR_MUZZLE_HEIGHT, pos[2]];
            let mid = [
                (start[0] + aim[0]) / 2,
                (start[1] + aim[1]) / 2,
                (start[2] + aim[2]) / 2,
            ];
            let rise = 150 + impact_rng().below(351) as i32;
            let top = [mid[0], mid[1] + rise, mid[2]];
            let apex = phys::trace_line(m, movers, mid, top).map_or(top[1], |h| h.pos[1]);
            if phys::trace_line(m, movers, start, [mid[0], apex, mid[2]]).is_none() {
                if let Some(v) = G::splat_toss(start, aim, apex) {
                    MORTAR_VEL = v;
                    PROP_STATE[pi] = PROP_STATE_ATTACK;
                    PROP_AI_TIMER[pi] = G::MORTAR_TICKS;
                    return;
                }
            }
        }
        if G::can_slash(in_cone(G::SLASH_FACING_Q12), dist) {
            PROP_STATE[pi] = 6;
            PROP_AI_TIMER[pi] = G::SLASH_TICKS;
            return;
        }
        // CanLayCrab: nothing else stands in the 64-unit square under her.
        let mut clear = true;
        let mut q = 0usize;
        while q < nprops {
            if q != pi && PROP_ACTIVE[q] != 0 && PROP_HEALTH[q] != 0 {
                let o = PROP_POS[q];
                if (o[0] - pos[0]).abs() < 32
                    && (o[2] - pos[2]).abs() < 32
                    && (o[1] - pos[1]).abs() < 64
                {
                    clear = false;
                }
            }
            q += 1;
        }
        if brood().can_lay(SIM_NOW, clear) {
            PROP_STATE[pi] = 7;
            PROP_AI_TIMER[pi] = G::LAY_TICKS;
            return;
        }
    }
    if phase == 2 {
        PROP_STATE[pi] = PROP_STATE_MOVE;
        prop_move_towards_point(m, movers, pi, aim, G::RUN_SPEED);
    } else {
        PROP_STATE[pi] = PROP_STATE_IDLE;
    }
}

/// `BIG_AE_MELEE_ATTACK1`: whatever she is fighting that stands in the box
/// 128 units in front of her (64 each side, 64 high) takes the slash; the
/// player is flung forward and up.
#[inline(never)]
#[optimize(size)]
unsafe fn gonarch_slash(
    pi: usize,
    pos: [i32; 3],
    player_pos: [i32; 3],
    health: &mut u16,
    armor: &mut u16,
    nprops: usize,
) {
    let target = PROP_AI_TARGET[pi];
    let Some(at) = target_org(target, player_pos, nprops) else {
        return;
    };
    let yaw = prop_yaw_value(PROP_YAW[pi]);
    let (fx, fz) = (sincos::sin_q12(yaw), sincos::sin_q12((yaw + 1024) & 0x0fff));
    // a victim counts by its hull (16 around the player)
    if (at[0] - pos[0] - ((fx * 128) >> 12)).abs() > 80
        || (at[2] - pos[2] - ((fz * 128) >> 12)).abs() > 80
        || at[1] < pos[1] - 16
        || at[1] > pos[1] + 80
    {
        return;
    }
    damage_target(target, skill_hit(Sk::BigMommaSlash), pos, health, armor);
    if target == PROP_TARGET_PLAYER {
        add_view_punch(171, 0); // punchangle.x = 15
                                // velocity += forward * 220 + up 200 (units a second -> a tick)
        PENDING_GAUSS_SHOVE = [(fx * 11) >> 12, 10, (fz * 11) >> 12];
    }
}

/// `BIG_AE_LAY_CRAB`: a baby appears at her feet, woken from the stock cooked
/// beside her or remade from a corpse that has lain five seconds.
#[inline(never)]
#[optimize(size)]
unsafe fn lay_baby(m: &Map, parent: usize, at: [i32; 3]) -> bool {
    let n = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
    let mut q = 0usize;
    while q < n
        && !(PROP_KIND[q] == 59
            && ((PROP_ACTIVE[q] == 0 && PROP_DORMANT[q] & PROP_RUNTIME_DORMANT_STOCK != 0)
                || (PROP_ACTIVE[q] != 0
                    && PROP_HEALTH[q] == 0
                    && SIM_NOW.wrapping_sub(PROP_DEATH_START[q]) > 100)))
    {
        q += 1;
    }
    if q >= n {
        return false;
    }
    prop_nav_cache_invalidate(q);
    PROP_HEALTH[q] = prop_start_health(59);
    PROP_DEATH_START[q] = 0; // alive: no flinch remembered yet
    PROP_STATE[q] = PROP_STATE_IDLE;
    PROP_AI_TIMER[q] = 0;
    PROP_SCRIPT_MODE[q] = 0;
    PROP_SCRIPT_LI[q] = u16::MAX;
    PROP_ANIM_CLIP[q] = PROP_ANIM_CLIP_FRESH;
    PROP_DORMANT[q] = 0;
    PROP_JUMP[q] = monsterjump::State::EMPTY;
    (*core::ptr::addr_of_mut!(PROP_FLIGHT)).release(q);
    PROP_ACTIVE[q] = 1;
    PROP_MAKER[q] = 0;
    PROP_NAME[q] = 0;
    PROP_AI_TARGET[q] = PROP_TARGET_NONE;
    // CBaseEntity::Create(.., pev->angles): the child faces where she does.
    PROP_YAW[q] = PROP_YAW[parent];
    prop_set_pos(m, &[], q, at);
    true
}

/// `BIG_AE_MORTAR_ATTACK`: the glob leaves her sack on the lob worked out when
/// the attack began.
#[inline(never)]
#[optimize(size)]
unsafe fn launch_mortar(pos: [i32; 3]) {
    let start = [
        pos[0],
        pos[1] + crate::gonarch::MORTAR_MUZZLE_HEIGHT,
        pos[2],
    ];
    let slot = spawn_projectile_dir(PROJ_BMORTAR, 0, start, [0; 3], true);
    PROJECTILES[slot].vel = MORTAR_VEL;
    PROJECTILES[slot].remainder = [0; 3];
}

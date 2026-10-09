//! monster_barnacle: the ceiling ambusher. Behaviour follows retail Half-Life as
//! measured under the Xash3D reference (c1a2, medium skill): prey that stays in
//! the tongue column is hooked, hauled up at about 53 units per second until it
//! reaches the barnacle's mouth, killed there, and chewed for two seconds. The
//! hooked player cannot walk away; shooting the barnacle drops him.

use crate::*;

/// Horizontal reach of the tongue column around the barnacle's origin.
const COLUMN_R: i32 = 28;
/// Longest tongue (units below the barnacle origin).
const REACH: i32 = 700;
/// The tongue starts growing 14 ticks after the level starts, 4 units per tick.
const TONGUE_DELAY: i32 = 14;
const TONGUE_RATE: i32 = 4;
/// Sim tick the level's first barnacle was seen on (a gap means a new level).
static mut BORN: u16 = 0;
static mut LAST_SEEN: u16 = 0;
/// Lift per tick (53 u/s at 20 Hz).
const LIFT: i32 = 3;
/// Horizontal pull toward the barnacle's axis per tick.
const PULL_XZ: i32 = 1;
/// Where the victim's origin stops, below the barnacle origin.
const PLAYER_TOP: i32 = 56;
const HUMAN_TOP: i32 = 92;
const CHEW_TICKS: u8 = 40;
/// The mouth hangs this far below the barnacle's origin, where the tongue starts.
const MOUTH: i32 = 28;

/// Where the player should stand next tick while hooked (consumed by play()).
static mut PULL: Option<[i32; 3]> = None;

/// The hook position for the player this tick, if a barnacle holds him.
#[inline(always)]
pub(crate) unsafe fn take_pull() -> Option<[i32; 3]> {
    (*core::ptr::addr_of_mut!(PULL)).take()
}

#[inline(always)]
fn in_column(b: [i32; 3], p: [i32; 3]) -> bool {
    let dx = p[0] - b[0];
    let dz = p[2] - b[2];
    dx * dx + dz * dz < COLUMN_R * COLUMN_R && p[1] < b[1] - 40 && b[1] - p[1] < REACH
}

/// The prey the tongue touches: the player first, then a human actor.
#[inline(never)]
#[optimize(size)]
unsafe fn find_prey(m: &Map, pi: usize, player_pos: [i32; 3]) -> u8 {
    let b = PROP_POS[pi];
    let tongue_len = TONGUE_RATE * (SIM_NOW.wrapping_sub(BORN) as i32 - TONGUE_DELAY);
    let tongue = [b[0], b[1] - 8, b[2]];
    if LOGIC_PLAYER_HEALTH > 0
        && !settings::debug_on(settings::DEBUG_FLY)
        && in_column(b, player_pos)
        && b[1] - player_pos[1] + PLAYER_HULL_HALF_HEIGHT <= tongue_len
        && phys::line_clear_world(
            m,
            tongue,
            [
                player_pos[0],
                player_pos[1] + PLAYER_HULL_HALF_HEIGHT,
                player_pos[2],
            ],
        )
    {
        return PROP_TARGET_PLAYER;
    }
    let nprops = PROP_COUNT.min(CARRY_MAILBOX_FIRST);
    let mut ti = 0usize;
    while ti < nprops {
        if ti != pi
            && PROP_ACTIVE[ti] != 0
            && PROP_HEALTH[ti] > 0
            && PROP_SCRIPT_MODE[ti] == 0
            && prop_is_human(PROP_KIND[ti])
            && in_column(b, PROP_POS[ti])
            && b[1] - PROP_POS[ti][1] <= tongue_len
            && phys::line_clear_world(m, tongue, prop_target(PROP_KIND[ti], PROP_POS[ti]))
        {
            return ti as u8;
        }
        ti += 1;
    }
    PROP_TARGET_NONE
}

/// One simulation tick of a barnacle: wait, hook, haul, chew.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick(m: &Map, pi: usize, player_pos: [i32; 3], health: &mut u16) {
    let b = PROP_POS[pi];
    if SIM_NOW.wrapping_sub(LAST_SEEN) > 1 {
        BORN = SIM_NOW;
    }
    LAST_SEEN = SIM_NOW;
    match PROP_STATE[pi] {
        PROP_STATE_MOVE => {
            let victim = PROP_AI_TARGET[pi];
            let (pos, top) = if victim == PROP_TARGET_PLAYER {
                if *health == 0 {
                    release(pi);
                    return;
                }
                (player_pos, PLAYER_TOP)
            } else if (victim as usize) < MAX_PROPS
                && PROP_ACTIVE[victim as usize] != 0
                && PROP_HEALTH[victim as usize] > 0
            {
                (PROP_POS[victim as usize], HUMAN_TOP)
            } else {
                release(pi);
                return;
            };
            if pos[1] + LIFT < b[1] - top {
                let step = |from: i32, to: i32| (to - from).clamp(-PULL_XZ, PULL_XZ);
                let next = [
                    pos[0] + step(pos[0], b[0]),
                    pos[1] + LIFT,
                    pos[2] + step(pos[2], b[2]),
                ];
                if victim == PROP_TARGET_PLAYER {
                    *core::ptr::addr_of_mut!(PULL) = Some(next);
                } else {
                    prop_set_pos_exact(m, victim as usize, next);
                    PROP_STATE[victim as usize] = PROP_STATE_IDLE;
                }
            } else {
                // At the mouth: the bite is lethal.
                if victim == PROP_TARGET_PLAYER {
                    *health = 0;
                } else {
                    damage_prop(victim as usize, u8::MAX, false);
                }
                PROP_STATE[pi] = PROP_STATE_ATTACK;
                PROP_AI_TIMER[pi] = CHEW_TICKS;
                sfx::play_world(sfx::HC_ATTACK, b);
            }
        }
        PROP_STATE_ATTACK => {
            PROP_AI_TIMER[pi] -= 1;
            if PROP_AI_TIMER[pi] == 0 {
                release(pi);
            }
        }
        _ => {
            let prey = find_prey(m, pi, player_pos);
            if prey != PROP_TARGET_NONE {
                PROP_AI_TARGET[pi] = prey;
                PROP_STATE[pi] = PROP_STATE_MOVE;
            }
        }
    }
}

/// The visible tongue of barnacle `pi` as (start, tip): it grows from the mouth at the
/// speed that grabs measured above until it reaches the floor, follows a hooked player's
/// head on the way up, and is drawn in (nothing) while the barnacle chews.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tongue(
    m: &Map,
    movers: &[phys::Mover],
    pi: usize,
) -> Option<([i32; 3], [i32; 3])> {
    let b = PROP_POS[pi];
    let grown = TONGUE_RATE * (SIM_NOW.wrapping_sub(BORN) as i32 - TONGUE_DELAY);
    if grown <= 0 || PROP_STATE[pi] == PROP_STATE_ATTACK {
        return None;
    }
    let from = [b[0], b[1] - MOUTH, b[2]];
    if PROP_STATE[pi] == PROP_STATE_MOVE && PROP_AI_TARGET[pi] == PROP_TARGET_PLAYER {
        let p = LOGIC_PLAYER_POS;
        return Some((from, [p[0], p[1] + PLAYER_HULL_HALF_HEIGHT, p[2]]));
    }
    let floor = crate::tripmine::beam_end(m, movers, from, [0, -4096, 0]);
    Some((
        from,
        [from[0], from[1] - grown.min(from[1] - floor[1]), from[2]],
    ))
}

#[inline(always)]
unsafe fn release(pi: usize) {
    PROP_STATE[pi] = PROP_STATE_IDLE;
    PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
    PROP_AI_TIMER[pi] = 0;
}

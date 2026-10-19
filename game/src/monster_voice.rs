//! Cries the original monsters make from IdleSound, AlertSound, PainSound and
//! DeathSound, played from the set-piece slots the map's audio bank carries
//! (hl_format::setpiece_audio). One sample per cry; a bank that could not hold
//! the sample leaves the cry silent, which the catalogue records as a limit.
//! The resident core samples (hc_pain, gr_die ...) stay where prop_voice
//! returns them; this table only holds the cries that are not in the core.

use crate::setpiece_sfx;
use hl_format::setpiece_audio as SP;

pub const ALERT: usize = 0;
pub const IDLE: usize = 1;
pub const PAIN: usize = 2;
pub const DIE: usize = 3;

const N: u8 = 0xFF;

/// (prop kind, [alert, idle, pain, die]) as set-piece slots.
static CRIES: [(u8, [u8; 4]); 17] = [
    (0, [N, N, SP::SCI_PAIN, N]),
    (2, [SP::HC_ALERT, SP::HC_IDLE, N, N]),
    (59, [SP::HC_ALERT, SP::HC_IDLE, N, N]),
    (5, [SP::ZO_ALERT, SP::ZO_IDLE, N, N]),
    (55, [SP::ZO_ALERT, SP::ZO_IDLE, N, N]),
    (6, [N, SP::HE_IDLE, N, N]),
    (7, [SP::BC_IDLE, SP::BC_IDLE, N, N]),
    (10, [SP::AG_ALERT, SP::AG_IDLE, SP::AG_PAIN, SP::AG_DIE]),
    (11, [SP::CON_ALERT, SP::CON_IDLE, SP::CON_PAIN, SP::CON_DIE]),
    (12, [N, N, N, SP::BCL_DIE]),
    (14, [N, N, N, SP::ROACH_DIE]),
    (19, [N, SP::ICHY_IDLE, SP::ICHY_PAIN, SP::ICHY_DIE]),
    (20, [SP::TU_ALERT, N, N, SP::TU_DIE]),
    (21, [SP::TU_ALERT, N, N, SP::TU_DIE]),
    (22, [SP::TU_ALERT, N, N, SP::TU_DIE]),
    (16, [N, N, N, N]),
    (18, [N, N, N, N]),
];

/// Kinds `think` has work for: the ones that cry on seeing the enemy or while idle, and the
/// walkers with npc footsteps (scientist, Barney, zombie).
// psx-numeric-allow-next-line: a constant bit mask tested with one shift, no arithmetic; the shipped profile and layout were collected on this form (the two-word form moved the layout and cost 1 to 3% on four maps). Replace with psx_math::Mask64 at the next repin.
const VOICED: u64 = (1 << 0)
    | (1 << 1)
    | (1 << 2)
    | (1 << 5)
    | (1 << 6)
    | (1 << 7)
    | (1 << 10)
    | (1 << 11)
    | (1 << 19)
    | (1 << 20)
    | (1 << 21)
    | (1 << 22)
    | (1 << 55)
    | (1 << 59);

/// One shift and test for the actors that never speak (most of a map's props).
#[inline(always)]
pub fn voiced(kind: u8) -> bool {
    kind < 64 && (VOICED >> kind) & 1 != 0
}

/// After an actor's own think: AlertSound on first sight of the enemy, the 1 in 100 per
/// think IdleSound of CBaseMonster::RunAI while idle (a think every 5 ticks), and the
/// footstep of a walker on its way.
#[inline(never)]
#[optimize(size)]
pub unsafe fn think(kind: u8, pi: usize, had_enemy: bool, in_player_pvs: bool) {
    if crate::PROP_STATE[pi] == crate::PROP_STATE_MOVE {
        step(kind, pi, crate::SIM_NOW as u32, crate::PROP_POS[pi]);
    }
    if crate::PROP_AI_TARGET[pi] != crate::PROP_TARGET_NONE {
        if !had_enemy && kind != crate::PROP_TYPE_HOUNDEYE {
            cue(kind, ALERT, crate::PROP_POS[pi]);
        }
    } else if in_player_pvs
        && crate::AI_TICK.wrapping_add(pi as u32) % 5 == 0
        && idle_roll(pi, crate::SIM_NOW as u32)
    {
        cue(kind, IDLE, crate::PROP_POS[pi]);
    }
}

/// Play `kind`'s cry for `event` at `pos`; false when it has none or the bank lacks it.
#[inline(never)]
#[optimize(size)]
pub unsafe fn cue(kind: u8, event: usize, pos: [i32; 3]) -> bool {
    let mut i = 0;
    while i < CRIES.len() {
        if CRIES[i].0 == kind {
            let slot = CRIES[i].1[event];
            if slot != N && setpiece_sfx::has(slot) {
                setpiece_sfx::play(slot, pos);
                return true;
            }
            return false;
        }
        i += 1;
    }
    false
}

/// The npc_step sounds of the walk sequence (the only gait the port animates) (model event 1004,
/// ATTN_IDLE): every 8 ticks while the actor moves, the cadence of the
/// scientist walk (frames 0 and 8 of 17 at 20 fps) and close to Barney's and
/// the zombie's.
#[inline(never)]
#[optimize(size)]
pub unsafe fn step(kind: u8, pi: usize, tick: u32, pos: [i32; 3]) {
    // Ticks between the two step events of each walk sequence: scientist 8
    // (frames 0 and 8 of 17 at 20 fps), Barney 10 (3 and 18 of 32 at 30), zombie 9.
    let every = match kind {
        0 => 8,
        1 => 10,
        5 | 55 => 9,
        _ => return,
    };
    let beat = tick.wrapping_add(pi as u32);
    if beat % every != 0 {
        return;
    }
    let id = setpiece_sfx::id(if (beat / every) & 1 == 0 {
        SP::NPC_STEP1
    } else {
        SP::NPC_STEP3
    });
    if id != u8::MAX {
        crate::sfx::play_map_authored(id, pos, 100, 0);
    }
}

/// The 1 in 100 of CBaseMonster::RunAI that picks a think for IdleSound. It
/// hashes the tick and the actor instead of drawing from `impact_rng`, whose
/// stream the rest of the simulation (and every recorded route) depends on.
#[inline]
pub fn idle_roll(pi: usize, tick: u32) -> bool {
    (tick.wrapping_mul(2_654_435_761) ^ (pi as u32).wrapping_mul(40_503).rotate_left(7)) >> 8
        & 0xffff
        < 656
}

//! Mortar field brain: where a use drops its shells, when each one lands,
//! and what a landing does. Free of PS1 state; the game feeds it through
//! [`MortarWorld`].

use crate::setpiece_math::{dist2_3, time_reached, TraceHit};
use psx_math::int32::isqrt_i32;

/// Shells that can be falling at once; a use with no free slot drops fewer.
pub const MAX_SHELLS: usize = 12;

/// The first shell of a use lands after 2.5 s (20 Hz ticks).
const SHELL_FIRST_DELAY_TICKS: u16 = 50;
/// Shells start at the top of the field and are traced this far down to find
/// the ground.
const SHELL_DROP: i32 = 4096;
/// Height of the beam each landing shows.
const BEAM_HEIGHT: i32 = 1024;
/// A landing blasts for 200 out to 500 units.
const SHELL_DAMAGE: u8 = 200;
const SHELL_RADIUS: i32 = 500;
/// A landing shakes the screen of a player within 750 units, strongest (25)
/// on top of it, for one second.
const SHAKE_RANGE: i32 = 750;
const SHAKE_AMPLITUDE: i32 = 25;
const SHAKE_TICKS: u16 = 20;

/// What the brain asks of, and tells, the game.
pub trait MortarWorld {
    /// A uniform random integer in `0..n` from the shared impact generator.
    fn random_below(&mut self, n: u32) -> u32;
    /// The 0..4096 position of the field's X (`axis` 0) or Y (`axis` 1)
    /// controller, when the map has it.
    fn controller(&mut self, axis: usize) -> Option<i32>;
    /// A line trace against the world and the moving brushes.
    fn trace_line(&mut self, from: [i32; 3], to: [i32; 3]) -> Option<TraceHit>;
    /// A vertical beam drawn from `from` to `to`.
    fn beam(&mut self, from: [i32; 3], to: [i32; 3]);
    /// An explosion of `damage` out to `radius`, credited to the player
    /// when `by_player`.
    fn explode(&mut self, at: [i32; 3], damage: u8, radius: i32, by_player: bool);
    /// Replace the player's screen shake with `amplitude` for `ticks`.
    fn set_shake(&mut self, amplitude: u16, ticks: u16);
}

/// One use of a field: the field's box (runtime axes, y up), its cooked
/// mode, shell count and spread, and who set it off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FieldUse {
    pub mins: [i32; 3],
    pub maxs: [i32; 3],
    /// 0 random spot, 1 over the activating player, 2 from the controllers.
    pub mode: u8,
    pub count: u8,
    pub spread: i32,
    pub by_player: bool,
    pub player_pos: [i32; 3],
}

/// A uniform integer in `a..=b` (just `a` when the range is empty).
fn span<W: MortarWorld + ?Sized>(w: &mut W, a: i32, b: i32) -> i32 {
    a + (w.random_below((b - a).max(0) as u32 + 1) as i32)
}

/// The shells in flight for the whole map.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MortarState {
    shell_pos: [[i32; 3]; MAX_SHELLS],
    /// The tick each shell lands on; 0 frees the slot.
    shell_at: [u16; MAX_SHELLS],
    shell_by_player: u16,
}

impl Default for MortarState {
    fn default() -> Self {
        Self::new()
    }
}

impl MortarState {
    pub const fn new() -> Self {
        Self {
            shell_pos: [[0; 3]; MAX_SHELLS],
            shell_at: [0; MAX_SHELLS],
            shell_by_player: 0,
        }
    }

    /// Forget every falling shell (a new map).
    pub fn reset(&mut self) {
        self.shell_at = [0; MAX_SHELLS];
    }

    /// Shells still falling, as (landing point, landing tick).
    #[allow(dead_code)] // read by the host tests
    pub fn pending(&self) -> impl Iterator<Item = ([i32; 3], u16)> + '_ {
        (0..MAX_SHELLS)
            .filter(|&s| self.shell_at[s] != 0)
            .map(|s| (self.shell_pos[s], self.shell_at[s]))
    }

    /// A field was used at tick `now`: pick the drop point, then schedule
    /// `count` shells scattered around it. The first lands 2.5 s later and
    /// each next one 0.2 to 0.5 s after the one before. Shells beyond the free
    /// slots are still rolled (the random stream stays the same) but dropped.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn field_use(&mut self, u: &FieldUse, now: u16, w: &mut impl MortarWorld) {
        // The random spot over the footprint is always rolled; the player and
        // controller modes then override the axes they know.
        let mut spot = [
            span(w, u.mins[0], u.maxs[0]),
            u.maxs[1],
            span(w, u.mins[2], u.maxs[2]),
        ];
        match u.mode {
            1 if u.by_player => {
                spot[0] = u.player_pos[0];
                spot[2] = u.player_pos[2];
            }
            2 => {
                for (axis, world_axis) in [(0, 0), (1, 2)] {
                    if let Some(frac) = w.controller(axis) {
                        let size = u.maxs[world_axis] - u.mins[world_axis];
                        spot[world_axis] = u.mins[world_axis] + ((frac * size) >> 12);
                    }
                }
            }
            _ => {}
        }
        let mut lands = now.wrapping_add(SHELL_FIRST_DELAY_TICKS);
        for _ in 0..u.count {
            let from = [
                spot[0] + span(w, -u.spread, u.spread),
                spot[1],
                spot[2] + span(w, -u.spread, u.spread),
            ];
            let down = [from[0], from[1] - SHELL_DROP, from[2]];
            let ground = w.trace_line(from, down).map_or(down, |hit| hit.pos);
            if let Some(slot) = self.shell_at.iter().position(|&at| at == 0) {
                self.shell_pos[slot] = ground;
                // Tick 0 marks a free slot, so a shell due then lands one later.
                self.shell_at[slot] = lands.max(1);
                let bit = 1 << slot;
                self.shell_by_player = if u.by_player {
                    self.shell_by_player | bit
                } else {
                    self.shell_by_player & !bit
                };
            }
            lands = lands.wrapping_add(4 + w.random_below(7) as u16);
        }
    }

    /// Land every shell whose time has come at tick `now`: a beam up from the
    /// impact, the blast, and a screen shake that fades out with the player's
    /// distance.
    #[inline(never)]
    #[cfg_attr(target_arch = "mips", optimize(size))]
    pub fn tick(&mut self, now: u16, player_pos: [i32; 3], w: &mut impl MortarWorld) {
        for slot in 0..MAX_SHELLS {
            let at = self.shell_at[slot];
            if at == 0 || !time_reached(now, at) {
                continue;
            }
            self.shell_at[slot] = 0;
            let pos = self.shell_pos[slot];
            w.beam(pos, [pos[0], pos[1] + BEAM_HEIGHT, pos[2]]);
            let by_player = self.shell_by_player & (1 << slot) != 0;
            w.explode(pos, SHELL_DAMAGE, SHELL_RADIUS, by_player);
            let dist = isqrt_i32(dist2_3(pos, player_pos));
            if dist < SHAKE_RANGE {
                let amplitude = SHAKE_AMPLITUDE * (SHAKE_RANGE - dist) / SHAKE_RANGE;
                w.set_shake(amplitude as u16, SHAKE_TICKS);
            }
        }
    }
}

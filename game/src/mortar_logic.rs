//! Mortar field brain: where a use drops its shells, when each one lands,
//! and what a landing does. Free of PS1 state; the game feeds it through
//! [`MortarWorld`].

use crate::setpiece_math::{dist2_3, time_reached, TraceHit};
use psx_math::int32::isqrt_i32;

/// Shells that can be falling at once; a use with no free slot drops fewer.
pub const MAX_SHELLS: usize = 12;

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

    /// A field was used at tick `now`: schedule its shells.
    pub fn field_use(&mut self, u: &FieldUse, now: u16, w: &mut impl MortarWorld) {
        let (mn, mx) = (u.mins, u.maxs);
        let mut start = [span(w, mn[0], mx[0]), mx[1], span(w, mn[2], mx[2])];
        match u.mode {
            1 if u.by_player => {
                start[0] = u.player_pos[0];
                start[2] = u.player_pos[2];
            }
            2 => {
                if let Some(f) = w.controller(0) {
                    start[0] = mn[0] + (((mx[0] - mn[0]) * f) >> 12);
                }
                if let Some(f) = w.controller(1) {
                    start[2] = mn[2] + (((mx[2] - mn[2]) * f) >> 12);
                }
            }
            _ => {}
        }
        let spread = u.spread;
        let mut t = 50u16;
        for _ in 0..u.count {
            let spot = [
                start[0] + span(w, -spread, spread),
                start[1],
                start[2] + span(w, -spread, spread),
            ];
            let down = [spot[0], spot[1] - 4096, spot[2]];
            let ground = w.trace_line(spot, down).map_or(down, |h| h.pos);
            if let Some(s) = (0..MAX_SHELLS).find(|&s| self.shell_at[s] == 0) {
                self.shell_pos[s] = ground;
                self.shell_at[s] = now.wrapping_add(t).max(1);
                self.shell_by_player =
                    (self.shell_by_player & !(1 << s)) | ((u.by_player as u16) << s);
            }
            t += 4 + w.random_below(7) as u16;
        }
    }

    /// Land every shell whose time has come at tick `now`.
    pub fn tick(&mut self, now: u16, player_pos: [i32; 3], w: &mut impl MortarWorld) {
        for s in 0..MAX_SHELLS {
            if self.shell_at[s] != 0 && time_reached(now, self.shell_at[s]) {
                self.shell_at[s] = 0;
                let p = self.shell_pos[s];
                w.beam(p, [p[0], p[1] + 1024, p[2]]);
                w.explode(p, 200, 500, self.shell_by_player & (1 << s) != 0);
                let d = isqrt_i32(dist2_3(p, player_pos));
                if d < 750 {
                    w.set_shake((25 * (750 - d) / 750) as u16, 20);
                }
            }
        }
    }
}

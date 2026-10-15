//! monster_leech: the little swimmer of c2a1a, c2a2a, c2a3, c3a1.
//!
//! The retail leech is a point-sized swimmer with two moods. Idle, it drifts
//! at 50 units per second inside the middle of its water column and now and
//! then turns. Hunting, it doubles to 100 units per second while it faces the
//! prey, chases the prey's eye height, and bites from 40 units. It thinks ten
//! times a second and only while the player can see its room. Everything here
//! is arithmetic over a [`World`], so the host test crate can run it; the game
//! supplies the traces and the water probe. The module is kept small on
//! purpose: the PS1 image has little RAM to spare.

use psx_math::sincos;

/// Concurrent swimmers. Leeches only think while the player can see their
/// room, so a map's twenty rarely need more than a handful at once; the rest
/// hold their pose until a slot frees.
pub const SLOTS: usize = 6;
/// A think is 0.1 s: every second 20 Hz tick.
pub const THINK_TICKS: u32 = 2;
/// Sight range of `Look`.
pub const LOOK_RANGE: i32 = 750;
const BITE_REACH: i32 = 40;
const SWIM_SPEED: i32 = 50;
const TURN_RATE: i32 = 90;
const CHECK_DIST: i32 = 45;
/// Ticks per second.
const HZ: i32 = 20;
/// cos 30 and sin 30 in Q12: the retail dive angle.
const COS30: i32 = 3547;
const SIN30: i32 = 2048;

/// What the leech needs to know about the world and the prey.
pub trait World {
    /// Free fraction (0..=4096, 4096 = clear) of a trace between two points.
    fn free_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> i32;
    /// Whether the point lies in water.
    fn wet(&mut self, p: [i32; 3]) -> bool;
    /// Uniform integer in `lo..=hi`.
    fn rand(&mut self, lo: i32, hi: i32) -> i32;
    /// The enemy this leech sees now (`Look(750)`), as its eye position.
    fn look(&mut self) -> Option<Enemy>;
    /// The current prey's eye position, if it still exists.
    fn enemy(&mut self) -> Option<Enemy>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Enemy {
    pub eye: [i32; 3],
    /// `waterlevel != 0` for the prey.
    pub wet: bool,
}

pub const FREE: u8 = 0xFF;

#[derive(Clone, Copy, Debug)]
pub struct Swim {
    /// Owning prop; `FREE` when the slot is unused.
    pub prop: u8,
    pub hunting: bool,
    /// -1, 0 or +1 times the turn rate while an obstacle is being avoided.
    turning: i8,
    dry: bool,
    /// Ticks until the next bite while the prey is within reach.
    bite_left: i8,
    /// Units per second along the heading, negative when backing off.
    speed: i16,
    height: i16,
    top: i16,
    bottom: i16,
    state_left: i16,
    water_left: i16,
    z_left: i16,
}

impl Swim {
    pub const EMPTY: Self = Self {
        prop: FREE,
        hunting: false,
        turning: 0,
        dry: false,
        bite_left: 0,
        speed: 0,
        height: 0,
        top: 0,
        bottom: 0,
        state_left: 0,
        water_left: 0,
        z_left: 0,
    };

    /// CLeech::Spawn plus Activate: idle for one to five seconds, then look.
    pub fn spawn<W: World>(prop: u8, pos: [i32; 3], w: &mut W) -> Self {
        let mut s = Self::EMPTY;
        s.prop = prop;
        s.state_left = secs(w.rand(1, 5));
        s.recalculate_water(pos, w);
        s
    }

    /// RecalculateWaterlevel: the water column under and over the leech with
    /// its outer fifths trimmed off, and a random swimming height inside it.
    fn recalculate_water<W: World>(&mut self, pos: [i32; 3], w: &mut W) {
        let frac = w.free_frac(pos, [pos[0], pos[1] - 400, pos[2]]);
        // One unit above the floor that was hit, else the end of the probe.
        let bottom = pos[1] - 400 * frac / 4096 + (frac < 4096) as i32;
        let top = water_surface(pos, w) - 1;
        self.bottom = ((bottom * 4 + top) / 5) as i16;
        self.top = ((bottom + top * 4) / 5) as i16;
        self.height = self.random_height(w);
        self.water_left = secs(w.rand(5, 7));
    }

    fn random_height<W: World>(&self, w: &mut W) -> i16 {
        let (lo, hi) = (self.bottom.min(self.top), self.bottom.max(self.top));
        w.rand(lo as i32, hi as i32) as i16
    }

    /// SwitchLeechState.
    fn switch_state<W: World>(&mut self, w: &mut W) {
        self.state_left = secs(w.rand(3, 6));
        if self.hunting {
            self.hunting = false;
        } else if w.look().is_some_and(|e| e.wet) {
            self.hunting = true;
            self.state_left = secs(w.rand(18, 25));
        }
    }

    /// One 0.1 s think. `yaw` is the 12-bit heading (0 = +Z, 1024 = +X).
    /// The result is the displacement and turn to apply for this think.
    pub fn think<W: World>(&mut self, pos: [i32; 3], yaw: u16, wet: bool, w: &mut W) -> Motion {
        self.state_left -= THINK_TICKS as i16;
        self.water_left -= THINK_TICKS as i16;
        self.z_left -= THINK_TICKS as i16;
        self.bite_left = self.bite_left.saturating_sub(THINK_TICKS as i8);
        let mut out = Motion::STILL;
        if !wet {
            // A leech out of the water drops and twitches.
            self.dry = true;
            self.speed = 0;
            out.dry = true;
            out.step = [0, -(GRAVITY_STEP), 0];
            return out;
        }
        if self.dry {
            // Back in the water: fly again and re-measure the column soon.
            self.dry = false;
            self.recalculate_water(pos, w);
            self.water_left = secs(2);
        }
        if self.water_left <= 0 {
            self.recalculate_water(pos, w);
        }
        if self.state_left <= 0 {
            self.switch_state(w);
        }

        let mut target_speed = SWIM_SPEED;
        let mut target_yaw = 0;
        let mut melee = false;
        let mut chasing = false;
        if self.hunting {
            if let Some(e) = w.enemy() {
                chasing = true;
                let (lo, hi) = (self.bottom.min(self.top), self.bottom.max(self.top));
                self.height = (e.eye[1] - 5).clamp(lo as i32, hi as i32) as i16;
                let to = [e.eye[0] - pos[0], e.eye[1] - pos[1], e.eye[2] - pos[2]];
                melee = length(to) < BITE_REACH;
                let diff = angle_diff_deg(heading_deg(to[0], to[2]), deg(yaw as i32));
                let limit = TURN_RATE * 3 / 4;
                target_yaw = diff.clamp(-limit, limit);
                if target_yaw == diff {
                    target_speed *= 2;
                }
            } else {
                self.switch_state(w);
            }
        } else {
            if self.z_left <= 0 {
                let fresh = self.random_height(w) as i32;
                self.height = ((self.height as i32 + fresh) / 2) as i16;
                self.z_left = secs(w.rand(1, 4));
            }
            if w.rand(0, 100) < 10 {
                target_yaw = w.rand(-30, 30);
            }
        }

        // Lean toward the target height: a dive of 30 degrees, as retail.
        let delta = self.height as i32 - pos[1];
        // Retail relaxes its lean at 6 degrees a think, so it keeps climbing a
        // few units past its 10 unit dead band; a 3 unit band stands in for it.
        let dive = if delta < -3 {
            -1
        } else if delta > 3 {
            1
        } else {
            0
        };
        let (cos, sin) = if dive == 0 { (4096, 0) } else { (COS30, SIN30) };
        let flat = flat_forward(yaw);
        // Heading with the dive, Q12.
        let dir = [flat[0] * cos >> 12, sin * dive, flat[1] * cos >> 12];
        let probe = |side: i32| {
            [
                pos[0] + (dir[0] * CHECK_DIST >> 12) + (flat[1] * side >> 12),
                pos[1] + (dir[1] * CHECK_DIST >> 12),
                pos[2] + (dir[2] * CHECK_DIST >> 12) - (flat[0] * side >> 12),
            ]
        };
        // ObstacleDistance: a wall straight ahead. The prey itself never
        // blocks (the trace does not see the player).
        let frac = w.free_frac(pos, probe(0));
        let blocked = frac < 4096 && !(chasing && (self.height as i32 - pos[1]).abs() <= 10);
        let obstacle = (frac.max(410)) as i32;
        if !blocked {
            self.turning = 0;
            self.speed = approach(target_speed, self.speed as i32, 8) as i16;
        } else {
            if self.turning == 0 {
                // Turn toward the freer side (10 units to each).
                let right_free = w.free_frac(pos, probe(10));
                let left_free = w.free_frac(pos, probe(-10));
                let delta = right_free - left_free;
                self.turning = if delta > 410 || (delta > -410 && w.rand(0, 100) < 50) {
                    -1
                } else {
                    1
                };
            }
            // Back off, faster the closer the wall.
            let step = (4096 / obstacle).clamp(1, 10);
            self.speed = approach(-(SWIM_SPEED / 2), self.speed as i32, step) as i16;
        }
        let mut rate = self.turning as i32 * TURN_RATE + target_yaw;
        if blocked {
            rate = rate * 4096 / obstacle;
        }

        if melee && self.hunting {
            out.biting = true;
            if self.bite_left <= 0 {
                // The attack sequence loops every 0.2 s.
                self.bite_left = 4;
                out.bite = true;
            }
        } else {
            self.bite_left = 2;
        }
        // Displacement over this 0.1 s think.
        let sp = self.speed as i32;
        out.step = [
            dir[0] * sp / 40960,
            dir[1] * sp / 40960,
            dir[2] * sp / 40960,
        ];
        out.turn_q12 = rate * 4096 / 360 / (HZ / 2);
        out
    }

    /// A bite landed: `m_stateTime -= 2`.
    pub fn bit(&mut self) {
        self.state_left -= secs(2);
    }
}

/// Fall per think once out of the water (a few units, growing is not worth
/// the code: the pool floor is close).
const GRAVITY_STEP: i32 = 12;

/// What a think asks of the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Units to move this think.
    pub step: [i32; 3],
    /// Heading change this think, in 12-bit units.
    pub turn_q12: i32,
    /// Playing the attack sequence.
    pub biting: bool,
    /// The bite event fired on this think.
    pub bite: bool,
    /// Out of the water: falls and twitches.
    pub dry: bool,
}

impl Motion {
    const STILL: Self = Self {
        step: [0; 3],
        turn_q12: 0,
        biting: false,
        bite: false,
        dry: false,
    };
}

/// Whether the prey is inside the front 240 degrees (`m_flFieldOfView -0.5`),
/// judged on the horizontal plane.
pub fn in_view(yaw: u16, dx: i32, dz: i32) -> bool {
    let f = flat_forward(yaw);
    let len = isqrt(dx * dx + dz * dz).max(1);
    (f[0] * dx + f[1] * dz) / len > -2048
}

/// Whether a bite connects: the prey is within 25 degrees of straight ahead
/// on the horizontal plane (dot > 0.9).
pub fn bite_connects(yaw: u16, dx: i32, dz: i32) -> bool {
    let f = flat_forward(yaw);
    let len = isqrt(dx * dx + dz * dz);
    len != 0 && (f[0] * dx + f[1] * dz) / len > 3686
}

/// UTIL_WaterLevel from the leech's origin: the first non-water height in the
/// 400 units above, by bisection; the origin itself when it is already dry.
fn water_surface<W: World>(pos: [i32; 3], w: &mut W) -> i32 {
    if !w.wet(pos) {
        return pos[1];
    }
    let (mut lo, mut hi) = (pos[1], pos[1] + 400);
    if w.wet([pos[0], hi, pos[2]]) {
        return hi;
    }
    for _ in 0..8 {
        let mid = (lo + hi) / 2;
        if w.wet([pos[0], mid, pos[2]]) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn secs(s: i32) -> i16 {
    (s * HZ) as i16
}

fn approach(target: i32, value: i32, step: i32) -> i32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

fn length(v: [i32; 3]) -> i32 {
    isqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
}

fn isqrt(n: i32) -> i32 {
    let mut remainder = n.max(0) as u32;
    let mut root = 0u32;
    let mut bit = 1u32 << 30;
    while bit > remainder {
        bit >>= 2;
    }
    while bit != 0 {
        if remainder >= root + bit {
            remainder -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root as i32
}

fn deg(q12: i32) -> i32 {
    q12 * 360 / 4096
}

fn heading_deg(dx: i32, dz: i32) -> i32 {
    deg(psx_math::atan2_q12(dx, dz) as i32)
}

/// Shortest signed difference `target - current` in degrees, in (-180, 180].
fn angle_diff_deg(target: i32, current: i32) -> i32 {
    let d = (target - current).rem_euclid(360);
    if d > 180 {
        d - 360
    } else {
        d
    }
}

/// Horizontal forward (x, z) of a heading, Q12.
fn flat_forward(yaw: u16) -> [i32; 2] {
    [
        sincos::sin_q12(yaw),
        sincos::sin_q12((yaw.wrapping_add(1024)) & 0x0fff),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box of water 400 high over a floor at y = 0 with a surface at y = 400,
    /// walls at |x|,|z| = 500, and a scripted prey.
    struct Pool {
        prey: Option<Enemy>,
        sees: bool,
        roll: u32,
        floor: i32,
        surface: i32,
    }

    impl Pool {
        fn new() -> Self {
            Self {
                prey: None,
                sees: false,
                roll: 7,
                floor: 0,
                surface: 400,
            }
        }
    }

    impl World for Pool {
        fn free_frac(&mut self, from: [i32; 3], to: [i32; 3]) -> i32 {
            // Slab test against the six faces of the pool.
            let mut best = 4096i64;
            for axis in 0..3 {
                let (lo, hi) = match axis {
                    1 => (self.floor as i64, 1000i64),
                    _ => (-500, 500),
                };
                let (a, b) = (from[axis] as i64, to[axis] as i64);
                for wall in [lo, hi] {
                    if (a - wall) * (b - wall) < 0 || b == wall {
                        let t = (wall - a) * 4096 / (b - a);
                        if (0..best).contains(&t) {
                            best = t;
                        }
                    }
                }
            }
            best as i32
        }
        fn wet(&mut self, p: [i32; 3]) -> bool {
            p[1] >= self.floor && p[1] < self.surface
        }
        fn rand(&mut self, lo: i32, hi: i32) -> i32 {
            self.roll = self.roll.wrapping_mul(1664525).wrapping_add(1013904223);
            lo + ((self.roll >> 8) % ((hi - lo + 1) as u32)) as i32
        }
        fn look(&mut self) -> Option<Enemy> {
            if self.sees {
                self.prey
            } else {
                None
            }
        }
        fn enemy(&mut self) -> Option<Enemy> {
            self.prey
        }
    }

    fn run(
        world: &mut Pool,
        s: &mut Swim,
        pos: &mut [i32; 3],
        yaw: &mut u16,
        thinks: i32,
    ) -> Vec<Motion> {
        let mut out = Vec::new();
        for _ in 0..thinks {
            let wet = world.wet(*pos);
            let m = s.think(*pos, *yaw, wet, world);
            for axis in 0..3 {
                pos[axis] += m.step[axis];
            }
            *yaw = ((*yaw as i32 + m.turn_q12).rem_euclid(4096)) as u16;
            out.push(m);
        }
        out
    }

    fn speed_of(m: &Motion) -> i32 {
        length(m.step) * 10
    }

    #[test]
    fn spawn_idles_one_to_five_seconds_then_looks() {
        let mut w = Pool::new();
        let s = Swim::spawn(3, [0, 200, 0], &mut w);
        assert!((20..=100).contains(&(s.state_left as i32)));
        assert!(!s.hunting);
        // The band is the water column with its outer fifths trimmed.
        assert!(
            s.bottom as i32 >= 70 && s.top as i32 <= 330,
            "{} {}",
            s.bottom,
            s.top
        );
    }

    #[test]
    fn idle_leech_cruises_at_fifty() {
        let mut w = Pool::new();
        let (mut pos, mut yaw) = ([0, 200, 0], 0u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.state_left = 10_000;
        s.height = 200;
        let motion = run(&mut w, &mut s, &mut pos, &mut yaw, 30);
        let speed = speed_of(motion.last().unwrap());
        assert!((40..=52).contains(&speed), "speed {speed}");
        assert!(w.wet(pos), "stays in the water: {pos:?}");
    }

    #[test]
    fn a_wet_visible_player_starts_the_hunt_a_dry_one_does_not() {
        let mut w = Pool::new();
        w.sees = true;
        w.prey = Some(Enemy {
            eye: [300, 250, 0],
            wet: false,
        });
        let (mut pos, mut yaw) = ([0, 200, 0], 1024u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.state_left = 2;
        run(&mut w, &mut s, &mut pos, &mut yaw, 1);
        assert!(!s.hunting, "a player out of the water is ignored");
        w.prey = Some(Enemy {
            eye: [300, 250, 0],
            wet: true,
        });
        s.state_left = 2;
        run(&mut w, &mut s, &mut pos, &mut yaw, 1);
        assert!(s.hunting);
        assert!(
            (18 * 20..=25 * 20).contains(&(s.state_left as i32)),
            "{}",
            s.state_left
        );
    }

    #[test]
    fn hunting_doubles_speed_when_facing_and_closes_to_bite_range() {
        let mut w = Pool::new();
        w.prey = Some(Enemy {
            eye: [300, 250, 0],
            wet: true,
        });
        let (mut pos, mut yaw) = ([0, 245, 0], 1024u16); // facing +X
        let mut s = Swim::spawn(0, pos, &mut w);
        s.hunting = true;
        s.state_left = 10_000;
        let motion = run(&mut w, &mut s, &mut pos, &mut yaw, 80);
        let fastest = motion.iter().map(speed_of).max().unwrap();
        assert!(fastest >= 90, "top speed {fastest}");
        assert!(
            motion.iter().any(|m| m.biting),
            "never reached the prey: {pos:?}"
        );
    }

    #[test]
    fn bites_come_every_fifth_of_a_second_and_each_shortens_the_hunt() {
        let mut w = Pool::new();
        w.prey = Some(Enemy {
            eye: [20, 250, 0],
            wet: true,
        });
        let (pos, yaw) = ([0, 245, 0], 1024u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.hunting = true;
        s.state_left = 10_000;
        let (mut bites, before) = (0, s.state_left);
        for _ in 0..50 {
            if s.think(pos, yaw, true, &mut w).bite {
                bites += 1;
                s.bit();
            }
        }
        // Five seconds of contact: one bite per 0.2 s, 2 s off the hunt each.
        assert!((23..=26).contains(&bites), "bites {bites}");
        assert_eq!(before - s.state_left, 100 + 40 * bites as i16);
    }

    #[test]
    fn a_wall_ahead_makes_it_back_off_and_turn() {
        let mut w = Pool::new();
        let (mut pos, mut yaw) = ([480, 200, 0], 1024u16); // 20 from the +X wall
        let mut s = Swim::spawn(0, pos, &mut w);
        s.state_left = 10_000;
        s.height = 200;
        s.speed = 50;
        let first = s.think(pos, yaw, true, &mut w);
        assert!(s.turning != 0, "should be avoiding the wall");
        assert!(first.turn_q12.abs() >= 50, "{}", first.turn_q12);
        let motion = run(&mut w, &mut s, &mut pos, &mut yaw, 40);
        assert!(pos[0] < 500, "never leaves the pool: {pos:?}");
        assert!(speed_of(motion.last().unwrap()) <= 55);
    }

    #[test]
    fn hunting_ends_when_the_state_runs_out() {
        let mut w = Pool::new();
        w.prey = Some(Enemy {
            eye: [300, 250, 0],
            wet: true,
        });
        let (mut pos, mut yaw) = ([0, 245, 0], 1024u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.hunting = true;
        s.state_left = 4;
        run(&mut w, &mut s, &mut pos, &mut yaw, 3);
        assert!(!s.hunting);
        assert!(
            (3 * 20 - 2..=6 * 20).contains(&(s.state_left as i32)),
            "{}",
            s.state_left
        );
    }

    #[test]
    fn out_of_the_water_it_stops_and_drops() {
        let mut w = Pool::new();
        let mut s = Swim::spawn(0, [0, 200, 0], &mut w);
        s.speed = 50;
        let m = s.think([0, 500, 0], 0, false, &mut w);
        assert!(m.dry && m.step[0] == 0 && m.step[1] < 0 && !m.biting);
    }

    #[test]
    fn view_cone_is_240_degrees_and_bites_need_a_facing_prey() {
        // Facing +Z: behind is outside, the side is inside.
        assert!(in_view(0, 0, 100));
        assert!(in_view(0, 100, 0));
        assert!(in_view(0, 100, -40));
        assert!(!in_view(0, 0, -100));
        assert!(bite_connects(0, 5, 100));
        assert!(!bite_connects(0, 100, 100));
    }

    #[test]
    fn it_climbs_toward_a_higher_prey_at_a_dive_angle() {
        let mut w = Pool::new();
        w.prey = Some(Enemy {
            eye: [300, 330, 0],
            wet: true,
        });
        let (mut pos, mut yaw) = ([0, 100, 0], 1024u16);
        let mut s = Swim::spawn(0, pos, &mut w);
        s.hunting = true;
        s.state_left = 10_000;
        let motion = run(&mut w, &mut s, &mut pos, &mut yaw, 20);
        assert!(motion.iter().any(|m| m.step[1] > 0));
        assert!(pos[1] > 100, "{pos:?}");
    }
}

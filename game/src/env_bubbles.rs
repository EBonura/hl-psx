//! Retail brush bubble emitter timing and use policy, independent of visuals.
pub const START_OFF: u16 = 1;
pub const INITIAL_TICKS: u16 = 40; // Spawn schedules its first think after 2 s.
pub const ENABLE_TICKS: u16 = 2; // Use schedules the first enabled think at 0.1 s.
pub fn initial_enabled(flags: u16) -> bool {
    flags & START_OFF == 0
}
pub fn interval_ticks(frequency: i32) -> u16 {
    if frequency >= 20 {
        10
    } else {
        (50i64 - frequency as i64 * 2).clamp(1, u16::MAX as i64) as u16
    }
}
pub fn use_enabled(current: bool, use_type: u8) -> bool {
    match use_type {
        0 => false,
        1 => true,
        _ => !current,
    }
}
/// Retail ShouldToggle gates the state inversion, not the subsequent think
/// scheduling. Even redundant USE_ON schedules enabled think at now + 0.1 s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scheduler {
    pub enabled: bool,
    pub next: u16,
}
impl Scheduler {
    pub fn spawn(flags: u16, now: u16) -> Self {
        let enabled = initial_enabled(flags);
        Self {
            enabled,
            next: if enabled {
                now.wrapping_add(INITIAL_TICKS)
            } else {
                0
            },
        }
    }
    pub fn use_event(&mut self, use_type: u8, now: u16) {
        self.enabled = use_enabled(self.enabled, use_type);
        self.next = if self.enabled {
            now.wrapping_add(ENABLE_TICKS)
        } else {
            0
        };
    }
    pub fn tick(&mut self, now: u16, interval: u16) -> bool {
        if !self.enabled || (now.wrapping_sub(self.next) as i16) < 0 {
            return false;
        }
        self.next = now.wrapping_add(interval.max(1));
        true
    }
}
/// Retail allocator zeros the sine multiplier and R_FizzEffect does not
/// change it. Therefore its sine branch leaves the linear base position intact.
pub fn advance_position(position_q4: &mut [i32; 3], velocity_q4: [i16; 3]) {
    for axis in 0..3 {
        position_q4[axis] += velocity_q4[axis] as i32;
    }
}
/// Half width of a bubble sprite in Q4: `base_w * 8 * scale * 256 /
/// (4096 * divisor_q8)`, which is `base_w * scale / (2 * divisor_q8)`.
pub fn half_width_q4(base_w: i32, world_scale: i32, divisor_q8: i32) -> i16 {
    psx_math::int32::mul_div_i32(base_w, world_scale, 2 * divisor_q8) as i16
}
pub fn velocity_q4(current: [i16; 2], rise: i32, world_scale: i32) -> [i16; 3] {
    [
        current[0],
        (rise * 16 * world_scale / (20 * 4096)) as i16,
        current[1],
    ]
}
/// Retail R_FizzEffect emits once even for wire density zero, then repeats
/// through the inclusive density limit. Count does not mean a volume cap.
pub fn burst_count(wire_density: u8) -> u16 {
    wire_density as u16 + 1
}
/// Original fizz particles rise at 80..140 u/s and live height/rise + 0.5 s.
pub fn lifetime_ticks(height: i32, rise: i32) -> u16 {
    // floor(20h / r) = 20 floor(h / r) + floor(20 (h mod r) / r): the first
    // term saturates (anything over 65,525 clamps), the second is below 20.
    let (height, rise) = (height.max(0) as u32, rise.max(1) as u32);
    (height / rise)
        .saturating_mul(20)
        .saturating_add(psx_math::int32::mul_div_u32(height % rise, 20, rise))
        .saturating_add(10)
        .clamp(1, u16::MAX as u32) as u16
}

pub fn advance_lifetime(remaining: &mut u16) -> bool {
    *remaining = remaining.saturating_sub(1);
    *remaining > 0
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifetime_and_sprite_width_match_the_wide_forms() {
        let wide_life = |height: i32, rise: i32| -> u16 {
            ((height.max(0) as i64 * 20 / rise.max(1) as i64) + 10).clamp(1, u16::MAX as i64) as u16
        };
        let wide_half = |base_w: i32, scale: i32, divisor: i32| -> i16 {
            ((base_w as i64 * 8 * scale as i64 * 256) / (4096 * divisor) as i64) as i16
        };
        let mut state = 0x5eed_1234u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let edges = [
            i32::MIN,
            -1000,
            -1,
            0,
            1,
            2,
            79,
            80,
            140,
            141,
            1000,
            4095,
            4096,
            65_525,
            65_526,
            1 << 20,
            107_374_182,
            107_374_183,
            i32::MAX - 1,
            i32::MAX,
        ];
        for &h in &edges {
            for &r in &edges {
                assert_eq!(lifetime_ticks(h, r), wide_life(h, r), "{h} {r}");
            }
        }
        for _ in 0..600_000 {
            let h = (next() as i32) >> (next() % 32);
            let r = if next() & 1 == 0 {
                80 + (next() % 61) as i32
            } else {
                (next() as i32) >> (next() % 32)
            };
            assert_eq!(lifetime_ticks(h, r), wide_life(h, r), "{h} {r}");
        }
        for &w in &[0, 1, 8, 16, 64, 255, 256, 65_535] {
            for &s in &[i32::MIN, -4096, 0, 1, 2048, 4096, 8192, 1 << 20, i32::MAX] {
                for d in (512..=1280).step_by(7).chain([512, 1280]) {
                    assert_eq!(half_width_q4(w, s, d), wide_half(w, s, d), "{w} {s} {d}");
                }
            }
        }
        for _ in 0..400_000 {
            let (w, s, d) = (
                (next() % 65_536) as i32,
                (next() as i32) >> (next() % 32),
                512 + (next() % 769) as i32,
            );
            assert_eq!(half_width_q4(w, s, d), wide_half(w, s, d), "{w} {s} {d}");
        }
    }
    #[test]
    fn original_startoff_toggle_and_explicit_use_preserve_state() {
        assert!(!initial_enabled(1));
        assert!(initial_enabled(0));
        assert!(use_enabled(false, 1));
        assert!(use_enabled(true, 1));
        assert!(!use_enabled(true, 0));
        assert!(!use_enabled(false, 0));
        assert!(!use_enabled(true, 3));
        assert!(use_enabled(false, 3));
        assert_eq!((INITIAL_TICKS, ENABLE_TICKS), (40, 2));
    }
    #[test]
    fn original_frequency_branch_and_wire_density_are_distinct() {
        assert_eq!(interval_ticks(3), 44);
        assert_eq!(interval_ticks(19), 12);
        assert_eq!(interval_ticks(20), 10);
        assert_eq!(interval_ticks(30), 10);
        assert_eq!(burst_count(10), 11);
        assert_eq!(burst_count(0), 1);
        assert_eq!(burst_count(255), 256);
    }
    #[test]
    fn actual_scheduler_preserves_redundant_on_off_and_wraparound() {
        let mut schedule = Scheduler::spawn(0, 100);
        assert_eq!(schedule.next, 140);
        schedule.use_event(1, 110);
        assert_eq!(schedule.next, 112);
        schedule.use_event(1, 111);
        assert_eq!(schedule.next, 113);
        assert!(!schedule.tick(112, 44));
        assert!(schedule.tick(113, 44));
        assert_eq!(schedule.next, 157);
        schedule.use_event(0, 120);
        assert_eq!(
            schedule,
            Scheduler {
                enabled: false,
                next: 0
            }
        );
        schedule.use_event(0, 121);
        assert!(!schedule.tick(200, 44));
        schedule.use_event(3, u16::MAX);
        assert_eq!(schedule.next, 1);
        assert!(!schedule.tick(0, 44));
        assert!(schedule.tick(1, 44));
        assert_eq!(schedule.next, 45);
    }
    #[test]
    fn actual_motion_preserves_current_rise_and_zero_sine_multiplier() {
        let mut position = [2504 * 16, -448 * 16, -1158 * 16];
        let velocity = velocity_q4([-80, 0], 100, 4096);
        for _ in 0..20 {
            advance_position(&mut position, velocity);
        }
        assert_eq!(position, [2404 * 16, -348 * 16, -1158 * 16]);
        assert_eq!(velocity_q4([0, 0], 80, 4096), [0, 64, 0]);
        assert_eq!(velocity_q4([0, 0], 140, 4096), [0, 112, 0]);
        assert_eq!(velocity_q4([0, 0], 100, 2048), [0, 40, 0]);
    }
    #[test]
    fn lifetime_is_height_over_rise_plus_half_second() {
        assert_eq!(lifetime_ticks(160, 80), 50);
        assert_eq!(lifetime_ticks(160, 140), 32);
        assert_eq!(lifetime_ticks(0, 80), 10);
    }
    #[test]
    fn actual_countdown_preserves_tall_volume_lifetime_above_byte_range() {
        let mut remaining = lifetime_ticks(2048, 80);
        assert_eq!(remaining, 522);
        for _ in 0..521 {
            assert!(advance_lifetime(&mut remaining));
        }
        assert_eq!(remaining, 1);
        assert!(!advance_lifetime(&mut remaining));
        assert_eq!(remaining, 0);
        assert!(!advance_lifetime(&mut remaining));
    }
}

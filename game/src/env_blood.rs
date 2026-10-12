#[derive(Clone, Copy)]
pub struct Debris {
    pub pos: [i32; 3], // 1/16 unit
    pub vel: [i16; 3], // 1/16 unit per tick
    pub ttl: u8,
    pub kind: u8,
    pub blood_sprite: u8,
    pub blood_frame: u8,
    pub blood_half_q4: i16,
}
impl Debris {
    pub const ZERO: Debris = Debris {
        pos: [0; 3],
        vel: [0; 3],
        ttl: 0,
        kind: 0,
        blood_sprite: 255,
        blood_frame: 0,
        blood_half_q4: 0,
    };
}

pub const SLOW_GRAVITY_Q4: i16 = 16;
pub const SPRAY_TICKS: u8 = 5;
pub fn advance_lifetime(drop: &mut Debris) {
    drop.ttl = drop.ttl.saturating_sub(1);
}
pub fn drop_count(scale: u8, first: u8, second: u8) -> u8 {
    scale + first + second
}
pub fn lifetime(sample: u8) -> u8 {
    20 + sample
}
pub fn velocity_q4(velocity: [i32; 3], world_scale: i32) -> [i16; 3] {
    velocity.map(|v| ((v as i64 * 16 * world_scale as i64) / (20 * 4096)) as i16)
}
pub fn admit<const N: usize>(pool: &mut [Debris; N], cursor: &mut usize, drop: Debris) {
    pool[*cursor % N] = drop;
    *cursor = (*cursor + 1) % N;
}
// Independently implemented retail blood message policy. Positions use the
// port's [original X, original Z, original Y] coordinates; directions are Q12.
pub const RANDOM: u16 = 1;
pub const STREAM: u16 = 2;
pub const PLAYER: u16 = 4;
pub const DECAL: u16 = 8;
pub fn sprite_scale(amount: u16) -> u8 {
    (if amount > 255 { 25 } else { amount / 10 }).clamp(3, 16) as u8
}
pub fn stream_amount(amount: u16) -> u8 {
    amount.min(255) as u8
}
pub fn color(palette: u16) -> (u8, u8, u8) {
    if palette == 195 {
        (203, 183, 15)
    } else {
        (103, 0, 0)
    }
}
/// PSX modulation has neutral128 rather than normalized255. Keep palette
/// RGB separate so the white original sprite cannot double its channels.
pub fn modulation(palette: u16) -> (u8, u8, u8) {
    let (r, g, b) = color(palette);
    (r.div_ceil(2), g.div_ceil(2), b.div_ceil(2))
}
pub fn position(origin: [i32; 3], eye: [i32; 3], flags: u16, jitter: [i32; 3]) -> [i32; 3] {
    if flags & PLAYER == 0 {
        origin
    } else {
        core::array::from_fn(|k| eye[k] + jitter[k])
    }
}
pub fn direction(authored: [i32; 3], flags: u16, sample: [i32; 3]) -> [i32; 3] {
    if flags & RANDOM == 0 {
        authored
    } else {
        sample
    }
}
pub fn decal_end(position: [i32; 3], direction: [i32; 3], amount: u16) -> [i32; 3] {
    core::array::from_fn(|k| position[k] + ((direction[k] as i64 * amount as i64 * 2) >> 12) as i32)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maximum_authored_burst_admits_all_drops_and_keeps_lifetime_and_motion() {
        let mut pool = [Debris::ZERO; 64];
        let mut cursor = 63;
        for _ in 0..drop_count(16, 8, 8) {
            admit(
                &mut pool,
                &mut cursor,
                Debris {
                    pos: [0; 3],
                    vel: velocity_q4([-96, 95, 95], 4096),
                    ttl: lifetime(20),
                    kind: 14 | 0x40,
                    blood_sprite: 2,
                    blood_frame: 8,
                    blood_half_q4: 80,
                },
            );
        }
        assert_eq!(pool.iter().filter(|d| d.ttl == 40).count(), 32);
        assert_eq!(cursor, 31);
        assert_eq!(drop_count(16, 1, 1), 18);
        assert_eq!(velocity_q4([-96, 95, 95], 4096), [-76, 76, 76]);
        assert_eq!(SLOW_GRAVITY_Q4, 16);
        assert_eq!(lifetime(0), 20);
        for _ in 0..40 {
            for d in &mut pool {
                advance_lifetime(d);
            }
        }
        assert!(pool.iter().all(|d| d.ttl == 0));
    }
    #[test]
    fn blood_modulation_preserves_palette_color_at_white_texels() {
        for palette in [195, 247] {
            let rgb = color(palette);
            let tint = modulation(palette);
            let rendered = |c: u8| (c as u16 * 255 / 128).min(255) as u8;
            assert_eq!((rendered(tint.0), rendered(tint.1), rendered(tint.2)), rgb);
        }
        assert_eq!(color(195), (203, 183, 15));
        assert_eq!(modulation(195), (102, 92, 8));
        assert_eq!(color(247), (103, 0, 0));
        assert_eq!(modulation(247), (52, 0, 0));
    }
    #[test]
    fn authored_yellow_burst_keeps_raw_decal_range() {
        assert_eq!(sprite_scale(2000), 16);
        assert_eq!(stream_amount(2000), 255);
        assert_eq!(color(195), (203, 183, 15));
        assert_eq!(
            decal_end([64, -176, 624], [0, 4096, 0], 2000),
            [64, 3824, 624]
        );
        assert_eq!(sprite_scale(1), 3);
        assert_eq!(sprite_scale(100), 10);
    }
    #[test]
    fn player_effect_and_decal_samples_are_independent() {
        let effect = position([64, -176, 624], [-1, 28, 2], 13, [-10, 0, 10]);
        let decal = position([64, -176, 624], [-1, 28, 2], 13, [10, -10, 0]);
        assert_eq!(effect, [-11, 28, 12]);
        assert_eq!(decal, [9, 18, 2]);
        assert_eq!(
            position([64, -176, 624], [0; 3], 0, [10; 3]),
            [64, -176, 624]
        );
    }
    #[test]
    fn random_up_axis_is_remapped_without_normalizing() {
        let d = direction([4096, 0, 0], 13, [-4096, 2048, 4096]);
        assert_eq!(d, [-4096, 2048, 4096]);
        assert_eq!(decal_end([0; 3], d, 2000), [-4000, 2000, 4000]);
        assert_eq!(direction([4096, 0, 0], 0, [0; 3]), [4096, 0, 0]);
    }
}

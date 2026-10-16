//! Air-node admission and three-axis controller movement.

use psx_math::int32::mul_div_i32;

pub const LAND: u8 = 1;
pub const AIR: u8 = 2;

pub fn accepts(node_type: u8, flying: bool) -> bool {
    node_type & if flying { AIR } else { LAND } != 0
}

pub fn distance2(a: [i32; 3], b: [i32; 3]) -> i32 {
    let d = [a[0]-b[0], a[1]-b[1], a[2]-b[2]];
    d.iter().fold(0i32, |sum, &v| sum.saturating_add(v.saturating_mul(v)))
}

#[derive(Clone, Copy)]
pub struct Flight {
    velocity_q8: [i16; 3],
    fraction_q8: [i16; 3],
}

impl Flight {
    pub const EMPTY: Self = Self { velocity_q8: [0;3], fraction_q8: [0;3] };

    /// Retail MoveExecute blends 80% prior velocity and 20% desired velocity
    /// on every movement update. Preserve the sub-unit integration remainder.
    ///
    /// The desired velocity is `d * speed * 256 / length` clamped to i16. A
    /// component never exceeds the length it is part of, so the quotient is at
    /// most `speed * 256` and fits `i32` while the speed is under 2^23 (larger
    /// speeds count as 2^23 - 1, which saturates the clamp just the same). The
    /// statement keeps the shape of the former i64 form so the shipped PGO
    /// profile still binds to this hot function's line offsets.
    pub fn candidate(&mut self, pos: [i32;3], goal: [i32;3], speed: i32) -> [i32;3] {
        let d = [goal[0]-pos[0],goal[1]-pos[1],goal[2]-pos[2]];
        let length = sqrt(distance2(pos,goal)).max(1);
        let mut out=pos;
        for axis in 0..3 {
            let desired=mul_div_i32(d[axis],speed.clamp(0,(1<<23)-1)*256,length)
                .clamp(i16::MIN as i32,i16::MAX as i32);
            let velocity=(self.velocity_q8[axis] as i32*4+desired)/5;
            self.velocity_q8[axis]=velocity as i16;
            let integrated=self.fraction_q8[axis] as i32+velocity;
            out[axis]+=integrated/256;
            self.fraction_q8[axis]=(integrated%256) as i16;
        }
        out
    }
}

fn sqrt(n: i32) -> i32 {
    let mut remainder=n.max(0) as u32;
    let mut root=0u32;
    let mut bit=1u32<<30;
    while bit>remainder { bit>>=2; }
    while bit!=0 {
        if remainder>=root+bit { remainder-=root+bit;root=(root>>1)+bit; }
        else { root>>=1; }
        bit>>=2;
    }
    root as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn air_and_land_admission_do_not_cross() {
        assert!(accepts(AIR,true));assert!(!accepts(LAND,true));
        assert!(accepts(LAND,false));assert!(!accepts(AIR,false));
    }
    #[test]
    fn movement_reaches_altitude_without_any_floor() {
        let mut flight=Flight::EMPTY;let mut pos=[0;3];
        for _ in 0..20 {pos=flight.candidate(pos,[0,200,0],6);}
        assert!(pos[1]>80);assert_eq!((pos[0],pos[2]),(0,0));
        let mut down=Flight::EMPTY;let mut p=[0;3];
        for _ in 0..20 {p=down.candidate(p,[0,-200,0],6);}
        assert_eq!(p[1],-pos[1]);
    }
    #[test]
    fn first_step_and_remainder_follow_velocity_blend() {
        let mut flight=Flight::EMPTY;
        assert_eq!(flight.candidate([0;3],[200,0,0],10),[2,0,0]);
        assert_eq!(flight.velocity_q8,[512,0,0]);
        assert_eq!(flight.candidate([2,0,0],[200,0,0],10),[5,0,0]);
        assert_eq!(flight.velocity_q8,[921,0,0]);
        assert_eq!(flight.fraction_q8,[153,0,0]);
    }
    #[test]
    fn candidate_matches_the_wide_form() {
        // The former i64 body of `Flight::candidate`, verbatim.
        let wide = |flight: &mut Flight, pos: [i32; 3], goal: [i32; 3], speed: i32| -> [i32; 3] {
            let d = [goal[0] - pos[0], goal[1] - pos[1], goal[2] - pos[2]];
            let length = sqrt(distance2(pos, goal)).max(1);
            let mut out = pos;
            for axis in 0..3 {
                let desired = (d[axis] as i64 * speed.max(0) as i64 * 256 / length as i64)
                    .clamp(i16::MIN as i64, i16::MAX as i64) as i32;
                let velocity = (flight.velocity_q8[axis] as i32 * 4 + desired) / 5;
                flight.velocity_q8[axis] = velocity as i16;
                let integrated = flight.fraction_q8[axis] as i32 + velocity;
                out[axis] += integrated / 256;
                flight.fraction_q8[axis] = (integrated % 256) as i16;
            }
            out
        };
        let mut state = 0x7f4a_7c15u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let speeds = [i32::MIN, -1, 0, 1, 2, 10, 120, 400, 3000, 65_535, 8_388_607];
        for &speed in &speeds {
            for &goal in &[[0, 0, 0], [1, 0, 0], [900, -300, 1200], [-4096, 4096, -4096], [0, 8192, 0]] {
                let (mut a, mut b) = (Flight::EMPTY, Flight::EMPTY);
                let (mut pa, mut pb) = ([5, 6, 7], [5, 6, 7]);
                for _ in 0..30 {
                    pa = a.candidate(pa, goal, speed);
                    pb = wide(&mut b, pb, goal, speed);
                    assert_eq!((pa, a.velocity_q8, a.fraction_q8), (pb, b.velocity_q8, b.fraction_q8));
                }
            }
        }
        for _ in 0..200_000 {
            let pos = [0; 3].map(|_| (next() % 16_385) as i32 - 8192);
            let goal = [0; 3].map(|_| (next() % 16_385) as i32 - 8192);
            let speed = (next() % 6000) as i32;
            let mut a = Flight::EMPTY;
            let mut b = Flight::EMPTY;
            a.velocity_q8 = [0; 3].map(|_| next() as i16);
            a.fraction_q8 = [0; 3].map(|_| (next() % 256) as i16);
            b.velocity_q8 = a.velocity_q8;
            b.fraction_q8 = a.fraction_q8;
            let (ra, rb) = (a.candidate(pos, goal, speed), wide(&mut b, pos, goal, speed));
            assert_eq!((ra, a.velocity_q8, a.fraction_q8), (rb, b.velocity_q8, b.fraction_q8));
        }
    }
    #[test]
    fn vertical_separation_is_not_a_reached_air_node() {
        assert_eq!(distance2([0;3],[0,128,0]),16384);
        assert_eq!(core::mem::size_of::<Flight>(),12);
    }
}

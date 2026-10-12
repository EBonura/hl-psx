//! Retail funnel flight on a 9 by 9 grid. Positions/velocities use Q4;
//! duration uses Q8 simulation ticks so the half-second fade stays distinct.
use psx_math::int32::isqrt_i32;

pub const PARTICLES: usize = 81;
// Retail c1a0e has two target-dispatched emitters; c3a2d/c4a3 each have one.
// The cooker rejects a map with more, so every emitter has full admission.
pub const BURSTS: usize = 2;
const FADE_Q8: u32 = 10 * 256;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Flight {
    pub velocity: [i16; 3],
    pub height: u16,
    pub duration_q8: u16,
}
impl Flight {
    pub const ZERO: Self = Self { velocity: [0; 3], height: 0, duration_q8: 0 };
    pub fn new(index: usize, height: u16, extra_speed: u16, source_y: i32,
               reverse: bool, scale_q12: u16) -> Option<Self> {
        if index >= PARTICLES || !(100..=800).contains(&height)
            || !(64..=128).contains(&extra_speed) || scale_q12 == 0 { return None; }
        let offset = grid(index, height);
        let distance = isqrt_i32(offset.iter().map(|v|v*v).sum()).max(1);
        let start_y = source_y + if reverse {0} else {height as i32};
        let speed = (start_y / 8).max(64) + extra_speed as i32;
        let sign = if reverse {1} else {-1};
        let mut velocity = [0; 3];
        for axis in 0..3 {
            let value = offset[axis] as i64 * sign * speed as i64 * scale_q12 as i64 * 16
                / (distance as i64 * 20 * 4096);
            velocity[axis] = i16::try_from(value).ok()?;
        }
        Some(Self {velocity, height,
            duration_q8: u16::try_from(distance as i64 * 20 * 256 / speed as i64).ok()?})
    }
    pub fn remaining_q8(&self, age: u16) -> u32 {
        (self.duration_q8 as u32).saturating_sub(age as u32 * 256)
    }
    pub fn opacity(&self, age: u16) -> u8 {
        // Retail die is arrival minus 0.5 seconds, fadeSpeed is 2, and initial
        // render amount is 200. Short flights therefore start already fading.
        (self.remaining_q8(age).min(FADE_Q8) * 200 / FADE_Q8) as u8
    }
    pub fn position_q4(&self, origin: [i32; 3], index: usize, reverse: bool,
                       scale_q12: u16, age: u16) -> [i32; 3] {
        let offset = if reverse {[0; 3]} else {grid(index,self.height)};
        core::array::from_fn(|axis| origin[axis] * 16
            + (offset[axis] as i64 * scale_q12 as i64 * 16 / 4096) as i32
            + self.velocity[axis] as i32 * age as i32)
    }
}
fn grid(index: usize, height: u16) -> [i32; 3] {
    [-256 + (index / 9) as i32 * 64, height as i32,
     -256 + (index % 9) as i32 * 64]
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Burst {
    pub origin: [i32; 3],
    pub age: u16,
    pub scale_q12: u16,
    pub sprite: u8,
    pub reverse: bool,
    pub flights: [Flight; PARTICLES],
}
impl Burst {
    pub const ZERO: Self = Self {origin:[0;3],age:0,scale_q12:0,sprite:255,
        reverse:false,flights:[Flight::ZERO;PARTICLES]};
    pub fn advance(&mut self) {
        if self.sprite == 255 {return;}
        self.age = self.age.saturating_add(1);
        if self.flights.iter().all(|f|f.remaining_q8(self.age)==0) {self.sprite=255;}
    }
}
pub fn admit(pool: &mut [Burst; BURSTS], burst: Burst) -> bool {
    if let Some(slot)=pool.iter_mut().find(|b|b.sprite==255) {*slot=burst;true}
    else {false}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_grid_has_all_81_unique_points_and_both_direction_policies() {
        let mut points=std::collections::BTreeSet::new();
        for index in 0..PARTICLES {
            assert!(points.insert(grid(index,100)));
            let inward=Flight::new(index,100,64,-1000,false,4096).unwrap();
            let outward=Flight::new(index,100,64,-1000,true,4096).unwrap();
            assert_eq!(inward.velocity,outward.velocity.map(|v|-v));
            assert_eq!(outward.position_q4([1,2,3],index,true,4096,0),[16,32,48]);
        }
        assert_eq!(points.len(),81);
        assert!(points.contains(&[-256,100,-256]) && points.contains(&[256,100,256]));
    }
    #[test]
    fn flight_ends_after_its_half_second_fade_not_half_second_after_arrival() {
        let f=Flight::new(40,800,64,-1000,false,4096).unwrap();
        assert_eq!(f.duration_q8,32000); // 800/128 seconds, 125 ticks
        assert_eq!(f.opacity(114),200);
        assert_eq!(f.opacity(120),100);
        assert_eq!(f.opacity(125),0);
        let short=Flight::new(40,100,128,2592,true,4096).unwrap();
        assert!(short.opacity(0)>0 && short.opacity(0)<200);
    }
    #[test]
    fn speed_uses_absolute_initial_source_height_before_world_scaling() {
        let inward=Flight::new(40,800,128,2592,false,4096).unwrap();
        let reverse=Flight::new(40,800,128,2592,true,4096).unwrap();
        assert_eq!(inward.duration_q8,(800*20*256/552) as u16);
        assert_eq!(reverse.duration_q8,(800*20*256/452) as u16);
        let scaled=Flight::new(40,800,128,2592,true,2048).unwrap();
        assert_eq!(scaled.duration_q8,reverse.duration_q8);
        assert_eq!(scaled.velocity[1],reverse.velocity[1]/2);
    }
    #[test]
    fn two_complete_bursts_are_admitted_without_overwriting_either() {
        assert_eq!(core::mem::size_of::<Flight>(),10);
        let mut pool=[Burst::ZERO;BURSTS];
        let mut burst=Burst::ZERO;burst.sprite=0;
        for index in 0..PARTICLES {burst.flights[index]=Flight::new(index,800,64,-1000,false,4096).unwrap();}
        assert!(admit(&mut pool,burst));assert!(admit(&mut pool,burst));
        assert!(!admit(&mut pool,burst));
        assert_eq!(pool.iter().flat_map(|b|b.flights.iter()).filter(|f|f.remaining_q8(0)>0).count(),162);
        for _ in 0..200 {for b in &mut pool {b.advance();}}
        assert!(pool.iter().all(|b|b.sprite==255));
        assert!(admit(&mut pool,burst));
    }
}

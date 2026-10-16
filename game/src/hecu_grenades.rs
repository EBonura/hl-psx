//! Authored HECU grenade equipment, simulation deadlines and ballistic plans.
use psx_math::int32::isqrt_i32;
pub const HAND: u8 = 2;
pub const LAUNCHER: u8 = 4;
pub const DEFAULT_WEAPONS: u8 = 3;
pub const CHECK_TICKS: u16 = 20;
pub const RECOVERY_TICKS: u16 = 120;
pub fn recovery_ticks(launcher: bool, hard: bool, roll: u16) -> u16 {
    if launcher && hard {40+roll.min(60)} else {RECOVERY_TICKS}
}
pub const HAND_FUSE_TICKS: u8 = 70;
pub const TOSS_RELEASE_TICKS: u8 = 24; // retail frame36 /30fps
pub const TOSS_ANIMATION_TICKS: u8 = 37; // retail55 intervals /30fps
pub const LAUNCH_RELEASE_TICKS: u8 = 11; // retail frame16 /30fps
pub const LAUNCH_ANIMATION_TICKS: u8 = 30;
pub fn release_due(remaining: u8, duration: u8, release_tick: u8) -> bool {
    remaining == duration - release_tick + 1
}
pub fn advance(velocity_q4: &mut [i32;3], remainder: &mut [i8;3]) -> [i32;3] {
    velocity_q4[1] -= 8;
    let mut step=[0;3];
    for axis in 0..3 {
        let value=velocity_q4[axis]+remainder[axis] as i32;
        step[axis]=value/16;
        remainder[axis]=(value%16) as i8;
    }
    velocity_q4[1] -= 8;
    step
}
const CLOCK_MASK: u16 = 0x0fff;

pub fn normalize_weapons(value: u8) -> u8 { if value == 0 { DEFAULT_WEAPONS } else { value & 15 } }
pub fn clock(weapons: u8, deadline: u16) -> u16 {
    ((normalize_weapons(weapons) as u16) << 12) | (deadline & CLOCK_MASK)
}
pub fn weapons(clock: u16) -> u8 { normalize_weapons((clock >> 12) as u8) }
pub fn carry_state(state: u8, weapons: u8) -> u8 { (state & 15) | (normalize_weapons(weapons) << 4) }
pub fn carry_weapons(state: u8) -> u8 { normalize_weapons(state >> 4) }
pub fn due(clock: u16, now: u16) -> bool {
    (now.wrapping_sub(clock & CLOCK_MASK) & CLOCK_MASK) < 0x0800
}
pub fn reschedule(value: u16, now: u16, delay: u16) -> u16 { clock(weapons(value), now.wrapping_add(delay)) }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attack { Hand, Launcher }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionStep { Inactive, Hold, Release(Attack) }
/// Advance the action fields used by the production grunt adapter. Airborne
/// environment motion owns the shared velocity field and cancels this action.
pub fn action_step(state: &mut u8, remaining: &mut u8, airborne: bool) -> ActionStep {
    let attack=match *state {6=>Attack::Hand,7=>Attack::Launcher,_=>return ActionStep::Inactive};
    if airborne { *state=0; *remaining=0; return ActionStep::Inactive; }
    let (duration,release)=match attack {
        Attack::Hand=>(TOSS_ANIMATION_TICKS,TOSS_RELEASE_TICKS),
        Attack::Launcher=>(LAUNCH_ANIMATION_TICKS,LAUNCH_RELEASE_TICKS),
    };
    let releasing=release_due(*remaining,duration,release);
    *remaining=remaining.saturating_sub(1);
    if *remaining==0 {*state=0;}
    if releasing {ActionStep::Release(attack)} else {ActionStep::Hold}
}
pub fn attack(weapons: u8) -> Option<Attack> {
    let value = normalize_weapons(weapons);
    if value & HAND != 0 { Some(Attack::Hand) }
    else if value & LAUNCHER != 0 { Some(Attack::Launcher) } else { None }
}
pub fn safe_target(origin: [i32;3], target: [i32;3]) -> bool {
    let dx=target[0]-origin[0];
    let dz=target[2]-origin[2];
    dx*dx+dz*dz > 256*256
}
pub fn ally_clear(origin: [i32;3], target: [i32;3]) -> bool {
    let delta=[0,1,2].map(|axis|origin[axis]-target[axis]);
    delta.iter().map(|n|n*n).sum::<i32>() > 256*256
}
#[derive(Clone, Copy, Debug)]
pub struct Plan { pub velocity_q4: [i16;3], pub apex: [i32;3] }
fn sqrt(value: i64) -> Option<i32> {
    let value=i32::try_from(value).ok()?;
    if value < 0 {None} else {Some(isqrt_i32(value))}
}
fn velocity(values: [i32;3]) -> Option<[i16;3]> {
    Some([i16::try_from(values[0]).ok()?,i16::try_from(values[1]).ok()?,i16::try_from(values[2]).ok()?])
}
/// Half of800u/s² gravity is one world unit per20Hz tick². Times use Q8;
/// velocities use Q4. Caller supplies original midpoint ceiling trace minus15.
/// Offsets stay under a few thousand units, so every product below fits 32 bits.
pub fn toss(start: [i32;3], target: [i32;3], apex_y: i32) -> Option<Plan> {
    if target[1]-start[1] > 500 {return None;}
    let up=apex_y-start[1];
    let down=apex_y-target[1];
    let t1=sqrt(up as i64*2*256*256)?;
    let t2=sqrt(down as i64*2*256*256)?;
    if t1 < 512 || t2 == 0 {return None;}
    let total=t1+t2;
    let delta=[0,1,2].map(|axis|target[axis]-start[axis]);
    let v=velocity([delta[0]*16*256/total,t1*16/256,delta[2]*16*256/total])?;
    let apex=[start[0]+delta[0]*t1/total,apex_y,start[2]+delta[2]*t1/total];
    Some(Plan {velocity_q4:v,apex})
}
/// Original fixed-speed throw: flight time is distance/speed, vertical launch
/// compensates half-gravity. Caller checks both segments through the apex.
pub fn launch(start: [i32;3], target: [i32;3], speed_per_second: u16) -> Option<Plan> {
    if speed_per_second == 0 {return None;}
    let delta=[0,1,2].map(|axis|target[axis]-start[axis]);
    let distance=sqrt(delta.iter().map(|&n|n as i64*n as i64).sum())?;
    let time=distance*20*256/speed_per_second as i32;
    if time == 0 {return None;}
    let v=velocity([delta[0]*16*256/time,delta[1]*16*256/time+time*8/256,delta[2]*16*256/time])?;
    let fall=((time as i64*time as i64)>>19) as i32; // time*time/(8*256*256)
    let apex=[start[0]+delta[0]/2,start[1]+delta[1]/2+fall,start[2]+delta[2]/2];
    Some(Plan {velocity_q4:v,apex})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn authored_equipment_and_deadline_wrap() {
        assert_eq!(attack(0),Some(Attack::Hand));
        assert_eq!(attack(3),Some(Attack::Hand));
        assert_eq!(attack(5),Some(Attack::Launcher));
        assert_eq!(attack(8),None); assert_eq!(attack(1),None);
        assert_eq!(attack(10),Some(Attack::Hand));
        assert_eq!(carry_weapons(carry_state(2,5)),5);
        assert_eq!(carry_weapons(2),3); // old transient records had no equipment bits
        let c=clock(10,4094);assert!(!due(c,4093));assert!(due(c,4094));
        let c=reschedule(c,4094,120);assert_eq!(weapons(c),10);
        assert!(!due(c,4095));assert!(!due(c,117));assert!(due(c,118));
        assert_eq!(recovery_ticks(true,true,0),40);
        assert_eq!(recovery_ticks(true,true,60),100);
        assert_eq!(recovery_ticks(false,true,0),120);
        assert_eq!(recovery_ticks(true,false,0),120);
    }
    #[test] fn ballistic_plans_and_original_self_clearance() {
        let p=toss([0,48,0],[512,0,0],256).unwrap();
        assert!(p.velocity_q4[0]>0 && p.velocity_q4[1]>0);
        assert!(p.apex[0]>0 && p.apex[0]<512);
        assert!(toss([0,48,0],[512,600,0],700).is_none());
        assert!(toss([0,48,0],[512,0,0],48).is_none());
        assert!(!safe_target([0,0,0],[256,0,0]));assert!(safe_target([0,0,0],[257,0,0]));
        assert!(!ally_clear([600,0,0],[512,0,0])); // partner behind target, clear arc
        assert!(!ally_clear([512,0,200],[512,0,0])); // partner beside target
        assert!(ally_clear([512,300,0],[512,0,0]));
        let slow=launch([0,48,0],[512,0,0],400).unwrap();
        let fast=launch([0,48,0],[512,0,0],800).unwrap();
        assert!(slow.apex[1]>fast.apex[1]);assert!(slow.velocity_q4[0]<fast.velocity_q4[0]);
    }
    #[test] fn authored_animation_events_release_once_on_exact_simulation_tick() {
        for (duration,release) in [(TOSS_ANIMATION_TICKS,TOSS_RELEASE_TICKS),
            (LAUNCH_ANIMATION_TICKS,LAUNCH_RELEASE_TICKS)] {
            let events=(1..=duration).filter(|tick|
                release_due(duration-tick+1,duration,release)).collect::<std::vec::Vec<_>>();
            assert_eq!(events,[release]);
        }
    }
    #[test] fn production_action_adapter_releases_once_and_cancels_environment_conflict() {
        for (mut state,duration,release,attack) in [(6,TOSS_ANIMATION_TICKS,TOSS_RELEASE_TICKS,Attack::Hand),
            (7,LAUNCH_ANIMATION_TICKS,LAUNCH_RELEASE_TICKS,Attack::Launcher)] {
            let mut remaining=duration;
            for tick in 1..=duration {
                assert_eq!(action_step(&mut state,&mut remaining,false),
                    if tick==release {ActionStep::Release(attack)} else {ActionStep::Hold});
            }
            assert_eq!((state,remaining),(0,0));
            assert_eq!(action_step(&mut state,&mut remaining,false),ActionStep::Inactive);
        }
        let mut state=6;let mut remaining=TOSS_ANIMATION_TICKS-TOSS_RELEASE_TICKS+1;
        assert_eq!(action_step(&mut state,&mut remaining,true),ActionStep::Inactive);
        assert_eq!((state,remaining),(0,0));
        for original in [2,3] {
            let mut state=original;let mut timer=0x56;
            assert_eq!(action_step(&mut state,&mut timer,false),ActionStep::Inactive);
            assert_eq!((state,timer),(original,0x56));
        }
    }
    #[test] fn fractional_ballistics_preserve_half_gravity_and_signed_motion() {
        let mut velocity=[17,160,-17];let mut remainder=[0;3];let mut pos=[0;3];
        for _ in 0..10 {
            let step=advance(&mut velocity,&mut remainder);
            for axis in 0..3 {pos[axis]+=step[axis];}
        }
        assert_eq!(pos,[10,50,-10]);
        assert_eq!(velocity,[17,0,-17]);
        assert_eq!(remainder,[10,0,-10]);
    }
}

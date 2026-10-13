//! Sentry contact/damage schedules deployment after the original 0.1 seconds.
pub const KIND: u8 = 20;
pub const DELAY_TICKS: u16 = 2;

pub fn schedule(kind: u8, active: bool, health: u8, inactive: bool, now: u16) -> Option<u16> {
    if kind == KIND && active && health != 0 && inactive {
        Some(now.wrapping_add(DELAY_TICKS))
    } else { None }
}

pub fn due(health: u8, inactive: bool, pending: bool, now: u16, deadline: u16) -> bool {
    health != 0 && inactive && pending && now.wrapping_sub(deadline) < 0x8000
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn damage_and_player_contact_schedule_without_changing_health() {
        assert_eq!(schedule(KIND,true,100,true,10),Some(12));
        assert_eq!(schedule(KIND,true,80,true,11),Some(13)); // repeated callback resets nextthink
        assert_eq!(schedule(21,true,100,true,10),None);
        assert_eq!(schedule(KIND,false,100,true,10),None);
        assert_eq!(schedule(KIND,true,0,true,10),None);
        assert_eq!(schedule(KIND,true,100,false,10),None);
    }
    #[test]
    fn activation_deadline_killing_damage_and_wrap_preserve_ordering() {
        assert!(!due(80,true,true,10,12));
        assert!(!due(80,true,true,11,12));
        assert!(due(80,true,true,12,12));
        assert!(!due(0,true,true,12,12));
        assert!(!due(80,true,false,12,12));
        assert!(!due(80,false,true,12,12));
        assert_eq!(schedule(KIND,true,80,true,65535),Some(1));
        assert!(!due(80,true,true,0,1));
        assert!(due(80,true,true,1,1));
    }
}

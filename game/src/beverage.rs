//! CEnvBeverage / CItemSoda rules that do not depend on the renderer.

pub const READY_TICKS: u16 = 10; // CanThink at 0.5 seconds, fixed update 20 Hz.

#[inline]
pub fn can_dispense(stock: i16, waiting_can: bool) -> bool {
    stock > 0 && !waiting_can
}

#[inline]
pub fn heal(health: u16, maximum: u16) -> u16 {
    // TakeHealth does not reduce health that is already above the maximum.
    if health < maximum {
        health + 1
    } else {
        health
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_can_blocks_stock_until_consumed() {
        assert!(can_dispense(2, false));
        assert!(!can_dispense(1, true));
        assert!(can_dispense(1, false));
        assert!(!can_dispense(0, false));
        assert!(!can_dispense(-1, false));
    }

    #[test]
    fn one_health_without_lowering_full_or_bonus_health() {
        assert_eq!(heal(98, 100), 99);
        assert_eq!(heal(99, 100), 100);
        assert_eq!(heal(100, 100), 100);
        assert_eq!(heal(125, 100), 125);
    }
}

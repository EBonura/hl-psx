//! G-Man damage response: stay alive while retaining authored damage reactions.

pub const PROP_KIND: u8 = 15;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Response {
    pub health: u8,
    pub light_damage: bool,
    pub heavy_damage: bool,
}

/// The supplied retail actor restores half of max health on every damage
/// call; positive hits signal light damage, and hits of at least 20 also
/// signal heavy damage. None of these calls kills him or creates a corpse.
pub fn take_damage(max_health: u8, damage: u8) -> Response {
    Response {
        health: (max_health / 2).max(1),
        light_damage: damage > 0,
        heavy_damage: damage >= 20,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retail_health_reset_and_damage_conditions_survive_repeated_hits() {
        assert_eq!(
            take_damage(100, 0),
            Response {
                health: 50,
                light_damage: false,
                heavy_damage: false
            }
        );
        assert_eq!(
            take_damage(100, 19),
            Response {
                health: 50,
                light_damage: true,
                heavy_damage: false
            }
        );
        assert_eq!(
            take_damage(100, 20),
            Response {
                health: 50,
                light_damage: true,
                heavy_damage: true
            }
        );
        for _ in 0..32 {
            assert_eq!(
                take_damage(100, 255),
                Response {
                    health: 50,
                    light_damage: true,
                    heavy_damage: true
                }
            );
        }
    }
}

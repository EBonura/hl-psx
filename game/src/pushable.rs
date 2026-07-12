//! Pure packed state/math for `func_pushable` brush entities.
//!
//! Runtime position lives in the existing `ENT_CACHE.origin`; this word uses
//! the existing `ENT_PHASE` slot for signed planar velocity, supporting mover,
//! and a one-bit trigger/PVS refresh marker. No per-cart BSS is needed.

pub const SUPPORT_NONE: u16 = u16::MAX;
pub const SUPPORT_WORLD: u16 = u16::MAX - 1;
pub const CONTACT_NONE: u8 = 0;
pub const CONTACT_PUSH: u8 = 1;
pub const CONTACT_PULL: u8 = 2;

const SUPPORT_SHIFT: u32 = 16;
const SUPPORT_MASK: u32 = 0x1ff;
const DIRTY_BIT: u32 = 1 << 25;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct State {
    pub vx: i8,
    pub vz: i8,
    pub vy: i8,
    pub support: u16,
    pub dirty: bool,
}

#[inline(always)]
pub const fn pack(vx: i8, vz: i8, vy: i8, support: u16, dirty: bool) -> i32 {
    let support_code = if support == SUPPORT_NONE {
        0
    } else if support == SUPPORT_WORLD {
        1
    } else if support < 510 {
        support + 2
    } else {
        0
    };
    let vy = if vy < -32 {
        -32
    } else if vy > 31 {
        31
    } else {
        vy
    };
    ((vx as u8 as u32)
        | ((vz as u8 as u32) << 8)
        | ((support_code as u32) << SUPPORT_SHIFT)
        | if dirty { DIRTY_BIT } else { 0 }
        | (((vy as i32 & 0x3f) as u32) << 26)) as i32
}

#[inline(always)]
pub const fn unpack(word: i32) -> State {
    let bits = word as u32;
    let support_code = ((bits >> SUPPORT_SHIFT) & SUPPORT_MASK) as u16;
    let vy6 = ((bits >> 26) & 0x3f) as i8;
    State {
        vx: bits as u8 as i8,
        vz: (bits >> 8) as u8 as i8,
        vy: if vy6 & 0x20 != 0 { vy6 - 64 } else { vy6 },
        support: if support_code == 0 {
            SUPPORT_NONE
        } else if support_code == 1 {
            SUPPORT_WORLD
        } else {
            support_code - 2
        },
        dirty: bits & DIRTY_BIT != 0,
    }
}

#[inline]
fn isqrt(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n as u32;
    let mut result = 0u32;
    let mut bit = 1u32 << 30;
    while bit > x {
        bit >>= 2;
    }
    while bit != 0 {
        if x >= result + bit {
            x -= result + bit;
            result = (result >> 1) + bit;
        } else {
            result >>= 1;
        }
        bit >>= 2;
    }
    result as i32
}

/// Euclidean-clamp a planar velocity to the cooked GoldSrc maximum.
pub fn clamp_velocity(vx: i32, vz: i32, max_speed: i32) -> (i8, i8) {
    let max_speed = max_speed.clamp(0, i8::MAX as i32);
    if max_speed == 0 {
        return (0, 0);
    }
    let len2 = vx.saturating_mul(vx).saturating_add(vz.saturating_mul(vz));
    if len2 <= max_speed * max_speed {
        return (vx.clamp(-127, 127) as i8, vz.clamp(-127, 127) as i8);
    }
    // Floor sqrt can normalize an oblique vector just outside the circle to
    // another vector still outside it (10,3 -> 9,2 at max 9). Divide by the
    // integer ceiling so the packed velocity can never exceed MaxSpeed.
    let mut len = isqrt(len2).max(1);
    if len * len < len2 {
        len += 1;
    }
    (
        (vx * max_speed / len).clamp(-127, 127) as i8,
        (vz * max_speed / len).clamp(-127, 127) as i8,
    )
}

/// Add the player's push velocity and clamp it to the entity's max speed.
pub fn accelerate(state: State, wish_x: i32, wish_z: i32, max_speed: i32) -> State {
    let (vx, vz) = clamp_velocity(
        state.vx as i32 + wish_x,
        state.vz as i32 + wish_z,
        max_speed,
    );
    State {
        vx,
        vz,
        vy: state.vy,
        dirty: state.dirty,
        support: state.support,
    }
}

/// One-unit ground drag once the player is no longer touching the cart.
pub const fn decay(state: State) -> State {
    State {
        vx: if state.vx > 0 {
            state.vx - 1
        } else if state.vx < 0 {
            state.vx + 1
        } else {
            0
        },
        vz: if state.vz > 0 {
            state.vz - 1
        } else if state.vz < 0 {
            state.vz + 1
        } else {
            0
        },
        vy: state.vy,
        dirty: state.dirty,
        support: state.support,
    }
}

/// Apply one tick of vertical gravity only while the cart has no support.
pub const fn fall_step(state: State, gravity: i8) -> State {
    State {
        vx: state.vx,
        vz: state.vz,
        vy: if state.support == SUPPORT_NONE {
            let next = state.vy as i16 - gravity as i16;
            if next < -32 {
                -32
            } else if next > 31 {
                31
            } else {
                next as i8
            }
        } else {
            0
        },
        support: state.support,
        dirty: state.dirty,
    }
}

/// Classify the zero-state GoldSrc cart contact without any runtime storage.
/// A normal side push must move toward the cart; holding +use enables pulling
/// in either wish direction, but neither path accepts an airborne player or a
/// player standing on the cart.
pub const fn contact_mode(
    grounded: bool,
    standing_on_cart: bool,
    touching: bool,
    use_held: bool,
    has_wish: bool,
    toward: i32,
) -> u8 {
    if !grounded || standing_on_cart || !touching || !has_wish {
        CONTACT_NONE
    } else if use_held {
        CONTACT_PULL
    } else if toward > 0 {
        CONTACT_PUSH
    } else {
        CONTACT_NONE
    }
}

/// CPushable::Use applies a non-player-touch factor of 0.25. Preserve at
/// least one fixed-point unit for a non-zero stick component so low analog
/// input still converges instead of quantizing the pull away entirely.
pub const fn pull_component(wish: i32) -> i32 {
    if wish > 0 {
        if wish < 4 {
            1
        } else {
            wish / 4
        }
    } else if wish < 0 {
        if wish > -4 {
            -1
        } else {
            wish / 4
        }
    } else {
        0
    }
}

/// Leave a small Q12 guard before a traced wall plane. This converts a point
/// sweep hit into a conservative cart-face clamp and prevents rounding from
/// taking the full step through thin geometry.
pub const fn safe_hit_fraction(hit_fraction: i32) -> i32 {
    if hit_fraction <= 32 {
        0
    } else if hit_fraction >= 4096 {
        4064
    } else {
        hit_fraction - 32
    }
}

/// Apply a Q12 sweep fraction; a blocked trace can never take the full delta.
#[inline(always)]
pub const fn swept_component(delta: i32, fraction: i32) -> i32 {
    let fraction = if fraction < 0 {
        0
    } else if fraction > 4096 {
        4096
    } else {
        fraction
    };
    (delta * fraction) >> 12
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_state_round_trips_signed_velocity_support_and_dirty() {
        let state = unpack(pack(-9, 7, -12, 148, true));
        assert_eq!(state.vx, -9);
        assert_eq!(state.vz, 7);
        assert_eq!(state.vy, -12);
        assert_eq!(state.support, 148);
        assert!(state.dirty);
        assert_eq!(
            unpack(pack(0, 0, 0, SUPPORT_NONE, false)).support,
            SUPPORT_NONE
        );
        assert_eq!(
            unpack(pack(0, 0, 0, SUPPORT_WORLD, false)).support,
            SUPPORT_WORLD
        );
    }

    #[test]
    fn friction_220_speed_clamps_to_nine_units_per_tick() {
        let state = State {
            vx: 0,
            vz: 0,
            vy: 0,
            support: SUPPORT_NONE,
            dirty: false,
        };
        let pushed = accelerate(state, 30, 0, 9);
        assert_eq!((pushed.vx, pushed.vz), (9, 0));
        let diagonal = accelerate(state, 9, 9, 9);
        assert!(
            diagonal.vx as i32 * diagonal.vx as i32 + diagonal.vz as i32 * diagonal.vz as i32 <= 81
        );
        let oblique = clamp_velocity(10, 3, 9);
        assert_eq!(oblique, (8, 2));
        assert!(oblique.0 as i32 * oblique.0 as i32 + oblique.1 as i32 * oblique.1 as i32 <= 81);
    }

    #[test]
    fn drag_converges_and_sweep_never_tunnels_to_full_delta() {
        let state = State {
            vx: -2,
            vz: 1,
            vy: 0,
            support: 3,
            dirty: true,
        };
        assert_eq!((decay(state).vx, decay(state).vz), (-1, 0));
        assert_eq!(swept_component(9, 2048), 4);
        assert_eq!(swept_component(-9, 2048), -5);
        assert_eq!(swept_component(9, 0), 0);
        assert_eq!(safe_hit_fraction(16), 0);
        assert_eq!(safe_hit_fraction(2048), 2016);
        assert!(swept_component(9, safe_hit_fraction(2048)) < 9);
    }

    #[test]
    fn only_grounded_side_contact_pushes_and_use_enables_pull() {
        assert_eq!(
            contact_mode(true, false, true, false, true, 10),
            CONTACT_PUSH
        );
        assert_eq!(
            contact_mode(true, false, true, false, true, -10),
            CONTACT_NONE
        );
        assert_eq!(
            contact_mode(true, false, true, true, true, -10),
            CONTACT_PULL
        );
        assert_eq!(
            contact_mode(true, false, true, true, true, 10),
            CONTACT_PULL
        );
        assert_eq!(
            contact_mode(false, false, true, true, true, -10),
            CONTACT_NONE
        );
        assert_eq!(
            contact_mode(true, true, true, true, true, -10),
            CONTACT_NONE
        );
        assert_eq!(
            contact_mode(true, false, false, true, true, -10),
            CONTACT_NONE
        );
        assert_eq!(pull_component(9), 2);
        assert_eq!(pull_component(-9), -2);
        assert_eq!(pull_component(1), 1);
    }

    #[test]
    fn unsupported_cart_falls_but_world_or_mover_support_cancels_fall() {
        let unsupported = State {
            vx: 0,
            vz: 0,
            vy: 0,
            support: SUPPORT_NONE,
            dirty: false,
        };
        assert_eq!(fall_step(unsupported, 2).vy, -2);
        assert_eq!(fall_step(fall_step(unsupported, 2), 2).vy, -4);

        let world = State {
            vy: -20,
            support: SUPPORT_WORLD,
            ..unsupported
        };
        assert_eq!(fall_step(world, 2).vy, 0);
        let lift = State {
            vy: -20,
            support: 148,
            ..unsupported
        };
        assert_eq!(fall_step(lift, 2).vy, 0);
    }
}

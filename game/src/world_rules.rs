//! Integer-only rules of world objects (floating pushables, rotating doors,
//! track waypoints, blood decals), kept free of PS1 state so the host runner
//! can pin their behaviour. The game side gathers observations from its
//! globals, calls these steps and applies the results.

/// A floating pushable as the buoyancy step sees it: the world centre of its
/// box, half its height, and its buoyancy factor (the authored skin value).
/// World Y is up.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FloatBody {
    pub center: [i32; 3],
    pub half_height: i32,
    pub buoyancy: i32,
}

/// What one buoyancy tick decided: the new vertical speed in whole units per
/// 20 Hz tick, and whether the pushable still rests on its support.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FloatTick {
    pub vy: i32,
    pub grounded: bool,
}

/// One 20 Hz buoyancy tick for a pushable. `vy` is the stored vertical speed
/// before gravity; `grounded` says whether it rests on a support. `wet`
/// answers whether a world point is inside water. Returns None when the
/// pushable has no buoyancy or its bottom is dry, so the caller falls as usual.
pub fn pushable_float_tick(
    body: FloatBody,
    vy: i32,
    grounded: bool,
    wet: &mut dyn FnMut([i32; 3]) -> bool,
) -> Option<FloatTick> {
    if body.buoyancy == 0 {
        return None;
    }
    let resting_vy = if grounded { 0 } else { vy };
    let c = body.center;
    let h = body.half_height;
    let bottom = c[1] - h;
    if !wet([c[0], bottom + 1, c[2]]) {
        return None;
    }
    let depth = if wet([c[0], c[1] + h, c[2]]) {
        2 * h
    } else {
        let (mut lo, mut hi) = (1, 2 * h);
        let mut i = 0;
        while i < 5 {
            let mid = (lo + hi) / 2;
            if wet([c[0], bottom + mid, c[2]]) {
                lo = mid;
            } else {
                hi = mid;
            }
            i += 1;
        }
        (lo + hi) / 2
    };
    // Gravity (800 u/s^2) balances the lift at depth 800 / buoyancy. The
    // speed is whole units per 20 Hz tick, so ease toward that depth rather
    // than integrating both forces undamped, which would swing the box.
    let balance = 800 / body.buoyancy.max(1);
    let toward = ((depth - balance) / 4).clamp(-8, 8);
    let next = (resting_vy + toward) / 2;
    // Lift that beats gravity raises a grounded box off its floor; otherwise
    // a grounded box stays put.
    if next > 0 {
        Some(FloatTick {
            vy: next,
            grounded: false,
        })
    } else if grounded {
        Some(FloatTick {
            vy: 0,
            grounded: true,
        })
    } else {
        Some(FloatTick {
            vy: next,
            grounded: false,
        })
    }
}

/// Rotating-door spawnflags that keep the authored swing direction.
pub const DOOR_ONE_WAY: u16 = 16;
pub const DOOR_ROTATE_Z: u16 = 64;
pub const DOOR_ROTATE_X: u16 = 128;

/// Whoever started the chain that opens a door: their position and the
/// horizontal direction they face as world (x, z), 1.0 = 4096.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DoorActivator {
    pub pos: [i32; 3],
    pub forward_xz: [i32; 2],
}

/// Which way a rotating door swings as it starts to open. Some(true) means
/// against its authored direction, Some(false) along it, and None leaves the
/// current direction alone (one-way and X/Z-axis doors, or nobody started
/// the chain).
pub fn rotating_door_opens_reversed(
    spawnflags: u16,
    hinge: [i32; 3],
    activator: Option<DoorActivator>,
) -> Option<bool> {
    if spawnflags & (DOOR_ONE_WAY | DOOR_ROTATE_Z | DOOR_ROTATE_X) != 0 {
        return None;
    }
    let a = activator?;
    let fwd = a.forward_xz;
    let dx = (a.pos[0] - hinge[0]).clamp(-32767, 32767);
    let dz = (a.pos[2] - hinge[2]).clamp(-32767, 32767);
    let cross = dx * fwd[1] - dz * fwd[0];
    Some(cross < 0)
}

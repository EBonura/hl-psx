//! Small integer helpers shared by the set-piece brains (`mortar_logic`,
//! `tank_logic`, `osprey_logic`, `apache_logic`, `garg_logic`,
//! `nihilanth_logic`). They are the game crate's own generic helpers,
//! repeated here so the brains stay free of `main.rs` and the host runner can
//! include them by path.

/// A line trace that stopped short of its end: the q12 fraction along the
/// segment, the impact point and the q12 plane normal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TraceHit {
    pub frac: i32,
    pub pos: [i32; 3],
    pub normal: [i32; 3],
}

/// Has the wrapping 20 Hz tick `now` reached `at`? (Half the u16 range
/// counts as the past.)
#[inline]
pub const fn time_reached(now: u16, at: u16) -> bool {
    now.wrapping_sub(at) < 0x8000
}

/// Squared distance between two points.
#[inline]
pub const fn dist2_3(a: [i32; 3], b: [i32; 3]) -> i32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

/// Squared horizontal (x/z) distance between two points.
#[inline]
pub const fn dist2_xz(a: [i32; 3], b: [i32; 3]) -> i32 {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    dx * dx + dz * dz
}

/// Signed atan2 in q12 turns from +x toward +y, within about 0.25 degrees
/// (scientist_logic's 0.273-bend fit); the SDK's octant-linear atan2_q12 is
/// up to 4 degrees off, which walks rounds off a distant target.
#[inline(never)]
#[cfg_attr(target_arch = "mips", optimize(size))]
pub fn atan_s(y: i32, x: i32) -> i32 {
    ((crate::scientist_logic::precise_yaw_from_vec(y, x) as i32 + 2048) & 0xfff) - 2048
}

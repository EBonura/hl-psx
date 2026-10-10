//! Where a bullet lands on an actor changes what it does.
//!
//! Studio models tag every hitbox with a hit group. The retail game scales the
//! damage of a bullet by the group it strikes: a head hit multiplies by
//! `sk_monster_head`, the HECU helmet and the alien grunt's armour plates take
//! 20 off first. The cooker (`hl-content hitgroups`) writes each roster model's
//! runs of boxes per group; the game's build script keeps the runs named by
//! [`RULES`] and [`scale`] turns a hit into damage. Measured under the Xash3D
//! reference (glock 8, .357 40): zombie, vortigaunt and soldier heads take 24
//! and 120, the helmet takes 0 and 60, the agrunt's plates 0 and 20.

/// `HITGROUP_HEAD`.
pub const HEAD: u8 = 1;
/// The alien grunt's armour plates.
pub const ARMOR: u8 = 10;
/// The HECU grunt's helmet.
pub const HELMET: u8 = 11;
/// What armour and helmets absorb from one bullet.
pub const ABSORB: u16 = 20;

/// (actor model type, hit group) pairs the game acts on: the humanoid
/// walkers, whose groups were measured or share the studio layout of one that
/// was. Other models keep every box at face value until measured.
pub const RULES: &[(u8, u8)] = &[
    (0, HEAD),
    (1, HEAD),
    (5, HEAD),
    (8, HEAD),
    (8, HELMET),
    (9, HEAD),
    (10, HEAD),
    (10, ARMOR),
    (25, HEAD),
    (51, HEAD),
    (54, HEAD),
    (55, HEAD),
];

/// The damage of a bullet of `dmg` after it strikes `group`, with
/// `sk_monster_head` as `head`.
pub const fn scale(group: u8, dmg: u16, head: u16) -> u16 {
    match group {
        HEAD => dmg * head,
        HELMET => dmg.saturating_sub(ABSORB) * head,
        ARMOR => dmg.saturating_sub(ABSORB),
        _ => dmg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_measured_hits() {
        // glock (8) and .357 (40) on a head, a helmet and agrunt plates.
        assert_eq!([scale(HEAD, 8, 3), scale(HEAD, 40, 3)], [24, 120]);
        assert_eq!([scale(HELMET, 8, 3), scale(HELMET, 40, 3)], [0, 60]);
        assert_eq!([scale(ARMOR, 8, 3), scale(ARMOR, 40, 3)], [0, 20]);
        assert_eq!(scale(0, 8, 3), 8);
    }
}

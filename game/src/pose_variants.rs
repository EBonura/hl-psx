//! Which cooked clip a death or a flinch plays, for the species that keep more than one.
//!
//! `CBaseMonster::GetDeathActivity` picks a region death from the last hit group
//! (`ACT_DIE_HEADSHOT`, `ACT_DIE_GUTSHOT`), else a death from the attack direction
//! (`ACT_DIEFORWARD` when the attacker is behind, `ACT_DIEBACKWARD` when in front, within
//! a dot of 0.3), and `ACT_DIESIMPLE` when the model has no such sequence or when there is
//! no room to fall that way (a 64-unit trace; the caller runs it, `needs_room`).
//! `CBaseMonster::GetSmallFlinchActivity` picks a limb flinch from the hit group the same
//! way. `LookupActivity` takes one of the sequences carrying the activity at random,
//! weighted by each one's activity weight.
//!
//! The slots are positions in host/hl-content/model-roster.txt (the five AI clips, then
//! the named clips in order); the test at the bottom holds them to the roster.

/// A cooked clip slot and its activity weight.
pub type Weighted = (u8, u8);

/// The death and flinch clips one species keeps beyond its slot-4 death and slot-3 flinch.
pub struct Variants {
    pub kind: u8,
    pub simple: &'static [Weighted],
    pub forward: &'static [Weighted],
    pub backward: &'static [Weighted],
    pub head: &'static [Weighted],
    pub gut: &'static [Weighted],
    /// Flinch slots by hit group 4-7 (left arm, right arm, left leg, right leg); 0: none.
    pub limbs: [u8; 4],
}

/// The death clip's base slot (main.rs PROP_CLIP_DEAD).
const DEAD: u8 = 4;

pub const TABLE: [Variants; 4] = [
    // monster_zombie: diesimple (slot 4), diebackward, dieforward, dieheadshot,
    // dieheadshot2 (ACT_DIE_GUTSHOT), then the four limb flinches (attack2 is slot 5).
    Variants {
        kind: 5,
        simple: &[(DEAD, 1)],
        forward: &[(7, 1)],
        backward: &[(6, 1)],
        head: &[(8, 1)],
        gut: &[(9, 1)],
        limbs: [10, 11, 12, 13],
    },
    // monster_houndeye: die and die_crumple (ACT_DIESIMPLE), die1 forward, die2 and die3
    // backward (weights 2 and 1).
    Variants {
        kind: 6,
        simple: &[(DEAD, 1), (10, 1)],
        forward: &[(7, 1)],
        backward: &[(8, 2), (9, 1)],
        head: &[],
        gut: &[],
        limbs: [0; 4],
    },
    // monster_bullchicken: slot 4 is `die` (ACT_DIEFORWARD); die1 is its ACT_DIESIMPLE.
    Variants {
        kind: 7,
        simple: &[(7, 1)],
        forward: &[(DEAD, 1)],
        backward: &[],
        head: &[],
        gut: &[],
        limbs: [0; 4],
    },
    // monster_alien_slave: diesimple (slot 4), diebackward, dieforward, dieheadshot, then
    // the four limb flinches (attack1 is slot 5).
    Variants {
        kind: 9,
        simple: &[(DEAD, 1)],
        forward: &[(7, 1)],
        backward: &[(6, 1)],
        head: &[(8, 1)],
        gut: &[],
        limbs: [9, 10, 11, 12],
    },
];

/// The zombie's heavy swing (`attack2`, ZOMBIE_AE_ATTACK_BOTH).
pub const ZOMBIE_ATTACK2: u8 = 5;

/// Hit groups (`m_LastHitGroup`).
pub const GROUP_HEAD: u8 = 1;
pub const GROUP_STOMACH: u8 = 3;

/// Where the attacker stands seen from the victim: `GetDeathActivity`'s flDot over 0.3
/// (behind) or at most -0.3 (in front), else neither.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Attacker {
    Behind,
    Front,
    Side,
}

/// Bits of a chosen variant (main.rs PROP_POSE_VARIANT): the slot in the low five bits, a
/// death with [`DEATH`], and the room check still owed for a fall forward or backward.
pub const SLOT: u8 = 31;
pub const DEATH: u8 = 0x80;
pub const ROOM_FORWARD: u8 = 0x40;
pub const ROOM_BACKWARD: u8 = 0x20;

pub fn variants(kind: u8) -> Option<&'static Variants> {
    let mut i = 0;
    while i < TABLE.len() {
        if TABLE[i].kind == kind {
            return Some(&TABLE[i]);
        }
        i += 1;
    }
    None
}

/// `LookupActivity`: one of `list` by weight, from `roll` (any value; taken modulo the total).
fn pick(list: &[Weighted], roll: u32) -> Option<u8> {
    let total: u32 = list.iter().map(|&(_, w)| w as u32).sum();
    if total == 0 {
        return None;
    }
    let mut r = roll % total;
    for &(slot, weight) in list {
        if r < weight as u32 {
            return Some(slot);
        }
        r -= weight as u32;
    }
    None
}

/// The death a species with variants plays (`GetDeathActivity`), as a [`DEATH`] variant with
/// its room check, or None for a species with no variants.
pub fn death(kind: u8, group: u8, from: Attacker, roll: u32) -> Option<u8> {
    let v = variants(kind)?;
    let directional = |from: Attacker| match from {
        Attacker::Behind => pick(v.forward, roll).map(|s| s | ROOM_FORWARD),
        Attacker::Front => pick(v.backward, roll).map(|s| s | ROOM_BACKWARD),
        Attacker::Side => None,
    };
    let region = match group {
        GROUP_HEAD => pick(v.head, roll),
        GROUP_STOMACH => pick(v.gut, roll),
        _ => None,
    };
    let chosen = match group {
        // A region death; without one, the direction is tried.
        GROUP_HEAD | GROUP_STOMACH => region.or_else(|| directional(from)),
        _ => directional(from),
    };
    let slot = chosen.or_else(|| pick(v.simple, roll))?;
    Some(slot | DEATH)
}

/// The simple death a variant that found no room falls back to.
pub fn simple(kind: u8, roll: u32) -> u8 {
    variants(kind)
        .and_then(|v| pick(v.simple, roll))
        .unwrap_or(DEAD)
        | DEATH
}

/// The limb flinch for hit group `group` (`GetSmallFlinchActivity`), or None for the
/// species' small flinch.
pub fn flinch(kind: u8, group: u8) -> Option<u8> {
    let v = variants(kind)?;
    match group {
        4..=7 => Some(v.limbs[(group - 4) as usize]).filter(|&s| s != 0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot each named clip takes in a roster line: the five AI clips, then each
    /// named token without `=` in order.
    fn roster_slots(kind: u8) -> Vec<String> {
        let roster = include_str!("../../host/hl-content/model-roster.txt");
        let line = roster
            .lines()
            .find(|line| line.split('|').next() == Some(&kind.to_string()))
            .expect("roster line");
        let spec = line.split('|').nth(2).expect("spec");
        spec.split(',')
            .filter(|token| !token.contains('='))
            .map(|token| token.split(':').next().unwrap().to_string())
            .collect()
    }

    #[test]
    fn slots_name_the_roster_clips() {
        let expect: [(u8, &[(u8, &str)]); 4] = [
            (
                5,
                &[
                    (ZOMBIE_ATTACK2, "attack2"),
                    (6, "diebackward"),
                    (7, "dieforward"),
                    (8, "dieheadshot"),
                    (9, "dieheadshot2"),
                    (10, "laflinch"),
                    (11, "raflinch"),
                    (12, "llflinch"),
                    (13, "rlflinch"),
                ],
            ),
            (
                6,
                &[
                    (7, "die1"),
                    (8, "die2"),
                    (9, "die3"),
                    (10, "die_crumple"),
                    (11, "jumpwindow"),
                ],
            ),
            (7, &[(4, "16"), (7, "die1")]),
            (
                9,
                &[
                    (6, "diebackward"),
                    (7, "dieforward"),
                    (8, "dieheadshot"),
                    (9, "laflinch"),
                    (10, "raflinch"),
                    (11, "llflinch"),
                    (12, "rlflinch"),
                ],
            ),
        ];
        for (kind, slots) in expect {
            let names = roster_slots(kind);
            for &(slot, name) in slots {
                assert_eq!(names[slot as usize], name, "type {kind} slot {slot}");
            }
        }
    }

    #[test]
    fn region_then_direction_then_simple() {
        // Zombie head shot: dieheadshot whatever the direction.
        assert_eq!(death(5, GROUP_HEAD, Attacker::Front, 0), Some(8 | DEATH));
        // Vortigaunt gut shot: no gut death, so the direction.
        assert_eq!(
            death(9, GROUP_STOMACH, Attacker::Behind, 0),
            Some(7 | DEATH | ROOM_FORWARD)
        );
        // Generic hit from the side: the simple death.
        assert_eq!(death(5, 2, Attacker::Side, 0), Some(4 | DEATH));
        // Bullsquid shot in front: no backward death, so die1.
        assert_eq!(death(7, 0, Attacker::Front, 0), Some(7 | DEATH));
        // A species without variants keeps its one death.
        assert_eq!(death(2, GROUP_HEAD, Attacker::Front, 0), None);
    }

    #[test]
    fn weights_pick_among_sequences() {
        // Houndeye backward: die2 twice as often as die3.
        let picks: Vec<u8> = (0..3)
            .map(|r| death(6, 0, Attacker::Front, r).unwrap() & SLOT)
            .collect();
        assert_eq!(picks, [8, 8, 9]);
        assert_eq!(simple(6, 1), 10 | DEATH);
    }

    #[test]
    fn limb_flinches_by_hit_group() {
        assert_eq!(flinch(5, 4), Some(10));
        assert_eq!(flinch(9, 7), Some(12));
        assert_eq!(flinch(5, GROUP_HEAD), None);
        assert_eq!(flinch(6, 5), None);
    }
}

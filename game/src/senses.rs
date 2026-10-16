//! What the monsters can tell about the world: who counts as a friend or an
//! enemy, how wide each one sees, how fast each one turns, and what noises
//! carry to them.
//!
//! Pure integer rules with no hardware, included by the host runner
//! (`hl-logic-tests`). The numbers are the retail game's rules: the class of
//! each creature and the table of how the classes feel about one another, the
//! field of view and yaw speed each species is spawned with, and the loudness
//! of the sounds a fight makes.

// ---- classes and the relationship table ----

pub const CLASS_NONE: u8 = 0;
pub const CLASS_MACHINE: u8 = 1;
pub const CLASS_PLAYER: u8 = 2;
pub const CLASS_HUMAN_PASSIVE: u8 = 3;
pub const CLASS_HUMAN_MILITARY: u8 = 4;
pub const CLASS_ALIEN_MILITARY: u8 = 5;
pub const CLASS_ALIEN_PASSIVE: u8 = 6;
pub const CLASS_ALIEN_MONSTER: u8 = 7;
pub const CLASS_ALIEN_PREY: u8 = 8;
pub const CLASS_ALIEN_PREDATOR: u8 = 9;
pub const CLASS_INSECT: u8 = 10;
pub const CLASS_PLAYER_ALLY: u8 = 11;
pub const CLASS_PLAYER_BIOWEAPON: u8 = 12;
pub const CLASS_ALIEN_BIOWEAPON: u8 = 13;
const N_CLASSES: usize = 14;

/// How one creature feels about another (`IRelationship`): the higher, the
/// more it wants to hurt it. Only a feeling above `R_NO` makes a target.
pub const R_AL: i8 = -1; // ally
pub const R_FR: i8 = 0; // afraid of
pub const R_NO: i8 = 1; // no feeling
pub const R_DL: i8 = 2; // dislikes
pub const R_HT: i8 = 3; // hates
pub const R_NM: i8 = 4; // nemesis

const T_NO: i8 = R_NO;
const T_AL: i8 = R_AL;
const T_FR: i8 = R_FR;
const T_DL: i8 = R_DL;
const T_HT: i8 = R_HT;

/// `FEELING[from][to]` over the 14 classes, in class order.
#[rustfmt::skip]
const FEELING: [[i8; N_CLASSES]; N_CLASSES] = [
    /* none          */ [T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO],
    /* machine       */ [T_NO, T_NO, T_DL, T_DL, T_NO, T_DL, T_DL, T_DL, T_DL, T_DL, T_NO, T_DL, T_DL, T_DL],
    /* player        */ [T_NO, T_DL, T_NO, T_NO, T_DL, T_DL, T_DL, T_DL, T_DL, T_DL, T_NO, T_NO, T_DL, T_DL],
    /* human passive */ [T_NO, T_NO, T_AL, T_AL, T_HT, T_FR, T_NO, T_HT, T_DL, T_FR, T_NO, T_AL, T_NO, T_NO],
    /* human mil.    */ [T_NO, T_NO, T_HT, T_DL, T_NO, T_HT, T_DL, T_DL, T_DL, T_DL, T_NO, T_HT, T_NO, T_NO],
    /* alien mil.    */ [T_NO, T_DL, T_HT, T_DL, T_HT, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_DL, T_NO, T_NO],
    /* alien passive */ [T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO],
    /* alien monster */ [T_NO, T_DL, T_DL, T_DL, T_DL, T_NO, T_NO, T_NO, T_NO, T_NO, T_NO, T_DL, T_NO, T_NO],
    /* alien prey    */ [T_NO, T_NO, T_DL, T_DL, T_DL, T_NO, T_NO, T_NO, T_NO, T_FR, T_NO, T_DL, T_NO, T_NO],
    /* alien predator*/ [T_NO, T_NO, T_DL, T_DL, T_DL, T_NO, T_NO, T_NO, T_HT, T_DL, T_NO, T_DL, T_NO, T_NO],
    /* insect        */ [T_FR, T_FR, T_FR, T_FR, T_FR, T_NO, T_FR, T_FR, T_FR, T_FR, T_NO, T_FR, T_NO, T_NO],
    /* player ally   */ [T_NO, T_DL, T_AL, T_AL, T_DL, T_DL, T_DL, T_DL, T_DL, T_DL, T_NO, T_NO, T_NO, T_NO],
    /* player bioweap*/ [T_NO, T_NO, T_DL, T_DL, T_DL, T_DL, T_DL, T_DL, T_DL, T_DL, T_NO, T_DL, T_NO, T_DL],
    /* alien bioweap */ [T_NO, T_NO, T_DL, T_DL, T_DL, T_AL, T_NO, T_DL, T_DL, T_NO, T_NO, T_DL, T_DL, T_NO],
];

/// Who the player is, as a "kind" in the actor tables.
pub const KIND_PLAYER: u8 = 255;

/// The class of an actor kind (the port's model type id); 0 for props and
/// scenery that nobody reacts to.
pub const fn class_of(kind: u8) -> u8 {
    match kind {
        KIND_PLAYER => CLASS_PLAYER,
        0 | 25 | 54 => CLASS_HUMAN_PASSIVE,       // scientists
        1 => CLASS_PLAYER_ALLY,                   // Barney
        2 | 59 => CLASS_ALIEN_PREY,               // headcrabs
        5 | 55 | 6 | 12 | 16 | 18 | 19 | 50 => CLASS_ALIEN_MONSTER,
        7 => CLASS_ALIEN_PREDATOR,                // bullsquid
        8 | 23 | 51 => CLASS_HUMAN_MILITARY,      // grunts, assassins, the apache
        9 | 10 | 11 | 17 | 58 => CLASS_ALIEN_MILITARY,
        13 | 14 => CLASS_INSECT,                  // leech, cockroach
        20 | 21 | 22 | 61 => CLASS_MACHINE,       // sentries, turrets, the osprey
        _ => CLASS_NONE,
    }
}

/// How `from` feels about `to`, with the individual overrides on top of the
/// class table: marines and alien grunts are each other's nemesis, and marines
/// have the same for the gargantua.
pub const fn relation(from: u8, to: u8) -> i8 {
    if from == 8 && (to == 10 || to == 16) {
        return R_NM;
    }
    if from == 10 && to == 8 {
        return R_NM;
    }
    FEELING[class_of(from) as usize][class_of(to) as usize]
}

/// A feeling that makes the other a target.
pub const fn is_enemy(feeling: i8) -> bool {
    feeling > R_NO
}

// ---- eyes ----

/// The cosine (q12) of the half-angle of each species' view cone
/// (`m_flFieldOfView`): the zombie and headcrabs 0.5, grunts and bullsquids
/// 0.2, the wide-eyed (scientists, Barney, vortigaunts) -0.7, the turrets and
/// controllers all around.
pub const fn fov_cos_q12(kind: u8) -> i16 {
    match kind {
        2 | 59 | 5 | 55 | 6 | 12 | 14 => 2048,  // 0.5
        7 | 8 | 10 => 819,                      // 0.2
        18 => 1229,                             // 0.3
        58 => 0,                                // 0.0
        23 | 61 => -2896,                       // -0.707
        0 | 1 | 9 | 19 | 25 | 51 | 54 => -2867, // -0.7
        13 => -2048,                            // -0.5
        16 => -819,                             // -0.2
        11 | 17 | 20 | 21 | 22 => -4096,        // all around
        _ => -4096,
    }
}

/// How far monsters look (`m_flDistLook`).
pub const LOOK_DISTANCE: i32 = 2048;

/// Whether a point lies inside the view cone of an actor looking along the
/// unit vector (`fx`, `fz`) (q12): the retail test is the flat dot product of
/// the direction to the point against the facing.
#[optimize(size)]
pub fn in_view_cone(kind: u8, fx: i32, fz: i32, dx: i32, dz: i32) -> bool {
    let cos = fov_cos_q12(kind) as i32;
    if cos <= -4096 {
        return true;
    }
    // Scale the offset down to 11 bits a side so every product stays in 32 bits.
    let (mut dx, mut dz) = (dx, dz);
    while dx.abs() > 2048 || dz.abs() > 2048 {
        dx >>= 1;
        dz >>= 1;
    }
    if dx == 0 && dz == 0 {
        return true;
    }
    let len = psx_math::int32::isqrt_i32(dx * dx + dz * dz) as i32;
    // dot / (len * 4096) >= cos / 4096   <=>   dot >= cos * len
    dx * fx + dz * fz >= cos * len
}

// ---- turning ----

/// Yaw per 20 Hz tick in q12 units for each species: the retail yaw speed in
/// degrees a second, doubled by `ChangeYaw`, over 20, in 4096ths of a turn.
/// Species with no entry keep the generic quick turn. `prop_face_point` applies the
/// table only to an actor that has an enemy: idle and scripted facing stay quick,
/// because the c1a0e cascade (tests/chain-new-game) does not complete when idle
/// actors turn at their retail rate. The talking allies have no entry either.
pub const fn turn_rate_q12(kind: u8) -> u16 {
    match kind {
        2 => 34,              // headcrab: 30
        59 | 5 | 55 | 11 | 14 | 58 => 137, // baby crab, zombie, controller, roach: 120
        6 | 7 | 18 => 102,    // houndeye, bullsquid, Gonarch: 90
        8 => 171,             // marine: 150
        9 => 80,              // vortigaunt: 70
        10 | 19 => 114,       // alien grunt, ichthyosaur: 100
        16 => 68,             // gargantua: 60
        51 => 410,            // assassin: 360
        20 | 21 | 22 => 17,   // turrets: the gun swings 30 degrees a second
        _ => 250,
    }
}

// ---- ears ----

/// Loudness (hearing radius in units) of the noises a fight makes.
pub const VOL_QUIET_GUN: u16 = 200;
pub const VOL_NORMAL_GUN: u16 = 600;
pub const VOL_LOUD_GUN: u16 = 1000;
pub const VOL_EXPLOSION: u16 = 1024;
pub const VOL_MONSTER_BURST: u16 = 384;

/// How loud the player's weapon is (`m_iWeaponVolume`) by weapon id: the
/// crowbar and pistols/MP5 normal, the heavy guns loud, the crossbow and the
/// hive hand quiet.
pub const fn weapon_volume(id: usize) -> u16 {
    match id {
        0 => 128,
        1 | 3 => VOL_NORMAL_GUN,
        2 | 4 | 6 | 7 => VOL_LOUD_GUN,
        5 | 9 => VOL_QUIET_GUN,
        8 => 450,
        _ => 0,
    }
}

const SLOTS: usize = 6;

/// The few noises still carrying: where each came from, how far it reaches
/// and how many ticks are left.
pub struct Sounds {
    pos: [[i32; 3]; SLOTS],
    vol: [u16; SLOTS],
    ttl: [u8; SLOTS],
}

impl Sounds {
    pub const EMPTY: Sounds = Sounds {
        pos: [[0; 3]; SLOTS],
        vol: [0; SLOTS],
        ttl: [0; SLOTS],
    };

    /// A noise at `pos` that carries `vol` units for `ticks`. The quietest
    /// slot (or an expired one) takes it.
    #[optimize(size)]
    pub fn insert(&mut self, pos: [i32; 3], vol: u16, ticks: u8) {
        if vol == 0 {
            return;
        }
        let mut slot = 0;
        let mut weakest = u32::MAX;
        for i in 0..SLOTS {
            let strength = if self.ttl[i] == 0 { 0 } else { self.vol[i] as u32 + 1 };
            if strength < weakest {
                weakest = strength;
                slot = i;
            }
        }
        self.pos[slot] = pos;
        self.vol[slot] = vol;
        self.ttl[slot] = ticks;
    }

    /// One tick passes.
    #[optimize(size)]
    pub fn tick(&mut self) {
        for i in 0..SLOTS {
            if self.ttl[i] != 0 {
                self.ttl[i] -= 1;
            }
        }
    }

    /// The nearest noise whose reach covers `ear`, if any (`Listen`: heard
    /// when the distance is within the loudness).
    #[optimize(size)]
    pub fn heard(&self, ear: [i32; 3]) -> Option<[i32; 3]> {
        let mut best: Option<([i32; 3], i32)> = None;
        for i in 0..SLOTS {
            if self.ttl[i] == 0 {
                continue;
            }
            let r = self.vol[i] as i32;
            let d = [
                self.pos[i][0] - ear[0],
                self.pos[i][1] - ear[1],
                self.pos[i][2] - ear[2],
            ];
            if d[0].abs() > r || d[1].abs() > r || d[2].abs() > r {
                continue;
            }
            let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if d2 <= r * r && best.map_or(true, |b| d2 < b.1) {
                best = Some((self.pos[i], d2));
            }
        }
        best.map(|b| b.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marines_and_aliens_are_enemies_and_marines_ignore_each_other() {
        assert!(is_enemy(relation(8, 10))); // marine vs alien grunt
        assert_eq!(relation(8, 10), R_NM);
        assert_eq!(relation(10, 8), R_NM);
        assert!(is_enemy(relation(8, 9))); // marine vs vortigaunt
        assert!(is_enemy(relation(9, 8)));
        assert!(!is_enemy(relation(8, 8)));
        assert!(!is_enemy(relation(9, 10))); // aliens leave each other alone
        assert!(is_enemy(relation(5, 8))); // zombie vs marine
        assert!(!is_enemy(relation(5, 9))); // zombie vs vortigaunt
    }

    #[test]
    fn bullsquids_hunt_headcrabs_and_headcrabs_fear_them() {
        assert_eq!(relation(7, 2), R_HT);
        assert_eq!(relation(2, 7), R_FR);
        assert!(is_enemy(relation(7, KIND_PLAYER)));
        assert!(is_enemy(relation(2, KIND_PLAYER)));
    }

    #[test]
    fn friends_and_machines() {
        assert_eq!(relation(0, KIND_PLAYER), R_AL);
        assert_eq!(relation(1, 0), R_AL);
        assert!(is_enemy(relation(1, 8))); // Barney vs a marine
        assert!(is_enemy(relation(8, 1)));
        assert!(is_enemy(relation(20, KIND_PLAYER))); // a sentry
        assert!(!is_enemy(relation(20, 8))); // machines ignore marines
        assert!(is_enemy(relation(20, 10)));
        assert_eq!(relation(14, KIND_PLAYER), R_FR); // roaches flee
        assert_eq!(relation(100, 8), R_NO);
    }

    #[test]
    fn view_cones() {
        // looking along +z (fx 0, fz 4096)
        assert!(in_view_cone(5, 0, 4096, 0, 100)); // zombie, straight ahead
        assert!(in_view_cone(5, 0, 4096, 80, 100)); // 38 degrees off
        assert!(!in_view_cone(5, 0, 4096, 100, 10)); // nearly sideways
        assert!(!in_view_cone(5, 0, 4096, 0, -100)); // behind
        assert!(in_view_cone(8, 0, 4096, 100, 30)); // marine: 0.2 sees 78 degrees
        assert!(!in_view_cone(8, 0, 4096, 100, -50));
        assert!(in_view_cone(0, 0, 4096, 100, -50)); // scientists are wide eyed
        assert!(!in_view_cone(0, 0, 4096, 0, -100)); // not straight behind
        assert!(in_view_cone(11, 0, 4096, 0, -100)); // controllers see all around
    }

    #[test]
    fn yaw_speeds_follow_the_retail_table() {
        // 120 deg/s doubled is 12 degrees a tick: 137 of 4096
        assert_eq!(turn_rate_q12(5), 137);
        assert!(turn_rate_q12(2) < turn_rate_q12(5));
        assert!(turn_rate_q12(16) < turn_rate_q12(8));
        assert_eq!(turn_rate_q12(60), 250);
    }

    #[test]
    fn a_noise_carries_exactly_its_loudness() {
        let mut s = Sounds::EMPTY;
        s.insert([1000, 0, 0], VOL_NORMAL_GUN, 4);
        assert!(s.heard([1000 + 600, 0, 0]).is_some());
        assert!(s.heard([1000 + 601, 0, 0]).is_none());
        s.tick();
        s.tick();
        s.tick();
        assert!(s.heard([1000, 0, 0]).is_some());
        s.tick();
        assert!(s.heard([1000, 0, 0]).is_none());
    }

    #[test]
    fn a_louder_noise_displaces_the_quietest() {
        let mut s = Sounds::EMPTY;
        for i in 0..SLOTS {
            s.insert([i as i32 * 5000, 0, 0], VOL_QUIET_GUN + i as u16, 10);
        }
        s.insert([-100, 0, 0], VOL_LOUD_GUN, 10);
        assert!(s.heard([-100, 0, 0]).is_some());
        // the quietest (index 0, at x = 0) is the one that went
        assert!(s.heard([0, 0, 0]) == Some([-100, 0, 0]) || s.heard([0, 0, 0]).is_none());
        assert!(s.heard([5000, 0, 0]).is_some());
    }

    #[test]
    fn weapon_volumes() {
        assert_eq!(weapon_volume(1), 600);
        assert_eq!(weapon_volume(4), 1000);
        assert_eq!(weapon_volume(5), 200);
        assert_eq!(weapon_volume(10), 0);
    }
}

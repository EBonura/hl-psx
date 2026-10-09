//! The difficulty contract: which `skill.cfg` cvar every gameplay number reads.
//!
//! Half-Life keeps its balance in `skill.cfg`: three values per cvar (Easy,
//! Medium, Hard) for monster health, every attack's damage, the pickups and
//! the player's weapons. The game crate's build script cooks the user's own
//! copy into a table indexed by [`Sk`] and by actor model type; this module is
//! the single list of which cvar (and which retail derivation, such as the baby
//! headcrab's 0.3 scale) each entry reads. The host parity test reads the same
//! list, so a row that is added, renamed or dropped cannot silently desync the
//! table the player feels from the file it is cooked from.
//!
//! Scales are integer thousandths so the list is `const` and exact: the build
//! computes `floor(value * milli / 1000)`.

/// One tunable value, in `u16` after cooking. The discriminant is the column in
/// the cooked per-difficulty table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Sk {
    // Monster attacks.
    HeadcrabBite,
    BabyHeadcrabBite,
    ZombieSlash,
    ZombieBothSlash,
    HoundeyeBlast,
    SquidSpit,
    SquidBite,
    SquidWhip,
    SlaveZap,
    SlaveClaw,
    SlaveClawRake,
    AgruntPunch,
    Hornet,
    ControllerBall,
    ControllerZap,
    IchthyosaurShake,
    GargFire,
    GargSlash,
    GargStomp,
    SnarkBite,
    LeechBite,
    HgruntKick,
    BigMommaSlash,
    BigMommaBlast,
    NihilanthZap,
    // Monster bullets (FireBullets maps the three monster bullet types).
    Bullet9mm,
    Bullet9mmAr,
    Bullet12mm,
    // Large health pools that the u8 actor health only stands for.
    GargHealth,
    ApacheHealth,
    NihilanthHealth,
    /// `sk_bigmomma_health_factor` in Q8 (the info_bigmomma nodes scale by it).
    BigMommaFactorQ8,
    // Pickups and chargers.
    HealthKit,
    Battery,
    HealthCharger,
    SuitCharger,
    // The player's weapons (the same at every difficulty in a stock cfg).
    PlrCrowbar,
    Plr9mm,
    Plr357,
    Plr9mmAr,
    PlrM203,
    PlrBuckshot,
    PlrXbowClient,
    PlrXbowMonster,
    PlrRpg,
    PlrGauss,
    PlrEgonNarrow,
    PlrEgonWide,
    PlrHandGrenade,
    PlrSatchel,
    PlrTripmine,
}

/// Number of [`Sk`] columns in the cooked table.
pub const SK_COUNT: usize = Sk::PlrTripmine as usize + 1;

/// The cvar each [`Sk`] reads and its scale in thousandths, in discriminant
/// order (checked by the host test).
pub const SK_KEYS: [(Sk, &str, u32); SK_COUNT] = [
    (Sk::HeadcrabBite, "sk_headcrab_dmg_bite", 1000),
    // CBabyCrab scales the headcrab's bite.
    (Sk::BabyHeadcrabBite, "sk_headcrab_dmg_bite", 300),
    (Sk::ZombieSlash, "sk_zombie_dmg_one_slash", 1000),
    (Sk::ZombieBothSlash, "sk_zombie_dmg_both_slash", 1000),
    (Sk::HoundeyeBlast, "sk_houndeye_dmg_blast", 1000),
    (Sk::SquidSpit, "sk_bullsquid_dmg_spit", 1000),
    (Sk::SquidBite, "sk_bullsquid_dmg_bite", 1000),
    (Sk::SquidWhip, "sk_bullsquid_dmg_whip", 1000),
    (Sk::SlaveZap, "sk_islave_dmg_zap", 1000),
    (Sk::SlaveClaw, "sk_islave_dmg_claw", 1000),
    (Sk::SlaveClawRake, "sk_islave_dmg_clawrake", 1000),
    (Sk::AgruntPunch, "sk_agrunt_dmg_punch", 1000),
    (Sk::Hornet, "sk_hornet_dmg", 1000),
    (Sk::ControllerBall, "sk_controller_dmgball", 1000),
    (Sk::ControllerZap, "sk_controller_dmgzap", 1000),
    (Sk::IchthyosaurShake, "sk_ichthyosaur_shake", 1000),
    (Sk::GargFire, "sk_gargantua_dmg_fire", 1000),
    (Sk::GargSlash, "sk_gargantua_dmg_slash", 1000),
    (Sk::GargStomp, "sk_gargantua_dmg_stomp", 1000),
    (Sk::SnarkBite, "sk_snark_dmg_bite", 1000),
    (Sk::LeechBite, "sk_leech_dmg_bite", 1000),
    (Sk::HgruntKick, "sk_hgrunt_kick", 1000),
    (Sk::BigMommaSlash, "sk_bigmomma_dmg_slash", 1000),
    (Sk::BigMommaBlast, "sk_bigmomma_dmg_blast", 1000),
    (Sk::NihilanthZap, "sk_nihilanth_zap", 1000),
    (Sk::Bullet9mm, "sk_9mm_bullet", 1000),
    (Sk::Bullet9mmAr, "sk_9mmAR_bullet", 1000),
    (Sk::Bullet12mm, "sk_12mm_bullet", 1000),
    (Sk::GargHealth, "sk_gargantua_health", 1000),
    (Sk::ApacheHealth, "sk_apache_health", 1000),
    (Sk::NihilanthHealth, "sk_nihilanth_health", 1000),
    (Sk::BigMommaFactorQ8, "sk_bigmomma_health_factor", 256_000),
    (Sk::HealthKit, "sk_healthkit", 1000),
    (Sk::Battery, "sk_battery", 1000),
    (Sk::HealthCharger, "sk_healthcharger", 1000),
    (Sk::SuitCharger, "sk_suitcharger", 1000),
    (Sk::PlrCrowbar, "sk_plr_crowbar", 1000),
    (Sk::Plr9mm, "sk_plr_9mm_bullet", 1000),
    (Sk::Plr357, "sk_plr_357_bullet", 1000),
    (Sk::Plr9mmAr, "sk_plr_9mmAR_bullet", 1000),
    (Sk::PlrM203, "sk_plr_9mmAR_grenade", 1000),
    (Sk::PlrBuckshot, "sk_plr_buckshot", 1000),
    (Sk::PlrXbowClient, "sk_plr_xbow_bolt_client", 1000),
    (Sk::PlrXbowMonster, "sk_plr_xbow_bolt_monster", 1000),
    (Sk::PlrRpg, "sk_plr_rpg", 1000),
    (Sk::PlrGauss, "sk_plr_gauss", 1000),
    (Sk::PlrEgonNarrow, "sk_plr_egon_narrow", 1000),
    (Sk::PlrEgonWide, "sk_plr_egon_wide", 1000),
    (Sk::PlrHandGrenade, "sk_plr_hand_grenade", 1000),
    (Sk::PlrSatchel, "sk_plr_satchel", 1000),
    (Sk::PlrTripmine, "sk_plr_tripmine", 1000),
];

/// Spawn health per actor model type: the type id, its `skill.cfg` cvar and a
/// scale in thousandths (the Gonarch spawns at 150 x its health factor).
pub const HEALTH_KEYS: &[(u8, &str, u32)] = &[
    (0, "sk_scientist_health", 1000),
    (1, "sk_barney_health", 1000),
    (2, "sk_headcrab_health", 1000),
    (5, "sk_zombie_health", 1000),
    (6, "sk_houndeye_health", 1000),
    (7, "sk_bullsquid_health", 1000),
    (8, "sk_hgrunt_health", 1000),
    (9, "sk_islave_health", 1000),
    (10, "sk_agrunt_health", 1000),
    (11, "sk_controller_health", 1000),
    (13, "sk_leech_health", 1000),
    (16, "sk_gargantua_health", 1000),
    (17, "sk_nihilanth_health", 1000),
    (18, "sk_bigmomma_health_factor", 150_000),
    (19, "sk_ichthyosaur_health", 1000),
    (20, "sk_sentry_health", 1000),
    (21, "sk_turret_health", 1000),
    (22, "sk_miniturret_health", 1000),
    (23, "sk_apache_health", 1000),
    (25, "sk_scientist_health", 1000),
    (51, "sk_hassassin_health", 1000),
    (54, "sk_scientist_health", 1000),
    (55, "sk_zombie_health", 1000),
    (58, "sk_snark_health", 1000),
    // CBabyCrab: the headcrab's health, quartered.
    (59, "sk_headcrab_health", 250),
];

/// The attack a model type's generic hit applies (the shared shooter's bullet,
/// the melee schedule's bite or slash). Types with several attacks name each
/// one at its call site instead.
pub const fn type_attack(ty: u8) -> Option<Sk> {
    Some(match ty {
        1 | 22 | 51 => Sk::Bullet9mm,
        2 => Sk::HeadcrabBite,
        5 | 55 => Sk::ZombieSlash,
        6 => Sk::HoundeyeBlast,
        7 => Sk::SquidSpit,
        8 | 20 => Sk::Bullet9mmAr,
        9 => Sk::SlaveZap,
        10 => Sk::Hornet,
        11 => Sk::ControllerBall,
        16 => Sk::GargFire,
        19 => Sk::IchthyosaurShake,
        21 => Sk::Bullet12mm,
        58 => Sk::SnarkBite,
        59 => Sk::BabyHeadcrabBite,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_follow_the_discriminants() {
        for (i, (sk, key, _)) in SK_KEYS.iter().enumerate() {
            assert_eq!(*sk as usize, i, "{key} is out of order");
            assert!(key.starts_with("sk_"));
        }
    }

    #[test]
    fn no_cvar_is_listed_twice_with_one_scale() {
        for (i, (_, a, sa)) in SK_KEYS.iter().enumerate() {
            for (_, b, sb) in &SK_KEYS[i + 1..] {
                assert!(a != b || sa != sb, "{a} duplicated at scale {sa}");
            }
        }
    }
}

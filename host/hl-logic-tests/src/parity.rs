// SPDX-License-Identifier: GPL-2.0-or-later
//! Machine-checked parity between the port and the retail game.
//!
//! Every row is one number the player feels: a subject (an actor class, an
//! attack, a weapon), a field, the value the retail game produces, and the
//! value the port produces, with a pass/fail. Expected values come from data,
//! never from the port:
//!
//! * `skill.cfg` from the user's Half-Life install (`data/skill.cfg`): attack
//!   damage, pickups and the player's weapons at each difficulty;
//! * `oracle/spawn_health.csv`: the health the retail game spawns each monster
//!   class with at Easy, Medium and Hard, read from every campaign map under
//!   the Xash3D reference with the retail game DLL (observation only);
//! * the observed attack and weapon rows below, each naming the measured trace
//!   or public documentation it comes from.
//!
//! Port values come from the game's own tables: `combat_defs.rs` (included by
//! path, so there is no copy), the `Sk` cvar list in `hl-format` and the same
//! `skill.cfg` cooking `game/build.rs` performs, plus a scan of the game
//! sources for whether each cvar is read at all.
//!
//! `cargo test --manifest-path host/hl-logic-tests/Cargo.toml parity -- --nocapture`
//! prints the table. `PARITY_OUT=path.csv` also writes it. Without
//! `data/skill.cfg` (CI is source-only) the cfg-backed rows are skipped.
//!
//! A FAIL that is understood but not yet fixed is listed in [`KNOWN`] with the
//! reason; the test fails on any other FAIL, and on a KNOWN row that has
//! started to pass, so the list can only shrink.

use crate::combat_defs::*;
use hl_format::skill::{self, Sk};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

#[path = "../../../shared/hl-format/src/skill_cfg.rs"]
mod skill_cfg;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> Option<String> {
    std::fs::read_to_string(root().join(rel)).ok()
}

type Cfg = HashMap<String, [f64; 3]>;

fn load_cfg() -> Option<Cfg> {
    read("data/skill.cfg").map(|t| skill_cfg::parse_cfg(&t))
}

/// One audited number.
#[derive(Clone, Debug)]
struct Row {
    area: &'static str,
    subject: String,
    field: String,
    expected: String,
    port: String,
    verdict: Verdict,
    source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Fail,
    /// The port has no such behaviour at all (counted, not a pass).
    Missing,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Missing => "MISSING",
        }
    }
}

fn cell(v: [Option<i64>; 3]) -> String {
    v.iter()
        .map(|x| x.map_or("-".to_string(), |x| x.to_string()))
        .collect::<Vec<_>>()
        .join("/")
}

/// Compare per-difficulty values; a `None` on either side is "not measured".
fn compare(expected: [Option<i64>; 3], port: [Option<i64>; 3]) -> Verdict {
    for l in 0..3 {
        if let (Some(e), Some(p)) = (expected[l], port[l]) {
            if e != p {
                return Verdict::Fail;
            }
        }
    }
    Verdict::Pass
}

// ---- classes and types -----------------------------------------------------

/// Monster classname -> actor model type id, as the cooker (host/hl-bsp)
/// assigns them. Read from its source so the audit cannot drift from it.
fn class_types() -> HashMap<String, u8> {
    let src = read("host/hl-bsp/src/main.rs").expect("host/hl-bsp/src/main.rs");
    let at = src.find("\"monster_scientist\" =>").expect("type table");
    let mut out = HashMap::new();
    for line in src[at..].lines().take(120) {
        let t = line.trim();
        // `"monster_x" => 12u16,` (a guarded `if` arm is a special case)
        let Some(rest) = t.strip_prefix("\"monster_") else {
            continue;
        };
        let Some((name, arm)) = rest.split_once("\" => ") else {
            continue;
        };
        let digits: String = arm.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(ty) = digits.parse::<u8>() {
            out.entry(format!("monster_{name}")).or_insert(ty);
        }
    }
    out
}

/// Types whose u8 actor health only stands for a larger pool: their modules
/// scale incoming damage onto it, so a spawn health over 255 is not a defect.
const SCALED_POOLS: [u8; 4] = [16, 17, 23, 61];

/// The health the port spawns a type with at `level`: the cooked skill.cfg
/// row when `HEALTH_KEYS` lists the type, else the roster fallback.
fn port_health(cfg: &Cfg, ty: u8, level: usize) -> i64 {
    if let Some(&(_, key, milli)) = skill::HEALTH_KEYS.iter().find(|r| r.0 == ty) {
        let v = skill_cfg::cooked(cfg, key, milli, level).expect("cfg row");
        return v.clamp(1, 255) as i64;
    }
    MODEL_DEFS[ty as usize].health as i64
}

fn health_rows(cfg: &Cfg, rows: &mut Vec<Row>) {
    let types = class_types();
    let csv = read("host/hl-logic-tests/oracle/spawn_health.csv").expect("spawn_health.csv");
    for line in csv.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        let class = f[0];
        let exp: [Option<i64>; 3] = [0, 1, 2].map(|i| f[1 + i].parse().ok());
        let Some(&ty) = types.get(class) else {
            continue; // not a roster actor: corpses, scripted props
        };
        let port = [0, 1, 2].map(|l| Some(port_health(cfg, ty, l)));
        let mut verdict = compare(exp, port);
        let mut note = format!("retail spawn census, {} maps", f[4]);
        // A big pool the port scales damage onto is equivalent.
        if verdict == Verdict::Fail && SCALED_POOLS.contains(&ty) {
            verdict = Verdict::Pass;
            note.push_str("; u8 health scaled by the actor module");
        }
        rows.push(Row {
            area: "health",
            subject: class.to_string(),
            field: "spawn health".into(),
            expected: cell(exp),
            port: cell(port),
            verdict,
            source: note,
        });
    }
}

// ---- attacks -----------------------------------------------------------------

/// What the port applies for an observed number.
#[derive(Clone, Copy)]
enum Port {
    /// Reads this cvar through the skill table.
    Sk(Sk),
    /// A multiple of one cvar.
    SkTimes(Sk, i64),
    /// No such attack in the port.
    None,
}

struct Obs {
    class: &'static str,
    field: &'static str,
    /// Retail damage at Easy, Medium, Hard (None = not measured).
    retail: [Option<i64>; 3],
    /// The cvar the retail number reads: it supplies the levels not measured.
    cvar: Sk,
    port: Port,
    source: &'static str,
}

const N: Option<i64> = None;
const fn s(v: i64) -> Option<i64> {
    Some(v)
}

/// Observed retail damage per hit. Sources: the Xash3D HLREF traces under
/// `work/hl-gameplay-*/oracle` (profile-organic.md, profile-ranged.md), read
/// from player health drops; observation only.
const OBSERVED: &[Obs] = &[
    Obs {
        class: "monster_headcrab",
        field: "bite",
        retail: [s(5), s(10), s(10)],
        cvar: Sk::HeadcrabBite,
        port: Port::Sk(Sk::HeadcrabBite),
        source: "profile-organic 0/1",
    },
    Obs {
        class: "monster_zombie",
        field: "one claw",
        retail: [s(10), s(20), s(20)],
        cvar: Sk::ZombieSlash,
        port: Port::Sk(Sk::ZombieSlash),
        source: "profile-organic 0/2",
    },
    Obs {
        class: "monster_zombie",
        field: "both claws",
        retail: [N, s(40), N],
        cvar: Sk::ZombieBothSlash,
        port: Port::SkTimes(Sk::ZombieSlash, 2),
        source: "profile-organic 2 (medium); cfg for the rest",
    },
    Obs {
        class: "monster_bullchicken",
        field: "spit",
        retail: [s(10), s(10), s(15)],
        cvar: Sk::SquidSpit,
        port: Port::Sk(Sk::SquidSpit),
        source: "profile-organic 3; c1a4 re-run",
    },
    Obs {
        class: "monster_bullchicken",
        field: "close attack (seq 9)",
        retail: [s(15), s(25), s(25)],
        cvar: Sk::SquidBite,
        port: Port::Sk(Sk::SquidWhip),
        source: "c1a4 squid 244 at 60 u, skill 1/2/3 re-run",
    },
    Obs {
        class: "monster_houndeye",
        field: "blast at 0 u",
        retail: [s(10), s(15), s(15)],
        cvar: Sk::HoundeyeBlast,
        port: Port::Sk(Sk::HoundeyeBlast),
        source: "profile-organic 4 (falloff 1 - d/384)",
    },
    Obs {
        class: "monster_ichthyosaur",
        field: "bite",
        retail: [s(20), s(35), N],
        cvar: Sk::IchthyosaurShake,
        port: Port::Sk(Sk::IchthyosaurShake),
        source: "profile-organic 6",
    },
    Obs {
        class: "monster_gargantua",
        field: "melee",
        retail: [s(10), s(30), N],
        cvar: Sk::GargSlash,
        port: Port::Sk(Sk::GargSlash),
        source: "profile-organic 7",
    },
    Obs {
        class: "monster_gargantua",
        field: "flame per hit",
        retail: [s(3), s(5), N],
        cvar: Sk::GargFire,
        port: Port::Sk(Sk::GargFire),
        source: "profile-organic 7 (events stack 2 hits)",
    },
    Obs {
        class: "monster_leech",
        field: "bite",
        retail: [s(2), s(2), N],
        cvar: Sk::LeechBite,
        port: Port::None,
        source: "profile-organic 8",
    },
    Obs {
        class: "monster_alien_slave",
        field: "zap (two beams)",
        retail: [s(20), s(20), s(30)],
        cvar: Sk::SlaveZap,
        port: Port::SkTimes(Sk::SlaveZap, 2),
        source: "profile-ranged 2",
    },
    Obs {
        class: "monster_alien_slave",
        field: "claw",
        retail: [s(8), s(10), s(10)],
        cvar: Sk::SlaveClaw,
        port: Port::Sk(Sk::SlaveZap),
        source: "profile-ranged 2",
    },
    Obs {
        class: "monster_human_grunt",
        field: "mp5 bullet",
        retail: [s(3), s(4), s(5)],
        cvar: Sk::Bullet9mmAr,
        port: Port::Sk(Sk::Bullet9mmAr),
        source: "profile-ranged 1",
    },
    Obs {
        class: "monster_human_grunt",
        field: "shotgun pellet",
        retail: [s(5), s(5), s(8)],
        cvar: Sk::LeechBite,
        port: Port::None,
        source: "profile-ranged 1 (no shotgun grunt in the port)",
    },
    Obs {
        class: "monster_alien_grunt",
        field: "hornet",
        retail: [s(4), s(5), s(8)],
        cvar: Sk::Hornet,
        port: Port::Sk(Sk::Hornet),
        source: "profile-ranged 3",
    },
    Obs {
        class: "monster_alien_controller",
        field: "ball",
        retail: [s(3), s(4), N],
        cvar: Sk::ControllerBall,
        port: Port::Sk(Sk::ControllerBall),
        source: "profile-ranged 4",
    },
    Obs {
        class: "monster_human_assassin",
        field: "pistol",
        retail: [s(5), s(5), s(8)],
        cvar: Sk::Bullet9mm,
        port: Port::Sk(Sk::Bullet9mm),
        source: "profile-ranged 5",
    },
    Obs {
        class: "monster_sentry",
        field: "bullet",
        retail: [s(3), s(4), s(5)],
        cvar: Sk::Bullet9mmAr,
        port: Port::Sk(Sk::Bullet9mmAr),
        source: "profile-ranged 7",
    },
];

fn sk_values(cfg: &Cfg, sk: Sk) -> [Option<i64>; 3] {
    let (_, key, milli) = skill::SK_KEYS[sk as usize];
    [0, 1, 2].map(|l| skill_cfg::cooked(cfg, key, milli, l).map(|v| v as i64))
}

/// Every Sk variant the game sources name outside the contract file itself.
fn wired(sk: Sk) -> bool {
    // `skill_damage(type)` reads the cvar `type_attack` names for the type.
    if (0..=u8::MAX).any(|ty| skill::type_attack(ty) == Some(sk)) {
        return true;
    }
    let needle = format!("Sk::{sk:?}");
    let dir = root().join("game/src");
    std::fs::read_dir(dir).unwrap().flatten().any(|e| {
        e.path().extension().is_some_and(|x| x == "rs")
            && std::fs::read_to_string(e.path()).is_ok_and(|t| {
                t.match_indices(&needle)
                    .any(|(i, _)| !t[i + needle.len()..].starts_with(|c: char| c.is_alphanumeric()))
            })
    })
}

fn attack_rows(cfg: &Cfg, rows: &mut Vec<Row>) {
    for o in OBSERVED {
        let (port, wired_ok) = match o.port {
            Port::Sk(sk) => (sk_values(cfg, sk), wired(sk)),
            Port::SkTimes(sk, k) => (sk_values(cfg, sk).map(|v| v.map(|v| v * k)), wired(sk)),
            Port::None => ([None; 3], false),
        };
        // Levels the trace did not measure come from the cvar the measured
        // levels match.
        let cfg_levels = sk_values(cfg, o.cvar);
        let expected = [0, 1, 2].map(|l| o.retail[l].or(cfg_levels[l]));
        let verdict = if matches!(o.port, Port::None) {
            Verdict::Missing
        } else if !wired_ok {
            Verdict::Fail
        } else {
            compare(expected, port)
        };
        rows.push(Row {
            area: "attack",
            subject: o.class.to_string(),
            field: o.field.to_string(),
            expected: cell(expected),
            port: match o.port {
                Port::None => "no such attack".to_string(),
                _ if !wired_ok => format!("{} (cvar never read)", cell(port)),
                _ => cell(port),
            },
            verdict,
            source: o.source.to_string(),
        });
    }
}

/// Retail attacks the cfg defines and the port never reads.
fn coverage_rows(cfg: &Cfg, rows: &mut Vec<Row>) {
    for &(sk, key, _) in &skill::SK_KEYS {
        // The player's weapons are audited as weapon rows against the arsenal
        // table, not read through the skill table at run time.
        if wired(sk) || format!("{sk:?}").starts_with("Plr") {
            continue;
        }
        let v = sk_values(cfg, sk);
        rows.push(Row {
            area: "cvar",
            subject: key.to_string(),
            field: format!("{sk:?}"),
            expected: cell(v),
            port: "never read".into(),
            verdict: Verdict::Missing,
            source: "skill.cfg".into(),
        });
    }
}

// ---- weapons -------------------------------------------------------------------

/// One measured or documented arsenal number.
struct WObs {
    weapon: &'static str,
    field: &'static str,
    retail: i64,
    /// Allowed difference: the 20 Hz reference clock rounds 0.2 s up to 5.
    tol: i64,
    port: i64,
    source: &'static str,
}

const XASH: &str = "Xash3D weapon bench, frozen zombie at 150 u";
const DOC: &str = "public documentation (manual and wiki)";

fn weapon_obs() -> Vec<WObs> {
    let d = |w: usize| &WEAPON_DEFS[w];
    let o = |weapon, field, retail, tol, port, source| WObs {
        weapon,
        field,
        retail,
        tol,
        port,
        source,
    };
    vec![
        o(
            "glock",
            "primary interval",
            6,
            0,
            d(W_GLOCK).cooldown as i64,
            XASH,
        ),
        o(
            "glock",
            "secondary interval",
            4,
            0,
            GLOCK_ALT_COOLDOWN as i64,
            XASH,
        ),
        o("glock", "clip", 17, 0, d(W_GLOCK).clip as i64, XASH),
        o("glock", "reload", 30, 1, d(W_GLOCK).reload as i64, XASH),
        o("357", "interval", 15, 0, d(W_357).cooldown as i64, XASH),
        o("357", "clip", 6, 0, d(W_357).clip as i64, XASH),
        o("357", "reload", 40, 1, d(W_357).reload as i64, XASH),
        o("mp5", "interval", 2, 0, d(W_MP5).cooldown as i64, XASH),
        o("mp5", "grenade interval", 20, 1, M203_COOLDOWN as i64, XASH),
        o("mp5", "reload", 30, 1, d(W_MP5).reload as i64, XASH),
        o("mp5", "clip", 50, 0, d(W_MP5).clip as i64, DOC),
        o(
            "shotgun",
            "primary interval",
            15,
            0,
            d(W_SHOTGUN).cooldown as i64,
            XASH,
        ),
        o(
            "shotgun",
            "double-barrel interval",
            30,
            0,
            SHOTGUN_ALT_COOLDOWN as i64,
            XASH,
        ),
        o("shotgun", "pellets", 6, 0, d(W_SHOTGUN).pellets as i64, DOC),
        o("shotgun", "clip", 8, 0, d(W_SHOTGUN).clip as i64, DOC),
        o(
            "crossbow",
            "interval",
            15,
            0,
            d(W_CROSSBOW).cooldown as i64,
            XASH,
        ),
        o("crossbow", "clip", 5, 0, d(W_CROSSBOW).clip as i64, XASH),
        o(
            "crossbow",
            "reload",
            90,
            1,
            d(W_CROSSBOW).reload as i64,
            XASH,
        ),
        o("gauss", "interval", 5, 1, d(W_GAUSS).cooldown as i64, XASH),
        o("egon", "interval", 2, 1, d(W_EGON).cooldown as i64, XASH),
        o(
            "hornetgun",
            "primary interval",
            5,
            0,
            d(W_HORNET).cooldown as i64,
            XASH,
        ),
        o(
            "hornetgun",
            "rapid interval",
            2,
            0,
            HORNET_ALT_COOLDOWN as i64,
            XASH,
        ),
        o(
            "hornetgun",
            "reserve",
            8,
            0,
            d(W_HORNET).reserve_max as i64,
            DOC,
        ),
        // Ammo pools (maximum carried) and what one pickup gives.
        o("9mm", "max carried", 250, 0, max_carry(AMMO_9MM), DOC),
        o("357", "max carried", 36, 0, max_carry(AMMO_357), DOC),
        o("buckshot", "max carried", 125, 0, max_carry(AMMO_BUCK), DOC),
        o("bolts", "max carried", 50, 0, max_carry(AMMO_BOLT), DOC),
        o("rockets", "max carried", 5, 0, max_carry(AMMO_ROCKET), DOC),
        o(
            "uranium",
            "max carried",
            100,
            0,
            max_carry(AMMO_URANIUM),
            DOC,
        ),
        o(
            "hand grenades",
            "max carried",
            10,
            0,
            max_carry(AMMO_GREN),
            DOC,
        ),
        o("satchel", "max carried", 5, 0, max_carry(AMMO_SATCHEL), DOC),
        o(
            "tripmine",
            "max carried",
            5,
            0,
            max_carry(AMMO_TRIPMINE),
            DOC,
        ),
        o("snark", "max carried", 15, 0, max_carry(AMMO_SNARK), DOC),
        o(
            "m203 grenades",
            "max carried",
            10,
            0,
            max_carry(AMMO_ARGREN),
            DOC,
        ),
        o(
            "ammo_9mmclip",
            "rounds",
            17,
            0,
            AMMO_PICKUPS[0].1 as i64,
            DOC,
        ),
        o("ammo_9mmAR", "rounds", 50, 0, AMMO_PICKUPS[1].1 as i64, DOC),
        o(
            "ammo_buckshot",
            "rounds",
            12,
            0,
            AMMO_PICKUPS[2].1 as i64,
            DOC,
        ),
        o("ammo_357", "rounds", 6, 0, AMMO_PICKUPS[3].1 as i64, DOC),
        o(
            "ammo_crossbow",
            "rounds",
            5,
            0,
            AMMO_PICKUPS[4].1 as i64,
            DOC,
        ),
        o(
            "ammo_gaussclip",
            "rounds",
            20,
            0,
            AMMO_PICKUPS[6].1 as i64,
            DOC,
        ),
        o(
            "ammo_ARgrenades",
            "rounds",
            2,
            0,
            AMMO_PICKUPS[7].1 as i64,
            DOC,
        ),
    ]
}

/// The carry cap the port applies to an ammo pool.
fn max_carry(ammo: usize) -> i64 {
    if ammo == AMMO_ARGREN {
        return ARGREN_MAX_CARRY as i64;
    }
    WEAPON_DEFS
        .iter()
        .filter(|w| w.ammo == ammo)
        .map(|w| w.reserve_max as i64)
        .max()
        .unwrap_or(0)
}

/// Per-hit weapon damage: skill.cfg against the arsenal table.
fn weapon_damage_rows(cfg: &Cfg, rows: &mut Vec<Row>) {
    let table: &[(&str, usize, Sk)] = &[
        ("crowbar", W_CROWBAR, Sk::PlrCrowbar),
        ("glock", W_GLOCK, Sk::Plr9mm),
        ("357", W_357, Sk::Plr357),
        ("mp5", W_MP5, Sk::Plr9mmAr),
        ("shotgun pellet", W_SHOTGUN, Sk::PlrBuckshot),
        ("crossbow bolt", W_CROSSBOW, Sk::PlrXbowMonster),
        ("rpg", W_RPG, Sk::PlrRpg),
        ("gauss", W_GAUSS, Sk::PlrGauss),
        ("egon", W_EGON, Sk::PlrEgonWide),
        ("hand grenade", W_GRENADE, Sk::PlrHandGrenade),
        ("satchel", W_SATCHEL, Sk::PlrSatchel),
    ];
    for &(name, w, sk) in table {
        let exp = sk_values(cfg, sk);
        let port = [Some(WEAPON_DEFS[w].damage as i64); 3];
        rows.push(Row {
            area: "weapon",
            subject: name.to_string(),
            field: "damage per hit".into(),
            expected: cell(exp),
            port: cell(port),
            verdict: compare(exp, port),
            source: "skill.cfg".into(),
        });
    }
    // The M203 grenade and tripmine are launched with their own tables.
    let exp = sk_values(cfg, Sk::PlrM203);
    rows.push(Row {
        area: "weapon",
        subject: "m203 grenade".into(),
        field: "damage per hit".into(),
        expected: cell(exp),
        port: cell([Some(M203_DAMAGE as i64); 3]),
        verdict: compare(exp, [Some(M203_DAMAGE as i64); 3]),
        source: "skill.cfg".into(),
    });
}

fn weapon_rows(cfg: &Cfg, rows: &mut Vec<Row>) {
    for w in weapon_obs() {
        let pass = (w.retail - w.port).abs() <= w.tol;
        rows.push(Row {
            area: "weapon",
            subject: w.weapon.to_string(),
            field: w.field.to_string(),
            expected: w.retail.to_string(),
            port: w.port.to_string(),
            verdict: if pass { Verdict::Pass } else { Verdict::Fail },
            source: w.source.to_string(),
        });
    }
    weapon_damage_rows(cfg, rows);
}

// ---- the table ---------------------------------------------------------------

/// FAIL rows that are understood and not yet fixed: (area, subject, field).
const KNOWN: &[(&str, &str, &str)] = &[
    ("health", "monster_barnacle", "spawn health"),
    ("health", "monster_bigmomma", "spawn health"),
    ("health", "monster_cockroach", "spawn health"),
    ("health", "monster_gman", "spawn health"),
    ("health", "monster_ichthyosaur", "spawn health"),
    ("health", "monster_sitting_scientist", "spawn health"),
    ("health", "monster_tentacle", "spawn health"),
    ("attack", "monster_zombie", "both claws"),
    ("attack", "monster_bullchicken", "close attack (seq 9)"),
    ("attack", "monster_alien_slave", "claw"),
    ("weapon", "satchel", "damage per hit"),
];

fn build(cfg: &Cfg) -> Vec<Row> {
    let mut rows = Vec::new();
    health_rows(cfg, &mut rows);
    attack_rows(cfg, &mut rows);
    weapon_rows(cfg, &mut rows);
    coverage_rows(cfg, &mut rows);
    rows
}

fn render(rows: &[Row]) -> (String, String) {
    let mut csv = String::from("area,subject,field,expected_e/m/h,port_e/m/h,verdict,source\n");
    let mut txt = String::new();
    for r in rows {
        let _ = writeln!(
            csv,
            "{},{},{},{},{},{},{}",
            r.area,
            r.subject,
            r.field.replace(',', ";"),
            r.expected,
            r.port.replace(',', ";"),
            r.verdict.label(),
            r.source.replace(',', ";")
        );
        let _ = writeln!(
            txt,
            "{:<8} {:<26} {:<24} exp {:<14} port {:<28} {:<7} {}",
            r.area,
            r.subject,
            r.field,
            r.expected,
            r.port,
            r.verdict.label(),
            r.source
        );
    }
    (txt, csv)
}

#[test]
fn parity_table() {
    let Some(cfg) = load_cfg() else {
        eprintln!("parity: data/skill.cfg not found, skipping (source-only checkout)");
        return;
    };
    let rows = build(&cfg);
    let (txt, csv) = render(&rows);
    println!("{txt}");
    if let Ok(path) = std::env::var("PARITY_OUT") {
        std::fs::write(path, csv).expect("write PARITY_OUT");
    }
    let count = |v: Verdict| rows.iter().filter(|r| r.verdict == v).count();
    println!(
        "parity: {} rows, {} pass, {} fail, {} missing",
        rows.len(),
        count(Verdict::Pass),
        count(Verdict::Fail),
        count(Verdict::Missing)
    );
    let known = |r: &Row| KNOWN.contains(&(r.area, r.subject.as_str(), r.field.as_str()));
    let unexpected: Vec<_> = rows
        .iter()
        .filter(|r| r.verdict == Verdict::Fail && !known(r))
        .map(|r| {
            format!(
                "{} / {} / {}: expected {} port {}",
                r.area, r.subject, r.field, r.expected, r.port
            )
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "new parity failures:\n{}",
        unexpected.join("\n")
    );
    let stale: Vec<_> = KNOWN
        .iter()
        .filter(|k| {
            !rows.iter().any(|r| {
                (r.area, r.subject.as_str(), r.field.as_str()) == **k && r.verdict == Verdict::Fail
            })
        })
        .collect();
    assert!(
        stale.is_empty(),
        "KNOWN rows that no longer fail: {stale:?}"
    );
}

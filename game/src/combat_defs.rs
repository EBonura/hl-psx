//! Combat data tables: the actor roster (health fallbacks, speed, range,
//! cadence) and the player's arsenal (ammo pools, clips, damage, cooldowns).
//!
//! Pure `const` data with no hardware, so the host runner (`hl-logic-tests`)
//! includes this very file and audits it against `skill.cfg` and the measured
//! retail behaviour (`parity.rs`). Gameplay code reads these tables directly;
//! nothing here is a copy.

pub const N_MODEL_TYPES: usize = 76;

pub const SCIENTIST_HEALTH: u8 = 20;
pub const BARNEY_HEALTH: u8 = 35;
pub const HEADCRAB_HEALTH: u8 = 16;

// `ceiling_dangle` extends the scientist pose well beyond the standing hull.
// Keep the model sphere large enough for every baked frame or the hanging
// scientist can be frustum/occlusion-culled while part of it is on-screen.
pub const SCIENTIST_RENDER_RADIUS: i32 = 138;
pub const BARNEY_RENDER_RADIUS: i32 = 77;
pub const HEADCRAB_RENDER_RADIUS: i32 = 36;
pub const ITEM_RENDER_RADIUS: i32 = 36;

pub const AI_ITEM: u8 = 0; // static pickup
pub const AI_FLEE: u8 = 1; // scientist
pub const AI_ALLY: u8 = 2; // barney
pub const AI_MELEE: u8 = 3; // approach + bite (headcrab and friends)
pub const AI_IDLE: u8 = 4; // render only (flyers/ceiling/bosses until they get real AI)
pub const AI_RANGED: u8 = 5; // approach to range, then fire (grunts, vorts, agrunt, controller)
pub const AI_TURRET: u8 = 6; // stationary: rotate to face + fire (sentry/turret/miniturret)

pub const GLOCK_RANGE: i32 = 8192; // shared hitscan reach for the ballistic weapons
                                   // CShotgun fires buckshot to 2048, a quarter of every other bullet weapon's
                                   // 8192. Without this the shotgun snipes across a hangar as well as a rifle.
pub const SHOTGUN_RANGE: i32 = 2048;
// Half-Life gives every bullet weapon a cone: FireBullets offsets the shot by
// `spread` along the view right/up axes. Our shot offsets are screen pixels
// against H_PROJ (160 px = the forward axis), so one SDK cone value converts as
// `px = 160 * value` -- the same units the shotgun's pellet rosette already
// used. Single-player branches only (the MP5 and shotgun widen in deathmatch).
pub const GLOCK_SPREAD_PX: i32 = 2; // GlockFire(0.01, 0.3, autoaim) -> 1.6 px
pub const GLOCK_ALT_SPREAD_PX: i32 = 16; // GlockFire(0.1, 0.2, no autoaim)
pub const PYTHON_SPREAD_PX: i32 = 1; // VECTOR_CONE_1DEGREES -> 1.4 px
pub const MP5_SPREAD_PX: i32 = 4; // VECTOR_CONE_3DEGREES -> 4.2 px
pub const BUCKSHOT_SPREAD_PX: i32 = 14; // VECTOR_CONE_10DEGREES -> 13.9 px
                                        // Every Half-Life bullet weapon has a cone; a zero here is a gun that shoots a
                                        // laser from a standstill, which is what these constants were before they were
                                        // wired into the single-bullet path. Buckshot also carries a quarter of the
                                        // reach. Guard both so a future retune cannot silently undo it.
const _: () = assert!(GLOCK_SPREAD_PX > 0 && PYTHON_SPREAD_PX > 0 && MP5_SPREAD_PX > 0);
const _: () = assert!(GLOCK_ALT_SPREAD_PX > GLOCK_SPREAD_PX);
const _: () = assert!(SHOTGUN_RANGE < GLOCK_RANGE);

pub const AMMO_NONE: usize = 0; // melee (crowbar): no ammo
pub const AMMO_9MM: usize = 1;
pub const AMMO_357: usize = 2;
pub const AMMO_BUCK: usize = 3;
pub const AMMO_BOLT: usize = 4;
pub const AMMO_ROCKET: usize = 5;
pub const AMMO_URANIUM: usize = 6;
pub const AMMO_HORNET: usize = 7;
pub const AMMO_GREN: usize = 8;
pub const AMMO_SNARK: usize = 9;
pub const AMMO_SATCHEL: usize = 10;
pub const AMMO_TRIPMINE: usize = 11;
// M203 grenades are their own pool in HL (M203_GRENADE_MAX_CARRY, weapons.h) --
// hand grenades no longer starve the MP5 launcher.
pub const AMMO_ARGREN: usize = 12;
pub const N_AMMO: usize = 13;
pub const ARGREN_MAX_CARRY: u16 = 10; // weapons.h M203_GRENADE_MAX_CARRY

// Fire archetypes.
pub const FIRE_MELEE: u8 = 0; // short-range trace (crowbar)
pub const FIRE_SEMI: u8 = 1; // one hitscan per trigger press (glock, .357, gauss)
pub const FIRE_AUTO: u8 = 2; // hitscan while held (mp5, egon)
pub const FIRE_SPREAD: u8 = 3; // multi-pellet hitscan per press (shotgun)
pub const FIRE_PROJ: u8 = 4; // spawns a projectile (rpg, crossbow, grenade, hornet, ...)

// Projectile kinds (FIRE_PROJ weapons). Explosive kinds do area damage.
pub const PROJ_BOLT: u8 = 0;
pub const PROJ_ROCKET: u8 = 1;
pub const PROJ_GRENADE: u8 = 2;
pub const PROJ_HORNET: u8 = 3;
pub const PROJ_SNARK: u8 = 4;
pub const PROJ_SATCHEL: u8 = 5;
pub const PROJ_SPIT: u8 = 6; // bullsquid acid spit (enemy projectile)
pub const PROJ_TRIPMINE: u8 = 7;
pub const PROJ_M203: u8 = 8;
pub const PROJ_HORNET_FAST: u8 = 9;
pub const PROJ_BOLT_WATER: u8 = 10;
pub const GRENADE_FUSE_TICKS: u32 = 60; // CHandGrenade: three-second fuse at 20 Hz

pub struct WeaponDef {
    pub ammo: usize,      // AMMO_*
    pub clip: u16,        // magazine size (0 = fires straight from the reserve)
    pub reserve_max: u16, // carry cap for this ammo type
    pub damage: u8,       // per hit / per pellet
    pub range: i32,       // hitscan reach
    pub pellets: u8,      // hitscan traces per press (shotgun > 1)
    pub spread: i32,      // per-pellet aim jitter (aim-cone pixels)
    pub cooldown: u8,     // ticks between shots
    pub reload: u8,       // reload ticks (0 = no magazine reload)
    pub fire: u8,         // FIRE_*
    pub proj: u8,         // PROJ_* (FIRE_PROJ only)
    pub wm: u8,           // viewmodel index: geom chunk 1000+wm, tex 2000+wm
}

pub const W_CROWBAR: usize = 0;
pub const W_GLOCK: usize = 1;
pub const W_357: usize = 2;
pub const W_MP5: usize = 3;
pub const W_SHOTGUN: usize = 4;
pub const W_CROSSBOW: usize = 5;
pub const W_RPG: usize = 6;
pub const W_GAUSS: usize = 7;
pub const W_EGON: usize = 8;
pub const W_HORNET: usize = 9;
pub const W_GRENADE: usize = 10;
#[allow(dead_code)] // completes the slot table; no code names this slot
pub const W_SNARK: usize = 11;
#[allow(dead_code)] // completes the slot table; no code names this slot
pub const W_TRIPMINE: usize = 12;
pub const W_SATCHEL: usize = 13;
pub const N_WEAPONS: usize = 14;

#[derive(Clone, Copy)]
pub struct ModelDef {
    pub health: u8,
    pub target_h: i32,
    pub radius: i32,
    pub ai: u8,
    pub speed: u8,        // move speed (world units/tick); 0 = stationary
    pub atk_range: u16,   // attack engage distance (world units)
    pub atk_damage: u8,   // HP per hit
    pub atk_cooldown: u8, // ticks between attacks (20 Hz)
}
pub const fn mdef(health: u8, target_h: i32, radius: i32, ai: u8) -> ModelDef {
    ModelDef {
        health,
        target_h,
        radius,
        ai,
        speed: 0,
        atk_range: 0,
        atk_damage: 0,
        atk_cooldown: 0,
    }
}
pub const fn mdef_atk(
    health: u8,
    target_h: i32,
    radius: i32,
    ai: u8,
    speed: u8,
    atk_range: u16,
    atk_damage: u8,
    atk_cooldown: u8,
) -> ModelDef {
    ModelDef {
        health,
        target_h,
        radius,
        ai,
        speed,
        atk_range,
        atk_damage,
        atk_cooldown,
    }
}
// Combat params (speed/range/damage/cooldown) for AI_RANGED + AI_TURRET come from
// the per-enemy MDL/Half-Life survey; melee/idle/passive types ignore them.
pub const MODEL_DEFS: [ModelDef; N_MODEL_TYPES] = [
    mdef(SCIENTIST_HEALTH, 40, SCIENTIST_RENDER_RADIUS, AI_FLEE), // 0 scientist
    mdef(BARNEY_HEALTH, 40, BARNEY_RENDER_RADIUS, AI_ALLY),       // 1 barney
    mdef(HEADCRAB_HEALTH, 12, HEADCRAB_RENDER_RADIUS, AI_MELEE),  // 2 headcrab
    mdef(0, 16, 70, AI_ITEM),                                     // 3 item_suit
    mdef(0, 16, ITEM_RENDER_RADIUS, AI_ITEM),                     // 4 item_battery
    // zombie.mdl walk: 194.3717 units over 59 frame intervals at 22 fps =
    // 72.48 units/s (Half-Life's GetSequenceInfo/MoveExecute path).  The
    // integer 20 Hz mover rounds that to 4 units/tick, not the old 2: the old
    // value left c1a1's monstermaker zombie in Gordon's path instead of
    // reaching the scripted Barney fight.
    mdef_atk(50, 40, 94, AI_MELEE, 4, 0, 0, 0), // 5 zombie (slow shambler)
    mdef_atk(20, 20, 70, AI_RANGED, 7, 300, 15, 45), // 6 houndeye (skitter in, sonic blast)
    mdef_atk(40, 32, 91, AI_RANGED, 6, 600, 15, 55), // 7 bullsquid (acid spit at range)
    mdef_atk(50, 40, 90, AI_RANGED, 16, 1000, 5, 8), // 8 hgrunt (mp5 bursts)
    mdef_atk(30, 40, 90, AI_RANGED, 15, 800, 10, 24), // 9 alien_slave (zap)
    mdef_atk(60, 48, 100, AI_RANGED, 16, 500, 8, 4), // 10 alien_grunt (hornets)
    mdef_atk(60, 40, 100, AI_RANGED, 6, 800, 3, 14), // 11 alien_controller (energy)
    mdef(25, 32, 170, AI_IDLE),                 // 12 barnacle (retail spawns it with 25)
    mdef(16, 8, 40, AI_IDLE),                   // 13 leech (flyer: render only)
    mdef(1, 4, 30, AI_IDLE),                    // 14 cockroach (passive; any hit kills it)
    mdef(100, 48, 90, AI_IDLE),                 // 15 gman (passive)
    // CGargantua: the melee roster wakes it; garg::tick runs its schedules.
    mdef_atk(200, 90, 360, AI_MELEE, 10, 0, 0, 0), // 16 gargantua
    mdef(200, 90, 1748, AI_IDLE),                  // 17 nihilanth (boss: render only)
    mdef(150, 70, 200, AI_IDLE),                   // 18 bigmomma (boss: render only)
    mdef_atk(40, 20, 223, AI_MELEE, 6, 0, 0, 0),   // 19 ichthyosaur
    mdef_atk(40, 40, 80, AI_TURRET, 0, 1000, 7, 3), // 20 sentry
    mdef_atk(50, 40, 80, AI_TURRET, 0, 1200, 8, 7), // 21 turret
    mdef_atk(30, 30, 60, AI_TURRET, 0, 1000, 5, 3), // 22 miniturret
    mdef(80, 60, 410, AI_IDLE),                    // 23 apache (flyer: render only)
    mdef(10, 20, 60, AI_IDLE),                     // 24 flyer_flock (passive)
    mdef(50, 25, SCIENTIST_RENDER_RADIUS, AI_IDLE), // 25 sitting scientist (50, unlike the standing 20)
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 26 weapon_crowbar
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 27 weapon_9mmhandgun
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 28 weapon_357
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 29 weapon_9mmAR
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 30 weapon_shotgun
    mdef(0, 12, 56, AI_ITEM),                       // 31 weapon_crossbow
    mdef(0, 12, 45, AI_ITEM),                       // 32 weapon_rpg
    mdef(0, 12, 51, AI_ITEM),                       // 33 weapon_gauss
    mdef(0, 12, 40, AI_ITEM),                       // 34 weapon_egon
    mdef(0, 12, 45, AI_ITEM),                       // 35 weapon_hornetgun
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 36 weapon_handgrenade
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 37 weapon_snark
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 38 weapon_tripmine
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 39 weapon_satchel
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 40 ammo_9mmclip
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 41 ammo_9mmAR
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 42 ammo_buckshot
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 43 ammo_357
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 44 ammo_crossbow
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 45 ammo_rpgclip
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 46 ammo_gaussclip
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 47 ammo_ARgrenades
    mdef(0, 12, ITEM_RENDER_RADIUS, AI_ITEM),       // 48 item_healthkit
    mdef(0, 12, 38, AI_ITEM),                       // 49 item_longjump
    mdef(75, 90, 912, AI_IDLE), // 50 tentacle (Blast Pit; killtargeted by the rocket)
    mdef_atk(30, 40, 90, AI_RANGED, 16, 900, 6, 10), // 51 human assassin (silenced 9mm)
    // Script-only monster_generic. Health must stay nonzero so its exact
    // targetname can be possessed by c0a0d's goingdown sequence.
    mdef(8, 48, 1242, AI_IDLE), // 52 construction loader (rampwalk translates ~1,241 units)
    mdef(8, 48, 1930, AI_IDLE), // 53 forklift (path clips translate ~1,929 units)
    // Standing scientist behavior with sitidle/sitstand added in a dedicated
    // stream. Keeping those long poses out of type 0 preserves c4a3's peak RAM.
    mdef(SCIENTIST_HEALTH, 40, SCIENTIST_RENDER_RADIUS, AI_FLEE), // 54 scripted sitter
    mdef_atk(50, 40, 94, AI_MELEE, 4, 0, 0, 0),                   // 55 c1a1b vent-script zombies
    mdef(8, 40, 96, AI_IDLE),                                     // 56 Hazard Course Holo
    // Types 57-74 place models the campaign authors but that had no cooked
    // stream at all until the shared map/model arena freed the budget.
    // Health/radius follow the SDK spawns; the scenery props are idle puppets.
    mdef(1, 8, 40, AI_IDLE), // 57 tripmine (laser mine: render only)
    mdef_atk(2, 8, 40, AI_MELEE, 8, 0, 0, 0), // 58 snark (fast skittering biter)
    mdef_atk(10, 12, 40, AI_MELEE, 6, 0, 0, 0), // 59 baby headcrab (Gonarch's brood)
    mdef(8, 8, 40, AI_IDLE), // 60 rat (scenery critter)
    mdef(255, 200, 800, AI_IDLE), // 61 osprey (flyer: render only)
    mdef(8, 8, 40, AI_IDLE), // 62 gib legbone
    mdef(8, 8, 40, AI_IDLE), // 63 gib pelvis
    mdef(8, 16, 60, AI_IDLE), // 64 gib ribcage
    mdef(8, 8, 40, AI_IDLE), // 65 gib riblet
    mdef(8, 16, 60, AI_IDLE), // 66 zombie gibs
    mdef(8, 32, 70, AI_IDLE), // 67 file cabinet
    mdef(8, 16, 50, AI_IDLE), // 68 Xen hair
    mdef(8, 24, 70, AI_IDLE), // 69 Xen fungus
    mdef(8, 12, 40, AI_IDLE), // 70 Xen fungus (small)
    mdef(8, 40, 96, AI_IDLE), // 71 Xen fungus (large)
    mdef(8, 16, 50, AI_IDLE), // 72 pipe bubbles
    mdef(8, 24, 60, AI_IDLE), // 73 Xen plant light
    mdef(8, 60, 140, AI_IDLE), // 74 Xen tree
    mdef(0, 8, 16, AI_ITEM), // 75 dispenser can
];

// (ammo pool, rounds) per ammo pickup type 40..=47.
pub const AMMO_PICKUPS: [(usize, u16); 8] = [
    (AMMO_9MM, 17),
    (AMMO_9MM, 50),
    (AMMO_BUCK, 12),
    (AMMO_357, 6),
    (AMMO_BOLT, 5),
    (AMMO_ROCKET, 1),
    (AMMO_URANIUM, 20),
    (AMMO_ARGREN, 2),
];

/// The total rounds a weapon pickup grants. A fresh pickup loads its clip from
/// this (the rest goes to the pool); a duplicate adds it to the pool. Clipless
/// weapons arrive usable (grenade 5, snark 5, satchel 1, tripmine 1, hivehand 8,
/// gauss and egon 20).
pub const WEAPON_DEFAULT_GIVE: [u16; N_WEAPONS] = [0, 17, 6, 25, 12, 5, 1, 20, 20, 8, 5, 5, 1, 1];

pub const fn wdef(
    ammo: usize,
    clip: u16,
    reserve_max: u16,
    damage: u8,
    range: i32,
    pellets: u8,
    spread: i32,
    cooldown: u8,
    reload: u8,
    fire: u8,
    proj: u8,
    wm: u8,
) -> WeaponDef {
    WeaponDef {
        ammo,
        clip,
        reserve_max,
        damage,
        range,
        pellets,
        spread,
        cooldown,
        reload,
        fire,
        proj,
        wm,
    }
}

// Retail HL1 values (cooldown/reload in 20 Hz sim ticks). Weapon-specific
// primary/secondary state machines live below; this table holds their shared
// ammo, damage, cadence, and viewmodel data.
pub static WEAPON_DEFS: [WeaponDef; N_WEAPONS] = [
    wdef(AMMO_NONE, 0, 0, 10, 96, 1, 0, 7, 0, FIRE_MELEE, 0, 4),
    wdef(
        AMMO_9MM,
        17,
        250,
        8,
        GLOCK_RANGE,
        1,
        GLOCK_SPREAD_PX,
        6,
        30,
        FIRE_SEMI,
        0,
        0,
    ),
    wdef(
        AMMO_357,
        6,
        36,
        40,
        GLOCK_RANGE,
        1,
        PYTHON_SPREAD_PX,
        15,
        40,
        FIRE_SEMI,
        0,
        1,
    ),
    wdef(
        AMMO_9MM,
        50,
        250,
        5,
        GLOCK_RANGE,
        1,
        MP5_SPREAD_PX,
        2,
        30,
        FIRE_AUTO,
        0,
        2,
    ),
    wdef(
        AMMO_BUCK,
        8,
        125,
        5,
        SHOTGUN_RANGE,
        6,
        BUCKSHOT_SPREAD_PX,
        15,
        24,
        FIRE_SPREAD,
        0,
        12,
    ),
    wdef(
        AMMO_BOLT,
        5,
        50,
        50,
        GLOCK_RANGE,
        1,
        0,
        15,
        90,
        FIRE_PROJ,
        PROJ_BOLT,
        3,
    ),
    wdef(
        AMMO_ROCKET,
        1,
        5,
        100,
        0,
        1,
        0,
        30,
        40,
        FIRE_PROJ,
        PROJ_ROCKET,
        9,
    ),
    wdef(
        AMMO_URANIUM,
        0,
        100,
        20,
        GLOCK_RANGE,
        1,
        0,
        4,
        0,
        FIRE_SEMI,
        0,
        6,
    ),
    wdef(
        AMMO_URANIUM,
        0,
        100,
        14,
        GLOCK_RANGE,
        1,
        0,
        2,
        0,
        FIRE_AUTO,
        0,
        5,
    ),
    wdef(
        AMMO_HORNET,
        0,
        8,
        8,
        0,
        1,
        0,
        5,
        0,
        FIRE_PROJ,
        PROJ_HORNET,
        8,
    ),
    wdef(
        AMMO_GREN,
        0,
        10,
        100,
        0,
        1,
        0,
        10,
        0,
        FIRE_PROJ,
        PROJ_GRENADE,
        7,
    ),
    wdef(
        AMMO_SNARK, 0, 15, 10, 0, 1, 0, 6, 0, FIRE_PROJ, PROJ_SNARK, 13,
    ),
    wdef(
        AMMO_TRIPMINE,
        0,
        5,
        100,
        0,
        1,
        0,
        6,
        0,
        FIRE_PROJ,
        PROJ_TRIPMINE,
        14,
    ),
    wdef(
        AMMO_SATCHEL,
        0,
        5,
        150,
        0,
        1,
        0,
        20,
        0,
        FIRE_PROJ,
        PROJ_SATCHEL,
        10,
    ),
];

// A HECU grunt's hand grenade, in 20 Hz ticks, measured under the Xash3D
// reference: none in the first 80 ticks of an engagement, one every 155, the
// blast 25 to 27 ticks after the 34-tick throw begins. The port's throw runs
// `GRENADE_THROW_TICKS` and lets go with `GRENADE_RELEASE_LEFT` of them left.
pub const GRENADE_FIRST_DELAY: u16 = 80;
pub const GRENADE_INTERVAL: u16 = 155;
pub const GRENADE_THROW_TICKS: u8 = 31;
pub const GRENADE_RELEASE_LEFT: u8 = 16;
/// Ticks from the release to the blast.
pub const GRENADE_FLIGHT: i32 = 10;
/// Nearest and farthest target a grunt throws at.
pub const GRENADE_MIN_RANGE: i32 = 250;
pub const GRENADE_MAX_RANGE: i32 = 900;

// Secondary-fire cooldowns in ticks (the primary's live in WEAPON_DEFS).
pub const GLOCK_ALT_COOLDOWN: u8 = 4;
pub const M203_COOLDOWN: u8 = 20;
/// Blast radius of an explosion of `dmg`: the retail game spreads every
/// explosion over two and a half units per point of damage (measured: the
/// tripmine, 150 damage, reaches 375 units and falls off linearly).
pub const fn blast_radius(dmg: u8) -> i32 {
    dmg as i32 * 5 / 2
}
/// What a zombie keeps of a hit that is exactly a bullet (the gauss, the hive
/// hand's hornets: 20 -> 6, 5 -> 2 under the Xash3D reference): 30 percent,
/// rounded up from a half. Ordinary gun fire and bolts take full damage.
pub const fn zombie_exact_bullet(dmg: u8) -> u8 {
    ((dmg as u16 * 3 + 9) / 10) as u8
}
/// The MP5 grenade's impact damage (`sk_plr_9mmAR_grenade`).
pub const M203_DAMAGE: u8 = 100;
pub const SHOTGUN_ALT_COOLDOWN: u8 = 30;
pub const HORNET_ALT_COOLDOWN: u8 = 2;

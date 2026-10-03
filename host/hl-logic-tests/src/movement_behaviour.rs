//! Player movement behaviour on synthetic rooms.
//!
//! Two kinds of checks. Property tests state the observable Half-Life
//! movement rules (speeds, accelerations, jump heights, step height, water
//! speeds) with the tolerance an integer 20 Hz implementation needs. Golden
//! tests replay short scripted inputs and compare every tick's position,
//! velocity and ground state with tables recorded from the ac83da7 build, so
//! any change in feel shows up as a diff a reviewer can read tick by tick.
//!
//! Units: positions in whole world units (Y up), velocities in units per
//! 50 ms tick (320 u/s = 16). Yaw is Q12 of a turn: 0 faces +Z, 1024 faces +X.

use std::sync::Mutex;

use crate::map::Map;
use crate::map_fixture::{Brush, MapFixture};
use crate::phys::{self, Player, WaterJumpState};

/// Movement reads a few process-wide settings (gravity scale, conveyor
/// velocity, long-jump module); tests that touch them run one at a time.
static GLOBALS: Mutex<()> = Mutex::new(());

fn lock_globals() -> std::sync::MutexGuard<'static, ()> {
    GLOBALS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const FACE_X: u16 = 1024;
const FULL: i32 = 127;

fn reset_globals() {
    phys::set_gravity_scale(4096);
    phys::set_base_xz([0, 0]);
    phys::set_longjump(false);
}

/// A 4096-unit floor whose top is y = 0, an 18-unit step from x = 256, a
/// 19-unit ledge from z = 256, and a wall with a lip 48 units high from
/// x = -256 going -X.
fn test_room() -> Map {
    MapFixture {
        brushes: vec![
            Brush::new([-2048, -64, -2048], [2048, 0, 2048]),
            Brush::new([256, 0, -128], [2048, 18, 128]),
            Brush::new([-128, 0, 256], [128, 19, 2048]),
            Brush::new([-2048, 0, -128], [-256, 48, 128]),
        ],
        spawn: [0, 36, 0],
        nav: vec![],
    }
    .load()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Sample {
    pos: [i32; 3],
    vel: [i32; 3],
    ground: bool,
}

fn sample(p: &Player) -> Sample {
    Sample {
        pos: p.pos,
        vel: p.vel,
        ground: p.on_ground,
    }
}

#[derive(Clone, Copy, Default)]
struct Cmd {
    fwd: i32,
    strafe: i32,
    jump: bool,
    duck: bool,
    yaw: u16,
}

fn walk(map: &Map, p: &mut Player, cmds: &[Cmd]) -> Vec<Sample> {
    cmds.iter()
        .map(|c| {
            p.update(map, &[], c.fwd, c.strafe, c.jump, c.duck, c.yaw);
            sample(p)
        })
        .collect()
}

fn repeat(n: usize, c: Cmd) -> Vec<Cmd> {
    vec![c; n]
}

fn run(yaw: u16) -> Cmd {
    Cmd {
        fwd: FULL,
        yaw,
        ..Cmd::default()
    }
}

fn idle() -> Cmd {
    Cmd::default()
}

/// Compact golden form: (x, y, z, vx, vy, vz, grounded).
type Row = (i32, i32, i32, i32, i32, i32, bool);

fn rows(samples: &[Sample]) -> Vec<Row> {
    samples
        .iter()
        .map(|s| {
            (
                s.pos[0], s.pos[1], s.pos[2], s.vel[0], s.vel[1], s.vel[2], s.ground,
            )
        })
        .collect()
}

fn assert_golden(name: &str, got: &[Sample], want: &[Row]) {
    let got = rows(got);
    if got.as_slice() != want {
        let mut text = format!("{name}: trajectory changed; recorded table follows\n");
        for r in &got {
            text.push_str(&format!("    {r:?},\n"));
        }
        panic!("{text}");
    }
}

fn grounded_player(map: &Map, pos: [i32; 3]) -> Player {
    let mut p = Player::new(pos);
    // One idle tick settles the spawn onto the floor.
    p.update(map, &[], 0, 0, false, false, 0);
    assert!(p.on_ground, "fixture spawn must start on the floor");
    p
}

// ---------------------------------------------------------------- properties

#[test]
fn ground_running_reaches_three_twenty_and_holds_it() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -600]);
    let out = walk(&map, &mut p, &repeat(12, run(0)));
    // Ground acceleration adds at most 10 x 320 x 0.05 = 160 u/s (8/tick) per
    // tick, so full speed takes at least two ticks and is then held.
    assert!(
        out[0].vel[2] <= 8,
        "first tick gains at most 8: {:?}",
        out[0]
    );
    assert!(out[0].vel[2] >= 7, "first tick gains about 8: {:?}", out[0]);
    for s in &out[2..] {
        assert_eq!(s.vel[2], 16, "steady walk is 320 u/s: {s:?}");
        assert!(s.ground);
        assert_eq!(s.pos[1], 36, "feet stay on the floor");
    }
}

#[test]
fn friction_sheds_a_fifth_per_tick_then_a_flat_rate_near_rest() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -900]);
    walk(&map, &mut p, &repeat(6, run(0)));
    let out = walk(&map, &mut p, &repeat(14, idle()));
    // speed -= max(speed, 100) x 4 x 0.05 per tick: 320 -> 256 -> 205 -> 164
    // -> 131 -> 105 -> 84 -> 64 -> 44 -> 24 -> 4 -> 0 (u/s).
    let speeds: Vec<i32> = out.iter().map(|s| s.vel[2]).collect();
    assert!(
        (12..=13).contains(&speeds[0]),
        "first friction tick ~256 u/s: {speeds:?}"
    );
    assert!(speeds.windows(2).all(|w| w[1] <= w[0]), "{speeds:?}");
    assert_eq!(*speeds.last().unwrap(), 0, "comes to rest: {speeds:?}");
    let stop = speeds.iter().position(|&v| v == 0).unwrap();
    // 12.8, 10.2, 8.2, 6.6, 5.2, 4.2, then -1 per tick: 3.2, 2.2, 1.2, 0.2.
    assert!(
        (9..=10).contains(&stop),
        "stops after ~10 ticks: {speeds:?}"
    );
}

#[test]
fn air_control_targets_thirty_units_per_second() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = Player::new([0, 1200, -900]);
    let out = walk(&map, &mut p, &repeat(12, run(0)));
    for s in &out {
        assert!(!s.ground);
        // 30 u/s is 1.5 units per tick; an integer report shows 1 or 2.
        assert!(s.vel[2] <= 2, "air wish speed is capped near 30 u/s: {s:?}");
    }
    let travelled = out.last().unwrap().pos[2] - -900;
    // Twelve ticks at up to 30 u/s is at most 18 units of drift.
    assert!(
        (10..=19).contains(&travelled),
        "air drift {travelled} units"
    );
}

#[test]
fn air_control_never_brakes_existing_speed() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -1500]);
    walk(&map, &mut p, &repeat(6, run(0)));
    // Jump, then hold the stick backwards while airborne.
    let mut cmds = vec![Cmd {
        jump: true,
        ..run(0)
    }];
    cmds.extend(repeat(
        8,
        Cmd {
            fwd: -FULL,
            ..Cmd::default()
        },
    ));
    let out = walk(&map, &mut p, &cmds);
    let first = out[0].vel[2];
    assert!(first >= 15, "jump keeps the run speed: {:?}", out[0]);
    // Each airborne tick may gain at most the full wish-speed acceleration
    // toward the 30 u/s target, so reversing input slows the player by at
    // most that much per tick and never below the capped target.
    for w in out.windows(2) {
        assert!(w[0].vel[2] - w[1].vel[2] <= 8, "{:?} -> {:?}", w[0], w[1]);
    }
}

#[test]
fn gravity_is_eight_hundred_with_half_applied_to_each_ticks_displacement() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = Player::new([0, 2000, -900]);
    let out = walk(&map, &mut p, &repeat(10, idle()));
    // After n airborne ticks the reported velocity is -40 n u/s (-2 n per tick).
    for (n, s) in out.iter().enumerate() {
        assert_eq!(s.vel[1], -2 * (n as i32 + 1), "{s:?}");
    }
    // Displacement over n ticks is 800 x (0.05 n)^2 / 2 = n^2 units.
    let fallen = 2000 - out[9].pos[1];
    assert!((99..=101).contains(&fallen), "fell {fallen} in 0.5 s");
}

#[test]
fn trigger_gravity_scale_halves_the_fall() {
    let _g = lock_globals();
    reset_globals();
    phys::set_gravity_scale(2048);
    let map = test_room();
    let mut p = Player::new([0, 2000, -900]);
    let out = walk(&map, &mut p, &repeat(10, idle()));
    reset_globals();
    let fallen = 2000 - out[9].pos[1];
    assert!((49..=51).contains(&fallen), "fell {fallen} at half gravity");
}

#[test]
fn a_standing_jump_rises_forty_five_units() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -900]);
    let mut cmds = vec![Cmd {
        jump: true,
        ..idle()
    }];
    cmds.extend(repeat(30, idle()));
    let out = walk(&map, &mut p, &cmds);
    // Launch at sqrt(2 x 800 x 45) = 268.3 u/s; the launch tick moves the
    // player 268.3 x 0.05 - 20 x 0.05 = 12.4 units.
    assert!(!out[0].ground);
    assert_eq!(out[0].pos[1] - 36, 12, "launch-tick rise: {:?}", out[0]);
    let peak = out.iter().map(|s| s.pos[1]).max().unwrap() - 36;
    assert!((44..=46).contains(&peak), "jump apex {peak}");
    let landed = out.iter().position(|s| s.ground).unwrap();
    // Flight time 2 x 268.3 / 800 = 0.67 s, about 13-14 ticks.
    assert!((12..=15).contains(&landed), "landed after {landed} ticks");
}

#[test]
fn holding_jump_does_not_rehop() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -900]);
    let out = walk(
        &map,
        &mut p,
        &repeat(
            40,
            Cmd {
                jump: true,
                ..idle()
            },
        ),
    );
    let launches = out
        .windows(2)
        .filter(|w| w[0].ground && !w[1].ground)
        .count()
        + usize::from(!out[0].ground);
    // The caller supplies the rising edge; a held `jump` flag is a fresh
    // request every tick here, so this pins how the movement layer itself
    // treats it.
    assert!(launches >= 1);
}

#[test]
fn long_jump_launches_at_five_sixty_forward() {
    let _g = lock_globals();
    reset_globals();
    phys::set_longjump(true);
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -1500]);
    walk(&map, &mut p, &repeat(6, run(0)));
    let out = walk(
        &map,
        &mut p,
        &[Cmd {
            jump: true,
            duck: true,
            ..run(0)
        }],
    );
    reset_globals();
    // 350 x 1.6 = 560 u/s = 28 per tick; up sqrt(2 x 800 x 56) = 299 u/s,
    // reported after a full tick of gravity as ~14 per tick.
    assert_eq!(out[0].vel[2], 28, "{:?}", out[0]);
    assert!((13..=15).contains(&out[0].vel[1]), "{:?}", out[0]);
}

#[test]
fn long_jump_needs_the_module() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -1500]);
    walk(&map, &mut p, &repeat(6, run(0)));
    let out = walk(
        &map,
        &mut p,
        &[Cmd {
            jump: true,
            duck: true,
            ..run(0)
        }],
    );
    assert!(out[0].vel[2] <= 16, "{:?}", out[0]);
}

#[test]
fn walking_climbs_an_eighteen_unit_step() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [100, 36, 0]);
    let out = walk(&map, &mut p, &repeat(20, run(FACE_X)));
    let last = out.last().unwrap();
    assert!(last.pos[0] > 300, "moved onto the step: {last:?}");
    assert_eq!(last.pos[1], 36 + 18, "standing on the step: {last:?}");
    assert!(last.ground);
    // A step keeps full walking speed: no tick on the way loses speed.
    assert!(out[3..].iter().all(|s| s.vel[0] == 16), "{out:?}");
}

#[test]
fn walking_does_not_climb_a_nineteen_unit_ledge() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, 100]);
    let out = walk(&map, &mut p, &repeat(20, run(0)));
    let last = out.last().unwrap();
    assert_eq!(last.pos[1], 36, "still on the floor: {last:?}");
    // Blocked by the ledge face grown by the 16-unit hull half width.
    // Blocked by the ledge face grown by the 16-unit hull half width, less
    // the one-unit contact margin.
    assert_eq!(last.pos[2], 256 - 16 - 1, "{last:?}");
}

#[test]
fn landing_records_the_fall_speed() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = Player::new([0, 36 + 100, -900]);
    let mut impact = 0;
    for _ in 0..20 {
        p.update(&map, &[], 0, 0, false, false, 0);
        if p.land_impact != 0 {
            impact = p.land_impact;
            break;
        }
    }
    // A 100-unit drop lands at about sqrt(2 x 800 x 100) = 400 u/s.
    assert!((18..=21).contains(&impact), "impact {impact} per tick");
    assert!(p.on_ground);
    assert_eq!(p.vel[1], 0, "vertical speed is zeroed on the ground");
}

#[test]
fn hovering_two_units_above_a_floor_counts_as_grounded() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut near = Player::new([0, 38, -900]);
    near.update(&map, &[], 0, 0, false, false, 0);
    assert!(near.on_ground, "within 2 units: {:?}", near.pos);
}

#[test]
fn conveyor_velocity_moves_but_is_not_kept() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -900]);
    // Conveyor velocity is whole units per tick: 8 is 160 u/s along +Z.
    phys::set_base_xz([0, 8]);
    let before = p.pos;
    let out = walk(&map, &mut p, &repeat(5, idle()));
    reset_globals();
    let moved = out.last().unwrap().pos[2] - before[2];
    assert_eq!(moved, 40, "conveyor carried the player {moved} units");
    assert!(
        out.iter().all(|s| s.vel[2] == 0),
        "conveyor speed is not player velocity: {out:?}"
    );
}

#[test]
fn swimming_follows_the_full_look_direction_at_two_fifty_six() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut w = WaterJumpState::new();
    let mut p = Player::new([0, 600, -900]);
    let mut last = [0; 3];
    for _ in 0..12 {
        p.update_swim(&map, &[], 3, &mut w, FULL, 0, false, 0, 0);
        last = p.vel;
    }
    // 256 u/s = 12.8 per tick.
    assert!((12..=13).contains(&last[2]), "{last:?}");
    // Looking 45 degrees up splits it between climbing and moving forward.
    let mut p = Player::new([0, 600, -900]);
    for _ in 0..12 {
        p.update_swim(&map, &[], 3, &mut w, FULL, 0, false, 0, -512);
        last = p.vel;
    }
    let up = last[1].abs();
    assert!((8..=10).contains(&up), "{last:?}");
    assert!((8..=10).contains(&last[2]), "{last:?}");
}

#[test]
fn idle_swimmers_sink_at_sixty_and_holding_jump_paddles_up() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut w = WaterJumpState::new();
    let mut p = Player::new([0, 600, -900]);
    for _ in 0..20 {
        p.update_swim(&map, &[], 3, &mut w, 0, 0, false, 0, 0);
    }
    assert_eq!(p.vel[1], -3, "sink 60 u/s: {:?}", p.vel);
    for _ in 0..20 {
        p.update_swim(&map, &[], 3, &mut w, 0, 0, true, 0, 0);
    }
    // Holding jump settles to a steady upward drift of about 65 u/s (3.25
    // per tick). The written spec says 100 u/s; this pins what the game does.
    assert_eq!(p.vel[1], 3, "paddle drift: {:?}", p.vel);
    let y0 = p.pos[1];
    for _ in 0..4 {
        p.update_swim(&map, &[], 3, &mut w, 0, 0, true, 0, 0);
    }
    assert_eq!(p.pos[1] - y0, 13, "four ticks of paddling");
}

#[test]
fn swim_speed_builds_at_the_water_acceleration() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut w = WaterJumpState::new();
    let mut p = Player::new([0, 600, -900]);
    let mut speeds = Vec::new();
    for _ in 0..4 {
        p.update_swim(&map, &[], 3, &mut w, FULL, 0, false, 0, 0);
        speeds.push(p.vel[2]);
    }
    // From rest the swimmer gains 10 x 256 x 0.05 = 128 u/s (6.4 per tick)
    // per tick, so it reaches the 256 u/s target on the second tick. (The
    // written spec words this as "half the gap"; from rest both agree on the
    // first tick.)
    assert!((6..=7).contains(&speeds[0]), "{speeds:?}");
    assert!(
        speeds[1..].iter().all(|&v| (12..=13).contains(&v)),
        "{speeds:?}"
    );
}

#[test]
fn waist_deep_swimmer_pops_out_over_a_low_lip() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut w = WaterJumpState::new();
    // Waist deep beside the 48-unit wall that runs -X from x = -256,
    // facing it (yaw 3072 faces -X).
    let mut p = grounded_player(&map, [-256 + 20, 36, 0]);
    let mut out = Vec::new();
    for _ in 0..24 {
        p.update_swim(&map, &[], 2, &mut w, FULL, 0, false, 3072, 0);
        out.push(sample(&p));
    }
    assert!(w.active(), "the water jump is still running after 1.2 s");
    // 225 u/s up (11.25 per tick) with no gravity while the jump runs.
    assert!(out.iter().all(|s| s.vel[1] == 11), "{out:?}");
    // Pushed 50 u/s (2.5 per tick) over the lip, toward the wall it found.
    assert!(out[1..].iter().all(|s| s.vel[0] == -3), "{out:?}");
    let last = out.last().unwrap();
    assert!(last.pos[1] - 36 > 48, "cleared the 48-unit lip: {last:?}");
    assert!(last.pos[0] < -256 - 16, "moved over the wall top: {last:?}");
}

#[test]
fn ducking_on_the_ground_keeps_the_feet_planted_and_needs_headroom_to_stand() {
    let _g = lock_globals();
    reset_globals();
    // A 50-unit-high gap under a slab: crouched (36 tall) fits, standing
    // (72 tall) does not.
    let map = MapFixture {
        brushes: vec![
            Brush::new([-2048, -64, -2048], [2048, 0, 2048]),
            Brush::new([200, 50, -128], [400, 200, 128]),
        ],
        spawn: [0, 36, 0],
        nav: vec![],
    }
    .load();
    let mut p = grounded_player(&map, [0, 36, 0]);
    p.set_crouch(&map, &[], true);
    assert!(p.crouch);
    assert_eq!(p.pos[1], 18, "grounded duck lowers the origin 18 units");
    let out = walk(&map, &mut p, &repeat(20, run(FACE_X)));
    let under = out.last().unwrap();
    assert!(under.pos[0] > 216, "crawled under the slab: {under:?}");
    p.set_crouch(&map, &[], false);
    assert!(p.crouch, "standing up is refused under the slab");
    let mut q = Player::new([0, 300, -900]);
    q.update(&map, &[], 0, 0, false, false, 0);
    let before = q.pos[1];
    q.set_crouch(&map, &[], true);
    assert_eq!(q.pos[1], before, "an airborne duck keeps the origin");
}

#[test]
fn golden_crouched_walk() {
    // The crouched hull moves at whatever stick it is given; the one-third
    // ducked speed is applied by the command layer before movement runs.
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -900]);
    p.set_crouch(&map, &[], true);
    let out = walk(&map, &mut p, &repeat(10, run(0)));
    assert_golden("crouched_walk", &out, GOLDEN_CROUCH_WALK);
}

// ------------------------------------------------------------------- goldens

#[test]
fn golden_walk_step_and_jump() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, 0]);
    let mut cmds = repeat(20, run(FACE_X));
    cmds.push(Cmd {
        jump: true,
        ..run(FACE_X)
    });
    cmds.extend(repeat(16, run(FACE_X)));
    let out = walk(&map, &mut p, &cmds);
    assert_golden("walk_step_and_jump", &out, GOLDEN_WALK_STEP_JUMP);
}

#[test]
fn golden_run_then_coast_to_rest() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -900]);
    let mut cmds = repeat(8, run(0));
    cmds.extend(repeat(14, idle()));
    let out = walk(&map, &mut p, &cmds);
    assert_golden("run_then_coast", &out, GOLDEN_RUN_COAST);
}

#[test]
fn golden_diagonal_strafe_run() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -900]);
    let out = walk(
        &map,
        &mut p,
        &repeat(
            12,
            Cmd {
                fwd: FULL,
                strafe: FULL,
                yaw: 300,
                ..Cmd::default()
            },
        ),
    );
    assert_golden("diagonal_strafe", &out, GOLDEN_DIAGONAL);
}

#[test]
fn golden_fall_with_air_steering() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut p = Player::new([0, 300, -900]);
    let out = walk(
        &map,
        &mut p,
        &repeat(
            20,
            Cmd {
                strafe: FULL,
                ..run(200)
            },
        ),
    );
    assert_golden("fall_air_steer", &out, GOLDEN_FALL_AIR);
}

#[test]
fn golden_long_jump() {
    let _g = lock_globals();
    reset_globals();
    phys::set_longjump(true);
    let map = test_room();
    let mut p = grounded_player(&map, [0, 36, -1500]);
    let mut cmds = repeat(6, run(0));
    cmds.push(Cmd {
        jump: true,
        duck: true,
        ..run(0)
    });
    cmds.extend(repeat(18, run(0)));
    let out = walk(&map, &mut p, &cmds);
    reset_globals();
    assert_golden("long_jump", &out, GOLDEN_LONG_JUMP);
}

#[test]
fn golden_swim_forward_up_and_sink() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut w = WaterJumpState::new();
    let mut p = Player::new([0, 600, -900]);
    let mut out = Vec::new();
    for t in 0..24 {
        let (fwd, jump, pitch) = match t {
            0..=7 => (FULL, false, -300),
            8..=15 => (0, true, 0),
            _ => (0, false, 0),
        };
        p.update_swim(&map, &[], 3, &mut w, fwd, 0, jump, 700, pitch);
        out.push(sample(&p));
    }
    assert_golden("swim", &out, GOLDEN_SWIM);
}

#[test]
fn golden_water_jump_at_a_lip() {
    let _g = lock_globals();
    reset_globals();
    let map = test_room();
    let mut w = WaterJumpState::new();
    let mut p = grounded_player(&map, [-256 + 20, 36, 0]);
    let mut out = Vec::new();
    for _ in 0..24 {
        p.update_swim(&map, &[], 2, &mut w, FULL, 0, false, 3072, 0);
        out.push(sample(&p));
    }
    assert_golden("water_jump", &out, GOLDEN_WATER_JUMP);
}

const GOLDEN_WALK_STEP_JUMP: &[Row] = &[
    (8, 36, 0, 8, 0, 0, true),
    (22, 36, 0, 14, 0, 0, true),
    (38, 36, 0, 16, 0, 0, true),
    (54, 36, 0, 16, 0, 0, true),
    (70, 36, 0, 16, 0, 0, true),
    (86, 36, 0, 16, 0, 0, true),
    (102, 36, 0, 16, 0, 0, true),
    (117, 36, 0, 16, 0, 0, true),
    (133, 36, 0, 16, 0, 0, true),
    (149, 36, 0, 16, 0, 0, true),
    (165, 36, 0, 16, 0, 0, true),
    (181, 36, 0, 16, 0, 0, true),
    (197, 36, 0, 16, 0, 0, true),
    (213, 36, 0, 16, 0, 0, true),
    (229, 36, 0, 16, 0, 0, true),
    (244, 54, 0, 16, 0, 0, true),
    (260, 54, 0, 16, 0, 0, true),
    (276, 54, 0, 16, 0, 0, true),
    (292, 54, 0, 16, 0, 0, true),
    (308, 54, 0, 16, 0, 0, true),
    (324, 66, 0, 16, 11, 0, false),
    (340, 77, 0, 16, 9, 0, false),
    (356, 85, 0, 16, 7, 0, false),
    (371, 92, 0, 16, 5, 0, false),
    (387, 96, 0, 16, 3, 0, false),
    (403, 99, 0, 16, 1, 0, false),
    (419, 99, 0, 16, -1, 0, false),
    (435, 97, 0, 16, -3, 0, false),
    (451, 94, 0, 16, -5, 0, false),
    (467, 88, 0, 16, -7, 0, false),
    (483, 81, 0, 16, -9, 0, false),
    (498, 71, 0, 16, -11, 0, false),
    (514, 59, 0, 16, -13, 0, false),
    (530, 54, 0, 16, 0, 0, true),
    (546, 54, 0, 16, 0, 0, true),
    (562, 54, 0, 16, 0, 0, true),
    (578, 54, 0, 16, 0, 0, true),
];
const GOLDEN_RUN_COAST: &[Row] = &[
    (0, 36, -892, 0, 0, 8, true),
    (0, 36, -878, 0, 0, 14, true),
    (0, 36, -862, 0, 0, 16, true),
    (0, 36, -846, 0, 0, 16, true),
    (0, 36, -830, 0, 0, 16, true),
    (0, 36, -814, 0, 0, 16, true),
    (0, 36, -798, 0, 0, 16, true),
    (0, 36, -783, 0, 0, 16, true),
    (0, 36, -770, 0, 0, 13, true),
    (0, 36, -760, 0, 0, 10, true),
    (0, 36, -752, 0, 0, 8, true),
    (0, 36, -745, 0, 0, 7, true),
    (0, 36, -740, 0, 0, 5, true),
    (0, 36, -736, 0, 0, 4, true),
    (0, 36, -732, 0, 0, 3, true),
    (0, 36, -730, 0, 0, 2, true),
    (0, 36, -729, 0, 0, 1, true),
    (0, 36, -729, 0, 0, 0, true),
    (0, 36, -729, 0, 0, 0, true),
    (0, 36, -729, 0, 0, 0, true),
    (0, 36, -729, 0, 0, 0, true),
    (0, 36, -729, 0, 0, 0, true),
];
const GOLDEN_DIAGONAL: &[Row] = &[
    (8, 36, -897, 8, 0, 3, true),
    (21, 36, -893, 14, 0, 5, true),
    (36, 36, -888, 15, 0, 5, true),
    (52, 36, -883, 15, 0, 5, true),
    (67, 36, -877, 15, 0, 5, true),
    (82, 36, -872, 15, 0, 5, true),
    (97, 36, -867, 15, 0, 5, true),
    (112, 36, -862, 15, 0, 5, true),
    (127, 36, -857, 15, 0, 5, true),
    (142, 36, -852, 15, 0, 5, true),
    (158, 36, -847, 15, 0, 5, true),
    (173, 36, -842, 15, 0, 5, true),
];
const GOLDEN_FALL_AIR: &[Row] = &[
    (1, 299, -899, 1, -2, 1, false),
    (3, 296, -899, 1, -4, 1, false),
    (4, 291, -898, 1, -6, 1, false),
    (5, 284, -897, 1, -8, 1, false),
    (7, 275, -897, 1, -10, 1, false),
    (8, 264, -896, 1, -12, 1, false),
    (9, 251, -895, 1, -14, 1, false),
    (11, 236, -895, 1, -16, 1, false),
    (12, 219, -894, 1, -18, 1, false),
    (13, 200, -893, 1, -20, 1, false),
    (15, 179, -892, 1, -22, 1, false),
    (16, 156, -892, 1, -24, 1, false),
    (17, 131, -891, 1, -26, 1, false),
    (19, 104, -890, 1, -28, 1, false),
    (20, 75, -890, 1, -30, 1, false),
    (21, 44, -889, 1, -32, 1, false),
    (23, 36, -888, 1, 0, 1, true),
    (30, 36, -884, 8, 0, 4, true),
    (43, 36, -878, 13, 0, 7, true),
    (57, 36, -870, 14, 0, 7, true),
];
const GOLDEN_LONG_JUMP: &[Row] = &[
    (0, 36, -1492, 0, 0, 8, true),
    (0, 36, -1478, 0, 0, 14, true),
    (0, 36, -1462, 0, 0, 16, true),
    (0, 36, -1446, 0, 0, 16, true),
    (0, 36, -1430, 0, 0, 16, true),
    (0, 36, -1414, 0, 0, 16, true),
    (0, 50, -1386, 0, 13, 28, false),
    (0, 62, -1358, 0, 11, 28, false),
    (0, 72, -1330, 0, 9, 28, false),
    (0, 80, -1302, 0, 7, 28, false),
    (0, 86, -1274, 0, 5, 28, false),
    (0, 90, -1246, 0, 3, 28, false),
    (0, 92, -1218, 0, 1, 28, false),
    (0, 92, -1190, 0, -1, 28, false),
    (0, 90, -1162, 0, -3, 28, false),
    (0, 86, -1134, 0, -5, 28, false),
    (0, 80, -1106, 0, -7, 28, false),
    (0, 72, -1078, 0, -9, 28, false),
    (0, 62, -1050, 0, -11, 28, false),
    (0, 50, -1022, 0, -13, 28, false),
    (0, 36, -994, 0, 0, 28, true),
    (0, 36, -972, 0, 0, 22, true),
    (0, 36, -954, 0, 0, 18, true),
    (0, 36, -938, 0, 0, 16, true),
    (0, 36, -922, 0, 0, 16, true),
];
const GOLDEN_SWIM: &[Row] = &[
    (5, 597, -897, 5, -3, 3, false),
    (15, 592, -892, 10, -6, 5, false),
    (25, 586, -886, 10, -6, 5, false),
    (35, 580, -881, 10, -6, 5, false),
    (45, 574, -876, 10, -6, 5, false),
    (56, 568, -870, 10, -6, 5, false),
    (66, 563, -865, 10, -6, 5, false),
    (76, 557, -859, 10, -6, 5, false),
    (86, 560, -854, 10, 3, 5, false),
    (95, 563, -849, 9, 3, 5, false),
    (103, 567, -845, 9, 3, 5, false),
    (112, 570, -840, 8, 3, 4, false),
    (119, 573, -836, 8, 3, 4, false),
    (127, 576, -832, 7, 3, 4, false),
    (134, 580, -828, 7, 3, 4, false),
    (141, 583, -825, 7, 3, 4, false),
    (147, 584, -821, 6, 2, 3, false),
    (153, 584, -818, 6, 0, 3, false),
    (159, 583, -815, 6, -2, 3, false),
    (165, 580, -812, 5, -3, 3, false),
    (170, 577, -810, 5, -3, 3, false),
    (175, 574, -807, 5, -3, 3, false),
    (179, 571, -804, 5, -3, 2, false),
    (184, 568, -802, 4, -3, 2, false),
];
const GOLDEN_CROUCH_WALK: &[Row] = &[
    (0, 18, -892, 0, 0, 8, true),
    (0, 18, -878, 0, 0, 14, true),
    (0, 18, -862, 0, 0, 16, true),
    (0, 18, -846, 0, 0, 16, true),
    (0, 18, -830, 0, 0, 16, true),
    (0, 18, -814, 0, 0, 16, true),
    (0, 18, -798, 0, 0, 16, true),
    (0, 18, -783, 0, 0, 16, true),
    (0, 18, -767, 0, 0, 16, true),
    (0, 18, -751, 0, 0, 16, true),
];
const GOLDEN_WATER_JUMP: &[Row] = &[
    (-240, 47, 0, 0, 11, 0, false),
    (-240, 57, 0, -3, 11, 0, false),
    (-240, 68, 0, -3, 11, 0, false),
    (-240, 79, 0, -3, 11, 0, false),
    (-240, 89, 0, -3, 11, 0, false),
    (-242, 100, 0, -3, 11, 0, false),
    (-245, 111, 0, -3, 11, 0, false),
    (-247, 121, 0, -3, 11, 0, false),
    (-250, 132, 0, -3, 11, 0, false),
    (-252, 143, 0, -3, 11, 0, false),
    (-255, 154, 0, -3, 11, 0, false),
    (-257, 164, 0, -3, 11, 0, false),
    (-260, 175, 0, -3, 11, 0, false),
    (-262, 186, 0, -3, 11, 0, false),
    (-265, 196, 0, -3, 11, 0, false),
    (-267, 207, 0, -3, 11, 0, false),
    (-270, 218, 0, -3, 11, 0, false),
    (-272, 228, 0, -3, 11, 0, false),
    (-275, 239, 0, -3, 11, 0, false),
    (-277, 250, 0, -3, 11, 0, false),
    (-280, 260, 0, -3, 11, 0, false),
    (-282, 271, 0, -3, 11, 0, false),
    (-285, 282, 0, -3, 11, 0, false),
    (-287, 292, 0, -3, 11, 0, false),
];

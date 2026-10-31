//! Schedule runtime against a scripted world: a squad of grunts meets an enemy.
use hl_format::ai::{cond, state, Tables};
use hl_logic_tests::ai::*;
use hl_sched::Source;

const SRC: &str = r#"
species base id=250
species grunt id=8 parent=base flags=A r1=0..2048
sched "Idle Stand" species=base interrupt=NEW_ENEMY|HEAR_SOUND sound=COMBAT
  SET_ACTIVITY IDLE
  WAIT 1
end
sched "Combat Face" species=base interrupt=NEW_ENEMY|CAN_RANGE_ATTACK1
  STOP_MOVING
  FACE_ENEMY
  WAIT 1.5
end
sched "Range Attack1A" species=grunt interrupt=ENEMY_DEAD|ENEMY_OCCLUDED
  STOP_MOVING
  FACE_ENEMY
  RANGE_ATTACK1
  FACE_ENEMY
  RANGE_ATTACK1
end
sched "Take Cover" species=grunt
  STOP_MOVING
  FIND_COVER_FROM_ENEMY
  RUN_PATH
  WAIT_FOR_MOVEMENT
  REMEMBER IN_COVER
  SET_SCHEDULE COMBAT_FACE
end
sched "Fail" species=base
  WAIT 2
end
type base IDLE_STAND -> "Idle Stand"
type base COMBAT_FACE -> "Combat Face"
type base FAIL -> "Fail"
type grunt RANGE_ATTACK1 -> "Range Attack1A"
type grunt TAKE_COVER_FROM_ENEMY -> "Take Cover"
pick grunt states=COMBAT any=NEW_ENEMY when IN_SQUAD,NOT_LEADER -> TAKE_COVER_FROM_ENEMY
pick grunt states=COMBAT any=CAN_RANGE_ATTACK1 when OCCUPY_ENGAGE -> RANGE_ATTACK1
pick grunt states=COMBAT none=CAN_RANGE_ATTACK1 -> TAKE_COVER_FROM_ENEMY
pick base states=IDLE|ALERT|COMBAT|HUNT -> IDLE_STAND
"#;

struct Mock {
    now: u16,
    sees: bool,
    shots: u32,
    seq_left: [u8; 8],
    moving: [u8; 8],
    log: Vec<(u16, u8, &'static str)>,
}

impl World for Mock {
    fn now(&self) -> u16 { self.now }
    fn rand(&mut self, _n: u32) -> u32 { 0 }
    fn skill_hard(&self) -> bool { false }
    fn look(&mut self, _p: u8, enemy: u8) -> (u8, u32) {
        if self.sees {
            let new = if enemy == NONE { cond::NEW_ENEMY } else { 0 };
            (0, cond::SEE_ENEMY | cond::CAN_RANGE_ATTACK1 | new)
        } else {
            (enemy, 0)
        }
    }
    fn enemy_is_player(&self, _e: u8) -> bool { true }
    fn damage_ahead(&self, _p: u8) -> bool { true }
    fn danger_sound(&self, _p: u8) -> bool { false }
    fn set_activity(&mut self, _p: u8, _a: u8) {}
    fn seq_done(&self, p: u8) -> bool { self.seq_left[p as usize] == 0 }
    fn face(&mut self, _p: u8, _e: bool) -> bool { true }
    fn turn_by(&mut self, _p: u8, _d: i16) {}
    fn stop(&mut self, p: u8) { self.moving[p as usize] = 0; }
    fn path_to(&mut self, _p: u8, _g: Goal, out: &mut [i16; 3]) -> bool { *out = [1, 2, 3]; true }
    fn begin_move(&mut self, p: u8, _run: bool) -> bool { self.moving[p as usize] = 6; true }
    fn move_tick(&mut self, p: u8) -> Move {
        let m = &mut self.moving[p as usize];
        if *m == 0 { Move::Done } else { *m -= 1; if *m == 0 { Move::Done } else { Move::Moving } }
    }
    fn find_cover(&mut self, _p: u8, _f: Cover, _a: i32, _b: i32, out: &mut [i16; 3]) -> bool { *out = [9, 9, 9]; true }
    fn store_position(&self, _p: u8) -> [i16; 3] { [0; 3] }
    fn attack(&mut self, p: u8, _t: u8) { self.seq_left[p as usize] = 4; self.shots += 1; }
    fn custom(&mut self, _p: u8, _id: i16, _first: bool) -> (Task, u32) { (Task::Complete, 0) }
    fn die(&mut self, _p: u8) {}
}

fn tables() -> Tables {
    let mut s = Source::default();
    s.add(SRC).expect("parse");
    let blob: &'static [u8] = Box::leak(s.encode(&["grunt"]).expect("encode").into_boxed_slice());
    Tables::new(blob).expect("valid")
}

#[test]
fn squad_forms_leader_engages_member_takes_cover() {
    let t = tables();
    let mut ai: Ai<8> = Ai::new();
    for p in 0..3u8 {
        ai.claim(p, 8, 0).expect("row");
    }
    ai.recruit(0, 4, 1_000_000, |_, _| 10);
    assert_eq!(ai.squad_count(0), 3);
    assert_eq!(ai.rows[1].squad, ai.rows[0].squad);
    let mut w = Mock { now: 0, sees: false, shots: 0, seq_left: [0; 8], moving: [0; 8], log: vec![] };
    for tick in 0..6 {
        w.now = tick;
        for i in 0..3 { ai.think(&mut w, &t, i, true); }
    }
    assert!(ai.rows.iter().take(3).all(|r| r.state == state::IDLE));
    // The enemy appears: every member sees it.
    w.sees = true;
    for tick in 6..60 {
        w.now = tick;
        for s in w.seq_left.iter_mut() { *s = s.saturating_sub(1); }
        for i in 0..3 { ai.think(&mut w, &t, i, true); }
    }
    assert!(ai.rows.iter().take(3).all(|r| r.state == state::COMBAT));
    assert!(w.shots > 0, "somebody fired");
    // Leader holds an engage slot; slots never exceed two.
    let id = ai.rows[0].squad as usize;
    assert!(ai.squads.slots[id].count_ones() <= 2);
    // Non-leaders went for cover first (they remembered it).
    let covered = (1..3).filter(|&i| ai.remembers(i, MEM_IN_COVER)).count();
    assert!(covered >= 1, "a member took cover");
}

#[test]
fn leaving_the_squad_promotes_a_member() {
    let mut ai: Ai<4> = Ai::new();
    for p in 0..3u8 {
        ai.claim(p, 8, 0);
    }
    ai.recruit(0, 4, 1_000_000, |_, _| 1);
    let id = ai.rows[0].squad as usize;
    assert_eq!(ai.squads.leader[id], 0);
    ai.release(0);
    assert_eq!(ai.squads.leader[id], 1);
    ai.release(1);
    ai.release(2);
    assert_eq!(ai.squads.leader[id], NONE);
}

#[test]
fn occupy_slot_rules() {
    let mut ai: Ai<4> = Ai::new();
    for p in 0..3u8 {
        ai.claim(p, 8, 0);
    }
    ai.recruit(0, 4, 1_000_000, |_, _| 1);
    assert!(ai.occupy(0, SLOTS_ENGAGE));
    assert!(ai.occupy(1, SLOTS_ENGAGE));
    assert!(!ai.occupy(2, SLOTS_ENGAGE), "two engage slots only");
    ai.leave_squad(0);
    assert!(ai.occupy(2, SLOTS_ENGAGE), "a vacated slot is free again");
}

fn shipped() -> (Tables, Vec<String>) {
    let mut s = Source::default();
    s.add(include_str!("../../hl-sched/sched/base.sched")).expect("base");
    s.add(include_str!("../../hl-sched/sched/grunt.sched")).expect("grunt");
    let names = s.names(&["grunt"]).expect("names");
    let blob: &'static [u8] = Box::leak(s.encode(&["grunt"]).expect("encode").into_boxed_slice());
    (Tables::new(blob).expect("valid"), names)
}

/// The retail trace of a trio meeting the player at range: leader establishes a line of fire, members take cover.
#[test]
fn shipped_grunt_squad_reacts_like_the_retail_trace() {
    let (t, names) = shipped();
    let mut ai: Ai<8> = Ai::new();
    for p in 0..3u8 {
        ai.claim(p, 8, F_LAUNCHER).expect("row");
    }
    ai.recruit(1, 4, 1_000_000, |_, _| 10); // row 1 recruits: it is the leader, like edict 97 in the trace
    let id = ai.rows[0].squad;
    assert_eq!(ai.squads.leader[id as usize], 1);
    struct Eye { sees: bool }
    let mut w = Mock { now: 0, sees: false, shots: 0, seq_left: [0; 8], moving: [0; 8], log: vec![] };
    let _ = Eye { sees: false };
    for tick in 0..8 {
        w.now = tick;
        for i in 0..3 { ai.think(&mut w, &t, i, true); }
    }
    let name = |ai: &Ai<8>, i: usize| names[ai.rows[i].sched as usize].clone();
    assert!(ai.rows.iter().take(3).all(|r| r.state == state::IDLE));
    assert_eq!(name(&ai, 0), "IdleStand");
    // Enemy seen at range with no clear shot yet: SEE_ENEMY only.
    w.sees = true;
    struct NoShot;
    let _ = NoShot;
    for tick in 8..10 {
        w.now = tick;
        for i in 0..3 { ai.think(&mut w, &t, i, true); }
    }
    assert!(ai.rows.iter().take(3).all(|r| r.state == state::COMBAT), "{:?}", (0..3).map(|i| ai.rows[i].state).collect::<Vec<_>>());
    // The mock reports CAN_RANGE_ATTACK1 too, so the leader suppresses and takes an engage slot is not required for Suppress.
    let leader = name(&ai, 1);
    assert!(leader == "Suppress" || leader == "SignalSuppress", "leader ran {leader}");
    for i in [0usize, 2] {
        let n = name(&ai, i);
        assert!(n == "TakeCover" || n == "GruntWaitInCover", "member {i} ran {n}");
    }
}

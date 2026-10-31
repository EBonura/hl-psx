// SPDX-License-Identifier: GPL-2.0-or-later
//! Monster schedule tables: the `.sched` text sources, their parser and the encoder that writes the
//! per-map `AIT1` section (`hl_format::ai`). The sources are written for this port from observed
//! behaviour (the retail oracle prints each monster's schedule name, task and conditions every tick),
//! so schedule names are the retail names and a trace maps onto table ids without a side table.
//!
//! Grammar (one statement per line, `#` starts a comment, times are seconds and become 20 Hz ticks):
//!
//! ```text
//! species NAME id=N [parent=NAME] [flags=A|B] [r1=MIN..MAX] [cone=F] [r2=MIN..MAX] [melee=N]
//! sched "Name" species=NAME [interrupt=COND|COND] [sound=SND|SND]
//!   OP [arg]
//! end
//! type SPECIES TYPE [when PRED[,PRED]] -> "Schedule name"
//! pick SPECIES states=STATE|STATE [any=COND|..] [all=COND|..] [none=COND|..] [when PRED[,PRED]] [fx=FX] -> TYPE
//! ```

use hl_format::ai::{self, cond, fx, layout, mem, op, pred, snd, state, ty};
use std::collections::BTreeMap;

/// A parse or link error with the 1-based source line.
#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub msg: String,
}

fn err<T>(line: usize, msg: impl Into<String>) -> Result<T, Error> {
    Err(Error { line, msg: msg.into() })
}

#[derive(Debug, Clone)]
struct SpeciesDef {
    id: u8,
    parent: Option<String>,
    flags: u8,
    r1: (u16, u16),
    cone_q12: i16,
    r2: (u16, u16),
    melee: u16,
}

#[derive(Debug, Clone)]
struct SchedDef {
    name: String,
    species: String,
    interrupt: u32,
    sound: u16,
    tasks: Vec<(u8, i16)>,
}

#[derive(Debug, Clone)]
struct TypeDef {
    species: String,
    ty: u8,
    pred: u8,
    sched: String,
    line: usize,
}

#[derive(Debug, Clone)]
struct PickDef {
    species: String,
    states: u16,
    any: u32,
    none: u32,
    all: u32,
    pa: u8,
    pb: u8,
    fx: u8,
    ty: u8,
}

/// All sources, parsed and name-checked, ready to encode a species subset.
#[derive(Default, Debug, Clone)]
pub struct Source {
    species: BTreeMap<String, SpeciesDef>,
    scheds: Vec<SchedDef>,
    types: Vec<TypeDef>,
    picks: Vec<PickDef>,
    /// Schedule type names declared by species sources (global ones are in `hl_format::ai::ty`).
    type_names: BTreeMap<String, u8>,
}

fn lookup(table: &[(&str, u32)], name: &str) -> Option<u32> {
    table.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

const CONDS: &[(&str, u32)] = &[
    ("NO_AMMO_LOADED", cond::NO_AMMO_LOADED),
    ("SEE_HATE", cond::SEE_HATE),
    ("SEE_FEAR", cond::SEE_FEAR),
    ("SEE_DISLIKE", cond::SEE_DISLIKE),
    ("SEE_ENEMY", cond::SEE_ENEMY),
    ("ENEMY_OCCLUDED", cond::ENEMY_OCCLUDED),
    ("SMELL_FOOD", cond::SMELL_FOOD),
    ("ENEMY_TOOFAR", cond::ENEMY_TOOFAR),
    ("LIGHT_DAMAGE", cond::LIGHT_DAMAGE),
    ("HEAVY_DAMAGE", cond::HEAVY_DAMAGE),
    ("CAN_RANGE_ATTACK1", cond::CAN_RANGE_ATTACK1),
    ("CAN_MELEE_ATTACK1", cond::CAN_MELEE_ATTACK1),
    ("CAN_RANGE_ATTACK2", cond::CAN_RANGE_ATTACK2),
    ("CAN_MELEE_ATTACK2", cond::CAN_MELEE_ATTACK2),
    ("PROVOKED", cond::PROVOKED),
    ("NEW_ENEMY", cond::NEW_ENEMY),
    ("HEAR_SOUND", cond::HEAR_SOUND),
    ("SMELL", cond::SMELL),
    ("ENEMY_FACING_ME", cond::ENEMY_FACING_ME),
    ("ENEMY_DEAD", cond::ENEMY_DEAD),
    ("SEE_CLIENT", cond::SEE_CLIENT),
    ("SEE_NEMESIS", cond::SEE_NEMESIS),
    ("SPECIAL1", cond::SPECIAL1),
    ("SPECIAL2", cond::SPECIAL2),
    ("TASK_FAILED", cond::TASK_FAILED),
    ("SCHEDULE_DONE", cond::SCHEDULE_DONE),
];

const SOUNDS: &[(&str, u32)] = &[
    ("COMBAT", snd::COMBAT as u32),
    ("WORLD", snd::WORLD as u32),
    ("PLAYER", snd::PLAYER as u32),
    ("CARCASS", snd::CARCASS as u32),
    ("MEAT", snd::MEAT as u32),
    ("DANGER", snd::DANGER as u32),
    ("GARBAGE", snd::GARBAGE as u32),
];

const MEMS: &[(&str, u32)] = &[
    ("PROVOKED", mem::PROVOKED as u32),
    ("IN_COVER", mem::IN_COVER as u32),
    ("SUSPICIOUS", mem::SUSPICIOUS as u32),
    ("PATH_FINISHED", mem::PATH_FINISHED as u32),
    ("ON_PATH", mem::ON_PATH as u32),
    ("MOVE_FAILED", mem::MOVE_FAILED as u32),
    ("FLINCHED", mem::FLINCHED as u32),
    ("KILLED", mem::KILLED as u32),
];

const STATES: &[(&str, u32)] = &[
    ("NONE", state::NONE as u32),
    ("IDLE", state::IDLE as u32),
    ("COMBAT", state::COMBAT as u32),
    ("ALERT", state::ALERT as u32),
    ("HUNT", state::HUNT as u32),
    ("PRONE", state::PRONE as u32),
    ("SCRIPT", state::SCRIPT as u32),
    ("PLAYDEAD", state::PLAYDEAD as u32),
    ("DEAD", state::DEAD as u32),
];

/// Retail activity numbers (`ACT_*`), the ones the schedules name.
const ACTS: &[(&str, u32)] = &[
    ("RESET", 0), ("IDLE", 1), ("GUARD", 2), ("WALK", 3), ("RUN", 4), ("FLY", 5), ("SWIM", 6),
    ("HOP", 7), ("LEAP", 8), ("FALL", 9), ("LAND", 10), ("STRAFE_LEFT", 11), ("STRAFE_RIGHT", 12),
    ("ROLL_LEFT", 13), ("ROLL_RIGHT", 14), ("TURN_LEFT", 15), ("TURN_RIGHT", 16), ("CROUCH", 17),
    ("CROUCHIDLE", 18), ("STAND", 19), ("USE", 20), ("SIGNAL1", 21), ("SIGNAL2", 22), ("SIGNAL3", 23),
    ("TWITCH", 24), ("COWER", 25), ("SMALL_FLINCH", 26), ("BIG_FLINCH", 27), ("RANGE_ATTACK1", 28),
    ("RANGE_ATTACK2", 29), ("MELEE_ATTACK1", 30), ("MELEE_ATTACK2", 31), ("RELOAD", 32), ("ARM", 33),
    ("DISARM", 34), ("EAT", 35), ("DIESIMPLE", 36), ("DIEBACKWARD", 37), ("DIEFORWARD", 38),
    ("DIEVIOLENT", 39), ("SLEEP", 44), ("INSPECT_FLOOR", 45), ("INSPECT_WALL", 46), ("IDLE_ANGRY", 47),
    ("WALK_HURT", 48), ("RUN_HURT", 49), ("HOVER", 50), ("GLIDE", 51), ("BITE", 56),
    ("THREAT_DISPLAY", 57), ("SPECIAL_ATTACK1", 60), ("SPECIAL_ATTACK2", 61), ("COMBAT_IDLE", 62),
    ("WALK_SCARED", 63), ("RUN_SCARED", 64), ("VICTORY_DANCE", 65),
];

const OPS: &[(&str, u8)] = &[
    ("WAIT", op::WAIT), ("WAIT_FACE_ENEMY", op::WAIT_FACE_ENEMY), ("WAIT_PVS", op::WAIT_PVS),
    ("WAIT_RANDOM", op::WAIT_RANDOM), ("WAIT_INDEFINITE", op::WAIT_INDEFINITE),
    ("SUGGEST_STATE", op::SUGGEST_STATE), ("GET_PATH_TO_ENEMY", op::GET_PATH_TO_ENEMY),
    ("GET_PATH_TO_ENEMY_LKP", op::GET_PATH_TO_ENEMY_LKP),
    ("GET_PATH_TO_ENEMY_CORPSE", op::GET_PATH_TO_ENEMY_CORPSE),
    ("GET_PATH_TO_LEADER", op::GET_PATH_TO_LEADER), ("GET_PATH_TO_TARGET", op::GET_PATH_TO_TARGET),
    ("GET_PATH_TO_LASTPOSITION", op::GET_PATH_TO_LASTPOSITION),
    ("GET_PATH_TO_BESTSOUND", op::GET_PATH_TO_BESTSOUND), ("RUN_PATH", op::RUN_PATH),
    ("WALK_PATH", op::WALK_PATH), ("WAIT_FOR_MOVEMENT", op::WAIT_FOR_MOVEMENT),
    ("CLEAR_MOVE_WAIT", op::CLEAR_MOVE_WAIT), ("STORE_LASTPOSITION", op::STORE_LASTPOSITION),
    ("CLEAR_LASTPOSITION", op::CLEAR_LASTPOSITION), ("STOP_MOVING", op::STOP_MOVING),
    ("FACE_IDEAL", op::FACE_IDEAL), ("FACE_ENEMY", op::FACE_ENEMY), ("FACE_TARGET", op::FACE_TARGET),
    ("FACE_ROUTE", op::FACE_ROUTE), ("FACE_LASTPOSITION", op::FACE_LASTPOSITION),
    ("TURN_LEFT", op::TURN_LEFT), ("TURN_RIGHT", op::TURN_RIGHT), ("RANGE_ATTACK1", op::RANGE_ATTACK1),
    ("RANGE_ATTACK2", op::RANGE_ATTACK2), ("MELEE_ATTACK1", op::MELEE_ATTACK1),
    ("MELEE_ATTACK2", op::MELEE_ATTACK2), ("SPECIAL_ATTACK1", op::SPECIAL_ATTACK1),
    ("SPECIAL_ATTACK2", op::SPECIAL_ATTACK2), ("RELOAD", op::RELOAD),
    ("SMALL_FLINCH", op::SMALL_FLINCH), ("SET_ACTIVITY", op::SET_ACTIVITY),
    ("PLAY_SEQUENCE", op::PLAY_SEQUENCE), ("PLAY_SEQUENCE_FACE_ENEMY", op::PLAY_SEQUENCE_FACE_ENEMY),
    ("SET_SCHEDULE", op::SET_SCHEDULE), ("SET_FAIL_SCHEDULE", op::SET_FAIL_SCHEDULE),
    ("CLEAR_FAIL_SCHEDULE", op::CLEAR_FAIL_SCHEDULE), ("REMEMBER", op::REMEMBER), ("FORGET", op::FORGET),
    ("FIND_COVER_FROM_ENEMY", op::FIND_COVER_FROM_ENEMY),
    ("FIND_COVER_FROM_BEST_SOUND", op::FIND_COVER_FROM_BEST_SOUND),
    ("FIND_COVER_FROM_ORIGIN", op::FIND_COVER_FROM_ORIGIN),
    ("FIND_FAR_NODE_COVER_FROM_ENEMY", op::FIND_FAR_NODE_COVER_FROM_ENEMY),
    ("FIND_NEAR_NODE_COVER_FROM_ENEMY", op::FIND_NEAR_NODE_COVER_FROM_ENEMY), ("SOUND", op::SOUND),
    ("DIE", op::DIE), ("WAIT_FOR_SCRIPT", op::WAIT_FOR_SCRIPT), ("PLAY_SCRIPT", op::PLAY_SCRIPT),
    ("CUSTOM", op::CUSTOM), ("FAIL", op::FAIL),
];

const PREDS: &[(&str, u32)] = &[
    ("IN_SQUAD", pred::IN_SQUAD as u32), ("NOT_IN_SQUAD", pred::NOT_IN_SQUAD as u32),
    ("IS_LEADER", pred::IS_LEADER as u32), ("NOT_LEADER", pred::NOT_LEADER as u32),
    ("SKILL_HARD", pred::SKILL_HARD as u32), ("RANDOM_HALF", pred::RANDOM_HALF as u32),
    ("RANDOM_TENTH", pred::RANDOM_TENTH as u32), ("OCCUPY_ENGAGE", pred::OCCUPY_ENGAGE as u32),
    ("OCCUPY_GRENADE", pred::OCCUPY_GRENADE as u32), ("OCCUPY_ATTACK", pred::OCCUPY_ATTACK as u32),
    ("HAS_ENEMY", pred::HAS_ENEMY as u32), ("FLAG_A", pred::FLAG_A as u32), ("FLAG_B", pred::FLAG_B as u32),
    ("SQUAD_ENEMY_ELUDED", pred::SQUAD_ENEMY_ELUDED as u32), ("STANDING", pred::STANDING as u32),
    ("FIRST_ENCOUNTER", pred::FIRST_ENCOUNTER as u32), ("ENEMY_IS_PLAYER", pred::ENEMY_IS_PLAYER as u32),
    ("LIGHT_DAMAGE_90", pred::LIGHT_DAMAGE_90 as u32), ("MOVETYPE_FLY", pred::MOVETYPE_FLY as u32),
    ("DAMAGE_AHEAD", pred::DAMAGE_AHEAD as u32), ("NOT_FLINCHED", pred::NOT_FLINCHED as u32),
    ("DANGER_SOUND", pred::DANGER_SOUND as u32), ("HARD_GRENADE_COVER", pred::HARD_GRENADE_COVER as u32),
    ("CAN_RANGE1_ENGAGE", pred::CAN_RANGE1_ENGAGE as u32), ("ROLL_STANDING", pred::ROLL_STANDING as u32),
    ("FIRST_ENCOUNTER_TAKE", pred::FIRST_ENCOUNTER_TAKE as u32),
];

const CUSTOMS: &[(&str, u32)] = &[
    ("GRUNT_FACE_TOSS_DIR", ai::custom::GRUNT_FACE_TOSS_DIR as u32),
    ("GRUNT_SPEAK", ai::custom::GRUNT_SPEAK as u32),
    ("GRUNT_CHECK_FIRE", ai::custom::GRUNT_CHECK_FIRE as u32),
];

const FXS: &[(&str, u32)] = &[
    ("CLEAR_ELUDED", fx::CLEAR_ELUDED as u32),
    ("CLEAR_FIRST_ENCOUNTER", fx::CLEAR_FIRST_ENCOUNTER as u32),
];

const TYPES: &[(&str, u32)] = &[
    ("IDLE_STAND", ty::IDLE_STAND as u32), ("IDLE_WALK", ty::IDLE_WALK as u32),
    ("WAKE_ANGRY", ty::WAKE_ANGRY as u32), ("ALERT_FACE", ty::ALERT_FACE as u32),
    ("ALERT_SMALL_FLINCH", ty::ALERT_SMALL_FLINCH as u32), ("ALERT_BIG_FLINCH", ty::ALERT_BIG_FLINCH as u32),
    ("ALERT_STAND", ty::ALERT_STAND as u32), ("INVESTIGATE_SOUND", ty::INVESTIGATE_SOUND as u32),
    ("COMBAT_FACE", ty::COMBAT_FACE as u32), ("COMBAT_STAND", ty::COMBAT_STAND as u32),
    ("CHASE_ENEMY", ty::CHASE_ENEMY as u32), ("CHASE_ENEMY_FAILED", ty::CHASE_ENEMY_FAILED as u32),
    ("VICTORY_DANCE", ty::VICTORY_DANCE as u32), ("SMALL_FLINCH", ty::SMALL_FLINCH as u32),
    ("TAKE_COVER_FROM_ENEMY", ty::TAKE_COVER_FROM_ENEMY as u32),
    ("TAKE_COVER_FROM_BEST_SOUND", ty::TAKE_COVER_FROM_BEST_SOUND as u32),
    ("TAKE_COVER_FROM_ORIGIN", ty::TAKE_COVER_FROM_ORIGIN as u32), ("COWER", ty::COWER as u32),
    ("MELEE_ATTACK1", ty::MELEE_ATTACK1 as u32), ("MELEE_ATTACK2", ty::MELEE_ATTACK2 as u32),
    ("RANGE_ATTACK1", ty::RANGE_ATTACK1 as u32), ("RANGE_ATTACK2", ty::RANGE_ATTACK2 as u32),
    ("SPECIAL_ATTACK1", ty::SPECIAL_ATTACK1 as u32), ("SPECIAL_ATTACK2", ty::SPECIAL_ATTACK2 as u32),
    ("STANDOFF", ty::STANDOFF as u32), ("ARM_WEAPON", ty::ARM_WEAPON as u32), ("RELOAD", ty::RELOAD as u32),
    ("DIE", ty::DIE as u32), ("WAIT_TRIGGER", ty::WAIT_TRIGGER as u32), ("FOLLOW", ty::FOLLOW as u32),
    ("AISCRIPT", ty::AISCRIPT as u32), ("FAIL", ty::FAIL as u32),
];

fn mask(table: &[(&str, u32)], text: &str, line: usize, what: &str) -> Result<u32, Error> {
    if text == "-" || text.is_empty() {
        return Ok(0);
    }
    let mut v = 0;
    for part in text.split('|') {
        match lookup(table, part) {
            Some(b) => v |= b,
            None => return err(line, format!("unknown {what} {part}")),
        }
    }
    Ok(v)
}

fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn kv<'a>(toks: &'a [String], key: &str) -> Option<&'a str> {
    toks.iter().find_map(|t| t.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn ticks(seconds: &str, line: usize) -> Result<i16, Error> {
    match seconds.parse::<f64>() {
        Ok(s) if (0.0..=1600.0).contains(&s) => Ok((s * 20.0).round() as i16),
        _ => err(line, format!("bad time {seconds}")),
    }
}

impl Source {
    /// Parse one source text and merge it in.
    pub fn add(&mut self, text: &str) -> Result<(), Error> {
        let mut open: Option<SchedDef> = None;
        for (i, raw) in text.lines().enumerate() {
            let n = i + 1;
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let t = tokens(line);
            if let Some(s) = open.as_mut() {
                if t[0] == "end" {
                    self.scheds.push(open.take().expect("open schedule"));
                    continue;
                }
                let task = self.task(&t, &s.species.clone(), n)?;
                s.tasks.push(task);
                continue;
            }
            match t[0].as_str() {
                "species" => self.species_line(&t, n)?,
                "sched" => {
                    if t.len() < 3 {
                        return err(n, "sched needs a name and species=");
                    }
                    let species = kv(&t, "species").ok_or(Error { line: n, msg: "sched needs species=".into() })?;
                    open = Some(SchedDef {
                        name: t[1].clone(),
                        species: species.to_string(),
                        interrupt: mask(CONDS, kv(&t, "interrupt").unwrap_or("-"), n, "condition")?,
                        sound: mask(SOUNDS, kv(&t, "sound").unwrap_or("-"), n, "sound")? as u16,
                        tasks: Vec::new(),
                    });
                }
                "type" => self.type_line(&t, n)?,
                "pick" => self.pick_line(&t, n)?,
                other => return err(n, format!("unknown statement {other}")),
            }
        }
        if open.is_some() {
            return err(text.lines().count(), "schedule not closed with end");
        }
        Ok(())
    }

    fn species_line(&mut self, t: &[String], n: usize) -> Result<(), Error> {
        let name = t.get(1).ok_or(Error { line: n, msg: "species needs a name".into() })?.clone();
        let id: u8 = kv(t, "id").and_then(|v| v.parse().ok()).ok_or(Error { line: n, msg: "species needs id=".into() })?;
        let range = |key: &str| -> Result<(u16, u16), Error> {
            match kv(t, key) {
                None => Ok((0, 0)),
                Some(v) => {
                    let (a, b) = v.split_once("..").ok_or(Error { line: n, msg: format!("{key} wants MIN..MAX") })?;
                    match (a.parse(), b.parse()) {
                        (Ok(a), Ok(b)) => Ok((a, b)),
                        _ => err(n, format!("bad {key}")),
                    }
                }
            }
        };
        let flags = match kv(t, "flags") {
            None => 0,
            Some(f) => f.split('|').fold(0u8, |acc, p| match p {
                "A" => acc | 1,
                "B" => acc | 2,
                _ => acc,
            }),
        };
        let cone = kv(t, "cone").map(|c| c.parse::<f64>().unwrap_or(0.0)).unwrap_or(0.0);
        let def = SpeciesDef {
            id,
            parent: kv(t, "parent").map(str::to_string),
            flags,
            r1: range("r1")?,
            cone_q12: (cone * 4096.0).round() as i16,
            r2: range("r2")?,
            melee: kv(t, "melee").and_then(|v| v.parse().ok()).unwrap_or(0),
        };
        if self.species.values().any(|s| s.id == id) {
            return err(n, format!("species id {id} already used"));
        }
        self.species.insert(name, def);
        Ok(())
    }

    fn type_id(&mut self, species: &str, name: &str) -> u8 {
        if let Some(v) = lookup(TYPES, name) {
            return v as u8;
        }
        let key = format!("{species}:{name}");
        let next = ty::FIRST_SPECIES_TYPE + self.type_names.len() as u8;
        *self.type_names.entry(key).or_insert(next)
    }

    fn task(&mut self, t: &[String], species: &str, n: usize) -> Result<(u8, i16), Error> {
        let opcode = OPS.iter().find(|(name, _)| *name == t[0]).map(|(_, v)| *v);
        let Some(opcode) = opcode else { return err(n, format!("unknown task {}", t[0])) };
        let a = t.get(1).map(String::as_str).unwrap_or("0");
        let arg = match opcode {
            op::WAIT | op::WAIT_FACE_ENEMY | op::WAIT_RANDOM => ticks(a, n)?,
            op::SET_ACTIVITY | op::PLAY_SEQUENCE | op::PLAY_SEQUENCE_FACE_ENEMY => match lookup(ACTS, a) {
                Some(v) => v as i16,
                None => return err(n, format!("unknown activity {a}")),
            },
            op::SET_SCHEDULE | op::SET_FAIL_SCHEDULE => self.type_id(species, a) as i16,
            op::SUGGEST_STATE => match lookup(STATES, a) {
                Some(v) => v as i16,
                None => return err(n, format!("unknown state {a}")),
            },
            op::REMEMBER | op::FORGET => mask(MEMS, a, n, "memory bit")? as i16,
            op::CUSTOM => match lookup(CUSTOMS, a) {
                Some(v) => v as i16,
                None => return err(n, format!("unknown species task {a}")),
            },
            _ => a.parse::<f64>().map(|v| v as i16).unwrap_or(0),
        };
        Ok((opcode, arg))
    }

    fn preds(t: &[String], n: usize) -> Result<(u8, u8), Error> {
        let Some(w) = t.iter().position(|x| x == "when") else { return Ok((0, 0)) };
        let list = t.get(w + 1).ok_or(Error { line: n, msg: "when needs predicates".into() })?;
        let mut out = [0u8; 2];
        for (k, p) in list.split(',').enumerate() {
            if k >= 2 {
                return err(n, "at most two predicates per row");
            }
            out[k] = lookup(PREDS, p).ok_or(Error { line: n, msg: format!("unknown predicate {p}") })? as u8;
        }
        Ok((out[0], out[1]))
    }

    fn type_line(&mut self, t: &[String], n: usize) -> Result<(), Error> {
        let arrow = t.iter().position(|x| x == "->").ok_or(Error { line: n, msg: "type row needs ->".into() })?;
        if t.len() < 5 || arrow != t.len() - 2 {
            return err(n, "type SPECIES TYPE [when P] -> \"Schedule\"");
        }
        let (pa, pb) = Self::preds(&t[..arrow], n)?;
        if pb != 0 {
            return err(n, "type rows take one predicate");
        }
        let species = t[1].clone();
        let ty = self.type_id(&species, &t[2]);
        self.types.push(TypeDef { species, ty, pred: pa, sched: t[arrow + 1].clone(), line: n });
        Ok(())
    }

    fn pick_line(&mut self, t: &[String], n: usize) -> Result<(), Error> {
        let arrow = t.iter().position(|x| x == "->").ok_or(Error { line: n, msg: "pick row needs ->".into() })?;
        if arrow != t.len() - 2 {
            return err(n, "pick SPECIES states=.. [any=..] [none=..] [when ..] [fx=..] -> TYPE");
        }
        let head = &t[..arrow];
        let (pa, pb) = Self::preds(head, n)?;
        let mut states = 0u16;
        for part in kv(head, "states").unwrap_or("-").split('|').filter(|p| *p != "-") {
            match lookup(STATES, part) {
                Some(v) => states |= 1 << v,
                None => return err(n, format!("unknown state {part}")),
            }
        }
        let fxv = match kv(head, "fx") {
            None => 0,
            Some(f) => lookup(FXS, f).ok_or(Error { line: n, msg: format!("unknown effect {f}") })? as u8,
        };
        let species = t[1].clone();
        let tyid = self.type_id(&species, &t[arrow + 1]);
        self.picks.push(PickDef {
            species,
            states,
            any: mask(CONDS, kv(head, "any").unwrap_or("-"), n, "condition")?,
            none: mask(CONDS, kv(head, "none").unwrap_or("-"), n, "condition")?,
            all: mask(CONDS, kv(head, "all").unwrap_or("-"), n, "condition")?,
            pa,
            pb,
            fx: fxv,
            ty: tyid,
        });
        Ok(())
    }

    /// Species ids by name, for the cooker.
    pub fn species_id(&self, name: &str) -> Option<u8> {
        self.species.get(name).map(|s| s.id)
    }

    /// Schedule names in encoding order for `species` (index = schedule id in the blob), for trace tools.
    pub fn names(&self, species: &[&str]) -> Result<Vec<String>, Error> {
        Ok(self.encode_inner(species)?.1)
    }

    /// Encode the tables of `species` and every parent they name. Schedule 0 is an empty placeholder.
    pub fn encode(&self, species: &[&str]) -> Result<Vec<u8>, Error> {
        Ok(self.encode_inner(species)?.0)
    }

    fn encode_inner(&self, wanted: &[&str]) -> Result<(Vec<u8>, Vec<String>), Error> {
        let mut set: Vec<String> = Vec::new();
        for w in wanted {
            let mut cur = Some(w.to_string());
            while let Some(c) = cur {
                if !self.species.contains_key(&c) {
                    return err(0, format!("unknown species {c}"));
                }
                if !set.contains(&c) {
                    set.push(c.clone());
                }
                cur = self.species[&c].parent.clone();
            }
        }
        set.sort();
        let mut sched_ids: BTreeMap<(String, String), u8> = BTreeMap::new();
        let mut names = vec![String::new()];
        let mut sched_bytes = vec![0u8; layout::SCHED_BYTES];
        let mut task_bytes = Vec::new();
        let mut ntask = 0u16;
        for s in self.scheds.iter().filter(|s| set.contains(&s.species)) {
            if names.len() >= 255 {
                return err(0, "more than 254 schedules in one map set");
            }
            sched_ids.insert((s.species.clone(), s.name.clone()), names.len() as u8);
            names.push(s.name.clone());
            sched_bytes.extend_from_slice(&s.interrupt.to_le_bytes());
            sched_bytes.extend_from_slice(&s.sound.to_le_bytes());
            sched_bytes.extend_from_slice(&ntask.to_le_bytes());
            sched_bytes.push(s.tasks.len() as u8);
            sched_bytes.push(0);
            for &(o, a) in &s.tasks {
                task_bytes.push(o);
                task_bytes.extend_from_slice(&a.to_le_bytes());
            }
            ntask += s.tasks.len() as u16;
        }
        let mut pick_bytes = Vec::new();
        let mut npick = 0u16;
        for p in self.picks.iter().filter(|p| set.contains(&p.species)) {
            pick_bytes.extend_from_slice(&[self.species[&p.species].id, 0, p.pa, p.pb, p.ty, p.fx]);
            pick_bytes.extend_from_slice(&p.states.to_le_bytes());
            pick_bytes.extend_from_slice(&p.any.to_le_bytes());
            pick_bytes.extend_from_slice(&p.none.to_le_bytes());
            pick_bytes.extend_from_slice(&p.all.to_le_bytes());
            npick += 1;
        }
        let mut type_bytes = Vec::new();
        let mut ntype = 0u16;
        for r in self.types.iter().filter(|r| set.contains(&r.species)) {
            if let Some(target) = r.sched.strip_prefix('@') {
                let tid = lookup(TYPES, target).map(|v| v as u8).or_else(|| {
                    self.type_names.iter().find(|(k, _)| k.ends_with(&format!(":{target}"))).map(|(_, v)| *v)
                });
                let tid = tid.ok_or(Error { line: r.line, msg: format!("unknown type {target}") })?;
                type_bytes.extend_from_slice(&[self.species[&r.species].id, r.ty, r.pred, ai::TYPE_REDIRECT | tid]);
                ntype += 1;
                continue;
            }
            let id = sched_ids
                .get(&(r.species.clone(), r.sched.clone()))
                .or_else(|| {
                    // a species may name a schedule its parent defines
                    let mut cur = self.species[&r.species].parent.clone();
                    let mut found = None;
                    while let Some(c) = cur {
                        if let Some(v) = sched_ids.get(&(c.clone(), r.sched.clone())) {
                            found = Some(v);
                            break;
                        }
                        cur = self.species[&c].parent.clone();
                    }
                    found
                })
                .ok_or(Error { line: r.line, msg: format!("unknown schedule \"{}\"", r.sched) })?;
            type_bytes.extend_from_slice(&[self.species[&r.species].id, r.ty, r.pred, *id]);
            ntype += 1;
        }
        let mut species_bytes = Vec::new();
        for name in &set {
            let s = &self.species[name];
            let parent = s.parent.as_ref().map(|p| self.species[p].id).unwrap_or(s.id);
            species_bytes.extend_from_slice(&[s.id, parent, s.flags, 0]);
            for v in [s.r1.0, s.r1.1, s.cone_q12 as u16, s.r2.0, s.r2.1, s.melee] {
                species_bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        let nsched = names.len() as u16;
        let nspecies = set.len() as u16;
        let mut out = vec![0u8; layout::HEADER_BYTES];
        let put = |out: &mut Vec<u8>, slot: usize, body: &[u8], count: u16| {
            let off = out.len() as u16;
            out[slot..slot + 2].copy_from_slice(&off.to_le_bytes());
            out[slot + 2..slot + 4].copy_from_slice(&count.to_le_bytes());
            out.extend_from_slice(body);
        };
        put(&mut out, 0, &sched_bytes, nsched);
        put(&mut out, 4, &task_bytes, ntask);
        put(&mut out, 8, &pick_bytes, npick);
        put(&mut out, 12, &type_bytes, ntype);
        put(&mut out, 16, &species_bytes, nspecies);
        Ok((out, names))
    }
}

/// Species name for a model type id (`combat_defs`/model roster numbering), for the species the sources define.
pub fn species_for_kind(kind: u16) -> Option<&'static str> {
    match kind {
        8 => Some("grunt"),
        _ => None,
    }
}

/// The shipped sources (`sched/*.sched`), parsed.
pub fn shipped() -> Result<Source, Error> {
    let mut s = Source::default();
    s.add(include_str!("../sched/base.sched"))?;
    s.add(include_str!("../sched/grunt.sched"))?;
    Ok(s)
}

/// The room trailer for a map that places monsters of these model type ids, or `None` when none of them
/// run on the schedule runtime. Also returns the species names it carries.
pub fn section_for_kinds(kinds: &[u16]) -> Result<Option<(Vec<u8>, Vec<&'static str>)>, Error> {
    let mut names: Vec<&'static str> = kinds.iter().filter_map(|k| species_for_kind(*k)).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Ok(None);
    }
    let blob = shipped()?.encode(&names)?;
    Ok(Some((trailer(&blob), names)))
}

/// Wrap a table blob as the room trailer (`tables | u32 bytes | "AIT1"`), 4-aligned.
pub fn trailer(tables: &[u8]) -> Vec<u8> {
    let mut t = tables.to_vec();
    while t.len() % 4 != 0 {
        t.push(0);
    }
    let total = (t.len() + ai::TRAILER_BYTES) as u32;
    t.extend_from_slice(&total.to_le_bytes());
    t.extend_from_slice(&ai::SECTION_TAG);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
species base id=250
species grunt id=8 parent=base flags=A r1=0..2048 cone=0.5 r2=256..1024 melee=80
sched "Idle Stand" species=base interrupt=NEW_ENEMY|HEAR_SOUND sound=COMBAT|DANGER
  SET_ACTIVITY IDLE
  WAIT 1.5
  WAIT_RANDOM 3
end
sched "Grunt Fail" species=grunt
  STOP_MOVING
  SET_FAIL_SCHEDULE COMBAT_FACE
  CUSTOM GRUNT_CHECK_FIRE
  REMEMBER IN_COVER
end
type base IDLE_STAND -> "Idle Stand"
type grunt FAIL when IN_SQUAD -> "Grunt Fail"
pick grunt states=COMBAT any=NEW_ENEMY none=ENEMY_DEAD when IN_SQUAD,NOT_LEADER fx=CLEAR_ELUDED -> TAKE_COVER_FROM_ENEMY
"#;

    fn encoded() -> Vec<u8> {
        let mut s = Source::default();
        s.add(SRC).expect("parse");
        s.encode(&["grunt"]).expect("encode")
    }

    #[test]
    fn encodes_and_validates() {
        let blob: &'static [u8] = Box::leak(encoded().into_boxed_slice());
        let t = ai::Tables::new(blob).expect("valid");
        assert_eq!(t.n_sched(), 3);
        assert_eq!(t.n_species(), 2);
        let idle = t.sched(1);
        assert_eq!(idle.interrupt, cond::NEW_ENEMY | cond::HEAR_SOUND);
        assert_eq!(idle.sound, snd::COMBAT | snd::DANGER);
        assert_eq!(t.task(&idle, 1), (op::WAIT, 30));
        let p = t.pick(0);
        assert_eq!((p.species, p.pred_a, p.pred_b, p.effect), (8, pred::IN_SQUAD, pred::NOT_LEADER, fx::CLEAR_ELUDED));
        assert_eq!(p.states, 1 << state::COMBAT);
        let g = t.species(8).expect("grunt");
        assert_eq!((g.parent, g.r1_max, g.cone_q12), (250, 2048, 2048));
    }

    #[test]
    fn trailer_round_trips() {
        let blob = encoded();
        let mut room = vec![7u8; 100];
        room.extend(trailer(&blob));
        let room: &'static [u8] = Box::leak(room.into_boxed_slice());
        let found = ai::section(room).expect("section");
        assert_eq!(&found[..blob.len()], &blob[..]);
        assert_eq!(ai::room_end(room), 100);
    }

    #[test]
    fn shipped_sources_encode_and_validate() {
        let mut s = Source::default();
        s.add(include_str!("../sched/base.sched")).expect("base");
        s.add(include_str!("../sched/grunt.sched")).expect("grunt");
        let blob = s.encode(&["grunt"]).expect("encode");
        let names = s.names(&["grunt"]).expect("names");
        assert!(names.iter().any(|n| n == "Range Attack1A"));
        assert!(names.iter().any(|n| n == "GruntEstablishLineOfFire"));
        let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
        let t = ai::Tables::new(blob).expect("valid");
        eprintln!("grunt set: {} schedules, {} tasks, {} picks, {} types, {} bytes", t.n_sched(), t.n_task(), t.n_pick(), t.n_type(), blob.len());
    }

    #[test]
    fn rejects_unknown_names() {
        let mut s = Source::default();
        assert!(s.add("sched \"X\" species=a\n  NOPE 1\nend\n").is_err());
        let mut s = Source::default();
        s.add("species a id=1\ntype a IDLE_STAND -> \"Missing\"\n").expect("parse");
        assert!(s.encode(&["a"]).is_err());
    }
}

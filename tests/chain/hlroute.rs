//! Route compiler for the Half-Life chain journeys.
//!
//! Single-file tool, built with `rustc --edition=2021 -O tests/chain/hlroute.rs`.
//!
//! A route is plain text: one block per map, each block a list of run-length
//! controller commands in the semantic units the game itself derives from a
//! DualShock (forward, strafe, turn, look, buttons). The compiler turns every
//! block into poll-bound `PXITAPE2` tapes (one stick/button state per sim
//! tick, which is one controller poll) and writes the `psoxide-gate` journey
//! that plays them: wait for the map to load, wait for its first simulation
//! tick, replay the tape, move on. Nothing here edits game state; the journey
//! only presses controller inputs and reads RAM.
//!
//! Route syntax (`#` starts a comment):
//!
//! ```text
//! @journey name=hl-chain title="..."        # once, first non-comment line
//! map c0a0 next c0a0a [id=6050]             # begin a block; id is the cooked
//!                                           # header word that identifies the map
//!   900                                     # 900 neutral ticks
//!   35 f127                                 # forward full for 35 ticks
//!   1 s127                                  # strafe right
//!   3 use                                   # press Square (use) for 3 ticks
//!   8 t127 l-127 jump                       # axes: f s t l, buttons by name
//!   @checkpoint name                        # frame capture; splits the tape
//!   @assert sym=LOGIC_PLAYER_HEALTH size=2 op=ge value=50
//!   @wait sym=... size=.. op=.. value=.. timeout=N   # RAM wait, N in polls (sim ticks)
//!   @hold f127 until sym=... op=.. value=.. timeout=N # closed-loop input, N in polls
//!   @align f40 sym=... offset=.. signed=true lo=A hi=B timeout=N  # settle onto a band, slowly
//!   @raw <toml line>                        # copied verbatim into the journey
//! ```
//!
//! A block's trailing neutral run is not replayed: it only waits for the next
//! map to load, which the journey does with a RAM wait instead, so a map that
//! changes a few ticks earlier or later than the route author saw cannot shift
//! the next block.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Gate ticks are video frames; a simulation tick (one controller poll) takes three of them
/// when the frame is built in time. Route timeouts are written in polls and compiled to gate
/// ticks with this factor, twice the nominal three, so a slower build still gets its stick
/// moves and waits but a leg that has really stopped still times out.
const GATE_TICKS_PER_POLL: u32 = 6;

const BTN_SELECT: u16 = 0x0001;
const BTN_L3: u16 = 0x0002;
const BTN_START: u16 = 0x0008;
const BTN_L2: u16 = 0x0100;
const BTN_R2: u16 = 0x0200;
const BTN_L1: u16 = 0x0400;
const BTN_R1: u16 = 0x0800;
const BTN_TRIANGLE: u16 = 0x1000;
const BTN_CIRCLE: u16 = 0x2000;
const BTN_CROSS: u16 = 0x4000;
const BTN_SQUARE: u16 = 0x8000;

/// Radial stick dead zone the game ships with (`ANALOG_DEADZONE_DEFAULT`).
const DEADZONE: i32 = 28;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Sample {
    forward: i32,
    strafe: i32,
    turn: i32,
    look: i32,
    buttons: u16,
    /// Right-stick deflection chosen by [`compensate`]; replaces the plain
    /// inverse of the aim curve when set.
    rx: Option<i32>,
    ry: Option<i32>,
}

impl Sample {
    fn neutral(&self) -> bool {
        *self == Sample::default()
    }
}

fn button_for(name: &str) -> Option<u16> {
    Some(match name {
        "use" => BTN_SQUARE,
        "jump" => BTN_CROSS,
        "duck" => BTN_TRIANGLE,
        "attack" => BTN_R2,
        "attack2" => BTN_L2,
        "reload" => BTN_CIRCLE,
        "next" => BTN_R1,
        "prev" => BTN_L1,
        "flash" => BTN_L3,
        "start" => BTN_START,
        "select" => BTN_SELECT,
        _ => return None,
    })
}

fn button_name(bit: u16) -> &'static str {
    match bit {
        BTN_SQUARE => "use",
        BTN_CROSS => "jump",
        BTN_TRIANGLE => "duck",
        BTN_R2 => "attack",
        BTN_L2 => "attack2",
        BTN_CIRCLE => "reload",
        BTN_R1 => "next",
        BTN_L1 => "prev",
        BTN_L3 => "flash",
        BTN_START => "start",
        BTN_SELECT => "select",
        _ => "?",
    }
}

/// `psx_pad::aim_curve_symmetric`, the game's right-stick response.
fn aim_curve(v: i32) -> i32 {
    let v = v.clamp(-128, 128);
    let sign = if v < 0 { -1 } else { 1 };
    let m = v.abs();
    let cubic = m * m / 128 * m / 128;
    sign * ((m * 45 + cubic * 55) / 100)
}

/// Right-stick deflection (-127..=127) whose curve output is nearest `want`.
fn invert_aim(want: i32) -> i32 {
    let mut best: i32 = 0;
    let mut err = i32::MAX;
    for d in -127..=127 {
        let e = (aim_curve(d) - want).abs();
        if e < err || (e == err && d.abs() < best.abs()) {
            best = d;
            err = e;
        }
    }
    best
}

/// Scale a stick vector that sits inside the radial dead zone out to its edge,
/// keeping the direction. The game reads nothing from a stick inside the zone,
/// so a route nudge smaller than that cannot be made with a controller; the
/// smallest real nudge is used instead and reported.
fn clear_deadzone(x: i32, y: i32, note: &mut Vec<String>, what: &str) -> (i32, i32) {
    if (x != 0 || y != 0) && x * x + y * y <= DEADZONE * DEADZONE {
        let mag = ((x * x + y * y) as f64).sqrt();
        let k = (DEADZONE as f64 + 1.0) / mag;
        let r = ((x as f64 * k).round() as i32, (y as f64 * k).round() as i32);
        note.push(format!("{what} ({x},{y}) raised to ({},{})", r.0, r.1));
        r
    } else {
        (x, y)
    }
}

/// The game's per-tick view step for a stick axis value (`angle_step_nearest`).
fn angle_step(axis: i32, rate: i32) -> i32 {
    let p = axis * rate;
    if p >= 0 { (p + 64) / 128 } else { -((-p + 64) / 128) }
}

/// A DualShock cannot reach a turn value of +127 (its fullest right deflection
/// reads 125), so a route written in the oracle's semantic units would slowly
/// lose heading. Walk the tape and choose the right-stick deflection for each
/// tick that keeps the accumulated view angle as close as the stick allows to
/// what the semantic route asks for. Ticks that also move or press a button
/// keep their own value; only turn/look and idle ticks are adjusted.
fn compensate(samples: &mut [Sample]) {
    // (rate, extract semantic, set stick) for yaw (turn) then pitch (look).
    let axes: [(i32, bool); 2] = [(130, false), (95, true)];
    for (rate, is_look) in axes {
        let (mut want, mut got) = (0i32, 0i32);
        for s in samples.iter_mut() {
            let sem = if is_look { s.look } else { s.turn };
            let target = want + angle_step(sem, rate);
            let idle = s.forward == 0 && s.strafe == 0 && s.buttons == 0;
            let other_active = if is_look { s.turn != 0 } else { s.look != 0 };
            let mut best = 0i32;
            let mut best_err = i32::MAX;
            let mut best_abs = i32::MAX;
            if sem != 0 || idle {
                for d in -128..=127i32 {
                    let reach = if is_look { -d } else { d };
                    if d != 0 && d.abs() <= DEADZONE && !other_active {
                        continue;
                    }
                    let step = angle_step(aim_curve(reach), rate);
                    let err = (got + step - target).abs();
                    if err < best_err || (err == best_err && d.abs() < best_abs) {
                        best = d;
                        best_err = err;
                        best_abs = d.abs();
                    }
                }
            }
            let applied = if is_look { -best } else { best };
            if sem != 0 || idle {
                if is_look { s.ry = Some(best); } else { s.rx = Some(best); }
            }
            let _ = applied;
            got += if sem != 0 || idle { angle_step(aim_curve(if is_look { -best } else { best }), rate) } else { 0 };
            want = target;
        }
    }
}

fn encode(s: Sample, note: &mut Vec<String>) -> Result<[u8; 6], String> {
    let (lx, ly) = clear_deadzone(s.strafe, -s.forward, note, "move");
    let rx = s.rx.unwrap_or(if s.turn == 0 { 0 } else { invert_aim(s.turn) });
    let ry = s.ry.unwrap_or(if s.look == 0 { 0 } else { -invert_aim(s.look) });
    let (rx, ry) = clear_deadzone(rx, ry, note, "aim");
    let byte = |c: i32| (128 + c.clamp(-128, 127)) as u8;
    let b = s.buttons.to_le_bytes();
    Ok([b[0], b[1], byte(rx), byte(ry), byte(lx), byte(ly)])
}

// ---------------------------------------------------------------------------
// Route model

#[derive(Debug)]
enum Item {
    Run(u32, Sample),
    Checkpoint {
        name: String,
        asserts: Vec<String>,
    },
    Wait(String, u32),
    Hold {
        sample: Sample,
        until: String,
        timeout: u32,
    },
    Raw(String),
    /// A gate loop step: `body` (holds and neutral waits only) repeats as a named block until the
    /// `until` condition holds, at most `max_iter` passes and `timeout` polls.
    Loop {
        body: Vec<Item>,
        until: String,
        max_iter: u32,
        timeout: u32,
    },
    /// Pause, Save Game to Card 1 Manual, then persist the card (`card-step-NNN.mcd` in the
    /// report folder). Later checkpoints no longer assert `card_unchanged`.
    Save,
    /// End of the journey: later items and blocks are not compiled.
    Stop,
}

#[derive(Debug)]
struct Block {
    map: String,
    next: String,
    id: Option<i64>,
    items: Vec<Item>,
}

#[derive(Debug, Default)]
struct Route {
    journey: Vec<(String, String)>,
    blocks: Vec<Block>,
}

fn parse_kv(tokens: &[&str]) -> Vec<(String, String)> {
    tokens
        .iter()
        .map(|t| match t.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (t.to_string(), "true".to_string()),
        })
        .collect()
}

fn assert_table(kind: &str, kvs: &[(String, String)]) -> String {
    // `expr="..."` makes any condition a gate expression (integer arithmetic over RAM reads).
    let kind = if kvs.iter().any(|(k, _)| k == "expr") { "expr" } else { kind };
    let mut out = format!("kind={kind:?}");
    for (k, v) in kvs {
        let numeric = v.parse::<i64>().is_ok() || v.starts_with("0x") || v == "true" || v == "false";
        if numeric {
            let _ = write!(out, ", {k}={v}");
        } else {
            let _ = write!(out, ", {k}={:?}", v.trim_matches('"'));
        }
    }
    format!("{{ {out} }}")
}

/// Turn the inline-table body `a=1, b="x, y"` into one `key = value` per line, splitting only at
/// the `, ` outside quotes (a gate expression may contain `, `).
fn split_top(body: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' if quoted && i + 1 < b.len() => {
                out.push(b[i] as char);
                out.push(b[i + 1] as char);
                i += 2;
                continue;
            }
            b'"' => quoted = !quoted,
            b',' if !quoted && b.get(i + 1) == Some(&b' ') => {
                out.push('\n');
                i += 2;
                continue;
            }
            _ => {}
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn parse_sample(tokens: &[&str]) -> Result<Sample, String> {
    let mut s = Sample::default();
    for t in tokens {
        if let Some(b) = button_for(t) {
            s.buttons |= b;
            continue;
        }
        let (head, rest) = t.split_at(1);
        let v: i32 = rest
            .parse()
            .map_err(|_| format!("unknown run token `{t}`"))?;
        match head {
            "f" => s.forward = v,
            "s" => s.strafe = v,
            "t" => s.turn = v,
            "l" => s.look = v,
            _ => return Err(format!("unknown run token `{t}`")),
        }
    }
    Ok(s)
}

fn split_line(line: &str) -> Vec<&str> {
    // Quoted values (`title="a b"`) stay whole.
    let mut out = Vec::new();
    let mut start = None;
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if let Some(s) = start.take() {
                    out.push(&line[s..i]);
                }
            }
            _ => {
                if start.is_none() {
                    start = Some(i);
                }
            }
        }
    }
    if let Some(s) = start {
        out.push(&line[s..]);
    }
    out
}

// ---------------------------------------------------------------------------
// Closed-loop aim: `@aim`, `@kill`, `@level`
//
// The conditions are gate expressions over the player's pose and the target's position, so the
// stick is held until the angle to the target (not a tick count, not the spot the player happened
// to stop on) says stop. Yaw word: 4096 a turn, 0 faces +y, 1024 faces +x, turning right adds.
// The sine is an integer rational approximation (Bhaskara, error under 0.2%), so the expression
// does not depend on where a build keeps its sine table.

/// What `@aim` looks at: a prop slot (position from `PROP_POS`, liveness from `PROP_HEALTH`) or a fixed point.
struct AimTarget {
    x: String,
    up: String,
    y: String,
    /// Expression that is true when the target no longer needs shooting (`0` for a point).
    dead: String,
}

fn sin_q12_expr(a: &str) -> String {
    format!("(let h = ({a}) & 2047 in (let p = h * (2048 - h) in (((({a}) & 2048) != 0) ? 0 - 65536 * p / (20971520 - 4 * p) : 65536 * p / (20971520 - 4 * p))))")
}

#[derive(Clone, Copy)]
enum Aim {
    Coarse(bool),
    Fine(bool),
    Within,
}

impl AimTarget {
    /// The yaw condition. `right` is the direction of the turn that is being held; `Within` is the
    /// settled test. `lat` is the lateral miss, in units, that still counts as on target (the
    /// angular floor is about 2.9 degrees, above the smallest turn step of 2.5).
    fn yaw(&self, lat: i64, a: Aim) -> String {
        let t = format!("max({lat} * 4096, d / 20)");
        let body = match a {
            Aim::Coarse(true) => format!("d > 0 && e <= 3 * {t}"),
            Aim::Coarse(false) => format!("d > 0 && e >= 0 - 3 * {t}"),
            Aim::Fine(true) => format!("d > 0 && e <= {t}"),
            Aim::Fine(false) => format!("d > 0 && e >= 0 - {t}"),
            Aim::Within => format!("d > 0 && abs(e) <= {t}"),
        };
        format!(
            "let yaw = u16(LOGIC_PLAYER_YAW), sn = {}, cs = {}, dx = {} - i32(LOGIC_PLAYER_POS[0]), dy = {} - i32(LOGIC_PLAYER_POS[2]), e = dx * cs - dy * sn, d = dx * sn + dy * cs in ({body})",
            sin_q12_expr("yaw"),
            sin_q12_expr("(yaw + 1024) & 4095"),
            self.x,
            self.y
        )
    }

    /// The pitch condition: the pitch word (4096 a turn, up is positive) against the elevation of the
    /// target from the eye. `vlat` is the vertical miss, in units, that still counts as on target.
    fn pitch(&self, vlat: i64, aimup: i64, a: Aim) -> String {
        let body = match a {
            Aim::Coarse(true) => "ep >= 0 - 3 * tol".to_string(),
            Aim::Coarse(false) => "ep <= 3 * tol".to_string(),
            Aim::Fine(true) => "ep >= 0 - tol".to_string(),
            Aim::Fine(false) => "ep <= tol".to_string(),
            Aim::Within => "abs(ep) <= tol".to_string(),
        };
        format!(
            "let dx = {} - i32(LOGIC_PLAYER_POS[0]), dy = {} - i32(LOGIC_PLAYER_POS[2]), hd = max(max(abs(dx), abs(dy)) + 3 * min(abs(dx), abs(dy)) / 8, 16), dz = {} + {aimup} - (i32(LOGIC_PLAYER_POS[1]) + i32(LOGIC_PLAYER_VIEW_HEIGHT)), ep = i16(LOGIC_PLAYER_PITCH) - (let r = abs(dz), u = min(r, hd) * 1024 / max(r, hd), at = u * 1024 / (1024 + ((287 * u * u) >> 20)) in ((dz < 0 ? 0 - 1 : 1) * (((r > hd ? 1608 - at : at)) * 652 >> 10))), tol = max({vlat} * 652 / hd, 40) in ({body})",
            self.x, self.y, self.up
        )
    }

    fn or_dead(&self, cond: String) -> String {
        if self.dead == "0" {
            cond
        } else {
            format!("({}) || ({cond})", self.dead)
        }
    }
}

/// `x>=-812`, `y<=-69`, `up>=-300` as a gate expression over `LOGIC_PLAYER_POS` (x, up, y).
fn position_shorthand(tok: &str) -> Option<String> {
    let (axis, rest) = if let Some(r) = tok.strip_prefix("up") {
        (1, r)
    } else if let Some(r) = tok.strip_prefix('x') {
        (0, r)
    } else if let Some(r) = tok.strip_prefix('y') {
        (2, r)
    } else {
        return None;
    };
    let (op, num) = ["<=", ">="].iter().find_map(|op| rest.strip_prefix(op).map(|n| (*op, n)))?;
    let v: i64 = num.parse().ok()?;
    Some(format!("i32(LOGIC_PLAYER_POS[{axis}]) {op} {v}"))
}

fn expr_cond(e: String) -> String {
    assert_table("ram", &[("expr".to_string(), e)])
}

fn sign(v: i64) -> &'static str {
    if v < 0 {
        "-"
    } else {
        "+"
    }
}

/// Distance-and-slot key of the nearest living prop within `r` units (3D): `distance^2 * 256 + slot`, or
/// 2000000000 when none is in range. The slot is the low byte.
fn nearest_prop_expr(r: i64, kind: Option<i64>, below: Option<i64>) -> String {
    let d2 = |i: &str| format!("(let qx = i32(PROP_POS[3 * {i}]) - i32(LOGIC_PLAYER_POS[0]), qy = i32(PROP_POS[3 * {i} + 2]) - i32(LOGIC_PLAYER_POS[2]), qz = i32(PROP_POS[3 * {i} + 1]) - i32(LOGIC_PLAYER_POS[1]) in qx * qx + qy * qy + qz * qz)");
    let kind = kind.map_or(String::new(), |k| format!(" && u8(PROP_KIND[i]) == {k}"));
    // `below=B`: skip props whose feet are more than B units under the player's centre (under the floor or on a
    // lower level, where a shot cannot reach them).
    let below = below.map_or(String::new(), |b| format!(" && i32(PROP_POS[3 * i + 1]) - i32(LOGIC_PLAYER_POS[1]) >= 0 - {b}"));
    format!("min(i in 0..128: (u8(PROP_HEALTH[i]) > 0{kind}{below} && {} < {}) ? ({} * 256 + i) : 2000000000)", d2("i"), r * r, d2("i"))
}

/// `@aim|@kill prop=N | near=R [kind=K] [below=B] | rel=DX,DUP,DY [unless=R] | at=X,UP,Y [lat=4] [vlat=4] [aimup=0] [rounds=8] [timeout=150]`, and `@level [tol=48]`.
fn aim_items(head: &str, kv: &[(String, String)]) -> Result<Vec<Item>, String> {
    let get = |k: &str| kv.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let num = |k: &str, d: i64| -> Result<i64, String> {
        get(k).map_or(Ok(d), |v| v.parse().map_err(|_| format!("{head}: bad {k}")))
    };
    let timeout = num("timeout", 150)? as u32;
    let hold = |sample: Sample, until: String| Item::Hold { sample, until: expr_cond(until), timeout };
    let mut out = Vec::new();
    if head == "@level" {
        let tol = num("tol", 48)?;
        let p = "i16(LOGIC_PLAYER_PITCH)";
        let up = Sample { look: 127, ..Sample::default() };
        let down = Sample { look: -127, ..Sample::default() };
        let fine_up = Sample { look: 29, ..Sample::default() };
        let fine_down = Sample { look: -29, ..Sample::default() };
        out.push(hold(up, format!("{p} >= 0 - {tol}")));
        out.push(hold(down, format!("{p} <= {tol}")));
        out.push(hold(fine_down, format!("{p} <= {tol}")));
        out.push(hold(fine_up, format!("{p} >= 0 - {tol}")));
        return Ok(out);
    }
    let kind: Option<i64> = match get("kind") {
        Some(k) => Some(k.parse().map_err(|_| format!("{head}: bad kind"))?),
        None => None,
    };
    let below: Option<i64> = match get("below") {
        Some(k) => Some(k.parse().map_err(|_| format!("{head}: bad below"))?),
        None => None,
    };
    let target = if let Some(n) = get("prop") {
        let n: i64 = n.parse().map_err(|_| format!("{head}: bad prop"))?;
        AimTarget {
            x: format!("i32(PROP_POS[{}])", 3 * n),
            up: format!("i32(PROP_POS[{}])", 3 * n + 1),
            y: format!("i32(PROP_POS[{}])", 3 * n + 2),
            dead: format!("u8(PROP_HEALTH[{n}]) == 0"),
        }
    } else if let Some(r) = get("near") {
        // The nearest living prop within `near` units (3D, feet height against the player's centre): the
        // slot is found by the expression itself, so the same line serves any slot numbering. The target
        // is "dead" when nothing living is in range, so rounds that cannot find anything cost no ticks.
        let r: i64 = r.parse().map_err(|_| format!("{head}: bad near"))?;
        let nearest = nearest_prop_expr(r, kind, below);
        let slot = format!("(({nearest}) & 255)");
        AimTarget {
            x: format!("i32(PROP_POS[3 * {slot}])"),
            up: format!("i32(PROP_POS[3 * {slot} + 1])"),
            y: format!("i32(PROP_POS[3 * {slot} + 2])"),
            dead: format!("({nearest}) >= 2000000000"),
        }
    } else if let Some(rel) = get("rel") {
        // A point fixed relative to the player (dx, dup from the eye, dy in world axes): holds the view in a
        // stance, for instance the direction an enemy is known to arrive from. `unless=R` ends it at once
        // when a living prop is within R (fight first, re-aim later).
        let v: Vec<&str> = rel.split(',').collect();
        let n: Vec<i64> = v.iter().filter_map(|s| s.parse().ok()).collect();
        if v.len() != 3 || n.len() != 3 {
            return Err(format!("{head}: rel=DX,DUP,DY"));
        }
        let dead = match get("unless") {
            Some(r) => {
                let r: i64 = r.parse().map_err(|_| format!("{head}: bad unless"))?;
                format!("({}) < 2000000000", nearest_prop_expr(r, kind, below))
            }
            None => "0".into(),
        };
        AimTarget {
            x: format!("(i32(LOGIC_PLAYER_POS[0]) {} {})", sign(n[0]), n[0].abs()),
            up: format!("(i32(LOGIC_PLAYER_POS[1]) + i32(LOGIC_PLAYER_VIEW_HEIGHT) {} {})", sign(n[1]), n[1].abs()),
            y: format!("(i32(LOGIC_PLAYER_POS[2]) {} {})", sign(n[2]), n[2].abs()),
            dead,
        }
    } else if let Some(at) = get("at") {
        let v: Vec<&str> = at.split(',').collect();
        if v.len() != 3 || v.iter().any(|s| s.parse::<i64>().is_err()) {
            return Err(format!("{head}: at=X,UP,Y"));
        }
        AimTarget { x: v[0].into(), up: v[1].into(), y: v[2].into(), dead: "0".into() }
    } else {
        return Err(format!("{head} needs prop=N, near=R, rel=DX,DUP,DY or at=X,UP,Y"));
    };
    let lat = num("lat", 4)?;
    let vlat = num("vlat", 4)?;
    let aimup = num("aimup", 0)?;
    let rounds = if head == "@kill" { num("rounds", 8)? } else { 1 };
    let turn = |v: i32| Sample { turn: v, ..Sample::default() };
    let look = |v: i32| Sample { look: v, ..Sample::default() };
    let with_pitch = get("pitch") != Some("0");
    for round in 0..rounds {
        // The first round brings the view on target (a coarse turn each way, then the dead-band triple of
        // the smallest turn, yaw then pitch); later rounds only trim, because the target moves while the
        // view turns, and then fire until it is dead or has moved out of the aim.
        let yaw_fine = |out: &mut Vec<Item>| {
            out.push(hold(turn(29), target.or_dead(target.yaw(lat, Aim::Fine(true)))));
            out.push(hold(turn(-29), target.or_dead(target.yaw(lat, Aim::Fine(false)))));
            out.push(hold(turn(29), target.or_dead(target.yaw(lat, Aim::Fine(true)))));
        };
        let pitch_fine = |out: &mut Vec<Item>| {
            out.push(hold(look(29), target.or_dead(target.pitch(vlat, aimup, Aim::Fine(true)))));
            out.push(hold(look(-29), target.or_dead(target.pitch(vlat, aimup, Aim::Fine(false)))));
            out.push(hold(look(29), target.or_dead(target.pitch(vlat, aimup, Aim::Fine(true)))));
        };
        if round == 0 {
            out.push(hold(turn(60), target.or_dead(target.yaw(lat, Aim::Coarse(true)))));
            out.push(hold(turn(-60), target.or_dead(target.yaw(lat, Aim::Coarse(false)))));
            yaw_fine(&mut out);
            if with_pitch {
                out.push(hold(look(127), target.or_dead(target.pitch(vlat, aimup, Aim::Coarse(true)))));
                out.push(hold(look(-127), target.or_dead(target.pitch(vlat, aimup, Aim::Coarse(false)))));
                pitch_fine(&mut out);
            }
        }
        yaw_fine(&mut out);
        if with_pitch {
            pitch_fine(&mut out);
        }
        if head == "@kill" {
            let on = format!("({}) && ({})", target.yaw(lat, Aim::Within), target.pitch(vlat, aimup, Aim::Within));
            let mut s = Sample::default();
            s.buttons = button_for("attack").unwrap();
            out.push(hold(s, target.or_dead(format!("!({on})"))));
        }
    }
    Ok(out)
}

/// `@fight near=R [kind=K] [below=B] [reload=N] [for=300] [sleep=10] [stance=DX,DUP,DY] [lat=8] [aimup=0] [rounds=3]`: a guard that
/// shoots whatever living prop (of `kind`) comes within `near` units, nearest first, for `for` polls. Each
/// round is `@kill near=` (no cost when nothing is in range), `sleep` neutral polls, and, when a `stance` is
/// given, `@aim rel=` back to the direction the enemies come from (skipped at once while one is in range).
/// Without `until=EXPR` the loop is unrolled at compile time (a long guard makes a huge journey); with it the
/// round is one gate block repeated until EXPR holds and nothing is in range, at most `for` polls.
fn fight_items(kv: &[(String, String)]) -> Result<Vec<Item>, String> {
    let get = |k: &str| kv.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let num = |k: &str, d: i64| -> Result<i64, String> {
        get(k).map_or(Ok(d), |v| v.parse().map_err(|_| format!("@fight: bad {k}")))
    };
    let near = get("near").ok_or("@fight needs near=R")?;
    let (sleep, total) = (num("sleep", 10)?.max(1), num("for", 300)?);
    let mut kill: Vec<(String, String)> = vec![("near".into(), near.clone()), ("rounds".into(), num("rounds", 3)?.to_string())];
    kill.push(("lat".into(), num("lat", 8)?.to_string()));
    for k in ["kind", "below", "aimup", "vlat"] {
        if let Some(v) = get(k) {
            kill.push((k.into(), v));
        }
    }
    let stance: Option<Vec<(String, String)>> = get("stance").map(|v| {
        vec![
            ("rel".into(), v),
            ("unless".into(), near.clone()),
            ("lat".into(), num("stancelat", 20).unwrap_or(20).to_string()),
            ("vlat".into(), num("stancevlat", 40).unwrap_or(40).to_string()),
            ("timeout".into(), "60".into()),
        ]
    });
    let topup = num("reload", 0)?;
    let mut round = Vec::new();
    if topup > 0 {
        // Reload when the magazine is under `reload` rounds (ends at once when it is not).
        let mut sample = Sample::default();
        sample.buttons = button_for("reload").unwrap();
        let until = expr_cond(format!("i16(LOGIC_PLAYER_CLIP_AMMO) >= {topup} || i16(LOGIC_PLAYER_RESERVE_AMMO) == 0"));
        round.push(Item::Hold { sample, until, timeout: 80 });
    }
    round.extend(aim_items("@kill", &kill)?);
    round.push(Item::Run(sleep as u32, Sample::default()));
    if let Some(st) = &stance {
        round.extend(aim_items("@aim", st)?);
    }
    if let Some(end) = get("until") {
        // A gate loop: the round is written once and repeats until `until` holds AND nothing living of
        // that kind is within `near` (the quiet check), at most `for` polls.
        let near_n: i64 = near.parse().map_err(|_| "@fight: bad near".to_string())?;
        let kind: Option<i64> = get("kind").and_then(|v| v.parse().ok());
        let below: Option<i64> = get("below").and_then(|v| v.parse().ok());
        let quiet = format!("({}) >= 2000000000", nearest_prop_expr(near_n, kind, below));
        let end = end.trim_matches('"');
        let until = expr_cond(format!("({end}) && {quiet}"));
        let max_iter = (total / (sleep + 1) + 1) as u32;
        return Ok(vec![Item::Loop { body: round, until, max_iter, timeout: total as u32 }]);
    }
    // No `until`: unrolled at compile time, `for` polls of rounds.
    let mut out = Vec::new();
    let mut spent = 0;
    while spent < total {
        out.extend(round.iter().map(|i| match i {
            Item::Hold { sample, until, timeout } => Item::Hold { sample: *sample, until: until.clone(), timeout: *timeout },
            Item::Run(n, s) => Item::Run(*n, *s),
            _ => unreachable!("a fight round holds only holds and rests"),
        }));
        spent += sleep + 1;
    }
    Ok(out)
}

fn parse_route(text: &str) -> Result<Route, String> {
    let mut route = Route::default();
    let mut pending_checkpoint: Option<(String, Vec<String>)> = None;
    for (n, raw) in text.lines().enumerate() {
        let at = |m: String| format!("line {}: {m}", n + 1);
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let tok = split_line(line);
        let head = tok[0];
        if head != "@assert" && head != "@probe" {
            if let (Some((name, asserts)), Some(b)) =
                (pending_checkpoint.take(), route.blocks.last_mut())
            {
                b.items.push(Item::Checkpoint { name, asserts });
            }
        }
        match head {
            "@journey" => route.journey = parse_kv(&tok[1..]),
            "map" => {
                let mut kv = parse_kv(&tok[2..]);
                let mut next = String::new();
                let mut id = None;
                if tok.len() < 2 {
                    return Err(at("map needs a name".into()));
                }
                let mut i = 0;
                while i < kv.len() {
                    // `next c0a0a` is two bare tokens.
                    if kv[i].0 == "next" && kv[i].1 == "true" && i + 1 < kv.len() {
                        next = kv[i + 1].0.clone();
                        kv.remove(i + 1);
                        kv.remove(i);
                        continue;
                    }
                    if kv[i].0 == "id" {
                        id = Some(kv[i].1.parse().map_err(|_| at("bad id".into()))?);
                    }
                    i += 1;
                }
                route.blocks.push(Block {
                    map: tok[1].to_string(),
                    next,
                    id,
                    items: Vec::new(),
                });
            }
            "@checkpoint" => {
                pending_checkpoint = Some((tok.get(1).ok_or_else(|| at("name".into()))?.to_string(), Vec::new()));
            }
            "@probe" => {
                // Authoring aid: print the player's position (x, up, y) and yaw in the report.
                let (_, asserts) = pending_checkpoint
                    .as_mut()
                    .ok_or_else(|| at("@probe outside a checkpoint".into()))?;
                for (off, size) in [(0, 4), (4, 4), (8, 4)] {
                    let kv = parse_kv(&["sym=LOGIC_PLAYER_POS", &format!("size={size}"), "signed=true", &format!("offset={off}"), "op=between", "value=-100000", "value_hi=100000"]);
                    asserts.push(assert_table("ram", &kv));
                }
                let kv = parse_kv(&["sym=LOGIC_PLAYER_YAW", "size=2", "op=between", "value=0", "value_hi=65535"]);
                asserts.push(assert_table("ram", &kv));
            }
            "@assert" => {
                let (_, asserts) = pending_checkpoint
                    .as_mut()
                    .ok_or_else(|| at("@assert outside a checkpoint".into()))?;
                asserts.push(assert_table("ram", &parse_kv(&tok[1..])));
            }
            "@wait" => {
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                let mut kv = parse_kv(&tok[1..]);
                let timeout = match kv.iter().position(|(k, _)| k == "timeout") {
                    Some(i) => kv.remove(i).1.parse().map_err(|_| at("timeout".into()))?,
                    None => 1500,
                };
                b.items.push(Item::Wait(assert_table("ram", &kv), timeout));
            }
            "@hold" => {
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                let split = tok
                    .iter()
                    .position(|t| *t == "until")
                    .ok_or_else(|| at("@hold needs `until`".into()))?;
                let sample = parse_sample(&tok[1..split]).map_err(at)?;
                // Shorthand for the commonest leg end: `until x>=-812`, `y<=-69`, `up>=-300`.
                let mut kv = match tok.get(split + 1).and_then(|t| position_shorthand(t)) {
                    Some(e) => {
                        let mut kv = vec![("expr".to_string(), e)];
                        kv.extend(parse_kv(&tok[split + 2..]));
                        kv
                    }
                    None => parse_kv(&tok[split + 1..]),
                };
                let ti = kv.iter().position(|(k, _)| k == "timeout");
                let timeout = match ti {
                    Some(i) => kv.remove(i).1.parse().map_err(|_| at("timeout".into()))?,
                    None => return Err(at("@hold needs timeout=".into())),
                };
                b.items.push(Item::Hold {
                    sample,
                    until: assert_table("ram", &kv),
                    timeout,
                });
            }
            "@align" => {
                // `@align SAMPLE sym=.. [offset=..] [size=..] [signed=true] lo=A hi=B timeout=N [settle=N]`:
                // bring a RAM word into [lo, hi] with slow nudges. SAMPLE is the (slow) input that
                // raises the word; its negation lowers it. A full-stick hold leaves the player
                // sliding for tens of units after the release, so each nudge is followed by a
                // neutral settle (default 25 ticks) before the word is read again.
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                let parts: Vec<&str> = tok[1].split('+').collect(); // `s40+duck`: axes and buttons joined
                let up = parse_sample(&parts).map_err(at)?;
                let down = Sample {
                    forward: -up.forward,
                    strafe: -up.strafe,
                    turn: -up.turn,
                    look: -up.look,
                    ..up
                };
                let mut kv = parse_kv(&tok[2..]);
                let mut take = |k: &str| -> Option<String> {
                    kv.iter().position(|(n, _)| n == k).map(|i| kv.remove(i).1)
                };
                let lo: i64 = take("lo").and_then(|v| v.parse().ok()).ok_or_else(|| at("@align needs lo=".into()))?;
                let hi: i64 = take("hi").and_then(|v| v.parse().ok()).ok_or_else(|| at("@align needs hi=".into()))?;
                let timeout: u32 = take("timeout").and_then(|v| v.parse().ok()).ok_or_else(|| at("@align needs timeout=".into()))?;
                let settle: u32 = take("settle").and_then(|v| v.parse().ok()).unwrap_or(25);
                if lo > hi {
                    return Err(at("@align lo is above hi".into()));
                }
                let cond = |op: &str, value: i64| {
                    let mut k = kv.clone();
                    k.push(("op".into(), op.into()));
                    k.push(("value".into(), value.to_string()));
                    assert_table("ram", &k)
                };
                for (sample, until) in [(up, cond("ge", lo)), (down, cond("le", hi)), (up, cond("ge", lo))] {
                    b.items.push(Item::Run(settle, Sample::default()));
                    b.items.push(Item::Hold { sample, until, timeout });
                }
                b.items.push(Item::Run(settle, Sample::default()));
            }
            "@face" => {
                // `@face YAW right|left [timeout=N] [duck=true]`: turn until the player's yaw word (0..4095, 1024 is
                // a quarter turn, right is increasing) is within 60 units past YAW (before it, turning left).
                // A coarse t60 turn to 90 units short of the target (one-sided), then a dead-band triple of
                // single-step turns (29 units) that recovers from a late stop in either direction.
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                let yaw: i32 = tok
                    .get(1)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| at("@face needs a yaw".into()))?;
                let right = match tok.get(2).copied() {
                    Some("right") => true,
                    Some("left") => false,
                    _ => return Err(at("@face needs right or left".into())),
                };
                let kv = parse_kv(&tok[3..]);
                let timeout = kv
                    .iter()
                    .find(|(k, _)| k == "timeout")
                    .map(|(_, v)| v.parse().unwrap_or(600))
                    .unwrap_or(600);
                // Bands as (low, high), both inside 0..=4095. The fine band is 60 wide on the target's own
                // side, so a coarse hold that ends one poll late (the gate looks every frame, the game polls every
                // third) and overshoots can still be recovered by turning back instead of a full circle.
                let (coarse, fine) = if right {
                    ((yaw - 170, yaw - 90), (yaw, yaw + 59))
                } else {
                    ((yaw + 90, yaw + 170), (yaw - 59, yaw))
                };
                for (lo, hi) in [coarse, fine] {
                    if lo < 0 || hi > 4095 {
                        return Err(at("@face band crosses the yaw wrap: pick another target or direction".into()));
                    }
                }
                let word = |op: &str, value: i32| {
                    assert_table(
                        "ram",
                        &[
                            ("sym".to_string(), "LOGIC_PLAYER_YAW".to_string()),
                            ("size".to_string(), "2".to_string()),
                            ("op".to_string(), op.to_string()),
                            ("value".to_string(), value.to_string()),
                        ],
                    )
                };
                let sign = if right { 1 } else { -1 };
                // `duck=true` keeps the crouch button down through the turn (a vent or a crawl: the hull
                // must not stand up).
                let buttons = if kv.iter().any(|(k, v)| k == "duck" && v == "true") { BTN_TRIANGLE } else { 0 };
                // Coarse: one-sided, so a start already past the band (or an overshoot) ends the hold at once
                // and the fine stage turns back, where a two-sided window would send it round the circle.
                b.items.push(Item::Hold {
                    sample: Sample { turn: 60 * sign, buttons, ..Sample::default() },
                    until: if right { word("ge", coarse.0 + 80) } else { word("le", coarse.1 - 80) },
                    timeout,
                });
                // Fine: dead-band triple with the smallest turn (29 a tick, narrower than the band): up to the
                // low edge, back down to the high edge, up again. Each hold that already holds ends at once.
                let up = Sample { turn: 29, buttons, ..Sample::default() };
                let down = Sample { turn: -29, buttons, ..Sample::default() };
                for (sample, until) in [(up, word("ge", fine.0)), (down, word("le", fine.1)), (up, word("ge", fine.0))] {
                    b.items.push(Item::Hold { sample, until, timeout });
                }
            }
            "@fight" => {
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                let items = fight_items(&parse_kv(&tok[1..])).map_err(at)?;
                b.items.extend(items);
            }
            "@aim" | "@kill" | "@level" => {
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                let items = aim_items(head, &parse_kv(&tok[1..])).map_err(at)?;
                b.items.extend(items);
            }
            "@stop" => {
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                b.items.push(Item::Stop);
            }
            "@swing" => {
                // `@swing ent=N [duck=true] [pitch=P] [timeout=40]`: swing the selected melee weapon (attack
                // held, crouched if asked) until brush entity N is gone (`ENT_ACTIVE[N] == 0`; N is the
                // cooked entity index, not the BSP model number: find it by scanning `ENT_ACTIVE` before
                // and after one swing). `pitch=P` first brings the view to P (+-48), crouched if asked, so the swing is aimed
                // without a target point.
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                let kv = parse_kv(&tok[1..]);
                let get = |k: &str| kv.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
                let ent: i64 = get("ent").and_then(|v| v.parse().ok()).ok_or_else(|| at("@swing needs ent=N".into()))?;
                let duck = get("duck") == Some("true");
                let timeout: u32 = get("timeout").map_or(Ok(40), |v| v.parse().map_err(|_| at("timeout".into())))?;
                let buttons = if duck { BTN_TRIANGLE } else { 0 };
                if let Some(p) = get("pitch") {
                    let p: i64 = p.parse().map_err(|_| at("pitch".into()))?;
                    let pe = "i16(LOGIC_PLAYER_PITCH)";
                    b.items.push(Item::Hold {
                        sample: Sample { look: 127, buttons, ..Sample::default() },
                        until: expr_cond(format!("{pe} >= {} ", p - 48)),
                        timeout: 40,
                    });
                    b.items.push(Item::Hold {
                        sample: Sample { look: -127, buttons, ..Sample::default() },
                        until: expr_cond(format!("{pe} <= {}", p + 48)),
                        timeout: 40,
                    });
                }
                b.items.push(Item::Hold {
                    sample: Sample { buttons: buttons | BTN_R2, ..Sample::default() },
                    until: expr_cond(format!("u8(ENT_ACTIVE[{ent}]) == 0")),
                    timeout,
                });
            }
            "@weapon" => {
                // `@weapon NAME [dir=prev|next] [timeout=60]`: press the cycle button once (L1 is
                // prev, R1 next, one rising edge) and hold it until the viewmodel of that weapon
                // is live (`VM_LIVE_WM`, which only reads it once its chunk has streamed in),
                // then rest for the deploy delay. Free if it is already in hand. `CARRY_CURRENT`
                // is the changelevel snapshot, not the live weapon: do not wait on it.
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                let name = tok.get(1).ok_or_else(|| at("@weapon needs a name".into()))?;
                let wm = WEAPON_VIEWMODELS
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, wm)| *wm)
                    .ok_or_else(|| at(format!("@weapon: unknown weapon `{name}`")))?;
                let kv = parse_kv(&tok[2..]);
                let get = |k: &str| kv.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
                let button = match get("dir").unwrap_or("prev") {
                    "prev" => BTN_L1,
                    "next" => BTN_R1,
                    _ => return Err(at("@weapon dir= is prev or next".into())),
                };
                let timeout: u32 = get("timeout").map_or(Ok(60), |v| v.parse().map_err(|_| at("timeout".into())))?;
                b.items.push(Item::Hold {
                    sample: Sample { buttons: button, ..Sample::default() },
                    until: expr_cond(format!("u8(VM_LIVE_WM) == {wm}")),
                    timeout,
                });
                b.items.push(Item::Run(WEAPON_DEPLOY_POLLS, Sample::default()));
            }
            "@save" => {
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                b.items.push(Item::Save);
            }
            "@raw" => {
                let b = route.blocks.last_mut().ok_or_else(|| at("no map".into()))?;
                b.items.push(Item::Raw(line["@raw".len()..].trim().to_string()));
            }
            _ => {
                let b = route.blocks.last_mut().ok_or_else(|| at("run before any map".into()))?;
                let ticks: u32 = head.parse().map_err(|_| at(format!("unknown directive `{head}`")))?;
                let s = parse_sample(&tok[1..]).map_err(at)?;
                let mut note = Vec::new();
                encode(s, &mut note).map_err(at)?;
                for msg in note {
                    eprintln!("note: line {}: {msg}", n + 1);
                }
                b.items.push(Item::Run(ticks, s));
            }
        }
    }
    if let (Some((name, asserts)), Some(b)) = (pending_checkpoint.take(), route.blocks.last_mut()) {
        b.items.push(Item::Checkpoint { name, asserts });
    }
    Ok(route)
}

// ---------------------------------------------------------------------------
// HLINPUT1 import (the oracle route format)

fn import(path: &Path) -> Result<String, String> {
    let d = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if d.len() < 16 || &d[..8] != b"HLINPUT1" {
        return Err("not an HLINPUT1 tape".into());
    }
    let u16_at = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
    let segments = u16_at(10);
    let mut o = 16;
    let mut out = String::new();
    out.push_str("# Imported from an HLINPUT1 route; tick counts are 20 Hz gameplay ticks.\n");
    for _ in 0..segments {
        let name = |s: &[u8]| String::from_utf8_lossy(s).trim_end_matches('\0').to_string();
        let map = name(&d[o..o + 16]);
        let next = name(&d[o + 16..o + 32]);
        let runs = u32_at(o + 36);
        o += 48;
        let _ = writeln!(out, "\nmap {map} next {next}");
        for _ in 0..runs {
            let dur = u16_at(o);
            let (f, s, t, l) = (d[o + 2] as i8, d[o + 3] as i8, d[o + 4] as i8, d[o + 5] as i8);
            let act = u16_at(o + 6);
            o += 8;
            let mut line = format!("  {dur}");
            for (k, v) in [("f", f), ("s", s), ("t", t), ("l", l)] {
                if v != 0 {
                    let _ = write!(line, " {k}{v}");
                }
            }
            for (bit, name) in [
                (0, "attack"),
                (1, "jump"),
                (2, "duck"),
                (3, "use"),
                (4, "attack2"),
                (5, "reload"),
                (6, "next"),
                (7, "prev"),
                (8, "flash"),
            ] {
                if act & (1 << bit) != 0 {
                    let _ = write!(line, " {name}");
                }
            }
            let _ = writeln!(out, "{line}");
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Compilation

fn write_tape(path: &Path, samples: &[Sample]) -> Result<(), String> {
    let mut b = Vec::with_capacity(16 + samples.len() * 6);
    b.extend_from_slice(b"PXITAPE2");
    b.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    let mut note = Vec::new();
    for s in samples {
        b.extend_from_slice(&encode(*s, &mut note)?);
    }
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    fs::write(path, b).map_err(|e| format!("{}: {e}", path.display()))
}

struct Opts {
    route: PathBuf,
    /// Repo-relative directory the tapes are written to and referenced from.
    tape_dir: String,
    repo: PathBuf,
    out: PathBuf,
    disc: String,
    symbols: String,
    /// Symbol/address of the cooked map header's identifying word.
    header: String,
    /// Linker map used to bind symbols the gate's own loader cannot read
    /// (the map prints `NAME (.0)` for split statics) as fixed addresses.
    map: Option<PathBuf>,
    until: Option<String>,
}

/// Viewmodel index (`WeaponDef::wm`) by weapon name: `VM_LIVE_WM` holds it once that weapon is drawn.
const WEAPON_VIEWMODELS: [(&str, u8); 14] = [
    ("crowbar", 4),
    ("glock", 0),
    ("357", 1),
    ("mp5", 2),
    ("shotgun", 12),
    ("crossbow", 3),
    ("rpg", 9),
    ("gauss", 6),
    ("egon", 5),
    ("hornet", 8),
    ("grenade", 7),
    ("snark", 13),
    ("tripmine", 14),
    ("satchel", 10),
];

/// DefaultDeploy blocks attacks for 0.5 s after a weapon is drawn.
const WEAPON_DEPLOY_POLLS: u32 = 14;

/// One gate step that holds a stick and buttons until a condition holds (`prefix` is `[[step]]` or `[[block.step]]`).
fn hold_toml(prefix: &str, sample: &Sample, until: &str, timeout: u32) -> Result<String, String> {
    let e = encode(*sample, &mut Vec::new())?;
    let mut press: Vec<&str> = Vec::new();
    for bit in [BTN_SQUARE, BTN_CROSS, BTN_TRIANGLE, BTN_R2, BTN_L2, BTN_CIRCLE, BTN_R1, BTN_L1, BTN_L3] {
        if sample.buttons & bit != 0 {
            press.push(match bit {
                BTN_SQUARE => "square",
                BTN_CROSS => "cross",
                BTN_TRIANGLE => "triangle",
                BTN_R2 => "r2",
                BTN_L2 => "l2",
                BTN_CIRCLE => "circle",
                BTN_R1 => "r1",
                BTN_L1 => "l1",
                _ => "l3",
            });
        }
    }
    let _ = button_name(0);
    Ok(format!(
        "\n{prefix}\nlstick = [{}, {}]\nrstick = [{}, {}]\npress = {press:?}\nhold_until = {until}\ntimeout = {}\n",
        e[4],
        e[5],
        e[2],
        e[3],
        timeout * GATE_TICKS_PER_POLL
    ))
}

/// The pause menu's Save Game to Card 1 Manual (the steps `save-repin.toml` proves), then the card
/// image is persisted into the report folder.
const SAVE_STEPS: &str = "
[[step]]
label = \"pause and open Save Game\"
press = [\"start\"]
hold = 8
wait = 25

[[step]]
press = [\"down\"]
hold = 8
wait = 12

[[step]]
press = [\"cross\"]
hold = 8
wait = 35

[[step]]
label = \"save to Card 1 Manual\"
press = [\"cross\"]
hold = 8
wait = 400

[[step]]
checkpoint = \"game-saved\"
golden = false
[[step.assert]]
kind = \"card_writes\"
min_count = 5

[[step]]
card = { action = \"snapshot\" }
";

fn compile(o: &Opts) -> Result<(), String> {
    let text = fs::read_to_string(&o.route).map_err(|e| format!("{}: {e}", o.route.display()))?;
    let mut route = parse_route(&text)?;
    if let Some(until) = &o.until {
        let at = route.blocks.iter().position(|b| &b.map == until).ok_or_else(|| format!("no block {until}"))?;
        route.blocks.truncate(at + 1);
    }
    if let Some(bi) = route
        .blocks
        .iter()
        .position(|b| b.items.iter().any(|i| matches!(i, Item::Stop)))
    {
        route.blocks.truncate(bi + 1);
        let b = &mut route.blocks[bi];
        let at = b.items.iter().position(|i| matches!(i, Item::Stop)).unwrap();
        b.items.truncate(at);
    }
    let jv = |k: &str| route.journey.iter().find(|(n, _)| n == k).map(|(_, v)| v.trim_matches('"').to_string());
    let name = jv("name").ok_or("@journey needs name=")?;
    let title = jv("title").unwrap_or_else(|| name.clone());
    let mut t = String::new();
    let _ = writeln!(t, "# Generated by tests/chain/hlroute.rs from {}.", o.route.file_name().unwrap().to_string_lossy());
    let _ = writeln!(t, "# Controller input only: no state is written, no map is forced.");
    let _ = writeln!(t, "name = {name:?}\ntitle = {title:?}");
    let _ = writeln!(t, "disc = [{:?}]\nsymbols = [{:?}]", o.disc, o.symbols);
    let _ = writeln!(t, "pad = \"analog\"\ncard_observe = true");
    let _ = writeln!(t, "max_ticks = {}", jv("max_ticks").unwrap_or("400000".into()));
    for (k, v) in &route.journey {
        if k == "memcard" {
            let _ = writeln!(t, "memcard = {v:?}");
        }
    }
    if let Some(card) = jv("card_fixture") {
        let sha = jv("card_sha256").ok_or("card_fixture needs card_sha256")?;
        let _ = writeln!(t, "[card_fixture]\npath = {card:?}\nsha256 = {sha:?}");
    }
    if let Some(prologue) = jv("prologue") {
        // `new_game`: wait out the boot and press Cross on the title.
        if prologue == "new_game" || prologue == "hazard" || prologue == "segment" || prologue == "load" {
            // A disc with the intro stage opens with the studio splash and then the
            // movie, each skipped with Start; a disc without them ignores the presses.
            t.push_str("\n[[step]]\nlabel = \"skip the studio splash and the startup movie, if the disc has them\"\nwait = 100\n");
            t.push_str("\n[[step]]\npress = [\"start\"]\nhold = 8\nwait = 62\n");
            t.push_str("\n[[step]]\npress = [\"start\"]\nhold = 8\nwait = 200\n");
            t.push_str("\n[[step]]\nlabel = \"boot to the title\"\nwait = 620\n");
        }
        // `hazard`: the title's Hazard Course row, then the map list; `hazard_row=N` moves
        // down N rows (0 is the first Hazard Course map).
        if prologue == "hazard" {
            t.push_str("\n[[step]]\nlabel = \"title: open Hazard Course map select\"\npress = [\"down\"]\nhold = 8\nwait = 12\n");
            t.push_str("\n[[step]]\npress = [\"cross\"]\nhold = 8\nwait = 25\n");
            let rows: u32 = jv("hazard_row").and_then(|v| v.parse().ok()).unwrap_or(0);
            if rows > 0 {
                let _ = writeln!(t, "\n[[step]]\nlabel = \"select Hazard Course map row {rows}\"\npress = [\"down\"]\nhold = 8\nwait = 12\nrepeat = {rows}");
            }
            t.push_str("\n[[step]]\nlabel = \"enter the selected Hazard Course map\"\npress = [\"cross\"]\nhold = 8\nwait = 12\n");
        }
        // `segment`: authoring aid, the Debug page's diagnostic Segment select launch;
        // `segment_row=N` is the row in play order. Never part of a shipping chain journey.
        if prologue == "segment" {
            let rows: u32 = jv("segment_row").and_then(|v| v.parse().ok()).unwrap_or(0);
            t.push_str("\n[[step]]\nlabel = \"open Options\"\npress = [\"down\"]\nhold = 8\nwait = 12\nrepeat = 2\n");
            t.push_str("\n[[step]]\npress = [\"cross\"]\nhold = 8\nwait = 25\n");
            t.push_str("\n[[step]]\nlabel = \"wrap to Debug\"\npress = [\"up\"]\nhold = 8\nwait = 12\n");
            t.push_str("\n[[step]]\npress = [\"cross\"]\nhold = 8\nwait = 25\n");
            t.push_str("\n[[step]]\nlabel = \"select Segment select\"\npress = [\"up\"]\nhold = 8\nwait = 12\nrepeat = 2\n");
            t.push_str("\n[[step]]\npress = [\"cross\"]\nhold = 8\nwait = 25\n");
            if rows > 0 {
                let _ = writeln!(t, "\n[[step]]\nlabel = \"scroll to row {rows}\"\npress = [\"down\"]\nhold = 8\nwait = 12\nrepeat = {rows}");
            }
            t.push_str("\n[[step]]\nlabel = \"launch the diagnostic segment\"\npress = [\"cross\"]\nhold = 8\nwait = 12\n");
        }
        // `load`: resume from a save card (`card_fixture=` + `card_sha256=`): the title's Load game row.
        // The first block's map wait then confirms what the card restored.
        if prologue == "load" {
            t.push_str("\n[[step]]\nlabel = \"title: Load game\"\npress = [\"down\"]\nhold = 8\nwait = 12\nrepeat = 3\n");
            t.push_str("\n[[step]]\npress = [\"cross\"]\nhold = 8\nwait = 60\n");
        }
        if prologue == "new_game" {
            // `skill=easy|hard`: Left/Right on the New game row first (Medium is the default).
            let (skill, key) = match jv("skill").as_deref() {
                Some("easy") => ("Easy", "left"),
                Some("hard") => ("Hard", "right"),
                _ => ("default", ""),
            };
            if !key.is_empty() {
                let _ = writeln!(t, "\n[[step]]\nlabel = \"title: choose {skill}\"\npress = [\"{key}\"]\nhold = 8\nwait = 20");
            }
            let _ = writeln!(t, "\n[[step]]\nlabel = \"title: New Game, {skill} skill\"\npress = [\"cross\"]\nhold = 12\nwait = 24");
        }
    }
    let mut total_samples = 0usize;
    let mut card_dirty = false;
    let mut loop_n = 0;
    let mut ids = std::collections::HashMap::new();
    let ids_text = fs::read_to_string(o.repo.join("tests/chain/map-ids.txt")).unwrap_or_default();
    for line in ids_text.lines().filter(|l| !l.starts_with('#')) {
        if let Some((m, v)) = line.split_once(' ') {
            ids.insert(m.to_string(), v.trim().parse::<i64>().map_err(|e| e.to_string())?);
        }
    }
    for block in &route.blocks {
        let _ = writeln!(t, "\n# ---- {} -> {} ----", block.map, block.next);
        let id = match block.id {
            Some(id) => id,
            None => *ids
                .get(&block.map)
                .ok_or_else(|| format!("map {} has no id= and is not in map-ids.txt", block.map))?,
        };
        let _ = writeln!(t, "[[step]]\nlabel = \"{} loads\"\nwait_until = {{ kind = \"ram\", {}, size = 4, op = \"eq\", value = {id} }}\ntimeout = 9000", block.map, o.header);
        let _ = writeln!(t, "\n[[step]]\nlabel = \"{} first simulation tick\"\nwait_until = {{ kind = \"ram\", sym = \"SIM_NOW\", size = 2, op = \"lt\", value = 64 }}\ntimeout = 9000", block.map);
        // Drop the trailing neutral run: the journey waits for the next map.
        let mut items: Vec<&Item> = block.items.iter().collect();
        let mut tail_ticks = 0u32;
        while let Some(Item::Run(n, s)) = items.last() {
            if s.neutral() {
                tail_ticks += n;
                items.pop();
            } else {
                break;
            }
        }
        let mut pending: Vec<Sample> = Vec::new();
        let mut piece = 0;
        let flush = |t: &mut String, pending: &mut Vec<Sample>, piece: &mut usize, total: &mut usize| -> Result<(), String> {
            if pending.is_empty() {
                return Ok(());
            }
            let file = format!("{}/{}-{:02}.pxtape", o.tape_dir, block.map, *piece);
            compensate(pending);
            write_tape(&o.repo.join(&file), pending)?;
            let _ = writeln!(t, "\n[[step]]\nlabel = \"{} controller tape {}\"\ntape = {file:?}\ntape_rebase = true\ntimeout = {}", block.map, *piece, pending.len() as u64 * 4 + 1200);
            *total += pending.len();
            *piece += 1;
            pending.clear();
            Ok(())
        };
        for item in items {
            match item {
                Item::Run(n, s) => pending.extend(std::iter::repeat(*s).take(*n as usize)),
                Item::Checkpoint { name, asserts } => {
                    flush(&mut t, &mut pending, &mut piece, &mut total_samples)?;
                    let _ = writeln!(t, "\n[[step]]\ncheckpoint = {name:?}\ngolden = false");
                    let _ = writeln!(t, "[[step.assert]]\nkind = \"not_flat\"");
                    if !card_dirty {
                        let _ = writeln!(t, "[[step.assert]]\nkind = \"card_unchanged\"");
                    }
                    for a in asserts {
                        let _ = writeln!(t, "[[step.assert]]\n{}", split_top(a.trim_start_matches("{ ").trim_end_matches(" }")));
                    }
                }
                Item::Wait(cond, timeout) => {
                    flush(&mut t, &mut pending, &mut piece, &mut total_samples)?;
                    let _ = writeln!(t, "\n[[step]]\nwait_until = {cond}\ntimeout = {}", timeout * GATE_TICKS_PER_POLL);
                }
                Item::Hold { sample, until, timeout } => {
                    flush(&mut t, &mut pending, &mut piece, &mut total_samples)?;
                    t.push_str(&hold_toml("[[step]]", sample, until, *timeout)?);
                }
                Item::Loop { body, until, max_iter, timeout } => {
                    flush(&mut t, &mut pending, &mut piece, &mut total_samples)?;
                    loop_n += 1;
                    let _ = writeln!(t, "\n[[block]]\nname = \"round{loop_n}\"");
                    for it in body {
                        match it {
                            Item::Hold { sample, until, timeout } => t.push_str(&hold_toml("[[block.step]]", sample, until, *timeout)?),
                            Item::Run(n, s) if s.neutral() => {
                                let _ = writeln!(t, "\n[[block.step]]\nwait = {}", n * 3);
                            }
                            _ => return Err("a loop body holds only holds and neutral rests".into()),
                        }
                    }
                    let _ = writeln!(
                        t,
                        "\n[[step]]\nlabel = \"loop round{loop_n}\"\nblock = \"round{loop_n}\"\nuntil = {until}\nmax_iter = {max_iter}\ntimeout = {}",
                        timeout * GATE_TICKS_PER_POLL
                    );
                }
                Item::Stop => {}
                Item::Save => {
                    flush(&mut t, &mut pending, &mut piece, &mut total_samples)?;
                    card_dirty = true;
                    t.push_str(SAVE_STEPS);
                }
                Item::Raw(line) => {
                    flush(&mut t, &mut pending, &mut piece, &mut total_samples)?;
                    let _ = writeln!(t, "{line}");
                }
            }
        }
        flush(&mut t, &mut pending, &mut piece, &mut total_samples)?;
        let _ = writeln!(t, "# trailing wait of {tail_ticks} ticks is replaced by the next block's load wait");
    }
    if let Some(last) = route.blocks.last() {
        if !last.next.is_empty() {
            // Terminal block: nothing more to wait for.
        }
    }
    if let Some(map) = &o.map {
        let mut names = std::collections::BTreeSet::new();
        for chunk in t.split("sym").skip(1) {
            let c = chunk.trim_start().trim_start_matches('=').trim_start();
            if let Some(rest) = c.strip_prefix('"') {
                if let Some(end) = rest.find('"') {
                    names.insert(rest[..end].to_string());
                }
            }
        }
        // Symbols named inside gate expressions (`expr = "..."`): capitalised identifiers.
        for line in t.lines().filter(|l| l.contains("expr=\"")) {
            let Some(at) = line.find("expr=\"") else { continue };
            let body = &line[at + 6..];
            let mut cur = String::new();
            for c in body.chars().chain(std::iter::once(' ')) {
                if c.is_ascii_alphanumeric() || c == '_' {
                    cur.push(c);
                } else {
                    if cur.len() >= 3 && cur.chars().next().unwrap().is_ascii_uppercase() && cur.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
                        names.insert(cur.clone());
                    }
                    cur.clear();
                }
            }
        }
        let map_text = fs::read_to_string(map).map_err(|e| format!("{}: {e}", map.display()))?;
        let mut table = String::from("\n# Bound to the build this journey was written against.\n[addr]\n");
        for name in &names {
            let needle = format!("hl_psx::{name}");
            let settings_needle = format!("hl_psx::settings::{name}");
            let hit = map_text.lines().find_map(|l| {
                let tk: Vec<&str> = l.split_whitespace().collect();
                let first_is_addr = tk.first().is_some_and(|a| a.len() == 8 && u32::from_str_radix(a, 16).is_ok());
                if first_is_addr && tk.get(4).is_none() && tk.get(3).is_some_and(|x| *x == needle) {
                    return None;
                }
                if tk.len() >= 5 && (tk[4] == needle || tk[4] == settings_needle) && (tk.len() == 5 || tk[5] == "(.0)") {
                    return u32::from_str_radix(tk[0], 16).ok();
                }
                None
            });
            match hit {
                Some(a) => { let _ = writeln!(table, "{name} = {a:#x}"); }
                None => eprintln!("note: {name} not found in the map; left to the gate's loader"),
            }
        }
        // Insert the table before the first step.
        if let Some(at) = t.find("\n[[step]]") {
            t.insert_str(at, &table);
        }
    }
    fs::write(&o.out, t).map_err(|e| format!("{}: {e}", o.out.display()))?;
    println!("wrote {} ({} blocks, {} tape samples)", o.out.display(), route.blocks.len(), total_samples);
    Ok(())
}

/// One block as a single poll-bound tape (trailing neutral kept) that starts
/// at `start_poll`, for replays through the frontend's `--input-tape`.
fn block_tape(route: &Path, maps: &[String], start_poll: u32, out: &Path) -> Result<(), String> {
    let text = fs::read_to_string(route).map_err(|e| format!("{}: {e}", route.display()))?;
    let parsed = parse_route(&text)?;
    let mut samples: Vec<Sample> = Vec::new();
    for m in maps {
        let b = parsed.blocks.iter().find(|b| &b.map == m).ok_or_else(|| format!("no block {m}"))?;
        for item in &b.items {
            if let Item::Run(n, s) = item {
                samples.extend(std::iter::repeat(*s).take(*n as usize));
            }
        }
    }
    compensate(&mut samples);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"PXITAPE2");
    bytes.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&start_poll.to_le_bytes());
    let mut note = Vec::new();
    for s in &samples {
        bytes.extend_from_slice(&encode(*s, &mut note)?);
    }
    fs::write(out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote {} ({} samples from poll {start_poll})", out.display(), samples.len());
    Ok(())
}

/// Write the named blocks as an HLINPUT1 oracle route (semantic units, 20 Hz
/// ticks, trailing neutral kept, 400-tick neutral tail), so the same route text
/// can be tried against the GoldSrc reference runtime.
fn export(route: &Path, maps: &[String], out: &Path) -> Result<(), String> {
    let text = fs::read_to_string(route).map_err(|e| format!("{}: {e}", route.display()))?;
    let parsed = parse_route(&text)?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"HLINPUT1");
    bytes.extend_from_slice(&20u16.to_le_bytes());
    bytes.extend_from_slice(&(maps.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    for (mi, m) in maps.iter().enumerate() {
        let b = parsed.blocks.iter().find(|b| &b.map == m).ok_or_else(|| format!("no block {m}"))?;
        let next = if mi + 1 == maps.len() { String::new() } else { b.next.clone() };
        let mut runs: Vec<(u32, Sample)> = Vec::new();
        for item in &b.items {
            if let Item::Run(n, s) = item {
                match runs.last_mut() {
                    Some((c, last)) if last == s && *c + n <= 65535 => *c += n,
                    _ => runs.push((*n, *s)),
                }
            }
        }
        let total: u32 = runs.iter().map(|r| r.0).sum();
        let pad = |s: &str| { let mut v = s.as_bytes().to_vec(); v.resize(16, 0); v };
        bytes.extend(pad(&b.map));
        bytes.extend(pad(&next));
        bytes.extend_from_slice(&total.to_le_bytes());
        bytes.extend_from_slice(&(runs.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&400u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for (n, s) in runs {
            bytes.extend_from_slice(&(n as u16).to_le_bytes());
            bytes.extend_from_slice(&[s.forward as i8 as u8, s.strafe as i8 as u8, s.turn as i8 as u8, s.look as i8 as u8]);
            let mut act = 0u16;
            for (bit, mask) in [(0, BTN_R2), (1, BTN_CROSS), (2, BTN_TRIANGLE), (3, BTN_SQUARE), (4, BTN_L2), (5, BTN_CIRCLE), (6, BTN_R1), (7, BTN_L1), (8, BTN_L3)] {
                if s.buttons & mask != 0 { act |= 1 << bit; }
            }
            bytes.extend_from_slice(&act.to_le_bytes());
        }
    }
    fs::write(out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}

/// `hlroute verify ROUTE REPO TOML [--map P] [--disc P] [--symbols P] [--tape-dir D]`: compile the
/// route into a scratch directory and compare the journey and every tape with the committed ones,
/// byte for byte. A committed journey that no longer matches its route (an edited route or
/// compiler with the old output left behind) fails here instead of as a red chain run.
fn verify(args: &[String]) -> Result<(), String> {
    let repo = PathBuf::from(&args[3]);
    let committed = PathBuf::from(&args[4]);
    let scratch = std::env::temp_dir().join(format!("hlroute-verify-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(scratch.join("tests/chain")).map_err(|e| e.to_string())?;
    let ids = repo.join("tests/chain/map-ids.txt");
    if ids.exists() {
        fs::copy(&ids, scratch.join("tests/chain/map-ids.txt")).map_err(|e| e.to_string())?;
    }
    let mut o = Opts {
        route: PathBuf::from(&args[2]),
        tape_dir: "tests/chain/tapes".into(),
        repo: scratch.clone(),
        out: scratch.join("out.toml"),
        disc: "dist/hl-psx.cue".into(),
        symbols: ".hlpsx/hl-psx.map".into(),
        header: "sym = \"WORLD_ARENA\", offset = 4".into(),
        map: None,
        until: None,
    };
    let mut i = 5;
    while i + 1 < args.len() {
        match args[i].as_str() {
            "--disc" => o.disc = args[i + 1].clone(),
            "--symbols" => o.symbols = args[i + 1].clone(),
            "--tape-dir" => o.tape_dir = args[i + 1].clone(),
            "--map" => o.map = Some(PathBuf::from(&args[i + 1])),
            "--until" => o.until = Some(args[i + 1].clone()),
            _ => {}
        }
        i += 2;
    }
    compile(&o)?;
    let mut stale: Vec<String> = Vec::new();
    let fresh = fs::read(&o.out).map_err(|e| e.to_string())?;
    if fs::read(&committed).map_err(|e| format!("{}: {e}", committed.display()))? != fresh {
        stale.push(committed.display().to_string());
    }
    let tapes = scratch.join(&o.tape_dir);
    let mut names: Vec<_> = fs::read_dir(&tapes).map(|d| d.filter_map(|e| e.ok()).collect()).unwrap_or_default();
    names.sort_by_key(|e| e.file_name());
    for e in names {
        let have = repo.join(&o.tape_dir).join(e.file_name());
        if fs::read(&have).ok() != fs::read(e.path()).ok() {
            stale.push(have.display().to_string());
        }
    }
    let _ = fs::remove_dir_all(&scratch);
    if stale.is_empty() {
        println!("hlroute: {} matches its route", committed.display());
        Ok(())
    } else {
        Err(format!("stale against the route, regenerate with `hlroute build`: {}", stale.join(", ")))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("export") if args.len() >= 5 => {
            let maps: Vec<String> = args[3].split(',').map(String::from).collect();
            export(Path::new(&args[2]), &maps, Path::new(&args[4]))
        }
        Some("tape") if args.len() >= 6 => {
            let maps: Vec<String> = args[3].split(',').map(String::from).collect();
            block_tape(Path::new(&args[2]), &maps, args[4].parse().unwrap_or(0), Path::new(&args[5]))
        }
        Some("verify") if args.len() >= 5 => verify(&args),
        Some("import") if args.len() == 3 => import(Path::new(&args[2])).map(|s| print!("{s}")),
        Some("build") if args.len() >= 5 => {
            let mut o = Opts {
                route: PathBuf::from(&args[2]),
                tape_dir: "tests/chain/tapes".into(),
                repo: PathBuf::from(&args[3]),
                out: PathBuf::from(&args[4]),
                disc: "dist/hl-psx.cue".into(),
                symbols: ".hlpsx/hl-psx.map".into(),
                header: "sym = \"WORLD_ARENA\", offset = 4".into(),
                map: None,
                until: None,
            };
            let mut i = 5;
            while i + 1 < args.len() {
                match args[i].as_str() {
                    "--disc" => o.disc = args[i + 1].clone(),
                    "--symbols" => o.symbols = args[i + 1].clone(),
                    "--tape-dir" => o.tape_dir = args[i + 1].clone(),
                    "--map" => o.map = Some(PathBuf::from(&args[i + 1])),
                    "--until" => o.until = Some(args[i + 1].clone()),
                    _ => {}
                }
                i += 2;
            }
            compile(&o)
        }
        _ => Err("usage: hlroute import FILE.hlinput | hlroute verify ROUTE REPO OUT.toml | hlroute build ROUTE REPO OUT.toml [--disc P] [--symbols P] [--tape-dir D]".into()),
    };
    if let Err(e) = result {
        eprintln!("hlroute: {e}");
        std::process::exit(1);
    }
}

// SPDX-License-Identifier: GPL-2.0-or-later
//! `hl-aicmp summary OUT...` prints the fight metrics of each oracle trace; `hl-aicmp envelope OUT...` prints the
//! [min, max] of every metric over the runs (the parity envelope for a fight). The oracle is the instrumented
//! xash reference (`HLREF|ai|`, `HLREF|player_hit|`, `HLREF|entity|` lines); see hl-ai/DESIGN.md section 6.

use std::collections::BTreeMap;
use std::fs;

/// Ticks at which positions and cumulative damage are sampled.
const SAMPLES: [u32; 4] = [50, 100, 200, 400];

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            for d in it.by_ref() {
                if d == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn fields(line: &str) -> BTreeMap<&str, &str> {
    line.split('|').filter_map(|f| f.split_once('=')).collect()
}

/// Metrics of one run: name -> value.
type Metrics = BTreeMap<String, f64>;

/// One monster at one map tick, from either source.
struct Rec {
    tick: u32,
    mon: String,
    sched: String,
    squad: bool,
    slot: i64,
    pos: (f64, f64),
}

/// A run: monster records and the player's hits `(tick, damage)`.
#[derive(Default)]
struct Run {
    recs: Vec<Rec>,
    hits: Vec<(u32, f64)>,
}

/// The oracle's trace of class `class`: `HLREF|ai|` records and `HLREF|player_hit|` lines, on map ticks.
fn parse_oracle(text: &str, class: &str, edicts: &[String]) -> Run {
    let mut run = Run::default();
    let mut tick_base: Option<u32> = None;
    for raw in text.lines() {
        let line = strip_ansi(raw);
        if let Some(rest) = line.split("HLREF|player_hit|").nth(1) {
            let f = fields(rest);
            if let (Some(t), Some(d)) = (f.get("tick"), f.get("damage")) {
                run.hits.push((t.parse().unwrap_or(0), d.parse().unwrap_or(0.0)));
            }
        } else if let Some(rest) = line.split("HLREF|ai|").nth(1) {
            let f = fields(rest);
            if f.get("class") != Some(&class)
                || (!edicts.is_empty() && !f.get("edict").is_some_and(|e| edicts.iter().any(|x| x == e)))
            {
                continue;
            }
            let (Some(t), Some(e)) = (f.get("tick"), f.get("edict")) else { continue };
            let t: u32 = t.parse().unwrap_or(0);
            let base = *tick_base.get_or_insert(t);
            run.recs.push(Rec {
                tick: t - base,
                mon: e.to_string(),
                sched: f.get("schedule").copied().unwrap_or("").to_string(),
                squad: f.get("squad") == Some(&"1"),
                slot: f.get("slot").and_then(|s| s.parse().ok()).unwrap_or(0),
                pos: (
                    f.get("x").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                    f.get("y").and_then(|v| v.parse().ok()).unwrap_or(0.0),
                ),
            });
        }
    }
    // The hits are on global ticks: bring them to map ticks like the records.
    if let Some(b) = tick_base {
        for h in &mut run.hits {
            h.0 = h.0.saturating_sub(b);
        }
    }
    run
}

fn sgn(v: u64) -> i64 {
    v as u32 as i32 as i64
}

/// The port's tick log (`psoxide-gate --tick-log` with the columns `tlog.sh` writes): `sim`, `dmg`, and per
/// monster `r<row>w0..w4` (the pool row, 36 bytes at `monster_ai::AI`) and `p<prop>x`, `p<prop>z`.
/// `mons` is `(row, prop)` pairs; `names` maps schedule ids to names.
fn parse_port(text: &str, mons: &[(u32, u32)], names: &[String]) -> Run {
    let mut run = Run::default();
    let mut lines = text.lines();
    let Some(head) = lines.next() else { return run };
    let cols: Vec<&str> = head.split(',').collect();
    let col = |n: &str| cols.iter().position(|c| *c == n);
    let (Some(sim), Some(dmg)) = (col("sim"), col("dmg")) else { return run };
    let mut last_sim: Option<u32> = None;
    let mut last_dmg = 0u32;
    for line in lines {
        let v: Vec<u64> = line.split(',').map(|x| x.trim().parse().unwrap_or(0)).collect();
        let s = (v[sim] & 0xffff) as u32;
        if last_sim == Some(s) {
            continue; // several video frames per sim tick
        }
        last_sim = Some(s);
        let d = v[dmg] as u32;
        if d > last_dmg {
            run.hits.push((s, (d - last_dmg) as f64));
            last_dmg = d;
        }
        for &(row, prop) in mons {
            let w = |k: u32| col(&format!("r{row}w{k}")).map_or(0, |c| v[c] as u32);
            let (w1, w4) = (w(1), w(4));
            let sched_id = (w1 & 0xff) as usize;
            let squad = (w4 >> 24) != 0;
            let slot = (w4 & 0xffff) as i64;
            let x = col(&format!("p{prop}x")).map_or(0, |c| sgn(v[c])) as f64;
            let z = col(&format!("p{prop}z")).map_or(0, |c| sgn(v[c])) as f64;
            run.recs.push(Rec {
                tick: s,
                mon: format!("r{row}"),
                sched: names.get(sched_id).cloned().unwrap_or_default(),
                squad,
                slot,
                pos: (x, z),
            });
        }
    }
    run
}

fn analyse(run: &Run) -> Metrics {
    let mut m = Metrics::new();
    let mut last: BTreeMap<String, String> = BTreeMap::new();
    let mut first_sched: BTreeMap<String, u32> = BTreeMap::new();
    let mut transitions = 0u32;
    let mut squad_tick: Option<u32> = None;
    let mut engage_grants = 0u32;
    let mut last_slot: BTreeMap<String, i64> = BTreeMap::new();
    let mut pos: BTreeMap<(String, u32), (f64, f64)> = BTreeMap::new();
    let mut monsters: Vec<String> = Vec::new();
    for r in &run.recs {
        if !monsters.contains(&r.mon) {
            monsters.push(r.mon.clone());
        }
        if last.get(&r.mon) != Some(&r.sched) {
            last.insert(r.mon.clone(), r.sched.clone());
            transitions += 1;
            let e = first_sched.entry(r.sched.clone()).or_insert(u32::MAX);
            *e = (*e).min(r.tick);
        }
        if squad_tick.is_none() && r.squad {
            squad_tick = Some(r.tick);
        }
        if r.slot > 0 && last_slot.get(&r.mon) != Some(&r.slot) {
            engage_grants += 1;
        }
        last_slot.insert(r.mon.clone(), r.slot);
        if SAMPLES.contains(&r.tick) {
            pos.insert((r.mon.clone(), r.tick), r.pos);
        }
    }
    m.insert("monsters".into(), monsters.len() as f64);
    if let Some(t) = squad_tick {
        m.insert("squad_formed_tick".into(), t as f64);
    }
    m.insert("first_hit_tick".into(), run.hits.first().map_or(-1.0, |h| h.0 as f64));
    for s in SAMPLES {
        let dmg: f64 = run.hits.iter().filter(|h| h.0 <= s).map(|h| h.1).sum();
        m.insert(format!("damage_by_{s}"), dmg);
    }
    m.insert("engage_slot_grants".into(), engage_grants as f64);
    for (name, t) in first_sched {
        if !name.is_empty() {
            m.insert(format!("first_tick[{name}]"), t as f64);
        }
    }
    m.insert("schedule_changes".into(), transitions as f64);
    for s in SAMPLES {
        let pts: Vec<(f64, f64)> = pos.iter().filter(|((_, t), _)| *t == s).map(|(_, p)| *p).collect();
        if pts.len() >= 2 {
            let (mut sum, mut n) = (0.0, 0.0);
            for i in 0..pts.len() {
                for j in i + 1..pts.len() {
                    sum += ((pts[i].0 - pts[j].0).powi(2) + (pts[i].1 - pts[j].1).powi(2)).sqrt();
                    n += 1.0;
                }
            }
            m.insert(format!("spread_at_{s}"), sum / n);
        }
    }
    m
}

/// Tolerance added each side of the oracle's [min, max] before a port metric counts as outside: ticks for
/// timings, a fraction of the oracle's maximum for damage, units for positions.
fn tolerance(key: &str, hi: f64) -> f64 {
    if key.starts_with("damage_by") {
        0.3 * hi + 6.0
    } else if key.starts_with("spread_at") {
        150.0
    } else if key.starts_with("first_tick") || key.contains("_tick") {
        10.0 + 0.2 * hi
    } else {
        2.0
    }
}

fn le32(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// `hl-aicmp mdlseqs FILE.mdl`: every sequence of a studio model with its activity, length in 20 Hz ticks
/// and animation events (frame converted to ticks), the source for the port's activity table.
fn mdlseqs(path: &str) {
    let d = fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(2)
    });
    let (n, base) = (le32(&d, 164) as usize, le32(&d, 168) as usize);
    for i in 0..n {
        let o = base + i * 176;
        let label: String = d[o..o + 32].iter().take_while(|b| **b != 0).map(|b| *b as char).collect();
        let fps = f32::from_le_bytes([d[o + 32], d[o + 33], d[o + 34], d[o + 35]]);
        let (flags, act, nev, ev0, frames) =
            (le32(&d, o + 36), le32(&d, o + 40), le32(&d, o + 48) as usize, le32(&d, o + 52) as usize, le32(&d, o + 56));
        let ticks = if frames > 1 && fps > 0.0 { (frames - 1) as f32 / fps * 20.0 } else { 0.0 };
        let mut ev = String::new();
        for k in 0..nev {
            let e = ev0 + k * 76;
            let t = if frames > 1 { le32(&d, e) as f32 / (frames - 1) as f32 * ticks } else { 0.0 };
            ev.push_str(&format!(" ev{}@{:.1}t", le32(&d, e + 4), t));
        }
        println!("{i:3} {label:<18} act={act:<3} fps={fps:<5} frames={frames:<4} loop={} ticks={ticks:.2}{ev}", flags & 1);
    }
}

fn read_text(f: &str) -> String {
    fs::read_to_string(f)
        .or_else(|_| fs::read(f).map(|b| String::from_utf8_lossy(&b).into_owned()))
        .unwrap_or_default()
}

fn envelope(runs: &[(String, Metrics)]) -> BTreeMap<String, (f64, f64, usize)> {
    let mut out = BTreeMap::new();
    let mut keys: Vec<&String> = runs.iter().flat_map(|(_, m)| m.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        let vals: Vec<f64> = runs.iter().filter_map(|(_, m)| m.get(k).copied()).collect();
        let (lo, hi) = vals.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        out.insert(k.clone(), (lo, hi, vals.len()));
    }
    out
}

const USAGE: &str = "usage: hl-aicmp summary|envelope OUT... [--class C]\n       hl-aicmp mdlseqs FILE.mdl\n       \
    hl-aicmp check TICKS.csv --mon ROW:PROP... [--species grunt] [--timeline] [--edicts 96,97,98] --oracle OUT... [--class C]";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "mdlseqs" {
        mdlseqs(&args[2]);
        return;
    }
    if args.len() < 3 {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let mut class = "monster_human_grunt".to_string();
    let mut species = "grunt".to_string();
    let mut files = Vec::new();
    let mut oracle = Vec::new();
    let mut mons: Vec<(u32, u32)> = Vec::new();
    let mut i = 2;
    let mut in_oracle = false;
    let mut edicts: Vec<String> = Vec::new();
    let mut timeline = false;
    while i < args.len() {
        match args[i].as_str() {
            "--edicts" if i + 1 < args.len() => {
                edicts = args[i + 1].split(',').map(str::to_string).collect();
                i += 2;
            }
            "--timeline" => {
                timeline = true;
                i += 1;
            }
            "--class" if i + 1 < args.len() => {
                class = args[i + 1].clone();
                i += 2;
            }
            "--species" if i + 1 < args.len() => {
                species = args[i + 1].clone();
                i += 2;
            }
            "--mon" if i + 1 < args.len() => {
                let (r, p) = args[i + 1].split_once(':').unwrap_or(("0", "0"));
                mons.push((r.parse().unwrap_or(0), p.parse().unwrap_or(0)));
                i += 2;
            }
            "--oracle" => {
                in_oracle = true;
                i += 1;
            }
            f => {
                if in_oracle {
                    oracle.push(f.to_string());
                } else {
                    files.push(f.to_string());
                }
                i += 1;
            }
        }
    }
    let oracle_runs = |list: &[String]| -> Vec<(String, Metrics)> {
        list.iter().map(|f| (f.clone(), analyse(&parse_oracle(&read_text(f), &class, &edicts)))).collect()
    };
    match args[1].as_str() {
        "summary" => {
            for (f, m) in oracle_runs(&files) {
                println!("== {f}");
                for (k, v) in m {
                    println!("  {k} = {v:.1}");
                }
            }
        }
        "envelope" => {
            let runs = oracle_runs(&files);
            println!("envelope over {} runs", runs.len());
            for (k, (lo, hi, n)) in envelope(&runs) {
                println!("  {k}: [{lo:.1}, {hi:.1}] seen in {n}/{} runs", runs.len());
            }
        }
        "check" => {
            let names = hl_sched::shipped().and_then(|s| s.names(&[species.as_str()])).unwrap_or_default();
            let port_run = parse_port(&read_text(&files[0]), &mons, &names);
            if timeline {
                let mut last: BTreeMap<String, String> = BTreeMap::new();
                for r in &port_run.recs {
                    if last.get(&r.mon) != Some(&r.sched) {
                        last.insert(r.mon.clone(), r.sched.clone());
                        println!("t{:>4} {} {:<28} slot {} pos ({:.0},{:.0})", r.tick, r.mon, r.sched, r.slot, r.pos.0, r.pos.1);
                    }
                }
                for h in &port_run.hits {
                    println!("t{:>4} hit {}", h.0, h.1);
                }
            }
            let port = analyse(&port_run);
            let env = envelope(&oracle_runs(&oracle));
            let mut failed = 0;
            for (k, v) in &port {
                match env.get(k) {
                    Some((lo, hi, _)) => {
                        let tol = tolerance(k, *hi);
                        let ok = *v >= lo - tol && *v <= hi + tol;
                        failed += !ok as u32;
                        println!("{} {k}: port {v:.1} oracle [{lo:.1}, {hi:.1}] tolerance {tol:.1}", if ok { "ok  " } else { "FAIL" });
                    }
                    None => println!("note {k}: port {v:.1} (not in the oracle)"),
                }
            }
            for (k, (lo, hi, _)) in &env {
                if !port.contains_key(k) && !k.starts_with("first_tick") {
                    println!("FAIL {k}: missing in the port, oracle [{lo:.1}, {hi:.1}]");
                    failed += 1;
                }
            }
            for (k, (lo, hi, _)) in &env {
                if k.starts_with("first_tick") && !port.contains_key(k) {
                    println!("miss {k}: oracle [{lo:.1}, {hi:.1}] never ran in the port");
                }
            }
            println!("{failed} metric(s) outside the envelope");
            if failed != 0 {
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("unknown command {other}\n{USAGE}");
            std::process::exit(2);
        }
    }
}

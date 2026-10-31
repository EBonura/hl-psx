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

fn analyse(text: &str, class: &str) -> Metrics {
    let mut m = Metrics::new();
    let mut hits: Vec<(u32, f64)> = Vec::new();
    // monster edict -> (last schedule, schedule start ticks by name)
    let mut last: BTreeMap<String, String> = BTreeMap::new();
    let mut first_sched: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut transitions: BTreeMap<String, u32> = BTreeMap::new();
    let mut squad_tick: Option<u32> = None;
    let mut engage_grants = 0u32;
    let mut last_slot: BTreeMap<String, i64> = BTreeMap::new();
    let mut pos: BTreeMap<(String, u32), (f64, f64)> = BTreeMap::new();
    let mut monsters: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = strip_ansi(raw);
        if let Some(rest) = line.split("HLREF|player_hit|").nth(1) {
            let f = fields(rest);
            if let (Some(t), Some(d)) = (f.get("tick"), f.get("damage")) {
                hits.push((t.parse().unwrap_or(0), d.parse().unwrap_or(0.0)));
            }
        } else if let Some(rest) = line.split("HLREF|ai|").nth(1) {
            let f = fields(rest);
            if f.get("class") != Some(&class) {
                continue;
            }
            let (Some(t), Some(e)) = (f.get("tick"), f.get("edict")) else { continue };
            let t: u32 = t.parse().unwrap_or(0);
            let e = e.to_string();
            if !monsters.contains(&e) {
                monsters.push(e.clone());
            }
            let sched = f.get("schedule").copied().unwrap_or("").to_string();
            if last.get(&e) != Some(&sched) {
                last.insert(e.clone(), sched.clone());
                *transitions.entry(e.clone()).or_insert(0) += 1;
                first_sched.entry((e.clone(), sched)).or_insert(t);
            }
            if squad_tick.is_none() && f.get("squad") == Some(&"1") {
                squad_tick = Some(t);
            }
            let slot: i64 = f.get("slot").and_then(|s| s.parse().ok()).unwrap_or(0);
            if slot > 0 && last_slot.get(&e) != Some(&slot) {
                engage_grants += 1;
            }
            last_slot.insert(e.clone(), slot);
            if SAMPLES.contains(&t) {
                let x = f.get("x").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let y = f.get("y").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                pos.insert((e, t), (x, y));
            }
        }
    }
    m.insert("monsters".into(), monsters.len() as f64);
    if let Some(t) = squad_tick {
        m.insert("squad_formed_tick".into(), t as f64);
    }
    m.insert("first_hit_tick".into(), hits.first().map_or(-1.0, |h| h.0 as f64));
    for s in SAMPLES {
        let dmg: f64 = hits.iter().filter(|h| h.0 <= s).map(|h| h.1).sum();
        m.insert(format!("damage_by_{s}"), dmg);
    }
    m.insert("engage_slot_grants".into(), engage_grants as f64);
    let mut names: BTreeMap<String, u32> = BTreeMap::new();
    for ((_, name), t) in &first_sched {
        let e = names.entry(name.clone()).or_insert(u32::MAX);
        *e = (*e).min(*t);
    }
    for (name, t) in names {
        if !name.is_empty() {
            m.insert(format!("first_tick[{name}]"), t as f64);
        }
    }
    m.insert("schedule_changes".into(), transitions.values().sum::<u32>() as f64);
    // Position spread at each sample: mean pairwise distance of the monsters (invariant to who is who).
    for s in SAMPLES {
        let pts: Vec<(f64, f64)> = pos.iter().filter(|((_, t), _)| *t == s).map(|(_, p)| *p).collect();
        if pts.len() >= 2 {
            let mut sum = 0.0;
            let mut n = 0.0;
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: hl-aicmp summary|envelope OUT... [--class monster_human_grunt]");
        std::process::exit(2);
    }
    let mut class = "monster_human_grunt".to_string();
    let mut files = Vec::new();
    let mut i = 2;
    while i < args.len() {
        if args[i] == "--class" && i + 1 < args.len() {
            class = args[i + 1].clone();
            i += 2;
        } else {
            files.push(args[i].clone());
            i += 1;
        }
    }
    let runs: Vec<(String, Metrics)> = files
        .iter()
        .map(|f| {
            let text = fs::read_to_string(f).or_else(|_| fs::read(f).map(|b| String::from_utf8_lossy(&b).into_owned())).unwrap_or_default();
            (f.clone(), analyse(&text, &class))
        })
        .collect();
    match args[1].as_str() {
        "summary" => {
            for (f, m) in &runs {
                println!("== {f}");
                for (k, v) in m {
                    println!("  {k} = {v:.1}");
                }
            }
        }
        "envelope" => {
            let mut keys: Vec<&String> = runs.iter().flat_map(|(_, m)| m.keys()).collect();
            keys.sort();
            keys.dedup();
            println!("envelope over {} runs", runs.len());
            for k in keys {
                let vals: Vec<f64> = runs.iter().filter_map(|(_, m)| m.get(k).copied()).collect();
                let (lo, hi) = vals.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
                println!("  {k}: [{lo:.1}, {hi:.1}] seen in {}/{} runs", vals.len(), runs.len());
            }
        }
        other => {
            eprintln!("unknown command {other}");
            std::process::exit(2);
        }
    }
}

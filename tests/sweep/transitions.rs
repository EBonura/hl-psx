// SPDX-License-Identifier: GPL-2.0-or-later
//! Changelevel transition sweep: probe generator and report.
//!
//! For exit K of every map, `gen` writes an interaction-probe section that stands the player on
//! a free hull spot inside the K-th `trigger_changelevel` volume (`tpfree`, biased toward the
//! source landmark) and a table with the landmark arithmetic the arrival must follow.
//! `report` reads the reduced traces of those runs and prints one verdict line per source map.
//!
//!     rustc --edition=2021 -O tests/sweep/transitions.rs -o /tmp/transitions
//!     /tmp/transitions gen <valve/maps dir> <maps.txt> <out dir> <K>   # out/tr-K.txt, out/tr-K.tsv
//!     /tmp/transitions report <out/tr-K.tsv> <dir of reduced traces, one MAP.txt per source map>
//!
//! `maps.txt` lists the map names in room order (game/src/map_catalog.rs MAPS), whitespace
//! separated. A reduced trace keeps the `tick`, `probe` and changelevel/map_start event lines of
//! one run (see README.md).

use std::collections::{BTreeMap, HashMap};

type Ent = BTreeMap<String, String>;

fn rd(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn rf(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn load(path: &str) -> (Vec<u8>, Vec<Ent>) {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let (eo, el) = (rd(&b, 4) as usize, rd(&b, 8) as usize);
    let text = String::from_utf8_lossy(&b[eo..eo + el])
        .trim_end_matches('\0')
        .to_string();
    let mut out = Vec::new();
    let mut cur = Ent::new();
    for line in text.lines() {
        let l = line.trim();
        if l == "{" {
            cur = Ent::new();
        } else if l == "}" {
            out.push(cur.clone());
        } else {
            let p: Vec<&str> = l.split('"').collect();
            if p.len() >= 4 {
                cur.insert(p[1].to_string(), p[3].to_string());
            }
        }
    }
    (b, out)
}

fn origin(e: &Ent) -> [f32; 3] {
    let v: Vec<f32> = e
        .get("origin")
        .map(|s| {
            s.split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect()
        })
        .unwrap_or_default();
    if v.len() == 3 {
        [v[0], v[1], v[2]]
    } else {
        [0.0; 3]
    }
}

fn landmark(ents: &[Ent], name: &str) -> Option<[f32; 3]> {
    ents.iter()
        .find(|e| {
            e.get("classname").map(String::as_str) == Some("info_landmark")
                && e.get("targetname").map(String::as_str) == Some(name)
        })
        .map(origin)
}

fn model_bounds(bsp: &[u8], ent: &Ent) -> Option<([f32; 3], [f32; 3])> {
    let index: usize = ent.get("model")?.trim_start_matches('*').parse().ok()?;
    let o = rd(bsp, 4 + 14 * 8) as usize + index * 64;
    Some((
        [rf(bsp, o), rf(bsp, o + 4), rf(bsp, o + 8)],
        [rf(bsp, o + 12), rf(bsp, o + 16), rf(bsp, o + 20)],
    ))
}

fn generate(maps_dir: &str, maps_file: &str, out: &str, k: usize) {
    let maps: Vec<String> = std::fs::read_to_string(maps_file)
        .expect("maps file")
        .split_whitespace()
        .map(String::from)
        .collect();
    let cache: BTreeMap<String, (Vec<u8>, Vec<Ent>)> = maps
        .iter()
        .map(|m| (m.clone(), load(&format!("{maps_dir}/{m}.bsp"))))
        .collect();
    let (mut probe, mut table) = (String::new(), String::new());
    for m in &maps {
        let (bsp, ents) = &cache[m];
        let models = rd(bsp, 4 + 14 * 8) as usize;
        let exits: Vec<&Ent> = ents
            .iter()
            .filter(|e| e.get("classname").map(String::as_str) == Some("trigger_changelevel"))
            .collect();
        let Some(exit) = exits.get(k) else { continue };
        let model: usize = exit
            .get("model")
            .and_then(|s| s.trim_start_matches('*').parse().ok())
            .unwrap_or(0);
        let o = models + model * 64;
        let mn = [rf(bsp, o), rf(bsp, o + 4), rf(bsp, o + 8)];
        let mx = [rf(bsp, o + 12), rf(bsp, o + 16), rf(bsp, o + 20)];
        let dst = exit.get("map").cloned().unwrap_or_default();
        let lm = exit.get("landmark").cloned().unwrap_or_default();
        let flags: u32 = exit
            .get("spawnflags")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let name = exit.get("targetname").cloned().unwrap_or_default();
        let src_lm = landmark(ents, &lm);
        let dst_lm = cache.get(&dst).and_then(|(_, d)| landmark(d, &lm));
        probe.push_str(&format!("map {m}\n"));
        // Search outward from the source landmark: that is where a player crosses, and where the
        // destination geometry is built to match. `tpfree` also demands that the spot really
        // touches a changelevel (bounds and clip hull), standing or ducking.
        if let Some(s) = src_lm {
            probe.push_str(&format!("39 tppref {:.0} {:.0} {:.0}\n", s[0], s[1], s[2]));
        }
        // A fired changelevel first tests that the player stands in the trigger_transition
        // volume named after its landmark, so a named exit is entered through that volume.
        let transition = if name.is_empty() {
            None
        } else {
            ents.iter()
                .find(|e| {
                    e.get("classname").map(String::as_str) == Some("trigger_transition")
                        && e.get("targetname") == Some(&lm)
                })
                .and_then(|e| model_bounds(bsp, e))
        };
        match transition {
            Some((tmn, tmx)) => probe.push_str(&format!(
                "40 tpfreeany {:.0} {:.0} {:.0} {:.0} {:.0} {:.0}\n",
                tmn[0], tmn[1], tmn[2], tmx[0], tmx[1], tmx[2]
            )),
            None => probe.push_str(&format!(
                "40 tpfree {:.0} {:.0} {:.0} {:.0} {:.0} {:.0}\n",
                mn[0], mn[1], mn[2], mx[0], mx[1], mx[2]
            )),
        }
        // A named exit is also fired by name, once the player stands where the landmark expects.
        if !name.is_empty() {
            probe.push_str(&format!("44 fire {name} on\n"));
        }
        let landmarks = match (src_lm, dst_lm) {
            (Some(s), Some(d)) => format!(
                "{:.0} {:.0} {:.0}|{:.0} {:.0} {:.0}",
                s[0], s[1], s[2], d[0], d[1], d[2]
            ),
            _ => "?".into(),
        };
        table.push_str(&format!(
            "{m}\t{k}\t{dst}\t{lm}\t{flags}\t{name}\t{landmarks}\n"
        ));
    }
    std::fs::write(format!("{out}/tr-{k}.txt"), probe).expect("write probe");
    std::fs::write(format!("{out}/tr-{k}.tsv"), table).expect("write table");
}

fn kv(line: &str) -> HashMap<&str, &str> {
    line.split('|').filter_map(|p| p.split_once('=')).collect()
}

fn int(m: &HashMap<&str, &str>, key: &str) -> i32 {
    m.get(key).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn report(table: &str, dir: &str) {
    let table = std::fs::read_to_string(table).expect("table");
    for row in table.lines() {
        let f: Vec<&str> = row.split('\t').collect();
        let (src, dst, landmarks) = (f[0], f[2], f[6]);
        let Ok(text) = std::fs::read_to_string(format!("{dir}/{src}.txt")) else {
            println!("{src}\tNORUN\t{dst}");
            continue;
        };
        let (mut changed, mut change_map, mut change_tick) = (None::<String>, String::new(), 0);
        let (mut tp, mut dst_ticks): (Option<(i32, i32)>, Vec<HashMap<&str, &str>>) =
            (None, vec![]);
        for l in text.lines() {
            if l.contains("event=changelevel") && changed.is_none() {
                let m = kv(l);
                changed = m.get("next_map").map(|s| s.to_string());
                change_map = m.get("map").map(|s| s.to_string()).unwrap_or_default();
                change_tick = int(&m, "tick");
            }
            if changed.is_none() && l.contains("event=tpfree") && !l.contains("none") {
                let m = kv(l);
                tp = Some((int(&m, "a"), int(&m, "c")));
            }
            if l.contains(" tick|") {
                let m = kv(l);
                if changed.as_deref().is_some_and(|c| m.get("map") == Some(&c)) {
                    dst_ticks.push(m);
                }
            }
        }
        let Some(next) = changed else {
            let why = if text.contains("tpfree-none") {
                "NO-FREE-SPOT"
            } else {
                "NO-CHANGELEVEL"
            };
            println!("{src}\t{why}\texpect {dst}");
            continue;
        };
        let mut notes = Vec::new();
        let mut lm_dist: Option<i32> = None;
        if change_map != src || change_tick < 40 {
            notes.push(format!(
                "CHANGE-BEFORE-PROBE from {change_map} t={change_tick}"
            ));
        }
        if next != dst {
            notes.push(format!("WRONG-DEST {next}"));
        }
        let (Some(first), Some(last)) = (dst_ticks.first(), dst_ticks.last()) else {
            println!("{src}\tCHECK\t{dst}\tno destination ticks");
            continue;
        };
        if dst_ticks.len() < 30 {
            notes.push(format!("SHORT-RUN {}", dst_ticks.len()));
        }
        let parts: Vec<Vec<i32>> = landmarks
            .split('|')
            .map(|p| {
                p.split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect()
            })
            .collect();
        match (parts.as_slice(), tp) {
            ([s, d], Some((x, y))) if s.len() == 3 && d.len() == 3 => {
                let (ex, ey) = (x - s[0] + d[0], y - s[1] + d[1]);
                lm_dist = Some((x - s[0]).abs().max((y - s[1]).abs()));
                let (dx, dy) = (int(first, "px") - ex, int(first, "pz") - ey);
                if dx.abs() > 24 || dy.abs() > 24 {
                    notes.push(format!("ARRIVAL-OFF dx={dx} dy={dy}"));
                }
            }
            _ => notes.push("NO-LANDMARK-PAIR".into()),
        }
        if int(last, "ground") == 0 {
            notes.push("AIRBORNE-AT-END".into());
        }
        if int(last, "health") < int(first, "health") {
            notes.push(format!(
                "HEALTH {}->{}",
                int(first, "health"),
                int(last, "health")
            ));
        }
        let moved =
            (int(last, "px") - int(first, "px")).abs() + (int(last, "pz") - int(first, "pz")).abs();
        if moved > 200 {
            notes.push(format!("DRIFT {moved}"));
        }
        if int(last, "py") < int(first, "py") - 300 {
            notes.push(format!("FELL {}", int(first, "py") - int(last, "py")));
        }
        if lm_dist.is_some_and(|d| d > 160) {
            notes.push(format!("FAR-FROM-LANDMARK {}", lm_dist.unwrap()));
        }
        println!(
            "{src}\t{}\t{dst}\tfirst=({},{},{}) last=({},{},{})\t{}",
            if notes.is_empty() { "OK" } else { "CHECK" },
            int(first, "px"),
            int(first, "py"),
            int(first, "pz"),
            int(last, "px"),
            int(last, "py"),
            int(last, "pz"),
            notes.join(" ")
        );
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("gen") if a.len() == 6 => generate(&a[2], &a[3], &a[4], a[5].parse().expect("K")),
        Some("report") if a.len() == 4 => report(&a[2], &a[3]),
        _ => eprintln!("usage: transitions gen <maps dir> <maps.txt> <out dir> <K> | report <table.tsv> <trace dir>"),
    }
}

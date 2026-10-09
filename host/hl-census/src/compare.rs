//! Compare the retail game's target dispatch (HLREF trace) with the port's
//! (HLPSX trace) for one map, after both ran the same root-firing script.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub struct Fire {
    pub tick: i64,
    pub target: String,
    pub use_type: i32,
    /// Retail only: class that called FireTargets.
    pub caller: String,
}

fn kv(line: &str) -> BTreeMap<&str, &str> {
    line.split('|')
        .skip(2)
        .filter_map(|p| p.split_once('='))
        .collect()
}

/// Retail target fires of one map, ticks rebased to the map's first tick.
pub fn retail_fires(text: &str, map: &str) -> (Vec<Fire>, i64) {
    let mut fires = Vec::new();
    let mut base = i64::MAX;
    let mut last = 0;
    for line in text.lines() {
        let Some(i) = line.find("HLREF|") else {
            continue;
        };
        let line = &line[i..];
        let kind = line.split('|').nth(1).unwrap_or("");
        let m = kv(line);
        if m.get("map") != Some(&map) {
            continue;
        }
        if kind == "tick" {
            if let (Some(t), Some(mt)) = (m.get("tick"), m.get("map_tick")) {
                if let (Ok(t), Ok(mt)) = (t.parse::<i64>(), mt.parse::<i64>()) {
                    base = base.min(t - mt);
                    last = last.max(mt);
                }
            }
        } else if kind == "event" && m.get("event") == Some(&"target_fire") {
            fires.push(Fire {
                tick: m.get("tick").and_then(|v| v.parse().ok()).unwrap_or(0),
                target: m.get("target").unwrap_or(&"").to_string(),
                use_type: m.get("use").and_then(|v| v.parse().ok()).unwrap_or(-1),
                caller: m.get("caller_class").unwrap_or(&"").to_string(),
            });
        }
    }
    if base == i64::MAX {
        base = 0;
    }
    for f in &mut fires {
        f.tick -= base;
    }
    (fires, last)
}

/// Port target fires of one map, minus the probe's own depth-0 fires.
pub fn port_fires(text: &str, map: &str) -> (Vec<Fire>, i64) {
    let mut fires = Vec::new();
    let mut probe: BTreeSet<(i64, String)> = BTreeSet::new();
    let mut last = 0;
    let mut raw: Vec<(Fire, bool)> = Vec::new();
    for line in text.lines() {
        let Some(i) = line.find("HLPSX|") else {
            continue;
        };
        let line = &line[i..];
        let kind = line.split('|').nth(1).unwrap_or("");
        let m = kv(line);
        if m.get("map") != Some(&map) {
            continue;
        }
        let tick: i64 = m.get("tick").and_then(|v| v.parse().ok()).unwrap_or(0);
        match kind {
            "tick" => last = last.max(tick),
            "probe" if m.get("event") == Some(&"fire") => {
                probe.insert((tick, m.get("name").unwrap_or(&"").to_string()));
            }
            "event" if m.get("event") == Some(&"target_fire") => {
                raw.push((
                    Fire {
                        tick,
                        target: m.get("target").unwrap_or(&"").to_string(),
                        use_type: m.get("use").and_then(|v| v.parse().ok()).unwrap_or(-1),
                        caller: String::new(),
                    },
                    m.get("depth") == Some(&"0"),
                ));
            }
            _ => {}
        }
    }
    for (f, depth0) in raw {
        if depth0 && probe.contains(&(f.tick, f.target.clone())) {
            continue;
        }
        fires.push(f);
    }
    (fires, last)
}

#[derive(Debug)]
pub struct Row {
    pub target: String,
    pub retail_first: Option<i64>,
    pub retail_n: usize,
    pub port_first: Option<i64>,
    pub port_n: usize,
    pub caller: String,
    pub flag: String,
}

#[derive(Debug, Default)]
pub struct MapReport {
    pub offset: i64,
    pub retail_total: usize,
    pub port_total: usize,
    pub rows: Vec<Row>,
}

fn median(v: &mut Vec<i64>) -> i64 {
    if v.is_empty() {
        return 0;
    }
    v.sort();
    v[v.len() / 2]
}

pub fn compare(
    retail: &[Fire],
    retail_last: i64,
    port: &[Fire],
    port_last: i64,
    late_tol: i64,
) -> MapReport {
    let mut first_r: BTreeMap<&str, (i64, &str)> = BTreeMap::new();
    let mut first_p: BTreeMap<&str, i64> = BTreeMap::new();
    for f in retail {
        first_r.entry(&f.target).or_insert((f.tick, &f.caller));
    }
    for f in port {
        first_p.entry(&f.target).or_insert(f.tick);
    }
    let mut diffs: Vec<i64> = first_r
        .iter()
        .filter_map(|(n, (t, _))| first_p.get(n).map(|p| t - p))
        .collect();
    let offset = median(&mut diffs);
    // Count only fires inside the window both runs covered.
    let window = (retail_last - offset).min(port_last);
    let mut rc: BTreeMap<&str, usize> = BTreeMap::new();
    let mut pc: BTreeMap<&str, usize> = BTreeMap::new();
    for f in retail {
        if f.tick - offset <= window {
            *rc.entry(&f.target).or_default() += 1;
        }
    }
    for f in port {
        if f.tick <= window {
            *pc.entry(&f.target).or_default() += 1;
        }
    }
    let names: BTreeSet<&str> = first_r.keys().chain(first_p.keys()).copied().collect();
    let mut rows = Vec::new();
    for n in names {
        let r = rc.get(n).copied().unwrap_or(0);
        let p = pc.get(n).copied().unwrap_or(0);
        let rf = first_r.get(n).map(|x| x.0 - offset);
        let pf = first_p.get(n).copied();
        let flag = if r > 0 && p == 0 {
            "MISSING-IN-PORT".to_string()
        } else if p > 0 && r == 0 {
            "EXTRA-IN-PORT".to_string()
        } else if r != p {
            format!("COUNT {r}/{p}")
        } else if let (Some(a), Some(b)) = (rf, pf) {
            if (a - b).abs() > late_tol {
                format!("TIME {:+}", b - a)
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        // A multisource fired by name has no registered caller in the port;
        // only a source entity's own Use completes it. Retail's console fire
        // is accepted regardless, so these rows are probe artefacts.
        let caller = first_r.get(n).map(|x| x.1.to_string()).unwrap_or_default();
        let flag = if !flag.is_empty() && caller == "multisource" {
            format!("({flag}) via multisource")
        } else {
            flag
        };
        rows.push(Row {
            target: n.to_string(),
            retail_first: rf,
            retail_n: r,
            port_first: pf,
            port_n: p,
            caller,
            flag,
        });
    }
    rows.sort_by_key(|r| r.retail_first.or(r.port_first).unwrap_or(i64::MAX));
    MapReport {
        offset,
        retail_total: retail.len(),
        port_total: port.len(),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_extra_and_count_are_flagged() {
        let r = vec![
            Fire {
                tick: 100,
                target: "a".into(),
                use_type: 3,
                caller: "multi_manager".into(),
            },
            Fire {
                tick: 110,
                target: "b".into(),
                use_type: 3,
                caller: String::new(),
            },
            Fire {
                tick: 120,
                target: "b".into(),
                use_type: 3,
                caller: String::new(),
            },
            Fire {
                tick: 130,
                target: "c".into(),
                use_type: 3,
                caller: String::new(),
            },
        ];
        let p = vec![
            Fire {
                tick: 50,
                target: "a".into(),
                use_type: 3,
                caller: String::new(),
            },
            Fire {
                tick: 60,
                target: "b".into(),
                use_type: 3,
                caller: String::new(),
            },
            Fire {
                tick: 80,
                target: "d".into(),
                use_type: 3,
                caller: String::new(),
            },
        ];
        let rep = compare(&r, 200, &p, 150, 20);
        let flag = |n: &str| {
            rep.rows
                .iter()
                .find(|x| x.target == n)
                .unwrap()
                .flag
                .clone()
        };
        assert_eq!(rep.offset, 50);
        assert_eq!(flag("a"), "");
        assert_eq!(flag("b"), "COUNT 2/1");
        assert_eq!(flag("c"), "MISSING-IN-PORT");
        assert_eq!(flag("d"), "EXTRA-IN-PORT");
    }
}

/// One brush entity's sampled track.
#[derive(Debug, Default, Clone)]
pub struct Track {
    pub class: String,
    pub targetname: String,
    /// map_tick -> centre in retail axes (x, y, z) and yaw/pitch/roll degrees.
    pub pos: BTreeMap<i64, ([f64; 3], [f64; 3])>,
}

fn f(m: &BTreeMap<&str, &str>, k: &str) -> f64 {
    m.get(k).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// Brush-entity tracks from retail `HLREF|entity` rows.
pub fn retail_tracks(text: &str, map: &str) -> BTreeMap<String, Track> {
    let mut out: BTreeMap<String, Track> = BTreeMap::new();
    let mut seen: BTreeMap<(String, i64), u32> = BTreeMap::new();
    for line in text.lines() {
        let Some(i) = line.find("HLREF|entity|") else {
            continue;
        };
        let m = kv(&line[i..]);
        if m.get("map") != Some(&map) {
            continue;
        }
        let brush = f(&m, "brush") as i64;
        let class = m.get("class").unwrap_or(&"");
        let name = m.get("targetname").unwrap_or(&"");
        let tick = f(&m, "map_tick") as i64;
        let (key, p) = if brush > 0 {
            (format!("*{brush}"), [f(&m, "cx"), f(&m, "cy"), f(&m, "cz")])
        } else if class.starts_with("monster_") && !name.is_empty() {
            (
                format!("{class}:{name}"),
                [f(&m, "x"), f(&m, "y"), f(&m, "z")],
            )
        } else {
            continue;
        };
        *seen.entry((key.clone(), tick)).or_default() += 1;
        let t = out.entry(key).or_default();
        t.class = class.to_string();
        t.targetname = name.to_string();
        t.pos
            .insert(tick, (p, [f(&m, "yaw"), f(&m, "pitch"), f(&m, "roll")]));
    }
    let ambiguous: BTreeSet<&String> = seen
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|((k, _), _)| k)
        .collect();
    out.retain(|k, _| !ambiguous.contains(k));
    out
}

/// Brush-entity tracks from port `HLPSX|entity` rows (height is `cy`).
pub fn port_tracks(text: &str, map: &str) -> BTreeMap<String, Track> {
    let mut out: BTreeMap<String, Track> = BTreeMap::new();
    let mut seen: BTreeMap<(String, i64), u32> = BTreeMap::new();
    for line in text.lines() {
        let Some(i) = line.find("HLPSX|entity|") else {
            continue;
        };
        let m = kv(&line[i..]);
        if m.get("map") != Some(&map) {
            continue;
        }
        let brush = f(&m, "brush") as i64;
        let class = m.get("class").unwrap_or(&"");
        let name = m.get("targetname").unwrap_or(&"");
        let tick = f(&m, "map_tick") as i64;
        let (key, p) = if brush > 0 {
            (format!("*{brush}"), [f(&m, "cx"), f(&m, "cz"), f(&m, "cy")])
        } else if class.starts_with("monster_") && !name.is_empty() {
            (
                format!("{class}:{name}"),
                [f(&m, "x"), f(&m, "z"), f(&m, "y")],
            )
        } else {
            continue;
        };
        *seen.entry((key.clone(), tick)).or_default() += 1;
        let yaw = f(&m, "yaw_q12") * 360.0 / 4096.0;
        let t = out.entry(key).or_default();
        t.class = class.to_string();
        t.targetname = name.to_string();
        t.pos.insert(tick, (p, [yaw, 0.0, 0.0]));
    }
    let ambiguous: BTreeSet<&String> = seen
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|((k, _), _)| k)
        .collect();
    out.retain(|k, _| !ambiguous.contains(k));
    out
}

fn travel(t: &Track) -> (f64, Option<i64>) {
    let mut it = t.pos.iter();
    let Some((&t0, &(p0, _))) = it.next() else {
        return (0.0, None);
    };
    let mut far = 0.0f64;
    let mut first = None;
    for (&tick, &(p, _)) in t.pos.iter() {
        let d = (0..3).map(|i| (p[i] - p0[i]).abs()).fold(0.0, f64::max);
        if d > 4.0 && first.is_none() {
            first = Some(tick - t0);
        }
        far = far.max(d);
    }
    (far, first)
}

fn swing(t: &Track) -> f64 {
    let mut it = t.pos.values();
    let Some(&(_, a0)) = it.next() else {
        return 0.0;
    };
    t.pos
        .values()
        .map(|&(_, a)| {
            (0..3)
                .map(|i| {
                    let d = (a[i] - a0[i]).rem_euclid(360.0);
                    if d > 180.0 {
                        360.0 - d
                    } else {
                        d
                    }
                })
                .fold(0.0, f64::max)
        })
        .fold(0.0, f64::max)
}

#[derive(Debug)]
pub struct MotionRow {
    pub class: String,
    pub name: String,
    pub brush: String,
    pub retail_move: f64,
    pub port_move: f64,
    pub retail_swing: f64,
    pub port_swing: f64,
    pub flag: String,
}

/// Compare how far each brush entity travelled and swung in both runs.
pub fn compare_motion(
    retail: &BTreeMap<String, Track>,
    port: &BTreeMap<String, Track>,
    tol: f64,
) -> Vec<MotionRow> {
    let mut rows = Vec::new();
    for (b, r) in retail {
        let b_key = b;
        let Some(p) = port.get(b) else {
            // The player's tram is cooked on its own path, not as an entity record.
            if r.class == "func_tracktrain" && r.targetname == "train" {
                continue;
            }
            let (rm, _) = travel(r);
            if rm > tol || swing(r) > 8.0 {
                rows.push(MotionRow {
                    class: r.class.clone(),
                    name: r.targetname.clone(),
                    brush: b.clone(),
                    retail_move: rm,
                    port_move: 0.0,
                    retail_swing: swing(r),
                    port_swing: 0.0,
                    flag: "NOT-IN-PORT".into(),
                });
            }
            continue;
        };
        // A fan spins continuously and a 20-tick sample aliases it; a
        // rotating mover's bounding-box centre moves with its angle.
        if r.class == "func_rotating" {
            continue;
        }
        if r.class.starts_with("monster_") {
            let (rm, _) = travel(r);
            let (pm, _) = travel(p);
            let flag = if (rm > 64.0) != (pm > 64.0) {
                if rm > 64.0 {
                    "STILL-IN-PORT"
                } else {
                    "MOVES-ONLY-IN-PORT"
                }
            } else {
                ""
            };
            if !flag.is_empty() {
                rows.push(MotionRow {
                    class: r.class.clone(),
                    name: r.targetname.clone(),
                    brush: b.clone(),
                    retail_move: rm,
                    port_move: pm,
                    retail_swing: 0.0,
                    port_swing: 0.0,
                    flag: flag.into(),
                });
            }
            continue;
        }
        // Where the brush stands at the first sample, before anything fires.
        if let (Some((_, &(a, _))), Some((_, &(b, _)))) = (r.pos.iter().next(), p.pos.iter().next())
        {
            let d = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f64::max);
            let rotating = matches!(
                r.class.as_str(),
                "func_door_rotating"
                    | "func_platrot"
                    | "func_pendulum"
                    | "func_rot_button"
                    | "momentary_rot_button"
            );
            // Retail reports a zero centre for brushes it never linked
            // (invisible effect trains), and a pushable's centre is its crate.
            let unlinked = a == [0.0; 3];
            if d > 16.0 && !rotating && !unlinked && r.class != "func_pushable" {
                rows.push(MotionRow {
                    class: r.class.clone(),
                    name: r.targetname.clone(),
                    brush: b_key.clone(),
                    retail_move: d,
                    port_move: d,
                    retail_swing: 0.0,
                    port_swing: 0.0,
                    flag: format!("START-POS off by {d:.0}"),
                });
                continue;
            }
        }
        let rotates = matches!(
            r.class.as_str(),
            "func_door_rotating"
                | "func_platrot"
                | "func_pendulum"
                | "func_rot_button"
                | "momentary_rot_button"
        );
        let (rm, rfirst) = travel(r);
        let (pm, pfirst) = travel(p);
        let (rs, ps) = (swing(r), swing(p));
        let mut flag = String::new();
        if rotates {
            if (rs > 8.0) != (ps > 8.0) {
                flag = if rs > 8.0 {
                    "NO-SWING-IN-PORT".into()
                } else {
                    "SWINGS-ONLY-IN-PORT".into()
                };
            }
        } else if (rm > tol) != (pm > tol) {
            flag = if rm > tol {
                "STILL-IN-PORT".into()
            } else {
                "MOVES-ONLY-IN-PORT".into()
            };
        } else if rm > tol && (rm - pm).abs() > rm.max(pm) * 0.3 + tol {
            flag = "TRAVEL".into();
        } else if (rs > 8.0) != (ps > 8.0) && !matches!(r.class.as_str(), "func_pendulum") {
            flag = if rs > 8.0 {
                "NO-SWING-IN-PORT".into()
            } else {
                "SWINGS-ONLY-IN-PORT".into()
            };
        } else if let (Some(a), Some(b2)) = (rfirst, pfirst) {
            if (a - b2).abs() > 100 {
                flag = format!("START {:+}", b2 - a);
            }
        }
        if !flag.is_empty() {
            rows.push(MotionRow {
                class: r.class.clone(),
                name: r.targetname.clone(),
                brush: b.clone(),
                retail_move: rm,
                port_move: pm,
                retail_swing: rs,
                port_swing: ps,
                flag,
            });
        }
    }
    rows
}

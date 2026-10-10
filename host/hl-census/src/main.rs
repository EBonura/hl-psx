use hl_census::census::{self, default_half_life, map_list};
use hl_census::contract::{Contract, Status};
use hl_census::{analyse, summary_line, Sources};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::exit;

const HELP: &str = "hl-census: entity census of the shipped maps against hl-psx

usage: hl-census <command> [--repo DIR] [--hl DIR] [--strict]

  report   print the summary and per-class gap tables (markdown)
  check    exit 1 if a used class, key or flag has no contract row, or a
           contract pointer does not resolve (--strict: stale rows too)
  seed     print contract rows for every unclassified class+item, guessing
           status from whether the cooker names the key (edit before commit)
";

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        print!("{HELP}");
        exit(2);
    };
    if matches!(cmd, "-h" | "--help" | "help") {
        print!("{HELP}");
        return;
    }
    let repo = PathBuf::from(arg(&args, "--repo").unwrap_or_else(|| ".".into()));
    let hl = arg(&args, "--hl")
        .map(PathBuf::from)
        .or_else(default_half_life)
        .unwrap_or_else(|| {
            eprintln!("Half-Life install not found; pass --hl DIR or set HL_DIR");
            exit(2)
        });
    let maps = map_list(&repo.join("host/hl-content/map-list.txt")).unwrap_or_else(|e| {
        eprintln!("{e}");
        exit(2)
    });
    let census = census::load(&hl, &maps).unwrap_or_else(|e| {
        eprintln!("{e}");
        exit(2)
    });
    let contract_text = std::fs::read_to_string(repo.join("host/hl-content/entity-contract.txt"))
        .unwrap_or_default();
    let contract = Contract::parse(&contract_text).unwrap_or_else(|errs| {
        for e in errs {
            eprintln!("{e}");
        }
        exit(2)
    });
    let src = Sources::load(&repo);
    let (findings, tally) = analyse(&census, &contract, &src);
    let strict = args.iter().any(|a| a == "--strict");
    match cmd {
        "check" => {
            println!(
                "{} maps, {} entities",
                census.maps.len(),
                census.total_entities()
            );
            println!("{}", summary_line(&tally));
            for (class, maps) in &findings.classes_untracked {
                println!("UNTRACKED class {class} ({maps} maps)");
            }
            for (class, item, maps, n) in &findings.unclassified {
                println!("UNCLASSIFIED {class}|{item} ({maps} maps, {n} uses)");
            }
            for b in &findings.broken {
                println!("BROKEN {b}");
            }
            for s in &findings.stale {
                println!("{} {s}", if strict { "STALE" } else { "stale (warning)" });
            }
            if args.iter().any(|a| a == "--reviewed") && tally.unreviewed > 0 {
                println!("{} unreviewed pairs remain", tally.unreviewed);
                exit(1);
            }
            exit(if findings.ok(strict) { 0 } else { 1 });
        }
        "seed" => {
            let key_known = |k: &str| src_has_literal(&repo, k);
            let catalog = std::fs::read_to_string(repo.join("host/hl-content/entity-support.txt"))
                .unwrap_or_default();
            let mut out = String::new();
            for (class, item, _, _) in &findings.unclassified {
                if item == census::CLASS_ITEM {
                    let status = catalog
                        .lines()
                        .filter(|l| !l.starts_with('#'))
                        .filter_map(|l| {
                            let mut f = l.splitn(3, '|');
                            Some((f.next()?, f.next()?, f.next().unwrap_or("")))
                        })
                        .find(|(c, _, _)| c.eq_ignore_ascii_case(class));
                    let (st, note) = match status {
                        Some((_, "runtime" | "cook_only", n)) => ("yes", n),
                        Some((_, "approximated", n)) => ("partial", n),
                        Some((_, "intentional", n)) => ("na", n),
                        Some((_, "missing_gameplay", n)) => ("no", n),
                        Some((_, "missing_visual", n)) => ("cosmetic", n),
                        _ => ("unreviewed", ""),
                    };
                    let ev = if st == "yes" || st == "partial" {
                        match src_has_literal(&repo, class) {
                            Some(kind) => format!("{kind}:\"{class}\""),
                            None => "-".to_string(),
                        }
                    } else {
                        "-".to_string()
                    };
                    let st = if (st == "yes" || st == "partial") && ev == "-" {
                        "unreviewed"
                    } else {
                        st
                    };
                    let _ = writeln!(out, "{class}|@class|{st}|{ev}|-|{note}");
                    continue;
                }
                let (status, ev) = if let Some(bit) = item.strip_prefix("flag:") {
                    ("unreviewed", format!("-|-|bit {bit}"))
                } else if item.starts_with('@') {
                    ("unreviewed", "-|-|".to_string())
                } else if let Some(kind) = key_known(item) {
                    (
                        "unreviewed",
                        format!("{kind}:\"{item}\"|-|a source file names the key"),
                    )
                } else {
                    (
                        "unreviewed",
                        "-|-|the cooker never names the key".to_string(),
                    )
                };
                let _ = writeln!(out, "{class}|{item}|{status}|{ev}");
            }
            print!("{out}");
        }
        "ents" => {
            let map = args.get(1).cloned().unwrap_or_default();
            let filt = args.get(2).cloned().unwrap_or_default();
            for (i, e) in census.entities.get(&map).into_iter().flatten().enumerate() {
                if filt.is_empty() || e.class().contains(&filt) {
                    let kv: Vec<String> = e.kv.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    println!("#{i} {}", kv.join(" | "));
                }
            }
        }
        "roots" => {
            // roots MAP... --out DIR [--gap N] [--start N]: probe sections and retail cfgs.
            let out = PathBuf::from(arg(&args, "--out").unwrap_or_else(|| ".".into()));
            let gap: u32 = arg(&args, "--gap")
                .and_then(|v| v.parse().ok())
                .unwrap_or(100);
            let start: u32 = arg(&args, "--start")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60);
            let _ = std::fs::create_dir_all(&out);
            let names: Vec<&String> = args[1..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .collect();
            let mut section = String::new();
            for (n, map) in names.iter().enumerate() {
                let ents = census.entities.get(map.as_str()).unwrap_or_else(|| {
                    eprintln!("unknown map {map}");
                    exit(2)
                });
                let r = hl_census::roots::roots(ents);
                let s = hl_census::roots::scripts(&r, gap, start);
                println!("{map}: {} roots, ends at tick {}", r.len(), s.end_tick);
                let mut port = format!("map {map}\n{}\n", s.port.join("\n"));
                if let (true, Some(next)) = (args.iter().any(|a| a == "--chain"), names.get(n + 1))
                {
                    port.push_str(&format!("{} goto {next}\n", s.end_tick + 200));
                }
                let _ = std::fs::write(out.join(format!("{map}-roots.txt")), &port);
                let _ = std::fs::write(
                    out.join(format!("probe_{map}.cfg")),
                    s.retail.join("\n") + "\n",
                );
                section.push_str(&port);
            }
            let _ = std::fs::write(out.join("batch.txt"), section);
        }
        "compare" => {
            // compare MAP... --ref-dir DIR --port LOG... [--tol N] [-v]: retail vs port target dispatch.
            let ref_dir = PathBuf::from(arg(&args, "--ref-dir").unwrap_or_else(|| "ref".into()));
            let port_logs: Vec<String> = args
                .windows(2)
                .filter(|w| w[0] == "--port")
                .map(|w| w[1].clone())
                .collect();
            let tol: i64 = arg(&args, "--tol")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            let names: Vec<&String> = args[1..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .collect();
            let port_text: String = port_logs
                .iter()
                .map(|p| std::fs::read_to_string(p).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n");
            let verbose = args.iter().any(|a| a == "-v");
            for map in names {
                let rt = std::fs::read_to_string(ref_dir.join(format!("{map}.trace")))
                    .unwrap_or_default();
                if rt.is_empty() || !port_text.contains(&format!("HLPSX|tick|map={map}|")) {
                    println!(
                        "== {map}: NO-DATA (retail {} bytes, port ticks present: {})",
                        rt.len(),
                        port_text.contains(&format!("HLPSX|tick|map={map}|"))
                    );
                    continue;
                }
                let (r, rl) = hl_census::compare::retail_fires(&rt, map);
                let (p, pl) = hl_census::compare::port_fires(&port_text, map);
                // Only names something in the map answers to matter: the engine
                // also fires special names (game_playerspawn) nobody listens to.
                let known: std::collections::BTreeSet<&str> = census
                    .entities
                    .get(map.as_str())
                    .into_iter()
                    .flatten()
                    .map(|e| e.targetname())
                    .chain(
                        census
                            .entities
                            .get(map.as_str())
                            .into_iter()
                            .flatten()
                            .filter(|e| e.class().starts_with("monster_"))
                            .map(|e| e.targetname()),
                    )
                    .collect();
                let r: Vec<_> = r
                    .into_iter()
                    .filter(|f| known.contains(f.target.as_str()))
                    .collect();
                let p: Vec<_> = p
                    .into_iter()
                    .filter(|f| known.contains(f.target.as_str()))
                    .collect();
                let rep = hl_census::compare::compare(&r, rl, &p, pl, tol);
                let bad = rep
                    .rows
                    .iter()
                    .filter(|x| !x.flag.is_empty() && !x.flag.starts_with('('))
                    .count();
                println!(
                    "== {map}: retail fires {} (to tick {rl}), port fires {} (to tick {pl}), offset {}, differing targets {bad}",
                    rep.retail_total, rep.port_total, rep.offset
                );
                if args.iter().any(|a| a == "--motion") {
                    let gz = |p: PathBuf| -> String {
                        std::process::Command::new("gzip")
                            .args(["-dc"])
                            .arg(p)
                            .output()
                            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                            .unwrap_or_default()
                    };
                    let rt2 = gz(ref_dir.join(format!("{map}.ent.gz")));
                    let pt2: String = port_logs
                        .iter()
                        .map(|p| {
                            gz(PathBuf::from(p.trim_end_matches(".ev")).with_extension("ent.gz"))
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut rtracks = hl_census::compare::retail_tracks(&rt2, map);
                    let mut ptracks = hl_census::compare::port_tracks(&pt2, map);
                    // Only the window both runs covered counts.
                    // Retail ticks run `offset` ahead of the port's.
                    let window = pl.min(rl - rep.offset);
                    for t in rtracks.values_mut() {
                        t.pos.retain(|&k, _| k <= window + rep.offset);
                    }
                    for t in ptracks.values_mut() {
                        t.pos.retain(|&k, _| k <= window);
                    }
                    if let Some(b) = arg(&args, "--track") {
                        let (r, p) = (rtracks.get(&b), ptracks.get(&b));
                        let mut ticks: Vec<i64> = r
                            .iter()
                            .chain(p.iter())
                            .flat_map(|t| t.pos.keys().copied())
                            .collect();
                        ticks.sort();
                        ticks.dedup();
                        for t in ticks {
                            let f = |tr: Option<&hl_census::compare::Track>| {
                                tr.and_then(|tr| tr.pos.get(&t))
                                    .map(|(p, a)| {
                                        format!(
                                            "({:7.0} {:7.0} {:7.0}) yaw {:6.1}",
                                            p[0], p[1], p[2], a[0]
                                        )
                                    })
                                    .unwrap_or_else(|| "-".into())
                            };
                            println!("  TRACK {b} t{t:<5} retail {}   port {}", f(r), f(p));
                        }
                    }
                    for x in hl_census::compare::compare_motion(&rtracks, &ptracks, 24.0) {
                        println!(
                            "  MOTION {:22} {:<4} {:20} retail move {:>5.0} swing {:>4.0}  port move {:>5.0} swing {:>4.0}  {}",
                            x.class, x.brush, x.name, x.retail_move, x.retail_swing, x.port_move, x.port_swing, x.flag
                        );
                    }
                }
                for x in rep.rows.iter().filter(|x| !x.flag.is_empty() || verbose) {
                    let t = |v: Option<i64>| v.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
                    println!(
                        "  {:28} retail@{:>5} x{:<3} port@{:>5} x{:<3} {:16} {}",
                        x.target,
                        t(x.retail_first),
                        x.retail_n,
                        t(x.port_first),
                        x.port_n,
                        x.flag,
                        x.caller
                    );
                }
            }
        }
        "solidity" => {
            // solidity [MAP...] --ref-dir DIR [--rooms DIR]: retail SOLID_NOT brushes against the
            // port's cooked collision hulls (data/rooms/room_<index>.psxc).
            let ref_dir = PathBuf::from(arg(&args, "--ref-dir").unwrap_or_else(|| "ref".into()));
            let rooms = PathBuf::from(arg(&args, "--rooms").unwrap_or_else(|| "data/rooms".into()));
            let mut names: Vec<String> = args[1..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .cloned()
                .collect();
            if names.is_empty() {
                names = census.maps.clone();
            }
            let mut total = BTreeMap::<(String, i64, &str), u32>::new();
            for map in &names {
                let Some(idx) = census.maps.iter().position(|m| m == map) else {
                    continue;
                };
                let path = match arg(&args, "--hlm") {
                    Some(dir) => PathBuf::from(dir).join(format!("{map}.hlm")),
                    None => rooms.join(format!("room_{}.psxc", idx * 2)),
                };
                let Ok(cooked) = std::fs::read(path) else {
                    continue;
                };
                let gz = std::process::Command::new("gzip")
                    .args(["-dc"])
                    .arg(ref_dir.join(format!("{map}.ent.gz")))
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default();
                let retail = hl_census::solidity::retail_brushes(&gz, map);
                let Ok(port) = hl_census::solidity::port_brushes(&cooked) else {
                    continue;
                };
                for mm in hl_census::solidity::compare(&retail, &port) {
                    println!(
                        "{map} *{} {} {} flags {} retail solid {} port kind {} head {}  {}",
                        mm.submodel,
                        mm.retail.class,
                        mm.retail.name,
                        mm.retail.spawnflags,
                        mm.retail.solid,
                        mm.port.as_ref().map(|p| p.kind).unwrap_or(0),
                        mm.port.as_ref().map(|p| p.head).unwrap_or(0),
                        mm.kind
                    );
                    *total
                        .entry((mm.retail.class.clone(), mm.retail.spawnflags, mm.kind))
                        .or_default() += 1;
                }
            }
            println!("--- by class and spawnflags");
            for ((class, flags, kind), n) in total {
                println!("{n:4} {class} flags {flags} {kind}");
            }
        }
        "instances" => {
            // instances MAP... --ref-dir DIR --port-dir DIR: entities per class at the first sampled
            // tick, retail against the port. A class the port instantiates fewer of is a gap.
            let ref_dir = PathBuf::from(arg(&args, "--ref-dir").unwrap_or_else(|| "ref".into()));
            let port_dir = PathBuf::from(arg(&args, "--port-dir").unwrap_or_else(|| ".".into()));
            let gz = |p: PathBuf| -> String {
                std::process::Command::new("gzip")
                    .args(["-dc"])
                    .arg(p)
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default()
            };
            let count = |text: &str, map: &str| -> BTreeMap<String, u32> {
                let mut c = BTreeMap::new();
                for l in text
                    .lines()
                    .filter(|l| l.contains("|map_tick=0|") && l.contains(&format!("|map={map}|")))
                {
                    if let Some(class) = l.split("|class=").nth(1).and_then(|r| r.split('|').next())
                    {
                        // Dead bodies are the live class plus a corpse pose in the port.
                        let class = class.trim_end_matches("_dead").to_string();
                        *c.entry(class).or_insert(0) += 1;
                    }
                }
                c
            };
            let mut totals = BTreeMap::<String, (u32, u32)>::new();
            for map in args[1..].iter().take_while(|a| !a.starts_with("--")) {
                let r = count(&gz(ref_dir.join(format!("{map}.ent.gz"))), map);
                let p = count(&gz(port_dir.join(format!("{map}.ent.gz"))), map);
                if p.is_empty() {
                    continue;
                }
                for (class, &n) in &r {
                    let have = p.get(class).copied().unwrap_or(0);
                    let t = totals.entry(class.clone()).or_default();
                    t.0 += n;
                    t.1 += have.min(n);
                    if have < n {
                        println!("{map}: {class} retail {n} port {have}");
                    }
                }
            }
            println!("--- by class (retail total, port instantiated)");
            for (class, (r, p)) in totals {
                println!(
                    "{class:28} {r:5} {p:5}{}",
                    if p < r { "   <-- short" } else { "" }
                );
            }
        }
        "triggers" => {
            // triggers [MAP...] [--hlm DIR | --rooms DIR]: trigger brush bounds from the BSP
            // against the cooked trigger records.
            let rooms = PathBuf::from(arg(&args, "--rooms").unwrap_or_else(|| "data/rooms".into()));
            let mut names: Vec<String> = args[1..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .cloned()
                .collect();
            if names.is_empty() {
                names = census.maps.clone();
            }
            let (mut total, mut ok) = (0usize, 0usize);
            for map in &names {
                let Some(idx) = census.maps.iter().position(|m| m == map) else {
                    continue;
                };
                let path = match arg(&args, "--hlm") {
                    Some(dir) => PathBuf::from(dir).join(format!("{map}.hlm")),
                    None => rooms.join(format!("room_{}.psxc", idx * 2)),
                };
                let Ok(cooked) = std::fs::read(path) else {
                    continue;
                };
                let retail = hl_census::triggers::bsp_triggers(
                    census.entities.get(map).map(Vec::as_slice).unwrap_or(&[]),
                    census.models.get(map).map(Vec::as_slice).unwrap_or(&[]),
                );
                let Ok(port) = hl_census::triggers::port_triggers(&cooked) else {
                    continue;
                };
                let (matched, bad) = hl_census::triggers::compare(&retail, &port, 16.0);
                total += retail.len();
                ok += matched;
                for b in bad {
                    println!(
                        "{map}: {} {} bounds {:?}..{:?} {}",
                        b.retail.class,
                        b.retail.name,
                        b.retail.mins,
                        b.retail.maxs,
                        b.error
                            .map(|e| format!("closest record is off by {e:.0}"))
                            .unwrap_or_else(|| "no record of that kind".into())
                    );
                }
            }
            println!(
                "--- {ok} of {total} retail trigger brushes have a cooked record within 16 units"
            );
        }
        "health" => {
            // health [MAP...] --ref-dir DIR [--hlm DIR | --rooms DIR]: retail func_breakable health
            // against the cooked record (a retail health of 0 cooks as 1).
            let ref_dir = PathBuf::from(arg(&args, "--ref-dir").unwrap_or_else(|| "ref".into()));
            let rooms = PathBuf::from(arg(&args, "--rooms").unwrap_or_else(|| "data/rooms".into()));
            let mut names: Vec<String> = args[1..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .cloned()
                .collect();
            if names.is_empty() {
                names = census.maps.clone();
            }
            let (mut total, mut bad) = (0, 0);
            for map in &names {
                let Some(idx) = census.maps.iter().position(|m| m == map) else {
                    continue;
                };
                let path = match arg(&args, "--hlm") {
                    Some(dir) => PathBuf::from(dir).join(format!("{map}.hlm")),
                    None => rooms.join(format!("room_{}.psxc", idx * 2)),
                };
                let Ok(cooked) = std::fs::read(path) else {
                    continue;
                };
                let gz = std::process::Command::new("gzip")
                    .args(["-dc"])
                    .arg(ref_dir.join(format!("{map}.ent.gz")))
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default();
                let retail = hl_census::solidity::retail_breakable_health(&gz, map);
                let port = hl_census::solidity::port_breakable_health(&cooked);
                for (sub, (hp, sf)) in &retail {
                    let Some(&have) = port.get(sub) else { continue };
                    total += 1;
                    let want = hp.max(1.0).round() as u32;
                    if have != want {
                        bad += 1;
                        println!("{map} *{sub} flags {sf}: retail health {hp} cooked {have}");
                    }
                }
            }
            println!("--- {bad} of {total} breakables cook a different health than retail");
        }
        "levels" => {
            // levels [--start MAP]: the changelevel graph of the shipped maps.
            let start = arg(&args, "--start").unwrap_or_else(|| "c0a0".into());
            let rep = hl_census::graph::check_levels(&census.entities, &start);
            let n = hl_census::graph::level_changes(&census.entities).len();
            println!("{n} changelevel triggers over {} maps", census.maps.len());
            for c in &rep.missing_destination {
                println!(
                    "MISSING-DESTINATION {} -> {} (landmark {})",
                    c.from, c.to, c.landmark
                );
            }
            for (c, a, b) in &rep.missing_landmark {
                println!(
                    "MISSING-LANDMARK {} -> {} landmark {} (in source: {a}, in destination: {b})",
                    c.from, c.to, c.landmark
                );
            }
            for m in &rep.unreachable {
                println!("UNREACHABLE from {start}: {m}");
            }
            exit(
                if rep.missing_destination.is_empty() && rep.missing_landmark.is_empty() {
                    0
                } else {
                    1
                },
            );
        }
        "kills" => {
            // kills MAP... --out DIR [--gap N]: probe sections that kill every monster with a
            // TriggerTarget, plus DIR/MAP-kills.exp (tick, target) for `check-kills`.
            let out = PathBuf::from(arg(&args, "--out").unwrap_or_else(|| ".".into()));
            let gap: u32 = arg(&args, "--gap")
                .and_then(|v| v.parse().ok())
                .unwrap_or(60);
            let _ = std::fs::create_dir_all(&out);
            let mut batch = String::new();
            let mut summary = String::new();
            for map in args[1..].iter().take_while(|a| !a.starts_with("--")) {
                let ents = census.entities.get(map.as_str()).unwrap_or_else(|| exit(2));
                let ks = hl_census::roots::kills(ents);
                if ks.is_empty() {
                    continue;
                }
                let (lines, end) = hl_census::roots::kill_script(&ks, gap, 60);
                batch.push_str(&format!("map {map}\n{}\n", lines.join("\n")));
                let exp: String = ks
                    .iter()
                    .enumerate()
                    .map(|(i, k)| {
                        format!(
                            "{}|{}|{}|{}|{}|{:?}\n",
                            60 + gap as usize * i,
                            k.target,
                            k.class,
                            k.name,
                            k.condition,
                            k.origin
                        )
                    })
                    .collect();
                let _ = std::fs::write(out.join(format!("{map}-kills.exp")), exp);
                summary.push_str(&format!("{map}: {} kills, ends at tick {end}\n", ks.len()));
            }
            let _ = std::fs::write(out.join("kills-batch.txt"), batch);
            print!("{summary}");
        }
        "check-kills" => {
            // check-kills MAP... --exp-dir DIR --port-dir DIR: did each kill fire its TriggerTarget?
            let exp_dir = PathBuf::from(arg(&args, "--exp-dir").unwrap_or_else(|| ".".into()));
            let port_dir = PathBuf::from(arg(&args, "--port-dir").unwrap_or_else(|| ".".into()));
            for map in args[1..].iter().take_while(|a| !a.starts_with("--")) {
                let Ok(exp) = std::fs::read_to_string(exp_dir.join(format!("{map}-kills.exp")))
                else {
                    continue;
                };
                let port =
                    std::fs::read_to_string(port_dir.join(format!("{map}.ev"))).unwrap_or_default();
                if port.is_empty() {
                    println!("== {map}: NO-DATA");
                    continue;
                }
                let (fires, _) = hl_census::compare::port_fires(&port, map);
                let killat: Vec<(i64, String)> = port
                    .lines()
                    .filter(|l| l.contains("HLPSX|probe|") && l.contains("event=killat"))
                    .filter_map(|l| {
                        let tick = l.split("|tick=").nth(1)?.split('|').next()?.parse().ok()?;
                        let v = l.split("|value=").nth(1)?.trim().to_string();
                        Some((tick, v))
                    })
                    .collect();
                println!("== {map}");
                for (i, line) in exp.lines().enumerate() {
                    let f: Vec<&str> = line.split('|').collect();
                    let tick: i64 = f[0].parse().unwrap_or(0);
                    let hit = fires
                        .iter()
                        .any(|x| x.target == f[1] && x.tick >= tick && x.tick <= tick + 60);
                    let found = killat.get(i).map(|k| k.1 != "-1").unwrap_or(false);
                    let verdict = if hit {
                        "OK"
                    } else if !found {
                        "NO-LIVE-MONSTER"
                    } else {
                        "NOT-FIRED"
                    };
                    println!(
                        "  {verdict:16} t{tick:<5} {} cond {} {} {} -> {}",
                        f[2], f[4], f[3], f[5], f[1]
                    );
                }
            }
        }
        "report" => report(&census, &contract, &tally, &findings),
        _ => {
            print!("{HELP}");
            exit(2)
        }
    }
}

/// Which part of the tree names `key` as a string literal, if any.
fn src_has_literal(repo: &std::path::Path, key: &str) -> Option<&'static str> {
    use std::sync::OnceLock;
    static TREES: OnceLock<[String; 2]> = OnceLock::new();
    let t = TREES.get_or_init(|| {
        let read = |dirs: &[&str]| {
            let mut s = String::new();
            for d in dirs {
                if let Ok(rd) = std::fs::read_dir(repo.join(d)) {
                    for e in rd.flatten() {
                        if e.path().extension().and_then(|x| x.to_str()) == Some("rs") {
                            if let Ok(x) = std::fs::read_to_string(e.path()) {
                                s.push_str(&x.to_ascii_lowercase());
                            }
                        }
                    }
                }
            }
            s
        };
        [
            read(&["host/hl-bsp/src", "host/hl-build", "host/hl-content/src"]),
            read(&["game/src"]),
        ]
    });
    let lit = format!("\"{}\"", key.to_ascii_lowercase());
    if t[0].contains(&lit) {
        Some("cook")
    } else if t[1].contains(&lit) {
        Some("game")
    } else {
        None
    }
}

fn report(c: &census::Census, ct: &Contract, t: &hl_census::Tally, f: &hl_census::Findings) {
    println!("# Entity census\n");
    println!("{} maps, {} entities.\n", c.maps.len(), c.total_entities());
    println!("{}\n", summary_line(t));
    // Gap table: class, item, maps, status, tested, ranked by maps.
    let mut rows: Vec<(usize, u32, &str, &str, String, String)> = Vec::new();
    let mut per_class: BTreeMap<&str, (usize, usize, usize, usize)> = BTreeMap::new();
    for ((class, item), u) in &c.items {
        if census::COMMON_KEYS.contains(&item.as_str())
            || (ct.get(class, item).is_none() && ct.scope_of(class).is_some())
        {
            continue;
        }
        let e = per_class.entry(class).or_default();
        e.0 += 1;
        match ct.get(class, item) {
            Some(r) => {
                if r.status.supported() {
                    e.1 += 1;
                }
                if r.tested != "-" && !r.tested.is_empty() {
                    e.2 += 1;
                }
                if r.status == Status::No {
                    e.3 += 1;
                    rows.push((
                        u.maps.len(),
                        u.count,
                        class,
                        item,
                        "no".into(),
                        r.note.clone(),
                    ));
                }
            }
            None => {
                e.3 += 1;
                rows.push((
                    u.maps.len(),
                    u.count,
                    class,
                    item,
                    "unclassified".into(),
                    String::new(),
                ));
            }
        }
    }
    println!("## Per class\n");
    println!("| class | maps | pairs | supported | tested | gaps |\n|---|---|---|---|---|---|");
    let mut classes: Vec<_> = per_class.iter().collect();
    classes.sort_by(|a, b| c.classes[*b.0].maps.len().cmp(&c.classes[*a.0].maps.len()));
    for (class, (n, sup, tst, gap)) in classes {
        println!(
            "| {class} | {} | {n} | {sup} | {tst} | {gap} |",
            c.classes[*class].maps.len()
        );
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    println!("\n## Gaps by reach\n");
    println!("| class | item | maps | uses | status | note |\n|---|---|---|---|---|---|");
    for (maps, n, class, item, st, note) in rows {
        println!("| {class} | {item} | {maps} | {n} | {st} | {note} |");
    }
    if !f.broken.is_empty() {
        println!("\n## Broken contract pointers\n");
        for b in &f.broken {
            println!("- {b}");
        }
    }
}

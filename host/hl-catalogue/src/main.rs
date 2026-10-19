//! hl-catalogue: for every monster and effect class the shipped maps place,
//! list what the original game does (sounds, attacks, sequences, aim blends)
//! next to what hl-psx plays, and say which rows agree.
//!
//! usage: hl-catalogue gen|check|dump CLASS [--repo DIR] [--hl DIR] [--sdk DIR] [--out FILE]

mod contract;
mod mdl;
mod sdk;
#[path = "../../../shared/hl-format/src/skill_cfg.rs"]
mod skill_cfg;

use contract::{Port, Row};
use hl_census::census::{self, default_half_life, map_list};
use sdk::Sdk;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::exit;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Effect and prop classes that make sound or are heard through one, besides
/// every `monster_*` the maps place.
const FX: [&str; 16] = [
    "func_button",
    "func_rot_button",
    "env_spark",
    "env_sprite",
    "env_glow",
    "env_beam",
    "env_laser",
    "env_explosion",
    "env_shooter",
    "env_bubbles",
    "env_funnel",
    "env_blood",
    "env_shake",
    "env_message",
    "func_breakable",
    "env_beverage",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Status {
    Ok,
    Wrong,
    Missing,
    Extra,
    Limit,
    Na,
    Unreviewed,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Wrong => "wrong sample",
            Status::Missing => "missing",
            Status::Extra => "extra",
            Status::Limit => "limit",
            Status::Na => "n/a",
            Status::Unreviewed => "unreviewed",
        }
    }
}

struct Out {
    class: String,
    state: String,
    original: Vec<String>,
    trigger: String,
    port: Vec<String>,
    status: Status,
    evidence: String,
    note: String,
}

struct Ctx {
    repo: PathBuf,
    valve: PathBuf,
    sdk: Sdk,
    rows: Vec<Row>,
    core: BTreeMap<String, (usize, String)>,
    slots: BTreeMap<String, (u8, String, u8)>,
    game_src: String,
    cook_src: String,
    /// slot index -> (maps whose cooked bank carries it, maps that place its class), when a cook is present
    carry: std::cell::RefCell<BTreeMap<usize, (usize, usize)>>,
}

fn read_tree(root: &Path, out: &mut String) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut ps: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    ps.sort();
    for p in ps {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            read_tree(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Ok(t) = std::fs::read_to_string(&p) {
                out.push_str(&t);
                out.push('\n');
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    if cmd == "help" || cmd == "-h" {
        println!("usage: hl-catalogue gen|check|dump CLASS [--repo DIR] [--hl DIR] [--sdk DIR] [--out FILE]");
        return;
    }
    let repo = PathBuf::from(arg(&args, "--repo").unwrap_or_else(|| ".".into()));
    let hl = arg(&args, "--hl").map(PathBuf::from).or_else(default_half_life).unwrap_or_else(|| {
        eprintln!("Half-Life install not found; pass --hl DIR or set HL_DIR");
        exit(2)
    });
    let valve = if hl.join("valve").is_dir() { hl.join("valve") } else { hl.clone() };
    let sdk_dir = PathBuf::from(arg(&args, "--sdk").unwrap_or_else(|| {
        std::env::var("HLSDK_DIR").unwrap_or_else(|_| "../halflife-official".into())
    }));
    let sdk = Sdk::scan(&sdk_dir.join("dlls")).unwrap_or_else(|e| {
        eprintln!("{e}");
        exit(2)
    });
    let contract_text = std::fs::read_to_string(repo.join("host/hl-content/sound-contract.txt")).unwrap_or_default();
    let rows = contract::parse(&contract_text).unwrap_or_else(|errs| {
        for e in errs {
            eprintln!("{e}");
        }
        exit(2)
    });
    let audio = std::fs::read_to_string(repo.join("host/hl-content/src/audio.rs")).unwrap_or_default();
    let fmt = std::fs::read_to_string(repo.join("shared/hl-format/src/lib.rs")).unwrap_or_default();
    let mut game_src = String::new();
    read_tree(&repo.join("game/src"), &mut game_src);
    let mut cook_src = String::new();
    for d in ["host/hl-bsp/src", "host/hl-build", "host/hl-content/src", "shared/hl-format/src"] {
        read_tree(&repo.join(d), &mut cook_src);
    }
    let ctx = Ctx {
        repo: repo.clone(),
        valve: valve.clone(),
        sdk,
        rows,
        core: contract::core_sounds(&audio),
        slots: contract::slot_sounds(&fmt),
        game_src,
        cook_src,
        carry: Default::default(),
    };
    match cmd {
        "dump" => {
            let class = args.get(1).cloned().unwrap_or_default();
            dump(&ctx, &class);
        }
        "limits" => {
            let maps = map_list(&repo.join("host/hl-content/map-list.txt")).unwrap_or_default();
            let census = census::load(&hl, &maps).unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(2)
            });
            let voices = repo.join(arg(&args, "--voices").unwrap_or_else(|| "data/voices".into()));
            let doc = limits(&ctx, &census, &voices).unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(2)
            });
            let out = arg(&args, "--out").unwrap_or_else(|| "docs/LIMITS-sound.md".into());
            let path = repo.join(&out);
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            std::fs::write(&path, doc).unwrap_or_else(|e| {
                eprintln!("{}: {e}", path.display());
                exit(2)
            });
            println!("wrote {}", path.display());
        }
        "where" => {
            let maps = map_list(&repo.join("host/hl-content/map-list.txt")).unwrap_or_default();
            let census = census::load(&hl, &maps).unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(2)
            });
            for class in &args[1..] {
                if class.starts_with("--") {
                    break;
                }
                let mut out = Vec::new();
                for (i, m) in census.maps.iter().enumerate() {
                    let n = census.entities[m].iter().filter(|e| e.class() == class).count();
                    if n > 0 {
                        out.push(format!("{i}:{m}:{n}"));
                    }
                }
                println!("{class}: {}", out.join(" "));
            }
        }
        "check" | "gen" => {
            let maps = map_list(&repo.join("host/hl-content/map-list.txt")).unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(2)
            });
            let census = census::load(&hl, &maps).unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(2)
            });
            if let Ok(stats) = carry_stats(&ctx, &census, &repo.join(arg(&args, "--voices").unwrap_or_else(|| "data/voices".into()))) {
                *ctx.carry.borrow_mut() = stats;
            }
            let (doc, problems, tally) = generate(&ctx, &census);
            for p in &problems {
                println!("PROBLEM {p}");
            }
            println!("{}", tally_line(&tally));
            if cmd == "gen" {
                let out = arg(&args, "--out").unwrap_or_else(|| "docs/CATALOGUE-monsters.md".into());
                let path = repo.join(&out);
                if let Some(d) = path.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                std::fs::write(&path, doc).unwrap_or_else(|e| {
                    eprintln!("{}: {e}", path.display());
                    exit(2)
                });
                println!("wrote {}", path.display());
            } else if args.iter().any(|a| a == "--strict") {
                // `check --strict` also fails while any row is wrong or missing, for gating a landing.
                if tally.get(&Status::Wrong).copied().unwrap_or(0) + tally.get(&Status::Missing).copied().unwrap_or(0) > 0 {
                    exit(1);
                }
            }
            if !problems.is_empty() {
                exit(1);
            }
        }
        _ => {
            eprintln!("unknown command {cmd}");
            exit(2)
        }
    }
}

fn tally_line(t: &BTreeMap<Status, usize>) -> String {
    let mut s = String::from("rows:");
    for st in [Status::Ok, Status::Wrong, Status::Missing, Status::Extra, Status::Limit, Status::Na, Status::Unreviewed] {
        let _ = write!(s, " {} {}", st.word(), t.get(&st).copied().unwrap_or(0));
    }
    s
}

fn dump(ctx: &Ctx, class: &str) {
    let Some(cpp) = ctx.sdk.link.get(class) else {
        println!("{class}: no LINK_ENTITY_TO_CLASS");
        return;
    };
    println!("{class} -> {cpp}, chain {:?}, model {:?}", ctx.sdk.chain(cpp), ctx.sdk.model.get(cpp));
    for c in ctx.sdk.chain(cpp) {
        for r in ctx.sdk.refs.iter().filter(|r| r.class == c) {
            println!("  {} {:<22} {} ({}:{}) {}", c, r.state, r.path, r.file, r.line, r.via);
        }
    }
}

/// Sounds and sentence groups the original names for a state, over the class chain.
fn original_for(ctx: &Ctx, cpp: &str, state: &str) -> (Vec<String>, Vec<String>) {
    let mut v: Vec<String> = Vec::new();
    let mut src: Vec<String> = Vec::new();
    for c in ctx.sdk.chain(cpp) {
        for r in ctx.sdk.refs.iter().filter(|r| r.class == c && r.state == state) {
            if !v.contains(&r.path) {
                v.push(r.path.clone());
            }
            let s = format!("{}:{}", r.file, r.line);
            if !src.contains(&s) && src.len() < 3 {
                src.push(s);
            }
        }
    }
    (v, src)
}

fn model_of(ctx: &Ctx, class: &str, cpp: &str) -> Option<String> {
    for c in ctx.sdk.chain(cpp) {
        if let Some(m) = ctx.sdk.model.get(&c) {
            return Some(m.clone());
        }
    }
    let _ = class;
    None
}

fn port_paths(ctx: &Ctx, ports: &[Port], problems: &mut Vec<String>, at: &str) -> (Vec<String>, Vec<String>, bool, Option<String>, Option<String>) {
    let mut paths = Vec::new();
    let mut names = Vec::new();
    let mut none = false;
    let mut limit = None;
    let mut na = None;
    for p in ports {
        match p {
            Port::Core(id) => match ctx.core.get(id) {
                Some((n, path)) => {
                    paths.push(path.clone());
                    names.push(format!("core {n} {id}"));
                }
                None => problems.push(format!("{at}: core:{id} is not in SOUNDS")),
            },
            Port::Slot(n) => match ctx.slots.get(n) {
                Some((i, path, tier)) => {
                    paths.push(path.clone());
                    names.push(format!("slot {i} {n} (tier {tier})"));
                }
                None => problems.push(format!("{at}: sp:{n} is not a set-piece slot")),
            },
            Port::Map(p) => {
                if !ctx.valve.join("sound").join(p).is_file() {
                    problems.push(format!("{at}: map:{p} is not a retail sound"));
                }
                paths.push(p.to_ascii_lowercase());
                names.push(format!("map bank {p}"));
            }
            Port::Voice(g) => {
                paths.push(format!("!{g}"));
                names.push(format!("voice !{g}"));
            }
            Port::None => none = true,
            Port::Limit(t) => limit = Some(t.clone()),
            Port::Na(t) => na = Some(t.clone()),
        }
    }
    (paths, names, none, limit, na)
}

fn check_evidence(ctx: &Ctx, row: &Row, problems: &mut Vec<String>) {
    for e in &row.evidence {
        let (kind, lit) = match e.split_once(':') {
            Some((k, l)) => (k, l.trim().trim_matches('"')),
            None => {
                problems.push(format!("sound-contract.txt:{}: evidence `{e}` is not kind:\"literal\"", row.line));
                continue;
            }
        };
        let hay = match kind {
            "game" => &ctx.game_src,
            "cook" => &ctx.cook_src,
            _ => {
                problems.push(format!("sound-contract.txt:{}: unknown evidence kind `{kind}`", row.line));
                continue;
            }
        };
        if !hay.contains(lit) {
            problems.push(format!("sound-contract.txt:{}: {kind} evidence \"{lit}\" not found ({} {})", row.line, row.class, row.state));
        }
    }
}

fn generate(ctx: &Ctx, census: &census::Census) -> (String, Vec<String>, BTreeMap<Status, usize>) {
    let mut problems = Vec::new();
    let mut outs: Vec<Out> = Vec::new();
    let mut tally: BTreeMap<Status, usize> = BTreeMap::new();

    // classes: placed monsters, monstermaker types, the FX list
    let mut placed: BTreeMap<String, (usize, BTreeSet<String>)> = BTreeMap::new();
    for (class, u) in &census.classes {
        if class.starts_with("monster_") || FX.contains(&class.as_str()) {
            placed.insert(class.clone(), (u.count as usize, u.maps.clone()));
        }
    }
    let mut maker: BTreeMap<String, usize> = BTreeMap::new();
    let mut generic_models: BTreeMap<String, usize> = BTreeMap::new();
    for ents in census.entities.values() {
        for e in ents {
            if matches!(e.class(), "monstermaker" | "squadmaker") {
                if let Some(t) = e.get("monstertype") {
                    *maker.entry(t.to_ascii_lowercase()).or_default() += 1;
                }
            }
            if e.class() == "monster_generic" {
                if let Some(m) = e.get("model") {
                    *generic_models.entry(m.to_ascii_lowercase()).or_default() += 1;
                }
            }
        }
    }
    for t in maker.keys().filter(|t| t.starts_with("monster_")) {
        placed.entry(t.clone()).or_insert((0, BTreeSet::new()));
    }
    // rows of the contract must name a class that exists
    let mut known_states: BTreeSet<(String, String)> = BTreeSet::new();

    let mut by_class: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut class_meta: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    for (class, (_n, _maps)) in &placed {
        let Some(cpp) = ctx.sdk.link.get(class).cloned() else { continue };
        let model_path = model_of(ctx, class, &cpp);
        class_meta.insert(class.clone(), (cpp.clone(), model_path.clone()));

        // states: everything the SDK emits for the class chain, minus precache
        let mut states: Vec<String> = Vec::new();
        for c in ctx.sdk.chain(&cpp) {
            for r in ctx.sdk.refs.iter().filter(|r| r.class == c && r.state != "precache") {
                if !states.contains(&r.state) {
                    states.push(r.state.clone());
                }
            }
        }
        // model sound events (1004, 1008, 5004) per sequence
        let mut mdl_states: Vec<(String, Vec<String>, String)> = Vec::new();
        let mut model = None;
        if let Some(mp) = &model_path {
            let p = ctx.valve.join(mp);
            if let Ok(m) = mdl::load(&p) {
                for s in &m.seqs {
                    let mut sounds = Vec::new();
                    let mut at = Vec::new();
                    for e in &s.events {
                        if matches!(e.id, 1004 | 1008 | 5004) && e.options.to_ascii_lowercase().ends_with(".wav") {
                            let w = e.options.to_ascii_lowercase().trim_start_matches('*').trim_start_matches("sound/").to_string();
                            if !sounds.contains(&w) {
                                sounds.push(w);
                            }
                            at.push(format!("ev{} f{}", e.id, e.frame));
                        }
                    }
                    if !sounds.is_empty() {
                        mdl_states.push((format!("mdl:{}", s.label), sounds, at.join(" ")));
                    }
                }
                model = Some(m);
            }
        }
        let _ = &model;
        for st in &states {
            known_states.insert((class.clone(), st.clone()));
            let (orig, src) = original_for(ctx, &cpp, st);
            let trigger = trigger_text(ctx, &cpp, st, model.as_ref(), &src);
            let row = find_row(ctx, class, st);
            let o = join(ctx, class, st, orig, trigger, row, &mut problems);
            by_class.entry(class.clone()).or_default().push(outs.len());
            outs.push(o);
        }
        for d in ctx.rows.iter().filter(|r| r.declared && &r.class == class) {
            known_states.insert((class.clone(), d.state.clone()));
            let row = find_row(ctx, class, &d.state);
            let o = join(ctx, class, &d.state, d.wavs.clone(), d.note.clone(), row, &mut problems);
            by_class.entry(class.clone()).or_default().push(outs.len());
            outs.push(o);
        }
        for (st, sounds, at) in mdl_states {
            known_states.insert((class.clone(), st.clone()));
            let row = find_row(ctx, class, &st);
            let o = join(ctx, class, &st, sounds, at, row, &mut problems);
            by_class.entry(class.clone()).or_default().push(outs.len());
            outs.push(o);
        }
    }
    // contract rows with no original counterpart are port-only (extra) rows
    for row in &ctx.rows {
        if row.declared {
            continue;
        }
        if !known_states.contains(&(row.class.clone(), row.state.clone())) {
            if row.state.starts_with('@') || row.state == "*" {
                continue;
            }
            let o = join(ctx, &row.class, &row.state, Vec::new(), String::new(), Some(row), &mut problems);
            by_class.entry(row.class.clone()).or_default().push(outs.len());
            outs.push(o);
        }
        check_evidence(ctx, row, &mut problems);
        if !placed.contains_key(&row.class) && !ctx.sdk.link.contains_key(&row.class) {
            problems.push(format!("sound-contract.txt:{}: class {} is not an SDK entity", row.line, row.class));
        }
    }
    for o in &outs {
        *tally.entry(o.status).or_default() += 1;
    }

    let doc = render(ctx, census, &placed, &maker, &generic_models, &class_meta, &by_class, &outs, &tally);
    (doc, problems, tally)
}

fn join(
    ctx: &Ctx,
    class: &str,
    state: &str,
    orig: Vec<String>,
    trigger: String,
    row: Option<&Row>,
    problems: &mut Vec<String>,
) -> Out {
    let at = format!("{class} {state}");
    let Some(row) = row else {
        return Out {
            class: class.into(),
            state: state.into(),
            original: orig,
            trigger,
            port: vec!["(no contract row)".into()],
            status: Status::Unreviewed,
            evidence: String::new(),
            note: "not reviewed".into(),
        };
    };
    let (paths, names, none, limit, na) = port_paths(ctx, &row.port, problems, &at);
    let mut carried_note = String::new();
    let mut limited = false;
    for p in &row.port {
        if let Port::Slot(n) = p {
            if let Some((i, _, _)) = ctx.slots.get(n) {
                if let Some((have, placing)) = ctx.carry.borrow().get(&(*i as usize)) {
                    if have < placing {
                        limited = true;
                        let _ = write!(carried_note, "{n} carried in {have} of {placing} maps; ");
                    }
                }
            }
        }
    }
    let status = if na.is_some() {
        Status::Na
    } else if limit.is_some() {
        Status::Limit
    } else if orig.is_empty() {
        if none || paths.is_empty() { Status::Ok } else { Status::Extra }
    } else if none || paths.is_empty() {
        Status::Missing
    } else if paths.iter().all(|p| orig.contains(p)) {
        if limited { Status::Limit } else { Status::Ok }
    } else {
        Status::Wrong
    };
    let mut port = names;
    if none {
        port.push("silent".into());
    }
    if let Some(l) = limit {
        port.push(format!("LIMIT {l}"));
    }
    if let Some(n) = na {
        port.push(format!("n/a: {n}"));
    }
    Out {
        class: class.into(),
        state: state.into(),
        original: orig,
        trigger,
        port,
        status,
        evidence: row.evidence.join("; "),
        note: format!("{carried_note}{}", row.note),
    }
}

fn find_row<'a>(ctx: &'a Ctx, class: &str, state: &str) -> Option<&'a Row> {
    ctx.rows
        .iter()
        .filter(|r| !r.declared)
        .find(|r| r.class == class && r.state == state)
        .or_else(|| {
            ctx.rows.iter().filter(|r| !r.declared).find(|r| {
                r.class == class && r.state.len() > 1 && r.state.ends_with('*') && state.starts_with(&r.state[..r.state.len() - 1])
            })
        })
        .or_else(|| ctx.rows.iter().filter(|r| !r.declared).find(|r| r.class == class && r.state == "*"))
}

fn trigger_text(ctx: &Ctx, cpp: &str, state: &str, model: Option<&mdl::Model>, src: &[String]) -> String {
    let mut s = String::new();
    if let Some(label) = state.strip_prefix("ae:") {
        if let (Some(id), Some(m)) = (ctx.sdk.ae.get(label), model) {
            let mut at = Vec::new();
            for q in &m.seqs {
                for e in &q.events {
                    if e.id == *id {
                        let tick = if q.fps > 0.0 { (e.frame as f32 / q.fps * 20.0).round() as i32 } else { 0 };
                        at.push(format!("{} f{} (tick {})", q.label, e.frame, tick));
                    }
                }
            }
            let _ = write!(s, "event {id}: {}", at.join(", "));
        }
    }
    let _ = cpp;
    if !src.is_empty() {
        if !s.is_empty() {
            s.push_str("; ");
        }
        s.push_str(&src.join(", "));
    }
    s
}

#[allow(clippy::too_many_arguments)]
fn render(
    ctx: &Ctx,
    census: &census::Census,
    placed: &BTreeMap<String, (usize, BTreeSet<String>)>,
    maker: &BTreeMap<String, usize>,
    generic_models: &BTreeMap<String, usize>,
    class_meta: &BTreeMap<String, (String, Option<String>)>,
    by_class: &BTreeMap<String, Vec<usize>>,
    outs: &[Out],
    tally: &BTreeMap<Status, usize>,
) -> String {
    let mut d = String::new();
    let _ = writeln!(d, "# Monster and effect catalogue\n");
    let _ = writeln!(d, "Generated by `host/hl-catalogue` (`hl-catalogue gen`); do not edit by hand. The original side comes from the Half-Life SDK server sources and the retail `.mdl` files, the port side from `host/hl-content/sound-contract.txt`, whose evidence strings the tool checks against the source tree on every run.\n");
    let _ = writeln!(d, "{} maps, {} entities scanned. {}\n", census.maps.len(), census.total_entities(), tally_line(tally));
    let _ = writeln!(d, "Status words: **ok** the port plays one of the original's samples at that point; **wrong sample** the port plays something the original does not there; **missing** the original plays something and the port is silent; **extra** the port plays where the original is silent; **limit** the port plays the right sample, but the SPU bank of some maps cannot carry it (the row says in how many of the maps that place the class; see LIMITS-monsters.md), or a contract row declares the sample left out and why.\n");
    let _ = writeln!(d, "## Classes\n");
    let _ = writeln!(d, "| class | placed | maps | C++ class | model | ok | wrong | missing | extra | limit | n/a | unrev |");
    let _ = writeln!(d, "|---|---|---|---|---|---|---|---|---|---|---|---|");
    for (class, (n, maps)) in placed {
        let (cpp, model) = class_meta.get(class).cloned().unwrap_or_default();
        let rows = by_class.get(class).cloned().unwrap_or_default();
        let c = |s: Status| rows.iter().filter(|i| outs[**i].status == s).count();
        let mk = maker.get(class).copied().unwrap_or(0);
        let placed_txt = if mk > 0 { format!("{n} (+{mk} makers)") } else { n.to_string() };
        let _ = writeln!(
            d,
            "| {class} | {placed_txt} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            maps.len(),
            cpp,
            model.unwrap_or_default(),
            c(Status::Ok),
            c(Status::Wrong),
            c(Status::Missing),
            c(Status::Extra),
            c(Status::Limit),
            c(Status::Na),
            c(Status::Unreviewed)
        );
    }
    let _ = writeln!(d, "\n`monster_generic` models used: {}\n", generic_models.iter().map(|(m, n)| format!("{m} x{n}")).collect::<Vec<_>>().join(", "));
    for (class, (_n, maps)) in placed {
        let Some(rows) = by_class.get(class) else { continue };
        let (cpp, model) = class_meta.get(class).cloned().unwrap_or_default();
        let _ = writeln!(d, "\n## {class}\n");
        let _ = writeln!(d, "C++ `{cpp}` (chain {}), model `{}`, placed in {} maps.\n", ctx.sdk.chain(&cpp).join(" > "), model.clone().unwrap_or_default(), maps.len());
        let _ = writeln!(d, "| state | original | at | port | status | evidence / note |");
        let _ = writeln!(d, "|---|---|---|---|---|---|");
        for i in rows {
            let o = &outs[*i];
            let _ = writeln!(
                d,
                "| {} | {} | {} | {} | {} | {} |",
                o.state,
                if o.original.is_empty() { "-".to_string() } else { o.original.join("<br>") },
                o.trigger,
                if o.port.is_empty() { "-".to_string() } else { o.port.join("<br>") },
                o.status.word(),
                [o.evidence.as_str(), o.note.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("; ")
            );
        }
        // attacks and skill values
        let mut skills: BTreeSet<String> = BTreeSet::new();
        for c in ctx.sdk.chain(&cpp) {
            if let Some(f) = ctx.sdk.skill_fields.get(&c) {
                for x in f {
                    if let Some(cv) = ctx.sdk.cvar.get(x) {
                        skills.insert(cv.clone());
                    }
                }
            }
        }
        if !skills.is_empty() {
            let cfg = std::fs::read_to_string(ctx.valve.join("skill.cfg")).unwrap_or_default();
            let cfg = skill_cfg::parse_cfg(&cfg);
            let _ = writeln!(d, "\nskill.cfg (easy / medium / hard):");
            for s in skills {
                match cfg.get(&s) {
                    Some(v) => {
                        let _ = writeln!(d, "- `{s}` {} / {} / {}", v[0], v[1], v[2]);
                    }
                    None => {
                        let _ = writeln!(d, "- `{s}` (not in skill.cfg)");
                    }
                }
            }
        }
        if let Some(mp) = &model {
            if let Ok(m) = mdl::load(&ctx.valve.join(mp)) {
                let blends: Vec<String> = m.seqs.iter().filter(|s| s.blends > 1).map(|s| format!("{}x{}", s.label, s.blends)).collect();
                let events: Vec<String> = m
                    .seqs
                    .iter()
                    .flat_map(|s| s.events.iter().filter(|e| !matches!(e.id, 1004 | 1008 | 5004)).map(move |e| (s, e)))
                    .map(|(s, e)| {
                        let tick = if s.fps > 0.0 { (e.frame as f32 / s.fps * 20.0).round() as i32 } else { 0 };
                        format!("{}:ev{}@f{}(t{})", s.label, e.id, e.frame, tick)
                    })
                    .collect();
                let roster = std::fs::read_to_string(ctx.repo.join("host/hl-content/model-roster.txt")).unwrap_or_default();
                let base = Path::new(mp).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
                let spec = roster
                    .lines()
                    .filter(|l| !l.starts_with('#'))
                    .filter_map(|l| {
                        let mut f = l.splitn(3, '|');
                        let _ty = f.next()?;
                        let model = f.next()?;
                        let spec = f.next()?;
                        (model.to_ascii_lowercase() == base).then(|| spec.to_string())
                    })
                    .next();
                if let Some(spec) = spec {
                    let cooked: Vec<String> = spec.split(',').filter(|t| !t.contains('=')).map(|t| t.split(':').next().unwrap_or("").to_ascii_lowercase()).collect();
                    let uncooked: Vec<String> = m
                        .seqs
                        .iter()
                        .filter(|s| s.events.iter().any(|e| matches!(e.id, 1004 | 1008 | 5004) || e.id < 1000))
                        .filter(|s| !cooked.contains(&s.label.to_ascii_lowercase()) && !cooked.contains(&s.index.to_string()))
                        .map(|s| s.label.clone())
                        .collect();
                    let _ = writeln!(d, "\nport clips: `{spec}`; sequences with sound or attack events the port does not cook: {}", if uncooked.is_empty() { "none".to_string() } else { uncooked.join(", ") });
                }
                let _ = writeln!(d, "\nsequences {} (blended: {}), bone controllers {}; animation events: {}", m.seqs.len(), if blends.is_empty() { "none".into() } else { blends.join(", ") }, m.bone_controllers.len(), if events.is_empty() { "none".into() } else { events.join(" ") });
            }
        }
        for r in ctx.rows.iter().filter(|r| &r.class == class && r.state.starts_with('@')) {
            let _ = writeln!(d, "\n{}: {}", r.state.trim_start_matches('@'), r.note);
        }
    }
    let _ = &ctx.repo;
    d
}

/// Which maps place the class of each set-piece slot, and which of them the cooked bank
/// actually carries (data/voices/manifest.txt: `map index|bank id|key`).
fn limits(ctx: &Ctx, census: &census::Census, voices: &Path) -> Result<String, String> {
    let manifest = std::fs::read_to_string(voices.join("manifest.txt"))
        .map_err(|e| format!("{}: {e} (cook the assets first)", voices.display()))?;
    let mut carried: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for l in manifest.lines() {
        let mut f = l.splitn(3, '|');
        if let (Some(m), Some(_), Some(key)) = (f.next(), f.next(), f.next()) {
            if let Ok(m) = m.parse::<usize>() {
                carried.entry(m).or_default().insert(key.to_string());
            }
        }
    }
    // slot names from the constants in lib.rs
    let fmt = std::fs::read_to_string(ctx.repo.join("shared/hl-format/src/lib.rs")).map_err(|e| e.to_string())?;
    let mut names: BTreeMap<u8, String> = BTreeMap::new();
    for (n, (i, _, _)) in &ctx.slots {
        names.insert(*i, n.clone());
    }
    let _ = fmt;
    let mut d = String::new();
    let _ = writeln!(d, "# Sound limits: set-piece cries the maps' SPU banks cannot always carry\n");
    let _ = writeln!(d, "Generated by `hl-catalogue limits` from the cooked `data/voices/manifest.txt`; do not edit by hand.\n");
    let _ = writeln!(d, "Each map's sound bank has to fit what is left of the 512 KiB SPU RAM after the resident core. Tier 1 to 3 samples compete with the map's other sounds for rate (cues added after the original set are cooked at no more than {} Hz and never take rate from a dialogue or chatter line); tier 4 takes only bytes the allocation leaves over. A sample the bank cannot hold is not played (the runtime stays silent, it never substitutes another sound).\n", hl_format::setpiece_audio::CUE_CAP_HZ);
    let _ = writeln!(d, "| slot | sample | class | tier | maps placing it | carried | not carried |");
    let _ = writeln!(d, "|---|---|---|---|---|---|---|");
    let mut not_carried_total = 0usize;
    for (slot, s) in hl_format::setpiece_audio::SOUNDS.iter().enumerate() {
        let mut placing = 0usize;
        let mut have = 0usize;
        for (mi, m) in census.maps.iter().enumerate() {
            let mut classes: BTreeSet<String> = BTreeSet::new();
            for e in &census.entities[m] {
                classes.insert(e.class().to_ascii_lowercase());
                if matches!(e.class(), "monstermaker" | "squadmaker") {
                    if let Some(t) = e.get("monstertype") {
                        classes.insert(t.to_ascii_lowercase());
                    }
                }
                if matches!(e.class(), "func_button" | "func_rot_button") && e.spawnflags() & 64 != 0 {
                    classes.insert("@button_spark".to_string());
                }
            }
            if !s.class.split(',').any(|c| classes.contains(c)) {
                continue;
            }
            placing += 1;
            let key = format!("sfx:{}:{}", if s.looping { "loop" } else { "shot" }, s.path);
            if carried.get(&mi).is_some_and(|k| k.contains(&key)) {
                have += 1;
            }
        }
        not_carried_total += placing - have;
        let name = names.get(&(slot as u8)).cloned().unwrap_or_else(|| format!("#{slot}"));
        let _ = writeln!(d, "| {name} | `{}` | {} | {} | {placing} | {have} | {} |", s.path, s.class, s.tier, placing - have);
    }
    let _ = writeln!(d, "\n{not_carried_total} (map, slot) pairs are not carried in all.");
    Ok(d)
}

/// slot index -> (maps whose cooked bank carries the slot's sample, maps that place its class).
fn carry_stats(ctx: &Ctx, census: &census::Census, voices: &Path) -> Result<BTreeMap<usize, (usize, usize)>, String> {
    let manifest = std::fs::read_to_string(voices.join("manifest.txt")).map_err(|e| format!("{}: {e}", voices.display()))?;
    let mut carried: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for l in manifest.lines() {
        let mut f = l.splitn(3, '|');
        if let (Some(m), Some(_), Some(key)) = (f.next(), f.next(), f.next()) {
            if let Ok(m) = m.parse::<usize>() {
                carried.entry(m).or_default().insert(key.to_string());
            }
        }
    }
    let _ = ctx;
    let mut out = BTreeMap::new();
    for (slot, s) in hl_format::setpiece_audio::SOUNDS.iter().enumerate() {
        let (mut placing, mut have) = (0usize, 0usize);
        for (mi, m) in census.maps.iter().enumerate() {
            let mut classes: BTreeSet<String> = BTreeSet::new();
            for e in &census.entities[m] {
                classes.insert(e.class().to_ascii_lowercase());
                if matches!(e.class(), "monstermaker" | "squadmaker") {
                    if let Some(t) = e.get("monstertype") {
                        classes.insert(t.to_ascii_lowercase());
                    }
                }
                if matches!(e.class(), "func_button" | "func_rot_button") && e.spawnflags() & 64 != 0 {
                    classes.insert("@button_spark".to_string());
                }
            }
            if !s.class.split(',').any(|c| classes.contains(c)) {
                continue;
            }
            placing += 1;
            let key = format!("sfx:{}:{}", if s.looping { "loop" } else { "shot" }, s.path);
            if carried.get(&mi).is_some_and(|k| k.contains(&key)) {
                have += 1;
            }
        }
        out.insert(slot, (have, placing));
    }
    Ok(out)
}

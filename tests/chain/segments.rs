// SPDX-License-Identifier: GPL-2.0-or-later
//! Segment inventory: every shipped map in play order with its chapter, its changelevel exits and the
//! entity classes that decide how a person plays it.
//!
//!     rustc --edition=2021 -O tests/chain/segments.rs -o /tmp/segments
//!     /tmp/segments "$HL_DIR/maps" game/src/map_catalog.rs > segments.tsv
//!
//! The map list, play order and chapter starts are read from `map_catalog.rs`, so the table follows the
//! game's own order. Columns: play rank, room id, map, chapter, exits as `dest@landmark[name]`, then
//! counts of monsters, scripted sequences and sentences, multi_managers, triggers, buttons, doors and
//! pickups. An exit with no `dest` is a `trigger_endsection` or a map with no changelevel.

use std::collections::BTreeMap;

type Ent = BTreeMap<String, String>;

fn rd(b: &[u8], o: usize) -> usize {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

fn entities(path: &str) -> Vec<Ent> {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let (off, len) = (rd(&b, 4), rd(&b, 8));
    let text = String::from_utf8_lossy(&b[off..off + len])
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
    out
}

fn block<'a>(src: &'a str, name: &str) -> &'a str {
    let at = src.find(name).unwrap_or_else(|| panic!("no {name}"));
    let rest = &src[at..];
    let open = rest.find("= [").unwrap() + 3;
    let close = rest[open..].find("];").unwrap();
    &rest[open..open + close]
}

fn numbers(text: &str) -> Vec<usize> {
    let stripped: String = text
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    stripped
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().unwrap())
        .collect()
}

fn strings(text: &str) -> Vec<String> {
    text.split('"')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: segments <valve/maps dir> <game/src/map_catalog.rs>");
        std::process::exit(2);
    }
    let src = std::fs::read_to_string(&args[2]).expect("map_catalog.rs");
    let maps = strings(block(&src, "pub const MAPS"));
    let play = numbers(block(&src, "pub const PLAY_ORDER"));
    let chapters = strings(block(&src, "pub const CHAPTERS"));
    let starts = numbers(block(&src, "const CHAPTER_START"));
    println!("rank\troom\tmap\tchapter\texits\tmonsters\tseq\tmanagers\ttriggers\tbuttons\tdoors\tpickups");
    for (rank, &room) in play.iter().enumerate() {
        let map = &maps[room];
        let chapter = if room >= 96 {
            "Hazard Course".to_string()
        } else {
            let c = starts.iter().rposition(|&s| s <= rank).unwrap();
            chapters[c].clone()
        };
        let ents = entities(&format!("{}/{}.bsp", args[1], map));
        let count = |f: &dyn Fn(&str) -> bool| {
            ents.iter()
                .filter(|e| f(e.get("classname").map(String::as_str).unwrap_or("")))
                .count()
        };
        let mut exits = Vec::new();
        for e in &ents {
            match e.get("classname").map(String::as_str) {
                Some("trigger_changelevel") => exits.push(format!(
                    "{}@{}[{}]",
                    e.get("map").map(String::as_str).unwrap_or(""),
                    e.get("landmark").map(String::as_str).unwrap_or(""),
                    e.get("targetname").map(String::as_str).unwrap_or("")
                )),
                Some("trigger_endsection") => exits.push("END".to_string()),
                _ => {}
            }
        }
        println!(
            "{rank}\t{room}\t{map}\t{chapter}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            exits.join(" "),
            count(&|c| c.starts_with("monster_") && !c.ends_with("_dead")),
            count(&|c| c.starts_with("scripted_")),
            count(&|c| c == "multi_manager"),
            count(&|c| c.starts_with("trigger_")),
            count(&|c| c == "func_button" || c == "func_rot_button"),
            count(&|c| c.starts_with("func_door")),
            count(&|c| c.starts_with("weapon_")
                || c.starts_with("item_")
                || c.starts_with("ammo_")),
        );
    }
}

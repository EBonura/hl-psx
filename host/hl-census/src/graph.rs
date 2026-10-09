//! Target graph of a map: who fires whom by name.
//!
//! The retail game dispatches `target` and `killtarget` (and a multi_manager's
//! outputs) by targetname. The port honours a Use on a named entity only for
//! classes it implements; this module finds, per map, every edge whose
//! destination class the contract says the port does not answer.

use crate::bsp::Ent;
use std::collections::BTreeMap;

/// The pseudo-item under which a class's Use handling is recorded.
pub const USE_ITEM: &str = "@use";
/// The pseudo-item for being the subject of a killtarget.
pub const KILL_ITEM: &str = "@kill";

/// Classes whose `target` names a path, a look-at point or a route rather
/// than something to fire.
const NOT_A_FIRE_TARGET: &[&str] = &[
    "path_corner",
    "path_track",
    "func_train",
    "func_tracktrain",
    "func_trackchange",
    "func_trackautochange",
    "func_guntarget",
    "trigger_camera",
    "info_bigmomma",
    "env_beam",
    "env_laser",
    "func_tank",
    "func_tankcontrols",
    "func_tanklaser",
    "func_tankrocket",
    "func_tankmortar",
    "func_tank2",
    "func_tank3",
    "light",
    "light_spot",
    "light_environment",
    "info_node",
    "info_node_air",
    "env_sprite",
    "env_glow",
    "cycler_sprite",
    "trigger_teleport",
];

#[derive(Debug, Clone)]
pub struct Edge {
    pub src: usize,
    pub kill: bool,
    pub dst_name: String,
}

/// Fire edges of one map. A multi_manager's outputs are every key that is not
/// one of its own; `name#2` repeats a target.
pub fn edges(ents: &[Ent]) -> Vec<Edge> {
    let mut out = Vec::new();
    for (i, e) in ents.iter().enumerate() {
        let class = e.class();
        if class == "multi_manager" {
            for (k, _) in &e.kv {
                let k = k.to_ascii_lowercase();
                if matches!(
                    k.as_str(),
                    "targetname"
                        | "origin"
                        | "angles"
                        | "spawnflags"
                        | "classname"
                        | "wait"
                        | "delay"
                        | "model"
                ) {
                    continue;
                }
                let name = k.split('#').next().unwrap_or(&k).to_string();
                out.push(Edge {
                    src: i,
                    kill: false,
                    dst_name: name,
                });
            }
            continue;
        }
        if class.starts_with("monster_") || class == "monstermaker" {
            // Monsters carry a patrol path or a trigger-on-death target; the
            // death/condition chain is handled by the AI contract.
            if let Some(t) = e.get("triggertarget").filter(|t| !t.is_empty()) {
                out.push(Edge {
                    src: i,
                    kill: false,
                    dst_name: t.to_string(),
                });
            }
            if class == "monstermaker" {
                if let Some(t) = e.get("target").filter(|t| !t.is_empty()) {
                    out.push(Edge {
                        src: i,
                        kill: false,
                        dst_name: t.to_string(),
                    });
                }
            }
            continue;
        }
        if !NOT_A_FIRE_TARGET.contains(&class) {
            if let Some(t) = e.get("target").filter(|t| !t.is_empty()) {
                out.push(Edge {
                    src: i,
                    kill: false,
                    dst_name: t.to_string(),
                });
            }
        }
        if let Some(t) = e.get("killtarget").filter(|t| !t.is_empty()) {
            out.push(Edge {
                src: i,
                kill: true,
                dst_name: t.to_string(),
            });
        }
    }
    out
}

/// Resolve edges to destination classes: (edge, destination class).
pub fn resolve<'a>(ents: &'a [Ent], edges: &'a [Edge]) -> Vec<(&'a Edge, &'a str)> {
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, e) in ents.iter().enumerate() {
        let n = e.targetname();
        if !n.is_empty() {
            by_name.entry(n).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    for edge in edges {
        if let Some(ix) = by_name.get(edge.dst_name.as_str()) {
            for &j in ix {
                out.push((edge, ents[j].class()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::parse_entities;

    #[test]
    fn manager_outputs_and_class_resolution() {
        let e = parse_entities(
            "{\"classname\" \"multi_manager\" \"targetname\" \"m\" \"a\" \"0\" \"b#2\" \"1\" \"spawnflags\" \"1\"}\
             {\"classname\" \"func_door\" \"targetname\" \"a\"}\
             {\"classname\" \"env_shake\" \"targetname\" \"b\"}\
             {\"classname\" \"path_corner\" \"target\" \"a\"}\
             {\"classname\" \"trigger_once\" \"target\" \"m\" \"killtarget\" \"a\"}",
        );
        let ed = edges(&e);
        let r = resolve(&e, &ed);
        let mut got: Vec<(bool, &str)> = r.iter().map(|(ed, c)| (ed.kill, *c)).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (false, "env_shake"),
                (false, "func_door"),
                (false, "multi_manager"),
                (true, "func_door")
            ]
        );
    }
}

/// One level change: `from` map, `to` map, landmark name, entity class.
#[derive(Debug, Clone)]
pub struct Change {
    pub from: String,
    pub to: String,
    pub landmark: String,
    pub use_only: bool,
}

/// Every trigger_changelevel of every map.
pub fn level_changes(maps: &BTreeMap<String, Vec<Ent>>) -> Vec<Change> {
    let mut out = Vec::new();
    for (from, ents) in maps {
        for e in ents {
            if e.class() == "trigger_changelevel" {
                out.push(Change {
                    from: from.clone(),
                    to: e.get("map").unwrap_or("").to_ascii_lowercase(),
                    landmark: e.get("landmark").unwrap_or("").to_string(),
                    use_only: e.spawnflags() & 2 != 0,
                });
            }
        }
    }
    out
}

#[derive(Debug, Default)]
pub struct LevelReport {
    /// Changes into a map the port does not ship.
    pub missing_destination: Vec<Change>,
    /// Changes whose landmark is absent from the source or the destination.
    pub missing_landmark: Vec<(Change, bool, bool)>,
    /// Maps no chain of changelevels reaches from `start`.
    pub unreachable: Vec<String>,
}

pub fn check_levels(maps: &BTreeMap<String, Vec<Ent>>, start: &str) -> LevelReport {
    let changes = level_changes(maps);
    let has_landmark = |map: &str, name: &str| {
        maps.get(map)
            .map(|es| {
                es.iter()
                    .any(|e| e.class() == "info_landmark" && e.targetname() == name)
            })
            .unwrap_or(false)
    };
    let mut rep = LevelReport::default();
    for c in &changes {
        if !maps.contains_key(&c.to) {
            rep.missing_destination.push(c.clone());
            continue;
        }
        let (a, b) = (
            has_landmark(&c.from, &c.landmark),
            has_landmark(&c.to, &c.landmark),
        );
        if !(a && b) {
            rep.missing_landmark.push((c.clone(), a, b));
        }
    }
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut queue = vec![start];
    while let Some(m) = queue.pop() {
        if !seen.insert(m) {
            continue;
        }
        for c in changes.iter().filter(|c| c.from == m) {
            if let Some((k, _)) = maps.get_key_value(c.to.as_str()) {
                queue.push(k);
            }
        }
    }
    rep.unreachable = maps
        .keys()
        .filter(|k| !seen.contains(k.as_str()))
        .cloned()
        .collect();
    rep
}

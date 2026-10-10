//! Differential probe scripts: fire every player-activated trigger target of a
//! map in lump order, once each, on both the retail game (through its
//! `ent_fire` console command) and the port (through the interaction probe).

use crate::bsp::Ent;

/// Classes a player sets off by touching, using or shooting them.
const ROOT_CLASSES: &[&str] = &[
    "trigger_once",
    "trigger_multiple",
    "func_button",
    "func_rot_button",
    "momentary_rot_button",
];

#[derive(Debug, Clone)]
pub struct Root {
    pub target: String,
    pub class: String,
}

/// Targets fired by unnamed player-activated entities, first occurrence only.
pub fn roots(ents: &[Ent]) -> Vec<Root> {
    let mut out: Vec<Root> = Vec::new();
    for e in ents {
        if !ROOT_CLASSES.contains(&e.class()) || !e.targetname().is_empty() {
            continue;
        }
        let Some(t) = e.get("target").filter(|t| !t.is_empty()) else {
            continue;
        };
        if !out.iter().any(|r| r.target == t) {
            out.push(Root {
                target: t.to_string(),
                class: e.class().to_string(),
            });
        }
    }
    out
}

pub struct Scripts {
    /// Port probe section lines (without the leading `map`).
    pub port: Vec<String>,
    /// Retail console script.
    pub retail: Vec<String>,
    pub end_tick: u32,
}

pub fn scripts(roots: &[Root], gap: u32, start: u32, spawn: Option<[f64; 4]>) -> Scripts {
    let mut port = vec!["0 god 2".to_string(), "0 trace 20".to_string()];
    // Put the player where the retail game started him (x y z yaw, HL axes).
    if let Some(s) = spawn {
        port.push(format!("2 tp {} {} {} {} 0 0", s[0], s[1], s[2], s[3]));
    }
    let mut retail = vec!["sv_cheats 1".to_string(), "god".to_string()];
    retail.extend(std::iter::repeat("wait".to_string()).take(start as usize));
    let mut tick = start;
    for r in roots {
        port.push(format!("{tick} fire {} toggle   # {}", r.target, r.class));
        retail.push(format!("ent_fire {} use", r.target));
        retail.extend(std::iter::repeat("wait".to_string()).take(gap as usize));
        tick += gap;
    }
    Scripts {
        port,
        retail,
        end_tick: tick,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::parse_entities;

    #[test]
    fn unnamed_roots_only_first_target() {
        let e = parse_entities(
            "{\"classname\" \"trigger_once\" \"target\" \"a\"}\
             {\"classname\" \"trigger_multiple\" \"target\" \"a\"}\
             {\"classname\" \"func_button\" \"targetname\" \"x\" \"target\" \"b\"}\
             {\"classname\" \"func_button\" \"target\" \"c\"}",
        );
        let r = roots(&e);
        assert_eq!(
            r.iter().map(|r| r.target.as_str()).collect::<Vec<_>>(),
            ["a", "c"]
        );
        let s = scripts(&r, 100, 60, None);
        assert_eq!(s.end_tick, 260);
        assert!(s.port[2].starts_with("60 fire a"));
    }
}

/// A monster whose death, damage or health threshold fires a target.
#[derive(Debug, Clone)]
pub struct Kill {
    pub class: String,
    pub name: String,
    pub origin: [i32; 3],
    pub target: String,
    pub condition: i32,
}

/// TriggerCondition values: 2 take damage, 3 half health, 4 death.
pub fn kills(ents: &[Ent]) -> Vec<Kill> {
    let mut out = Vec::new();
    for e in ents {
        if !e.class().starts_with("monster_") {
            continue;
        }
        let (Some(t), Some(c)) = (e.get("triggertarget"), e.int("triggercondition")) else {
            continue;
        };
        if t.is_empty() || !(2..=4).contains(&c) {
            continue;
        }
        let o: Vec<i32> = e
            .get("origin")
            .unwrap_or("0 0 0")
            .split_whitespace()
            .filter_map(|v| v.parse::<f64>().ok())
            .map(|v| v as i32)
            .collect();
        if o.len() != 3 {
            continue;
        }
        out.push(Kill {
            class: e.class().to_string(),
            name: e.targetname().to_string(),
            origin: [o[0], o[1], o[2]],
            target: t.to_string(),
            condition: c as i32,
        });
    }
    out
}

/// Probe script lines that kill each monster in turn, `gap` ticks apart.
pub fn kill_script(kills: &[Kill], gap: u32, start: u32) -> (Vec<String>, u32) {
    let mut lines = vec!["0 god 2".to_string(), "0 trace 20".to_string()];
    let mut tick = start;
    for k in kills {
        // A named monster is found by name (a script may have moved it);
        // an unnamed one by where the map placed it.
        if k.name.is_empty() {
            lines.push(format!(
                "{tick} killat {} {} {}   # {} cond {} -> {}",
                k.origin[0], k.origin[1], k.origin[2], k.class, k.condition, k.target
            ));
        } else {
            lines.push(format!(
                "{tick} kill {}   # {} cond {} -> {}",
                k.name, k.class, k.condition, k.target
            ));
        }
        tick += gap;
    }
    (lines, tick)
}

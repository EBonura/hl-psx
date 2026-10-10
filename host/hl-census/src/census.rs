//! Usage census: which classes, keyvalues and spawnflag bits the shipped maps use.

use crate::bsp::{load_map, Ent};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Keys every entity carries that no per-class contract row is needed for.
/// `classname` is the class itself; the rest are positional or editor keys.
pub const COMMON_KEYS: &[&str] = &[
    "classname",
    "origin",
    "angles",
    "angle",
    "model",
    "targetname",
    "target",
    "spawnflags",
    "wad",
    "mapversion",
    "message",
    "_light",
    "_fade",
    "_falloff",
    "_cone",
    "_cone2",
    "_diffuse_light",
    "_minlight",
    "_sky",
    "zhlt_lightflags",
    "light_origin",
];

#[derive(Default, Clone, Debug)]
pub struct Usage {
    pub maps: BTreeSet<String>,
    pub count: u32,
    /// How often each distinct value occurs (keys only; capped at 12 values).
    pub values: BTreeMap<String, u32>,
}

impl Usage {
    fn add(&mut self, map: &str) {
        self.maps.insert(map.to_string());
        self.count += 1;
    }

    fn add_value(&mut self, map: &str, value: &str) {
        self.add(map);
        if self.values.len() < 12 || self.values.contains_key(value) {
            *self.values.entry(value.to_string()).or_default() += 1;
        }
    }

    /// `value x count` for the commonest values, e.g. `0 x38, 1 x4`.
    pub fn value_summary(&self) -> String {
        let mut v: Vec<_> = self.values.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        v.iter()
            .take(4)
            .map(|(k, n)| format!("{k} x{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Every use carried the same value, and it was 0 or empty.
    pub fn always_zero(&self) -> bool {
        !self.values.is_empty()
            && self
                .values
                .keys()
                .all(|v| matches!(v.trim(), "" | "0" | "0.0" | "0 0 0"))
    }
}

pub type Item = String;

/// The pseudo-item recording that the class exists at all.
pub const CLASS_ITEM: &str = "@class";

pub fn flag_item(bit: u32) -> Item {
    format!("flag:{bit}")
}

#[derive(Default)]
pub struct Census {
    pub maps: Vec<String>,
    pub entities: BTreeMap<String, Vec<Ent>>,
    /// map -> submodel bounds from the BSP models lump
    pub models: BTreeMap<String, Vec<([f32; 3], [f32; 3])>>,
    /// class -> usage of the class itself
    pub classes: BTreeMap<String, Usage>,
    /// (class, item) -> usage; items are lowercase keys or `flag:<bit value>`
    pub items: BTreeMap<(String, Item), Usage>,
}

impl Census {
    pub fn total_entities(&self) -> usize {
        self.entities.values().map(Vec::len).sum()
    }
}

/// The 103 shipped runtime maps, in the order of the content list.
pub fn map_list(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split_whitespace().next().unwrap().to_string())
        .collect())
}

pub fn default_half_life() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("HL_DIR") {
        return Some(PathBuf::from(p));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [
        "Library/Application Support/Steam/steamapps/common/Half-Life",
        ".steam/steam/steamapps/common/Half-Life",
        ".local/share/Steam/steamapps/common/Half-Life",
    ]
    .iter()
    .map(|p| home.join(p))
    .find(|p| p.is_dir())
}

pub fn load(hl_dir: &Path, maps: &[String]) -> Result<Census, String> {
    let valve = if hl_dir.join("valve").is_dir() {
        hl_dir.join("valve")
    } else {
        hl_dir.to_path_buf()
    };
    let mut census = Census::default();
    for map in maps {
        let path = valve.join("maps").join(format!("{map}.bsp"));
        let ents = load_map(&path)?;
        if let Ok(bytes) = std::fs::read(&path) {
            census
                .models
                .insert(map.clone(), crate::bsp::model_bounds(&bytes));
        }
        census.maps.push(map.clone());
        for e in &ents {
            let class = e.class().to_ascii_lowercase();
            census.classes.entry(class.clone()).or_default().add(map);
            census
                .items
                .entry((class.clone(), CLASS_ITEM.to_string()))
                .or_default()
                .add(map);
            for (k, v) in &e.kv {
                let k = item_key(&class, k);
                if k == "classname" || k == "spawnflags" {
                    continue;
                }
                census
                    .items
                    .entry((class.clone(), k))
                    .or_default()
                    .add_value(map, v);
            }
            let flags = e.spawnflags();
            for bit in 0..32 {
                if flags >> bit & 1 == 1 {
                    census
                        .items
                        .entry((class.clone(), flag_item(1 << bit)))
                        .or_default()
                        .add(map);
                }
            }
        }
        // Use/kill receivers: a class is a receiver of every edge that names it.
        let edges = crate::graph::edges(&ents);
        for (edge, dst_class) in crate::graph::resolve(&ents, &edges) {
            let item = if edge.kill {
                crate::graph::KILL_ITEM
            } else {
                crate::graph::USE_ITEM
            };
            census
                .items
                .entry((dst_class.to_ascii_lowercase(), item.to_string()))
                .or_default()
                .add(map);
        }
        census.entities.insert(map.clone(), ents);
    }
    Ok(census)
}

/// Keys a multi_manager really has. Every other key on one is a
/// `<target name>` = `<delay>` output, however many there are.
const MANAGER_KEYS: &[&str] = &[
    "targetname",
    "origin",
    "angles",
    "spawnflags",
    "classname",
    "wait",
];

/// Normalise a key into a census item. A multi_manager's outputs collapse
/// into `<output>`: the target names are map data, not engine surface.
pub fn item_key(class: &str, key: &str) -> String {
    let k = key.to_ascii_lowercase();
    if class == "multi_manager" && !MANAGER_KEYS.contains(&k.as_str()) {
        return "<output>".to_string();
    }
    k
}

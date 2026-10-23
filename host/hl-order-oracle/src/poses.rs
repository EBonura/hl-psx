//! Camera pose generation: probe-script sections that teleport the player to spread-out points
//! of a map and arm one oracle snapshot at each.

use std::fs;
use std::path::Path;

/// One entity of a BSP entity lump.
pub struct Entity {
    pub class: String,
    pub origin: Option<[i32; 3]>,
    pub model: Option<u32>,
    pub target: String,
}

/// Read the entity lump of a GoldSrc BSP (version 30).
pub fn read_entities(bsp: &Path) -> Result<Vec<Entity>, String> {
    let data = fs::read(bsp).map_err(|e| format!("{}: {e}", bsp.display()))?;
    if data.len() < 12 {
        return Err(format!("{}: too short", bsp.display()));
    }
    let word = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
    let (offset, length) = (word(4), word(8));
    let text = String::from_utf8_lossy(
        data.get(offset..offset + length)
            .ok_or_else(|| format!("{}: entity lump out of range", bsp.display()))?,
    )
    .into_owned();
    let mut entities = Vec::new();
    let mut current: Option<Vec<(String, String)>> = None;
    for line in text.lines() {
        let line = line.trim();
        if line == "{" {
            current = Some(Vec::new());
        } else if line == "}" {
            if let Some(pairs) = current.take() {
                entities.push(entity_of(&pairs));
            }
        } else if let Some(pairs) = current.as_mut() {
            let parts: Vec<&str> = line.split('"').collect();
            if parts.len() >= 4 {
                pairs.push((parts[1].to_string(), parts[3].to_string()));
            }
        }
    }
    Ok(entities)
}

fn entity_of(pairs: &[(String, String)]) -> Entity {
    let get = |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.as_str());
    let origin = get("origin").and_then(|v| {
        let n: Vec<i32> = v.split_whitespace().filter_map(|t| t.parse::<f64>().ok()).map(|f| f as i32).collect();
        (n.len() == 3).then(|| [n[0], n[1], n[2]])
    });
    Entity {
        class: get("classname").unwrap_or("").to_string(),
        origin,
        model: get("model").and_then(|m| m.strip_prefix('*')).and_then(|m| m.parse().ok()),
        target: get("targetname").unwrap_or("").to_string(),
    }
}

fn spot_height(class: &str) -> i32 {
    if class.starts_with("item_") || class.starts_with("weapon_") || class.starts_with("ammo_") {
        24
    } else {
        0
    }
}

/// Entities whose origin is a plausible standing spot (not inside a light fixture or a brush).
fn is_spot(e: &Entity) -> bool {
    e.model.is_none()
        && e.origin.is_some()
        && (e.class.starts_with("info_player")
            || e.class.starts_with("monster_")
            || e.class.starts_with("item_")
            || e.class.starts_with("weapon_")
            || e.class.starts_with("ammo_")
            || e.class == "info_node"
            || e.class == "info_teleport_destination"
            || e.class == "env_sprite")
}

fn dist2(a: [i32; 3], b: [i32; 3]) -> i64 {
    (0..3).map(|k| i64::from(a[k] - b[k]).pow(2)).sum()
}

/// Up to `count` spread-out spots by farthest-point sampling, starting at the player start.
pub fn pick_spots(entities: &[Entity], count: usize) -> Vec<[i32; 3]> {
    let spots: Vec<[i32; 3]> = entities
        .iter()
        .filter(|e| is_spot(e))
        .filter_map(|e| e.origin.map(|o| [o[0], o[1], o[2] + spot_height(&e.class)]))
        .collect();
    let start = entities
        .iter()
        .find(|e| e.class == "info_player_start")
        .and_then(|e| e.origin)
        .or_else(|| spots.first().copied());
    let Some(first) = start else {
        return Vec::new();
    };
    let mut chosen = vec![first];
    while chosen.len() < count {
        let next = spots
            .iter()
            .copied()
            .max_by_key(|s| chosen.iter().map(|c| dist2(*s, *c)).min().unwrap_or(0));
        match next {
            Some(s) if chosen.iter().all(|c| dist2(*c, s) > 64 * 64) => chosen.push(s),
            _ => break,
        }
    }
    chosen
}

/// A probe-script section for one map: `spots` x `yaws` snapshots.
pub fn section(map: &str, spots: &[[i32; 3]], yaws: &[i32], spacing: u32) -> String {
    let mut out = format!("map {map}\n0 god 2\n");
    let mut tick = 40;
    for (i, s) in spots.iter().enumerate() {
        for (j, base) in yaws.iter().enumerate() {
            let yaw = (base + 37 * (i as i32)) % 360;
            out.push_str(&format!("{tick} tp {} {} {} {yaw} 0 {}\n", s[0], s[1], s[2], spacing));
            out.push_str(&format!("{} snap\n", tick + 3 + j as u32 % 2));
            tick += spacing;
        }
    }
    out
}

/// First face and face count of every model of a GoldSrc BSP (lump 14, 64 bytes each).
pub fn read_models(bsp: &Path) -> Result<Vec<(u32, u32)>, String> {
    let data = fs::read(bsp).map_err(|e| format!("{}: {e}", bsp.display()))?;
    let word = |o: usize| u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
    let (offset, length) = (word(4 + 8 * 14), word(8 + 8 * 14));
    let mut models = Vec::new();
    for i in 0..length / 64 {
        let base = offset + i * 64;
        models.push((word(base + 56) as u32, word(base + 60) as u32));
    }
    Ok(models)
}

/// What the census knows about one map: which model a face belongs to and what entity owns it.
pub struct MapInfo {
    pub models: Vec<(u32, u32)>,
    /// `classname targetname` per model number (empty for the world).
    pub names: Vec<String>,
}

impl MapInfo {
    pub fn load(bsp: &Path) -> Result<Self, String> {
        let models = read_models(bsp)?;
        let mut names = vec![String::new(); models.len()];
        for e in read_entities(bsp)? {
            if let Some(m) = e.model.map(|m| m as usize).filter(|m| *m < names.len()) {
                names[m] = if e.target.is_empty() {
                    e.class.clone()
                } else {
                    format!("{} {}", e.class, e.target)
                };
            }
        }
        Ok(Self { models, names })
    }

    /// Model number of a face (0 for the world).
    pub fn model_of(&self, face: u32) -> u16 {
        self.models
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, (first, num))| face >= *first && face < first + num)
            .map_or(0, |(i, _)| i as u16)
    }
}

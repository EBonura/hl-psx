//! Entity lump reader for GoldSrc BSP version 30 files.
//!
//! Only the entity lump (lump 0) is read. The maps come from the player's own
//! Half-Life installation; nothing here is distributed.

use std::fs;
use std::path::Path;

/// One entity: ordered key/value pairs exactly as authored (duplicates kept).
#[derive(Clone, Debug, Default)]
pub struct Ent {
    pub kv: Vec<(String, String)>,
}

impl Ent {
    /// Last value wins, matching how the engine delivers duplicate keys.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.kv
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn class(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    pub fn targetname(&self) -> &str {
        self.get("targetname").unwrap_or("")
    }

    pub fn spawnflags(&self) -> u32 {
        self.get("spawnflags")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .map(|v| v as u32)
            .unwrap_or(0)
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| v.trim().parse().ok())
    }

    pub fn float(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.trim().parse().ok())
    }
}

/// Extract the entity lump bytes of a version 30 BSP.
pub fn entity_lump(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.len() < 8 + 16 {
        return Err("file shorter than a BSP header".into());
    }
    let version = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    if version != 30 {
        return Err(format!("BSP version {version}, expected 30"));
    }
    let off = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    bytes
        .get(off..off.checked_add(len).ok_or("entity lump overflows")?)
        .ok_or_else(|| "entity lump outside the file".to_string())
}

/// Parse entity text. Braces inside quoted values (an infodecal's `{BLOOD4`)
/// are data, not structure.
pub fn parse_entities(text: &str) -> Vec<Ent> {
    let b = text.as_bytes();
    let mut ents = Vec::new();
    let mut i = 0;
    let mut cur: Option<Ent> = None;
    let mut pending_key: Option<String> = None;
    while i < b.len() {
        match b[i] {
            b'{' if cur.is_none() => {
                cur = Some(Ent::default());
                pending_key = None;
                i += 1;
            }
            b'}' if cur.is_some() => {
                ents.push(cur.take().unwrap());
                i += 1;
            }
            b'"' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'"' {
                    j += 1;
                }
                let s = String::from_utf8_lossy(&b[start..j.min(b.len())]).into_owned();
                i = j + 1;
                if let Some(e) = cur.as_mut() {
                    match pending_key.take() {
                        Some(k) => e.kv.push((k, s)),
                        None => pending_key = Some(s),
                    }
                }
            }
            _ => i += 1,
        }
    }
    ents
}

pub fn load_map(path: &Path) -> Result<Vec<Ent>, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let lump = entity_lump(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let end = lump.iter().position(|&b| b == 0).unwrap_or(lump.len());
    Ok(parse_entities(&String::from_utf8_lossy(&lump[..end])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braces_in_values_stay_data() {
        let e = parse_entities(
            "{\n\"classname\" \"infodecal\"\n\"texture\" \"{BLOOD4\"\n}\n{\n\"classname\" \"worldspawn\"\n}\n",
        );
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].get("texture"), Some("{BLOOD4"));
        assert_eq!(e[1].class(), "worldspawn");
    }

    #[test]
    fn duplicate_keys_last_wins() {
        let e = parse_entities("{\"a\" \"1\" \"a\" \"2\"}");
        assert_eq!(e[0].get("a"), Some("2"));
        assert_eq!(e[0].kv.len(), 2);
    }

    #[test]
    fn spawnflags_default_zero() {
        let e = parse_entities("{\"classname\" \"x\" \"spawnflags\" \"260\"}");
        assert_eq!(e[0].spawnflags(), 260);
    }
}

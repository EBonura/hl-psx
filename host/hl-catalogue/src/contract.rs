//! The port side of the catalogue: `host/hl-content/sound-contract.txt`.
//!
//! One row per (entity class, state) the port answers:
//!
//! ```text
//! class|state|port|evidence|note
//! ```
//!
//! `state` is a label the SDK scan produces (`idle`, `alert`, `pain`, `die`,
//! `attack`, `ae:LABEL`, `mdl:SEQUENCE`) or a port-only label. `port` is a
//! comma list of what the port plays there:
//!
//! - `core:ID`   a resident core sample (the id column of `SOUNDS` in audio.rs)
//! - `sp:NAME`   a set-piece slot (`hl_format::setpiece_audio`)
//! - `voice:GRP` a streamed sentence group
//! - `none`      the port plays nothing there
//! - `limit:TEXT` the original sample is not kept and TEXT says why, with numbers
//!
//! `evidence` is `game:"literal"` (must occur in game/src) or `cook:"literal"`
//! (must occur in host/ or shared/), several joined with `;`, or `-`.

use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub enum Port {
    Core(String),
    Slot(String),
    Voice(String),
    Map(String),
    None,
    Limit(String),
    Na(String),
}

#[derive(Clone, Debug)]
pub struct Row {
    /// A `!class` row declares an original sound the SDK scan cannot see (a helper
    /// shared through another class); `wavs` lists it and `note` cites the source.
    pub declared: bool,
    pub wavs: Vec<String>,
    pub class: String,
    pub state: String,
    pub port: Vec<Port>,
    pub evidence: Vec<String>,
    pub note: String,
    pub line: usize,
}

pub fn parse(text: &str) -> Result<Vec<Row>, Vec<String>> {
    let mut rows = Vec::new();
    let mut errs = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = l.splitn(5, '|').collect();
        if f.len() < 4 {
            errs.push(format!("sound-contract.txt:{}: want class|state|port|evidence|note", i + 1));
            continue;
        }
        let mut port = Vec::new();
        let declared = f[0].trim().starts_with('!');
        let whole = f[2].trim();
        if declared {
            rows.push(Row {
                declared,
                wavs: whole.split(',').map(|s| s.trim().to_ascii_lowercase()).collect(),
                class: f[0].trim().trim_start_matches('!').to_string(),
                state: f[1].trim().to_string(),
                port: Vec::new(),
                evidence: Vec::new(),
                note: f.get(4).map(|s| s.trim().to_string()).unwrap_or_default(),
                line: i + 1,
            });
            continue;
        }
        let parts: Vec<&str> = if whole.starts_with("na:") || whole.starts_with("limit:") {
            vec![whole]
        } else {
            whole.split(',').collect()
        };
        for p in parts {
            let p = p.trim();
            if p == "none" {
                port.push(Port::None);
            } else if let Some(v) = p.strip_prefix("core:") {
                port.push(Port::Core(v.to_string()));
            } else if let Some(v) = p.strip_prefix("sp:") {
                port.push(Port::Slot(v.to_string()));
            } else if let Some(v) = p.strip_prefix("map:") {
                port.push(Port::Map(v.to_string()));
            } else if let Some(v) = p.strip_prefix("voice:") {
                port.push(Port::Voice(v.to_string()));
            } else if let Some(v) = p.strip_prefix("limit:") {
                port.push(Port::Limit(v.to_string()));
            } else if let Some(v) = p.strip_prefix("na:") {
                port.push(Port::Na(v.to_string()));
            } else {
                errs.push(format!("sound-contract.txt:{}: unknown port ref `{p}`", i + 1));
            }
        }
        let evidence = f[3]
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "-")
            .map(str::to_string)
            .collect();
        rows.push(Row {
            declared: false,
            wavs: Vec::new(),
            class: f[0].trim().to_string(),
            state: f[1].trim().to_string(),
            port,
            evidence,
            note: f.get(4).map(|s| s.trim().to_string()).unwrap_or_default(),
            line: i + 1,
        });
    }
    if errs.is_empty() {
        Ok(rows)
    } else {
        Err(errs)
    }
}

/// `SOUNDS` of host/hl-content/src/audio.rs: id -> path, and the position in the table.
pub fn core_sounds(audio_rs: &str) -> BTreeMap<String, (usize, String)> {
    let mut out = BTreeMap::new();
    let Some(start) = audio_rs.find("const SOUNDS: [(&str, &str);") else {
        return out;
    };
    let body = &audio_rs[start..];
    let end = body.find("\n];").unwrap_or(body.len());
    for line in body[..end].lines() {
        let t = line.trim();
        if t.starts_with("(\"") {
            let mut it = t.split('"');
            it.next();
            if let (Some(id), Some(_), Some(path)) = (it.next(), it.next(), it.next()) {
                let n = out.len();
                out.insert(id.to_string(), (n, path.to_ascii_lowercase()));
            }
        }
    }
    out
}

/// `NAME -> path` for the set-piece slots.
pub fn slot_sounds(lib_rs: &str) -> BTreeMap<String, (u8, String, u8)> {
    let mut names: Vec<(String, u8)> = Vec::new();
    let mut inside = false;
    for line in lib_rs.lines() {
        let t = line.trim();
        if t.starts_with("pub mod setpiece_audio") {
            inside = true;
        } else if inside && t.starts_with("pub mod ") {
            break;
        }
        if inside && t.starts_with("pub const ") && t.contains(": u8 =") {
            let mut it = t["pub const ".len()..].split(':');
            let name = it.next().unwrap().trim().to_string();
            if let Some(v) = t.split('=').nth(1) {
                if let Ok(n) = v.trim().trim_end_matches(';').trim().parse::<u8>() {
                    names.push((name, n));
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    for (n, i) in names {
        if let Some(s) = hl_format::setpiece_audio::SOUNDS.get(i as usize) {
            out.insert(n, (i, s.path.to_string(), s.tier));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_parse_ports_and_keep_commas_inside_na_text() {
        let rows = parse(
            "# c\nmonster_x|pain|core:a,sp:B|game:\"x\"; cook:\"y\"|note\nmonster_x|idle|na:a, b|-|\nmonster_x|die|none|-|\n",
        )
        .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].port.len(), 2);
        assert_eq!(rows[0].evidence.len(), 2);
        assert!(matches!(&rows[1].port[0], Port::Na(t) if t == "a, b"));
        assert!(matches!(rows[2].port[0], Port::None));
    }

    #[test]
    fn unknown_port_refs_are_errors() {
        assert!(parse("c|s|wat:1|-|\n").is_err());
        assert!(parse("c|s\n").is_err());
    }

    #[test]
    fn core_table_ids_map_to_paths() {
        let text = "const SOUNDS: [(&str, &str); 2] = [\n    (\"glock\", \"weapons/pl_gun3.wav\"),\n    (\"mp5\", \"weapons/hks1.wav\"),\n];\n";
        let t = core_sounds(text);
        assert_eq!(t["mp5"], (1, "weapons/hks1.wav".to_string()));
    }
}

//! Brush solidity parity: which brush entities the retail game spawns
//! SOLID_NOT (the player walks through them) against which cooked entity
//! records the port gives a collision hull.

use std::collections::BTreeMap;

/// Offsets inside a cooked map (see `hl_format::map` and `game/src/map.rs`).
const HEADER_ENTITY_OFFSET: usize = 28;
const ENT_SZ: usize = 56;

/// Entity kinds whose record never collides regardless of its hull word:
/// 2 visual-only, 4 ladder volume, 6 water.
const NON_COLLIDING_KINDS: &[u16] = &[2, 4, 6];
const KIND_FAN: u16 = 5;
const SF_ROTATING_NOT_SOLID: i32 = 64;

#[derive(Debug, Clone)]
pub struct PortBrush {
    pub kind: u16,
    pub head: i32,
    pub solid: bool,
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    u32_at(d, o).map(|v| v as i32)
}

/// Cooked brush entity records of a map, by BSP submodel index.
pub fn port_brushes(cooked: &[u8]) -> Result<BTreeMap<u32, PortBrush>, String> {
    if cooked.get(0..3) != Some(b"HLM") {
        return Err("not a cooked HLM map".into());
    }
    let ent_off = u32_at(cooked, HEADER_ENTITY_OFFSET).ok_or("short header")? as usize;
    let n_models = u32_at(cooked, ent_off).ok_or("short entity section")? as usize;
    let mut o = ent_off + 4 + n_models * 8;
    let n_ents = u32_at(cooked, o).ok_or("short entity count")? as usize;
    o += 4;
    let mut out = BTreeMap::new();
    for i in 0..n_ents {
        let r = o + i * ENT_SZ;
        let rec = cooked
            .get(r..r + ENT_SZ)
            .ok_or("entity record past the end")?;
        let submodel = u16::from_le_bytes([rec[0], rec[1]]) as u32;
        let kind = u16::from_le_bytes([rec[2], rec[3]]) & 0xff;
        let head = i32_at(rec, 44).unwrap_or(0);
        let mv2 = i32_at(rec, 24).unwrap_or(0);
        let solid = !NON_COLLIDING_KINDS.contains(&kind)
            && head & 0xffff != 0
            && !(kind == KIND_FAN && mv2 & SF_ROTATING_NOT_SOLID != 0);
        out.insert(submodel, PortBrush { kind, head, solid });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct RetailBrush {
    pub class: String,
    pub name: String,
    pub spawnflags: i64,
    /// SOLID_NOT = 0, SOLID_TRIGGER = 1, SOLID_BBOX = 2, SOLID_SLIDEBOX = 3, SOLID_BSP = 4.
    pub solid: i64,
}

/// Brush entities at the retail game's first sampled tick, by submodel index.
pub fn retail_brushes(entity_rows: &str, map: &str) -> BTreeMap<u32, RetailBrush> {
    let mut out = BTreeMap::new();
    for line in entity_rows.lines() {
        if !line.contains("|map_tick=0|") {
            continue;
        }
        let m: BTreeMap<&str, &str> = line
            .split('|')
            .skip(2)
            .filter_map(|p| p.split_once('='))
            .collect();
        if m.get("map") != Some(&map) {
            continue;
        }
        let Some(brush) = m
            .get("brush")
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|b| *b > 0)
        else {
            continue;
        };
        out.insert(
            brush as u32,
            RetailBrush {
                class: m.get("class").unwrap_or(&"").to_string(),
                name: m.get("targetname").unwrap_or(&"").to_string(),
                spawnflags: m
                    .get("spawnflags")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                solid: m.get("solid").and_then(|v| v.parse().ok()).unwrap_or(-1),
            },
        );
    }
    out
}

#[derive(Debug)]
pub struct Mismatch {
    pub submodel: u32,
    pub retail: RetailBrush,
    pub port: Option<PortBrush>,
    /// "PORT-SOLID" (retail walks through it) or "PORT-HOLLOW" (retail blocks).
    pub kind: &'static str,
}

/// Brushes whose collision differs. Triggers and volumes (retail solid 1) are
/// not collision entities and are skipped.
pub fn compare(
    retail: &BTreeMap<u32, RetailBrush>,
    port: &BTreeMap<u32, PortBrush>,
) -> Vec<Mismatch> {
    let mut out = Vec::new();
    for (&sub, r) in retail {
        if r.solid == 1 || r.solid < 0 {
            continue;
        }
        // A func_wall_toggle that starts off keeps its hull in the record; the
        // runtime hides and unlinks it through the entity's active flag.
        if r.class == "func_wall_toggle" {
            continue;
        }
        let Some(p) = port.get(&sub) else { continue };
        let retail_solid = r.solid != 0;
        if retail_solid != p.solid {
            out.push(Mismatch {
                submodel: sub,
                retail: r.clone(),
                port: Some(p.clone()),
                kind: if p.solid { "PORT-SOLID" } else { "PORT-HOLLOW" },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: u16, head: i32) -> Vec<u8> {
        let mut r = vec![0u8; ENT_SZ];
        r[0] = 1;
        r[2..4].copy_from_slice(&kind.to_le_bytes());
        r[44..48].copy_from_slice(&head.to_le_bytes());
        r
    }

    fn map_with(recs: &[Vec<u8>]) -> Vec<u8> {
        let mut d = vec![0u8; 64];
        d[0..4].copy_from_slice(b"HLMH");
        d[HEADER_ENTITY_OFFSET..HEADER_ENTITY_OFFSET + 4].copy_from_slice(&64u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // no models
        d.extend_from_slice(&(recs.len() as u32).to_le_bytes());
        for r in recs {
            d.extend_from_slice(r);
        }
        d
    }

    #[test]
    fn solid_means_a_hull_and_a_colliding_kind() {
        let b = port_brushes(&map_with(&[rec(1, 7)])).unwrap();
        assert!(b[&1].solid);
        let b = port_brushes(&map_with(&[rec(1, 0)])).unwrap();
        assert!(!b[&1].solid);
        let b = port_brushes(&map_with(&[rec(6, 9)])).unwrap();
        assert!(!b[&1].solid, "water never collides");
    }

    #[test]
    fn mismatch_names_which_side_is_solid() {
        let retail = retail_brushes(
            "HLREF|entity|map=m|tick=3|map_tick=0|class=func_door|targetname=d|brush=1|solid=0|spawnflags=8\n",
            "m",
        );
        let port = port_brushes(&map_with(&[rec(1, 7)])).unwrap();
        let mm = compare(&retail, &port);
        assert_eq!(mm.len(), 1);
        assert_eq!(mm[0].kind, "PORT-SOLID");
    }
}

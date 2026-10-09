//! Trigger volume parity: the bounds of every retail trigger brush against the
//! cooked logic record that stands for it.

use hl_format::logic as k;

const HEADER_LOGIC_OFFSET: usize = 48;
const LOGIC_SZ: usize = 64;

#[derive(Debug, Clone)]
pub struct PortTrigger {
    pub kind: u8,
    pub name: String,
    pub mins: [i32; 3],
    pub maxs: [i32; 3],
    /// A hull-shaped trigger: the box is only a broad phase.
    pub shaped: bool,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// Classname -> the logic kinds the cooker emits for it.
pub fn kinds_of(class: &str) -> &'static [u8] {
    match class {
        "trigger_once" => &[k::TRIGGER_ONCE],
        "trigger_multiple" => &[k::TRIGGER_MULTIPLE],
        "trigger_hurt" => &[k::TRIGGER_HURT],
        "trigger_push" => &[k::TRIGGER_PUSH],
        "trigger_teleport" => &[k::TRIGGER_TELEPORT],
        "trigger_changelevel" => &[k::TRIGGER_CHANGELEVEL],
        "trigger_gravity" | "func_friction" => &[k::TRIGGER_GRAVITY],
        _ => &[],
    }
}

/// The trigger-like logic records of a cooked map.
pub fn port_triggers(cooked: &[u8]) -> Result<Vec<PortTrigger>, String> {
    if cooked.get(0..3) != Some(b"HLM") {
        return Err("not a cooked HLM map".into());
    }
    let off = i32_at(cooked, HEADER_LOGIC_OFFSET).ok_or("short header")? as usize;
    let n = u16_at(cooked, off).ok_or("short logic header")? as usize;
    let n_aux = u16_at(cooked, off + 2).ok_or("short logic header")? as usize;
    let n_names = u16_at(cooked, off + 4).ok_or("short logic header")? as usize;
    let recs = off + 8;
    let offs = recs + n * LOGIC_SZ + n_aux * 4;
    let names_base = offs + n_names * 2;
    let name = |id: u16| -> String {
        if id == 0 || id as usize > n_names {
            return String::new();
        }
        let o = u16_at(cooked, offs + (id as usize - 1) * 2).unwrap_or(0) as usize;
        let start = names_base + o;
        let end = cooked[start.min(cooked.len())..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| start + p)
            .unwrap_or(start);
        String::from_utf8_lossy(&cooked[start.min(cooked.len())..end.min(cooked.len())])
            .into_owned()
    };
    let mut out = Vec::new();
    for i in 0..n {
        let o = recs + i * LOGIC_SZ;
        let kind = cooked[o];
        if !matches!(
            kind,
            k::TRIGGER_ONCE
                | k::TRIGGER_MULTIPLE
                | k::TRIGGER_HURT
                | k::TRIGGER_PUSH
                | k::TRIGGER_TELEPORT
                | k::TRIGGER_CHANGELEVEL
                | k::TRIGGER_GRAVITY
        ) {
            continue;
        }
        let brush = u16_at(cooked, o + 10).unwrap_or(k::BRUSH_NONE);
        let g = |at: usize| i32_at(cooked, o + at).unwrap_or(0);
        out.push(PortTrigger {
            kind,
            name: name(u16_at(cooked, o + 4).unwrap_or(0)),
            mins: [g(40), g(44), g(48)],
            maxs: [g(52), g(56), g(60)],
            shaped: brush != k::BRUSH_NONE && brush & k::BRUSH_SHAPE != 0,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct RetailTrigger {
    pub class: String,
    pub name: String,
    pub mins: [f64; 3],
    pub maxs: [f64; 3],
}

/// Trigger brushes straight from the BSP: the entity lump names the model, the
/// models lump holds its bounds (HL axes), shifted by the entity's origin.
pub fn bsp_triggers(
    ents: &[crate::bsp::Ent],
    models: &[([f32; 3], [f32; 3])],
) -> Vec<RetailTrigger> {
    let mut out = Vec::new();
    for e in ents {
        if kinds_of(e.class()).is_empty() {
            continue;
        }
        let Some(n) = e
            .get("model")
            .and_then(|m| m.strip_prefix('*'))
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        let Some(&(lo, hi)) = models.get(n) else {
            continue;
        };
        let o: Vec<f64> = e
            .get("origin")
            .unwrap_or("0 0 0")
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        let o = if o.len() == 3 {
            [o[0], o[1], o[2]]
        } else {
            [0.0; 3]
        };
        out.push(RetailTrigger {
            class: e.class().to_string(),
            name: e.targetname().to_string(),
            mins: [
                lo[0] as f64 + o[0],
                lo[1] as f64 + o[1],
                lo[2] as f64 + o[2],
            ],
            maxs: [
                hi[0] as f64 + o[0],
                hi[1] as f64 + o[1],
                hi[2] as f64 + o[2],
            ],
        });
    }
    out
}

#[derive(Debug)]
pub struct Mismatch {
    pub retail: RetailTrigger,
    /// Worst per-axis bound error against the closest same-kind record.
    pub error: Option<f64>,
}

/// Retail trigger volumes with no cooked record within `tol` units. The port
/// swaps HL's y and z: its axes are (x, up, y).
pub fn compare(retail: &[RetailTrigger], port: &[PortTrigger], tol: f64) -> (usize, Vec<Mismatch>) {
    let mut matched = 0;
    let mut bad = Vec::new();
    let mut used = vec![false; port.len()];
    for r in retail {
        let kinds = kinds_of(&r.class);
        let hl = |p: &PortTrigger| -> ([f64; 3], [f64; 3]) {
            (
                [p.mins[0] as f64, p.mins[2] as f64, p.mins[1] as f64],
                [p.maxs[0] as f64, p.maxs[2] as f64, p.maxs[1] as f64],
            )
        };
        let mut best: Option<(usize, f64)> = None;
        for (i, p) in port.iter().enumerate() {
            if used[i] || !kinds.contains(&p.kind) {
                continue;
            }
            let (lo, hi) = hl(p);
            let e = (0..3)
                .map(|a| (lo[a] - r.mins[a]).abs().max((hi[a] - r.maxs[a]).abs()))
                .fold(0.0, f64::max);
            if best.is_none_or(|(_, b)| e < b) {
                best = Some((i, e));
            }
        }
        match best {
            Some((i, e)) if e <= tol => {
                used[i] = true;
                matched += 1;
            }
            Some((_, e)) => bad.push(Mismatch {
                retail: r.clone(),
                error: Some(e),
            }),
            None => bad.push(Mismatch {
                retail: r.clone(),
                error: None,
            }),
        }
    }
    (matched, bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_within_tolerance_and_swaps_axes() {
        let retail = vec![RetailTrigger {
            class: "trigger_once".into(),
            name: String::new(),
            mins: [0.0, 10.0, 20.0],
            maxs: [5.0, 15.0, 25.0],
        }];
        let port = vec![PortTrigger {
            kind: k::TRIGGER_ONCE,
            name: String::new(),
            mins: [0, 20, 10],
            maxs: [5, 25, 15],
            shaped: false,
        }];
        let (n, bad) = compare(&retail, &port, 1.0);
        assert_eq!((n, bad.len()), (1, 0));
        let off = vec![PortTrigger {
            mins: [0, 120, 10],
            maxs: [5, 125, 15],
            ..port[0].clone()
        }];
        let (n, bad) = compare(&retail, &off, 1.0);
        assert_eq!((n, bad.len()), (0, 1));
    }
}

//! Cook the stationary sitting scientist's short-hull floor drop from the
//! original BSP. Runtime brush colliders only retain the standing hull.

use super::{
    ent_value, entity_text, f32le, i32le, parse_vec3, Bsp, LUMP_CLIPNODES, LUMP_ENTITIES,
    LUMP_MODELS, LUMP_PLANES,
};

fn solid_entry(
    nodes: &[u8],
    planes: &[u8],
    head: i32,
    start: [f32; 3],
    end: [f32; 3],
    lo: f32,
    hi: f32,
) -> Option<f32> {
    if head < 0 {
        return (head == -2).then_some(lo);
    }
    let node = head as usize * 8;
    let plane = i32le(nodes, node)? as usize * 20;
    let normal = [
        f32le(planes, plane)?,
        f32le(planes, plane + 4)?,
        f32le(planes, plane + 8)?,
    ];
    let distance = f32le(planes, plane + 12)?;
    let side = |p: [f32; 3]| normal[0] * p[0] + normal[1] * p[1] + normal[2] * p[2] - distance;
    let a = side(start);
    let b = side(end);
    let child = |back: bool| {
        i16::from_le_bytes([
            nodes[node + 4 + back as usize * 2],
            nodes[node + 5 + back as usize * 2],
        ]) as i32
    };
    if a >= 0.0 && b >= 0.0 {
        return solid_entry(nodes, planes, child(false), start, end, lo, hi);
    }
    if a < 0.0 && b < 0.0 {
        return solid_entry(nodes, planes, child(true), start, end, lo, hi);
    }
    let fraction = (a / (a - b)).clamp(0.0, 1.0);
    let mid = std::array::from_fn(|i| start[i] + (end[i] - start[i]) * fraction);
    let at = lo + (hi - lo) * fraction;
    solid_entry(nodes, planes, child(a < 0.0), start, mid, lo, at)
        .or_else(|| solid_entry(nodes, planes, child(a >= 0.0), mid, end, at, hi))
}

pub fn origin(bsp: &Bsp<'_>, position: [f32; 3]) -> [f32; 3] {
    let nodes = bsp.lump(LUMP_CLIPNODES);
    let planes = bsp.lump(LUMP_PLANES);
    let models = bsp.lump(LUMP_MODELS);
    let mut fraction: f32 = 1.0;
    let mut probe = |model: usize, offset: [f32; 3]| {
        let Some(head) = i32le(models, model * 64 + 48) else {
            return;
        };
        // SV_HullForEntity chooses hull 3 for a 28x28x36 scientist.
        // clip_mins (-16,-16,-18) minus actor mins (-14,-14,0).
        let start = [
            position[0] + 2.0 - offset[0],
            position[1] + 2.0 - offset[1],
            position[2] + 18.0 - offset[2],
        ];
        let end = [start[0], start[1], start[2] - 256.0];
        if let Some(hit) = solid_entry(nodes, planes, head, start, end, 0.0, 1.0) {
            fraction = fraction.min(hit);
        }
    };
    probe(0, [0.0; 3]);
    let text = entity_text(bsp.lump(LUMP_ENTITIES));
    for block in text.split('{') {
        // The stationary supports used by seated actors are world brushes,
        // func_wall chairs, and func_pushable furniture. Other brush classes
        // either move or do not provide a spawn support for these actors.
        if ent_value(block, "classname") != Some("func_wall") {
            continue;
        }
        let Some(model) = ent_value(block, "model")
            .and_then(|m| m.strip_prefix('*'))
            .and_then(|m| m.parse::<usize>().ok())
        else {
            continue;
        };
        let offset = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        probe(model, offset);
    }
    // Pushable furniture uses SOLID_BBOX, not its visible brush hull.
    for block in text.split('{') {
        if ent_value(block, "classname") != Some("func_pushable") {
            continue;
        }
        let Some(model) = ent_value(block, "model")
            .and_then(|m| m.strip_prefix('*'))
            .and_then(|m| m.parse::<usize>().ok())
        else {
            continue;
        };
        let offset = ent_value(block, "origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        let bounds: Option<Vec<f32>> = (0..6).map(|i| f32le(models, model * 64 + i * 4)).collect();
        let Some(bounds) = bounds else {
            continue;
        };
        if position[0] + 14.0 < bounds[0] + offset[0]
            || position[0] - 14.0 > bounds[3] + offset[0]
            || position[1] + 14.0 < bounds[1] + offset[1]
            || position[1] - 14.0 > bounds[4] + offset[1]
        {
            continue;
        }
        let top = bounds[5] + offset[2];
        if position[2] <= top && position[2] + 36.0 >= bounds[2] + offset[2] {
            return position;
        }
        if top < position[2] {
            fraction = fraction.min(((position[2] - top) / 256.0).clamp(0.0, 1.0));
        }
    }
    if fraction == 0.0 || fraction >= 1.0 {
        position
    } else {
        [
            position[0],
            position[1],
            position[2] - 256.0 * fraction + 0.03125,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a local Half-Life installation in HL_GAME"]
    fn seated_origins_match_c1a0_goldsrc_spawn_trace() {
        let root = std::env::var("HL_GAME").expect("set HL_GAME to the valve directory");
        let bytes = std::fs::read(std::path::Path::new(&root).join("maps/c1a0.bsp")).unwrap();
        let bsp = Bsp::parse(&bytes).unwrap();
        // Captured from the original CSittingScientist::Spawn in Xash/HLSDK.
        for (input, expected) in [
            ([-762.0, 514.0, -176.0], -200.143814),
            ([-598.0, 514.0, -176.0], -200.143814),
            ([-382.0, 286.0, -176.0], -196.893845),
            ([-1054.0, -451.0, -173.0], -173.0),
        ] {
            let result = origin(&bsp, input);
            eprintln!("seated {input:?}: {} (reference {expected})", result[2]);
            assert!((result[2] - expected).abs() < 0.5);
        }
    }

    #[test]
    fn downward_short_hull_stops_at_first_solid_plane() {
        let mut plane = Vec::new();
        for value in [0.0f32, 0.0, 1.0, 12.0] {
            plane.extend(value.to_le_bytes());
        }
        plane.extend(2i32.to_le_bytes());
        let mut nodes = 0i32.to_le_bytes().to_vec();
        nodes.extend((-1i16).to_le_bytes());
        nodes.extend((-2i16).to_le_bytes());
        assert_eq!(
            solid_entry(
                &nodes,
                &plane,
                0,
                [0.0, 0.0, 20.0],
                [0.0, 0.0, 4.0],
                0.0,
                1.0
            ),
            Some(0.5)
        );
        assert_eq!(
            solid_entry(
                &nodes,
                &plane,
                0,
                [0.0, 0.0, 10.0],
                [0.0, 0.0, 4.0],
                0.0,
                1.0
            ),
            Some(0.0)
        );
        assert_eq!(
            solid_entry(
                &nodes,
                &plane,
                0,
                [0.0, 0.0, 20.0],
                [0.0, 0.0, 15.0],
                0.0,
                1.0
            ),
            None
        );
    }
}

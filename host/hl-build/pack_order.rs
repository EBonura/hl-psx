//! Disc layout of WORLD.PAK: which chunk sits next to which.
//!
//! A map load is a fixed series of chunk reads (see `play` and
//! `stream_map_models` in the game). On the disc every read that starts
//! further than a few hundred sectors from the previous one costs the drive's
//! seek (a third of a second), and a read that follows its predecessor closely
//! costs a tenth of that. The pack used to be sorted by chunk id, which put the
//! chunks one load needs in four far apart families and made the model reads
//! hop back and forth. This module lays them out in the order a load asks for
//! them instead. Chunk ids are unchanged, so the game needs no change: the
//! pack table is looked up by id, never by position.
//!
//! Layout, front to back:
//!
//! 1. Common chunks every load reads: the HUD sheet, the explosion sprite, the
//!    decals, the menu art, the sound banks, then the viewmodels.
//! 2. The actor and item models, grouped by type in the order the game streams
//!    them (importance tier, then largest first), each type followed by its
//!    carry-over and per-map variant chunks.
//! 3. One cluster per room in the order the load reads it: textures, voices,
//!    sprites, world.
//!
//! Anything this module does not recognise goes last in id order.

use super::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// First chunk id of the viewmodels (`MODEL_CHUNK_V_9MMHANDGUN`) and their count.
const VIEWMODEL_FIRST: u32 = 1000;
/// `hl_format::weapon_audio::BANK_CHUNK_BASE`: the weapon sound banks.
const BANK_FIRST: u32 = hl_format::weapon_audio::BANK_CHUNK_BASE;
const VIEWMODEL_COUNT: u32 = 15;
/// Texture, geometry and carry-over chunk bases: `base + model type`.
const TEXTURE_BASE: u32 = 1100;
const GEOMETRY_BASE: u32 = 1300;
const CARRY_BASE: u32 = 1500;
/// Room chunk ids are `2 * room` (world) and `2 * room + 1` (textures).
const VOICE_BASE: u32 = 3100;
const SPRITE_BASE: u32 = 3300;
/// Common small chunks in the order the load reads them: HUD sheet, explosion
/// sprite, decals, menu art.
const HUD_FIRST: [u32; 4] = [3001, 3002, 3004, 3003];
/// Per-map actor chunks the game names by constant: (chunk id, model type).
const SPECIAL_MODEL_CHUNKS: [(u32, u8); 10] = [
    (1816, 16),
    (1817, 16),
    (1818, 5),
    (1819, 19),
    (1820, 9),
    (1916, 16),
    (1917, 16),
    (1918, 5),
    (1919, 19),
    (1920, 9),
];

/// What the layout needs to know about the staged pack.
pub struct PackContents {
    /// Every chunk id the pack holds.
    pub ids: BTreeSet<u32>,
    /// Streaming priority of each model type, as `game/build.rs` computes it:
    /// the largest of the type's geometry chunk (`1300 + type`) and its
    /// per-map variant chunks, in cooked (uncompressed) bytes.
    pub stream_bytes: BTreeMap<u8, u64>,
    /// Model type of each deduplicated per-map variant chunk.
    pub variant_types: BTreeMap<u32, u8>,
    /// Maps (room ids) that use each variant chunk.
    pub variant_maps: BTreeMap<u32, BTreeSet<u32>>,
}

/// Importance tier the game streams a model type in (0 combat, 1 items, 2
/// decorative). Mirrors `ai_pass` in `model_audit`, which the audit tests tie
/// to the game's AI table.
fn tier(ty: u8) -> u8 {
    super::model_audit::ai_pass(ty) as u8
}

fn model_type(id: u32, variant_types: &BTreeMap<u32, u8>) -> Option<u8> {
    let in_family = |base: u32| id.checked_sub(base).filter(|&t| t < 100).map(|t| t as u8);
    in_family(TEXTURE_BASE)
        .or_else(|| in_family(GEOMETRY_BASE))
        .or_else(|| in_family(CARRY_BASE))
        .or_else(|| {
            SPECIAL_MODEL_CHUNKS
                .iter()
                .find(|(special, _)| *special == id)
                .map(|&(_, ty)| ty)
        })
        .or_else(|| variant_types.get(&id).copied())
}

/// The one room that uses variant chunk `id`, if only one does.
fn exclusive_map(id: u32, contents: &PackContents) -> Option<u32> {
    let maps = contents.variant_maps.get(&id)?;
    (maps.len() == 1).then(|| *maps.iter().next().unwrap())
}

/// Position of a chunk inside its model type's group: geometry first, then the
/// split texture the game reads right after it, carry-over, specials, variants.
fn within_type(id: u32) -> (u32, u32) {
    match id {
        1300..=1399 => (0, id),
        1100..=1199 => (1, id),
        1500..=1599 => (2, id),
        _ => (3, id),
    }
}

/// The chunk ids in disc order.
pub fn layout(contents: &PackContents) -> Vec<u32> {
    let mut placed = BTreeSet::new();
    let mut order = Vec::with_capacity(contents.ids.len());
    let mut place = |id: u32, order: &mut Vec<u32>| {
        if contents.ids.contains(&id) && placed.insert(id) {
            order.push(id);
        }
    };

    for id in HUD_FIRST {
        place(id, &mut order);
    }
    let banks = BANK_FIRST..BANK_FIRST + VIEWMODEL_COUNT;
    for id in contents.ids.range(3000..3100).filter(|id| !banks.contains(id)) {
        place(*id, &mut order);
    }
    // A weapon's sound bank lies right behind its viewmodel: drawing the weapon
    // reads the two one after the other, with no seek between them.
    for id in VIEWMODEL_FIRST..VIEWMODEL_FIRST + VIEWMODEL_COUNT {
        place(id, &mut order);
        place(BANK_FIRST + (id - VIEWMODEL_FIRST), &mut order);
    }

    let mut types: BTreeSet<u8> = BTreeSet::new();
    let mut by_type: BTreeMap<u8, Vec<u32>> = BTreeMap::new();
    for &id in contents.ids.range(1100..3000) {
        if exclusive_map(id, contents).is_some() {
            continue;
        }
        if let Some(ty) = model_type(id, &contents.variant_types) {
            types.insert(ty);
            by_type.entry(ty).or_default().push(id);
        }
    }
    let mut types: Vec<u8> = types.into_iter().collect();
    types.sort_by_key(|&ty| {
        let bytes = contents.stream_bytes.get(&ty).copied().unwrap_or(0);
        (tier(ty), std::cmp::Reverse(bytes), ty)
    });
    let types_in_order = types.clone();
    for ty in types {
        let mut ids = by_type.remove(&ty).unwrap_or_default();
        ids.sort_by_key(|&id| within_type(id));
        for id in ids {
            place(id, &mut order);
        }
    }

    let rank: BTreeMap<u8, usize> = types_in_order
        .iter()
        .enumerate()
        .map(|(i, &t)| (t, i))
        .collect();
    let rooms: BTreeSet<u32> = contents
        .ids
        .iter()
        .filter(|&&id| id < VIEWMODEL_FIRST)
        .map(|&id| id / 2)
        .collect();
    for room in rooms {
        for id in [
            2 * room + 1,
            VOICE_BASE + room,
            SPRITE_BASE + room,
            2 * room,
        ] {
            place(id, &mut order);
        }
        // Variants only this room uses follow its world chunk, in streaming order.
        let mut own: Vec<u32> = contents
            .variant_maps
            .iter()
            .filter(|(_, maps)| maps.len() == 1 && maps.contains(&room))
            .map(|(&chunk, _)| chunk)
            .collect();
        own.sort_by_key(|chunk| {
            let ty = contents
                .variant_types
                .get(chunk)
                .copied()
                .unwrap_or(u8::MAX);
            (rank.get(&ty).copied().unwrap_or(usize::MAX), *chunk)
        });
        for chunk in own {
            place(chunk, &mut order);
        }
    }

    for &id in &contents.ids {
        place(id, &mut order);
    }
    order
}

fn id_of(path: &Path, prefix: &str) -> Option<u32> {
    path.file_stem()?
        .to_str()?
        .strip_prefix(prefix)?
        .parse()
        .ok()
}

/// Read the staged pack tree (`stage_world_pack`) and the variant table.
pub fn contents(repository: &Path, staged: &Path) -> Result<PackContents> {
    let mut ids = BTreeSet::new();
    for (family, prefix) in [
        ("rooms", "room_"),
        ("modelpack", "chunk_"),
        ("sfx", "chunk_"),
        ("voices", "chunk_"),
        ("sprites", "chunk_"),
    ] {
        for entry in fs::read_dir(staged.join(family))? {
            let path = entry?.path();
            let Some(id) = id_of(&path, prefix) else {
                continue;
            };
            ids.insert(id);
        }
    }
    let variants = repository.join("data/modelpack/map-model-variants.txt");
    let mut variant_types = BTreeMap::new();
    let mut variant_maps: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    if let Ok(text) = fs::read_to_string(&variants) {
        for line in text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let fields: Vec<&str> = line.split('|').collect();
            if let (Some(map), Some(ty), Some(chunk)) = (
                fields.first().and_then(|f| f.parse::<u32>().ok()),
                fields.get(2).and_then(|f| f.parse::<u8>().ok()),
                fields.get(3).and_then(|f| f.parse::<u32>().ok()),
            ) {
                variant_types.insert(chunk, ty);
                variant_maps.entry(chunk).or_default().insert(map);
            }
        }
    }
    let cooked_len = |id: u32| {
        fs::metadata(repository.join(format!("data/modelpack/chunk_{id}.psxm"))).map(|m| m.len())
    };
    let mut stream_bytes: BTreeMap<u8, u64> = BTreeMap::new();
    for ty in 0..100u8 {
        if let Ok(len) = cooked_len(GEOMETRY_BASE + u32::from(ty)) {
            stream_bytes.insert(ty, len);
        }
    }
    for (&chunk, &ty) in &variant_types {
        if let Ok(len) = cooked_len(chunk) {
            let largest = stream_bytes.entry(ty).or_insert(0);
            *largest = (*largest).max(len);
        }
    }
    Ok(PackContents {
        ids,
        stream_bytes,
        variant_types,
        variant_maps,
    })
}

/// Write the `--world-pack-order-file` for the staged pack and return its path.
pub fn write_order_file(repository: &Path, staged: &Path) -> Result<std::path::PathBuf> {
    let order = layout(&contents(repository, staged)?);
    let mut text = String::from("# WORLD.PAK chunk order (host/hl-build/pack_order.rs)\n");
    for id in &order {
        text.push_str(&format!("{id}\n"));
    }
    let path = staged.join("world-pack-order.txt");
    fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PackContents {
        let mut ids: BTreeSet<u32> = BTreeSet::new();
        // Two rooms, their voices and sprites, the common chunks, three model
        // types (scientist 0 tier 0, houndeye 6 tier 0, battery 4 tier 1) and a
        // variant of type 6.
        ids.extend([0, 1, 2, 3, 3100, 3101, 3300, 3301]);
        ids.extend([3000, 3001, 3002, 3003, 3004, 3050]);
        ids.extend(1000..1015);
        ids.extend([1300, 1100, 1306, 1304, 1500, 2106]);
        PackContents {
            ids,
            stream_bytes: BTreeMap::from([(0, 9000), (6, 20000), (4, 100)]),
            variant_types: BTreeMap::from([(2106, 6)]),
            variant_maps: BTreeMap::from([(2106, BTreeSet::from([0, 1]))]),
        }
    }

    #[test]
    fn layout_places_every_chunk_once() {
        let contents = sample();
        let order = layout(&contents);
        assert_eq!(order.len(), contents.ids.len());
        assert_eq!(order.iter().copied().collect::<BTreeSet<_>>(), contents.ids);
    }

    #[test]
    fn layout_follows_the_load_order() {
        let order = layout(&sample());
        let pos = |id: u32| order.iter().position(|&x| x == id).unwrap();
        // Common chunks first, in the order a load reads them, then models, then rooms.
        assert!(pos(3001) < pos(3002) && pos(3002) < pos(3004) && pos(3004) < pos(3003));
        assert!(pos(3050) < pos(1000) && pos(1000) < pos(1014));
        assert!(pos(1014) < pos(1306) && pos(1304) > pos(1306));
        // Larger geometry streams first inside a tier; items follow combat types.
        assert!(pos(1306) < pos(1300) && pos(1300) < pos(1304));
        // A type's geometry, split texture, carry chunk and variant stay together.
        assert!(pos(1300) < pos(1100) && pos(1100) < pos(1500));
        assert!(pos(1306) < pos(2106) && pos(2106) < pos(1300));
        // Per room: textures, voices, sprites, world.
        assert!(pos(1304) < pos(1));
        assert!(pos(1) < pos(3100) && pos(3100) < pos(3300) && pos(3300) < pos(0));
        assert!(pos(0) < pos(3) && pos(3) < pos(3101));
    }

    #[test]
    fn unknown_chunks_land_last() {
        let mut contents = sample();
        contents.ids.insert(2999);
        contents.ids.insert(4000);
        let order = layout(&contents);
        assert_eq!(&order[order.len() - 2..], &[2999, 4000]);
    }
}

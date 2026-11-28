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
//!
//! Room clusters: each room's cluster also gets its own copy of the shared
//! chunks its load reads, under alias ids (`room_alias`), in read order: the
//! core sound bank before the textures, then after the world the glock and the
//! models the room places, in the order the game streams them. A load then
//! seeks once to the cluster and reads on from there; every read that starts
//! where the previous one ended costs about 1.6 VBlanks of drive time instead of
//! about 20 for any other (PSoXide drive model, measured 2026-11-27). The disc
//! grows by the copies. Carried actors and the player's own weapon models are
//! not predictable from the cook and keep reading the shared chunk.

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

/// Alias ids of the room-cluster copies: `ROOM_ALIAS_BASE + room * ROOM_ALIAS_STRIDE +
/// (id - 1000)` for a shared chunk `id` in `1000..3100` (`game/src/main.rs` has the twin).
pub const ROOM_ALIAS_BASE: u32 = 1 << 20;
pub const ROOM_ALIAS_STRIDE: u32 = 4096;
/// Most copies one room may get; `game/build.rs` sizes the pack-table cache with it.
pub const ROOM_ALIAS_MAX: usize = 48;
/// The per-map core sound bank table (`sfx::CORE_TABLE_CHUNK`, "HCPT").
const CORE_TABLE_CHUNK: u32 = 3052;

/// The alias id of room `room`'s copy of shared chunk `id`.
pub fn room_alias(room: u32, id: u32) -> u32 {
    ROOM_ALIAS_BASE + room * ROOM_ALIAS_STRIDE + (id - VIEWMODEL_FIRST)
}

/// One room's copies, as pack ids in read order: `head` before its textures, `tail` after
/// its world. Entries are alias ids, or the real id of a variant only this room uses.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Cluster {
    pub head: Vec<u32>,
    pub tail: Vec<u32>,
}

/// Every room's cluster copies, from the cooked maps (their props), the variant table and
/// the core-bank table. The model order is the game's (`stream_map_models`: importance
/// pass, then largest stream first, geometry then its split texture).
pub fn cluster_plan(repository: &Path, contents: &PackContents) -> Result<BTreeMap<u32, Cluster>> {
    let table = repository.join(format!("data/sfx/chunk_{CORE_TABLE_CHUNK}.psxa"));
    let core_table = fs::read(&table).map_err(|error| {
        format!(
            "room clusters need the core-bank table {}: {error}",
            table.display()
        )
    })?;
    if core_table.len() < 8 || &core_table[..4] != b"HCPT" {
        return Err(format!("{} is not a core-bank table (HCPT)", table.display()).into());
    }
    let core_profile = |room: u32| -> Option<u32> {
        if core_table.len() < 8 || &core_table[..4] != b"HCPT" {
            return None;
        }
        let count = u16::from_le_bytes([core_table[4], core_table[5]]) as u32;
        let at = 8 + 2 * room as usize;
        if room >= count || at + 2 > core_table.len() {
            return None;
        }
        let id = u32::from(u16::from_le_bytes([core_table[at], core_table[at + 1]]));
        (id != 0 && contents.ids.contains(&id)).then_some(id)
    };
    // (map, type) -> variant chunk, for the rows a map's own props use.
    let mut variant_of: BTreeMap<(u32, u8), u32> = BTreeMap::new();
    let variants = repository.join("data/modelpack/map-model-variants.txt");
    if let Ok(text) = fs::read_to_string(&variants) {
        for line in text
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let f: Vec<&str> = line.split('|').collect();
            let (Some(map), Some(ty), Some(chunk)) = (
                f.first().and_then(|v| v.parse::<u32>().ok()),
                f.get(2).and_then(|v| v.parse::<u8>().ok()),
                f.get(3).and_then(|v| v.parse::<u32>().ok()),
            ) else {
                continue;
            };
            if f.get(4).map(|v| v.trim()) != Some("1") {
                variant_of.insert((map, ty), chunk);
            }
        }
    }
    let rank = |ty: u8| {
        let bytes = contents.stream_bytes.get(&ty).copied().unwrap_or(0);
        (tier(ty), std::cmp::Reverse(bytes), ty)
    };
    let rooms: BTreeSet<u32> = contents
        .ids
        .iter()
        .filter(|&&id| id < VIEWMODEL_FIRST)
        .map(|&id| id / 2)
        .collect();
    let mut plan = BTreeMap::new();
    for room in rooms {
        let world = repository.join(format!("data/rooms/room_{}.psxc", 2 * room));
        let placed = super::model_audit::map_actor_types(&world).map_err(|error| {
            format!(
                "room clusters: cooked map {} does not read: {error}",
                world.display()
            )
        })?;
        let mut cluster = Cluster::default();
        if let Some(profile) = core_profile(room) {
            cluster.head.push(room_alias(room, profile));
        }
        if contents.ids.contains(&VIEWMODEL_FIRST) {
            cluster.tail.push(room_alias(room, VIEWMODEL_FIRST));
        }
        let mut types: Vec<u8> = placed
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        types.sort_by_key(|&ty| rank(ty));
        for ty in types {
            let geometry = variant_of
                .get(&(room, ty))
                .copied()
                .unwrap_or(GEOMETRY_BASE + u32::from(ty));
            if contents.ids.contains(&geometry) {
                cluster
                    .tail
                    .push(if exclusive_map(geometry, contents) == Some(room) {
                        geometry
                    } else {
                        room_alias(room, geometry)
                    });
            }
            let texture = TEXTURE_BASE + u32::from(ty);
            if contents.ids.contains(&texture) {
                cluster.tail.push(room_alias(room, texture));
            }
        }
        let copies = cluster.head.len() + cluster.tail.len();
        if copies > ROOM_ALIAS_MAX {
            return Err(format!(
                "room {room} wants {copies} cluster copies, more than {ROOM_ALIAS_MAX}"
            )
            .into());
        }
        plan.insert(room, cluster);
    }
    Ok(plan)
}

/// The shared chunk an alias id stands for.
pub fn alias_source(alias: u32) -> Option<u32> {
    let rel = alias.checked_sub(ROOM_ALIAS_BASE)?;
    Some(VIEWMODEL_FIRST + rel % ROOM_ALIAS_STRIDE)
}

/// Stage the room-cluster copies into `staged/clusters`: each alias is a copy of the staged
/// shared chunk's file.
pub fn stage_room_clusters(repository: &Path, staged: &Path) -> Result<()> {
    let contents = contents(repository, staged)?;
    let plan = cluster_plan(repository, &contents)?;
    let dir = staged.join("clusters");
    fs::create_dir_all(&dir)?;
    let mut files: BTreeMap<u32, std::path::PathBuf> = BTreeMap::new();
    for family in ["modelpack", "sfx"] {
        for entry in fs::read_dir(staged.join(family))? {
            let path = entry?.path();
            if let Some(id) = id_of(&path, "chunk_") {
                files.insert(id, path);
            }
        }
    }
    let (mut copies, mut bytes) = (0usize, 0u64);
    for cluster in plan.values() {
        for &alias in cluster.head.iter().chain(&cluster.tail) {
            let Some(source) = alias_source(alias).filter(|_| alias >= ROOM_ALIAS_BASE) else {
                continue;
            };
            let from = files
                .get(&source)
                .ok_or_else(|| format!("cluster copy of chunk {source}: no staged file"))?;
            let ext = from.extension().and_then(|e| e.to_str()).unwrap_or("bin");
            fs::copy(from, dir.join(format!("chunk_{alias}.{ext}")))?;
            copies += 1;
            bytes += fs::metadata(from)?.len();
        }
    }
    println!(
        "room clusters: {copies} copies over {} rooms, {bytes} bytes staged",
        plan.len()
    );
    Ok(())
}

/// The chunk ids in disc order (`clusters` empty: no room-cluster copies).
pub fn layout(contents: &PackContents, clusters: &BTreeMap<u32, Cluster>) -> Vec<u32> {
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
    for id in contents
        .ids
        .range(3000..3100)
        .filter(|id| !banks.contains(id))
    {
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
    let no_cluster = Cluster::default();
    for room in rooms {
        let cluster = clusters.get(&room).unwrap_or(&no_cluster);
        for &id in &cluster.head {
            place(id, &mut order);
        }
        for id in [
            2 * room + 1,
            VOICE_BASE + room,
            SPRITE_BASE + room,
            2 * room,
        ] {
            place(id, &mut order);
        }
        for &id in &cluster.tail {
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
        ("clusters", "chunk_"),
    ] {
        let Ok(dir) = fs::read_dir(staged.join(family)) else {
            continue; // clusters only exists with the switch on
        };
        for entry in dir {
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
    let contents = contents(repository, staged)?;
    let clusters = cluster_plan(repository, &contents)?;
    let order = layout(&contents, &clusters);
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
        let order = layout(&contents, &BTreeMap::new());
        assert_eq!(order.len(), contents.ids.len());
        assert_eq!(order.iter().copied().collect::<BTreeSet<_>>(), contents.ids);
    }

    #[test]
    fn layout_follows_the_load_order() {
        let order = layout(&sample(), &BTreeMap::new());
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
    fn cluster_copies_follow_the_room_read_order() {
        let mut contents = sample();
        let (core, glock, model) = (
            room_alias(1, 3050),
            room_alias(1, 1000),
            room_alias(1, 1306),
        );
        contents.ids.extend([core, glock, model]);
        let clusters = BTreeMap::from([(
            1,
            Cluster {
                head: vec![core],
                tail: vec![glock, model],
            },
        )]);
        let order = layout(&contents, &clusters);
        let pos = |id: u32| order.iter().position(|&x| x == id).unwrap();
        assert_eq!(order.len(), contents.ids.len());
        // Room 1: core copy, textures, voices, sprites, world, glock copy, model copy.
        let run = [core, 3, 3101, 3301, 2, glock, model];
        for pair in run.windows(2) {
            assert_eq!(pos(pair[1]), pos(pair[0]) + 1);
        }
        assert_eq!(alias_source(model), Some(1306));
    }

    #[test]
    fn unknown_chunks_land_last() {
        let mut contents = sample();
        contents.ids.insert(2999);
        contents.ids.insert(4000);
        let order = layout(&contents, &BTreeMap::new());
        assert_eq!(&order[order.len() - 2..], &[2999, 4000]);
    }
}

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{run, HostBins, Result};

const MAP_MODEL_CHUNK_BASE: u32 = 2_100;

type Entity = HashMap<String, String>;

#[derive(Clone)]
struct RosterEntry {
    model: String,
    fields: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GeometryKey {
    mode: &'static str,
    model: String,
    fields: Vec<String>,
    split: bool,
}

#[derive(Clone)]
struct MapVariant {
    ty: u8,
    geometry: GeometryKey,
    manifest_fields: Vec<String>,
    script_names: Vec<String>,
    incoming_only: bool,
}

fn u32le(data: &[u8], offset: usize, what: &Path) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| format!("{}: truncated u32 at {offset}", what.display()))?;
    Ok(u32::from_le_bytes(bytes.try_into()?))
}

fn quoted(input: &[u8], cursor: &mut usize) -> Option<String> {
    while *cursor < input.len() && input[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
    if input.get(*cursor) != Some(&b'"') {
        return None;
    }
    *cursor += 1;
    let start = *cursor;
    while *cursor < input.len() && input[*cursor] != b'"' {
        *cursor += 1;
    }
    let value = String::from_utf8_lossy(&input[start..*cursor]).into_owned();
    *cursor += usize::from(*cursor < input.len());
    Some(value)
}

fn parse_entities(input: &[u8]) -> Vec<Entity> {
    let mut cursor = 0usize;
    let mut entities = Vec::new();
    while cursor < input.len() {
        while cursor < input.len() && input[cursor] != b'{' {
            cursor += 1;
        }
        if cursor == input.len() {
            break;
        }
        cursor += 1;
        let mut entity = Entity::new();
        loop {
            while cursor < input.len() && input[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor == input.len() || input[cursor] == b'}' {
                cursor += usize::from(cursor < input.len());
                break;
            }
            let Some(key) = quoted(input, &mut cursor) else {
                break;
            };
            let Some(value) = quoted(input, &mut cursor) else {
                break;
            };
            entity.insert(key, value);
        }
        entities.push(entity);
    }
    entities
}

fn bsp_entities(path: &Path) -> Result<Vec<Entity>> {
    let data = fs::read(path)?;
    if data.len() < 124 || u32le(&data, 0, path)? != 30 {
        return Err(format!("{}: not a GoldSrc BSP30 map", path.display()).into());
    }
    let offset = u32le(&data, 4, path)? as usize;
    let len = u32le(&data, 8, path)? as usize;
    let lump = data
        .get(offset..offset.saturating_add(len))
        .ok_or_else(|| format!("{}: truncated entity lump", path.display()))?;
    Ok(parse_entities(lump))
}

fn load_roster(path: &Path) -> Result<BTreeMap<u8, RosterEntry>> {
    let mut roster = BTreeMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.splitn(3, '|');
        let ty = fields.next().ok_or("model roster type missing")?.parse()?;
        let model = fields
            .next()
            .ok_or("model roster basename missing")?
            .to_string();
        let fields = fields
            .next()
            .ok_or("model roster clip specification missing")?
            .split(',')
            .map(str::to_string)
            .collect();
        roster.insert(ty, RosterEntry { model, fields });
    }
    Ok(roster)
}

fn load_clip_slots(path: &Path) -> Result<HashMap<(u8, String), usize>> {
    let mut clips = HashMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let mut fields = line.split('|');
        let (Some(ty), Some(name), Some(slot)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if let (Ok(ty), Ok(slot)) = (ty.parse::<u8>(), slot.parse::<usize>()) {
            clips.insert((ty, name.to_ascii_lowercase()), slot);
        }
    }
    Ok(clips)
}

fn load_transition_types(path: &Path) -> Result<HashMap<String, HashMap<String, u8>>> {
    let mut maps: HashMap<String, HashMap<String, u8>> = HashMap::new();
    for line in fs::read_to_string(path)?.lines() {
        let fields = line.split('|').collect::<Vec<_>>();
        if fields.len() < 3 {
            continue;
        }
        if let Ok(ty) = fields[1].parse::<u8>() {
            maps.entry(fields[0].to_string())
                .or_default()
                .insert(fields[2].to_string(), ty);
        }
    }
    Ok(maps)
}

fn direct_monster_type(classname: &str) -> Option<u8> {
    Some(match classname {
        "monster_scientist" => 0,
        "monster_barney" => 1,
        "monster_headcrab" => 2,
        "monster_zombie" => 5,
        "monster_houndeye" => 6,
        "monster_bullchicken" => 7,
        "monster_human_grunt" => 8,
        "monster_alien_slave" => 9,
        "monster_alien_grunt" => 10,
        "monster_alien_controller" => 11,
        "monster_barnacle" => 12,
        "monster_leech" => 13,
        "monster_cockroach" => 14,
        "monster_gman" => 15,
        "monster_gargantua" => 16,
        "monster_nihilanth" => 17,
        "monster_bigmomma" => 18,
        "monster_ichthyosaur" => 19,
        "monster_sentry" => 20,
        "monster_turret" => 21,
        "monster_miniturret" => 22,
        "monster_apache" => 23,
        "monster_flyer_flock" => 24,
        "monster_sitting_scientist" => 25,
        "monster_tentacle" => 50,
        "monster_human_assassin" => 51,
        "monster_tripmine" => 57,
        "monster_snark" => 58,
        "monster_babycrab" => 59,
        "monster_rat" => 60,
        "monster_osprey" => 61,
        "xen_plantlight" => 73,
        "xen_tree" => 74,
        "xen_hair" => 68,
        "xen_spore_small" => 70,
        "xen_spore_medium" => 69,
        "xen_spore_large" => 71,
        _ => return None,
    })
}

fn generic_monster_type(entity: &Entity) -> Option<u8> {
    let model = entity
        .get("model")?
        .replace('\\', "/")
        .rsplit('/')
        .next()?
        .to_ascii_lowercase();
    Some(match model.as_str() {
        "scientist.mdl" => 0,
        "barney.mdl" => 1,
        "loader.mdl" => 52,
        "forklift.mdl" => 53,
        "holo.mdl" => 56,
        "gib_legbone.mdl" => 62,
        "pelvis.mdl" => 63,
        "ribcage.mdl" => 64,
        "riblet1.mdl" => 65,
        "zombiegibs1.mdl" => 66,
        "filecabinet.mdl" => 67,
        "hair.mdl" => 68,
        "fungus.mdl" => 69,
        "fungus(small).mdl" => 70,
        "fungus(large).mdl" => 71,
        "pipe_bubbles.mdl" => 72,
        _ => return None,
    })
}

fn base_actor_type(entity: &Entity) -> Option<u8> {
    let classname = entity.get("classname").map(String::as_str).unwrap_or("");
    match classname {
        "monster_generic" | "monster_furniture" | "cycler" => generic_monster_type(entity),
        "monstermaker" => {
            direct_monster_type(entity.get("monstertype").map(String::as_str).unwrap_or(""))
        }
        "monster_scientist_dead" | "monster_hevsuit_dead" => Some(0),
        "monster_barney_dead" => Some(1),
        "monster_hgrunt_dead" => Some(8),
        _ => direct_monster_type(classname),
    }
}

fn cooked_actor_type(
    entity: &Entity,
    vent_zombies: bool,
    scripted_sitters: &BTreeSet<String>,
) -> Option<u8> {
    let mut ty = base_actor_type(entity)?;
    if ty == 5 && vent_zombies {
        ty = 55;
    } else if ty == 0
        && entity
            .get("targetname")
            .is_some_and(|name| scripted_sitters.contains(name))
    {
        ty = 54;
    }
    Some(ty)
}

fn uses_map_variant(ty: u8) -> bool {
    matches!(ty, 0..=2 | 5..=25 | 50..=56)
}

fn base_clip_count(ty: u8) -> usize {
    match ty {
        14 => 2,
        25 => 7,
        56 => 1,
        _ => 5,
    }
}

fn carry_field(field: &str) -> String {
    let Some((sequence, samples)) = field.rsplit_once(':') else {
        return field.to_string();
    };
    let samples = samples.parse::<usize>().unwrap_or(1).clamp(1, 2);
    format!("{sequence}:{samples}")
}

fn special_fields(map: &str, ty: u8) -> Option<(&'static str, Vec<String>)> {
    let (mode, fields): (&str, &[&str]) = match (map, ty) {
        ("c4a1b", 16) => ("--mdl7", &["2:2", "4:2", "6:2", "12:3", "14:4"]),
        ("c4a3", 16) => ("--mdl7-lean", &["2:1", "4:1", "6:1", "12:1", "14:1"]),
        ("c4a3", 19) => ("--mdl7", &["0:1", "1:1", "8:1", "5:1", "3:1"]),
        ("c1a2b", 5) => (
            "--mdl7",
            &["0:4", "10:4", "8:5", "3:2", "17:3", "eatbody:2"],
        ),
        ("c1a2b" | "c4a3", 9) => ("--mdl7", &["0:4", "4:4", "12:4", "13:2", "19:3", "grab:2"]),
        _ => return None,
    };
    Some((
        mode,
        fields.iter().map(|field| (*field).to_string()).collect(),
    ))
}

/// The opening chapter -- the Black Mesa Inbound tram and Anomalous Materials.
/// These maps carry few actors (60-146 KiB of pool slack each), so their
/// scripted scientists/barneys bake the sequence the map actually names instead
/// of the roster's shared RAM stand-in. Every other chapter keeps the stand-in:
/// the fleet peak lives on c1a2b/c1a3, which have kilobytes, not tens of them.
const VERBATIM_SCRIPT_MAPS: &[&str] = &[
    "c0a0", "c0a0a", "c0a0b", "c0a0c", "c0a0d", "c0a0e", "c1a0", "c1a0a", "c1a0b", "c1a0c",
    "c1a0d", "c1a0e",
];

/// Maps that cannot afford a verbatim bake and keep the roster's stand-in. The
/// aliases exist only as a RAM compromise, so the default is to ignore them
/// everywhere; this list is what the roster audit pushes back. It is EMPTY now
/// that the map and the model pool share one arena: c1a2b/c1a1b/c4a1b were only
/// here because the pool was sized for a map bigger than their own, and each has
/// tens of kilobytes of slack once the pool starts where their BSP ends.
/// Keep it minimal and re-measure before adding to it.
const STAND_IN_SCRIPT_MAPS: &[&str] = &[];

/// Chapter 1 is where the player stands and watches, and its maps carry few
/// actors, so its scripts keep eight retained poses. Everywhere else four is the
/// baseline, which the fleet peak absorbs once the three maps that cannot afford
/// a verbatim bake at all are excluded.
fn verbatim_poses(map: &str) -> usize {
    if VERBATIM_SCRIPT_MAPS.contains(&map) {
        8
    } else {
        4
    }
}

/// Roster aliases that exist because the model has no such sequence at all
/// (barney is never authored sitting twice; the loader/forklift idles are named
/// differently). Baking these verbatim would ask `cook_mdl` for a missing label.
fn alias_is_mandatory(ty: u8, name: &str) -> bool {
    matches!(
        (ty, name),
        (1, "sit2") | (1, "sit3") | (52, "idle1") | (53, "idle1")
    )
}

/// Script names this map should bake for real rather than resolve through the
/// roster's `name=other_name` stand-in. Slot aliases (`name=@0`) are left alone:
/// they point at a base clip that is already resident.
fn verbatim_names(map: &str, ty: u8, roster: &RosterEntry) -> BTreeSet<String> {
    if STAND_IN_SCRIPT_MAPS.contains(&map) {
        return BTreeSet::new();
    }
    roster
        .fields
        .iter()
        .filter_map(|field| field.split_once('='))
        .filter(|(name, _)| !name.eq_ignore_ascii_case("body"))
        .filter(|(name, target)| !target.starts_with('@') && !alias_is_mandatory(ty, name))
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect()
}

fn field_label(field: &str) -> &str {
    field
        .split_once('=')
        .map(|pair| pair.0)
        .or_else(|| field.rsplit_once(':').map(|pair| pair.0))
        .unwrap_or(field)
}

fn build_variant(
    map: &str,
    ty: u8,
    scripts: &BTreeSet<String>,
    incoming_only: bool,
    roster: &RosterEntry,
    clip_slots: &HashMap<(u8, String), usize>,
) -> MapVariant {
    let global_actual = roster
        .fields
        .iter()
        .filter(|field| !field.contains('='))
        .cloned()
        .collect::<Vec<_>>();
    let (mode, available) =
        special_fields(map, ty).unwrap_or_else(|| ("--mdl7", global_actual.clone()));
    let base_count = base_clip_count(ty).min(available.len());
    let mut geometry_fields = available[..base_count].to_vec();
    if incoming_only {
        geometry_fields = geometry_fields
            .iter()
            .map(|field| carry_field(field))
            .collect();
    }
    let mut manifest_fields = geometry_fields.clone();
    let mut local_for_global = BTreeMap::new();
    for slot in 0..base_count {
        local_for_global.insert(slot, slot);
    }

    let verbatim = verbatim_names(map, ty, roster);
    for name in scripts {
        let alias = (!verbatim.contains(&name.to_ascii_lowercase()))
            .then(|| clip_slots.get(&(ty, name.to_ascii_lowercase())))
            .flatten();
        if let Some(&global_slot) = alias {
            if !local_for_global.contains_key(&global_slot) {
                let source = available
                    .get(global_slot)
                    .or_else(|| global_actual.get(global_slot));
                if let Some(source) = source.filter(|field| !field.contains('=')) {
                    let mut source = source.clone();
                    if incoming_only && global_slot < 5 {
                        source = carry_field(&source);
                    }
                    let local = geometry_fields.len();
                    geometry_fields.push(source.clone());
                    manifest_fields.push(source);
                    local_for_global.insert(global_slot, local);
                }
            }
            if let Some(&local) = local_for_global.get(&global_slot) {
                if !manifest_fields
                    .iter()
                    .any(|field| field_label(field).eq_ignore_ascii_case(name))
                {
                    manifest_fields.push(format!("{name}=@{local}"));
                }
                continue;
            }
        }
        if !manifest_fields
            .iter()
            .any(|field| field_label(field).eq_ignore_ascii_case(name))
        {
            // Previously unsupported retail scripts become real HMD8 clips.
            // Four retained poses are the conservative quality/RAM baseline.
            // The opening chapter is where the player stands and watches these
            // gestures, and it has the pool slack the parity report asked to
            // spend: eight poses roughly halve its reconstruction error.
            let poses =
                if ty == 0 && hl_format::map::SCIENTIST_CORPSE_POSES.contains(&name.as_str()) {
                    1
                } else {
                    verbatim_poses(map)
                };
            geometry_fields.push(format!("{name}:{poses}"));
            manifest_fields.push(format!("{name}:{poses}"));
        }
    }

    MapVariant {
        ty,
        geometry: GeometryKey {
            mode,
            model: roster.model.clone(),
            fields: geometry_fields,
            split: split_model_stream(ty),
        },
        manifest_fields,
        script_names: scripts.iter().cloned().collect(),
        incoming_only,
    }
}

fn split_model_stream(ty: u8) -> bool {
    matches!(ty, 0 | 1 | 15 | 16 | 17 | 25 | 52 | 54)
}

fn plan_maps(
    valve: &Path,
    maps: &[&str],
    roster: &BTreeMap<u8, RosterEntry>,
    clip_slots: &HashMap<(u8, String), usize>,
    transitions: &HashMap<String, HashMap<String, u8>>,
) -> Result<(Vec<Vec<MapVariant>>, Vec<String>)> {
    let mut planned = Vec::with_capacity(maps.len());
    let mut unresolved = Vec::new();
    for &map in maps {
        let entities = bsp_entities(&valve.join("maps").join(format!("{map}.bsp")))?;
        let vent_zombies = entities.iter().any(|entity| {
            matches!(
                entity.get("classname").map(String::as_str),
                Some("scripted_sequence" | "aiscripted_sequence")
            ) && [entity.get("m_iszIdle"), entity.get("m_iszPlay")]
                .into_iter()
                .flatten()
                .any(|name| {
                    name.eq_ignore_ascii_case("ventclimbidle")
                        || name.eq_ignore_ascii_case("ventclimb")
                })
        });
        let scripted_sitters = entities
            .iter()
            .filter(|entity| {
                matches!(
                    entity.get("classname").map(String::as_str),
                    Some("scripted_sequence" | "aiscripted_sequence")
                ) && entity
                    .get("m_iszIdle")
                    .is_some_and(|name| name.eq_ignore_ascii_case("sitidle"))
            })
            .filter_map(|entity| entity.get("m_iszEntity").cloned())
            .collect::<BTreeSet<_>>();
        let mut static_types = BTreeSet::new();
        let mut target_types = HashMap::new();
        for entity in &entities {
            let Some(ty) = cooked_actor_type(entity, vent_zombies, &scripted_sitters) else {
                continue;
            };
            if uses_map_variant(ty) {
                static_types.insert(ty);
            }
            if let Some(name) = entity.get("targetname").filter(|name| !name.is_empty()) {
                target_types.insert(name.clone(), ty);
            }
        }
        if let Some(incoming) = transitions.get(map) {
            for (name, &ty) in incoming {
                target_types.insert(name.clone(), ty);
            }
        }

        let mut scripts: BTreeMap<u8, BTreeSet<String>> = BTreeMap::new();
        for entity in entities.iter().filter(|entity| {
            matches!(
                entity.get("classname").map(String::as_str),
                Some("scripted_sequence" | "aiscripted_sequence")
            )
        }) {
            let target = entity.get("m_iszEntity").map(String::as_str).unwrap_or("");
            let ty = target_types
                .get(target)
                .copied()
                .or_else(|| direct_monster_type(target));
            let names = [entity.get("m_iszPlay"), entity.get("m_iszIdle")]
                .into_iter()
                .flatten()
                .filter(|name| !name.is_empty())
                .cloned()
                .collect::<Vec<_>>();
            let Some(ty) = ty else {
                if !names.is_empty() {
                    unresolved.push(format!("{map}|{target}|{}", names.join("+")));
                }
                continue;
            };
            if uses_map_variant(ty) {
                scripts.entry(ty).or_default().extend(names.into_iter());
            }
        }

        for entity in &entities {
            if entity.get("classname").map(String::as_str) == Some("monster_scientist_dead") {
                let pose = entity
                    .get("pose")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let name = hl_format::map::SCIENTIST_CORPSE_POSES
                    .get(pose)
                    .ok_or_else(|| format!("{map}: invalid scientist corpse pose {pose}"))?;
                scripts.entry(0).or_default().insert((*name).to_owned());
            }
        }

        let mut types = static_types.clone();
        if let Some(incoming) = transitions.get(map) {
            types.extend(
                incoming
                    .values()
                    .copied()
                    .filter(|&ty| uses_map_variant(ty)),
            );
        }
        types.extend(scripts.keys().copied());
        let mut variants = Vec::new();
        for ty in types {
            let roster_entry = roster
                .get(&ty)
                .ok_or_else(|| format!("{map}: model type {ty} is absent from the roster"))?;
            variants.push(build_variant(
                map,
                ty,
                scripts.get(&ty).unwrap_or(&BTreeSet::new()),
                !static_types.contains(&ty),
                roster_entry,
                clip_slots,
            ));
        }
        planned.push(variants);
    }
    Ok((planned, unresolved))
}

pub fn cook_map_variants(
    repository: &Path,
    valve: &Path,
    bins: &HostBins,
    maps: &[&str],
) -> Result<()> {
    let model_pack = repository.join("data/modelpack");
    let roster_path = repository.join("host/hl-content/model-roster.txt");
    let roster = load_roster(&roster_path)?;
    let clip_slots = load_clip_slots(&model_pack.join("clips.txt"))?;
    let transitions = load_transition_types(&model_pack.join("transition_props.txt"))?;
    let (planned, unresolved) = plan_maps(valve, maps, &roster, &clip_slots, &transitions)?;

    let mut unique = BTreeSet::new();
    for variants in &planned {
        unique.extend(variants.iter().map(|variant| variant.geometry.clone()));
    }
    if unique.len() > (3_000 - MAP_MODEL_CHUNK_BASE) as usize {
        return Err(format!("too many map-model variants: {}", unique.len()).into());
    }
    let chunk_for = unique
        .into_iter()
        .enumerate()
        .map(|(index, key)| (key, MAP_MODEL_CHUNK_BASE + index as u32))
        .collect::<BTreeMap<_, _>>();

    for (key, &chunk_id) in &chunk_for {
        let geometry = model_pack.join(format!("chunk_{chunk_id}.psxm"));
        let texture = model_pack.join(format!("variant_{chunk_id}.tex"));
        let mut command = Command::new(&bins.bsp);
        command
            .arg(key.mode)
            .arg(valve.join("models").join(format!("{}.mdl", key.model)))
            .arg(&geometry)
            .arg(key.fields.join(","))
            .arg(&texture);
        run(
            &mut command,
            &format!("cook map HMD8 variant {chunk_id} {}", key.model),
        )?;
        if key.split {
            fs::remove_file(&texture)?;
        } else {
            let mut merge = Command::new(&bins.content);
            merge.arg("merge-model").arg(&geometry).arg(&texture);
            run(&mut merge, &format!("merge map HMD8 variant {chunk_id}"))?;
        }
    }

    let clip_dir = model_pack.join("map-clips");
    if clip_dir.exists() {
        fs::remove_dir_all(&clip_dir)?;
    }
    fs::create_dir_all(&clip_dir)?;
    let mut index = String::from("# map_index|map|type|chunk_id|incoming_only\n");
    let mut report =
        String::from("map_index,map,type,chunk_id,incoming_only,cooked_clips,script_clips\n");
    for (map_index, (&map, variants)) in maps.iter().zip(&planned).enumerate() {
        let roster_file = clip_dir.join(format!("roster_{map_index}.txt"));
        let mut roster_text = String::new();
        for variant in variants {
            let chunk_id = chunk_for[&variant.geometry];
            roster_text.push_str(&format!(
                "{}|{}|{}\n",
                variant.ty,
                variant.geometry.model,
                variant.manifest_fields.join(",")
            ));
            index.push_str(&format!(
                "{map_index}|{map}|{}|{chunk_id}|{}\n",
                variant.ty,
                u8::from(variant.incoming_only)
            ));
            report.push_str(&format!(
                "{map_index},{map},{},{chunk_id},{},{},{}\n",
                variant.ty,
                u8::from(variant.incoming_only),
                variant.geometry.fields.len(),
                variant.script_names.join("+")
            ));
        }
        fs::write(&roster_file, roster_text)?;
        let clips_file = clip_dir.join(format!("clips_{map_index}.txt"));
        let mut command = Command::new(&bins.content);
        command
            .arg("clips")
            .arg(valve.join("models"))
            .arg(&roster_file)
            .arg(&clips_file);
        run(&mut command, &format!("generate {map} map clip manifest"))?;
        fs::remove_file(roster_file)?;
    }
    fs::write(model_pack.join("map-model-variants.txt"), index)?;
    let report_path = repository.join(".hlpsx/reports/model-clip-residency.csv");
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&report_path, report)?;
    let unresolved_path = repository.join(".hlpsx/reports/model-clip-unresolved.txt");
    fs::write(&unresolved_path, unresolved.join("\n"))?;
    println!(
        "map HMD8 variants: {} map/type uses -> {} unique chunks",
        planned.iter().map(Vec::len).sum::<usize>(),
        chunk_for.len()
    );
    println!("  residency: {}", report_path.display());
    if !unresolved.is_empty() {
        println!(
            "  unresolved scripted actors: {} (already unsupported; {})",
            unresolved.len(),
            unresolved_path.display()
        );
    }
    Ok(())
}

pub fn clips_manifest(repository: &Path, map_index: usize) -> PathBuf {
    repository
        .join("data/modelpack/map-clips")
        .join(format!("clips_{map_index}.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_only_retains_the_static_corpse_poses_it_uses() {
        let roster = RosterEntry {
            model: "scientist".into(),
            fields: ["13:4", "0:6", "24:6", "8:3", "31:3"]
                .map(str::to_owned)
                .to_vec(),
        };
        let scripts = BTreeSet::from(["lying_on_stomach".to_owned()]);
        let variant = build_variant("c1a0c", 0, &scripts, false, &roster, &HashMap::new());
        assert_eq!(variant.geometry.fields.len(), 6);
        assert_eq!(variant.geometry.fields[5], "lying_on_stomach:1");
        assert!(!variant
            .geometry
            .fields
            .iter()
            .any(|f| f.starts_with("dead_sitting")));
    }

    #[test]
    fn carry_fields_keep_at_most_two_poses() {
        assert_eq!(carry_field("walk:8"), "walk:2");
        assert_eq!(carry_field("idle:1"), "idle:1");
    }

    #[test]
    fn the_opening_chapter_bakes_soft_aliases_but_keeps_missing_sequences() {
        let barney = RosterEntry {
            model: "barney".to_string(),
            fields: ["0:4", "sit1:6", "relaxstand=sit1", "sit2=sit1", "idle1=@0"]
                .map(str::to_string)
                .to_vec(),
        };
        let opening = verbatim_names("c1a0d", 1, &barney);
        assert!(
            opening.contains("relaxstand"),
            "a standing barney must not reuse the sitting clip"
        );
        assert!(
            !opening.contains("sit2"),
            "barney.mdl has no sit2 sequence to bake"
        );
        assert!(
            !opening.contains("idle1"),
            "slot aliases already point at a resident clip"
        );
        // Verbatim everywhere: STAND_IN_SCRIPT_MAPS is empty since the map and
        // the model pool started sharing one arena. Only a map the roster audit
        // pushes back on would keep the stand-in.
        assert!(!verbatim_names("c2a5", 1, &barney).is_empty());
        assert!(!verbatim_names("c1a2b", 1, &barney).is_empty());
        // `body=N` is a cook-side bodygroup selector, never a clip stand-in.
        let tripmine = RosterEntry {
            model: "v_tripmine".to_string(),
            fields: ["body=3", "7:1"].map(str::to_string).to_vec(),
        };
        assert!(verbatim_names("c2a2d", 57, &tripmine).is_empty());
    }

    #[test]
    fn generic_barney_resolves_to_the_barney_stream() {
        let entity = HashMap::from([
            ("classname".to_string(), "monster_generic".to_string()),
            ("model".to_string(), "models/barney.mdl".to_string()),
        ]);
        assert_eq!(base_actor_type(&entity), Some(1));
    }
}

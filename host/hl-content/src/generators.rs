use crate::Result;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;

fn i32le(data: &[u8], offset: usize) -> Result<i32> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .ok_or_else(|| format!("i32 at {offset} exceeds {} bytes", data.len()))?
        .try_into()?;
    Ok(i32::from_le_bytes(bytes))
}

fn f32le(data: &[u8], offset: usize) -> Result<f32> {
    Ok(f32::from_bits(i32le(data, offset)? as u32))
}

fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes[..end].iter().map(|&b| b as char).collect()
}

pub fn merge_model(geometry: &Path, texture: &Path) -> Result<()> {
    let geom = fs::read(geometry)?;
    let tex = if texture.exists() {
        fs::read(texture)?
    } else {
        Vec::new()
    };
    let mut merged = Vec::with_capacity(8 + geom.len() + tex.len());
    merged.extend_from_slice(b"HMRG");
    merged.extend_from_slice(&(geom.len() as u32).to_le_bytes());
    merged.extend_from_slice(&geom);
    merged.extend_from_slice(&tex);
    fs::write(geometry, merged)?;
    if texture.exists() {
        fs::remove_file(texture)?;
    }
    Ok(())
}

fn sequence_hold_quanta(data: &[u8], sequence: usize) -> Result<u16> {
    let (_, count, table) = sequences(data)?;
    if sequence >= count {
        return Ok(0);
    }
    let desc = table + sequence * SEQDESC_BYTES;
    let fps = f32le(data, desc + 32)?;
    let frame_count = i32le(data, desc + 56)?;
    if !fps.is_finite() || fps <= 0.0 || frame_count <= 1 {
        return Ok(1);
    }
    Ok((((frame_count - 1) as f32 * 10.0 / fps).ceil() as i32).clamp(1, 1023) as u16)
}

pub fn clips(models: &Path, roster: &Path, output: &Path) -> Result<()> {
    let mut lines = Vec::new();
    for raw in fs::read_to_string(roster)?.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.splitn(3, '|');
        let ty = fields.next().ok_or("roster type missing")?;
        let model = fields.next().ok_or("roster model missing")?;
        let specs = fields.next().ok_or("roster specs missing")?;
        let data = fs::read(models.join(format!("{model}.mdl")))?;
        let (sequence_labels, sequence_count, _) = sequences(&data)?;
        let mut slot = 0usize;
        let mut slot_hold_quanta = Vec::<u16>::new();
        let mut named = HashMap::<String, usize>::new();
        let mut named_order = Vec::<String>::new();
        let mut aliases = Vec::<(String, String)>::new();
        for raw_token in specs.split(',') {
            let token = raw_token.trim();
            if token.is_empty() {
                continue;
            }
            if let Some((alias, target)) = token.split_once('=') {
                if alias.trim().eq_ignore_ascii_case("body") {
                    continue; // cook-side bodygroup selector, not a clip alias
                }
                aliases.push((
                    alias.trim().to_ascii_lowercase(),
                    target.trim().to_ascii_lowercase(),
                ));
                continue;
            }
            let label = token.split_once(':').map_or(token, |v| v.0).trim();
            let sequence = if let Ok(sequence) = label.parse::<i32>() {
                if sequence < 0 || sequence as usize >= sequence_count {
                    return Err(format!(
                        "type {ty} {model}: sequence {sequence} is outside 0..{}",
                        sequence_count.saturating_sub(1)
                    )
                    .into());
                }
                sequence as usize
            } else {
                let label = label.to_ascii_lowercase();
                if !named.contains_key(&label) {
                    named_order.push(label.clone());
                }
                named.insert(label.clone(), slot);
                sequence_labels
                    .get(&label)
                    .copied()
                    .ok_or_else(|| format!("type {ty} {model}: sequence {label:?} is missing"))?
            };
            slot_hold_quanta.push(sequence_hold_quanta(&data, sequence)?);
            slot += 1;
        }
        for name in named_order {
            let slot = named[&name];
            lines.push(format!("{ty}|{name}|{slot}|{}", slot_hold_quanta[slot]));
        }
        for (alias, target) in aliases {
            let resolved = target
                .strip_prefix('@')
                .and_then(|s| s.parse::<usize>().ok())
                .or_else(|| named.get(&target).copied());
            if let Some(slot) = resolved {
                let fallback = slot_hold_quanta.get(slot).copied().ok_or_else(|| {
                    format!("type {ty} {model}: alias {alias} target slot {slot} is missing")
                })?;
                let hold = if let Some(&sequence) = sequence_labels.get(&alias) {
                    sequence_hold_quanta(&data, sequence)?
                } else {
                    // Some retail maps ask a model for a label it does not
                    // actually own (Barney sit2/sit3). Their visual alias is
                    // intentional; inherit the target clip's real duration
                    // instead of emitting a zero-duration manifest entry.
                    fallback
                };
                lines.push(format!("{ty}|{alias}|{slot}|{hold}"));
            } else {
                return Err(
                    format!("type {ty} {model}: alias {alias}={target} target is unknown").into(),
                );
            }
        }
    }
    fs::write(
        output,
        lines.join("\n") + if lines.is_empty() { "" } else { "\n" },
    )?;
    Ok(())
}

const SEQDESC_BYTES: usize = 176;
const EVENT_BYTES: usize = 76;
const SCRIPT_EVENT_FIRE_TARGET: i32 = 1003;

fn sequences(data: &[u8]) -> Result<(HashMap<String, usize>, usize, usize)> {
    if data.len() < 212 || &data[..4] != b"IDST" {
        return Err("not a GoldSrc studio MDL".into());
    }
    let count = i32le(data, 164)?;
    let offset = i32le(data, 168)?;
    if count < 0 || offset < 0 || offset as usize + count as usize * SEQDESC_BYTES > data.len() {
        return Err("studio sequence table exceeds MDL".into());
    }
    let mut labels = HashMap::new();
    for index in 0..count as usize {
        let at = offset as usize + index * SEQDESC_BYTES;
        labels
            .entry(cstr(&data[at..at + 32]).to_ascii_lowercase())
            .or_insert(index);
    }
    Ok((labels, count as usize, offset as usize))
}

fn extract_events(data: &[u8], sequence: usize) -> Result<Vec<(u16, u16, String)>> {
    let (_, count, table) = sequences(data)?;
    if sequence >= count {
        return Ok(Vec::new());
    }
    let desc = table + sequence * SEQDESC_BYTES;
    let fps = f32le(data, desc + 32)?;
    let event_count = i32le(data, desc + 48)?;
    let event_offset = i32le(data, desc + 52)?;
    let frame_count = i32le(data, desc + 56)?;
    if !fps.is_finite() || fps <= 0.0 || event_count <= 0 {
        return Ok(Vec::new());
    }
    if event_offset < 0 || event_offset as usize + event_count as usize * EVENT_BYTES > data.len() {
        return Err("studio event table exceeds MDL".into());
    }
    let period = (((frame_count - 1).max(0) as f32 * 20.0 / fps).ceil() as i32)
        .clamp(1, u16::MAX as i32) as u16;
    let mut result = Vec::new();
    for index in 0..event_count as usize {
        let at = event_offset as usize + index * EVENT_BYTES;
        let frame = i32le(data, at)?;
        let event = i32le(data, at + 4)?;
        let target = cstr(&data[at + 12..at + EVENT_BYTES]).trim().to_string();
        if event != SCRIPT_EVENT_FIRE_TARGET || target.is_empty() || target.contains('|') {
            continue;
        }
        let tick =
            ((frame.max(0) as f32 * 20.0 / fps).ceil() as i32).clamp(1, u16::MAX as i32) as u16;
        result.push((tick, period, target));
    }
    Ok(result)
}

pub fn studio_events(models: &Path, roster: &Path, output: &Path) -> Result<()> {
    let mut records = Vec::new();
    let mut seen = HashSet::new();
    for raw in fs::read_to_string(roster)?.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.splitn(3, '|');
        let actor_type: u8 = fields.next().ok_or("roster type missing")?.parse()?;
        let model = fields.next().ok_or("roster model missing")?;
        let specs = fields.next().ok_or("roster specs missing")?;
        let data = fs::read(models.join(format!("{model}.mdl")))?;
        let (labels, sequence_count, _) = sequences(&data)?;
        for raw_token in specs.split(',') {
            let token = raw_token.trim();
            if token.is_empty() {
                continue;
            }
            let label = if let Some((label, _)) = token.split_once('=') {
                label.trim()
            } else {
                token.split_once(':').map_or(token, |v| v.0).trim()
            };
            let (sequence, script_name) = match label.parse::<i32>() {
                Ok(index) if index >= 0 => (index as usize, String::new()),
                _ => (
                    labels
                        .get(&label.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(usize::MAX),
                    label.to_ascii_lowercase(),
                ),
            };
            if sequence >= sequence_count || script_name.is_empty() {
                continue;
            }
            for (tick, period, target) in extract_events(&data, sequence)? {
                let record = format!("{actor_type}|{script_name}|{tick}|{period}|{target}");
                if seen.insert(record.clone()) {
                    records.push(record);
                }
            }
        }
    }
    fs::write(
        output,
        records.join("\n") + if records.is_empty() { "" } else { "\n" },
    )?;
    println!(
        "studio target events: {} -> {}",
        records.len(),
        output.display()
    );
    Ok(())
}

pub(crate) type Entity = HashMap<String, String>;

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
    let text = input[start..*cursor].iter().map(|&b| b as char).collect();
    *cursor += usize::from(*cursor < input.len());
    Some(text)
}

pub(crate) fn parse_entities_bytes(input: &[u8]) -> Vec<Entity> {
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

pub(crate) fn bsp_entities(path: &Path) -> Result<Vec<Entity>> {
    let data = fs::read(path)?;
    if data.len() < 124 || i32le(&data, 0)? != 30 {
        return Err(format!("{}: not a GoldSrc BSP30 map", path.display()).into());
    }
    let offset = i32le(&data, 4)?;
    let length = i32le(&data, 8)?;
    if offset < 0 || length < 0 || offset as usize + length as usize > data.len() {
        return Err(format!("{}: invalid entity lump", path.display()).into());
    }
    Ok(parse_entities_bytes(
        &data[offset as usize..offset as usize + length as usize],
    ))
}

fn base_actor_type(entity: &Entity) -> Option<u8> {
    let class = entity.get("classname").map(String::as_str).unwrap_or("");
    let direct = match class {
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
        _ => u8::MAX,
    };
    if direct != u8::MAX {
        return Some(direct);
    }
    if class != "monster_generic" {
        return None;
    }
    let model = entity
        .get("model")?
        .replace('\\', "/")
        .rsplit('/')
        .next()?
        .to_ascii_lowercase();
    match model.as_str() {
        "scientist.mdl" => Some(0),
        "barney.mdl" => Some(1),
        "loader.mdl" => Some(52),
        "forklift.mdl" => Some(53),
        "holo.mdl" => Some(56),
        _ => None,
    }
}

fn cooked_actor_type(entity: &Entity, map_entities: &[Entity]) -> Option<u8> {
    let resolved = base_actor_type(entity)?;
    if resolved == 5
        && map_entities.iter().any(|script| {
            script.get("classname").map(String::as_str) == Some("scripted_sequence")
                && matches!(
                    script.get("m_iszIdle").or_else(|| script.get("m_iszPlay")).map(|s| s.to_ascii_lowercase()),
                    Some(ref name) if name == "ventclimbidle" || name == "ventclimb"
                )
        })
    {
        return Some(55);
    }
    if resolved == 0 {
        let targetname = entity.get("targetname").map(String::as_str).unwrap_or("");
        if !targetname.is_empty()
            && map_entities.iter().any(|script| {
                script.get("classname").map(String::as_str) == Some("scripted_sequence")
                    && script.get("m_iszEntity").map(String::as_str) == Some(targetname)
                    && script
                        .get("m_iszIdle")
                        .map(|v| v.eq_ignore_ascii_case("sitidle"))
                        == Some(true)
            })
        {
            return Some(54);
        }
    }
    Some(resolved)
}

fn parse_vec3(value: &str) -> Option<[f32; 3]> {
    let mut fields = value.split_whitespace();
    let result = [
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
    ];
    fields.next().is_none().then_some(result)
}

fn entity_yaw(entity: &Entity) -> f32 {
    entity
        .get("angles")
        .and_then(|value| parse_vec3(value))
        .map(|angles| angles[1])
        .or_else(|| entity.get("angle").and_then(|value| value.parse().ok()))
        .unwrap_or(0.0)
}

fn landmark_origin(entities: &[Entity], name: &str) -> Option<[f32; 3]> {
    entities.iter().find_map(|entity| {
        (entity.get("classname").map(String::as_str) == Some("info_landmark")
            && entity.get("targetname").map(String::as_str) == Some(name))
        .then(|| entity.get("origin").and_then(|value| parse_vec3(value)))
        .flatten()
    })
}

/// Shortest deterministic landmark path from an actor's authored map to the
/// destination that scripts it. Each edge carries the common landmark name;
/// GoldSrc transition transforms are translations between those two origins.
fn transition_path(
    outgoing: &BTreeMap<String, Vec<(String, String)>>,
    source: &str,
    destination: &str,
) -> Option<Vec<(String, String, String)>> {
    if source == destination {
        return Some(Vec::new());
    }
    let mut queue = VecDeque::from([source.to_string()]);
    let mut previous = BTreeMap::<String, (String, String)>::new();
    let mut visited = BTreeSet::from([source.to_string()]);
    while let Some(map) = queue.pop_front() {
        for (next, landmark) in outgoing.get(&map).into_iter().flatten() {
            if !visited.insert(next.clone()) {
                continue;
            }
            previous.insert(next.clone(), (map.clone(), landmark.clone()));
            if next == destination {
                let mut path = Vec::new();
                let mut cursor = destination.to_string();
                while cursor != source {
                    let (prior, edge_landmark) = previous.get(&cursor)?.clone();
                    path.push((prior.clone(), cursor, edge_landmark));
                    cursor = prior;
                }
                path.reverse();
                return Some(path);
            }
            queue.push_back(next.clone());
        }
    }
    None
}

fn manifest_number(value: f32) -> String {
    if value.fract() == 0.0 {
        return format!("{}", value as i32);
    }
    let mut text = format!("{value:.6}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn transition_prop_lines(
    entities: &BTreeMap<String, Vec<Entity>>,
    maps: &[String],
) -> Result<Vec<String>> {
    let mut incoming = BTreeMap::<String, Vec<(String, String)>>::new();
    let mut outgoing = BTreeMap::<String, Vec<(String, String)>>::new();
    for (source, list) in entities {
        for entity in list {
            if entity.get("classname").map(String::as_str) != Some("trigger_changelevel") {
                continue;
            }
            let destination = entity.get("map").cloned().unwrap_or_default();
            let landmark = entity.get("landmark").cloned().unwrap_or_default();
            if entities.contains_key(&destination) && !landmark.is_empty() {
                incoming
                    .entry(destination.clone())
                    .or_default()
                    .push((source.clone(), landmark.clone()));
                outgoing
                    .entry(source.clone())
                    .or_default()
                    .push((destination, landmark));
            }
        }
    }
    for edges in outgoing.values_mut() {
        edges.sort();
    }

    type Identity = (u8, String);
    let mut available = BTreeMap::<String, BTreeMap<String, BTreeSet<Identity>>>::new();
    for (map, list) in entities {
        let map_available = available.entry(map.clone()).or_default();
        for entity in list {
            let Some(ty) = cooked_actor_type(entity, list) else {
                continue;
            };
            let target = entity.get("targetname").cloned().unwrap_or_default();
            if !target.is_empty() && ty != 16 && ty != 50 {
                map_available
                    .entry(target)
                    .or_default()
                    .insert((ty, map.clone()));
            }
        }
    }
    loop {
        let snapshot = available.clone();
        let mut changed = false;
        for destination in entities.keys() {
            for (source, _) in incoming.get(destination).into_iter().flatten() {
                for (target, values) in snapshot.get(source).into_iter().flat_map(|v| v.iter()) {
                    let dest = available
                        .entry(destination.clone())
                        .or_default()
                        .entry(target.clone())
                        .or_default();
                    let before = dest.len();
                    dest.extend(values.iter().cloned());
                    changed |= dest.len() != before;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut result = Vec::new();
    for destination in maps {
        let Some(list) = entities.get(destination) else {
            continue;
        };
        let scripted: BTreeSet<String> = list
            .iter()
            .filter(|e| {
                matches!(
                    e.get("classname").map(String::as_str),
                    Some("scripted_sequence" | "aiscripted_sequence")
                )
            })
            .filter_map(|e| e.get("m_iszEntity").filter(|s| !s.is_empty()).cloned())
            .collect();
        let existing: BTreeSet<String> = list
            .iter()
            .filter(|e| cooked_actor_type(e, list).is_some())
            .filter_map(|e| e.get("targetname").filter(|s| !s.is_empty()).cloned())
            .collect();
        for target in scripted.difference(&existing) {
            let Some(hints) = available.get(destination).and_then(|m| m.get(target)) else {
                continue;
            };
            let types: BTreeSet<u8> = hints.iter().map(|v| v.0).collect();
            if types.len() != 1 {
                return Err(format!(
                    "{destination}: scripted incoming identity {target:?} has conflicting types {types:?}"
                )
                .into());
            }
            let ty = *types.iter().next().unwrap();
            let Some((source, path)) = hints
                .iter()
                .filter(|hint| hint.0 == ty)
                .filter_map(|hint| {
                    transition_path(&outgoing, &hint.1, destination)
                        .map(|path| (hint.1.clone(), path))
                })
                .min_by(|a, b| (a.1.len(), &a.0).cmp(&(b.1.len(), &b.0)))
            else {
                return Err(format!(
                    "{destination}: incoming identity {target:?} has no transition path"
                )
                .into());
            };
            let source_entities = entities
                .get(&source)
                .ok_or_else(|| format!("{source}: source entities are missing"))?;
            let actor = source_entities
                .iter()
                .find(|entity| {
                    entity.get("targetname").map(String::as_str) == Some(target)
                        && cooked_actor_type(entity, source_entities) == Some(ty)
                })
                .ok_or_else(|| format!("{source}: incoming actor {target:?} is missing"))?;
            let mut origin = actor
                .get("origin")
                .and_then(|value| parse_vec3(value))
                .ok_or_else(|| format!("{source}: incoming actor {target:?} has no origin"))?;
            for (from, to, landmark) in path {
                let from_origin =
                    landmark_origin(&entities[&from], &landmark).ok_or_else(|| {
                        format!("{from}: transition landmark {landmark:?} is missing")
                    })?;
                let to_origin = landmark_origin(&entities[&to], &landmark)
                    .ok_or_else(|| format!("{to}: transition landmark {landmark:?} is missing"))?;
                for axis in 0..3 {
                    origin[axis] += to_origin[axis] - from_origin[axis];
                }
            }
            let yaw = entity_yaw(actor);
            let spawnflags = actor
                .get("spawnflags")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            let body = actor
                .get("body")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0)
                .clamp(0, 7);
            result.push(format!(
                "{destination}|{ty}|{target}|{}|{}|{}|{}|{source}|{spawnflags}|{body}",
                manifest_number(origin[0]),
                manifest_number(origin[1]),
                manifest_number(origin[2]),
                manifest_number(yaw),
            ));
        }
    }
    Ok(result)
}

pub fn transition_props(maps_dir: &Path, output: &Path, maps: &[String]) -> Result<()> {
    let mut entities = BTreeMap::<String, Vec<Entity>>::new();
    for map in maps {
        let path = maps_dir.join(format!("{map}.bsp"));
        if path.is_file() {
            entities.insert(map.clone(), bsp_entities(&path)?);
        }
    }
    let result = transition_prop_lines(&entities, maps)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        output,
        result.join("\n") + if result.is_empty() { "" } else { "\n" },
    )?;
    println!(
        "transition type hints -> {} ({} actors)",
        output.display(),
        result.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_parser_preserves_goldsrc_keys() {
        let entities =
            parse_entities_bytes(br#"{ "classname" "monster_barney" "targetname" "guard" }"#);
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0].get("classname").map(String::as_str),
            Some("monster_barney")
        );
        assert_eq!(
            entities[0].get("targetname").map(String::as_str),
            Some("guard")
        );
    }

    fn entity(fields: &[(&str, &str)]) -> Entity {
        fields
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn transition_fallback_keeps_landmark_transform_and_actor_defaults() {
        let mut maps = BTreeMap::new();
        maps.insert(
            "source".to_string(),
            vec![
                entity(&[
                    ("classname", "monster_barney"),
                    ("targetname", "guard"),
                    ("origin", "10 20 30"),
                    ("angle", "90"),
                    ("spawnflags", "256"),
                    ("body", "2"),
                ]),
                entity(&[
                    ("classname", "info_landmark"),
                    ("targetname", "gate"),
                    ("origin", "1 2 3"),
                ]),
                entity(&[
                    ("classname", "trigger_changelevel"),
                    ("map", "destination"),
                    ("landmark", "gate"),
                ]),
            ],
        );
        maps.insert(
            "destination".to_string(),
            vec![
                entity(&[
                    ("classname", "info_landmark"),
                    ("targetname", "gate"),
                    ("origin", "101 202 303"),
                ]),
                entity(&[("classname", "scripted_sequence"), ("m_iszEntity", "guard")]),
            ],
        );
        let order = vec!["source".to_string(), "destination".to_string()];
        assert_eq!(
            transition_prop_lines(&maps, &order).unwrap(),
            ["destination|1|guard|110|220|330|90|source|256|2"]
        );
    }

    #[test]
    fn transition_fallback_follows_multi_hop_landmarks() {
        let mut maps = BTreeMap::new();
        maps.insert(
            "a".to_string(),
            vec![
                entity(&[
                    ("classname", "monster_scientist"),
                    ("targetname", "walker"),
                    ("origin", "10 0 0"),
                ]),
                entity(&[
                    ("classname", "info_landmark"),
                    ("targetname", "ab"),
                    ("origin", "5 0 0"),
                ]),
                entity(&[
                    ("classname", "trigger_changelevel"),
                    ("map", "b"),
                    ("landmark", "ab"),
                ]),
            ],
        );
        maps.insert(
            "b".to_string(),
            vec![
                entity(&[
                    ("classname", "info_landmark"),
                    ("targetname", "ab"),
                    ("origin", "105 0 0"),
                ]),
                entity(&[
                    ("classname", "info_landmark"),
                    ("targetname", "bc"),
                    ("origin", "100 10 0"),
                ]),
                entity(&[
                    ("classname", "trigger_changelevel"),
                    ("map", "c"),
                    ("landmark", "bc"),
                ]),
            ],
        );
        maps.insert(
            "c".to_string(),
            vec![
                entity(&[
                    ("classname", "info_landmark"),
                    ("targetname", "bc"),
                    ("origin", "200 20 0"),
                ]),
                entity(&[
                    ("classname", "scripted_sequence"),
                    ("m_iszEntity", "walker"),
                ]),
            ],
        );
        let order = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(
            transition_prop_lines(&maps, &order).unwrap(),
            ["c|0|walker|210|10|0|0|a|0|0"]
        );
    }
}

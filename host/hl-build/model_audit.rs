use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::Result;

// These are the shipping runtime capacities. Keep the synchronization tests at
// the bottom of this file: a pool change must update the auditor in the same
// commit, otherwise the Rust build fails before it can emit a disc.
// Kept in lock-step with game/build.rs. The pool retains audit-enforced
// worst-map slack while funding actor interpolation and world-cache replay.
const MODEL_POOL_WORDS: usize = 55_936;
/// Per-map model budget. The runtime keeps ONE arena (map staging capacity plus
/// MODEL_POOL_WORDS) and starts the model pool where the *loaded* map ends, so a
/// map's real budget is the arena minus its own cooked size. That is never below
/// MODEL_POOL_WORDS and is far above it wherever the BSP is smaller than the
/// fleet's largest. The staging capacity is taken as the largest cooked chunk,
/// which understates game/build.rs's MAP_WORDS (it adds a decode margin), so
/// every budget here is conservative.
fn map_pool_budgets(repository: &Path, maps: usize) -> Result<Vec<usize>> {
    let rooms = repository.join("data/rooms");
    let staging_words = fs::read_dir(&rooms)?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with("room_") && matches!(path.extension()?.to_str()?, "psxc" | "psxw"))
                .then(|| fs::metadata(path).ok().map(|m| m.len() as usize))
                .flatten()
        })
        .max()
        .ok_or("model residency audit found no cooked room chunks")?
        .div_ceil(4);
    (0..maps)
        .map(|index| {
            let world =
                fs::metadata(rooms.join(format!("room_{}.psxc", index * 2)))?.len() as usize;
            Ok(staging_words + MODEL_POOL_WORDS - world.div_ceil(4).min(staging_words))
        })
        .collect()
}
const VM_POOL_WORDS: usize = 20_224;
const MAX_LOADED_MODELS: usize = 23;
const POOL_FACE_CAP: usize = 8_640;
const POOL_FACE_RUN_CAP: usize = 192;
const POOL_TEX_SLOTS: usize = 176;
const N_MODEL_TYPES: usize = 76;
const MODEL_GEOM_CHUNK_BASE: usize = 1300;
const MODEL_TEX_CHUNK_BASE: usize = 1100;
const MODEL_CARRY_CHUNK_BASE: usize = 1500;
const VIEWMODEL_CHUNK_BASE: usize = 1000;
const VIEWMODEL_COUNT: usize = 16;
const VIEWMODEL_POOL_WORDS: usize = 20_224;
const VIEWMODEL_SORT_TRIS: usize = 1_152;
const VIEWMODEL_SORT_BUCKETS: usize = 64;
const VIEWMODEL_CACHE_GUARD_WORDS: usize = 2_030;
const C4A3_GARG_MODEL_CHUNK: usize = 1816;
const C4A3_GARG_TEXTURE_CHUNK: usize = 1916;
const C4A1B_GARG_MODEL_CHUNK: usize = 1817;
const C4A1B_GARG_TEXTURE_CHUNK: usize = 1917;
const C4A3_ICKY_MODEL_CHUNK: usize = 1819;
const C4A3_ICKY_TEXTURE_CHUNK: usize = 1919;
const C1A2B_ZOMBIE_MODEL_CHUNK: usize = 1818;
const C1A2B_ZOMBIE_TEXTURE_CHUNK: usize = 1918;
const PRESSURE_ISLAVE_MODEL_CHUNK: usize = 1820;
const PRESSURE_ISLAVE_TEXTURE_CHUNK: usize = 1920;
const PROP_TYPE_MASK: u16 = 0x0fff;
const CARRY_CAPACITY: usize = 15;
// Keep synchronized with the source MDL cooker. Sixteen remains the default;
// explicitly budgeted small models may retain more source poses.
const MAX_BAKED_SEQUENCE_FRAMES: usize = 64;

const HMD_FLAG_BODY_MASKS: u16 = 1 << 0;
const HMD_FLAG_MOUTH: u16 = 1 << 1;
const HMD_FLAG_I8_NORMALS: u16 = 1 << 3;
const HMD_FLAG_HITBOXES: u16 = 1 << 4;
const HMD_FLAG_FRAME_TIMES: u16 = 1 << 5;
const HMD_FLAG_ALIGNED_MODEL_DATA: u16 = 1 << 6;
const HMD_FLAG_VERTEX_SOA: u16 = 1 << 7;
const MOUTH_XFORM_BYTES: usize = 24;
const HMD7_HEADER_BYTES: usize = 36;
const HMD7_RANGE_BYTES: usize = 8;
const HMD8_AFFINE_BYTES: usize = 20;
const HMD7_HITBOX_BYTES: usize = 14;

const fn valid_hmd8_scale_q12(raw: u16) -> bool {
    matches!(raw, 0 | 256 | 512 | 1024 | 2048 | 4096)
}

const LUMP_PLANES: usize = 1;
const LUMP_VISIBILITY: usize = 4;
const LUMP_NODES: usize = 5;
const LUMP_LEAVES: usize = 10;
const LUMP_MODELS: usize = 14;

#[derive(Clone, Copy, Debug)]
struct Actor {
    ty: u8,
    body: u8,
}

#[derive(Debug)]
struct ModelChunk {
    merged: bool,
    geometry_bytes: usize,
    texture_bytes: usize,
    kept_bytes: usize,
    n_verts: usize,
    n_frames: usize,
    n_clips: usize,
    clip_frames: Vec<usize>,
    clip_hold_ticks: Vec<u16>,
    tris: Vec<(u16, u8)>,
    n_textures: usize,
    body_ranges: Vec<(usize, u8)>,
    n_bones: usize,
    has_mouth: bool,
    has_frame_times: bool,
}

impl ModelChunk {
    fn stream_priority_bytes(&self) -> usize {
        if self.merged {
            8 + self.geometry_bytes + self.texture_bytes
        } else {
            self.geometry_bytes
        }
    }

    fn visible_stats(&self, visible_bodies: u8) -> (usize, usize, usize, usize) {
        let mut faces = 0usize;
        let mut runs = 0usize;
        let mut last = None;
        let mut texture_mask = 0u32;
        for &(texture, body_mask) in &self.tris {
            if body_mask & visible_bodies == 0 {
                continue;
            }
            faces += 1;
            if last != Some((texture, body_mask)) {
                runs += 1;
                last = Some((texture, body_mask));
            }
            if texture < 32 {
                texture_mask |= 1 << texture;
            }
        }
        let textures = if self.body_ranges.is_empty() {
            self.n_textures
        } else {
            texture_mask.count_ones() as usize
        };
        let kept = if self.body_ranges.is_empty() {
            self.kept_bytes
        } else {
            let visible_ranges = self
                .body_ranges
                .iter()
                .filter(|&&(_, mask)| mask & visible_bodies != 0)
                .count();
            let visible_verts = self
                .body_ranges
                .iter()
                .filter(|&&(_, mask)| mask & visible_bodies != 0)
                .map(|&(count, _)| count)
                .sum::<usize>();
            let mouth_bytes = if self.has_mouth {
                self.n_frames * MOUTH_XFORM_BYTES
            } else {
                0
            };
            HMD7_HEADER_BYTES
                + self.n_clips * 4
                + if self.has_frame_times {
                    self.n_frames
                } else {
                    0
                }
                + visible_ranges * HMD7_RANGE_BYTES
                + visible_verts * 6
                + self.n_frames * self.n_bones * HMD8_AFFINE_BYTES
                + mouth_bytes
        };
        (faces, runs, textures, kept)
    }

    fn transient_end_words(&self, geom_word: usize, kept_bytes: usize) -> usize {
        if self.merged {
            geom_word + (8 + self.geometry_bytes + self.texture_bytes).div_ceil(4)
        } else {
            let geometry_end = geom_word + self.geometry_bytes.div_ceil(4);
            let texture_end = geom_word + kept_bytes.div_ceil(4) + self.texture_bytes.div_ceil(4);
            geometry_end.max(texture_end)
        }
    }
}

#[derive(Clone, Debug)]
struct AuditRow {
    label: String,
    pool_words: usize,
    resident_words: usize,
    peak_words: usize,
    faces: usize,
    runs: usize,
    textures: usize,
    slots: usize,
}

#[derive(Debug)]
pub struct AuditSummary {
    pub maps: usize,
    pub transitions: usize,
    pub peak_label: String,
    pub peak_words: usize,
    pub slack_bytes: usize,
    pub report: PathBuf,
}

fn checked_range<'a>(data: &'a [u8], offset: usize, len: usize, what: &str) -> Result<&'a [u8]> {
    data.get(offset..offset.saturating_add(len))
        .ok_or_else(|| format!("{what}: truncated range at {offset}+{len}").into())
}

fn u16le(data: &[u8], offset: usize, what: &str) -> Result<u16> {
    Ok(u16::from_le_bytes(
        checked_range(data, offset, 2, what)?.try_into()?,
    ))
}

fn u32le(data: &[u8], offset: usize, what: &str) -> Result<u32> {
    Ok(u32::from_le_bytes(
        checked_range(data, offset, 4, what)?.try_into()?,
    ))
}

fn i32le(data: &[u8], offset: usize, what: &str) -> Result<i32> {
    Ok(u32le(data, offset, what)? as i32)
}

fn f32le(data: &[u8], offset: usize, what: &str) -> Result<f32> {
    Ok(f32::from_bits(u32le(data, offset, what)?))
}

fn cstr(data: &[u8]) -> String {
    let end = data
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

fn parse_geometry(geometry: &[u8], texture: &[u8], merged: bool, what: &str) -> Result<ModelChunk> {
    let magic = checked_range(geometry, 0, 4, what)?;
    if magic != b"HMD8" {
        return Err(format!("{what}: unsupported model magic {magic:?}").into());
    }
    let n_verts = u32le(geometry, 4, what)? as usize;
    let n_tris = u32le(geometry, 8, what)? as usize;
    let packed_texture_hitbox_counts = u32le(geometry, 12, what)?;
    let n_frames = (u32le(geometry, 16, what)? as usize).max(1);
    let n_clips = (u32le(geometry, 20, what)? as usize).max(1);
    let model_data_len = u32le(geometry, 24, what)? as usize;
    let local_to_world_q12 = u16le(geometry, 28, what)?;
    if !valid_hmd8_scale_q12(local_to_world_q12) {
        return Err(format!(
            "{what}: HMD8 local-to-world Q12 scale {local_to_world_q12} is not an exact power-of-two"
        )
        .into());
    }
    let flags = u16le(geometry, 30, what)?;
    let n_bones = u16le(geometry, 32, what)? as usize;
    let n_ranges = u16le(geometry, 34, what)? as usize;
    let clips_off = HMD7_HEADER_BYTES;
    let has_frame_times = flags & HMD_FLAG_FRAME_TIMES != 0;
    let frame_times_off = clips_off + n_clips * 4;
    let unaligned_ranges_off = frame_times_off + if has_frame_times { n_frames } else { 0 };
    let vertex_soa = flags & HMD_FLAG_VERTEX_SOA != 0;
    let ranges_off = if vertex_soa {
        (unaligned_ranges_off + 3) & !3
    } else if flags & HMD_FLAG_ALIGNED_MODEL_DATA != 0 {
        (unaligned_ranges_off + 1) & !1
    } else {
        unaligned_ranges_off
    };
    let vertices_off = ranges_off + n_ranges * HMD7_RANGE_BYTES;
    let poses_off = vertices_off + n_verts * 6;
    let mouth_off = poses_off + n_frames * n_bones * HMD8_AFFINE_BYTES;
    let tri_off = ranges_off + model_data_len;
    let tri_size = if flags & HMD_FLAG_I8_NORMALS != 0 {
        20
    } else {
        16
    };
    checked_range(geometry, tri_off, n_tris.saturating_mul(tri_size), what)?;

    let has_body_masks = flags & HMD_FLAG_BODY_MASKS != 0;
    let has_mouth = flags & HMD_FLAG_MOUTH != 0;
    let mouth_len = if has_mouth {
        n_frames * MOUTH_XFORM_BYTES
    } else {
        0
    };
    let n_hitboxes = if flags & HMD_FLAG_HITBOXES != 0 {
        (packed_texture_hitbox_counts >> 16) as usize
    } else {
        0
    };
    let hitbox_len = n_hitboxes.saturating_mul(HMD7_HITBOX_BYTES);
    if mouth_off
        .saturating_add(mouth_len)
        .saturating_add(hitbox_len)
        != tri_off
    {
        return Err(
            format!("{what}: HMD8 model-data layout does not reach triangle stream").into(),
        );
    }
    checked_range(geometry, ranges_off, n_ranges * HMD7_RANGE_BYTES, what)?;
    if has_frame_times {
        checked_range(geometry, frame_times_off, n_frames, what)?;
    }
    checked_range(geometry, vertices_off, n_verts * 6, what)?;
    if vertex_soa && vertices_off & 3 != 0 {
        return Err(format!("{what}: HMD8 SoA vertex stream is not word aligned").into());
    }
    checked_range(
        geometry,
        poses_off,
        n_frames * n_bones * HMD8_AFFINE_BYTES,
        what,
    )?;
    let body_ranges = if has_body_masks {
        (0..n_ranges)
            .map(|range| {
                let offset = ranges_off + range * HMD7_RANGE_BYTES;
                Ok((
                    u16le(geometry, offset + 2, what)? as usize,
                    geometry[offset + 6],
                ))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let clip_frames = (0..n_clips)
        .map(|clip| {
            u16le(geometry, clips_off + clip * 4 + 2, what)
                .map(|packed| ((packed & 0x00ff) as usize).max(1))
        })
        .collect::<Result<Vec<_>>>()?;
    let clip_hold_ticks = (0..n_clips)
        .map(|clip| {
            let packed_first = u16le(geometry, clips_off + clip * 4, what)?;
            let packed_count = u16le(geometry, clips_off + clip * 4 + 2, what)?;
            let quanta = (packed_count >> 8)
                | if packed_first & 0x8000 != 0 {
                    0x0100
                } else {
                    0
                };
            Ok(if quanta == 0 {
                40
            } else {
                quanta.saturating_mul(2)
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut tris = Vec::with_capacity(n_tris);
    for index in 0..n_tris {
        let offset = tri_off + index * tri_size;
        let texture = u16le(geometry, offset + 6, what)?;
        let body_mask = if has_body_masks {
            geometry[offset
                + if flags & HMD_FLAG_I8_NORMALS != 0 {
                    17
                } else {
                    14
                }]
        } else {
            0xff
        };
        tris.push((texture, body_mask));
    }
    if texture.get(0..4) != Some(b"HLTX") {
        return Err(format!("{what}: missing HLTX texture payload").into());
    }
    let n_textures = u32le(texture, 4, what)? as usize;
    Ok(ModelChunk {
        merged,
        geometry_bytes: geometry.len(),
        texture_bytes: texture.len(),
        kept_bytes: tri_off,
        n_verts,
        n_frames,
        n_clips,
        clip_frames,
        clip_hold_ticks,
        tris,
        n_textures,
        body_ranges,
        n_bones,
        has_mouth,
        has_frame_times,
    })
}

fn load_chunks(
    model_pack: &Path,
    geometry_base: usize,
    split_texture_base: Option<usize>,
) -> Result<Vec<Option<ModelChunk>>> {
    let mut chunks = Vec::with_capacity(N_MODEL_TYPES);
    for ty in 0..N_MODEL_TYPES {
        let path = model_pack.join(format!("chunk_{}.psxm", geometry_base + ty));
        if !path.is_file() {
            chunks.push(None);
            continue;
        }
        let data = fs::read(&path)?;
        let what = path.display().to_string();
        if data.get(0..4) == Some(b"HMRG") {
            let geometry_len = u32le(&data, 4, &what)? as usize;
            let geometry = checked_range(&data, 8, geometry_len, &what)?;
            let texture = checked_range(
                &data,
                8 + geometry_len,
                data.len() - 8 - geometry_len,
                &what,
            )?;
            chunks.push(Some(parse_geometry(geometry, texture, true, &what)?));
        } else {
            let texture_base = split_texture_base
                .ok_or_else(|| format!("{what}: carry variant must be a merged HMRG stream"))?;
            let texture_path = model_pack.join(format!("chunk_{}.psxm", texture_base + ty));
            let texture = fs::read(&texture_path).map_err(|error| {
                format!("{}: split texture missing: {error}", texture_path.display())
            })?;
            chunks.push(Some(parse_geometry(&data, &texture, false, &what)?));
        }
    }
    Ok(chunks)
}

fn load_split_chunk(
    model_pack: &Path,
    geometry_id: usize,
    texture_id: usize,
) -> Result<ModelChunk> {
    let geometry_path = model_pack.join(format!("chunk_{geometry_id}.psxm"));
    let texture_path = model_pack.join(format!("chunk_{texture_id}.psxm"));
    let geometry = fs::read(&geometry_path)?;
    let texture = fs::read(&texture_path)?;
    parse_geometry(
        &geometry,
        &texture,
        false,
        &geometry_path.display().to_string(),
    )
}

fn load_map_variants(
    model_pack: &Path,
    map_count: usize,
) -> Result<Vec<HashMap<usize, ModelChunk>>> {
    let mut maps = (0..map_count).map(|_| HashMap::new()).collect::<Vec<_>>();
    let path = model_pack.join("map-model-variants.txt");
    if !path.is_file() {
        return Ok(maps);
    }
    for (line_no, line) in fs::read_to_string(&path)?.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('|').collect::<Vec<_>>();
        if fields.len() < 4 {
            return Err(format!(
                "{}:{}: malformed variant record",
                path.display(),
                line_no + 1
            )
            .into());
        }
        let map = fields[0].parse::<usize>()?;
        let ty = fields[2].parse::<usize>()?;
        let chunk_id = fields[3].parse::<usize>()?;
        if map >= maps.len() || ty >= N_MODEL_TYPES {
            return Err(format!(
                "{}:{}: variant map/type out of range: {map}/{ty}",
                path.display(),
                line_no + 1
            )
            .into());
        }
        let geometry_path = model_pack.join(format!("chunk_{chunk_id}.psxm"));
        let geometry = fs::read(&geometry_path)?;
        let what = geometry_path.display().to_string();
        let chunk = if geometry.get(0..4) == Some(b"HMRG") {
            let geometry_len = u32le(&geometry, 4, &what)? as usize;
            let hmd = checked_range(&geometry, 8, geometry_len, &what)?;
            let texture = checked_range(
                &geometry,
                8 + geometry_len,
                geometry.len().saturating_sub(8 + geometry_len),
                &what,
            )?;
            parse_geometry(hmd, texture, true, &what)?
        } else {
            let texture_path = model_pack.join(format!("chunk_{}.psxm", MODEL_TEX_CHUNK_BASE + ty));
            let texture = fs::read(&texture_path)?;
            parse_geometry(&geometry, &texture, false, &what)?
        };
        if maps[map].insert(ty, chunk).is_some() {
            return Err(format!(
                "{}:{}: duplicate map/type variant",
                path.display(),
                line_no + 1
            )
            .into());
        }
    }
    Ok(maps)
}

fn audit_map_clip_manifests(
    repository: &Path,
    maps: &[&str],
    chunks: &[Option<ModelChunk>],
    variants: &[HashMap<usize, ModelChunk>],
) -> Result<PathBuf> {
    let clip_dir = repository.join("data/modelpack/map-clips");
    let report = repository.join(".hlpsx/reports/model-clip-ram.csv");
    if let Some(parent) = report.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = String::from(
        "map_index,map,type,baseline_clips,variant_clips,baseline_frames,variant_frames,baseline_frame_bytes,variant_frame_bytes,delta_bytes,manifest_names\n",
    );
    let mut manifest_records = 0usize;
    for (map_index, &map) in maps.iter().enumerate() {
        let path = clip_dir.join(format!("clips_{map_index}.txt"));
        let text = fs::read_to_string(&path).map_err(|error| {
            format!("{}: per-map clip manifest missing: {error}", path.display())
        })?;
        let mut names: HashMap<usize, Vec<String>> = HashMap::new();
        for (line_no, line) in text.lines().enumerate() {
            let fields = line.split('|').collect::<Vec<_>>();
            if fields.len() < 3 {
                continue;
            }
            let ty = fields[0].parse::<usize>()?;
            let slot = fields[2].parse::<usize>()?;
            let variant = variants
                .get(map_index)
                .and_then(|map| map.get(&ty))
                .ok_or_else(|| {
                    format!(
                        "{}:{}: clip T{ty} has no map HMD8 variant",
                        path.display(),
                        line_no + 1
                    )
                })?;
            if slot >= variant.n_clips {
                return Err(format!(
                    "{}:{}: clip T{ty} slot {slot} exceeds {} cooked clips",
                    path.display(),
                    line_no + 1,
                    variant.n_clips
                )
                .into());
            }
            names.entry(ty).or_default().push(fields[1].to_string());
            manifest_records += 1;
        }
        for (&ty, variant) in variants
            .get(map_index)
            .into_iter()
            .flat_map(|map| map.iter())
        {
            let baseline = chunks
                .get(ty)
                .and_then(Option::as_ref)
                .ok_or_else(|| format!("{map}: variant T{ty} lacks a baseline model"))?;
            let baseline_frames = baseline.n_frames
                * (baseline.n_bones * HMD8_AFFINE_BYTES
                    + usize::from(baseline.has_mouth) * MOUTH_XFORM_BYTES
                    + usize::from(baseline.has_frame_times));
            let variant_frames = variant.n_frames
                * (variant.n_bones * HMD8_AFFINE_BYTES
                    + usize::from(variant.has_mouth) * MOUTH_XFORM_BYTES
                    + usize::from(variant.has_frame_times));
            let mut manifest_names = names.remove(&ty).unwrap_or_default();
            manifest_names.sort();
            manifest_names.dedup();
            output.push_str(&format!(
                "{map_index},{map},{ty},{},{},{},{},{baseline_frames},{variant_frames},{},{}\n",
                baseline.n_clips,
                variant.n_clips,
                baseline.n_frames,
                variant.n_frames,
                variant_frames as isize - baseline_frames as isize,
                manifest_names.join("+")
            ));
        }
    }
    fs::write(&report, output)?;
    println!(
        "map clip audit: {manifest_records} named mappings, every slot is inside its selected HMD8"
    );
    println!("  report: {}", report.display());
    Ok(report)
}

fn parse_map_actors(path: &Path) -> Result<Vec<Actor>> {
    let data = fs::read(path)?;
    let what = path.display().to_string();
    if data.len() < 52
        || !matches!(
            data.get(0..4),
            Some(b"HLMA" | b"HLMB" | b"HLMC" | b"HLMD" | b"HLME" | b"HLMF" | b"HLMG" | b"HLMH")
        )
    {
        return Err(format!("{what}: not a cooked HLM map").into());
    }
    let props_off = u32le(&data, 36, &what)? as usize;
    let counts = u32le(&data, props_off, &what)?;
    let count = if counts & 0x8000_0000 != 0 {
        (counts & 0xffff) as usize
    } else {
        counts as usize
    };
    checked_range(&data, props_off + 4, count.saturating_mul(24), &what)?;
    let mut actors = Vec::with_capacity(count);
    for index in 0..count {
        let offset = props_off + 4 + index * 24;
        let ty = (u16le(&data, offset, &what)? & PROP_TYPE_MASK) as u8;
        let yaw = u32le(&data, offset + 16, &what)?;
        if ty < N_MODEL_TYPES as u8 {
            actors.push(Actor {
                ty,
                body: ((yaw >> 12) & 7) as u8,
            });
        }
    }
    Ok(actors)
}

fn ai_pass(ty: u8) -> usize {
    if (3..=4).contains(&ty) || (26..=49).contains(&ty) || ty == 75 {
        1
    } else if matches!(ty, 12..=18 | 23..=25 | 50 | 52 | 53) {
        2
    } else {
        0
    }
}

fn visible_bodies(
    ty: u8,
    actors: &[Actor],
    carry_count: usize,
    carry_only: bool,
    has_body_masks: bool,
) -> u8 {
    if !has_body_masks {
        return 0xff;
    }
    if ty == 75 {
        return 0x3f;
    }
    if ty == 1 && !carry_only {
        return 0x07;
    }
    if !matches!(ty, 0 | 1 | 25 | 54) {
        return 1;
    }
    let body_actors = if carry_only {
        &actors[..carry_count.min(actors.len())]
    } else {
        actors
    };
    let mask = body_actors
        .iter()
        .filter(|actor| actor.ty == ty)
        .fold(0u8, |mask, actor| mask | (1 << actor.body.min(7)));
    mask.max(1)
}

fn model_name(ty: u8) -> &'static str {
    match ty {
        0 => "scientist",
        1 => "barney",
        2 => "headcrab",
        3 => "suit",
        4 => "battery",
        5 => "zombie",
        6 => "houndeye",
        7 => "bullsquid",
        8 => "hgrunt",
        9 => "islave",
        10 => "agrunt",
        11 => "controller",
        12 => "barnacle",
        13 => "leech",
        14 => "roach",
        15 => "gman",
        16 => "gargantua",
        17 => "nihilanth",
        18 => "bigmomma",
        19 => "ichthyosaur",
        20 => "sentry",
        21 => "turret",
        22 => "miniturret",
        23 => "apache",
        24 => "boid",
        25 => "sitting_scientist",
        50 => "tentacle",
        51 => "assassin",
        52 => "loader",
        53 => "forklift",
        54 => "scripted_sitter",
        55 => "vent_zombie",
        _ => "pickup",
    }
}

fn simulate(
    label: &str,
    pool_words: usize,
    chunks: &[Option<ModelChunk>],
    carry_chunks: &[Option<ModelChunk>],
    garg_variant: Option<&ModelChunk>,
    icky_variant: Option<&ModelChunk>,
    zombie_variant: Option<&ModelChunk>,
    islave_variant: Option<&ModelChunk>,
    map_variants: Option<&HashMap<usize, ModelChunk>>,
    actors: &[Actor],
    carry_count: usize,
) -> Result<AuditRow> {
    let verbose = std::env::var("HLPSX_AUDIT_VERBOSE")
        .ok()
        .is_some_and(|needle| label.contains(&needle));
    let mut geom_word = VM_POOL_WORDS;
    let mut peak_words = geom_word;
    let mut faces = 0usize;
    let mut runs = 0usize;
    let mut textures = 0usize;
    let mut slots = 0usize;
    let mut seen = [false; N_MODEL_TYPES];
    let mut drops = Vec::new();

    let mut stream_order = (0..N_MODEL_TYPES).collect::<Vec<_>>();
    stream_order.sort_by_key(|&ty| {
        (
            std::cmp::Reverse(
                chunks
                    .get(ty)
                    .and_then(Option::as_ref)
                    .map(ModelChunk::stream_priority_bytes)
                    .unwrap_or(0),
            ),
            ty,
        )
    });
    let carry_count = carry_count.min(actors.len());
    for pass in 0..3 {
        for source in 0..2 {
            let source_actors = if source == 0 {
                &actors[..carry_count]
            } else {
                &actors[carry_count..]
            };
            for &ordered_type in &stream_order {
                let Some(actor) = source_actors
                    .iter()
                    .find(|actor| actor.ty as usize == ordered_type)
                else {
                    continue;
                };
                let ty = actor.ty as usize;
                if ty >= N_MODEL_TYPES || seen[ty] || ai_pass(actor.ty) != pass {
                    continue;
                }
                seen[ty] = true;
                let carry_only = source == 0
                    && !actors[carry_count..]
                        .iter()
                        .any(|destination| destination.ty as usize == ty);
                let chunk =
                    if let Some(variant) = map_variants.and_then(|variants| variants.get(&ty)) {
                        Some(variant)
                    } else if !carry_only && ty == 5 && zombie_variant.is_some() {
                        zombie_variant
                    } else if !carry_only && ty == 9 && islave_variant.is_some() {
                        islave_variant
                    } else if !carry_only && ty == 16 && garg_variant.is_some() {
                        garg_variant
                    } else if !carry_only && ty == 19 && icky_variant.is_some() {
                        icky_variant
                    } else if carry_only {
                        carry_chunks.get(ty).and_then(Option::as_ref)
                    } else {
                        chunks.get(ty).and_then(Option::as_ref)
                    };
                let Some(chunk) = chunk else {
                    drops.push(format!("{}:missing chunk", model_name(actor.ty)));
                    continue;
                };
                let bodies = visible_bodies(
                    actor.ty,
                    actors,
                    carry_count,
                    carry_only,
                    !chunk.body_ranges.is_empty(),
                );
                let (model_faces, model_runs, model_textures, kept_bytes) =
                    chunk.visible_stats(bodies);
                let transient_end = chunk.transient_end_words(geom_word, kept_bytes);
                peak_words = peak_words.max(transient_end);
                if verbose {
                    println!(
                    "  {label}: T{:02} {:<20} start={} kept={} transient_end={} faces={} runs={} tex={} bodies=0x{bodies:02x}",
                    actor.ty,
                    model_name(actor.ty),
                    geom_word,
                    kept_bytes.div_ceil(4),
                    transient_end,
                    model_faces,
                    model_runs,
                    model_textures
                );
                }
                let reason = if slots >= MAX_LOADED_MODELS {
                    Some(format!("model slots {}/{}", slots + 1, MAX_LOADED_MODELS))
                } else if transient_end > pool_words {
                    Some(format!(
                        "transient model RAM {transient_end}/{pool_words} words"
                    ))
                } else if faces + model_faces > POOL_FACE_CAP {
                    Some(format!(
                        "face pool {}/{}",
                        faces + model_faces,
                        POOL_FACE_CAP
                    ))
                } else if runs + model_runs > POOL_FACE_RUN_CAP {
                    Some(format!(
                        "face-run pool {}/{}",
                        runs + model_runs,
                        POOL_FACE_RUN_CAP
                    ))
                } else if textures + model_textures > POOL_TEX_SLOTS {
                    Some(format!(
                        "texture slots {}/{}",
                        textures + model_textures,
                        POOL_TEX_SLOTS
                    ))
                } else {
                    None
                };
                if let Some(reason) = reason {
                    drops.push(format!("{}:{reason}", model_name(actor.ty)));
                    continue;
                }
                // Merged chunks retain their two-word envelope before the HMD
                // frame section. Split streams begin directly at geom_word.
                geom_word += usize::from(chunk.merged) * 2 + kept_bytes.div_ceil(4);
                faces += model_faces;
                runs += model_runs;
                textures += model_textures;
                slots += 1;
            }
        }
    }
    if !drops.is_empty() {
        return Err(format!("{label}: invisible model drop(s): {}", drops.join(", ")).into());
    }
    Ok(AuditRow {
        label: label.to_string(),
        pool_words,
        resident_words: geom_word,
        peak_words,
        faces,
        runs,
        textures,
        slots,
    })
}

type Entity = HashMap<String, String>;

struct Bsp {
    data: Vec<u8>,
    lumps: [(usize, usize); 15],
    vis_leaf_count: usize,
}

impl Bsp {
    fn load(path: &Path) -> Result<Self> {
        let data = fs::read(path)?;
        let what = path.display().to_string();
        if data.len() < 124 || i32le(&data, 0, &what)? != 30 {
            return Err(format!("{what}: not a GoldSrc BSP30 map").into());
        }
        let mut lumps = [(0usize, 0usize); 15];
        for (index, slot) in lumps.iter_mut().enumerate() {
            let offset = i32le(&data, 4 + index * 8, &what)?;
            let len = i32le(&data, 8 + index * 8, &what)?;
            if offset < 0 || len < 0 || offset as usize + len as usize > data.len() {
                return Err(format!("{what}: invalid lump {index}").into());
            }
            *slot = (offset as usize, len as usize);
        }
        let (models_off, models_len) = lumps[LUMP_MODELS];
        let vis_leaf_count = if models_len >= 56 {
            i32le(&data, models_off + 52, &what)?.max(0) as usize
        } else {
            0
        };
        Ok(Self {
            data,
            lumps,
            vis_leaf_count,
        })
    }

    fn lump(&self, index: usize) -> &[u8] {
        let (offset, len) = self.lumps[index];
        &self.data[offset..offset + len]
    }

    fn entities(&self) -> Vec<Entity> {
        let (offset, len) = self.lumps[0];
        parse_entities(&self.data[offset..offset + len])
    }

    fn point_leaf(&self, point: [f32; 3]) -> i32 {
        let nodes = self.lump(LUMP_NODES);
        let planes = self.lump(LUMP_PLANES);
        let mut node = 0i32;
        for _ in 0..512 {
            if node < 0 {
                return -node - 1;
            }
            let offset = node as usize * 24;
            if offset + 8 > nodes.len() {
                return 0;
            }
            let plane_index =
                i32::from_le_bytes(nodes[offset..offset + 4].try_into().unwrap()).max(0) as usize;
            let plane_offset = plane_index * 20;
            if plane_offset + 16 > planes.len() {
                return 0;
            }
            let normal = [
                f32::from_bits(u32::from_le_bytes(
                    planes[plane_offset..plane_offset + 4].try_into().unwrap(),
                )),
                f32::from_bits(u32::from_le_bytes(
                    planes[plane_offset + 4..plane_offset + 8]
                        .try_into()
                        .unwrap(),
                )),
                f32::from_bits(u32::from_le_bytes(
                    planes[plane_offset + 8..plane_offset + 12]
                        .try_into()
                        .unwrap(),
                )),
            ];
            let distance = f32::from_bits(u32::from_le_bytes(
                planes[plane_offset + 12..plane_offset + 16]
                    .try_into()
                    .unwrap(),
            ));
            let side =
                point[0] * normal[0] + point[1] * normal[1] + point[2] * normal[2] - distance;
            node = i16::from_le_bytes(
                nodes[offset + if side >= 0.0 { 4 } else { 6 }
                    ..offset + if side >= 0.0 { 6 } else { 8 }]
                    .try_into()
                    .unwrap(),
            ) as i32;
        }
        0
    }

    fn decompress_vis(&self, offset: i32) -> Vec<u8> {
        let row_bytes = self.vis_leaf_count.div_ceil(8);
        if offset < 0 {
            return vec![0xff; row_bytes];
        }
        let data = self.lump(LUMP_VISIBILITY);
        let mut cursor = offset as usize;
        let mut output = Vec::with_capacity(row_bytes);
        while output.len() < row_bytes && cursor < data.len() {
            let value = data[cursor];
            cursor += 1;
            if value != 0 {
                output.push(value);
            } else if cursor < data.len() {
                let count = data[cursor] as usize;
                cursor += 1;
                output.resize((output.len() + count).min(row_bytes), 0);
            }
        }
        output.resize(row_bytes, 0);
        output
    }

    fn box_visible(&self, viewpoint: [f32; 3], bounds: ([f32; 3], [f32; 3])) -> bool {
        let view_leaf = self.point_leaf(viewpoint);
        if view_leaf <= 0 {
            return true;
        }
        let leaves = self.lump(LUMP_LEAVES);
        let leaf_offset = view_leaf as usize * 28;
        if leaf_offset + 8 > leaves.len() {
            return true;
        }
        let vis_offset =
            i32::from_le_bytes(leaves[leaf_offset + 4..leaf_offset + 8].try_into().unwrap());
        let row = self.decompress_vis(vis_offset);
        self.box_visible_node(0, 0, bounds, &row)
    }

    fn box_visible_node(
        &self,
        node: i32,
        depth: usize,
        bounds: ([f32; 3], [f32; 3]),
        row: &[u8],
    ) -> bool {
        if node < 0 {
            let cluster = -node - 2;
            return cluster >= 0
                && (cluster as usize) < self.vis_leaf_count
                && row
                    .get(cluster as usize >> 3)
                    .is_some_and(|byte| byte & (1 << (cluster as usize & 7)) != 0);
        }
        if depth > 512 {
            return true;
        }
        let nodes = self.lump(LUMP_NODES);
        let planes = self.lump(LUMP_PLANES);
        let offset = node as usize * 24;
        if offset + 8 > nodes.len() {
            return true;
        }
        let plane_index = i32::from_le_bytes(nodes[offset..offset + 4].try_into().unwrap());
        if plane_index < 0 {
            return true;
        }
        let po = plane_index as usize * 20;
        if po + 16 > planes.len() {
            return true;
        }
        let normal = [
            f32::from_bits(u32::from_le_bytes(planes[po..po + 4].try_into().unwrap())),
            f32::from_bits(u32::from_le_bytes(
                planes[po + 4..po + 8].try_into().unwrap(),
            )),
            f32::from_bits(u32::from_le_bytes(
                planes[po + 8..po + 12].try_into().unwrap(),
            )),
        ];
        let distance = f32::from_bits(u32::from_le_bytes(
            planes[po + 12..po + 16].try_into().unwrap(),
        ));
        let mut near = [0.0; 3];
        let mut far = [0.0; 3];
        for axis in 0..3 {
            near[axis] = if normal[axis] >= 0.0 {
                bounds.0[axis]
            } else {
                bounds.1[axis]
            };
            far[axis] = if normal[axis] >= 0.0 {
                bounds.1[axis]
            } else {
                bounds.0[axis]
            };
        }
        let min_side = normal[0] * near[0] + normal[1] * near[1] + normal[2] * near[2] - distance;
        let max_side = normal[0] * far[0] + normal[1] * far[1] + normal[2] * far[2] - distance;
        let child0 = i16::from_le_bytes(nodes[offset + 4..offset + 6].try_into().unwrap()) as i32;
        let child1 = i16::from_le_bytes(nodes[offset + 6..offset + 8].try_into().unwrap()) as i32;
        if min_side >= 0.0 {
            self.box_visible_node(child0, depth + 1, bounds, row)
        } else if max_side < 0.0 {
            self.box_visible_node(child1, depth + 1, bounds, row)
        } else {
            self.box_visible_node(child0, depth + 1, bounds, row)
                || self.box_visible_node(child1, depth + 1, bounds, row)
        }
    }

    fn brush_bounds(&self, entity: &Entity) -> Option<([f32; 3], [f32; 3])> {
        let model = entity
            .get("model")?
            .strip_prefix('*')?
            .parse::<usize>()
            .ok()?;
        if model == 0 {
            return None;
        }
        let models = self.lump(LUMP_MODELS);
        let offset = model * 64;
        if offset + 24 > models.len() {
            return None;
        }
        let mut mins = [0.0; 3];
        let mut maxs = [0.0; 3];
        let origin =
            parse_vec3(entity.get("origin").map(String::as_str).unwrap_or("")).unwrap_or([0.0; 3]);
        for axis in 0..3 {
            mins[axis] = f32::from_bits(u32::from_le_bytes(
                models[offset + axis * 4..offset + axis * 4 + 4]
                    .try_into()
                    .unwrap(),
            )) + origin[axis];
            maxs[axis] = f32::from_bits(u32::from_le_bytes(
                models[offset + 12 + axis * 4..offset + 16 + axis * 4]
                    .try_into()
                    .unwrap(),
            )) + origin[axis];
        }
        Some((mins, maxs))
    }
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
    let value = input[start..*cursor]
        .iter()
        .map(|&byte| byte as char)
        .collect();
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

fn parse_vec3(value: &str) -> Option<[f32; 3]> {
    let mut fields = value.split_whitespace();
    let result = [
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
    ];
    fields.next().is_none().then_some(result)
}

fn base_actor_type(entity: &Entity) -> Option<u8> {
    Some(
        match entity.get("classname").map(String::as_str).unwrap_or("") {
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
            "monster_generic" => {
                let model = entity
                    .get("model")?
                    .replace('\\', "/")
                    .rsplit('/')
                    .next()?
                    .to_ascii_lowercase();
                return match model.as_str() {
                    "scientist.mdl" => Some(0),
                    "loader.mdl" => Some(52),
                    "forklift.mdl" => Some(53),
                    "holo.mdl" => Some(56),
                    _ => None,
                };
            }
            _ => return None,
        },
    )
}

fn cooked_actor_type(entity: &Entity, entities: &[Entity]) -> Option<u8> {
    let ty = base_actor_type(entity)?;
    if ty == 5
        && entities.iter().any(|script| {
            script.get("classname").map(String::as_str) == Some("scripted_sequence")
                && script
                    .get("m_iszIdle")
                    .or_else(|| script.get("m_iszPlay"))
                    .is_some_and(|name| {
                        name.eq_ignore_ascii_case("ventclimbidle")
                            || name.eq_ignore_ascii_case("ventclimb")
                    })
        })
    {
        return Some(55);
    }
    if ty == 0 {
        let target = entity.get("targetname").map(String::as_str).unwrap_or("");
        if !target.is_empty()
            && entities.iter().any(|script| {
                script.get("classname").map(String::as_str) == Some("scripted_sequence")
                    && script.get("m_iszEntity").map(String::as_str) == Some(target)
                    && script
                        .get("m_iszIdle")
                        .is_some_and(|clip| clip.eq_ignore_ascii_case("sitidle"))
            })
        {
            return Some(54);
        }
    }
    Some(ty)
}

fn actor_body(entity: &Entity, ty: u8, origin: [f32; 3]) -> u8 {
    if !matches!(ty, 0 | 25 | 54) {
        return 0;
    }
    match entity
        .get("body")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0)
    {
        -1 => {
            let seed = origin[0].to_bits()
                ^ origin[1].to_bits().rotate_left(11)
                ^ origin[2].to_bits().rotate_left(22);
            (seed & 3) as u8
        }
        value => value.clamp(0, 7) as u8,
    }
}

fn actor_bounds(entity: &Entity, ty: u8) -> Option<([f32; 3], [f32; 3])> {
    let origin = parse_vec3(entity.get("origin").map(String::as_str).unwrap_or(""))?;
    let (mins, maxs) = match ty {
        2 => ([-12.0, -12.0, 0.0], [12.0, 12.0, 24.0]),
        6 => ([-16.0, -16.0, 0.0], [16.0, 16.0, 36.0]),
        7 | 10 | 11 | 17 | 18 => ([-32.0, -32.0, 0.0], [32.0, 32.0, 64.0]),
        12 => ([-16.0, -16.0, -32.0], [16.0, 16.0, 0.0]),
        13 | 14 => ([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]),
        19 => ([-32.0, -32.0, -32.0], [32.0, 32.0, 32.0]),
        20 => ([-16.0, -16.0, -64.0], [16.0, 16.0, 64.0]),
        21 => ([-32.0, -32.0, -16.0], [32.0, 32.0, 16.0]),
        22 => ([-16.0, -16.0, -16.0], [16.0, 16.0, 16.0]),
        23 => ([-32.0, -32.0, -64.0], [32.0, 32.0, 0.0]),
        24 => ([-5.0, -5.0, 0.0], [5.0, 5.0, 2.0]),
        25 => ([-14.0, -14.0, 0.0], [14.0, 14.0, 36.0]),
        _ => ([-16.0, -16.0, 0.0], [16.0, 16.0, 72.0]),
    };
    Some((
        [
            origin[0] + mins[0],
            origin[1] + mins[1],
            origin[2] + mins[2],
        ],
        [
            origin[0] + maxs[0],
            origin[1] + maxs[1],
            origin[2] + maxs[2],
        ],
    ))
}

fn bounds_overlap(a: ([f32; 3], [f32; 3]), b: ([f32; 3], [f32; 3])) -> bool {
    (0..3).all(|axis| a.0[axis] <= b.1[axis] && a.1[axis] >= b.0[axis])
}

fn transition_rows(
    valve: &Path,
    maps: &[&str],
    static_actors: &[Vec<Actor>],
    chunks: &[Option<ModelChunk>],
    carry_chunks: &[Option<ModelChunk>],
    c4a1b_garg: &ModelChunk,
    c4a3_garg: &ModelChunk,
    c4a3_icky: &ModelChunk,
    c1a2b_zombie: &ModelChunk,
    pressure_islave: &ModelChunk,
    map_variants: &[HashMap<usize, ModelChunk>],
    budgets: &[usize],
) -> Result<Vec<AuditRow>> {
    let mut bsps = HashMap::new();
    let mut map_index = HashMap::new();
    for (index, &name) in maps.iter().enumerate() {
        map_index.insert(name.to_string(), index);
        bsps.insert(
            name.to_string(),
            Bsp::load(&valve.join("maps").join(format!("{name}.bsp")))?,
        );
    }
    let mut rows = Vec::new();
    for &source in maps {
        let bsp = &bsps[source];
        let entities = bsp.entities();
        let landmarks = entities
            .iter()
            .filter(|entity| entity.get("classname").map(String::as_str) == Some("info_landmark"))
            .filter_map(|entity| {
                Some((
                    entity.get("targetname")?.clone(),
                    parse_vec3(entity.get("origin").map(String::as_str).unwrap_or(""))?,
                ))
            })
            .collect::<HashMap<_, _>>();
        for changelevel in entities.iter().filter(|entity| {
            entity.get("classname").map(String::as_str) == Some("trigger_changelevel")
        }) {
            let destination = changelevel.get("map").map(String::as_str).unwrap_or("");
            let landmark = changelevel
                .get("landmark")
                .map(String::as_str)
                .unwrap_or("");
            let Some(&destination_index) = map_index.get(destination) else {
                continue;
            };
            let Some(&viewpoint) = landmarks.get(landmark) else {
                continue;
            };
            let volumes = entities
                .iter()
                .filter(|entity| {
                    entity.get("classname").map(String::as_str) == Some("trigger_transition")
                        && entity.get("targetname").map(String::as_str) == Some(landmark)
                })
                .filter_map(|entity| bsp.brush_bounds(entity))
                .collect::<Vec<_>>();
            let mut carry = Vec::new();
            for entity in &entities {
                let Some(ty) = cooked_actor_type(entity, &entities) else {
                    continue;
                };
                if matches!(ty, 16 | 50)
                    || (!entity.contains_key("targetname") && !entity.contains_key("globalname"))
                {
                    continue;
                }
                let Some(bounds) = actor_bounds(entity, ty) else {
                    continue;
                };
                if !bsp.box_visible(viewpoint, bounds)
                    || (!volumes.is_empty()
                        && !volumes
                            .iter()
                            .copied()
                            .any(|volume| bounds_overlap(bounds, volume)))
                {
                    continue;
                }
                let origin = parse_vec3(entity.get("origin").map(String::as_str).unwrap_or(""))
                    .unwrap_or([0.0; 3]);
                carry.push(Actor {
                    ty,
                    body: actor_body(entity, ty, origin),
                });
                if carry.len() == CARRY_CAPACITY {
                    break;
                }
            }
            let mut ordered = carry;
            ordered.extend_from_slice(&static_actors[destination_index]);
            let label = format!("{source}->{destination}[{landmark}]");
            rows.push(simulate(
                &label,
                budgets[destination_index],
                chunks,
                carry_chunks,
                match destination {
                    "c4a1b" => Some(c4a1b_garg),
                    "c4a3" => Some(c4a3_garg),
                    _ => None,
                },
                (destination == "c4a3").then_some(c4a3_icky),
                (destination == "c1a2b").then_some(c1a2b_zombie),
                matches!(destination, "c1a2b" | "c4a3").then_some(pressure_islave),
                map_variants.get(destination_index),
                &ordered,
                ordered.len() - static_actors[destination_index].len(),
            )?);
        }
    }
    Ok(rows)
}

fn write_report(path: &Path, rows: &[AuditRow]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = String::from(
        "map_or_transition,resident_words,transient_peak_words,slack_bytes,faces,face_runs,textures,model_slots\n",
    );
    for row in rows {
        output.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            row.label,
            row.resident_words,
            row.peak_words,
            row.pool_words.saturating_sub(row.peak_words) * 4,
            row.faces,
            row.runs,
            row.textures,
            row.slots
        ));
    }
    fs::write(path, output)?;
    Ok(())
}

fn audit_weapon_cache_tail(
    repository: &Path,
    maps: &[&str],
    rows: &[AuditRow],
    model_chunks: &[Option<ModelChunk>],
) -> Result<PathBuf> {
    let rooms = repository.join("data/rooms");
    let map_pool_words = fs::read_dir(&rooms)?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with("room_") && matches!(path.extension()?.to_str()?, "psxc" | "psxw"))
                .then(|| {
                    fs::metadata(path)
                        .ok()
                        .map(|metadata| metadata.len() as usize)
                })
                .flatten()
        })
        .max()
        .ok_or("weapon cache audit found no cooked room chunks")?
        .div_ceil(4);

    let model_pack = repository.join("data/modelpack");
    let mut max_chunk_words = 0usize;
    let mut max_model_verts = model_chunks
        .iter()
        .flatten()
        .map(|chunk| chunk.n_verts)
        .max()
        .unwrap_or(0);
    for index in 0..VIEWMODEL_COUNT {
        let path = model_pack.join(format!("chunk_{}.psxm", VIEWMODEL_CHUNK_BASE + index));
        let data = fs::read(&path)?;
        let what = path.display().to_string();
        if data.get(0..4) != Some(b"HMRG") || data.len() < 20 {
            return Err(format!("{what}: expected merged HMD8 viewmodel").into());
        }
        let geometry_len = u32le(&data, 4, &what)? as usize;
        let geometry = checked_range(&data, 8, geometry_len, &what)?;
        if geometry.get(0..4) != Some(b"HMD8") {
            return Err(format!("{what}: merged geometry is not HMD8").into());
        }
        max_model_verts = max_model_verts.max(u32le(geometry, 4, &what)? as usize);
        max_chunk_words = max_chunk_words.max(data.len().div_ceil(4));
    }
    let projected_words = max_model_verts + max_model_verts.div_ceil(2);
    // Conservatively reserve the 64 u16 bucket heads in the MODEL_BUF overlay.
    // A renderer may relocate them to CPU scratchpad, but the animation/cache
    // checkpoint must remain independently safe without that optimization.
    let bucket_head_words = (VIEWMODEL_SORT_BUCKETS * core::mem::size_of::<u16>()).div_ceil(4);
    let scratch_words = projected_words + VIEWMODEL_SORT_TRIS * 2 + bucket_head_words;
    let scratch_start = VIEWMODEL_POOL_WORDS
        .checked_sub(scratch_words)
        .ok_or("viewmodel scratch exceeds its fixed pool")?;
    let backup_words = max_chunk_words.saturating_sub(scratch_start);
    let required_words = max_chunk_words + backup_words + VIEWMODEL_CACHE_GUARD_WORDS;

    let map_index = maps
        .iter()
        .enumerate()
        .map(|(index, &name)| (name, index))
        .collect::<HashMap<_, _>>();
    let report = repository.join(".hlpsx/reports/weapon-cache-ram.csv");
    let mut output = String::from(
        "map_or_transition,destination,world_words,model_resident_words,combined_tail_bytes,required_bytes,margin_bytes\n",
    );
    let mut minimum = None::<(&str, usize)>;
    for row in rows {
        let destination = row
            .label
            .split_once("->")
            .map(|(_, tail)| tail.split('[').next().unwrap_or(tail))
            .unwrap_or(&row.label);
        let Some(&index) = map_index.get(destination) else {
            return Err(format!("{}: audit destination is not in map registry", row.label).into());
        };
        let world = fs::metadata(rooms.join(format!("room_{}.psxc", index * 2)))?.len() as usize;
        let world_words = world.div_ceil(4);
        let combined_words = map_pool_words.saturating_sub(world_words)
            + MODEL_POOL_WORDS.saturating_sub(row.resident_words);
        if combined_words < required_words {
            return Err(format!(
                "{}: weapon cache needs {} B but combined resident tails provide {} B",
                row.label,
                required_words * 4,
                combined_words * 4
            )
            .into());
        }
        let margin = combined_words - required_words;
        if minimum.is_none_or(|(_, current)| margin < current) {
            minimum = Some((&row.label, margin));
        }
        output.push_str(&format!(
            "{},{destination},{world_words},{},{},{},{}\n",
            row.label,
            row.resident_words,
            combined_words * 4,
            required_words * 4,
            margin * 4
        ));
    }
    if let Some(parent) = report.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&report, output)?;
    let (minimum_label, minimum_words) = minimum.ok_or("weapon cache audit had no rows")?;
    println!(
        "weapon cache RAM audit: {} B required, minimum margin {} B at {}",
        required_words * 4,
        minimum_words * 4,
        minimum_label
    );
    println!("  report: {}", report.display());
    Ok(report)
}

fn required_semantic_clips(ty: usize) -> usize {
    match ty {
        // Static pickups/world items do not have an AI state machine.
        3 | 4 | 24 | 26..=49 => 1,
        // Retail roach has only idle/scuttle; there is no combat/death set.
        14 => 2,
        // Script-only construction models expose authored route clips beyond
        // the common idle/move/attack/death/hit state set.
        52 => 6,
        53 => 7,
        54 | 55 => 7,
        _ => 5,
    }
}

const STUDIO_SEQDESC_BYTES: usize = 176;
const STUDIO_EVENT_BYTES: usize = 76;
const STUDIO_LOOPING: i32 = 1;

#[derive(Debug)]
struct SourceSequence {
    index: usize,
    label: String,
    fps: f32,
    flags: i32,
    activity: i32,
    activity_weight: i32,
    frames: usize,
    motion_type: i32,
    blends: usize,
    events: Vec<(i32, i32)>,
}

impl SourceSequence {
    fn looping(&self) -> bool {
        self.flags & STUDIO_LOOPING != 0
    }

    fn hold_ticks(&self) -> u16 {
        if self.frames <= 1 || !self.fps.is_finite() || self.fps <= 0.0 {
            return 1;
        }
        (((self.frames - 1) as f32 * 20.0 / self.fps).ceil() as usize).clamp(1, u16::MAX as usize)
            as u16
    }
}

#[derive(Clone, Copy, Debug)]
struct SelectedSequence {
    sequence: usize,
    slot: usize,
    frame_cap: usize,
}

fn source_sequences(path: &Path) -> Result<Vec<SourceSequence>> {
    let data = fs::read(path)?;
    let what = path.display().to_string();
    if checked_range(&data, 0, 4, &what)? != b"IDST" {
        return Err(format!("{what}: not a GoldSrc studio MDL").into());
    }
    let count = i32le(&data, 164, &what)?;
    let table = i32le(&data, 168, &what)?;
    if count < 0 || table < 0 {
        return Err(format!("{what}: invalid studio sequence table").into());
    }
    checked_range(
        &data,
        table as usize,
        count as usize * STUDIO_SEQDESC_BYTES,
        &what,
    )?;
    let mut result = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        let at = table as usize + index * STUDIO_SEQDESC_BYTES;
        let event_count = i32le(&data, at + 48, &what)?;
        let event_table = i32le(&data, at + 52, &what)?;
        if event_count < 0 || event_table < 0 {
            return Err(format!("{what}: sequence {index} has an invalid event table").into());
        }
        checked_range(
            &data,
            event_table as usize,
            event_count as usize * STUDIO_EVENT_BYTES,
            &what,
        )?;
        let mut events = Vec::with_capacity(event_count as usize);
        for event_index in 0..event_count as usize {
            let event_at = event_table as usize + event_index * STUDIO_EVENT_BYTES;
            events.push((
                i32le(&data, event_at, &what)?,
                i32le(&data, event_at + 4, &what)?,
            ));
        }
        result.push(SourceSequence {
            index,
            label: cstr(checked_range(&data, at, 32, &what)?),
            fps: f32le(&data, at + 32, &what)?,
            flags: i32le(&data, at + 36, &what)?,
            activity: i32le(&data, at + 40, &what)?,
            activity_weight: i32le(&data, at + 44, &what)?,
            frames: i32le(&data, at + 56, &what)?.max(1) as usize,
            motion_type: i32le(&data, at + 68, &what)?,
            blends: i32le(&data, at + 120, &what)?.max(1) as usize,
            events,
        });
    }
    Ok(result)
}

fn selected_sequences(text: &str, sequences: &[SourceSequence]) -> Result<Vec<SelectedSequence>> {
    let mut result = Vec::new();
    for token in text
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        if token.contains('=') {
            continue;
        }
        let (name, cap) = token.split_once(':').unwrap_or((token, "16"));
        let frame_cap = cap.parse::<usize>()?.clamp(1, MAX_BAKED_SEQUENCE_FRAMES);
        let sequence = if let Ok(index) = name.parse::<usize>() {
            index
        } else {
            sequences
                .iter()
                .position(|sequence| sequence.label.eq_ignore_ascii_case(name))
                .ok_or_else(|| format!("source sequence {name:?} is missing"))?
        };
        if sequence >= sequences.len() {
            return Err(format!(
                "source sequence {sequence} is outside 0..{}",
                sequences.len().saturating_sub(1)
            )
            .into());
        }
        result.push(SelectedSequence {
            sequence,
            slot: result.len(),
            frame_cap,
        });
    }
    Ok(result)
}

fn sampled_source_frames(sequence: &SourceSequence, count: usize) -> String {
    (0..count.max(1))
        .map(|frame| {
            let source = if count <= 1 || sequence.frames <= 1 {
                0
            } else if sequence.looping() {
                frame * (sequence.frames - 1) / count
            } else {
                frame * (sequence.frames - 1) / (count - 1)
            };
            source.to_string()
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn csv_cell(value: &str) -> String {
    if value
        .bytes()
        .any(|byte| matches!(byte, b',' | b'"' | b'\n' | b'\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn load_merged_model_chunk(path: &Path) -> Result<ModelChunk> {
    let data = fs::read(path)?;
    let what = path.display().to_string();
    if data.get(0..4) != Some(b"HMRG") {
        return Err(format!("{what}: expected merged model stream").into());
    }
    let geometry_len = u32le(&data, 4, &what)? as usize;
    let geometry = checked_range(&data, 8, geometry_len, &what)?;
    let texture = checked_range(
        &data,
        8 + geometry_len,
        data.len().saturating_sub(8 + geometry_len),
        &what,
    )?;
    parse_geometry(geometry, texture, true, &what)
}

struct AnimationAuditRow<'a> {
    kind: &'a str,
    runtime_index: usize,
    model: &'a str,
    sequence: &'a SourceSequence,
    selection: Option<SelectedSequence>,
    chunk: &'a ModelChunk,
}

fn append_animation_row(output: &mut String, row: AnimationAuditRow<'_>) {
    let sequence = row.sequence;
    let source_ticks = sequence.hold_ticks();
    let (slot, frame_cap, cooked_frames, cooked_ticks, samples, status) =
        if let Some(selected) = row.selection {
            let cooked_frames = row
                .chunk
                .clip_frames
                .get(selected.slot)
                .copied()
                .unwrap_or(0);
            let cooked_ticks = row
                .chunk
                .clip_hold_ticks
                .get(selected.slot)
                .copied()
                .unwrap_or(0);
            let timing_error = cooked_ticks as i32 - source_ticks as i32;
            let status = if cooked_frames == 0 {
                "missing-clip"
            } else if cooked_frames == 1 && sequence.frames > 1 {
                "static-loss"
            } else if timing_error.unsigned_abs() > 1 {
                "timing-drift"
            } else if cooked_frames < sequence.frames {
                "sampled"
            } else {
                "exact"
            };
            (
                selected.slot.to_string(),
                selected.frame_cap.to_string(),
                cooked_frames.to_string(),
                cooked_ticks.to_string(),
                sampled_source_frames(sequence, cooked_frames),
                status,
            )
        } else {
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                "omitted",
            )
        };
    let timing_error = if cooked_ticks.is_empty() {
        String::new()
    } else {
        (cooked_ticks.parse::<i32>().unwrap_or(0) - source_ticks as i32).to_string()
    };
    let events = sequence
        .events
        .iter()
        .map(|(frame, event)| format!("{frame}:{event}"))
        .collect::<Vec<_>>()
        .join(";");
    output.push_str(&format!(
        "{},{},{},{},{},{:.3},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
        row.kind,
        row.runtime_index,
        csv_cell(row.model),
        sequence.index,
        csv_cell(&sequence.label),
        sequence.fps,
        usize::from(sequence.looping()),
        sequence.activity,
        sequence.activity_weight,
        sequence.motion_type,
        sequence.blends,
        sequence.frames,
        source_ticks,
        slot,
        frame_cap,
        cooked_frames,
        cooked_ticks,
        timing_error,
        samples,
        csv_cell(&events),
        status,
    ));
}

fn audit_animation_parity(
    repository: &Path,
    valve: &Path,
    chunks: &[Option<ModelChunk>],
) -> Result<PathBuf> {
    let report = repository.join(".hlpsx/reports/animation-parity.csv");
    if let Some(parent) = report.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = String::from(
        "kind,runtime_index,model,source_sequence,label,fps,looping,activity,activity_weight,motion_type,blends,source_frames,source_ticks,cooked_slot,frame_cap,cooked_frames,cooked_ticks,timing_error_ticks,sampled_source_frames,events,status\n",
    );
    let model_pack = repository.join("data/modelpack");
    for (index, weapon) in super::WEAPON_MODELS.iter().enumerate() {
        let sequences =
            source_sequences(&valve.join("models").join(format!("{}.mdl", weapon.name)))?;
        let selected = selected_sequences(weapon.sequences, &sequences)?;
        let chunk =
            load_merged_model_chunk(&model_pack.join(format!("chunk_{}.psxm", 1000 + index)))?;
        for sequence in &sequences {
            let matches = selected
                .iter()
                .copied()
                .filter(|selected| selected.sequence == sequence.index)
                .collect::<Vec<_>>();
            if matches.is_empty() {
                append_animation_row(
                    &mut output,
                    AnimationAuditRow {
                        kind: "weapon",
                        runtime_index: index,
                        model: weapon.name,
                        sequence,
                        selection: None,
                        chunk: &chunk,
                    },
                );
            } else {
                for selection in matches {
                    append_animation_row(
                        &mut output,
                        AnimationAuditRow {
                            kind: "weapon",
                            runtime_index: index,
                            model: weapon.name,
                            sequence,
                            selection: Some(selection),
                            chunk: &chunk,
                        },
                    );
                }
            }
        }
    }
    let roster = fs::read_to_string(repository.join("host/hl-content/model-roster.txt"))?;
    for line in roster
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.splitn(3, '|');
        let ty = fields
            .next()
            .ok_or("roster type missing")?
            .parse::<usize>()?;
        let model = fields.next().ok_or("roster model missing")?;
        let specs = fields.next().ok_or("roster sequences missing")?;
        let sequences = source_sequences(&valve.join("models").join(format!("{model}.mdl")))?;
        let selected = selected_sequences(specs, &sequences)?;
        let chunk = chunks
            .get(ty)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("model type {ty} ({model}) has no cooked stream"))?;
        for sequence in &sequences {
            let matches = selected
                .iter()
                .copied()
                .filter(|selected| selected.sequence == sequence.index)
                .collect::<Vec<_>>();
            if matches.is_empty() {
                append_animation_row(
                    &mut output,
                    AnimationAuditRow {
                        kind: "npc",
                        runtime_index: ty,
                        model,
                        sequence,
                        selection: None,
                        chunk,
                    },
                );
            } else {
                for selection in matches {
                    append_animation_row(
                        &mut output,
                        AnimationAuditRow {
                            kind: "npc",
                            runtime_index: ty,
                            model,
                            sequence,
                            selection: Some(selection),
                            chunk,
                        },
                    );
                }
            }
        }
    }
    fs::write(&report, output)?;
    println!("original/cooked animation parity -> {}", report.display());
    Ok(report)
}

fn audit_animation_pose_error(repository: &Path) -> Result<Option<PathBuf>> {
    let model_pack = repository.join("data/modelpack");
    let mut output = String::from(
        "kind,runtime_index,model,slot,sequence,label,source_frames,baked_frames,fps,looping,source_ticks,rms_vertex_error,max_vertex_error,sampled_source_frames,error_curve_rms_max,hmd7_palette_rms,hmd7_palette_max\n",
    );
    let mut rows = 0usize;
    let mut append = |kind: &str, runtime_index: usize, model: &str, path: &Path| -> Result<()> {
        if !path.is_file() {
            return Ok(());
        }
        for line in fs::read_to_string(path)?.lines().skip(1) {
            if line.trim().is_empty() {
                continue;
            }
            output.push_str(&format!(
                "{kind},{runtime_index},{},{}\n",
                csv_cell(model),
                line
            ));
            rows += 1;
        }
        Ok(())
    };
    for (index, weapon) in super::WEAPON_MODELS.iter().enumerate() {
        append(
            "weapon",
            index,
            weapon.name,
            &model_pack.join(format!("anim_weapon_{index}.csv")),
        )?;
    }
    let roster = fs::read_to_string(repository.join("host/hl-content/model-roster.txt"))?;
    for line in roster
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.splitn(3, '|');
        let ty = fields
            .next()
            .ok_or("roster type missing")?
            .parse::<usize>()?;
        let model = fields.next().ok_or("roster model missing")?;
        append(
            "npc",
            ty,
            model,
            &model_pack.join(format!("anim_npc_{ty}.csv")),
        )?;
    }
    if rows == 0 {
        return Ok(None);
    }
    let report = repository.join(".hlpsx/reports/animation-pose-error.csv");
    fs::write(&report, output)?;
    println!(
        "original/reconstructed pose error: {rows} clips -> {}",
        report.display()
    );
    Ok(Some(report))
}

fn audit_animation_coverage(repository: &Path, chunks: &[Option<ModelChunk>]) -> Result<PathBuf> {
    let mut names = vec![String::new(); N_MODEL_TYPES];
    let roster = fs::read_to_string(repository.join("host/hl-content/model-roster.txt"))?;
    for line in roster
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
    {
        let mut fields = line.split('|');
        let Some(ty) = fields.next().and_then(|value| value.parse::<usize>().ok()) else {
            continue;
        };
        if ty < names.len() {
            names[ty] = fields.next().unwrap_or("").to_string();
        }
    }
    let report = repository.join(".hlpsx/reports/model-animation-coverage.csv");
    if let Some(parent) = report.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = String::from(
        "type,model,required_semantic_clips,cooked_clips,cooked_frames,min_clip_frames,max_clip_frames,status\n",
    );
    for ty in 0..N_MODEL_TYPES {
        let required = required_semantic_clips(ty);
        let chunk = chunks[ty]
            .as_ref()
            .ok_or_else(|| format!("model type {ty} ({}) has no cooked stream", names[ty]))?;
        let min_frames = chunk.clip_frames.iter().copied().min().unwrap_or(0);
        let max_frames = chunk.clip_frames.iter().copied().max().unwrap_or(0);
        let status = if chunk.n_clips >= required && min_frames > 0 {
            "ok"
        } else {
            "missing"
        };
        output.push_str(&format!(
            "{ty},{},{required},{},{},{min_frames},{max_frames},{status}\n",
            names[ty], chunk.n_clips, chunk.n_frames
        ));
        if status != "ok" {
            return Err(format!(
                "model type {ty} ({}) has {} cooked clips, needs {required}",
                names[ty], chunk.n_clips
            )
            .into());
        }
    }
    fs::write(&report, output)?;
    println!("model animation coverage -> {}", report.display());
    Ok(report)
}

pub fn audit_model_residency(
    repository: &Path,
    valve: Option<&Path>,
    maps: &[&str],
) -> Result<AuditSummary> {
    let model_pack = repository.join("data/modelpack");
    let chunks = load_chunks(
        &model_pack,
        MODEL_GEOM_CHUNK_BASE,
        Some(MODEL_TEX_CHUNK_BASE),
    )?;
    audit_animation_coverage(repository, &chunks)?;
    if let Some(valve) = valve {
        audit_animation_parity(repository, valve, &chunks)?;
    }
    audit_animation_pose_error(repository)?;
    let carry_chunks = load_chunks(&model_pack, MODEL_CARRY_CHUNK_BASE, None)?;
    let c4a1b_garg = load_split_chunk(
        &model_pack,
        C4A1B_GARG_MODEL_CHUNK,
        C4A1B_GARG_TEXTURE_CHUNK,
    )?;
    let c4a3_garg = load_split_chunk(&model_pack, C4A3_GARG_MODEL_CHUNK, C4A3_GARG_TEXTURE_CHUNK)?;
    let c4a3_icky = load_split_chunk(&model_pack, C4A3_ICKY_MODEL_CHUNK, C4A3_ICKY_TEXTURE_CHUNK)?;
    let c1a2b_zombie = load_split_chunk(
        &model_pack,
        C1A2B_ZOMBIE_MODEL_CHUNK,
        C1A2B_ZOMBIE_TEXTURE_CHUNK,
    )?;
    let pressure_islave = load_split_chunk(
        &model_pack,
        PRESSURE_ISLAVE_MODEL_CHUNK,
        PRESSURE_ISLAVE_TEXTURE_CHUNK,
    )?;
    let map_variants = load_map_variants(&model_pack, maps.len())?;
    if model_pack.join("map-model-variants.txt").is_file() {
        audit_map_clip_manifests(repository, maps, &chunks, &map_variants)?;
    }
    let budgets = map_pool_budgets(repository, maps.len())?;
    let mut static_actors = Vec::with_capacity(maps.len());
    let mut rows = Vec::with_capacity(maps.len() + 256);
    for (index, &name) in maps.iter().enumerate() {
        let actors = parse_map_actors(
            &repository
                .join("data/rooms")
                .join(format!("room_{}.psxc", index * 2)),
        )?;
        rows.push(simulate(
            name,
            budgets[index],
            &chunks,
            &carry_chunks,
            match name {
                "c4a1b" => Some(&c4a1b_garg),
                "c4a3" => Some(&c4a3_garg),
                _ => None,
            },
            (name == "c4a3").then_some(&c4a3_icky),
            (name == "c1a2b").then_some(&c1a2b_zombie),
            matches!(name, "c1a2b" | "c4a3").then_some(&pressure_islave),
            map_variants.get(index),
            &actors,
            0,
        )?);
        static_actors.push(actors);
    }
    let transitions = if let Some(valve) = valve {
        let transition_rows = transition_rows(
            valve,
            maps,
            &static_actors,
            &chunks,
            &carry_chunks,
            &c4a1b_garg,
            &c4a3_garg,
            &c4a3_icky,
            &c1a2b_zombie,
            &pressure_islave,
            &map_variants,
            &budgets,
        )?;
        let count = transition_rows.len();
        rows.extend(transition_rows);
        count
    } else {
        0
    };
    let peak = rows
        .iter()
        .min_by_key(|row| row.pool_words.saturating_sub(row.peak_words))
        .ok_or("model residency audit had no maps")?;
    audit_weapon_cache_tail(repository, maps, &rows, &chunks)?;
    let report = repository.join(".hlpsx/reports/model-residency.csv");
    write_report(&report, &rows)?;
    let summary = AuditSummary {
        maps: maps.len(),
        transitions,
        peak_label: peak.label.clone(),
        peak_words: peak.peak_words,
        slack_bytes: peak.pool_words.saturating_sub(peak.peak_words) * 4,
        report,
    };
    println!(
        "model RAM audit: {} maps + {} transitions, peak {} = {} / {} words ({} B slack)",
        summary.maps,
        summary.transitions,
        summary.peak_label,
        summary.peak_words,
        peak.pool_words,
        summary.slack_bytes
    );
    println!("  report: {}", summary.report.display());
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_capacities_match_the_runtime_sources() {
        let main = include_str!("../../game/src/main.rs");
        let build = include_str!("../../game/build.rs");
        for declaration in [
            "const N_MODEL_TYPES: usize = 76;",
            "const MAX_LOADED_MODELS: usize = 23;",
            "const POOL_TEX_SLOTS: usize = 176;",
            "const POOL_FACE_CAP: usize = 8640;",
            "const POOL_FACE_RUN_CAP: usize = 192;",
            "const VM_POOL_WORDS: usize = 20_224;",
            "const VM_TAIL_GUARD_WORDS: usize = 2_030;",
            "const VM_SORT_RECORD_WORDS: usize = MAX_WEAPON_TRIS;",
        ] {
            assert!(main.contains(declaration), "runtime drift: {declaration}");
        }
        assert!(build.contains("const MODEL_POOL_WORDS: usize = 55_936;"));
    }

    #[test]
    fn hmd8_depth_scale_is_always_shift_exact() {
        for valid in [0, 256, 512, 1024, 2048, 4096] {
            assert!(valid_hmd8_scale_q12(valid));
        }
        for invalid in [1, 255, 257, 511, 513, 1000, 4095, 4097, u16::MAX] {
            assert!(!valid_hmd8_scale_q12(invalid));
        }
    }

    #[test]
    fn hazard_course_holo_uses_source_idle_and_walk_slots() {
        let roster = include_str!("../hl-content/model-roster.txt");
        assert!(roster
            .lines()
            .any(|line| { line.starts_with("56|holo|troom_talkidle:4,walk:4,troom_quadjump:4") }));
    }

    #[test]
    fn entity_parser_preserves_transition_keys() {
        let entities = parse_entities(
            br#"{ "classname" "trigger_changelevel" "map" "c1a0a" "landmark" "lm" }"#,
        );
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].get("map").map(String::as_str), Some("c1a0a"));
    }

    #[test]
    fn ai_streaming_tiers_match_runtime_classes() {
        assert_eq!(ai_pass(8), 0);
        assert_eq!(ai_pass(30), 1);
        assert_eq!(ai_pass(17), 2);
        assert_eq!(ai_pass(54), 0);
    }
}

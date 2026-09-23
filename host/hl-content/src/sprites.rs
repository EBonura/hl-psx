use crate::generators::bsp_entities;
use crate::Result;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const SPR_MAX: usize = 64;
const MAX_FRAMES: usize = 6;
const MAX_SPRITES_PER_MAP: usize = 12;
const MAX_FRAMES_PER_MAP: usize = 49;
// Per-map voice packs now span 3100..3202 (103 maps). Keep a full namespace
// gap so adding training maps cannot collide with per-map sprite chunks.
const CHUNK_BASE: usize = 3300;
const EXPLOSION_CHUNK: usize = 3002;
/// Resident decal splats (decals.wad), loaded every map like the explosion.
const DECAL_CHUNK: usize = 3004;
/// The SDK's random choices, in the order the runtime indexes them: bullet and
/// crowbar hits take DECAL_GUNSHOT1 + RANDOM_LONG(0,4) (CBaseEntity::DamageDecal),
/// red blood DECAL_BLOOD1 + RANDOM_LONG(0,5) and yellow blood DECAL_YBLOOD1 +
/// RANDOM_LONG(0,5) (UTIL_BloodDecalTrace).
const DECAL_FAMILIES: [(&str, usize); 3] = [("{shot", 5), ("{blood", 6), ("{yblood", 6)];
/// Every decal shares one 128x128 4bpp texture (one atlas window, one CLUT):
/// blood splats resampled into 32x32 cells, gunshot holes at their native
/// 16x16 along the bottom row.
const DECAL_ATLAS: usize = 128;

#[derive(Clone)]
struct Frame {
    width: usize,
    height: usize,
    indices: Vec<u8>,
    palette: Vec<[u8; 3]>,
}

#[derive(Clone)]
struct Sprite {
    blend: u8,
    base_width: u16,
    base_height: u16,
    frames: Vec<Frame>,
}

struct Crushed {
    width: u16,
    height: u16,
    clut: [u16; 16],
    pixels: Vec<u8>,
}

fn i32le(data: &[u8], offset: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or("short i32")?
            .try_into()?,
    ))
}

fn u16le(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        data.get(offset..offset + 2)
            .ok_or("short u16")?
            .try_into()?,
    ))
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn bgr555(color: [u8; 3]) -> u16 {
    ((color[2] as u16 >> 3) << 10) | ((color[1] as u16 >> 3) << 5) | (color[0] as u16 >> 3)
}

fn find_case_insensitive(directory: &Path, wanted: &str) -> Result<Option<PathBuf>> {
    let wanted = wanted
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(wanted)
        .to_ascii_lowercase();
    let wanted = if wanted.ends_with(".spr") {
        wanted
    } else {
        format!("{wanted}.spr")
    };
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(current)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|v| v.to_str())
                .map(|v| v.eq_ignore_ascii_case(&wanted))
                == Some(true)
            {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

fn decode(path: &Path) -> Result<Sprite> {
    let data = fs::read(path)?;
    if data.len() < 42 || &data[..4] != b"IDSP" {
        return Err(format!("{}: not an IDSP sprite", path.display()).into());
    }
    let texture_format = i32le(&data, 12)?;
    let base_width = i32le(&data, 20)?.clamp(0, u16::MAX as i32) as u16;
    let base_height = i32le(&data, 24)?.clamp(0, u16::MAX as i32) as u16;
    let frame_count = i32le(&data, 28)?.max(0) as usize;
    let color_count = u16le(&data, 40)? as usize;
    let palette_at = 42usize;
    if palette_at + color_count * 3 > data.len() {
        return Err("sprite palette exceeds file".into());
    }
    let palette: Vec<[u8; 3]> = data[palette_at..palette_at + color_count * 3]
        .chunks_exact(3)
        .map(|v| [v[0], v[1], v[2]])
        .collect();
    let mut cursor = palette_at + color_count * 3;
    let step = if frame_count > MAX_FRAMES {
        (frame_count / MAX_FRAMES).max(1)
    } else {
        1
    };
    let mut frames = Vec::new();
    for frame_index in 0..frame_count {
        let group = i32le(&data, cursor)?;
        cursor += 4;
        if group != 0 {
            let count = i32le(&data, cursor)?.max(0) as usize;
            cursor += 4 + count * 4;
        }
        let width = i32le(&data, cursor + 8)?;
        let height = i32le(&data, cursor + 12)?;
        cursor += 16;
        if width <= 0 || height <= 0 {
            return Err("invalid sprite frame size".into());
        }
        let size = width as usize * height as usize;
        let indices = data
            .get(cursor..cursor + size)
            .ok_or("sprite frame exceeds file")?
            .to_vec();
        cursor += size;
        if frame_index % step == 0 && frames.len() < MAX_FRAMES {
            frames.push(Frame {
                width: width as usize,
                height: height as usize,
                indices,
                palette: palette.clone(),
            });
        }
    }
    Ok(Sprite {
        blend: u8::from(texture_format == 1),
        base_width,
        base_height,
        frames,
    })
}

fn median_cut(mut colors: Vec<[u8; 3]>, count: usize) -> Vec<[u8; 3]> {
    if colors.is_empty() {
        return vec![[0, 0, 0]];
    }
    let mut boxes = vec![std::mem::take(&mut colors)];
    while boxes.len() < count {
        let mut best = None;
        for (index, colors) in boxes.iter().enumerate() {
            if colors.len() < 2 {
                continue;
            }
            let mut mins = [u8::MAX; 3];
            let mut maxs = [u8::MIN; 3];
            for color in colors {
                for channel in 0..3 {
                    mins[channel] = mins[channel].min(color[channel]);
                    maxs[channel] = maxs[channel].max(color[channel]);
                }
            }
            let (channel, extent) = (0..3)
                .map(|channel| (channel, maxs[channel] - mins[channel]))
                .max_by_key(|v| v.1)
                .unwrap();
            if best.map(|(_, _, old)| extent > old).unwrap_or(true) {
                best = Some((index, channel, extent));
            }
        }
        let Some((index, channel, _)) = best else {
            break;
        };
        boxes[index].sort_unstable_by_key(|color| color[channel]);
        let middle = boxes[index].len() / 2;
        let right = boxes[index].split_off(middle);
        boxes.insert(index + 1, right);
    }
    boxes
        .into_iter()
        .map(|colors| {
            let mut sum = [0u64; 3];
            for color in &colors {
                for channel in 0..3 {
                    sum[channel] += color[channel] as u64;
                }
            }
            [
                (sum[0] / colors.len() as u64) as u8,
                (sum[1] / colors.len() as u64) as u8,
                (sum[2] / colors.len() as u64) as u8,
            ]
        })
        .collect()
}

fn nearest(palette: &[[u8; 3]], color: [u8; 3]) -> u8 {
    palette
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| {
            (0..3)
                .map(|channel| {
                    let delta = color[channel] as i32 - candidate[channel] as i32;
                    delta * delta
                })
                .sum::<i32>()
        })
        .map(|v| v.0 as u8)
        .unwrap_or(0)
}

fn power_of_two_at_most(value: usize, cap: usize) -> usize {
    let limit = value.min(cap);
    let mut result = 2usize;
    while result * 2 <= limit {
        result *= 2;
    }
    result
}

fn crush(frame: &Frame, blend: u8, cap: usize) -> Crushed {
    let width = power_of_two_at_most(frame.width, cap);
    let height = power_of_two_at_most(frame.height, cap);
    let mut colors = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let index = frame.indices
                [(y * frame.height / height) * frame.width + x * frame.width / width]
                as usize;
            colors.push(frame.palette.get(index).copied().unwrap_or([0, 0, 0]));
        }
    }
    let mut palette = median_cut(colors.clone(), 16);
    if blend == 1 {
        if let Some((darkest, _)) = palette
            .iter()
            .enumerate()
            .min_by_key(|(_, color)| color.iter().map(|&v| v as u16).sum::<u16>())
        {
            palette[darkest] = [0, 0, 0];
        }
    }
    let mut clut = [0u16; 16];
    for (output, color) in clut.iter_mut().zip(&palette) {
        *output = bgr555(*color) | if blend == 1 { 0x8000 } else { 0 };
    }
    let mut pixels = Vec::with_capacity(width * height / 2);
    for pair in colors.chunks(2) {
        let low = nearest(&palette, pair[0]);
        let high = pair
            .get(1)
            .map(|&color| nearest(&palette, color))
            .unwrap_or(0);
        pixels.push(low | (high << 4));
    }
    Crushed {
        width: width as u16,
        height: height as u16,
        clut,
        pixels,
    }
}

fn texture_blob(frame: &Crushed) -> Vec<u8> {
    let mut output = Vec::with_capacity(4 + 32 + frame.pixels.len());
    push_u16(&mut output, frame.width);
    push_u16(&mut output, frame.height);
    for color in frame.clut {
        push_u16(&mut output, color);
    }
    output.extend_from_slice(&frame.pixels);
    output
}

/// Per-map sprite list. The bool marks names referenced by env_beam/env_laser:
/// GoldSrc renders beams additive regardless of the .spr texFormat (lgtning.spr
/// is SPR_NORMAL), so beam textures must take the additive CLUT path in crush().
fn map_sprite_names(bsp: &Path) -> Result<Vec<(String, bool)>> {
    const CLASSES: [&str; 7] = [
        "env_sprite",
        "env_glow",
        "env_spark",
        "env_explosion",
        "cycler_sprite",
        "env_beam",
        "env_laser",
    ];
    let mut seen = HashMap::<String, usize>::new();
    let mut result: Vec<(String, bool)> = Vec::new();
    for entity in bsp_entities(bsp)? {
        let class = entity.get("classname").map(String::as_str).unwrap_or("");
        if !CLASSES.contains(&class) {
            continue;
        }
        let is_beam = class == "env_beam" || class == "env_laser";
        let model = entity
            .get("model")
            .or_else(|| entity.get("texture"))
            .cloned()
            .unwrap_or_default();
        if !model.to_ascii_lowercase().ends_with(".spr") {
            continue;
        }
        let base = model
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&model)
            .to_ascii_lowercase();
        match seen.get(&base) {
            Some(&i) => result[i].1 |= is_beam,
            None => {
                seen.insert(base.clone(), result.len());
                result.push((base, is_beam));
            }
        }
    }
    result.truncate(MAX_SPRITES_PER_MAP);
    Ok(result)
}

fn write_pack(
    output: &Path,
    records: &[(u8, u8, u16, u16, u16, u16, u16)],
    frames: &[Vec<u8>],
) -> Result<()> {
    let mut blob = Vec::new();
    blob.extend_from_slice(b"HSPR");
    push_u16(&mut blob, records.len() as u16);
    push_u16(&mut blob, frames.len() as u16);
    for &(frame_count, blend, first, base_width, base_height, width, height) in records {
        blob.push(frame_count);
        blob.push(blend);
        for value in [first, base_width, base_height, width, height] {
            push_u16(&mut blob, value);
        }
    }
    for frame in frames {
        blob.extend_from_slice(frame);
    }
    fs::write(output, blob)?;
    Ok(())
}

fn build_explosion(valve: &Path, output: &Path) -> Result<()> {
    let sprites = valve.join("sprites");
    let path = find_case_insensitive(&sprites, "zerogxplode.spr")?
        .or(find_case_insensitive(&sprites, "s_explod.spr")?)
        .ok_or("explosion sprite not found")?;
    let sprite = decode(&path)?;
    let crushed: Vec<Crushed> = sprite
        .frames
        .iter()
        .take(5)
        .map(|frame| crush(frame, 1, 32))
        .collect();
    if crushed.is_empty() {
        return Ok(());
    }
    let records = vec![(
        crushed.len() as u8,
        1,
        0,
        sprite.base_width,
        sprite.base_height,
        crushed[0].width,
        crushed[0].height,
    )];
    let frames: Vec<Vec<u8>> = crushed.iter().map(texture_blob).collect();
    write_pack(
        &output.join(format!("chunk_{EXPLOSION_CHUNK}.psxa")),
        &records,
        &frames,
    )?;
    println!(
        "resident explosion -> chunk_{EXPLOSION_CHUNK} ({}, {} frames)",
        path.display(),
        frames.len()
    );
    Ok(())
}

/// Mip level 0 of every `{` decal in a WAD3, by lower-case name.
fn wad_decals(path: &Path) -> Result<HashMap<String, (usize, usize, Vec<u8>)>> {
    let data = fs::read(path)?;
    if data.len() < 12 || &data[..4] != b"WAD3" {
        return Err(format!("{}: not a WAD3", path.display()).into());
    }
    let count = i32le(&data, 4)?.max(0) as usize;
    let directory = i32le(&data, 8)?.max(0) as usize;
    let mut out = HashMap::new();
    for i in 0..count {
        let entry = directory + i * 32;
        let at = i32le(&data, entry)?.max(0) as usize;
        let raw = data.get(entry + 16..entry + 32).ok_or("short WAD entry")?;
        let name = raw
            .split(|&b| b == 0)
            .next()
            .map(|v| String::from_utf8_lossy(v).to_ascii_lowercase())
            .unwrap_or_default();
        if !name.starts_with('{') {
            continue;
        }
        let width = i32le(&data, at + 16)?.max(0) as usize;
        let height = i32le(&data, at + 20)?.max(0) as usize;
        let mip0 = at + i32le(&data, at + 24)?.max(0) as usize;
        let pixels = data
            .get(mip0..mip0 + width * height)
            .ok_or("decal exceeds WAD")?
            .to_vec();
        out.insert(name, (width, height, pixels));
    }
    Ok(out)
}

/// One `size`x`size` cell of GoldSrc decal coverage. A `{` decal's texel index
/// is its alpha (the colour is the palette's last entry), so each cell texel is
/// the mean alpha of the source block it covers, reduced to three PS1 classes:
/// clear (index 0, CLUT 0x0000), half (semi-transparent), full (opaque).
fn decal_cell(width: usize, height: usize, alpha: &[u8], size: usize) -> Vec<u8> {
    let mut cell = vec![0u8; size * size];
    for y in 0..size {
        for x in 0..size {
            let x0 = x * width / size;
            let x1 = ((x + 1) * width / size).max(x0 + 1);
            let y0 = y * height / size;
            let y1 = ((y + 1) * height / size).max(y0 + 1);
            let mut sum = 0usize;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    sum += alpha[sy * width + sx] as usize;
                }
            }
            let mean = sum / ((x1 - x0) * (y1 - y0));
            cell[y * size + x] = if mean >= 150 {
                8
            } else if mean >= 50 {
                1
            } else {
                0
            };
        }
    }
    cell
}

/// Pack "HDCL": magic | u8 decals | u8 per family (shot, blood, yblood) |
/// per decal u8 u, v, cell size, half-size in world units (a decal is drawn
/// one unit per source texel) | pad to 4 | one texture blob
/// (u16 w,h | u16 clut[16] | u8 pix4). The runtime tints each decal with its
/// vertex colour, so the CLUT is white.
fn build_decals(valve: &Path, output: &Path) -> Result<()> {
    let wad = valve.join("decals.wad");
    let decals = wad_decals(&wad)?;
    let mut atlas = vec![0u8; DECAL_ATLAS * DECAL_ATLAS];
    let mut records = Vec::new();
    let mut family_counts = Vec::new();
    let (mut splats, mut holes) = (0usize, 0usize);
    for (prefix, count) in DECAL_FAMILIES {
        for n in 1..=count {
            let name = format!("{prefix}{n}");
            let (w, h, alpha) = decals
                .get(&name)
                .ok_or_else(|| format!("{}: {name} missing", wad.display()))?;
            let (size, cx, cy) = if prefix == "{shot" {
                holes += 1;
                (16, (holes - 1) * 16, DECAL_ATLAS - 32)
            } else {
                splats += 1;
                (32, ((splats - 1) % 4) * 32, ((splats - 1) / 4) * 32)
            };
            if cx + size > DECAL_ATLAS || (prefix != "{shot" && cy + size > DECAL_ATLAS - 32) {
                return Err("decal atlas overflow".into());
            }
            let cell = decal_cell(*w, *h, alpha, size);
            for y in 0..size {
                for x in 0..size {
                    atlas[(cy + y) * DECAL_ATLAS + cx + x] = cell[y * size + x];
                }
            }
            records.extend_from_slice(&[
                cx as u8,
                cy as u8,
                size as u8,
                ((*w.max(h)) / 2).clamp(1, 255) as u8,
            ]);
        }
        family_counts.push(count as u8);
    }
    let mut clut = [0u16; 16];
    for (index, entry) in clut.iter_mut().enumerate().skip(1) {
        *entry = if index < 8 { 0xffff } else { 0x7fff };
    }
    let mut blob = Vec::new();
    blob.extend_from_slice(b"HDCL");
    blob.push((records.len() / 4) as u8);
    blob.extend_from_slice(&family_counts);
    blob.extend_from_slice(&records);
    while blob.len() % 4 != 0 {
        blob.push(0);
    }
    push_u16(&mut blob, DECAL_ATLAS as u16);
    push_u16(&mut blob, DECAL_ATLAS as u16);
    for entry in clut {
        push_u16(&mut blob, entry);
    }
    for pair in atlas.chunks(2) {
        blob.push(pair[0] | (pair[1] << 4));
    }
    fs::write(output.join(format!("chunk_{DECAL_CHUNK}.psxa")), &blob)?;
    println!(
        "resident decals -> chunk_{DECAL_CHUNK} ({} decals, {} bytes)",
        records.len() / 4,
        blob.len()
    );
    Ok(())
}

pub fn build(valve: &Path, map_list: &str, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    let sprite_dir = valve.join("sprites");
    let mut cache = HashMap::<String, Option<Sprite>>::new();
    let mut manifest = Vec::new();
    for (map_index, map_name) in map_list.split_whitespace().enumerate() {
        let bsp = valve.join("maps").join(format!("{map_name}.bsp"));
        if !bsp.exists() {
            continue;
        }
        let mut records = Vec::new();
        let mut frames = Vec::new();
        let mut first = 0usize;
        for (base, is_beam) in map_sprite_names(&bsp)? {
            if !cache.contains_key(&base) {
                let decoded = find_case_insensitive(&sprite_dir, &base)?
                    .map(|path| decode(&path))
                    .transpose()?;
                cache.insert(base.clone(), decoded);
            }
            let Some(sprite) = cache.get(&base).and_then(Option::as_ref) else {
                continue;
            };
            // Beam textures render additive whatever their texFormat says.
            let blend = if is_beam { 1 } else { sprite.blend };
            let crushed: Vec<Crushed> = sprite
                .frames
                .iter()
                .map(|frame| crush(frame, blend, SPR_MAX))
                .collect();
            if crushed.is_empty() {
                continue;
            }
            if first + crushed.len() > MAX_FRAMES_PER_MAP {
                break;
            }
            let local_id = records.len();
            records.push((
                crushed.len() as u8,
                blend,
                first as u16,
                sprite.base_width,
                sprite.base_height,
                crushed[0].width,
                crushed[0].height,
            ));
            frames.extend(crushed.iter().map(texture_blob));
            manifest.push(format!(
                "{map_index}|{local_id}|{base}|{}|{}|{}|{}",
                blend,
                crushed.len(),
                sprite.base_width,
                sprite.base_height
            ));
            first += crushed.len();
        }
        if !records.is_empty() {
            write_pack(
                &output.join(format!("chunk_{}.psxa", CHUNK_BASE + map_index)),
                &records,
                &frames,
            )?;
        }
    }
    fs::write(output.join("manifest.txt"), manifest.join("\n") + "\n")?;
    println!(
        "sprites -> {} ({} entries)",
        output.display(),
        manifest.len()
    );
    build_explosion(valve, output)?;
    build_decals(valve, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_are_power_of_two_and_capped() {
        assert_eq!(power_of_two_at_most(100, 64), 64);
        assert_eq!(power_of_two_at_most(31, 64), 16);
    }

    #[test]
    fn bgr555_keeps_red_in_low_bits() {
        assert_eq!(bgr555([255, 0, 0]), 31);
    }
}

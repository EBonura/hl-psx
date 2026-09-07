use crate::{bonnie_logo::COVER_BONNIE, Result};
use image::imageops::{self, FilterType};
use image::{GrayImage, Luma};
use std::fs;
use std::path::Path;

const LOGO_W: u32 = 224;
const BONNIE_W: u16 = 128;
const BONNIE_H: u16 = 128;
const PICO8_CLUT: [u16; 16] = [
    0x0421, 0x28A3, 0x288F, 0x2A00, 0x1955, 0x254B, 0x6318, 0x77DF, 0x241F, 0x029F, 0x13BF, 0x1B80,
    0x7EA5, 0x4DD0, 0x55DF, 0x573F,
];

fn u16le(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        data.get(offset..offset + 2)
            .ok_or("short u16")?
            .try_into()?,
    ))
}

fn u32le(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or("short u32")?
            .try_into()?,
    ))
}

fn i32le(data: &[u8], offset: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or("short i32")?
            .try_into()?,
    ))
}

fn push_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn bgr555(r: u8, g: u8, b: u8) -> u16 {
    ((b as u16 >> 3) << 10) | ((g as u16 >> 3) << 5) | (r as u16 >> 3)
}

/// Compose the retail menu backdrop from `resource/background/`.
///
/// A Steam install ships this as a tile grid rather than one image, and only
/// the ultrawide set carries the artwork -- every `800_*` (4:3) tile is solid
/// black, which is exactly what the retail 4:3 menu shows. The `21_9_*` set is
/// the grunge, the lambda and the decay-constant scribbles, so compose that and
/// take the centred 4:3 window: it frames the lambda right-of-centre and the
/// uranium-235 text top-left, the way the retail widescreen menu does.
///
/// Returns `None` when the directory is absent (older or non-Steam layouts), so
/// the caller can fall back to the console backdrop.
fn build_official_background(valve: &Path) -> Option<GrayImage> {
    const ROWS: usize = 7;
    const COLUMNS: usize = 15;
    let tile = |row: usize, column: usize| {
        let name = format!(
            "resource/background/21_9_{}_{}_loading.tga",
            row + 1,
            (b'a' + column as u8) as char
        );
        image::open(valve.join(name)).ok().map(|t| t.to_luma8())
    };
    let widths = (0..COLUMNS)
        .map(|column| tile(0, column).map(|t| t.width()))
        .collect::<Option<Vec<_>>>()?;
    let heights = (0..ROWS)
        .map(|row| tile(row, 0).map(|t| t.height()))
        .collect::<Option<Vec<_>>>()?;
    let mut composite = GrayImage::new(widths.iter().sum(), heights.iter().sum());
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let piece = tile(row, column)?;
            let x = widths[..column].iter().sum::<u32>();
            let y = heights[..row].iter().sum::<u32>();
            imageops::replace(&mut composite, &piece, x as i64, y as i64);
        }
    }
    let height = composite.height();
    let window = (height * 4 / 3).min(composite.width());
    let left = (composite.width() - window) / 2;
    Some(imageops::crop_imm(&composite, left, 0, window, height).to_image())
}

fn build_console_background(valve: &Path) -> Result<GrayImage> {
    let data = fs::read(valve.join("gfx/conback.lmp"))?;
    let width = u32le(&data, 0)?;
    let height = u32le(&data, 4)?;
    let pixel_count = width as usize * height as usize;
    let palette_offset = 8 + pixel_count;
    if palette_offset + 256 * 3 > data.len() {
        return Err("conback.lmp palette exceeds file".into());
    }
    let mut source = GrayImage::new(width, height);
    for (index, pixel) in source.pixels_mut().enumerate() {
        let color = data[8 + index] as usize;
        let at = palette_offset + color * 3;
        let lum = (data[at] as u16 + data[at + 1] as u16 + data[at + 2] as u16) / 3;
        *pixel = Luma([lum as u8]);
    }
    Ok(source)
}

fn build_background(valve: &Path, output: &Path) -> Result<()> {
    let (source, origin) = match build_official_background(valve) {
        Some(image) => (image, "resource/background"),
        None => (build_console_background(valve)?, "gfx/conback.lmp"),
    };
    // Triangle, not Lanczos: this is 3840 pixels of photographic grunge going
    // to 256, and Lanczos' ringing turns into visible speckle once quantised.
    let gray = imageops::resize(&source, 256, 240, FilterType::Triangle);

    // 8bpp, where every other menu texture is 4bpp, and drawn straight through:
    // the pixel byte is the index, and the CLUT is the 8-to-5-bit identity. No
    // normalisation, no curve, no floor.
    //
    // That is what retail does -- it draws this image -- and measuring a retail
    // capture is what settled it. Its backdrop runs mean 27, median 25,
    // ninetieth percentile 40, with ninety-three percent under 48 of 255. It is
    // a dark, low-contrast field. Every version here that normalised against
    // the source's own peak stretched that across the full range and came back
    // three to four times too bright, and every version that shaped it with a
    // curve or a black point turned a soft field into hard speckle.
    //
    // Four bits could not do it at all: sixteen levels for a picture living
    // almost entirely in the bottom fifth of the range banded into flat plates
    // however the ramp was arranged. Eight is what makes a faithful copy
    // possible, even though CLUT entries are 15-bit colour and grey therefore
    // tops out at 32 levels whatever the depth.
    //
    // Costs 128 VRAM columns rather than 64, plus a 256-entry CLUT. The menu
    // can afford both: no map is loaded, so this is the one screen with VRAM to
    // spare.
    //
    // The console's black crush is a separate question and deliberately not
    // answered here. A 2026-08-04 capture suggested hardware loses everything
    // below about 64 of 255, which would take most of this with it. But that
    // was one photograph through one capture chain, and every attempt to lift
    // the floor to suit it produced something nothing like the game. Put a grey
    // ramp on screen and measure it properly before trading the look away.
    //
    // Layout is `u16 w | u16 h | u16 clut[256] | u8 pixels`.
    let mut blob = Vec::with_capacity(4 + 512 + 256 * 240);
    push_u16(&mut blob, 256);
    push_u16(&mut blob, 240);
    for index in 0..256u16 {
        let value = index >> 3; // 8-bit grey into the SPU's 5 bits per channel
        push_u16(&mut blob, (value << 10) | (value << 5) | value);
    }
    for row in 0..240 {
        for column in 0..256 {
            blob.push(gray.get_pixel(column, row)[0]);
        }
    }
    fs::write(output.join("bg.tex"), &blob)?;
    println!("bg.tex: 256x240 8bpp from {origin}, {} bytes", blob.len());
    Ok(())
}

fn build_logo(valve: &Path, output: &Path) -> Result<()> {
    build_logo_plate(valve, output, "resource/logo.tga", "logo.tex")
}

fn build_logo_plate(valve: &Path, output: &Path, source: &str, name: &str) -> Result<()> {
    let source = image::open(valve.join(source))?.to_rgba8();
    let height =
        ((LOGO_W as f32 * source.height() as f32 / source.width() as f32).round() as u32).max(1);
    let mut gray = GrayImage::new(source.width(), source.height());
    for (dst, src) in gray.pixels_mut().zip(source.pixels()) {
        let alpha = src[3] as u32;
        let lum = (src[0] as u32 * 77 + src[1] as u32 * 150 + src[2] as u32 * 29) >> 8;
        *dst = Luma([((lum * alpha) / 255) as u8]);
    }
    let gray = imageops::resize(&gray, LOGO_W, height, FilterType::Lanczos3);
    let mut blob = Vec::with_capacity(4 + 32 + (LOGO_W * height / 2) as usize);
    push_u16(&mut blob, LOGO_W as u16);
    push_u16(&mut blob, height as u16);
    push_u16(&mut blob, 0);
    for index in 1..16u16 {
        let value = ((index as f32 / 15.0) * 31.0).round() as u16;
        push_u16(&mut blob, (value << 10) | (value << 5) | value);
    }
    for row in 0..height {
        for column in (0..LOGO_W).step_by(2) {
            let low = gray.get_pixel(column, row)[0] >> 4;
            let high = gray.get_pixel(column + 1, row)[0] >> 4;
            blob.push((high << 4) | low);
        }
    }
    fs::write(output.join(name), &blob)?;
    println!("{name}: {}x{} 4bpp, {} bytes", LOGO_W, height, blob.len());
    Ok(())
}

const LOGO_ANIM_W: usize = 320;
const LOGO_ANIM_H: usize = 50;
const LOGO_ANIM_FRAMES: usize = 37;
const BAYER4: [u16; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
// RLE8 palette/downsample noise leaves even-valued residuals after the
// per-pixel film backdrop is subtracted. The square-root response below would
// otherwise promote a residual of two to a visible additive CLUT entry, making
// the whole 320x50 movie rectangle read as a lighter band. Subtract the source
// noise floor before companding; the authored sweep remains comfortably above
// it, while index zero stays genuinely transparent.
const LOGO_ANIM_MATTE_FLOOR: u8 = 8;

fn avi_chunks<'a>(
    data: &'a [u8],
    mut at: usize,
    end: usize,
    out: &mut Vec<([u8; 4], &'a [u8])>,
) -> Result<()> {
    while at + 8 <= end && at + 8 <= data.len() {
        let id: [u8; 4] = data[at..at + 4].try_into()?;
        let size = u32le(data, at + 4)? as usize;
        let body = at + 8;
        let body_end = body.checked_add(size).ok_or("AVI chunk size overflow")?;
        if body_end > end || body_end > data.len() {
            return Err("AVI chunk exceeds RIFF bounds".into());
        }
        if &id == b"LIST" || &id == b"RIFF" {
            if size < 4 {
                return Err("short AVI list".into());
            }
            avi_chunks(data, body + 4, body_end, out)?;
        } else {
            out.push((id, &data[body..body_end]));
        }
        at = body + ((size + 1) & !1);
    }
    Ok(())
}

fn decode_rle8_frame(chunk: &[u8], width: usize, height: usize, frame: &mut [u8]) -> Result<()> {
    let mut at = 0usize;
    let mut x = 0usize;
    let mut y = 0usize;
    while at + 2 <= chunk.len() {
        let count = chunk[at] as usize;
        let value = chunk[at + 1];
        at += 2;
        if count != 0 {
            if y >= height || x + count > width {
                return Err("RLE8 encoded run exceeds frame".into());
            }
            let row = height - 1 - y;
            frame[row * width + x..row * width + x + count].fill(value);
            x += count;
            continue;
        }
        match value {
            0 => {
                x = 0;
                y += 1;
            }
            1 => return Ok(()),
            2 => {
                if at + 2 > chunk.len() {
                    return Err("short RLE8 delta".into());
                }
                x += chunk[at] as usize;
                y += chunk[at + 1] as usize;
                at += 2;
                if x > width || y > height {
                    return Err("RLE8 delta exceeds frame".into());
                }
            }
            literal => {
                let count = literal as usize;
                if at + count > chunk.len() || y >= height || x + count > width {
                    return Err("RLE8 literal run exceeds frame".into());
                }
                let row = height - 1 - y;
                frame[row * width + x..row * width + x + count]
                    .copy_from_slice(&chunk[at..at + count]);
                x += count;
                at += (count + 1) & !1;
            }
        }
    }
    Err("RLE8 frame has no end marker".into())
}

/// Decode one frame to 320x50 luminance. Quantisation is deferred: every frame
/// of `logo.avi` carries its own copy of the menu backdrop behind the sweep,
/// and that has to be subtracted across the whole clip before anything is
/// reduced to four bits.
fn decode_logo_frame(frame: &[u8], palette: &[u8; 256], width: usize) -> Vec<u8> {
    let mut indices = vec![0u8; LOGO_ANIM_W * LOGO_ANIM_H];
    for y in 0..LOGO_ANIM_H {
        for x in 0..LOGO_ANIM_W {
            let sx = x * 2;
            let sy = y * 2;
            let lum = (palette[frame[sy * width + sx] as usize] as u16
                + palette[frame[sy * width + sx + 1] as usize] as u16
                + palette[frame[(sy + 1) * width + sx] as usize] as u16
                + palette[frame[(sy + 1) * width + sx + 1] as usize] as u16
                + 2)
                / 4;
            indices[y * LOGO_ANIM_W + x] = lum.min(255) as u8;
        }
    }
    indices
}

/// Quantise one already-background-subtracted frame to 4bpp.
fn pack_logo_frame(sweep: &[u8]) -> Vec<u8> {
    let mut indices = vec![0u8; LOGO_ANIM_W * LOGO_ANIM_H];
    for y in 0..LOGO_ANIM_H {
        for x in 0..LOGO_ANIM_W {
            let lum = sweep[y * LOGO_ANIM_W + x].saturating_sub(LOGO_ANIM_MATTE_FLOOR) as u16;
            // The sweep is dark once its backdrop is gone: a linear ramp spends
            // most of sixteen indices on values it never reaches, so use a
            // square-root response to put them where the gradient is.
            let ramp = ((lum as f32 / 255.0).sqrt() * 255.0) as u16;
            let scaled = ramp * 15;
            let base = scaled / 255;
            let rem = scaled % 255;
            let threshold = BAYER4[(y & 3) * 4 + (x & 3)];
            indices[y * LOGO_ANIM_W + x] =
                (base + u16::from(rem * 16 > threshold * 255)).min(15) as u8;
        }
    }
    let mut packed = Vec::with_capacity(LOGO_ANIM_W * LOGO_ANIM_H / 2);
    for row in indices.chunks_exact(LOGO_ANIM_W) {
        for pair in row.chunks_exact(2) {
            packed.push(pair[0] | (pair[1] << 4));
        }
    }
    packed
}

/// Decode GoldSrc's real animated menu title (`media/logo.avi`). The source is
/// an 8-bit RLE DIB, so the Rust cooker can read it without an external video
/// tool. Thirty-seven evenly spaced frames preserve its 4.58-second motion at
/// 8 fps while fitting comfortably in the streamed map buffer.
fn build_logo_animation(valve: &Path, output: &Path) -> Result<()> {
    let data = fs::read(valve.join("media/logo.avi"))?;
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"AVI " {
        return Err("media/logo.avi is not an AVI RIFF".into());
    }
    let mut chunks = Vec::new();
    avi_chunks(&data, 12, data.len(), &mut chunks)?;

    let avih = chunks
        .iter()
        .find(|(id, _)| id == b"avih")
        .map(|(_, body)| *body)
        .ok_or("logo.avi has no avih")?;
    let micros_per_frame = u32le(avih, 0)? as u64;
    let strf = chunks
        .iter()
        .find(|(id, body)| {
            id == b"strf"
                && body.len() >= 40
                && u16::from_le_bytes([body[14], body[15]]) == 8
                && u32::from_le_bytes([body[16], body[17], body[18], body[19]]) == 1
        })
        .map(|(_, body)| *body)
        .ok_or("logo.avi has no 8-bit RLE video format")?;
    let width = i32le(strf, 4)?;
    let height = i32le(strf, 8)?;
    if width != 640 || height != 100 {
        return Err(format!("logo.avi is {width}x{height}, expected 640x100").into());
    }
    let palette_at = u32le(strf, 0)? as usize;
    if palette_at + 256 * 4 > strf.len() {
        return Err("logo.avi palette is truncated".into());
    }
    let mut palette = [0u8; 256];
    for (index, lum) in palette.iter_mut().enumerate() {
        let at = palette_at + index * 4;
        let b = strf[at] as u32;
        let g = strf[at + 1] as u32;
        let r = strf[at + 2] as u32;
        *lum = ((r * 77 + g * 150 + b * 29) >> 8) as u8;
    }

    let encoded = chunks
        .iter()
        // RLE8 keyframes use `db`, while delta frames use `dc`.
        .filter(|(id, _)| id == b"00db" || id == b"00dc")
        .map(|(_, body)| *body)
        .collect::<Vec<_>>();
    if encoded.is_empty() {
        return Err("logo.avi has no compressed video frames".into());
    }
    let mut decoded = vec![0u8; width as usize * height as usize];
    let mut selected = Vec::with_capacity(LOGO_ANIM_FRAMES);
    let mut next_slot = 0usize;
    for (source, chunk) in encoded.iter().enumerate() {
        decode_rle8_frame(chunk, width as usize, height as usize, &mut decoded)?;
        while next_slot < LOGO_ANIM_FRAMES
            && source >= next_slot * (encoded.len() - 1) / (LOGO_ANIM_FRAMES - 1)
        {
            selected.push(decode_logo_frame(&decoded, &palette, width as usize));
            next_slot += 1;
        }
    }
    if selected.len() != LOGO_ANIM_FRAMES {
        return Err("logo.avi frame selection was incomplete".into());
    }
    // Every frame of logo.avi contains the menu backdrop, not just the title:
    // the engine plays it over matching art so the two blend, which our own
    // backdrop crop can never do. Drawn opaque it lays a rectangle of the
    // film's background over ours; drawn additively it adds a second copy of a
    // background. What is actually wanted is the light that moves, so take the
    // per-pixel minimum over the clip -- that is the backdrop plus the static
    // wordmark, everything the sweep never touches -- and subtract it. What
    // remains is the sweep and the sliding wordmark, which composite additively
    // over any backdrop and stay where the light is instead of filling the
    // rectangle. Nothing in the floor exceeds half scale, which is the check
    // that it really is only the film's backdrop.
    let floor = (0..LOGO_ANIM_W * LOGO_ANIM_H)
        .map(|i| selected.iter().map(|f| f[i]).min().unwrap_or(0))
        .collect::<Vec<u8>>();
    let selected = selected
        .iter()
        .map(|frame| {
            let sweep = frame
                .iter()
                .zip(&floor)
                .map(|(value, base)| value.saturating_sub(*base))
                .collect::<Vec<u8>>();
            pack_logo_frame(&sweep)
        })
        .collect::<Vec<_>>();
    let cycle_vblanks =
        ((encoded.len() as u64 * micros_per_frame * 60 + 500_000) / 1_000_000) as u16;
    let mut blob = Vec::with_capacity(8 + 32 + LOGO_ANIM_FRAMES * LOGO_ANIM_W * LOGO_ANIM_H / 2);
    push_u16(&mut blob, LOGO_ANIM_W as u16);
    push_u16(&mut blob, LOGO_ANIM_H as u16);
    push_u16(&mut blob, LOGO_ANIM_FRAMES as u16);
    push_u16(&mut blob, cycle_vblanks);
    for index in 0..16u16 {
        let value = index * 31 / 15;
        let grey = (value << 10) | (value << 5) | value;
        // Index 0 stays 0x0000, which the GPU skips outright. Every other entry
        // carries the semi-transparency bit so the runtime can add the title
        // over the backdrop instead of pasting the film's black across it.
        push_u16(&mut blob, if index == 0 { 0 } else { 0x8000 | grey });
    }
    for frame in selected {
        blob.extend_from_slice(&frame);
    }
    fs::write(output.join("logo_anim.tex"), &blob)?;
    println!(
        "logo_anim.tex: {}x{} 4bpp, {LOGO_ANIM_FRAMES}/{} frames over {cycle_vblanks} vblanks, {} bytes",
        LOGO_ANIM_W,
        LOGO_ANIM_H,
        encoded.len(),
        blob.len()
    );
    Ok(())
}

/// Emit the exact 4bpp Bonnie Studios logo used by the Celeste Classic
/// Collection. The generated Rust pixels keep the Steam-to-disc workflow
/// self-contained: no Python or external branding file is required.
fn build_bonnie_logo(output: &Path) -> Result<()> {
    let mut blob = Vec::with_capacity(4 + 32 + COVER_BONNIE.len() * 2);
    push_u16(&mut blob, BONNIE_W);
    push_u16(&mut blob, BONNIE_H);
    for color in PICO8_CLUT {
        push_u16(&mut blob, color);
    }
    for word in COVER_BONNIE {
        push_u16(&mut blob, word);
    }
    fs::write(output.join("bonnie.tex"), &blob)?;
    println!(
        "bonnie.tex: {BONNIE_W}x{BONNIE_H} 4bpp, {} bytes",
        blob.len()
    );
    Ok(())
}

fn sprite_frames(path: &Path) -> Result<Vec<(GrayImage, Vec<u8>)>> {
    let data = fs::read(path)?;
    if data.len() < 42 || &data[..4] != b"IDSP" {
        return Err(format!("{}: not an IDSP sprite", path.display()).into());
    }
    let count = u16le(&data, 40)? as usize;
    let palette_offset = 42usize;
    let frame_count = i32le(&data, 28)?.max(0) as usize;
    let mut frame_offset = palette_offset + count * 3;
    let mut frames = Vec::with_capacity(frame_count);
    for _ in 0..frame_count {
        let frame_type = i32le(&data, frame_offset)?;
        if frame_type != 0 {
            return Err(
                format!("{}: grouped sprite frames are unsupported", path.display()).into(),
            );
        }
        frame_offset += 4;
        let width = i32le(&data, frame_offset + 8)?;
        let height = i32le(&data, frame_offset + 12)?;
        if width <= 0 || height <= 0 {
            return Err("invalid sprite frame size".into());
        }
        let pixels_at = frame_offset + 16;
        let pixels = data
            .get(pixels_at..pixels_at + width as usize * height as usize)
            .ok_or("sprite pixels exceed file")?;
        let mut gray = GrayImage::new(width as u32, height as u32);
        for (dst, &index) in gray.pixels_mut().zip(pixels) {
            let at = palette_offset + index as usize * 3;
            *dst =
                Luma([((data[at] as u16 + data[at + 1] as u16 + data[at + 2] as u16) / 3) as u8]);
        }
        frames.push((gray, pixels.to_vec()));
        frame_offset = pixels_at + width as usize * height as usize;
    }
    Ok(frames)
}

fn sprite_first_frame(path: &Path) -> Result<(GrayImage, Vec<u8>)> {
    sprite_frames(path)?
        .into_iter()
        .next()
        .ok_or_else(|| format!("{}: sprite has no frames", path.display()).into())
}

fn crop_resize(
    source: &GrayImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    out_w: u32,
    out_h: u32,
) -> GrayImage {
    let crop = imageops::crop_imm(source, x, y, width, height).to_image();
    if width == out_w && height == out_h {
        crop
    } else {
        imageops::resize(&crop, out_w, out_h, FilterType::Lanczos3)
    }
}

fn blit(atlas: &mut [u8], atlas_width: usize, image: &GrayImage, x: usize, y: usize) {
    for row in 0..image.height() as usize {
        for column in 0..image.width() as usize {
            let alpha = image.get_pixel(column as u32, row as u32)[0];
            atlas[(y + row) * atlas_width + x + column] = if alpha < 32 {
                0
            } else {
                (alpha >> 4).clamp(1, 15)
            };
        }
    }
}

fn build_hud(valve: &Path, output: &Path) -> Result<()> {
    // Native 320-res art is the exact source GoldSrc chooses at the PS1's
    // 320x240 output. Never resample this sheet: the old 75%-of-640 path made
    // digits wider, blurred one-pixel strokes, and moved every HUD cluster.
    const WIDTH: usize = 256;
    const HEIGHT: usize = 248;
    const WEAPONS: [&str; 14] = [
        "weapon_crowbar",
        "weapon_9mmhandgun",
        "weapon_357",
        "weapon_9mmar",
        "weapon_shotgun",
        "weapon_crossbow",
        "weapon_rpg",
        "weapon_gauss",
        "weapon_egon",
        "weapon_hornetgun",
        "weapon_handgrenade",
        "weapon_snark",
        "weapon_tripmine",
        "weapon_satchel",
    ];
    let (hud, _) = sprite_first_frame(&valve.join("sprites/320hud2.spr"))?;
    let mut atlas = vec![0u8; WIDTH * HEIGHT];
    for digit in 0..10usize {
        let image = crop_resize(&hud, (digit * 12) as u32, 0, 12, 16, 12, 16);
        blit(&mut atlas, WIDTH, &image, digit * 12, 0);
    }
    for (source, destination) in [
        ((0, 52, 20, 20), (80, 16)),    // suit_full
        ((20, 52, 20, 20), (100, 16)),  // suit_empty
        ((0, 72, 16, 16), (120, 16)),   // health cross
        ((120, 0, 1, 20), (136, 16)),   // divider/bar
        ((48, 52, 20, 20), (138, 16)),  // item_battery
        ((68, 52, 20, 20), (72, 108)),  // item_healthkit
        ((88, 52, 20, 20), (92, 108)),  // item_longjump
        ((16, 72, 18, 16), (112, 108)), // flash_full
        ((34, 72, 18, 16), (130, 108)), // flash_empty
        ((52, 72, 6, 16), (148, 108)),  // flash_beam
    ] {
        let image = crop_resize(
            &hud, source.0, source.1, source.2, source.3, source.2, source.3,
        );
        blit(&mut atlas, WIDTH, &image, destination.0, destination.1);
    }
    // Five source TrainSpeed states (neutral/slow/medium/fast/back). The
    // otherwise-unused u=240..255 tail of the HUD texture page fits compact
    // 16x16 versions without stealing room from world textures.
    let train_frames = sprite_frames(&valve.join("sprites/320_train.spr"))?;
    if train_frames.len() != 5 {
        return Err(format!("expected 5 train HUD frames, found {}", train_frames.len()).into());
    }
    for (index, (frame, _)) in train_frames.iter().enumerate() {
        let image = imageops::resize(frame, 16, 16, FilterType::Lanczos3);
        blit(&mut atlas, WIDTH, &image, 240, 128 + index * 16);
    }
    for bucket in 0..5usize {
        let image = crop_resize(&hud, 108, 16 + bucket as u32 * 12, 12, 12, 12, 12);
        blit(&mut atlas, WIDTH, &image, bucket * 12, 36);
    }

    // Compact projected flashlight cookie. The otherwise-empty 48x24 gap sits
    // below the flashlight sprites and to the right of the crossbow zoom strip.
    // Runtime stretches this normalized ellipse to the screen-space radius of
    // GoldSrc's 80-unit hit light; the 4-bit radial intensity and white additive
    // CLUT give a soft circular pool without another VRAM allocation.
    const SPOT_X: usize = 104;
    const SPOT_Y: usize = 124;
    const SPOT_W: usize = 48;
    const SPOT_H: usize = 24;
    for y in 0..SPOT_H {
        for x in 0..SPOT_W {
            let nx = (2 * x + 1) as f32 / SPOT_W as f32 - 1.0;
            let ny = (2 * y + 1) as f32 / SPOT_H as f32 - 1.0;
            let edge = (1.0 - (nx * nx + ny * ny).sqrt()).max(0.0);
            atlas[(SPOT_Y + y) * WIDTH + SPOT_X + x] = (edge * edge * 15.0) as u8;
        }
    }

    let (selection_sheet, _) = sprite_first_frame(&valve.join("sprites/320hud1.spr"))?;
    let selection = crop_resize(&selection_sheet, 160, 160, 80, 20, 80, 20);
    blit(&mut atlas, WIDTH, &selection, 0, 16);

    let cross_data = fs::read(valve.join("sprites/crosshairs.spr"))?;
    let (_, cross_indices) = sprite_first_frame(&valve.join("sprites/crosshairs.spr"))?;
    let count = u16le(&cross_data, 40)? as usize;
    let frame = 42 + count * 3 + 4;
    let cross_w = i32le(&cross_data, frame + 8)? as u32;
    let cross_h = i32le(&cross_data, frame + 12)? as u32;
    let mut mask = GrayImage::new(cross_w, cross_h);
    for (pixel, &index) in mask.pixels_mut().zip(&cross_indices) {
        *pixel = Luma([if index == 255 { 0 } else { 255 }]);
    }

    let mut sheets = std::collections::HashMap::<String, GrayImage>::new();
    let mut cross_index = 0usize;
    for (weapon_index, weapon) in WEAPONS.iter().enumerate() {
        let text = fs::read_to_string(valve.join("sprites").join(format!("{weapon}.txt")))?;
        let mut active = None;
        let mut ammo = None;
        let mut ammo2 = None;
        let mut crosshair = None;
        let mut zoom = None;
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 7 || fields[1] != "320" {
                continue;
            }
            let rect = (
                fields[2].to_string(),
                fields[3].parse::<u32>()?,
                fields[4].parse::<u32>()?,
                fields[5].parse::<u32>()?,
                fields[6].parse::<u32>()?,
            );
            match fields[0] {
                "weapon_s" => active = Some(rect),
                "ammo" => ammo = Some(rect),
                "ammo2" => ammo2 = Some(rect),
                "crosshair" => crosshair = Some(rect),
                "zoom" => zoom = Some(rect),
                _ => {}
            }
        }
        let load_rect = |rect: &(String, u32, u32, u32, u32),
                         sheets: &mut std::collections::HashMap<String, GrayImage>|
         -> Result<GrayImage> {
            if !sheets.contains_key(&rect.0) {
                let (sheet, _) =
                    sprite_first_frame(&valve.join("sprites").join(format!("{}.spr", rect.0)))?;
                sheets.insert(rect.0.clone(), sheet);
            }
            Ok(crop_resize(
                &sheets[&rect.0],
                rect.1,
                rect.2,
                rect.3,
                rect.4,
                rect.3,
                rect.4,
            ))
        };

        if let Some(rect) = active.as_ref() {
            let icon = load_rect(rect, &mut sheets)?;
            let (x, y) = if weapon_index < 10 {
                ((weapon_index % 2) * 80, 148 + (weapon_index / 2) * 20)
            } else {
                (160, 128 + (weapon_index - 10) * 20)
            };
            blit(&mut atlas, WIDTH, &icon, x, y);
        } else {
            eprintln!("warn: no 320 weapon_s rect for {weapon}");
        }

        if let Some(rect) = ammo.as_ref() {
            let icon = load_rect(rect, &mut sheets)?;
            blit(
                &mut atlas,
                WIDTH,
                &icon,
                (weapon_index % 7) * 18,
                48 + (weapon_index / 7) * 18,
            );
        }
        if weapon_index == 3 {
            if let Some(rect) = ammo2.as_ref() {
                let icon = load_rect(rect, &mut sheets)?;
                blit(&mut atlas, WIDTH, &icon, 126, 48);
            }
        }

        if let Some(rect) = crosshair.as_ref() {
            // Crosshair transparency is palette index 255, already converted to
            // an exact binary mask above. All native weapon rectangles are 24².
            let icon = crop_resize(&mask, rect.1, rect.2, rect.3, rect.4, rect.3, rect.4);
            blit(
                &mut atlas,
                WIDTH,
                &icon,
                (cross_index % 6) * 24,
                84 + (cross_index / 6) * 24,
            );
            cross_index += 1;
        }
        if weapon_index == 5 {
            if let Some(rect) = zoom.as_ref() {
                let icon = crop_resize(&mask, rect.1, rect.2, rect.3, rect.4, rect.3, rect.4);
                blit(&mut atlas, WIDTH, &icon, 0, 132);
            }
        }
    }
    if cross_index != 9 {
        return Err(format!("expected 9 native weapon crosshairs, found {cross_index}").into());
    }

    // Sparse HUD2 storage: the gameplay font owns u=160..255/v=0..127 and
    // four explosive weapon icons occupy u=160..239/v=128..207, and the train
    // states use the final 16-pixel tail. Packing
    // the left strip plus that right island avoids 8.7 KB of transparent data;
    // critically, the HUD stays below the largest room chunk and cannot enlarge
    // the PS1's shared decompression buffer.
    const LEFT_W: usize = 160;
    const RIGHT_X: usize = 160;
    const RIGHT_Y: usize = 128;
    const RIGHT_W: usize = 96;
    const RIGHT_H: usize = 80;
    let mut blob = Vec::with_capacity(4 + 64 + LEFT_W * HEIGHT / 2 + RIGHT_W * RIGHT_H / 2);
    blob.extend_from_slice(b"HUD2");
    // Index zero is transparent. Mark every visible entry semi-transparent so
    // the runtime's additive material matches GoldSrc SPR_DrawAdditive.
    push_u16(&mut blob, 0);
    for index in 1..16u16 {
        let r = (255 * index / 15) as u8;
        let g = (160 * index / 15) as u8;
        push_u16(&mut blob, bgr555(r, g, 0) | 0x8000);
    }
    push_u16(&mut blob, 0);
    for index in 1..16u16 {
        let value = (255 * index / 15) as u8;
        push_u16(&mut blob, bgr555(value, value, value) | 0x8000);
    }
    for y in 0..HEIGHT {
        for x in (0..LEFT_W).step_by(2) {
            blob.push(atlas[y * WIDTH + x] | (atlas[y * WIDTH + x + 1] << 4));
        }
    }
    for y in RIGHT_Y..RIGHT_Y + RIGHT_H {
        for x in (RIGHT_X..RIGHT_X + RIGHT_W).step_by(2) {
            blob.push(atlas[y * WIDTH + x] | (atlas[y * WIDTH + x + 1] << 4));
        }
    }
    fs::write(output.join("hud.tex"), &blob)?;
    println!(
        "hud.tex: {WIDTH}x{HEIGHT} native 320-res sparse 4bpp, {} bytes",
        blob.len()
    );
    Ok(())
}

fn pack(output: &Path) -> Result<()> {
    // Section order must match game/src/menu.rs split_menu().
    const SECTIONS: [&str; 4] = ["bg.tex", "logo.tex", "bonnie.tex", "logo_anim.tex"];
    let parts = SECTIONS
        .iter()
        .map(|name| fs::read(output.join(name)))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut blob =
        Vec::with_capacity(4 * SECTIONS.len() + parts.iter().map(Vec::len).sum::<usize>());
    for part in &parts {
        blob.extend_from_slice(&(part.len() as u32).to_le_bytes());
    }
    for part in &parts {
        blob.extend_from_slice(part);
    }
    fs::write(output.join("menu.pak"), &blob)?;
    println!("menu.pak: {} bytes", blob.len());
    Ok(())
}

pub fn build(valve: &Path, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    // Older cooks extracted GoldSrc's angular QFONT here. The runtime now uses
    // PSoXide's linked font, so remove the obsolete generated blob as well.
    let stale_font = output.join("hlfont.bin");
    if stale_font.exists() {
        fs::remove_file(stale_font)?;
    }
    build_logo(valve, output)?;
    build_logo_animation(valve, output)?;
    build_bonnie_logo(output)?;
    build_background(valve, output)?;
    build_hud(valve, output)?;
    pack(output)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_rle8_frame, pack_logo_frame, LOGO_ANIM_H, LOGO_ANIM_MATTE_FLOOR, LOGO_ANIM_W,
    };

    fn unpack_4bpp(packed: &[u8]) -> Vec<u8> {
        packed
            .iter()
            .flat_map(|byte| [byte & 0x0f, byte >> 4])
            .collect()
    }

    #[test]
    fn rle8_decodes_bottom_up_runs_literals_and_deltas() {
        let encoded = [
            4, 1, 0, 0, // bottom row: encoded run, end of line
            0, 2, 1, 0, // middle row: skip one pixel
            2, 2, 0, 0, // two-pixel run, end of line
            0, 4, 4, 5, 6, 7, // top row: four literal pixels
            0, 1, // end of bitmap
        ];
        let mut frame = [0u8; 12];

        decode_rle8_frame(&encoded, 4, 3, &mut frame).unwrap();

        assert_eq!(frame, [4, 5, 6, 7, 0, 2, 2, 0, 1, 1, 1, 1]);
    }

    #[test]
    fn rle8_rejects_runs_past_the_right_edge() {
        let mut frame = [0u8; 4];
        let error = decode_rle8_frame(&[5, 1, 0, 1], 4, 1, &mut frame).unwrap_err();
        assert!(error.to_string().contains("exceeds frame"));
    }

    #[test]
    fn logo_animation_matte_floor_stays_transparent() {
        let sweep = vec![LOGO_ANIM_MATTE_FLOOR; LOGO_ANIM_W * LOGO_ANIM_H];
        assert!(unpack_4bpp(&pack_logo_frame(&sweep))
            .into_iter()
            .all(|index| index == 0));
    }

    #[test]
    fn logo_animation_keeps_authored_highlights() {
        let sweep = vec![u8::MAX; LOGO_ANIM_W * LOGO_ANIM_H];
        assert!(unpack_4bpp(&pack_logo_frame(&sweep))
            .into_iter()
            .all(|index| index >= 14));
    }
}

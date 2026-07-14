use crate::Result;
use fontdue::Font;
use image::imageops::{self, FilterType};
use image::{GrayImage, Luma};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const SIZE: f32 = 16.0;
const GLYPH_W_CAP: usize = 16;
const THRESHOLD: u8 = 100;
const LOGO_W: u32 = 224;

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

fn find_font(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env::var_os("MENU_FONT") {
        return Ok(path.into());
    }
    for candidate in [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "C:/Windows/Fonts/arial.ttf",
    ] {
        let path = Path::new(candidate);
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
    }
    Err("no Arial-compatible menu font found; set MENU_FONT=/path/font.ttf".into())
}

fn build_font(output: &Path, font_path: &Path) -> Result<()> {
    let font = Font::from_bytes(fs::read(font_path)?, fontdue::FontSettings::default())?;
    let first = 32u16;
    let count = 95u16;
    let line = font
        .horizontal_line_metrics(SIZE)
        .ok_or("font has no horizontal metrics")?;
    let baseline = line.ascent.ceil().max(1.0) as i32;
    let glyph_h = (line.ascent.ceil() + (-line.descent).ceil()).clamp(1.0, 255.0) as usize;
    let mut advances = Vec::with_capacity(count as usize);
    let mut glyphs = Vec::with_capacity(count as usize);
    let mut glyph_w = 1usize;
    for code in first..first + count {
        let (metrics, bitmap) = font.rasterize(char::from_u32(code as u32).unwrap(), SIZE);
        advances.push(metrics.advance_width.round().clamp(1.0, 255.0) as u8);
        glyph_w = glyph_w.max((metrics.xmin + metrics.width as i32).max(0) as usize);
        glyphs.push((metrics, bitmap));
    }
    glyph_w = glyph_w.min(GLYPH_W_CAP);
    let row_bytes = glyph_w.div_ceil(8);
    let mut bitmap = Vec::with_capacity(count as usize * glyph_h * row_bytes);
    for (metrics, pixels) in glyphs {
        let top = baseline - metrics.ymin - metrics.height as i32;
        let left = metrics.xmin.max(0) as usize;
        for row in 0..glyph_h {
            let mut packed = vec![0u8; row_bytes];
            let source_row = row as i32 - top;
            if source_row >= 0 && source_row < metrics.height as i32 {
                for source_column in 0..metrics.width.min(glyph_w.saturating_sub(left)) {
                    let column = left + source_column;
                    if pixels[source_row as usize * metrics.width + source_column] >= THRESHOLD {
                        packed[column >> 3] |= 0x80 >> (column & 7);
                    }
                }
            }
            bitmap.extend_from_slice(&packed);
        }
    }
    let mut blob = Vec::new();
    blob.push(glyph_w as u8);
    blob.push(glyph_h as u8);
    push_u16(&mut blob, count);
    push_u16(&mut blob, first);
    push_u16(&mut blob, 0);
    blob.extend_from_slice(&advances);
    blob.extend_from_slice(&bitmap);
    fs::write(output.join("hlfont.bin"), &blob)?;
    println!(
        "hlfont.bin: {} {}x{}, {} glyphs, {} bytes",
        font_path.display(),
        glyph_w,
        glyph_h,
        count,
        blob.len()
    );
    Ok(())
}

fn build_background(valve: &Path, output: &Path) -> Result<()> {
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
    let gray = imageops::resize(&source, 256, 240, FilterType::Lanczos3);
    let mut blob = Vec::with_capacity(4 + 32 + 256 * 240 / 2);
    push_u16(&mut blob, 256);
    push_u16(&mut blob, 240);
    for index in 0..16u16 {
        let value = ((index as f32 / 15.0) * 10.0).round() as u16;
        push_u16(&mut blob, (value << 10) | (value << 5) | value);
    }
    for row in 0..240 {
        for column in (0..256).step_by(2) {
            let low = gray.get_pixel(column, row)[0] >> 4;
            let high = gray.get_pixel(column + 1, row)[0] >> 4;
            blob.push((high << 4) | low);
        }
    }
    fs::write(output.join("bg.tex"), &blob)?;
    println!("bg.tex: 256x240 4bpp, {} bytes", blob.len());
    Ok(())
}

fn build_logo(valve: &Path, output: &Path) -> Result<()> {
    let source = image::open(valve.join("resource/logo.tga"))?.to_rgba8();
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
    fs::write(output.join("logo.tex"), &blob)?;
    println!("logo.tex: {}x{} 4bpp, {} bytes", LOGO_W, height, blob.len());
    Ok(())
}

fn sprite_first_frame(path: &Path) -> Result<(GrayImage, Vec<u8>)> {
    let data = fs::read(path)?;
    if data.len() < 42 || &data[..4] != b"IDSP" {
        return Err(format!("{}: not an IDSP sprite", path.display()).into());
    }
    let count = u16le(&data, 40)? as usize;
    let palette_offset = 42usize;
    let frame_offset = palette_offset + count * 3 + 4;
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
        *dst = Luma([((data[at] as u16 + data[at + 1] as u16 + data[at + 2] as u16) / 3) as u8]);
    }
    Ok((gray, pixels.to_vec()))
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
    const WIDTH: usize = 160;
    const HEIGHT: usize = 188;
    let (hud, _) = sprite_first_frame(&valve.join("sprites/640hud7.spr"))?;
    let mut atlas = vec![0u8; WIDTH * HEIGHT];
    for digit in 0..10usize {
        let image = crop_resize(&hud, (digit * 24) as u32, 0, 20, 24, 15, 18);
        blit(&mut atlas, WIDTH, &image, digit * 15, 0);
    }
    for (source, size, destination) in [
        ((0, 24, 40, 40), (30, 30), (0, 18)),
        ((40, 24, 40, 40), (30, 30), (30, 18)),
        ((80, 24, 32, 32), (24, 24), (60, 18)),
        ((0, 72, 24, 24), (18, 18), (84, 18)),
        ((240, 0, 2, 40), (2, 30), (132, 18)),
    ] {
        let image = crop_resize(&hud, source.0, source.1, source.2, source.3, size.0, size.1);
        blit(&mut atlas, WIDTH, &image, destination.0, destination.1);
    }
    let (battery_sheet, _) = sprite_first_frame(&valve.join("sprites/640hud2.spr"))?;
    let battery = crop_resize(&battery_sheet, 176, 0, 44, 44, 24, 24);
    blit(&mut atlas, WIDTH, &battery, 136, 18);

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
    let crosshair = crop_resize(&mask, 24, 0, 24, 24, 18, 18);
    blit(&mut atlas, WIDTH, &crosshair, 108, 18);

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
    let mut sheets = std::collections::HashMap::<String, GrayImage>::new();
    for (weapon_index, weapon) in WEAPONS.iter().enumerate() {
        let text = fs::read_to_string(valve.join("sprites").join(format!("{weapon}.txt")))?;
        let mut rect = None;
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() >= 7 && fields[0] == "weapon_s" && fields[1] == "320" {
                rect = Some((
                    fields[2].to_string(),
                    fields[3].parse::<u32>()?,
                    fields[4].parse::<u32>()?,
                    fields[5].parse::<u32>()?,
                    fields[6].parse::<u32>()?,
                ));
                break;
            }
        }
        let Some((sheet_name, x, y, width, height)) = rect else {
            eprintln!("warn: no 320 weapon_s rect for {weapon}");
            continue;
        };
        if !sheets.contains_key(&sheet_name) {
            let (sheet, _) =
                sprite_first_frame(&valve.join("sprites").join(format!("{sheet_name}.spr")))?;
            sheets.insert(sheet_name.clone(), sheet);
        }
        let icon = crop_resize(&sheets[&sheet_name], x, y, width, height, 80, 20);
        blit(
            &mut atlas,
            WIDTH,
            &icon,
            (weapon_index % 2) * 80,
            48 + (weapon_index / 2) * 20,
        );
    }
    let mut blob = Vec::with_capacity(4 + 32 + WIDTH * HEIGHT / 2);
    push_u16(&mut blob, WIDTH as u16);
    push_u16(&mut blob, HEIGHT as u16);
    push_u16(&mut blob, 0);
    for index in 1..16u16 {
        let r = (255 * index / 15) as u8;
        let g = (170 * index / 15) as u8;
        push_u16(&mut blob, bgr555(r, g, 0));
    }
    for pair in atlas.chunks_exact(2) {
        blob.push(pair[0] | (pair[1] << 4));
    }
    fs::write(output.join("hud.tex"), &blob)?;
    println!("hud.tex: {WIDTH}x{HEIGHT} 4bpp, {} bytes", blob.len());
    Ok(())
}

fn pack(output: &Path) -> Result<()> {
    let background = fs::read(output.join("bg.tex"))?;
    let logo = fs::read(output.join("logo.tex"))?;
    let mut blob = Vec::with_capacity(8 + background.len() + logo.len());
    blob.extend_from_slice(&(background.len() as u32).to_le_bytes());
    blob.extend_from_slice(&(logo.len() as u32).to_le_bytes());
    blob.extend_from_slice(&background);
    blob.extend_from_slice(&logo);
    fs::write(output.join("menu.pak"), &blob)?;
    println!("menu.pak: {} bytes", blob.len());
    Ok(())
}

pub fn build(valve: &Path, output: &Path, font: Option<&Path>) -> Result<()> {
    fs::create_dir_all(output)?;
    build_font(output, &find_font(font)?)?;
    build_logo(valve, output)?;
    build_background(valve, output)?;
    build_hud(valve, output)?;
    pack(output)
}

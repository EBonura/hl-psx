use crate::generators::bsp_entities;
use crate::Result;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const SECTOR: usize = 2352;
const HL_CD_PLAYLIST: [&str; 27] = [
    "Half-Life01.mp3",
    "Prospero01.mp3",
    "Half-Life12.mp3",
    "Half-Life07.mp3",
    "Half-Life10.mp3",
    "Suspense01.mp3",
    "Suspense03.mp3",
    "Half-Life09.mp3",
    "Half-Life02.mp3",
    "Half-Life13.mp3",
    "Half-Life04.mp3",
    "Half-Life15.mp3",
    "Half-Life14.mp3",
    "Half-Life16.mp3",
    "Suspense02.mp3",
    "Half-Life03.mp3",
    "Half-Life08.mp3",
    "Prospero02.mp3",
    "Half-Life05.mp3",
    "Prospero04.mp3",
    "Half-Life11.mp3",
    "Half-Life06.mp3",
    "Prospero03.mp3",
    "Half-Life17.mp3",
    "Prospero05.mp3",
    "Suspense05.mp3",
    "Suspense07.mp3",
];

fn decode_mp3(path: &Path) -> Result<(u32, usize, Vec<i16>)> {
    let source = File::open(path)?;
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let format_options = FormatOptions {
        // Match normal media decoders (including the old afconvert path): do
        // not emit the MP3 encoder delay or end padding as audible CDDA data.
        enable_gapless: true,
        ..FormatOptions::default()
    };
    let probed = symphonia::default::get_probe().format(
        &hint,
        stream,
        &format_options,
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| format!("{} has no default audio track", path.display()))?;
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut rate = track.codec_params.sample_rate.unwrap_or(44_100);
    let mut channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(2);
    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(error) => return Err(error.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        rate = decoded.spec().rate;
        channels = decoded.spec().channels.count();
        let mut buffer = SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec());
        buffer.copy_interleaved_ref(decoded);
        samples.extend_from_slice(buffer.samples());
    }
    if samples.is_empty() || channels == 0 {
        return Err(format!("{} decoded no PCM", path.display()).into());
    }
    Ok((rate, channels, samples))
}

fn stereo_44100(rate: u32, channels: usize, input: &[i16]) -> Vec<i16> {
    let frames = input.len() / channels;
    let out_frames = ((frames as u64 * 44_100) / rate.max(1) as u64).max(1) as usize;
    let mut output = Vec::with_capacity(out_frames * 2);
    for index in 0..out_frames {
        let source =
            ((index as u64 * rate as u64) / 44_100).min(frames.saturating_sub(1) as u64) as usize;
        let at = source * channels;
        let left = input[at];
        let right = if channels > 1 { input[at + 1] } else { left };
        output.push(left);
        output.push(right);
    }
    output
}

pub fn build_music(valve: &Path, output: &Path) -> Result<()> {
    let media = valve.join("media");
    if !media.is_dir() {
        eprintln!("no media dir at {}; skipping music", media.display());
        return Ok(());
    }
    fs::create_dir_all(output)?;
    let missing: Vec<&str> = HL_CD_PLAYLIST
        .iter()
        .copied()
        .filter(|name| !media.join(name).is_file())
        .collect();
    if !missing.is_empty() {
        return Err(format!("missing Half-Life CD tracks: {}", missing.join(", ")).into());
    }
    let mut listing = Vec::new();
    let mut total_sectors = 0usize;
    for (index, name) in HL_CD_PLAYLIST.iter().enumerate() {
        let (rate, channels, decoded) = decode_mp3(&media.join(name))?;
        let pcm = stereo_44100(rate, channels, &decoded);
        let out = output.join(format!("track_{:02}.cdda", index + 1));
        let mut bytes = Vec::with_capacity(pcm.len() * 2 + SECTOR);
        for sample in pcm {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes.resize(bytes.len().div_ceil(SECTOR) * SECTOR, 0);
        fs::write(&out, &bytes)?;
        let sectors = bytes.len() / SECTOR;
        total_sectors += sectors;
        listing.push(fs::canonicalize(&out)?.to_string_lossy().into_owned());
        println!(
            "  track {:2} (disc) = {name} ({sectors} sectors)",
            index + 2
        );
    }
    fs::write(output.join("tracks.txt"), listing.join("\n") + "\n")?;
    println!(
        "music -> {} ({} tracks, {} MB)",
        output.display(),
        HL_CD_PLAYLIST.len(),
        total_sectors * SECTOR / (1 << 20)
    );
    Ok(())
}

#[derive(Clone)]
struct Pcm {
    rate: u32,
    samples: Vec<i16>,
}

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

fn read_wav(path: &Path) -> Result<Pcm> {
    let data = fs::read(path)?;
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(format!("{}: unsupported WAV", path.display()).into());
    }
    let mut cursor = 12usize;
    let mut format = None;
    let mut pcm_data = None;
    while cursor + 8 <= data.len() {
        let id = &data[cursor..cursor + 4];
        let len = u32le(&data, cursor + 4)? as usize;
        cursor += 8;
        let end = cursor.saturating_add(len).min(data.len());
        if id == b"fmt " && end >= cursor + 16 {
            format = Some((
                u16le(&data, cursor)?,
                u16le(&data, cursor + 2)? as usize,
                u32le(&data, cursor + 4)?,
                u16le(&data, cursor + 14)?,
            ));
        } else if id == b"data" {
            pcm_data = Some(&data[cursor..end]);
        }
        cursor = end + (len & 1);
    }
    let (encoding, channels, rate, bits) = format.ok_or("WAV fmt chunk missing")?;
    let raw = pcm_data.ok_or("WAV data chunk missing")?;
    if encoding != 1 || !(channels == 1 || channels == 2) || !(bits == 8 || bits == 16) {
        return Err(format!(
            "{}: only PCM 8/16-bit mono/stereo WAV is supported",
            path.display()
        )
        .into());
    }
    let mut interleaved = Vec::new();
    if bits == 8 {
        interleaved.extend(raw.iter().map(|&v| ((v as i16) - 128) << 8));
    } else {
        for pair in raw.chunks_exact(2) {
            interleaved.push(i16::from_le_bytes([pair[0], pair[1]]));
        }
    }
    let mut mono = Vec::with_capacity(interleaved.len() / channels);
    for frame in interleaved.chunks_exact(channels) {
        mono.push(if channels == 2 {
            ((frame[0] as i32 + frame[1] as i32) / 2) as i16
        } else {
            frame[0]
        });
    }
    Ok(Pcm {
        rate,
        samples: mono,
    })
}

fn resample(input: &Pcm, rate: u32) -> Pcm {
    if input.rate == rate {
        return input.clone();
    }
    let count =
        ((input.samples.len() as u64 * rate as u64) / input.rate.max(1) as u64).max(1) as usize;
    let mut samples = Vec::with_capacity(count);
    for index in 0..count {
        let source = ((index as u64 * input.rate as u64) / rate as u64)
            .min(input.samples.len().saturating_sub(1) as u64) as usize;
        samples.push(input.samples[source]);
    }
    Pcm { rate, samples }
}

fn write_wav(path: &Path, pcm: &Pcm) -> Result<()> {
    let data_len = pcm.samples.len() * 2;
    let mut output = Vec::with_capacity(44 + data_len);
    output.extend_from_slice(b"RIFF");
    output.extend_from_slice(&(36u32 + data_len as u32).to_le_bytes());
    output.extend_from_slice(b"WAVEfmt ");
    output.extend_from_slice(&16u32.to_le_bytes());
    output.extend_from_slice(&1u16.to_le_bytes());
    output.extend_from_slice(&1u16.to_le_bytes());
    output.extend_from_slice(&pcm.rate.to_le_bytes());
    output.extend_from_slice(&(pcm.rate * 2).to_le_bytes());
    output.extend_from_slice(&2u16.to_le_bytes());
    output.extend_from_slice(&16u16.to_le_bytes());
    output.extend_from_slice(b"data");
    output.extend_from_slice(&(data_len as u32).to_le_bytes());
    for sample in &pcm.samples {
        output.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, output)?;
    Ok(())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(prefix: &str) -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn zip_wavs(path: &Path, directory: &Path, ids: &[String]) -> Result<()> {
    let file = File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for id in ids {
        zip.start_file(format!("{id}.wav"), options)?;
        zip.write_all(&fs::read(directory.join(format!("{id}.wav")))?)?;
    }
    zip.finish()?;
    Ok(())
}

fn run_psxed(
    psxed: &Path,
    directory: &Path,
    ids: &[String],
    rate: u32,
    label: &str,
) -> Result<Vec<Vec<u8>>> {
    let zip_path = directory.join(format!("{label}.zip"));
    zip_wavs(&zip_path, directory, ids)?;
    let archive = fs::read(&zip_path)?;
    let hash = format!("{:x}", Sha256::digest(&archive));
    let manifest = json!({
        "source": {
            "name": "player Half-Life install",
            "url": "",
            "license": "user-supplied",
            "archive_sha256": hash
        },
        "target_sample_rate_hz": rate,
        "normalize_peak": 0.9,
        "sounds": ids.iter().map(|id| json!({"id": id, "path": format!("{id}.wav")})).collect::<Vec<_>>()
    });
    let manifest_path = directory.join(format!("{label}.json"));
    fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
    let out = directory.join("out");
    let result = Command::new(psxed)
        .arg("audio-pack")
        .arg(&manifest_path)
        .arg("--zip")
        .arg(&zip_path)
        .arg("--out-dir")
        .arg(&out)
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "psxed failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        )
        .into());
    }
    ids.iter()
        .map(|id| fs::read(out.join("psau").join(format!("{id}.psau"))).map_err(Into::into))
        .collect()
}

fn hsfx(blobs: &[Vec<u8>]) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(b"HSFX");
    output.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    let mut offset = 8 + blobs.len() * 8;
    for blob in blobs {
        output.extend_from_slice(&(offset as u32).to_le_bytes());
        output.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        offset += blob.len();
    }
    for blob in blobs {
        output.extend_from_slice(blob);
    }
    output
}

const SOUNDS: [(&str, &str); 47] = [
    ("glock", "weapons/pl_gun3.wav"),
    ("mp5", "weapons/hks1.wav"),
    ("shotgun", "weapons/sbarrel1.wav"),
    ("python", "weapons/357_shot1.wav"),
    ("xbow", "weapons/xbow_fire1.wav"),
    ("gauss", "weapons/gauss2.wav"),
    ("rpg", "weapons/rocketfire1.wav"),
    ("cbar_miss", "weapons/cbar_miss1.wav"),
    ("cbar_hit", "weapons/cbar_hit1.wav"),
    ("explode", "weapons/explode3.wav"),
    ("ric", "weapons/ric1.wav"),
    ("electro", "weapons/electro4.wav"),
    ("pain", "player/pl_pain6.wav"),
    ("bodydrop", "common/bodydrop3.wav"),
    ("door_move", "doors/doormove1.wav"),
    ("door_stop", "doors/doorstop1.wav"),
    ("button", "buttons/button3.wav"),
    ("pickup", "items/gunpickup2.wav"),
    ("suit", "items/suitchargeok1.wav"),
    ("hc_attack", "headcrab/hc_attack1.wav"),
    ("zo_attack", "zombie/zo_attack1.wav"),
    ("he_blast", "houndeye/he_blast1.wav"),
    ("glass_break", "debris/bustglass1.wav"),
    ("wood_break", "debris/bustcrate1.wav"),
    ("medshot", "items/medshot4.wav"),
    ("step1", "player/pl_step1.wav"),
    ("step2", "player/pl_step2.wav"),
    ("reload", "weapons/reload1.wav"),
    ("dry", "common/wpn_denyselect.wav"),
    ("zo_pain", "zombie/zo_pain2.wav"),
    ("hc_pain", "headcrab/hc_pain1.wav"),
    ("hc_die", "headcrab/hc_die1.wav"),
    ("gr_pain", "hgrunt/gr_pain3.wav"),
    ("gr_die", "hgrunt/gr_die1.wav"),
    ("ba_pain", "barney/ba_pain1.wav"),
    ("ba_die", "barney/ba_die1.wav"),
    ("he_pain", "houndeye/he_pain3.wav"),
    ("he_die", "houndeye/he_die1.wav"),
    ("slv_pain", "aslave/slv_pain2.wav"),
    ("slv_die", "aslave/slv_die1.wav"),
    ("bc_pain", "bullchicken/bc_pain1.wav"),
    ("bc_die", "bullchicken/bc_die1.wav"),
    ("hev_bell", "fvox/bell.wav"),
    ("geiger", "player/geiger1.wav"),
    ("hev_activate", "fvox/powerarmor_on.wav"),
    ("hev_health_crit", "fvox/health_critical.wav"),
    ("hev_near_death", "fvox/near_death.wav"),
];

pub fn build_sfx(sound: &Path, output: &Path, psxed: &Path) -> Result<()> {
    let scratch = Scratch::new("hlsfx")?;
    let mut rates = HashMap::new();
    for (id, relative) in SOUNDS {
        let source = read_wav(&sound.join(relative))?;
        let rate = if source.rate >= 22_050 { 11_025 } else { 8_000 };
        write_wav(
            &scratch.0.join(format!("{id}.wav")),
            &resample(&source, rate),
        )?;
        rates.insert(id, rate);
    }
    let mut encoded = HashMap::<String, Vec<u8>>::new();
    for rate in [8_000, 11_025] {
        let ids: Vec<String> = SOUNDS
            .iter()
            .filter(|(id, _)| rates.get(id) == Some(&rate))
            .map(|(id, _)| (*id).to_string())
            .collect();
        for (id, blob) in ids.iter().zip(run_psxed(
            psxed,
            &scratch.0,
            &ids,
            rate,
            &format!("sfx{rate}"),
        )?) {
            encoded.insert(id.clone(), blob);
        }
    }
    let blobs: Vec<Vec<u8>> = SOUNDS
        .iter()
        .map(|(id, _)| encoded.remove(*id).unwrap())
        .collect();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let pack = hsfx(&blobs);
    fs::write(output, &pack)?;
    println!(
        "sfx pack: {} samples, {} bytes -> {}",
        blobs.len(),
        pack.len(),
        output.display()
    );
    Ok(())
}

fn strip_sentence_params(token: &str) -> String {
    let mut depth = 0u32;
    token
        .chars()
        .filter(|&ch| match ch {
            '(' => {
                depth += 1;
                false
            }
            ')' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect::<String>()
        .trim_matches(['(', ')'])
        .to_string()
}

fn load_sentences(valve: &Path) -> Result<HashMap<String, Vec<String>>> {
    let path = valve.join("sound/sentences.txt");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let text: String = fs::read(path)?.iter().map(|&b| b as char).collect();
    let mut result = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let mut current_dir = String::new();
        let mut wavs = Vec::new();
        for raw_token in parts {
            let token = strip_sentence_params(raw_token);
            if token.is_empty() || token == "." || token == "," {
                continue;
            }
            if let Some((directory, _)) = token.rsplit_once('/') {
                current_dir = format!("{directory}/");
                wavs.push(format!("{token}.wav"));
            } else {
                wavs.push(format!("{current_dir}{token}.wav"));
            }
        }
        if !wavs.is_empty() {
            result.insert(name.to_ascii_uppercase(), wavs);
        }
    }
    Ok(result)
}

fn voice_keys(
    valve: &Path,
    map: &Path,
    sentences: &HashMap<String, Vec<String>>,
) -> Result<Vec<(String, Vec<String>)>> {
    const VOICE_DIRS: [&str; 7] = [
        "barney/",
        "scientist/",
        "gman/",
        "hgrunt/",
        "tride/",
        "vox/",
        "fvox/",
    ];
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for entity in bsp_entities(map)? {
        match entity.get("classname").map(String::as_str) {
            Some("scripted_sentence") => {
                let key = entity
                    .get("sentence")
                    .map(|s| s.trim_start_matches('!').to_ascii_uppercase())
                    .unwrap_or_default();
                if let Some(wavs) = sentences.get(&key) {
                    if seen.insert(key.clone()) {
                        result.push((key, wavs.clone()));
                    }
                }
            }
            Some("ambient_generic") => {
                let message = entity.get("message").cloned().unwrap_or_default();
                let lower = message.to_ascii_lowercase();
                if lower.ends_with(".wav")
                    && VOICE_DIRS.iter().any(|prefix| message.starts_with(prefix))
                    && seen.insert(lower.clone())
                {
                    result.push((lower, vec![message]));
                }
            }
            _ => {}
        }
    }
    let _ = valve;
    Ok(result)
}

fn concat_wavs(valve: &Path, wavs: &[String], rate: u32) -> Result<Option<Pcm>> {
    let mut samples = Vec::new();
    for relative in wavs {
        let path = valve.join("sound").join(relative);
        if !path.exists() {
            return Ok(None);
        }
        samples.extend(resample(&read_wav(&path)?, rate).samples);
    }
    Ok(Some(Pcm { rate, samples }))
}

pub fn build_voices(valve: &Path, map_list: &str, output: &Path, psxed: &Path) -> Result<()> {
    const RATES: [u32; 5] = [11_025, 8_000, 6_000, 5_000, 4_000];
    fs::create_dir_all(output)?;
    for entry in fs::read_dir(output)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("chunk_") || name == "manifest.txt" {
            fs::remove_file(path)?;
        }
    }
    let core = output
        .parent()
        .unwrap_or(output)
        .join("sfx/chunk_3000.psxa");
    let core_bytes = fs::metadata(core)
        .map(|v| v.len() as usize)
        .unwrap_or(414 * 1024);
    let budget = 512 * 1024 - 0x1010 - core_bytes - 4096;
    let sentences = load_sentences(valve)?;
    let mut manifest = Vec::new();
    for (map_index, map_name) in map_list.split_whitespace().enumerate() {
        let bsp = valve.join("maps").join(format!("{map_name}.bsp"));
        if !bsp.exists() {
            continue;
        }
        let keys = voice_keys(valve, &bsp, &sentences)?;
        if keys.is_empty() {
            continue;
        }
        for (rate_index, rate) in RATES.iter().copied().enumerate() {
            let scratch = Scratch::new("hlvox")?;
            let mut ids = Vec::new();
            let mut used = Vec::new();
            for (key, wavs) in &keys {
                let id = format!("v{:02}", ids.len());
                if let Some(pcm) = concat_wavs(valve, wavs, rate)? {
                    write_wav(&scratch.0.join(format!("{id}.wav")), &pcm)?;
                    used.push((ids.len(), key.clone()));
                    ids.push(id);
                }
            }
            if ids.is_empty() {
                break;
            }
            let blobs = run_psxed(psxed, &scratch.0, &ids, rate, "voices")?;
            let pack = hsfx(&blobs);
            if pack.len() <= budget || rate_index + 1 == RATES.len() {
                let chunk = 3100 + map_index;
                fs::write(output.join(format!("chunk_{chunk}.psxa")), &pack)?;
                manifest.extend(
                    used.into_iter()
                        .map(|(id, key)| format!("{map_index}|{id}|{key}")),
                );
                let flag = if pack.len() > budget {
                    " OVER-BUDGET"
                } else {
                    ""
                };
                println!(
                    "  {map_name} (idx {map_index}, chunk {chunk}): {} lines, {} KB @{rate}Hz{flag}",
                    ids.len(),
                    pack.len() / 1024
                );
                break;
            }
        }
    }
    fs::write(output.join("manifest.txt"), manifest.join("\n") + "\n")?;
    println!(
        "voices -> {} ({} lines across maps)",
        output.display(),
        manifest.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_resampler_preserves_duration() {
        let input = Pcm {
            rate: 22_050,
            samples: (0..2205).collect(),
        };
        assert_eq!(resample(&input, 11_025).samples.len(), 1102);
    }

    #[test]
    fn sentence_parameters_are_not_part_of_wav_name() {
        assert_eq!(strip_sentence_params("barney/hello(p120)"), "barney/hello");
    }
}

use crate::generators::bsp_entities;
use crate::Result;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
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
    has_loop_metadata: bool,
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

/// GoldSrc leaves physical sample wrapping to the WAV. An ambient entity can
/// retain looping/toggle state without its waveform containing a loop, so the
/// entity spawnflags alone must never manufacture a hardware repeat.
fn wav_chunk_declares_loop(id: &[u8], payload: &[u8]) -> bool {
    if id == b"cue " {
        return payload
            .get(..4)
            .and_then(|count| count.try_into().ok())
            .map(u32::from_le_bytes)
            .unwrap_or(0)
            != 0;
    }
    if id == b"smpl" {
        return payload
            .get(28..32)
            .and_then(|count| count.try_into().ok())
            .map(u32::from_le_bytes)
            .unwrap_or(0)
            != 0;
    }
    false
}

fn read_wav(path: &Path) -> Result<Pcm> {
    let data = fs::read(path)?;
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(format!("{}: unsupported WAV", path.display()).into());
    }
    let mut cursor = 12usize;
    let mut format = None;
    let mut pcm_data = None;
    let mut has_loop_metadata = false;
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
        has_loop_metadata |= wav_chunk_declares_loop(id, &data[cursor..end]);
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
        has_loop_metadata,
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
    Pcm {
        rate,
        samples,
        has_loop_metadata: input.has_loop_metadata,
    }
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

fn cook_psau(directory: &Path, ids: &[String], rate: u32, label: &str) -> Result<Vec<Vec<u8>>> {
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
    psxed_audio::import_pack(
        &manifest_path,
        &zip_path,
        &out,
        &psxed_audio::PackOptions {
            write_preview_wav: false,
        },
    )?;
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

/// Convert the cooker's one-shot PSAU block flags into a hardware loop while
/// retaining its version-1 wrapper. The runtime parser still validates the
/// ordinary PSAU layout; only the raw SPU ADPCM control bytes differ.
fn mark_psau_loop(blob: &mut [u8]) -> Result<()> {
    const ADPCM_START: usize = 32; // 12-byte AssetHeader + 20-byte AudioHeader
    if blob.len() < ADPCM_START + 16 || &blob[..4] != b"PSAU" {
        return Err("charger PSAU is truncated or invalid".into());
    }
    let first_flag = ADPCM_START + 1;
    let last_flag = blob.len() - 15;
    if first_flag == last_flag {
        blob[first_flag] = (blob[first_flag] & !0x07) | 0x07;
    } else {
        blob[first_flag] = (blob[first_flag] & !0x07) | 0x04;
        blob[last_flag] = (blob[last_flag] & !0x07) | 0x03;
    }
    Ok(())
}

fn apply_map_audio_loop_flags(
    blob: &mut [u8],
    class: MapAudioClass,
    source_has_loop_metadata: bool,
) -> Result<()> {
    if class == MapAudioClass::Loop && source_has_loop_metadata {
        mark_psau_loop(blob)?;
    }
    Ok(())
}

const SOUNDS: [(&str, &str); 70] = [
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
    // Legacy id slots stay stable, but mover audio is now authored and
    // streamed per map. Null placeholders recover the long door beds without
    // renumbering every runtime id. Keep one tiny resident button fallback for
    // non-BSP interactions (tram controls, mounted guns, scripted panels).
    ("door_move", "common/null.wav"),
    ("door_stop", "common/null.wav"),
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
    // Dedicated wall-charger loops retain runtime ids 47/48. The runtime
    // patches their cooked one-shot ADPCM end flags into hardware loops and
    // owns them through a reserved SPU voice rather than the rotating SFX pool.
    ("charger_health_loop", "items/medcharge4.wav"),
    ("charger_hev_loop", "items/suitcharge1.wav"),
    ("flashlight", "items/flashlight1.wav"),
    ("m203", "weapons/glauncher.wav"),
    ("shotgun_double", "weapons/dbarrel1.wav"),
    ("gauss_charge", "weapons/electro5.wav"),
    // Compact, resident gameplay feedback restored by the sound sweep. These
    // are shared across the campaign, so keeping one low-rate SPU copy is much
    // cheaper than repeating them in all 96 per-map banks.
    ("ammo_pickup", "items/9mmclip1.wav"),
    ("healthkit", "items/smallmedkit1.wav"),
    ("health_deny", "items/medshotno1.wav"),
    ("suit_deny", "items/suitchargeno1.wav"),
    ("reload_357", "weapons/357_reload1.wav"),
    ("reload_xbow", "weapons/xbow_reload1.wav"),
    ("mp5_clip_release", "items/cliprelease1.wav"),
    ("mp5_clip_insert", "items/clipinsert1.wav"),
    ("reload_glock", "items/9mmclip2.wav"),
    ("reload_shotgun_alt", "weapons/reload3.wav"),
    ("shotgun_pump", "weapons/scock1.wav"),
    ("barney_attack", "barney/ba_attack2.wav"),
    // Half-Life's tiny 46 ms menu tick gives both boot and pause menus feedback
    // before any per-map bank exists. Navigation and confirm intentionally
    // share it: c3a2d leaves no SPU space for a second resident UI sample.
    ("menu_move", "common/menu1.wav"),
    // TEXTURETYPE_PlaySound CHAR_TEX_FLESH pair (dlls/sound.cpp): bullets
    // striking a living target thud instead of ricocheting. Crowbar keeps its
    // own cbar_hit sound, matching the SDK's crowbar exclusion.
    ("bullet_hit1", "weapons/bullet_hit1.wav"),
    ("bullet_hit2", "weapons/bullet_hit2.wav"),
    // The generic cbar_hit remains the quiet tool strike. GoldSrc layers this
    // material thump when TRACE_TEXTURE resolves CHAR_TEX_WOOD.
    ("wood_impact", "debris/wood1.wav"),
    // CCrowbar::Swing plays cbar_hitbod1-3 when the swing lands on anything
    // whose Classify() is neither CLASS_NONE nor CLASS_MACHINE -- the wet
    // thwack, never the metal dong the world hit uses.
    ("cbar_hitbod", "weapons/cbar_hitbod1.wav"),
];

pub fn build_sfx(sound: &Path, output: &Path) -> Result<()> {
    let scratch = Scratch::new("hlsfx")?;
    let mut rates = HashMap::new();
    for (id, relative) in SOUNDS {
        let source = read_wav(&sound.join(relative))?;
        // The two continuous charger beds are long enough that the ordinary
        // SFX rate would crowd the largest per-map dialogue bank out of SPU
        // RAM. Their mechanical/noise character survives 5 kHz cleanly and
        // leaves the full campaign dialogue roster resident.
        let rate = if matches!(
            id,
            "wood_impact" | "wood_break" | "cbar_hitbod" | "cbar_hit"
        ) {
            // Short noisy transients (wood splinter, both crowbar impacts)
            // remain clear at 4 kHz. Keeping them there is the smallest
            // quality-neutral saving that leaves c3a2d's full mandatory
            // dialogue bank inside physical SPU RAM -- adding cbar_hitbod at
            // any higher tier pushes that map over its budget. Buttons and the
            // two continuous chargers retain 5 kHz.
            4_000
        } else if id.starts_with("charger_") || id == "button" {
            5_000
        } else if matches!(
            id,
            "ammo_pickup"
                | "healthkit"
                | "health_deny"
                | "suit_deny"
                | "reload_357"
                | "reload_xbow"
                | "mp5_clip_release"
                | "mp5_clip_insert"
                | "reload_glock"
                | "reload_shotgun_alt"
                | "shotgun_pump"
                | "barney_attack"
                | "menu_move"
                | "bullet_hit1"
                | "bullet_hit2"
                | "dry"
                | "flashlight"
        ) {
            // These short, noisy/mechanical cues tolerate the same compact
            // rate as the charger beds. This saves enough resident SPU RAM for
            // c3a2d's unusually large dialogue roster without dropping a line.
            5_000
        } else if source.rate >= 22_050 {
            11_025
        } else {
            8_000
        };
        write_wav(
            &scratch.0.join(format!("{id}.wav")),
            &resample(&source, rate),
        )?;
        rates.insert(id, rate);
    }
    let mut encoded = HashMap::<String, Vec<u8>>::new();
    for rate in [4_000, 5_000, 8_000, 11_025] {
        let ids: Vec<String> = SOUNDS
            .iter()
            .filter(|(id, _)| rates.get(id) == Some(&rate))
            .map(|(id, _)| (*id).to_string())
            .collect();
        for (id, mut blob) in
            ids.iter()
                .zip(cook_psau(&scratch.0, &ids, rate, &format!("sfx{rate}"))?)
        {
            if id.starts_with("charger_") {
                mark_psau_loop(&mut blob)?;
            }
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

    // The complete combat bank remains the default. Anomalous Materials and
    // Hazard Course cannot emit most of it, so loading all 284 KB there merely
    // forces narration into telephone-rate ADPCM. Keep the 68 stable runtime
    // ids, replacing unreachable entries with a one-block silent PSAU.
    write_wav(
        &scratch.0.join("unused.wav"),
        &Pcm {
            rate: 5_000,
            samples: vec![0; 28],
            has_loop_metadata: false,
        },
    )?;
    let silence = cook_psau(&scratch.0, &["unused".to_string()], 5_000, "unused")?
        .pop()
        .ok_or("failed to cook silent SFX placeholder")?;
    let anomalous: Vec<Vec<u8>> = blobs
        .iter()
        .enumerate()
        .map(|(id, blob)| {
            if light_profile_keeps(id) {
                blob.clone()
            } else {
                silence.clone()
            }
        })
        .collect();
    let training: Vec<Vec<u8>> = blobs
        .iter()
        .enumerate()
        .map(|(id, blob)| {
            if training_weapon_profile_keeps(id) {
                blob.clone()
            } else {
                silence.clone()
            }
        })
        .collect();
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    for (chunk, label, profile) in [
        (3050, "narrative/lightweight", anomalous),
        (3051, "Hazard Course weapons", training),
    ] {
        let profile = hsfx(&profile);
        let path = parent.join(format!("chunk_{chunk}.psxa"));
        fs::write(&path, &profile)?;
        println!(
            "sfx profile: {label}: {} bytes -> {}",
            profile.len(),
            path.display()
        );
    }
    Ok(())
}

fn light_profile_keeps(id: usize) -> bool {
    matches!(
        id,
        9 | 10
            | 12
            | 13
            | 16..=19
            | 22..=26
            | 28
            | 30
            | 31
            | 34
            | 35
            | 43
            | 45..=49
            | 53..=56
            | 64..=67
    )
}

fn training_weapon_profile_keeps(id: usize) -> bool {
    light_profile_keeps(id) || matches!(id, 1 | 7 | 8 | 27 | 50 | 59 | 60 | 63 | 68)
}

fn resident_core_chunk(map_index: usize) -> u32 {
    if (6..=11).contains(&map_index) || (96..=97).contains(&map_index) {
        3050
    } else if map_index >= 98 {
        3051
    } else {
        3000
    }
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
    Ok(parse_sentences(&text))
}

fn parse_sentences(text: &str) -> HashMap<String, Vec<String>> {
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
            // GoldSrc accepts commas both as standalone sentence pauses and
            // directly between words. Treat them as separators rather than
            // part of a WAV filename (HEV_AAx uses both forms).
            for comma_part in raw_token.split(',') {
                let token = strip_sentence_params(comma_part);
                if token.is_empty() || token == "." {
                    continue;
                }
                if let Some((directory, _)) = token.rsplit_once('/') {
                    current_dir = format!("{directory}/");
                    wavs.push(format!("{token}.wav"));
                } else {
                    wavs.push(format!("{current_dir}{token}.wav"));
                }
            }
        }
        if !wavs.is_empty() {
            result.insert(name.to_ascii_uppercase(), wavs);
        }
    }
    result
}

fn push_voice_key(
    result: &mut Vec<MapAudioKey>,
    seen: &mut HashSet<String>,
    key: String,
    wavs: Vec<String>,
    class: MapAudioClass,
) {
    if seen.insert(key.clone()) {
        result.push(MapAudioKey { key, wavs, class });
    } else if class == MapAudioClass::Dialogue {
        // An authored line can share a sentence with autonomous chatter. It
        // must inherit authored priority so the allocator never sacrifices a
        // scripted sequence merely because the same key was discovered first.
        if let Some(existing) = result.iter_mut().find(|entry| entry.key == key) {
            existing.class = MapAudioClass::Dialogue;
        }
    }
}

fn voice_keys(
    _valve: &Path,
    map: &Path,
    map_index: u16,
    sentences: &HashMap<String, Vec<String>>,
) -> Result<Vec<MapAudioKey>> {
    const VOICE_DIRS: [&str; 8] = [
        "barney/",
        "scientist/",
        "gman/",
        "hgrunt/",
        "tride/",
        "vox/",
        "fvox/",
        "holo/",
    ];
    let entities = bsp_entities(map)?;
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    prepend_use_replies(&entities, sentences, &mut seen, &mut result)?;
    // Hazard Course narration is the primary information channel. Generic
    // predisaster small talk otherwise forces t0a0 down to the 2.4 kHz
    // emergency profile even though that chatter is not part of the course.
    let is_training = map
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.starts_with("t0"));
    if !is_training {
        let mut chatter = Vec::new();
        prepend_scientist_dialogue_keys(&entities, map_index, sentences, &mut seen, &mut chatter);
        result.extend(chatter.into_iter().map(|(key, wavs)| MapAudioKey {
            key,
            wavs,
            class: MapAudioClass::Chatter,
        }));
    }
    // CItemSuit::MyTouch emits one of these sentence definitions directly;
    // it is not authored as a scripted_sentence entity, so discover it from
    // the pickup itself. Keeping the concatenated sentence in this map's
    // streamed voice bank avoids approximating the logon with two resident
    // fragments (and avoids spending scarce resident SPU RAM campaign-wide).
    for entity in &entities {
        let class = entity.get("classname").map(String::as_str);
        let is_suit = class == Some("item_suit")
            || (class == Some("world_items")
                && entity.get("type").map(String::as_str) == Some("45"));
        if !is_suit {
            continue;
        }
        let short = entity
            .get("spawnflags")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            & 1
            != 0;
        let key = if short { "HEV_A0" } else { "HEV_AAX" };
        if let Some(wavs) = sentences.get(key) {
            push_voice_key(
                &mut result,
                &mut seen,
                key.to_string(),
                wavs.clone(),
                MapAudioClass::Dialogue,
            );
        }
    }
    for entity in entities {
        match entity.get("classname").map(String::as_str) {
            Some("scripted_sentence") => {
                let key = entity
                    .get("sentence")
                    .map(|s| s.trim_start_matches('!').to_ascii_uppercase())
                    .unwrap_or_default();
                if let Some(wavs) = sentences.get(&key) {
                    push_voice_key(
                        &mut result,
                        &mut seen,
                        key,
                        wavs.clone(),
                        MapAudioClass::Dialogue,
                    );
                }
            }
            Some("ambient_generic") | Some("speaker") => {
                let message = entity.get("message").cloned().unwrap_or_default();
                if let Some(sentence) = message.strip_prefix('!') {
                    let key = sentence.to_ascii_uppercase();
                    if let Some(wavs) = sentences.get(&key) {
                        push_voice_key(
                            &mut result,
                            &mut seen,
                            key,
                            wavs.clone(),
                            MapAudioClass::Dialogue,
                        );
                    }
                    continue;
                }
                let normalized = normalized_sound_path(&message);
                if normalized.ends_with(".wav")
                    && VOICE_DIRS
                        .iter()
                        .any(|prefix| normalized.starts_with(prefix))
                {
                    push_voice_key(
                        &mut result,
                        &mut seen,
                        normalized.clone(),
                        vec![normalized],
                        MapAudioClass::Dialogue,
                    );
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

const BUTTON_SOUNDS: [&str; 26] = [
    "common/null.wav",
    "buttons/button1.wav",
    "buttons/button2.wav",
    "buttons/button3.wav",
    "buttons/button4.wav",
    "buttons/button5.wav",
    "buttons/button6.wav",
    "buttons/button7.wav",
    "buttons/button8.wav",
    "buttons/button9.wav",
    "buttons/button10.wav",
    "buttons/button11.wav",
    "buttons/latchlocked1.wav",
    "buttons/latchunlocked1.wav",
    "buttons/lightswitch2.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/button9.wav",
    "buttons/lever1.wav",
    "buttons/lever2.wav",
    "buttons/lever3.wav",
    "buttons/lever4.wav",
    "buttons/lever5.wav",
];

const DOOR_MOVE_SOUNDS: [&str; 11] = [
    "common/null.wav",
    "doors/doormove1.wav",
    "doors/doormove2.wav",
    "doors/doormove3.wav",
    "doors/doormove4.wav",
    "doors/doormove5.wav",
    "doors/doormove6.wav",
    "doors/doormove7.wav",
    "doors/doormove8.wav",
    "doors/doormove9.wav",
    "doors/doormove10.wav",
];

const DOOR_STOP_SOUNDS: [&str; 9] = [
    "common/null.wav",
    "doors/doorstop1.wav",
    "doors/doorstop2.wav",
    "doors/doorstop3.wav",
    "doors/doorstop4.wav",
    "doors/doorstop5.wav",
    "doors/doorstop6.wav",
    "doors/doorstop7.wav",
    "doors/doorstop8.wav",
];

const PLAT_MOVE_SOUNDS: [&str; 14] = [
    "common/null.wav",
    "plats/bigmove1.wav",
    "plats/bigmove2.wav",
    "plats/elevmove1.wav",
    "plats/elevmove2.wav",
    "plats/elevmove3.wav",
    "plats/freightmove1.wav",
    "plats/freightmove2.wav",
    "plats/heavymove1.wav",
    "plats/rackmove1.wav",
    "plats/railmove1.wav",
    "plats/squeekmove1.wav",
    "plats/talkmove1.wav",
    "plats/talkmove2.wav",
];

const PLAT_STOP_SOUNDS: [&str; 9] = [
    "common/null.wav",
    "plats/bigstop1.wav",
    "plats/bigstop2.wav",
    "plats/freightstop1.wav",
    "plats/heavystop2.wav",
    "plats/rackstop1.wav",
    "plats/railstop1.wav",
    "plats/squeekstop1.wav",
    "plats/talkstop1.wav",
];

const TRACKTRAIN_SOUNDS: [&str; 7] = [
    "common/null.wav",
    "plats/ttrain1.wav",
    "plats/ttrain2.wav",
    "plats/ttrain3.wav",
    "plats/ttrain4.wav",
    "plats/ttrain6.wav",
    "plats/ttrain7.wav",
];

const FAN_SOUNDS: [&str; 6] = [
    "common/null.wav",
    "fans/fan1.wav",
    "fans/fan2.wav",
    "fans/fan3.wav",
    "fans/fan4.wav",
    "fans/fan5.wav",
];

const BREAK_SOUNDS: [&str; 8] = [
    "debris/bustglass1.wav",
    "debris/bustcrate1.wav",
    "debris/bustmetal1.wav",
    "debris/bustflesh1.wav",
    "debris/bustconcrete1.wav",
    "debris/bustceiling.wav",
    "debris/bustmetal1.wav",
    "debris/bustglass1.wav",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum MapAudioClass {
    Dialogue,
    Chatter,
    OneShot,
    Loop,
}

struct MapAudioKey {
    key: String,
    wavs: Vec<String>,
    class: MapAudioClass,
}

fn normalized_sound_path(path: &str) -> String {
    path.trim()
        // GoldSrc accepts both a leading slash and the `*` streaming marker
        // on ambient voice samples. Both refer to the same file below sound/.
        .trim_start_matches(['/', '\\', '*'])
        .to_ascii_lowercase()
}

fn is_voice_sound_path(path: &str) -> bool {
    let path = normalized_sound_path(path);
    path.ends_with(".wav")
        && [
            "barney/",
            "scientist/",
            "gman/",
            "hgrunt/",
            "tride/",
            "vox/",
            "fvox/",
            "holo/",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

fn push_map_sound(
    result: &mut Vec<MapAudioKey>,
    seen: &mut HashSet<String>,
    path: &str,
    class: MapAudioClass,
) {
    let path = normalized_sound_path(path);
    if path.is_empty() || path == "common/null.wav" || !path.ends_with(".wav") {
        return;
    }
    let mode = if class == MapAudioClass::Loop {
        "loop"
    } else {
        "shot"
    };
    let key = format!("sfx:{mode}:{path}");
    if seen.insert(key.clone()) {
        result.push(MapAudioKey {
            key,
            wavs: vec![path],
            class,
        });
    }
}

fn prepend_use_replies(
    entities: &[HashMap<String, String>],
    sentences: &HashMap<String, Vec<String>>,
    seen: &mut HashSet<String>,
    result: &mut Vec<MapAudioKey>,
) -> Result<()> {
    // Keep direct-use replies ahead of autonomous chatter. Stable alias keys
    // let the runtime use the actual cooked ids, including authored overrides.
    for (class, tag, defaults) in [
        (
            "monster_barney",
            "barney",
            ["BA_OK0", "BA_WAIT4", "BA_POK2"],
        ),
        (
            "monster_scientist",
            "scientist",
            ["SC_OK7", "SC_WAIT6", "SC_POK1"],
        ),
    ] {
        let Some(actor) = entities
            .iter()
            .find(|entity| entity.get("classname").map(String::as_str) == Some(class))
        else {
            continue;
        };
        for (index, action) in ["start", "stop", "decline"].iter().enumerate() {
            let eligible = entities.iter().any(|entry| {
                entry.get("classname").map(String::as_str) == Some(class)
                    && (entry
                        .get("spawnflags")
                        .and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(0)
                        & 256
                        != 0)
                        == (index == 2)
            });
            if !eligible {
                continue;
            }
            let authored = match index {
                0 => actor.get("UseSentence"),
                1 => actor.get("UnUseSentence"),
                _ => None,
            };
            let group = authored.map(|s| s.trim_start_matches('!').to_ascii_uppercase());
            let key = group
                .as_deref()
                .filter(|key| sentences.contains_key(*key))
                .map(str::to_owned)
                .or_else(|| {
                    group
                        .map(|group| format!("{group}0"))
                        .filter(|key| sentences.contains_key(key))
                })
                .unwrap_or_else(|| defaults[index].to_string());
            let wavs = sentences
                .get(&key)
                .ok_or_else(|| format!("missing NPC use sentence {key}"))?;
            push_voice_key(
                result,
                seen,
                format!("use:{tag}:{action}"),
                wavs.clone(),
                MapAudioClass::Dialogue,
            );
        }
    }
    Ok(())
}

fn map_audio_keys(
    valve: &Path,
    map: &Path,
    map_index: u16,
    sentences: &HashMap<String, Vec<String>>,
) -> Result<Vec<MapAudioKey>> {
    let mut result = voice_keys(valve, map, map_index, sentences)?;
    let mut seen: HashSet<String> = result.iter().map(|entry| entry.key.clone()).collect();
    for entity in bsp_entities(map)? {
        let class = entity.get("classname").map(String::as_str).unwrap_or("");
        match class {
            "env_beverage" => push_map_sound(
                &mut result,
                &mut seen,
                "weapons/g_bounce3.wav",
                MapAudioClass::OneShot,
            ),
            "func_button" | "func_rot_button" | "momentary_rot_button" => {
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "sounds", &BUTTON_SOUNDS),
                    MapAudioClass::OneShot,
                );
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "locked_sound", &BUTTON_SOUNDS),
                    MapAudioClass::OneShot,
                );
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "unlocked_sound", &BUTTON_SOUNDS),
                    MapAudioClass::OneShot,
                );
            }
            "func_door" | "func_door_rotating" | "momentary_door" => {
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "movesnd", &DOOR_MOVE_SOUNDS),
                    MapAudioClass::Loop,
                );
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "stopsnd", &DOOR_STOP_SOUNDS),
                    MapAudioClass::OneShot,
                );
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "locked_sound", &BUTTON_SOUNDS),
                    MapAudioClass::OneShot,
                );
            }
            "func_plat" | "func_platrot" | "func_train" => {
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "movesnd", &PLAT_MOVE_SOUNDS),
                    MapAudioClass::Loop,
                );
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "stopsnd", &PLAT_STOP_SOUNDS),
                    MapAudioClass::OneShot,
                );
            }
            "func_tracktrain" => {
                push_map_sound(
                    &mut result,
                    &mut seen,
                    "plats/ttrain_start1.wav",
                    MapAudioClass::OneShot,
                );
                push_map_sound(
                    &mut result,
                    &mut seen,
                    ordinal_sound(&entity, "sounds", &TRACKTRAIN_SOUNDS),
                    MapAudioClass::Loop,
                );
                push_map_sound(
                    &mut result,
                    &mut seen,
                    "plats/ttrain_brake1.wav",
                    MapAudioClass::OneShot,
                );
            }
            "func_rotating" => {
                let path = entity
                    .get("message")
                    .filter(|value| !value.is_empty())
                    .map(String::as_str)
                    .unwrap_or_else(|| ordinal_sound(&entity, "sounds", &FAN_SOUNDS));
                push_map_sound(&mut result, &mut seen, path, MapAudioClass::Loop);
            }
            "func_breakable" | "func_pushable" => {
                let material = entity
                    .get("material")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0)
                    .min(BREAK_SOUNDS.len() - 1);
                push_map_sound(
                    &mut result,
                    &mut seen,
                    BREAK_SOUNDS[material],
                    MapAudioClass::OneShot,
                );
            }
            "env_spark" | "env_debris" => push_map_sound(
                &mut result,
                &mut seen,
                "buttons/spark1.wav",
                MapAudioClass::OneShot,
            ),
            "ambient_generic" => {
                let path = entity.get("message").map(String::as_str).unwrap_or("");
                if !is_voice_sound_path(path) {
                    let spawnflags = entity
                        .get("spawnflags")
                        .and_then(|value| value.parse::<u32>().ok())
                        .unwrap_or(0);
                    let class = if spawnflags & 32 != 0 {
                        MapAudioClass::OneShot
                    } else {
                        MapAudioClass::Loop
                    };
                    push_map_sound(&mut result, &mut seen, path, class);
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

fn ordinal_sound<'a>(entity: &HashMap<String, String>, key: &str, table: &'a [&str]) -> &'a str {
    let ordinal = entity
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    table
        .get(ordinal)
        .copied()
        .unwrap_or_else(|| table.last().copied().unwrap_or("common/null.wav"))
}

fn count_sound(
    counts: &mut BTreeMap<String, (usize, HashSet<String>)>,
    family: &str,
    path: &str,
    map: &str,
) {
    if path.is_empty() || path.eq_ignore_ascii_case("common/null.wav") {
        return;
    }
    let entry = counts
        .entry(format!("{family:<16} {path}"))
        .or_insert_with(|| (0, HashSet::new()));
    entry.0 += 1;
    entry.1.insert(map.to_string());
}

/// Enumerate every map-authored mechanical and ambient sound selection in the
/// shipped campaign. This deliberately uses the same Rust BSP entity parser as
/// the production cooker, so the report also guards the extraction path.
pub fn audit_map_sounds(valve: &Path, map_list: &str) -> Result<()> {
    let mut counts = BTreeMap::<String, (usize, HashSet<String>)>::new();
    let mut per_map = BTreeMap::<String, (HashSet<String>, HashSet<String>)>::new();
    let mut entities_seen = 0usize;
    for map in map_list.split_whitespace() {
        let bsp = valve.join("maps").join(format!("{map}.bsp"));
        if !bsp.is_file() {
            continue;
        }
        for entity in bsp_entities(&bsp)? {
            entities_seen += 1;
            let class = entity.get("classname").map(String::as_str).unwrap_or("");
            match class {
                "func_button" | "func_rot_button" | "momentary_rot_button" => count_sound(
                    &mut counts,
                    "button",
                    ordinal_sound(&entity, "sounds", &BUTTON_SOUNDS),
                    map,
                ),
                "func_door" | "func_door_rotating" | "momentary_door" => {
                    count_sound(
                        &mut counts,
                        "door-move",
                        ordinal_sound(&entity, "movesnd", &DOOR_MOVE_SOUNDS),
                        map,
                    );
                    count_sound(
                        &mut counts,
                        "door-stop",
                        ordinal_sound(&entity, "stopsnd", &DOOR_STOP_SOUNDS),
                        map,
                    );
                }
                "func_plat" | "func_platrot" | "func_train" => {
                    count_sound(
                        &mut counts,
                        "plat-move",
                        ordinal_sound(&entity, "movesnd", &PLAT_MOVE_SOUNDS),
                        map,
                    );
                    count_sound(
                        &mut counts,
                        "plat-stop",
                        ordinal_sound(&entity, "stopsnd", &PLAT_STOP_SOUNDS),
                        map,
                    );
                }
                "func_tracktrain" => {
                    count_sound(
                        &mut counts,
                        "tracktrain",
                        ordinal_sound(&entity, "sounds", &TRACKTRAIN_SOUNDS),
                        map,
                    );
                    count_sound(&mut counts, "track-start", "plats/ttrain_start1.wav", map);
                    count_sound(&mut counts, "track-brake", "plats/ttrain_brake1.wav", map);
                }
                "func_rotating" => {
                    let path = entity
                        .get("message")
                        .filter(|value| !value.is_empty())
                        .map(String::as_str)
                        .unwrap_or_else(|| ordinal_sound(&entity, "sounds", &FAN_SOUNDS));
                    count_sound(&mut counts, "fan", path, map);
                }
                "ambient_generic" => {
                    let path = entity.get("message").map(String::as_str).unwrap_or("");
                    if path.to_ascii_lowercase().ends_with(".wav") {
                        let spawnflags = entity
                            .get("spawnflags")
                            .and_then(|value| value.parse::<u32>().ok())
                            .unwrap_or(0);
                        let family = if spawnflags & 32 != 0 {
                            "ambient-shot"
                        } else {
                            "ambient-loop"
                        };
                        count_sound(&mut counts, family, path, map);
                        let entry = per_map.entry(map.to_string()).or_default();
                        let normalized = path.trim_start_matches('/').to_ascii_lowercase();
                        if spawnflags & 32 != 0 {
                            entry.0.insert(normalized);
                        } else {
                            entry.1.insert(normalized);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    println!("GoldSrc sound audit: {entities_seen} entities");
    for (sound, (uses, maps)) in &counts {
        println!("{uses:4} uses  {:3} maps  {sound}", maps.len());
    }
    println!("{} distinct authored sound/family pairs", counts.len());
    let mut busiest: Vec<_> = per_map
        .iter()
        .map(|(map, (shots, loops))| (shots.len() + loops.len(), shots.len(), loops.len(), map))
        .collect();
    busiest.sort_unstable_by(|a, b| b.cmp(a));
    println!("busiest ambient maps (total / one-shot / loop):");
    for (total, shots, loops, map) in busiest.into_iter().take(20) {
        println!("  {map:<8} {total:3} / {shots:3} / {loops:3}");
    }
    Ok(())
}

#[inline]
fn predisaster_idle_variant(map_index: u16, actor_index: usize) -> usize {
    let actor = actor_index % 5;
    (actor * actor * 9 + actor * 7 + map_index as usize * 3 + 1) % 11
}

#[inline]
fn predisaster_question_variant(map_index: u16, actor_index: usize) -> usize {
    let parity_bias = if actor_index & 1 == 0 { 14 } else { 7 };
    (map_index as usize * 12 + parity_bias) % 18
}

#[inline]
fn scientist_answer_variant(map_index: u16, actor_index: usize) -> usize {
    let parity_bias = if actor_index & 1 == 0 { 2 } else { 13 };
    (map_index as usize * 3 + parity_bias) % 30
}

/// Prefix only the deterministic variants this map's standing scientists can
/// actually choose, in scientist entity order. The runtime derives the same
/// first-occurrence ids by scanning its existing prop roster. Pre-disaster
/// small talk is bounded to five idle statements, two questions, and two
/// answers per map so authored dialogue retains its SPU budget.
fn prepend_scientist_dialogue_keys(
    entities: &[HashMap<String, String>],
    map_index: u16,
    sentences: &HashMap<String, Vec<String>>,
    seen: &mut HashSet<String>,
    result: &mut Vec<(String, Vec<String>)>,
) {
    let scientists: Vec<(usize, bool)> = entities
        .iter()
        .filter(|entity| entity.get("classname").map(String::as_str) == Some("monster_scientist"))
        .enumerate()
        .map(|(scientist_index, entity)| {
            let spawnflags = entity
                .get("spawnflags")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            (scientist_index, spawnflags & 256 != 0)
        })
        .collect();

    for &(scientist_index, predisaster) in &scientists {
        let variant = if predisaster {
            (map_index as usize * 5 + scientist_index * 2 + 5) % 7
        } else {
            (map_index as usize * 7 + scientist_index * 3 + 3) % 9
        };
        let prefix = if predisaster { "SC_PHELLO" } else { "SC_HELLO" };
        let key = format!("{prefix}{variant}");
        if let Some(wavs) = sentences.get(&key) {
            if seen.insert(key.clone()) {
                result.push((key, wavs.clone()));
            }
        }
    }

    for (prefix, variant) in [
        (
            "SC_PIDLE",
            predisaster_idle_variant as fn(u16, usize) -> usize,
        ),
        ("SC_PQUEST", predisaster_question_variant),
        ("SC_ANSWER", scientist_answer_variant),
    ] {
        for &(scientist_index, predisaster) in &scientists {
            if !predisaster {
                continue;
            }
            let key = format!("{prefix}{}", variant(map_index, scientist_index));
            if let Some(wavs) = sentences.get(&key) {
                if seen.insert(key.clone()) {
                    result.push((key, wavs.clone()));
                }
            }
        }
    }
}

fn concat_wavs(valve: &Path, wavs: &[String], rate: u32) -> Result<Option<Pcm>> {
    let mut samples = Vec::new();
    let mut has_loop_metadata = false;
    for relative in wavs {
        let path = valve.join("sound").join(normalized_sound_path(relative));
        if !path.exists() {
            return Ok(None);
        }
        let source = resample(&read_wav(&path)?, rate);
        has_loop_metadata |= source.has_loop_metadata;
        samples.extend(source.samples);
    }
    Ok(Some(Pcm {
        rate,
        samples,
        has_loop_metadata,
    }))
}

const DIALOGUE_RATES: &[u32] = &[11_025, 8_000, 6_000, 5_000, 4_000, 3_200, 2_800, 2_400];
const CHATTER_RATES: &[u32] = &[8_000, 6_000, 5_000, 4_000, 3_200, 2_800, 2_400];
const SHOT_RATES: &[u32] = &[8_000, 6_000, 5_000, 4_000, 3_200, 2_800, 2_400];
const LOOP_RATES: &[u32] = &[
    5_000, 4_000, 3_500, 3_000, 2_500, 2_000, 1_800, 1_600, 1_400,
];

fn class_rates(class: MapAudioClass) -> &'static [u32] {
    match class {
        MapAudioClass::Dialogue => DIALOGUE_RATES,
        MapAudioClass::Chatter => CHATTER_RATES,
        MapAudioClass::OneShot => SHOT_RATES,
        MapAudioClass::Loop => LOOP_RATES,
    }
}

fn resampled_sample_count(valve: &Path, wavs: &[String], rate: u32) -> Result<Option<usize>> {
    let mut count = 0usize;
    for relative in wavs {
        let path = valve.join("sound").join(normalized_sound_path(relative));
        if !path.exists() {
            return Ok(None);
        }
        let source = read_wav(&path)?;
        count = count.saturating_add(
            ((source.samples.len() as u64 * rate as u64) / source.rate.max(1) as u64).max(1)
                as usize,
        );
    }
    Ok(Some(count))
}

#[inline]
fn predicted_psau_size(sample_count: usize) -> usize {
    // Sony ADPCM stores 28 PCM samples per 16-byte block. psxed_audio adds a
    // fixed 32-byte PSAU header and pads only the final ADPCM block.
    32 + sample_count.div_ceil(28) * 16
}

fn predicted_size_ladders(valve: &Path, entries: &[&MapAudioKey]) -> Result<Vec<Vec<usize>>> {
    let mut ladders = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut sizes = Vec::with_capacity(class_rates(entry.class).len());
        for &rate in class_rates(entry.class) {
            let count = resampled_sample_count(valve, &entry.wavs, rate)?
                .ok_or_else(|| format!("missing map-audio source for {}", entry.key))?;
            sizes.push(predicted_psau_size(count));
        }
        ladders.push(sizes);
    }
    Ok(ladders)
}

fn predicted_map_pack_size(size_ladders: &[Vec<usize>], rate_steps: &[usize]) -> usize {
    let mut bytes = 8 + size_ladders.len() * 8; // HSFX header + sample table
    for (sizes, &step) in size_ladders.iter().zip(rate_steps) {
        bytes = bytes.saturating_add(sizes[step]);
    }
    bytes
}

pub fn build_voices(valve: &Path, map_list: &str, output: &Path) -> Result<()> {
    const RUNTIME_MAX_MAP_AUDIO: usize = 96;
    fs::create_dir_all(output)?;
    for entry in fs::read_dir(output)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("chunk_") || name == "manifest.txt" {
            fs::remove_file(path)?;
        }
    }
    let sfx_dir = output.parent().unwrap_or(output).join("sfx");
    let sentences = load_sentences(valve)?;
    let mut manifest = Vec::new();
    for (map_index, map_name) in map_list.split_whitespace().enumerate() {
        let bsp = valve.join("maps").join(format!("{map_name}.bsp"));
        if !bsp.exists() {
            continue;
        }
        let keys = map_audio_keys(valve, &bsp, map_index as u16, &sentences)?;
        if keys.is_empty() {
            continue;
        }
        if keys.len() > RUNTIME_MAX_MAP_AUDIO {
            return Err(format!(
                "{map_name}: {} map-audio samples exceed runtime maximum {RUNTIME_MAX_MAP_AUDIO}",
                keys.len()
            )
            .into());
        }
        let core_chunk = resident_core_chunk(map_index);
        let core_bytes = fs::metadata(sfx_dir.join(format!("chunk_{core_chunk}.psxa")))
            .map(|v| v.len() as usize)
            .unwrap_or(414 * 1024);
        let budget = 512 * 1024 - 0x1010 - core_bytes - 4096;
        let mut valid: Vec<&MapAudioKey> = keys
            .iter()
            .filter(|entry| {
                entry.wavs.iter().all(|relative| {
                    valve
                        .join("sound")
                        .join(normalized_sound_path(relative))
                        .is_file()
                })
            })
            .collect();
        if valid.is_empty() {
            return Err(format!("{map_name}: no map-audio samples could be encoded").into());
        }
        if keys
            .iter()
            .filter(|entry| entry.key.starts_with("use:"))
            .count()
            != valid
                .iter()
                .filter(|entry| entry.key.starts_with("use:"))
                .count()
        {
            return Err(format!("{map_name}: missing audio for an NPC use reply").into());
        }

        // The complete HEV logon is deliberately preferred where it fits,
        // but some combat maps combine spare suits with far more mandatory
        // authored dialogue than the SPU can hold. Only omit the logon when
        // the pack would exceed the budget even with every sample at its
        // minimum rate; the runtime then uses its resident activation line.
        let minimum_pack_size = |entries: &[&MapAudioKey]| -> Result<usize> {
            let ladders = predicted_size_ladders(valve, entries)?;
            let minimum_steps: Vec<usize> = ladders
                .iter()
                .map(|sizes| sizes.len().saturating_sub(1))
                .collect();
            Ok(predicted_map_pack_size(&ladders, &minimum_steps))
        };
        if minimum_pack_size(&valid)? > budget {
            if let Some(index) = valid
                .iter()
                .position(|entry| matches!(entry.key.as_str(), "HEV_AAX" | "HEV_A0"))
            {
                println!(
                    "  {map_name}: full HEV logon cannot fit beside mandatory map audio; using resident pickup line"
                );
                valid.remove(index);
            }
        }

        // Direct-use replies must remain available. On the few maps whose
        // mandatory speech already fills SPU RAM, omit autonomous small talk
        // before sacrificing a scripted line or an interaction response.
        if minimum_pack_size(&valid)? > budget {
            valid.retain(|entry| entry.class != MapAudioClass::Chatter);
            println!(
                "  {map_name}: reserving speech bank for authored and player-triggered dialogue"
            );
        }

        // Start every sample at its class's preferred rate. When the bank is
        // too large, consume quality from loops, short effects, and autonomous
        // chatter—in that order—before touching authored dialogue. Within a
        // class the largest byte saving wins, so a long machine bed cannot
        // force every spoken line through one map-wide emergency profile.
        let size_ladders = predicted_size_ladders(valve, &valid)?;
        let mut rate_steps = vec![0usize; valid.len()];
        let mut predicted = predicted_map_pack_size(&size_ladders, &rate_steps);
        while predicted > budget {
            let mut selected = None;
            for class in [
                MapAudioClass::Loop,
                MapAudioClass::OneShot,
                MapAudioClass::Chatter,
                MapAudioClass::Dialogue,
            ] {
                let mut best_saving = 0usize;
                for (index, entry) in valid.iter().enumerate() {
                    if entry.class != class || rate_steps[index] + 1 >= class_rates(class).len() {
                        continue;
                    }
                    let current = size_ladders[index][rate_steps[index]];
                    let next = size_ladders[index][rate_steps[index] + 1];
                    let saving = current.saturating_sub(next);
                    if saving > best_saving {
                        best_saving = saving;
                        selected = Some(index);
                    }
                }
                if selected.is_some() {
                    break;
                }
            }
            let Some(index) = selected else {
                let class_bytes = |class| {
                    valid
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| entry.class == class)
                        .map(|(index, _)| size_ladders[index][rate_steps[index]])
                        .sum::<usize>()
                };
                return Err(format!(
                    "{map_name}: map-audio pack is {predicted} bytes at minimum per-sample rates; SPU budget is {budget} (dialogue {}, chatter {}, shots {}, loops {})",
                    class_bytes(MapAudioClass::Dialogue),
                    class_bytes(MapAudioClass::Chatter),
                    class_bytes(MapAudioClass::OneShot),
                    class_bytes(MapAudioClass::Loop),
                )
                .into());
            };
            rate_steps[index] += 1;
            predicted = predicted_map_pack_size(&size_ladders, &rate_steps);
        }

        let scratch = Scratch::new("hlvox")?;
        let mut cooked = Vec::<(&MapAudioKey, u32, String, bool)>::new();
        for (index, entry) in valid.iter().enumerate() {
            let rate = class_rates(entry.class)[rate_steps[index]];
            let id = format!("a{index:02}");
            let pcm = concat_wavs(valve, &entry.wavs, rate)?
                .ok_or_else(|| format!("missing map-audio source for {}", entry.key))?;
            write_wav(&scratch.0.join(format!("{id}.wav")), &pcm)?;
            cooked.push((entry, rate, id, pcm.has_loop_metadata));
        }
        let mut blobs: Vec<Option<Vec<u8>>> = vec![None; cooked.len()];
        let mut rate_groups = BTreeMap::<u32, Vec<usize>>::new();
        for (index, (_, rate, _, _)) in cooked.iter().enumerate() {
            rate_groups.entry(*rate).or_default().push(index);
        }
        for (rate, indices) in rate_groups {
            let ids: Vec<String> = indices
                .iter()
                .map(|&index| cooked[index].2.clone())
                .collect();
            for (&index, mut blob) in
                indices
                    .iter()
                    .zip(cook_psau(&scratch.0, &ids, rate, "map-audio")?)
            {
                apply_map_audio_loop_flags(&mut blob, cooked[index].0.class, cooked[index].3)?;
                blobs[index] = Some(blob);
            }
        }
        let blobs: Vec<Vec<u8>> = blobs
            .into_iter()
            .map(|blob| blob.expect("every map-audio rate group was encoded"))
            .collect();
        let pack = hsfx(&blobs);
        if pack.len() > budget {
            return Err(format!(
                "{map_name}: predicted {predicted} byte pack encoded to {} bytes, over {budget} byte SPU budget",
                pack.len()
            )
            .into());
        }
        let chunk = 3100 + map_index;
        fs::write(output.join(format!("chunk_{chunk}.psxa")), &pack)?;
        manifest.extend(
            cooked
                .iter()
                .enumerate()
                .map(|(id, (entry, _, _, _))| format!("{map_index}|{id}|{}", entry.key)),
        );
        let count = |class| cooked.iter().filter(|entry| entry.0.class == class).count();
        let range = |class| {
            let mut rates = cooked
                .iter()
                .filter(|entry| entry.0.class == class)
                .map(|entry| entry.1);
            let Some(first) = rates.next() else {
                return String::from("-");
            };
            let (mut lo, mut hi) = (first, first);
            for rate in rates {
                lo = lo.min(rate);
                hi = hi.max(rate);
            }
            if lo == hi {
                format!("{lo}")
            } else {
                format!("{lo}-{hi}")
            }
        };
        println!(
            "  {map_name} (idx {map_index}, chunk {chunk}): {} dialogue @{} + {} chatter @{} + {} shot @{} + {} loop @{} Hz, {} KB",
            count(MapAudioClass::Dialogue),
            range(MapAudioClass::Dialogue),
            count(MapAudioClass::Chatter),
            range(MapAudioClass::Chatter),
            count(MapAudioClass::OneShot),
            range(MapAudioClass::OneShot),
            count(MapAudioClass::Loop),
            range(MapAudioClass::Loop),
            pack.len() / 1024,
        );
    }
    fs::write(output.join("manifest.txt"), manifest.join("\n") + "\n")?;
    println!(
        "map audio -> {} ({} samples across maps)",
        output.display(),
        manifest.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campaign_and_training_use_replies_keep_stable_aliases() {
        let actors = vec![
            HashMap::from([
                ("classname".into(), "monster_barney".into()),
                ("UseSentence".into(), "BA_HAZ_OK".into()),
                ("UnUseSentence".into(), "BA_HAZ_WAIT".into()),
            ]),
            HashMap::from([("classname".into(), "monster_scientist".into())]),
        ];
        let sentences: HashMap<_, _> = [
            "BA_HAZ_OK0",
            "BA_HAZ_WAIT0",
            "BA_POK2",
            "SC_OK7",
            "SC_WAIT6",
            "SC_POK1",
        ]
        .into_iter()
        .map(|key| (key.to_string(), vec![format!("{key}.wav")]))
        .collect();
        let mut result = Vec::new();
        prepend_use_replies(&actors, &sentences, &mut HashSet::new(), &mut result).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].key, "use:barney:start");
        assert_eq!(result[0].wavs, ["BA_HAZ_OK0.wav"]);
        assert_eq!(result[1].wavs, ["BA_HAZ_WAIT0.wav"]);
        assert_eq!(result[2].key, "use:scientist:start");
        assert_eq!(result[3].wavs, ["SC_WAIT6.wav"]);
    }

    #[test]
    fn psau_loop_marks_first_and_last_adpcm_blocks() {
        let mut blob = vec![0u8; 32 + 3 * 16];
        blob[..4].copy_from_slice(b"PSAU");
        blob[33] = 0x01;
        blob[49] = 0x02;
        blob[65] = 0x01;

        mark_psau_loop(&mut blob).unwrap();

        assert_eq!(blob[33] & 0x07, 0x04);
        assert_eq!(blob[49] & 0x07, 0x02);
        assert_eq!(blob[65] & 0x07, 0x03);
    }

    #[test]
    fn wav_loop_metadata_requires_an_authored_loop_record() {
        let mut cue = vec![0u8; 28];
        cue[..4].copy_from_slice(&1u32.to_le_bytes());
        assert!(wav_chunk_declares_loop(b"cue ", &cue));

        let mut smpl = vec![0u8; 60];
        smpl[28..32].copy_from_slice(&1u32.to_le_bytes());
        assert!(wav_chunk_declares_loop(b"smpl", &smpl));

        assert!(!wav_chunk_declares_loop(b"LIST", b"adtlltxt"));
        assert!(!wav_chunk_declares_loop(b"cue ", &[0, 0, 0, 0]));
        assert!(!wav_chunk_declares_loop(b"smpl", &[0; 32]));
    }

    #[test]
    fn map_audio_needs_both_loop_semantics_and_wav_metadata() {
        let blob = || {
            let mut blob = vec![0u8; 32 + 2 * 16];
            blob[..4].copy_from_slice(b"PSAU");
            blob[33] = 0x01;
            blob[49] = 0x01;
            blob
        };

        let mut missing_metadata = blob();
        apply_map_audio_loop_flags(&mut missing_metadata, MapAudioClass::Loop, false).unwrap();
        assert_eq!(missing_metadata[33] & 0x07, 0x01);
        assert_eq!(missing_metadata[49] & 0x07, 0x01);

        let mut explicit_one_shot = blob();
        apply_map_audio_loop_flags(&mut explicit_one_shot, MapAudioClass::OneShot, true).unwrap();
        assert_eq!(explicit_one_shot[33] & 0x07, 0x01);
        assert_eq!(explicit_one_shot[49] & 0x07, 0x01);

        let mut authored_loop = blob();
        apply_map_audio_loop_flags(&mut authored_loop, MapAudioClass::Loop, true).unwrap();
        assert_eq!(authored_loop[33] & 0x07, 0x04);
        assert_eq!(authored_loop[49] & 0x07, 0x03);
    }

    #[test]
    fn nearest_resampler_preserves_duration() {
        let input = Pcm {
            rate: 22_050,
            samples: (0..2205).collect(),
            has_loop_metadata: true,
        };
        let output = resample(&input, 11_025);
        assert_eq!(output.samples.len(), 1102);
        assert!(output.has_loop_metadata);
    }

    #[test]
    fn sentence_parameters_are_not_part_of_wav_name() {
        assert_eq!(strip_sentence_params("barney/hello(p120)"), "barney/hello");
    }

    #[test]
    fn suit_logon_sentence_splits_attached_goldsrc_commas() {
        let sentences = parse_sentences(
            "HEV_AAx fvox/bell, HEV_logon, powerarmor_on, \
             atmospherics_on,vitalsigns_on, automedic_on, weaponselect_on, \
             munitionview_on, communications_on, safe_day",
        );
        assert_eq!(
            sentences.get("HEV_AAX").unwrap(),
            &[
                "fvox/bell.wav",
                "fvox/HEV_logon.wav",
                "fvox/powerarmor_on.wav",
                "fvox/atmospherics_on.wav",
                "fvox/vitalsigns_on.wav",
                "fvox/automedic_on.wav",
                "fvox/weaponselect_on.wav",
                "fvox/munitionview_on.wav",
                "fvox/communications_on.wav",
                "fvox/safe_day.wav",
            ]
        );
    }

    #[test]
    fn goldsrc_sound_paths_normalize_stream_and_root_markers() {
        assert_eq!(
            normalized_sound_path("*/Scientist/C1A0_Test.WAV"),
            "scientist/c1a0_test.wav"
        );
        assert!(is_voice_sound_path("/BARNEY/BA_HELLO.WAV"));
    }

    #[test]
    fn button_sound_ordinals_keep_the_distinctive_authored_samples() {
        let mut entity = HashMap::new();
        entity.insert("sounds".to_string(), "14".to_string());
        assert_eq!(
            ordinal_sound(&entity, "sounds", &BUTTON_SOUNDS),
            "buttons/lightswitch2.wav"
        );
        entity.insert("sounds".to_string(), "21".to_string());
        assert_eq!(
            ordinal_sound(&entity, "sounds", &BUTTON_SOUNDS),
            "buttons/lever1.wav"
        );
    }

    #[test]
    fn resident_sound_ids_stay_in_runtime_order() {
        assert_eq!(SOUNDS[16].1, "buttons/button3.wav");
        assert_eq!(SOUNDS[53].1, "items/9mmclip1.wav");
        assert_eq!(SOUNDS[54].1, "items/smallmedkit1.wav");
        assert_eq!(SOUNDS[64].1, "barney/ba_attack2.wav");
        assert_eq!(SOUNDS[65].1, "common/menu1.wav");
    }

    #[test]
    fn resident_profiles_keep_stable_ids_at_chapter_boundaries() {
        assert_eq!(resident_core_chunk(5), 3000);
        assert_eq!(resident_core_chunk(6), 3050);
        assert_eq!(resident_core_chunk(11), 3050);
        assert_eq!(resident_core_chunk(12), 3000);
        assert_eq!(resident_core_chunk(96), 3050);
        assert_eq!(resident_core_chunk(97), 3050);
        assert_eq!(resident_core_chunk(98), 3051);

        assert!(light_profile_keeps(16), "buttons remain available");
        assert!(
            !light_profile_keeps(1),
            "the MP5 is absent before weapon training"
        );
        assert!(training_weapon_profile_keeps(1));
        assert!(
            training_weapon_profile_keeps(50),
            "M203 is taught in Hazard Course"
        );
    }

    #[test]
    fn scientist_greetings_have_stable_local_ids_before_authored_dialogue() {
        let mut sentences = HashMap::new();
        for (prefix, count) in [("SC_PHELLO", 7usize), ("SC_HELLO", 9usize)] {
            for variant in 0..count {
                let key = format!("{prefix}{variant}");
                sentences.insert(key.clone(), vec![format!("scientist/{key}.wav")]);
            }
        }
        let mut scientist0 = HashMap::new();
        scientist0.insert("classname".to_string(), "monster_scientist".to_string());
        scientist0.insert("spawnflags".to_string(), "256".to_string());
        let scientist1 = scientist0.clone();
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        prepend_scientist_dialogue_keys(
            &[scientist0, scientist1],
            7,
            &sentences,
            &mut seen,
            &mut result,
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "SC_PHELLO5");
        assert_eq!(result[1].0, "SC_PHELLO0");
    }

    #[test]
    fn scientist_small_talk_is_bounded_and_follows_greetings() {
        let mut sentences = HashMap::new();
        for (prefix, count) in [
            ("SC_PHELLO", 7usize),
            ("SC_PIDLE", 11usize),
            ("SC_PQUEST", 18usize),
            ("SC_ANSWER", 30usize),
        ] {
            for variant in 0..count {
                let key = format!("{prefix}{variant}");
                sentences.insert(key.clone(), vec![format!("scientist/{key}.wav")]);
            }
        }
        let scientists: Vec<_> = (0..13)
            .map(|_| {
                let mut entity = HashMap::new();
                entity.insert("classname".to_string(), "monster_scientist".to_string());
                entity.insert("spawnflags".to_string(), "256".to_string());
                entity
            })
            .collect();
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        prepend_scientist_dialogue_keys(&scientists, 6, &sentences, &mut seen, &mut result);
        let keys: Vec<_> = result.iter().map(|entry| entry.0.as_str()).collect();
        let first_small_talk = keys
            .iter()
            .position(|key| key.starts_with("SC_PIDLE"))
            .unwrap();
        assert!(keys[..first_small_talk]
            .iter()
            .all(|key| key.starts_with("SC_PHELLO")));
        assert!(
            keys.iter()
                .filter(|key| key.starts_with("SC_PIDLE"))
                .count()
                <= 5
        );
        assert!(
            keys.iter()
                .filter(|key| key.starts_with("SC_PQUEST"))
                .count()
                <= 2
        );
        assert!(
            keys.iter()
                .filter(|key| key.starts_with("SC_ANSWER"))
                .count()
                <= 2
        );
    }
}

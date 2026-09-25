use crate::generators::bsp_entities;
use crate::Result;
use psx_audio_cook::resample::Sinc;
use psx_audio_cook::{CookOptions, Looping, Wav};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

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
        // mkisopsx resolves entries relative to tracks.txt, so cooked music
        // remains packable after moving the asset directory.
        listing.push(format!("track_{:02}.cdda", index + 1));
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

/// A sound to cook: mono samples at the authored rate, plus whether the WAV
/// itself declares a loop (GoldSrc's rule for hardware repeats).
#[derive(Clone)]
struct Source {
    wav: Wav,
    has_loop_metadata: bool,
}

impl Source {
    fn seconds(&self) -> f64 {
        self.wav.samples.len() as f64 / self.wav.rate.max(1) as f64
    }
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

fn wav_declares_loop(data: &[u8]) -> bool {
    let mut cursor = 12usize;
    let mut found = false;
    while cursor + 8 <= data.len() {
        let id = &data[cursor..cursor + 4];
        let len = u32::from_le_bytes([
            data[cursor + 4],
            data[cursor + 5],
            data[cursor + 6],
            data[cursor + 7],
        ]) as usize;
        cursor += 8;
        let end = cursor.saturating_add(len).min(data.len());
        found |= wav_chunk_declares_loop(id, &data[cursor..end]);
        cursor = end + (len & 1);
    }
    found
}

fn read_source(path: &Path) -> Result<Source> {
    let data = fs::read(path)?;
    let wav = psx_audio_cook::wav::read(&data).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Source {
        has_loop_metadata: wav_declares_loop(&data),
        wav,
    })
}

fn looping_mode(looping: bool) -> Looping {
    if looping {
        Looping::Whole
    } else {
        Looping::None
    }
}

/// Cook one sound with the shared SDK encoder (psx-audio-cook): windowed-sinc
/// resampling, pre-emphasis for the SPU's Gaussian interpolation, trellis
/// ADPCM, peak 0.9 as before. A loop repeats the whole sample, rounded to
/// whole ADPCM blocks so no padding plays at the seam.
fn cook_psau(source: &Source, rate: u32, looping: bool) -> Vec<u8> {
    let mut options = CookOptions::one_shot(rate);
    options.looping = looping_mode(looping);
    let cooked = psx_audio_cook::cook(&source.wav, &options);
    psx_audio_cook::psau(rate, cooked.pcm.len(), &cooked.adpcm)
}

/// Exact size of [`cook_psau`]'s output: 32 header bytes plus the blocks.
fn psau_size(source: &Source, rate: u32, looping: bool) -> usize {
    32 + psx_audio_cook::adpcm_bytes(psx_audio_cook::cooked_len(
        &source.wav,
        rate,
        looping_mode(looping),
    ))
}

/// Cook `jobs` (source, rate, looping) on every core; output order matches.
fn cook_parallel(jobs: &[(&Source, u32, bool)]) -> Vec<Vec<u8>> {
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Vec<u8>>> = jobs.iter().map(|_| Mutex::new(Vec::new())).collect();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(jobs.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(source, rate, looping)) = jobs.get(index) else {
                    break;
                };
                let blob = cook_psau(source, rate, looping);
                *results[index].lock().expect("cook worker") = blob;
            });
        }
    });
    results
        .into_iter()
        .map(|slot| slot.into_inner().expect("cook worker"))
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

/// Resident rate per core id. These are the rates the core has always used;
/// the encoder change keeps every core size the same and only changes how
/// the bytes are spent.
fn resident_rate(id: &str, source_rate: u32) -> u32 {
    if matches!(
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
        // The two continuous charger beds are long enough that the ordinary
        // SFX rate would crowd the largest per-map dialogue bank out of SPU
        // RAM. Their mechanical/noise character survives 5 kHz cleanly.
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
    } else if source_rate >= 22_050 {
        11_025
    } else {
        8_000
    }
}

pub fn build_sfx(sound: &Path, output: &Path) -> Result<()> {
    let sources: Vec<Source> = SOUNDS
        .iter()
        .map(|(_, relative)| read_source(&sound.join(relative)))
        .collect::<Result<_>>()?;
    let jobs: Vec<(&Source, u32, bool)> = SOUNDS
        .iter()
        .zip(&sources)
        .map(|((id, _), source)| {
            (
                source,
                resident_rate(id, source.wav.rate),
                // The runtime owns the charger beds through a reserved voice
                // and needs them as hardware loops.
                id.starts_with("charger_"),
            )
        })
        .collect();
    let blobs = cook_parallel(&jobs);
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
    let silent = Source {
        wav: Wav {
            rate: 5_000,
            samples: vec![0.0; 28],
            loop_start: None,
            loop_end: None,
            bits: 16,
        },
        has_loop_metadata: false,
    };
    let silence = cook_psau(&silent, 5_000, false);
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

/// A sentence's WAVs joined at the highest of their authored rates.
fn concat_sources(valve: &Path, wavs: &[String]) -> Result<Option<Source>> {
    let mut parts = Vec::with_capacity(wavs.len());
    for relative in wavs {
        let path = valve.join("sound").join(normalized_sound_path(relative));
        if !path.exists() {
            return Ok(None);
        }
        parts.push(read_source(&path)?);
    }
    if parts.len() == 1 {
        return Ok(parts.pop());
    }
    let rate = parts.iter().map(|p| p.wav.rate).max().unwrap_or(11_025);
    let sinc = Sinc::new();
    let mut samples = Vec::new();
    let mut has_loop_metadata = false;
    for part in &parts {
        has_loop_metadata |= part.has_loop_metadata;
        if part.wav.rate == rate {
            samples.extend_from_slice(&part.wav.samples);
        } else {
            samples.extend(sinc.resample(&part.wav.samples, part.wav.rate, rate));
        }
    }
    Ok(Some(Source {
        wav: Wav {
            rate,
            samples,
            loop_start: None,
            loop_end: None,
            bits: 16,
        },
        has_loop_metadata,
    }))
}

/// Candidate rates, high to low. The allocator picks one per sample.
const RATE_LADDER: [u32; 18] = [
    11_025, 10_000, 9_000, 8_000, 7_000, 6_000, 5_500, 5_000, 4_500, 4_000, 3_600, 3_200, 2_800,
    2_400, 2_000, 1_800, 1_600, 1_400,
];

/// Highest and lowest rate a class may take.
fn class_range(class: MapAudioClass) -> (u32, u32) {
    match class {
        MapAudioClass::Dialogue => (11_025, 2_400),
        MapAudioClass::Chatter => (8_000, 2_400),
        MapAudioClass::OneShot => (11_025, 2_400),
        MapAudioClass::Loop => (8_000, 1_400),
    }
}

/// The rates `entry` may take: the class range, never above the source rate.
fn entry_ladder(class: MapAudioClass, source_rate: u32) -> Vec<u32> {
    let (top, floor) = class_range(class);
    let top = top.min(source_rate);
    let mut ladder: Vec<u32> = RATE_LADDER
        .iter()
        .copied()
        .filter(|&rate| rate < top && rate >= floor)
        .collect();
    ladder.insert(0, top);
    ladder
}

/// Monster vocalisations that are not under a speech directory but are the
/// Nihilanth's voice, weighted like speech.
fn is_monster_voice(path: &str) -> bool {
    let path = normalized_sound_path(path);
    [
        "nihilanth/",
        "x/x_pain",
        "x/x_laugh",
        "x/x_recharge",
        "x/x_attack",
        "x/x_die",
        "x/nih_die",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

/// How much a second of lost quality in `entry` matters to the allocator.
/// Speech carries the story, a monster's voice or attack cue carries gameplay,
/// ambient beds matter least.
fn class_weight(entry: &MapAudioKey) -> f64 {
    match entry.class {
        MapAudioClass::Dialogue => 4.0,
        MapAudioClass::Chatter => 2.0,
        MapAudioClass::OneShot if entry.wavs.iter().any(|w| is_monster_voice(w)) => 4.0,
        MapAudioClass::OneShot => 1.5,
        MapAudioClass::Loop => 1.0,
    }
}

/// Set-piece sounds (hl_format::setpiece_audio) whose class this map places
/// directly or through a monstermaker, as (slot, bank key).
fn setpiece_candidates(map: &Path) -> Result<Vec<(usize, MapAudioKey)>> {
    let mut classes = HashSet::new();
    for entity in bsp_entities(map)? {
        if let Some(class) = entity.get("classname") {
            classes.insert(class.to_ascii_lowercase());
        }
        if entity.get("classname").map(String::as_str) == Some("monstermaker") {
            if let Some(kind) = entity.get("monstertype") {
                classes.insert(kind.to_ascii_lowercase());
            }
        }
    }
    let mut out = Vec::new();
    for (slot, sound) in hl_format::setpiece_audio::SOUNDS.iter().enumerate() {
        if !classes.contains(sound.class) {
            continue;
        }
        let class = if sound.looping {
            MapAudioClass::Loop
        } else {
            MapAudioClass::OneShot
        };
        let mode = if sound.looping { "loop" } else { "shot" };
        out.push((
            slot,
            MapAudioKey {
                key: format!("sfx:{mode}:{}", sound.path),
                wavs: vec![sound.path.to_string()],
                class,
            },
        ));
    }
    Ok(out)
}

/// One bank entry ready for allocation.
struct Planned<'a> {
    entry: &'a MapAudioKey,
    source: Source,
    looping: bool,
    ladder: Vec<u32>,
    bytes: Vec<usize>,
}

fn plan_entry<'a>(valve: &Path, entry: &'a MapAudioKey) -> Result<Planned<'a>> {
    let source = concat_sources(valve, &entry.wavs)?
        .ok_or_else(|| format!("missing map-audio source for {}", entry.key))?;
    let looping = entry.class == MapAudioClass::Loop && source.has_loop_metadata;
    let ladder = entry_ladder(entry.class, source.wav.rate);
    let bytes = ladder
        .iter()
        .map(|&rate| psau_size(&source, rate, looping))
        .collect();
    Ok(Planned {
        entry,
        source,
        looping,
        ladder,
        bytes,
    })
}

fn pack_overhead(entries: usize) -> usize {
    8 + entries * 8 // HSFX header + sample table
}

fn minimum_pack_size(planned: &[Planned<'_>]) -> usize {
    pack_overhead(planned.len())
        + planned
            .iter()
            .map(|p| *p.bytes.last().expect("non-empty ladder"))
            .sum::<usize>()
}

/// Core ids only particular entities can emit, from the runtime's call sites
/// (game/src/main.rs: `prop_voice` per prop kind, the houndeye blast and the
/// wall chargers). Everything else in the core (weapons, player, items, HEV,
/// impacts, the headcrab-family attack bark some leapers share) is always
/// reachable, and so are Barney's sounds: a following guard can be carried
/// through any number of changelevels.
const CORE_EMITTERS: [(&[usize], &[&str]); 8] = [
    (&[30, 31], &["monster_headcrab"]),
    (&[20, 29], &["monster_zombie"]),
    (&[21, 36, 37], &["monster_houndeye"]),
    (&[40, 41], &["monster_bullchicken", "monster_ichthyosaur"]),
    (&[32, 33], &["monster_human_grunt"]),
    (
        &[38, 39],
        &[
            "monster_alien_slave",
            "monster_alien_grunt",
            "monster_alien_controller",
        ],
    ),
    (&[24, 47, 55], &["func_healthcharger"]),
    (&[48, 56], &["func_recharge"]),
];
/// Size of the map's chapter core (3000/3050/3051) before census profiles:
/// the budget the previous cooker had.
fn resident_core_bytes(sfx_dir: &Path, map_index: usize) -> usize {
    fs::metadata(sfx_dir.join(format!("chunk_{}.psxa", resident_core_chunk(map_index))))
        .map(|m| m.len() as usize)
        .unwrap_or(414 * 1024)
}

/// Chunk of the per-map core table the runtime reads at boot.
const CORE_TABLE_CHUNK: usize = 3052;
/// Chunk ids available to deduplicated core profiles.
const CORE_PROFILE_CHUNKS: std::ops::RangeInclusive<usize> = 3053..=3099;

/// Entity classes a map places, directly or through a monstermaker.
fn map_classes(map: &Path) -> Result<HashSet<String>> {
    let mut classes = HashSet::new();
    for entity in bsp_entities(map)? {
        if let Some(class) = entity.get("classname") {
            classes.insert(class.to_ascii_lowercase());
        }
        if entity.get("classname").map(String::as_str) == Some("monstermaker") {
            if let Some(kind) = entity.get("monstertype") {
                classes.insert(kind.to_ascii_lowercase());
            }
        }
    }
    Ok(classes)
}

/// Core ids none of the map's entities can emit.
fn unreachable_core_ids(classes: &HashSet<String>) -> Vec<usize> {
    let mut ids: Vec<usize> = CORE_EMITTERS
        .iter()
        .filter(|(_, emitters)| !emitters.iter().any(|class| classes.contains(*class)))
        .flat_map(|(ids, _)| ids.iter().copied())
        .collect();
    ids.sort_unstable();
    ids
}

fn hsfx_entries(pack: &[u8]) -> Result<Vec<Vec<u8>>> {
    if pack.len() < 8 || &pack[..4] != b"HSFX" {
        return Err("core SFX pack is not HSFX".into());
    }
    let word = |at: usize| -> Result<usize> {
        Ok(u32::from_le_bytes(pack.get(at..at + 4).ok_or("short HSFX")?.try_into()?) as usize)
    };
    (0..word(4)?)
        .map(|i| {
            let (offset, len) = (word(8 + i * 8)?, word(12 + i * 8)?);
            Ok(pack
                .get(offset..offset + len)
                .ok_or("HSFX entry out of range")?
                .to_vec())
        })
        .collect()
}

/// The core bank each map loads: its chapter profile (3000/3050/3051) with
/// the sounds its entities cannot emit replaced by one silent block, so ids
/// stay stable and the SPU RAM goes to the map's own bank. Identical
/// profiles share a chunk, so walking between maps with the same census does
/// not reload the core. Writes the profiles and the table into `sfx_dir` and
/// returns (chunk id, bytes) per map index.
fn build_core_profiles(
    valve: &Path,
    map_list: &str,
    sfx_dir: &Path,
) -> Result<Vec<(usize, usize)>> {
    for entry in fs::read_dir(sfx_dir)? {
        let path = entry?.path();
        let chunk = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("chunk_"))
            .and_then(|s| s.strip_suffix(".psxa"))
            .and_then(|s| s.parse::<usize>().ok());
        if chunk.is_some_and(|c| c == CORE_TABLE_CHUNK || CORE_PROFILE_CHUNKS.contains(&c)) {
            fs::remove_file(path)?;
        }
    }
    let silent = Source {
        wav: Wav {
            rate: 5_000,
            samples: vec![0.0; 28],
            loop_start: None,
            loop_end: None,
            bits: 16,
        },
        has_loop_metadata: false,
    };
    let silence = cook_psau(&silent, 5_000, false);
    let mut bases: HashMap<usize, Vec<Vec<u8>>> = HashMap::new();
    let maps: Vec<&str> = map_list.split_whitespace().collect();
    // Per map: the classes it can hold, its own plus those of every map one
    // changelevel away in either direction (monsters standing in a
    // transition volume are carried across it).
    let mut own: Vec<HashSet<String>> = Vec::with_capacity(maps.len());
    let mut links: Vec<HashSet<String>> = Vec::with_capacity(maps.len());
    for map_name in &maps {
        let bsp = valve.join("maps").join(format!("{map_name}.bsp"));
        if !bsp.exists() {
            own.push(HashSet::new());
            links.push(HashSet::new());
            continue;
        }
        own.push(map_classes(&bsp)?);
        links.push(
            bsp_entities(&bsp)?
                .iter()
                .filter(|e| e.get("classname").map(String::as_str) == Some("trigger_changelevel"))
                .filter_map(|e| e.get("map").map(|m| m.to_ascii_lowercase()))
                .collect(),
        );
    }
    let census: Vec<HashSet<String>> = (0..maps.len())
        .map(|i| {
            let mut classes = own[i].clone();
            for (j, name) in maps.iter().enumerate() {
                if links[i].contains(&name.to_ascii_lowercase())
                    || links[j].contains(&maps[i].to_ascii_lowercase())
                {
                    classes.extend(own[j].iter().cloned());
                }
            }
            classes
        })
        .collect();
    // Per map: its chapter base and the core ids it could silence.
    let mut wants: Vec<(usize, Vec<usize>)> = Vec::with_capacity(maps.len());
    for (map_index, map_name) in maps.iter().enumerate() {
        let base = resident_core_chunk(map_index) as usize;
        let base_path = sfx_dir.join(format!("chunk_{base}.psxa"));
        let bsp = valve.join("maps").join(format!("{map_name}.bsp"));
        if !bsp.exists() || !base_path.exists() {
            wants.push((base, Vec::new()));
            continue;
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = bases.entry(base) {
            slot.insert(hsfx_entries(&fs::read(&base_path)?)?);
        }
        let entries = &bases[&base];
        let silenced = unreachable_core_ids(&census[map_index])
            .into_iter()
            .filter(|&id| {
                entries
                    .get(id)
                    .is_some_and(|blob| blob.len() > silence.len())
            })
            .collect();
        wants.push((base, silenced));
    }
    let saved = |base: usize, ids: &[usize]| -> usize {
        ids.iter()
            .map(|&id| bases[&base][id].len() - silence.len())
            .sum()
    };
    // Candidate profiles are the distinct masks. A map may use any profile of
    // its base that silences a subset of what it can silence; keep the
    // profiles that save the most bytes summed over the maps that could use
    // them, as many as there are chunk ids.
    let mut candidates: Vec<(usize, Vec<usize>)> = Vec::new();
    for want in &wants {
        if !want.1.is_empty() && !candidates.contains(want) {
            candidates.push(want.clone());
        }
    }
    let usable = |profile: &(usize, Vec<usize>), want: &(usize, Vec<usize>)| {
        profile.0 == want.0 && profile.1.iter().all(|id| want.1.contains(id))
    };
    let mut value: Vec<(usize, usize)> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let maps_using = wants.iter().filter(|w| usable(c, w)).count();
            (saved(c.0, &c.1) * maps_using, i)
        })
        .collect();
    value.sort_by(|a, b| b.cmp(a));
    let chosen: Vec<(usize, Vec<usize>)> = value
        .iter()
        .take(CORE_PROFILE_CHUNKS.clone().count())
        .map(|&(_, i)| candidates[i].clone())
        .collect();
    let mut written: Vec<Option<(usize, usize)>> = vec![None; chosen.len()];
    let mut table = Vec::with_capacity(maps.len());
    for (map_index, want) in wants.iter().enumerate() {
        let base = want.0;
        let base_bytes = fs::metadata(sfx_dir.join(format!("chunk_{base}.psxa")))
            .map(|m| m.len() as usize)
            .unwrap_or(414 * 1024);
        let best = chosen
            .iter()
            .enumerate()
            .filter(|(_, c)| usable(c, want))
            .max_by_key(|(i, c)| (saved(c.0, &c.1), std::cmp::Reverse(*i)));
        let Some((slot, profile)) = best else {
            table.push((base, base_bytes));
            continue;
        };
        if written[slot].is_none() {
            let chunk = CORE_PROFILE_CHUNKS
                .clone()
                .nth(slot)
                .expect("chosen fits the id range");
            let blobs: Vec<Vec<u8>> = bases[&base]
                .iter()
                .enumerate()
                .map(|(id, blob)| {
                    if profile.1.contains(&id) {
                        silence.clone()
                    } else {
                        blob.clone()
                    }
                })
                .collect();
            let pack = hsfx(&blobs);
            fs::write(sfx_dir.join(format!("chunk_{chunk}.psxa")), &pack)?;
            println!(
                "  core profile {chunk} (from {base}, first {}): {} B, silences ids {:?}",
                maps[map_index],
                pack.len(),
                profile.1
            );
            written[slot] = Some((chunk, pack.len()));
        }
        table.push(written[slot].expect("written above"));
    }
    let mut bytes = Vec::with_capacity(8 + table.len() * 2);
    bytes.extend_from_slice(b"HCPT");
    bytes.extend_from_slice(&(table.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    for (chunk, _) in &table {
        bytes.extend_from_slice(&(*chunk as u16).to_le_bytes());
    }
    fs::write(
        sfx_dir.join(format!("chunk_{CORE_TABLE_CHUNK}.psxa")),
        bytes,
    )?;
    Ok(table)
}

/// The previous cooker's per-class ladders, kept only to compute the
/// no-regression reference below.
fn previous_class_rates(class: MapAudioClass) -> &'static [u32] {
    match class {
        MapAudioClass::Dialogue => &[11_025, 8_000, 6_000, 5_000, 4_000, 3_200, 2_800, 2_400],
        MapAudioClass::Chatter | MapAudioClass::OneShot => {
            &[8_000, 6_000, 5_000, 4_000, 3_200, 2_800, 2_400]
        }
        MapAudioClass::Loop => &[
            5_000, 4_000, 3_500, 3_000, 2_500, 2_000, 1_800, 1_600, 1_400,
        ],
    }
}

/// The rate each entry would have had under the previous cooker (hl-psx
/// final-5/final-6): same budget rule, per-class ladders, and "largest
/// saving first, loops then shots then chatter then dialogue". Sizes only,
/// nothing is encoded. `None` when that cooker could not fit the entries.
fn previous_rates(planned: &[&Planned<'_>], budget: usize) -> Option<Vec<u32>> {
    let size = |p: &Planned<'_>, rate: u32| {
        let count = (p.source.wav.samples.len() as u64 * rate as u64
            / p.source.wav.rate.max(1) as u64)
            .max(1) as usize;
        32 + count.div_ceil(28) * 16
    };
    let ladders: Vec<&[u32]> = planned
        .iter()
        .map(|p| previous_class_rates(p.entry.class))
        .collect();
    let mut steps = vec![0usize; planned.len()];
    let total = |steps: &[usize]| -> usize {
        pack_overhead(planned.len())
            + planned
                .iter()
                .zip(steps)
                .zip(&ladders)
                .map(|((p, &s), l)| size(p, l[s]))
                .sum::<usize>()
    };
    while total(&steps) > budget {
        let mut selected = None;
        for class in [
            MapAudioClass::Loop,
            MapAudioClass::OneShot,
            MapAudioClass::Chatter,
            MapAudioClass::Dialogue,
        ] {
            let mut best_saving = 0usize;
            for (i, p) in planned.iter().enumerate() {
                if p.entry.class != class || steps[i] + 1 >= ladders[i].len() {
                    continue;
                }
                let saving =
                    size(p, ladders[i][steps[i]]).saturating_sub(size(p, ladders[i][steps[i] + 1]));
                if saving > best_saving {
                    best_saving = saving;
                    selected = Some(i);
                }
            }
            if selected.is_some() {
                break;
            }
        }
        steps[selected?] += 1;
    }
    Some(
        planned
            .iter()
            .enumerate()
            .map(|(i, _)| ladders[i][steps[i]])
            .collect(),
    )
}

/// fwSNRseg (psx_audio_cook::metrics) of `source` played back after cooking
/// at `rate`, by the previous pipeline (`legacy`) or the current one. Loops
/// are measured as one-shots (a whole loop is stretched by under half a
/// block, which only misaligns it against the reference).
fn playback_quality(source: &Source, rate: u32, legacy: bool) -> f64 {
    let wav = &source.wav;
    let cooked = if legacy {
        let pcm: Vec<i16> = wav
            .samples
            .iter()
            .map(|&v| v.round().clamp(-32_768.0, 32_767.0) as i16)
            .collect();
        let (pcm, adpcm) = psx_audio_cook::legacy::hl_cook(&pcm, wav.rate, rate, 0.9);
        psx_audio_cook::Cooked {
            rate,
            pcm,
            adpcm,
            loop_block: None,
        }
    } else {
        psx_audio_cook::cook(wav, &CookOptions::one_shot(rate))
    };
    let reference = psx_audio_cook::reference_44k(wav);
    let played = psx_audio_cook::playback(&cooked);
    let n = reference.len().min(played.len());
    psx_audio_cook::metrics::fw_snr_seg_db(
        &reference[..n],
        &played[..n],
        (wav.rate as f64 / 2.0).min(11_025.0),
    )
}

/// Lowest rate on `p`'s ladder whose measured playback quality is at least
/// the previous pipeline's at `previous` Hz (the previous rate itself when
/// none is). A rate, not a step: one sound can sit on different ladders in
/// different maps (a loop in one, a one-shot in another).
fn no_regression_rate(p: &Planned<'_>, previous: u32) -> u32 {
    let target = playback_quality(&p.source, previous, true);
    let mut floor = p
        .ladder
        .iter()
        .position(|&rate| rate <= previous)
        .unwrap_or(p.ladder.len() - 1);
    while floor + 1 < p.ladder.len()
        && playback_quality(&p.source, p.ladder[floor + 1], false) >= target
    {
        floor += 1;
    }
    p.ladder[floor]
}

pub fn build_voices(valve: &Path, map_list: &str, output: &Path) -> Result<()> {
    const RUNTIME_MAX_MAP_AUDIO: usize = 96;
    fs::create_dir_all(output)?;
    for entry in fs::read_dir(output)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with("chunk_") || name == "manifest.txt" || name == "rates.txt" {
            fs::remove_file(path)?;
        }
    }
    let sfx_dir = output.parent().unwrap_or(output).join("sfx");
    let core_profiles = build_core_profiles(valve, map_list, &sfx_dir)?;
    let sentences = load_sentences(valve)?;
    let mut manifest = Vec::new();
    let mut rates_report = vec![String::from(
        "map|id|key|class|rate_hz|bytes|seconds|predicted_loss_db|wavs",
    )];
    // Band loss depends only on the source, and blobs only on (source, rate,
    // loop); many maps share both.
    let mut loss_cache: HashMap<String, Vec<(u32, f64)>> = HashMap::new();
    let mut blob_cache: HashMap<(String, u32, bool), Vec<u8>> = HashMap::new();
    let mut floor_cache: HashMap<(String, u32), u32> = HashMap::new();
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
        let (core_chunk, core_bytes) = core_profiles[map_index];
        let budget = 512 * 1024 - 0x1010 - core_bytes - 4096;
        let mut planned: Vec<Planned<'_>> = Vec::new();
        for entry in &keys {
            let present = entry.wavs.iter().all(|relative| {
                valve
                    .join("sound")
                    .join(normalized_sound_path(relative))
                    .is_file()
            });
            if present {
                planned.push(plan_entry(valve, entry)?);
            }
        }
        if planned.is_empty() {
            return Err(format!("{map_name}: no map-audio samples could be encoded").into());
        }
        if keys
            .iter()
            .filter(|entry| entry.key.starts_with("use:"))
            .count()
            != planned
                .iter()
                .filter(|p| p.entry.key.starts_with("use:"))
                .count()
        {
            return Err(format!("{map_name}: missing audio for an NPC use reply").into());
        }

        // The complete HEV logon is deliberately preferred where it fits,
        // but some combat maps combine spare suits with far more mandatory
        // authored dialogue than the SPU can hold. Only omit the logon when
        // the pack would exceed the budget even with every sample at its
        // minimum rate; the runtime then uses its resident activation line.
        if minimum_pack_size(&planned) > budget {
            if let Some(index) = planned
                .iter()
                .position(|p| matches!(p.entry.key.as_str(), "HEV_AAX" | "HEV_A0"))
            {
                println!(
                    "  {map_name}: full HEV logon cannot fit beside mandatory map audio; using resident pickup line"
                );
                planned.remove(index);
            }
        }

        // Direct-use replies must remain available. On the few maps whose
        // mandatory speech already fills SPU RAM, omit autonomous small talk
        // before sacrificing a scripted line or an interaction response.
        if minimum_pack_size(&planned) > budget {
            planned.retain(|p| p.entry.class != MapAudioClass::Chatter);
            println!(
                "  {map_name}: reserving speech bank for authored and player-triggered dialogue"
            );
        }

        // Set-piece monster sounds all join the bank and compete for rate
        // like everything else. One is dropped only when the bank cannot hold
        // it even with every sample at its lowest rate, the highest tier (the
        // least gameplay information) first.
        let setpiece = setpiece_candidates(&bsp)?;
        let mut setpiece_kept: Vec<String> = Vec::new();
        let mut setpiece_dropped: Vec<String> = Vec::new();
        let mut joined: Vec<(u8, String)> = Vec::new();
        for (slot, entry) in setpiece.iter() {
            let path = &entry.wavs[0];
            if planned.iter().any(|p| p.entry.key == entry.key) {
                setpiece_kept.push(format!("{path} (shared)"));
                continue;
            }
            if !valve
                .join("sound")
                .join(normalized_sound_path(path))
                .is_file()
            {
                setpiece_dropped.push(format!("{path} (missing)"));
                continue;
            }
            planned.push(plan_entry(valve, entry)?);
            joined.push((
                hl_format::setpiece_audio::SOUNDS[*slot].tier,
                entry.key.clone(),
            ));
        }
        while minimum_pack_size(&planned) > budget || planned.len() > RUNTIME_MAX_MAP_AUDIO {
            let Some(victim) = joined
                .iter()
                .enumerate()
                .max_by_key(|(order, (tier, _))| (*tier, *order))
                .map(|(order, _)| order)
            else {
                break;
            };
            let (_, key) = joined.remove(victim);
            if let Some(index) = planned.iter().position(|p| p.entry.key == key) {
                setpiece_dropped.push(planned[index].entry.wavs[0].clone());
                planned.remove(index);
            }
        }
        for (_, key) in &joined {
            if let Some(p) = planned.iter().find(|p| &p.entry.key == key) {
                setpiece_kept.push(p.entry.wavs[0].clone());
            }
        }

        // Rates: minimise the time-weighted quality loss for the SPU bytes
        // available (psx_audio_cook::rate::allocate). Each sample's loss per
        // rate comes from its own spectrum, so a deep voice or a rumble goes
        // low before a sibilant line or a hiss does, and long lines are not
        // starved to save the most bytes per step.
        // The previous cooker left the full HEV logon out of maps whose other
        // speech filled the bank. Keep that choice when including it would
        // push the map's other sounds below their previous quality.
        let (candidates, steps, relaxed) = loop {
            let mut candidates = Vec::with_capacity(planned.len());
            for p in &planned {
                let cache_key = p.entry.wavs.join("|");
                let losses = loss_cache.entry(cache_key).or_insert_with(|| {
                    let rates: Vec<u32> = RATE_LADDER
                        .iter()
                        .copied()
                        .chain(std::iter::once(p.source.wav.rate.min(11_025)))
                        .collect();
                    let loss = psx_audio_cook::rate::band_loss(
                        &p.source.wav.samples,
                        p.source.wav.rate,
                        &rates,
                    );
                    rates.into_iter().zip(loss).collect()
                });
                let loss: Vec<f64> = p
                    .ladder
                    .iter()
                    .map(|rate| {
                        losses
                            .iter()
                            .find(|(r, _)| r == rate)
                            .map(|(_, l)| *l)
                            .unwrap_or(0.0)
                    })
                    .collect();
                candidates.push(psx_audio_cook::rate::Candidate {
                    bytes: p.bytes.clone(),
                    loss,
                    weight: class_weight(p.entry) * p.source.seconds(),
                    max_step: p.ladder.len() - 1,
                });
            }
            // No regressions: an entry the previous cooker also placed keeps at
            // least the playback quality it had there (measured, both pipelines,
            // only for entries the allocation would put below their previous
            // rate). The reference is the previous policy without set pieces
            // (final-5) for the map's own sounds, and with its tier-1 set pieces
            // (final-6) for those. Floors that cannot all fit are relaxed loops
            // first, then chatter, then effects; speech keeps its floor longest.
            let is_setpiece = |p: &Planned<'_>| joined.iter().any(|(_, key)| key == &p.entry.key);
            let own: Vec<&Planned<'_>> = planned.iter().filter(|p| !is_setpiece(p)).collect();
            let tier1: Vec<&Planned<'_>> = planned
                .iter()
                .filter(|p| {
                    !is_setpiece(p)
                        || joined
                            .iter()
                            .any(|(tier, key)| *tier == 1 && key == &p.entry.key)
                })
                .collect();
            let base_budget = 512 * 1024 - 0x1010 - resident_core_bytes(&sfx_dir, map_index) - 4096;
            let mut previous: Vec<Option<u32>> = vec![None; planned.len()];
            for (subset, only_setpieces) in [(&own, false), (&tier1, true)] {
                // The previous cooker dropped the HEV logon, then chatter, from
                // maps it could not otherwise fit; so does its reference.
                let mut subset: Vec<&Planned<'_>> = subset.to_vec();
                let mut rates = previous_rates(&subset, base_budget);
                if rates.is_none() {
                    subset.retain(|p| !matches!(p.entry.key.as_str(), "HEV_AAX" | "HEV_A0"));
                    rates = previous_rates(&subset, base_budget);
                }
                if rates.is_none() {
                    subset.retain(|p| p.entry.class != MapAudioClass::Chatter);
                    rates = previous_rates(&subset, base_budget);
                }
                let subset = &subset;
                if let Some(rates) = rates {
                    for (p, rate) in subset.iter().zip(rates) {
                        if is_setpiece(p) == only_setpieces {
                            let i = planned
                                .iter()
                                .position(|q| std::ptr::eq(q, *p))
                                .expect("member");
                            previous[i] = Some(rate);
                        }
                    }
                }
            }
            let overhead = pack_overhead(planned.len());
            let mut floors: Vec<Option<usize>> = vec![None; planned.len()];
            let mut steps = psx_audio_cook::rate::allocate(&candidates, overhead, budget);
            let mut relaxed = Vec::new();
            while let Some(current) = steps.clone() {
                let mut added = false;
                for i in 0..planned.len() {
                    if let (None, Some(prev)) = (floors[i], previous[i]) {
                        if planned[i].ladder[current[i]] < prev {
                            let key = (planned[i].entry.wavs.join("|"), prev);
                            let rate = *floor_cache
                                .entry(key)
                                .or_insert_with(|| no_regression_rate(&planned[i], prev));
                            let step = planned[i]
                                .ladder
                                .iter()
                                .position(|&r| r <= rate)
                                .unwrap_or(planned[i].ladder.len() - 1);
                            floors[i] = Some(step);
                            added = true;
                        }
                    }
                }
                if !added {
                    break;
                }
                let mut attempt = candidates.clone();
                for (c, floor) in attempt.iter_mut().zip(&floors) {
                    if let Some(f) = floor {
                        c.max_step = *f;
                    }
                }
                let mut result = psx_audio_cook::rate::allocate(&attempt, overhead, budget);
                for class in [
                    MapAudioClass::Loop,
                    MapAudioClass::Chatter,
                    MapAudioClass::OneShot,
                    MapAudioClass::Dialogue,
                ] {
                    if result.is_some() {
                        break;
                    }
                    for (i, c) in attempt.iter_mut().enumerate() {
                        if planned[i].entry.class == class {
                            c.max_step = planned[i].ladder.len() - 1;
                            floors[i] = Some(c.max_step);
                        }
                    }
                    relaxed.push(class);
                    result = psx_audio_cook::rate::allocate(&attempt, overhead, budget);
                }
                steps = result;
            }
            let logon = planned
                .iter()
                .position(|p| matches!(p.entry.key.as_str(), "HEV_AAX" | "HEV_A0"));
            if let (false, Some(i)) = (relaxed.is_empty(), logon) {
                if previous[i].is_none() {
                    println!(
                        "  {map_name}: full HEV logon would cost other lines their quality; using resident pickup line"
                    );
                    planned.remove(i);
                    continue;
                }
            }
            break (candidates, steps, relaxed);
        };
        if !relaxed.is_empty() {
            println!(
                "  {map_name}: no-regression floors relaxed for {} class(es)",
                relaxed.len()
            );
        }
        let Some(steps) = steps else {
            let class_bytes = |class| {
                planned
                    .iter()
                    .filter(|p| p.entry.class == class)
                    .map(|p| *p.bytes.last().unwrap())
                    .sum::<usize>()
            };
            return Err(format!(
                "{map_name}: map-audio pack is {} bytes at minimum per-sample rates; SPU budget is {budget} (dialogue {}, chatter {}, shots {}, loops {})",
                minimum_pack_size(&planned),
                class_bytes(MapAudioClass::Dialogue),
                class_bytes(MapAudioClass::Chatter),
                class_bytes(MapAudioClass::OneShot),
                class_bytes(MapAudioClass::Loop),
            )
            .into());
        };
        if !setpiece_kept.is_empty() || !setpiece_dropped.is_empty() {
            println!(
                "  {map_name}: set-piece audio kept [{}] dropped [{}]",
                setpiece_kept.join(", "),
                setpiece_dropped.join(", ")
            );
        }

        let rates: Vec<u32> = planned
            .iter()
            .zip(&steps)
            .map(|(p, &step)| p.ladder[step])
            .collect();
        let blob_key = |p: &Planned<'_>, rate: u32| (p.entry.wavs.join("|"), rate, p.looping);
        // One job per distinct (sound, rate, loop): a map can list the same
        // sound under two keys (an NPC use reply and its sentence).
        let mut wanted: Vec<((String, u32, bool), &Planned<'_>)> = Vec::new();
        for (p, &rate) in planned.iter().zip(&rates) {
            let key = blob_key(p, rate);
            if !blob_cache.contains_key(&key) && !wanted.iter().any(|(k, _)| *k == key) {
                wanted.push((key, p));
            }
        }
        let jobs: Vec<(&Source, u32, bool)> = wanted
            .iter()
            .map(|((_, rate, looping), p)| (&p.source, *rate, *looping))
            .collect();
        for ((key, _), blob) in wanted.iter().zip(cook_parallel(&jobs)) {
            blob_cache.insert(key.clone(), blob);
        }
        let blobs: Vec<Vec<u8>> = planned
            .iter()
            .zip(&rates)
            .map(|(p, &rate)| blob_cache[&blob_key(p, rate)].clone())
            .collect();
        for ((p, &rate), blob) in planned.iter().zip(&rates).zip(&blobs) {
            let cooked_rate = u32::from_le_bytes(blob[16..20].try_into()?);
            if cooked_rate != rate || blob.len() != psau_size(&p.source, rate, p.looping) {
                return Err(format!("{map_name}: {} cooked out of order", p.entry.key).into());
            }
        }
        let pack = hsfx(&blobs);
        let predicted = pack_overhead(planned.len())
            + planned
                .iter()
                .zip(&steps)
                .map(|(p, &s)| p.bytes[s])
                .sum::<usize>();
        if pack.len() != predicted || pack.len() > budget {
            return Err(format!(
                "{map_name}: predicted {predicted} byte pack encoded to {} bytes (SPU budget {budget})",
                pack.len()
            )
            .into());
        }
        let chunk = 3100 + map_index;
        fs::write(output.join(format!("chunk_{chunk}.psxa")), &pack)?;
        manifest.extend(
            planned
                .iter()
                .enumerate()
                .map(|(id, p)| format!("{map_index}|{id}|{}", p.entry.key)),
        );
        for (id, ((p, &rate), (c, &step))) in planned
            .iter()
            .zip(&rates)
            .zip(candidates.iter().zip(&steps))
            .enumerate()
        {
            let class = match p.entry.class {
                MapAudioClass::Dialogue => "dialogue",
                MapAudioClass::Chatter => "chatter",
                MapAudioClass::OneShot => "shot",
                MapAudioClass::Loop => "loop",
            };
            rates_report.push(format!(
                "{map_name}|{id}|{}|{class}|{rate}|{}|{:.2}|{:.2}|{}",
                p.entry.key,
                c.bytes[step],
                p.source.seconds(),
                c.loss[step],
                p.entry.wavs.join("+")
            ));
        }
        let count = |class| planned.iter().filter(|p| p.entry.class == class).count();
        let range = |class| {
            let mut class_rates = planned
                .iter()
                .zip(&rates)
                .filter(|(p, _)| p.entry.class == class)
                .map(|(_, &rate)| rate);
            let Some(first) = class_rates.next() else {
                return String::from("-");
            };
            let (mut lo, mut hi) = (first, first);
            for rate in class_rates {
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
            "  {map_name} (idx {map_index}, chunk {chunk}, core {core_chunk}): {} dialogue @{} + {} chatter @{} + {} shot @{} + {} loop @{} Hz, {} KB",
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
    fs::write(output.join("rates.txt"), rates_report.join("\n") + "\n")?;
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
    fn predicted_psau_size_matches_the_cooked_blob() {
        for (rate, looping) in [
            (11_025, false),
            (6_000, false),
            (1_400, true),
            (5_000, true),
        ] {
            let source = Source {
                wav: Wav {
                    rate: 22_050,
                    samples: (0..9_001)
                        .map(|i| ((i * 37) % 2_000) as f64 - 1_000.0)
                        .collect(),
                    loop_start: None,
                    loop_end: None,
                    bits: 8,
                },
                has_loop_metadata: looping,
            };
            assert_eq!(
                cook_psau(&source, rate, looping).len(),
                psau_size(&source, rate, looping)
            );
        }
    }

    #[test]
    fn map_loops_become_whole_block_hardware_loops() {
        let source = Source {
            wav: Wav {
                rate: 11_025,
                samples: (0..5_000)
                    .map(|i| ((i % 50) as f64 - 25.0) * 400.0)
                    .collect(),
                loop_start: None,
                loop_end: None,
                bits: 16,
            },
            has_loop_metadata: true,
        };
        let blob = cook_psau(&source, 1_400, true);
        let adpcm = &blob[32..];
        let blocks = adpcm.len() / 16;
        assert_eq!(adpcm[1] & 0x07, 0x04, "loop starts on the first block");
        assert_eq!(adpcm[0] >> 4, 0, "the re-entry block ignores history");
        assert_eq!(adpcm[(blocks - 1) * 16 + 1] & 0x07, 0x03);
        let count = u32::from_le_bytes(blob[20..24].try_into().unwrap()) as usize;
        assert_eq!(count, blocks * 28, "no zero padding plays at the seam");
    }

    #[test]
    fn entry_ladders_respect_class_range_and_source_rate() {
        assert_eq!(entry_ladder(MapAudioClass::Dialogue, 22_050)[0], 11_025);
        assert_eq!(
            *entry_ladder(MapAudioClass::Dialogue, 22_050)
                .last()
                .unwrap(),
            2_400
        );
        assert_eq!(entry_ladder(MapAudioClass::Loop, 11_025)[0], 8_000);
        assert_eq!(
            *entry_ladder(MapAudioClass::Loop, 11_025).last().unwrap(),
            1_400
        );
        let low = entry_ladder(MapAudioClass::OneShot, 5_512);
        assert_eq!(low[0], 5_512);
        assert!(low.windows(2).all(|w| w[0] > w[1]));
    }

    #[test]
    fn nihilanth_vocalisations_weigh_like_speech() {
        let key = |path: &str| MapAudioKey {
            key: format!("sfx:shot:{path}"),
            wavs: vec![path.to_string()],
            class: MapAudioClass::OneShot,
        };
        assert!(class_weight(&key("x/x_die1.wav")) > class_weight(&key("x/x_ballattack1.wav")));
        assert!(
            class_weight(&key("nihilanth/nil_thetruth.wav"))
                > class_weight(&key("debris/bustglass1.wav"))
        );
    }

    #[test]
    fn core_census_keeps_what_a_map_can_emit() {
        let classes: HashSet<String> = ["monster_alien_slave", "func_healthcharger"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let silenced = unreachable_core_ids(&classes);
        for kept in [24, 38, 39, 47, 55, 34, 35, 64, 0, 12, 19] {
            assert!(!silenced.contains(&kept), "id {kept} must stay");
        }
        for gone in [20, 21, 29, 30, 31, 32, 33, 36, 37, 40, 41, 48, 56] {
            assert!(silenced.contains(&gone), "id {gone} is unreachable");
        }
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

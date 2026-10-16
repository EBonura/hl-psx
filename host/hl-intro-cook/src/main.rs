//! Encodes the movie Half-Life plays at startup for the intro program.
//!
//! The game's own `media/StartupVids.txt` names it (one `media/<file>` line);
//! the cook reads that file from the user's install, encodes it with
//! `psxavenc` as a 320x240, 15 frames a second, bitstream version 2 STR with
//! interleaved XA-ADPCM audio, then decodes every frame once with the SDK's
//! bitstream decoder and writes what the guest has to size its buffers for.
//! Nothing it writes belongs in the repository: the movie and everything
//! derived from it stay under `.hlpsx/intro/` beside the other build output.
//!
//! ```text
//! hl-intro-cook --valve DIR --out DIR [--psxavenc PATH]
//! ```
//!
//! Writes `INTRO.STR` (raw 2336-byte sectors for `mkisopsx --xa-file`) and
//! `movie.rs` (constants the intro program includes). `psxavenc` comes from
//! `--psxavenc`, then `PSXAVENC`, then the `PATH`. Exit status 3 means the
//! install has no startup movie or the encoder is missing, which is not an
//! error for the caller: the disc is then built without an intro.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod limiter;
mod loudness;
mod mux;
mod xa;

use psx_fmv::bitstream as bs;
use psx_fmv::stream::{Chunk, FrameAssembler, CHUNK_PAYLOAD_BYTES, MAX_CHUNKS};

/// Raw XA sector record: 8-byte subheader and 2328 bytes.
const XA_SECTOR: usize = 2336;
const SUBMODE_AUDIO: u8 = 0x04;
const SUBMODE_FORM2: u8 = 0x20;

/// Picture and rate the intro plays at.
const WIDTH: u16 = 320;
const HEIGHT: u16 = 240;
const FPS: &str = "15";
/// XA file and channel the movie's audio is filed under (psxavenc `-F`, `-C`).
const XA_FILE: u8 = 1;
const XA_CHANNEL: u8 = 0;
/// Exit status for "no intro on this machine".
const NO_INTRO: u8 = 3;

/// Sample rate of the movie's sound on the disc.
const SOUND_HZ: f64 = 37_800.0;
/// VBlanks from a picture's last sector to its flip, as the intro asks the
/// player (`Config::latency_vblanks`); written to `movie.rs`.
const LATENCY_VBLANKS: u32 = 5;
/// How much quieter the player's sound path is than the stream it is fed:
/// the CD sound reaches the output through the SPU's main volume, whose
/// full setting (0x3FFF) the emulator mixes at half scale (6.02 dB), and the
/// 37.8 to 44.1 kHz conversion costs about 0.9 dB more. Measured on the SPU
/// output of the headless emulator against the install's movie (-23.7 LUFS
/// heard, -16.8 LUFS for the original played to both speakers). The cook
/// makes the stream this much louder than the install's movie, with a
/// limiter so that it cannot clip, so that what is heard matches it.
const PLAYBACK_LOSS_DB: f64 = 6.9;
/// Sectors the sound is moved later behind the encoder's placement, so that
/// it plays together with the picture. The picture shows its frame the
/// headroom of [`LATENCY_VBLANKS`] after the frame's last sector arrives, and
/// a frame is read in 10 sectors (150 a second, 15 frames a second), so the
/// sound has to wait 10 sectors plus 5 / 59.94 s (12.5 sectors) behind where
/// the encoder starts it; 1.5 more sectors (10 ms) are what the first frame's
/// decode and the flip to the next VBlank add, measured on the emulator.
const AUDIO_DELAY_SECTORS: usize = 24;

#[derive(Debug, Default, PartialEq, Eq)]
struct Movie {
    frames: u32,
    width: u16,
    height: u16,
    max_chunks: u16,
    rle_words: usize,
    xa: Option<(u8, u8)>,
    video_sectors: u32,
    audio_sectors: u32,
    other_sectors: u32,
    versions: [u32; 4],
}

/// Decodes every frame of `raw` (2336-byte sectors) and returns the figures
/// the guest needs, or the first reason the player could not show it.
fn measure(raw: &[u8]) -> Result<Movie, String> {
    if raw.is_empty() || !raw.len().is_multiple_of(XA_SECTOR) {
        return Err(format!(
            "{} bytes is not a whole number of {XA_SECTOR}-byte sectors",
            raw.len()
        ));
    }
    let mut m = Movie::default();
    let mut asm = FrameAssembler::new();
    let mut frame = vec![0u8; MAX_CHUNKS as usize * CHUNK_PAYLOAD_BYTES];
    let mut rle = vec![0u16; 1 << 17];
    for (i, sector) in raw.chunks_exact(XA_SECTOR).enumerate() {
        let (sub, data) = sector.split_at(8);
        let submode = sub[2];
        if submode & SUBMODE_AUDIO != 0 {
            if submode & SUBMODE_FORM2 == 0 {
                return Err(format!("sector {i}: an audio sector outside Form 2"));
            }
            let at = (sub[0], sub[1]);
            match m.xa {
                None => m.xa = Some(at),
                Some(xa) if xa != at => {
                    return Err(format!("sector {i}: audio from {at:?}, earlier {xa:?}"))
                }
                Some(_) => {}
            }
            m.audio_sectors += 1;
            continue;
        }
        let user = &data[..2048];
        let Some(chunk) = Chunk::parse(user) else {
            m.other_sectors += 1;
            continue;
        };
        if submode & SUBMODE_FORM2 != 0 {
            return Err(format!("sector {i}: a video sector in Form 2"));
        }
        m.video_sectors += 1;
        m.max_chunks = m.max_chunks.max(chunk.count);
        let Some(done) = asm.add(user, &mut frame) else {
            continue;
        };
        if done.number != m.frames + 1 {
            return Err(format!(
                "sector {i}: frame {} follows frame {}",
                done.number, m.frames
            ));
        }
        if m.frames == 0 {
            (m.width, m.height) = (done.width, done.height);
        } else if (done.width, done.height) != (m.width, m.height) {
            return Err(format!(
                "frame {}: {}x{}, earlier frames {}x{}",
                done.number, done.width, done.height, m.width, m.height
            ));
        }
        m.frames = done.number;
        let bytes = &frame[..(done.size as usize).min(frame.len())];
        let header =
            bs::Header::parse(bytes).map_err(|e| format!("frame {}: {e:?}", done.number))?;
        m.versions[(header.version as usize).min(3)] += 1;
        let macroblocks = (done.width as u32 / 16) * (done.height as u32 / 16);
        let words = bs::decode_frame(bytes, &mut rle, macroblocks, macroblocks, &mut || {})
            .map_err(|e| format!("frame {}: {e:?}", done.number))?;
        m.rle_words = m.rle_words.max(words);
    }
    if asm.dropped != 0 {
        return Err(format!("{} frames have sectors missing", asm.dropped));
    }
    if m.frames == 0 || (m.width, m.height) != (WIDTH, HEIGHT) {
        return Err(format!(
            "{} frames of {}x{} is not a {WIDTH}x{HEIGHT} movie",
            m.frames, m.width, m.height
        ));
    }
    if m.xa.is_none() {
        return Err("the encoder wrote no audio".into());
    }
    Ok(m)
}

/// Where the data of a sector record ends and what the disc builder rebuilds
/// begins: after the 2048 bytes of a Form 1 sector's user data, and after the
/// 18 sound groups of 128 bytes of a Form 2 audio sector.
const FORM1_TAIL: std::ops::Range<usize> = 8 + 2048..XA_SECTOR;
const AUDIO_TAIL: std::ops::Range<usize> = 8 + 18 * 128..XA_SECTOR;

/// Zero what follows each sector's data: the encoder leaves it as whatever its
/// memory held, which makes the disc differ from one cook of the same movie to
/// the next, and the disc builder rebuilds it (checksums, and for audio 20
/// unused bytes the drive never plays) at the sector's final address.
fn zero_sector_tails(raw: &mut [u8]) {
    for sector in raw.chunks_exact_mut(XA_SECTOR) {
        let tail = if sector[2] & SUBMODE_AUDIO != 0 {
            AUDIO_TAIL
        } else {
            FORM1_TAIL
        };
        sector[tail].fill(0);
    }
}

/// The generated constants the intro program includes.
fn constants(m: &Movie, source: &str) -> String {
    format!(
        "// Generated by host/hl-intro-cook from the user's Half-Life install ({source}).\n\
         pub const FRAMES: u32 = {};\n\
         pub const WIDTH: u16 = {};\n\
         pub const HEIGHT: u16 = {};\n\
         pub const MAX_CHUNKS: u16 = {};\n\
         pub const RLE_WORDS: usize = {};\n\
         pub const XA: Option<(u8, u8)> = {:?};\n\
         pub const LATENCY_VBLANKS: u32 = {};\n",
        m.frames, m.width, m.height, m.max_chunks, m.rle_words, m.xa, LATENCY_VBLANKS
    )
}

/// The movie `StartupVids.txt` names, resolved against the game's folder.
fn startup_movie(valve: &Path) -> Result<PathBuf, String> {
    let list = valve.join("media/StartupVids.txt");
    let text = std::fs::read_to_string(&list).map_err(|e| format!("{}: {e}", list.display()))?;
    let mut names = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = names
        .next()
        .ok_or_else(|| format!("{} names no movie", list.display()))?;
    if let Some(second) = names.next() {
        return Err(format!(
            "{} names more than one movie ({first}, {second}); the intro plays one",
            list.display()
        ));
    }
    let path = valve.join(first);
    if !path.is_file() {
        return Err(format!(
            "{} names {first}, which is not there",
            list.display()
        ));
    }
    Ok(path)
}

fn find_psxavenc(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = explicit.or_else(|| std::env::var_os("PSXAVENC").map(PathBuf::from)) {
        return path.is_file().then_some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("psxavenc"))
            .find(|candidate| candidate.is_file())
    })
}

/// Runs the encoder with `args`.
fn run_psxavenc(psxavenc: &Path, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let status = Command::new(psxavenc)
        .arg("-q")
        .args(args)
        .status()
        .map_err(|e| format!("{}: {e}", psxavenc.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} failed: {status}", psxavenc.display()))
    }
}

/// The encoder as the XA options of one audio pass: mono 37.8 kHz, `bits`
/// per sample, filed under the movie's file and channel.
fn xa_args(bits: &'static str) -> Vec<std::ffi::OsString> {
    ["-f", "37800", "-c", "1", "-b", bits, "-F"]
        .iter()
        .map(std::ffi::OsString::from)
        .chain([
            XA_FILE.to_string().into(),
            "-C".into(),
            XA_CHANNEL.to_string().into(),
        ])
        .collect()
}

fn pass(psxavenc: &Path, head: &[&str], tail: &[&Path], bits: &'static str) -> Result<(), String> {
    let xa = xa_args(bits);
    let mut args: Vec<&std::ffi::OsStr> = head.iter().map(std::ffi::OsStr::new).collect();
    args.extend(xa.iter().map(|a| a.as_os_str()));
    args.extend(tail.iter().map(|p| p.as_os_str()));
    run_psxavenc(psxavenc, &args)
}

/// What the gain stage did to the sound, for the log.
#[derive(Debug)]
struct Mastering {
    source_lufs: f64,
    target_lufs: f64,
    gain_db: f64,
    result_lufs: f64,
    peak_dbfs: f64,
    limiter: limiter::Report,
}

/// Raises the sound by [`PLAYBACK_LOSS_DB`] over the install's own loudness,
/// through a look-ahead limiter so that it cannot clip, and returns the new
/// samples. The movie is mastered with about 17 dB between its loudness and
/// its peak; the player's path needs 7 dB more level than the source file
/// to be heard as loud, which the peaks alone could not carry.
fn master(pcm: &[i16]) -> Result<(Vec<i16>, Mastering), String> {
    let x: Vec<f64> = pcm.iter().map(|s| f64::from(*s) / 32768.0).collect();
    let source = loudness::integrated(&x, SOUND_HZ).ok_or("the movie's sound is silent")?;
    let target = source + PLAYBACK_LOSS_DB;
    let gain = limiter::gain_for(&x, SOUND_HZ, target)
        .ok_or_else(|| format!("{target:.1} LUFS is out of reach for the movie's sound"))?;
    let (y, report) = limiter::apply(&x, gain, SOUND_HZ);
    let out: Vec<i16> = y
        .iter()
        .map(|v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
        .collect();
    let peak = y.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let result = loudness::integrated(&y, SOUND_HZ).ok_or("the mastered sound is silent")?;
    Ok((
        out,
        Mastering {
            source_lufs: source,
            target_lufs: target,
            gain_db: 20.0 * gain.log10(),
            result_lufs: result,
            peak_dbfs: 20.0 * peak.log10(),
            limiter: report,
        },
    ))
}

/// A 16-bit mono PCM WAV, the form the encoder reads without guessing.
fn wav(pcm: &[i16], hz: u32) -> Vec<u8> {
    let data = pcm.len() as u32 * 2;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&hz.to_le_bytes());
    w.extend_from_slice(&(hz * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// The movie as an STR with its sound beside the picture.
///
/// The encoder makes the picture (320x240, 15 frames a second, bitstream
/// version 2) and the sound twice. First the sound at 8 bits, which the cook
/// decodes to samples, loudens and limits ([`master`]); then those samples at
/// 4 bits, the form the drive plays. [`mux::interleave`] puts the two streams
/// back together with the sound [`AUDIO_DELAY_SECTORS`] later than the encoder
/// would place it.
///
/// Mono 37.8 kHz XA: the movie's sound is mono, so nothing is lost, the
/// encoder's centre mix to stereo (-3 dB) is avoided, and the sound takes
/// half the sectors a stereo stream would, which go to the picture instead.
/// The drive plays a mono XA stream to both speakers.
fn encode(psxavenc: &Path, movie: &Path, work: &Path) -> Result<(Vec<u8>, Mastering), String> {
    std::fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
    let size = format!("{WIDTH}x{HEIGHT}");
    let (picture, wide, mastered, sound) = (
        work.join("picture.str"),
        work.join("wide.xa"),
        work.join("sound.wav"),
        work.join("sound.xa"),
    );
    pass(
        psxavenc,
        &["-t", "str", "-v", "v2", "-s", &size, "-r", FPS, "-x", "2"],
        &[movie, &picture],
        "4",
    )?;
    pass(psxavenc, &["-t", "xa"], &[movie, &wide], "8")?;
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let (pcm, mastering) = master(&xa::decode_mono_37800(&read(&wide)?)?)?;
    std::fs::write(&mastered, wav(&pcm, SOUND_HZ as u32))
        .map_err(|e| format!("{}: {e}", mastered.display()))?;
    pass(psxavenc, &["-t", "xa"], &[&mastered, &sound], "4")?;
    let stream = mux::interleave(&read(&picture)?, &read(&sound)?, AUDIO_DELAY_SECTORS)?;
    std::fs::remove_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
    Ok((stream, mastering))
}

struct Args {
    valve: PathBuf,
    out: PathBuf,
    psxavenc: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let (mut valve, mut out, mut psxavenc) = (None, None, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--valve" => valve = Some(value()?),
            "--out" => out = Some(value()?),
            "--psxavenc" => psxavenc = Some(value()?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let usage = "usage: hl-intro-cook --valve DIR --out DIR [--psxavenc PATH]";
    Ok(Args {
        valve: valve.ok_or(usage)?,
        out: out.ok_or(usage)?,
        psxavenc,
    })
}

fn run() -> Result<ExitCode, String> {
    let args = parse_args()?;
    let movie = match startup_movie(&args.valve) {
        Ok(movie) => movie,
        Err(reason) => {
            eprintln!("hl-intro-cook: no startup movie: {reason}");
            return Ok(ExitCode::from(NO_INTRO));
        }
    };
    let Some(psxavenc) = find_psxavenc(args.psxavenc) else {
        eprintln!("hl-intro-cook: psxavenc not found (--psxavenc, PSXAVENC or PATH)");
        return Ok(ExitCode::from(NO_INTRO));
    };
    std::fs::create_dir_all(&args.out).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let target = args.out.join("INTRO.STR");
    let (mut raw, mastering) = encode(&psxavenc, &movie, &args.out.join("work"))?;
    // Same movie, same bytes: the encoder leaves the unused tail of each audio
    // sector as whatever its memory held, which would make the disc differ
    // from one cook to the next.
    zero_sector_tails(&mut raw);
    std::fs::write(&target, &raw).map_err(|e| format!("{}: {e}", target.display()))?;
    let m = measure(&raw).map_err(|e| format!("intro movie: {e}"))?;
    let name = movie
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("startup movie");
    std::fs::write(args.out.join("movie.rs"), constants(&m, name))
        .map_err(|e| format!("movie.rs: {e}"))?;
    eprintln!(
        "hl-intro-cook: sound {:.1} LUFS in the install, {:+.1} dB gain to reach {:.1}, {:.1} LUFS and {:.1} dBFS peak after \
         the limiter (at most {:.1} dB, {:.1}% of the time)",
        mastering.source_lufs,
        mastering.gain_db,
        mastering.target_lufs,
        mastering.result_lufs,
        mastering.peak_dbfs,
        mastering.limiter.max_reduction_db,
        mastering.limiter.active * 100.0
    );
    eprintln!(
        "hl-intro-cook: {name}: {} frames {}x{}, {} video + {} audio + {} other sectors ({} bytes), \
         max {} chunks, max {} RLE words, bitstream versions {:?}, XA {:?}",
        m.frames,
        m.width,
        m.height,
        m.video_sectors,
        m.audio_sectors,
        m.other_sectors,
        raw.len(),
        m.max_chunks,
        m.rle_words,
        m.versions,
        m.xa
    );
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("hl-intro-cook: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_that_is_not_whole_sectors_is_refused() {
        assert!(measure(&[0u8; XA_SECTOR + 1]).is_err());
        assert!(measure(&[]).is_err());
    }

    #[test]
    fn only_what_follows_each_sectors_data_is_zeroed() {
        let mut raw = vec![0xAAu8; XA_SECTOR * 2];
        raw[2] = SUBMODE_AUDIO | SUBMODE_FORM2;
        raw[XA_SECTOR + 2] = 0;
        zero_sector_tails(&mut raw);
        // Audio sector: the sound groups stay, the tail goes.
        assert!(raw[AUDIO_TAIL].iter().all(|b| *b == 0));
        assert_eq!(raw[AUDIO_TAIL.start - 1], 0xAA);
        // Data sector: the user data stays, the tail goes.
        let second = &raw[XA_SECTOR..];
        assert!(second[FORM1_TAIL].iter().all(|b| *b == 0));
        assert_eq!(second[FORM1_TAIL.start - 1], 0xAA);
    }

    #[test]
    fn a_stream_without_video_is_refused() {
        // All-zero sectors are neither audio nor an STR chunk.
        assert!(measure(&vec![0u8; XA_SECTOR * 4]).is_err());
    }

    #[test]
    fn the_constants_name_every_figure_the_guest_sizes_by() {
        let m = Movie {
            frames: 150,
            width: 320,
            height: 240,
            max_chunks: 9,
            rle_words: 20_000,
            xa: Some((1, 0)),
            ..Movie::default()
        };
        let text = constants(&m, "valve.webm");
        for want in [
            "FRAMES: u32 = 150",
            "WIDTH: u16 = 320",
            "HEIGHT: u16 = 240",
            "MAX_CHUNKS: u16 = 9",
            "RLE_WORDS: usize = 20000",
            "XA: Option<(u8, u8)> = Some((1, 0))",
            "LATENCY_VBLANKS: u32 = 5",
        ] {
            assert!(text.contains(want), "{want}\n{text}");
        }
    }

    #[test]
    fn a_named_movie_must_be_the_only_line_and_exist() {
        let dir = std::env::temp_dir().join(format!("hl-intro-cook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("media/StartupVids.txt"), "media/a.webm\n").unwrap();
        assert!(startup_movie(&dir).is_err());
        std::fs::write(dir.join("media/a.webm"), b"x").unwrap();
        assert_eq!(startup_movie(&dir).unwrap(), dir.join("media/a.webm"));
        std::fs::write(
            dir.join("media/StartupVids.txt"),
            "media/a.webm\nmedia/b.avi\n",
        )
        .unwrap();
        assert!(startup_movie(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

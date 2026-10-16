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
    if raw.is_empty() || raw.len() % XA_SECTOR != 0 {
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

/// The generated constants the intro program includes.
fn constants(m: &Movie, source: &str) -> String {
    format!(
        "// Generated by host/hl-intro-cook from the user's Half-Life install ({source}).\n\
         pub const FRAMES: u32 = {};\n\
         pub const WIDTH: u16 = {};\n\
         pub const HEIGHT: u16 = {};\n\
         pub const MAX_CHUNKS: u16 = {};\n\
         pub const RLE_WORDS: usize = {};\n\
         pub const XA: Option<(u8, u8)> = {:?};\n",
        m.frames, m.width, m.height, m.max_chunks, m.rle_words, m.xa
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

/// Mono 37.8 kHz XA beside the picture: the movie's sound is mono, so nothing is
/// lost, the encoder's centre mix to stereo (-3 dB) is avoided, and the
/// audio takes half the sectors a stereo stream would, which go to the
/// picture instead. The drive plays a mono XA stream to both speakers.
fn encode(psxavenc: &Path, movie: &Path, out: &Path) -> Result<(), String> {
    let status = Command::new(psxavenc)
        .args(["-q", "-t", "str", "-v", "v2", "-s"])
        .arg(format!("{WIDTH}x{HEIGHT}"))
        .args([
            "-r", FPS, "-x", "2", "-f", "37800", "-c", "1", "-b", "4", "-F",
        ])
        .arg(XA_FILE.to_string())
        .arg("-C")
        .arg(XA_CHANNEL.to_string())
        .arg(movie)
        .arg(out)
        .status()
        .map_err(|e| format!("{}: {e}", psxavenc.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} failed: {status}", psxavenc.display()))
    }
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
    encode(&psxavenc, &movie, &target)?;
    let raw = std::fs::read(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    let m = measure(&raw).map_err(|e| format!("intro movie: {e}"))?;
    let name = movie
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("startup movie");
    std::fs::write(args.out.join("movie.rs"), constants(&m, name))
        .map_err(|e| format!("movie.rs: {e}"))?;
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

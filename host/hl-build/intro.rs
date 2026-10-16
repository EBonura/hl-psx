//! The intro stage of the disc: Half-Life's startup movie, played by a small
//! boot program that then loads the game.
//!
//! The game's image has almost no RAM to spare, so the movie player cannot be
//! part of it. A disc with an intro boots `intro/` (linked high in RAM; the
//! BIOS runs it as `PSX.EXE`), which streams `INTRO.STR` and then loads the
//! game's own executable, an ordinary root file called `HLGAME.EXE`, over the
//! RAM it was using (the SDK's chain loader, the one the demo disc uses).
//! Everything else about the disc is where it always was: the fixed boot
//! area still ends at `WORLD.PAK`, so no pack LBA moves.
//!
//! The movie comes from the player's own install (`host/hl-intro-cook`) and
//! lives, with everything derived from it, under `.hlpsx/intro/`, never in
//! the repository. A machine without the encoder (or an install without a
//! startup movie) builds the disc without an intro stage.

use super::*;

/// Where the intro program links: above the game's loaded image, below its
/// 32 KiB stack.
pub const LOAD_ADDR: u32 = 0x801E_0000;
/// Room kept between the end of the game's loaded image and the intro.
const MARGIN: u32 = 0x1000;
/// The cooker's exit status for "no intro on this machine".
const NO_INTRO: i32 = 3;
const GAME_FILE: &str = "HLGAME.EXE";
/// Bytes of PS-X EXE header before the payload.
const HEADER: usize = 0x800;

pub fn dir(repository: &Path) -> PathBuf {
    repository.join(".hlpsx/intro")
}

/// The cooked movie, if this checkout has one.
pub fn cooked(repository: &Path) -> Option<PathBuf> {
    let dir = dir(repository);
    (dir.join("INTRO.STR").is_file() && dir.join("movie.rs").is_file()).then_some(dir)
}

/// Encode the install's startup movie. Without the encoder or the movie the
/// stage is dropped (and any earlier one removed, so the disc cannot carry a
/// movie that no longer matches the install).
pub fn cook(repository: &Path, valve: &Path) -> Result<bool> {
    let manifest = repository.join("host/hl-intro-cook/Cargo.toml");
    run(
        Command::new(cargo())
            .current_dir(repository)
            .args(["build", "--release", "--manifest-path"])
            .arg(&manifest),
        "build intro movie cooker",
    )?;
    let out = dir(repository);
    println!("\n==> cook the intro movie");
    let status = Command::new(executable(
        repository.join("host/hl-intro-cook/target/release/hl-intro-cook"),
    ))
    .arg("--valve")
    .arg(valve)
    .arg("--out")
    .arg(&out)
    .status()?;
    match status.code() {
        Some(0) => Ok(true),
        Some(NO_INTRO) => {
            let _ = fs::remove_dir_all(&out);
            println!("warning: no intro movie cooked; the disc will boot straight into the game");
            Ok(false)
        }
        _ => Err(format!("cook the intro movie failed with {status}").into()),
    }
}

/// 32-bit FNV-1a, the chain loader's payload checksum.
pub fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811C_9DC5u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

/// A PS-X EXE ready to be a root file the chain loader reads.
pub struct GameImage {
    /// Header and payload, padded to the payload size the header declares.
    pub bytes: Vec<u8>,
    /// Where the payload loads, and its size.
    pub address: u32,
    pub size: u32,
    /// FNV-1a of the payload, as the loader checks it in RAM.
    pub fnv: u32,
}

pub fn game_image(exe: &[u8]) -> Result<GameImage> {
    if exe.len() < HEADER || &exe[..8] != b"PS-X EXE" {
        return Err("the game executable is not a PS-X EXE".into());
    }
    let word = |at: usize| u32::from_le_bytes(exe[at..at + 4].try_into().expect("4 bytes"));
    let (address, size) = (word(0x18), word(0x1C));
    if size as usize % HEADER != 0 {
        return Err(format!("the game's payload ({size} bytes) is not whole sectors").into());
    }
    let mut bytes = exe.to_vec();
    bytes.resize(HEADER + size as usize, 0);
    let fnv = fnv1a(&bytes[HEADER..HEADER + size as usize]);
    Ok(GameImage {
        bytes,
        address,
        size,
        fnv,
    })
}

/// Fails when the game's loaded image would reach the intro.
pub fn check_fit(game: &GameImage) -> Result<()> {
    let end = game.address.saturating_add(game.size);
    if end.saturating_add(MARGIN) > LOAD_ADDR {
        return Err(format!(
            "the game's image ends at {end:#x}, within {MARGIN:#x} of the intro at {LOAD_ADDR:#x}; \
             build without the intro (--no-intro) or move the intro up"
        )
        .into());
    }
    Ok(())
}

/// Compile the intro for this game image: the chain loader checks the
/// game's payload checksum, so the build carries it.
pub fn compile(repository: &Path, psoxide: &Path, game: &GameImage) -> Result<PathBuf> {
    let intro = repository.join("intro");
    let map = dir(repository).join("hl-intro.map");
    let mut command = Command::new(cargo());
    command
        .current_dir(&intro)
        .args(["build", "--release"])
        .env("PSOXIDE", psoxide)
        .env("HL_INTRO_LOAD_ADDR", format!("{LOAD_ADDR:#x}"))
        .env("HL_INTRO_GAME_FNV", format!("{:x}", game.fnv))
        .env("HLPSX_LINK_MAP", &map);
    run_compile(&mut command, "compile the hl-psx intro for PlayStation")?;
    let exe = intro.join("target/mipsel-sony-psx/release/hl-intro.exe");
    if !exe.is_file() {
        return Err(format!("the intro build did not produce {}", exe.display()).into());
    }
    // LLVM's delay-slot filler leaves a load in a slot here and there; the
    // SDK's patcher reroutes those branches through psx-rt's trampolines, and
    // the scanner then proves the image clean (as it does for the game).
    run(
        hazard_tool(psoxide, "hazard-patch")
            .arg(&exe)
            .arg("--map")
            .arg(&map),
        "patch load-delay hazards in hl-intro.exe",
    )?;
    run(
        hazard_tool(psoxide, "hazard-scan")
            .arg(&exe)
            .arg("--map")
            .arg(&map),
        "prove hl-intro.exe free of load-delay hazards",
    )?;
    Ok(exe)
}

/// Stage the game's executable as the root file the intro loads, and return
/// its path.
pub fn stage_game(repository: &Path, game: &GameImage) -> Result<PathBuf> {
    let directory = dir(repository).join("disc");
    fs::create_dir_all(&directory)?;
    let path = directory.join(GAME_FILE);
    fs::write(&path, &game.bytes)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(address: u32, size: u32, stored: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; HEADER + stored];
        bytes[..8].copy_from_slice(b"PS-X EXE");
        bytes[0x18..0x1C].copy_from_slice(&address.to_le_bytes());
        bytes[0x1C..0x20].copy_from_slice(&size.to_le_bytes());
        bytes
    }

    #[test]
    fn fnv1a_matches_the_published_vectors() {
        assert_eq!(fnv1a(b""), 0x811C_9DC5);
        assert_eq!(fnv1a(b"a"), 0xE40C_292C);
        assert_eq!(fnv1a(b"foobar"), 0xBF9C_F968);
    }

    #[test]
    fn the_image_is_padded_to_the_declared_payload_and_hashed_whole() {
        let mut bytes = exe(0x8001_0000, 0x1000, 0x900);
        bytes[HEADER] = 7;
        let image = game_image(&bytes).unwrap();
        assert_eq!(image.bytes.len(), HEADER + 0x1000);
        assert_eq!(image.fnv, fnv1a(&image.bytes[HEADER..]));
        assert_ne!(image.fnv, fnv1a(&image.bytes[HEADER..HEADER + 0x900]));
    }

    #[test]
    fn a_payload_that_is_not_whole_sectors_or_not_an_exe_is_refused() {
        assert!(game_image(&exe(0x8001_0000, 0x1001, 0x1001)).is_err());
        assert!(game_image(&[0u8; HEADER]).is_err());
    }

    #[test]
    fn the_game_must_end_a_margin_below_the_intro() {
        let end = LOAD_ADDR - MARGIN;
        let fits = game_image(&exe(0x8001_0000, end - 0x8001_0000, 0)).unwrap();
        assert!(check_fit(&fits).is_ok());
        let touches = game_image(&exe(0x8001_0000, end - 0x8001_0000 + 0x800, 0)).unwrap();
        assert!(check_fit(&touches).is_err());
    }
}

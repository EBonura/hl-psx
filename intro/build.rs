//! Links the intro high in RAM.
//!
//! The SDK's linker script puts every program at 0x80010000, which is where
//! the game's image loads, so an intro linked there would be overwritten by
//! the very read that loads the game. The intro links at `HL_INTRO_LOAD_ADDR`
//! instead (hl-build sets it from the game's own image end), with the SDK's
//! script otherwise unchanged: a derivative is written to OUT_DIR so the
//! pinned SDK stays pristine and a layout change upstream fails loudly.

use std::{env, fs, path::PathBuf};

const SDK_LOAD: &str = "LOAD_ADDR   = RAM_BASE + BIOS_SIZE;";
const SDK_LENGTH: &str = "LENGTH = EXE_HEAD_BYTES + RAM_SIZE - BIOS_SIZE - STACK_RESERVE";

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let psoxide = env::var("PSOXIDE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("../.psoxide"));
    let ld = psoxide.join("sdk/psoxide.ld");
    let ld = ld.canonicalize().unwrap_or(ld);
    println!("cargo:rerun-if-changed={}", ld.display());
    println!("cargo:rerun-if-env-changed=HL_INTRO_LOAD_ADDR");
    println!("cargo:rerun-if-env-changed=HL_INTRO_GAME_FNV");
    println!("cargo:rerun-if-env-changed=HLPSX_LINK_MAP");

    let base = env::var("HL_INTRO_LOAD_ADDR").unwrap_or_else(|_| "0x801E0000".to_string());
    let sdk = fs::read_to_string(&ld).expect("read PSoXide linker script");
    for pattern in [SDK_LOAD, SDK_LENGTH] {
        assert_eq!(
            sdk.matches(pattern).count(),
            1,
            "the SDK linker script changed: {pattern:?}"
        );
    }
    let script = sdk
        .replace(SDK_LOAD, &format!("LOAD_ADDR   = {base};"))
        .replace(
            SDK_LENGTH,
            "LENGTH = EXE_HEAD_BYTES + RAM_BASE + RAM_SIZE - LOAD_ADDR - STACK_RESERVE",
        );
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("hl-intro.ld");
    fs::write(&out, script).expect("write the intro linker script");
    println!("cargo:rustc-link-arg=-T{}", out.display());
    println!("cargo:rustc-link-arg=--oformat=binary");
    if let Ok(map) = env::var("HLPSX_LINK_MAP") {
        if !map.is_empty() {
            println!("cargo:rustc-link-arg=-Map={map}");
        }
    }

    // The game's payload checksum, which the chain loader verifies in RAM.
    // A development build with no game image yet links with a zero.
    let fnv = env::var("HL_INTRO_GAME_FNV").unwrap_or_else(|_| "0".to_string());
    let fnv =
        u32::from_str_radix(fnv.trim_start_matches("0x"), 16).expect("HL_INTRO_GAME_FNV is hex");
    println!("cargo:rustc-env=HL_INTRO_GAME_FNV_VALUE={fnv}");
}

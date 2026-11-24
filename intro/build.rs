//! Links the intro high in RAM, on the scene core.
//!
//! The SDK's linker script puts every program at 0x80010000, which is where
//! the game's image loads, so an intro linked there would be overwritten by
//! the very read that loads the game. The intro links at `HL_INTRO_LOAD_ADDR`
//! instead (hl-build sets it from the game's own image end), through the
//! script's LOAD_ADDR_OVERRIDE, so the pinned SDK stays pristine.
//!
//! On the scene core (CORE-ABI 4.1): the RAM size and load address first, then
//! psoxide-core.ld, psoxide-boot.ld and psoxide.ld, as an ELF; hl-build writes
//! the EXE with psx-exe, `.boot` stored after `.data` and the core's image
//! after it. The splash and the movie player are boot-only memory (`.boot`),
//! run above `.bss` (hl-build checks the map: the section must end below the
//! stack reserve).

use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let psoxide = env::var("PSOXIDE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest.join("../.psoxide"));
    let script_path = |name: &str| {
        let path = psoxide.join("sdk").join(name);
        let path = path.canonicalize().unwrap_or(path);
        println!("cargo:rerun-if-changed={}", path.display());
        path
    };
    let ld = script_path("psoxide.ld");
    let core = script_path("psoxide-core.ld");
    let boot = script_path("psoxide-boot.ld");
    println!("cargo:rerun-if-env-changed=HL_INTRO_LOAD_ADDR");
    println!("cargo:rerun-if-env-changed=HLPSX_LINK_MAP");

    let base = env::var("HL_INTRO_LOAD_ADDR").unwrap_or_else(|_| "0x801E0000".to_string());
    let sdk = fs::read_to_string(&ld).expect("read PSoXide linker script");
    assert!(
        sdk.contains("LOAD_ADDR_OVERRIDE"),
        "the SDK's psoxide.ld takes no LOAD_ADDR_OVERRIDE: the scene core needs a newer pin"
    );
    // psx-rt carries `__boot_entry` (src/boot.rs) on every SDK with the scene core; boot.rs
    // keeps its own copy only for older pins.
    println!("cargo::rustc-check-cfg=cfg(sdk_boot_entry)");
    if psoxide.join("sdk/crates/psx-rt/src/boot.rs").exists() {
        println!("cargo:rustc-cfg=sdk_boot_entry");
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let load = out_dir.join("hl-intro-load.ld");
    fs::write(
        &load,
        format!("LOAD_ADDR_OVERRIDE = {base};\nRAM_SIZE_OVERRIDE = 0x1FE000;\n"),
    )
    .expect("write the intro's load address script");
    for script in [&load, &core, &boot, &ld] {
        println!("cargo:rustc-link-arg=-T{}", script.display());
    }
    if let Ok(map) = env::var("HLPSX_LINK_MAP") {
        if !map.is_empty() {
            println!("cargo:rustc-link-arg=-Map={map}");
        }
    }
}

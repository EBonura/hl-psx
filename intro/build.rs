//! Links the intro high in RAM.
//!
//! The SDK's linker script puts every program at 0x80010000, which is where
//! the game's image loads, so an intro linked there would be overwritten by
//! the very read that loads the game. The intro links at `HL_INTRO_LOAD_ADDR`
//! instead (hl-build sets it from the game's own image end): through the
//! script's LOAD_ADDR_OVERRIDE where the SDK has it, else with a derivative of
//! the script written to OUT_DIR, so the pinned SDK stays pristine and a
//! layout change upstream fails loudly.
//!
//! The splash and the movie player are boot-only memory, through the SDK's
//! psoxide-boot.ld (read first, as its header says), run from the end of
//! `.bss`. The address is the link's own `__bss_end`,
//! given as a `--defsym` expression, so one link places it (hl-build checks
//! the map: the section must end below the stack reserve).

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
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    // An SDK whose script takes LOAD_ADDR_OVERRIDE is linked as it is, after a one-line script
    // that sets it; an older one gets the patched derivative.
    let scripts = if sdk.contains("LOAD_ADDR_OVERRIDE") {
        let load = out_dir.join("hl-intro-load.ld");
        fs::write(&load, format!("LOAD_ADDR_OVERRIDE = {base};\n"))
            .expect("write the intro's load address script");
        vec![load, ld.clone()]
    } else {
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
        let out = out_dir.join("hl-intro.ld");
        fs::write(&out, script).expect("write the intro linker script");
        vec![out]
    };
    let boot = psoxide.join("sdk/psoxide-boot.ld");
    let boot = boot.canonicalize().unwrap_or(boot);
    // An SDK whose psx-rt carries `__boot_entry` (src/boot.rs) gives the mover; boot.rs keeps its
    // own copy only for older pins. Such an SDK names the run and load addresses
    // BOOT_VMA_OVERRIDE and BOOT_LMA_OVERRIDE.
    let sdk_mover = psoxide.join("sdk/crates/psx-rt/src/boot.rs").exists();
    println!("cargo::rustc-check-cfg=cfg(sdk_boot_entry)");
    if sdk_mover {
        println!("cargo:rustc-cfg=sdk_boot_entry");
    }
    let boot_script = fs::read_to_string(&boot).expect("read psoxide-boot.ld");
    let (vma, lma) = if boot_script.contains("BOOT_LMA_OVERRIDE") {
        ("BOOT_VMA_OVERRIDE", "BOOT_LMA_OVERRIDE")
    } else {
        ("BOOT_VMA", "BOOT_LMA")
    };
    println!("cargo:rerun-if-changed={}", boot.display());
    if env::var_os("CARGO_FEATURE_SCENE_CORE").is_some() {
        // On the scene core (CORE-ABI 4.1): the RAM size and load address first, then
        // psoxide-core.ld, psoxide-boot.ld and psoxide.ld, as an ELF; hl-build writes the EXE
        // with psx-exe, `.boot` stored after `.data` and the core's image after it.
        let core = psoxide.join("sdk/psoxide-core.ld");
        let core = core.canonicalize().unwrap_or(core);
        println!("cargo:rerun-if-changed={}", core.display());
        let (load, sdk_script) = (&scripts[0], scripts.last().expect("psoxide.ld"));
        let mut text = fs::read_to_string(load).expect("read the intro's load address script");
        text.push_str("RAM_SIZE_OVERRIDE = 0x1FE000;\n");
        fs::write(load, text).expect("write the intro's load address script");
        for script in [load, &core, &boot, sdk_script] {
            println!("cargo:rustc-link-arg=-T{}", script.display());
        }
    } else {
        println!("cargo:rustc-link-arg=-T{}", boot.display());
        println!("cargo:rustc-link-arg=-u__boot_entry");
        // Stored where it runs (BOOT_LMA = BOOT_VMA = __bss_end), not after `.data`: ld.lld's
        // `--oformat=binary` writes no program headers and so places every section at its VMA,
        // ignoring the script's AT() (the first link of this program put `.boot` at __bss_end in
        // the file whatever BOOT_LMA said, and its mover then copied from the wrong place). The
        // file carries this program's `.bss` (180 B) as zeros in exchange; `_start` clears it.
        println!("cargo:rustc-link-arg=--defsym={vma}=__bss_end");
        println!("cargo:rustc-link-arg=--defsym={lma}=__bss_end");
        for script in &scripts {
            println!("cargo:rustc-link-arg=-T{}", script.display());
        }
        println!("cargo:rustc-link-arg=--oformat=binary");
    }
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

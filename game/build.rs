//! Inject PSoXide's PSX linker script into the final link, by absolute
//! path derived from this crate's location. This keeps the crate buildable
//! from anywhere (no brittle relative `-T` paths in RUSTFLAGS) while the
//! script itself lives in the pinned submodule.

use std::path::PathBuf;

fn main() {
    // This crate lives at <repo>/game, so the repo root is one level up.
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest.parent().expect("crate must live at <repo>/game");
    let ld = repo_root.join("third_party/PSoXide/sdk/psoxide.ld");
    let ld = ld.canonicalize().unwrap_or(ld);

    // `-T` selects the linker script; `--oformat=binary` dumps a flat PSX-EXE
    // image (the script lays out the executable header) instead of an ELF.
    println!("cargo:rustc-link-arg=-T{}", ld.display());
    println!("cargo:rustc-link-arg=--oformat=binary");
    println!("cargo:rerun-if-changed={}", ld.display());
}

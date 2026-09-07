use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod model_audit;
mod model_variants;
mod regression;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const ROOT_MANIFEST: &str = include_str!("../../Cargo.toml");
const MAP_LIST: &str = include_str!("../hl-content/map-list.txt");
const COOK_MANIFEST_SCHEMA: u32 = 1;
const COOK_MANIFEST_PATH: &str = "data/.hlpsx-cook.json";

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
struct CookManifest {
    schema: u32,
    hl_psx_revision: String,
    hl_psx_tree_sha256: String,
    psoxide_source: String,
    psoxide_revision: String,
    psoxide_tree_sha256: String,
    half_life_input_sha256: String,
    cooked_tree_sha256: String,
}

fn hex_sha256(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn collect_files_skipping(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
    skipped_directories: &[&str],
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
            if !skipped_directories.contains(&name) {
                collect_files_skipping(root, &path, files, skipped_directories)?;
            }
        } else if metadata.is_file() {
            files.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

fn digest_files(root: &Path, mut files: Vec<PathBuf>) -> Result<String> {
    files.sort();
    files.dedup();
    let mut digest = Sha256::new();
    digest.update(b"hl-psx-tree-v1\0");
    for relative in files {
        let path = root.join(&relative);
        if !path.is_file() {
            return Err(format!("provenance input is missing: {}", path.display()).into());
        }
        let normalized = relative.to_string_lossy().replace('\\', "/");
        digest.update((normalized.len() as u64).to_le_bytes());
        digest.update(normalized.as_bytes());
        digest.update(path.metadata()?.len().to_le_bytes());
        let mut stream = fs::File::open(&path)?;
        let mut buffer = [0u8; 1024 * 1024];
        loop {
            let count = stream.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
    }
    Ok(hex_sha256(digest.finalize()))
}

fn tree_digest(root: &Path, excluded: Option<&Path>) -> Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    if let Some(excluded) = excluded {
        files.retain(|path| path != excluded);
    }
    digest_files(root, files)
}

fn git_output(repository: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(repository)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed for {}: {}",
            args.join(" "),
            repository.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(output.stdout)
}

fn git_revision(repository: &Path) -> Result<String> {
    if let Ok(output) = git_output(repository, &["rev-parse", "--verify", "HEAD^{commit}"]) {
        let revision = String::from_utf8(output)?.trim().to_ascii_lowercase();
        if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(revision);
        }
    }
    Ok(format!("tree:{}", source_tree_digest(repository)?))
}

fn source_tree_digest(repository: &Path) -> Result<String> {
    let files = match git_output(repository, &["ls-files", "-z"]) {
        Ok(listed) => listed
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8(path.to_vec()).map(PathBuf::from))
            .collect::<std::result::Result<Vec<_>, _>>()?,
        Err(_) => {
            let mut files = Vec::new();
            collect_files_skipping(
                repository,
                repository,
                &mut files,
                &[
                    ".git",
                    ".psoxide",
                    ".hlpsx",
                    "target",
                    "data",
                    "assets",
                    "dist",
                    "captures",
                    "graphify-out",
                    "docs",
                    "reference",
                    "tools",
                    "build",
                    "tmp",
                    "__pycache__",
                ],
            )?;
            files.retain(|path| {
                !matches!(
                    path.file_name().and_then(OsStr::to_str),
                    Some(".DS_Store" | "config.mk")
                )
            });
            files
        }
    };
    digest_files(repository, files)
}

fn psoxide_tree_digest(psoxide: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files_skipping(
        psoxide,
        psoxide,
        &mut files,
        &[
            ".git",
            "target",
            "build",
            "dist",
            "baked",
            "cooked",
            "node_modules",
            "captures",
            "graphify-out",
            ".web",
            "__pycache__",
        ],
    )?;
    files.retain(|path| path.file_name().and_then(OsStr::to_str) != Some(".DS_Store"));
    digest_files(psoxide, files)
}

fn psoxide_source(psoxide: &Path) -> Result<String> {
    let marker = psoxide.join(".psoxide-source");
    let source = fs::read_to_string(&marker)
        .map_err(|error| format!("cannot read {}: {error}", marker.display()))?;
    let source = source.trim();
    if source.is_empty() {
        return Err(format!("{} is empty", marker.display()).into());
    }
    Ok(source.to_string())
}

fn psoxide_revision(psoxide: &Path) -> Result<String> {
    let source = psoxide_source(psoxide)?;
    if let Some(revision) = source.strip_prefix("git:") {
        if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(revision.to_ascii_lowercase());
        }
    }
    if let Some(path) = source.strip_prefix("local:") {
        return git_revision(Path::new(path));
    }
    Ok(format!("tree:{}", psoxide_tree_digest(psoxide)?))
}

fn cook_manifest_path(repository: &Path) -> PathBuf {
    repository.join(COOK_MANIFEST_PATH)
}

fn cooked_tree_digest(repository: &Path) -> Result<String> {
    tree_digest(
        &repository.join("data"),
        Some(Path::new(".hlpsx-cook.json")),
    )
}

fn current_cook_manifest(repository: &Path, valve: &Path, psoxide: &Path) -> Result<CookManifest> {
    Ok(CookManifest {
        schema: COOK_MANIFEST_SCHEMA,
        hl_psx_revision: git_revision(repository)?,
        hl_psx_tree_sha256: source_tree_digest(repository)?,
        psoxide_source: psoxide_source(psoxide)?,
        psoxide_revision: psoxide_revision(psoxide)?,
        psoxide_tree_sha256: psoxide_tree_digest(psoxide)?,
        half_life_input_sha256: tree_digest(valve, None)?,
        cooked_tree_sha256: cooked_tree_digest(repository)?,
    })
}

fn invalidate_cook_manifest(repository: &Path) -> Result<()> {
    let path = cook_manifest_path(repository);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn write_cook_manifest(repository: &Path, valve: &Path, psoxide: &Path) -> Result<()> {
    let manifest = current_cook_manifest(repository, valve, psoxide)?;
    let path = cook_manifest_path(repository);
    let temporary = path.with_extension("json.tmp");
    let mut serialized = serde_json::to_vec_pretty(&manifest)?;
    serialized.push(b'\n');
    fs::write(&temporary, serialized)?;
    fs::rename(&temporary, &path)?;
    println!("Cook provenance -> {}", path.display());
    println!("  HL-PSX  : {}", manifest.hl_psx_revision);
    println!(
        "  PSoXide : {} ({})",
        manifest.psoxide_revision, manifest.psoxide_source
    );
    println!("  cooked  : {}", manifest.cooked_tree_sha256);
    Ok(())
}

fn verify_cook_manifest(repository: &Path, psoxide: &Path) -> Result<CookManifest> {
    let path = cook_manifest_path(repository);
    let manifest: CookManifest = serde_json::from_slice(&fs::read(&path).map_err(|error| {
        format!(
            "cooked-asset provenance is missing at {} ({error}); run `cargo run --release -- assets --half-life PATH --psoxide PATH` before packing",
            path.display()
        )
    })?)?;
    if manifest.schema != COOK_MANIFEST_SCHEMA {
        return Err(format!(
            "unsupported cooked-asset provenance schema {} at {}; run a fresh full asset cook",
            manifest.schema,
            path.display()
        )
        .into());
    }
    let revision = git_revision(repository)?;
    let source_tree = source_tree_digest(repository)?;
    let psoxide_source = psoxide_source(psoxide)?;
    let psoxide_revision = psoxide_revision(psoxide)?;
    let psoxide_tree = psoxide_tree_digest(psoxide)?;
    let cooked_tree = cooked_tree_digest(repository)?;
    let checks = [
        (
            "HL-PSX revision",
            manifest.hl_psx_revision.as_str(),
            revision.as_str(),
        ),
        (
            "HL-PSX source tree",
            manifest.hl_psx_tree_sha256.as_str(),
            source_tree.as_str(),
        ),
        (
            "PSoXide source",
            manifest.psoxide_source.as_str(),
            psoxide_source.as_str(),
        ),
        (
            "PSoXide revision",
            manifest.psoxide_revision.as_str(),
            psoxide_revision.as_str(),
        ),
        (
            "PSoXide hydrated tree",
            manifest.psoxide_tree_sha256.as_str(),
            psoxide_tree.as_str(),
        ),
        (
            "cooked asset tree",
            manifest.cooked_tree_sha256.as_str(),
            cooked_tree.as_str(),
        ),
    ];
    if let Some((label, expected, actual)) = checks
        .into_iter()
        .find(|(_, expected, actual)| expected != actual)
    {
        return Err(format!(
            "stale cooked assets: {label} does not match the full cook (manifest {expected}, current {actual}); run `cargo run --release -- assets --half-life PATH --psoxide PATH`",
        )
        .into());
    }
    println!("Cook provenance verified: {}", path.display());
    println!("  HL-PSX  : {revision}");
    println!("  PSoXide : {psoxide_revision} ({psoxide_source})");
    println!("  cooked  : {cooked_tree}");
    Ok(manifest)
}

fn manifest_dependency_rev<'a>(manifest: &'a str, dependency: &str) -> Option<&'a str> {
    manifest.lines().find_map(|line| {
        let (name, specification) = line.split_once('=')?;
        if name.trim() != dependency {
            return None;
        }
        let marker = "rev = \"";
        let start = specification.find(marker)? + marker.len();
        let end = specification[start..].find('"')? + start;
        Some(&specification[start..end])
    })
}

/// The root manifest is the single authoritative PSoXide pin.
///
/// Both git dependencies must resolve to the same checkout. Deriving the
/// hydration marker from that manifest prevents the build driver from silently
/// copying a stale SDK after a dependency bump.
fn psoxide_rev() -> &'static str {
    let link = manifest_dependency_rev(ROOT_MANIFEST, "psoxide-link")
        .expect("root Cargo.toml must pin psoxide-link by rev");
    let pack = manifest_dependency_rev(ROOT_MANIFEST, "psx-pack")
        .expect("root Cargo.toml must pin psx-pack by rev");
    assert_eq!(link, pack, "psoxide-link and psx-pack revs must match");
    link
}
struct WeaponModel {
    name: &'static str,
    // Runtime clip order is model-specific; game/src/main.rs maps the common
    // idle/fire/alt/reload/draw states onto these compact source clips.
    sequences: &'static str,
}

const WEAPON_MODELS: [WeaponModel; 16] = [
    WeaponModel {
        name: "v_9mmhandgun",
        // Nine samples land on the sharp recoil/recovery quality knee while
        // leaving the slower idle/reload clips at their existing budgets.
        sequences: "idle1:5,shoot:9,reload:12,draw:5",
    },
    WeaponModel {
        name: "v_357",
        sequences: "idle1:1,fire1:5,reload:9,draw:2",
    },
    WeaponModel {
        name: "v_9mmar",
        sequences: "longidle:2,shoot:7,grenade:6,reload:15,deploy:6",
    },
    WeaponModel {
        name: "v_crossbow",
        sequences: "idle1:1,fire1:8,reload:10,draw1:4",
    },
    WeaponModel {
        name: "v_crowbar",
        // This small mesh fits every authored source pose losslessly.
        sequences: "idle1:36,attack1miss:11,attack1:11,draw:13",
    },
    WeaponModel {
        name: "v_chub",
        sequences: "idle1:3,Throw:18,up:16",
    },
    WeaponModel {
        name: "v_egon",
        sequences: "idle1:4,fire3:8,altfirecycle:8,draw:6",
    },
    WeaponModel {
        name: "v_gauss",
        // Secondary fire is a real hold/release charge: retain the authored
        // coil spin-up and looping spin instead of jumping straight to fire2.
        sequences: "idle:4,spinup:6,spin:6,fire:6,fire2:10,draw:4",
    },
    WeaponModel {
        name: "v_grenade",
        // Redistribute the pose budget toward the fast release arc.
        // The throw has a sharp quality knee at 14 poses (3.37 -> 0.87 RMS);
        // a static idle tolerates two. Two extra pin-pull samples halve that
        // clip's peak reconstruction error without affecting render cost.
        sequences: "idle:2,throw1:14,pinpull:7,draw:5",
    },
    WeaponModel {
        name: "v_hgun",
        // All three clips fit at their complete source-pose counts.
        sequences: "idle1:31,Shoot:11,up:31",
    },
    WeaponModel {
        name: "v_rpg",
        sequences: "idle:3,fire:6,reload:16,draw1:5",
    },
    WeaponModel {
        name: "v_satchel",
        sequences: "idle1:4,drop:16,draw:25",
    },
    WeaponModel {
        name: "v_satchel_radio",
        // Fire and draw retain every authored source pose.
        sequences: "idle1:4,fire:31,draw:19",
    },
    WeaponModel {
        name: "v_shotgun",
        // Shotgun reloads are staged in the SDK: move aside, insert each
        // shell, then pump.  Keep all three authored clips; treating `reload`
        // as a magazine swap both looked wrong and filled the tube at once.
        // The double-shot recoil has a second quality knee at 15 poses.
        sequences: "sm_idle:3,shoot:9,shoot_big:15,reload:10,pump:9,start_reload:6,draw:5",
    },
    WeaponModel {
        name: "v_squeak",
        sequences: "idle1:3,throw:14,up:20",
    },
    WeaponModel {
        name: "v_tripmine",
        sequences: "idle1:4,place:11,arm1:30,draw:8",
    },
];

// Keep these synchronized with game/src/main.rs. The audit below derives the
// projected-vertex scratch from the cooked roster, just like game/build.rs, so
// growth in either a weapon or an enemy cannot silently squeeze live weapon
// geometry out of the shared 81 KiB reserve.
const VIEWMODEL_POOL_WORDS: usize = 20_224;
const VIEWMODEL_SORT_TRIS: usize = 1_152;
const VIEWMODEL_SORT_BUCKETS: usize = 64;
const VIEWMODEL_SLOT_CAP: usize = 40;
const MODEL_INDEX_VERTEX_LIMIT: usize = 1_024;
const HMD_FLAG_PACKED_NORMALS: u16 = 1 << 2;
const CARRY_MODEL_CHUNK_BASE: usize = 1500;
const CARRY_MODEL_TEXTURE_CHUNK_BASE: usize = 1700;
// Gargantua quality variants for the two rooms whose other live models leave
// less RAM than the ordinary enhanced stream. c4a1b keeps the former baseline;
// c4a3 contains a distant Garg alongside Nihilanth and uses one pose per state.
const C4A1B_GARG_MODEL_CHUNK: usize = 1817;
const C4A1B_GARG_TEXTURE_CHUNK: usize = 1917;
const C4A1B_GARG_SEQUENCES: &str = "2:2,4:2,6:2,12:3,14:4";
const C4A3_GARG_MODEL_CHUNK: usize = 1816;
const C4A3_GARG_TEXTURE_CHUNK: usize = 1916;
const C4A3_GARG_SEQUENCES: &str = "2:1,4:1,6:1,12:1,14:1";
const C4A3_ICKY_MODEL_CHUNK: usize = 1819;
const C4A3_ICKY_TEXTURE_CHUNK: usize = 1919;
// This incidental c4a3 actor yields its animation RAM to the actual boss.
const C4A3_ICKY_SEQUENCES: &str = "0:1,1:1,8:1,5:1,3:1";
const C1A2B_ZOMBIE_MODEL_CHUNK: usize = 1818;
const C1A2B_ZOMBIE_TEXTURE_CHUNK: usize = 1918;
// c1a2b is the campaign's crowded transient-stream peak. Keep its former
// five-pose attack while other rooms use the smoother six-pose stream.
const C1A2B_ZOMBIE_SEQUENCES: &str = "0:4,10:4,8:5,3:2,17:3,eatbody:2";
const PRESSURE_ISLAVE_MODEL_CHUNK: usize = 1820;
const PRESSURE_ISLAVE_TEXTURE_CHUNK: usize = 1920;
const PRESSURE_ISLAVE_SEQUENCES: &str = "0:4,4:4,12:4,13:2,19:3,grab:2";

fn carry_model_type(model_type: usize) -> bool {
    model_type < 56 && !matches!(model_type, 3 | 4 | 16 | 26..=50)
}

/// Carried actors resume ordinary AI after a changelevel; scripted work is
/// deliberately reset by restore_transition_actors. Preserve the five common
/// idle/move/attack/death states, with two poses per animated state. This is a
/// map-specific residency variant, not a replacement for the full map model.
fn carry_model_sequences(sequences: &str) -> String {
    sequences
        .split(',')
        .take(5)
        .map(|field| {
            let Some((sequence, samples)) = field.rsplit_once(':') else {
                return field.to_string();
            };
            let samples = samples.parse::<usize>().unwrap_or(1).clamp(1, 2);
            format!("{sequence}:{samples}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn read_u16_le(data: &[u8], offset: usize, what: &str) -> Result<u16> {
    let bytes: [u8; 2] = data
        .get(offset..offset + 2)
        .ok_or_else(|| format!("{what}: truncated u16 at byte {offset}"))?
        .try_into()?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32_le(data: &[u8], offset: usize, what: &str) -> Result<usize> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .ok_or_else(|| format!("{what}: truncated u32 at byte {offset}"))?
        .try_into()?;
    Ok(u32::from_le_bytes(bytes) as usize)
}

fn round_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

fn audit_viewmodels(model_pack: &Path) -> Result<()> {
    let mut max_model_verts = 0usize;
    for entry in fs::read_dir(model_pack)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if !name.starts_with("chunk_") || path.extension() != Some(OsStr::new("psxm")) {
            continue;
        }
        let data = fs::read(&path)?;
        if data.get(0..4) == Some(b"HMRG") && data.len() >= 16 {
            max_model_verts = max_model_verts.max(read_u32_le(&data, 12, name)?);
        }
    }
    let projected_verts = round_up(max_model_verts + 16, 16).min(MODEL_INDEX_VERTEX_LIMIT);
    let projected_words = (projected_verts * 6).div_ceil(4);
    let sort_link_words = (VIEWMODEL_SORT_TRIS * 2).div_ceil(4);
    let sort_head_words = (VIEWMODEL_SORT_BUCKETS * 2).div_ceil(4);
    let scratch_words = projected_words + sort_link_words + sort_head_words;
    let geometry_limit_words = VIEWMODEL_POOL_WORDS
        .checked_sub(scratch_words)
        .ok_or("viewmodel scratch exceeds the fixed pool")?;

    let mut glock_textures = 0usize;
    println!(
        "viewmodel RAM audit: pool={} B, geometry_end<={} B, projected_verts={}",
        VIEWMODEL_POOL_WORDS * 4,
        geometry_limit_words * 4,
        projected_verts
    );
    for (index, model) in WEAPON_MODELS.iter().enumerate() {
        let path = model_pack.join(format!("chunk_{}.psxm", 1000 + index));
        let data = fs::read(&path)?;
        if data.get(0..4) != Some(b"HMRG") || data.get(8..12) != Some(b"HMD8") {
            return Err(format!("{}: expected merged HMD8 viewmodel", path.display()).into());
        }
        if read_u16_le(&data, 38, model.name)? & HMD_FLAG_PACKED_NORMALS == 0 {
            return Err(format!(
                "{}: compact viewmodel is missing packed normals",
                model.name
            )
            .into());
        }
        let geometry_bytes = read_u32_le(&data, 4, model.name)?;
        let n_verts = read_u32_le(&data, 12, model.name)?;
        let n_tris = read_u32_le(&data, 16, model.name)?;
        // HMD8 stores studio-hitbox count in the high half of its otherwise
        // geometry-only texture word; actual texture count remains low 16.
        let n_textures = read_u32_le(&data, 20, model.name)? & 0xffff;
        if geometry_bytes + 8 > data.len() {
            return Err(format!("{}: merged geometry exceeds chunk", model.name).into());
        }
        let geometry_end_words = 2 + geometry_bytes.div_ceil(4);
        if data.len().div_ceil(4) > VIEWMODEL_POOL_WORDS {
            return Err(format!(
                "{}: merged {} B exceeds {} B viewmodel staging pool",
                model.name,
                data.len(),
                VIEWMODEL_POOL_WORDS * 4
            )
            .into());
        }
        if geometry_end_words > geometry_limit_words {
            return Err(format!(
                "{}: geometry end {} B overlaps scratch beginning at {} B",
                model.name,
                geometry_end_words * 4,
                geometry_limit_words * 4
            )
            .into());
        }
        if n_verts > projected_verts || n_tris > VIEWMODEL_SORT_TRIS {
            return Err(format!(
                "{}: {} verts / {} tris exceeds {} verts / {} tris runtime scratch",
                model.name, n_verts, n_tris, projected_verts, VIEWMODEL_SORT_TRIS
            )
            .into());
        }
        if index == 0 {
            glock_textures = n_textures;
        }
        let live_slots = if index == 0 {
            n_textures
        } else {
            glock_textures + n_textures
        };
        if live_slots > VIEWMODEL_SLOT_CAP {
            return Err(format!(
                "{}: Glock prefix + selected textures need {} of {} slots",
                model.name, live_slots, VIEWMODEL_SLOT_CAP
            )
            .into());
        }
        println!(
            "  {:<18} merged={:>5} B geom={:>5} B verts={:>3} tris={:>4} tex={:>2}",
            model.name,
            data.len(),
            geometry_bytes,
            n_verts,
            n_tris,
            n_textures
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Sdk,
    Build,
    Assets,
    Models,
    Audit,
    Regress,
    Compile,
    Pack,
    Disc,
    Install,
    Check,
}

#[derive(Debug)]
struct Options {
    action: Action,
    half_life: Option<PathBuf>,
    psoxide_source: Option<PathBuf>,
    features: Option<String>,
    regression_scenario: Option<String>,
    games_dir: Option<PathBuf>,
}

const CANONICAL_GAME_NAME: &str = "Half-Life (hl-psx)";

const HELP: &str = "hl-psx Rust build\n\n\
         USAGE:\n\
           cargo run --release -- [sdk|build|assets|models|audit|regress|compile|pack|disc|install|check] [OPTIONS]\n\n\
         ACTIONS:\n\
           sdk        hydrate the exact pinned PSoXide SDK only\n\
           build      extract/cook and create dist/hl-psx.bin/.cue (default)\n\
           assets     extract and cook all Half-Life assets, including music\n\
           models     recook model streams, then run the full RAM audit\n\
           audit      verify every cooked map/transition model RAM residency\n\
           regress    run deterministic PSoXide visual/performance scenarios\n\
           compile    compile only the PS1 executable (requires cooked assets)\n\
           pack       compile and pack dist/hl-psx.bin/.cue\n\
           disc       compile, pack, and copy the disc to --games-dir\n\
           install    full build and copy the disc to --games-dir\n\
           check      locate and validate the Half-Life installation\n\n\
         OPTIONS:\n\
           --half-life PATH   Half-Life directory, or its valve directory\n\
           --psoxide PATH     optional local PSoXide source override\n\
           --features LIST    comma-separated hl-psx Cargo features\n\
           --scenario NAME    regress only one named deterministic scenario\n\
           --games-dir PATH   destination required by disc/install; optional after regress\n\n\
         HL_DIR, PSOXIDE, and GAMES_DIR provide environment defaults.\n\
         Nothing is copied outside this repository unless --games-dir is supplied.";

fn usage() -> ! {
    eprintln!("{HELP}");
    std::process::exit(2);
}

fn help() -> ! {
    println!("{HELP}");
    std::process::exit(0);
}

fn parse_args() -> Options {
    let mut args = env::args_os().skip(1).peekable();
    let action = match args.peek().and_then(|s| s.to_str()) {
        Some("sdk") => {
            args.next();
            Action::Sdk
        }
        Some("build") => {
            args.next();
            Action::Build
        }
        Some("assets") => {
            args.next();
            Action::Assets
        }
        Some("models") => {
            args.next();
            Action::Models
        }
        Some("audit") => {
            args.next();
            Action::Audit
        }
        Some("regress") => {
            args.next();
            Action::Regress
        }
        Some("compile") => {
            args.next();
            Action::Compile
        }
        Some("pack") => {
            args.next();
            Action::Pack
        }
        Some("disc") => {
            args.next();
            Action::Disc
        }
        Some("install") => {
            args.next();
            Action::Install
        }
        Some("check") => {
            args.next();
            Action::Check
        }
        Some("help" | "-h" | "--help") => help(),
        Some(value) if !value.starts_with('-') => usage(),
        _ => Action::Build,
    };

    let mut options = Options {
        action,
        half_life: env::var_os("HL_DIR").map(PathBuf::from),
        psoxide_source: env::var_os("PSOXIDE").map(PathBuf::from),
        features: None,
        regression_scenario: None,
        games_dir: env::var_os("GAMES_DIR").map(PathBuf::from),
    };
    while let Some(arg) = args.next() {
        let Some(flag) = arg.to_str() else { usage() };
        let value = |args: &mut std::iter::Peekable<std::iter::Skip<env::ArgsOs>>| {
            args.next().unwrap_or_else(|| usage())
        };
        match flag {
            "--half-life" => options.half_life = Some(PathBuf::from(value(&mut args))),
            "--psoxide" => options.psoxide_source = Some(PathBuf::from(value(&mut args))),
            "--features" => {
                options.features = Some(value(&mut args).to_string_lossy().into_owned())
            }
            "--scenario" => {
                options.regression_scenario = Some(value(&mut args).to_string_lossy().into_owned())
            }
            "--games-dir" => options.games_dir = Some(PathBuf::from(value(&mut args))),
            "-h" | "--help" => help(),
            _ => usage(),
        }
    }
    options
}

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn run(command: &mut Command, label: &str) -> Result<()> {
    println!("\n==> {label}");
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{label} failed with {status}").into());
    }
    Ok(())
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const WORLD_PACK_SECTOR_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompressionOutcome {
    Compressed,
    NoSectorGain,
    UnsafeInPlace,
}

#[derive(Default)]
struct PackFamilyStats {
    chunks: usize,
    compressed: usize,
    unsafe_fallbacks: usize,
    raw_bytes: usize,
    stored_bytes: usize,
    raw_sectors: usize,
    stored_sectors: usize,
}

/// LZ4-wrap a chunk only when it removes at least one physical WORLD.PAK
/// sector. Saving bytes inside the same 2 KiB allocation cannot reduce a CD
/// read and would only spend PS1 CPU on decompression.
///
/// `target_capacity` is supplied for model chunks. Unlike MAP_BUF (whose exact
/// decoder margin is content-sized by game/build.rs), models share tightly
/// packed arenas. Requiring the real SDK decoder to succeed within an existing
/// raw-fit capacity makes compression incapable of consuming new RAM.
fn encode_world_pack_chunk(
    raw: &[u8],
    target_capacity: Option<usize>,
) -> (Vec<u8>, CompressionOutcome) {
    let compressed = lz4_flex::block::compress(raw);
    let stored_len = compressed.len().saturating_add(8);
    let raw_sectors = raw.len().div_ceil(WORLD_PACK_SECTOR_BYTES);
    let stored_sectors = stored_len.div_ceil(WORLD_PACK_SECTOR_BYTES);
    if stored_len >= raw.len() || stored_sectors >= raw_sectors {
        return (raw.to_vec(), CompressionOutcome::NoSectorGain);
    }
    let Ok(raw_len) = u32::try_from(raw.len()) else {
        return (raw.to_vec(), CompressionOutcome::NoSectorGain);
    };
    let mut framed = Vec::with_capacity(stored_len);
    framed.extend_from_slice(b"HLZC");
    framed.extend_from_slice(&raw_len.to_le_bytes());
    framed.extend_from_slice(&compressed);

    // With no runtime arena constraint, use disjoint source/output staging to
    // verify that the compressor and the exact pinned guest decoder agree.
    let capacity = target_capacity.unwrap_or(raw.len().saturating_add(compressed.len()));
    if capacity < raw.len() || capacity < framed.len() {
        return (raw.to_vec(), CompressionOutcome::UnsafeInPlace);
    }
    let mut verification = vec![0u8; capacity];
    verification[..framed.len()].copy_from_slice(&framed);
    if psx_pack::decompress_hlzc_in_place(&mut verification, framed.len()) != Some(raw.len())
        || verification[..raw.len()] != *raw
    {
        return (raw.to_vec(), CompressionOutcome::UnsafeInPlace);
    }
    (framed, CompressionOutcome::Compressed)
}

fn pack_chunk_id(path: &Path, rooms: bool) -> Result<Option<u32>> {
    if !path.is_file() {
        return Ok(None);
    }
    if rooms
        && !matches!(
            path.extension().and_then(OsStr::to_str),
            Some("psxc" | "psxw")
        )
    {
        return Ok(None);
    }
    let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
        return Ok(None);
    };
    let prefix = if rooms { "room_" } else { "chunk_" };
    let Some(raw_id) = stem.strip_prefix(prefix) else {
        return Ok(None);
    };
    let id = raw_id
        .parse::<u32>()
        .map_err(|_| format!("invalid WORLD.PAK chunk filename: {}", path.display()))?;
    Ok(Some(id))
}

fn stage_pack_family(
    family: &str,
    source: &Path,
    destination: &Path,
    rooms: bool,
    model_chunks: bool,
    report: &mut String,
) -> Result<PackFamilyStats> {
    fs::create_dir_all(destination)?;
    let mut paths = fs::read_dir(source)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    paths.sort();
    let mut stats = PackFamilyStats::default();
    for path in paths {
        let Some(chunk_id) = pack_chunk_id(&path, rooms)? else {
            continue;
        };
        let raw = fs::read(&path)?;
        // Selected viewmodels always stage at word zero in their fixed private
        // pool. Other model payloads must decode in exactly their raw length;
        // that conservative rule is safe at every possible shared-pool offset.
        let target_capacity = model_chunks.then_some(if (1000..1016).contains(&chunk_id) {
            VIEWMODEL_POOL_WORDS * 4
        } else {
            raw.len()
        });
        let (stored, outcome) = encode_world_pack_chunk(&raw, target_capacity);
        let file_name = path
            .file_name()
            .ok_or_else(|| format!("{} has no file name", path.display()))?;
        fs::write(destination.join(file_name), &stored)?;

        stats.chunks += 1;
        stats.compressed += usize::from(outcome == CompressionOutcome::Compressed);
        stats.unsafe_fallbacks += usize::from(outcome == CompressionOutcome::UnsafeInPlace);
        stats.raw_bytes += raw.len();
        stats.stored_bytes += stored.len();
        stats.raw_sectors += raw.len().div_ceil(WORLD_PACK_SECTOR_BYTES);
        stats.stored_sectors += stored.len().div_ceil(WORLD_PACK_SECTOR_BYTES);
        let outcome = match outcome {
            CompressionOutcome::Compressed => "compressed",
            CompressionOutcome::NoSectorGain => "raw_no_sector_gain",
            CompressionOutcome::UnsafeInPlace => "raw_in_place_safety",
        };
        report.push_str(&format!(
            "{family},{chunk_id},{},{},{},{},{},{outcome}\n",
            file_name.to_string_lossy(),
            raw.len(),
            stored.len(),
            raw.len().div_ceil(WORLD_PACK_SECTOR_BYTES),
            stored.len().div_ceil(WORLD_PACK_SECTOR_BYTES),
        ));
    }
    Ok(stats)
}

/// Build a local, disposable WORLD.PAK source tree. This keeps compression in
/// the Rust-only hl-psx pipeline and avoids modifying source/cooked assets or
/// relying on mkisopsx's older room-only policy.
fn stage_world_pack(repository: &Path) -> Result<PathBuf> {
    let staging = repository.join(".hlpsx/world-pack");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let families = [
        ("rooms", true, false),
        ("modelpack", false, true),
        ("sfx", false, false),
        ("voices", false, false),
        ("sprites", false, false),
    ];
    println!("\nWORLD.PAK sector-aware compression:");
    println!(
        "{:<10} {:>7} {:>7} {:>12} {:>12} {:>9}",
        "family", "chunks", "packed", "raw sectors", "disc sectors", "saved"
    );
    let mut report = String::from(
        "family,chunk_id,file,raw_bytes,stored_bytes,raw_sectors,stored_sectors,outcome\n",
    );
    let mut total = PackFamilyStats::default();
    for (family, rooms, model_chunks) in families {
        let stats = stage_pack_family(
            family,
            &repository.join("data").join(family),
            &staging.join(family),
            rooms,
            model_chunks,
            &mut report,
        )?;
        let saved = stats.raw_sectors.saturating_sub(stats.stored_sectors);
        println!(
            "{family:<10} {:>7} {:>7} {:>12} {:>12} {:>9}",
            stats.chunks, stats.compressed, stats.raw_sectors, stats.stored_sectors, saved
        );
        if stats.unsafe_fallbacks > 0 {
            println!(
                "  {family}: {} compressible chunk(s) kept raw for in-place RAM safety",
                stats.unsafe_fallbacks
            );
        }
        total.chunks += stats.chunks;
        total.compressed += stats.compressed;
        total.unsafe_fallbacks += stats.unsafe_fallbacks;
        total.raw_bytes += stats.raw_bytes;
        total.stored_bytes += stats.stored_bytes;
        total.raw_sectors += stats.raw_sectors;
        total.stored_sectors += stats.stored_sectors;
    }
    let saved = total.raw_sectors.saturating_sub(total.stored_sectors);
    let percent = if total.raw_sectors == 0 {
        0.0
    } else {
        saved as f64 * 100.0 / total.raw_sectors as f64
    };
    println!(
        "{:<10} {:>7} {:>7} {:>12} {:>12} {:>9} ({percent:.1}%)",
        "TOTAL", total.chunks, total.compressed, total.raw_sectors, total.stored_sectors, saved
    );
    println!(
        "WORLD.PAK payload bytes: {} -> {}",
        total.raw_bytes, total.stored_bytes
    );
    let report_dir = repository.join(".hlpsx/reports");
    fs::create_dir_all(&report_dir)?;
    let report_path = report_dir.join("world-pack-compression.csv");
    fs::write(&report_path, report)?;
    println!("WORLD.PAK compression report -> {}", report_path.display());
    Ok(staging)
}

fn executable(path: PathBuf) -> PathBuf {
    if cfg!(windows) {
        path.with_extension("exe")
    } else {
        path
    }
}

fn maps() -> Vec<&'static str> {
    MAP_LIST
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn valve_dir(path: &Path) -> Option<PathBuf> {
    let direct = path.join("maps/c0a0.bsp");
    if direct.is_file() {
        return Some(path.to_path_buf());
    }
    let nested = path.join("valve");
    if nested.join("maps/c0a0.bsp").is_file() {
        return Some(nested);
    }
    None
}

fn half_life_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home_dir() {
        paths.push(home.join("Library/Application Support/Steam/steamapps/common/Half-Life"));
        paths.push(home.join(".local/share/Steam/steamapps/common/Half-Life"));
        paths.push(home.join(".steam/steam/steamapps/common/Half-Life"));
    }
    if let Some(program_files) = env::var_os("ProgramFiles(x86)").map(PathBuf::from) {
        paths.push(program_files.join("Steam/steamapps/common/Half-Life"));
    }
    paths
}

fn resolve_half_life(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return valve_dir(path).ok_or_else(|| {
            format!(
                "Half-Life assets not found under {} (expected valve/maps/c0a0.bsp)",
                path.display()
            )
            .into()
        });
    }
    for candidate in half_life_candidates() {
        if let Some(valve) = valve_dir(&candidate) {
            return Ok(valve);
        }
    }
    Err("Half-Life was not found in a standard Steam location; pass --half-life PATH".into())
}

fn count_extension(directory: &Path, extension: &str) -> usize {
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
        })
        .count()
}

fn check_assets(valve: &Path) -> Result<()> {
    for relative in ["maps/c0a0.bsp", "models/v_crowbar.mdl", "sound"] {
        if !valve.join(relative).exists() {
            return Err(format!("missing required Half-Life asset: {relative}").into());
        }
    }
    println!("Half-Life assets: {}", valve.display());
    println!(
        "  BSP maps : {}",
        count_extension(&valve.join("maps"), "bsp")
    );
    println!(
        "  MDL files: {}",
        count_extension(&valve.join("models"), "mdl")
    );
    println!("  WAD files: {}", count_extension(valve, "wad"));
    println!("  PAK files: {}", count_extension(valve, "pak"));
    Ok(())
}

/// Hydrate the pinned PSoXide, or a working tree when `--psoxide` names one.
///
/// This used to be four functions here -- a cargo metadata probe, a skip list,
/// a recursive copy and a marker check -- and every other game on the demo disc
/// had grown its own answer to the same question. It lives in the SDK now, so
/// there is one of it. `--psoxide` is what the demo disc passes to put every
/// program it presses on a single tree.
fn prepare_psoxide(explicit: Option<&Path>) -> Result<PathBuf> {
    let destination = root().join(".psoxide");
    match explicit {
        Some(path) => {
            let path = path.canonicalize()?;
            psoxide_link::hydrate(&path, &destination, None, false)?;
        }
        None => psoxide_link::hydrate_pinned(&destination, psoxide_rev(), true)?,
    }
    Ok(destination)
}

struct HostBins {
    bsp: PathBuf,
    content: PathBuf,
}

fn build_host_tools(repository: &Path) -> Result<HostBins> {
    let bsp_manifest = repository.join("host/hl-bsp/Cargo.toml");
    run(
        Command::new(cargo())
            .current_dir(repository)
            .args(["build", "--release", "--manifest-path"])
            .arg(&bsp_manifest),
        "build Rust BSP/model cooker",
    )?;
    let content_manifest = repository.join("host/hl-content/Cargo.toml");
    run(
        Command::new(cargo())
            .current_dir(repository)
            .args(["build", "--release", "--manifest-path"])
            .arg(&content_manifest),
        "build Rust content compiler",
    )?;
    Ok(HostBins {
        bsp: executable(repository.join("host/hl-bsp/target/release/hl-bsp")),
        content: executable(repository.join("host/hl-content/target/release/hl-content")),
    })
}

fn remove_matching(directory: &Path, prefix: &str, extensions: &[&str]) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        let extension_matches = path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| extensions.contains(&ext));
        if name.starts_with(prefix) && extension_matches {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn run_content(binary: &Path, args: &[&OsStr], label: &str) -> Result<()> {
    let mut command = Command::new(binary);
    command.args(args);
    run(&mut command, label)
}

fn cook_models(repository: &Path, valve: &Path, bins: &HostBins) -> Result<()> {
    let model_pack = repository.join("data/modelpack");
    fs::create_dir_all(repository.join("data/models"))?;
    fs::create_dir_all(&model_pack)?;
    remove_matching(&model_pack, "chunk_", &["psxm"])?;
    remove_matching(&model_pack, "anim_", &["csv"])?;

    for (index, model) in WEAPON_MODELS.iter().enumerate() {
        let geometry = model_pack.join(format!("chunk_{}.psxm", 1000 + index));
        let texture = model_pack.join(format!("chunk_{}.psxm", 2000 + index));
        run(
            Command::new(&bins.bsp)
                .arg("--mdl7-vm")
                .arg(valve.join("models").join(format!("{}.mdl", model.name)))
                .arg(&geometry)
                .arg(model.sequences)
                .arg(&texture)
                .arg(model_pack.join(format!("anim_weapon_{index}.csv"))),
            &format!("cook viewmodel {}", model.name),
        )?;
        run_content(
            &bins.content,
            &[
                OsStr::new("merge-model"),
                geometry.as_os_str(),
                texture.as_os_str(),
            ],
            &format!("merge viewmodel {}", model.name),
        )?;
    }

    let roster_source = repository.join("host/hl-content/model-roster.txt");
    let roster = model_pack.join("roster.txt");
    fs::copy(&roster_source, &roster)?;
    for raw in fs::read_to_string(&roster_source)?.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.splitn(3, '|');
        let model_type: usize = fields.next().ok_or("roster type missing")?.parse()?;
        let model = fields.next().ok_or("roster model missing")?;
        let sequences = fields.next().ok_or("roster sequences missing")?;
        let geometry = model_pack.join(format!("chunk_{}.psxm", 1300 + model_type));
        let texture = model_pack.join(format!("chunk_{}.psxm", 1100 + model_type));
        run(
            Command::new(&bins.bsp)
                .arg("--mdl7")
                .arg(valve.join("models").join(format!("{model}.mdl")))
                .arg(&geometry)
                .arg(sequences)
                .arg(&texture)
                .arg(model_pack.join(format!("anim_npc_{model_type}.csv"))),
            &format!("cook model T{model_type} {model}"),
        )?;
        // Human bodygroups retain every head/weapon variant in one geometry
        // stream. Keep those and the large construction loader split like
        // G-Man so the runtime can overwrite the dead triangle tail with the
        // texture chunk instead of requiring both to coexist in MODEL_BUF.
        if !matches!(model_type, 0 | 1 | 15 | 16 | 17 | 25 | 52 | 54) {
            run_content(
                &bins.content,
                &[
                    OsStr::new("merge-model"),
                    geometry.as_os_str(),
                    texture.as_os_str(),
                ],
                &format!("merge model T{model_type} {model}"),
            )?;
        }
        if carry_model_type(model_type) {
            let carry_geometry = model_pack.join(format!(
                "chunk_{}.psxm",
                CARRY_MODEL_CHUNK_BASE + model_type
            ));
            let carry_texture = model_pack.join(format!(
                "chunk_{}.psxm",
                CARRY_MODEL_TEXTURE_CHUNK_BASE + model_type
            ));
            let carry_sequences = carry_model_sequences(sequences);
            run(
                Command::new(&bins.bsp)
                    .arg("--mdl7")
                    .arg(valve.join("models").join(format!("{model}.mdl")))
                    .arg(&carry_geometry)
                    .arg(&carry_sequences)
                    .arg(&carry_texture),
                &format!("cook carry model T{model_type} {model}"),
            )?;
            run_content(
                &bins.content,
                &[
                    OsStr::new("merge-model"),
                    carry_geometry.as_os_str(),
                    carry_texture.as_os_str(),
                ],
                &format!("merge carry model T{model_type} {model}"),
            )?;
        }
        if model_type == 16 {
            let c4a1b_geometry = model_pack.join(format!("chunk_{C4A1B_GARG_MODEL_CHUNK}.psxm"));
            let c4a1b_texture = model_pack.join(format!("chunk_{C4A1B_GARG_TEXTURE_CHUNK}.psxm"));
            run(
                Command::new(&bins.bsp)
                    .arg("--mdl7")
                    .arg(valve.join("models").join(format!("{model}.mdl")))
                    .arg(&c4a1b_geometry)
                    .arg(C4A1B_GARG_SEQUENCES)
                    .arg(&c4a1b_texture),
                "cook c4a1b baseline Gargantua variant",
            )?;
            let geometry = model_pack.join(format!("chunk_{C4A3_GARG_MODEL_CHUNK}.psxm"));
            let texture = model_pack.join(format!("chunk_{C4A3_GARG_TEXTURE_CHUNK}.psxm"));
            run(
                Command::new(&bins.bsp)
                    .arg("--mdl7-lean")
                    .arg(valve.join("models").join(format!("{model}.mdl")))
                    .arg(&geometry)
                    .arg(C4A3_GARG_SEQUENCES)
                    .arg(&texture),
                "cook c4a3 lean Gargantua variant",
            )?;
        }
        if model_type == 19 {
            let geometry = model_pack.join(format!("chunk_{C4A3_ICKY_MODEL_CHUNK}.psxm"));
            let texture = model_pack.join(format!("chunk_{C4A3_ICKY_TEXTURE_CHUNK}.psxm"));
            run(
                Command::new(&bins.bsp)
                    .arg("--mdl7")
                    .arg(valve.join("models").join(format!("{model}.mdl")))
                    .arg(&geometry)
                    .arg(C4A3_ICKY_SEQUENCES)
                    .arg(&texture),
                "cook c4a3 baseline Ichthyosaur variant",
            )?;
        }
        if model_type == 5 {
            let geometry = model_pack.join(format!("chunk_{C1A2B_ZOMBIE_MODEL_CHUNK}.psxm"));
            let texture = model_pack.join(format!("chunk_{C1A2B_ZOMBIE_TEXTURE_CHUNK}.psxm"));
            run(
                Command::new(&bins.bsp)
                    .arg("--mdl7")
                    .arg(valve.join("models").join(format!("{model}.mdl")))
                    .arg(&geometry)
                    .arg(C1A2B_ZOMBIE_SEQUENCES)
                    .arg(&texture),
                "cook c1a2b baseline zombie variant",
            )?;
        }
        if model_type == 9 {
            let geometry = model_pack.join(format!("chunk_{PRESSURE_ISLAVE_MODEL_CHUNK}.psxm"));
            let texture = model_pack.join(format!("chunk_{PRESSURE_ISLAVE_TEXTURE_CHUNK}.psxm"));
            run(
                Command::new(&bins.bsp)
                    .arg("--mdl7")
                    .arg(valve.join("models").join(format!("{model}.mdl")))
                    .arg(&geometry)
                    .arg(PRESSURE_ISLAVE_SEQUENCES)
                    .arg(&texture),
                "cook crowded-map baseline Vortigaunt variant",
            )?;
        }
    }
    audit_viewmodels(&model_pack)?;
    let clips = model_pack.join("clips.txt");
    run_content(
        &bins.content,
        &[
            OsStr::new("clips"),
            valve.join("models").as_os_str(),
            roster.as_os_str(),
            clips.as_os_str(),
        ],
        "generate model clip manifest",
    )?;
    let events = model_pack.join("studio_events.txt");
    let models_dir = valve.join("models");
    run_content(
        &bins.content,
        &[
            OsStr::new("studio-events"),
            models_dir.as_os_str(),
            roster.as_os_str(),
            events.as_os_str(),
        ],
        "generate studio event manifest",
    )?;
    fs::remove_file(roster)?;
    Ok(())
}

fn cook_rooms(repository: &Path, valve: &Path, bins: &HostBins) -> Result<()> {
    let rooms = repository.join("data/rooms");
    let model_pack = repository.join("data/modelpack");
    let voices = repository.join("data/voices");
    let sprites = repository.join("data/sprites");
    fs::create_dir_all(&rooms)?;
    remove_matching(&rooms, "room_", &["psxc", "psxw"])?;

    let report_dir = repository.join(".hlpsx/reports");
    fs::create_dir_all(&report_dir)?;
    let subdivision_report = report_dir.join("world-subdivision.csv");
    let entity_coverage_report = report_dir.join("entity-coverage.csv");
    fs::write(
        &subdivision_report,
        "map_index,map,source_vertices,source_fan_triangles,pre_grid_vertices,pre_grid_triangles,grid_added_vertices,grid_added_triangles,cooked_vertices,cooked_triangles,candidate_faces,grid_refined_faces,grid_cells,grid_quad_cells,grid_boundary_triangles,grid_budget_fallbacks,liquid_planes,coplanar_liquid_faces,tjunction_triangles,resident_bytes,texture_bytes,native_patch_faces,native_patch_quad_records,native_patch_triangle_records,native_patch_blocked_records,native_patch_seamed_pair_records,world_cells,world_source_faces,world_patches,world_cell_vertices,world_unique_vertices,world_max_visible_cells,world_max_visible_vertices,world_max_visible_packets,world_max_neighbor_packets,world_max_exact_cells,world_max_exact_vertices,world_max_exact_packets,world_min_exact_fill_x100,world_fallback_patches\n",
    )?;
    fs::write(&entity_coverage_report, "map,classname,count,status\n")?;

    let map_names = maps();
    let maps_dir = valve.join("maps");
    let transition_props = model_pack.join("transition_props.txt");
    let mut transition = Command::new(&bins.content);
    transition
        .arg("transition-props")
        .arg(&maps_dir)
        .arg(&transition_props)
        .args(&map_names);
    run(&mut transition, "generate transition prop hints")?;
    model_variants::cook_map_variants(repository, valve, bins, &map_names)?;

    for (index, map) in map_names.iter().enumerate() {
        let world = rooms.join(format!("room_{}.psxc", index * 2));
        let textures = rooms.join(format!("room_{}.psxc", index * 2 + 1));
        let status = Command::new(&bins.bsp)
            .arg("--cook")
            .arg(maps_dir.join(format!("{map}.bsp")))
            .arg(&world)
            .arg(&textures)
            .env(
                "CLIPS_MANIFEST",
                model_variants::clips_manifest(repository, index),
            )
            .env(
                "STUDIO_EVENTS_MANIFEST",
                model_pack.join("studio_events.txt"),
            )
            .env("VOICES_MANIFEST", voices.join("manifest.txt"))
            .env("SPRITES_MANIFEST", sprites.join("manifest.txt"))
            .env("TRANSITION_PROPS_MANIFEST", &transition_props)
            .env("SUBDIVISION_REPORT", &subdivision_report)
            .env("ENTITY_COVERAGE_REPORT", &entity_coverage_report)
            .env("ENTITY_COVERAGE_STRICT", "1")
            .env("MAP_NAME", map)
            .env("MAP_INDEX", index.to_string())
            .stdout(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(format!("cook map {map} failed with {status}").into());
        }
        println!("  room_{}/{} = {map}", index * 2, index * 2 + 1);
    }
    validate_world_subdivision_report(&subdivision_report, &map_names)?;
    validate_entity_coverage_report(&entity_coverage_report, &map_names)?;
    println!(
        "world subdivision audit -> {}",
        subdivision_report.display()
    );
    println!(
        "entity support audit -> {}",
        entity_coverage_report.display()
    );
    Ok(())
}

fn validate_entity_coverage_report(path: &Path, map_names: &[&str]) -> Result<()> {
    let report = fs::read_to_string(path)?;
    let mut rows = report.lines();
    if rows.next() != Some("map,classname,count,status") {
        return Err("entity coverage report has an unexpected header".into());
    }

    let mut covered_maps = BTreeSet::new();
    let mut unknown = Vec::new();
    for row in rows {
        let fields = row.split(',').collect::<Vec<_>>();
        if fields.len() != 4 {
            return Err(format!("malformed entity coverage row: {row}").into());
        }
        covered_maps.insert(fields[0]);
        if fields[3] == "unknown" {
            unknown.push(format!("{}:{}", fields[0], fields[1]));
        }
    }
    if !unknown.is_empty() {
        return Err(format!("unknown campaign entity classes: {}", unknown.join(", ")).into());
    }
    let expected = map_names.iter().copied().collect::<BTreeSet<_>>();
    if covered_maps != expected {
        return Err(format!(
            "entity coverage report maps differ: expected {}, got {}",
            expected.len(),
            covered_maps.len()
        )
        .into());
    }
    Ok(())
}

fn validate_world_subdivision_report(path: &Path, map_names: &[&str]) -> Result<()> {
    let report = fs::read_to_string(path)?;
    let mut rows = report.lines();
    let header = rows.next().ok_or("world subdivision report is empty")?;
    if header.split(',').count() != 40 {
        return Err(format!("world subdivision report has an unexpected header: {header}").into());
    }

    let rows = rows.collect::<Vec<_>>();
    if rows.len() != map_names.len() {
        return Err(format!(
            "world subdivision report has {} map rows, expected {}",
            rows.len(),
            map_names.len()
        )
        .into());
    }
    for (expected_index, (expected_map, row)) in map_names.iter().zip(&rows).enumerate() {
        let columns = row.split(',').collect::<Vec<_>>();
        if columns.len() != 40
            || columns[0].parse::<usize>() != Ok(expected_index)
            || columns[1] != *expected_map
        {
            return Err(format!(
                "world subdivision report row {} does not match map {}: {}",
                expected_index, expected_map, row
            )
            .into());
        }
        let number = |index: usize| -> Result<usize> {
            columns[index].parse::<usize>().map_err(|_| {
                format!(
                    "world subdivision report has a non-numeric field for map {}: {}",
                    expected_map, row
                )
                .into()
            })
        };
        let pre_grid_vertices = number(4)?;
        let pre_grid_triangles = number(5)?;
        let added_vertices = number(6)?;
        let added_triangles = number(7)?;
        let cooked_vertices = number(8)?;
        let cooked_triangles = number(9)?;
        let candidate_faces = number(10)?;
        let refined_faces = number(11)?;
        let grid_cells = number(12)?;
        let grid_quad_cells = number(13)?;
        let native_patch_faces = number(21)?;
        let native_patch_quad_records = number(22)?;
        let native_patch_triangle_records = number(23)?;
        let native_patch_blocked_records = number(24)?;
        let native_patch_seamed_pair_records = number(25)?;
        let world_cells = number(26)?;
        let world_source_faces = number(27)?;
        let world_patches = number(28)?;
        let world_cell_vertices = number(29)?;
        let world_unique_vertices = number(30)?;
        let world_max_visible_cells = number(31)?;
        let world_max_visible_vertices = number(32)?;
        let world_max_visible_packets = number(33)?;
        let world_max_neighbor_packets = number(34)?;
        let world_max_exact_cells = number(35)?;
        let world_max_exact_vertices = number(36)?;
        let world_max_exact_packets = number(37)?;
        let world_min_exact_fill_x100 = number(38)?;
        let world_fallback_patches = number(39)?;
        if cooked_vertices > 12_288
            || added_triangles > 1_536
            || pre_grid_vertices + added_vertices != cooked_vertices
            || pre_grid_triangles + added_triangles != cooked_triangles
            || refined_faces > candidate_faces
            || grid_quad_cells > grid_cells
            || grid_cells < refined_faces
            || (refined_faces == 0) != (added_triangles == 0)
            || native_patch_faces == 0
            || native_patch_quad_records == 0
            || native_patch_blocked_records > native_patch_quad_records
            || native_patch_seamed_pair_records * 2 > native_patch_triangle_records
            || native_patch_quad_records + native_patch_triangle_records < native_patch_faces
            || world_cells == 0
            || world_source_faces == 0
            || world_patches == 0
            || world_source_faces > native_patch_faces
            || world_patches > native_patch_quad_records
            || world_unique_vertices > world_cell_vertices
            || world_max_visible_cells > world_cells
            || world_max_visible_vertices > world_cell_vertices
            || world_max_visible_packets > world_patches
            || world_max_neighbor_packets < world_max_visible_packets
            || world_max_neighbor_packets > world_patches
            || world_max_exact_cells > world_max_visible_cells
            || world_max_exact_vertices > world_max_visible_vertices
            || world_max_exact_packets > world_max_visible_packets
            || world_max_exact_packets > 1_800
            || world_min_exact_fill_x100 > 3_200
            || world_fallback_patches > native_patch_quad_records + native_patch_triangle_records
        {
            return Err(format!(
                "world subdivision invariants failed for map {}: {}",
                expected_map, row
            )
            .into());
        }
    }
    Ok(())
}

fn cook_assets(repository: &Path, valve: &Path, psoxide: &Path) -> Result<()> {
    // A failed or interrupted recook must never leave an older provenance file
    // making the partially replaced data tree look release-ready.
    invalidate_cook_manifest(repository)?;
    let bins = build_host_tools(repository)?;
    let menu = repository.join("data/menu");
    let music = repository.join("data/music");
    let sfx = repository.join("data/sfx");
    let voices = repository.join("data/voices");
    let sprites = repository.join("data/sprites");
    let map_list = maps().join(" ");

    run_content(
        &bins.content,
        &[OsStr::new("menu"), valve.as_os_str(), menu.as_os_str()],
        "extract menu and HUD assets",
    )?;
    run_content(
        &bins.content,
        &[OsStr::new("music"), valve.as_os_str(), music.as_os_str()],
        "extract and decode CD audio",
    )?;
    fs::create_dir_all(&sfx)?;
    let sound = valve.join("sound");
    let sfx_pack = sfx.join("chunk_3000.psxa");
    run_content(
        &bins.content,
        &[OsStr::new("sfx"), sound.as_os_str(), sfx_pack.as_os_str()],
        "extract and encode sound effects",
    )?;
    fs::copy(menu.join("hud.tex"), sfx.join("chunk_3001.psxa"))?;
    fs::copy(menu.join("menu.pak"), sfx.join("chunk_3003.psxa"))?;
    run_content(
        &bins.content,
        &[
            OsStr::new("voices"),
            valve.as_os_str(),
            OsStr::new(&map_list),
            voices.as_os_str(),
        ],
        "extract and encode per-map dialogue",
    )?;
    run_content(
        &bins.content,
        &[
            OsStr::new("sprites"),
            valve.as_os_str(),
            OsStr::new(&map_list),
            sprites.as_os_str(),
        ],
        "extract sprites",
    )?;
    cook_models(repository, valve, &bins)?;
    cook_rooms(repository, valve, &bins)?;
    model_audit::audit_model_residency(repository, Some(valve), &maps())?;
    write_cook_manifest(repository, valve, psoxide)?;
    println!("\nAssets -> {}", repository.join("data").display());
    Ok(())
}

fn compile_game(repository: &Path, psoxide: &Path, features: Option<&str>) -> Result<PathBuf> {
    let game = repository.join("game");
    let mut command = Command::new(cargo());
    command.current_dir(&game).args(["build", "--release"]);
    if let Some(features) = features.filter(|value| !value.trim().is_empty()) {
        command.args(["--features", features]);
    }
    command.env("PSOXIDE", psoxide);
    run(&mut command, "compile hl-psx for PlayStation")?;
    let exe = game.join("target/mipsel-sony-psx/release/hl-psx.exe");
    if !exe.is_file() {
        return Err(format!("game build did not produce {}", exe.display()).into());
    }
    // Reroute the R3000 load-delay hazards LLVM's delay-slot filler leaves
    // behind (loads in delay slots consumed one instruction later) through the
    // guest's HAZARD_TRAMPOLINES array, then prove the image clean. Zero RAM
    // beyond that array; the alternative flag costs 31 KB of nops.
    let patcher = repository.join("host/hazard_patch.py");
    run(
        Command::new("python3").arg(&patcher).arg(&exe),
        "patch load-delay hazards in hl-psx.exe",
    )?;
    println!("EXE -> {}", exe.display());
    Ok(exe)
}

fn pack_disc(repository: &Path, psoxide: &Path, exe: &Path) -> Result<PathBuf> {
    // Compile can update generated output, so repeat the gate immediately
    // before layout. Packing is the last point at which stale ignored assets
    // can be prevented from entering a distributable image.
    verify_cook_manifest(repository, psoxide)?;
    let dist = repository.join("dist");
    fs::create_dir_all(&dist)?;
    let image = dist.join("hl-psx.bin");
    let staged_pack = stage_world_pack(repository)?;
    let mut command = Command::new(cargo());
    command
        .current_dir(psoxide)
        .args(["run", "--release", "-p", "mkisopsx", "--"])
        .arg("--exe")
        .arg(exe)
        .arg("--out")
        .arg(&image)
        .args(["--volume", "HLPSX", "--world-pack-rooms-dir"])
        .arg(staged_pack.join("rooms"));
    for directory in ["modelpack", "sfx", "voices", "sprites"] {
        command
            .arg("--world-pack-extra-dir")
            .arg(staged_pack.join(directory));
    }
    let tracks = repository.join("data/music/tracks.txt");
    if tracks.is_file() {
        command.arg("--cdda-track-list").arg(tracks);
    }
    run(&mut command, "pack PlayStation BIN/CUE disc")?;
    let cue = image.with_extension("cue");
    println!("DISC -> {}", cue.display());
    Ok(cue)
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn install_disc(cue: &Path, games_dir: &Path) -> Result<()> {
    let game_name = CANONICAL_GAME_NAME;
    let image = cue.with_extension("bin");
    let cue_text = fs::read_to_string(cue)?;
    let mut replaced = false;
    let mut output = String::new();
    for line in cue_text.lines() {
        if !replaced && line.trim_start().starts_with("FILE ") {
            output.push_str(&format!("FILE \"{game_name}.bin\" BINARY\n"));
            replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    if !replaced {
        return Err(format!("{} has no FILE directive", cue.display()).into());
    }
    fs::create_dir_all(games_dir)?;
    let target_bin = games_dir.join(format!("{game_name}.bin"));
    let target_cue = games_dir.join(format!("{game_name}.cue"));
    fs::copy(&image, &target_bin)?;
    fs::write(&target_cue, output)?;
    println!("INSTALLED -> {}", target_cue.display());
    Ok(())
}

fn main() -> Result<()> {
    let options = parse_args();
    let repository = root();
    let needs_half_life = matches!(
        options.action,
        Action::Build
            | Action::Assets
            | Action::Models
            | Action::Audit
            | Action::Install
            | Action::Check
    );
    // Host cookers consume the same draw-surface wire types as the guest.
    // Hydrate before any action that builds those tools; the previous order
    // hydrated only immediately before the guest compile.
    let psoxide = if matches!(options.action, Action::Check | Action::Audit) {
        None
    } else {
        Some(prepare_psoxide(options.psoxide_source.as_deref())?)
    };
    if options.action == Action::Sdk {
        println!(
            "PSoXide SDK {} -> {}",
            psoxide_rev(),
            psoxide
                .as_deref()
                .expect("sdk action hydrates PSoXide")
                .display()
        );
        return Ok(());
    }
    if needs_half_life {
        let valve = resolve_half_life(options.half_life.as_deref())?;
        check_assets(&valve)?;
        if options.action == Action::Check {
            return Ok(());
        }
        if options.action == Action::Audit {
            model_audit::audit_model_residency(&repository, Some(&valve), &maps())?;
            return Ok(());
        }
        if options.action == Action::Models {
            invalidate_cook_manifest(&repository)?;
            let bins = build_host_tools(&repository)?;
            cook_models(&repository, &valve, &bins)?;
            // Script clip slots are map-local now, so a model recook must also
            // refresh the dependent room logic tokens and sparse variant index.
            cook_rooms(&repository, &valve, &bins)?;
            model_audit::audit_model_residency(&repository, Some(&valve), &maps())?;
            println!(
                "model-only cook invalidated {}; run a full `assets` cook before packing",
                cook_manifest_path(&repository).display()
            );
            return Ok(());
        }
        cook_assets(
            &repository,
            &valve,
            psoxide
                .as_deref()
                .expect("asset builds hydrate PSoXide before cooking"),
        )?;
    }
    if options.action == Action::Assets {
        return Ok(());
    }

    if matches!(
        options.action,
        Action::Compile | Action::Pack | Action::Disc | Action::Regress
    ) {
        verify_cook_manifest(
            &repository,
            psoxide
                .as_deref()
                .expect("compile and pack actions hydrate PSoXide"),
        )?;
        model_audit::audit_model_residency(&repository, None, &maps())?;
    }

    let psoxide = psoxide.expect("all build actions hydrate PSoXide");
    let regression_features;
    let features = if options.action == Action::Regress {
        let mut required =
            "performance-telemetry,debug-map-boot,debug-regression-viewpoints".to_string();
        if regression::needs_weapon_gallery(options.regression_scenario.as_deref()) {
            required.push_str(",debug-weapon-gallery");
        }
        regression_features = match options.features.as_deref() {
            Some(extra) if !extra.trim().is_empty() => {
                format!("{required},{extra}")
            }
            _ => required,
        };
        Some(regression_features.as_str())
    } else {
        options.features.as_deref()
    };
    let exe = compile_game(&repository, &psoxide, features)?;
    if options.action == Action::Compile {
        return Ok(());
    }
    let cue = pack_disc(&repository, &psoxide, &exe)?;
    if options.action == Action::Regress {
        // The game links against the hydrated SDK copy above, while the
        // frontend runs directly from an explicit development checkout so its
        // Cargo cache survives and the matrix always exercises the user's
        // latest emulator code.
        let regression_psoxide = options
            .psoxide_source
            .as_deref()
            .unwrap_or(psoxide.as_path());
        let regression_result = regression::run_matrix(
            &repository,
            regression_psoxide,
            &cue,
            options.regression_scenario.as_deref(),
        );
        // Leave dist/ as the shipping artifact, not the instrumented test disc.
        let shipping_exe = compile_game(&repository, &psoxide, None)?;
        pack_disc(&repository, &psoxide, &shipping_exe)?;
        regression_result?;
        if let Some(games_dir) = options.games_dir.as_deref() {
            install_disc(&cue, games_dir)?;
        }
        return Ok(());
    }
    if matches!(options.action, Action::Disc | Action::Install) {
        let games_dir = options
            .games_dir
            .ok_or("disc/install requires --games-dir PATH or GAMES_DIR")?;
        install_disc(&cue, &games_dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn psoxide_pin_has_one_manifest_source_of_truth() {
        let rev = psoxide_rev();
        assert_eq!(rev.len(), 40);
        assert!(rev.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn tree_digest_is_ordered_and_detects_content_changes() {
        let root = std::env::temp_dir().join(format!(
            "hl-psx-provenance-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("z.bin"), b"last").unwrap();
        fs::write(root.join("nested/a.bin"), b"first").unwrap();
        let before = tree_digest(&root, None).unwrap();
        assert_eq!(before, tree_digest(&root, None).unwrap());
        fs::write(root.join("nested/a.bin"), b"changed").unwrap();
        assert_ne!(before, tree_digest(&root, None).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cooked_tree_digest_excludes_only_its_provenance_file() {
        let root = std::env::temp_dir().join(format!(
            "hl-psx-cooked-provenance-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("room.psxc"), b"room").unwrap();
        fs::write(root.join(".hlpsx-cook.json"), b"one").unwrap();
        let first = tree_digest(&root, Some(Path::new(".hlpsx-cook.json"))).unwrap();
        fs::write(root.join(".hlpsx-cook.json"), b"two").unwrap();
        assert_eq!(
            first,
            tree_digest(&root, Some(Path::new(".hlpsx-cook.json"))).unwrap()
        );
        fs::write(root.join("room.psxc"), b"changed").unwrap();
        assert_ne!(
            first,
            tree_digest(&root, Some(Path::new(".hlpsx-cook.json"))).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn campaign_registry_is_complete_and_unique() {
        let maps = maps();
        assert_eq!(maps.len(), 103);
        assert_eq!(maps.iter().copied().collect::<HashSet<_>>().len(), 103);
        assert_eq!(maps.first(), Some(&"c0a0"));
        assert_eq!(maps.get(95), Some(&"c5a1"));
        assert_eq!(maps.get(96), Some(&"t0a0"));
        assert_eq!(maps.last(), Some(&"t0a0d"));
    }

    #[test]
    fn subdivision_report_requires_every_map_in_campaign_order() {
        let temp = std::env::temp_dir().join(format!(
            "hl-psx-subdivision-report-{}-{}.csv",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let header = (0..40)
            .map(|index| format!("h{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let row = |index: usize, map: &str| {
            let mut columns = vec![index.to_string(), map.to_string()];
            columns.extend((2..40).map(|_| "0".to_string()));
            columns[21] = "1".to_string();
            columns[22] = "1".to_string();
            for column in &mut columns[26..40] {
                *column = "1".to_string();
            }
            columns.join(",")
        };
        fs::write(
            &temp,
            format!("{header}\n{}\n{}\n", row(0, "a"), row(1, "b")),
        )
        .unwrap();
        assert!(validate_world_subdivision_report(&temp, &["a", "b"]).is_ok());
        assert!(validate_world_subdivision_report(&temp, &["b", "a"]).is_err());
        assert!(validate_world_subdivision_report(&temp, &["a"]).is_err());
        fs::remove_file(temp).unwrap();
    }

    #[test]
    fn entity_coverage_report_rejects_unknown_or_missing_maps() {
        let temp = std::env::temp_dir().join(format!(
            "hl-psx-entity-coverage-{}-{}.csv",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(
            &temp,
            "map,classname,count,status\na,worldspawn,1,runtime\nb,func_door,2,runtime\n",
        )
        .unwrap();
        assert!(validate_entity_coverage_report(&temp, &["a", "b"]).is_ok());
        assert!(validate_entity_coverage_report(&temp, &["a"]).is_err());
        fs::write(
            &temp,
            "map,classname,count,status\na,new_entity,1,unknown\n",
        )
        .unwrap();
        assert!(validate_entity_coverage_report(&temp, &["a"]).is_err());
        fs::remove_file(temp).unwrap();
    }

    #[test]
    fn pack_compression_requires_a_sector_saving() {
        let raw = vec![0x5a; 1024];
        let (stored, outcome) = encode_world_pack_chunk(&raw, None);
        assert_eq!(outcome, CompressionOutcome::NoSectorGain);
        assert_eq!(stored, raw);
    }

    #[test]
    fn packed_chunk_round_trips_through_the_guest_decoder() {
        let raw = (0..8192).map(|i| (i % 23) as u8).collect::<Vec<_>>();
        let (stored, outcome) = encode_world_pack_chunk(&raw, None);
        assert_eq!(outcome, CompressionOutcome::Compressed);
        assert!(stored.len().div_ceil(WORLD_PACK_SECTOR_BYTES) < 4);
        let mut decoded = vec![0u8; raw.len() + stored.len()];
        decoded[..stored.len()].copy_from_slice(&stored);
        assert_eq!(
            psx_pack::decompress_hlzc_in_place(&mut decoded, stored.len()),
            Some(raw.len())
        );
        assert_eq!(&decoded[..raw.len()], raw.as_slice());
    }

    #[test]
    fn model_chunk_falls_back_when_exact_raw_capacity_cannot_decode() {
        let raw = (0..8192).map(|i| (i % 23) as u8).collect::<Vec<_>>();
        let (stored, outcome) = encode_world_pack_chunk(&raw, Some(raw.len()));
        assert_eq!(outcome, CompressionOutcome::UnsafeInPlace);
        assert_eq!(stored, raw);
    }
}

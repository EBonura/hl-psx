//! Explicit, byte-checked reuse of an immutable full-cook receipt.
//!
//! Reuse preserves the original cooker environment's effects in the verified
//! cooked bytes. Historical environment values are unrecorded; this verifier
//! does not reconstruct them or run any cooker.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

const REUSE_PATH: &str = ".hlpsx/cook-reuse.json";
const DELEGATE: &str = "    if let Some(manifest) = cook_provenance::verify_reuse(repository, psoxide)? {\n        return Ok(manifest);\n    }\n";
const COOK_ROOTS: [&str; 4] = [
    "host/hl-bsp",
    "host/hl-content",
    "host/hl-build",
    "shared/hl-format",
];
const SKIP: [&str; 3] = ["target", ".git", "__pycache__"];

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ReuseReceipt {
    schema: u32,
    original_snapshot: String,
    original_revision: String,
    original_manifest_sha256: String,
    original_source_sha256: String,
    original_platform_sha256: String,
    original_log: String,
    original_log_sha256: String,
    original_tool_receipt: String,
    original_tool_receipt_sha256: String,
    historical_environment: String,
    current_revision: String,
    current_source_sha256: String,
    current_source_is_clean: bool,
    current_components_sha256: String,
    current_platform_sha256: String,
    cook_inputs_sha256: String,
    dependency_resolution_sha256: String,
    retail_input: String,
    retail_input_sha256: String,
    cooked_tree_sha256: String,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn required_path(name: &str) -> Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("set {name} for explicit compatible cook reuse").into())
}
fn verify_tool_receipt(path: &Path) -> Result<()> {
    let base = path
        .parent()
        .and_then(Path::parent)
        .ok_or("tool receipt root missing")?;
    let text = fs::read_to_string(path)?;
    if text.lines().count() == 0 {
        return Err("original tool identity receipt is empty".into());
    }
    for line in text.lines() {
        let (expected, relative) = line
            .split_once("  ")
            .ok_or("invalid original tool identity row")?;
        let relative = Path::new(relative);
        if expected.len() != 64
            || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("invalid original tool identity".into());
        }
        if hash(&fs::read(base.join(relative))?) != expected {
            return Err("original retained tool bytes differ from recorded identity".into());
        }
    }
    Ok(())
}
fn sdk_revision(repository: &Path) -> Result<String> {
    let lock: serde_json::Value =
        serde_json::from_slice(&fs::read(repository.join("components.lock.json"))?)?;
    lock["components"]["sdk"]["revision"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "SDK component revision is missing".into())
}
fn normalize_sdk_references(bytes: Vec<u8>, current: &str, original: &str) -> Result<Vec<u8>> {
    let text = String::from_utf8(bytes)?;
    let mut normalized = String::new();
    for line in text.split_inclusive('\n') {
        let sdk_dependency =
            line.starts_with("psoxide-link = {") || line.starts_with("psx-pack = {");
        let sdk_lock_source =
            line.starts_with("source = \"git+https://github.com/EBonura/PSoXide.git?");
        if sdk_dependency || sdk_lock_source {
            normalized.push_str(&line.replace(current, original));
        } else {
            normalized.push_str(line);
        }
    }
    Ok(normalized.into_bytes())
}
fn normalize_driver(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let text = String::from_utf8(bytes)?;
    let module_count = text.matches("mod cook_provenance;\n").count();
    let delegate_count = text.matches(DELEGATE).count();
    if module_count != delegate_count || module_count > 1 {
        return Err("unexpected cook-reuse delegation shape".into());
    }
    Ok(text
        .replace("mod cook_provenance;\n", "")
        .replace(DELEGATE, "")
        .into_bytes())
}
fn source_files(root: &Path, relative: &str) -> Result<Vec<PathBuf>> {
    let directory = root.join(relative);
    if !directory.is_dir() {
        return Err(format!("cook source root missing: {}", directory.display()).into());
    }
    let mut files = Vec::new();
    collect_cook_files(root, &directory, &mut files)?;
    Ok(files)
}
fn collect_cook_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| SKIP.contains(&name))
        {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(format!("unbound symbolic link in cook source: {}", path.display()).into());
        }
        if metadata.is_dir() {
            collect_cook_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}
fn recipe(
    repository: &Path,
    platform: &Path,
    original_sdk: &str,
) -> Result<BTreeMap<String, String>> {
    let sdk = sdk_revision(repository)?;
    let mut result = BTreeMap::new();
    for root in COOK_ROOTS {
        for path in source_files(repository, root)? {
            if path == Path::new("host/hl-build/cook_provenance.rs") {
                continue;
            }
            let mut bytes = fs::read(repository.join(&path))?;
            if path == Path::new("host/hl-build/main.rs") {
                bytes = normalize_driver(bytes)?;
            }
            if path == Path::new("host/hl-build/Cargo.toml")
                || path == Path::new("host/hl-build/Cargo.lock")
            {
                bytes = normalize_sdk_references(bytes, &sdk, original_sdk)?;
            }
            result.insert(format!("hl/{}", path.display()), hash(&bytes));
        }
    }
    for relative in ["rust-toolchain.toml", ".cargo/config.toml"] {
        result.insert(
            format!("hl/{relative}"),
            hash(&fs::read(repository.join(relative))?),
        );
    }
    let mut platform_files = Vec::new();
    collect_files_skipping(
        platform,
        platform,
        &mut platform_files,
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
    for path in platform_files {
        if path.file_name().is_some_and(|name| name == ".DS_Store")
            || path == Path::new(".psoxide-source")
            || path == Path::new(".components-receipt.json")
            || path.starts_with("sdk/crates/psx-segments")
            || path == Path::new("sdk/crates/psx-settings/src/lib.rs")
        {
            continue;
        }
        let mut bytes = fs::read(platform.join(&path))?;
        if path == Path::new("sdk/Cargo.toml") {
            let text = String::from_utf8(bytes)?;
            bytes = text
                .replace("    \"crates/psx-segments\",\n", "")
                .replace("psx-segments = { path = \"crates/psx-segments\" }\n", "")
                .into_bytes();
        }
        if path == Path::new("sdk/Cargo.lock") {
            bytes = String::from_utf8(bytes)?
                .replace(
                    "[[package]]\nname = \"psx-segments\"\nversion = \"0.1.0\"\n\n",
                    "",
                )
                .into_bytes();
        }
        result.insert(format!("platform/{}", path.display()), hash(&bytes));
    }
    Ok(result)
}
fn assert_original_roots_bound(snapshot: &Path) -> Result<()> {
    let listed = git_output(snapshot, &["ls-files", "-z"])?;
    let tracked: BTreeSet<PathBuf> = listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8(path.to_vec()).map(PathBuf::from))
        .collect::<std::result::Result<_, _>>()?;
    for root in COOK_ROOTS {
        for path in source_files(snapshot, root)? {
            if !tracked.contains(&path) {
                return Err(format!(
                    "original cook source was not bound by its receipt: {}",
                    path.display()
                )
                .into());
            }
        }
    }
    Ok(())
}
fn current_source_digest(repository: &Path) -> Result<String> {
    let listed = git_output(repository, &["ls-files", "-z"])?;
    let mut files: Vec<PathBuf> = listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8(path.to_vec()).map(PathBuf::from))
        .collect::<std::result::Result<_, _>>()?;
    for root in ["game", "host", "shared", "scripts", "tests", ".cargo"] {
        if repository.join(root).is_dir() {
            files.extend(source_files(repository, root)?);
        }
    }
    files.retain(|path| !path.starts_with("game/pgo"));
    digest_files(repository, files)
}

fn normalize_value(value: &mut serde_json::Value, repository: &Path) {
    match value {
        serde_json::Value::String(text) => {
            *text = text.replace(&repository.to_string_lossy().into_owned(), "{snapshot}");
        }
        serde_json::Value::Array(values) => {
            for value in values {
                normalize_value(value, repository);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                normalize_value(value, repository);
            }
        }
        _ => {}
    }
}
fn dependency_graph(
    repository: &Path,
    manifest: &Path,
    root_name: &str,
) -> Result<serde_json::Value> {
    let output = Command::new(cargo())
        .args([
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "locked cook dependency metadata failed for {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("metadata packages missing")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("metadata resolution missing")?;
    let root = packages
        .iter()
        .find(|package| package["name"] == root_name)
        .ok_or("cook root package missing")?;
    let mut pending = vec![root["id"]
        .as_str()
        .ok_or("cook root identity missing")?
        .to_owned()];
    let mut reached = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !reached.insert(id.clone()) {
            continue;
        }
        let node = nodes
            .iter()
            .find(|node| node["id"] == id)
            .ok_or("cook dependency node missing")?;
        for dependency in node["deps"]
            .as_array()
            .ok_or("cook dependency edges missing")?
        {
            if dependency["dep_kinds"]
                .as_array()
                .ok_or("dependency kinds missing")?
                .iter()
                .any(|kind| kind["kind"] != "dev")
            {
                pending.push(
                    dependency["pkg"]
                        .as_str()
                        .ok_or("dependency identity missing")?
                        .to_owned(),
                );
            }
        }
    }
    let mut graph = BTreeMap::new();
    for id in &reached {
        let package = packages
            .iter()
            .find(|package| package["id"] == *id)
            .ok_or("dependency package missing")?;
        if package["name"] == "psx-settings" || package["name"] == "psx-segments" {
            return Err("guest-only SDK exclusions entered a cooker dependency closure".into());
        }
        let node = nodes
            .iter()
            .find(|node| node["id"] == *id)
            .ok_or("dependency node missing")?;
        let mut package = package.clone();
        let mut node = node.clone();
        node["deps"]
            .as_array_mut()
            .ok_or("dependency edges missing")?
            .retain(|dependency| {
                dependency["dep_kinds"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|kind| kind["kind"] != "dev"))
            });
        normalize_value(&mut package, repository);
        normalize_value(&mut node, repository);
        let key = id.replace(&repository.to_string_lossy().into_owned(), "{snapshot}");
        graph.insert(
            key,
            serde_json::json!({ "package": package, "resolution": node }),
        );
    }
    Ok(serde_json::to_value(graph)?)
}
fn dependencies(repository: &Path, platform: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::json!({
        "bsp": dependency_graph(repository, &repository.join("host/hl-bsp/Cargo.toml"), "hl-bsp")?,
        "content": dependency_graph(repository, &repository.join("host/hl-content/Cargo.toml"), "hl-content")?,
        "viewmodels": dependency_graph(repository, &platform.join("tools/psoxide-vmcook/Cargo.toml"), "psoxide-vmcook")?,
    }))
}

/// Verify an explicit compatible reuse on every compile/pack/profile action.
/// The original full-cook manifest is returned unchanged and never rewritten.
pub(super) fn verify_reuse(repository: &Path, platform: &Path) -> Result<Option<CookManifest>> {
    let Some(snapshot) = env::var_os("HLPSX_COOK_ORIGINAL") else {
        return Ok(None);
    };
    let snapshot = PathBuf::from(snapshot).canonicalize()?;
    let retail = required_path("HLPSX_COOK_RETAIL_INPUT")?.canonicalize()?;
    let original_log = required_path("HLPSX_COOK_ORIGINAL_LOG")?.canonicalize()?;
    let tool_receipt = required_path("HLPSX_COOK_ORIGINAL_TOOLS")?.canonicalize()?;
    verify_tool_receipt(&tool_receipt)?;
    let original_bytes = fs::read(cook_manifest_path(&snapshot))?;
    let manifest: CookManifest = serde_json::from_slice(&original_bytes)?;
    if manifest.schema != COOK_MANIFEST_SCHEMA {
        return Err("unsupported original cook manifest".into());
    }
    if source_tree_digest(&snapshot)? != manifest.hl_psx_tree_sha256
        || git_revision(&snapshot)? != manifest.hl_psx_revision
    {
        return Err(
            "original HL snapshot does not match its recorded source tree and revision".into(),
        );
    }
    let original_platform = snapshot.join(".psoxide");
    if psoxide_tree_digest(&original_platform)? != manifest.psoxide_tree_sha256 {
        return Err("original platform snapshot does not match its recorded source tree".into());
    }
    assert_original_roots_bound(&snapshot)?;
    if fs::read(cook_manifest_path(repository))? != original_bytes {
        return Err("current cooked manifest differs from immutable original receipt".into());
    }
    let original_sdk = sdk_revision(&snapshot)?;
    let original_recipe = recipe(&snapshot, &original_platform, &original_sdk)?;
    let current_recipe = recipe(repository, platform, &original_sdk)?;
    if original_recipe != current_recipe {
        return Err("cook inputs changed; compatible reuse rejected".into());
    }
    let original_dependencies = dependencies(&snapshot, &original_platform)?;
    let current_dependencies = dependencies(repository, platform)?;
    if original_dependencies != current_dependencies {
        return Err(
            "cook dependency resolution or features changed; compatible reuse rejected".into(),
        );
    }
    let retail_hash = tree_digest(&retail, None)?;
    if retail_hash != manifest.half_life_input_sha256 {
        return Err("retail input bytes changed; compatible reuse rejected".into());
    }
    let cooked_hash = cooked_tree_digest(repository)?;
    if cooked_hash != manifest.cooked_tree_sha256
        || cooked_tree_digest(&snapshot)? != manifest.cooked_tree_sha256
    {
        return Err("cooked bytes changed; compatible reuse rejected".into());
    }
    let status = git_output(
        repository,
        &["status", "--porcelain", "--untracked-files=all"],
    )?;
    let receipt = ReuseReceipt {
        schema: 1,
        original_snapshot: snapshot.display().to_string(),
        original_revision: manifest.hl_psx_revision.clone(),
        original_manifest_sha256: hash(&original_bytes),
        original_source_sha256: manifest.hl_psx_tree_sha256.clone(),
        original_platform_sha256: manifest.psoxide_tree_sha256.clone(),
        original_log: original_log.display().to_string(),
        original_log_sha256: hash(&fs::read(&original_log)?),
        original_tool_receipt: tool_receipt.display().to_string(),
        original_tool_receipt_sha256: hash(&fs::read(&tool_receipt)?),
        historical_environment: "Unrecorded; unchanged cooked bytes retain its effects".to_owned(),
        current_revision: git_revision(repository)?,
        current_source_sha256: current_source_digest(repository)?,
        current_source_is_clean: status.is_empty(),
        current_components_sha256: hash(&fs::read(repository.join("components.lock.json"))?),
        current_platform_sha256: psoxide_tree_digest(platform)?,
        cook_inputs_sha256: hash(&serde_json::to_vec(&current_recipe)?),
        dependency_resolution_sha256: hash(&serde_json::to_vec(&current_dependencies)?),
        retail_input: retail.display().to_string(),
        retail_input_sha256: retail_hash,
        cooked_tree_sha256: cooked_hash,
    };
    let path = repository.join(REUSE_PATH);
    fs::create_dir_all(path.parent().ok_or("reuse receipt directory missing")?)?;
    let mut encoded = serde_json::to_vec_pretty(&receipt)?;
    encoded.push(b'\n');
    if fs::read(&path).ok().as_deref() != Some(encoded.as_slice()) {
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, encoded)?;
        fs::rename(temporary, &path)?;
    }
    println!(
        "Compatible cooked bytes verified against immutable original receipt: {}",
        path.display()
    );
    Ok(Some(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = env::temp_dir().join(format!(
                "hl-cook-provenance-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let fixture = Self(root);
            for path in COOK_ROOTS {
                fixture.write(&format!("{path}/input.rs"), b"cook input");
            }
            fixture.write(
                "components.lock.json",
                br#"{"components":{"sdk":{"revision":"original"}}}"#,
            );
            fixture.write("rust-toolchain.toml", b"nightly-pinned");
            fixture.write(".cargo/config.toml", b"strict config");
            fixture.write(".psoxide/sdk/Cargo.toml", b"[workspace]\n");
            fixture.write(".psoxide/sdk/Cargo.lock", b"version = 4\n");
            fixture.write(".psoxide/crates/psx-asset/lib.rs", b"format input");
            fixture.write(".psoxide/assets/model.asset", b"asset input");
            fixture
        }
        fn write(&self, relative: &str, bytes: &[u8]) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        fn recipe(&self) -> BTreeMap<String, String> {
            recipe(&self.0, &self.0.join(".psoxide"), "original").unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn sdk_git_lock_revision_is_normalized_without_changing_other_sources() {
        let input = b"source = \"git+https://github.com/EBonura/PSoXide.git?rev=current#current\"\nsource = \"git+https://github.com/Other/PSoXide.git?rev=current#current\"\n".to_vec();
        let expected = b"source = \"git+https://github.com/EBonura/PSoXide.git?rev=original#original\"\nsource = \"git+https://github.com/Other/PSoXide.git?rev=current#current\"\n".to_vec();
        assert_eq!(
            normalize_sdk_references(input, "current", "original").unwrap(),
            expected
        );
    }

    #[test]
    fn cook_input_mutations_and_additions_are_rejected() {
        for path in [
            "host/hl-bsp/input.rs",
            "host/hl-content/input.rs",
            "shared/hl-format/input.rs",
            ".psoxide/crates/psx-asset/lib.rs",
            ".psoxide/assets/model.asset",
            "rust-toolchain.toml",
            ".cargo/config.toml",
            ".psoxide/sdk/Cargo.lock",
            "host/hl-bsp/new-module.rs",
        ] {
            let fixture = Fixture::new();
            let before = fixture.recipe();
            fixture.write(path, b"changed input");
            assert_ne!(before, fixture.recipe(), "mutation accepted: {path}");
        }
        let fixture = Fixture::new();
        let before = fixture.recipe();
        fs::remove_file(fixture.0.join("host/hl-content/input.rs")).unwrap();
        assert_ne!(before, fixture.recipe());
    }

    #[test]
    fn guest_only_inputs_do_not_invalidate_cooked_bytes() {
        let fixture = Fixture::new();
        let before = fixture.recipe();
        fixture.write("game/src/menu.rs", b"guest menu change");
        fixture.write(
            ".psoxide/sdk/crates/psx-segments/src/lib.rs",
            b"guest selector",
        );
        fixture.write(
            ".psoxide/sdk/crates/psx-settings/src/lib.rs",
            b"guest settings",
        );
        fixture.write(".psoxide/sdk/Cargo.toml", b"[workspace]\n    \"crates/psx-segments\",\npsx-segments = { path = \"crates/psx-segments\" }\n");
        fixture.write(
            ".psoxide/sdk/Cargo.lock",
            b"version = 4\n[[package]]\nname = \"psx-segments\"\nversion = \"0.1.0\"\n\n",
        );
        assert_eq!(before, fixture.recipe());
    }

    #[test]
    fn only_exact_driver_delegation_is_normalized() {
        let original = b"fn verify() {\n}\n".to_vec();
        let changed = format!("mod cook_provenance;\nfn verify() {{\n{DELEGATE}}}\n");
        assert_eq!(normalize_driver(changed.into_bytes()).unwrap(), original);
        assert!(normalize_driver(b"mod cook_provenance;\n".to_vec()).is_err());
        assert_ne!(
            normalize_driver(b"fn verify() { unsafe_change(); }\n".to_vec()).unwrap(),
            original
        );
    }

    #[test]
    fn cooked_and_retail_byte_mutations_change_identity() {
        let fixture = Fixture::new();
        fixture.write("retail/input", b"retail");
        fixture.write("cooked/input", b"cooked");
        for root in ["retail", "cooked"] {
            let before = tree_digest(&fixture.0.join(root), None).unwrap();
            fixture.write(&format!("{root}/input"), b"mutated");
            assert_ne!(before, tree_digest(&fixture.0.join(root), None).unwrap());
        }
        assert!(source_files(&fixture.0, "missing-original-snapshot").is_err());
    }

    #[test]
    fn original_tool_identity_is_checked() {
        let fixture = Fixture::new();
        fixture.write("retained-tools/host/tool", b"retained tool");
        let identity = format!("{}  retained-tools/host/tool\n", hash(b"retained tool"));
        fixture.write("retained-tools/identities.sha256", identity.as_bytes());
        let receipt = fixture.0.join("retained-tools/identities.sha256");
        verify_tool_receipt(&receipt).unwrap();
        fixture.write("retained-tools/host/tool", b"modified tool");
        assert!(verify_tool_receipt(&receipt).is_err());
    }

    #[test]
    #[ignore = "requires explicit retained snapshot, retail input and current cooked tree"]
    fn real_compatible_reuse_preserves_original_manifest() {
        let current = required_path("HLPSX_COOK_CURRENT")
            .unwrap()
            .canonicalize()
            .unwrap();
        let original = required_path("HLPSX_COOK_ORIGINAL")
            .unwrap()
            .canonicalize()
            .unwrap();
        let before = fs::read(cook_manifest_path(&original)).unwrap();
        assert!(verify_reuse(&current, &current.join(".psoxide"))
            .unwrap()
            .is_some());
        assert_eq!(before, fs::read(cook_manifest_path(&original)).unwrap());
        assert_eq!(before, fs::read(cook_manifest_path(&current)).unwrap());
        let receipt: ReuseReceipt =
            serde_json::from_slice(&fs::read(current.join(REUSE_PATH)).unwrap()).unwrap();
        assert_eq!(receipt.original_manifest_sha256, hash(&before));
    }
}

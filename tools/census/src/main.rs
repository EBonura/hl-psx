//! Original retail-data census. Evidence is explicit; source hits are never proof.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Open,
    Close,
}
fn tokens(data: &[u8]) -> Result<Vec<Token>> {
    let decoded = match std::str::from_utf8(data) {
        Ok(s) => s.to_owned(),
        Err(_) => data.iter().map(|&b| char::from(b)).collect(),
    };
    let text = decoded.trim_end_matches('\0');
    let b = text.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        match b[i] {
            b'{' => {
                out.push(Token::Open);
                i += 1;
            }
            b'}' => {
                out.push(Token::Close);
                i += 1;
            }
            b'"' => {
                i += 1;
                let mut s = Vec::new();
                let mut closed = false;
                while i < b.len() {
                    if b[i] == b'"' {
                        i += 1;
                        closed = true;
                        break;
                    }
                    if b[i] == b'\\' && i + 1 < b.len() && matches!(b[i + 1], b'"' | b'\\') {
                        i += 1;
                    }
                    s.push(b[i]);
                    i += 1;
                }
                if !closed {
                    return Err("unterminated quoted value".into());
                }
                out.push(Token::Word(String::from_utf8(s)?));
            }
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'{' | b'}') {
                    i += 1;
                }
                out.push(Token::Word(text[start..i].to_owned()));
            }
        }
    }
    Ok(out)
}
fn entities(data: &[u8]) -> Result<Vec<BTreeMap<String, String>>> {
    let t = tokens(data)?;
    let mut i = 0;
    let mut out = Vec::new();
    while i < t.len() {
        if t[i] != Token::Open {
            return Err("entity must start with brace".into());
        }
        i += 1;
        let mut e = BTreeMap::new();
        while i < t.len() && t[i] != Token::Close {
            let (Some(Token::Word(k)), Some(Token::Word(v))) = (t.get(i), t.get(i + 1)) else {
                return Err("malformed entity key/value".into());
            };
            if e.insert(k.clone(), v.clone()).is_some() {
                return Err(format!("duplicate entity key {k}").into());
            }
            i += 2;
        }
        if t.get(i) != Some(&Token::Close) {
            return Err("unclosed entity".into());
        }
        i += 1;
        if !e.contains_key("classname") {
            return Err("entity missing classname".into());
        }
        out.push(e);
    }
    Ok(out)
}
fn entity_lump(data: &[u8]) -> Result<&[u8]> {
    if data.len() < 124 {
        return Err("truncated BSP header".into());
    }
    let read = |n| i32::from_le_bytes(data[n..n + 4].try_into().unwrap());
    if read(0) != 30 {
        return Err("expected retail BSP version30".into());
    }
    let (off, len) = (usize::try_from(read(4))?, usize::try_from(read(8))?);
    if off < 124 {
        return Err("entity lump overlaps header".into());
    }
    data.get(off..off.checked_add(len).ok_or("entity lump overflow")?)
        .ok_or_else(|| "entity lump outside BSP".into())
}
fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
#[derive(Serialize)]
struct Occurrence {
    original: String,
    index: usize,
    keys: BTreeMap<String, String>,
}
#[derive(Serialize, Default)]
struct Feature {
    category: String,
    original: Vec<Occurrence>,
    implementation: Option<Evidence>,
    evidence_error: Option<String>,
    exercised: bool,
}
#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Classification {
    Implemented,
    Missing,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Evidence {
    classification: Classification,
    source_sha256: String,
    #[serde(default)]
    additional_sources: BTreeMap<String, String>,
    file: String,
    symbol: String,
    reason: String,
    #[serde(default)]
    journey: Option<Coverage>,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Coverage {
    report: String,
    report_sha256: String,
    assertion: String,
    feature_observation: String,
}
#[derive(Serialize)]
struct Census {
    schema: u32,
    source_sha: String,
    retail_root: String,
    input_hashes: BTreeMap<String, String>,
    map_count: usize,
    entity_count: usize,
    total: usize,
    missing: usize,
    unverified: usize,
    implemented_not_exercised: usize,
    exercised: usize,
    features: BTreeMap<String, Feature>,
}
fn category(c: &str) -> &str {
    if c.starts_with("monster_") {
        "monster"
    } else if c.starts_with("weapon_") {
        "weapon"
    } else if c.starts_with("item_") || c.starts_with("ammo_") {
        "item/ammo"
    } else if c.starts_with("trigger_") {
        "trigger"
    } else if c.starts_with("func_") {
        "func"
    } else if c.starts_with("scripted_") {
        "scripted"
    } else if c.starts_with("env_") {
        "environment"
    } else {
        "entity"
    }
}
fn insert(
    features: &mut BTreeMap<String, Feature>,
    id: String,
    category: &str,
    original: String,
    index: usize,
    keys: BTreeMap<String, String>,
) {
    let f = features.entry(id).or_default();
    f.category = category.to_owned();
    f.original.push(Occurrence {
        original,
        index,
        keys,
    });
}
fn kv_leaves(data: &[u8]) -> Result<Vec<(String, String)>> {
    fn walk(
        t: &[Token],
        i: &mut usize,
        prefix: &str,
        nested: bool,
        out: &mut Vec<(String, String)>,
    ) -> Result<()> {
        while *i < t.len() {
            if t[*i] == Token::Close {
                if !nested {
                    return Err("unexpected closing resource brace".into());
                }
                *i += 1;
                return Ok(());
            }
            let Some(Token::Word(k)) = t.get(*i) else {
                return Err("resource expected key".into());
            };
            *i += 1;
            let key = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}/{k}")
            };
            match t.get(*i) {
                Some(Token::Open) => {
                    *i += 1;
                    walk(t, i, &key, true, out)?;
                }
                Some(Token::Word(v)) => {
                    out.push((key, v.clone()));
                    *i += 1;
                }
                _ => return Err("resource expected value or block".into()),
            }
        }
        if nested {
            return Err("unclosed resource brace".into());
        }
        Ok(())
    }
    let t = tokens(data)?;
    let mut out = Vec::new();
    walk(&t, &mut 0, "", false, &mut out)?;
    Ok(out)
}
fn verify_evidence(root: &Path, e: &Evidence) -> Result<bool> {
    let path = Path::new(&e.file);
    if path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err("evidence path must stay in repository".into());
    }
    let source = fs::read_to_string(root.join(path))?;
    if hash(source.as_bytes()) != e.source_sha256
        || e.reason.is_empty()
        || (e.classification == Classification::Implemented
            && (e.symbol.is_empty() || !source.contains(&e.symbol)))
    {
        return Err(format!(
            "stale explicitly reviewed implementation evidence {}",
            e.file
        )
        .into());
    }
    for (file, expected) in &e.additional_sources {
        let path = Path::new(file);
        if path.is_absolute()
            || path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
            || hash(&fs::read(root.join(path))?) != *expected
        {
            return Err(format!("reviewed execution-path source changed: {file}").into());
        }
    }
    let Some(j) = &e.journey else {
        return Ok(false);
    };
    let report = fs::read(&j.report)?;
    if hash(&report) != j.report_sha256 || j.feature_observation.is_empty() {
        return Err("coverage evidence hash or observation missing".into());
    }
    let report: serde_json::Value = serde_json::from_slice(&report)?;
    verify_coverage_receipt(root, j, &report)?;
    Ok(true)
}
fn verify_coverage_receipt(
    root: &Path,
    coverage: &Coverage,
    receipt: &serde_json::Value,
) -> Result<()> {
    if receipt["schema"] != "audited-feature-coverage-v1" {
        return Err("coverage needs explicit audited structured receipt".into());
    }
    let revision = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !revision.status.success()
        || receipt["source_revision"].as_str() != Some(String::from_utf8(revision.stdout)?.trim())
    {
        return Err("coverage source revision changed".into());
    }
    for kind in ["journey", "disc"] {
        let path = receipt[kind]["path"]
            .as_str()
            .ok_or("coverage input path missing")?;
        let expected = receipt[kind]["sha256"]
            .as_str()
            .ok_or("coverage input hash missing")?;
        if hash(&fs::read(path)?) != expected {
            return Err(format!("coverage {kind} input changed").into());
        }
    }
    let assertions = receipt["assertions"]
        .as_array()
        .ok_or("coverage assertions missing")?;
    let matching: Vec<_> = assertions
        .iter()
        .filter(|a| a["name"].as_str() == Some(&coverage.assertion))
        .collect();
    if matching.len() != 1
        || matching[0]["status"] != "PASS"
        || matching[0]["observation"].as_str() != Some(&coverage.feature_observation)
    {
        return Err("feature assertion missing, failed, duplicated or observation differs".into());
    }
    Ok(())
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 4 {
        return Err("usage: hl-census RETAIL_VALVE_ROOT PORT_REPO OUTPUT_DIR".into());
    }
    let retail = PathBuf::from(&args[1]).canonicalize()?;
    let root = PathBuf::from(&args[2]).canonicalize()?;
    let out = PathBuf::from(&args[3]);
    fs::create_dir_all(&out)?;
    let maplist = fs::read_to_string(root.join("host/hl-content/map-list.txt"))?;
    let maps: Vec<_> = maplist.lines().filter(|s| !s.is_empty()).collect();
    if maps.len() != 103 || maps.iter().collect::<BTreeSet<_>>().len() != 103 {
        return Err(
            "canonical campaign/Hazard map list must have exactly103 unique entries".into(),
        );
    }
    let mut features = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    let mut count = 0;
    for map in &maps {
        if !map.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err("unsafe map name".into());
        }
        let original = format!("maps/{map}.bsp");
        let data = fs::read(retail.join(&original))?;
        hashes.insert(original.clone(), hash(&data));
        for (index, e) in entities(entity_lump(&data)?)?.into_iter().enumerate() {
            let c = e["classname"].clone();
            insert(
                &mut features,
                format!("entity:{c}"),
                category(&c),
                original.clone(),
                index,
                e,
            );
            count += 1;
        }
    }
    let mut resources = vec![
        "GameMenu.res".to_owned(),
        "NewGameDialog.res".to_owned(),
        "LoadGameDialog.res".to_owned(),
        "SaveGameDialog.res".to_owned(),
        "DialogOptionsIngame.res".to_owned(),
    ];
    for p in fs::read_dir(retail.join("resource"))? {
        let p = p?;
        let name = p.file_name().to_string_lossy().into_owned();
        if name.starts_with("OptionsSub") && name.ends_with(".res") {
            resources.push(name);
        }
    }
    resources.sort();
    resources.dedup();
    for name in resources {
        let original = format!("resource/{name}");
        let data = fs::read(retail.join(&original))?;
        hashes.insert(original.clone(), hash(&data));
        for (index, (key, value)) in kv_leaves(&data)?.into_iter().enumerate() {
            let leaf = key.rsplit('/').next().unwrap().to_ascii_lowercase();
            if !matches!(
                leaf.as_str(),
                "command" | "cvar_name" | "label" | "labeltext" | "controlname"
            ) {
                continue;
            }
            let cat = if name.starts_with("Options") || name.starts_with("DialogOptions") {
                "option"
            } else {
                "menu"
            };
            insert(
                &mut features,
                format!("{cat}:{name}:{key}"),
                cat,
                original.clone(),
                index,
                BTreeMap::from([("key".into(), key), ("value".into(), value)]),
            );
        }
    }
    let original = "sprites/hud.txt";
    let data = fs::read(retail.join(original))?;
    hashes.insert(original.into(), hash(&data));
    for (index, line) in std::str::from_utf8(&data)?.lines().enumerate() {
        let line = line.split("//").next().unwrap().trim();
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        } // Header count and blank/comment lines.
        insert(
            &mut features,
            format!("hud:{}", fields[0]),
            "HUD",
            original.into(),
            index + 1,
            BTreeMap::from([("descriptor".into(), line.into())]),
        );
    }
    let evidence: BTreeMap<String, Evidence> =
        serde_json::from_slice(&fs::read(root.join("tools/census/evidence.json"))?)?;
    for (id, e) in evidence {
        let f = features
            .get_mut(&id)
            .ok_or_else(|| format!("evidence names absent original feature {id}"))?;
        match verify_evidence(&root, &e) {
            Ok(exercised) => {
                f.exercised = exercised;
                f.implementation = Some(e);
            }
            Err(error) => f.evidence_error = Some(error.to_string()),
        }
    }
    let missing = features
        .values()
        .filter(|f| {
            f.implementation
                .as_ref()
                .is_some_and(|e| e.classification == Classification::Missing)
        })
        .count();
    let unverified = features
        .values()
        .filter(|f| f.implementation.is_none())
        .count();
    let not_exercised = features
        .values()
        .filter(|f| {
            f.implementation
                .as_ref()
                .is_some_and(|e| e.classification == Classification::Implemented)
                && !f.exercised
        })
        .count();
    let exercised = features.values().filter(|f| f.exercised).count();
    let output = Command::new("git")
        .args([
            "-C",
            root.to_str().ok_or("non-UTF8 root")?,
            "rev-parse",
            "HEAD",
        ])
        .output()?;
    if !output.status.success() {
        return Err("cannot bind source revision".into());
    }
    let census = Census {
        schema: 1,
        source_sha: String::from_utf8(output.stdout)?.trim().into(),
        retail_root: retail.display().to_string(),
        input_hashes: hashes,
        map_count: maps.len(),
        entity_count: count,
        total: features.len(),
        missing,
        unverified,
        implemented_not_exercised: not_exercised,
        exercised,
        features,
    };
    fs::write(out.join("census.json"), serde_json::to_vec_pretty(&census)?)?;
    let mut report=format!("# Half-Life missing feature queue\n\nSource revision: `{}`. Original retail data: `{}`.\n\n103 BSP maps, {} entity instances. {} total distinct feature records: {} proven missing, {} implementation unverified, {} implemented but not exercised, {} exercised.\n\nImplemented means a reviewed port representation or execution path, not complete original behavior or parity. Implementation unverified does not prove absence. Only reviewed execution-path evidence qualifies; classname recognition and model presence do not. UI records describe original resources, not proof of original interaction semantics. HUD records describe native sprite declarations; runtime parity remains separate. No full campaign completeness claim.\n\nEvery row below needs implementation review or a feature-specific journey. Prioritize blockers in map order, then actors/combat, triggers/scripts/movers, environment, HUD and menu/options. Original occurrence keys and all 103 map SHA256 bindings and every original location are in [census.json](census.json).\n\n| Rank | Feature | State | Original where | Port evidence | Journey exercised |\n| --- | --- | --- | --- | --- | --- |\n",census.source_sha,census.retail_root,count,census.total,missing,unverified,not_exercised,exercised);
    let mut queue: Vec<_> = census
        .features
        .iter()
        .filter(|(_, f)| !f.exercised)
        .collect();
    let priority = |f: &Feature| match f.category.as_str() {
        "trigger" | "scripted" | "func" => 0,
        "monster" => 1,
        "weapon" | "item/ammo" => 2,
        "environment" => 3,
        "HUD" => 4,
        "menu" | "option" => 5,
        _ => 6,
    };
    queue.sort_by_key(|(id, f)| (priority(f), *id));
    for (rank, (id, f)) in queue.into_iter().enumerate() {
        let first = &f.original[0];
        let port = f
            .implementation
            .as_ref()
            .map(|e| format!("{}: {} ({})", e.file, e.symbol, e.reason))
            .unwrap_or_else(|| {
                f.evidence_error
                    .clone()
                    .unwrap_or_else(|| "unverified execution-path evidence".into())
            });
        report.push_str(&format!(
            "| {} | {} | {} | {} #{} ({} occurrences) | {} | no |\n",
            rank + 1,
            id,
            match f.implementation.as_ref().map(|e| &e.classification) {
                Some(Classification::Implemented) => "implemented, unexercised",
                Some(Classification::Missing) => "proven missing",
                None => "unverified",
            },
            first.original,
            first.index,
            f.original.len(),
            port.replace('|', "/")
        ));
    }
    report.push_str("\nOriginal behavior not determined from resource data: keyboard/mouse remapping semantics, graphics/backend capability, network/server browser, native save browser interoperability and sprite animation timing. These remain explicit scope-review tasks. Original models/skins not referenced by canonical BSP entities are not classified as game features from filenames alone.\n");
    fs::write(out.join("MISSING-FEATURES-hl-psx.md"), report)?;
    println!("maps={} entities={count} total={} missing={missing} unverified={unverified} implemented_not_exercised={not_exercised} exercised={exercised}",maps.len(),census.total);
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("census: {e}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_entities_fail_closed() {
        for input in [
            b"{\"classname\"}".as_slice(),
            b"{\"classname\" \"x\"",
            b"{\"classname\" \"x\" \"classname\" \"y\"}",
            b"{\"target\" \"x\"}",
        ] {
            assert!(entities(input).is_err());
        }
    }
    #[test]
    fn comments_and_quoted_keys_preserve_evidence() {
        let x = entities(
            b"//ignored\n{\"classname\" \"monster_test\" \"target\" \"quoted \\\"value\\\"\"}\0",
        )
        .unwrap();
        assert_eq!(x[0]["target"], "quoted \"value\"");
    }
    #[test]
    fn bsp_offsets_are_checked() {
        let mut b = vec![0; 124];
        b[0..4].copy_from_slice(&30i32.to_le_bytes());
        b[4..8].copy_from_slice(&124i32.to_le_bytes());
        assert_eq!(entity_lump(&b).unwrap(), b"");
        b[8..12].copy_from_slice(&1i32.to_le_bytes());
        assert!(entity_lump(&b).is_err());
        b[4..8].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(entity_lump(&b).is_err());
    }
    #[test]
    fn classname_presence_never_marks_implementation() {
        let mut f = BTreeMap::new();
        insert(
            &mut f,
            "entity:monster_x".into(),
            "monster",
            "maps/x.bsp".into(),
            0,
            BTreeMap::new(),
        );
        assert!(f["entity:monster_x"].implementation.is_none());
        assert!(!f["entity:monster_x"].exercised);
    }
    #[test]
    fn nested_resources_are_not_flattened() {
        assert_eq!(
            kv_leaves(b"root { row { command \"go\" } }").unwrap(),
            vec![("root/row/command".into(), "go".into())]
        );
        assert!(kv_leaves(b"root { row { command }").is_err());
    }
    #[test]
    fn failed_assertion_matching_hash_never_exercises_and_input_mutation_invalidates() {
        let root = env::temp_dir().join(format!("hl-census-evidence-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("source.rs"), b"fn behavior() {}\n").unwrap();
        fs::write(root.join("journey.toml"), b"synthetic test journey").unwrap();
        fs::write(root.join("disc.bin"), b"synthetic test disc").unwrap();
        let git = |args: &[&str]| {
            let result = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            result.stdout
        };
        git(&["init", "--quiet"]);
        git(&["add", "source.rs"]);
        git(&[
            "-c",
            "user.name=ebonura-fastly",
            "-c",
            "user.email=ebonura@fastly.com",
            "commit",
            "--quiet",
            "-m",
            "Synthetic census fixture",
        ]);
        let revision = String::from_utf8(git(&["rev-parse", "HEAD"])).unwrap();
        let mut receipt = serde_json::json!({"schema":"audited-feature-coverage-v1","source_revision":revision.trim(),"journey":{"path":root.join("journey.toml"),"sha256":hash(b"synthetic test journey")},"disc":{"path":root.join("disc.bin"),"sha256":hash(b"synthetic test disc")},"assertions":[{"name":"feature-specific assertion","status":"FAIL","observation":"synthetic fixture observation"}]});
        let report = root.join("receipt.json");
        let mut evidence = Evidence {
            classification: Classification::Implemented,
            source_sha256: hash(b"fn behavior() {}\n"),
            additional_sources: BTreeMap::new(),
            file: "source.rs".into(),
            symbol: "fn behavior".into(),
            reason: "synthetic unit fixture, never production coverage".into(),
            journey: Some(Coverage {
                report: report.display().to_string(),
                report_sha256: String::new(),
                assertion: "feature-specific assertion".into(),
                feature_observation: "synthetic fixture observation".into(),
            }),
        };
        let bind = |receipt: &serde_json::Value, evidence: &mut Evidence| {
            let bytes = serde_json::to_vec(receipt).unwrap();
            fs::write(&report, &bytes).unwrap();
            evidence.journey.as_mut().unwrap().report_sha256 = hash(&bytes);
        };
        bind(&receipt, &mut evidence);
        assert!(verify_evidence(&root, &evidence).is_err());
        receipt["assertions"][0]["status"] = "PASS".into();
        bind(&receipt, &mut evidence);
        assert_eq!(verify_evidence(&root, &evidence).unwrap(), true);
        fs::write(root.join("journey.toml"), b"changed").unwrap();
        assert!(verify_evidence(&root, &evidence).is_err());
        fs::write(root.join("journey.toml"), b"synthetic test journey").unwrap();
        fs::write(root.join("source.rs"), b"fn behavior() { changed(); }\n").unwrap();
        assert!(verify_evidence(&root, &evidence).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn non_utf8_retail_bytes_are_preserved_as_reversible_latin1() {
        let parsed = entities(b"{\"classname\" \"worldspawn\" \"message\" \"caf\xe9\"}").unwrap();
        assert_eq!(parsed[0]["message"], "caf\u{e9}");
    }
}

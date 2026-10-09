//! hl-census: compare what the shipped Half-Life maps ask of the engine with
//! what hl-psx implements, with every claim checked against the source tree.

pub mod bsp;
pub mod census;
pub mod compare;
pub mod contract;
pub mod graph;
pub mod roots;
pub mod solidity;

use census::{Census, COMMON_KEYS};
use contract::{Contract, Status};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Source text of the repository parts the contract's evidence points into.
pub struct Sources {
    cook: String,
    game: String,
    fmt: String,
    tests: String,
    runs: BTreeMap<String, String>,
}

fn read_tree(root: &Path, out: &mut String) {
    let Ok(rd) = fs::read_dir(root) else { return };
    let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            read_tree(&p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Ok(t) = fs::read_to_string(&p) {
                out.push_str(&t);
                out.push('\n');
            }
        }
    }
}

impl Sources {
    pub fn load(repo: &Path) -> Self {
        let mut cook = String::new();
        read_tree(&repo.join("host/hl-bsp/src"), &mut cook);
        read_tree(&repo.join("host/hl-build"), &mut cook);
        read_tree(&repo.join("host/hl-content/src"), &mut cook);
        let mut game = String::new();
        read_tree(&repo.join("game/src"), &mut game);
        let mut fmt = String::new();
        read_tree(&repo.join("shared/hl-format/src"), &mut fmt);
        let mut tests = String::new();
        read_tree(&repo.join("host/hl-logic-tests/src"), &mut tests);
        read_tree(&repo.join("host/hl-bsp/src"), &mut tests);
        read_tree(&repo.join("game/src"), &mut tests);
        read_tree(&repo.join("host/hl-census/src"), &mut tests);
        let mut runs = BTreeMap::new();
        if let Ok(t) = fs::read_to_string(repo.join("host/hl-census/runs.txt")) {
            for l in t.lines() {
                let l = l.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                let mut f = l.splitn(2, '|');
                if let (Some(id), Some(rest)) = (f.next(), f.next()) {
                    runs.insert(id.trim().to_string(), rest.trim().to_string());
                }
            }
        }
        Sources {
            cook,
            game,
            fmt,
            tests,
            runs,
        }
    }

    /// Resolve an evidence pointer. `Err` names what is wrong.
    pub fn resolve_evidence(&self, ev: &str) -> Result<(), String> {
        let (kind, text) = ev
            .split_once(':')
            .ok_or_else(|| format!("evidence `{ev}` has no kind"))?;
        if text.is_empty() {
            return Err(format!("evidence `{ev}` is empty"));
        }
        let hay = match kind {
            "cook" => &self.cook,
            "game" => &self.game,
            "fmt" => &self.fmt,
            _ => return Err(format!("unknown evidence kind `{kind}`")),
        };
        // Cooker keys are matched case-insensitively (`m_iszEntity`).
        if hay.contains(text)
            || hay
                .to_ascii_lowercase()
                .contains(&text.to_ascii_lowercase())
        {
            Ok(())
        } else {
            Err(format!("evidence `{ev}` not found in the {kind} sources"))
        }
    }

    pub fn resolve_tested(&self, t: &str) -> Result<(), String> {
        if t == "-" || t.is_empty() {
            return Ok(());
        }
        let (kind, name) = t
            .split_once(':')
            .ok_or_else(|| format!("tested `{t}` has no kind"))?;
        match kind {
            "unit" => {
                let pat = format!("fn {name}(");
                if self.tests.contains(&pat) {
                    Ok(())
                } else {
                    Err(format!("test fn `{name}` not found"))
                }
            }
            "run" => {
                if self.runs.contains_key(name) {
                    Ok(())
                } else {
                    Err(format!("run id `{name}` not in host/hl-census/runs.txt"))
                }
            }
            _ => Err(format!("unknown tested kind `{kind}`")),
        }
    }
}

#[derive(Default, Debug)]
pub struct Findings {
    /// Used by a map, no contract row and no scope.
    pub unclassified: Vec<(String, String, usize, u32)>,
    /// Contract rows whose pointers do not resolve, or a missing pointer.
    pub broken: Vec<String>,
    /// Rows no map uses (class-specific rows only).
    pub stale: Vec<String>,
    /// Classes (not items) with neither a scope row nor any item row.
    pub classes_untracked: Vec<(String, usize)>,
}

impl Findings {
    pub fn ok(&self, strict: bool) -> bool {
        self.unclassified.is_empty()
            && self.broken.is_empty()
            && self.classes_untracked.is_empty()
            && (!strict || self.stale.is_empty())
    }
}

#[derive(Default, Debug)]
pub struct Tally {
    pub classes_used: usize,
    pub classes_scoped: usize,
    pub classes_tracked: usize,
    pub items_used: usize,
    pub supported: usize,
    pub partial: usize,
    pub cosmetic: usize,
    pub na: usize,
    pub no: usize,
    pub unclassified: usize,
    pub unreviewed: usize,
    pub tested: usize,
}

pub fn analyse(c: &Census, ct: &Contract, src: &Sources) -> (Findings, Tally) {
    let mut f = Findings::default();
    let mut t = Tally {
        classes_used: c.classes.len(),
        ..Tally::default()
    };
    for (class, u) in &c.classes {
        if ct.tracks(class) {
            t.classes_tracked += 1;
        } else if ct.scope_of(class).is_some() {
            t.classes_scoped += 1;
        } else {
            f.classes_untracked.push((class.clone(), u.maps.len()));
        }
    }
    for ((class, item), u) in &c.items {
        if COMMON_KEYS.contains(&item.as_str()) {
            continue;
        }
        let row = ct.get(class, item);
        if row.is_none() && ct.scope_of(class).is_some() {
            continue;
        }
        t.items_used += 1;
        match row {
            None => {
                t.unclassified += 1;
                f.unclassified
                    .push((class.clone(), item.clone(), u.maps.len(), u.count));
            }
            Some(row) => {
                match row.status {
                    Status::Yes => t.supported += 1,
                    Status::Partial => {
                        t.supported += 1;
                        t.partial += 1
                    }
                    Status::Cosmetic => t.cosmetic += 1,
                    Status::Na => t.na += 1,
                    Status::No => t.no += 1,
                    Status::Scope => {}
                    Status::Unreviewed => t.unreviewed += 1,
                }
                if row.tested != "-" && !row.tested.is_empty() {
                    t.tested += 1;
                }
            }
        }
    }
    for row in &ct.rows {
        let tag = format!("line {} {}|{}", row.line, row.class, row.item);
        if row.status.needs_evidence() || (row.status == Status::Unreviewed && row.evidence != "-")
        {
            if let Err(e) = src.resolve_evidence(&row.evidence) {
                f.broken.push(format!("{tag}: {e}"));
            }
        }
        if let Err(e) = src.resolve_tested(&row.tested) {
            f.broken.push(format!("{tag}: {e}"));
        }
        if row.class != "*" && row.status != Status::Scope {
            let used = c.items.contains_key(&(row.class.clone(), row.item.clone()));
            if !used {
                f.stale.push(tag);
            }
        }
    }
    f.unclassified
        .sort_by(|a, b| b.2.cmp(&a.2).then(b.3.cmp(&a.3)));
    f.classes_untracked.sort_by(|a, b| b.1.cmp(&a.1));
    (f, t)
}

pub fn summary_line(t: &Tally) -> String {
    format!(
        "classes {} (scoped away {}, tracked {}); class+key/flag pairs {}: supported {} (partial {}), cosmetic {}, retail-ignored {}, gap {}, unreviewed {}, unclassified {}; behaviour-tested {}",
        t.classes_used,
        t.classes_scoped,
        t.classes_tracked,
        t.items_used,
        t.supported,
        t.partial,
        t.cosmetic,
        t.na,
        t.no,
        t.unreviewed,
        t.unclassified,
        t.tested
    )
}

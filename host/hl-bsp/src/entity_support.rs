use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

const CATALOG: &str = include_str!("../../hl-content/entity-support.txt");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Support {
    Runtime,
    Approximated,
    CookOnly,
    Intentional,
    MissingGameplay,
    MissingVisual,
    Unknown,
}

impl Support {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Approximated => "approximated",
            Self::CookOnly => "cook_only",
            Self::Intentional => "intentional",
            Self::MissingGameplay => "missing_gameplay",
            Self::MissingVisual => "missing_visual",
            Self::Unknown => "unknown",
        }
    }
}

fn parse_status(raw: &str) -> Support {
    match raw {
        "runtime" => Support::Runtime,
        "approximated" => Support::Approximated,
        "cook_only" => Support::CookOnly,
        "intentional" => Support::Intentional,
        "missing_gameplay" => Support::MissingGameplay,
        "missing_visual" => Support::MissingVisual,
        _ => Support::Unknown,
    }
}

pub fn classify(classname: &str) -> Support {
    CATALOG
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.splitn(3, '|');
            Some((fields.next()?, fields.next()?))
        })
        .find(|(known, _)| known.eq_ignore_ascii_case(classname))
        .map(|(_, status)| parse_status(status))
        .unwrap_or(Support::Unknown)
}

pub fn audit(
    entity_text: &str,
    map_path: &str,
    report_path: Option<&str>,
    strict: bool,
) -> Result<(), String> {
    let mut counts = BTreeMap::<String, usize>::new();
    for block in entity_text.split('{') {
        let Some(classname) = super::ent_value(block, "classname") else {
            continue;
        };
        if !classname.is_empty() {
            *counts.entry(classname.to_string()).or_default() += 1;
        }
    }

    let map = std::env::var("MAP_NAME").unwrap_or_else(|_| {
        Path::new(map_path)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string()
    });
    let unknown = counts
        .keys()
        .filter(|classname| classify(classname) == Support::Unknown)
        .cloned()
        .collect::<Vec<_>>();
    if strict && !unknown.is_empty() {
        return Err(format!(
            "{map}: entity coverage catalog has unknown classnames: {}",
            unknown.join(", ")
        ));
    }

    if let Some(path) = report_path {
        let mut report = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| format!("open entity coverage report {path}: {error}"))?;
        for (classname, count) in &counts {
            writeln!(
                report,
                "{map},{classname},{count},{}",
                classify(classname).name()
            )
            .map_err(|error| format!("write entity coverage report {path}: {error}"))?;
        }
    }

    let mut by_status = BTreeMap::<Support, usize>::new();
    for (classname, count) in counts {
        *by_status.entry(classify(&classname)).or_default() += count;
    }
    eprintln!(
        "  entity coverage: {} runtime, {} approximated, {} cook-only, {} missing gameplay, {} missing visual, {} unknown",
        by_status.get(&Support::Runtime).copied().unwrap_or(0),
        by_status.get(&Support::Approximated).copied().unwrap_or(0),
        by_status.get(&Support::CookOnly).copied().unwrap_or(0),
        by_status
            .get(&Support::MissingGameplay)
            .copied()
            .unwrap_or(0),
        by_status
            .get(&Support::MissingVisual)
            .copied()
            .unwrap_or(0),
        by_status.get(&Support::Unknown).copied().unwrap_or(0),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn catalog_has_unique_well_formed_rows() {
        let mut names = BTreeSet::new();
        for line in CATALOG
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        {
            let fields = line.split('|').collect::<Vec<_>>();
            assert_eq!(fields.len(), 3, "bad entity-support row: {line}");
            assert!(
                names.insert(fields[0].to_ascii_lowercase()),
                "duplicate: {line}"
            );
            assert_ne!(
                parse_status(fields[1]),
                Support::Unknown,
                "bad status: {line}"
            );
        }
    }

    #[test]
    fn unknown_class_is_never_silently_promoted() {
        assert_eq!(classify("new_unreviewed_entity"), Support::Unknown);
    }
}

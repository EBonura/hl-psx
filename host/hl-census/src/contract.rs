//! The entity contract: one reviewed row per (class, keyvalue or spawnflag)
//! that says whether hl-psx honours it, where, and what tests it.
//!
//! Format, one row per line, `#` comments:
//!
//! ```text
//! class|item|status|evidence|tested|note
//! ```
//!
//! * `class` is a classname, `*` for keys shared by every class, or a
//!   `scope` row (`item` = `*`, `status` = `scope`, `evidence` = owner) which
//!   hands a whole class family to someone else's contract.
//! * `item` is a lowercase key, or `flag:<bit value>` for a spawnflag bit,
//!   or `@use` / `@kill` for being fired or killed by name. Several items
//!   separated by commas share the row.
//! * `status`: `yes` honoured, `partial` honoured with a stated deviation,
//!   `cosmetic` ignored and affects only look or sound, `na` retail ignores
//!   it too, `no` a gameplay gap.
//! * `unreviewed` is the seeded state: classified from whether the cooker
//!   names the key at all, not yet checked by a person. `check --reviewed`
//!   fails while any remain.
//! * `evidence`: `cook:<text>`, `game:<text>` or `fmt:<text>`. The text must
//!   occur in that part of the source tree. Required for `yes` and `partial`.
//! * `tested`: `unit:<fn name>` (a function that exists), `run:<id>` (an id
//!   in `host/hl-census/runs.txt`) or `-`.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Status {
    Yes,
    Partial,
    Cosmetic,
    Na,
    No,
    Scope,
    Unreviewed,
}

impl Status {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "yes" => Self::Yes,
            "partial" => Self::Partial,
            "cosmetic" => Self::Cosmetic,
            "na" => Self::Na,
            "no" => Self::No,
            "scope" => Self::Scope,
            "unreviewed" => Self::Unreviewed,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::Partial => "partial",
            Self::Cosmetic => "cosmetic",
            Self::Na => "na",
            Self::No => "no",
            Self::Scope => "scope",
            Self::Unreviewed => "unreviewed",
        }
    }

    /// Counts as supported in the census.
    pub fn supported(self) -> bool {
        matches!(self, Self::Yes | Self::Partial)
    }

    pub fn needs_evidence(self) -> bool {
        self.supported()
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub class: String,
    pub item: String,
    pub status: Status,
    pub evidence: String,
    pub tested: String,
    pub note: String,
    pub line: usize,
}

#[derive(Default)]
pub struct Contract {
    pub rows: Vec<Row>,
    index: BTreeMap<(String, String), usize>,
}

impl Contract {
    pub fn parse(text: &str) -> Result<Self, Vec<String>> {
        let mut c = Contract::default();
        let mut errs = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.splitn(6, '|').collect();
            if f.len() < 5 {
                errs.push(format!(
                    "contract line {}: expected class|item|status|evidence|tested[|note]",
                    n + 1
                ));
                continue;
            }
            let Some(status) = Status::parse(f[2].trim()) else {
                errs.push(format!(
                    "contract line {}: unknown status `{}`",
                    n + 1,
                    f[2]
                ));
                continue;
            };
            // `item` may list several comma-separated items sharing one verdict.
            for item in f[1].split(',') {
                let row = Row {
                    class: f[0].trim().to_ascii_lowercase(),
                    item: item.trim().to_ascii_lowercase(),
                    status,
                    evidence: f[3].trim().to_string(),
                    tested: f[4].trim().to_string(),
                    note: f.get(5).map(|s| s.trim().to_string()).unwrap_or_default(),
                    line: n + 1,
                };
                let key = (row.class.clone(), row.item.clone());
                if let Some(prev) = c.index.get(&key) {
                    errs.push(format!(
                        "contract line {}: duplicate of line {} ({}|{})",
                        n + 1,
                        c.rows[*prev].line,
                        key.0,
                        key.1
                    ));
                    continue;
                }
                c.index.insert(key, c.rows.len());
                c.rows.push(row);
            }
        }
        if errs.is_empty() {
            Ok(c)
        } else {
            Err(errs)
        }
    }

    /// Exact class row, then a prefix row (`monster_*`), then a shared `*` row.
    pub fn get(&self, class: &str, item: &str) -> Option<&Row> {
        if let Some(&i) = self.index.get(&(class.to_string(), item.to_string())) {
            return Some(&self.rows[i]);
        }
        let prefix = self.rows.iter().find(|r| {
            r.item == item
                && r.status != Status::Scope
                && r.class.len() > 1
                && r.class.ends_with('*')
                && class.starts_with(&r.class[..r.class.len() - 1])
        });
        if prefix.is_some() {
            return prefix;
        }
        self.index
            .get(&("*".to_string(), item.to_string()))
            .map(|&i| &self.rows[i])
    }

    /// The scope row that hands the rest of a class to another owner, if any.
    /// Scope rows are the fallback: a key with its own row is still tracked.
    pub fn scope_of(&self, class: &str) -> Option<&Row> {
        if let Some(&i) = self.index.get(&(class.to_string(), "*".to_string())) {
            if self.rows[i].status == Status::Scope {
                return Some(&self.rows[i]);
            }
        }
        self.rows.iter().find(|r| {
            r.status == Status::Scope
                && r.class.len() > 1
                && r.class.ends_with('*')
                && class.starts_with(&r.class[..r.class.len() - 1])
        })
    }

    /// Does any row, exact or prefix, speak about this class?
    pub fn tracks(&self, class: &str) -> bool {
        self.rows.iter().any(|r| {
            r.status != Status::Scope
                && (r.class == class
                    || (r.class.len() > 1
                        && r.class.ends_with('*')
                        && class.starts_with(&r.class[..r.class.len() - 1])))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rows_and_scopes() {
        let c = Contract::parse(
            "# c\nfunc_door|speed|yes|cook:\"speed\"|-|x\nmonster_*|*|scope|ai|-\n*|origin|yes|cook:origin|-\n",
        )
        .unwrap();
        assert_eq!(c.get("func_door", "speed").unwrap().status, Status::Yes);
        assert_eq!(c.get("func_button", "origin").unwrap().status, Status::Yes);
        assert!(c.scope_of("monster_zombie").is_some());
        assert!(c.scope_of("func_door").is_none());
    }

    #[test]
    fn rejects_bad_status_and_duplicates() {
        assert!(Contract::parse("a|b|maybe|-|-").is_err());
        assert!(Contract::parse("a|b|yes|x|-\na|b|no|-|-").is_err());
    }
}

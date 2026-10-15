//! RAM accounting for the PlayStation executable, read from the lld link map.
//!
//! `cargo hl-build ram` links the game (or reads the map of the last link) and
//! prints one number first: the free RAM, by the SDK's own check
//! (`psoxide-pgo ram`: bytes between `__bss_end` and the stack reserve). The
//! rest of the report says where the other bytes went: per output section
//! (`.text`, `.data` with the read-only data, `.bss`), per crate, per module of
//! this crate, and the largest symbols of each section. The budget gate
//! (`game/ram-budget`) fails a build whose free RAM falls under its floor.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// End of psoxide.ld's RAM region: 2 MiB from 0x8000_0000 less the 64 KiB the
/// BIOS keeps and the 32 KiB stack reserve. Mirrors `psoxide-pgo`'s
/// `ram::RAM_END`; `free_matches_sdk_check` below pins the two together.
pub const RAM_END: u32 = 0x801F_8000;

/// How many of the largest symbols each section lists.
const TOP: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Text,
    Data,
    Bss,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Text => ".text (code)",
            Kind::Data => ".data (data + rodata)",
            Kind::Bss => ".bss (zeroed statics)",
        }
    }
}

/// One input section placed in the image.
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: Kind,
    pub size: u32,
    /// Crate the object file belongs to (`hl_psx`, `psx_asset`, ...).
    pub krate: String,
    /// Demangled symbol when the map names one, else the input section name.
    pub name: String,
}

#[derive(Debug, Default)]
pub struct Map {
    pub items: Vec<Item>,
    /// Size of each output section, padding included.
    pub text_total: u32,
    pub data_total: u32,
    pub bss_total: u32,
    pub bss_end: Option<u32>,
}

impl Map {
    pub fn free(&self) -> Option<u32> {
        self.bss_end.map(|end| RAM_END.saturating_sub(end))
    }
}

/// The text after the first `n` whitespace-separated columns, spaces intact.
fn after_columns(line: &str, n: usize) -> &str {
    let mut rest = line.trim_start();
    for _ in 0..n {
        rest = rest
            .split_once(char::is_whitespace)
            .map_or("", |(_, tail)| tail.trim_start());
    }
    rest.trim_end()
}

fn hex(field: &str) -> Option<u32> {
    u32::from_str_radix(field, 16).ok()
}

/// The crate an object file belongs to: `deps/hl_psx-<hash>...o` is the game,
/// `deps/libpsx_gpu-<hash>.rlib(member)` is `psx_gpu`.
fn crate_of(object: &str) -> String {
    let path = object.split(":(").next().unwrap_or(object);
    let file = path.rsplit('/').next().unwrap_or(path);
    let file = file.split('(').next().unwrap_or(file);
    let file = file.strip_prefix("lib").unwrap_or(file);
    file.split(['-', '.']).next().unwrap_or(file).to_string()
}

pub fn parse(map: &str) -> Map {
    let mut parsed = Map::default();
    let mut section: Option<Kind> = None;
    // The item an upcoming symbol line may name.
    let mut pending: Option<usize> = None;
    for line in map.lines() {
        let mut fields = line.split_whitespace();
        let (Some(vma), Some(_lma), Some(size), Some(align)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Some(vma), Some(size)) = (hex(vma), hex(size)) else {
            continue;
        };
        let rest: Vec<&str> = fields.collect();
        if rest.is_empty() {
            continue;
        }
        let tail = after_columns(line, 4);
        // Output section header: `800c32f8 800c32f8   133d60     8 .bss`.
        if rest.len() == 1 && !align.is_empty() {
            match rest[0] {
                ".text" => {
                    section = Some(Kind::Text);
                    parsed.text_total = size;
                    pending = None;
                    continue;
                }
                ".data" => {
                    section = Some(Kind::Data);
                    parsed.data_total = size;
                    pending = None;
                    continue;
                }
                ".bss" => {
                    section = Some(Kind::Bss);
                    parsed.bss_total = size;
                    pending = None;
                    continue;
                }
                ".ps_exe_head" => {
                    section = None;
                    pending = None;
                    continue;
                }
                // Debug sections have VMA 0 and are not part of the image.
                name if vma == 0 && name.starts_with('.') => {
                    section = None;
                    pending = None;
                    continue;
                }
                _ => {}
            }
        }
        if line.contains("__bss_end = .") {
            parsed.bss_end = Some(vma);
            continue;
        }
        let Some(kind) = section else { continue };
        if tail.contains(":(") {
            // `<object>:(<input section>)`: a new input section.
            pending = None;
            if size == 0 {
                continue;
            }
            let name = tail
                .rsplit_once(":(")
                .map(|(_, section)| section.trim_end_matches(')').to_string())
                .unwrap_or_default();
            parsed.items.push(Item {
                kind,
                size,
                krate: crate_of(tail),
                name,
            });
            pending = Some(parsed.items.len() - 1);
        } else if let Some(index) = pending {
            // A symbol under the input section above: the first one with the
            // section's size names it.
            if tail.starts_with('.') || tail.contains(" = ") || tail.starts_with("PROVIDE") {
                continue;
            }
            let item = &mut parsed.items[index];
            if size == item.size && item.name.starts_with('.') {
                item.name = tail.to_string();
                pending = None;
            }
        }
    }
    parsed
}

/// The module a symbol sits in: `hl_psx::species::fn` is `hl_psx::species`, a
/// root item of the game is `hl_psx (root)`.
fn module_of(item: &Item) -> String {
    let name = item.name.trim_start_matches('<');
    let name = name.strip_prefix("& ").unwrap_or(name);
    let mut parts = name.split("::");
    let first = parts.next().unwrap_or("");
    if item.krate == "hl_psx" && first == "hl_psx" {
        match (parts.next(), parts.next()) {
            (Some(second), Some(_)) => format!("hl_psx::{second}"),
            _ => "hl_psx (root)".to_string(),
        }
    } else if item.name.starts_with('.') {
        format!("{} (unnamed)", item.krate)
    } else if first == item.krate || first.is_empty() {
        item.krate.clone()
    } else {
        // Generic code instantiated from another crate into this object.
        format!("{} (instantiated in {})", first, item.krate)
    }
}

fn kb(bytes: u32) -> String {
    format!("{}.{} KB", bytes / 1024, (bytes % 1024) * 10 / 1024)
}

fn table(out: &mut String, title: &str, rows: BTreeMap<String, u32>, limit: usize) {
    let mut rows: Vec<(String, u32)> = rows.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let _ = writeln!(out, "  {title}");
    for (name, bytes) in rows.iter().take(limit) {
        let _ = writeln!(out, "    {bytes:>8}  {name}");
    }
    if rows.len() > limit {
        let rest: u32 = rows.iter().skip(limit).map(|row| row.1).sum();
        let _ = writeln!(out, "    {rest:>8}  ({} more)", rows.len() - limit);
    }
}

/// The full report. The first line is the number the budget gate reads.
pub fn report(parsed: &Map) -> String {
    let mut out = String::new();
    let free = parsed.free().unwrap_or(0);
    let _ = writeln!(out, "free {free} B");
    let image = parsed.text_total + parsed.data_total + parsed.bss_total;
    let _ = writeln!(
        out,
        "image {} B = .text {} + .data {} + .bss {}; RAM region {} B for the image, {} B free ({})",
        image,
        parsed.text_total,
        parsed.data_total,
        parsed.bss_total,
        image + free,
        free,
        kb(free)
    );
    for kind in [Kind::Text, Kind::Data, Kind::Bss] {
        let items: Vec<&Item> = parsed.items.iter().filter(|i| i.kind == kind).collect();
        let total: u32 = items.iter().map(|i| i.size).sum();
        let _ = writeln!(
            out,
            "\n{} {} B ({}), {} items",
            kind.label(),
            total,
            kb(total),
            items.len()
        );
        let mut by_crate = BTreeMap::new();
        let mut by_module = BTreeMap::new();
        let mut by_symbol = BTreeMap::new();
        for item in &items {
            *by_crate.entry(item.krate.clone()).or_insert(0) += item.size;
            *by_module.entry(module_of(item)).or_insert(0) += item.size;
            *by_symbol
                .entry(format!("{} [{}]", item.name, item.krate))
                .or_insert(0) += item.size;
        }
        table(&mut out, "by crate", by_crate, 12);
        table(&mut out, "by module", by_module, 24);
        table(&mut out, "largest symbols", by_symbol, TOP);
    }
    out
}

/// Every placed input section, one per line: `kind size crate name`, largest
/// first. For diffing two links or finding a symbol the report folded away.
pub fn items(parsed: &Map) -> String {
    let mut rows: Vec<&Item> = parsed.items.iter().collect();
    rows.sort_by(|a, b| b.size.cmp(&a.size).then(a.name.cmp(&b.name)));
    let mut out = String::new();
    for item in rows {
        let kind = match item.kind {
            Kind::Text => "text",
            Kind::Data => "data",
            Kind::Bss => "bss",
        };
        let _ = writeln!(out, "{kind}\t{}\t{}\t{}", item.size, item.krate, item.name);
    }
    out
}

/// A budget file: the first number on the first non-comment line is the floor.
pub fn parse_floor(text: &str) -> Option<u32> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "\
     VMA      LMA     Size Align Out     In      Symbol
       0        0        0     1 STACK_RESERVE = 0x8000
80010000 80010000      100     4 .text
80010000 80010000       78     4         /r/game/target/deps/hl_psx-1.rcgu.o:(.text._start)
80010000 80010000       78     1                 _start
80010078 80010078       88     4         /r/game/target/deps/libpsx_asset-2.rlib(psx_asset-2.cgu.0.rcgu.o):(.text._RNvNtCs_9psx_asset4hmd85Model4pose)
80010078 80010078       88     1                 psx_asset::hmd8::Model::pose
80010100 80010100       20     4 .data
80010100 80010100       10     4         /r/game/target/deps/hl_psx-1.rcgu.o:(.rodata..L__unnamed_3)
80010110 80010110       10     4         /r/game/target/deps/hl_psx-1.rcgu.o:(.data._RNvCs_6hl_psx8MAP_BUF)
80010110 80010110       10     1                 hl_psx::MAP_BUF
80010200 80010200      400     8 .bss
80010200 80010200      300     8         /r/game/target/deps/hl_psx-1.rcgu.o:(.bss._RNvNtCs_6hl_psx7species4POOL)
80010200 80010200      300     1                 hl_psx::species::POOL
80010500 80010500      100     8         /r/game/target/deps/hl_psx-1.rcgu.o:(.bss._RNvCs_6hl_psx3BIG)
80010500 80010500      100     1                 hl_psx::BIG (.0)
80010600 80010600        0     1         __bss_end = .
";

    #[test]
    fn attributes_sections_crates_and_modules() {
        let parsed = parse(MAP);
        assert_eq!(parsed.text_total, 0x100);
        assert_eq!(parsed.bss_end, Some(0x8001_0600));
        let text: u32 = parsed
            .items
            .iter()
            .filter(|i| i.kind == Kind::Text)
            .map(|i| i.size)
            .sum();
        assert_eq!(text, 0x78 + 0x88);
        let pose = parsed
            .items
            .iter()
            .find(|i| i.name.contains("pose"))
            .unwrap();
        assert_eq!(pose.krate, "psx_asset");
        let pool = parsed
            .items
            .iter()
            .find(|i| i.name.contains("POOL"))
            .unwrap();
        assert_eq!(module_of(pool), "hl_psx::species");
        let big = parsed
            .items
            .iter()
            .find(|i| i.name.contains("BIG"))
            .unwrap();
        assert_eq!(module_of(big), "hl_psx (root)");
        let unnamed = parsed
            .items
            .iter()
            .find(|i| i.name.contains("unnamed"))
            .unwrap();
        assert_eq!(module_of(unnamed), "hl_psx (unnamed)");
    }

    #[test]
    fn free_matches_sdk_check() {
        // psoxide-pgo ram: RAM_END - __bss_end, same region constant.
        assert_eq!(parse(MAP).free(), Some(0x801F_8000 - 0x8001_0600));
    }

    #[test]
    fn floor_file_skips_comments() {
        assert_eq!(parse_floor("# floor\n\n12288  # bytes\n"), Some(12288));
        assert_eq!(parse_floor("# only a comment\n"), None);
    }
}

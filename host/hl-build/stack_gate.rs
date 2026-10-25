//! Stack watermark gate for the shipped disc.
//!
//! The linker script reserves `STACK_RESERVE` bytes below the initial stack
//! pointer for the call stack (`game/stack-reserve`). This module replays the
//! shipped image in the emulator, enters maps through the title menu with
//! controller pulses (no recorded route), lets the player walk, and reads the
//! emulator's stack high-water profile (`--stack-profile-log`, which watches
//! the stack pointer without touching guest RAM). A scenario fails when its
//! watermark plus the safety margin no longer fits the reserve.
//!
//! The profile is rooted at `play()`, so it does not see the title menu, the
//! intro or the ending (their 18 KB, set by the 16 KB frame of the SDK's
//! `FontAtlas::upload`, is covered by the static bound below, which starts at
//! `main`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::Result;

/// Top of main RAM; the emulator reports stack depths from the entry SP, which
/// sits a few bytes below it.
pub const RAM_TOP: u32 = 0x8020_0000;
/// Safety margin kept over the deepest stack seen, in percent.
pub const MARGIN_PERCENT: u32 = 25;

const DOWN: u16 = 0x0040;
const CROSS: u16 = 0x4000;
const UP: u16 = 0x0010;
/// Route tick at which the title menu is up and takes input.
const MENU_READY_TICK: u32 = 900;
/// Ticks between menu presses (each held 4 ticks).
const PRESS_SPACING: u32 = 16;
/// Port-1 polls to run a scenario for: the load, then a walk.
pub const STOP_POLLS: u32 = 2600;

/// The reserve the deepest watermark needs: watermark plus the margin, rounded
/// up to 1 KiB.
pub fn reserve_for(watermark: u32) -> u32 {
    (watermark + watermark * MARGIN_PERCENT / 100).next_multiple_of(1024)
}

/// Bytes between the top of RAM and `main`'s entry stack pointer (`_start`'s
/// frame and the BIOS's hand-off).
pub const MAIN_ENTRY_OFFSET: u32 = 48;

/// Bytes the stack used from the top of RAM: the frames above `play` plus the
/// depth below its entry.
pub fn watermark(entry_sp: u32, depth: u32) -> u32 {
    RAM_TOP - entry_sp + depth
}

/// `(entry_sp, depth_bytes)` of the root call in a `--stack-profile-log` CSV.
pub fn parse_profile(text: &str) -> Option<(u32, u32)> {
    let row = text.lines().nth(1)?;
    let fields: Vec<&str> = row.split(',').collect();
    let entry = u32::from_str_radix(fields.get(3)?.trim_start_matches("0x"), 16).ok()?;
    let depth = fields.get(5)?.parse().ok()?;
    Some((entry, depth))
}

/// Numbers of the first `const NAME: [..] = [ ... ];` array in `source`.
pub(crate) fn array_of(source: &str, name: &str) -> Option<Vec<usize>> {
    let start = source.find(&format!("const {name}:"))?;
    let rest = &source[start..];
    let open = rest.find("= [")? + 3;
    let close = rest[open..].find("];")? + open;
    let body: String = rest[open..close]
        .lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ");
    body.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.parse().ok())
        .collect()
}

/// The game's own map tables, read from its source so the gate cannot drift.
pub struct Catalog {
    chapter_start: Vec<usize>,
    campaign_maps: usize,
    pub maps: usize,
}

impl Catalog {
    pub fn parse(source: &str) -> Option<Catalog> {
        let chapter_start = array_of(source, "CHAPTER_START")?;
        let order = array_of(source, "PLAY_ORDER")?;
        let campaign = source.find("pub const CAMPAIGN_MAP_COUNT: usize = ")?;
        let campaign_maps = source[campaign..]
            .split('=')
            .nth(1)?
            .split(';')
            .next()?
            .trim()
            .parse()
            .ok()?;
        Some(Catalog {
            chapter_start,
            campaign_maps,
            maps: order.len(),
        })
    }

    /// Controller pulses that start `play_order` (an index into PLAY_ORDER)
    /// from the title menu: Chapters, the chapter row, the map row.
    pub fn pulses(&self, play_order: usize) -> String {
        let mut presses: Vec<u16> = Vec::new();
        if play_order >= self.campaign_maps {
            // Hazard Course is main-menu row 1; its maps follow the campaign.
            presses.extend([DOWN, CROSS]);
            presses.extend(std::iter::repeat(DOWN).take(play_order - self.campaign_maps));
            presses.push(CROSS);
        } else {
            let chapter = self
                .chapter_start
                .iter()
                .rposition(|start| *start <= play_order)
                .unwrap_or(0);
            let row = play_order - self.chapter_start[chapter];
            presses.extend(std::iter::repeat(DOWN).take(4)); // Chapters
            presses.push(CROSS);
            presses.extend(std::iter::repeat(DOWN).take(chapter));
            presses.push(CROSS);
            presses.extend(std::iter::repeat(DOWN).take(row));
            presses.push(CROSS);
        }
        let mut tick = MENU_READY_TICK;
        let mut out = Vec::new();
        for press in presses {
            out.push(format!("0x{press:04x}@{tick}+4"));
            tick += PRESS_SPACING;
        }
        // After the load: walk forward, jump, step back, a few dozen times, so
        // movement, collision and the monsters near the spawn all run.
        let mut tick = tick + 1500;
        for _ in 0..40 {
            out.push(format!("0x{UP:04x}@{tick}+60"));
            out.push(format!("0x{CROSS:04x}@{}+6", tick + 70));
            out.push(format!("0x{DOWN:04x}@{}+30", tick + 100));
            tick += 160;
        }
        out.join(",")
    }
}

/// One scenario's outcome.
#[derive(Debug)]
pub struct Reading {
    pub play_order: usize,
    pub watermark: u32,
}

/// Run `play_order` on `cue` and read its stack watermark under `play_pc`.
pub fn measure(
    frontend: &Path,
    cue: &Path,
    catalog: &Catalog,
    play_pc: u32,
    play_order: usize,
    out_dir: &Path,
) -> Result<Reading> {
    fs::create_dir_all(out_dir)?;
    let log = out_dir.join(format!("p{play_order}.csv"));
    let _ = fs::remove_file(&log);
    let status = Command::new(frontend)
        .arg("launch")
        .arg("--path")
        .arg(cue)
        .args([
            "--embedded-playtest",
            "--steps",
            "4000000000",
            "--no-memcard",
        ])
        .arg("--pad-pulses")
        .arg(catalog.pulses(play_order))
        .arg("--stack-profile-log")
        .arg(&log)
        .arg("--stack-profile-root-pc")
        .arg(format!("{play_pc:#x}"))
        .args(["--stop-at-poll", &STOP_POLLS.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(format!("frontend failed for play-order {play_order}: {status}").into());
    }
    let text = fs::read_to_string(&log)?;
    let (entry, depth) = parse_profile(&text)
        .ok_or_else(|| format!("play-order {play_order} never entered play()"))?;
    Ok(Reading {
        play_order,
        watermark: watermark(entry, depth),
    })
}

/// Address of `hl_psx::play` in a link map.
pub fn play_pc(map: &str) -> Option<u32> {
    map.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let vma = fields.next()?;
        let name = line.rsplit_once(' ')?.1;
        (name == "hl_psx::play").then(|| u32::from_str_radix(vma, 16).ok())?
    })
}

/// Maps the gate always runs: the tram ride, the first lab, the heaviest
/// rosters, the boss maps, the end.
pub const DEFAULT_MAPS: [usize; 8] = [0, 6, 20, 37, 70, 88, 94, 95];

/// Check every scenario against `reserve`; prints a table and fails when a
/// watermark plus the margin exceeds it.
pub fn check(readings: &[Reading], static_bound: Option<u32>, reserve: u32) -> Result<u32> {
    let mut worst = 0;
    println!("stack watermarks (bytes from the top of RAM; margin {MARGIN_PERCENT}%, reserve {reserve} B):");
    let mut failures = Vec::new();
    for reading in readings {
        let needed = reserve_for(reading.watermark);
        println!(
            "  play-order {:>3}: {:>6} B, needs a reserve of {} B{}",
            reading.play_order,
            reading.watermark,
            needed,
            if needed > reserve { "  OVER" } else { "" }
        );
        worst = worst.max(reading.watermark);
        if needed > reserve {
            failures.push(reading.play_order);
        }
    }
    if let Some(bound) = static_bound {
        let needed = reserve_for(bound);
        println!(
            "  static bound (every call chain, recursion at the campaign's deepest trees): {bound} B, needs a reserve of {needed} B{}",
            if needed > reserve { "  OVER" } else { "" }
        );
        worst = worst.max(bound);
        if needed > reserve {
            failures.push(usize::MAX);
        }
    }
    if failures.is_empty() {
        println!(
            "stack gate: worst {worst} B, reserve {reserve} B holds it with a {}% margin ({} B spare)",
            MARGIN_PERCENT,
            reserve - worst
        );
        Ok(worst)
    } else {
        Err(format!(
            "stack gate: {} come within {MARGIN_PERCENT}% of the {reserve} B reserve \
             in game/stack-reserve (worst {worst} B needs {} B)",
            failures
                .iter()
                .map(|f| if *f == usize::MAX {
                    "the static bound".to_string()
                } else {
                    format!("play-order {f}")
                })
                .collect::<Vec<_>>()
                .join(", "),
            reserve_for(worst)
        )
        .into())
    }
}

/// The reserve the repository commits (`game/stack-reserve`, first number).
pub fn committed_reserve(repository: &Path) -> Result<u32> {
    let path: PathBuf = repository.join("game/stack-reserve");
    let text =
        fs::read_to_string(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    super::ram_report::parse_floor(&text)
        .ok_or_else(|| format!("{} holds no number", path.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = include_str!("../../game/src/map_catalog.rs");

    #[test]
    fn reserve_is_watermark_plus_a_quarter_in_whole_kib() {
        assert_eq!(reserve_for(8_000), 10_240);
        assert_eq!(reserve_for(9_831), 12_288); // 9,831 + 2,457 = 12,288 exactly
        assert_eq!(reserve_for(9_832), 13_312); // one byte over: the next KiB
        assert_eq!(reserve_for(1_024), 2_048);
    }

    #[test]
    fn reads_the_game_catalog() {
        let catalog = Catalog::parse(CATALOG).expect("catalog parses");
        assert_eq!(catalog.maps, 103);
        assert_eq!(catalog.chapter_start.len(), 19);
        assert_eq!(catalog.campaign_maps, 96);
    }

    #[test]
    fn menu_pulses_reach_the_requested_row() {
        let catalog = Catalog::parse(CATALOG).unwrap();
        // c0a0: Chapters, chapter row 0, map row 0.
        let first = catalog.pulses(0);
        assert!(first.starts_with("0x0040@900+4,0x0040@916+4,0x0040@932+4,0x0040@948+4,0x4000@964+4,0x4000@980+4,0x4000@996+4,"), "{first}");
        // play-order 6 is chapter 1, row 0: one extra DOWN.
        assert!(catalog
            .pulses(6)
            .contains("0x4000@964+4,0x0040@980+4,0x4000@996+4,0x4000@1012+4"));
        // Hazard Course starts from main-menu row 1.
        assert!(catalog
            .pulses(96)
            .starts_with("0x0040@900+4,0x4000@916+4,0x4000@932+4,"));
    }

    #[test]
    fn parses_the_profile_row() {
        let text = "call,root_pc,return_pc,entry_sp,min_sp,depth_bytes,observations,completed\n0,0x80011064,0x800107fc,0x801ffe78,0x801fdd60,8480,1,1\n";
        assert_eq!(parse_profile(text), Some((0x801f_fe78, 8480)));
        assert_eq!(watermark(0x801f_fe78, 8480), 0x20_0000 - 0x1f_fe78 + 8480);
    }

    #[test]
    fn finds_play_in_a_map() {
        let map = "800107d8 800107d8       1c     1                 hl_psx::other\n80011064 80011064     dbc0     1                 hl_psx::play\n";
        assert_eq!(play_pc(map), Some(0x8001_1064));
    }

    #[test]
    fn the_gate_fails_inside_the_margin() {
        let near = [Reading {
            play_order: 6,
            watermark: 10_000,
        }];
        assert!(check(&near, None, 12_288).is_err());
        assert!(check(&near, None, 13_312).is_ok());
        // The static bound is held to the same margin.
        assert!(check(&[], Some(20_000), 24_576).is_err());
        assert!(check(&[], Some(20_000), 25_600).is_ok());
    }
}

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
//!
//! The presses start when the title menu is up: a first run of the disc reads `MENU_STATE`
//! (screen 1, the main menu) from the route log, as the boot to the title takes as long as the
//! intro and the scene core make it, not a fixed tick. The profiler counts every main-RAM `$sp`
//! while `play()` runs, interrupt handlers included, and psx-rt's dispatcher runs its handlers
//! on its own 512 B stack in `.bss` (and passes through `lui $sp` with only the high half set on
//! the way): a map with a dispatched handler (c1a0) read that stack, tens of KB below the main
//! one, as its depth. A reading whose lowest `$sp` lies below the stack reserve is therefore
//! taken from main RAM instead: the disc is run twice more to the same polls with the RAM
//! dumped, once at the title menu and once at the scenario's end, and the lowest word of the
//! stack reserve that differs is the deepest the main stack was written in between (the map
//! load and the walk). The dispatcher's stack lies outside the reserve, so it cannot show there.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::Result;

/// Safety margin kept over the deepest stack seen, in percent.
pub const MARGIN_PERCENT: u32 = 25;

const DOWN: u16 = 0x0040;
const CROSS: u16 = 0x4000;
const UP: u16 = 0x0010;
/// Ticks after the title menu's first frame before the first press: a press must begin after
/// the menu has polled the pad once with nothing held.
pub const MENU_SETTLE_TICKS: u32 = 30;
/// Port-1 polls the first run may take to reach the title menu (the intro's film included).
const MENU_PROBE_POLLS: u32 = 2000;
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

/// Bytes the stack used from the top of RAM (`ram_top`, `RAM_BASE + RAM_SIZE` of the link
/// under test, read from its map: below 0x8020_0000 when the scene core's window sits above
/// the stack): the frames above `play` plus the depth below its entry.
pub fn watermark(ram_top: u32, entry_sp: u32, depth: u32) -> u32 {
    ram_top - entry_sp + depth
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
    /// from the title menu, up from route tick `menu_tick`: Chapters, the chapter row, the map row.
    pub fn pulses(&self, play_order: usize, menu_tick: u32) -> String {
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
        let mut tick = menu_tick;
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
    /// Read from main RAM (the profile saw a `$sp` below the reserve), not from the profile.
    pub from_ram: bool,
}

/// The address of the game's `MENU_STATE` (screen, highlighted row) in a link map.
pub fn menu_state_address(map: &str) -> Option<u32> {
    map.lines().find_map(|line| {
        let address = line.split_whitespace().next()?;
        line.ends_with(" MENU_STATE")
            .then(|| u32::from_str_radix(address, 16).ok())?
    })
}

/// Where the title menu is up on `cue`: its first route tick and port-1 poll.
#[derive(Clone, Copy, Debug)]
pub struct MenuReady {
    pub tick: u32,
    pub poll: u32,
}

/// Run `cue` until the title menu shows (`MENU_STATE` screen 1 at `menu_state`, its address).
pub fn menu_ready(
    frontend: &Path,
    cue: &Path,
    menu_state: u32,
    out_dir: &Path,
) -> Result<MenuReady> {
    fs::create_dir_all(out_dir)?;
    let log = out_dir.join("menu-probe.csv");
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
        .arg("--route-log")
        .arg(&log)
        .arg("--route-watch-u32")
        .arg(format!("{menu_state:#x}"))
        .args(["--stop-at-poll", &MENU_PROBE_POLLS.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(format!("frontend failed for the title menu probe: {status}").into());
    }
    parse_menu_ready(&fs::read_to_string(&log)?, menu_state).ok_or_else(|| {
        format!(
            "the title menu never showed within {MENU_PROBE_POLLS} polls ({})",
            log.display()
        )
        .into()
    })
}

/// The first row of a route log whose watched `MENU_STATE` word has screen 1 (the main menu).
pub fn parse_menu_ready(text: &str, menu_state: u32) -> Option<MenuReady> {
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next()?.split(',').collect();
    let column = |name: &str| header.iter().position(|field| *field == name);
    let (tick, poll) = (column("route_tick")?, column("port1_polls")?);
    let state = column(&format!("ram_{menu_state:08x}"))?;
    lines.find_map(|line| {
        let fields: Vec<&str> = line.split(',').collect();
        let value: u32 = fields.get(state)?.parse().ok()?;
        (value & 0xff == 1).then(|| {
            Some(MenuReady {
                tick: fields.get(tick)?.parse().ok()?,
                poll: fields.get(poll)?.parse().ok()?,
            })
        })?
    })
}

/// The link's main stack: `STACK_INIT` (the initial `$sp`, under the scene core's window) and
/// the reserve below it.
#[derive(Clone, Copy, Debug)]
pub struct MainStack {
    pub init: u32,
    pub reserve: u32,
}

impl MainStack {
    /// The reserve's lowest address.
    pub fn bottom(&self) -> u32 {
        self.init - self.reserve
    }
}

/// Run `play_order` on `cue` and read its stack watermark under `play_pc`.
#[allow(clippy::too_many_arguments)]
pub fn measure(
    frontend: &Path,
    cue: &Path,
    catalog: &Catalog,
    play_pc: u32,
    ram_top: u32,
    stack: MainStack,
    menu: MenuReady,
    play_order: usize,
    out_dir: &Path,
) -> Result<Reading> {
    fs::create_dir_all(out_dir)?;
    let log = out_dir.join(format!("p{play_order}.csv"));
    let _ = fs::remove_file(&log);
    let pulses = catalog.pulses(play_order, menu.tick + MENU_SETTLE_TICKS);
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
        .arg(&pulses)
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
    let lowest = entry.saturating_sub(depth);
    if lowest >= stack.bottom() {
        return Ok(Reading {
            play_order,
            watermark: watermark(ram_top, entry, depth),
            from_ram: false,
        });
    }
    // Below the reserve: the dispatcher's stack, or an overflow. Read main RAM instead.
    let dump = |polls: u32, name: &str| -> Result<Vec<u8>> {
        let path = out_dir.join(format!("p{play_order}-{name}.ram"));
        let _ = fs::remove_file(&path);
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
            .arg(&pulses)
            .args(["--stop-at-poll", &polls.to_string()])
            .arg("--dump-ram")
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if !status.success() {
            return Err(
                format!("frontend failed dumping play-order {play_order}: {status}").into(),
            );
        }
        let bytes = fs::read(&path)?;
        let _ = fs::remove_file(&path);
        Ok(bytes)
    };
    let before = dump(menu.poll, "menu")?;
    let after = dump(STOP_POLLS, "end")?;
    let written = lowest_written(&before, &after, stack).ok_or_else(|| {
        format!("play-order {play_order}: the RAM dumps do not cover the main stack")
    })?;
    println!(
        "  play-order {play_order}: the profile's lowest $sp {lowest:#x} is below the reserve \
         ({:#x}); main RAM says the stack was written down to {written:#x}",
        stack.bottom()
    );
    Ok(Reading {
        play_order,
        watermark: ram_top - written,
        from_ram: true,
    })
}

/// The lowest word of the main stack reserve that differs between two dumps of main RAM
/// (offset 0 = 0x8000_0000), or `init` when none does.
pub fn lowest_written(before: &[u8], after: &[u8], stack: MainStack) -> Option<u32> {
    let offset = |address: u32| (address & 0x1F_FFFF) as usize;
    let (low, high) = (offset(stack.bottom()), offset(stack.init));
    let (before, after) = (before.get(low..high)?, after.get(low..high)?);
    let changed = before
        .chunks_exact(4)
        .zip(after.chunks_exact(4))
        .position(|(a, b)| a != b);
    Some(changed.map_or(stack.init, |word| stack.bottom() + word as u32 * 4))
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
            "  play-order {:>3}: {:>6} B{}, needs a reserve of {} B{}",
            reading.play_order,
            reading.watermark,
            if reading.from_ram { " (main RAM)" } else { "" },
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
        let first = catalog.pulses(0, 900);
        assert!(first.starts_with("0x0040@900+4,0x0040@916+4,0x0040@932+4,0x0040@948+4,0x4000@964+4,0x4000@980+4,0x4000@996+4,"), "{first}");
        // play-order 6 is chapter 1, row 0: one extra DOWN.
        assert!(catalog
            .pulses(6, 900)
            .contains("0x4000@964+4,0x0040@980+4,0x4000@996+4,0x4000@1012+4"));
        // Hazard Course starts from main-menu row 1.
        assert!(catalog
            .pulses(96, 900)
            .starts_with("0x0040@900+4,0x4000@916+4,0x4000@932+4,"));
    }

    #[test]
    fn parses_the_profile_row() {
        let text = "call,root_pc,return_pc,entry_sp,min_sp,depth_bytes,observations,completed\n0,0x80011064,0x800107fc,0x801ffe78,0x801fdd60,8480,1,1\n";
        assert_eq!(parse_profile(text), Some((0x801f_fe78, 8480)));
        assert_eq!(
            watermark(0x8020_0000, 0x801f_fe78, 8480),
            0x20_0000 - 0x1f_fe78 + 8480
        );
    }

    #[test]
    fn finds_the_title_menu_in_a_route_log() {
        let text = "route_tick,port1_polls,ram_801ae320\n1220,710,0\n1221,711,1\n1222,712,257\n";
        let ready = parse_menu_ready(text, 0x801a_e320).unwrap();
        assert_eq!((ready.tick, ready.poll), (1221, 711));
        assert!(
            parse_menu_ready("route_tick,port1_polls,ram_801ae320\n1,1,0\n", 0x801a_e320).is_none()
        );
    }

    #[test]
    fn reads_the_lowest_written_stack_word() {
        let stack = MainStack {
            init: 0x8000_0100,
            reserve: 0x40,
        };
        let before = vec![0u8; 0x200];
        let mut after = before.clone();
        assert_eq!(lowest_written(&before, &after, stack), Some(0x8000_0100));
        after[0xF0] = 1;
        after[0xD4] = 7;
        assert_eq!(lowest_written(&before, &after, stack), Some(0x8000_00D4));
        after[0x80] = 1; // below the reserve: not the stack's
        assert_eq!(lowest_written(&before, &after, stack), Some(0x8000_00D4));
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
            from_ram: false,
        }];
        assert!(check(&near, None, 12_288).is_err());
        assert!(check(&near, None, 13_312).is_ok());
        // The static bound is held to the same margin.
        assert!(check(&[], Some(20_000), 24_576).is_err());
        assert!(check(&[], Some(20_000), 25_600).is_ok());
    }
}

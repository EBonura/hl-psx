//! Behaviour goldens: build-speed-independent checkpoints of real gameplay.
//!
//! The regression matrix compares a build with itself (run A against run B)
//! and drives input on the emulator's vblank clock, so it cannot say whether
//! a rewrite still plays the same game. A golden route instead feeds a
//! poll-bound input tape (one pad sample per simulation tick) and stops at
//! fixed poll counts, so two builds of different speed reach the same
//! simulation state. At each checkpoint it keeps:
//!
//! - the displayed frame, and
//! - a state record: the live prop table (kind, position, yaw, state, health),
//!   every logic entity's state byte and the brush-entity table, read from a
//!   RAM dump through the build's link map.
//!
//! `cargo hl-build goldens --golden-dir DIR --record` writes them; without
//! `--record` the same run compares against DIR and fails on any difference.
//! Goldens are derived from Half-Life data and stay outside the repository.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use super::Result;

const L1: u16 = 1 << 10;
const R1: u16 = 1 << 11;
const UP: u16 = 0x0010;
const RIGHT: u16 = 0x0020;
const DOWN: u16 = 0x0040;
const L2: u16 = 0x0100;
const R2: u16 = 0x0200;
const L3: u16 = 0x0002;
const CROSS: u16 = 0x4000;
const SQUARE: u16 = 0x8000;
/// Pad reads the debug boot makes before the first simulation tick.
const BOOT_POLLS: u32 = 2;

/// Which disc a golden boots.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Disc {
    /// The instrumented regression disc: direct map boot plus viewpoints.
    Regression,
    /// The shipping disc: the interactive menu.
    Shipping,
}

/// Hold `buttons` for `polls` simulation ticks starting at poll `from`.
struct Press {
    from: u32,
    polls: u32,
    buttons: u16,
}

const fn press(from: u32, polls: u32, buttons: u16) -> Press {
    Press {
        from,
        polls,
        buttons,
    }
}

struct Golden {
    name: &'static str,
    disc: Disc,
    map_index: u8,
    /// Debug selector: weapon gallery id + 1, or a viewpoint selector.
    selector: u8,
    /// Boot with R1 held: the game's own scripted walk on c1a0.
    route: bool,
    presses: &'static [Press],
    checkpoints: &'static [u32],
}

const fn golden(
    name: &'static str,
    map_index: u8,
    selector: u8,
    presses: &'static [Press],
    checkpoints: &'static [u32],
) -> Golden {
    Golden {
        name,
        disc: Disc::Regression,
        map_index,
        selector,
        route: false,
        presses,
        checkpoints,
    }
}

const GOLDENS: &[Golden] = &[
    // Player movement on a real map: the c1a0 tick-owned walk and turn route.
    Golden {
        name: "golden-walk",
        disc: Disc::Regression,
        map_index: 6,
        selector: 2,
        route: true,
        presses: &[],
        checkpoints: &[100, 200, 300, 380],
    },
    // Swim to the second t0a0c basin exit and climb out over its lip.
    golden(
        "golden-water-exit",
        101,
        14,
        &[press(5, 400, UP | CROSS)],
        &[60, 120, 200, 300],
    ),
    // Climb the t0a0 ladder, with a short sideways nudge on the way up.
    golden(
        "golden-ladder",
        96,
        13,
        &[press(5, 170, UP), press(25, 6, RIGHT)],
        &[60, 120, 200],
    ),
    // Pull the c1a0 office crate backwards with held use.
    golden(
        "golden-pushable",
        6,
        11,
        &[press(5, 300, SQUARE | DOWN)],
        &[60, 150, 300],
    ),
    // The c1a0 security door's opening motion from a fixed camera.
    golden("golden-door", 6, 15, &[], &[5, 10, 15, 20, 25, 30, 40, 80]),
    // c1a0 and c1a0a scientists: scripted walks, greetings and small talk.
    golden("golden-c1a0-scripts", 6, 0, &[], &[100, 250, 400, 600]),
    golden("golden-c1a0a-talk", 7, 0, &[], &[100, 300, 500, 700]),
    // t0a0 observation-room scientists behind the cutout glass.
    golden("golden-t0a0-scientists", 96, 15, &[], &[100, 300]),
    // HUD: the suit lamp on and draining, then the weapon selection strip.
    golden(
        "golden-hud-flashlight",
        18,
        2,
        &[press(20, 2, L3)],
        &[30, 200, 400],
    ),
    golden(
        "golden-hud-selection",
        18,
        2,
        &[press(20, 2, R1)],
        &[24, 40],
    ),
    // Tau Cannon: charge for five seconds, release, then a primary shot.
    golden(
        "golden-gauss",
        18,
        8,
        &[press(20, 100, L2), press(150, 2, R2)],
        &[40, 100, 125, 160],
    ),
    // Pickup history: stand on the c1a2 battery.
    golden("golden-pickup", 18, 11, &[], &[5, 40, 100]),
    // Train indicator: take the t0a0d training train and notch it forward.
    golden(
        "golden-train",
        102,
        13,
        &[press(30, 2, SQUARE), press(45, 2, UP), press(60, 2, UP)],
        &[50, 70, 150],
    ),
    // c2a2d: the silo guard gun tracks and shoots the player at the start.
    golden("golden-tank", 45, 0, &[], &[20, 40, 60, 80, 120, 200]),
    // c1a3b: the osprey takes off and circles over the start. The grunts
    // there kill the player before poll 200, so the checkpoints stay early.
    golden("golden-osprey", 25, 15, &[], &[40, 70, 100, 130]),
    // c2a5g: the table-guided and random mortar fields, fired at tick 20,
    // seen from the table looking at where the guided shells land. The first
    // shell lands 2.5 s after the use.
    golden("golden-mortar", 71, 15, &[], &[60, 72, 74, 76, 80, 90, 110]),
    // c1a3c: a player-guided mortar field, fired at tick 20.
    golden(
        "golden-mortar-player",
        26,
        15,
        &[],
        &[60, 70, 72, 74, 76, 80, 100],
    ),
    // c2a5: the apache, triggered at tick 20, hunting the player.
    golden("golden-apache", 64, 14, &[], &[60, 200, 400, 600]),
    // c4a3: the nihilanth.
    golden("golden-boss", 94, 15, &[], &[100, 300, 600]),
    // The shipping menu: idle, cursor down, then into the highlighted item.
    Golden {
        name: "golden-menu",
        disc: Disc::Shipping,
        map_index: 0,
        selector: 0,
        route: false,
        presses: &[press(320, 2, DOWN), press(360, 2, CROSS)],
        checkpoints: &[300, 340, 400],
    },
];

/// Statics whose bytes make up a checkpoint's state record. They belong to
/// the game loop, not to any module a clean-room rewrite replaces, so their
/// layout is stable across those rewrites.
const STATE_SYMBOLS: &[&str] = &[
    "PROP_COUNT",
    "PROP_ACTIVE",
    "PROP_KIND",
    "PROP_POS",
    "PROP_YAW",
    "PROP_STATE",
    "PROP_HEALTH",
    "LOGIC_STATE",
    "ENT_ACTIVE",
    "ENT_PHASE",
];

/// One poll-bound `PXITAPE2` tape. The debug boot reads the pad twice before
/// gameplay (the map, then the viewpoint selector), so samples 0 and 1 carry
/// the boot selection; gameplay input starts at poll 2. A tape holding it for
/// those two polls reproduces a 400-vblank boot pulse frame for frame.
fn tape(g: &Golden) -> Vec<u8> {
    let total = g.checkpoints.iter().copied().max().unwrap_or(0) + 16;
    let boot = if g.disc == Disc::Regression {
        L1 | g.map_index as u16 | ((g.selector as u16) << 12) | if g.route { R1 } else { 0 }
    } else {
        0
    };
    let mut bytes = Vec::with_capacity(16 + total as usize * 6);
    bytes.extend_from_slice(b"PXITAPE2");
    bytes.extend_from_slice(&total.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    for poll in 0..total {
        let mut buttons = if poll < BOOT_POLLS { boot } else { 0 };
        for p in g.presses {
            if poll >= p.from && poll < p.from + p.polls {
                buttons |= p.buttons;
            }
        }
        bytes.extend_from_slice(&buttons.to_le_bytes());
        bytes.extend_from_slice(&[0x80, 0x80, 0x80, 0x80]);
    }
    bytes
}

/// `(address, size)` of each state symbol in a link map.
fn state_layout(link_map: &Path) -> Result<Vec<(&'static str, u32, u32)>> {
    let text = fs::read_to_string(link_map)?;
    let mut out = Vec::new();
    for name in STATE_SYMBOLS {
        let needle = format!("hl_psx::{name}");
        let line = text
            .lines()
            .find(|line| line.trim_end().ends_with(&needle) && line.split_whitespace().count() == 5)
            .ok_or_else(|| format!("{name} is not in {}", link_map.display()))?;
        let fields: Vec<&str> = line.split_whitespace().collect();
        let address = u32::from_str_radix(fields[0], 16)?;
        let size = u32::from_str_radix(fields[2], 16)?;
        out.push((*name, address, size));
    }
    Ok(out)
}

fn state_record(ram: &[u8], layout: &[(&'static str, u32, u32)]) -> Result<String> {
    let mut text = String::new();
    for &(name, address, size) in layout {
        let start = (address & 0x1f_ffff) as usize;
        let end = start + size as usize;
        let bytes = ram
            .get(start..end)
            .ok_or_else(|| format!("{name} lies outside the RAM dump"))?;
        text.push_str(name);
        for chunk in bytes.chunks(32) {
            text.push_str("\n  ");
            for byte in chunk {
                text.push_str(&format!("{byte:02x}"));
            }
        }
        text.push('\n');
    }
    Ok(text)
}

struct Checkpoint<'a> {
    golden: &'a Golden,
    poll: u32,
}

fn launch(frontend: &Path, cue: &Path, tape: &Path, poll: u32, out: &Path) -> Result<()> {
    let status = Command::new(frontend)
        .args(["launch", "--path"])
        .arg(cue)
        .arg("--embedded-playtest")
        .args(["--steps", "4000000000", "--stop-at-poll"])
        .arg(poll.to_string())
        .arg("--input-tape")
        .arg(tape)
        .arg("--dump-display")
        .arg(out.join(format!("poll-{poll}.ppm")))
        .arg("--dump-ram")
        .arg(out.join(format!("poll-{poll}.ram")))
        .stdout(fs::File::create(out.join(format!("poll-{poll}.log")))?)
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(format!("frontend failed at poll {poll} in {}", out.display()).into());
    }
    let log = fs::read_to_string(out.join(format!("poll-{poll}.log")))?;
    let reached = log
        .split_whitespace()
        .find_map(|field| field.strip_prefix("port1-polls="))
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    if reached < poll {
        return Err(format!(
            "{} stopped at poll {reached} before checkpoint {poll} (step cap)",
            out.display()
        )
        .into());
    }
    Ok(())
}

fn ppm_pixels(bytes: &[u8]) -> &[u8] {
    // P6 header: magic, width, height, maxval, each followed by whitespace.
    let mut fields = 0;
    let mut i = 0;
    while fields < 4 && i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        fields += 1;
    }
    &bytes[(i + 1).min(bytes.len())..]
}

fn changed_pixels(a: &[u8], b: &[u8]) -> usize {
    let (a, b) = (ppm_pixels(a), ppm_pixels(b));
    if a.len() != b.len() {
        return usize::MAX;
    }
    a.chunks(3).zip(b.chunks(3)).filter(|(x, y)| x != y).count()
}

pub struct Discs<'a> {
    pub regression_cue: &'a Path,
    pub regression_map: &'a Path,
    pub shipping_cue: &'a Path,
    pub shipping_map: &'a Path,
}

/// Run every golden route (or the one named `only`), then record into or
/// compare against `golden_dir`.
pub fn run(
    repository: &Path,
    frontend: &Path,
    discs: &Discs<'_>,
    only: Option<&str>,
    golden_dir: &Path,
    record: bool,
) -> Result<()> {
    let selected: Vec<&Golden> = GOLDENS
        .iter()
        .filter(|g| only.is_none_or(|name| name == g.name))
        .collect();
    if selected.is_empty() {
        return Err(format!("unknown golden `{}`", only.unwrap_or_default()).into());
    }
    let root = repository.join(".hlpsx/goldens");
    let regression_layout = state_layout(discs.regression_map)?;
    let shipping_layout = state_layout(discs.shipping_map)?;
    let mut work = Vec::new();
    for g in &selected {
        let out = root.join(g.name);
        if out.is_dir() {
            fs::remove_dir_all(&out)?;
        }
        fs::create_dir_all(&out)?;
        fs::write(out.join("route.tape"), tape(g))?;
        for &poll in g.checkpoints {
            work.push(Checkpoint { golden: g, poll });
        }
    }

    // The frontend is single-threaded; run two checkpoints at once so a
    // shared machine keeps headroom for other builds.
    let queue = Mutex::new(work.into_iter());
    let errors = Mutex::new(Vec::<String>::new());
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(2).clamp(1, 2))
        .unwrap_or(2);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let Some(job) = queue.lock().unwrap().next() else {
                    break;
                };
                let out = root.join(job.golden.name);
                let cue = match job.golden.disc {
                    Disc::Regression => discs.regression_cue,
                    Disc::Shipping => discs.shipping_cue,
                };
                println!("  {} poll {}", job.golden.name, job.poll);
                if let Err(error) = launch(frontend, cue, &out.join("route.tape"), job.poll, &out) {
                    errors.lock().unwrap().push(error.to_string());
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("; ").into());
    }

    let mut report = String::from("golden,poll,frame,changed_pixels,state\n");
    let mut failures = Vec::new();
    for g in &selected {
        let out = root.join(g.name);
        let keep = golden_dir.join(g.name);
        if record {
            fs::create_dir_all(&keep)?;
        }
        let layout = match g.disc {
            Disc::Regression => &regression_layout,
            Disc::Shipping => &shipping_layout,
        };
        for &poll in g.checkpoints {
            let frame = fs::read(out.join(format!("poll-{poll}.ppm")))?;
            let ram = fs::read(out.join(format!("poll-{poll}.ram")))?;
            let state = state_record(&ram, layout)?;
            fs::write(out.join(format!("poll-{poll}.state")), &state)?;
            // RAM dumps are large and only the state record is kept.
            fs::remove_file(out.join(format!("poll-{poll}.ram")))?;
            let frame_name = format!("poll-{poll}.ppm");
            let state_name = format!("poll-{poll}.state");
            if record {
                fs::write(keep.join(&frame_name), &frame)?;
                fs::write(keep.join(&state_name), &state)?;
                report.push_str(&format!("{},{poll},recorded,0,recorded\n", g.name));
                continue;
            }
            let want_frame = fs::read(keep.join(&frame_name)).map_err(|error| {
                format!(
                    "missing golden {} ({error})",
                    keep.join(&frame_name).display()
                )
            })?;
            let want_state = fs::read_to_string(keep.join(&state_name)).map_err(|error| {
                format!(
                    "missing golden {} ({error})",
                    keep.join(&state_name).display()
                )
            })?;
            let changed = if want_frame == frame {
                0
            } else {
                changed_pixels(&want_frame, &frame)
            };
            let state_same = want_state == state;
            report.push_str(&format!(
                "{},{poll},{},{changed},{}\n",
                g.name,
                if changed == 0 { "same" } else { "DIFFERS" },
                if state_same { "same" } else { "DIFFERS" }
            ));
            if changed != 0 || !state_same {
                failures.push(format!(
                    "{} poll {poll}: {changed} pixels changed, state {}",
                    g.name,
                    if state_same { "same" } else { "differs" }
                ));
            }
        }
    }
    let report_path = root.join("report.csv");
    fs::write(&report_path, report)?;
    println!("golden report -> {}", report_path.display());
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; ").into())
    }
}

/// Where the goldens action keeps each disc and its link map.
pub fn disc_dir(repository: &Path, shipping: bool) -> PathBuf {
    repository.join(if shipping {
        ".hlpsx/golden-shipping-disc"
    } else {
        ".hlpsx/golden-regression-disc"
    })
}

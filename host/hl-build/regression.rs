use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{cargo, run, Result};

// PSoXide's measured NTSC visual deadline: 2.967 vblanks at 564,480 bus
// cycles/vblank. This is the real 20 fps gate used by the frontend profiler.
const TARGET_RENDER_CYCLES: u64 = 1_674_624;
const PSX_CPU_HZ: f64 = 33_868_800.0;
const L1: u16 = 1 << 10;
const R1: u16 = 1 << 11;

const TIMING_STAGES: [&str; 21] = [
    "frame_cycles",
    "fixed_update_task",
    "update",
    "render",
    "present",
    "room",
    "room_visible_list",
    "room_cell_select",
    "room_depth_prep",
    "room_project",
    "room_surface_draw",
    "model_instances",
    "model_bounds",
    "textured_model_joints",
    "textured_model_project",
    "textured_model_faces",
    "equipment",
    "world_flush",
    "ot_submit",
    "ot_wait",
    "sim_collision",
];

struct Scenario {
    name: &'static str,
    map: &'static str,
    map_index: u8,
    frames: u32,
    weapon_selector: u8,
    subject_viewpoint: bool,
    extra_pulses: &'static str,
}

// Each reported regression has an explicit route. The map selector is sampled
// before gameplay; later pulses exercise the affected system without relying
// on menu timing or host input cadence.
const SCENARIOS: [Scenario; 32] = [
    // Default c2a5 spawn looks obliquely across a large tiled rooftop. It is a
    // deterministic stress view for the single cooked shared-edge mesh and
    // must remain comfortably inside the 20 fps budget.
    Scenario {
        name: "world-subdivision",
        map: "c2a5",
        map_index: 64,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: false,
        extra_pulses: "",
    },
    // Anomalous Materials is the primary visual-parity route: the entry, main
    // laboratory run, and exit map collectively expose close walls, long
    // corridors, machinery, railings, and the highest-refinement chapter map.
    Scenario {
        name: "anomalous-entry",
        map: "c1a0",
        map_index: 6,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Exact player-authored c1a0 hallway pose recovered from a PSoXide save.
    // This straight corridor view makes affine discontinuities obvious across
    // the long floor, ceiling strips, and the coloured bands on the right wall.
    Scenario {
        name: "anomalous-hallway",
        map: "c1a0",
        map_index: 6,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Real gameplay performance route: a simulation-tick-owned command stream
    // walks and turns through c1a0's first room. Unlike a host/vblank tape it
    // reaches identical camera states when renderer cost changes; selector 2
    // keeps the Glock viewmodel resident and rendered for the whole sample.
    Scenario {
        name: "anomalous-walk",
        map: "c1a0",
        map_index: 6,
        frames: 360,
        weapon_selector: 2,
        subject_viewpoint: false,
        extra_pulses: "",
    },
    // Guest frame 20 catches the c1a0 security door before its moving panels
    // can hide a corrupt compact face or an axial wheel taking the wrong
    // reflected arc. `launch` gives this route a fixed guest-frame stop rather
    // than a render-count stop, so renderer cost cannot change door phase.
    Scenario {
        name: "anomalous-door-motion",
        map: "c1a0",
        map_index: 6,
        frames: 55,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    Scenario {
        name: "anomalous-lab",
        map: "c1a0c",
        map_index: 9,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Sweep the same close-wall laboratory view slowly right and then left.
    // The diagnostic camera consumes D-pad strafe as yaw while pinning its
    // position, exercising refined-face ordering across hundreds of changing
    // projections without collision or actor motion contaminating the route.
    Scenario {
        name: "anomalous-motion",
        map: "c1a0c",
        map_index: 9,
        frames: 240,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: ",0x0020@500+450,0x0080@950+450",
    },
    Scenario {
        name: "anomalous-exit",
        map: "c1a0e",
        map_index: 11,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Close view of c1a0e's translucent motor-button cover. The map writes
    // plural zero angles before a legacy 45-degree angle and selects the
    // GoldSrc roll axis; both the base pose and swing axis must survive cook.
    Scenario {
        name: "anomalous-cover",
        map: "c1a0e",
        map_index: 11,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // c5a1 mirrors c0a0e's angled tram-door func_train. This fixed camera sits
    // inside the final tram and keeps the closed door against both jambs, so
    // rotating the model centre before subtracting it opens a visible gap.
    Scenario {
        name: "endgame-tram-door",
        map: "c5a1",
        map_index: 95,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Near-field probes for world rails and studio layers sharing OT buckets.
    Scenario {
        name: "hazard-railing-depth",
        map: "t0a0a",
        map_index: 97,
        frames: 360,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    Scenario {
        name: "hazard-scientist-depth",
        map: "t0a0",
        map_index: 96,
        frames: 360,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Fixed reproduction of the t0a0a ramp/pipe/hologram ordering failure.
    // This frame exercises local refined-cell keys while retaining the Holo's
    // additive self-ordering and ordinary world-wall occlusion in one stable
    // comparison image.
    Scenario {
        name: "hazard-ordering-fixed",
        map: "t0a0a",
        map_index: 97,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Sweep away from and back to the exact fixed camera. The pinned observer
    // removes traversal/AI noise so changing hashes are solely projections of
    // the refined world and additive Holo against its surrounding walls.
    Scenario {
        name: "hazard-ordering-yaw",
        map: "t0a0a",
        map_index: 97,
        frames: 240,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: ",0x0020@500+450,0x0080@950+450",
    },
    // Exact player-authored t0a0 floor-grid pose recovered from PSoXide save
    // slot 3. Its steep grazing angle is the regression for thin binary-alpha
    // grate bars collapsing into transparent texels across large affine faces.
    Scenario {
        name: "hazard-floor-grid",
        map: "t0a0",
        map_index: 96,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Walk only a few steps from the t0a0a hologram-room entrance. The foreground floor then
    // crosses the near/guard boundary and exposes any large affine triangle
    // that escapes the ordinary complete-patch ranking path.
    Scenario {
        name: "hazard-entry-edge",
        map: "t0a0",
        map_index: 96,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Exact save-slot 14 view toward the observation room. Three scientists
    // pass both the map PVS and camera frustum here; model *93 between them and
    // the player is rendermode 4, so its transparent texels must not turn its
    // complete BSP volume into an actor visibility occluder.
    Scenario {
        name: "hazard-scientist-cutout",
        map: "t0a0",
        map_index: 96,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Fixed view of the three jump-completion panels. Held Use is intercepted
    // only by the diagnostic build and routes USE_ON through each cooked
    // lightstyle LogicEnt; the paired neutral run below proves the rendered
    // panels actually change rather than merely existing in target logic.
    Scenario {
        name: "hazard-panel-lights",
        map: "t0a0",
        map_index: 96,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: ",0x8000@650+600",
    },
    // Fixed observer outside t0a0a's vertical func_train. The diagnostic
    // route starts the real lift after twenty simulation ticks; successive
    // visual hashes must follow its TRAIN_OFF translation up the shaft rather
    // than replaying one world-aligned brush packet for the entire journey.
    Scenario {
        name: "hazard-elevator-motion",
        map: "t0a0a",
        map_index: 97,
        frames: 90,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    // Live lower-face mount on t0a0's ladder model *149. Selector 13 chooses
    // an unpinned diagnostic start; the sustained Up pulse must seat the
    // player and produce substantial vertical travel without lateral escape.
    Scenario {
        name: "hazard-ladder-mount",
        map: "t0a0",
        map_index: 96,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        // A short Right overlap would eject the old full-speed strafe path.
        extra_pulses: ",0x0010@600+500,0x0020@660+18",
    },
    // t0a0a has two momentary_rot_buttons sharing target `momentary` with a
    // momentary_door. Hold +use on the near wheel: all three brush entities
    // must consume the same normalized phase rather than animating the aimed
    // wheel in isolation.
    Scenario {
        name: "hazard-valve-link",
        map: "t0a0a",
        map_index: 97,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: ",0x8000@650+600",
    },
    // c1a0's model *64 office crate starts against the player. GoldSrc continuous
    // use keeps the aimed box selected after the first backward step opens a
    // physical gap; the paired control run below holds Back without Use.
    Scenario {
        name: "hazard-pushable-pull",
        map: "c1a0",
        map_index: 6,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: ",0x8040@400+10000",
    },
    // Exact save-slot 13 view of t0a0b target 1 at its raised path endpoint.
    // The camera remains fixed while the brush occupies a different BSP leaf
    // from its authored origin, proving live mover PVS relinking directly.
    Scenario {
        name: "hazard-guntarget-visibility",
        map: "t0a0b",
        map_index: 98,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    Scenario {
        name: "fan-axis",
        map: "c1a2",
        map_index: 18,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    Scenario {
        name: "laser-occlusion",
        map: "c1a3",
        map_index: 23,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    Scenario {
        name: "shotgun-reload",
        map: "c1a2",
        map_index: 18,
        // Stop six simulation frames after the reload transition: late enough
        // to show the authored start-reload pose, before the first shell lands.
        frames: 210,
        // Debug gallery selector is weapon id + 1; shotgun is id 4.
        weapon_selector: 5,
        subject_viewpoint: false,
        // Input is sampled at 20 Hz while route pulses are vblank-based. Hold
        // each command across several simulation ticks so neither can land
        // entirely between ItemPostFrame samples.
        // Route pulses use PSoXide's vblank clock. The faster renderer reaches
        // visual frame 210 substantially earlier than the old baseline; these
        // timings keep the final proof image inside the active authored reload.
        extra_pulses: ",0x0200@1274+30,0x2000@1374+100",
    },
    // Switch Glock -> .357 (cold CD stream) -> Glock (tail-cache hit). This
    // proves the previous merged weapon survives the projection/sort overlay
    // and can republish without a second disc read or CDDA interruption.
    Scenario {
        name: "weapon-cache-toggle",
        // c1a2b is also the model-roster RAM stress scene, but renders fast
        // enough that this proof reaches both switches before the headless
        // safety limit. The slower c1a2 route obscured cache correctness with
        // an unrelated, already-known performance failure.
        map: "c1a2b",
        map_index: 20,
        frames: 170,
        weapon_selector: 2,
        subject_viewpoint: false,
        // c1a2b finishes its direct-map load at roughly vblank 730. Keep both
        // edges inside gameplay and far enough apart for the 19-sector cold
        // stream to finish before requesting the cached Glock.
        extra_pulses: ",0x0800@800+60,0x0400@1200+60",
    },
    Scenario {
        name: "water-surface",
        map: "c2a5",
        map_index: 64,
        frames: 240,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: ",0x4000@700+8",
    },
    // Exact player-authored pose below the second t0a0c swim-basin exit.
    // Sustained look-up + forward+jump must retain GoldSrc's combined paddle
    // and wish acceleration and carry the player above the authored lip.
    Scenario {
        name: "hazard-water-exit",
        map: "t0a0c",
        map_index: 101,
        frames: 240,
        weapon_selector: 0,
        subject_viewpoint: true,
        // The authored manoeuvre: look steeply upward, then hold forward+jump.
        // GoldSrc combines the 100 u/s paddle with upward wish acceleration;
        // applying the paddle afterward loses that acceleration and hits the
        // solid north rim before gaining enough height.
        extra_pulses: ",0x4010@650+1200",
    },
    Scenario {
        name: "enemy-animation-boss",
        map: "c4a3",
        map_index: 94,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: true,
        extra_pulses: "",
    },
    Scenario {
        name: "ram-peak-roster",
        map: "c1a2b",
        map_index: 20,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: false,
        extra_pulses: ",0x0020@620+120",
    },
    Scenario {
        name: "model-run-peak",
        map: "c3a2c",
        map_index: 80,
        frames: 180,
        weapon_selector: 0,
        subject_viewpoint: false,
        extra_pulses: ",0x0020@620+120",
    },
];

// One fixed-camera idle capture per authored first-person model. Keeping this
// in the same deterministic matrix catches lost packed normals, corrupt VRAM
// texture placement, chrome-UV regressions, and a weapon that no longer fits
// the on-demand viewmodel pool. Selector = weapon id + 1.
const WEAPON_SCENARIOS: [Scenario; 14] = [
    weapon_scenario("weapon-crowbar", 1),
    weapon_scenario("weapon-glock", 2),
    weapon_scenario("weapon-357", 3),
    weapon_scenario("weapon-mp5", 4),
    Scenario {
        name: "weapon-shotgun",
        map: "c1a2",
        map_index: 18,
        // Match shotgun-reload exactly so their proof frames differ only by
        // the fire/reload route, making the animation check deterministic.
        frames: 210,
        weapon_selector: 5,
        subject_viewpoint: false,
        extra_pulses: "",
    },
    weapon_scenario("weapon-crossbow", 6),
    weapon_scenario("weapon-rpg", 7),
    weapon_scenario("weapon-gauss", 8),
    weapon_scenario("weapon-egon", 9),
    weapon_scenario("weapon-hornet", 10),
    weapon_scenario("weapon-grenade", 11),
    weapon_scenario("weapon-snark", 12),
    weapon_scenario("weapon-tripmine", 13),
    weapon_scenario("weapon-satchel", 14),
];

// HUD parity captures stop after c1a2's OFFICE COMPLEX card has cleared.  The
// c1a2a direct spawn eventually enters a full-screen red map fade, which made a
// deterministic terminal image useless as visual evidence. Together these
// prove the native 320-res fixed fields, two ammo layouts, weapon-specific
// crosshairs, the MP5 secondary-ammo line, and the crossbow zoom reticle.
const HUD_SCENARIOS: [Scenario; 5] = [
    hud_scenario("hud-glock", 2, ""),
    hud_scenario("hud-mp5-secondary", 4, ""),
    // L3 toggles the HEV lamp and remains held through the proof frame.
    // c1a2 has entered gameplay by this point. A short edge is long enough for
    // a 20 Hz sample without letting the synthetic pad route reconfigure while
    // L3 remains held.
    Scenario {
        name: "hud-flashlight",
        map: "c1a2",
        map_index: 18,
        // Continue long enough for the lamp to own most steady-state renders
        // and drain enough charge to move the horizontally clipped HUD fill.
        frames: 340,
        weapon_selector: 2,
        subject_viewpoint: false,
        extra_pulses: ",0x0002@1150+8",
    },
    // Switch late enough that the 30-tick selection strip is still present in
    // the proof frame (R1 = 0x0800). This exercises the maximum HUD packet mix.
    hud_scenario("hud-weapon-selection", 2, ",0x0800@1150+100"),
    // Tap L2 after the direct-boot pulse. Crossbow zoom is a latched 20-degree
    // FOV state; a release must not cancel it.
    hud_scenario("hud-crossbow", 6, ",0x0100@1150+12"),
];

// Mode-pair captures exercise the physical R2/L2 paths, not direct state
// injection. Their hash streams are compared below so a secondary path that
// silently aliases primary fire (or does nothing) fails deterministically.
const FIRE_SCENARIOS: [Scenario; 16] = [
    fire_scenario("fire-glock-primary", 2, ",0x0200@1150+40"),
    fire_scenario("fire-glock-secondary", 2, ",0x0100@1150+40"),
    fire_scenario("fire-mp5-primary", 4, ",0x0200@1150+40"),
    fire_scenario("fire-mp5-secondary", 4, ",0x0100@1150+40"),
    fire_scenario("fire-shotgun-primary", 5, ",0x0200@1150+40"),
    fire_scenario("fire-shotgun-secondary", 5, ",0x0100@1150+40"),
    fire_scenario("fire-hornet-primary", 10, ",0x0200@1150+40"),
    fire_scenario("fire-hornet-secondary", 10, ",0x0100@1150+40"),
    fire_scenario("fire-crossbow-primary", 6, ",0x0200@1150+12"),
    fire_scenario("fire-crossbow-secondary", 6, ",0x0100@1150+12"),
    fire_scenario("fire-rpg-primary", 7, ",0x0200@1150+12"),
    fire_scenario("fire-rpg-secondary", 7, ",0x0100@1150+12"),
    fire_scenario("fire-gauss-primary", 8, ",0x0200@1150+12"),
    fire_scenario("fire-gauss-secondary", 8, ",0x0100@1000+300"),
    satchel_fire_scenario("fire-satchel-primary", ",0x0200@1050+12"),
    satchel_fire_scenario("fire-satchel-secondary", ",0x0200@1050+12,0x0100@1300+12"),
];

const MODE_PAIRS: [(&str, &str); 8] = [
    ("fire-glock-primary", "fire-glock-secondary"),
    ("fire-mp5-primary", "fire-mp5-secondary"),
    ("fire-shotgun-primary", "fire-shotgun-secondary"),
    ("fire-hornet-primary", "fire-hornet-secondary"),
    ("fire-crossbow-primary", "fire-crossbow-secondary"),
    ("fire-rpg-primary", "fire-rpg-secondary"),
    ("fire-gauss-primary", "fire-gauss-secondary"),
    ("fire-satchel-primary", "fire-satchel-secondary"),
];

/// Visual-only targeted probes do not need the debug weapon gallery. Keeping
/// it out of those binaries recovers the RAM that their camera/affine
/// instrumentation needs, while the full matrix and every weapon-bearing route
/// retain the gallery exactly as before.
pub fn needs_weapon_gallery(only: Option<&str>) -> bool {
    let Some(name) = only else {
        return true;
    };
    if matches!(name, "fire-matrix" | "tessellation-matrix") {
        return true;
    }
    SCENARIOS
        .iter()
        .chain(WEAPON_SCENARIOS.iter())
        .chain(HUD_SCENARIOS.iter())
        .chain(FIRE_SCENARIOS.iter())
        .find(|scenario| scenario.name == name)
        .map_or(true, |scenario| scenario.weapon_selector != 0)
}

const fn hud_scenario(name: &'static str, selector: u8, extra_pulses: &'static str) -> Scenario {
    Scenario {
        name,
        map: "c1a2",
        map_index: 18,
        frames: 180,
        weapon_selector: selector,
        subject_viewpoint: false,
        extra_pulses,
    }
}

const fn weapon_scenario(name: &'static str, selector: u8) -> Scenario {
    Scenario {
        name,
        map: "c1a2",
        map_index: 18,
        frames: 120,
        weapon_selector: selector,
        subject_viewpoint: false,
        extra_pulses: "",
    }
}

const fn fire_scenario(name: &'static str, selector: u8, extra_pulses: &'static str) -> Scenario {
    Scenario {
        name,
        map: "c1a2",
        map_index: 18,
        frames: 180,
        weapon_selector: selector,
        subject_viewpoint: false,
        extra_pulses,
    }
}

const fn satchel_fire_scenario(name: &'static str, extra_pulses: &'static str) -> Scenario {
    Scenario {
        name,
        map: "c1a2",
        map_index: 18,
        frames: 210,
        weapon_selector: 14,
        subject_viewpoint: false,
        extra_pulses,
    }
}

fn frontend(psoxide: &Path) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HLPSX_FRONTEND") {
        let binary = PathBuf::from(path);
        if !binary.is_file() {
            return Err(
                format!("HLPSX_FRONTEND does not name a file: {}", binary.display()).into(),
            );
        }
        return Ok(binary);
    }
    let binary = psoxide.join("target/release").join(if cfg!(windows) {
        "frontend.exe"
    } else {
        "frontend"
    });
    // A local fidelity checkout changes frequently during hardware passes.
    // Cargo's incremental no-op is cheap; silently reusing an older frontend
    // binary invalidates every visual/performance conclusion from this matrix.
    let mut command = Command::new(cargo());
    command
        .current_dir(psoxide)
        // Headless regressions exercise the emulator core, GPU, SPU and CLI;
        // they do not embed the editor. Keeping default editor features here
        // made unrelated in-progress FBX/UI work block hardware-fidelity tests.
        .args([
            "build",
            "--release",
            "-p",
            "frontend",
            "--no-default-features",
        ]);
    run(&mut command, "build PSoXide headless frontend")?;
    if !binary.is_file() {
        return Err(format!("PSoXide frontend missing at {}", binary.display()).into());
    }
    Ok(binary)
}

fn csv_column(path: &Path, name: &str) -> Result<Vec<String>> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| format!("{} is empty", path.display()))?;
    let column = header
        .split(',')
        .position(|field| field == name)
        .ok_or_else(|| format!("{} has no `{name}` column", path.display()))?;
    Ok(lines
        .filter_map(|line| line.split(',').nth(column).map(str::to_owned))
        .collect())
}

fn csv_column_occurrence(path: &Path, name: &str, occurrence: usize) -> Result<Vec<String>> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| format!("{} is empty", path.display()))?;
    let column = header
        .split(',')
        .enumerate()
        .filter_map(|(index, field)| (field == name).then_some(index))
        .nth(occurrence)
        .ok_or_else(|| {
            format!(
                "{} has no occurrence {} of `{name}`",
                path.display(),
                occurrence + 1
            )
        })?;
    Ok(lines
        .filter_map(|line| line.split(',').nth(column).map(str::to_owned))
        .collect())
}

fn write_affine_error_summary(profile: &Path, out: &Path) -> Result<()> {
    let frames = csv_column(profile, "guest_frame")?;
    let max_q8 = csv_column(profile, "room_surf_material")?;
    // PSoXide's profile schema predates this game-specific diagnostic; these
    // otherwise-unused legacy room-counter columns are relabelled here.
    let p50_q8 = csv_column_occurrence(profile, "room_surf_projected", 0)?;
    let p95_q8 = csv_column_occurrence(profile, "room_surf_screen", 0)?;
    let p99_q8 = csv_column_occurrence(profile, "room_surf_kind", 0)?;
    let candidates = csv_column_occurrence(profile, "room_surf_backface", 0)?;
    let requested = csv_column_occurrence(profile, "room_surf_lighting", 0)?;
    let emitted = csv_column_occurrence(profile, "room_surf_submit", 0)?;
    let split_patches = csv_column(profile, "room_surf_screen_culled")?;
    let added_gte = csv_column(profile, "room_surf_backface_culled")?;
    let native_gt4 = csv_column(profile, "room_surf_whole_quads")?;
    let remaining_max_q8 = csv_column(profile, "room_submit_hw_safe_test")?;
    let remaining_p50_q8 = csv_column(profile, "room_submit_packet_fill")?;
    let remaining_p95_q8 = csv_column(profile, "room_submit_primitive_push")?;
    let remaining_p99_q8 = csv_column(profile, "room_submit_depth")?;
    let lowest_selected_priority = csv_column(profile, "room_submit_command")?;
    let highest_rejected_priority = csv_column(profile, "room_submit_fallback")?;
    let rows = [
        frames.len(),
        max_q8.len(),
        p50_q8.len(),
        p95_q8.len(),
        p99_q8.len(),
        candidates.len(),
        requested.len(),
        emitted.len(),
        native_gt4.len(),
        split_patches.len(),
        added_gte.len(),
        remaining_max_q8.len(),
        remaining_p50_q8.len(),
        remaining_p95_q8.len(),
        remaining_p99_q8.len(),
        lowest_selected_priority.len(),
        highest_rejected_priority.len(),
    ];
    if rows.iter().any(|&len| len != rows[0]) {
        return Err(format!(
            "affine telemetry columns differ in length in {}",
            profile.display()
        )
        .into());
    }
    let mut csv = String::from(
        "guest_frame,input_max_error_texels,input_p50_error_texels,input_p95_error_texels,input_p99_error_texels,remaining_max_error_texels,remaining_p50_error_texels,remaining_p95_error_texels,remaining_p99_error_texels,split_candidates,extra_packets_requested,extra_packets_emitted,extra_packets_rejected,native_gt4_packets,split_patches,added_gte_transforms,lowest_selected_priority,highest_rejected_priority,projected_priority_inversion\n",
    );
    let texels = |value: &str| -> Result<String> {
        let q8 = value.parse::<u32>()?;
        Ok(format!("{:.3}", q8 as f64 / 256.0))
    };
    for i in 0..rows[0] {
        let lowest_selected = lowest_selected_priority[i].parse::<u32>()?;
        let highest_rejected = highest_rejected_priority[i].parse::<u32>()?;
        if lowest_selected != 0 && highest_rejected > lowest_selected {
            return Err(format!(
                "affine patch priority inversion at guest frame {}: rejected projected span {} exceeds selected span {}",
                frames[i], highest_rejected, lowest_selected
            )
            .into());
        }
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            frames[i],
            texels(&max_q8[i])?,
            texels(&p50_q8[i])?,
            texels(&p95_q8[i])?,
            texels(&p99_q8[i])?,
            texels(&remaining_max_q8[i])?,
            texels(&remaining_p50_q8[i])?,
            texels(&remaining_p95_q8[i])?,
            texels(&remaining_p99_q8[i])?,
            candidates[i],
            requested[i],
            emitted[i],
            requested[i]
                .parse::<u32>()?
                .saturating_sub(emitted[i].parse::<u32>()?),
            native_gt4[i],
            split_patches[i],
            added_gte[i],
            lowest_selected_priority[i],
            highest_rejected_priority[i],
            0,
        ));
    }
    fs::write(out, csv)?;
    Ok(())
}

fn ppm_token<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    loop {
        while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
            *cursor += 1;
        }
        if bytes.get(*cursor) != Some(&b'#') {
            break;
        }
        while *cursor < bytes.len() && bytes[*cursor] != b'\n' {
            *cursor += 1;
        }
    }
    let start = *cursor;
    while *cursor < bytes.len() && !bytes[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
    (start < *cursor).then_some(&bytes[start..*cursor])
}

fn ppm_rgb(path: &Path) -> Result<(usize, usize, Vec<u8>)> {
    let bytes = fs::read(path)?;
    let mut cursor = 0usize;
    if ppm_token(&bytes, &mut cursor) != Some(b"P6") {
        return Err(format!("{} is not a binary PPM", path.display()).into());
    }
    let width = std::str::from_utf8(ppm_token(&bytes, &mut cursor).ok_or("PPM width missing")?)?
        .parse::<usize>()?;
    let height = std::str::from_utf8(ppm_token(&bytes, &mut cursor).ok_or("PPM height missing")?)?
        .parse::<usize>()?;
    if ppm_token(&bytes, &mut cursor) != Some(b"255") {
        return Err(format!("{} is not an 8-bit PPM", path.display()).into());
    }
    if bytes.get(cursor) == Some(&b'\r') {
        cursor += 1;
        if bytes.get(cursor) == Some(&b'\n') {
            cursor += 1;
        }
    } else if bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    let length = width.saturating_mul(height).saturating_mul(3);
    if bytes.len().saturating_sub(cursor) < length {
        return Err(format!("{} has a truncated pixel payload", path.display()).into());
    }
    Ok((width, height, bytes[cursor..cursor + length].to_vec()))
}

fn ppm_changed_pixels(a: &Path, b: &Path, rect: (usize, usize, usize, usize)) -> Result<usize> {
    let (aw, ah, ap) = ppm_rgb(a)?;
    let (bw, bh, bp) = ppm_rgb(b)?;
    if (aw, ah) != (bw, bh) {
        return Err("flashlight proof images have different dimensions".into());
    }
    let (x0, y0, x1, y1) = rect;
    let mut changed = 0usize;
    for y in y0.min(ah)..y1.min(ah) {
        for x in x0.min(aw)..x1.min(aw) {
            let offset = (y * aw + x) * 3;
            changed += (ap[offset..offset + 3] != bp[offset..offset + 3]) as usize;
        }
    }
    Ok(changed)
}

fn percentile(sorted: &[u64], numerator: usize, denominator: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) * numerator).div_ceil(denominator);
    sorted[index.min(sorted.len() - 1)]
}

fn write_timing_summary(profile: &Path, output: &Path) -> Result<()> {
    let rendered = csv_column(profile, "render")?
        .into_iter()
        .map(|value| value.parse::<u64>().unwrap_or(0) != 0)
        .collect::<Vec<_>>();
    let mut seen_render = false;
    let steady = rendered
        .iter()
        .map(|&is_render| {
            if !is_render {
                false
            } else if !seen_render {
                seen_render = true;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
    if !seen_render || !steady.iter().any(|&keep| keep) {
        return Err(format!("{} has no steady rendered frames", profile.display()).into());
    }

    let mut report = String::from(
        "stage,p50_cycles,p95_cycles,max_cycles,p50_ms,p95_ms,p50_budget_percent,p50_equivalent_fps\n",
    );
    for stage in TIMING_STAGES {
        let mut values = csv_column(profile, stage)?
            .into_iter()
            .zip(steady.iter().copied())
            .filter_map(|(value, keep)| keep.then(|| value.parse::<u64>().ok()).flatten())
            .collect::<Vec<_>>();
        values.sort_unstable();
        let p50 = percentile(&values, 50, 100);
        let p95 = percentile(&values, 95, 100);
        let max = values.last().copied().unwrap_or(0);
        let p50_ms = p50 as f64 * 1000.0 / PSX_CPU_HZ;
        let p95_ms = p95 as f64 * 1000.0 / PSX_CPU_HZ;
        let budget = p50 as f64 * 100.0 / TARGET_RENDER_CYCLES as f64;
        let fps = if p50 == 0 {
            0.0
        } else {
            PSX_CPU_HZ / p50 as f64
        };
        report.push_str(&format!(
            "{stage},{p50},{p95},{max},{p50_ms:.3},{p95_ms:.3},{budget:.2},{fps:.3}\n"
        ));
    }
    fs::write(output, report)?;
    Ok(())
}

/// Render costs for frames whose camera changed since the preceding fixed
/// update. This excludes exact-view packet-cache hits and makes the c1a0 walk
/// result impossible to improve by merely extending its stationary tail.
fn moving_render_cycles(profile: &Path) -> Result<Vec<u64>> {
    let render = csv_column(profile, "render")?;
    let x = csv_column(profile, "camera_x_biased")?;
    let y = csv_column(profile, "camera_y_biased")?;
    let z = csv_column(profile, "camera_z_biased")?;
    let yaw = csv_column(profile, "player_view_yaw_q12")?;
    let count = render
        .len()
        .min(x.len())
        .min(y.len())
        .min(z.len())
        .min(yaw.len());
    let mut values = Vec::new();
    let mut previous: Option<(&str, &str, &str, &str)> = None;
    for i in 0..count {
        let camera = (x[i].as_str(), y[i].as_str(), z[i].as_str(), yaw[i].as_str());
        if previous.is_some_and(|old| old != camera) {
            let cycles = render[i].parse::<u64>().unwrap_or(0);
            if cycles != 0 {
                values.push(cycles);
            }
        }
        previous = Some(camera);
    }
    values.sort_unstable();
    Ok(values)
}

fn launch(
    frontend: &Path,
    cue: &Path,
    scenario: &Scenario,
    out: &Path,
    full_capture: bool,
    affine_heatmap: bool,
) -> Result<()> {
    fs::create_dir_all(out)?;
    let selector = if scenario.name == "hazard-ladder-mount" {
        13
    } else if matches!(
        scenario.name,
        "hazard-valve-link" | "hazard-valve-neutral-proof"
    ) {
        12
    } else if matches!(
        scenario.name,
        "hazard-pushable-pull" | "hazard-pushable-no-use-proof"
    ) {
        11
    } else if scenario.name == "hazard-entry-edge" {
        10
    } else if matches!(
        scenario.name,
        "hazard-panel-lights" | "hazard-panel-lights-off-proof"
    ) {
        9
    } else if scenario.name == "hazard-elevator-motion" {
        8
    } else if matches!(
        scenario.name,
        "hazard-guntarget-visibility" | "hazard-scientist-depth"
    ) {
        7
    } else if matches!(
        scenario.name,
        "anomalous-hallway"
            | "hazard-railing-depth"
            | "anomalous-cover"
            | "endgame-tram-door"
            | "hazard-floor-grid"
            | "hazard-water-exit"
    ) {
        14
    } else if scenario.subject_viewpoint {
        15
    } else {
        scenario.weapon_selector
    };
    let boot_mask = L1
        | scenario.map_index as u16
        | ((selector as u16) << 12)
        | if affine_heatmap || scenario.name == "anomalous-walk" {
            R1
        } else {
            0
        };
    let pulses = format!("0x{boot_mask:04x}@0+400{}", scenario.extra_pulses);
    let fixed_guest_stop = (scenario.name == "anomalous-door-motion").then_some(20u32);
    let mut command = Command::new(frontend);
    command
        .args(["launch", "--path"])
        .arg(cue)
        // Regression results must not depend on the interactive frontend's
        // persisted fast-boot toggle.  The editor playtest path is explicit,
        // deterministic, keeps the authored disc mounted for normal CD reads,
        // and starts this instrumented homebrew through PSoXide's HLE BIOS.
        .arg("--embedded-playtest")
        // PSoXide emits the fixed-update guest marker before the render task.
        // Stopping on that marker can therefore produce a deterministic black
        // image with zero completed renders.  Visual-frame telemetry is the
        // actual contract for a screenshot/performance regression.
        .args(["--steps", "1000000000", "--guest-visual-frames"])
        .arg(scenario.frames.to_string())
        // A healthy 20 Hz build emits roughly one visual per fixed update.
        // Bound a broken catch-up/render loop so it fails in minutes rather
        // than consuming the full billion-instruction safety cap.
        .arg("--guest-frames")
        .arg(
            fixed_guest_stop
                .unwrap_or_else(|| scenario.frames.saturating_mul(5))
                .to_string(),
        )
        .args(["--visual-hash-log"])
        .arg(out.join("visual-hashes.csv"))
        .args([
            "--visual-hash-interval",
            if fixed_guest_stop.is_some() {
                "1"
            } else {
                "30"
            },
        ]);
    command.args(["--pad-pulses", &pulses]);
    if full_capture {
        command
            .arg("--profile-log")
            .arg(out.join("profile.csv"))
            .arg("--counter-log")
            .arg(out.join("counters.csv"))
            .arg("--dump-display")
            .arg(out.join("display.ppm"));
        if scenario.name == "anomalous-walk" {
            // Keep the benchmark's GTE opcode/load summary beside its guest
            // stage counters. This catches an apparent CPU win that merely
            // oversubscribes COP2 and would stall differently on real silicon.
            command.arg("--dump-guest-profile");
        }
    }
    if std::env::var_os("HLPSX_REGRESSION_WIREFRAME").is_some() {
        command
            .arg("--wireframe")
            .arg("--dump-hw")
            .arg(out.join("wireframe.ppm"));
    }
    run(
        &mut command,
        &format!("PSoXide regression {} ({})", scenario.name, scenario.map),
    )
}

pub fn run_matrix(
    repository: &Path,
    psoxide: &Path,
    cue: &Path,
    only: Option<&str>,
) -> Result<PathBuf> {
    let frontend = frontend(psoxide)?;
    let fire_matrix = only == Some("fire-matrix");
    let tessellation_matrix = only == Some("tessellation-matrix");
    let root = if let Some(name) = only {
        repository.join(".hlpsx/regression-target").join(name)
    } else {
        repository.join(".hlpsx/regression")
    };
    if root.is_dir() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    let mut report = String::from(
        "scenario,map,visual_frames,deterministic,render_p50,render_p95,render_max,frames_at_20fps,total_profiled_frames,moving_p50,moving_p95,moving_max,moving_frames_at_20fps,total_moving_frames\n",
    );
    let mut matched = false;
    let mut performance_failures = Vec::new();

    for scenario in SCENARIOS
        .iter()
        .chain(WEAPON_SCENARIOS.iter())
        .chain(HUD_SCENARIOS.iter())
        .chain(FIRE_SCENARIOS.iter())
    {
        // The three Anomalous Materials cameras are opt-in visual-comparison
        // probes. Keep the already-large release matrix bounded, but retain
        // the ordinary deterministic and performance checks when one is
        // explicitly requested with `--scenario anomalous-*`.
        if only.is_none() && scenario.name.starts_with("anomalous-") {
            continue;
        }
        if only.is_some_and(|name| {
            if fire_matrix {
                !scenario.name.starts_with("fire-")
            } else if tessellation_matrix {
                scenario.name != "world-subdivision" && !scenario.name.starts_with("anomalous-")
            } else {
                name != scenario.name
            }
        }) {
            continue;
        }
        matched = true;
        let scenario_dir = if only.is_none() || fire_matrix || tessellation_matrix {
            root.join(scenario.name)
        } else {
            root.clone()
        };
        let first = scenario_dir.join("run-a");
        let second = scenario_dir.join("run-b");
        launch(&frontend, cue, scenario, &first, true, false)?;
        launch(&frontend, cue, scenario, &second, false, false)?;

        let hashes_a = csv_column(&first.join("visual-hashes.csv"), "display_hash")?;
        let hashes_b = csv_column(&second.join("visual-hashes.csv"), "display_hash")?;
        if hashes_a.is_empty() || hashes_b.is_empty() {
            return Err(format!(
                "{} emitted no visual checkpoints (game did not reach the requested rendered frame)",
                scenario.name
            )
            .into());
        }
        if hashes_a != hashes_b {
            return Err(
                format!("{} produced nondeterministic display hashes", scenario.name).into(),
            );
        }
        let mut affine_profile = first.join("profile.csv");
        let capture_affine = !matches!(
            scenario.name,
            "hazard-ladder-mount" | "hazard-valve-link" | "hazard-pushable-pull"
        ) && (tessellation_matrix || only == Some(scenario.name))
            && (scenario.subject_viewpoint || scenario.weapon_selector == 0);
        if capture_affine {
            let heatmap = scenario_dir.join("affine-heatmap");
            launch(&frontend, cue, scenario, &heatmap, true, true)?;
            fs::copy(
                heatmap.join("display.ppm"),
                scenario_dir.join("affine-error-heatmap.ppm"),
            )?;
            // Exhaustive rational error histograms are deliberately confined
            // to the offline heatmap launch. Normal performance captures run
            // only the bounded exact split test, so profiler instrumentation
            // cannot become the renderer's dominant moving-view cost.
            affine_profile = heatmap.join("profile.csv");
        }
        write_affine_error_summary(&affine_profile, &scenario_dir.join("affine-error.csv"))?;
        if scenario.name == "hud-flashlight" {
            // The old pulse landed during c1a2 streaming, so the supposed on
            // capture was byte-identical to hud-glock. Prove the world beam,
            // not merely the HUD icon, against an otherwise identical off run.
            let off_scenario = Scenario {
                name: "hud-flashlight-off-proof",
                map: scenario.map,
                map_index: scenario.map_index,
                frames: scenario.frames,
                weapon_selector: scenario.weapon_selector,
                subject_viewpoint: scenario.subject_viewpoint,
                extra_pulses: "",
            };
            let off = scenario_dir.join("off-proof");
            launch(&frontend, cue, &off_scenario, &off, true, false)?;
            let changed = ppm_changed_pixels(
                &first.join("display.ppm"),
                &off.join("display.ppm"),
                (112, 72, 208, 168),
            )?;
            if changed < 256 {
                return Err(format!(
                    "flashlight proof changed only {changed} centre-beam pixels (toggle or projected light is inactive)"
                )
                .into());
            }
        }
        if scenario.name == "hazard-valve-link" {
            // Compare the held-Use terminal frame against the exact same
            // player-authored camera with no input. This proves the ray hit
            // the real wheel and advanced the visible mechanism; a test that
            // merely boots beside it would also pass when linkage is broken.
            let neutral_scenario = Scenario {
                name: "hazard-valve-neutral-proof",
                map: scenario.map,
                map_index: scenario.map_index,
                frames: scenario.frames,
                weapon_selector: scenario.weapon_selector,
                subject_viewpoint: scenario.subject_viewpoint,
                extra_pulses: "",
            };
            let neutral = scenario_dir.join("neutral-proof");
            launch(&frontend, cue, &neutral_scenario, &neutral, true, false)?;
            let changed = ppm_changed_pixels(
                &first.join("display.ppm"),
                &neutral.join("display.ppm"),
                // Tight valve/shaft crop: exclude the weapon and wandering
                // hologram so only the linked brush mechanism can satisfy it.
                (110, 70, 185, 150),
            )?;
            if changed < 128 {
                return Err(format!(
                    "Hazard Course valve proof changed only {changed} mechanism pixels (the aimed wheel did not drive its linkage)"
                )
                .into());
            }
        }
        if scenario.name == "hazard-panel-lights" {
            let off_scenario = Scenario {
                name: "hazard-panel-lights-off-proof",
                map: scenario.map,
                map_index: scenario.map_index,
                frames: scenario.frames,
                weapon_selector: scenario.weapon_selector,
                subject_viewpoint: scenario.subject_viewpoint,
                extra_pulses: "",
            };
            let off = scenario_dir.join("off-proof");
            launch(&frontend, cue, &off_scenario, &off, true, false)?;
            let changed = ppm_changed_pixels(
                &first.join("display.ppm"),
                &off.join("display.ppm"),
                // The three complete boards, excluding most surrounding room
                // lighting and the weapon. A few bright strip pixels must
                // never be enough to pass this proof again.
                (45, 85, 275, 160),
            )?;
            if changed < 5_000 {
                return Err(format!(
                    "Hazard Course panel proof changed only {changed} board pixels (the complete dynamic-lit faces did not illuminate)"
                )
                .into());
            }
        }
        if scenario.name == "hazard-pushable-pull" {
            // Keep the retreat route identical but omit Use. Both cameras must
            // finish together; only the continuously selected box should
            // differ in the central world crop.
            let no_use_scenario = Scenario {
                name: "hazard-pushable-no-use-proof",
                map: scenario.map,
                map_index: scenario.map_index,
                frames: scenario.frames,
                weapon_selector: scenario.weapon_selector,
                subject_viewpoint: scenario.subject_viewpoint,
                extra_pulses: ",0x0040@400+10000",
            };
            let no_use = scenario_dir.join("no-use-proof");
            launch(&frontend, cue, &no_use_scenario, &no_use, true, false)?;
            let pulled_camera = (
                csv_column(&first.join("counters.csv"), "cam_x_biased")?
                    .last()
                    .cloned(),
                csv_column(&first.join("counters.csv"), "cam_y_biased")?
                    .last()
                    .cloned(),
                csv_column(&first.join("counters.csv"), "cam_z_biased")?
                    .last()
                    .cloned(),
            );
            let control_camera = (
                csv_column(&no_use.join("counters.csv"), "cam_x_biased")?
                    .last()
                    .cloned(),
                csv_column(&no_use.join("counters.csv"), "cam_y_biased")?
                    .last()
                    .cloned(),
                csv_column(&no_use.join("counters.csv"), "cam_z_biased")?
                    .last()
                    .cloned(),
            );
            if pulled_camera != control_camera {
                return Err(format!(
                    "pushable proof camera diverged from its no-use control: pull={pulled_camera:?}, control={control_camera:?}"
                )
                .into());
            }
            let changed = ppm_changed_pixels(
                &first.join("display.ppm"),
                &no_use.join("display.ppm"),
                (80, 40, 240, 210),
            )?;
            if changed < 256 {
                return Err(format!(
                    "Hazard Course pushable proof changed only {changed} world pixels (held Use did not pull the box)"
                )
                .into());
            }
        }
        let current_room = csv_column(&first.join("counters.csv"), "current_room")?
            .last()
            .ok_or_else(|| format!("{} emitted no room counter", scenario.name))?
            .parse::<u8>()?;
        if current_room != scenario.map_index {
            return Err(format!(
                "{} booted room {current_room}, expected {} ({})",
                scenario.name, scenario.map_index, scenario.map
            )
            .into());
        }
        let overflows = csv_column(
            &first.join("profile.csv"),
            "room_submit_primitive_overflows",
        )?;
        if overflows.iter().any(|value| value != "0") {
            return Err(format!(
                "{} exhausted the world primitive arena during its proof route",
                scenario.name
            )
            .into());
        }
        let timing_path = scenario_dir.join("frame-time.csv");
        write_timing_summary(&first.join("profile.csv"), &timing_path)?;
        if scenario.name == "anomalous-motion" {
            let rooms = csv_column(&first.join("profile.csv"), "current_room")?;
            let yaw = csv_column(&first.join("profile.csv"), "player_view_yaw_q12")?;
            let route_yaw = rooms
                .iter()
                .zip(yaw.iter())
                .filter_map(|(room, yaw)| {
                    (room.parse::<u8>().ok() == Some(scenario.map_index))
                        .then(|| yaw.parse::<u16>().ok())
                        .flatten()
                })
                .collect::<Vec<_>>();
            let mut unique = route_yaw.clone();
            unique.sort_unstable();
            unique.dedup();
            if unique.len() < 32 {
                return Err(format!(
                    "anomalous-motion changed through only {} camera angles (sweep input did not engage)",
                    unique.len()
                )
                .into());
            }
        }
        if scenario.name == "anomalous-walk" {
            let camera_x = csv_column(&first.join("profile.csv"), "camera_x_biased")?;
            let camera_z = csv_column(&first.join("profile.csv"), "camera_z_biased")?;
            let yaw = csv_column(&first.join("profile.csv"), "player_view_yaw_q12")?;
            let equipment = csv_column(&first.join("profile.csv"), "equipment")?;
            let positions = camera_x
                .iter()
                .zip(camera_z.iter())
                .filter_map(|(x, z)| Some((x.parse::<i32>().ok()?, z.parse::<i32>().ok()?)))
                .collect::<Vec<_>>();
            let first = positions
                .first()
                .copied()
                .ok_or("anomalous-walk emitted no camera positions")?;
            let displacement = positions
                .iter()
                .map(|&(x, z)| (x - first.0).abs().max((z - first.1).abs()))
                .max()
                .unwrap_or(0);
            let mut unique_yaw = yaw
                .iter()
                .filter_map(|value| value.parse::<u16>().ok())
                .collect::<Vec<_>>();
            unique_yaw.sort_unstable();
            unique_yaw.dedup();
            let weapon_frames = equipment
                .iter()
                .filter(|value| value.parse::<u64>().unwrap_or(0) != 0)
                .count();
            if displacement < 128 || unique_yaw.len() < 32 || weapon_frames < 120 {
                return Err(format!(
                    "anomalous-walk did not exercise real weapon-on traversal: displacement={displacement}, yaw_angles={}, weapon_frames={weapon_frames}",
                    unique_yaw.len()
                )
                .into());
            }
        }
        if scenario.name == "water-surface" {
            let rooms = csv_column(&first.join("counters.csv"), "current_room")?;
            let camera_y = csv_column(&first.join("counters.csv"), "cam_y_biased")?;
            let samples = rooms
                .iter()
                .zip(camera_y.iter())
                .filter_map(|(room, y)| {
                    (room.parse::<u8>().ok() == Some(scenario.map_index))
                        .then(|| y.parse::<i32>().ok().map(|value| value - 32768))
                        .flatten()
                })
                .collect::<Vec<_>>();
            let first_y = samples
                .first()
                .copied()
                .ok_or("water tape has no camera samples")?;
            let last_y = samples.last().copied().unwrap_or(first_y);
            let min_y = samples.iter().copied().min().unwrap_or(first_y);
            if first_y <= -1280 || !(-1380..=-1300).contains(&last_y) || min_y < -1380 {
                return Err(format!(
                    "water tape did not enter and remain in the c2a5 pool: first={first_y}, last={last_y}, min={min_y}"
                )
                .into());
            }
        }
        if scenario.name == "hazard-water-exit" {
            let rooms = csv_column(&first.join("counters.csv"), "current_room")?;
            let camera_y = csv_column(&first.join("counters.csv"), "cam_y_biased")?;
            let samples = rooms
                .iter()
                .zip(camera_y.iter())
                .filter_map(|(room, y)| {
                    (room.parse::<u8>().ok() == Some(scenario.map_index))
                        .then(|| y.parse::<i32>().ok().map(|value| value - 32768))
                        .flatten()
                })
                .collect::<Vec<_>>();
            let first_y = samples
                .first()
                .copied()
                .ok_or("hazard water-exit tape has no camera samples")?;
            let max_y = samples.iter().copied().max().unwrap_or(first_y);
            if first_y > -300 || max_y < -80 {
                return Err(format!(
                    "second Hazard Course water exit did not clear its lip: first={first_y}, max={max_y}"
                )
                .into());
            }
        }
        if scenario.name == "hazard-ladder-mount" {
            let rooms = csv_column(&first.join("counters.csv"), "current_room")?;
            let camera_x = csv_column(&first.join("counters.csv"), "cam_x_biased")?;
            let camera_y = csv_column(&first.join("counters.csv"), "cam_y_biased")?;
            let camera_z = csv_column(&first.join("counters.csv"), "cam_z_biased")?;
            let samples = rooms
                .iter()
                .zip(camera_x.iter())
                .zip(camera_y.iter())
                .zip(camera_z.iter())
                .filter_map(|(((room, x), y), z)| {
                    (room.parse::<u8>().ok() == Some(scenario.map_index))
                        .then(|| {
                            Some((
                                x.parse::<i32>().ok()? - 32768,
                                y.parse::<i32>().ok()? - 32768,
                                z.parse::<i32>().ok()? - 32768,
                            ))
                        })
                        .flatten()
                })
                .collect::<Vec<_>>();
            let (first_x, first_y, first_z) = samples
                .first()
                .copied()
                .ok_or("hazard ladder tape has no camera samples")?;
            let max_y = samples
                .iter()
                .map(|sample| sample.1)
                .max()
                .unwrap_or(first_y);
            // The pulse lands during the climb, well below the upper plane.
            // It must move enough to prove input was sampled, but not eject
            // the player. GoldSrc never recentres at top-out, so the final
            // tangent must preserve that deliberate displacement rather than
            // adding a second sideways throw.
            let climb_lateral = samples
                .iter()
                .filter(|sample| sample.1 < max_y - 20)
                .map(|sample| (sample.0 - first_x).abs())
                .max()
                .unwrap_or(0);
            let (last_x, _, last_z) = samples
                .last()
                .copied()
                .unwrap_or((first_x, first_y, first_z));
            let final_lateral = (last_x - first_x).abs();
            let min_normal = samples
                .iter()
                .map(|sample| sample.2)
                .min()
                .unwrap_or(first_z);
            let max_normal = samples
                .iter()
                .map(|sample| sample.2)
                .max()
                .unwrap_or(first_z);
            let forward_topout = last_z - first_z;
            // The same diagnostic pose was run in the original game through
            // the instrumented PM_LadderMove path. GoldSrc stays at Y=764 for
            // every climb tick, then ordinary walking settles at Y=767.96875;
            // it never snaps away from the face or mirrors through it. Runtime
            // PSX Z is GoldSrc Y, rounded to whole units.
            if max_y - first_y < 96
                || !(4..=28).contains(&climb_lateral)
                || (final_lateral - climb_lateral).abs() > 1
                || first_z != 764
                || min_normal < 764
                || max_normal > 768
                || !(0..=4).contains(&forward_topout)
            {
                return Err(format!(
                    "hazard ladder assist failed: rise={}, climb_lateral={}, final_lateral={}, normal={}..{}->{}, forward_topout={}",
                    max_y - first_y,
                    climb_lateral,
                    final_lateral,
                    first_z,
                    max_normal,
                    last_z,
                    forward_topout,
                )
                .into());
            }
        }

        let mut render = csv_column(&first.join("profile.csv"), "render")?
            .into_iter()
            .filter_map(|value| value.parse::<u64>().ok())
            .filter(|&cycles| cycles != 0)
            .collect::<Vec<_>>();
        if render.is_empty() {
            return Err(format!(
                "{} completed no visual renders (refusing a black/zero-work pass)",
                scenario.name
            )
            .into());
        }
        // The first completed render owns cold PVS/texture work. Keep it in the
        // max artifact, but steady-state percentiles start after that one frame.
        let max = render.iter().copied().max().unwrap_or(0);
        if render.len() > 1 {
            render.remove(0);
        }
        render.sort_unstable();
        let p50 = percentile(&render, 50, 100);
        let p95 = percentile(&render, 95, 100);
        let at_target = render
            .iter()
            .filter(|&&cycles| cycles <= TARGET_RENDER_CYCLES)
            .count();
        let moving = moving_render_cycles(&first.join("profile.csv"))?;
        let moving_p50 = percentile(&moving, 50, 100);
        let moving_p95 = percentile(&moving, 95, 100);
        let moving_max = moving.last().copied().unwrap_or(0);
        let moving_at_target = moving
            .iter()
            .filter(|&&cycles| cycles <= TARGET_RENDER_CYCLES)
            .count();
        // Subject-viewpoint routes are fixed visual/topology probes: they keep
        // deterministic pixels, route validation, affine-error distributions,
        // and packet-overflow checks. Their deliberately pathological static
        // cameras are quality fixtures, not timing gates; world-subdivision and
        // the weapon-on anomalous-walk route retain the real performance gates.
        if p50 > TARGET_RENDER_CYCLES
            && scenario.name != "anomalous-door-motion"
            && !scenario.subject_viewpoint
        {
            performance_failures.push(format!(
                "{} p50 {p50} exceeds the 20 fps budget {TARGET_RENDER_CYCLES}",
                scenario.name
            ));
        }
        report.push_str(&format!(
            "{},{},{},yes,{p50},{p95},{max},{at_target},{},{moving_p50},{moving_p95},{moving_max},{moving_at_target},{}\n",
            scenario.name,
            scenario.map,
            scenario.frames,
            render.len(),
            moving.len()
        ));
        println!(
            "  {:<22} {:<6} deterministic | render p50={p50} p95={p95} max={max} | 20fps {at_target}/{}",
            scenario.name,
            scenario.map,
            render.len()
        );
        if scenario.name == "anomalous-walk" {
            println!(
                "    moving camera         | render p50={moving_p50} p95={moving_p95} max={moving_max} | 20fps {moving_at_target}/{}",
                moving.len()
            );
        }
        println!("    frame-time breakdown -> {}", timing_path.display());
    }

    if !matched {
        return Err(format!("unknown regression scenario `{}`", only.unwrap_or_default()).into());
    }

    // With matching camera, duration, map, and weapon, these captures must
    // differ: otherwise the reload route never reached an authored pose. Do
    // this only for the complete matrix; either scenario remains independently
    // rerunnable while diagnosing it via `regress --scenario NAME`.
    if only.is_none() {
        let reload = fs::read(root.join("shotgun-reload/run-a/display.ppm"))?;
        let idle = fs::read(root.join("weapon-shotgun/run-a/display.ppm"))?;
        if reload == idle {
            return Err("shotgun reload proof frame is identical to idle".into());
        }
    }
    if only.is_none() || fire_matrix {
        for &(primary, secondary) in &MODE_PAIRS {
            let a = fs::read(root.join(primary).join("run-a/visual-hashes.csv"))?;
            let b = fs::read(root.join(secondary).join("run-a/visual-hashes.csv"))?;
            if a == b {
                return Err(format!(
                    "{secondary} is visually identical to {primary}; secondary mode did not engage"
                )
                .into());
            }
        }
    }

    let report_path = root.join("report.csv");
    fs::write(&report_path, report)?;
    println!("PSoXide regression report -> {}", report_path.display());
    if !performance_failures.is_empty() {
        return Err(performance_failures.join("; ").into());
    }
    Ok(report_path)
}

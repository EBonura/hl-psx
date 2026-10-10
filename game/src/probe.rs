//! Scratch-only interaction probe. Replays a build-time script of teleports,
//! target fires and input overrides so every scripted interaction of a map can
//! be exercised headlessly and traced. Never part of a shipping build.

use crate::semantic_input;

pub struct Cmd {
    pub sec: u8,
    pub op: u8,
    pub tick: u16,
    pub a: [i16; 6],
    pub name: u16,
}

include!(concat!(env!("OUT_DIR"), "/probe_script.rs"));

static mut NEXT: usize = 0;
static mut PIN: bool = false;
static mut PIN_UNTIL: u32 = 0;
static mut PIN_POSE: ([i32; 3], u16, i16) = ([0; 3], 0, 0);
static mut GOD: i32 = 0;
static mut FACE: u16 = 0;
static mut TRACE_EVERY: u32 = 20;
static mut IN_UNTIL: u32 = 0;
static mut IN_SAMPLE: semantic_input::Sample = semantic_input::Sample {
    forward: 0,
    strafe: 0,
    turn: 0,
    look: 0,
    actions: 0,
};
static mut LAST_MAP: &str = "";
static mut SEC: usize = 0;
static mut HOLD_CL: bool = false;
static mut ENDED: bool = false;
static mut GOTO: Option<(usize, bool)> = None;
static mut FORCE_NEXT: bool = false;

pub unsafe fn take_goto() -> Option<(usize, bool)> {
    let g = GOTO;
    GOTO = None;
    if g.is_some() {
        FORCE_NEXT = true;
    }
    g
}

/// Changelevel touches wait for the section's first teleport (arrival pose may sit in a back trigger).
pub fn hold_changelevel() -> bool {
    unsafe { HOLD_CL }
}

pub fn chapter() -> bool {
    PROBE_CHAPTER
}

pub fn trace_due(map_tick: u32) -> bool {
    unsafe { map_tick % TRACE_EVERY.max(1) == 0 }
}

pub fn god() -> bool {
    unsafe { GOD != 0 }
}

pub unsafe fn name_id(m: &crate::map::Map, name: &str) -> u16 {
    let mut id = 1usize;
    while id <= m.n_logic_names {
        if m.logic_name(id as u16) == name {
            return id as u16;
        }
        id += 1;
    }
    0
}

/// Runs before the tick consumes `sample`. Returns the sample to use.
#[inline(never)]
pub unsafe fn step(
    m: &crate::map::Map,
    map_name: &'static str,
    nlogic: usize,
    nents: usize,
    now: u32,
    sample: semantic_input::Sample,
    player: &mut crate::phys::Player,
    yaw: &mut u16,
    pitch: &mut i16,
    health: &mut u16,
    weapons: &mut crate::Arsenal,
) -> semantic_input::Sample {
    // Commands belong to the probed map only; a changelevel ends the script.
    if ENDED {
        return sample;
    }
    if LAST_MAP != map_name || FORCE_NEXT {
        FORCE_NEXT = false;
        if !LAST_MAP.is_empty() {
            ENDED = true;
            LAST_MAP = map_name;
            crate::reference_trace::probe(now, "ended", map_name, SEC as i32);
            return sample;
        }
        // The first map of a run picks its own section, so one baked script
        // serves a separate launch per map.
        SEC = 0;
        while SEC < PROBE_SECS.len() && PROBE_SECS[SEC] != map_name {
            SEC += 1;
        }
        if SEC == PROBE_SECS.len() {
            SEC = 0;
        }
        crate::reference_trace::probe(now, "sec", map_name, SEC as i32);
        NEXT = 0;
        while NEXT < PROBE_CMDS.len() && (PROBE_CMDS[NEXT].sec as usize) < SEC {
            NEXT += 1;
        }
        LAST_MAP = map_name;
        // A debug spawn can sit inside a changelevel volume; touch-fired level
        // changes stay off for the whole script (use-fired ones still run).
        HOLD_CL = true;
    }
    while NEXT < PROBE_CMDS.len()
        && PROBE_CMDS[NEXT].sec as usize == SEC
        && PROBE_CMDS[NEXT].tick as u32 <= now
    {
        let c = &PROBE_CMDS[NEXT];
        let cname = PROBE_NAMES[c.name as usize];
        NEXT += 1;
        match c.op {
            1 => {
                player.pos = [c.a[0] as i32, c.a[1] as i32, c.a[2] as i32];
                crate::reference_trace::probe_vals(
                    now,
                    "tp",
                    &[c.a[0] as i32, c.a[1] as i32, c.a[2] as i32],
                );
                if c.a[5] > 0 {
                    PIN = true;
                    PIN_UNTIL = now + c.a[5] as u32;
                }
                player.clear_velocity();
                player.on_ground = false;
                player.ground_mover = -1;
                *yaw = c.a[3] as u16;
                *pitch = c.a[4] as i16;
                PIN_POSE = (player.pos, *yaw, *pitch);
            }
            2 => {
                let id = name_id(m, cname);
                crate::reference_trace::probe(now, "fire", cname, id as i32);
                crate::logic_fire_targets(
                    m,
                    nlogic,
                    nents,
                    id,
                    c.a[0] as u8,
                    now as u16,
                    0,
                    crate::logic_state::CALLER_NONE,
                );
            }
            3 => {
                IN_SAMPLE = semantic_input::Sample {
                    forward: c.a[0] as i8,
                    strafe: c.a[1] as i8,
                    turn: c.a[2] as i8,
                    look: c.a[3] as i8,
                    actions: c.a[4] as u16,
                };
                IN_UNTIL = now + c.a[5].max(1) as u32;
            }
            4 => PIN = c.a[0] != 0,
            5 => GOD = c.a[0] as i32,
            6 => TRACE_EVERY = c.a[0].max(1) as u32,
            7 => {
                *yaw = c.a[0] as u16;
                *pitch = c.a[1] as i16;
                PIN_POSE.1 = *yaw;
                PIN_POSE.2 = *pitch;
            }
            8 => crate::reference_trace::probe(now, "mark", cname, 0),
            9 => weapons.give_all_debug(),
            10 => {
                let mut ei = 0usize;
                while ei < nents {
                    if m.entity(ei).submodel as i32 == c.a[0] as i32 {
                        let li = crate::ENT_BRUSH_LOGIC[ei];
                        crate::reference_trace::probe(now, "use", "", li as i32);
                        if (li as usize) < nlogic {
                            crate::LOGIC_ACTIVATOR = 1;
                            crate::logic_use_entity(
                                m,
                                nlogic,
                                nents,
                                li as usize,
                                crate::map::USE_TOGGLE,
                                now as u16,
                                0,
                                crate::logic_state::CALLER_NONE,
                            );
                            crate::LOGIC_ACTIVATOR = 0;
                        }
                    }
                    ei += 1;
                }
            }
            11 | 12 => {
                let id = if c.op == 11 { name_id(m, cname) } else { 0 };
                let mut pi = 0usize;
                while pi < crate::PROP_COUNT {
                    let hit = if c.op == 11 {
                        id != 0 && crate::PROP_NAME[pi] == id
                    } else {
                        crate::PROP_KIND[pi] as i32 == c.a[0] as i32
                    };
                    if hit && crate::PROP_ACTIVE[pi] != 0 && crate::PROP_HEALTH[pi] != 0 {
                        crate::reference_trace::probe(now, "hurt", cname, pi as i32);
                        let mut n = 0;
                        while n < 8 && crate::PROP_HEALTH[pi] != 0 {
                            crate::damage_prop(
                                pi,
                                if c.op == 12 {
                                    if c.a[1] > 0 {
                                        c.a[1].clamp(1, 255) as u8
                                    } else {
                                        255
                                    }
                                } else {
                                    c.a[0].clamp(1, 255) as u8
                                },
                                true,
                            );
                            if c.op == 11 && c.a[0] != 255 || c.op == 12 && c.a[1] > 0 {
                                break;
                            }
                            n += 1;
                        }
                    }
                    pi += 1;
                }
            }
            14 => {
                let mut i = 0usize;
                while i < crate::menu::MAPS.len() && crate::menu::MAPS[i] != cname {
                    i += 1;
                }
                crate::reference_trace::probe(now, "goto", cname, i as i32);
                if i < crate::menu::MAPS.len() {
                    GOTO = Some((i, c.a[0] != 0));
                }
            }
            15 => {
                GOD = 0;
                *health = 0;
                crate::reference_trace::probe(now, "die", "", 0);
            }
            17 => {
                let mut best = usize::MAX;
                let mut best_d = 160i32;
                let mut pi = 0usize;
                while pi < crate::PROP_COUNT {
                    if crate::PROP_ACTIVE[pi] != 0 && crate::PROP_HEALTH[pi] != 0 {
                        let t = crate::PROP_POS[pi];
                        let d = (t[0] - c.a[0] as i32).abs()
                            + (t[1] - c.a[1] as i32).abs()
                            + (t[2] - c.a[2] as i32).abs();
                        if d < best_d {
                            best_d = d;
                            best = pi;
                        }
                    }
                    pi += 1;
                }
                if best == usize::MAX {
                    crate::reference_trace::probe(now, "killat", "none", -1);
                } else {
                    crate::reference_trace::probe(now, "killat", "", best as i32);
                    let mut n = 0;
                    while n < 8 && crate::PROP_HEALTH[best] != 0 {
                        crate::damage_prop(best, 255, true);
                        n += 1;
                    }
                }
            }
            16 => FACE = if cname == "-" { 0 } else { name_id(m, cname) },
            13 => crate::reference_trace::probe_vals(
                now,
                "pos",
                &[player.pos[0], player.pos[1], player.pos[2], *health as i32],
            ),
            _ => {}
        }
    }
    if FACE != 0 {
        let mut pi = 0usize;
        while pi < crate::PROP_COUNT {
            if crate::PROP_ACTIVE[pi] != 0 && crate::PROP_NAME[pi] == FACE {
                let t = crate::PROP_POS[pi];
                let h = crate::model_def(crate::PROP_KIND[pi]).target_h / 2;
                let d = [
                    t[0] - player.pos[0],
                    t[1] + h - player.pos[1] - 28,
                    t[2] - player.pos[2],
                ];
                *yaw = crate::yaw_from_vec(d[0], d[2]);
                let horiz = psx_math::int32::isqrt_i32(d[0] * d[0] + d[2] * d[2]);
                let mut p = psx_math::atan2_q12(d[1], horiz) as i32;
                if p > 2048 {
                    p -= 4096;
                }
                *pitch = p as i16;
                break;
            }
            pi += 1;
        }
    }
    if PIN && PIN_UNTIL != 0 && now >= PIN_UNTIL {
        PIN = false;
        PIN_UNTIL = 0;
    }
    if PIN {
        player.pos = PIN_POSE.0;
        player.clear_velocity();
        if FACE == 0 {
            *yaw = PIN_POSE.1;
            *pitch = PIN_POSE.2;
        }
    }
    if GOD == 2 {
        if *health < 1000 {
            crate::reference_trace::probe(now, "god_heal", "", *health as i32);
        }
        *health = 1000;
    } else if GOD != 0 && *health < 50 {
        crate::reference_trace::probe(now, "god_heal", "", *health as i32);
        *health = 100;
    }
    if now < IN_UNTIL {
        IN_SAMPLE
    } else {
        sample
    }
}

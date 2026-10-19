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
static mut WATCH_EVERY: u32 = 10;
static mut PROJ_LOG: bool = false;
static mut WATCH_KINDS: [i32; 4] = [-1; 4];
static mut PROJ_WATCH: bool = false;
static mut HOLD_ROLL: (i32, u32) = (0, 0);

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
        if !LAST_MAP.is_empty() && !PROBE_CHAIN {
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
            if PROBE_CHAIN && !LAST_MAP.is_empty() {
                ENDED = true;
                LAST_MAP = map_name;
                return sample;
            }
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
    if now < HOLD_ROLL.1 {
        crate::PUNCH_ROLL = HOLD_ROLL.0;
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
                if c.a[5] < 0 {
                    HOLD_CL = false;
                }
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
            51 => {
                let level = crate::settings::skill() as i32;
                crate::reference_trace::probe_vals(now, "skill", &[level, c.a[0] as i32]);
                if c.a[0] >= 0 && c.a[0] as i32 != level {
                    crate::settings::set_skill(c.a[0].min(2) as u8);
                    let mut i = 0usize;
                    while i < crate::menu::MAPS.len() && crate::menu::MAPS[i] != map_name {
                        i += 1;
                    }
                    if i < crate::menu::MAPS.len() {
                        GOTO = Some((i, false));
                    }
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
            #[cfg(feature = "probe-fixes")]
            47 => crate::nihilanth::probe_head(),
            #[cfg(feature = "probe-fixes")]
            49 => {
                if c.a[0] >= 0 {
                    let mut pi = 0usize;
                    while pi < crate::PROP_COUNT {
                        if crate::PROP_ACTIVE[pi] != 0
                            && crate::PROP_HEALTH[pi] != 0
                            && crate::PROP_KIND[pi] as i32 == c.a[0] as i32
                        {
                            crate::TALKING_PROP = pi as u8;
                            crate::TALKING_VOICE = 0;
                            crate::TALKING_UNTIL = now as u16 + 400;
                            break;
                        }
                        pi += 1;
                    }
                }
                crate::reference_trace::probe(
                    now,
                    "talkwho",
                    "",
                    crate::TALKING_PROP as i32 * 1000
                        + (crate::TALKING_UNTIL.wrapping_sub(now as u16) as i32).min(999),
                );
            }
            #[cfg(feature = "probe-fixes")]
            48 => {
                let mut pi = 0usize;
                while pi < crate::PROP_COUNT {
                    if crate::PROP_ACTIVE[pi] != 0
                        && crate::PROP_HEALTH[pi] != 0
                        && crate::PROP_KIND[pi] as i32 == c.a[0] as i32
                    {
                        let at = [c.a[1] as i32, c.a[2] as i32, c.a[3] as i32];
                        crate::prop_set_pos_grounded(m, pi, crate::prop_grounded_pos(m, pi, at));
                        crate::reference_trace::probe(now, "montp", "", pi as i32);
                        break;
                    }
                    pi += 1;
                }
            }
            #[cfg(feature = "probe-fixes")]
            18 => *health = c.a[0].clamp(0, 1000) as u16,
            #[cfg(feature = "probe-fixes")]
            19 => {
                let mut pi = 0usize;
                while pi < crate::PROP_COUNT {
                    if crate::PROP_ACTIVE[pi] != 0
                        && crate::PROP_HEALTH[pi] != 0
                        && crate::PROP_KIND[pi] as i32 == c.a[0] as i32
                    {
                        crate::prop_scientist_set_flag(pi, crate::PROP_SCI_FOLLOWING, true);
                        crate::prop_scientist_set_flag(pi, crate::PROP_SCI_HELLO_SAID, true);
                        crate::reference_trace::probe(now, "follow", "", pi as i32);
                        break;
                    }
                    pi += 1;
                }
            }
            #[cfg(feature = "probe-fixes")]
            20 => {
                let dmg = c.a[3].clamp(1, 255) as u8;
                crate::reference_trace::probe(now, "explode", "", dmg as i32);
                crate::explode(
                    m,
                    [c.a[0] as i32, c.a[1] as i32, c.a[2] as i32],
                    dmg,
                    crate::combat_defs::blast_radius(dmg),
                    true,
                );
            }
            30 | 31 => {
                let mut n = c.a[0] as i32;
                let mut ei = 0usize;
                while ei < crate::ENT_SOLID_COUNT {
                    if crate::ENT_CACHE[ei].kind == crate::ENT_KIND_PUSHABLE {
                        if n == 0 {
                            if c.op == 30 {
                                let o = &mut crate::ENT_CACHE[ei].origin;
                                o[0] += c.a[1] as i32;
                                o[1] += c.a[2] as i32;
                                o[2] += c.a[3] as i32;
                            }
                            let o = crate::ENT_CACHE[ei].origin;
                            crate::reference_trace::probe_vals(
                                now,
                                "pushpos",
                                &[ei as i32, o[0], o[1], o[2]],
                            );
                            break;
                        }
                        n -= 1;
                    }
                    ei += 1;
                }
            }
            21 => {
                let dmg = c.a[0].clamp(1, 255) as u8;
                let body = [player.pos[0], player.pos[1] + 18, player.pos[2]];
                let mut done = false;
                for ring in [96i32, 128, 160] {
                    for k in 0..16u16 {
                        if done {
                            break;
                        }
                        let a = k * 256;
                        let src = [
                            player.pos[0] + ((psx_math::sincos::sin_q12(a) * ring) >> 12),
                            player.pos[1] + 18,
                            player.pos[2]
                                + ((psx_math::sincos::sin_q12((a + 1024) & 0xfff) * ring) >> 12),
                        ];
                        let leaf = crate::camera_leaf(m, src);
                        let movers = core::slice::from_raw_parts(
                            core::ptr::addr_of!(crate::MOVERS).cast::<crate::phys::Mover>(),
                            crate::MOVER_COUNT.min(crate::MAX_ENTS + 1),
                        );
                        let hidden =
                            crate::phys::trace_line(m, movers, [src[0], src[1] + 1, src[2]], body)
                                .is_some();
                        if leaf > 0 && hidden {
                            crate::reference_trace::probe_vals(
                                now,
                                "blastwall",
                                &[src[0], src[1], src[2], ring, dmg as i32, *health as i32],
                            );
                            crate::explode(
                                m,
                                src,
                                dmg,
                                crate::combat_defs::blast_radius(dmg),
                                true,
                            );
                            done = true;
                        }
                    }
                }
                if !done {
                    crate::reference_trace::probe(now, "blastwall", "no-hidden-spot", 0);
                }
            }
            40 => {
                let x = c.a[0] as i32;
                let z = c.a[2] as i32;
                let movers = core::slice::from_raw_parts(
                    core::ptr::addr_of!(crate::MOVERS).cast::<crate::phys::Mover>(),
                    crate::MOVER_COUNT.min(crate::MAX_ENTS + 1),
                );
                let mut y = c.a[1] as i32 + 200;
                while y >= c.a[1] as i32 - 400 {
                    let wet = crate::water_touch(m, m.n_ents, [x, y, z]);
                    let hit =
                        crate::phys::trace_line(m, movers, [x, y, z], [x, y - 8, z]).is_some();
                    crate::reference_trace::probe_vals(
                        now,
                        "scan",
                        &[
                            x,
                            y,
                            z,
                            wet as i32,
                            hit as i32,
                            crate::camera_leaf(m, [x, y, z]),
                        ],
                    );
                    y -= 8;
                }
            }
            23 => {
                let mut k = 0;
                while k < 3 && WATCH_KINDS[k] >= 0 {
                    k += 1;
                }
                WATCH_KINDS[k] = c.a[0] as i32;
                WATCH_EVERY = if c.a[1] > 0 { c.a[1] as u32 } else { 10 };
            }
            41 => PROJ_WATCH = c.a[0] != 0,
            46 => {
                PREF = [c.a[0] as i32, c.a[1] as i32, c.a[2] as i32];
                HAS_PREF = true;
            }
            45 | 47 => {
                // tpfree (47: tpfreeany, no changelevel touch needed, for a trigger_transition box): nearest world-free standing spot to the centre of a box (engine axes
                // x, up, y), kept within one hull half-width of the box so its touch fires.
                let lo = [c.a[0] as i32, c.a[1] as i32, c.a[2] as i32];
                let hi = [c.a[3] as i32, c.a[4] as i32, c.a[5] as i32];
                let (cx, cy, cz) = if HAS_PREF {
                    (
                        PREF[0].clamp(lo[0], hi[0]),
                        PREF[1].clamp(lo[1], hi[1]) - 20,
                        PREF[2].clamp(lo[2], hi[2]),
                    )
                } else {
                    (
                        (lo[0] + hi[0]) / 2,
                        (lo[1] + hi[1]) / 2 - 20,
                        (lo[2] + hi[2]) / 2,
                    )
                };
                HAS_PREF = false;
                let mut found = false;
                let mut crouched = false;
                let mut pass = 0usize;
                while pass < 4 && !found {
                    crouched = pass & 1 == 1;
                    let need_floor = pass < 2;
                    let mut zi = 0usize;
                    while zi < 5 && !found {
                        let dy = [0, 24, -24, 48, -48][zi];
                        let y = (cy + dy).clamp(lo[1] - 30, hi[1] + 30);
                        let mut r = 0i32;
                        while r <= 192 && !found {
                            let mut dx = -r;
                            while dx <= r && !found {
                                let mut dz = -r;
                                while dz <= r && !found {
                                    if dx.abs() == r || dz.abs() == r {
                                        let p = [cx + dx, y, cz + dz];
                                        if p[0] >= lo[0] - 14
                                            && p[0] <= hi[0] + 14
                                            && p[2] >= lo[2] - 14
                                            && p[2] <= hi[2] + 14
                                            && crate::phys::probe_pos_free(m, p, crouched)
                                            && (!need_floor
                                                || crate::phys::probe_pos_supported(m, p, crouched))
                                            && (c.op == 47
                                                || touches_changelevel(m, nlogic, p, crouched))
                                        {
                                            player.pos = p;
                                            found = true;
                                        }
                                    }
                                    dz += 8;
                                }
                                dx += 8;
                            }
                            r += 8;
                        }
                        zi += 1;
                    }
                    pass += 1;
                }
                let (mut n_free, mut n_touch) = (0i32, 0i32);
                if !found {
                    // Diagnose: grid points of the box that are hull-free, and that touch a changelevel.
                    let mut gx = lo[0] - 14;
                    while gx <= hi[0] + 14 {
                        let mut gz = lo[2] - 14;
                        while gz <= hi[2] + 14 {
                            let mut gi = 0usize;
                            while gi < 5 {
                                let gy = ((lo[1] + hi[1]) / 2 - 20 + [0, 24, -24, 48, -48][gi])
                                    .clamp(lo[1] - 30, hi[1] + 30);
                                let gp = [gx, gy, gz];
                                if crate::phys::probe_pos_free(m, gp, false) {
                                    n_free += 1;
                                }
                                if touches_changelevel(m, nlogic, gp, false) {
                                    n_touch += 1;
                                }
                                gi += 1;
                            }
                            gz += 8;
                        }
                        gx += 8;
                    }
                }
                player.crouch = found && crouched;
                HOLD_CL = false;
                player.clear_velocity();
                player.on_ground = false;
                player.ground_mover = -1;
                PIN_POSE = (player.pos, *yaw, *pitch);
                crate::reference_trace::probe_vals(
                    now,
                    if found { "tpfree" } else { "tpfree-none" },
                    &[
                        player.pos[0],
                        player.pos[1],
                        player.pos[2],
                        crouched as i32,
                        n_free,
                        n_touch,
                    ],
                );
            }
            43 | 44 => {
                let mut pi = 0usize;
                while pi < crate::PROP_COUNT {
                    if crate::PROP_ACTIVE[pi] != 0
                        && crate::PROP_HEALTH[pi] != 0
                        && crate::PROP_KIND[pi] as i32 == c.a[0] as i32
                    {
                        let t = crate::PROP_POS[pi];
                        if c.op == 43 {
                            player.pos = [
                                t[0] + c.a[1] as i32,
                                t[1] + c.a[2] as i32,
                                t[2] + c.a[3] as i32,
                            ];
                            player.clear_velocity();
                            player.on_ground = false;
                            player.ground_mover = -1;
                        }
                        let eye = [
                            player.pos[0],
                            player.pos[1] + crate::VIEW_HEIGHT,
                            player.pos[2],
                        ];
                        let aim = [t[0], t[1] + 24 + c.a[1] as i32, t[2]];
                        let d = [aim[0] - eye[0], aim[1] - eye[1], aim[2] - eye[2]];
                        let flat = crate::isqrt_i32(d[0] * d[0] + d[2] * d[2]).max(1);
                        *yaw = crate::yaw_from_vec(d[0], d[2]);
                        // pitch from the aim slope, in the view's 4096ths of a turn
                        *pitch = (d[1] * 652 / flat).clamp(-1000, 1000) as i16;
                        PIN_POSE = (player.pos, *yaw, *pitch);
                        crate::reference_trace::probe_vals(
                            now,
                            if c.op == 43 { "tpnear" } else { "aimat" },
                            &[
                                pi as i32,
                                player.pos[0],
                                player.pos[1],
                                player.pos[2],
                                *yaw as i32,
                                *pitch as i32,
                            ],
                        );
                        break;
                    }
                    pi += 1;
                }
            }
            42 => {
                let cur = weapons.current;
                let d = crate::wdef_of(cur);
                crate::reference_trace::probe_vals(
                    now,
                    "arsenal",
                    &[
                        weapons.owned as i32,
                        cur as i32,
                        weapons.clip[cur] as i32,
                        weapons.ammo[d.ammo] as i32,
                        weapons.reload_ticks as i32,
                        weapons.cooldown as i32,
                    ],
                );
            }
            #[cfg(feature = "probe-fixes")]
            22 => weapons.current = c.a[0].clamp(0, 13) as usize,
            #[cfg(feature = "probe-fixes")]
            25 => {
                let mut best = usize::MAX;
                let mut best_d = i32::MAX;
                let mut pi = 0usize;
                while pi < crate::PROP_COUNT {
                    if crate::PROP_ACTIVE[pi] != 0
                        && crate::PROP_HEALTH[pi] != 0
                        && crate::PROP_KIND[pi] as i32 == c.a[0] as i32
                    {
                        let d = crate::dist2_3(crate::PROP_POS[pi], player.pos);
                        if d < best_d {
                            best_d = d;
                            best = pi;
                        }
                    }
                    pi += 1;
                }
                if best != usize::MAX {
                    let dmg = crate::logic_state::club_damage_for(
                        crate::PROP_KIND[best],
                        true,
                        c.a[1].clamp(1, 255) as u8,
                    );
                    crate::reference_trace::probe_vals(
                        now,
                        "club",
                        &[best as i32, dmg as i32, crate::PROP_HEALTH[best] as i32],
                    );
                    crate::damage_prop(best, dmg, true);
                }
            }
            24 => PROJ_LOG = c.a[0] != 0,
            27 => {
                // ammo POOL N : set one ammo pool (and every magazine to N when POOL is 99)
                let pool = c.a[0].clamp(0, crate::combat_defs::N_AMMO as i16 - 1) as usize;
                weapons.ammo[pool] = c.a[1].max(0) as u16;
            }
            26 => {
                // propdump : one line per live actor (index, kind, health, position)
                let mut pi = 0usize;
                while pi < crate::PROP_COUNT {
                    if crate::PROP_ACTIVE[pi] != 0 && crate::PROP_HEALTH[pi] != 0 {
                        let t = crate::PROP_POS[pi];
                        crate::reference_trace::probe_vals(
                            now,
                            "prop",
                            &[
                                pi as i32,
                                crate::PROP_KIND[pi] as i32,
                                crate::PROP_HEALTH[pi] as i32,
                                t[0],
                                t[1],
                                t[2],
                            ],
                        );
                    }
                    pi += 1;
                }
            }
            32 => {
                crate::mapsnap::keep_now(&crate::LIVE_CHECKPOINT);
                crate::reference_trace::probe(
                    now,
                    "snap",
                    "",
                    crate::mapsnap::bytes().len() as i32,
                );
            }
            33 => {
                let id = name_id(m, cname);
                let mut li = 0usize;
                while li < nlogic {
                    if id != 0 && m.logic(li).targetname == id {
                        crate::reference_trace::probe_vals(
                            now,
                            "logic",
                            &[
                                li as i32,
                                crate::LOGIC_STATE[li] as i32,
                                crate::LOGIC_NEXT[li] as i32,
                                crate::LOGIC_COUNTER[li] as i32,
                            ],
                        );
                    }
                    li += 1;
                }
            }
            28 => {
                let cp = crate::LIVE_CHECKPOINT;
                crate::mapsnap::keep_now(&cp);
                let target = crate::save::Target::choices(false)[0];
                let r = crate::save::write(
                    target,
                    &cp,
                    crate::mapsnap::bytes(),
                    crate::mapsnap::tick(),
                    crate::save_staging(),
                );
                let code = match r {
                    crate::save::CardResult::Ok => 0,
                    crate::save::CardResult::NoCard => 1,
                    crate::save::CardResult::Failed => 2,
                };
                crate::reference_trace::probe_vals(
                    now,
                    "savecard",
                    &[code, crate::mapsnap::bytes().len() as i32],
                );
            }
            29 => {
                let room = crate::load_saved_room();
                crate::reference_trace::probe(now, "loadcard", "", room.map_or(-1, |r| r as i32));
                if let Some(r) = room {
                    GOTO = Some((r, false));
                }
            }
            37 => {
                // hit KIND DMG : one hit on the player of that HEV damage kind
                let kind = match c.a[0] {
                    1 => crate::hev::Kind::Fall,
                    2 => crate::hev::Kind::Crush,
                    3 => crate::hev::Kind::Bullet,
                    4 => crate::hev::Kind::Slash,
                    5 => crate::hev::Kind::Sonic,
                    6 => crate::hev::Kind::Poison,
                    7 => crate::hev::Kind::Acid,
                    8 => crate::hev::Kind::NerveGas,
                    9 => crate::hev::Kind::Radiation,
                    _ => crate::hev::Kind::Generic,
                };
                crate::fall_damage_player(health, c.a[1].max(0) as u16, kind);
            }
            38 => {
                // ammo : the current weapon has one round left and no reserve
                let d = weapons.def();
                weapons.clip[weapons.current] = 1;
                weapons.ammo[d.ammo] = 0;
            }
            50 => {
                // cartreset N X Y Z SUPPORT : put the Nth pushable back at that resting origin, at rest
                let mut n = c.a[0] as i32;
                let mut ei = 0usize;
                while ei < crate::ENT_SOLID_COUNT {
                    if crate::ENT_CACHE[ei].kind == crate::ENT_KIND_PUSHABLE {
                        if n == 0 {
                            let movers = core::slice::from_raw_parts_mut(
                                core::ptr::addr_of_mut!(crate::MOVERS).cast::<crate::phys::Mover>(),
                                crate::MOVER_COUNT.min(crate::MAX_ENTS + 1),
                            );
                            crate::pushable_publish_offset(
                                m,
                                movers,
                                ei,
                                [c.a[1] as i32, c.a[3] as i32, c.a[2] as i32],
                            );
                            crate::ENT_PHASE[ei] =
                                psx_goldsrc::pushable::pack(0, 0, 0, c.a[4] as u16, false);
                            crate::reference_trace::probe_vals(now, "cartreset", &[ei as i32]);
                            break;
                        }
                        n -= 1;
                    }
                    ei += 1;
                }
            }
            16 => FACE = if cname == "-" { 0 } else { name_id(m, cname) },
            13 => crate::reference_trace::probe_vals(
                now,
                "pos",
                &[player.pos[0], player.pos[1], player.pos[2], *health as i32],
            ),
            61 => HOLD_ROLL = (c.a[0] as i32, now + c.a[1] as u32),
            60 => {
                player.clear_velocity();
                player.vel[1] = -(c.a[0] as i32 / 20);
                player.on_ground = false;
                crate::reference_trace::probe_vals(now, "fallv", &[player.vel[1]]);
            }
            _ => {}
        }
    }
    if PROJ_LOG {
        crate::probe_projectiles(now);
    }
    if PROJ_WATCH && now % 2 == 0 {
        let mut i = 0usize;
        while i < crate::MAX_PROJECTILES {
            let p = crate::PROJECTILES[i];
            if p.active {
                crate::reference_trace::probe_vals(
                    now,
                    "proj",
                    &[
                        i as i32,
                        p.kind as i32,
                        p.pos[0],
                        p.pos[1],
                        p.pos[2],
                        p.life as i32,
                    ],
                );
            }
            i += 1;
        }
    }
    if WATCH_KINDS[0] >= 0 && now % WATCH_EVERY == 0 {
        let mut pi = 0usize;
        while pi < crate::PROP_COUNT {
            let kind = crate::PROP_KIND[pi] as i32;
            if crate::PROP_ACTIVE[pi] != 0 && WATCH_KINDS.contains(&kind) {
                let t = crate::PROP_POS[pi];
                crate::reference_trace::probe_vals(
                    now,
                    "watch",
                    &[
                        pi as i32,
                        crate::PROP_HEALTH[pi] as i32,
                        t[0],
                        t[1],
                        t[2],
                        crate::PROP_STATE[pi] as i32,
                        crate::PROP_OCC_VIS[pi] as i32,
                        crate::PROP_AI_TARGET[pi] as i32,
                        kind,
                    ],
                );
            }
            pi += 1;
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

/// Preferred centre (engine axes) for the next `tpfree`, set by `tppref`.
static mut PREF: [i32; 3] = [0; 3];
static mut HAS_PREF: bool = false;

/// Would the player standing (or ducking) at `p` set off a trigger_changelevel? The same
/// bounds and clip-hull tests `logic_touch_triggers` applies.
unsafe fn touches_changelevel(
    m: &crate::map::Map,
    nlogic: usize,
    p: [i32; 3],
    crouched: bool,
) -> bool {
    let hh = if crouched {
        crate::PLAYER_DUCK_HALF_HEIGHT
    } else {
        crate::PLAYER_HULL_HALF_HEIGHT
    };
    let mins = [p[0] - 16, p[1] - hh, p[2] - 16];
    let maxs = [p[0] + 16, p[1] + hh, p[2] + 16];
    let mut li = 0usize;
    while li < nlogic {
        if crate::LOGIC_KIND[li] == crate::map::LOGIC_TRIGGER_CHANGELEVEL
            && m.logic_touches_bounds(li, mins, maxs)
        {
            let rec = m.logic(li);
            if rec.brush == crate::map::LOGIC_BRUSH_NONE
                || rec.brush & crate::map::LOGIC_BRUSH_SHAPE == 0
                || crate::phys::inside_clip_hull(
                    m,
                    (rec.brush & !crate::map::LOGIC_BRUSH_SHAPE) as i16,
                    p,
                )
            {
                return true;
            }
        }
        li += 1;
    }
    false
}

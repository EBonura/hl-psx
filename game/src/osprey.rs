//! COsprey (osprey.cpp): path_corner flight, grunt resupply and crash.

use crate::*;

// ---- COsprey (osprey.cpp): path_corner flight, grunt resupply, crash ----
// One osprey per map. Positions are world units, velocities units/s,
// angles q12 turns [pitch, yaw, roll] with yaw unwrapped like m_ang1.y.
pub(crate) const PROP_TYPE_OSPREY: u8 = 61;
pub(crate) const OSPREY_WAIT: u8 = 0;
pub(crate) const OSPREY_FLY: u8 = 1;
pub(crate) const OSPREY_DEPLOY: u8 = 2;
pub(crate) const OSPREY_HOVER: u8 = 3;
pub(crate) const OSPREY_DYING: u8 = 4;
#[derive(Clone, Copy)]
pub(crate) struct Osprey {
    pub(crate) li: u16,
    pub(crate) aux: u16,
    pub(crate) corners: u8,
    pub(crate) loop_start: u8,
    pub(crate) pi: u8,
    pub(crate) phase: u8,
    pub(crate) goal: u8,
    pub(crate) start: u16,
    pub(crate) dt: u16,
    pub(crate) next: u16,
    pub(crate) spd: [u16; 2],
    pub(crate) p: [[i16; 3]; 2],
    pub(crate) v: [[i16; 3]; 2],
    pub(crate) a: [[i16; 3]; 2],
    pub(crate) vel: [i16; 3],
    pub(crate) repel: [u8; 4],
    pub(crate) repel_y: [i16; 4],
    pub(crate) grunts: [u32; 4],
}
pub(crate) static mut OSPREY: Osprey = Osprey {
    li: u16::MAX,
    aux: 0,
    corners: 1,
    loop_start: 0,
    pi: 0,
    phase: 0,
    goal: 0,
    start: 0,
    dt: 0,
    next: 0,
    spd: [0; 2],
    p: [[0; 3]; 2],
    v: [[0; 3]; 2],
    a: [[0; 3]; 2],
    vel: [0; 3],
    repel: [0xff; 4],
    repel_y: [0; 4],
    grunts: [0; 4],
};

/// Corner `k` of the osprey's cooked chain: (pos, speed, angles q12).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn osprey_corner(m: &Map, k: usize) -> ([i16; 3], u16, [i16; 3]) {
    let fa = OSPREY.aux as usize + k * 3;
    let (a, b, c) = (m.logic_aux(fa), m.logic_aux(fa + 1), m.logic_aux(fa + 2));
    let q = |v: u16| ((v as u8 as i8) as i16) << 4;
    (
        [a.target as i16, a.delay_ticks as i16, b.target as i16],
        b.delay_ticks,
        [q(c.delay_ticks), (c.target & 0xfff) as i16, q(c.delay_ticks >> 8)],
    )
}

/// COsprey::UpdateGoal toward corner OSPREY.goal.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn osprey_update_goal(m: &Map) {
    let o = &mut OSPREY;
    let (pos, speed, mut ang) = osprey_corner(m, o.goal as usize);
    o.p[0] = o.p[1];
    o.a[0] = o.a[1];
    o.v[0] = o.v[1];
    o.spd[0] = o.spd[1];
    // UTIL_MakeAimVectors(0, yaw, 0) * speed
    let y = ang[1] as u16;
    o.v[1] = [
        ((sincos::sin_q12(y) * speed as i32) >> 12) as i16,
        0,
        ((sincos::sin_q12((y + 1024) & 0xfff) * speed as i32) >> 12) as i16,
    ];
    o.p[1] = pos;
    o.spd[1] = speed;
    o.start = o.start.wrapping_add(o.dt);
    let d = [pos[0] as i32 - o.p[0][0] as i32, pos[1] as i32 - o.p[0][1] as i32, pos[2] as i32 - o.p[0][2] as i32];
    o.dt = (isqrt_i32(dist2_3(d, [0; 3])) * 40 / (o.spd[0] as i32 + speed as i32).max(1)).clamp(1, 4000) as u16;
    // m_ang1.y is unwrapped to the short arc toward the new goal.
    let dy = o.a[0][1] as i32 - ang[1] as i32;
    if dy < -2048 {
        o.a[0][1] += 4096;
    } else if dy > 2048 {
        o.a[0][1] -= 4096;
    }
    o.a[1] = ang;
}

/// COsprey::HasDead: has any grunt of the map died since FindAllThink?
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn osprey_has_dead() -> bool {
    let mut pi = 0;
    while pi < MAX_PROPS {
        if OSPREY.grunts[pi >> 5] & (1 << (pi & 31)) != 0 && (PROP_ACTIVE[pi] == 0 || PROP_HEALTH[pi] == 0) {
            return true;
        }
        pi += 1;
    }
    false
}

/// COsprey::DeployThink/MakeGrunt: each rope point below the hull brings a
/// dead grunt's slot back as a fresh grunt gliding down at ~160 u/s.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn osprey_deploy(m: &Map) {
    let o = &mut OSPREY;
    let op = PROP_POS[o.pi as usize];
    let yaw = (o.a[1][1] as u16) & 0xfff;
    let (s, c) = (sincos::sin_q12(yaw), sincos::sin_q12((yaw + 1024) & 0xfff));
    let mut r = 0;
    let mut pi = 0;
    while r < 4 {
        // forward (+32 / -64), right (+-100), up -96 (UTIL_MakeAimVectors)
        let f = if r & 1 == 0 { 32 } else { -64 };
        let side = if r < 2 { 100 } else { -100 };
        let src = [op[0] + ((s * f + c * side) >> 12), op[1] - 96, op[2] + ((c * f - s * side) >> 12)];
        o.repel[r] = 0xff;
        while pi < MAX_PROPS {
            if o.grunts[pi >> 5] & (1 << (pi & 31)) != 0 && (PROP_ACTIVE[pi] == 0 || PROP_HEALTH[pi] == 0) {
                PROP_ACTIVE[pi] = 1;
                PROP_HEALTH[pi] = prop_start_health(8);
                PROP_STATE[pi] = PROP_STATE_IDLE;
                PROP_AI_TARGET[pi] = PROP_TARGET_NONE;
                PROP_AI_TIMER[pi] = 0;
                PROP_DORMANT[pi] |= PROP_RUNTIME_PRISONER; // ACT_GLIDE until it lands
                PROP_YAW[pi] = prop_with_yaw(PROP_YAW[pi], yaw);
                prop_set_pos_exact(m, pi, src);
                seed_prop_render_transform(pi);
                o.repel[r] = pi as u8;
                o.repel_y[r] = prop_floor_y_down(m, pi, src, 4096).unwrap_or(src[1]) as i16;
                pi += 1;
                break;
            }
            pi += 1;
        }
        r += 1;
    }
}

/// Bind the osprey record at map load (COsprey::Spawn).
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn osprey_init(li: usize, rec: map::LogicEnt, pi: usize) {
    let o = &mut OSPREY;
    o.li = li as u16;
    o.aux = rec.first_aux as u16;
    o.corners = (rec.aux_count / 3).max(1) as u8;
    o.loop_start = rec.flags;
    o.pi = pi as u8;
    o.phase = OSPREY_WAIT;
    o.repel = [0xff; 4];
    let p = PROP_POS[pi];
    o.p[1] = [p[0] as i16, p[1] as i16, p[2] as i16];
    o.v[1] = [0; 3];
    o.spd[1] = 0;
    OSPREY_TILT = u16::MAX; // authored until the first think
    // SF_WAITFORTRIGGER holds until CommandUse; else FindAllThink at +1 s.
    o.next = if rec.speed & 0x40 != 0 { 0xffff } else { SIM_NOW.wrapping_add(20) };
}

/// COsprey::FindAllThink .. FlyThink / Flight / HoverThink / DyingThink.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick_osprey(m: &Map, movers: &[phys::Mover]) {
    let o = &mut OSPREY;
    let pi = o.pi as usize;
    let now = SIM_NOW;
    let think = o.next != 0xffff && time_reached(now, o.next);
    if think {
        o.next = now.wrapping_add(2);
    }
    if PROP_HEALTH[pi] == 0 && o.phase != OSPREY_DYING {
        // Killed: MOVETYPE_TOSS at 0.3 gravity with the flight velocity,
        // exploding on the first solid it meets or after four seconds.
        o.phase = OSPREY_DYING;
        o.start = now.wrapping_add(80);
    }
    match o.phase {
        OSPREY_WAIT => {
            if think {
                let tilt = ((m.prop_orientation(pi) as u32) >> 16) as u16;
                let q = |v: u16| ((v as u8 as i8) as i16) << 4;
                o.a[1] = [q(tilt), (PROP_YAW[pi] & PROP_YAW_MASK) as i16, q(tilt >> 8)];
                OSPREY_TILT = tilt;
                let mut n = 0;
                o.grunts = [0; 4];
                let mut qi = 0;
                while qi < PROP_COUNT.min(CARRY_MAILBOX_FIRST) {
                    if PROP_KIND[qi] == 8 && PROP_ACTIVE[qi] != 0 && PROP_HEALTH[qi] != 0 && n < 24 {
                        o.grunts[qi >> 5] |= 1 << (qi & 31);
                        n += 1;
                    }
                    qi += 1;
                }
                if n == 0 {
                    // "osprey error: no grunts to resupply": UTIL_Remove.
                    PROP_ACTIVE[pi] = 0;
                    o.li = u16::MAX;
                    return;
                }
                o.phase = OSPREY_FLY;
                o.start = now;
                o.dt = 0;
                o.goal = 0;
                osprey_update_goal(m);
            }
            return;
        }
        OSPREY_DEPLOY => {
            if think {
                osprey_deploy(m);
                o.phase = OSPREY_HOVER;
            }
        }
        OSPREY_HOVER => {
            if think && (0..4).all(|r| {
                let g = o.repel[r] as usize;
                g >= MAX_PROPS || PROP_HEALTH[g] == 0 || (PROP_DORMANT[g] & PROP_RUNTIME_PRISONER) == 0
            }) {
                o.start = now;
                o.phase = OSPREY_FLY;
            }
        }
        OSPREY_DYING => {
            let p = PROP_POS[pi];
            o.vel[1] -= 12; // 0.3 * sv_gravity per 20 Hz tick
            let mut np = p;
            for k in 0..3 {
                np[k] += o.vel[k] as i32 / 20;
            }
            let hit = phys::trace_line(m, movers, p, np);
            if think && now & 3 == 0 {
                queue_explosion_fx([np[0] + (IMPACT_RNG.below(301) as i32 - 150), np[1] - 100, np[2] + (IMPACT_RNG.below(301) as i32 - 150)], 60);
            }
            if hit.is_some() || time_reached(now, o.start) {
                // RadiusDamage(300, DMG_BLAST) and the gib shower.
                explode(m, np, 255, 750, false);
                PROP_ACTIVE[pi] = 0;
                o.li = u16::MAX;
                return;
            }
            prop_set_pos_exact(m, pi, np);
            return;
        }
        _ => {
            if think && time_reached(now, o.start.wrapping_add(o.dt)) {
                let (n, loop_start) = (o.corners as usize, o.loop_start);
                if osprey_corner(m, o.goal as usize).1 == 0 {
                    o.phase = OSPREY_DEPLOY;
                }
                // Skip the slow deploy corners while every grunt lives.
                let dead = osprey_has_dead();
                let mut guard = n;
                loop {
                    let g = o.goal as usize + 1;
                    o.goal = if g >= n { loop_start } else { g as u8 };
                    guard -= 1;
                    if guard == 0 || osprey_corner(m, o.goal as usize).1 >= 400 || dead {
                        break;
                    }
                }
                osprey_update_goal(m);
            }
        }
    }
    // Repel ropes: the grunts glide down to the floor under them.
    let mut r = 0;
    while r < 4 {
        let g = o.repel[r] as usize;
        if g < MAX_PROPS && PROP_DORMANT[g] & PROP_RUNTIME_PRISONER != 0 {
            let p = PROP_POS[g];
            let y = (p[1] - 8).max(o.repel_y[r] as i32);
            prop_set_pos_exact(m, g, [p[0], y, p[2]]);
            push_tracer_styled([p[0], PROP_POS[pi][1] + 16, p[2]], [p[0], y + 72, p[2]], TRACER_ROPE);
            if y == o.repel_y[r] as i32 {
                PROP_DORMANT[g] &= !PROP_RUNTIME_PRISONER;
            }
        }
        r += 1;
    }
    if o.phase != OSPREY_FLY {
        return;
    }
    // Flight: Hermite blend of the two corner extrapolations.
    let dt = o.dt as i32;
    let t = (now.wrapping_sub(o.start) as i16 as i32).clamp(0, dt);
    let x = t * 4096 / dt;
    let x2 = (x * x) >> 12;
    let f = 3 * x2 - 2 * ((x2 * x) >> 12);
    let mut pos = [0i32; 3];
    let mut ang = [0i32; 3];
    let mut k = 0;
    while k < 3 {
        ang[k] = (o.a[0][k] as i32 * (4096 - f) + o.a[1][k] as i32 * f) >> 12;
        let a = o.p[0][k] as i32 + o.v[0][k] as i32 * t / 20;
        let b = o.p[1][k] as i32 - o.v[1][k] as i32 * (dt - t) / 20;
        pos[k] = (a * (4096 - f) + b * f) >> 12;
        o.vel[k] = ((o.v[0][k] as i32 * (4096 - f) + o.v[1][k] as i32 * f) >> 12) as i16;
        k += 1;
    }
    prop_set_pos_exact(m, pi, pos);
    PROP_YAW[pi] = prop_with_yaw(PROP_YAW[pi], ang[1] as u16 & 0xfff);
    OSPREY_TILT = ((ang[0] >> 4) as u16 & 0xff) | (((ang[2] >> 4) as u16 & 0xff) << 8);
}
pub(crate) static mut OSPREY_TILT: u16 = 0;

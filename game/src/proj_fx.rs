//! What the player's weapon projectiles and the enemies' energy shots look like.
//!
//! A projectile used to be one flat square. Here a rocket, grenade, satchel,
//! bolt, hornet, snark or placed tripmine is a small flat-shaded solid built
//! from rings of a few sides (the shapes of the original models, not copies of
//! them), a lit rocket carries a flame and leaves a smoke trail, and glows
//! (the laser designator, the hornet, the energy balls) are nested additive
//! diamonds. Everything is integer maths and goes into the world ordering table
//! at its own depth, so a wall in front hides it.
//!
//! The solids scale up with distance so they never drop under a pixel or two:
//! the old square was five pixels at any range, and a rocket you cannot see is
//! worse than a rocket that is a little too big.

use crate::*;

/// A body of revolution along the flight axis: `sides` corners round a ring at
/// each end, `nose` extra length to a tip (0 = flat caps), optional fins.
#[derive(Clone, Copy)]
struct Body {
    sides: u8,
    /// World units behind and ahead of the position.
    back: i16,
    front: i16,
    /// Ring radius in quarter units.
    radius_q2: i16,
    nose: i16,
    main: (u8, u8, u8),
    tip: (u8, u8, u8),
    fins: bool,
}

const ROCKET: Body = Body {
    sides: 4,
    back: 14,
    front: 6,
    radius_q2: 10,
    nose: 8,
    main: (128, 130, 136),
    tip: (176, 62, 42),
    fins: true,
};
const GRENADE: Body = Body {
    sides: 6,
    back: 4,
    front: 4,
    radius_q2: 13,
    nose: 0,
    main: (88, 112, 62),
    tip: (45, 55, 36),
    fins: false,
};
const LAUNCHED_GRENADE: Body = Body {
    sides: 6,
    back: 4,
    front: 3,
    radius_q2: 10,
    nose: 4,
    main: (120, 112, 70),
    tip: (190, 150, 60),
    fins: false,
};
const SATCHEL: Body = Body {
    sides: 4,
    back: 5,
    front: 5,
    radius_q2: 16,
    nose: 0,
    main: (96, 104, 64),
    tip: (55, 60, 40),
    fins: false,
};
const BOLT: Body = Body {
    sides: 3,
    back: 9,
    front: 3,
    radius_q2: 4,
    nose: 4,
    main: (112, 82, 52),
    tip: (205, 205, 195),
    fins: false,
};
const HORNET: Body = Body {
    sides: 3,
    back: 5,
    front: 3,
    radius_q2: 6,
    nose: 3,
    main: (255, 190, 40),
    tip: (70, 45, 10),
    fins: false,
};
const SNARK: Body = Body {
    sides: 6,
    back: 3,
    front: 3,
    radius_q2: 11,
    nose: 2,
    main: (178, 150, 62),
    tip: (96, 70, 30),
    fins: false,
};
const MINE: Body = Body {
    sides: 4,
    back: 1,
    front: 2,
    radius_q2: 14,
    nose: 0,
    main: (74, 84, 74),
    tip: (36, 42, 36),
    fins: false,
};

/// A puff of rocket smoke: where it was left and how long ago.
#[derive(Clone, Copy)]
struct Puff {
    pos: [i16; 3],
    age: u8,
    live: bool,
}

const MAX_PUFFS: usize = 20;
const PUFF_TICKS: u8 = 24;
static mut PUFFS: [Puff; MAX_PUFFS] = [Puff {
    pos: [0; 3],
    age: 0,
    live: false,
}; MAX_PUFFS];
static mut PUFF_CURSOR: usize = 0;

/// Ticks a rocket coasts before its motor lights (`IgniteThink`, 0.4 s).
const IGNITE_TICKS: u8 = 8;

/// Forget every puff (map load, death).
pub(crate) unsafe fn clear() {
    let mut i = 0;
    while i < MAX_PUFFS {
        PUFFS[i].live = false;
        i += 1;
    }
}

/// One simulation tick: age the smoke and leave a puff behind every lit rocket.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn tick() {
    let mut i = 0;
    while i < MAX_PUFFS {
        if PUFFS[i].live {
            PUFFS[i].age += 1;
            if PUFFS[i].age >= PUFF_TICKS {
                PUFFS[i].live = false;
            }
        }
        i += 1;
    }
    let mut p = 0;
    while p < MAX_PROJECTILES {
        let pr = PROJECTILES[p];
        if pr.active && pr.kind == PROJ_ROCKET && pr.aux == 1 && 255 - pr.life >= IGNITE_TICKS {
            let f = unit([pr.dir[0] as i32, pr.dir[1] as i32, pr.dir[2] as i32])
                .unwrap_or([0, 0, 4096]);
            let at = [
                pr.pos[0] - ((f[0] * 14) >> 12),
                pr.pos[1] - ((f[1] * 14) >> 12),
                pr.pos[2] - ((f[2] * 14) >> 12),
            ];
            let slot = PUFF_CURSOR % MAX_PUFFS;
            PUFF_CURSOR = PUFF_CURSOR.wrapping_add(1);
            PUFFS[slot] = Puff {
                pos: [at[0] as i16, at[1] as i16, at[2] as i16],
                age: 0,
                live: true,
            };
        }
        p += 1;
    }
}

/// `v` scaled to a q12 unit vector, or `None` for a zero vector.
fn unit(v: [i32; 3]) -> Option<[i32; 3]> {
    let len = isqrt_i32(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    if len == 0 {
        return None;
    }
    Some([
        mul_div_i32(v[0], 4096, len),
        mul_div_i32(v[1], 4096, len),
        mul_div_i32(v[2], 4096, len),
    ])
}

/// Cross product of two q12 vectors, in q12.
fn cross(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [
        (a[1] * b[2] - a[2] * b[1]) >> 12,
        (a[2] * b[0] - a[0] * b[2]) >> 12,
        (a[0] * b[1] - a[1] * b[0]) >> 12,
    ]
}

/// A point in camera space as screen x, screen y and depth. Unlike
/// `project_world_point` it keeps points a little off screen, so a body
/// half out of view is not dropped whole.
fn cam(p: [i32; 3], rot: &Mat3I16, base_t: [i32; 3]) -> Option<(i32, i32, i32)> {
    let vz = dot12(rot.m[2], p) + base_t[2];
    if !(render::NEAR_Z..=far_view()).contains(&vz) {
        return None;
    }
    let vx = dot12(rot.m[0], p) + base_t[0];
    let vy = dot12(rot.m[1], p) + base_t[1];
    let h = render::projection_h();
    let (sx, sy) = (160 + (vx * h) / vz, 120 + (vy * h) / vz);
    if !(-700..=1000).contains(&sx) || !(-700..=1000).contains(&sy) {
        return None;
    }
    Some((sx, sy, vz))
}

/// One opaque flat triangle at the depth of its middle.
unsafe fn push_tri(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    v: [(i32, i32, i32); 3],
    rgb: (u8, u8, u8),
) {
    let z = (v[0].2 + v[1].2 + v[2].2) / 3 - 8;
    let at = |i: usize| (v[i].0 as i16, v[i].1 as i16);
    if let Some(packet) = packets.push(TriFlat::new([at(0), at(1), at(2)], rgb.0, rgb.1, rgb.2)) {
        ot.resume_frame().add_raw(
            clamp_otz((z.max(render::NEAR_Z) as usize) >> OT_SHIFT),
            core::ptr::from_mut(packet).cast(),
            TriFlat::WORDS,
        );
    } else {
        note_render_packet_drop(false);
    }
}

/// `rgb` lit by a fixed light from above and to one side, by the face normal
/// `n` (any length, q12 per unit).
fn lit(rgb: (u8, u8, u8), n: [i32; 3]) -> (u8, u8, u8) {
    let len = isqrt_i32(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).max(1);
    // Light direction (0.35, 0.80, 0.48) in q12.
    let d = (n[0] * 1434 + n[1] * 3277 + n[2] * 1966) / len;
    let f = 150 + (d.clamp(-4096, 4096) * 105) / 4096;
    let s = |c: u8| ((c as i32 * f) / 255).clamp(0, 255) as u8;
    (s(rgb.0), s(rgb.1), s(rgb.2))
}

/// An additive glow: three nested diamonds of falling brightness, `half` world
/// units across at the middle. `at` is its centre.
unsafe fn glow(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    at: [i32; 3],
    half: i32,
    min_px: i32,
    rgb: (u8, u8, u8),
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    let Some((cx, cy, z)) = cam(at, rot, base_t) else {
        return;
    };
    let r = ((half * render::projection_h()) / z).clamp(min_px, 70);
    let otz = clamp_otz(((z - 10).max(render::NEAR_Z) as usize) >> OT_SHIFT);
    let layers = [(r, 4u32), (r * 2 / 3, 3), (r / 3, 2)];
    let mut l = 0;
    while l < 3 {
        let (rr, div) = layers[l];
        let c = |v: u8| (v as u32 / div).min(255) as u8;
        let col = (c(rgb.0), c(rgb.1), c(rgb.2));
        let (t, rt, b, lf) = (
            (cx as i16, (cy - rr) as i16),
            ((cx + rr) as i16, cy as i16),
            (cx as i16, (cy + rr) as i16),
            ((cx - rr) as i16, cy as i16),
        );
        let mut half_tri = 0;
        while half_tri < 2 {
            let verts = if half_tri == 0 { [t, rt, b] } else { [t, b, lf] };
            if let Some(packet) = packets.push(BeamTri::new(verts, col)) {
                ot.resume_frame()
                    .add_raw(otz, core::ptr::from_mut(packet).cast(), BeamTri::WORDS);
            } else {
                note_render_packet_drop(false);
            }
            half_tri += 1;
        }
        l += 1;
    }
}

/// Draw one `body` at `pos` pointing along the q12 unit vector `f`, turned by
/// `roll` (q12 angle) about its axis.
#[inline(never)]
#[optimize(size)]
unsafe fn body(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    b: Body,
    pos: [i32; 3],
    f: [i32; 3],
    roll: i32,
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    let Some((_, _, z)) = cam(pos, rot, base_t) else {
        return;
    };
    // Keep the radius at 1.5 px or more by growing the whole body (scale q4).
    let screen_q4 = (b.radius_q2 as i32 * render::projection_h() * 4) / z.max(1);
    let k = if screen_q4 >= 24 {
        16
    } else {
        ((16 * 24) / screen_q4.max(1)).min(16 * 8)
    };
    let sc = |v: i32| (v * k) >> 4;
    let rad = sc(b.radius_q2 as i32) >> 2;
    let (back, front, nose) = (sc(b.back as i32), sc(b.front as i32), sc(b.nose as i32));
    let up = if f[1].abs() > 3900 {
        [4096, 0, 0]
    } else {
        [0, 4096, 0]
    };
    let Some(r) = unit(cross(f, up)) else {
        return;
    };
    let u = cross(r, f);
    let n = (b.sides as usize).clamp(3, 6);
    let along = |z_off: i32, radial: [i32; 3], radius: i32| {
        [
            pos[0] + ((f[0] * z_off + radial[0] * radius) >> 12),
            pos[1] + ((f[1] * z_off + radial[1] * radius) >> 12),
            pos[2] + ((f[2] * z_off + radial[2] * radius) >> 12),
        ]
    };
    let mut radial = [[0i32; 3]; 6];
    let mut mid = [[0i32; 3]; 6];
    let mut ring_b = [(0i32, 0i32, 0i32); 6];
    let mut ring_f = [(0i32, 0i32, 0i32); 6];
    let mut ok = true;
    let mut k_side = 0;
    while k_side < n {
        let step = 4096 / n as i32;
        let ang = (roll + k_side as i32 * step) & 0x0fff;
        let amid = (ang + step / 2) & 0x0fff;
        let dir = |a: i32| {
            let (s, c) = (sincos::sin_q12(a as u16), sincos::sin_q12(((a + 1024) & 0x0fff) as u16));
            [
                (r[0] * c + u[0] * s) >> 12,
                (r[1] * c + u[1] * s) >> 12,
                (r[2] * c + u[2] * s) >> 12,
            ]
        };
        radial[k_side] = dir(ang);
        mid[k_side] = dir(amid);
        match (
            cam(along(-back, radial[k_side], rad), rot, base_t),
            cam(along(front, radial[k_side], rad), rot, base_t),
        ) {
            (Some(pb), Some(pf)) => {
                ring_b[k_side] = pb;
                ring_f[k_side] = pf;
            }
            _ => ok = false,
        }
        k_side += 1;
    }
    if !ok {
        return;
    }
    let cam_fwd = [
        rot.m[2][0] as i32,
        rot.m[2][1] as i32,
        rot.m[2][2] as i32,
    ];
    // A face whose normal points the way the camera looks faces away.
    let away = |nrm: [i32; 3]| {
        ((nrm[0] * cam_fwd[0] + nrm[1] * cam_fwd[1] + nrm[2] * cam_fwd[2]) >> 12) > 0
    };
    let centre_f = cam(along(front, [0; 3], 0), rot, base_t);
    let centre_b = cam(along(-back, [0; 3], 0), rot, base_t);
    let tip = cam(along(front + nose, [0; 3], 0), rot, base_t);
    let mut s = 0;
    while s < n {
        let t = (s + 1) % n;
        if !away(mid[s]) {
            let col = lit(b.main, mid[s]);
            push_tri(packets, ot, [ring_b[s], ring_b[t], ring_f[t]], col);
            push_tri(packets, ot, [ring_b[s], ring_f[t], ring_f[s]], col);
        }
        if nose > 0 {
            let nn = [mid[s][0] + f[0], mid[s][1] + f[1], mid[s][2] + f[2]];
            if let (false, Some(tp)) = (away(nn), tip) {
                push_tri(packets, ot, [ring_f[s], ring_f[t], tp], lit(b.tip, nn));
            }
        } else if let Some(cf) = centre_f {
            if !away(f) {
                push_tri(packets, ot, [ring_f[s], ring_f[t], cf], lit(b.tip, f));
            }
        }
        if let Some(cb) = centre_b {
            let neg = [-f[0], -f[1], -f[2]];
            if !away(neg) {
                let cap = if nose > 0 {
                    (b.main.0 / 2, b.main.1 / 2, b.main.2 / 2)
                } else {
                    b.tip
                };
                push_tri(packets, ot, [ring_b[s], cb, ring_b[t]], lit(cap, neg));
            }
        }
        s += 1;
    }
    if b.fins {
        let mut s = 0;
        while s < n {
            let a = cam(along(-back + rad * 3, radial[s], rad), rot, base_t);
            let tipf = cam(along(-back, radial[s], rad * 5 / 2), rot, base_t);
            if let (Some(a), Some(tf)) = (a, tipf) {
                let col = lit(b.main, mid[s]);
                push_tri(packets, ot, [ring_b[s], tf, a], col);
            }
            s += 1;
        }
    }
}

/// Queue every live projectile, the rocket smoke and the RPG's designator dot.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn queue(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    let mut i = 0;
    while i < MAX_PUFFS {
        let p = PUFFS[i];
        if p.live {
            let v = (70 * (PUFF_TICKS - p.age) as i32 / PUFF_TICKS as i32) as u8;
            let at = [p.pos[0] as i32, p.pos[1] as i32, p.pos[2] as i32];
            glow(packets, ot, at, 2 + p.age as i32 / 3, 2, (v, v, v), rot, base_t);
        }
        i += 1;
    }
    let mut i = 0;
    while i < MAX_PROJECTILES {
        let p = PROJECTILES[i];
        i += 1;
        if !p.active {
            continue;
        }
        let vel_dir = unit(p.vel);
        let tumble = (SIM_NOW as i32).wrapping_mul(170) & 0x0fff;
        match p.kind {
            PROJ_ROCKET => {
                let f = unit([p.dir[0] as i32, p.dir[1] as i32, p.dir[2] as i32])
                    .or(vel_dir)
                    .unwrap_or([0, 0, 4096]);
                body(packets, ot, ROCKET, p.pos, f, 0, rot, base_t);
                if 255 - p.life >= IGNITE_TICKS {
                    let tail = [
                        p.pos[0] - ((f[0] * 18) >> 12),
                        p.pos[1] - ((f[1] * 18) >> 12),
                        p.pos[2] - ((f[2] * 18) >> 12),
                    ];
                    glow(packets, ot, tail, 7, 3, (255, 150, 50), rot, base_t);
                }
            }
            PROJ_GRENADE | PROJ_M203 => {
                let f = vel_dir.unwrap_or([4096, 0, 0]);
                let b = if p.kind == PROJ_M203 {
                    LAUNCHED_GRENADE
                } else {
                    GRENADE
                };
                let roll = if p.vel == [0; 3] { 0 } else { tumble };
                body(packets, ot, b, p.pos, f, roll, rot, base_t);
            }
            PROJ_SATCHEL => {
                let f = vel_dir.unwrap_or([4096, 0, 0]);
                let roll = if p.vel == [0; 3] { 0 } else { tumble };
                body(packets, ot, SATCHEL, p.pos, f, roll, rot, base_t);
            }
            PROJ_BOLT | PROJ_BOLT_WATER => {
                let f = vel_dir
                    .or_else(|| unit([p.dir[0] as i32, p.dir[1] as i32, p.dir[2] as i32]))
                    .unwrap_or([0, 0, 4096]);
                body(packets, ot, BOLT, p.pos, f, 0, rot, base_t);
            }
            PROJ_HORNET | PROJ_HORNET_FAST => {
                let f = vel_dir.unwrap_or([0, 0, 4096]);
                let red = p.kind == PROJ_HORNET && p.aux & 1 == 0 && !p.from_enemy;
                body(packets, ot, HORNET, p.pos, f, tumble, rot, base_t);
                let col = if red { (255, 70, 40) } else { (255, 170, 40) };
                glow(packets, ot, p.pos, 4, 3, col, rot, base_t);
            }
            PROJ_SNARK => {
                let f = vel_dir.unwrap_or([0, 0, 4096]);
                body(packets, ot, SNARK, p.pos, f, 0, rot, base_t);
            }
            PROJ_TRIPMINE => {
                let placed = p.vel[0].abs().max(p.vel[1].abs()).max(p.vel[2].abs()) > 512;
                if let (true, Some(n)) = (placed, vel_dir) {
                    body(packets, ot, MINE, p.pos, n, 512, rot, base_t);
                    let lens = [
                        p.pos[0] + ((n[0] * 5) >> 12),
                        p.pos[1] + ((n[1] * 5) >> 12),
                        p.pos[2] + ((n[2] * 5) >> 12),
                    ];
                    let armed = p.life == 0 || p.life >= tripmine::FUSE_BASE;
                    let col = if armed { (255, 40, 30) } else { (160, 110, 20) };
                    glow(packets, ot, lens, 2, 2, col, rot, base_t);
                } else {
                    glow(packets, ot, p.pos, 3, 2, (255, 60, 40), rot, base_t);
                }
            }
            PROJ_SPIT | PROJ_BMORTAR => {
                glow(packets, ot, p.pos, 6, 3, (150, 230, 80), rot, base_t);
            }
            PROJ_CBALL => glow(packets, ot, p.pos, 8, 3, (130, 170, 255), rot, base_t),
            PROJ_HEADBALL => glow(packets, ot, p.pos, 14, 4, (225, 230, 255), rot, base_t),
            _ => {}
        }
    }
    if RPG_SPOT_ACTIVE {
        // CLaserSpot: a small red glow where the designator lands, pulled off
        // the surface a unit so it sorts in front of it.
        glow(packets, ot, RPG_SPOT_POS, 4, 4, (255, 36, 24), rot, base_t);
    }
}

/// One flat additive segment of `half` world units either side, thick enough
/// to read (up to 18 px), sorted at its own depth. `draw_beam` caps a line at
/// four pixels, which suits tracers and lasers but not the Gluon Gun.
unsafe fn thick_segment(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    a: [i32; 3],
    b: [i32; 3],
    half: i32,
    rgb: (u8, u8, u8),
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    let (Some((x0, y0, z0)), Some((x1, y1, z1))) = (cam(a, rot, base_t), cam(b, rot, base_t))
    else {
        return;
    };
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = isqrt_i32(dx * dx + dy * dy);
    if len < 2 {
        return;
    }
    let z = (z0 + z1) / 2;
    let hw = ((half * render::projection_h()) / z.max(1)).clamp(1, 18);
    let (px, py) = ((-dy * hw) / len, (dx * hw) / len);
    let otz = clamp_otz(((z - BEAM_DEPTH_BIAS).max(render::NEAR_Z) as usize) >> OT_SHIFT);
    let p = |x: i32, y: i32| (x as i16, y as i16);
    let quad = [
        [p(x0 + px, y0 + py), p(x0 - px, y0 - py), p(x1 + px, y1 + py)],
        [p(x0 - px, y0 - py), p(x1 - px, y1 - py), p(x1 + px, y1 + py)],
    ];
    let mut t = 0;
    while t < 2 {
        if let Some(packet) = packets.push(BeamTri::new(quad[t], rgb)) {
            ot.resume_frame()
                .add_raw(otz, core::ptr::from_mut(packet).cast(), BeamTri::WORDS);
        } else {
            note_render_packet_drop(false);
        }
        t += 1;
    }
}

/// The Gluon Gun's beam from `start` (the gun) to `end` (what it hit): a wide
/// blue ribbon that wanders sideways along its length (`SetNoise`), a bright
/// core, and the flare at the far end (`EGON_FLARE_SPRITE`). The blue pulses
/// as `CEgon::UpdateEffect` does.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn egon_beam(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    start: [i32; 3],
    end: [i32; 3],
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    const SEGMENTS: usize = 8;
    let span = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let Some(f) = unit(span) else {
        return;
    };
    let up = if f[1].abs() > 3900 {
        [4096, 0, 0]
    } else {
        [0, 4096, 0]
    };
    let Some(r) = unit(cross(f, up)) else {
        return;
    };
    let u = cross(r, f);
    let t = SIM_NOW as i32;
    let pulse = sincos::sin_q12(((t * 330) & 0x0fff) as u16).abs();
    let blue = (64 + ((80 * pulse) >> 12)) as u8;
    let mut pts = [[0i32; 3]; SEGMENTS + 1];
    let mut i = 0;
    while i <= SEGMENTS {
        let mut p = [
            start[0] + span[0] * i as i32 / SEGMENTS as i32,
            start[1] + span[1] * i as i32 / SEGMENTS as i32,
            start[2] + span[2] * i as i32 / SEGMENTS as i32,
        ];
        if i > 0 && i < SEGMENTS {
            let a = ((i as i32 * 1531 + t * 700) & 0x0fff) as u16;
            let (s, c) = (sincos::sin_q12(a), sincos::sin_q12((a + 1024) & 0x0fff));
            let amp = 5;
            let mut k = 0;
            while k < 3 {
                p[k] += ((r[k] * c + u[k] * s) >> 12) * amp >> 12;
                k += 1;
            }
        }
        pts[i] = p;
        i += 1;
    }
    let mut i = 0;
    while i < SEGMENTS {
        thick_segment(packets, ot, pts[i], pts[i + 1], 6, (22, 30, blue), rot, base_t);
        thick_segment(packets, ot, pts[i], pts[i + 1], 2, (90, 120, 255), rot, base_t);
        i += 1;
    }
    glow(packets, ot, end, 10, 4, (140, 190, 255), rot, base_t);
}

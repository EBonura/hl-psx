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

/// A resident weapon sprite as an additive camera-facing square `half` world
/// units out from `at`, tinted `rgb`, sorted a little nearer than its centre.
/// Beams and glows are overlays: the packets go straight into the ordering table
/// and never into the world plan the cache replays.
unsafe fn sprite_quad(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    at: [i32; 3],
    half: i32,
    id: usize,
    frame: usize,
    rgb: (u8, u8, u8),
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    let slot = sprite::slot_for(id, frame);
    let Some((cx, cy, z)) = cam(at, rot, base_t) else {
        return;
    };
    if !slot.valid {
        return;
    }
    let d = sprite::def(id);
    let r = ((half * render::projection_h()) / z).clamp(1, 110);
    let otz = clamp_otz(((z - 10).max(render::NEAR_Z) as usize) >> OT_SHIFT);
    let packet = TexturedGouraudPacketMaterial::from_texture(
        slot.material.with_blend_mode(BlendMode::Add),
    );
    let uw = d.crush_w.saturating_sub(1).min(255) as u8;
    let uh = d.crush_h.saturating_sub(1).min(255) as u8;
    let (l, t, rr, b) = (
        (cx - r) as i16,
        (cy - r) as i16,
        (cx + r) as i16,
        (cy + r) as i16,
    );
    let tris = [
        [(l, t), (rr, t), (l, b)],
        [(rr, t), (rr, b), (l, b)],
    ];
    let uvs = [
        [uv_word((0, 0)), uv_word((uw, 0)), uv_word((0, uh))],
        [uv_word((uw, 0)), uv_word((uw, uh)), uv_word((0, uh))],
    ];
    let mut k = 0;
    while k < 2 {
        let prim = TriTexturedGouraud {
            tag: 0,
            tex_window: packet.tex_window_word,
            color0_cmd: (packet.color0_command_word | 0x0200_0000) | pack_rgb_word(rgb),
            v0: pack_vertex_word(tris[k][0].0, tris[k][0].1),
            uv0_clut: uvs[k][0] as u32 | packet.clut_high_word,
            color1: pack_rgb_word(rgb),
            v1: pack_vertex_word(tris[k][1].0, tris[k][1].1),
            uv1_tpage: uvs[k][1] as u32 | packet.tpage_high_word,
            color2: pack_rgb_word(rgb),
            v2: pack_vertex_word(tris[k][2].0, tris[k][2].1),
            uv2: uvs[k][2] as u32,
        };
        if let Some(pk) = packets.push(prim) {
            ot.resume_frame().add_raw(
                otz,
                core::ptr::from_mut(pk).cast(),
                TriTexturedGouraud::WORDS,
            );
        } else {
            note_render_packet_drop(false);
        }
        k += 1;
    }
}

/// A soft round glow (the hot glow sprite).
#[inline]
pub(crate) unsafe fn glow(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    at: [i32; 3],
    half: i32,
    rgb: (u8, u8, u8),
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    sprite_quad(packets, ot, at, half, sprite::WPN_HOT_GLOW, 0, rgb, rot, base_t);
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
            glow(packets, ot, at, 2 + p.age as i32 / 3, (v, v, v), rot, base_t);
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
                    glow(packets, ot, tail, 7, (255, 150, 50), rot, base_t);
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
                glow(packets, ot, p.pos, 4, col, rot, base_t);
            }
            PROJ_SNARK => {
                if resident(PROP_TYPE_SNARK).is_none() {
                    let f = vel_dir.unwrap_or([0, 0, 4096]);
                    body(packets, ot, SNARK, p.pos, f, 0, rot, base_t);
                }
            }
            PROJ_TRIPMINE => {
                let placed = p.vel[0].abs().max(p.vel[1].abs()).max(p.vel[2].abs()) > 512;
                if let (true, Some(n)) = (placed, vel_dir) {
                    if resident(tripmine::PROP_TYPE_TRIPMINE).is_none() {
                        body(packets, ot, MINE, p.pos, n, 512, rot, base_t);
                    }
                    let lens = [
                        p.pos[0] + ((n[0] * 5) >> 12),
                        p.pos[1] + ((n[1] * 5) >> 12),
                        p.pos[2] + ((n[2] * 5) >> 12),
                    ];
                    let armed = p.life == 0 || p.life >= tripmine::FUSE_BASE;
                    let col = if armed { (255, 40, 30) } else { (160, 110, 20) };
                    glow(packets, ot, lens, 2, col, rot, base_t);
                } else {
                    glow(packets, ot, p.pos, 3, (255, 60, 40), rot, base_t);
                }
            }
            PROJ_SPIT | PROJ_BMORTAR => {
                glow(packets, ot, p.pos, 6, (150, 230, 80), rot, base_t);
            }
            PROJ_CBALL => glow(packets, ot, p.pos, 8, (130, 170, 255), rot, base_t),
            PROJ_HEADBALL => glow(packets, ot, p.pos, 14, (225, 230, 255), rot, base_t),
            _ => {}
        }
    }
    if RPG_SPOT_ACTIVE {
        // CLaserSpot: a small red glow where the designator lands, pulled off
        // the surface a unit so it sorts in front of it.
        glow(packets, ot, RPG_SPOT_POS, 5, (255, 40, 30), rot, base_t);
        sprite_quad(
            packets,
            ot,
            RPG_SPOT_POS,
            4,
            sprite::WPN_LASER_DOT,
            0,
            (255, 255, 255),
            rot,
            base_t,
        );
    }
}

/// The slot of the resident model of type `ty`, when this map has it.
unsafe fn resident(ty: u8) -> Option<usize> {
    let slot = TYPE_TO_SLOT[ty as usize];
    (slot != MODEL_SLOT_NONE && LOADED_MODELS[slot as usize].valid).then_some(slot as usize)
}

/// Draw the player's own snarks and placed tripmines from their models (the
/// types `stream_map_models` keeps resident on every map from the weapon's
/// first one). Returns the vertices projected. A snark heads the way it last
/// ran (its heading is kept in the slot's spare `remainder`), and runs while it
/// moves; a mine faces out of its wall.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn draw_bodies(
    packets: &mut ModelPacketWriter<'_>,
    eye: [i32; 3],
    rot: &Mat3I16,
    base_t: [i32; 3],
    now: u32,
    np: &mut usize,
) -> u32 {
    let mut projected = 0u32;
    let mut i = 0;
    while i < MAX_PROJECTILES {
        let p = PROJECTILES[i];
        i += 1;
        if !p.active {
            continue;
        }
        let mut tilt = 0u16;
        let (ty, yaw, clip) = match p.kind {
            PROJ_SNARK => {
                let moving = p.vel[0].abs() + p.vel[2].abs() > 1;
                (
                    PROP_TYPE_SNARK,
                    (p.remainder[0] as u8 as u16) << 4,
                    if moving { PROP_CLIP_MOVE } else { PROP_CLIP_IDLE },
                )
            }
            PROJ_TRIPMINE if p.vel[0].abs().max(p.vel[1].abs()).max(p.vel[2].abs()) > 512 => {
                // The mine faces out of its surface along the normal kept in `vel`:
                // yaw from its horizontal part, pitch up (floor) or down (ceiling)
                // from the vertical one, in the 256ths of a turn the model matrix takes.
                // The horizontal length is taken as |x| + |z| (exact on a wall or a floor, a
                // few degrees low on a diagonal slope), which spares a square root.
                tilt = (atan2_q12(p.vel[1], p.vel[0].abs() + p.vel[2].abs()) >> 4) & 0xff;
                (
                    tripmine::PROP_TYPE_TRIPMINE,
                    yaw_from_vec(p.vel[0], p.vel[2]),
                    PROP_CLIP_IDLE,
                )
            }
            _ => continue,
        };
        let Some(slot) = resident(ty) else {
            continue;
        };
        // Past the view or far behind it, nothing to draw.
        let rel = [p.pos[0] - eye[0], p.pos[1] - eye[1], p.pos[2] - eye[2]];
        let vz = dot12(rot.m[2], rel);
        if !(-24..=3000).contains(&vz) {
            continue;
        }
        let lm = LOADED_MODELS[slot];
        let md = loaded_model(slot);
        let clip = clip.min(md.clip_count().saturating_sub(1));
        let (sf, sf2, sfrac) = md.looped_clip_phase(clip, md.clip_hold_ticks(clip) as usize, now as usize);
        CROSSFADE = (0, 0, 0);
        projected = projected.saturating_add(draw_model(
            packets,
            md,
            &POOL_TEX[lm.tex_start..lm.tex_start + lm.n_tex],
            lm.face_indices,
            lm.face_payloads,
            core::ptr::addr_of!(POOL_FACE_RUNS)
                .cast::<u32>()
                .add(lm.run_start as usize),
            lm.n_runs as usize,
            lm.n_faces,
            p.pos,
            yaw,
            tilt,
            0,
            0,
            sf,
            sf2,
            sfrac,
            BlendMode::Opaque,
            MODEL_SHADE,
            eye,
            rot,
            np,
        ));
        CROSSFADE_BONES = 0;
        // HMD8 projection leaves the last bone affine loaded; the next body's
        // sphere test and every later draw need the camera again.
        scene::load_rotation(rot);
        scene::load_translation(Vec3I32::new(base_t[0], base_t[1], base_t[2]));
    }
    projected
}

/// The Gluon Gun's beam from `start` (the gun) to `end` (what it hit), after
/// `CEgon::UpdateEffect`: the `xbeam1` ribbon that wanders sideways along its
/// length (`SetNoise`), a narrower, brighter one inside it, a glow at the gun,
/// and the `XSpark1` flare where it lands. The blue pulses with time.
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
    let t = SIM_NOW as i32;
    let pulse = sincos::sin_q12(((t * 330) & 0x0fff) as u16).abs();
    let blue = (150 + ((105 * pulse) >> 12)) as u8;
    draw_beam_textured(
        packets, ot, start, end, 7, (50, 70, blue), sprite::WPN_XBEAM, 6, 3, 1, rot, base_t,
    );
    draw_beam_textured(
        packets, ot, start, end, 3, (190, 210, 255), sprite::WPN_XBEAM, 2, 5, 7, rot, base_t,
    );
    glow(packets, ot, start, 7, (70, 90, 255), rot, base_t);
    sprite_quad(
        packets,
        ot,
        end,
        11,
        sprite::WPN_XSPARK,
        (t / 2) as usize % 4,
        (200, 220, 255),
        rot,
        base_t,
    );
}

/// The Tau Cannon's beam (`EV_FireGauss`): the smoke sprite as a beam,
/// amber for the primary shot and white for a charged one, with a bright core
/// line, a hot glow at the muzzle and a bigger one where it lands.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn gauss_beam(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    start: [i32; 3],
    end: [i32; 3],
    charged: bool,
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    let (half, body, core, big) = if charged {
        (7, (255, 255, 255), (255, 255, 255), 14)
    } else {
        (4, (255, 140, 30), (255, 230, 150), 9)
    };
    draw_beam_textured(
        packets, ot, start, end, half, body, sprite::WPN_SMOKE, 0, 0, 3, rot, base_t,
    );
    draw_beam(packets, ot, start, end, half / 3, core, rot, base_t);
    glow(packets, ot, start, big / 2, body, rot, base_t);
    glow(packets, ot, end, big, body, rot, base_t);
}

/// A tripmine's laser, `laserbeam` in the SDK's 0,214,198, scrolling along.
#[inline(never)]
#[optimize(size)]
pub(crate) unsafe fn mine_beam(
    packets: &mut PrimitivePacketArena<'_>,
    ot: &mut OrderingTable<OT_LEN>,
    from: [i32; 3],
    end: [i32; 3],
    rot: &Mat3I16,
    base_t: [i32; 3],
) {
    draw_beam_textured(
        packets, ot, from, end, 1, (0, 214, 198), sprite::WPN_LASER_BEAM, 0, 8, 5, rot, base_t,
    );
    glow(packets, ot, end, 2, (0, 150, 140), rot, base_t);
}

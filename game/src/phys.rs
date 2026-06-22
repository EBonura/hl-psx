//! Player physics on the BSP clip hull -- a fixed-point port of Quake/GoldSrc
//! `SV_RecursiveHullCheck` + a slide-move. The clip hull (`hull1`) is the BSP
//! pre-expanded by the player box, so we trace the player ORIGIN as a point.
//!
//! All world units are i32; plane normals are i16 in 1.3.12 (×4096); fractions
//! along a move are Q0.12 (4096 = full). No floats (no FPU on the PS1).

use psx_math::sincos;

use crate::map::Map;

const SOLID: i16 = -2; // CONTENTS_SOLID
const GROUND_NY: i32 = 2867; // floor if plane normal Y > ~0.7 (×4096)

// Tunables (world units per frame). HL feel-ish; adjust from captures.
const GRAVITY: i32 = 12;
const MOVE_SPEED: i32 = 18;
const JUMP: i32 = 64;
const STEP_DOWN: i32 = 8; // ground probe depth

#[inline]
fn dot(n: [i16; 3], p: [i32; 3]) -> i64 {
    n[0] as i64 * p[0] as i64 + n[1] as i64 * p[1] as i64 + n[2] as i64 * p[2] as i64
}

struct Trace {
    frac: i32, // Q0.12 along the move (4096 = reached end)
    normal: [i32; 3], // hit plane normal (×4096)
    allsolid: bool,
    startsolid: bool,
}

fn point_contents(map: &Map, mut num: i16, p: [i32; 3]) -> i16 {
    let mut guard = 0;
    while num >= 0 {
        if num as usize >= map.n_clip || guard > 256 {
            return -1; // treat as empty on bad data
        }
        guard += 1;
        let cn = map.clipnode(num as usize);
        let t = (dot(cn.n, p) >> 12) as i32 - cn.dist;
        num = if t >= 0 { cn.c0 } else { cn.c1 };
    }
    num
}

fn recurse(map: &Map, num: i16, p1f: i32, p2f: i32, p1: [i32; 3], p2: [i32; 3], tr: &mut Trace, depth: u8) -> bool {
    if depth > 120 {
        return true;
    }
    if num < 0 {
        if num != SOLID {
            tr.allsolid = false;
        } else {
            tr.startsolid = true;
        }
        return true; // empty subtree -> no impact
    }
    if num as usize >= map.n_clip {
        tr.allsolid = false;
        return true;
    }
    let cn = map.clipnode(num as usize);
    let t1 = (dot(cn.n, p1) >> 12) as i32 - cn.dist;
    let t2 = (dot(cn.n, p2) >> 12) as i32 - cn.dist;
    if t1 >= 0 && t2 >= 0 {
        return recurse(map, cn.c0, p1f, p2f, p1, p2, tr, depth + 1);
    }
    if t1 < 0 && t2 < 0 {
        return recurse(map, cn.c1, p1f, p2f, p1, p2, tr, depth + 1);
    }
    // Crosses the plane -- split the segment.
    let denom = (t1 - t2) as i64;
    let frac = if denom == 0 { 0 } else { ((t1 as i64 * 4096) / denom).clamp(0, 4096) as i32 };
    let midf = p1f + (((p2f - p1f) as i64 * frac as i64) >> 12) as i32;
    let mid = [
        p1[0] + (((p2[0] - p1[0]) as i64 * frac as i64) >> 12) as i32,
        p1[1] + (((p2[1] - p1[1]) as i64 * frac as i64) >> 12) as i32,
        p1[2] + (((p2[2] - p1[2]) as i64 * frac as i64) >> 12) as i32,
    ];
    let side = t1 < 0; // true -> back side first
    let (near, far) = if side { (cn.c1, cn.c0) } else { (cn.c0, cn.c1) };
    if !recurse(map, near, p1f, midf, p1, mid, tr, depth + 1) {
        return false;
    }
    if point_contents(map, far, mid) != SOLID {
        return recurse(map, far, midf, p2f, mid, p2, tr, depth + 1);
    }
    if tr.allsolid {
        return false;
    }
    // Impact: the far side is solid at the split point.
    tr.normal = if side {
        [-(cn.n[0] as i32), -(cn.n[1] as i32), -(cn.n[2] as i32)]
    } else {
        [cn.n[0] as i32, cn.n[1] as i32, cn.n[2] as i32]
    };
    tr.frac = midf;
    false
}

fn trace(map: &Map, head: i32, p1: [i32; 3], p2: [i32; 3]) -> Trace {
    let mut tr = Trace { frac: 4096, normal: [0, 0, 0], allsolid: true, startsolid: false };
    recurse(map, head as i16, 0, 4096, p1, p2, &mut tr, 0);
    tr
}

/// Remove the component of `v` along `n` (×4096) -- slide along a plane.
fn clip(v: [i32; 3], n: [i32; 3]) -> [i32; 3] {
    let proj = ((v[0] as i64 * n[0] as i64 + v[1] as i64 * n[1] as i64 + v[2] as i64 * n[2] as i64) >> 12) as i32;
    [
        v[0] - ((n[0] as i64 * proj as i64) >> 12) as i32,
        v[1] - ((n[1] as i64 * proj as i64) >> 12) as i32,
        v[2] - ((n[2] as i64 * proj as i64) >> 12) as i32,
    ]
}

pub struct Player {
    pub pos: [i32; 3],
    pub vel: [i32; 3],
    pub on_ground: bool,
}

impl Player {
    pub fn new(pos: [i32; 3]) -> Player {
        Player { pos, vel: [0, 0, 0], on_ground: false }
    }

    /// Advance the player one frame. `fwd`/`strafe` are -1/0/1 relative to
    /// `yaw` (Q0.12); `jump` triggers when grounded.
    pub fn update(&mut self, map: &Map, fwd: i32, strafe: i32, jump: bool, yaw: u16) {
        // Forward = (sin yaw, 0, cos yaw); right = (cos yaw, 0, -sin yaw). Both
        // sin/cos are ×4096, so a unit wish direction stays ×4096.
        let s = sincos::sin_q12(yaw);
        let c = sincos::sin_q12((yaw + 1024) & 0xFFF);
        let wx = s * fwd + c * strafe;
        let wz = c * fwd - s * strafe;
        self.vel[0] = (wx * MOVE_SPEED) >> 12;
        self.vel[2] = (wz * MOVE_SPEED) >> 12;

        if self.on_ground {
            if self.vel[1] < 0 {
                self.vel[1] = 0;
            }
            if jump {
                self.vel[1] = JUMP;
                self.on_ground = false;
            }
        } else {
            self.vel[1] -= GRAVITY;
        }

        // Slide-move: trace, advance, clip the remainder to the hit plane, retry.
        let mut d = self.vel;
        let head = map.hull1_head;
        for _ in 0..4 {
            if d == [0, 0, 0] {
                break;
            }
            let end = [self.pos[0] + d[0], self.pos[1] + d[1], self.pos[2] + d[2]];
            let tr = trace(map, head, self.pos, end);
            if tr.startsolid {
                break; // already embedded -- don't shove deeper
            }
            self.pos = [
                self.pos[0] + ((d[0] as i64 * tr.frac as i64) >> 12) as i32,
                self.pos[1] + ((d[1] as i64 * tr.frac as i64) >> 12) as i32,
                self.pos[2] + ((d[2] as i64 * tr.frac as i64) >> 12) as i32,
            ];
            if tr.frac >= 4096 {
                break;
            }
            let rem = [end[0] - self.pos[0], end[1] - self.pos[1], end[2] - self.pos[2]];
            d = clip(rem, tr.normal);
            self.vel = clip(self.vel, tr.normal);
        }

        // Ground check: probe straight down a little.
        let down = [self.pos[0], self.pos[1] - STEP_DOWN, self.pos[2]];
        let g = trace(map, head, self.pos, down);
        self.on_ground = g.frac < 4096 && g.normal[1] > GROUND_NY;
        if self.on_ground {
            // Snap onto the floor and kill downward speed.
            self.pos[1] += ((down[1] - self.pos[1]) as i64 * g.frac as i64 >> 12) as i32;
            if self.vel[1] < 0 {
                self.vel[1] = 0;
            }
        }
    }
}

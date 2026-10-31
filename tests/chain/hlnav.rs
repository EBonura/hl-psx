//! Route authoring aid: a walk planner over a retail BSP's standing hull (hull 1) that prints
//! `hlroute` legs.
//!
//!     rustc --edition=2021 -O tests/chain/hlnav.rs -o /tmp/hlnav
//!     /tmp/hlnav MAP walk X Y Z GX GY [GX GY ...] [--models 2,3,4] [--maps DIR]
//!     /tmp/hlnav MAP hop  X Y Z GX GY [...]       [--hop 170] [--drop 140]
//!     /tmp/hlnav MAP floors X0 X1 Y0 Y1 [STEP]    (ascii floor map)
//!
//! `Z` is the player's origin height (`LOGIC_PLAYER_POS` up), the hull centre, not the floor. The maps
//! directory defaults to `$HL_DIR/maps` (the retail `valve` folder).
//!
//! Only the static world (model 0) and the brush models named by `--models` are solid, so doors,
//! lifts and func_walls that are not named count as open or absent; the legs still have to be tried on
//! the PS1 and the blockers (closed doors, ladders, buttons) found by running them. `walk` takes
//! 8-neighbour steps of at most 18 units of rise (stairs), plus jump edges of up to 40. `hop` takes
//! longer jump hops over gaps (crate to crate), up to `--hop` units across and `--drop` down.
//! The printed legs assume the view faces +x (yaw 1024), so the output starts with `@face 1018 left`.
//! Every printed leg is a closed-loop `@hold` on the player's position with a dead-band pair on the
//! minor axis, the pattern the chain routes use; paste them into a route and add checkpoints.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;

const STEP: f64 = 18.0;
const JUMP: f64 = 40.0;
const SLIDE: f64 = 40.0;
const POS: &str = "sym=LOGIC_PLAYER_POS size=4 signed=true";

type P3 = (f64, f64, f64);

/// The collision data of one BSP: planes, clip nodes and the hull-1 heads of the models in play.
struct World {
    planes: Vec<[f64; 4]>,
    clip: Vec<(i32, i16, i16)>,
    heads: Vec<i32>,
    zmin: f64,
    zmax: f64,
    floor_cache: RefCell<HashMap<(i32, i32), Vec<f64>>>,
}

fn le_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"))
}

fn le_f32(b: &[u8], o: usize) -> f64 {
    f32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes")) as f64
}

impl World {
    /// Reads `maps_dir/MAP.bsp`; `extra_models` adds those brush models' hulls to the solid set.
    fn load(maps_dir: &PathBuf, map: &str, extra_models: &[usize]) -> Result<World, String> {
        let path = maps_dir.join(format!("{map}.bsp"));
        let b = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let lump = |i: usize| (le_i32(&b, 4 + 8 * i) as usize, le_i32(&b, 8 + 8 * i) as usize);
        let (po, pl) = lump(1);
        let planes = (0..pl / 20)
            .map(|k| {
                let o = po + 20 * k;
                [le_f32(&b, o), le_f32(&b, o + 4), le_f32(&b, o + 8), le_f32(&b, o + 12)]
            })
            .collect();
        let (co, cl) = lump(9);
        let clip = (0..cl / 8)
            .map(|k| {
                let o = co + 8 * k;
                let c0 = i16::from_le_bytes([b[o + 4], b[o + 5]]);
                let c1 = i16::from_le_bytes([b[o + 6], b[o + 7]]);
                (le_i32(&b, o), c0, c1)
            })
            .collect();
        let (mo, ml) = lump(14);
        let model_count = ml / 64;
        // A model is 9 floats (mins, maxs, origin) then headnode[4] (index 1 is the standing hull).
        let head = |k: usize| le_i32(&b, mo + 64 * k + 36 + 4);
        let mut heads = vec![head(0)];
        for &m in extra_models {
            if m >= model_count {
                return Err(format!("{map} has no model *{m}"));
            }
            heads.push(head(m));
        }
        let (zmin, zmax) = (le_f32(&b, mo + 8) - 8.0, le_f32(&b, mo + 20) + 8.0);
        Ok(World { planes, clip, heads, zmin, zmax, floor_cache: RefCell::new(HashMap::new()) })
    }

    fn solid(&self, x: f64, y: f64, z: f64) -> bool {
        for &h in &self.heads {
            let mut num = h;
            while num >= 0 {
                let (pl, c0, c1) = self.clip[num as usize];
                let n = &self.planes[pl as usize];
                num = i32::from(if n[0] * x + n[1] * y + n[2] * z - n[3] >= 0.0 { c0 } else { c1 });
            }
            if num == -2 {
                return true;
            }
        }
        false
    }

    /// Hull origin heights at which a player can stand at (x, y), highest first.
    fn floors(&self, x: f64, y: f64) -> Vec<f64> {
        let key = (x.round() as i32, y.round() as i32);
        if let Some(v) = self.floor_cache.borrow().get(&key) {
            return v.clone();
        }
        let mut out = Vec::new();
        let mut z = self.zmax;
        let mut s = self.solid(x, y, z);
        while z > self.zmin {
            let z2 = z - 4.0;
            let s2 = self.solid(x, y, z2);
            if s2 && !s {
                let (mut lo, mut hi) = (z2, z);
                for _ in 0..4 {
                    let mid = (lo + hi) / 2.0;
                    if self.solid(x, y, mid) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                out.push((hi * 2.0).round() / 2.0);
            }
            z = z2;
            s = s2;
        }
        self.floor_cache.borrow_mut().insert(key, out.clone());
        out
    }

    fn seg_clear(&self, a: P3, b: P3) -> bool {
        let d = dist3(a, b);
        let n = ((d / 4.0) as i32).max(2);
        (1..n).all(|k| {
            let t = f64::from(k) / f64::from(n);
            !self.solid(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t + 0.5)
        })
    }

    /// Distance to the nearest solid along (dx, dy) from (x, y, z + 1), in steps of 4 up to `lim`.
    fn extent(&self, x: f64, y: f64, z: f64, dx: f64, dy: f64, lim: f64) -> f64 {
        let mut d = 0.0;
        while d < lim && !self.solid(x + dx * d, y + dy * d, z + 1.0) {
            d += 4.0;
        }
        d
    }

    fn clearance(&self, x: f64, y: f64, z: f64) -> f64 {
        [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)]
            .iter()
            .map(|&(dx, dy)| self.extent(x, y, z, dx, dy, 48.0))
            .fold(f64::MAX, f64::min)
    }

    /// Nudges a waypoint to the middle of a passage that is narrower than 96 on either axis.
    fn center(&self, p: P3) -> P3 {
        let (mut x, mut y, z) = p;
        for _ in 0..2 {
            let (l, r) = (self.extent(x, y, z, -1.0, 0.0, 48.0), self.extent(x, y, z, 1.0, 0.0, 48.0));
            if (l < 48.0 || r < 48.0) && !(l == 0.0 || r == 0.0) {
                x += (r - l) / 2.0;
            }
            let (b, t) = (self.extent(x, y, z, 0.0, -1.0, 48.0), self.extent(x, y, z, 0.0, 1.0, 48.0));
            if (b < 48.0 || t < 48.0) && !(b == 0.0 || t == 0.0) {
                y += (t - b) / 2.0;
            }
        }
        (x, y, z)
    }

    fn floor_under(&self, x: f64, y: f64, z: f64) -> bool {
        self.floors((x / 4.0).round() * 4.0, (y / 4.0).round() * 4.0).iter().any(|f| (f - z).abs() <= STEP)
    }
}

fn dist3(a: P3, b: P3) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

/// A search node: grid cell and floor height (half units, so it hashes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Node {
    cx: i32,
    cy: i32,
    z2: i32,
}

impl Node {
    fn z(self) -> f64 {
        f64::from(self.z2) / 2.0
    }
}

fn node(cx: i32, cy: i32, z: f64) -> Node {
    Node { cx, cy, z2: (z * 2.0).round() as i32 }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Walk,
    Hop { reach: i32, drop: i32 },
}

fn cell_size(mode: Mode) -> f64 {
    match mode {
        Mode::Walk => 16.0,
        Mode::Hop { .. } => 32.0,
    }
}

/// Edges out of `n`: (next node, cost).
fn neighbours(w: &World, mode: Mode, n: Node) -> Vec<(Node, f64)> {
    let cell = cell_size(mode);
    let (x, y, z) = (f64::from(n.cx) * cell, f64::from(n.cy) * cell, n.z());
    let mut out = Vec::new();
    match mode {
        Mode::Walk => {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (n.cx + dx, n.cy + dy);
                    let tgt_xy = (f64::from(nx) * cell, f64::from(ny) * cell);
                    for fz in w.floors(tgt_xy.0, tgt_xy.1) {
                        let dz = fz - z;
                        if dz > JUMP || dz < -60.0 {
                            continue;
                        }
                        let top = z.max(fz) + 1.0;
                        let tgt = (tgt_xy.0, tgt_xy.1, fz + 1.0);
                        let step = f64::from(dx).hypot(f64::from(dy)) * cell;
                        if dz > STEP {
                            // A jump: rise first, then move.
                            if w.solid(x, y, z + dz + 1.0) || !w.seg_clear((x, y, z + dz + 1.0), tgt) {
                                continue;
                            }
                            out.push((node(nx, ny, fz), step + 40.0));
                        } else {
                            if !w.seg_clear((x, y, z + 1.0), (tgt.0, tgt.1, top)) || !w.seg_clear((x, y, top), tgt) {
                                continue;
                            }
                            out.push((node(nx, ny, fz), step + dz.abs() * 0.5));
                        }
                    }
                }
            }
        }
        Mode::Hop { reach, drop } => {
            let r = (f64::from(reach) / cell) as i32 + 1;
            for dx in -r..=r {
                for dy in -r..=r {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let d = f64::from(dx).hypot(f64::from(dy)) * cell;
                    if d > f64::from(reach) {
                        continue;
                    }
                    let (nx, ny) = (n.cx + dx, n.cy + dy);
                    let tgt_xy = (f64::from(nx) * cell, f64::from(ny) * cell);
                    for fz in w.floors(tgt_xy.0, tgt_xy.1) {
                        let dz = fz - z;
                        if fz <= -4000.0 || dz > JUMP || dz < -f64::from(drop) {
                            continue;
                        }
                        let walk = d <= cell * 1.5 && dz.abs() <= STEP;
                        let h = if walk {
                            z.max(fz) + 2.0
                        } else {
                            z.max(fz) + 12.0 + if dz > 0.0 { 45.0 } else { 20.0 }
                        };
                        let steps = ((d / 8.0) as i32).max(2);
                        let clear = (1..steps).all(|k| {
                            let t = f64::from(k) / f64::from(steps);
                            !w.solid(x + (tgt_xy.0 - x) * t, y + (tgt_xy.1 - y) * t, h)
                        });
                        if clear {
                            out.push((node(nx, ny, fz), if walk { d } else { d * 1.5 + 60.0 }));
                        }
                    }
                }
            }
        }
    }
    out
}

#[derive(PartialEq)]
struct Item(f64, Node);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Item {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.partial_cmp(&self.0).unwrap_or(Ordering::Equal)
    }
}

/// A* from the start to any floor in the goal's cell. `Err` carries the nearest cell reached.
fn plan(w: &World, mode: Mode, start: P3, goal: (f64, f64)) -> Result<Vec<Node>, (P3, f64)> {
    let cell = cell_size(mode);
    // The start cell is the nearest one (of the 3x3 around the start) with a floor at the start's height,
    // so a start that rounds into a different storey's cell still plans from the right floor.
    let (c0x, c0y) = ((start.0 / cell).round() as i32, (start.1 / cell).round() as i32);
    let mut snode = node(c0x, c0y, start.2);
    let mut best_key = (f64::MAX, f64::MAX);
    for cx in c0x - 1..=c0x + 1 {
        for cy in c0y - 1..=c0y + 1 {
            for fz in w.floors(f64::from(cx) * cell, f64::from(cy) * cell) {
                let key = ((fz - start.2).abs(), (f64::from(cx) * cell - start.0).hypot(f64::from(cy) * cell - start.1));
                if key.0 + 0.01 * key.1 < best_key.0 + 0.01 * best_key.1 {
                    best_key = key;
                    snode = node(cx, cy, fz);
                }
            }
        }
    }
    let (gx, gy) = ((goal.0 / cell).round() as i32, (goal.1 / cell).round() as i32);
    let mut best: HashMap<Node, f64> = HashMap::from([(snode, 0.0)]);
    let mut prev: HashMap<Node, Node> = HashMap::new();
    let mut heap = BinaryHeap::from([Item(0.0, snode)]);
    let mut near = (f64::MAX, snode);
    while let Some(Item(_, cur)) = heap.pop() {
        if (cur.cx, cur.cy) == (gx, gy) {
            let mut path = vec![cur];
            let mut at = cur;
            while let Some(&p) = prev.get(&at) {
                path.push(p);
                at = p;
            }
            path.reverse();
            return Ok(path);
        }
        let base = best[&cur];
        for (nb, cost) in neighbours(w, mode, cur) {
            let nc = base + cost;
            if nc < best.get(&nb).copied().unwrap_or(f64::MAX) {
                best.insert(nb, nc);
                prev.insert(nb, cur);
                let h = f64::from(nb.cx - gx).hypot(f64::from(nb.cy - gy)) * cell;
                if h < near.0 {
                    near = (h, nb);
                }
                heap.push(Item(nc + h, nb));
            }
        }
    }
    Err(((f64::from(near.1.cx) * cell, f64::from(near.1.cy) * cell, near.1.z()), near.0))
}

/// Drops waypoints that a straight, level, floored segment already covers (walk paths only).
fn simplify(w: &World, mode: Mode, path: &[Node]) -> Vec<P3> {
    let cell = cell_size(mode);
    let pts: Vec<P3> = path.iter().map(|n| (f64::from(n.cx) * cell, f64::from(n.cy) * cell, n.z())).collect();
    if matches!(mode, Mode::Hop { .. }) {
        return pts;
    }
    let mut out = vec![pts[0]];
    let mut i = 0;
    while i + 1 < pts.len() {
        let mut j = pts.len() - 1;
        while j > i + 1 {
            let flat = (i..=j).all(|k| (pts[k].2 - pts[i].2).abs() <= STEP + 0.1);
            let (a, b) = (pts[i], pts[j]);
            if flat
                && w.seg_clear((a.0, a.1, a.2 + 1.0), (b.0, b.1, b.2 + 1.0))
                && (1..20).all(|t| {
                    let f = f64::from(t) / 20.0;
                    w.floor_under(a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, a.2 + (b.2 - a.2) * f)
                })
            {
                break;
            }
            j -= 1;
        }
        out.push(pts[j]);
        i = j;
    }
    out
}

/// Adds intermediate waypoints along segments that squeeze through a narrow passage.
fn densify(w: &World, wps: &[P3]) -> Vec<P3> {
    let mut out = vec![wps[0]];
    for pair in wps.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let d = (b.0 - a.0).hypot(b.1 - a.1);
        let n = (d / 48.0) as i32;
        let at = |k: i32| {
            let t = f64::from(k) / f64::from(n + 1);
            (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t)
        };
        if (1..=n).any(|k| {
            let q = at(k);
            w.clearance(q.0, q.1, q.2) < 36.0
        }) {
            out.extend((1..=n).map(at));
        }
        out.push(b);
    }
    out
}

/// One `@hold` toward each waypoint (closed loop on the position) plus a dead-band pair on the minor axis.
fn legs(w: &World, wps: &[P3]) -> Vec<String> {
    let mut out = Vec::new();
    let last = wps.len() - 2;
    for (i, pair) in wps.windows(2).enumerate() {
        let ((x0, y0, _), (x1, y1, z1)) = (pair[0], pair[1]);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let d = dx.hypot(dy);
        if d < 1.0 {
            continue;
        }
        let f = (127.0 * dx / d).round() as i32;
        let s = (-127.0 * dy / d).round() as i32;
        let mut toks = Vec::new();
        if f != 0 {
            toks.push(format!("f{f}"));
        }
        if s != 0 {
            toks.push(format!("s{s}"));
        }
        let comp = (if i == last { SLIDE } else { 12.0 }).min(d * 0.5);
        let (off, op, val) = if dx.abs() >= dy.abs() {
            (0, if dx > 0.0 { "ge" } else { "le" }, x1 - (comp * dx.abs() / d).copysign(dx))
        } else {
            (8, if dy > 0.0 { "ge" } else { "le" }, y1 - (comp * dy.abs() / d).copysign(dy))
        };
        out.push(format!(
            "  @hold {} until {POS} offset={off} op={op} value={} timeout={}",
            toks.join(" "),
            val.round() as i32,
            40 + (d / 8.0) as i32
        ));
        // Minor axis: a dead-band pair (a hold whose condition already holds ends at once).
        let (moff, mt) = if off == 0 { (8, y1) } else { (0, x1) };
        let tol = if i == last {
            12.0
        } else if w.clearance(x1, y1, z1) < 36.0 {
            14.0
        } else {
            28.0
        };
        let (up, dn) = if moff == 8 { ("s-70", "s70") } else { ("f70", "f-70") };
        out.push(format!("  @hold {up} until {POS} offset={moff} op=ge value={} timeout=40", (mt - tol).round() as i32));
        out.push(format!("  @hold {dn} until {POS} offset={moff} op=le value={} timeout=40", (mt + tol).round() as i32));
    }
    out
}

fn floor_map(w: &World, x0: i32, x1: i32, y0: i32, y1: i32, step: i32) {
    println!("# '#' floor, '.' none; origin heights shown as the highest floor modulo 10 is not printed, use walk for heights");
    let mut y = y1;
    while y >= y0 {
        let row: String = (0..)
            .map(|k| x0 + k * step)
            .take_while(|&x| x <= x1)
            .map(|x| match w.floors(f64::from(x), f64::from(y)).len() {
                0 => '.',
                1 => '#',
                _ => '2',
            })
            .collect();
        println!("{y:6} {row}");
        y -= step;
    }
    println!("# x from {x0} to {x1} step {step} ('2' = more than one floor)");
}

fn parse_args() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: hlnav MAP walk|hop X Y Z GX GY [GX GY ...] [--models 2,3] [--hop N] [--drop N] [--maps DIR] | hlnav MAP floors X0 X1 Y0 Y1 [STEP]";
    if args.len() < 3 {
        return Err(usage.into());
    }
    let (mut pos, mut models, mut reach, mut drop) = (Vec::new(), Vec::new(), 170, 140);
    let mut maps = std::env::var("HL_DIR").map(|d| PathBuf::from(d).join("maps")).unwrap_or_default();
    let mut it = args[2..].iter();
    let flag = |v: Option<&String>| v.cloned().ok_or(usage.to_string());
    while let Some(a) = it.next() {
        match a.as_str() {
            "--models" => {
                models = flag(it.next())?.split(',').filter_map(|m| m.parse().ok()).collect();
            }
            "--hop" => reach = flag(it.next())?.parse().map_err(|_| "bad --hop")?,
            "--drop" => drop = flag(it.next())?.parse().map_err(|_| "bad --drop")?,
            "--maps" => maps = PathBuf::from(flag(it.next())?),
            n => pos.push(n.parse::<f64>().map_err(|_| format!("bad number {n}"))?),
        }
    }
    let w = World::load(&maps, &args[0], &models)?;
    match args[1].as_str() {
        "floors" if pos.len() >= 4 => {
            let step = pos.get(4).map_or(16, |&s| s as i32);
            floor_map(&w, pos[0] as i32, pos[1] as i32, pos[2] as i32, pos[3] as i32, step);
            Ok(())
        }
        mode_name @ ("walk" | "hop") if pos.len() >= 5 && pos.len() % 2 == 1 => {
            let mode = if mode_name == "walk" { Mode::Walk } else { Mode::Hop { reach, drop } };
            let mut cur = (pos[0], pos[1], pos[2]);
            let mut all = vec![cur];
            for g in pos[3..].chunks(2) {
                let path = plan(&w, mode, cur, (g[0], g[1])).map_err(|(at, dist)| {
                    format!("no path to ({}, {}); nearest reached ({}, {}, {}) at {} units", g[0], g[1], at.0, at.1, at.2, dist.round())
                })?;
                let wp = simplify(&w, mode, &path);
                all.extend(wp[1..].iter().map(|&q| if mode == Mode::Walk { w.center(q) } else { q }));
                cur = *wp.last().expect("a path has a waypoint");
            }
            // The leg sticks are written for a view facing +x (yaw 1024): forward is +x, strafe right is -y.
            println!("  @face 1018 left");
            for l in legs(&w, &densify(&w, &all)) {
                println!("{l}");
            }
            let list: Vec<String> = all.iter().map(|p| format!("({},{},{})", p.0 as i32, p.1 as i32, p.2)).collect();
            println!("# waypoints: {}", list.join(" "));
            Ok(())
        }
        _ => Err(usage.into()),
    }
}

fn main() {
    if let Err(e) = parse_args() {
        eprintln!("hlnav: {e}");
        std::process::exit(1);
    }
}

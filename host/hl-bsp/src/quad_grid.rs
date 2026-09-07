//! Preserve rectangular source patches while inserting boundary junctions.
//! Cuts propagate to every rectangular neighbour before any face is emitted.
use super::{push_refined_tri, RefineCorner, MAX_COOK_VERTS};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

const MAX_CELLS: usize = 8;

#[derive(Clone)]
struct Plan {
    face: usize,
    axes: [usize; 2],
    lo: [i16; 3],
    hi: [i16; 3],
    cuts: [BTreeSet<i16>; 2],
    corners: [RefineCorner; 4], // low/low, high/low, low/high, high/high
    reverse: bool,
    tex: u16,
    extra_cuts: Vec<(usize, i16)>,
}

fn plan(
    face: usize,
    verts: &[[i16; 3]],
    idx: &[u16],
    uv: &[u8],
    rgb: &[u8],
    tex: u16,
) -> Option<Plan> {
    let mut corners = BTreeMap::new();
    for k in 0..6 {
        let c = RefineCorner {
            idx: idx[k],
            uv: [uv[k * 2], uv[k * 2 + 1]],
            rgb: [rgb[k * 3], rgb[k * 3 + 1], rgb[k * 3 + 2]],
        };
        let p = *verts.get(c.idx as usize)?;
        if let Some(old) = corners.insert(p, c) {
            if old.uv != c.uv || old.rgb != c.rgb {
                return None;
            }
        }
    }
    if corners.len() != 4 {
        return None;
    }
    let mut lo = [i16::MAX; 3];
    let mut hi = [i16::MIN; 3];
    for p in corners.keys() {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    let axes: Vec<_> = (0..3).filter(|&a| lo[a] != hi[a]).collect();
    if axes.len() != 2 || !axes.contains(&1) {
        return None;
    }
    let axes = [axes[0], axes[1]];
    let mut ordered = [*corners.values().next()?; 4];
    for k in 0..4 {
        let mut p = lo;
        if k & 1 != 0 {
            p[axes[0]] = hi[axes[0]];
        }
        if k & 2 != 0 {
            p[axes[1]] = hi[axes[1]];
        }
        ordered[k] = *corners.get(&p)?;
    }
    // UV discontinuities and non-affine mappings retain their authored triangles.
    for a in 0..2 {
        if ordered[0].uv[a] as i32 + ordered[3].uv[a] as i32
            != ordered[1].uv[a] as i32 + ordered[2].uv[a] as i32
        {
            return None;
        }
    }
    let cross = |t: usize| {
        let a = verts[idx[t] as usize];
        let b = verts[idx[t + 1] as usize];
        let c = verts[idx[t + 2] as usize];
        (b[axes[0]] as i64 - a[axes[0]] as i64) * (c[axes[1]] as i64 - a[axes[1]] as i64)
            - (b[axes[1]] as i64 - a[axes[1]] as i64) * (c[axes[0]] as i64 - a[axes[0]] as i64)
    };
    let area =
        (hi[axes[0]] as i64 - lo[axes[0]] as i64) * (hi[axes[1]] as i64 - lo[axes[1]] as i64);
    let a = cross(0);
    let b = cross(3);
    if a.abs() != area || b != a {
        return None;
    }
    // Both source halves must share the rectangle's diagonal, not overlap.
    let shared: Vec<_> = idx[..3].iter().filter(|i| idx[3..].contains(i)).collect();
    if shared.len() != 2 {
        return None;
    }
    let p = verts[*shared[0] as usize];
    let q = verts[*shared[1] as usize];
    if p[axes[0]] == q[axes[0]] || p[axes[1]] == q[axes[1]] {
        return None;
    }
    Some(Plan {
        face,
        axes,
        lo,
        hi,
        cuts: [
            BTreeSet::from([lo[axes[0]], hi[axes[0]]]),
            BTreeSet::from([lo[axes[1]], hi[axes[1]]]),
        ],
        corners: ordered,
        reverse: a < 0,
        tex,
        extra_cuts: Vec::new(),
    })
}

// A line is keyed by its varying world axis and its two fixed coordinates.
fn line(p: [i16; 3], axis: usize) -> (usize, i16, i16) {
    (axis, p[(axis + 1) % 3], p[(axis + 2) % 3])
}

fn close(plans: &mut [Plan], verts: &[[i16; 3]]) -> bool {
    for p in plans.iter_mut() {
        for a in 0..2 {
            p.cuts[a] = BTreeSet::from([p.lo[p.axes[a]], p.hi[p.axes[a]]]);
        }
    }
    let mut lines: BTreeMap<_, Vec<(usize, usize)>> = BTreeMap::new();
    for (i, p) in plans.iter().enumerate() {
        for a in 0..2 {
            for end in [p.lo[p.axes[1 - a]], p.hi[p.axes[1 - a]]] {
                let mut point = p.lo;
                point[p.axes[1 - a]] = end;
                lines
                    .entry(line(point, p.axes[a]))
                    .or_default()
                    .push((i, a));
            }
        }
    }
    let mut queue: VecDeque<_> = verts.iter().copied().collect();
    for p in plans.iter() {
        for &(a, cut) in &p.extra_cuts {
            for end in [p.lo[p.axes[1 - a]], p.hi[p.axes[1 - a]]] {
                let mut point = p.lo;
                point[p.axes[a]] = cut;
                point[p.axes[1 - a]] = end;
                queue.push_back(point);
            }
        }
    }
    while let Some(point) = queue.pop_front() {
        for axis in 0..3 {
            if let Some(owners) = lines.get(&line(point, axis)) {
                for &(i, a) in owners {
                    let p = &mut plans[i];
                    let cut = point[axis];
                    if cut <= p.lo[axis] || cut >= p.hi[axis] || !p.cuts[a].insert(cut) {
                        continue;
                    }
                    if (p.cuts[0].len() - 1) * (p.cuts[1].len() - 1) > MAX_CELLS {
                        return false;
                    }

                    for end in [p.lo[p.axes[1 - a]], p.hi[p.axes[1 - a]]] {
                        let mut generated = p.lo;
                        generated[axis] = cut;
                        generated[p.axes[1 - a]] = end;
                        queue.push_back(generated);
                    }
                }
            }
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) fn refine(
    verts: &mut Vec<[i16; 3]>,
    idx: &mut Vec<u16>,
    tex: &mut Vec<u16>,
    uv: &mut Vec<u8>,
    rgb: &mut Vec<u8>,
    first: &mut [u32],
    count: &mut [u16],
    native: &[bool],
    refined: &mut [bool],
    pairs: &mut HashSet<usize>,
    max_added: usize,
) -> usize {
    let mut plans = Vec::new();
    for f in 0..first.len() {
        let t = first[f] as usize;
        if native[f] && !refined[f] && count[f] == 2 && tex[t] == tex[t + 1] {
            if let Some(p) = plan(
                f,
                verts,
                &idx[t * 3..t * 3 + 6],
                &uv[t * 6..t * 6 + 12],
                &rgb[t * 9..t * 9 + 18],
                tex[t],
            ) {
                plans.push(p);
            }
        }
    }
    // Admit only rectangles whose existing boundary constraints fit the cap.
    // Do this before propagation so a dense neighbouring face cannot exclude a
    // small patch which can stay a grid while that neighbour uses welding.
    for p in &mut plans {
        for point in verts.iter() {
            if (0..3).any(|a| point[a] < p.lo[a] || point[a] > p.hi[a]) {
                continue;
            }
            for a in 0..2 {
                let other = p.axes[1 - a];
                if point[other] == p.lo[other] || point[other] == p.hi[other] {
                    p.cuts[a].insert(point[p.axes[a]]);
                }
            }
        }
    }

    // Spend at most eight cells on an already constrained wall. A horizontal
    // cut anchors vertical texture variation (painted bands otherwise form a
    // saw-tooth across tall columns); remaining cuts shorten the broad axis.
    for p in &mut plans {
        let a = usize::from(p.axes[0] == 1);
        let mut horizontal = p.cuts[a].clone();
        if (p.cuts[0].len() - 1) * (p.cuts[1].len() - 1) == 1 {
            continue;
        }
        let rows = if p.cuts[1 - a].len() == 2 {
            let cut = ((p.lo[p.axes[1 - a]] as i32 + p.hi[p.axes[1 - a]] as i32) / 2) as i16;
            if cut > p.lo[p.axes[1 - a]] && cut < p.hi[p.axes[1 - a]] {
                p.extra_cuts.push((1 - a, cut));
                2
            } else {
                1
            }
        } else {
            p.cuts[1 - a].len() - 1
        };
        while horizontal.len() * rows <= MAX_CELLS {
            let points: Vec<_> = horizontal.iter().copied().collect();
            let pair = points
                .windows(2)
                .max_by_key(|w| w[1] as i32 - w[0] as i32)
                .unwrap();
            let cut = ((pair[0] as i32 + pair[1] as i32) / 2) as i16;
            if cut <= pair[0] || cut >= pair[1] {
                break;
            }
            horizontal.insert(cut);
            p.extra_cuts.push((a, cut));
        }
    }
    plans.retain(|p| (p.cuts[0].len() - 1) * (p.cuts[1].len() - 1) <= MAX_CELLS);
    // Admit patches by world area per grid cell. A later patch is
    // declined if its propagated cuts would over-refine an already admitted
    // neighbour. This preserves small useful grids without global fan-out.
    plans.sort_by_key(|p| {
        let cells = (p.cuts[0].len() - 1) * (p.cuts[1].len() - 1);
        let area = (p.hi[p.axes[0]] as i64 - p.lo[p.axes[0]] as i64)
            * (p.hi[p.axes[1]] as i64 - p.lo[p.axes[1]] as i64);
        (std::cmp::Reverse(area / cells as i64), p.face)
    });
    // Existing UV grids retain their own ownership and bypass later welding.
    // Never introduce a fresh junction into one of their boundary segments.
    let existing: BTreeSet<_> = verts.iter().copied().collect();
    let mut protected = Vec::new();
    for f in 0..first.len() {
        if !refined[f] {
            continue;
        }
        let mut edges = BTreeMap::new();
        for t in first[f] as usize..first[f] as usize + count[f] as usize {
            for e in 0..3 {
                let a = verts[idx[t * 3 + e] as usize];
                let b = verts[idx[t * 3 + (e + 1) % 3] as usize];
                *edges
                    .entry(if a < b { (a, b) } else { (b, a) })
                    .or_insert(0usize) += 1;
            }
        }
        protected.extend(edges.into_iter().filter_map(|(e, n)| (n == 1).then_some(e)));
    }
    let protected_cut = |plans: &[Plan]| {
        plans.iter().any(|p| {
            p.cuts[0].iter().any(|&x| {
                p.cuts[1].iter().any(|&y| {
                    let mut q = p.lo;
                    q[p.axes[0]] = x;
                    q[p.axes[1]] = y;
                    if existing.contains(&q) {
                        return false;
                    }
                    protected.iter().any(|&(a, b)| {
                        if (0..3).any(|i| q[i] < a[i].min(b[i]) || q[i] > a[i].max(b[i]))
                            || q == a
                            || q == b
                        {
                            return false;
                        }
                        let d = [
                            b[0] as i64 - a[0] as i64,
                            b[1] as i64 - a[1] as i64,
                            b[2] as i64 - a[2] as i64,
                        ];
                        let v = [
                            q[0] as i64 - a[0] as i64,
                            q[1] as i64 - a[1] as i64,
                            q[2] as i64 - a[2] as i64,
                        ];
                        d[0] * v[1] == d[1] * v[0]
                            && d[0] * v[2] == d[2] * v[0]
                            && d[1] * v[2] == d[2] * v[1]
                    })
                })
            })
        })
    };
    let mut accepted = Vec::new();
    for p in plans {
        let mut trial = accepted.clone();
        trial.push(p);
        let fits = close(&mut trial, verts) && {
            let added: usize = trial
                .iter()
                .map(|p| 2 * ((p.cuts[0].len() - 1) * (p.cuts[1].len() - 1) - 1))
                .sum();
            let vertices: usize = trial
                .iter()
                .map(|p| p.cuts[0].len() * p.cuts[1].len() - 4)
                .sum();
            added + (vertices * 6 + 18) / 19 <= max_added
                && verts.len() + vertices <= MAX_COOK_VERTS
        };
        if fits && !protected_cut(&trial) {
            accepted = trial;
        }
    }
    let mut plans = accepted;
    plans.retain(|p| p.cuts[0].len() > 2 || p.cuts[1].len() > 2);
    let added: usize = plans
        .iter()
        .map(|p| 2 * ((p.cuts[0].len() - 1) * (p.cuts[1].len() - 1) - 1))
        .sum();
    let existing: BTreeSet<_> = verts.iter().copied().collect();
    let mut generated = BTreeSet::new();
    for p in &plans {
        for &y in &p.cuts[1] {
            for &x in &p.cuts[0] {
                let mut point = p.lo;
                point[p.axes[0]] = x;
                point[p.axes[1]] = y;
                if !existing.contains(&point) {
                    generated.insert(point);
                }
            }
        }
    }
    let upper_new = generated.len();
    eprintln!(
        "quad grid plan: {} faces, +{} triangles, {} new vertices, {} triangle budget",
        plans.len(),
        added,
        upper_new,
        max_added
    );
    if added == 0
        || added.saturating_add((upper_new * 6 + 18) / 19) > max_added
        || verts.len() + upper_new > MAX_COOK_VERTS
    {
        return 0;
    }
    let mut positions: BTreeMap<_, _> = BTreeMap::new();
    for (i, &p) in verts.iter().enumerate() {
        positions.entry(p).or_insert(i as u16);
    }
    let mut by_face = plans
        .into_iter()
        .map(|p| (p.face, p))
        .collect::<BTreeMap<_, _>>();
    let mut ni = Vec::new();
    let mut nt = Vec::new();
    let mut nu = Vec::new();
    let mut nr = Vec::new();
    let mut np = HashSet::new();
    for f in 0..first.len() {
        let start = first[f] as usize;
        let n = count[f] as usize;
        first[f] = nt.len() as u32;
        if let Some(p) = by_face.remove(&f) {
            let xs: Vec<_> = p.cuts[0].iter().copied().collect();
            let ys: Vec<_> = p.cuts[1].iter().copied().collect();
            let width = p.hi[p.axes[0]] as i64 - p.lo[p.axes[0]] as i64;
            let height = p.hi[p.axes[1]] as i64 - p.lo[p.axes[1]] as i64;
            let den = width * height;
            let mut grid = Vec::new();
            for &y in &ys {
                for &x in &xs {
                    let mut pos = p.lo;
                    pos[p.axes[0]] = x;
                    pos[p.axes[1]] = y;
                    let vi = *positions.entry(pos).or_insert_with(|| {
                        let i = verts.len() as u16;
                        verts.push(pos);
                        i
                    });
                    let dx = x as i64 - p.lo[p.axes[0]] as i64;
                    let dy = y as i64 - p.lo[p.axes[1]] as i64;
                    let weights = [
                        (width - dx) * (height - dy),
                        dx * (height - dy),
                        (width - dx) * dy,
                        dx * dy,
                    ];
                    let interp = |values: [u8; 4]| -> u8 {
                        ((values
                            .iter()
                            .zip(weights)
                            .map(|(&v, w)| v as i64 * w)
                            .sum::<i64>()
                            + den / 2)
                            / den) as u8
                    };
                    grid.push(RefineCorner {
                        idx: vi,
                        uv: [
                            interp(p.corners.map(|c| c.uv[0])),
                            interp(p.corners.map(|c| c.uv[1])),
                        ],
                        rgb: [
                            interp(p.corners.map(|c| c.rgb[0])),
                            interp(p.corners.map(|c| c.rgb[1])),
                            interp(p.corners.map(|c| c.rgb[2])),
                        ],
                    });
                }
            }
            for y in 0..ys.len() - 1 {
                for x in 0..xs.len() - 1 {
                    let k = y * xs.len() + x;
                    let q = [
                        grid[k],
                        grid[k + 1],
                        grid[k + xs.len()],
                        grid[k + xs.len() + 1],
                    ];
                    np.insert(nt.len());
                    for mut t in [[q[0], q[1], q[2]], [q[1], q[3], q[2]]] {
                        if p.reverse {
                            t.swap(1, 2);
                        }
                        push_refined_tri(t, p.tex, &mut ni, &mut nt, &mut nu, &mut nr);
                    }
                }
            }
            refined[f] = true;
            count[f] = (nt.len() - first[f] as usize) as u16;
        } else {
            for t in start..start + n {
                if pairs.contains(&t) {
                    np.insert(nt.len());
                }
                ni.extend_from_slice(&idx[t * 3..t * 3 + 3]);
                nt.push(tex[t]);
                nu.extend_from_slice(&uv[t * 6..t * 6 + 6]);
                nr.extend_from_slice(&rgb[t * 9..t * 9 + 9]);
            }
        }
    }
    *idx = ni;
    *tex = nt;
    *uv = nu;
    *rgb = nr;
    *pairs = np;
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct Mesh {
        v: Vec<[i16; 3]>,
        i: Vec<u16>,
        t: Vec<u16>,
        u: Vec<u8>,
        r: Vec<u8>,
        f: Vec<u32>,
        n: Vec<u16>,
        refined: Vec<bool>,
        pairs: HashSet<usize>,
    }
    impl Mesh {
        fn new() -> Self {
            Self {
                v: vec![],
                i: vec![],
                t: vec![],
                u: vec![],
                r: vec![],
                f: vec![],
                n: vec![],
                refined: vec![],
                pairs: HashSet::new(),
            }
        }
        fn rect(&mut self, x0: i16, x1: i16, y0: i16, y1: i16) {
            let base = self.v.len() as u16;
            self.v
                .extend([[x0, y0, 0], [x1, y0, 0], [x0, y1, 0], [x1, y1, 0]]);
            self.f.push(self.t.len() as u32);
            self.n.push(2);
            self.refined.push(false);
            for k in [0u16, 1, 2, 1, 3, 2] {
                self.i.push(base + k);
                self.u.extend([
                    if k & 1 == 0 { 0 } else { 128 },
                    if k & 2 == 0 { 0 } else { 128 },
                ]);
                self.r.extend([64, 96, 128]);
            }
            self.t.extend([0, 0]);
        }
        fn run(&mut self, budget: usize) -> usize {
            refine(
                &mut self.v,
                &mut self.i,
                &mut self.t,
                &mut self.u,
                &mut self.r,
                &mut self.f,
                &mut self.n,
                &vec![true; self.refined.len()],
                &mut self.refined,
                &mut self.pairs,
                budget,
            )
        }
        fn paired_rectangles(&self) {
            for &t in &self.pairs {
                assert!(plan(
                    0,
                    &self.v,
                    &self.i[t * 3..t * 3 + 6],
                    &self.u[t * 6..t * 6 + 12],
                    &self.r[t * 9..t * 9 + 18],
                    self.t[t]
                )
                .is_some());
            }
        }
    }
    #[test]
    fn boundary_points_make_rectangular_cells_not_independent_triangle_fans() {
        let mut m = Mesh::new();
        m.rect(0, 128, 0, 128);
        m.v.extend([[64, 0, 0], [0, 64, 0]]);
        assert_eq!(m.run(100), 14);
        assert_eq!(m.n, [16]);
        assert_eq!(m.pairs.len(), 8);
        m.paired_rectangles();
        assert!(m.v.contains(&[64, 128, 0]));
        assert!(m.v.contains(&[128, 64, 0]));
        assert!(m.v.contains(&[64, 64, 0]));
        for k in 0..m.i.len() {
            let p = m.v[m.i[k] as usize];
            assert_eq!(&m.u[k * 2..k * 2 + 2], &[p[0] as u8, p[1] as u8]);
        }
    }
    #[test]
    fn propagation_retains_matching_vertices_on_adjacent_rectangles() {
        let mut m = Mesh::new();
        m.rect(0, 128, 0, 128);
        m.rect(0, 128, 128, 256);
        m.v.push([64, 0, 0]);
        assert_eq!(m.run(100), 20);
        assert_eq!(m.n, [16, 8]);
        m.paired_rectangles();
        for f in 0..2 {
            let start = m.f[f] as usize * 3;
            let end = start + m.n[f] as usize * 3;
            assert!(m.i[start..end]
                .iter()
                .any(|&i| m.v[i as usize] == [64, 128, 0]));
        }
    }
    #[test]
    fn no_junction_is_byte_identical_and_budget_failure_is_atomic() {
        let mut m = Mesh::new();
        m.rect(0, 128, 0, 128);
        let before = m.clone();
        assert_eq!(m.run(100), 0);
        assert_eq!(m.i, before.i);
        assert_eq!(m.v, before.v);
        m.v.push([64, 0, 0]);
        let before = m.clone();
        assert_eq!(m.run(1), 0);
        assert_eq!(m.v, before.v);
        assert_eq!(m.i, before.i);
        assert_eq!(m.u, before.u);
        assert_eq!(m.f, before.f);
        assert_eq!(m.n, before.n);
        assert_eq!(m.refined, before.refined);
    }
    #[test]
    fn reversed_winding_is_preserved_and_uv_seams_are_rejected() {
        let mut m = Mesh::new();
        m.rect(0, 128, 0, 128);
        m.v.push([64, 0, 0]);
        for t in 0..2 {
            m.i.swap(t * 3 + 1, t * 3 + 2);
            for a in 0..2 {
                m.u.swap(t * 6 + 2 + a, t * 6 + 4 + a);
            }
            for a in 0..3 {
                m.r.swap(t * 9 + 3 + a, t * 9 + 6 + a);
            }
        }
        assert_eq!(m.run(100), 14);
        m.paired_rectangles();
        for t in 0..m.t.len() {
            let a = m.v[m.i[t * 3] as usize];
            let b = m.v[m.i[t * 3 + 1] as usize];
            let c = m.v[m.i[t * 3 + 2] as usize];
            assert!(
                (b[0] as i32 - a[0] as i32) * (c[1] as i32 - a[1] as i32)
                    - (b[1] as i32 - a[1] as i32) * (c[0] as i32 - a[0] as i32)
                    < 0
            );
        }
        let mut m = Mesh::new();
        m.rect(0, 128, 0, 128);
        m.v.push([64, 0, 0]);
        m.u[6] = 127;
        assert_eq!(m.run(100), 0);
    }
    #[test]
    fn existing_uv_grid_boundary_rejects_new_junctions_atomically() {
        let mut m = Mesh::new();
        m.rect(0, 128, 0, 128);
        m.rect(0, 128, 128, 256);
        m.refined[1] = true;
        m.v.push([64, 0, 0]);
        let before = m.clone();
        assert_eq!(m.run(100), 0);
        assert_eq!(m.v, before.v);
        assert_eq!(m.i, before.i);
        assert_eq!(m.u, before.u);
        assert_eq!(m.r, before.r);
        assert_eq!(m.refined, before.refined);
    }

    #[test]
    fn new_grid_junction_welds_a_short_nonrectangular_neighbour() {
        let mut m = Mesh::new();
        m.rect(0, 64, 0, 64);
        m.v.extend([[32, 0, 0], [16, 64, 1]]);
        let base = m.v.len() as u16;
        m.v.extend([[0, 64, 0], [64, 64, 0], [32, 96, 0]]);
        m.f.push(2);
        m.n.push(1);
        m.refined.push(false);
        m.i.extend([base, base + 1, base + 2]);
        m.t.push(0);
        m.u.extend([0, 0, 64, 0, 32, 32]);
        m.r.extend([128; 9]);
        let original_vertices = m.v.len();
        assert_eq!(m.run(100), 14);
        assert!(!boundaries_conform(
            &m.v,
            &m.i,
            &m.f,
            &m.n,
            original_vertices
        ));
        let added = super::super::weld_tjunctions(
            &m.v,
            &mut m.i,
            &mut m.t,
            &mut m.u,
            &mut m.r,
            &mut m.f,
            &mut m.n,
            &m.refined,
            &mut m.pairs,
            100,
            original_vertices,
        );
        assert!(boundaries_conform(
            &m.v,
            &m.i,
            &m.f,
            &m.n,
            original_vertices
        ));
        assert_eq!(added, 3);
        assert_eq!(m.n[1], 4);
        // The old one-unit-off-plane weld candidate cannot replace a new
        // exact shared junction or create a non-simple projected boundary.
        let start = m.f[1] as usize * 3;
        assert!(m.i[start..].iter().all(|&i| m.v[i as usize][2] == 0));
        assert!(m.i[start..start + 6]
            .iter()
            .any(|&i| m.v[i as usize] == [32, 64, 0]));
    }
}

/// A generated junction must be an endpoint on every incident face boundary.
/// Check the emitted mesh, since the weld's resident budget can reject a split.
/// The caller rolls back the grid transaction if a neighbour cannot conform.
pub(super) fn boundaries_conform(
    verts: &[[i16; 3]],
    idx: &[u16],
    first: &[u32],
    count: &[u16],
    generated_from: usize,
) -> bool {
    let mut lines: BTreeMap<_, BTreeSet<i16>> = BTreeMap::new();
    for &p in &verts[generated_from..] {
        for a in 0..3 {
            lines.entry(line(p, a)).or_default().insert(p[a]);
        }
    }
    for (&first, &count) in first.iter().zip(count) {
        let mut edges = BTreeMap::new();
        for t in first as usize..first as usize + count as usize {
            for e in 0..3 {
                let a = verts[idx[t * 3 + e] as usize];
                let b = verts[idx[t * 3 + (e + 1) % 3] as usize];
                *edges
                    .entry(if a < b { (a, b) } else { (b, a) })
                    .or_insert(0usize) += 1;
            }
        }
        for ((p, q), uses) in edges {
            if uses != 1 {
                continue;
            }
            let axes: Vec<_> = (0..3).filter(|&a| p[a] != q[a]).collect();
            if axes.len() != 1 {
                continue;
            }
            let a = axes[0];
            if let Some(points) = lines.get(&line(p, a)) {
                use std::ops::Bound::Excluded;
                if points
                    .range((Excluded(p[a].min(q[a])), Excluded(p[a].max(q[a]))))
                    .next()
                    .is_some()
                {
                    return false;
                }
            }
        }
    }
    true
}

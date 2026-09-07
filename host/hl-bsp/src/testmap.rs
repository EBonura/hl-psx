//! Synthetic GoldSrc BSP v30 test maps for empirical tessellation work.
//!
//! One box room under a high-frequency checker texture. Faces are emitted in
//! 256-unit tiles so lightmap extents stay inside engine limits, the floor is
//! the grazing-angle worst case, the +Y wall is the head-on control (near-zero
//! affine warp: it must stay unsplit), and pinned camera yaws make any wall
//! oblique on demand. Vis is a real single-visleaf row so the PVS/pipeline
//! path engages exactly as on authored maps.

fn f32b(v: f32, o: &mut Vec<u8>) {
    o.extend_from_slice(&v.to_le_bytes());
}

struct Bsp {
    lumps: [Vec<u8>; 15],
}

const LUMP_ENTITIES: usize = 0;
const LUMP_PLANES: usize = 1;
const LUMP_TEXTURES: usize = 2;
const LUMP_VERTEXES: usize = 3;
const LUMP_VISIBILITY: usize = 4;
const LUMP_NODES: usize = 5;
const LUMP_TEXINFO: usize = 6;
const LUMP_FACES: usize = 7;
const LUMP_LIGHTING: usize = 8;
const LUMP_CLIPNODES: usize = 9;
const LUMP_LEAFS: usize = 10;
const LUMP_MARKSURFACES: usize = 11;
const LUMP_EDGES: usize = 12;
const LUMP_SURFEDGES: usize = 13;
const LUMP_MODELS: usize = 14;

const ROOM_MIN: [f32; 3] = [-512.0, -512.0, 0.0];
const ROOM_MAX: [f32; 3] = [512.0, 512.0, 256.0];
const TILE: f32 = 128.0;

pub fn generate(out: &str) -> Result<(), String> {
    let mut b = Bsp {
        lumps: Default::default(),
    };

    // -- entities ---------------------------------------------------------
    b.lumps[LUMP_ENTITIES] = format!(
        "{{\n\"classname\" \"worldspawn\"\n\"wad\" \"\"\n}}\n{{\n\"classname\" \"info_player_start\"\n\"origin\" \"0 -400 36\"\n\"angle\" \"90\"\n}}\n\0"
    )
    .into_bytes();

    // -- texture: 64x64 checker, 8px cells --------------------------------
    let mut tex = Vec::new();
    tex.extend_from_slice(&1i32.to_le_bytes()); // count
    tex.extend_from_slice(&8i32.to_le_bytes()); // offset of miptex 0
    let mut mip = Vec::new();
    let name = b"TESTCHK\0\0\0\0\0\0\0\0\0";
    mip.extend_from_slice(name);
    mip.extend_from_slice(&64u32.to_le_bytes());
    mip.extend_from_slice(&64u32.to_le_bytes());
    let data_start = 16 + 8 + 16; // name + w/h + 4 offsets
    let mut off = data_start;
    for level in 0..4u32 {
        mip.extend_from_slice(&(off as u32).to_le_bytes());
        off += (64 >> level) * (64 >> level);
    }
    for level in 0..4u32 {
        let size = 64 >> level;
        let cell = (8 >> level).max(1);
        for y in 0..size {
            for x in 0..size {
                let c = ((x / cell) + (y / cell)) & 1;
                mip.push(if c == 0 { 0u8 } else { 1u8 });
            }
        }
    }
    mip.extend_from_slice(&256u16.to_le_bytes());
    for i in 0..256 {
        // Index 0 dark grey, 1 white, rest a grey ramp (never all-black).
        let v = match i {
            0 => 40u8,
            1 => 255u8,
            other => (other & 0xff) as u8,
        };
        mip.extend_from_slice(&[v, v, v]);
    }
    tex.extend_from_slice(&mip);
    b.lumps[LUMP_TEXTURES] = tex;

    // -- geometry builders -------------------------------------------------
    let mut planes: Vec<[f32; 5]> = Vec::new(); // nx ny nz dist type
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut edges: Vec<(u16, u16)> = vec![(0, 0)];
    let mut surfedges: Vec<i32> = Vec::new();
    let mut faces = Vec::new();
    let mut lighting = Vec::new();
    let mut texinfos = Vec::new();
    let mut marks: Vec<u16> = Vec::new();

    let add_plane = |planes: &mut Vec<[f32; 5]>, n: [f32; 3], d: f32| -> (u16, u16) {
        // Store the axis-positive plane; side=1 flips it, like the compilers.
        let (n, d, side) = if n[0] + n[1] + n[2] < 0.0 {
            ([-n[0], -n[1], -n[2]], -d, 1u16)
        } else {
            (n, d, 0u16)
        };
        let ty = if n[0].abs() == 1.0 {
            0.0
        } else if n[1].abs() == 1.0 {
            1.0
        } else {
            2.0
        };
        for (i, p) in planes.iter().enumerate() {
            if p[0] == n[0] && p[1] == n[1] && p[2] == n[2] && p[3] == d {
                return (i as u16, side);
            }
        }
        planes.push([n[0], n[1], n[2], d, ty]);
        ((planes.len() - 1) as u16, side)
    };

    let add_texinfo = |texinfos: &mut Vec<u8>, s: [f32; 4], t: [f32; 4]| -> u16 {
        let idx = texinfos.len() / 40;
        for v in s {
            f32b(v, texinfos);
        }
        for v in t {
            f32b(v, texinfos);
        }
        texinfos.extend_from_slice(&0i32.to_le_bytes()); // miptex 0
        texinfos.extend_from_slice(&0i32.to_le_bytes()); // flags
        idx as u16
    };

    // One quad face from four CCW-when-viewed corners.
    let add_face = |corners: [[f32; 3]; 4],
                    normal: [f32; 3],
                    s: [f32; 4],
                    t: [f32; 4],
                    planes: &mut Vec<[f32; 5]>,
                    verts: &mut Vec<[f32; 3]>,
                    edges: &mut Vec<(u16, u16)>,
                    surfedges: &mut Vec<i32>,
                    faces: &mut Vec<u8>,
                    lighting: &mut Vec<u8>,
                    texinfos: &mut Vec<u8>,
                    marks: &mut Vec<u16>| {
        let d = normal[0] * corners[0][0] + normal[1] * corners[0][1] + normal[2] * corners[0][2];
        let (plane, side) = add_plane(planes, normal, d);
        let ti = add_texinfo(texinfos, s, t);
        let first_edge = surfedges.len() as i32;
        // GoldSrc faces wind clockwise seen from the front; the generator's
        // corner lists are authored counter-clockwise, so emit them reversed.
        let corners = [corners[0], corners[3], corners[2], corners[1]];
        let mut vidx = [0u16; 4];
        for (i, c) in corners.iter().enumerate() {
            vidx[i] = verts.len() as u16;
            verts.push(*c);
        }
        for i in 0..4 {
            let a = vidx[i];
            let bq = vidx[(i + 1) & 3];
            edges.push((a, bq));
            surfedges.push((edges.len() - 1) as i32);
        }
        // Lightmap block from texture-coordinate extents, engine convention.
        let mut smin = f32::MAX;
        let mut smax = f32::MIN;
        let mut tmin = f32::MAX;
        let mut tmax = f32::MIN;
        for c in corners {
            let sv = s[0] * c[0] + s[1] * c[1] + s[2] * c[2] + s[3];
            let tv = t[0] * c[0] + t[1] * c[1] + t[2] * c[2] + t[3];
            smin = smin.min(sv);
            smax = smax.max(sv);
            tmin = tmin.min(tv);
            tmax = tmax.max(tv);
        }
        let bs = (smin / 16.0).floor() as i32;
        let es = (smax / 16.0).ceil() as i32;
        let bt = (tmin / 16.0).floor() as i32;
        let et = (tmax / 16.0).ceil() as i32;
        let lw = (es - bs + 1) as usize;
        let lh = (et - bt + 1) as usize;
        let lightofs = lighting.len() as i32;
        for _ in 0..lw * lh {
            lighting.extend_from_slice(&[160, 160, 160]);
        }
        let face_index = (faces.len() / 20) as u16;
        faces.extend_from_slice(&plane.to_le_bytes());
        faces.extend_from_slice(&side.to_le_bytes());
        faces.extend_from_slice(&first_edge.to_le_bytes());
        faces.extend_from_slice(&4u16.to_le_bytes());
        faces.extend_from_slice(&ti.to_le_bytes());
        faces.extend_from_slice(&[0u8, 255, 255, 255]);
        faces.extend_from_slice(&lightofs.to_le_bytes());
        marks.push(face_index);
    };

    let (x0, y0, z0) = (ROOM_MIN[0], ROOM_MIN[1], ROOM_MIN[2]);
    let (x1, y1, z1) = (ROOM_MAX[0], ROOM_MAX[1], ROOM_MAX[2]);
    let steps = |a: f32, b: f32| ((b - a) / TILE) as i32;

    // Floor (normal +Z), tiled.
    for iy in 0..steps(y0, y1) {
        for ix in 0..steps(x0, x1) {
            let (ax, ay) = (x0 + ix as f32 * TILE, y0 + iy as f32 * TILE);
            add_face(
                [
                    [ax, ay, z0],
                    [ax + TILE, ay, z0],
                    [ax + TILE, ay + TILE, z0],
                    [ax, ay + TILE, z0],
                ],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0, 0.0],
                [0.0, -1.0, 0.0, 0.0],
                &mut planes,
                &mut verts,
                &mut edges,
                &mut surfedges,
                &mut faces,
                &mut lighting,
                &mut texinfos,
                &mut marks,
            );
        }
    }
    // Ceiling (normal -Z).
    for iy in 0..steps(y0, y1) {
        for ix in 0..steps(x0, x1) {
            let (ax, ay) = (x0 + ix as f32 * TILE, y0 + iy as f32 * TILE);
            add_face(
                [
                    [ax, ay + TILE, z1],
                    [ax + TILE, ay + TILE, z1],
                    [ax + TILE, ay, z1],
                    [ax, ay, z1],
                ],
                [0.0, 0.0, -1.0],
                [1.0, 0.0, 0.0, 0.0],
                [0.0, -1.0, 0.0, 0.0],
                &mut planes,
                &mut verts,
                &mut edges,
                &mut surfedges,
                &mut faces,
                &mut lighting,
                &mut texinfos,
                &mut marks,
            );
        }
    }
    // +Y head-on control wall (normal -Y, faces the spawn).
    for iz in 0..1 {
        let _ = iz;
        for ix in 0..steps(x0, x1) {
            let ax = x0 + ix as f32 * TILE;
            add_face(
                [
                    [ax, y1, z0],
                    [ax + TILE, y1, z0],
                    [ax + TILE, y1, z1],
                    [ax, y1, z1],
                ],
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, -1.0, 0.0],
                &mut planes,
                &mut verts,
                &mut edges,
                &mut surfedges,
                &mut faces,
                &mut lighting,
                &mut texinfos,
                &mut marks,
            );
        }
    }
    // -Y wall (normal +Y).
    for ix in 0..steps(x0, x1) {
        let ax = x0 + ix as f32 * TILE;
        add_face(
            [
                [ax + TILE, y0, z0],
                [ax, y0, z0],
                [ax, y0, z1],
                [ax + TILE, y0, z1],
            ],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            &mut planes,
            &mut verts,
            &mut edges,
            &mut surfedges,
            &mut faces,
            &mut lighting,
            &mut texinfos,
            &mut marks,
        );
    }
    // -X wall (normal +X) and +X wall (normal -X): oblique under camera yaw.
    for iy in 0..steps(y0, y1) {
        let ay = y0 + iy as f32 * TILE;
        add_face(
            [
                [x0, ay, z0],
                [x0, ay + TILE, z0],
                [x0, ay + TILE, z1],
                [x0, ay, z1],
            ],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            &mut planes,
            &mut verts,
            &mut edges,
            &mut surfedges,
            &mut faces,
            &mut lighting,
            &mut texinfos,
            &mut marks,
        );
        add_face(
            [
                [x1, ay + TILE, z0],
                [x1, ay, z0],
                [x1, ay, z1],
                [x1, ay + TILE, z1],
            ],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            &mut planes,
            &mut verts,
            &mut edges,
            &mut surfedges,
            &mut faces,
            &mut lighting,
            &mut texinfos,
            &mut marks,
        );
    }

    let n_faces = faces.len() / 20;

    // -- BSP tree: six splitting nodes, inside chains to the room leaf ----
    // Node planes use the axis-positive stored planes; child -1 = leaf 0
    // (solid), -2 = leaf 1 (room). Children[0] = front (positive side).
    let mut nodes = Vec::new();
    let bounds = [
        // (normal, dist, room is on the FRONT (positive) side?)
        ([1.0f32, 0.0, 0.0], x0, true), // x >= x0 keeps the room in front
        ([1.0, 0.0, 0.0], x1, false),   // x <= x1: room behind
        ([0.0, 1.0, 0.0], y0, true),
        ([0.0, 1.0, 0.0], y1, false),
        ([0.0, 0.0, 1.0], z0, true),
        ([0.0, 0.0, 1.0], z1, false),
    ];
    let mut plane_ids = Vec::new();
    for (n, d, _) in bounds {
        let (pid, _side) = add_plane(&mut planes, n, d);
        plane_ids.push(pid);
    }
    for (i, (_, _, room_front)) in bounds.iter().enumerate() {
        let next: i16 = if i == 5 { -2 } else { (i + 1) as i16 };
        let (front, back) = if *room_front {
            (next, -1i16)
        } else {
            (-1i16, next)
        };
        nodes.extend_from_slice(&(plane_ids[i] as u32).to_le_bytes());
        nodes.extend_from_slice(&front.to_le_bytes());
        nodes.extend_from_slice(&back.to_le_bytes());
        for v in [x0, y0, z0] {
            nodes.extend_from_slice(&(v as i16).to_le_bytes());
        }
        for v in [x1, y1, z1] {
            nodes.extend_from_slice(&(v as i16).to_le_bytes());
        }
        nodes.extend_from_slice(&0u16.to_le_bytes()); // firstface
        nodes.extend_from_slice(&0u16.to_le_bytes()); // nfaces
    }
    b.lumps[LUMP_NODES] = nodes;

    // -- clipnodes: hulls 1..3, box expanded per hull ---------------------
    let mut clipnodes = Vec::new();
    let mut clip_roots = [0i32; 3];
    let hull_expand = [16.0f32, 32.0, 16.0];
    for (h, expand) in hull_expand.iter().enumerate() {
        clip_roots[h] = (clipnodes.len() / 8) as i32;
        for (i, (n, d, room_front)) in bounds.iter().enumerate() {
            let d = if *room_front { d - expand } else { d + expand };
            let (pid, _s) = add_plane(&mut planes, *n, d);
            let base = clip_roots[h] as i16;
            let next: i16 = if i == 5 { -1 } else { base + (i as i16) + 1 };
            // CONTENTS_EMPTY = -1, CONTENTS_SOLID = -2.
            let (front, back) = if *room_front {
                (next, -2i16)
            } else {
                (-2i16, next)
            };
            clipnodes.extend_from_slice(&(pid as i32).to_le_bytes());
            clipnodes.extend_from_slice(&front.to_le_bytes());
            clipnodes.extend_from_slice(&back.to_le_bytes());
        }
    }
    b.lumps[LUMP_CLIPNODES] = clipnodes;

    // -- planes (serialized after all additions) --------------------------
    let mut pl = Vec::new();
    for p in &planes {
        for v in [p[0], p[1], p[2], p[3]] {
            f32b(v, &mut pl);
        }
        pl.extend_from_slice(&(p[4] as i32).to_le_bytes());
    }
    b.lumps[LUMP_PLANES] = pl;

    // -- vertexes / edges / surfedges -------------------------------------
    let mut vl = Vec::new();
    for v in &verts {
        for c in v {
            f32b(*c, &mut vl);
        }
    }
    b.lumps[LUMP_VERTEXES] = vl;
    let mut el = Vec::new();
    for (a, bq) in &edges {
        el.extend_from_slice(&a.to_le_bytes());
        el.extend_from_slice(&bq.to_le_bytes());
    }
    b.lumps[LUMP_EDGES] = el;
    let mut sl = Vec::new();
    for s in &surfedges {
        sl.extend_from_slice(&s.to_le_bytes());
    }
    b.lumps[LUMP_SURFEDGES] = sl;
    b.lumps[LUMP_TEXINFO] = texinfos;
    b.lumps[LUMP_FACES] = faces;
    b.lumps[LUMP_LIGHTING] = lighting;

    // -- vis: one visleaf seeing itself -----------------------------------
    b.lumps[LUMP_VISIBILITY] = vec![0x01];

    // -- leafs -------------------------------------------------------------
    let mut leafs = Vec::new();
    // leaf 0: solid, no vis
    leafs.extend_from_slice(&(-2i32).to_le_bytes());
    leafs.extend_from_slice(&(-1i32).to_le_bytes());
    for _ in 0..6 {
        leafs.extend_from_slice(&0i16.to_le_bytes());
    }
    leafs.extend_from_slice(&0u16.to_le_bytes());
    leafs.extend_from_slice(&0u16.to_le_bytes());
    leafs.extend_from_slice(&[0u8; 4]);
    // leaf 1: the room, visofs 0, all marksurfaces
    leafs.extend_from_slice(&(-1i32).to_le_bytes());
    leafs.extend_from_slice(&0i32.to_le_bytes());
    for v in [
        x0 as i16, y0 as i16, z0 as i16, x1 as i16, y1 as i16, z1 as i16,
    ] {
        leafs.extend_from_slice(&v.to_le_bytes());
    }
    leafs.extend_from_slice(&0u16.to_le_bytes());
    leafs.extend_from_slice(&(marks.len() as u16).to_le_bytes());
    leafs.extend_from_slice(&[0u8; 4]);
    b.lumps[LUMP_LEAFS] = leafs;

    let mut ml = Vec::new();
    for m in &marks {
        ml.extend_from_slice(&m.to_le_bytes());
    }
    b.lumps[LUMP_MARKSURFACES] = ml;

    // -- model 0 -----------------------------------------------------------
    let mut models = Vec::new();
    for v in [x0, y0, z0] {
        f32b(v, &mut models);
    }
    for v in [x1, y1, z1] {
        f32b(v, &mut models);
    }
    for _ in 0..3 {
        f32b(0.0, &mut models);
    }
    models.extend_from_slice(&0i32.to_le_bytes()); // headnode[0] = bsp root
    for r in clip_roots {
        models.extend_from_slice(&r.to_le_bytes());
    }
    models.extend_from_slice(&1i32.to_le_bytes()); // visleafs
    models.extend_from_slice(&0i32.to_le_bytes()); // firstface
    models.extend_from_slice(&(n_faces as i32).to_le_bytes());
    b.lumps[LUMP_MODELS] = models;

    // -- serialize ---------------------------------------------------------
    let mut out_bytes = Vec::new();
    out_bytes.extend_from_slice(&30i32.to_le_bytes());
    let mut offset = 4 + 15 * 8;
    let mut dir = Vec::new();
    for lump in &b.lumps {
        dir.extend_from_slice(&(offset as i32).to_le_bytes());
        dir.extend_from_slice(&(lump.len() as i32).to_le_bytes());
        offset += (lump.len() + 3) & !3;
    }
    out_bytes.extend_from_slice(&dir);
    for lump in &b.lumps {
        out_bytes.extend_from_slice(lump);
        while out_bytes.len() % 4 != 0 {
            out_bytes.push(0);
        }
    }
    std::fs::write(out, &out_bytes).map_err(|e| format!("write {}: {}", out, e))?;
    eprintln!(
        "testmap: {} faces, {} planes, {} verts, {} bytes -> {}",
        n_faces,
        planes.len(),
        verts.len(),
        out_bytes.len(),
        out
    );
    Ok(())
}

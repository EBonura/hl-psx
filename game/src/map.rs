//! Parse a cooked `.hlm` map (`host/hl-bsp --cook`). HLMA adds BSP visibility
//! plus brush-entity leaf membership for PVS culling; HLMB additionally tags
//! exact positive axial clip planes for faster collision traversal. HLMC packs
//! the world model's GoldSrc PVS-cluster count separately from the BSP's total
//! leaf-record count (submodels append leaves which are not PVS bits). HLMD
//! keeps that layout and stores plane distances in Q27.5 so collision retains
//! GoldSrc's 1/32-unit `DIST_EPSILON` without growing any plane record. HLMF
//! appends the texture-animation chain section after the logic names (offset
//! derived, so the 52-byte header layout is unchanged). HLMG adds sparse,
//! independently switchable lightmap planes after the base light palette; its
//! 15-bit face count carries GoldSrc's map-wide water-alpha verdict in bit 15.
//! HLMH grows the header by one word for a versioned source-addressed world
//! pipeline section.
//!
//!   magic "HLMA" .. "HLMH" | u32 n_verts,n_tris,n_texs,n_faces,bsp_off
//!     | u32 clip_off,ent_off,tram_off,prop_off,sky_tex_base,nav_off,logic_off
//!     | HLMH: u32 world_pipeline_off
//!   verts i16×3 | u32 n_loopverts | FaceVert[5B] × n_loopverts
//!     | TriRec[16B] × n_tris (raw/dirty faces only) | light palette (u16 × 256)
//!     FaceVert = u16 idx, u8 uv[2], u8 light_idx     (loop faces fan these)
//!     TriRec   = u16 idx[3], u8 uv[6], u8 tex, u8 light_idx[3]
//!   optional legacy textures × n_texs: u16 w,h | u16 clut[16] | u8 pix[w*h/2]
//!   modern streamed builds keep texture pixels in a separate HLTX chunk:
//!     magic "HLTX" | u32 n_texs | textures...
//!   bsp @ bsp_off:
//!     u32 n_planes,n_face_groups,n_nodes,leaf_counts,n_marks,vis_len
//!       HLMA/B leaf_counts = n_leaves (legacy; PVS bits assumed n_leaves-1)
//!       HLMC/D leaf_counts = n_leaves | (n_visleaves << 16)
//!     PlaneRec[10B] × n_planes = i16 Q14 normal[3], i32 dist_q5
//!     FaceGroup[2B] × n_face_groups = signed plane reference
//!     FaceRec[16B] × n_faces
//!       FaceRec = u16 first, u16 count, u16 plane_group, i16 center[3],
//!                 u16 radius, u8 tex, u8 flags
//!                 (bit0: loop; bit1: liquid; bit4: native patch records)
//!       loop face: (first,count) = loopvert range; raw face: = tri range
//!       patch face: first = loopvert start, count = fixed four-corner records
//!         (bit15 on record corner 3 marks a triangle fallback)
//!     nodes  (u16 plane, i16 c0, i16 c1) × n_nodes
//!     leaves (i32 visofs, u16 mark_start, u16 mark_count) × n_leaves
//!     marks  u16 × n_marks (4-aligned on BOTH sides -- see marks_off)
//!     vis    u8  × vis_len (pad 4)
//!   clip:
//!     u32 n_clip | i32 legacy_hull0_head | i32 hull1_head | i32 hull3_head |
//!     i32 spawn[3] |
//!     i32 spawn_yaw | ClipNode[6B] × n_clip
//!       HLMA plane_ref = untagged u16 plane index
//!       HLMB/C/D plane_ref = tag[15:14] | plane_index[13:0]
//!         tag 00=generic, 01=+X, 10=+Y, 11=+Z
//!   entities:
//!     u32 n_models | (u32 firstface,u32 numface) × n_models
//!     u32 n_ents | EntRec[56B] × n_ents | u32 n_ent_leafs | u16 leaf_idx[]
//!   tram header (24 bytes):
//!     u16 submodel,n_way | u32 packed speed/wheels/authored_start |
//!     i32 clip_head | i32 authored_base[3]
//!   tram payload:
//!     i32 way[3] × n_way | u16 way_speed × n_way | u16 way_pass × n_way
//!   props/items:
//!     u32 0x80000000|(n_sprites<<16)|n_props |
//!     PropRec[24B] × n_props | SpriteRec[12B] × n_sprites
//!   nav (18-byte packed nodes in both modes):
//!     exact retail route: u16 n_nav,0x8000|route_bytes |
//!       (i32 origin[3],i16 leaf,u16 route_off,u8 node_type,u8 pad) × n_nav |
//!       GoldSrc compressed NextNodeInRoute bytes
//!     legacy/custom fallback: u16 n_nav,n_nav_links |
//!       (i32 origin[3],i16 leaf,u16 first_link,u8 link_count,u8 pad) × n_nav |
//!       u16 link_dest × n_nav_links
//!   logic:
//!     u16 n_logic,n_aux,n_names,name_bytes |
//!     LogicRec[64B] × n_logic | LogicAux[4B] × n_aux |
//!     u16 name_offsets[n_names] | u8 nul_terminated_names[name_bytes]
//!   texanim (HLME/HLMF, at align4(logic names end)):
//!     u16 n_chains | u16 reserved | per chain: u8 n_primary | u8 n_alt |
//!     u8 primary_tex_ids[] | u8 alt_tex_ids[]
//!
//! Fields are read via `from_le_bytes` (include_bytes! is only byte-aligned).

pub use hl_format::logic::*;
use hl_format::map as cooked;
use psx_gte::math::Vec3I16;

// ponytail: unchecked byte reads. The R3000 has no data cache, so every field
// read hits RAM; the per-read bounds branch is pure overhead and, more
// importantly, blocks LLVM from coalescing the consecutive byte loads into a
// single MIPS unaligned word load (lwl/lwr). Offsets all come from header
// counts in our own cooked blob (magic-checked at load), so they are in range
// by construction. If a map ever fails to cook cleanly, re-add the checks.
#[inline(always)]
fn rd_u32(d: &[u8], o: usize) -> u32 {
    unsafe {
        u32::from_le_bytes([
            *d.get_unchecked(o),
            *d.get_unchecked(o + 1),
            *d.get_unchecked(o + 2),
            *d.get_unchecked(o + 3),
        ])
    }
}
#[inline(always)]
fn rd_i32(d: &[u8], o: usize) -> i32 {
    rd_u32(d, o) as i32
}
#[inline(always)]
fn rd_u16(d: &[u8], o: usize) -> u16 {
    unsafe { u16::from_le_bytes([*d.get_unchecked(o), *d.get_unchecked(o + 1)]) }
}
#[inline(always)]
fn rd_i16(d: &[u8], o: usize) -> i16 {
    rd_u16(d, o) as i16
}

/// Halfword load for cooked sections whose format guarantees even offsets.
/// Unlike the generic byte-safe readers above, this lets the R3000 use one
/// `lh`/`lhu` instead of assembling every value from two byte loads.
#[inline(always)]
fn rd_u16_aligned(d: &[u8], o: usize) -> u16 {
    debug_assert_eq!((d.as_ptr() as usize + o) & 1, 0);
    u16::from_le(unsafe { d.as_ptr().add(o).cast::<u16>().read() })
}

#[inline(always)]
fn rd_i16_aligned(d: &[u8], o: usize) -> i16 {
    rd_u16_aligned(d, o) as i16
}
#[inline(always)]
fn align4(x: usize) -> usize {
    (x + 3) & !3
}

/// Convert a Q27.5 plane distance for render-only callers which still operate
/// on whole world units. Collision and BSP-side tests retain Q5 end to end.
#[inline(always)]
fn q5_to_world_nearest(value: i32) -> i32 {
    if value >= 0 {
        value.wrapping_add(16) >> 5
    } else {
        -((-value).wrapping_add(16) >> 5)
    }
}

#[inline(always)]
fn expand5(v: u16) -> u8 {
    ((v << 3) | (v >> 2)) as u8
}

/// Re-quantise a curved channel triple back into the cooked rgb555 packing.
/// The source was already 5-bit, so this only rounds the curve's own output.
#[inline(always)]
fn pack_rgb555(r: u8, g: u8, b: u8) -> u16 {
    (r as u16 >> 3) | ((g as u16 >> 3) << 5) | ((b as u16 >> 3) << 10)
}

#[inline(always)]
fn unpack_rgb555(v: u16) -> (u8, u8, u8) {
    (
        expand5(v & 31),
        expand5((v >> 5) & 31),
        expand5((v >> 10) & 31),
    )
}

// Load-time-expanded light palette (see `expand_light_palette`).
static mut LIGHT_PAL_RGB: [u32; 256] = [0; 256]; // 0x00BBGGRR, one lw per corner
                                                 // Same treatment for HLMG's sparse lightstyle palette, so a flickering light
                                                 // tracks the brightness setting instead of adding un-curved colour on top of a
                                                 // curved base. Kept in the cooked rgb555 packing rather than pre-expanded to a
                                                 // word: the per-corner decode then keeps the exact shape it had when it read
                                                 // the map blob, and LLVM's unrolling of the 3x3 dynamic loop does not change.
static mut DYNAMIC_PAL_555: [u16; DYNAMIC_PALETTE_COLORS] = [0; DYNAMIC_PALETTE_COLORS];
// Where the two palettes were expanded from, so the options screen can re-run
// the curve without threading a `&Map` through the pause menu.
static mut LIGHT_PAL_SRC: *const u8 = core::ptr::null();
static mut LIGHT_PAL_SRC_OFF: usize = 0;
static mut DYNAMIC_PAL_SRC_OFF: usize = 0; // zero when the map has no lightstyles

#[inline(always)]
unsafe fn pal_rgb555_at(base: *const u8, off: usize) -> (u8, u8, u8) {
    let p = base.add(off);
    unpack_rgb555(u16::from_le_bytes([*p, *p.add(1)]))
}

/// Expand both light palettes from the resident map blob, applying the current
/// brightness curve. Costs 320 curve evaluations and runs only on map load and
/// on a brightness change, so it is kept out of the hot text bucket.
#[inline(never)]
#[link_section = ".hlpsx_cold.brightness"]
fn expand_light_palettes() {
    let base = unsafe { LIGHT_PAL_SRC };
    if base.is_null() {
        return;
    }
    let (light_off, dynamic_off) = unsafe { (LIGHT_PAL_SRC_OFF, DYNAMIC_PAL_SRC_OFF) };
    for i in 0..256 {
        let (r, g, b) = unsafe { pal_rgb555_at(base, light_off + i * 2) };
        let word = crate::settings::bright_cold(r) as u32
            | ((crate::settings::bright_cold(g) as u32) << 8)
            | ((crate::settings::bright_cold(b) as u32) << 16);
        unsafe { LIGHT_PAL_RGB[i] = word };
    }
    for i in 0..DYNAMIC_PALETTE_COLORS {
        let packed = if dynamic_off == 0 {
            0
        } else {
            let (r, g, b) = unsafe { pal_rgb555_at(base, dynamic_off + 4 + i * 2) };
            pack_rgb555(
                crate::settings::bright_cold(r),
                crate::settings::bright_cold(g),
                crate::settings::bright_cold(b),
            )
        };
        unsafe { DYNAMIC_PAL_555[i] = packed };
    }
}

/// Re-run the brightness curve over the resident map's palettes. No-op before
/// the first map load.
pub fn reapply_brightness() {
    expand_light_palettes();
}

// HLMG's dynamic-light context is selected once per face by the world/entity
// walkers. Ordinary faces leave it disabled, so their triangle hot path is the
// same three base-palette reads as before.
static mut DYNAMIC_TRI_FIRST: u16 = u16::MAX;
static mut DYNAMIC_TRI_COUNT: u16 = 0;
static mut DYNAMIC_DATA_OFF: usize = 0;
static mut DYNAMIC_ACTIVE_PLANES: u8 = 0;
static mut DYNAMIC_LOOP_POOL: bool = false;
const DYNAMIC_PALETTE_COLORS: usize = 64;
const DYNAMIC_FACE_REC_SZ: usize = cooked::DYNAMIC_FACE_RECORD_SIZE;
const DYNAMIC_TRI_BYTES: usize = 9; // 3 style planes x 3 triangle corners

const PLANE_SZ: usize = cooked::PLANE_RECORD_SIZE;
pub const PLANE_NORMAL_FRAC_BITS: i32 = 14;
pub const PLANE_NORMAL_ONE: i32 = 1 << PLANE_NORMAL_FRAC_BITS;
const FACE_GROUP_SZ: usize = cooked::FACE_GROUP_RECORD_SIZE;
const NODE_SZ: usize = cooked::NODE_RECORD_SIZE;
// The cooker writes the fields back-to-back: 12 + 2 + 2 + 1 + 1 = 18 bytes.
// Do not use Rust's naturally aligned struct size here. A stale 20-byte stride
// corrupted every waypoint after node zero and made scripted actors walk into
// walls instead of following the authored info_node graph.
const NAV_NODE_SZ: usize = cooked::NAV_NODE_RECORD_SIZE;
const NAV_EXACT_ROUTES: u16 = 0x8000;
pub const SKY_TEX_NONE: usize = usize::MAX;

pub const MATERIAL_CONCRETE: u8 = 0;
pub const MATERIAL_METAL: u8 = 1;
pub const MATERIAL_DIRT: u8 = 2;
pub const MATERIAL_VENT: u8 = 3;
pub const MATERIAL_GRATE: u8 = 4;
pub const MATERIAL_TILE: u8 = 5;
pub const MATERIAL_SLOSH: u8 = 6;
pub const MATERIAL_WOOD: u8 = 7;
pub const MATERIAL_COMPUTER: u8 = 8;
pub const MATERIAL_GLASS: u8 = 9;

pub struct Node {
    pub n: [i16; 3],
    pub dist_q5: i32,
    pub c0: i32,
    pub c1: i32,
}

pub struct Map {
    data: &'static [u8],
    pub n_verts: usize,
    pub n_tris: usize,
    pub n_texs: usize,
    pub n_faces: usize,
    pub sky_tex_base: usize,
    v_off: usize,
    lv_off: usize,       // 4-aligned `idx | uv << 16` words, one per loop vertex
    lv_light_off: usize, // parallel light-palette indices, one byte per vertex
    tri_off: usize,
    light_pal_off: usize, // 256 rgb555 entries, indexed by light_idx
    dynamic_off: usize,   // HLMG sparse lightstyle appendix; zero on older maps
    n_dynamic_faces: usize,
    water_alpha_supported: bool,

    // BSP / PVS
    pub n_planes: usize,
    pub n_face_groups: usize,
    pub n_nodes: usize,
    /// PVS clusters in world dmodel[0]. GoldSrc RLE rows contain exactly this
    /// many bits; later leaf records belong to brush submodels.
    pub n_visleaves: usize,
    pub n_marks: usize,
    planes_off: usize,
    face_groups_off: usize,
    faces_off: usize,
    nodes_off: usize,
    leaves_off: usize,
    marks_off: usize,
    vis_off: usize,
    vis_len: usize,
    // Clip hull + spawn
    pub n_clip: usize,
    pub hull1_head: i32,
    pub hull3_head: i32, // crouch hull (32x32x36); < 0 = none -> fall back to hull1
    pub spawn_pos: [i32; 3],
    pub spawn_yaw: i32,
    clipn_off: usize,
    // HLMA: 0 keeps its full untagged u16 plane index. HLMB/C: 0xc000 extracts
    // the axial tag and clears it from the low-14-bit plane-table index.
    clip_plane_tag_mask: u16,
    // Entities (brush models)
    pub n_models: usize,
    pub n_ents: usize,
    models_off: usize,
    ents_off: usize,
    ent_leafs_off: usize,
    n_ent_leafs: usize,
    // Tram (func_tracktrain ride)
    pub tram_submodel: usize,
    pub tram_speed: i32,
    pub tram_start: usize,
    pub tram_head: i32,
    pub tram_base: [i32; 3], // authored-start waypoint: places the brush on the track
    pub n_way: usize,
    way_off: usize,
    // Props/items (point-entity model placements)
    pub n_props: usize,
    props_off: usize,
    pub n_sprites: usize,
    sprites_off: usize,
    // AI navigation graph (land info_node graph)
    pub n_nav: usize,
    nav_meta: u16,
    nav_nodes_off: usize,
    nav_links_off: usize,
    // Half-Life-style target/use/touch entity graph
    pub n_logic: usize,
    pub n_logic_aux: usize,
    pub n_logic_names: usize,
    logic_off: usize,
    logic_aux_off: usize,
    logic_name_offsets_off: usize,
    logic_names_off: usize,
    // Texture-animation chains (HLME/HLMF; zero on older magics)
    pub n_tex_anim: usize,
    tex_anim_off: usize,
}

const LEAF_SZ: usize = cooked::LEAF_RECORD_SIZE;
const FACE_SZ: usize = cooked::FACE_RECORD_SIZE;
const TRI_SZ: usize = cooked::TRI_RECORD_SIZE;
const LOOPVERT_SZ: usize = cooked::LOOP_VERTEX_RECORD_SIZE;
const CLIPNODE_SZ: usize = cooked::CLIPNODE_RECORD_SIZE;
const ENT_SZ: usize = cooked::ENTITY_RECORD_SIZE;
const PROP_SZ: usize = cooked::PROP_RECORD_SIZE;
/// High targetname bit used by the cooker for actors supplied only by an
/// incoming transition. Runtime strips it before ordinary target dispatch.
pub const PROP_NAME_INCOMING_ONLY: u16 = 0x8000;
const SPRITE_REC_SZ: usize = cooked::SPRITE_RECORD_SIZE;
const LOGIC_SZ: usize = cooked::LOGIC_RECORD_SIZE;

// ---- GoldSrc animated textures (+0../+9 primary, +a../+j alternate) ----
// Display remap consulted by the per-triangle decode: entry i = the chain
// frame to draw for source texture i this tick, identity elsewhere. Row 0 is
// entity frame-state 0 (a face's own chain), row 1 frame-state 1 (the OTHER
// chain -- R_TextureAnimation's alternate swap). main.rs rebuilds the rows at
// 10 Hz; the emit hot path pays one indexed byte load, like LIGHT_PAL_RGB.
pub const TEX_ANIM_MAX: usize = 256;

const fn tex_anim_identity() -> [u8; TEX_ANIM_MAX] {
    let mut row = [0u8; TEX_ANIM_MAX];
    let mut i = 0;
    while i < TEX_ANIM_MAX {
        row[i] = i as u8;
        i += 1;
    }
    row
}

static mut TEX_ANIM_REMAP: [[u8; TEX_ANIM_MAX]; 2] = [tex_anim_identity(); 2];
static mut TEX_ANIM_SELECT: u8 = 0;

/// Select which frame-state row the triangle decoders resolve through. Set
/// alongside EMIT_BLEND by the entity walkers (frame-toggled func_walls,
/// pressed buttons); the world always draws with frame-state 0.
#[inline(always)]
pub fn tex_anim_select(alt_frame: bool) {
    unsafe { TEX_ANIM_SELECT = alt_frame as u8 };
}

/// Reset both rows to identity (per-map, textures change meaning on load).
pub fn tex_anim_reset() {
    let identity = tex_anim_identity();
    unsafe {
        TEX_ANIM_REMAP = [identity; 2];
        TEX_ANIM_SELECT = 0;
    }
}

/// Point source texture `id` at the given display frames. Returns true when
/// either row actually changed (drives the render-cache invalidation counter).
#[inline]
pub fn tex_anim_set(id: usize, frame0: u8, frame1: u8) -> bool {
    let i = id & (TEX_ANIM_MAX - 1);
    unsafe {
        let changed = TEX_ANIM_REMAP[0][i] != frame0 || TEX_ANIM_REMAP[1][i] != frame1;
        TEX_ANIM_REMAP[0][i] = frame0;
        TEX_ANIM_REMAP[1][i] = frame1;
        changed
    }
}

/// Resolve a source texture id to its current display frame under the active
/// frame-state row. Identity for everything that is not a chain member.
#[inline(always)]
pub fn tex_anim_display(tex: usize) -> usize {
    unsafe {
        *TEX_ANIM_REMAP
            .get_unchecked(TEX_ANIM_SELECT as usize)
            .get_unchecked(tex & (TEX_ANIM_MAX - 1)) as usize
    }
}

/// GoldSrc brush textures use `ANIM_CYCLE = 2`: `R_TextureAnimation` samples
/// a 10 Hz clock, but each numbered/lettered miptex owns two tenths.
#[inline(always)]
pub const fn tex_anim_frame_index(tenth: u16, frame_count: usize) -> usize {
    if frame_count == 0 {
        0
    } else {
        (tenth as usize / 2) % frame_count
    }
}

pub const SPRITE_ID_MASK: u16 = 0x000F;
pub const SPRITE_INITIAL_ON: u16 = 0x0010;
pub const SPRITE_ONCE: u16 = 0x0020;

pub struct ClipNode {
    /// Generic plane normal. Tagged axial nodes leave this zero and carry the
    /// canonical positive axis in `axis`, avoiding three normal loads.
    pub n: [i16; 3],
    pub c0: i16,
    pub c1: i16,
    /// 0 = generic, 1 = +X, 2 = +Y, 3 = +Z.
    pub axis: u8,
    pub dist_q5: i32,
}

// `axis` occupies the two bytes that were already padding before `dist`, so
// the decoded hot-path record does not grow.
const _: [(); 16] = [(); core::mem::size_of::<ClipNode>()];

#[derive(Clone, Copy)]
pub struct NavNode {
    pub pos: [i32; 3],
    #[allow(dead_code)]
    pub leaf: i16,
    pub first_link: usize,
    pub link_count: usize,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct LogicEnt {
    pub kind: u8,
    pub use_type: u8,
    pub spawnflags: u16,
    pub targetname: u16,
    pub target: u16,
    pub killtarget: u16,
    pub brush: u16,
    pub first_aux: usize,
    pub aux_count: usize,
    pub flags: u8,
    pub wait_ticks: i16,
    pub delay_ticks: u16,
    pub speed: u16,
    pub arg0: u16,
    pub arg1: u16,
    pub sound0: u8,
    pub sound1: u8,
    pub origin: [i32; 3],
    pub mins: [i32; 3],
    pub maxs: [i32; 3],
}

#[derive(Clone, Copy)]
pub struct LogicAux {
    pub target: u16,
    pub delay_ticks: u16,
}

#[derive(Clone, Copy)]
pub struct Ent {
    pub submodel: usize,
    pub kind: u16, // 0 static, 1 door, 2 visual, 3 button, 4 ladder, 5 fan, 7 axial brush, 8 platrot, 9 pushable, 10 pendulum, 11 rot button
    pub blend: u8, // 0 opaque, 1 average, 2 additive, 3 cutout; bit7 hidden
    pub origin: [i32; 3],
    pub mv: [i32; 3],
    pub center: [i32; 3],
    pub r2: i32,
    pub head: i32,  // submodel hull-1 clipnode root
    pub head0: i32, // submodel hull-0 BSP node root (point solidity for grates)
    pub leaf_start: usize,
    pub leaf_count: usize,
}

#[derive(Clone, Copy)]
pub struct RenderTri {
    pub idx: [u16; 3],
    pub tex: usize,
    pub uv_words: [u16; 3],
    pub rgb: [u32; 3],
}

/// PS1-ready loop vertex used by the hot world fan walker. Keeping the light
/// colour in the palette's packed 0x00BBGGRR form avoids unpacking three bytes,
/// spilling a nested tuple, and repacking the same colour into the GPU packet.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct PackedLoopVert {
    pub idx: u16,
    pub uv: u16,
    pub rgb: u32,
}

#[inline]
fn tri_corner(tri: RenderTri, corner: usize) -> PackedLoopVert {
    PackedLoopVert {
        idx: tri.idx[corner],
        uv: tri.uv_words[corner],
        rgb: tri.rgb[corner],
    }
}

#[inline]
fn same_packed_corner(a: PackedLoopVert, b: PackedLoopVert) -> bool {
    a.idx == b.idx && a.uv == b.uv && a.rgb == b.rgb
}

/// Return the four packet corners for two adjacent, consistently wound
/// triangles. The resulting GT4 splits as `(b,a,c) + (a,d,c)`, exactly the
/// two input triangles. Matching the complete packed shared corners prevents
/// a merge across a UV or light seam even when the geometric indices agree.
///
/// The renderer still performs its view-dependent near-plane, clamp, winding,
/// and opposite-side checks before accepting the quad. This helper proves only
/// the topology and attributes that are invariant across camera motion.
#[inline]
pub fn tri_pair_quad_corners(t0: RenderTri, t1: RenderTri) -> Option<[PackedLoopVert; 4]> {
    if t0.tex != t1.tex {
        return None;
    }
    let pairs = [(t0, t1), (t1, t0)];
    for (first, second) in pairs {
        for rotation in 0..3 {
            let b = tri_corner(first, rotation);
            let a = tri_corner(first, (rotation + 1) % 3);
            let c = tri_corner(first, (rotation + 2) % 3);
            if a.idx == b.idx || a.idx == c.idx || b.idx == c.idx {
                continue;
            }
            for second_rotation in 0..3 {
                let second_a = tri_corner(second, second_rotation);
                let d = tri_corner(second, (second_rotation + 1) % 3);
                let second_c = tri_corner(second, (second_rotation + 2) % 3);
                if same_packed_corner(a, second_a)
                    && same_packed_corner(c, second_c)
                    && d.idx != a.idx
                    && d.idx != b.idx
                    && d.idx != c.idx
                {
                    return Some([a, b, c, d]);
                }
            }
        }
    }
    None
}

/// Apply the cook-time pair solution without repeating the 18-way search. The
/// low two bits select the second rotation, bits 2..3 the first rotation, and
/// bit 4 swaps the two input triangles.
#[inline(always)]
pub fn encoded_pair_quad_corners(t0: RenderTri, t1: RenderTri, code: u8) -> [PackedLoopVert; 4] {
    let (first, second) = if code & 0x10 == 0 { (t0, t1) } else { (t1, t0) };
    let r0 = ((code >> 2) & 3) as usize;
    let r1 = (code & 3) as usize;
    [
        tri_corner(first, (r0 + 1) % 3),
        tri_corner(first, r0),
        tri_corner(first, (r0 + 2) % 3),
        tri_corner(second, (r1 + 1) % 3),
    ]
}

#[cfg(test)]
mod tri_pair_tests {
    use super::{
        encoded_pair_quad_corners, tex_anim_frame_index, tri_pair_quad_corners, RenderTri,
    };

    fn tri(idx: [u16; 3], uv: [u16; 3], rgb: [u32; 3]) -> RenderTri {
        RenderTri {
            idx,
            tex: 7,
            uv_words: uv,
            rgb,
        }
    }

    #[test]
    fn adjacent_children_map_to_the_exact_gt4_diagonal() {
        let a = tri([0, 4, 2], [10, 40, 20], [100, 400, 200]);
        let b = tri([4, 1, 2], [40, 11, 20], [400, 110, 200]);
        let corners = tri_pair_quad_corners(a, b).expect("shared child edge");
        assert_eq!(
            [
                corners[0].idx,
                corners[1].idx,
                corners[2].idx,
                corners[3].idx,
            ],
            [4, 0, 2, 1]
        );
    }

    #[test]
    fn rotated_input_order_is_still_pairable() {
        let a = tri([2, 0, 4], [20, 10, 40], [200, 100, 400]);
        let b = tri([1, 2, 4], [11, 20, 40], [110, 200, 400]);
        assert!(tri_pair_quad_corners(a, b).is_some());
    }

    #[test]
    fn encoded_pair_solution_matches_the_runtime_search() {
        let a = tri([2, 0, 4], [20, 10, 40], [200, 100, 400]);
        let b = tri([1, 2, 4], [11, 20, 40], [110, 200, 400]);
        let searched = tri_pair_quad_corners(a, b).expect("shared child edge");
        // This pair is solved with the input order intact, rotating the first
        // triangle once and the second twice.
        let encoded = encoded_pair_quad_corners(a, b, 0x20 | (1 << 2) | 2);
        let fields = |corners: [super::PackedLoopVert; 4]| {
            corners.map(|corner| (corner.idx, corner.uv, corner.rgb))
        };
        assert_eq!(fields(encoded), fields(searched));
    }

    #[test]
    fn packed_uv_or_light_seams_stay_as_two_triangles() {
        let a = tri([0, 4, 2], [10, 40, 20], [100, 400, 200]);
        let uv_seam = tri([4, 1, 2], [41, 11, 20], [400, 110, 200]);
        let light_seam = tri([4, 1, 2], [40, 11, 20], [401, 110, 200]);
        assert!(tri_pair_quad_corners(a, uv_seam).is_none());
        assert!(tri_pair_quad_corners(a, light_seam).is_none());
    }

    #[test]
    fn a_single_shared_vertex_is_not_a_quad() {
        let a = tri([0, 1, 2], [0, 1, 2], [10, 11, 12]);
        let b = tri([2, 3, 4], [2, 3, 4], [12, 13, 14]);
        assert!(tri_pair_quad_corners(a, b).is_none());
    }

    #[test]
    fn texture_animation_holds_each_frame_for_two_tenths() {
        let frames = (0..10)
            .map(|tenth| tex_anim_frame_index(tenth, 3))
            .collect::<alloc::vec::Vec<_>>();
        assert_eq!(frames, [0, 0, 1, 1, 2, 2, 0, 0, 1, 1]);
        assert_eq!(tex_anim_frame_index(123, 0), 0);
    }
}

impl Map {
    pub fn load(data: &'static [u8]) -> Map {
        let magic = rd_u32(data, cooked::HEADER_MAGIC_OFFSET);
        let clip_plane_tag_mask = if cooked::supports_tagged_clip_planes(magic) {
            cooked::CLIP_PLANE_TAG_MASK
        } else {
            0
        };
        let n_verts = rd_u32(data, cooked::HEADER_VERT_COUNT_OFFSET) as usize;
        let n_tris = rd_u32(data, cooked::HEADER_TRI_COUNT_OFFSET) as usize;
        let n_texs = rd_u32(data, cooked::HEADER_TEXTURE_COUNT_OFFSET) as usize;
        let n_faces = rd_u32(data, cooked::HEADER_FACE_COUNT_OFFSET) as usize;
        let bsp_off = rd_u32(data, cooked::HEADER_BSP_OFFSET) as usize;
        let clip_off = rd_u32(data, cooked::HEADER_CLIP_OFFSET) as usize;
        let ent_off = rd_u32(data, cooked::HEADER_ENTITY_OFFSET) as usize;
        let tram_off = rd_u32(data, cooked::HEADER_TRAM_OFFSET) as usize;
        let prop_off = rd_u32(data, cooked::HEADER_PROP_OFFSET) as usize;
        let sky_tex_raw = rd_u32(data, cooked::HEADER_SKY_TEXTURE_OFFSET);
        let sky_tex_base = if sky_tex_raw == u32::MAX {
            SKY_TEX_NONE
        } else {
            sky_tex_raw as usize
        };
        let nav_off = rd_u32(data, cooked::HEADER_NAV_OFFSET) as usize;
        let logic_off = rd_u32(data, cooked::HEADER_LOGIC_OFFSET) as usize;

        let v_off = cooked::header_size(magic);
        // verts | u32 n_loopverts + FaceVert[5B] loop pool | TriRec[16B] (dirty
        // faces only, = n_tris) | light palette.
        let lv_count_off = v_off + n_verts * 6;
        let n_loopverts = rd_u32(data, lv_count_off) as usize;
        // Two parallel arrays: 4-aligned `idx | uv << 16` words, then one
        // light index per vertex. Same forty bits as the old interleaved
        // five-byte record, but the hot pair is one aligned word instead of
        // LWL + LWR. Mirror the cook's padding here and before the TriRecs.
        let lv_off = align4(lv_count_off + 4);
        let lv_light_off = lv_off + n_loopverts * 4;
        let tri_off = align4(lv_light_off + n_loopverts);
        let light_pal_off = tri_off + n_tris * TRI_SZ; // palette follows the raw tris
        let (dynamic_off, n_dynamic_faces, water_alpha_supported) =
            if cooked::supports_dynamic_lightmaps(magic) {
                let off = light_pal_off + 256 * 2;
                let packed = rd_u16(data, off);
                (off, (packed & 0x7fff) as usize, packed & 0x8000 != 0)
            } else {
                (0, 0, false)
            };

        let n_planes = rd_u32(data, bsp_off) as usize;
        let n_face_groups = rd_u32(data, bsp_off + 4) as usize;
        let n_nodes = rd_u32(data, bsp_off + 8) as usize;
        let leaf_counts = rd_u32(data, bsp_off + 12);
        let (n_leaves, n_visleaves) = if cooked::supports_split_leaf_counts(magic) {
            let n_leaves = (leaf_counts & 0xffff) as usize;
            let n_visleaves = (leaf_counts >> 16) as usize;
            (n_leaves, n_visleaves.min(n_leaves.saturating_sub(1)))
        } else {
            let n_leaves = leaf_counts as usize;
            (n_leaves, n_leaves.saturating_sub(1))
        };
        let n_marks = rd_u32(data, bsp_off + 16) as usize;
        let vis_len = rd_u32(data, bsp_off + 20) as usize;
        let planes_off = bsp_off + 24;
        let face_groups_off = planes_off + n_planes * PLANE_SZ;
        let faces_off = face_groups_off + n_face_groups * FACE_GROUP_SZ;
        let nodes_off = faces_off + n_faces * FACE_SZ;
        let leaves_off = nodes_off + n_nodes * NODE_SZ;
        // The cook 4-aligns before the mark array (leaves are 8 B but the BSP
        // region start parity varies per map); reading unaligned here shifted
        // every marksurface window one entry back on parity-2 maps -- each leaf
        // gained its neighbour's tail face and lost its own last face (the
        // map-dependent "missing geometry" bug).
        let marks_off = align4(leaves_off + n_leaves * LEAF_SZ);
        let vis_off = align4(marks_off + n_marks * 2);

        let n_clip = rd_u32(data, clip_off) as usize;
        // Reserved legacy field. GoldSrc hull 0 uses render-node root 0; new
        // cooks write -1 here so it can never alias a compact clipnode.
        let _legacy_hull0_head = rd_i32(data, clip_off + 4);
        let hull1_head = rd_i32(data, clip_off + 8);
        let hull3_head = rd_i32(data, clip_off + 12);
        let spawn_pos = [
            rd_i32(data, clip_off + 16),
            rd_i32(data, clip_off + 20),
            rd_i32(data, clip_off + 24),
        ];
        let spawn_yaw = rd_i32(data, clip_off + 28);
        let clipn_off = clip_off + 32;

        let n_models = rd_u32(data, ent_off) as usize;
        let models_off = ent_off + 4;
        let n_ents_off = models_off + n_models * 8;
        let n_ents = rd_u32(data, n_ents_off) as usize;
        let ents_off = n_ents_off + 4;
        let n_ent_leafs_off = ents_off + n_ents * ENT_SZ;
        let n_ent_leafs = rd_u32(data, n_ent_leafs_off) as usize;
        let ent_leafs_off = n_ent_leafs_off + 4;

        let tram_submodel = rd_u16(data, tram_off) as usize;
        let n_way = rd_u16(data, tram_off + 2) as usize;
        let (tram_speed, tram_start, _tram_wheels) =
            crate::tram_logic::decode_motion_word(rd_u32(data, tram_off + 4), n_way);
        let tram_head = rd_i32(data, tram_off + 8);
        let tram_base = [
            rd_i32(data, tram_off + 12),
            rd_i32(data, tram_off + 16),
            rd_i32(data, tram_off + 20),
        ];
        let way_off = tram_off + 24;

        let prop_counts = rd_u32(data, prop_off);
        let split_props = prop_counts & cooked::PROP_SPLIT_FORMAT != 0;
        let n_props = if split_props {
            (prop_counts & 0xFFFF) as usize
        } else {
            prop_counts as usize
        };
        let n_sprites = if split_props {
            ((prop_counts >> 16) & 0x7FFF) as usize
        } else {
            0
        };
        let props_off = prop_off + 4;
        let sprites_off = props_off + n_props * PROP_SZ;

        let n_nav = rd_u16(data, nav_off) as usize;
        let nav_meta = rd_u16(data, nav_off + 2);
        let nav_nodes_off = nav_off + 4;
        let nav_links_off = nav_nodes_off + n_nav * NAV_NODE_SZ;

        let n_logic = rd_u16(data, logic_off) as usize;
        let n_logic_aux = rd_u16(data, logic_off + 2) as usize;
        let n_logic_names = rd_u16(data, logic_off + 4) as usize;
        let logic_name_bytes = rd_u16(data, logic_off + 6) as usize;
        let logic_records_off = logic_off + 8;
        let logic_aux_off = logic_records_off + n_logic * LOGIC_SZ;
        let logic_name_offsets_off = logic_aux_off + n_logic_aux * 4;
        let logic_names_off = logic_name_offsets_off + n_logic_names * 2;
        // HLME/HLMF/HLMG: texture-animation chains follow the (4-aligned)
        // logic name blob; older magics have no such section.
        let (n_tex_anim, tex_anim_off) = if cooked::supports_texture_animation(magic) {
            let off = align4(logic_names_off + logic_name_bytes);
            (rd_u16(data, off) as usize, off + 4)
        } else {
            (0, 0)
        };

        Map {
            data,
            n_verts,
            n_tris,
            n_texs,
            n_faces,
            sky_tex_base,
            v_off,
            lv_off,
            lv_light_off,
            tri_off,
            light_pal_off,
            dynamic_off,
            n_dynamic_faces,
            water_alpha_supported,

            n_planes,
            n_face_groups,
            n_nodes,
            n_visleaves,
            n_marks,
            planes_off,
            face_groups_off,
            faces_off,
            nodes_off,
            leaves_off,
            marks_off,
            vis_off,
            vis_len,
            n_clip,
            hull1_head,
            hull3_head,
            spawn_pos,
            spawn_yaw,
            clipn_off,
            clip_plane_tag_mask,
            n_models,
            n_ents,
            models_off,
            ents_off,
            ent_leafs_off,
            n_ent_leafs,
            tram_submodel,
            tram_speed,
            tram_start,
            tram_head,
            tram_base,
            n_way,
            way_off,
            n_props,
            props_off,
            n_sprites,
            sprites_off,
            n_nav,
            nav_meta,
            nav_nodes_off,
            nav_links_off,
            n_logic,
            n_logic_aux,
            n_logic_names,
            logic_off: logic_records_off,
            logic_aux_off,
            logic_name_offsets_off,
            logic_names_off,
            n_tex_anim,
            tex_anim_off,
        }
    }

    /// `(primary_frames, alternate_frames)` of texture-animation chain `i` as
    /// compact texture ids. Chains are variable-length back-to-back records;
    /// per-map counts are tiny (a handful of blinking screens), so the walk is
    /// cheaper than caching offsets in resident RAM.
    pub fn tex_anim_chain(&self, i: usize) -> (&'static [u8], &'static [u8]) {
        let mut o = self.tex_anim_off;
        let mut c = 0usize;
        while c < i && c < self.n_tex_anim {
            o += 2 + self.data[o] as usize + self.data[o + 1] as usize;
            c += 1;
        }
        if c != i || i >= self.n_tex_anim {
            return (&[], &[]);
        }
        let np = self.data[o] as usize;
        let na = self.data[o + 1] as usize;
        (
            &self.data[o + 2..o + 2 + np],
            &self.data[o + 2 + np..o + 2 + np + na],
        )
    }

    /// `(model_type, origin, packed orientation, leaf)` for point prop/item `i`.
    /// Orientation retains yaw/body in its low half and pitch/roll in its high
    /// half, so the 24-byte record remains binary-compatible.
    #[inline]
    pub fn prop(&self, i: usize) -> (u16, [i32; 3], i32, i16) {
        let o = self.props_off + i * PROP_SZ;
        (
            rd_u16(self.data, o),
            [
                rd_i32(self.data, o + 4),
                rd_i32(self.data, o + 8),
                rd_i32(self.data, o + 12),
            ],
            rd_i32(self.data, o + 16),
            rd_i16(self.data, o + 2),
        )
    }

    /// Immutable packed orientation for point prop/item `i`.
    #[inline(always)]
    pub fn prop_orientation(&self, i: usize) -> i32 {
        rd_i32(self.data, self.props_off + i * PROP_SZ + 16)
    }

    /// Logic-name id of the prop's targetname (0 = unnamed); scripts and
    /// triggers address monsters through this. Bit 15 marks a direct-launch
    /// fallback and is stripped during prop initialization.
    #[inline]
    pub fn prop_name(&self, i: usize) -> u16 {
        rd_u16(self.data, self.props_off + i * PROP_SZ + 20)
    }

    /// Stable cross-map identity cooked into PropRec's former padding word.
    /// Bit 15 marks `globalname` on live props. On authored scientist corpses,
    /// which never transition, it marks a map-local static pose clip instead.
    #[inline]
    pub fn prop_carry_id(&self, i: usize) -> u16 {
        rd_u16(self.data, self.props_off + i * PROP_SZ + 22)
    }

    /// `(origin, leaf, targetname, packed)` for placed sprite `i`.
    /// `packed` holds local sprite id, initial/once flags, and half-width.
    #[inline]
    pub fn sprite_prop(&self, i: usize) -> ([i32; 3], i16, u16, u16) {
        let o = self.sprites_off + i * SPRITE_REC_SZ;
        (
            [
                rd_i16(self.data, o) as i32,
                rd_i16(self.data, o + 2) as i32,
                rd_i16(self.data, o + 4) as i32,
            ],
            rd_i16(self.data, o + 6),
            rd_u16(self.data, o + 8),
            rd_u16(self.data, o + 10),
        )
    }

    #[inline]
    pub fn waypoint(&self, i: usize) -> [i32; 3] {
        let o = self.way_off + i * 12;
        [
            rd_i32(self.data, o),
            rd_i32(self.data, o + 4),
            rd_i32(self.data, o + 8),
        ]
    }

    /// GoldSrc tracktrain forward look-ahead (`wheels`). Read it lazily from
    /// the already-resident room blob so the parsed `Map` grows by zero bytes.
    #[inline(always)]
    pub fn tram_wheels(&self) -> i32 {
        crate::tram_logic::decode_motion_word(rd_u32(self.data, self.way_off - 20), self.n_way).2
    }

    /// Authored speed change at waypoint `i` (the path_track's "speed" key in
    /// u/s; 0 = keep the current speed, matching HL's CPathTrack).
    #[inline]
    pub fn way_speed(&self, i: usize) -> i32 {
        rd_u16(self.data, self.way_off + self.n_way * 12 + i * 2) as i32
    }

    /// Fire-on-pass logic-name id at waypoint `i` (`path_track.message`, or a
    /// terminal `path_track.netname` fired by CFuncTrackTrain::DeadEnd; 0 =
    /// none). The intro ride's changelevels + station scripts fire this way --
    /// the train's passage, not the rider, is the trigger in HL.
    #[inline]
    pub fn way_pass(&self, i: usize) -> u16 {
        rd_u16(self.data, self.way_off + self.n_way * 14 + i * 2)
    }

    /// `(first_face, num_faces)` for BSP submodel `m` (0 = world).
    #[inline]
    pub fn submodel(&self, m: usize) -> (usize, usize) {
        let o = self.models_off + m * 8;
        (
            rd_u32(self.data, o) as usize,
            rd_u32(self.data, o + 4) as usize,
        )
    }

    #[inline]
    pub fn entity(&self, i: usize) -> Ent {
        let o = self.ents_off + i * ENT_SZ;
        let d = self.data;
        let raw_kind = rd_u16(d, o + 2);
        Ent {
            submodel: rd_u16(d, o) as usize,
            kind: raw_kind & 0xFF,
            blend: (raw_kind >> 8) as u8,
            origin: [rd_i32(d, o + 4), rd_i32(d, o + 8), rd_i32(d, o + 12)],
            mv: [rd_i32(d, o + 16), rd_i32(d, o + 20), rd_i32(d, o + 24)],
            center: [rd_i32(d, o + 28), rd_i32(d, o + 32), rd_i32(d, o + 36)],
            r2: rd_i32(d, o + 40),
            head: rd_i32(d, o + 44),
            head0: rd_i32(d, o + 48),
            leaf_start: rd_u16(d, o + 52) as usize,
            leaf_count: rd_u16(d, o + 54) as usize,
        }
    }

    #[inline]
    pub fn ent_leaf(&self, i: usize) -> usize {
        if i >= self.n_ent_leafs {
            return 0;
        }
        rd_u16(self.data, self.ent_leafs_off + i * 2) as usize
    }

    #[inline]
    pub fn nav_node(&self, i: usize) -> NavNode {
        let o = self.nav_nodes_off + i * NAV_NODE_SZ;
        let d = self.data;
        NavNode {
            pos: [rd_i32(d, o), rd_i32(d, o + 4), rd_i32(d, o + 8)],
            leaf: rd_i16(d, o + 12),
            first_link: rd_u16(d, o + 14) as usize,
            link_count: d[o + 16] as usize,
        }
    }

    #[inline]
    pub fn nav_link(&self, i: usize) -> usize {
        let count = (self.nav_meta & !NAV_EXACT_ROUTES) as usize;
        if self.nav_meta & NAV_EXACT_ROUTES != 0 || i >= count {
            return 0;
        }
        rd_u16(self.data, self.nav_links_off + i * 2) as usize
    }

    #[inline]
    pub fn nav_has_exact_routes(&self) -> bool {
        self.nav_meta & NAV_EXACT_ROUTES != 0
    }

    /// GoldSrc `CGraph::NextNodeInRoute`, operating directly on the cooker-
    /// repacked human-hull/door-capable table from the retail `.nod` file.
    /// Returning `current` means the source graph considers the destination
    /// unreachable (the SDK uses the same sentinel behavior).
    #[inline(never)]
    pub fn nav_route_next(&self, current: usize, dest: usize) -> usize {
        if !self.nav_has_exact_routes() || current >= self.n_nav || dest >= self.n_nav {
            return current;
        }
        let route_len = (self.nav_meta & !NAV_EXACT_ROUTES) as usize;
        let mut p = self.nav_links_off + self.nav_node(current).first_link;
        let end = self.nav_links_off + route_len;
        let mut left = dest + 1;
        while left > 0 && p < end {
            let phrase = self.data[p] as i8;
            p += 1;
            if phrase < 0 {
                let count = -(phrase as i16) as usize;
                if left <= count {
                    return dest;
                }
                left -= count;
            } else {
                if p >= end {
                    return current;
                }
                let delta = self.data[p] as i8 as i32;
                p += 1;
                let count = phrase as usize + 1;
                if left <= count {
                    let mut next = current as i32 + delta;
                    if next >= self.n_nav as i32 {
                        next -= self.n_nav as i32;
                    } else if next < 0 {
                        next += self.n_nav as i32;
                    }
                    return next as usize;
                }
                left -= count;
            }
        }
        current
    }

    #[inline]
    pub fn nav_node_type(&self, i: usize) -> u8 {
        if !self.nav_has_exact_routes() || i >= self.n_nav {
            return 1; // legacy nodes were all cooked from land info_node ents
        }
        self.data[self.nav_nodes_off + i * NAV_NODE_SZ + 16]
    }

    #[inline]
    pub fn logic(&self, i: usize) -> LogicEnt {
        let o = self.logic_off + i * LOGIC_SZ;
        let d = self.data;
        LogicEnt {
            kind: d[o],
            use_type: d[o + 1],
            spawnflags: rd_u16(d, o + 2),
            targetname: rd_u16(d, o + 4),
            target: rd_u16(d, o + 6),
            killtarget: rd_u16(d, o + 8),
            brush: rd_u16(d, o + 10),
            first_aux: rd_u16(d, o + 12) as usize,
            aux_count: d[o + 14] as usize,
            flags: d[o + 15],
            wait_ticks: rd_i16(d, o + 16),
            delay_ticks: rd_u16(d, o + 18),
            speed: rd_u16(d, o + 20),
            arg0: rd_u16(d, o + 22),
            arg1: rd_u16(d, o + 24),
            sound0: d[o + 26],
            sound1: d[o + 27],
            origin: [rd_i32(d, o + 28), rd_i32(d, o + 32), rd_i32(d, o + 36)],
            mins: [rd_i32(d, o + 40), rd_i32(d, o + 44), rd_i32(d, o + 48)],
            maxs: [rd_i32(d, o + 52), rd_i32(d, o + 56), rd_i32(d, o + 60)],
        }
    }

    /// AABB-only precheck for the per-tick touch hotlist. Logic records are
    /// 64 bytes; reject the overwhelmingly common non-overlap after reading
    /// only the six bounds words, before decoding the rest of the record.
    #[inline]
    pub fn logic_touches_bounds(&self, i: usize, pmins: [i32; 3], pmaxs: [i32; 3]) -> bool {
        let o = self.logic_off + i * LOGIC_SZ;
        let d = self.data;
        pmins[0] <= rd_i32(d, o + 52)
            && pmaxs[0] >= rd_i32(d, o + 40)
            && pmins[1] <= rd_i32(d, o + 56)
            && pmaxs[1] >= rd_i32(d, o + 44)
            && pmins[2] <= rd_i32(d, o + 60)
            && pmaxs[2] >= rd_i32(d, o + 48)
    }

    #[inline]
    pub fn logic_aux(&self, i: usize) -> LogicAux {
        if i >= self.n_logic_aux {
            return LogicAux {
                target: 0,
                delay_ticks: 0,
            };
        }
        let o = self.logic_aux_off + i * 4;
        LogicAux {
            target: rd_u16(self.data, o),
            delay_ticks: rd_u16(self.data, o + 2),
        }
    }

    #[allow(dead_code)]
    pub fn logic_name(&self, id: u16) -> &'static str {
        if id == 0 || id as usize > self.n_logic_names {
            return "";
        }
        let idx = id as usize - 1;
        let start = rd_u16(self.data, self.logic_name_offsets_off + idx * 2) as usize;
        let mut end = start;
        while self.logic_names_off + end < self.data.len()
            && self.data[self.logic_names_off + end] != 0
        {
            end += 1;
        }
        core::str::from_utf8(&self.data[self.logic_names_off + start..self.logic_names_off + end])
            .unwrap_or("")
    }

    #[inline(always)]
    pub fn clipnode(&self, i: usize) -> ClipNode {
        let o = self.clipn_off + i * CLIPNODE_SZ;
        let plane_ref = rd_u16(self.data, o);
        // Branchlessly version the reference. HLMA's mask is zero, preserving
        // all 16 index bits; HLMB/C/D extracts the high tag and clears it from the
        // 14-bit index. Only a generic node reads its three normal components.
        let tag_bits = plane_ref & self.clip_plane_tag_mask;
        let axis = (tag_bits >> 14) as u8;
        let plane = (plane_ref ^ tag_bits) as usize;
        // Clipnode plane references are cook-validated. Decode the 10-byte
        // plane directly here: routing this hot traversal through plane() kept
        // a bounds branch and an out-of-line call on every BSP node visit.
        let po = self.planes_off + plane * PLANE_SZ;
        let n = if axis == 0 {
            [
                rd_i16(self.data, po),
                rd_i16(self.data, po + 2),
                rd_i16(self.data, po + 4),
            ]
        } else {
            [0, 0, 0]
        };
        ClipNode {
            n,
            c0: rd_i16(self.data, o + 2),
            c1: rd_i16(self.data, o + 4),
            axis,
            dist_q5: rd_i32(self.data, po + 6),
        }
    }

    #[inline]
    pub fn vert(&self, i: usize) -> Vec3I16 {
        let o = self.v_off + i * 6;
        #[cfg(target_arch = "mips")]
        unsafe {
            // Runtime maps live in the u32-aligned MAP_BUF, HEADER_SIZE is
            // four-aligned, and every six-byte vertex begins on an even
            // address. Tell LLVM that fact explicitly: the generic byte-slice
            // decoder expanded each vertex into six LBU instructions plus
            // shifts/ORs before RTPS. Native i16 loads reduce that to three
            // memory instructions without changing the cooked format or RAM.
            let p = self.data.as_ptr().add(o).cast::<i16>();
            Vec3I16::new(p.read(), p.add(1).read(), p.add(2).read())
        }
        #[cfg(not(target_arch = "mips"))]
        {
            // Host-side parsers/tests may supply byte-aligned slices.
            Vec3I16::new(
                rd_i16(self.data, o),
                rd_i16(self.data, o + 2),
                rd_i16(self.data, o + 4),
            )
        }
    }

    /// One aligned u32 of a TriRec (the cook 4-aligns the array; word `w` of
    /// record `t`). Unchecked: t < n_tris is the caller contract and the cook
    /// sizes the section (this is the hottest data read in the game).
    #[inline(always)]
    fn tri_word(&self, t: usize, w: usize) -> u32 {
        unsafe {
            let p = self.data.as_ptr().add(self.tri_off + t * TRI_SZ + w * 4) as *const u32;
            *p
        }
    }

    #[inline]
    pub fn tri_uv_words(&self, t: usize) -> [u16; 3] {
        let w1 = self.tri_word(t, 1);
        let w2 = self.tri_word(t, 2);
        [(w1 >> 16) as u16, w2 as u16, (w2 >> 16) as u16]
    }

    /// Just the three vertex indices -- the cheap read used to project + cull a
    /// triangle before deciding whether to decode its (heavier) uv/rgb/tex.
    #[inline]
    pub fn tri_idx(&self, t: usize) -> [u16; 3] {
        let w0 = self.tri_word(t, 0);
        let w1 = self.tri_word(t, 1);
        [
            w0 as u16 & 0x3fff,
            (w0 >> 16) as u16 & 0x3fff,
            w1 as u16 & 0x3fff,
        ]
    }

    #[inline]
    pub fn tri_tex(&self, t: usize) -> usize {
        tex_anim_display((self.tri_word(t, 3) & 0xFF) as usize)
    }

    /// Look up a per-corner lightmap colour. Reads the palette pre-expanded at
    /// load (`expand_light_palette`): one static read instead of an uncached
    /// blob u16 load plus three shift/or unpacks per corner, on the hottest
    /// per-triangle path in the game.
    #[inline]
    pub(crate) fn light_word(&self, idx: u8) -> u32 {
        unsafe { *LIGHT_PAL_RGB.get_unchecked(idx as usize) }
    }

    #[inline]
    fn light_color(&self, idx: u8) -> (u8, u8, u8) {
        let c = self.light_word(idx);
        (c as u8, (c >> 8) as u8, (c >> 16) as u8)
    }

    /// Select the sparse qrad style planes for one face. The records are sorted
    /// by compact face id, so a tiny binary search is paid only by the handful
    /// of faces carrying switchable lighting; all ordinary faces reset the
    /// context immediately through their packed zero gate.
    #[inline]
    pub fn select_dynamic_face(&self, face: usize, active_styles: u32) {
        Self::reset_dynamic_face();
        // With every switchable style off, no face can add a light plane.
        if active_styles == 0 || self.dynamic_off == 0 || self.face_lightstyle(face) == 0 {
            return;
        }
        let records_off = self.dynamic_off + 4 + DYNAMIC_PALETTE_COLORS * 2;
        let mut lo = 0usize;
        let mut hi = self.n_dynamic_faces;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let o = records_off + mid * DYNAMIC_FACE_REC_SZ;
            let candidate = rd_u16(self.data, o) as usize;
            if candidate < face {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= self.n_dynamic_faces {
            return;
        }
        let o = records_off + lo * DYNAMIC_FACE_REC_SZ;
        if rd_u16(self.data, o) as usize != face {
            return;
        }
        let mut active = 0u8;
        for plane in 0..3 {
            let slot = unsafe { *self.data.get_unchecked(o + 6 + plane) };
            if slot != 0 && active_styles & (1u32 << slot) != 0 {
                active |= 1 << plane;
            }
        }
        if active == 0 {
            return;
        }
        let data_blob = records_off + self.n_dynamic_faces * DYNAMIC_FACE_REC_SZ;
        unsafe {
            DYNAMIC_TRI_FIRST = rd_u16(self.data, o + 2);
            DYNAMIC_TRI_COUNT = rd_u16(self.data, o + 4);
            DYNAMIC_DATA_OFF = data_blob + rd_u16(self.data, o + 10) as usize;
            DYNAMIC_ACTIVE_PLANES = active;
            DYNAMIC_LOOP_POOL = *self.data.get_unchecked(o + 9) != 0;
        }
    }

    #[inline(always)]
    pub fn reset_dynamic_face() {
        unsafe {
            DYNAMIC_TRI_FIRST = u16::MAX;
            DYNAMIC_TRI_COUNT = 0;
            DYNAMIC_ACTIVE_PLANES = 0;
            DYNAMIC_LOOP_POOL = false;
        }
    }

    #[inline(always)]
    fn dynamic_light_word(&self, idx: u8) -> u32 {
        // Pre-expanded with the brightness curve applied, exactly like the base
        // palette; the cooked indices never exceed the 64-colour table.
        let packed = unsafe {
            *DYNAMIC_PAL_555.get_unchecked((idx as usize) & (DYNAMIC_PALETTE_COLORS - 1))
        };
        let (r, g, b) = unpack_rgb555(packed);
        (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
    }

    #[inline(always)]
    fn add_light_words(base: u32, extra: u32) -> u32 {
        let r = ((base & 0xff) + (extra & 0xff)).min(255);
        let g = (((base >> 8) & 0xff) + ((extra >> 8) & 0xff)).min(255);
        let b = (((base >> 16) & 0xff) + ((extra >> 16) & 0xff)).min(255);
        r | (g << 8) | (b << 16)
    }

    #[inline]
    fn apply_dynamic_light(&self, tri: usize, mut rgb: [u32; 3]) -> [u32; 3] {
        if unsafe { DYNAMIC_LOOP_POOL } {
            return rgb;
        }
        let first = unsafe { DYNAMIC_TRI_FIRST } as usize;
        let count = unsafe { DYNAMIC_TRI_COUNT } as usize;
        if tri < first || tri >= first + count {
            return rgb;
        }
        let row = unsafe { DYNAMIC_DATA_OFF } + (tri - first) * DYNAMIC_TRI_BYTES;
        let active = unsafe { DYNAMIC_ACTIVE_PLANES };
        for plane in 0..3 {
            if active & (1 << plane) == 0 {
                continue;
            }
            for corner in 0..3 {
                let idx = unsafe { *self.data.get_unchecked(row + plane * 3 + corner) };
                rgb[corner] = Self::add_light_words(rgb[corner], self.dynamic_light_word(idx));
            }
        }
        rgb
    }

    #[inline]
    fn apply_dynamic_loop_light(&self, vertex: usize, mut rgb: u32) -> u32 {
        if !unsafe { DYNAMIC_LOOP_POOL } {
            return rgb;
        }
        let first = unsafe { DYNAMIC_TRI_FIRST } as usize;
        let count = unsafe { DYNAMIC_TRI_COUNT } as usize;
        if vertex < first || vertex >= first + count {
            return rgb;
        }
        let row = unsafe { DYNAMIC_DATA_OFF } + (vertex - first) * 3;
        let active = unsafe { DYNAMIC_ACTIVE_PLANES };
        for plane in 0..3 {
            if active & (1 << plane) != 0 {
                let idx = unsafe { *self.data.get_unchecked(row + plane) };
                rgb = Self::add_light_words(rgb, self.dynamic_light_word(idx));
            }
        }
        rgb
    }

    /// Pre-expand the 256-entry rgb555 light palette into bytes, with the
    /// options screen's brightness curve baked in. Call once after `load` (the
    /// palette is per-map); `reapply_brightness` repeats it in place when the
    /// level changes with the map still resident.
    pub fn expand_light_palette(&self) {
        unsafe {
            LIGHT_PAL_SRC = self.data.as_ptr();
            LIGHT_PAL_SRC_OFF = self.light_pal_off;
            DYNAMIC_PAL_SRC_OFF = self.dynamic_off;
        }
        expand_light_palettes();
    }

    #[inline]
    pub fn tri_rgb(&self, t: usize) -> [(u8, u8, u8); 3] {
        let w3 = self.tri_word(t, 3);
        [
            self.light_color((w3 >> 8) as u8),
            self.light_color((w3 >> 16) as u8),
            self.light_color((w3 >> 24) as u8),
        ]
    }

    #[inline]
    pub fn tri_rgb_words(&self, t: usize) -> [u32; 3] {
        let w3 = self.tri_word(t, 3);
        self.apply_dynamic_light(
            t,
            [
                self.light_word((w3 >> 8) as u8),
                self.light_word((w3 >> 16) as u8),
                self.light_word((w3 >> 24) as u8),
            ],
        )
    }

    #[inline]
    pub fn render_tri(&self, t: usize, uv_words: [u16; 3]) -> RenderTri {
        let w0 = self.tri_word(t, 0);
        let w1 = self.tri_word(t, 1);
        let w3 = self.tri_word(t, 3);
        RenderTri {
            idx: [
                w0 as u16 & 0x3fff,
                (w0 >> 16) as u16 & 0x3fff,
                w1 as u16 & 0x3fff,
            ],
            tex: tex_anim_display((w3 & 0xFF) as usize),
            uv_words,
            rgb: self.apply_dynamic_light(
                t,
                [
                    self.light_word((w3 >> 8) as u8),
                    self.light_word((w3 >> 16) as u8),
                    self.light_word((w3 >> 24) as u8),
                ],
            ),
        }
    }

    /// Recover the cook-time swap/rotation solution packed into the unused high
    /// two bits of this record's three 14-bit indices.
    #[inline(always)]
    pub fn tri_quad_pair_code(&self, t: usize) -> Option<u8> {
        let w0 = self.tri_word(t, 0);
        let w1 = self.tri_word(t, 1);
        let code = (((w0 as u16) >> 14) as u8)
            | ((((w0 >> 16) as u16 >> 14) as u8) << 2)
            | ((((w1 as u16) >> 14) as u8) << 4);
        (code & 0x20 != 0).then_some(code)
    }

    // ---- BSP / PVS ----

    #[inline]
    pub fn node(&self, i: usize) -> Node {
        let o = self.nodes_off + i * NODE_SZ;
        // Render-node plane references are remapped and cook-validated by
        // hl-bsp, just like clipnodes. Decode the compact plane in place so
        // every physics/visibility BSP visit avoids plane_q5's generic bounds
        // check and call boundary.
        let po = self.planes_off + rd_u16(self.data, o) as usize * PLANE_SZ;
        Node {
            n: [
                rd_i16(self.data, po),
                rd_i16(self.data, po + 2),
                rd_i16(self.data, po + 4),
            ],
            dist_q5: rd_i32(self.data, po + 6),
            c0: rd_i16(self.data, o + 2) as i32,
            c1: rd_i16(self.data, o + 4) as i32,
        }
    }

    #[inline]
    fn plane(&self, i: usize) -> ([i16; 3], i32) {
        let (n, dist_q5) = self.plane_q5(i);
        (n, q5_to_world_nearest(dist_q5))
    }

    #[inline]
    fn plane_q5(&self, i: usize) -> ([i16; 3], i32) {
        if i >= self.n_planes {
            return ([0, PLANE_NORMAL_ONE as i16, 0], 0);
        }
        let o = self.planes_off + i * PLANE_SZ;
        (
            [
                rd_i16(self.data, o),
                rd_i16(self.data, o + 2),
                rd_i16(self.data, o + 4),
            ],
            rd_i32(self.data, o + 6),
        )
    }

    #[inline]
    fn signed_plane(&self, plane_ref: i16) -> ([i16; 3], i32) {
        if plane_ref >= 0 {
            self.plane(plane_ref as usize)
        } else {
            let (n, d) = self.plane((-(plane_ref as i32) - 1) as usize);
            ([-n[0], -n[1], -n[2]], -d)
        }
    }

    /// `(visofs, mark_start, mark_count)` for leaf `i`.
    #[inline]
    pub fn leaf(&self, i: usize) -> (i32, usize, usize) {
        let o = self.leaves_off + i * LEAF_SZ;
        (
            rd_i32(self.data, o),
            rd_u16(self.data, o + 4) as usize,
            cooked::leaf_mark_count(rd_u16(self.data, o + 6)) as usize,
        )
    }

    /// Source BSP liquid class for leaf `i`: 0 ordinary, 1 water, 2 slime,
    /// 3 lava. This is the authoritative GoldSrc point-contents result used by
    /// swimming; func_water entities remain a separate moving-volume path.
    #[inline]
    pub fn leaf_liquid(&self, i: usize) -> u8 {
        // The world BSP rooted at node zero addresses leaf 0 plus exactly
        // dmodel[0].visleafs. Later records belong to brush submodels.
        if i > self.n_visleaves {
            return 0;
        }
        let o = self.leaves_off + i * LEAF_SZ;
        cooked::leaf_liquid(rd_u16(self.data, o + 6))
    }

    /// Face index referenced by marksurface `j`.
    #[inline]
    pub fn mark(&self, j: usize) -> usize {
        rd_u16(self.data, self.marks_off + j * 2) as usize
    }

    #[inline]
    pub fn face_plane(&self, f: usize) -> ([i16; 3], i32) {
        self.group_plane(self.face_group(f))
    }

    #[inline]
    pub fn group_plane(&self, group: usize) -> ([i16; 3], i32) {
        if group >= self.n_face_groups {
            return ([0, 4096, 0], 0);
        }
        self.signed_plane(rd_i16(
            self.data,
            self.face_groups_off + group * FACE_GROUP_SZ,
        ))
    }

    /// Fused plane-group decode for the PVS active-group hot loop. Both the
    /// group id and its signed plane reference are cook-validated, so avoid the
    /// two generic bounds branches and nested calls paid once per visible group
    /// per frame.
    #[inline(always)]
    pub fn cooked_group_plane(&self, group: usize) -> ([i16; 3], i32) {
        let plane_ref = rd_i16_aligned(self.data, self.face_groups_off + group * FACE_GROUP_SZ);
        let flipped = plane_ref < 0;
        let plane = if flipped {
            (-(plane_ref as i32) - 1) as usize
        } else {
            plane_ref as usize
        };
        let o = self.planes_off + plane * PLANE_SZ;
        let n = [
            rd_i16_aligned(self.data, o),
            rd_i16_aligned(self.data, o + 2),
            rd_i16_aligned(self.data, o + 4),
        ];
        // Plane records are 10 bytes, so the i32 distance alternates between
        // word-aligned and word+2. Two aligned halfword loads are exact in
        // both cases and avoid the four-byte reconstruction in this hot loop.
        let d_lo = rd_u16_aligned(self.data, o + 6) as u32;
        let d_hi = rd_u16_aligned(self.data, o + 8) as u32;
        let d = q5_to_world_nearest((d_lo | (d_hi << 16)) as i32);
        if flipped {
            ([-n[0], -n[1], -n[2]], -d)
        } else {
            (n, d)
        }
    }

    #[inline]
    pub fn face_group(&self, f: usize) -> usize {
        let p = unsafe { self.data.as_ptr().add(self.faces_off + f * FACE_SZ + 4) };
        unsafe {
            (u16::from_le_bytes([p.read(), p.add(1).read()]) & cooked::FACE_GROUP_MASK) as usize
        }
    }

    /// Compact authored dynamic-light gate (0 = ordinary baked lighting,
    /// 1 = face has one or more sparse lightstyle planes).
    #[inline]
    pub fn face_lightstyle(&self, f: usize) -> u8 {
        let o = self.faces_off + f * FACE_SZ;
        unsafe {
            let p = self.data.as_ptr().add(o);
            let packed_group = u16::from_le_bytes([p.add(4).read(), p.add(5).read()]);
            ((packed_group >> cooked::FACE_GROUP_BITS) as u8) & 1
        }
    }

    /// GoldSrc `sound/materials.txt` classification packed by the cooker into
    /// the otherwise-unused face metadata bits (zero/default = concrete).
    #[inline]
    pub fn face_material(&self, f: usize) -> u8 {
        let o = self.faces_off + f * FACE_SZ;
        unsafe {
            let p = self.data.as_ptr().add(o);
            let packed_group = u16::from_le_bytes([p.add(4).read(), p.add(5).read()]);
            (((packed_group >> 13) as u8) & 7) | ((p.add(15).read() >> 4) & 8)
        }
    }

    #[inline(never)]
    fn impact_material_faces(
        &self,
        pos: [i32; 3],
        normal_q12: [i32; 3],
        first: usize,
        count: usize,
    ) -> Option<(i64, u8)> {
        let end = first.saturating_add(count).min(self.n_faces);
        let mut best = i64::MAX;
        let mut material = None;
        for f in first.min(end)..end {
            let (n, dist) = self.face_plane(f);
            let alignment = ((n[0] as i32 * normal_q12[0])
                + (n[1] as i32 * normal_q12[1])
                + (n[2] as i32 * normal_q12[2]))
                >> 12;
            if alignment.abs() < PLANE_NORMAL_ONE / 2 {
                continue;
            }
            let plane_pos =
                ((n[0] as i32 * pos[0]) + (n[1] as i32 * pos[1]) + (n[2] as i32 * pos[2]))
                    >> PLANE_NORMAL_FRAC_BITS;
            let plane_error = (plane_pos - dist).abs();
            if plane_error > 6 {
                continue;
            }
            let (center, radius) = self.face_bounds(f);
            let dx = (pos[0] - center[0]) as i64;
            let dy = (pos[1] - center[1]) as i64;
            let dz = (pos[2] - center[2]) as i64;
            let distance2 = dx * dx + dy * dy + dz * dz;
            let reach = (radius + 8) as i64;
            if distance2 > reach * reach {
                continue;
            }
            // Plane agreement dominates; the centre term disambiguates two
            // coplanar materials sharing a leaf or brush submodel.
            let score = plane_error as i64 * 1_000_000 + distance2;
            if score < best {
                best = score;
                material = Some((score, self.face_material(f)));
            }
        }
        material
    }

    /// Material at a static-world impact. The normal-side nudge selects the
    /// visible leaf, reducing the cold crowbar lookup from every map face to
    /// that leaf's compact marksurface list.
    pub fn world_impact_material(&self, pos: [i32; 3], normal_q12: [i32; 3], leaf: i32) -> u8 {
        if leaf >= 0 {
            let (_, mark_first, mark_count) = self.leaf(leaf as usize);
            let mut best: Option<(i64, u8)> = None;
            let mut i = mark_first;
            while i < mark_first.saturating_add(mark_count).min(self.n_marks) {
                let f = self.mark(i);
                if let Some(found) = self.impact_material_faces(pos, normal_q12, f, 1) {
                    if best.map_or(true, |old| found.0 < old.0) {
                        best = Some(found);
                    }
                }
                i += 1;
            }
            if let Some((_, found)) = best {
                return found;
            }
        }
        let (first, count) = self.submodel(0);
        self.impact_material_faces(pos, normal_q12, first, count)
            .map_or(MATERIAL_CONCRETE, |(_, material)| material)
    }

    /// Material at an entity-local brush impact (caller applies the mover's
    /// inverse translation/rotation once, matching its collision trace).
    pub fn submodel_impact_material(
        &self,
        submodel: usize,
        local_pos: [i32; 3],
        local_normal_q12: [i32; 3],
    ) -> u8 {
        let (first, count) = self.submodel(submodel);
        self.impact_material_faces(local_pos, local_normal_q12, first, count)
            .map_or(MATERIAL_CONCRETE, |(_, material)| material)
    }

    #[inline]
    /// (center, radius): the face's frustum-cull bounding sphere.
    pub fn face_bounds(&self, f: usize) -> ([i32; 3], i32) {
        let o = self.faces_off + f * FACE_SZ + 6;
        let p = unsafe { self.data.as_ptr().add(o) };
        let h = |n: usize| unsafe { u16::from_le_bytes([p.add(n).read(), p.add(n + 1).read()]) };
        (
            [h(0) as i16 as i32, h(2) as i16 as i32, h(4) as i16 as i32],
            h(6) as i32,
        )
    }

    /// (first, count): a loop face's loop-vertex range, or a raw face's tri range.
    #[inline]
    pub fn face_tris(&self, f: usize) -> (usize, usize) {
        let o = self.faces_off + f * FACE_SZ;
        let p = unsafe { self.data.as_ptr().add(o) };
        let h = |n: usize| unsafe { u16::from_le_bytes([p.add(n).read(), p.add(n + 1).read()]) };
        (h(0) as usize, (h(2) & cooked::FACE_COUNT_MASK) as usize)
    }

    /// True if face `f` stores a vertex loop (fan at render time); else raw tris.
    #[inline]
    pub fn face_is_loop(&self, f: usize) -> bool {
        unsafe {
            self.data
                .as_ptr()
                .add(self.faces_off + f * FACE_SZ + 15)
                .read()
                & 1
                != 0
        }
    }

    /// True when `(first,count)` addresses fixed four-corner records in the
    /// loop-vertex pool. Native records preserve the source quad until runtime;
    /// bit15 on the fourth corner marks a three-corner fallback record.
    #[inline]
    pub fn face_is_patch(&self, f: usize) -> bool {
        unsafe {
            self.data
                .as_ptr()
                .add(self.faces_off + f * FACE_SZ + 15)
                .read()
                & 0x10
                != 0
        }
    }

    /// True when this native-patch face contains at least one positional quad
    /// stored as two GT3 records because its diagonal carries an authored UV
    /// or light seam. The cook flag avoids probing every ordinary GT3 record.
    #[inline]
    pub fn face_has_seamed_pair(&self, f: usize) -> bool {
        unsafe {
            self.data
                .as_ptr()
                .add(self.faces_off + f * FACE_SZ + 15)
                .read()
                & 0x20
                != 0
        }
    }

    /// True for a masked-cutout face the cook found solid brushwork close
    /// behind (grate over slabs). Only these take the cutout OT pull-forward;
    /// a freestanding cutout (gate, trim) pulled forward would paint over
    /// nearer opaque geometry instead.
    #[inline]
    pub fn face_cutout_backed(&self, f: usize) -> bool {
        unsafe {
            self.data
                .as_ptr()
                .add(self.faces_off + f * FACE_SZ + 15)
                .read()
                & 0x40
                != 0
        }
    }

    /// True when the cook refined this raw face into UV-grid cells. Children
    /// receive local painter keys; only explicitly tagged regular cells may
    /// combine their two coplanar triangles into one depth-safe GT4.
    #[inline]
    pub fn face_refined_topology(&self, f: usize) -> bool {
        unsafe {
            self.data
                .as_ptr()
                .add(self.faces_off + f * FACE_SZ + 15)
                .read()
                & 4
                != 0
        }
    }

    /// True for a liquid face that is directly coplanar with opaque terrain.
    /// The PS1 renderer biases it behind that terrain so subdivision cannot
    /// reverse the source BSP's surface ownership.
    #[inline]
    pub fn face_coplanar_backdrop(&self, f: usize) -> bool {
        unsafe {
            self.data
                .as_ptr()
                .add(self.faces_off + f * FACE_SZ + 15)
                .read()
                & 8
                != 0
        }
    }

    /// True if the face belongs to a liquid boundary. This identity is kept
    /// even when GoldSrc's map-wide VIS test requires opaque water.
    #[inline]
    pub fn face_liquid(&self, f: usize) -> bool {
        unsafe {
            self.data
                .as_ptr()
                .add(self.faces_off + f * FACE_SZ + 15)
                .read()
                & 2
                != 0
        }
    }

    /// True when this liquid face may be blended over geometry visible across
    /// the boundary. GoldSrc decides this once for the complete map.
    #[inline]
    pub fn face_translucent(&self, f: usize) -> bool {
        self.water_alpha_supported && self.face_liquid(f)
    }

    #[inline]
    pub fn water_alpha_supported(&self) -> bool {
        self.water_alpha_supported
    }

    #[inline]
    pub fn face_tex(&self, f: usize) -> usize {
        unsafe {
            self.data
                .as_ptr()
                .add(self.faces_off + f * FACE_SZ + 14)
                .read() as usize
        }
    }

    #[inline]
    fn loopvert_o(&self, v: usize) -> usize {
        self.lv_off + v * 4
    }

    #[inline]
    pub fn loop_vert_idx(&self, v: usize) -> u16 {
        rd_u16(self.data, self.loopvert_o(v))
    }

    #[inline]
    pub fn loop_vert_uv_word(&self, v: usize) -> u16 {
        rd_u16(self.data, self.loopvert_o(v) + 2)
    }

    #[inline]
    pub fn loop_vert_light(&self, v: usize) -> (u8, u8, u8) {
        self.light_color(self.data[self.lv_light_off + v])
    }

    /// Fused single-pass decode of one static loop vertex in PS1-ready packed
    /// form. One offset computation + one unaligned word read + one palette
    /// hit. Patch batches select this after one face-context test.
    #[inline]
    fn loop_vert_static(&self, v: usize) -> PackedLoopVert {
        let o = self.loopvert_o(v);
        unsafe {
            let idx_uv = self.data.as_ptr().add(o).cast::<u32>().read();
            let light = *self.data.as_ptr().add(self.lv_light_off + v);
            let rgb = *core::ptr::addr_of!(LIGHT_PAL_RGB)
                .cast::<u32>()
                .add(light as usize);
            PackedLoopVert {
                // Five-byte loop records are intentionally dense, so their
                // first word is not generally aligned. On MIPS, one explicit
                // unaligned word read becomes LWL/LWR instead of four LBU
                // loads plus shifts and ORs in every world-fan iteration.
                //
                // Bits 14/15 of the index word are cook-authored topology
                // flags (triangle/seamed markers, blocked edges, the runtime
                // subdivision bit), NOT part of the vertex index. The legacy
                // fan walk consumed them unmasked, so any flagged face that
                // fell back from the quad path projected through a garbage
                // index (real_index | 0x4000) -- an unchecked write into the
                // projection scratch on trusted paths, a bounds panic on
                // checked ones. Flag readers use `loop_vert_idx` (raw).
                idx: idx_uv as u16 & 0x3fff,
                uv: (idx_uv >> 16) as u16,
                rgb,
            }
        }
    }

    #[inline(always)]
    pub fn dynamic_loop_active() -> bool {
        unsafe { DYNAMIC_LOOP_POOL }
    }

    /// General loop-vertex decode used by rolling fans. It retains switchable
    /// qrad lightstyles; four-corner patches hoist this sparse-state decision
    /// through `patch_corners` instead.
    #[inline]
    pub fn loop_vert(&self, v: usize) -> PackedLoopVert {
        let mut vertex = self.loop_vert_static(v);
        vertex.rgb = self.apply_dynamic_loop_light(v, vertex.rgb);
        vertex
    }

    #[inline]
    pub fn patch_is_triangle(&self, base: usize, patch: usize) -> bool {
        self.loop_vert_idx(base + patch * 4 + 3) & 0x8000 != 0
    }

    /// The current GT3 and the following GT3 share a valid positional quad,
    /// but could not become one GT4 because their UV or light values differ on
    /// the diagonal. Runtime may correct them together without erasing that
    /// authored seam.
    #[inline]
    pub fn patch_is_seamed_pair_start(&self, base: usize, patch: usize) -> bool {
        self.loop_vert_idx(base + patch * 4 + 3) & 0xC000 == 0xC000
    }

    /// Boundary edges on which runtime midpoint insertion is forbidden. The
    /// source BSP loop contained an authored collinear point on these spans,
    /// so treating the collapsed span as one edge would reopen a T-junction.
    /// Edge order matches the runtime quad: q0-q1, q1-q3, q3-q2, q2-q0.
    #[inline]
    pub fn patch_blocked_edges(&self, base: usize, patch: usize) -> u8 {
        let first = base + patch * 4;
        let c0 = (self.loop_vert_idx(first) >> 14) as u8 & 1;
        let c1 = (self.loop_vert_idx(first + 1) >> 14) as u8 & 1;
        let c2 = (self.loop_vert_idx(first + 2) >> 14) as u8 & 1;
        let c3 = (self.loop_vert_idx(first + 3) >> 14) as u8 & 1;
        c0 | (c1 << 1) | (c3 << 2) | (c2 << 3)
    }

    /// Persistent runtime subdivision bit for a native quad. The cooker leaves
    /// bit 15 of corner zero unused; triangle/seamed markers live on corner
    /// three and blocked-edge flags use bit 14. Keeping ownership beside the
    /// source patch avoids four edge-table searches on policy-hold frames.
    #[inline(always)]
    pub unsafe fn patch_subdiv_active(&self, first: usize) -> bool {
        rd_u16(self.data, self.loopvert_o(first)) & 0x8000 != 0
    }

    #[inline(always)]
    pub unsafe fn set_patch_subdiv_active(&self, first: usize, active: bool) {
        let ptr = self
            .data
            .as_ptr()
            .add(self.loopvert_o(first))
            .cast::<u16>()
            .cast_mut();
        let word = ptr.read_unaligned();
        ptr.write_unaligned((word & 0x7fff) | ((active as u16) << 15));
    }

    #[inline(always)]
    pub fn loop_corners4(&self, first: usize) -> [PackedLoopVert; 4] {
        // Test the face context once per patch, not once per corner. Ordinary
        // static geometry then compiles to four straight packed decodes.
        if Self::dynamic_loop_active() {
            [
                self.loop_vert(first),
                self.loop_vert(first + 1),
                self.loop_vert(first + 2),
                self.loop_vert(first + 3),
            ]
        } else {
            [
                self.loop_vert_static(first),
                self.loop_vert_static(first + 1),
                self.loop_vert_static(first + 2),
                self.loop_vert_static(first + 3),
            ]
        }
    }

    #[inline]
    pub fn patch_corners(&self, base: usize, patch: usize) -> [PackedLoopVert; 4] {
        self.patch_corners_meta(base, patch).0
    }

    /// Decode one patch and retain the topology bits already present in the
    /// four index words. The renderer previously decoded the corners, masked
    /// those bits, then reread one word for the triangle marker and all four
    /// words for the blocked-edge mask.
    #[inline]
    pub fn patch_corners_meta(&self, base: usize, patch: usize) -> ([PackedLoopVert; 4], bool, u8) {
        let first = base + patch * 4;
        // `loop_vert_static` masks the topology bits out of the decoded
        // index, so read the flags from the raw index words.
        let f0 = (self.loop_vert_idx(first) >> 14) as u8;
        let f1 = (self.loop_vert_idx(first + 1) >> 14) as u8;
        let f2 = (self.loop_vert_idx(first + 2) >> 14) as u8;
        let f3 = (self.loop_vert_idx(first + 3) >> 14) as u8;
        let triangle = f3 & 2 != 0;
        let blocked_edges = (f0 & 1) | ((f1 & 1) << 1) | ((f3 & 1) << 2) | ((f2 & 1) << 3);
        (self.loop_corners4(first), triangle, blocked_edges)
    }

    /// Build a RenderTri from three absolute loop-vertex indices (the runtime fan
    /// of a loop face), carrying the face's single texture.
    #[inline]
    pub fn loop_render_tri(&self, tex: usize, va: usize, vb: usize, vc: usize) -> RenderTri {
        // Submodel loops can carry switchable lightstyles. Hoist their sparse
        // context test over the complete triangle instead of repeating it for
        // each of its three vertices.
        let (a, b, c) = if Self::dynamic_loop_active() {
            (self.loop_vert(va), self.loop_vert(vb), self.loop_vert(vc))
        } else {
            (
                self.loop_vert_static(va),
                self.loop_vert_static(vb),
                self.loop_vert_static(vc),
            )
        };
        RenderTri {
            idx: [a.idx, b.idx, c.idx],
            tex,
            uv_words: [a.uv, b.uv, c.uv],
            rgb: [a.rgb, b.rgb, c.rgb],
        }
    }

    pub fn vis(&self) -> &'static [u8] {
        &self.data[self.vis_off..self.vis_off + self.vis_len]
    }
}

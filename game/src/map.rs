//! Parse a cooked `.hlm` map (tools/hl-bsp --cook). HLMA adds BSP visibility
//! plus brush-entity leaf membership for PVS culling; HLMB additionally tags
//! exact positive axial clip planes for faster collision traversal. HLMC packs
//! the world model's GoldSrc PVS-cluster count separately from the BSP's total
//! leaf-record count (submodels append leaves which are not PVS bits).
//!
//!   magic "HLMA" | "HLMB" | "HLMC" | u32 n_verts,n_tris,n_texs,n_faces,bsp_off
//!     | u32 clip_off,ent_off,tram_off,prop_off,sky_tex_base,nav_off,logic_off
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
//!       HLMC   leaf_counts = n_leaves | (n_visleaves << 16)
//!     PlaneRec[10B] × n_planes = i16 normal[3], i32 dist
//!     FaceGroup[2B] × n_face_groups = signed plane reference
//!     FaceRec[16B] × n_faces
//!       FaceRec = u16 first, u16 count, u16 plane_group, i16 center[3],
//!                 u16 radius, u8 tex, u8 flags (bit0: 1=loop; bit1: translucent)
//!       loop face: (first,count) = loopvert range; raw face: = tri range
//!     nodes  (u16 plane, i16 c0, i16 c1) × n_nodes
//!     leaves (i32 visofs, u16 mark_start, u16 mark_count) × n_leaves
//!     marks  u16 × n_marks (4-aligned on BOTH sides -- see marks_off)
//!     vis    u8  × vis_len (pad 4)
//!   clip:
//!     u32 n_clip | i32 legacy_hull0_head | i32 hull1_head | i32 hull3_head |
//!     i32 spawn[3] |
//!     i32 spawn_yaw | ClipNode[6B] × n_clip
//!       HLMA plane_ref = untagged u16 plane index
//!       HLMB/C plane_ref = tag[15:14] | plane_index[13:0]
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
//!
//! Fields are read via `from_le_bytes` (include_bytes! is only byte-aligned).

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
#[inline(always)]
fn align4(x: usize) -> usize {
    (x + 3) & !3
}

#[inline(always)]
fn expand5(v: u16) -> u8 {
    ((v << 3) | (v >> 2)) as u8
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

const PLANE_SZ: usize = 10;
const FACE_GROUP_SZ: usize = 2;
const NODE_SZ: usize = 6;
// The cooker writes the fields back-to-back: 12 + 2 + 2 + 1 + 1 = 18 bytes.
// Do not use Rust's naturally aligned struct size here. A stale 20-byte stride
// corrupted every waypoint after node zero and made scripted actors walk into
// walls instead of following the authored info_node graph.
const NAV_NODE_SZ: usize = 18;
const NAV_EXACT_ROUTES: u16 = 0x8000;
pub const SKY_TEX_NONE: usize = usize::MAX;

pub struct Node {
    pub n: [i16; 3],
    pub dist: i32,
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
    lv_off: usize, // FaceVert[5B] loop pool (loop-faces index into this)
    tri_off: usize,
    light_pal_off: usize, // 256 rgb555 entries, indexed by light_idx
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
}

const LEAF_SZ: usize = 8; // visofs i32 + marks u16×2
const FACE_SZ: usize = 16; // first|count|plane_group|center[3]|radius|tex|flags
const TRI_SZ: usize = 16; // u16 idx[3] | u8 uv[6] | u8 tex | u8 light_idx[3]
const LOOPVERT_SZ: usize = 5; // u16 idx | u8 uv[2] | u8 light_idx
const CLIPNODE_SZ: usize = 6;
const ENT_SZ: usize = 56;
const PROP_SZ: usize = 24;
const SPRITE_REC_SZ: usize = 12;
const LOGIC_SZ: usize = 64;
const PROP_SPLIT_FORMAT: u32 = 0x8000_0000;
const HLM_MAGIC_HLMB: u32 = u32::from_le_bytes(*b"HLMB");
const HLM_MAGIC_HLMC: u32 = u32::from_le_bytes(*b"HLMC");
const CLIP_PLANE_TAG_MASK: u16 = 0xC000;

pub const SPRITE_ID_MASK: u16 = 0x000F;
pub const SPRITE_INITIAL_ON: u16 = 0x0010;
pub const SPRITE_ONCE: u16 = 0x0020;

pub const LOGIC_BRUSH_NONE: u16 = u16::MAX;
pub const LOGIC_FUNC_DOOR: u8 = 1;
pub const LOGIC_FUNC_BUTTON: u8 = 2;
pub const LOGIC_TRIGGER_ONCE: u8 = 3;
pub const LOGIC_TRIGGER_MULTIPLE: u8 = 4;
pub const LOGIC_TRIGGER_RELAY: u8 = 5;
pub const LOGIC_MULTI_MANAGER: u8 = 6;
pub const LOGIC_TRIGGER_AUTO: u8 = 7;
pub const LOGIC_TRIGGER_CHANGELEVEL: u8 = 8;
pub const LOGIC_INFO_LANDMARK: u8 = 9;
pub const LOGIC_TRIGGER_COUNTER: u8 = 10;
pub const LOGIC_TRIGGER_CHANGETARGET: u8 = 11;
pub const LOGIC_ITEM_SUIT: u8 = 12;
pub const LOGIC_ITEM_BATTERY: u8 = 13;
pub const LOGIC_TRIGGER_HURT: u8 = 14;
pub const LOGIC_FUNC_TRACKTRAIN: u8 = 15;
pub const LOGIC_FUNC_BREAKABLE: u8 = 16;
pub const LOGIC_TRIGGER_TELEPORT: u8 = 17;
pub const LOGIC_TRIGGER_PUSH: u8 = 18;
pub const LOGIC_TRIGGER_GRAVITY: u8 = 19;
pub const LOGIC_HEALTH_CHARGER: u8 = 20;
pub const LOGIC_HEV_CHARGER: u8 = 21;
pub const LOGIC_MONSTERMAKER: u8 = 22;
pub const LOGIC_SCRIPTED: u8 = 24;
pub const LOGIC_SCRIPTED_HAS_IDLE: u8 = 0x80; // flags high bit; selector stays in low 7 bits
pub const LOGIC_FUNC_TRAIN: u8 = 25;
pub const LOGIC_WEAPONSTRIP: u8 = 26;
pub const LOGIC_ENV_MESSAGE: u8 = 27; // titles.txt overlay: arg0 = text name id
pub const LOGIC_ENV_FADE: u8 = 28; // screen fade: arg0 = duration ticks
pub const LOGIC_MAP_FLAGS: u8 = 29; // worldspawn startdark/gametitle + chaptertitle
pub const LOGIC_CDTRACK: u8 = 30; // CD music trigger: arg0 = track (-1 stop)
pub const LOGIC_SENTENCE: u8 = 31; // scripted_sentence: arg0 = per-map voice id
pub const LOGIC_AMBIENT: u8 = 32; // ambient_generic speech: arg0 = per-map voice id
pub const LOGIC_ENV_SHAKE: u8 = 33; // env_shake: arg0 = amplitude, speed = duration ticks
pub const LOGIC_WALL_TOGGLE: u8 = 34; // func_wall_toggle: toggle brush draw+collision
pub const LOGIC_MULTISOURCE: u8 = 35; // AND-gate: arg0 = input count, arg1 = globalstate hash
pub const LOGIC_ENV_GLOBAL: u8 = 36; // sets a persistent global: arg0 = hash, arg1 = triggermode
pub const LOGIC_ENV_EXPLOSION: u8 = 37; // scripted explosion FX: arg0 = magnitude
pub const LOGIC_TANK: u8 = 38; // func_tank mountable gun: arg0 = damage, speed = fire cooldown
pub const LOGIC_BEAM: u8 = 39; // env_beam/env_laser: aux = start+end xyz, arg1 = half-width, speed = color
pub const LOGIC_ENV_SPARK: u8 = 40; // env_spark: origin sparks intermittently
pub const LOGIC_MONSTERCLIP: u8 = 41; // func_monsterclip: mins/maxs AABB blocks NPCs, not the player
pub const LOGIC_MOMENTARY: u8 = 42; // momentary_rot_button valve wheel: hold +use ramps target door
/// GoldSrc transition-volume marker. It is never touched/fired in normal
/// gameplay; changelevel snapshots use its targetname and cooked brush AABB.
pub const LOGIC_TRIGGER_TRANSITION: u8 = 43;
pub const LOGIC_FUNC_ROTATING: u8 = 44; // targeted fan: persistent angle + start/stop ramp

pub const USE_OFF: u8 = 0;
pub const USE_ON: u8 = 1;
pub const USE_TOGGLE: u8 = 3;

pub struct ClipNode {
    /// Generic plane normal. Tagged axial nodes leave this zero and carry the
    /// canonical positive axis in `axis`, avoiding three normal loads.
    pub n: [i16; 3],
    pub c0: i16,
    pub c1: i16,
    /// 0 = generic, 1 = +X, 2 = +Y, 3 = +Z.
    pub axis: u8,
    pub dist: i32,
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
    pub kind: u16, // 0 static, 1 door, 2 visual, 3 button, 4 ladder, 5 rotating, 8 platrot, 9 pushable
    pub blend: u8, // 0 opaque, 1 semi-transparent (glass), 2 additive (glows)
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

impl Map {
    pub fn load(data: &'static [u8]) -> Map {
        let magic = rd_u32(data, 0);
        let clip_plane_tag_mask = if magic == HLM_MAGIC_HLMB || magic == HLM_MAGIC_HLMC {
            CLIP_PLANE_TAG_MASK
        } else {
            0
        };
        let n_verts = rd_u32(data, 4) as usize;
        let n_tris = rd_u32(data, 8) as usize;
        let n_texs = rd_u32(data, 12) as usize;
        let n_faces = rd_u32(data, 16) as usize;
        let bsp_off = rd_u32(data, 20) as usize;
        let clip_off = rd_u32(data, 24) as usize;
        let ent_off = rd_u32(data, 28) as usize;
        let tram_off = rd_u32(data, 32) as usize;
        let prop_off = rd_u32(data, 36) as usize;
        let sky_tex_raw = rd_u32(data, 40);
        let sky_tex_base = if sky_tex_raw == u32::MAX {
            SKY_TEX_NONE
        } else {
            sky_tex_raw as usize
        };
        let nav_off = rd_u32(data, 44) as usize;
        let logic_off = rd_u32(data, 48) as usize;
        let v_off = 52;
        // verts | u32 n_loopverts + FaceVert[5B] loop pool | TriRec[16B] (dirty
        // faces only, = n_tris) | light palette.
        let lv_count_off = v_off + n_verts * 6;
        let n_loopverts = rd_u32(data, lv_count_off) as usize;
        let lv_off = lv_count_off + 4;
        // The cook 4-aligns the TriRec array (see the writer) so each record
        // decodes as four u32 loads; the reader MUST mirror the padding.
        let tri_off = align4(lv_off + n_loopverts * LOOPVERT_SZ);
        let light_pal_off = tri_off + n_tris * TRI_SZ; // palette follows the raw tris

        let n_planes = rd_u32(data, bsp_off) as usize;
        let n_face_groups = rd_u32(data, bsp_off + 4) as usize;
        let n_nodes = rd_u32(data, bsp_off + 8) as usize;
        let leaf_counts = rd_u32(data, bsp_off + 12);
        let (n_leaves, n_visleaves) = if magic == HLM_MAGIC_HLMC {
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
        let split_props = prop_counts & PROP_SPLIT_FORMAT != 0;
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
        let _ = logic_name_bytes;

        Map {
            data,
            n_verts,
            n_tris,
            n_texs,
            n_faces,
            sky_tex_base,
            v_off,
            lv_off,
            tri_off,
            light_pal_off,
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
        }
    }

    /// `(model_type, origin, yaw, leaf)` for point prop/item `i`.
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

    /// Logic-name id of the prop's targetname (0 = unnamed); scripts and
    /// triggers address monsters through this.
    #[inline]
    pub fn prop_name(&self, i: usize) -> u16 {
        rd_u16(self.data, self.props_off + i * PROP_SZ + 20)
    }

    /// Stable cross-map identity cooked into PropRec's former padding word.
    /// Bit 15 marks `globalname`; zero means the actor does not transition.
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
        // all 16 index bits; HLMB/C extracts the high tag and clears it from the
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
            dist: rd_i32(self.data, po + 6),
        }
    }

    #[inline]
    pub fn vert(&self, i: usize) -> Vec3I16 {
        let o = self.v_off + i * 6;
        Vec3I16::new(
            rd_i16(self.data, o),
            rd_i16(self.data, o + 2),
            rd_i16(self.data, o + 4),
        )
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
        [w0 as u16, (w0 >> 16) as u16, w1 as u16]
    }

    #[inline]
    pub fn tri_tex(&self, t: usize) -> usize {
        (self.tri_word(t, 3) & 0xFF) as usize
    }

    /// Look up a per-corner lightmap colour. Reads the palette pre-expanded at
    /// load (`expand_light_palette`): one static read instead of an uncached
    /// blob u16 load plus three shift/or unpacks per corner, on the hottest
    /// per-triangle path in the game.
    #[inline]
    fn light_word(&self, idx: u8) -> u32 {
        unsafe { *LIGHT_PAL_RGB.get_unchecked(idx as usize) }
    }

    #[inline]
    fn light_color(&self, idx: u8) -> (u8, u8, u8) {
        let c = self.light_word(idx);
        (c as u8, (c >> 8) as u8, (c >> 16) as u8)
    }

    /// Pre-expand the 256-entry rgb555 light palette into bytes. Call once
    /// after `load` (the palette is per-map).
    pub fn expand_light_palette(&self) {
        for i in 0..256 {
            let (r, g, b) = unpack_rgb555(rd_u16(self.data, self.light_pal_off + i * 2));
            unsafe { LIGHT_PAL_RGB[i] = (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) };
        }
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
        [
            self.light_word((w3 >> 8) as u8),
            self.light_word((w3 >> 16) as u8),
            self.light_word((w3 >> 24) as u8),
        ]
    }

    #[inline]
    pub fn render_tri(&self, t: usize, uv_words: [u16; 3]) -> RenderTri {
        let w0 = self.tri_word(t, 0);
        let w1 = self.tri_word(t, 1);
        let w3 = self.tri_word(t, 3);
        RenderTri {
            idx: [w0 as u16, (w0 >> 16) as u16, w1 as u16],
            tex: (w3 & 0xFF) as usize,
            uv_words,
            rgb: [
                self.light_word((w3 >> 8) as u8),
                self.light_word((w3 >> 16) as u8),
                self.light_word((w3 >> 24) as u8),
            ],
        }
    }

    // ---- BSP / PVS ----

    #[inline]
    pub fn node(&self, i: usize) -> Node {
        let o = self.nodes_off + i * NODE_SZ;
        let (n, dist) = self.plane(rd_u16(self.data, o) as usize);
        Node {
            n,
            dist,
            c0: rd_i16(self.data, o + 2) as i32,
            c1: rd_i16(self.data, o + 4) as i32,
        }
    }

    #[inline]
    fn plane(&self, i: usize) -> ([i16; 3], i32) {
        if i >= self.n_planes {
            return ([0, 4096, 0], 0);
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
            rd_u16(self.data, o + 6) as usize,
        )
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
        let plane_ref = rd_i16(self.data, self.face_groups_off + group * FACE_GROUP_SZ);
        let flipped = plane_ref < 0;
        let plane = if flipped {
            (-(plane_ref as i32) - 1) as usize
        } else {
            plane_ref as usize
        };
        let o = self.planes_off + plane * PLANE_SZ;
        let n = [
            rd_i16(self.data, o),
            rd_i16(self.data, o + 2),
            rd_i16(self.data, o + 4),
        ];
        let d = rd_i32(self.data, o + 6);
        if flipped {
            ([-n[0], -n[1], -n[2]], -d)
        } else {
            (n, d)
        }
    }

    #[inline]
    pub fn face_group(&self, f: usize) -> usize {
        rd_u16(self.data, self.faces_off + f * FACE_SZ + 4) as usize
    }

    #[inline]
    /// (center, radius): the face's frustum-cull bounding sphere.
    pub fn face_bounds(&self, f: usize) -> ([i32; 3], i32) {
        let o = self.faces_off + f * FACE_SZ + 6;
        (
            [
                rd_i16(self.data, o) as i32,
                rd_i16(self.data, o + 2) as i32,
                rd_i16(self.data, o + 4) as i32,
            ],
            rd_u16(self.data, o + 6) as i32,
        )
    }

    /// (first, count): a loop face's loop-vertex range, or a raw face's tri range.
    #[inline]
    pub fn face_tris(&self, f: usize) -> (usize, usize) {
        let o = self.faces_off + f * FACE_SZ;
        (
            rd_u16(self.data, o) as usize,
            rd_u16(self.data, o + 2) as usize,
        )
    }

    /// True if face `f` stores a vertex loop (fan at render time); else raw tris.
    #[inline]
    pub fn face_is_loop(&self, f: usize) -> bool {
        self.data[self.faces_off + f * FACE_SZ + 15] & 1 != 0
    }

    /// True if the face is a translucent surface (water/liquid) -- drawn with
    /// semi-transparent blending over the opaque world.
    #[inline]
    pub fn face_translucent(&self, f: usize) -> bool {
        self.data[self.faces_off + f * FACE_SZ + 15] & 2 != 0
    }

    #[inline]
    pub fn face_tex(&self, f: usize) -> usize {
        self.data[self.faces_off + f * FACE_SZ + 14] as usize
    }

    #[inline]
    fn loopvert_o(&self, v: usize) -> usize {
        self.lv_off + v * LOOPVERT_SZ
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
        self.light_color(self.data[self.loopvert_o(v) + 4])
    }

    /// Fused single-pass decode of one loop vertex in PS1-ready packed form.
    /// One offset computation + two halfword reads + one palette-table hit.
    #[inline]
    pub fn loop_vert(&self, v: usize) -> PackedLoopVert {
        let o = self.loopvert_o(v);
        let d = self.data;
        unsafe {
            PackedLoopVert {
                idx: rd_u16(d, o),
                uv: rd_u16(d, o + 2),
                rgb: *LIGHT_PAL_RGB.get_unchecked(*d.get_unchecked(o + 4) as usize),
            }
        }
    }

    /// Build a RenderTri from three absolute loop-vertex indices (the runtime fan
    /// of a loop face), carrying the face's single texture.
    #[inline]
    pub fn loop_render_tri(&self, tex: usize, va: usize, vb: usize, vc: usize) -> RenderTri {
        let a = self.loop_vert(va);
        let b = self.loop_vert(vb);
        let c = self.loop_vert(vc);
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

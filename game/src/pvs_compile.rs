//! Visible-face list compiler with the entry capacity separate from the face-id
//! bound.
//!
//! `psx_goldsrc::pvs_faces::compile_faces` reads `indices.len()` as both the
//! number of faces a room may have and the number of list entries it may emit.
//! The renderer's lists only ever hold the faces one PVS row (or the merge of
//! two) makes visible, a third of the largest room at most, so they are sized
//! from that bound (`MAX_PVS_FACES`, `MAX_PVS_GROUPS`) while faces keep their
//! 16-bit ids. This is the same walk with the two limits split: the face-id
//! bound is the marker bitmap's width, the entry capacity is `indices.len()`
//! and the group capacity is `active_groups.len()`. For any room whose row fits
//! the capacity the output is identical, entry for entry.

//! `Map::compile_visible_faces` is the walk the renderer runs. This module is the reference
//! it is checked against, and exists only in the `pvs-check` diagnostic build.

use psx_goldsrc::pvs_faces::{
    build_pvs_face_rec, FaceBuffers, FaceSource, FaceSummary, PVS_FACE_COUNT_MASK,
    PVS_GROUP_FACE_MASK, PVS_GROUP_LIQUID, PVS_LINK_END,
};

/// Compile one already-unioned visibility row into the caller's face lists.
///
/// # Panics
/// Panics on inconsistent buffer shapes, as `compile_faces` does.
#[optimize(size)]
pub fn compile_faces_capped<M: FaceSource>(
    m: &M,
    visibility: &[u8],
    animated: &[u32],
    previous_groups: usize,
    out: FaceBuffers<'_>,
) -> FaceSummary {
    let face_limit = out.marks.len() * 32;
    let entry_capacity = out.indices.len();
    let max_groups = out.group_first.len();
    let group_capacity = out.active_groups.len();
    let max_leaves = visibility.len() * 8;
    let texture_count = animated.len() * 32;
    assert!(face_limit <= u16::MAX as usize + 1 && max_groups <= u16::MAX as usize);
    assert!(out.next.len() >= entry_capacity);
    assert!(out.group_face.len() == max_groups && group_capacity <= max_groups);
    assert!(previous_groups <= group_capacity);
    assert!(out.animated_textures.len() == animated.len() && texture_count.is_power_of_two());
    let mut old_group = 0usize;
    while old_group < previous_groups {
        out.group_face[out.active_groups[old_group] as usize] = PVS_LINK_END;
        old_group += 1;
    }
    let mut summary = FaceSummary::default();
    out.animated_textures.fill(0);
    out.marks.fill(0);

    for i in 0..m.visible_leaf_count().min(max_leaves) {
        if visibility[i >> 3] & (1u8 << (i & 7)) == 0 {
            continue;
        }
        let leaf = i + 1;
        summary.leaf_count += 1;

        let (_, m0, mc) = m.leaf(leaf);
        for mj in m0..m0 + mc {
            if mj >= m.mark_count() {
                break;
            }
            let face = m.mark(mj);
            if face >= face_limit {
                continue;
            }
            let mark_word = face >> 5;
            let mark_bit = 1u32 << (face & 31);
            if out.marks[mark_word] & mark_bit != 0 {
                continue;
            }
            out.marks[mark_word] |= mark_bit;
            let (first, cnt) = m.face_tris(face);
            if cnt == 0 || first > u16::MAX as usize || cnt > PVS_FACE_COUNT_MASK as usize {
                continue;
            }

            if summary.face_count >= entry_capacity {
                continue;
            }

            let group = m.face_group(face);
            if group >= max_groups {
                continue;
            }
            if out.group_face[group] == PVS_LINK_END {
                if summary.group_count >= group_capacity {
                    break;
                }
                out.group_first[group] = PVS_LINK_END;
                out.group_face[group] = face as u16 & PVS_GROUP_FACE_MASK;
                out.active_groups[summary.group_count] = group as u16;
                summary.group_count += 1;
            }
            if m.face_liquid(face) {
                out.group_face[group] |= PVS_GROUP_LIQUID;
            }

            let entry = summary.face_count;
            out.indices[entry] = face as u16;
            let liquid = m.face_liquid(face);
            let face_tex = m.face_tex(face);
            let tex_index = face_tex & (texture_count - 1);
            let animated = animated[tex_index >> 5] & (1 << (tex_index & 31)) != 0;
            summary.has_translucent |= liquid;
            summary.has_texture_animation |= animated;
            if animated {
                let tex = face_tex & (texture_count - 1);
                out.animated_textures[tex >> 5] |= 1u32 << (tex & 31);
            }
            if entry < out.records.len() {
                out.records[entry] = build_pvs_face_rec(m, face);
            }
            summary.triangle_references += cnt;
            out.next[entry] = out.group_first[group];
            out.group_first[group] = entry as u16;
            summary.face_count += 1;
        }
    }

    summary
}

use crate::{MAX_FACES, MAX_FACE_GROUPS, MAX_PVS_FACES, MAX_PVS_FACE_RECS, MAX_PVS_GROUPS};
use psx_goldsrc::pvs_faces::PvsFaceRec;

static mut REF_INDEX: [u16; MAX_PVS_FACES] = [0; MAX_PVS_FACES];
static mut REF_NEXT: [u16; MAX_PVS_FACES] = [0; MAX_PVS_FACES];
static mut REF_REC: [PvsFaceRec; MAX_PVS_FACE_RECS] =
    [psx_goldsrc::pvs_faces::EMPTY_PVS_FACE_REC; MAX_PVS_FACE_RECS];
static mut REF_MARK: [u32; MAX_FACES.div_ceil(32)] = [0; MAX_FACES.div_ceil(32)];
static mut REF_GROUP_FIRST: [u16; MAX_FACE_GROUPS] = [PVS_LINK_END; MAX_FACE_GROUPS];
static mut REF_GROUP_FACE: [u16; MAX_FACE_GROUPS] = [PVS_LINK_END; MAX_FACE_GROUPS];
static mut REF_GROUP_ACTIVE: [u16; MAX_PVS_GROUPS] = [0; MAX_PVS_GROUPS];
static mut REF_GROUP_COUNT: usize = 0;
static mut REF_REBUILDS: u32 = 0;

/// Run the reference walk on the same row and panic, naming the first field that
/// differs, unless it produced exactly what the fast walk left in the live lists.
///
/// # Safety
/// Called from the render loop only, once per rebuild, after the fast walk.
#[allow(clippy::too_many_arguments)]
pub unsafe fn check_against_reference(
    m: &crate::map::Map,
    visibility: &[u8],
    animated: &[u32],
    fast: &FaceSummary,
    index: &[u16],
    next: &[u16],
    records: &[PvsFaceRec],
    marks: &[u32],
    group_face: &[u16],
    group_first: &[u16],
    group_active: &[u16],
) {
    // The reference needs its own copy of the animation mask output.
    let mut ref_animated = [0u32; crate::map::TEX_ANIM_MAX / 32];
    let reference = compile_faces_capped(
        m,
        visibility,
        animated,
        REF_GROUP_COUNT,
        FaceBuffers {
            indices: &mut *core::ptr::addr_of_mut!(REF_INDEX),
            next: &mut *core::ptr::addr_of_mut!(REF_NEXT),
            records: &mut *core::ptr::addr_of_mut!(REF_REC),
            marks: &mut *core::ptr::addr_of_mut!(REF_MARK),
            group_first: &mut *core::ptr::addr_of_mut!(REF_GROUP_FIRST),
            group_face: &mut *core::ptr::addr_of_mut!(REF_GROUP_FACE),
            active_groups: &mut *core::ptr::addr_of_mut!(REF_GROUP_ACTIVE),
            animated_textures: &mut ref_animated,
        },
    );
    REF_GROUP_COUNT = reference.group_count;
    REF_REBUILDS += 1;
    let fail = |what: &str| -> ! { panic!("pvs-check: {what} differs on rebuild {}", REF_REBUILDS) };
    if reference != *fast {
        fail("summary");
    }
    let faces = reference.face_count;
    if index[..faces] != (&*core::ptr::addr_of!(REF_INDEX))[..faces] {
        fail("indices");
    }
    if next[..faces] != (&*core::ptr::addr_of!(REF_NEXT))[..faces] {
        fail("next links");
    }
    let rec_count = faces.min(records.len());
    if records[..rec_count] != (&*core::ptr::addr_of!(REF_REC))[..rec_count] {
        fail("records");
    }
    if marks != &*core::ptr::addr_of!(REF_MARK) {
        fail("face marks");
    }
    let groups = reference.group_count;
    if group_active[..groups] != (&*core::ptr::addr_of!(REF_GROUP_ACTIVE))[..groups] {
        fail("active groups");
    }
    let mut g = 0usize;
    while g < groups {
        let group = group_active[g] as usize;
        if group_face[group] != (&*core::ptr::addr_of!(REF_GROUP_FACE))[group]
            || group_first[group] != (&*core::ptr::addr_of!(REF_GROUP_FIRST))[group]
        {
            fail("group heads");
        }
        g += 1;
    }
    if animated.len() != ref_animated.len() {
        fail("animation mask width");
    }
}

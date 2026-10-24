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

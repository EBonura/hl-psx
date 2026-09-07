# Changelog

## Unreleased

- Reserve 80 bytes for entity dispatch indices so crowded maps retain their
  trigger, timed-entity, spark, and beam lists instead of repeatedly scanning
  every entity. On the full post-lift chapter-two recording, moving laboratory
  performance improves from 6.98 to 7.23 FPS; moving gameplay overall improves
  from 12.26 to 12.32 FPS. Menus, loading, and stationary intervals are excluded.

- Reuse the shared diagonal when checking GPU quad size limits. On the fresh
  chapter-two recording this improves 12.82 to 12.87 rendered FPS excluding
  loading, with identical final display, VRAM and gameplay state.

- Skip unrelated entity records in the spark and beam fallback scans. The recorded
  Anomalous Materials run improves from 12.46 to 12.98 rendered FPS excluding
  loading; its late laboratory section improves from 4.79 to 7.04 FPS in PSoXide.
- Check carried-player clearance on rotating lifts so coordinate rounding into
  a side wall can use the existing bounded collision recovery.
- Scientists and guards now answer when spoken to, including refusals to follow before the disaster.
- Fixed the canteen microwave's overlapping buttons so repeated presses register
  on the next button instead of the one that already moved away. The dish does
  not explode yet.
- Fixed floating seated scientists and incorrect poses for dead scientists.
- Fixed missing scientist heads in crowded scenes and the overturned cabinet drawing through a body.
- Removed the redundant button legend from the pause menu.

## Source 2026.09.05.1

- Removed the unused packet-ready world renderer and 4x4 tessellation experiment.
- Kept the production 2x2 refinement and the existing cooked-map format.

This source cleanup does not replace the validated discs. No new binary download
accompanies this tag.

## Source 2026.09.05

This source snapshot is tagged `source-2026.09.05`. Download versions are
listed separately below; source cleanup does not replace an already published disc.

- Improved actor depth ordering within the existing PSX ordering-table buckets.
- Preserved rectangular wall grids across boundary welding to reduce affine texture distortion.
- Kept host-only helpers out of guest builds and prepared a source-only public export.

No new binary download accompanies this source snapshot.

The public release is source only and requires a local Half-Life installation.
The private combined-disc build is not an itch.io download.

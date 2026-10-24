# Segments: the 103 maps in play order, with journey coverage

Generated from the retail BSPs and `game/src/map_catalog.rs` by `tests/chain/segments.rs`:

```
rustc --edition=2021 -O tests/chain/segments.rs -o /tmp/segments
/tmp/segments "$HL_DIR/maps" game/src/map_catalog.rs > segments.tsv
```

The journey column was filled in by hand from the committed journeys under `tests/`. "Traversed" means a controller-only journey starts from the previous segment's end state and plays the segment to its exit; every other value is weaker than the completion standard. Segments with no parity chapter file are covered by the static entity census only: `hl-census map <name>` prints the same table for any map.

Counts: 12 traversed (all in `tests/chain-new-game.toml`; the chain also arrives in c1a1 and checks the arrival), 1 partly traversed (t0a0, start lift to the catwalk, in `tests/chain-hazard.toml`), 1 entry and a short walk (c4a1a), 2 narrow replays or probe discs (t0a0b, c1a1b), 87 with no journey.

Columns: actors = `monster_*` placements (not the `_dead` props); scripts = `scripted_*`; exits list `destination@landmark[trigger targetname]`, `END` is a `trigger_endsection`.

| # | map | chapter | exits | actors | scripts | managers | triggers | journey | parity |
|---|---|---|---|---|---|---|---|---|---|
| 1 | c0a0 | Black Mesa Inbound | c0a0a@c0a0toa[] | 15 | 17 | 6 | 7 | traversed (chain-new-game) | [black-mesa-inbound](black-mesa-inbound.md#c0a0) |
| 2 | c0a0a | Black Mesa Inbound | c0a0@c0a0toa[]<br>c0a0b@c0a0tob[] | 1 | 1 | 6 | 7 | traversed (chain-new-game) | [black-mesa-inbound](black-mesa-inbound.md#c0a0a) |
| 3 | c0a0b | Black Mesa Inbound | c0a0a@c0a0tob[]<br>c0a0c@c0a0btoc[transition] | 2 | 4 | 6 | 6 | traversed (chain-new-game) | [black-mesa-inbound](black-mesa-inbound.md#c0a0b) |
| 4 | c0a0c | Black Mesa Inbound | c0a0d@c0a0ctod[transition2]<br>c0a0b@c0a0btoc[transition] | 2 | 4 | 12 | 5 | traversed (chain-new-game) | [black-mesa-inbound](black-mesa-inbound.md#c0a0c) |
| 5 | c0a0d | Black Mesa Inbound | c0a0c@c0a0ctod[transition2]<br>c0a0e@c0a0dtoe[levelchangetoe] | 4 | 2 | 6 | 8 | traversed (chain-new-game) | [black-mesa-inbound](black-mesa-inbound.md#c0a0d) |
| 6 | c0a0e | Black Mesa Inbound | c0a0d@c0a0dtoe[levelchangetoe]<br>c1a0@c0a0toc1a0[changetoc1a0] | 0 | 5 | 7 | 8 | traversed (chain-new-game) | [black-mesa-inbound](black-mesa-inbound.md#c0a0e) |
| 7 | c1a0 | Anomalous Materials | c0a0e@c0a0toc1a0[changetoc1a0]<br>c1a0d@c1a0toc1a0d[] | 17 | 61 | 11 | 15 | traversed (chain-new-game) | [anomalous-materials](anomalous-materials.md#c1a0) |
| 8 | c1a0d | Anomalous Materials | c1a0a@c1a0dtoa[airlocktransition1]<br>c1a0@c1a0toc1a0d[] | 9 | 34 | 9 | 19 | traversed (chain-new-game) | [anomalous-materials](anomalous-materials.md#c1a0d) |
| 9 | c1a0a | Anomalous Materials | c1a0d@c1a0dtoa[airlocktransition1]<br>c1a0b@c1a0atob[] | 5 | 4 | 11 | 14 | traversed (chain-new-game) | [anomalous-materials](anomalous-materials.md#c1a0a) |
| 10 | c1a0b | Anomalous Materials | c1a0a@c1a0atob[]<br>c1a0c@c1a0btoc[change_level]<br>c1a0e@c1a0btoe[change_to_c1a0e] | 8 | 18 | 10 | 12 | traversed (chain-new-game) | [anomalous-materials](anomalous-materials.md#c1a0b) |
| 11 | c1a0e | Anomalous Materials | c1a0c@c1a0etoc[change_level]<br>c1a0b@c1a0btoe[] | 9 | 16 | 45 | 57 | traversed (chain-new-game) | [anomalous-materials](anomalous-materials.md#c1a0e) |
| 12 | c1a0c | Unforeseen Consequences | c1a1@c1a0catoc1a1[]<br>c1a0e@c1a0etoc[change_level] | 5 | 14 | 16 | 41 | traversed (chain-new-game) | [unforeseen-consequences](unforeseen-consequences.md#c1a0c) |
| 13 | c1a1 | Unforeseen Consequences | c1a1a@c1a1[]<br>c1a0c@c1a0catoc1a1[] | 7 | 4 | 5 | 17 | none | [unforeseen-consequences](unforeseen-consequences.md#c1a1) |
| 14 | c1a1a | Unforeseen Consequences | c1a1f@c1a1atof[]<br>c1a1@c1a1[] | 3 | 1 | 2 | 10 | none | [unforeseen-consequences](unforeseen-consequences.md#c1a1a) |
| 15 | c1a1f | Unforeseen Consequences | c1a1a@c1a1atof[]<br>c1a1b@c1a1b[] | 11 | 8 | 18 | 25 | none | [unforeseen-consequences](unforeseen-consequences.md#c1a1f) |
| 16 | c1a1b | Unforeseen Consequences | c1a1c@c1a1bc[]<br>c1a1f@c1a1b[] | 17 | 9 | 17 | 29 | narrow tape replay (unstick-vent, present-cap) | [unforeseen-consequences](unforeseen-consequences.md#c1a1b) |
| 17 | c1a1c | Unforeseen Consequences | c1a1b@c1a1bc[]<br>c1a1d@c1a1c/d[]<br>c1a1d@c1a1dtoc1a1c[]<br>c1a2@a1a2[changetoc1a2] | 11 | 0 | 9 | 34 | none | [unforeseen-consequences](unforeseen-consequences.md#c1a1c) |
| 18 | c1a1d | Unforeseen Consequences | c1a1c@c1a1c/d[]<br>c1a1c@c1a1dtoc1a1c[] | 0 | 0 | 0 | 4 | none | [unforeseen-consequences](unforeseen-consequences.md#c1a1d) |
| 19 | c1a2 | Office Complex | c1a2a@one[]<br>c1a1c@a1a2[changetoc1a1e]<br>c1a2d@c1a2toc1a2d[] | 68 | 23 | 14 | 58 | none | census only |
| 20 | c1a2a | Office Complex | c1a2b@ab[]<br>c1a2@one[] | 29 | 10 | 6 | 22 | none | census only |
| 21 | c1a2b | Office Complex | c1a2a@ab[]<br>c1a2c@vents[]<br>c1a3@c1a2_to_c1a3[transition_to_c1a3]<br>c1a2c@freezer[] | 20 | 22 | 11 | 45 | none | census only |
| 22 | c1a2c | Office Complex | c1a2b@freezer[]<br>c1a2b@vents[] | 37 | 4 | 0 | 19 | none | census only |
| 23 | c1a2d | Office Complex | c1a2@c1a2toc1a2d[] | 3 | 2 | 0 | 3 | none | census only |
| 24 | c1a3 | We've Got Hostiles | c1a3c@lm_c1a3_c0[]<br>c1a4@c1a3toc1a4[]<br>c1a2b@c1a2_to_c1a3[transition_to_c1a3]<br>c1a3d@lm_c1a3_0d[]<br>c1a3d@lm_c1a3_0d2[] | 15 | 13 | 13 | 47 | none | census only |
| 25 | c1a3a | We've Got Hostiles | c1a3d@lm_c1a3_da[c1a3a_changelevel]<br>c1a3b@lm_c1a3_ab[c1a3a_changelevel2] | 30 | 6 | 4 | 14 | none | census only |
| 26 | c1a3b | We've Got Hostiles | c1a3c@lm_c1a3_bc[]<br>c1a3a@lm_c1a3_ab[c1a3b_changelevel] | 5 | 0 | 2 | 13 | none | census only |
| 27 | c1a3c | We've Got Hostiles | c1a3@lm_c1a3_c0[]<br>c1a3b@lm_c1a3_bc[] | 3 | 0 | 2 | 7 | none | census only |
| 28 | c1a3d | We've Got Hostiles | c1a3a@lm_c1a3_da[c1a3_changelevel]<br>c1a3@lm_c1a3_0d[]<br>c1a3@lm_c1a3_0d2[] | 9 | 3 | 3 | 19 | none | census only |
| 29 | c1a4 | Blast Pit | c1a4k@c1a4toc1a4k[]<br>c1a3@c1a3toc1a4[] | 17 | 12 | 6 | 24 | none | census only |
| 30 | c1a4k | Blast Pit | c1a4b@c1a4k_to_c1a4b[]<br>c1a4@c1a4toc1a4k[] | 4 | 0 | 2 | 17 | none | census only |
| 31 | c1a4b | Blast Pit | c1a4d@c1a4atod[]<br>c1a4f@c1a4btoc[]<br>c1a4i@c1a4btoi3[c1a4bdoor3cl]<br>c1a4i@c1a4btoi5[c1a4bdoor5cl]<br>c1a4i@c1a4btoi1[c1a4idoor1cl]<br>c1a4k@c1a4k_to_c1a4b[] | 14 | 2 | 3 | 42 | none | census only |
| 32 | c1a4f | Blast Pit | c1a4b@c1a4btoc[] | 9 | 8 | 14 | 26 | none | census only |
| 33 | c1a4d | Blast Pit | c1a4e@c1a4dtoe[]<br>c1a4b@c1a4atod[]<br>c1a4e@c1a4dtoc1a4e[] | 9 | 0 | 2 | 6 | none | census only |
| 34 | c1a4e | Blast Pit | c1a4d@c1a4dtoe[]<br>c1a4d@c1a4dtoc1a4e[] | 17 | 0 | 0 | 6 | none | census only |
| 35 | c1a4i | Blast Pit | c1a4b@c1a4btoi3[c1a4bdoor3cl]<br>c1a4b@c1a4btoi5[c1a4bdoor5cl]<br>c1a4b@c1a4btoi1[c1a4idoor1cl]<br>c1a4g@c1a4itoc1a4g[] | 13 | 8 | 8 | 52 | none | census only |
| 36 | c1a4g | Blast Pit | c1a4j@c1a4gtoc2a1[]<br>c1a4i@c1a4itoc1a4g[] | 0 | 0 | 0 | 37 | none | census only |
| 37 | c1a4j | Blast Pit | c1a4g@c1a4gtoc2a1[]<br>c2a1@c1a4-c2a1[] | 1 | 0 | 2 | 7 | none | census only |
| 38 | c2a1 | Power Up | c1a4j@c1a4-c2a1[]<br>c2a2@c2a1-c2a2[]<br>c2a1b@c2a1c2a1b[] | 19 | 10 | 12 | 39 | none | census only |
| 39 | c2a1a | Power Up | c2a1b@lm1[] | 33 | 3 | 18 | 20 | none | census only |
| 40 | c2a1b | Power Up | c2a1a@lm1[]<br>c2a1@c2a1c2a1b[] | 7 | 6 | 5 | 14 | none | census only |
| 41 | c2a2 | On A Rail | c2a1@c2a1-c2a2[]<br>c2a2a@c2a2c2a2a[] | 5 | 3 | 3 | 18 | none | census only |
| 42 | c2a2a | On A Rail | c2a2b2@c2a2ac2a2b2[]<br>c2a2@c2a2c2a2a[]<br>c2a2b1@c2a2ac2a2b1[] | 39 | 0 | 5 | 10 | none | census only |
| 43 | c2a2b1 | On A Rail | c2a2a@c2a2ac2a2b1[]<br>c2a2c@c2a2b1c2a2c[]<br>c2a2c@c2a2b1c2a2c-2[] | 30 | 0 | 5 | 7 | none | census only |
| 44 | c2a2b2 | On A Rail | c2a2a@c2a2ac2a2b2[] | 41 | 0 | 1 | 7 | none | census only |
| 45 | c2a2c | On A Rail | c2a2b1@c2a2b1c2a2c[]<br>c2a2b1@c2a2b1c2a2c-2[]<br>c2a2d@c2a2cc2a2d[] | 16 | 0 | 0 | 5 | none | census only |
| 46 | c2a2d | On A Rail | c2a2e@c2a2dc2a2e[]<br>c2a2c@c2a2cc2a2d[] | 31 | 0 | 5 | 20 | none | census only |
| 47 | c2a2e | On A Rail | c2a2d@c2a2dc2a2e[]<br>c2a2f@c2a2ec2a2f[] | 24 | 1 | 6 | 12 | none | census only |
| 48 | c2a2f | On A Rail | c2a2e@c2a2ec2a2f[]<br>c2a2g@c2a2fc2a2g[]<br>c2a2g@c2a2fc2a2g2[] | 8 | 0 | 5 | 17 | none | census only |
| 49 | c2a2g | On A Rail | c2a3@c2a2/3[]<br>c2a2h@c2a2gc2a2h[c2a2gc2a2htrans]<br>c2a2f@c2a2fc2a2g[]<br>c2a2f@c2a2fc2a2g2[] | 10 | 7 | 3 | 15 | none | census only |
| 50 | c2a2h | On A Rail | c2a2g@c2a2gc2a2h[c2a2gc2a2htrans] | 5 | 1 | 4 | 13 | none | census only |
| 51 | c2a3 | Apprehension | c2a3a@c2a3/a[]<br>c2a2g@c2a2/3[] | 25 | 4 | 3 | 9 | none | census only |
| 52 | c2a3a | Apprehension | c2a3@c2a3/a[]<br>c2a3b@c2a3a/b[] | 21 | 3 | 4 | 17 | none | census only |
| 53 | c2a3b | Apprehension | c2a3a@c2a3a/b[]<br>c2a3c@c2a3b/c[] | 22 | 1 | 9 | 12 | none | census only |
| 54 | c2a3c | Apprehension | c2a3b@c2a3b/c[]<br>c2a3d@c2a3cd[] | 13 | 2 | 2 | 11 | none | census only |
| 55 | c2a3d | Apprehension | c2a3c@c2a3cd[]<br>c2a3e@c2a3de[gotodrag] | 6 | 2 | 6 | 8 | none | census only |
| 56 | c2a3e | Apprehension | c2a4@c2a3d4[]<br>c2a3d@c2a3de[] | 2 | 8 | 4 | 8 | none | census only |
| 57 | c2a4 | Residue Processing | c2a4a@lm1[]<br>c2a3e@c2a3d4[] | 0 | 1 | 4 | 6 | none | census only |
| 58 | c2a4a | Residue Processing | c2a4@lm1[]<br>c2a4b@lm2[] | 55 | 4 | 16 | 38 | none | census only |
| 59 | c2a4b | Residue Processing | c2a4a@lm2[]<br>c2a4c@lm1[] | 4 | 4 | 8 | 39 | none | census only |
| 60 | c2a4c | Residue Processing | c2a4b@lm1[]<br>c2a4d@lm2[] | 12 | 0 | 7 | 31 | none | census only |
| 61 | c2a4d | Questionable Ethics | c2a4e@lm1[]<br>c2a4c@lm2[] | 12 | 2 | 6 | 13 | none | census only |
| 62 | c2a4e | Questionable Ethics | c2a4d@lm1[]<br>c2a4f@lm2[]<br>c2a4g@c2a4e-c2a4g[]<br>c2a4f@lm4[] | 34 | 11 | 9 | 28 | none | census only |
| 63 | c2a4f | Questionable Ethics | c2a4e@lm2[]<br>c2a4e@lm4[] | 11 | 5 | 9 | 21 | none | census only |
| 64 | c2a4g | Questionable Ethics | c2a4e@c2a4e-c2a4g[]<br>c2a5@c2a4gc2a5[] | 3 | 0 | 0 | 4 | none | census only |
| 65 | c2a5 | Surface Tension | c2a4g@c2a4gc2a5[]<br>c2a5w@c2a5-c2a5w[] | 8 | 6 | 16 | 61 | none | census only |
| 66 | c2a5w | Surface Tension | c2a5x@wtoxa[]<br>c2a5@c2a5-c2a5w[]<br>c2a5x@wtoxb[] | 10 | 1 | 0 | 5 | none | census only |
| 67 | c2a5x | Surface Tension | c2a5w@wtoxa[]<br>c2a5w@wtoxb[]<br>c2a5a@c2a5a[] | 7 | 0 | 18 | 24 | none | census only |
| 68 | c2a5a | Surface Tension | c2a5b@c2a5b[]<br>c2a5x@c2a5a[] | 13 | 0 | 1 | 15 | none | census only |
| 69 | c2a5b | Surface Tension | c2a5a@c2a5b[]<br>c2a5c@c2a5_c[c2a5bexit] | 8 | 0 | 2 | 9 | none | census only |
| 70 | c2a5c | Surface Tension | c2a5d@c2a5d[]<br>c2a5b@c2a5_c[c2a5cexit] | 6 | 2 | 18 | 29 | none | census only |
| 71 | c2a5d | Surface Tension | c2a5e@c2a5e[]<br>c2a5c@c2a5d[] | 23 | 7 | 7 | 11 | none | census only |
| 72 | c2a5e | Surface Tension | c2a5d@c2a5e[]<br>c2a5f@c2a5e/f[] | 17 | 18 | 13 | 31 | none | census only |
| 73 | c2a5f | Surface Tension | c2a5e@c2a5e/f[]<br>c2a5g@c2a5ftog[] | 26 | 17 | 23 | 52 | none | census only |
| 74 | c2a5g | Surface Tension | c3a1@c2a5g/c3a1[]<br>c2a5f@c2a5ftog[] | 3 | 6 | 7 | 18 | none | census only |
| 75 | c3a1 | Forget About Freeman! | c3a1a@a1a1a[]<br>c3a1b@a1a1b[]<br>c2a5g@c2a5g/c3a1[] | 29 | 0 | 19 | 50 | none | census only |
| 76 | c3a1a | Forget About Freeman! | c3a1@a1a1a[]<br>c3a1b@a1a2[changetoc1a2] | 35 | 0 | 7 | 19 | none | census only |
| 77 | c3a1b | Forget About Freeman! | c3a1a@a1a2[changetoc1a2]<br>c3a2e@c3a1c3a2[c3a1toc3a2] | 10 | 0 | 13 | 20 | none | census only |
| 78 | c3a2e | Lambda Core | c3a2@c3a2e[]<br>c3a1b@c3a1c3a2[c3a2toc3a1] | 11 | 0 | 1 | 6 | none | census only |
| 79 | c3a2 | Lambda Core | c3a2a@c3a2f[]<br>c3a2e@c3a2e[] | 13 | 10 | 9 | 14 | none | census only |
| 80 | c3a2a | Lambda Core | c3a2b@c3a2_ab[]<br>c3a2@c3a2f[] | 23 | 4 | 21 | 27 | none | census only |
| 81 | c3a2b | Lambda Core | c3a2a@c3a2_ab[]<br>c3a2c@c3a2_bc[]<br>c3a2c@c3a2_cb[]<br>c3a2c@c3a2_bc2[] | 2 | 0 | 14 | 16 | none | census only |
| 82 | c3a2c | Lambda Core | c3a2d@c3a2_cd[]<br>c3a2b@c3a2_bc[]<br>c3a2b@c3a2_bc2[]<br>c3a2f@c3a2_cf[]<br>c3a2d@c3a2_fc[]<br>c3a2b@c3a2_cb[]<br>c3a2f@c3a2_cf2[] | 8 | 1 | 21 | 65 | none | census only |
| 83 | c3a2f | Lambda Core | c3a2c@c3a2_cf[]<br>c3a2c@c3a2_cf2[]<br>c3a2c@c3a2_fc[] | 0 | 0 | 1 | 9 | none | census only |
| 84 | c3a2d | Lambda Core | c3a2c@c3a2_cd[]<br>c4a1@c3a2_c4a1[change] | 7 | 26 | 27 | 38 | none | census only |
| 85 | c4a1 | Xen | c4a2@c4a2[change]<br>c3a2d@c3a2_c4a1[] | 0 | 3 | 22 | 25 | none | census only |
| 86 | c4a2 | Gonarch's Lair | c4a2a@c4a2_0a[]<br>c4a1@c4a2[] | 3 | 0 | 3 | 11 | none | census only |
| 87 | c4a2a | Gonarch's Lair | c4a2@c4a2_0a[]<br>c4a2b@c4a2_ab[] | 0 | 0 | 7 | 14 | none | census only |
| 88 | c4a2b | Gonarch's Lair | c4a2a@c4a2_ab[]<br>c4a1a@c4a1a[change] | 0 | 1 | 5 | 10 | none | census only |
| 89 | c4a1a | Interloper | c4a2b@c4a1a[]<br>c4a1b@c4a1b[change] | 9 | 0 | 25 | 75 | entry and a short walk (journey (Chapters row)) | census only |
| 90 | c4a1b | Interloper | c4a1a@c4a1b[]<br>c4a1c@c4a1c[change] | 17 | 15 | 16 | 41 | none | census only |
| 91 | c4a1c | Interloper | c4a1d@c4a1d[]<br>c4a1b@c4a1c[] | 6 | 18 | 10 | 15 | none | census only |
| 92 | c4a1d | Interloper | c4a1e@c4a1e[]<br>c4a1c@c4a1d[] | 13 | 0 | 16 | 19 | none | census only |
| 93 | c4a1e | Interloper | c4a1d@c4a1e[]<br>c4a1f@c4a1f[change] | 16 | 0 | 18 | 15 | none | census only |
| 94 | c4a1f | Interloper | c4a1e@c4a1f[]<br>c4a3@c4a3[change] | 0 | 0 | 16 | 14 | none | census only |
| 95 | c4a3 | Nihilanth | c5a1@c4a3toc5a1[change_to_c5a1]<br>c4a1f@c4a3[] | 11 | 1 | 21 | 77 | none | census only |
| 96 | c5a1 | Endgame | c4a3@c4a3toc5a1[change_to_c5a1] | 17 | 10 | 12 | 26 | none | census only |
| 97 | t0a0 | Hazard Course | t0a0a@t0a0t0a0a[] | 17 | 14 | 15 | 43 | start corridor to the catwalk (the way on is by water, see hazard-course.md) (chain-hazard; entry journeys: journey, save-repin, skill-save, hazard-load, pause-overlay, segment-select) | [hazard-course](hazard-course.md#t0a0) |
| 98 | t0a0a | Hazard Course | t0a0@t0a0t0a0a[]<br>t0a0b1@t0a0at0a0b[] | 9 | 8 | 9 | 35 | none | [hazard-course](hazard-course.md#t0a0a) |
| 99 | t0a0b | Hazard Course | t0a0a@t0a0at0a0b[]<br>t0a0c@t0a0bt0a0c[]<br>t0a0c@t0a0bt0a0c2[] | 26 | 12 | 18 | 30 | probe disc (hazard-panels) | [hazard-course](hazard-course.md#t0a0b) |
| 100 | t0a0b1 | Hazard Course | t0a0a@t0a0at0a0b[]<br>t0a0b2@t0a0b12[] | 14 | 5 | 3 | 19 | none | [hazard-course](hazard-course.md#t0a0b1) |
| 101 | t0a0b2 | Hazard Course | t0a0c@t0a0bt0a0c[]<br>t0a0c@t0a0bt0a0c2[]<br>t0a0b1@t0a0b12[] | 12 | 7 | 15 | 14 | none | [hazard-course](hazard-course.md#t0a0b2) |
| 102 | t0a0c | Hazard Course | t0a0b2@t0a0bt0a0c[]<br>t0a0b@t0a0bt0a0c2[]<br>t0a0d@a1a2[changetoc1a2] | 7 | 4 | 6 | 19 | none | [hazard-course](hazard-course.md#t0a0c) |
| 103 | t0a0d | Hazard Course | END<br>t0a0c@a1a2[changetoc1a1e] | 7 | 4 | 4 | 13 | none | [hazard-course](hazard-course.md#t0a0d) |

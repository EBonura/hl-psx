# Performance notes: what the R3000 and the 4 KB I-cache reward

Measured on the c0a0 window (polls 760..1700 with the intro) of the shipping
disc, 2026-11-08, with the emulator's cycle attribution
(`--cpu-cycle-profile-log`, `--ram-load-stall-line-log`,
`--icache-stall-line-log`, `--pc-line-log --pc-log-words`).

## Where a frame goes

Issue 36%, RAM load stalls 38% (stack spills 12%, data 26%), I-cache refill
19%, MMIO 3%, mul/div 2%. The I-cache figure is 0.53 stall cycles per
instruction issued: the render loop streams through the 4 KB direct-mapped
cache every frame, nothing stays resident between faces. Rules of thumb that
follow, and that the profile confirmed:

- A hot instruction costs about 1.5 cycles (issue plus its share of refill),
  a RAM load about 6.5 more. Fewer instructions and fewer loads on the hot
  path is the whole game; placement (the `psoxide-pgo place` layout) only
  trims conflict misses on top.
- Spill traffic is 10.7% of all instructions (sp-relative loads and stores)
  and 212M stall cycles: 15% of the window. It comes from functions that
  carry many live values across calls (play, try_emit_quad_ctx,
  emit_world_face_patches, emit_submodel_face, emit_pvs_walk).
- memcpy ran 363k times a window (2.5%): 32-byte `CVert`, `SVert` and
  `[PackedLoopVert; 4]` copies on the clip and split paths, and closures
  returning a 32-byte struct that LLVM did not inline.

## A helper on a >10% path is not cold (step 1, 2026-11-08)

Moving the quad core's affine route, refined-pair key and near-band split out
of line (so the plain GT4 path would carry fewer live values) measured
c0a0 -2.1%, c4a3 -2.9%, chain -1.7% at equal inlining rung. The core got
107 cycles cheaper per quad, but its hot bytes and spills did not shrink, and
the route, which runs on 59% of the quads, cost 598 cycles per call as a
function: it re-read the projected words and the corners, materialised the
ranking arguments, and streamed its own 488 hot bytes every time (about
7 cycles per 16 bytes of callee executed). Out-of-line helpers pay only when
the moved code runs on a small fraction of the calls, or when the caller's
own hot bytes shrink by more than the helper streams.

What did measure cheaper and stayed: decoding a patch record straight into the
array the core reads by reference (the returned tuple was decoded onto the
stack and copied again, sixteen RAM accesses per patch), and the core's
projection-cache store inline (the near-band correction out of line; inlined
at every projection site the store cost 2.5 KB).

# The cook manifest: how a stale cook is caught

`data/.hlpsx-cook.json` records what wrote `data/`. `cargo hl-build pack|disc|...` refuses to
build when it does not match, and tells you to run `cargo hl-build assets`.

What it checks (`verify_cook_manifest` in host/hl-build/main.rs):

* `cook_inputs_sha256`: a digest of every file the cooker reads: `host/hl-bsp`, `host/hl-content`,
  `shared/hl-format`, `host/hl-build/model_variants.rs`, the three game files the cooker includes by
  path (`game/src/env_bubbles.rs`, `weaponbox.rs`, `map_catalog.rs`) and the toolchain pins. A cooker
  change moves it and forces a recook; a game, test or pack-step change does not (they cannot change
  cooked bytes), so landings that touch only those need no recook and no hand-edit of the manifest.
* the PSoXide source, revision and hydrated tree it was cooked against;
* `cooked_tree_sha256`, a digest of `data/` itself, so edited or partial data is caught.

A manifest without `cook_inputs_sha256` (written before the field existed) is refused with the
digest in the message: nothing says which cooker wrote its data, so recook. Do not edit the manifest
to make a build pass; the failure that motivated this (10-31) was data cooked on 10-25 with a manifest
copied forward to 82fee48, which lacked the SCRIPT_EVENT_DEAD records and cost a journey 80 HP.

When you add a file the cooker reads (a new `#[path]` include or `include_str!` of a game file from
a host crate), add it to `COOK_INPUT_FILES`.

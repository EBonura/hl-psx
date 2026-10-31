# Resident RAM: where it goes and how a new entity type gets its state

Free RAM is the bytes between `.bss` end and the stack reserve (`cargo hl-build ram`,
floor in `game/ram-budget`). The game is one 2 MB image: code (`.text`) about 740 KB,
the shared map/model arena about 880 KB, and about 290 KB of other statics.

## Rule for a new entity type

Do not add a `[T; MAX_PROPS]` array. Every per-prop array costs its row size times
`LIVE_PROPS` (113) on every map, whether or not the map places the entity.

1. State every walker needs (health, position, anim clip, AI timer) stays in the per-prop
   arrays in `game/src/main.rs`. Add a field there only if it is read by generic code.
2. State only your kind reads or writes goes in a keyed pool, `prop_pool::PropPool<T, N>`:
   `get(pi)` (None = empty), `claim(pi, empty)` (finds or takes a free row), `release(pi)`,
   `clear()` at map load. `N` is the most props of that kind any room of the whole game
   places, plus a small spare. `game/build.rs` measures it from `data/rooms` (see
   `scan_flyers`, which emits `room_budget::MAX_FLYERS`) so a recook or a new chapter
   resizes the pool by itself. Copy that scan for a new kind: one function, one emitted
   constant, one `assert!` that the game's type id equals the scan's.
3. State needed on one map only (a boss, a set piece) lives in the module of that boss
   as a single static, as `nihilanth::N` does, and is reset at map load.
4. Release the row wherever the prop is respawned, killed or recycled (the places that
   used to write `EMPTY` into the array), and `clear()` at map load.
5. Keep the empty value all zero so the static is `.bss`.

A pool that is too small must degrade, not corrupt: `claim` returns `None` and the caller
treats it as empty state (the flight pool then only loses velocity smoothing).

## Per-prop array rows

`LIVE_PROPS` (113) is `MAX_PROPS` (128) minus the 15 transition-mailbox rows. Three arrays
keep 128 rows because their unused tail holds the per-map candidate lists (`PROP_ACTIVE`
pickups, `PROP_KIND` hostiles, `PROP_AI_TARGET` humans), and `PROP_NEAR_ENTS` keeps them
because its tail is the mailbox. Whole-game census (103 rooms): at most 109 props in a room.

## Not poolable (measured)

* `PROP_JUMP` (10 B a prop): every walker that loses its floor uses it.
* `PROP_SCRIPT_GOAL`, `PROP_SCRIPT_PLAY_CLIP` and friends look like scripted-actor state
  but are reused as airborne momentum for any pushed or falling walker and as the houndeye
  jump window, so they are generic walker state.

## The PGO rung decides how much "free RAM" there is

Free RAM is not a measure of code growth: `game/pgo/hot-callsite-threshold` (the sample-PGO
inlining rung) moves `.text` by kilobytes at constant source, and `cargo hl-build pgo` ships the
highest rung that links with a safe stack above the floor in `game/ram-budget` (2,048 B since the
21,920 B floor was dropped on 10-10). Same code, 10-24 link maps (hl-mem-perf): rung 70 44,140 B
free, 125 41,772, 250 36,540, 500 31,980, 1000 30,108; frame rate was best at 125..250, rung 1000
was 1% slower on two of the four maps. To buy headroom, lower the rung (or raise the floor so
`pgo` picks a lower one) and recollect the layout with `dfix.sh`; benchmark c4a3 and c0a0.

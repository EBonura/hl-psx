# Deliberate differences: world logic and movement (lane L3)

Each entry is an inventory row (see the row id) that the port keeps different from the original on purpose, with the
evidence. The integrator folds this file into the repository limits document.

## Capacity (RAM)

- **Delayed outputs, 64 at once** (E "Delayed-event queue capacity", `main.rs` `MAX_LOGIC_EVENTS`). The busiest map
  (c1a0e) holds 331 authored manager events in total and one manager of 32 (c3a2d has one of 33), counted from the
  retail entity lumps; managers fire at different times, so the pending set stays under 64. The New Game chain gate
  runs through c1a0e's cascade. A full queue drops the new output.
- **Global state, 32 slots keyed by a 16-bit hash** (A D8, `main.rs` `GLOBAL_HASH`). The 103 retail maps use 15 distinct
  `globalstate` names (c1a4fpower, c1a4doxy, c1a4dfuel, c2a1_train_power, c2a1a_power_master, c2a2g_trainblastdoors,
  c2a2bay1screen, c2a2bay2screen, c2a4e_alarms, stormdrain_ms, c3a2global1..4, hldemo_global_dish_ms); their hashes do
  not collide.
- **Dead `globalname` breakables, 32 remembered** (E GLOBAL_DEAD). Only breakables that broke are kept. Not yet part of a
  save (lane L1 owns the checkpoint).
- **Carried entities across a changelevel, 15** (A D7, recorded in `main.rs` at `CARRY_CAPACITY`).

## Not present in any shipped map

- **Water currents, slime and lava contents** (A B7, B8). A scan of the leaf contents of every retail BSP finds none of
  `CONTENTS_CURRENT_*`, slime or lava. Hazard pools are `trigger_hurt` volumes, which are modelled.

## Not reproduced, with the reason

- **Ledge braking** (A A2, edge friction x2 toward a drop): implemented and removed. It moves the New Game journey's
  ledge approach in c1a0e (gate step 48 timed out with it on, passed with it off) and costs about 400 bytes of the
  scarce image RAM; the effect is small (a player stops a little sooner at a drop).
- **Ladder strafe and mount assist** (A A17): the ladder code is the shared `psx-goldsrc` module and records its own
  limits in `ladder_logic.rs` (strafe is a third of the retail speed; ladders are taken as axis-aligned).
- **Door crush damage rate** (E "Door crush"): the original hurts once per server frame while a door is blocked, so the
  rate depends on the host's frame rate; the port hurts once per 20 Hz tick, the same amount per hit.
- **`env_shake` frequency** (E): the shake noise is drawn per presented frame; only amplitude, duration, radius and the
  grounded-player rule are authored.
- **Split doors** (E "Touching one half of a split door"): the SDK has no linking code, the port links touching leaves; a
  retail trace is needed to say which is right.
- **Track speed anticipation** (E `func_tracktrain`): the look-ahead is applied on the 55-node c0a0a route only; the
  other tracks interleave scripted stops and trackchange phases whose timing the compact cooker keeps node-based.
- **Drivable train reverse, `OnControls` gating, `func_trackchange` rotation** (E, #30): need a backward path walk and a
  rotating platform carrying the train; two `func_traincontrols` and six trackchange placements. Open.
- **`trigger_push` ONCE and the c2a5 flag-4 case** (E): all eight placements are start-off volumes fired by scripts to
  kick props (barrels, rocket bars, door debris); the port pushes neither items nor debris.
- **Conveyor texture scroll** (E, #30): brush texture scrolling does not exist in the renderer (lane L5).
- **Scripted-sequence state gate, `DelayStart`, `scripted_sentence` speaker choice** (E): coupled to the monster state
  machine of lane L2; open until its state API lands.

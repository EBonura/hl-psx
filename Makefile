# hl-psx -- a bring-your-own-assets Half-Life renderer for the PlayStation 1,
# built on the sibling PSoXide Rust SDK checkout.
# A plain `cargo build --release` already produces a PSX-EXE (see
# game/.cargo/config.toml + game/build.rs); these targets add disc packing.

ROOT     := $(CURDIR)
GAME     := $(ROOT)/game
PSOXIDE  ?= $(abspath $(ROOT)/../PSoXide)
MKISOPSX = $(PSOXIDE)/tools/mkisopsx
TARGET   := mipsel-sony-psx
DIST     := $(ROOT)/dist
CAPTURE_DIR ?= $(ROOT)/captures

# Where `make install` drops a playable disc so you can boot it from the PSoXide
# game library. Override via the environment or on the command line.
GAMES_DIR ?= $(HOME)/Downloads/ps1 games
GAME_NAME ?= Half-Life (hl-psx)
# The crate lives under game/ so its PSX-target .cargo/config can't leak onto
# the host tools (mkisopsx) built from PSoXide.
EXE      := $(GAME)/target/$(TARGET)/release/hl-psx.exe
FEATURES ?=
CARGO_FEATURE_ARGS := $(if $(strip $(FEATURES)),--features $(FEATURES),)
PSOXIDE_LAUNCH = cd $(PSOXIDE)/emu && cargo run -p frontend --release -- launch
PSOXIDE_DEV    = cargo run --manifest-path $(PSOXIDE)/tools/psoxide-dev/Cargo.toml --release --
PSOXIDE_SMOKE_STEPS ?= 50000000
PSOXIDE_GAMEPLAY_STEPS ?= 320000000
PSOXIDE_PROFILE_STEPS ?= 600000000
PSOXIDE_PROFILE_VISUAL_FRAMES ?= 24
PSOXIDE_PROFILE_FRAMES ?= 1200
PSOXIDE_MAP_SMOKE_STEPS ?= 240000000
PSOXIDE_MAP_SMOKE_VISUAL_FRAMES ?= 8
MAP_INDEX ?= 0
# Default gameplay route confirms New Game from the top-level menu.
# Menu assets + intro rendering run before menu::run primes its PadTracker. A
# pulse at tick 90 can finish before that prime and is then swallowed; tick 500
# is after menu readiness on the full 356 MB campaign disc and remains early
# enough that a profile spends virtually all of its measured rows in gameplay.
PSOXIDE_MENU_PLAY_PULSES ?= 0x4000@500+20

# ---- Source Half-Life assets (bring your own; never committed) ----
# We read the original GoldSrc files (WAD textures, BSP maps, MDL models)
# straight from your own Half-Life install. Default is the standard macOS Steam
# location; override HL_DIR if it lives elsewhere (a different Steam library, a
# retail copy, ...). Persist an override locally without editing this file by
# creating config.mk (git-ignored), e.g.:
#     echo 'HL_DIR = /path/to/Half-Life' > config.mk
-include config.mk
HL_DIR  ?= $(HOME)/Library/Application Support/Steam/steamapps/common/Half-Life
# GoldSrc base-game content lives in the valve/ sub-mod.
HL_GAME ?= $(HL_DIR)/valve

# Rust host-side content compilers. These are product build sources: a fresh
# Steam install is converted without Python or platform media utilities.
HLBSP    := $(ROOT)/host/hl-bsp
HLBSP_BIN := $(HLBSP)/target/release/hl-bsp
HLCONTENT := $(ROOT)/host/hl-content
HLCONTENT_BIN := $(HLCONTENT)/target/release/hl-content
MODEL_ROSTER := $(HLCONTENT)/model-roster.txt
MAP      ?= c1a0

.DEFAULT_GOAL := build
.PHONY: help psoxide-check build compile disc assets full-disc install run check-assets bsp-info cook rooms transition-props menu-assets clean psoxide-smoke psoxide-gameplay psoxide-profile psoxide-map-smoke psoxide-chart

help:
	@echo "hl-psx targets:"
	@echo "  make psoxide-check - verify the sibling PSoXide checkout"
	@echo "  make            - build + install into PSoXide (default; every build is live)"
	@echo "  make build      - same as 'make': build, pack, and install into PSoXide"
	@echo "  make compile    - fast EXE-only rebuild, no disc/install -> $(EXE)"
	@echo "  make disc       - compile + pack a burnable .bin/.cue into dist/"
	@echo "  make assets     - recook menu assets, rooms, and models from Half-Life"
	@echo "  make full-disc  - assets + disc"
	@echo "  make install    - pack + install the disc into the PSoXide game"
	@echo "                    library ($(GAMES_DIR))"
	@echo "  make run        - alias for build (build + drop in the library)"
	@echo "  make psoxide-smoke    - headless menu screenshot/hash via PSoXide"
	@echo "  make psoxide-gameplay - headless c1a0 gameplay screenshot/hash"
	@echo "  make psoxide-profile  - telemetry build + CSV/profile screenshot"
	@echo "  make psoxide-map-smoke MAP_INDEX=N - boot map N directly in PSoXide"
	@echo "  make psoxide-chart    - profile + HTML vblank chart"
	@echo "  make check-assets - verify the source Half-Life install (HL_DIR)"
	@echo "  make bsp-info   - geometry + texture-VRAM budget for one map (MAP=$(MAP))"
	@echo "  make cook       - cook a map to data/maps/<MAP>.hlm (MAP=$(MAP))"
	@echo "  make clean      - remove build output"
	@echo ""
	@echo "  Source assets read from HL_DIR (default: macOS Steam path)."
	@echo "  Override: make check-assets HL_DIR=/path/to/Half-Life"

psoxide-check:
	@test -f "$(PSOXIDE)/sdk/psoxide.ld" || (echo "PSoXide checkout not found: $(PSOXIDE)"; exit 1)
	@echo "PSoXide -> $(PSOXIDE)"

# `make` / `make build` build AND install into the PSoXide library, so every
# build is immediately playable there. `make compile` is the fast EXE-only path
# (no disc/install) for quick compile checks.
build: install
	@echo "build -> live in PSoXide ($(GAMES_DIR)/$(GAME_NAME))"

compile:
	cd $(GAME) && PSOXIDE="$(PSOXIDE)" cargo build --release $(CARGO_FEATURE_ARGS)
	@echo "EXE -> $(EXE)"

disc: compile
	@mkdir -p $(DIST)
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(EXE) \
		--out $(DIST)/hl-psx.bin \
		--volume HLPSX \
		--world-pack-rooms-dir $(ROOMS) \
		--world-pack-compress-rooms \
		--world-pack-extra-dir $(MODELPACK) \
		--world-pack-extra-dir $(SFXPACK) \
		--world-pack-extra-dir $(VOICEPACK) \
		--world-pack-extra-dir $(SPRITEPACK) \
		$(if $(wildcard $(ROOT)/data/music/tracks.txt),--cdda-track-list $(ROOT)/data/music/tracks.txt,)
	@echo "DISC -> $(DIST)/hl-psx.cue"

# HL music -> CDDA track payloads (data/music, git-ignored). The disc target
# appends them as audio tracks when present.
$(HLCONTENT_BIN): $(HLCONTENT)/Cargo.toml $(wildcard $(HLCONTENT)/src/*.rs)
	cd $(HLCONTENT) && cargo build --release

music-assets: $(HLCONTENT_BIN)
	$(HLCONTENT_BIN) music "$(HL_GAME)" $(ROOT)/data/music

# HL SFX -> SPU-ADPCM pack (one WORLD.PAK chunk, id 3000). Needs the sibling
# PSoXide checkout for the psxed audio-pack encoder.
sfx-assets: $(HLCONTENT_BIN)
	cd $(PSOXIDE) && cargo build --release -p psxed
	$(HLCONTENT_BIN) sfx "$(HL_GAME)/sound" $(SFXPACK)/chunk_3000.psxa \
		"$(PSOXIDE)/target/release/psxed"
	@cp data/menu/hud.tex $(SFXPACK)/chunk_3001.psxa  # HUD atlas streams from the pack
	@cp data/menu/menu.pak $(SFXPACK)/chunk_3003.psxa  # menu bg+logo stream from the pack (not baked)

# HL dialogue -> per-map 8kHz SPU-ADPCM voice packs (WORLD.PAK chunks
# 3100+idx) + manifest. Must run BEFORE `rooms` (the map cook reads the
# manifest to resolve scripted_sentence/ambient_generic to local voice ids).
voices-assets: $(HLCONTENT_BIN)
	cd $(PSOXIDE) && cargo build --release -p psxed
	$(HLCONTENT_BIN) voices "$(HL_GAME)" "$(MAPLIST)" $(VOICEPACK) \
		"$(PSOXIDE)/target/release/psxed"

# HL sprites -> per-map 4bpp billboard packs (WORLD.PAK chunks 3200+idx) +
# manifest. Must run BEFORE `rooms` (the map cook reads the manifest to resolve
# env_sprite/env_glow models to per-map sprite local ids).
sprites-assets: $(HLCONTENT_BIN)
	$(HLCONTENT_BIN) sprites "$(HL_GAME)" "$(MAPLIST)" $(SPRITEPACK)

assets: check-assets menu-assets voices-assets sprites-assets rooms models sfx-assets
	@echo "assets -> data/menu data/rooms data/models data/modelpack data/sfx data/voices data/sprites"

full-disc: assets disc

psoxide-smoke: disc
	@mkdir -p $(CAPTURE_DIR)
	$(PSOXIDE_LAUNCH) \
		--path $(DIST)/hl-psx.cue \
		--embedded-playtest \
		--steps $(PSOXIDE_SMOKE_STEPS) \
		--dump-hw $(CAPTURE_DIR)/hl-psx-menu-hw.ppm \
		--dump-display $(CAPTURE_DIR)/hl-psx-menu-display.ppm \
		--dump-hash
	@echo "SMOKE -> $(CAPTURE_DIR)/hl-psx-menu-display.ppm"

psoxide-gameplay: disc
	@mkdir -p $(CAPTURE_DIR)
	$(PSOXIDE_LAUNCH) \
		--path $(DIST)/hl-psx.cue \
		--embedded-playtest \
		--steps $(PSOXIDE_GAMEPLAY_STEPS) \
		--pad-pulses '$(PSOXIDE_MENU_PLAY_PULSES)' \
		--dump-hw $(CAPTURE_DIR)/hl-psx-gameplay-hw.ppm \
		--dump-display $(CAPTURE_DIR)/hl-psx-gameplay-display.ppm \
		--dump-hash
	@echo "GAMEPLAY -> $(CAPTURE_DIR)/hl-psx-gameplay-display.ppm"

psoxide-profile:
	$(MAKE) disc FEATURES=emulator-telemetry PSOXIDE="$(PSOXIDE)"
	@mkdir -p $(CAPTURE_DIR)
	$(PSOXIDE_LAUNCH) \
		--path $(DIST)/hl-psx.cue \
		--embedded-playtest \
		--steps $(PSOXIDE_PROFILE_STEPS) \
		--guest-visual-frames $(PSOXIDE_PROFILE_VISUAL_FRAMES) \
		--guest-frames $(PSOXIDE_PROFILE_FRAMES) \
		--pad-pulses '$(PSOXIDE_MENU_PLAY_PULSES)' \
		--profile-log $(CAPTURE_DIR)/hl-psx-profile.csv \
		--counter-log $(CAPTURE_DIR)/hl-psx-counter.csv \
		--dump-guest-profile \
		--guest-debug-log \
		--dump-hw $(CAPTURE_DIR)/hl-psx-profile-hw.ppm \
		--dump-hash
	@echo "PROFILE -> $(CAPTURE_DIR)/hl-psx-profile.csv"

psoxide-map-smoke:
	$(MAKE) disc FEATURES=emulator-telemetry,debug-map-boot PSOXIDE="$(PSOXIDE)"
	@mkdir -p $(CAPTURE_DIR)
	@mask=$$(printf "0x%04x" $$((0x0400 | $(MAP_INDEX)))); \
	out="$(CAPTURE_DIR)/hl-psx-map-$$(printf "%03d" $(MAP_INDEX))"; \
	echo "direct map boot MAP_INDEX=$(MAP_INDEX) pad=$$mask"; \
	$(PSOXIDE_LAUNCH) \
		--path $(DIST)/hl-psx.cue \
		--embedded-playtest \
		--steps $(PSOXIDE_MAP_SMOKE_STEPS) \
		--guest-visual-frames $(PSOXIDE_MAP_SMOKE_VISUAL_FRAMES) \
		--pad-pulses "$$mask@1+180" \
		--profile-log "$$out.csv" \
		--counter-log "$$out-counter.csv" \
		--dump-guest-profile \
		--guest-debug-log \
		--dump-display "$$out.ppm" \
		--dump-hash
	@echo "MAP SMOKE -> $(CAPTURE_DIR)/hl-psx-map-$$(printf "%03d" $(MAP_INDEX)).ppm"

psoxide-chart: psoxide-profile
	$(PSOXIDE_DEV) vblank-chart \
		--in $(CAPTURE_DIR)/hl-psx-profile.csv \
		--out $(CAPTURE_DIR)/hl-psx-profile.html \
		--title "hl-psx per-frame work"
	@echo "CHART -> $(CAPTURE_DIR)/hl-psx-profile.html"

# Cook streamed maps into WORLD.PAK chunks. Each menu room N gets two chunk IDs:
#   room_<2N>.psxc   = resident HLMA/HLMB/HLMC/HLMD world/collision/entity data
#   room_<2N+1>.psxc = temporary HLTX texture payload for VRAM upload
# Keep MAPLIST in the same order as `game/src/menu.rs`'s MAPS registry.
ROOMS := $(ROOT)/data/rooms
MODELPACK := $(ROOT)/data/modelpack
SFXPACK   := $(ROOT)/data/sfx
VOICEPACK := $(ROOT)/data/voices
SPRITEPACK := $(ROOT)/data/sprites
# Runnable campaign map registry, staying below the current static MAP_BUF
# budget. Oversized maps are intentionally omitted until the streamed map format
# grows a compression/chunking path. Keep this in sync with `game/src/menu.rs`.
MAPLIST := \
	c0a0 c0a0a c0a0b c0a0c c0a0d c0a0e c1a0 c1a0a \
	c1a0b c1a0c c1a0d c1a0e c1a1 c1a1a c1a1b c1a1c \
	c1a1d c1a1f c1a2 c1a2a c1a2b c1a2c c1a2d c1a3 \
	c1a3a c1a3b c1a3c c1a3d c1a4 c1a4b c1a4d c1a4e \
	c1a4f c1a4g c1a4i c1a4j c1a4k c2a1 c2a1a c2a1b \
	c2a2 c2a2a c2a2b1 c2a2b2 c2a2c c2a2d c2a2e c2a2f \
	c2a2g c2a2h c2a3 c2a3a c2a3b c2a3c c2a3d c2a3e \
	c2a4 c2a4a c2a4b c2a4c c2a4d c2a4e c2a4f c2a4g \
	c2a5 c2a5a c2a5b c2a5c c2a5d c2a5e c2a5f c2a5g \
	c2a5w c2a5x c3a1 c3a1a c3a1b c3a2 c3a2a c3a2b \
	c3a2c c3a2d c3a2e c3a2f c4a1 c4a1a c4a1b c4a1c \
	c4a1d c4a1e c4a1f c4a2 c4a2a c4a2b c4a3 c5a1
transition-props: $(HLCONTENT_BIN)
	$(HLCONTENT_BIN) transition-props "$(HL_GAME)/maps" \
		$(MODELPACK)/transition_props.txt $(MAPLIST)

rooms: transition-props models
	cd $(HLBSP) && cargo build --release
	@mkdir -p $(ROOMS)
	@rm -f $(ROOMS)/room_*.psxc $(ROOMS)/room_*.psxw
	@i=0; for m in $(MAPLIST); do \
		w=$$((i * 2)); t=$$((w + 1)); \
		CLIPS_MANIFEST=$(MODELPACK)/clips.txt \
		STUDIO_EVENTS_MANIFEST=$(MODELPACK)/studio_events.txt \
		VOICES_MANIFEST=$(VOICEPACK)/manifest.txt \
		SPRITES_MANIFEST=$(SPRITEPACK)/manifest.txt \
		TRANSITION_PROPS_MANIFEST=$(MODELPACK)/transition_props.txt \
		MAP_INDEX=$$i \
		$(HLBSP_BIN) --cook "$(HL_GAME)/maps/$$m.bsp" $(ROOMS)/room_$$w.psxc $(ROOMS)/room_$$t.psxc >/dev/null && \
		echo "  room_$$w/$$t = $$m"; i=$$((i+1)); \
	done
	@echo "rooms -> $(ROOMS) ($(words $(MAPLIST)) maps)"

# Extract the menu font (HL fonts.wad -> data/menu/hlfont.bin, git-ignored).
# The runtime include_bytes!'s it, so run this once before building.
menu-assets: $(HLCONTENT_BIN)
	$(HLCONTENT_BIN) menu "$(HL_GAME)" $(ROOT)/data/menu

# Install the playable disc into the PSoXide game library as its own folder with
# matching <name>.bin/.cue (the layout the library expects).
install: disc
	@mkdir -p "$(GAMES_DIR)/$(GAME_NAME)"
	@cp "$(DIST)/hl-psx.bin" "$(GAMES_DIR)/$(GAME_NAME)/$(GAME_NAME).bin"
	@awk -v bin="$(GAME_NAME).bin" 'BEGIN { replaced = 0 } \
		!replaced && /^[[:space:]]*FILE[[:space:]]/ { \
			print "FILE \"" bin "\" BINARY"; replaced = 1; next \
		} { print }' "$(DIST)/hl-psx.cue" \
		> "$(GAMES_DIR)/$(GAME_NAME)/$(GAME_NAME).cue.tmp"
	@mv "$(GAMES_DIR)/$(GAME_NAME)/$(GAME_NAME).cue.tmp" \
		"$(GAMES_DIR)/$(GAME_NAME)/$(GAME_NAME).cue"
	@echo "INSTALLED -> $(GAMES_DIR)/$(GAME_NAME)/"

run: install

# Verify the source Half-Life install resolves and report what's there. This is
# the smoke test for the asset path before any extractor exists.
check-assets:
	@if [ ! -d "$(HL_GAME)" ]; then \
	  echo "Half-Life assets NOT found at:"; \
	  echo "  $(HL_GAME)"; \
	  echo ""; \
	  echo "Install Half-Life via Steam, or point HL_DIR at your copy:"; \
	  echo "  make check-assets HL_DIR=/path/to/Half-Life"; \
	  echo "or persist it: echo 'HL_DIR = /path/to/Half-Life' > config.mk"; \
	  exit 1; fi
	@echo "Half-Life assets found: $(HL_GAME)"
	@printf '  WADs (textures) : %s\n' "$$(ls "$(HL_GAME)"/*.wad 2>/dev/null | wc -l | tr -d ' ')"
	@printf '  BSPs (maps)     : %s\n' "$$(ls "$(HL_GAME)"/maps/*.bsp 2>/dev/null | wc -l | tr -d ' ')"
	@printf '  MDLs (models)   : %s\n' "$$(ls "$(HL_GAME)"/models/*.mdl 2>/dev/null | wc -l | tr -d ' ')"
	@printf '  PAKs (archives) : %s\n' "$$(ls "$(HL_GAME)"/*.pak 2>/dev/null | wc -l | tr -d ' ')"

# Inspect one map's geometry + texture-VRAM budget (the M1 feasibility probe).
# Pick a map with MAP=<name>, e.g. `make bsp-info MAP=c1a0` (Black Mesa Inbound).
bsp-info:
	cd $(HLBSP) && cargo build --release
	@if [ ! -f "$(HL_GAME)/maps/$(MAP).bsp" ]; then \
	  echo "Map not found: $(HL_GAME)/maps/$(MAP).bsp"; \
	  echo "Run 'make check-assets' first, or set MAP=<name> / HL_DIR=<path>."; \
	  exit 1; fi
	$(HLBSP_BIN) "$(HL_GAME)/maps/$(MAP).bsp"

# Cook a map to split PS1-native .hlm/.hltx files for inspection. Runtime disc
# builds use `make rooms`.
cook:
	cd $(HLBSP) && cargo build --release
	@mkdir -p $(ROOT)/data/maps
	$(HLBSP_BIN) --cook "$(HL_GAME)/maps/$(MAP).bsp" $(ROOT)/data/maps/$(MAP).hlm $(ROOT)/data/maps/$(MAP).hltx
	@cp $(ROOT)/data/maps/$(MAP).hlm $(ROOT)/data/maps/current.hlm
	@cp $(ROOT)/data/maps/$(MAP).hltx $(ROOT)/data/maps/current.hltx
	@echo "current map -> $(MAP)"

# Cook the studio models the runtime include_bytes!'s (data/models). NPCs use a
# shared clip order: idle, move, attack, pain, death. The sequence ids come from
# the shipped GoldSrc MDL activity metadata.
WEAPON_CHUNK_BASE := 1000
NPC_TEX_CHUNK_SCIENTIST := 1100
NPC_TEX_CHUNK_BARNEY := 1101
NPC_TEX_CHUNK_HEADCRAB := 1102
ITEM_TEX_CHUNK_SUIT := 1200
ITEM_TEX_CHUNK_BATTERY := 1201
WEAPON_TEX_CHUNK_BASE := 2000
WEAPONLIST := v_9mmhandgun v_357 v_9mmar v_crossbow v_crowbar v_chub v_egon v_gauss v_grenade v_hgun v_rpg v_satchel v_satchel_radio v_shotgun v_squeak v_tripmine
models: $(HLCONTENT_BIN)
	cd $(HLBSP) && cargo build --release
	@mkdir -p $(ROOT)/data/models
	@mkdir -p $(MODELPACK)
	@rm -f $(MODELPACK)/chunk_*.psxm
	@# Viewmodels draw one static full-bright frame, so cook them --mdl4 "0:1":
	@# HMD5 drops the per-tri normals (--mdl6 retains them for callers that need
	@# them, but the viewmodel is flat-shaded) and only frame 0 is baked.
	@# That shrinks each chunk enough to keep more weapons resident.
	@i=0; for m in $(WEAPONLIST); do \
		chunk=$$(( $(WEAPON_CHUNK_BASE) + i )); \
		texchunk=$$(( $(WEAPON_TEX_CHUNK_BASE) + i )); \
		$(HLBSP_BIN) --mdl4 "$(HL_GAME)/models/$$m.mdl" "$(MODELPACK)/chunk_$$chunk.psxm" "0:1" "$(MODELPACK)/chunk_$$texchunk.psxm" >/dev/null; \
		$(HLCONTENT_BIN) merge-model "$(MODELPACK)/chunk_$$chunk.psxm" "$(MODELPACK)/chunk_$$texchunk.psxm"; \
		echo "  weapon chunk $$chunk (merged) = $$m ($$(wc -c < $(MODELPACK)/chunk_$$chunk.psxm) B)"; \
		i=$$((i+1)); \
	done
	@echo "  --- model roster (all streamed per-map): geom chunk 1300+id, tex chunk 1100+id ---"
	@# Actors use --mdl5: compact animation frames and the same quarter-unit /
	@# q12=1024 scale as --mdl6, without its runtime-unused per-triangle normals.
	@cp $(MODEL_ROSTER) $(MODELPACK)/roster.txt
	@while IFS='|' read -r t mdl seq; do \
	  case "$$t" in ''|'#'*) continue ;; esac; \
	  geom=$$((1300+t)); tex=$$((1100+t)); \
	  if $(HLBSP_BIN) --mdl5 "$(HL_GAME)/models/$$mdl.mdl" "$(MODELPACK)/chunk_$$geom.psxm" "$$seq" "$(MODELPACK)/chunk_$$tex.psxm" >/tmp/mck_$$mdl.log 2>&1; then \
	    if [ "$$t" -eq 15 ]; then \
	      echo "  T$$t $$mdl -> split $$geom/$$tex ($$(wc -c < $(MODELPACK)/chunk_$$geom.psxm)+$$(wc -c < $(MODELPACK)/chunk_$$tex.psxm) B)"; \
	    else \
	      $(HLCONTENT_BIN) merge-model "$(MODELPACK)/chunk_$$geom.psxm" "$(MODELPACK)/chunk_$$tex.psxm"; \
	      echo "  T$$t $$mdl -> merged $$geom ($$(wc -c < $(MODELPACK)/chunk_$$geom.psxm) B)"; \
	    fi; \
	  else echo "  T$$t $$mdl FAILED: $$(tail -1 /tmp/mck_$$mdl.log | cut -c1-60)"; fi; \
	done < $(MODEL_ROSTER)
	@$(HLCONTENT_BIN) clips $(MODELPACK)/roster.txt $(MODELPACK)/clips.txt
	@$(HLCONTENT_BIN) studio-events "$(HL_GAME)/models" \
		$(MODELPACK)/roster.txt $(MODELPACK)/studio_events.txt
	@rm -f $(MODELPACK)/roster.txt
	@echo "  clips manifest -> $(MODELPACK)/clips.txt ($$(wc -l < $(MODELPACK)/clips.txt) entries)"

clean:
	cd $(GAME) && cargo clean
	rm -rf $(DIST)

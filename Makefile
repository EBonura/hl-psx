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
PSOXIDE_PROFILE_FRAMES ?= 180
MEMORY_MAP ?= $(CAPTURE_DIR)/hl-psx.map
MIN_HEADROOM_KB ?= 96
# Default gameplay route selects c1a0: Down once, then Cross to play.
PSOXIDE_MENU_PLAY_PULSES ?= 0x0040@40+4,0x4000@90+8

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

# Host-side map inspector (tools/hl-bsp): geometry + texture-VRAM budget report.
HLBSP    := $(ROOT)/tools/hl-bsp
HLBSP_BIN := $(HLBSP)/target/release/hl-bsp
MAP      ?= c1a0

.DEFAULT_GOAL := build
.PHONY: help psoxide-check build compile disc assets full-disc install run check-assets bsp-info cook rooms menu-assets clean psoxide-smoke psoxide-gameplay psoxide-profile psoxide-chart memory-report

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
	@echo "  make psoxide-chart    - profile + HTML vblank chart"
	@echo "  make memory-report    - linker-map RAM budget + top symbols"
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

memory-report:
	@mkdir -p $(CAPTURE_DIR)
	cd $(GAME) && RUSTFLAGS='-Clink-arg=-Map -Clink-arg=$(MEMORY_MAP)' \
		PSOXIDE="$(PSOXIDE)" cargo build --release $(CARGO_FEATURE_ARGS)
	python3 $(ROOT)/tools/memory_report.py $(MEMORY_MAP) \
		--min-headroom-kb $(MIN_HEADROOM_KB)

disc: compile
	@mkdir -p $(DIST)
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(EXE) \
		--out $(DIST)/hl-psx.bin \
		--volume HLPSX \
		--world-pack-rooms-dir $(ROOMS)
	@echo "DISC -> $(DIST)/hl-psx.cue"

assets: check-assets menu-assets rooms models
	@echo "assets -> data/menu data/rooms data/models"

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
		--guest-frames $(PSOXIDE_PROFILE_FRAMES) \
		--pad-pulses '$(PSOXIDE_MENU_PLAY_PULSES)' \
		--profile-log $(CAPTURE_DIR)/hl-psx-profile.csv \
		--counter-log $(CAPTURE_DIR)/hl-psx-counter.csv \
		--dump-guest-profile \
		--guest-debug-log \
		--dump-hw $(CAPTURE_DIR)/hl-psx-profile-hw.ppm \
		--dump-hash
	@echo "PROFILE -> $(CAPTURE_DIR)/hl-psx-profile.csv"

psoxide-chart: psoxide-profile
	$(PSOXIDE_DEV) vblank-chart \
		--in $(CAPTURE_DIR)/hl-psx-profile.csv \
		--out $(CAPTURE_DIR)/hl-psx-profile.html \
		--title "hl-psx per-frame work"
	@echo "CHART -> $(CAPTURE_DIR)/hl-psx-profile.html"

# Cook streamed maps into WORLD.PAK chunks. Each menu room N gets two chunk IDs:
#   room_<2N>.psxc   = resident HLMA world/collision/entity data
#   room_<2N+1>.psxc = temporary HLTX texture payload for VRAM upload
# Keep MAPLIST in the same order as `game/src/menu.rs`'s chapter list.
ROOMS := $(ROOT)/data/rooms
# One representative per gameplay chapter, staying below the current static
# MAP_BUF budget. Some chapter starts are too large today, so use the first
# fitting map from that chapter instead.
MAPLIST := c0a0 c1a0 c1a1a c1a3a
rooms:
	cd $(HLBSP) && cargo build --release
	@mkdir -p $(ROOMS)
	@rm -f $(ROOMS)/room_*.psxc $(ROOMS)/room_*.psxw
	@i=0; for m in $(MAPLIST); do \
		w=$$((i * 2)); t=$$((w + 1)); \
		$(HLBSP_BIN) --cook "$(HL_GAME)/maps/$$m.bsp" $(ROOMS)/room_$$w.psxc $(ROOMS)/room_$$t.psxc >/dev/null && \
		echo "  room_$$w/$$t = $$m"; i=$$((i+1)); \
	done
	@echo "rooms -> $(ROOMS) ($(words $(MAPLIST)) maps)"

# Extract the menu font (HL fonts.wad -> data/menu/hlfont.bin, git-ignored).
# The runtime include_bytes!'s it, so run this once before building.
menu-assets:
	HL_GAME="$(HL_GAME)" python3 $(ROOT)/tools/extract_menu.py

# Install the playable disc into the PSoXide game library as its own folder with
# matching <name>.bin/.cue (the layout the library expects).
install: disc
	@mkdir -p "$(GAMES_DIR)/$(GAME_NAME)"
	@cp "$(DIST)/hl-psx.bin" "$(GAMES_DIR)/$(GAME_NAME)/$(GAME_NAME).bin"
	@printf 'FILE "%s.bin" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n' \
		"$(GAME_NAME)" > "$(GAMES_DIR)/$(GAME_NAME)/$(GAME_NAME).cue"
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

# Cook the studio models the runtime include_bytes!'s (data/models). Most are
# baked to sequence 0. Headcrab uses an HMD3 clip pack:
#   0 idle1, 4 run, 10 jump/attack, 7 dieback
MODELLIST := scientist barney v_9mmhandgun
models:
	cd $(HLBSP) && cargo build --release
	@mkdir -p $(ROOT)/data/models
	@for m in $(MODELLIST); do \
		$(HLBSP_BIN) --mdl "$(HL_GAME)/models/$$m.mdl" $(ROOT)/data/models/$$m.hlmdl 0; \
	done
	$(HLBSP_BIN) --mdl "$(HL_GAME)/models/headcrab.mdl" $(ROOT)/data/models/headcrab.hlmdl 0:8,4:8,10:7,7:7

clean:
	cd $(GAME) && cargo clean
	rm -rf $(DIST)

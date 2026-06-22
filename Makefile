# hl-psx -- a bring-your-own-assets Half-Life renderer for the PlayStation 1,
# built on the PSoXide Rust SDK (pinned as a git submodule under third_party/).
# A plain `cargo build --release` already produces a PSX-EXE (see
# game/.cargo/config.toml + game/build.rs); these targets add disc packing.

ROOT     := $(CURDIR)
GAME     := $(ROOT)/game
PSOXIDE  := $(ROOT)/third_party/PSoXide
MKISOPSX := $(PSOXIDE)/tools/mkisopsx
TARGET   := mipsel-sony-psx
DIST     := $(ROOT)/dist

# Where `make install` drops a playable disc so you can boot it from the PSoXide
# game library. Override via the environment or on the command line.
GAMES_DIR ?= $(HOME)/Downloads/ps1 games
GAME_NAME ?= Half-Life (hl-psx)
# The crate lives under game/ so its PSX-target .cargo/config can't leak onto
# the host tools (mkisopsx) built from the submodule.
EXE      := $(GAME)/target/$(TARGET)/release/hl-psx.exe
FEATURES ?=
CARGO_FEATURE_ARGS := $(if $(strip $(FEATURES)),--features $(FEATURES),)

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

.PHONY: help submodule build disc install run check-assets bsp-info cook clean

help:
	@echo "hl-psx targets:"
	@echo "  make submodule  - init/update the pinned PSoXide submodule"
	@echo "  make build      - build the PSX-EXE  -> $(EXE)"
	@echo "  make disc       - build + pack a burnable .bin/.cue into dist/"
	@echo "  make install    - build + install the disc into the PSoXide game"
	@echo "                    library ($(GAMES_DIR))"
	@echo "  make run        - alias for install (build + drop in the library)"
	@echo "  make check-assets - verify the source Half-Life install (HL_DIR)"
	@echo "  make bsp-info   - geometry + texture-VRAM budget for one map (MAP=$(MAP))"
	@echo "  make cook       - cook a map to data/maps/<MAP>.hlm (MAP=$(MAP))"
	@echo "  make clean      - remove build output"
	@echo ""
	@echo "  Source assets read from HL_DIR (default: macOS Steam path)."
	@echo "  Override: make check-assets HL_DIR=/path/to/Half-Life"

submodule:
	git submodule update --init --recursive

build:
	cd $(GAME) && cargo build --release $(CARGO_FEATURE_ARGS)
	@echo "EXE -> $(EXE)"

disc: build
	@mkdir -p $(DIST)
	cd $(MKISOPSX) && cargo run --release -- \
		--exe $(EXE) \
		--out $(DIST)/hl-psx.bin \
		--volume HLPSX
	@echo "DISC -> $(DIST)/hl-psx.cue"

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

# Cook a map to the PS1-native .hlm the runtime include_bytes!'s (data/maps/).
# M1 hard-codes c1a0; cooking a different MAP also needs the path in
# game/src/main.rs updated.
cook:
	cd $(HLBSP) && cargo build --release
	@mkdir -p $(ROOT)/data/maps
	$(HLBSP_BIN) --cook "$(HL_GAME)/maps/$(MAP).bsp" $(ROOT)/data/maps/$(MAP).hlm

clean:
	cd $(GAME) && cargo clean
	rm -rf $(DIST)

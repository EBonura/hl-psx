# Optional developer shortcuts. The supported end-user workflow is the root
# Cargo build driver documented in README.md; Make is never required.

ROOT := $(CURDIR)
HL_DIR ?=
PSOXIDE ?= $(abspath $(ROOT)/../PSoXide)
GAMES_DIR ?=
FEATURES ?=
MAP ?= c1a0
MAP_INDEX ?= 0
CAPTURE_DIR ?= $(ROOT)/captures

DRIVER := cargo run --release --
HL_ARG := $(if $(strip $(HL_DIR)),--half-life "$(HL_DIR)",)
FEATURE_ARG := $(if $(strip $(FEATURES)),--features "$(FEATURES)",)
DIST := $(ROOT)/dist
HLBSP := $(ROOT)/host/hl-bsp
HLBSP_BIN := $(HLBSP)/target/release/hl-bsp
PSOXIDE_LAUNCH := cd "$(PSOXIDE)/emu" && cargo run -p frontend --release -- launch
PSOXIDE_DEV := cargo run --manifest-path "$(PSOXIDE)/tools/psoxide-dev/Cargo.toml" --release --

PSOXIDE_SMOKE_STEPS ?= 50000000
PSOXIDE_GAMEPLAY_STEPS ?= 320000000
PSOXIDE_PROFILE_STEPS ?= 600000000
PSOXIDE_PROFILE_VISUAL_FRAMES ?= 24
PSOXIDE_PROFILE_FRAMES ?= 1200
PSOXIDE_MAP_SMOKE_STEPS ?= 240000000
PSOXIDE_MAP_SMOKE_VISUAL_FRAMES ?= 8
PSOXIDE_MENU_PLAY_PULSES ?= 0x4000@500+20

.DEFAULT_GOAL := build
.PHONY: help build assets full-disc compile disc install run check-assets \
	psoxide-check psoxide-smoke psoxide-gameplay psoxide-profile \
	psoxide-map-smoke psoxide-chart bsp-info cook clean

help:
	@echo "Cargo is the supported build interface:"
	@echo "  cargo run --release -- build"
	@echo ""
	@echo "Optional Make aliases:"
	@echo "  make build       - full extraction/cook + installed BIN/CUE"
	@echo "  make assets      - recook every asset"
	@echo "  make compile     - PS1 executable only"
	@echo "  make disc        - compile + pack + install existing assets"
	@echo "  make install     - full build + install into the PSoXide library"
	@echo "  make psoxide-smoke / psoxide-gameplay / psoxide-profile"

build full-disc:
	$(DRIVER) build $(HL_ARG) $(FEATURE_ARG)

assets:
	$(DRIVER) assets $(HL_ARG)

compile:
	$(DRIVER) compile $(FEATURE_ARG)

disc:
	$(DRIVER) pack $(FEATURE_ARG)

install:
	@test -n "$(strip $(GAMES_DIR))" || (echo "Set GAMES_DIR to the destination directory."; exit 1)
	$(DRIVER) install $(HL_ARG) $(FEATURE_ARG) \
		--games-dir "$(GAMES_DIR)"

run: install

check-assets:
	$(DRIVER) check $(HL_ARG)

psoxide-check:
	@test -f "$(PSOXIDE)/sdk/psoxide.ld" || (echo "PSoXide checkout not found: $(PSOXIDE)"; exit 1)
	@echo "PSoXide -> $(PSOXIDE)"

psoxide-smoke: disc psoxide-check
	@mkdir -p "$(CAPTURE_DIR)"
	$(PSOXIDE_LAUNCH) \
		--path "$(DIST)/hl-psx.cue" \
		--embedded-playtest \
		--steps $(PSOXIDE_SMOKE_STEPS) \
		--dump-hw "$(CAPTURE_DIR)/hl-psx-menu-hw.ppm" \
		--dump-display "$(CAPTURE_DIR)/hl-psx-menu-display.ppm" \
		--dump-hash

psoxide-gameplay: disc psoxide-check
	@mkdir -p "$(CAPTURE_DIR)"
	$(PSOXIDE_LAUNCH) \
		--path "$(DIST)/hl-psx.cue" \
		--embedded-playtest \
		--steps $(PSOXIDE_GAMEPLAY_STEPS) \
		--pad-pulses '$(PSOXIDE_MENU_PLAY_PULSES)' \
		--dump-hw "$(CAPTURE_DIR)/hl-psx-gameplay-hw.ppm" \
		--dump-display "$(CAPTURE_DIR)/hl-psx-gameplay-display.ppm" \
		--dump-hash

psoxide-profile: psoxide-check
	$(DRIVER) disc --features emulator-telemetry
	@mkdir -p "$(CAPTURE_DIR)"
	$(PSOXIDE_LAUNCH) \
		--path "$(DIST)/hl-psx.cue" \
		--embedded-playtest \
		--steps $(PSOXIDE_PROFILE_STEPS) \
		--guest-visual-frames $(PSOXIDE_PROFILE_VISUAL_FRAMES) \
		--guest-frames $(PSOXIDE_PROFILE_FRAMES) \
		--pad-pulses '$(PSOXIDE_MENU_PLAY_PULSES)' \
		--profile-log "$(CAPTURE_DIR)/hl-psx-profile.csv" \
		--counter-log "$(CAPTURE_DIR)/hl-psx-counter.csv" \
		--dump-guest-profile --guest-debug-log \
		--dump-hw "$(CAPTURE_DIR)/hl-psx-profile-hw.ppm" --dump-hash

psoxide-map-smoke: psoxide-check
	$(DRIVER) disc --features emulator-telemetry,debug-map-boot
	@mkdir -p "$(CAPTURE_DIR)"
	@mask=$$(printf "0x%04x" $$((0x0400 | $(MAP_INDEX)))); \
	out="$(CAPTURE_DIR)/hl-psx-map-$$(printf "%03d" $(MAP_INDEX))"; \
	$(PSOXIDE_LAUNCH) --path "$(DIST)/hl-psx.cue" --embedded-playtest \
		--steps $(PSOXIDE_MAP_SMOKE_STEPS) \
		--guest-visual-frames $(PSOXIDE_MAP_SMOKE_VISUAL_FRAMES) \
		--pad-pulses "$$mask@1+180" --profile-log "$$out.csv" \
		--counter-log "$$out-counter.csv" --dump-guest-profile \
		--guest-debug-log --dump-display "$$out.ppm" --dump-hash

psoxide-chart: psoxide-profile
	$(PSOXIDE_DEV) vblank-chart --in "$(CAPTURE_DIR)/hl-psx-profile.csv" \
		--out "$(CAPTURE_DIR)/hl-psx-profile.html" \
		--title "hl-psx per-frame work"

bsp-info:
	cargo build --release --manifest-path "$(HLBSP)/Cargo.toml"
	"$(HLBSP_BIN)" "$(HL_DIR)/valve/maps/$(MAP).bsp"

cook:
	cargo build --release --manifest-path "$(HLBSP)/Cargo.toml"
	@mkdir -p "$(ROOT)/data/maps"
	"$(HLBSP_BIN)" --cook "$(HL_DIR)/valve/maps/$(MAP).bsp" \
		"$(ROOT)/data/maps/$(MAP).hlm" "$(ROOT)/data/maps/$(MAP).hltx"

clean:
	cargo clean
	cargo clean --manifest-path game/Cargo.toml
	cargo clean --manifest-path host/hl-bsp/Cargo.toml
	cargo clean --manifest-path host/hl-content/Cargo.toml

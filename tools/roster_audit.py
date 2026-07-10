#!/usr/bin/env python3
"""Model-pool roster audit: simulate `stream_map_models` (game/src/main.rs) for
every MAPLIST map against the cooked chunks, and fail if any placed model type
would drop. This is the guardrail for trimming MODEL_POOL_WORDS /
POOL_FACE_CAP -- run it after `make models` or `make rooms`.

Mirrors the runtime exactly: 3-tier passes (combat > pickups > decor), the
whole-HMRG-chunk transient fit, frame-section-only residency, the whole-type
skip when POOL_FACES can't take all of a model's tris, and the slot/tex caps.
Keep the constants below in sync with game/build.rs + game/src/main.rs.
"""
import re
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROOMS, PACK = ROOT / "data/rooms", ROOT / "data/modelpack"

# --- mirrored constants (game/build.rs + game/src/main.rs) ---
MODEL_WORDS = 92_416     # build.rs MODEL_POOL_WORDS
VM_POOL_WORDS = 20_224   # main.rs VM_POOL_WORDS
MAX_LOADED = 22          # main.rs MAX_LOADED_MODELS
FACE_CAP = 7_936         # main.rs POOL_FACE_CAP
TEX_SLOTS = 240          # main.rs POOL_TEX_SLOTS
N_TYPES = 52             # main.rs N_MODEL_TYPES

# main.rs MODEL_DEFS ai classes -> streaming pass (0 combat, 1 item, 2 decor)
AI_ITEM, AI_IDLE = 0, 4
AI = {0: 1, 1: 2, 2: 3, 3: 0, 4: 0, 5: 3, 6: 5, 7: 5, 8: 5, 9: 5, 10: 5,
      11: 5, 12: 4, 13: 4, 14: 4, 15: 4, 16: 4, 17: 4, 18: 4, 19: 3, 20: 6,
      21: 6, 22: 6, 23: 4, 24: 4, 25: 4, 50: 4, 51: 5}
for t in range(26, 50):
    AI[t] = AI_ITEM
NAME = {0: "scientist", 1: "barney", 2: "headcrab", 3: "w_suit", 4: "w_battery",
        5: "zombie", 6: "houndeye", 7: "bullsquid", 8: "hgrunt", 9: "islave",
        10: "agrunt", 11: "controller", 12: "barnacle", 13: "leech", 14: "roach",
        15: "gman", 16: "garg", 17: "nihilanth", 18: "bigmom", 19: "icky",
        20: "sentry", 21: "turret", 22: "miniturret", 23: "apache", 24: "boid",
        25: "sit_sci", 50: "tentacle", 51: "hassassin"}
for t in range(26, 50):
    NAME[t] = f"pickup{t}"


def u32(d, o):
    return struct.unpack_from("<I", d, o)[0]


def u16(d, o):
    return struct.unpack_from("<H", d, o)[0]


def maplist():
    mk = (ROOT / "Makefile").read_text()
    block = re.search(r"^MAPLIST := \\\n((?:\t.*\\?\n)+)", mk, re.M).group(1)
    return re.sub(r"[\\\n\t]", " ", block).split()


def parse_chunks():
    chunks = {}
    for t in range(N_TYPES):
        p = PACK / f"chunk_{1300 + t}.psxm"
        if not p.exists():
            continue
        d = p.read_bytes()
        assert d[:4] == b"HMRG", p
        glen = u32(d, 4)
        g = d[8:8 + glen]
        n_tris, n_frames = u32(g, 8), max(u32(g, 16), 1)
        n_clips = max(u32(g, 20), 1)
        fdl = u32(g, 24)
        clips_off = 32 if g[:4] in (b"HMD5", b"HMD6") else 28
        tri_off = clips_off + n_clips * 4 + n_frames * 8 + fdl  # frame_section_len
        tex = d[8 + glen:]
        n_tex = u32(tex, 4) if tex[:4] == b"HLTX" else 0
        chunks[t] = dict(clen=len(d), kept=tri_off, tris=n_tris, ntex=n_tex)
    return chunks


def simulate(chunks, types_in_order):
    geom_word, peak = VM_POOL_WORDS, VM_POOL_WORDS
    face = tex = slots = 0
    resident, dropped = [], []
    seen = set()
    for pas in (0, 1, 2):
        for ty in types_in_order:
            if ty in seen:
                continue
            want = 2 if AI[ty] == AI_IDLE else (1 if AI[ty] == AI_ITEM else 0)
            if want != pas:
                continue
            seen.add(ty)
            c = chunks.get(ty)
            if slots >= MAX_LOADED or geom_word >= MODEL_WORDS or not c:
                dropped.append((ty, pas, "slots/pool/chunk"))
                continue
            if geom_word + (c["clen"] + 3) // 4 > MODEL_WORDS:
                dropped.append((ty, pas, "transient"))
                continue
            # the runtime streams the whole chunk BEFORE the face check, so a
            # face-skipped type still contributes its transient peak
            peak = max(peak, geom_word + (c["clen"] + 3) // 4)
            if face + c["tris"] > FACE_CAP:
                dropped.append((ty, pas, "faces"))
                continue
            geom_word = geom_word + 2 + (c["kept"] + 3) // 4
            face += c["tris"]
            tex += c["ntex"]
            slots += 1
            resident.append(ty)
    return resident, dropped, geom_word, face, tex, slots, peak


def main():
    chunks = parse_chunks()
    maps = maplist()
    fails, stats = [], []
    for mi, name in enumerate(maps):
        d = (ROOMS / f"room_{mi * 2}.psxc").read_bytes()
        po = u32(d, 36)
        prop_counts = u32(d, po)
        n_actors = prop_counts & 0xFFFF if prop_counts & 0x8000_0000 else prop_counts
        order, seen = [], set()
        for i in range(n_actors):
            ty = u16(d, po + 4 + i * 24) & 0x3FFF  # PROP_TYPE_MASK
            if ty < N_TYPES and ty not in seen:
                seen.add(ty)
                order.append(ty)
        res, drop, gw, face, tex, slots, peak = simulate(chunks, order)
        stats.append((name, gw, peak, face, tex, slots))
        bad = [(t, p) for t, p, _ in drop]
        if bad:
            fails.append(name)
            print(f"  {name}: MODEL DROP {[NAME[t] for t, _ in bad]}")
    mx = {k: max(stats, key=lambda s: s[i]) for i, k in
          enumerate(("_", "resident", "peak", "faces", "tex", "slots")) if i}
    print(f"maps audited: {len(maps)}; model-drop failures: {len(fails)} {fails}")
    print(f"peak pool   : {mx['peak'][0]} {mx['peak'][2]} of {MODEL_WORDS} words "
          f"({(MODEL_WORDS - mx['peak'][2]) * 4} B slack)")
    print(f"peak faces  : {mx['faces'][0]} {mx['faces'][3]} of {FACE_CAP}")
    print(f"peak tex    : {mx['tex'][0]} {mx['tex'][4]} of {TEX_SLOTS}")
    print(f"peak slots  : {mx['slots'][0]} {mx['slots'][5]} of {MAX_LOADED}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Model-pool roster audit: simulate `stream_map_models` (game/src/main.rs) for
every MAPLIST map against the cooked chunks, and fail if any placed model type
would drop. This is the guardrail for trimming MODEL_POOL_WORDS,
POOL_FACE_CAP, POOL_FACE_RUN_CAP, or POOL_TEX_SLOTS -- run it after `make
models` or `make rooms`.

Mirrors the runtime exactly: 3-tier passes (combat > pickups > decor), the
whole-HMRG-chunk transient fit, frame-section-only residency, and whole-type
skips when the face, ordered texture-run, or texture-slot pools cannot take a
model. Keep the constants below in sync with game/build.rs + game/src/main.rs.
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
POOL_FACE_RUN_CAP = 160   # main.rs POOL_FACE_RUN_CAP
POOL_TEX_SLOTS = 240      # main.rs POOL_TEX_SLOTS
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
        magic = g[:4]
        compact = magic in (b"HMD4", b"HMD5", b"HMD6")
        n_verts, n_tris = u32(g, 4), u32(g, 8)
        n_frames = max(u32(g, 16), 1)
        n_clips = max(u32(g, 20), 1) if magic == b"HMD3" or compact else 1
        fdl = u32(g, 24) if compact else 0
        if magic in (b"HMD5", b"HMD6"):
            clips_off = 32
        elif magic == b"HMD4":
            clips_off = 28
        elif magic == b"HMD3":
            clips_off = 24
        else:
            clips_off = 0
        if compact:
            tri_off = clips_off + n_clips * 4 + n_frames * 8 + fdl
        elif magic == b"HMD3":
            tri_off = clips_off + n_clips * 4 + n_frames * n_verts * 6
        else:
            tri_off = 20 + n_frames * n_verts * 6
        tri_sz = 20 if magic == b"HMD6" else 16
        assert tri_off + n_tris * tri_sz <= len(g), p
        runs, last_tex = 0, None
        for i in range(n_tris):
            tri_tex = u16(g, tri_off + i * tri_sz + 6)
            if tri_tex != last_tex:
                runs += 1
                last_tex = tri_tex
        tex = d[8 + glen:]
        n_tex = u32(tex, 4) if tex[:4] == b"HLTX" else 0
        chunks[t] = dict(clen=len(d), kept=tri_off, tris=n_tris,
                         runs=runs, ntex=n_tex)
    return chunks


def simulate(chunks, types_in_order):
    geom_word, peak = VM_POOL_WORDS, VM_POOL_WORDS
    face = runs = tex = slots = 0
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
            # The runtime streams the whole chunk BEFORE the pool checks, so a
            # capacity-skipped type still contributes its transient peak.
            peak = max(peak, geom_word + (c["clen"] + 3) // 4)
            if face + c["tris"] > FACE_CAP:
                dropped.append((ty, pas, "faces"))
                continue
            if runs + c["runs"] > POOL_FACE_RUN_CAP:
                dropped.append((ty, pas, "face runs"))
                continue
            if tex + c["ntex"] > POOL_TEX_SLOTS:
                dropped.append((ty, pas, "texture slots"))
                continue
            geom_word = geom_word + 2 + (c["kept"] + 3) // 4
            face += c["tris"]
            runs += c["runs"]
            tex += c["ntex"]
            slots += 1
            resident.append(ty)
    return resident, dropped, geom_word, face, runs, tex, slots, peak


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
        res, drop, gw, face, runs, tex, slots, peak = simulate(chunks, order)
        stats.append((name, gw, peak, face, runs, tex, slots))
        bad = [(t, reason) for t, _, reason in drop]
        if bad:
            fails.append(name)
            print(f"  {name}: MODEL DROP "
                  f"{[f'{NAME[t]} ({reason})' for t, reason in bad]}")
    mx = {k: max(stats, key=lambda s: s[i]) for i, k in
          enumerate(("_", "resident", "peak", "faces", "runs", "tex", "slots")) if i}
    print(f"maps audited: {len(maps)}; model-drop failures: {len(fails)} {fails}")
    print(f"peak pool   : {mx['peak'][0]} {mx['peak'][2]} of {MODEL_WORDS} words "
          f"({(MODEL_WORDS - mx['peak'][2]) * 4} B slack)")
    print(f"peak faces  : {mx['faces'][0]} {mx['faces'][3]} of {FACE_CAP}")
    print(f"peak runs   : {mx['runs'][0]} {mx['runs'][4]} of {POOL_FACE_RUN_CAP}")
    print(f"peak tex    : {mx['tex'][0]} {mx['tex'][5]} of {POOL_TEX_SLOTS}")
    print(f"peak slots  : {mx['slots'][0]} {mx['slots'][6]} of {MAX_LOADED}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())

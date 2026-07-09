#!/usr/bin/env python3
"""Cook the HL SFX set into a single WORLD.PAK chunk (data/sfx/chunk_3000.psxa).

Reads WAVs from the player's install (HL_DIR), normalises to 16-bit mono
22050 Hz, cooks each to SPU-ADPCM .psau via `psxed audio-pack`, then packs:

  "HSFX" | u32 count | count x (u32 offset, u32 len) | psau blobs

The ID ORDER below must match game/src/sfx.rs.
"""
import hashlib
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import wave
import zipfile

# (id, sound-relative path) -- ORDER IS THE RUNTIME SFX ID SPACE (sfx.rs).
SOUNDS = [
    ("glock", "weapons/pl_gun3.wav"),
    ("mp5", "weapons/hks1.wav"),
    ("shotgun", "weapons/sbarrel1.wav"),
    ("python", "weapons/357_shot1.wav"),
    ("xbow", "weapons/xbow_fire1.wav"),
    ("gauss", "weapons/gauss2.wav"),
    ("rpg", "weapons/rocketfire1.wav"),
    ("cbar_miss", "weapons/cbar_miss1.wav"),
    ("cbar_hit", "weapons/cbar_hit1.wav"),
    ("explode", "weapons/explode3.wav"),
    ("ric", "weapons/ric1.wav"),
    ("electro", "weapons/electro4.wav"),
    ("pain", "player/pl_pain6.wav"),
    ("bodydrop", "common/bodydrop3.wav"),
    ("door_move", "doors/doormove1.wav"),
    ("door_stop", "doors/doorstop1.wav"),
    ("button", "buttons/button3.wav"),
    ("pickup", "items/gunpickup2.wav"),
    ("suit", "items/suitchargeok1.wav"),
    ("hc_attack", "headcrab/hc_attack1.wav"),
    ("zo_attack", "zombie/zo_attack1.wav"),
    ("he_blast", "houndeye/he_blast1.wav"),
    ("glass_break", "debris/bustglass1.wav"),
    ("wood_break", "debris/bustcrate1.wav"),
    ("medshot", "items/medshot4.wav"),
    ("step1", "player/pl_step1.wav"),
    ("step2", "player/pl_step2.wav"),
    ("reload", "weapons/reload1.wav"),
    ("dry", "common/wpn_denyselect.wav"),
    ("zo_pain", "zombie/zo_pain2.wav"),
    ("hc_pain", "headcrab/hc_pain1.wav"),
    ("hc_die", "headcrab/hc_die1.wav"),
    ("gr_pain", "hgrunt/gr_pain3.wav"),
    ("gr_die", "hgrunt/gr_die1.wav"),
    ("ba_pain", "barney/ba_pain1.wav"),
    ("ba_die", "barney/ba_die1.wav"),
    ("he_pain", "houndeye/he_pain3.wav"),
    ("he_die", "houndeye/he_die1.wav"),
    ("slv_pain", "aslave/slv_pain2.wav"),
    ("slv_die", "aslave/slv_die1.wav"),
    ("bc_pain", "bullchicken/bc_pain1.wav"),
    ("bc_die", "bullchicken/bc_die1.wav"),
    ("hev_bell", "fvox/bell.wav"),
    ("geiger", "player/geiger1.wav"),
    # HEV suit voice: complete fvox phrases (resident -- they fire anywhere on
    # health thresholds / suit pickup, not per map).
    ("hev_activate", "fvox/powerarmor_on.wav"),
    ("hev_health_crit", "fvox/health_critical.wav"),
    ("hev_near_death", "fvox/near_death.wav"),
    # Dialogue/voice lines are NOT here -- they stream per-map (extract_voices.py
    # -> chunk 3100+idx) so each map loads only its own lines at a reduced rate.
]
def to_pcm16_mono(src, dst):
    """Normalise to 16-bit mono at the source's NATIVE rate (the runtime
    honours per-sample rates; upsampling 11k voices to 22k doubled SPU cost)."""
    with wave.open(src, "rb") as w:
        nch, sw, rate, nfr = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
        raw = w.readframes(nfr)
    if sw == 1:  # 8-bit unsigned -> 16-bit signed
        pcm = bytearray()
        for b in raw:
            pcm += struct.pack("<h", (b - 128) << 8)
        raw, sw = bytes(pcm), 2
    if nch == 2:  # average to mono
        mono = bytearray()
        for i in range(0, len(raw), 4):
            l = struct.unpack_from("<h", raw, i)[0]
            r = struct.unpack_from("<h", raw, i + 2)[0]
            mono += struct.pack("<h", (l + r) // 2)
        raw = bytes(mono)
    with wave.open(dst, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(raw)
    return rate


def main():
    hl_sound = sys.argv[1]
    psxed_manifest_dir = tempfile.mkdtemp(prefix="hlsfx-")
    out_pack = sys.argv[2]
    psxed = sys.argv[3]

    # Normalise each source at its native rate, then cook one audio-pack run
    # per distinct rate (the manifest's target rate is global).
    rates = {}
    for sid, rel in SOUNDS:
        src = os.path.join(hl_sound, rel)
        tmp = os.path.join(psxed_manifest_dir, sid + ".wav")
        rates[sid] = to_pcm16_mono(src, tmp)

    for rate in sorted(set(rates.values())):
        group = [sid for sid, _ in SOUNDS if rates[sid] == rate]
        zpath = os.path.join(psxed_manifest_dir, f"sfx{rate}.zip")
        with zipfile.ZipFile(zpath, "w") as z:
            for sid in group:
                z.write(os.path.join(psxed_manifest_dir, sid + ".wav"), sid + ".wav")
        sha = hashlib.sha256(open(zpath, "rb").read()).hexdigest()
        manifest = {
            "source": {"name": "player Half-Life install", "url": "", "license": "user-supplied",
                       "archive_sha256": sha},
            "target_sample_rate_hz": rate,
            "normalize_peak": 0.9,
            "sounds": [{"id": sid, "path": sid + ".wav"} for sid in group],
        }
        mpath = os.path.join(psxed_manifest_dir, f"sel{rate}.json")
        with open(mpath, "w") as f:
            json.dump(manifest, f)
        subprocess.run(psxed.split() + ["audio-pack", mpath, "--zip", zpath,
                                        "--out-dir", os.path.join(psxed_manifest_dir, "out")],
                       check=True, capture_output=True, text=True)

    blobs = []
    for sid, _ in SOUNDS:
        with open(os.path.join(psxed_manifest_dir, "out", "psau", sid + ".psau"), "rb") as f:
            blobs.append(f.read())
    table_sz = 8 + 8 * len(blobs)
    out = bytearray(b"HSFX")
    out += struct.pack("<I", len(blobs))
    off = table_sz
    for b in blobs:
        out += struct.pack("<II", off, len(b))
        off += len(b)
    for b in blobs:
        out += b
    os.makedirs(os.path.dirname(out_pack), exist_ok=True)
    with open(out_pack, "wb") as f:
        f.write(out)
    print(f"sfx pack: {len(blobs)} samples, {len(out)} bytes -> {out_pack}")
    shutil.rmtree(psxed_manifest_dir)


if __name__ == "__main__":
    main()

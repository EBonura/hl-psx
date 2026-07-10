#!/usr/bin/env python3
"""Per-map dialogue: cook each map's voice lines to 8 kHz SPU-ADPCM.

For every campaign map, gather the voice audio it actually uses
(scripted_sentence sentences + speech ambient_generic messages), resolve to
WAV(s), concatenate + downsample to 8 kHz (voice stays intelligible; SPU RAM is
tight and dialogue is only needed on its own map), and cook one HSFX pack per
map -> data/voices/chunk_<3100+idx>.psxa. Also emits data/voices/manifest.txt
(map_idx|local_id|key) so the map cook (hl-bsp VOICES_MANIFEST) can resolve each
scripted_sentence / ambient_generic to a per-map local voice id.

Local-only, git-ignored, no asset bytes committed (same rule as extract_sfx.py).
Usage: extract_voices.py <valve_dir> <maps "c0a0 c0a0a ..."> <out_dir> <psxed>
"""
import hashlib
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
import wave
import zipfile

# Per-map dialogue streams into the SPU region above the resident core SFX
# (512 KB total, samples from 0x1010; see game/src/sfx.rs). Cook at the highest
# rate that fits; drop only when a map's dialogue overflows -- voice stays
# intelligible down to ~5 kHz. The budget is derived from the CURRENT core pack
# so it tracks core-SFX growth (a stale constant here silently drops runtime
# tail lines on every map whose pack exceeds the real region).
VOICE_RATES = (11025, 8000, 6000, 5000, 4000)


def voice_budget():
    core = os.path.join(os.path.dirname(__file__), "..", "data", "sfx", "chunk_3000.psxa")
    try:
        core_bytes = os.path.getsize(core)
    except OSError:
        core_bytes = 414 * 1024  # last known core size; recook sfx first for exactness
    return 512 * 1024 - 0x1010 - core_bytes - 4096  # 4K slack for ADPCM 8-align


VOICE_BUDGET = voice_budget()
VOICE_DIRS = ("barney/", "scientist/", "gman/", "hgrunt/", "tride/", "vox/", "fvox/")


def parse_ents(bsp_path):
    d = open(bsp_path, "rb").read()
    eoff, elen = struct.unpack_from("<ii", d, 4)
    txt = d[eoff:eoff + elen].decode("latin1")
    out = []
    for block in re.findall(r"\{(.*?)\}", txt, re.S):
        e = dict(re.findall(r'"([^"]+)"\s+"([^"]*)"', block))
        out.append(e)
    return out


def load_sentences(valve):
    """name(upper) -> list of relative wav paths (sentences.txt expansion)."""
    out = {}
    path = os.path.join(valve, "sound", "sentences.txt")
    if not os.path.exists(path):
        return out
    for raw in open(path, "r", encoding="latin1"):
        line = raw.strip()
        if not line or line.startswith("//"):
            continue
        parts = line.split()
        if len(parts) < 2:
            continue
        name = parts[0].upper()
        wavs, cur_dir = [], ""
        for tok in parts[1:]:
            tok = re.sub(r"\(.*?\)", "", tok)  # strip (pitch/vol) params
            tok = tok.strip().lstrip("(").rstrip(")")
            if not tok or tok in (".", ","):
                continue
            if "/" in tok:
                cur_dir = tok.rsplit("/", 1)[0] + "/"
                wavs.append(tok + ".wav")
            else:
                wavs.append(cur_dir + tok + ".wav")
        if wavs:
            out[name] = wavs
    return out


def map_voice_keys(ents, sentences):
    """Ordered de-duped list of (key, [wav paths]) this map needs."""
    keys, seen = [], set()

    def add(key, wavs):
        if key in seen or not wavs:
            return
        seen.add(key)
        keys.append((key, wavs))

    for e in ents:
        c = e.get("classname", "")
        if c == "scripted_sentence":
            s = e.get("sentence", "").lstrip("!").upper()
            if s and s in sentences:
                add(s, sentences[s])
        elif c == "ambient_generic":
            msg = e.get("message", "")
            if msg.lower().endswith(".wav") and any(msg.startswith(d) for d in VOICE_DIRS):
                add(msg.lower(), [msg])
    return keys


def concat_wavs(valve, wavs, dst, target_rate):
    """Concatenate wavs -> mono 16-bit at `target_rate`. Returns True on success."""
    pcm = bytearray()
    for rel in wavs:
        p = os.path.join(valve, "sound", rel)
        if not os.path.exists(p):
            return False
        with wave.open(p, "rb") as w:
            nch, sw, rate, nfr = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
            raw = w.readframes(nfr)
        if sw == 1:
            raw = b"".join(struct.pack("<h", (b - 128) << 8) for b in raw)
        if nch == 2:
            mono = bytearray()
            for i in range(0, len(raw) - 3, 4):
                left = struct.unpack_from("<h", raw, i)[0]
                right = struct.unpack_from("<h", raw, i + 2)[0]
                mono += struct.pack("<h", (left + right) // 2)
            raw = bytes(mono)
        # nearest-neighbour resample (voice; quality is secondary to SPU RAM)
        n_in = len(raw) // 2
        n_out = max(1, n_in * target_rate // max(rate, 1))
        res = bytearray(n_out * 2)
        for j in range(n_out):
            si = j * rate // target_rate
            if si >= n_in:
                si = n_in - 1
            res[j * 2:j * 2 + 2] = raw[si * 2:si * 2 + 2]
        pcm += res
    with wave.open(dst, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(target_rate)
        w.writeframes(bytes(pcm))
    return True


def cook_pack(psxed, tmp, wav_ids, out_pack, target_rate):
    """audio-pack the per-map wavs into an HSFX pack; returns byte size."""
    zpath = os.path.join(tmp, "voices.zip")
    with zipfile.ZipFile(zpath, "w") as z:
        for sid in wav_ids:
            z.write(os.path.join(tmp, sid + ".wav"), sid + ".wav")
    manifest = {
        "source": {"name": "player Half-Life install", "url": "", "license": "user-supplied",
                   "archive_sha256": hashlib.sha256(open(zpath, "rb").read()).hexdigest()},
        "target_sample_rate_hz": target_rate,
        "normalize_peak": 0.9,
        "sounds": [{"id": sid, "path": sid + ".wav"} for sid in wav_ids],
    }
    mpath = os.path.join(tmp, "sel.json")
    json.dump(manifest, open(mpath, "w"))
    subprocess.run(psxed.split() + ["audio-pack", mpath, "--zip", zpath,
                                    "--out-dir", os.path.join(tmp, "out")],
                   check=True, capture_output=True, text=True)
    blobs = [open(os.path.join(tmp, "out", "psau", sid + ".psau"), "rb").read() for sid in wav_ids]
    out = bytearray(b"HSFX") + struct.pack("<I", len(blobs))
    off = 8 + 8 * len(blobs)
    for b in blobs:
        out += struct.pack("<II", off, len(b))
        off += len(b)
    for b in blobs:
        out += b
    open(out_pack, "wb").write(out)
    return len(out)


def main():
    valve, maplist, out_dir, psxed = sys.argv[1], sys.argv[2].split(), sys.argv[3], sys.argv[4]
    os.makedirs(out_dir, exist_ok=True)
    for f in os.listdir(out_dir):
        if f.startswith("chunk_") or f == "manifest.txt":
            os.remove(os.path.join(out_dir, f))
    sentences = load_sentences(valve)
    manifest_lines = []
    for idx, m in enumerate(maplist):
        bsp = os.path.join(valve, "maps", m + ".bsp")
        if not os.path.exists(bsp):
            continue
        keys = map_voice_keys(parse_ents(bsp), sentences)
        if not keys:
            continue
        chunk = 3100 + idx
        out_pack = os.path.join(out_dir, f"chunk_{chunk}.psxa")
        # Try rates high->low; keep the highest that fits the per-map budget.
        for rate in VOICE_RATES:
            with tempfile.TemporaryDirectory(prefix="hlvox-") as tmp:
                wav_ids, used = [], []
                for key, wavs in keys:
                    # Only successfully resolved sources occupy the pack, so
                    # assign ids after resolution. Preserving an earlier failed
                    # source's index creates a manifest gap while HSFX blobs
                    # remain contiguous, shifting every later runtime lookup.
                    local_id = len(wav_ids)
                    sid = f"v{local_id:02d}"
                    if concat_wavs(valve, wavs, os.path.join(tmp, sid + ".wav"), rate):
                        wav_ids.append(sid)
                        used.append((local_id, key))
                if not wav_ids:
                    break
                size = cook_pack(psxed, tmp, wav_ids, out_pack, rate)
                if size <= VOICE_BUDGET or rate == VOICE_RATES[-1]:
                    for local_id, key in used:
                        manifest_lines.append(f"{idx}|{local_id}|{key}")
                    flag = "" if size <= VOICE_BUDGET else "  OVER-BUDGET"
                    print(f"  {m} (idx {idx}, chunk {chunk}): {len(wav_ids)} lines, "
                          f"{size // 1024} KB @{rate}Hz{flag}")
                    break
    open(os.path.join(out_dir, "manifest.txt"), "w").write("\n".join(manifest_lines) + "\n")
    print(f"voices -> {out_dir} ({len(manifest_lines)} lines across maps)")


if __name__ == "__main__":
    main()

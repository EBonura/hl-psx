#!/usr/bin/env python3
"""Extract the player's own Half-Life music into CDDA track payloads.

valve/media/Half-LifeNN.mp3 -> data/music/track_NN.cdda (44.1 kHz s16le stereo
raw PCM, padded to a whole number of 2352-byte sectors) + data/music/tracks.txt
(one path per line, mkisopsx --cdda-track-list order).

Track numbering: the data track is 1; appended audio tracks are 2..N. Retail
Half-Life maps its CD track T to media/Half-Life{T-1:02d}.mp3, so appending
Half-Life01.. in order makes disc track T carry HL track T's music -- the
worldspawn `sounds` key and trigger_cdaudio values pass through unchanged.

Decodes with macOS afconvert (no extra deps). Output is git-ignored: the music
is the user's own asset, never committed.
"""
import os
import struct
import subprocess
import sys
import tempfile
import wave

SECTOR = 2352


def decode(mp3, raw_out):
    with tempfile.TemporaryDirectory() as td:
        wav = os.path.join(td, "t.wav")
        subprocess.run(
            ["afconvert", "-f", "WAVE", "-d", "LEI16@44100", "-c", "2", mp3, wav],
            check=True,
            capture_output=True,
        )
        with wave.open(wav, "rb") as w:
            assert w.getnchannels() == 2 and w.getsampwidth() == 2
            pcm = w.readframes(w.getnframes())
    pad = (-len(pcm)) % SECTOR
    pcm += b"\x00" * pad
    with open(raw_out, "wb") as f:
        f.write(pcm)
    return len(pcm) // SECTOR


def main():
    hl_game = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser(
        "~/Library/Application Support/Steam/steamapps/common/Half-Life/valve"
    )
    out_dir = sys.argv[2] if len(sys.argv) > 2 else "data/music"
    media = os.path.join(hl_game, "media")
    if not os.path.isdir(media):
        print(f"no media dir at {media}; skipping music", file=sys.stderr)
        return
    os.makedirs(out_dir, exist_ok=True)
    tracks = sorted(
        f for f in os.listdir(media) if f.lower().startswith("half-life") and f.lower().endswith(".mp3")
    )
    listing = []
    total = 0
    for i, name in enumerate(tracks):
        out = os.path.join(out_dir, f"track_{i + 1:02d}.cdda")
        sectors = decode(os.path.join(media, name), out)
        total += sectors
        listing.append(os.path.abspath(out))
        print(f"  track {i + 2:2d} (disc) = {name} ({sectors} sectors)")
    with open(os.path.join(out_dir, "tracks.txt"), "w") as f:
        f.write("\n".join(listing) + "\n")
    print(f"music -> {out_dir} ({len(tracks)} tracks, {total * SECTOR // (1 << 20)} MB)")


if __name__ == "__main__":
    main()

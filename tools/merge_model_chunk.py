#!/usr/bin/env python3
"""Merge a model's geom + tex chunks into one HMRG chunk (halves the CD
per-chunk handshake count at map load): "HMRG" | u32 geom_len | geom | tex.
Overwrites the geom file and removes the tex file."""
import struct, sys, os

geom_path, tex_path = sys.argv[1], sys.argv[2]
geom = open(geom_path, "rb").read()
tex = open(tex_path, "rb").read() if os.path.exists(tex_path) else b""
out = b"HMRG" + struct.pack("<I", len(geom)) + geom + tex
open(geom_path, "wb").write(out)
if os.path.exists(tex_path):
    os.remove(tex_path)

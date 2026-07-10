#!/usr/bin/env python3
"""Per-map sprite packs: decode the .spr billboards each map uses, crush to 4bpp.

For every campaign map, gather the sprites it references (env_sprite / env_glow /
env_spark / env_explosion / cycler_sprite `model`, and env_beam / env_laser
`texture`), decode each IDSP v2 sprite, crush every frame to a 4bpp PS1 texture
(median-cut to 16 colours; additive sprites keep index 0 = black so dark areas
add nothing under Add blend), and cook one pack per map ->
data/sprites/chunk_<3200+idx>.psxa. Also emits data/sprites/manifest.txt
(map_idx|local_id|spr_basename|blend|n_frames|base_w|base_h) so the map cook
(hl-bsp SPRITES_MANIFEST) can resolve each sprite entity to a per-map local id.

Pack format ("HSPR"):
  magic "HSPR" | u16 n_sprites, u16 n_frames_total
  per sprite (12B): u8 n_frames, u8 blend(0 normal/1 add), u16 first_frame,
                    u16 base_w, u16 base_h (native px, for world scale),
                    u16 crush_w, u16 crush_h (the 4bpp texture size, for UVs)
  then n_frames_total texture blobs: u16 w,h | u16 clut[16] | u8 pix4 (== the
  .hlm/.hlmdl blob upload_tex_blob reads).

Local-only, git-ignored, no asset bytes committed (same rule as extract_sfx.py).
Usage: extract_sprites.py <valve_dir> <maps "c0a0 c0a0a ..."> <out_dir>
"""
import glob
import os
import re
import struct
import sys

SPR_MAX = 64          # cap each frame to <=64px, power-of-2 (VRAM atlas requires it)
MAX_FRAMES = 6        # explosion sheets: sample up to 6 frames
MAX_SPR_PER_MAP = 12  # per-map unique sprite cap
# Full-campaign census: c2a5e is the maximum at 49 sampled frames; no map
# references more than 10 valid sprites. The previous 14-frame cap stopped at
# the first overflowing sprite and silently omitted tail FX on 38/96 maps.
MAX_FRAMES_PER_MAP = 49
CHUNK_BASE = 3200     # WORLD.PAK chunk id = CHUNK_BASE + map_index


def to_bgr555(r, g, b):
    return ((b >> 3) << 10) | ((g >> 3) << 5) | (r >> 3)


def median_cut(colors, n):
    """<=n representative colours (same box-split as hl-bsp's median_cut)."""
    if not colors:
        return [(0, 0, 0)]
    boxes = [colors]
    while len(boxes) < n:
        # split the box with the largest single-channel extent
        best_i, best_ext = -1, -1
        for i, box in enumerate(boxes):
            if len(box) < 2:
                continue
            mn = [min(c[k] for c in box) for k in range(3)]
            mx = [max(c[k] for c in box) for k in range(3)]
            ext = max(mx[k] - mn[k] for k in range(3))
            if ext > best_ext:
                best_ext, best_i, best_ch = ext, i, max(range(3), key=lambda k: mx[k] - mn[k])
        if best_i < 0:
            break
        box = sorted(boxes[best_i], key=lambda c: c[best_ch])
        mid = len(box) // 2
        boxes[best_i:best_i + 1] = [box[:mid], box[mid:]]
    out = []
    for box in boxes:
        n_c = len(box)
        out.append(tuple(sum(c[k] for c in box) // n_c for k in range(3)))
    return out


def nearest(pal, c):
    bd, bi = 1 << 30, 0
    for i, p in enumerate(pal):
        d = (c[0] - p[0]) ** 2 + (c[1] - p[1]) ** 2 + (c[2] - p[2]) ** 2
        if d < bd:
            bd, bi = d, i
    return bi


def find_spr(valve, name):
    """Case-insensitive lookup of a .spr under sprites/ (HL refs vary in case)."""
    base = os.path.basename(name).lower()
    if not base.endswith(".spr"):
        base += ".spr"
    for f in glob.glob(os.path.join(valve, "sprites", "*.spr")):
        if os.path.basename(f).lower() == base:
            return f
    # some beams reference sprites/ prefix already stripped or subdir
    for f in glob.glob(os.path.join(valve, "sprites", "**", "*.spr"), recursive=True):
        if os.path.basename(f).lower() == base:
            return f
    return None


def decode_spr(path):
    """-> (blend, base_w, base_h, [ (w,h,indices,palette) per frame ]) or None."""
    d = open(path, "rb").read()
    if d[:4] != b"IDSP":
        return None
    ver, typ, texfmt = struct.unpack_from("<iii", d, 4)
    mw, mh, nf = struct.unpack_from("<iii", d, 20)
    ncol = struct.unpack_from("<H", d, 40)[0]
    paloff = 42
    pal = [tuple(d[paloff + i * 3:paloff + i * 3 + 3]) for i in range(ncol)]
    off = paloff + ncol * 3
    blend = 1 if texfmt == 1 else 0  # SPR_ADDITIVE=1
    frames = []
    step = max(1, nf // MAX_FRAMES) if nf > MAX_FRAMES else 1
    fi = 0
    while fi < nf and len(frames) < MAX_FRAMES:
        grp = struct.unpack_from("<i", d, off)[0]
        off += 4
        if grp != 0:  # SPR_GROUP: skip the interval table (rare in HL)
            cnt = struct.unpack_from("<i", d, off)[0]
            off += 4 + cnt * 4  # count + intervals; then cnt frames follow
        ox, oy, w, h = struct.unpack_from("<iiii", d, off)
        off += 16
        px = d[off:off + w * h]
        off += w * h
        if fi % step == 0:
            frames.append((w, h, px, pal))
        fi += 1
    return (blend, mw, mh, frames)


def pow2_le(v, cap):
    """Largest power of two <= min(v, cap) (the VRAM atlas needs pow2 dims)."""
    v = min(v, cap)
    p = 2
    while p * 2 <= v:
        p *= 2
    return p


def crush_frame(w, h, px, pal, blend, cap=SPR_MAX):
    """Downscale + 4bpp crush -> (fw, fh, clut16[u16], pix4 bytes). Pow2 dims."""
    fw = pow2_le(w, cap)
    fh = pow2_le(h, cap)
    idx = bytearray(fw * fh)
    for y in range(fh):
        for x in range(fw):
            idx[y * fw + x] = px[(y * h // fh) * w + (x * w // fw)]
    colors = [pal[i] for i in idx]
    pal16 = median_cut(colors, 16)
    if blend == 1:
        # Additive: force the darkest cluster to black (adds nothing), and set
        # the PS1 semi-transparency bit (0x8000) on every entry so the GPU blends
        # the texel under Add mode instead of drawing it opaque.
        di = min(range(len(pal16)), key=lambda i: sum(pal16[i]))
        pal16[di] = (0, 0, 0)
        clut = [0x8000 | to_bgr555(*c) for c in pal16] + [0] * (16 - len(pal16))
    else:
        clut = [to_bgr555(*c) for c in pal16] + [0] * (16 - len(pal16))
    pix = bytearray(fw * fh // 2)
    for i in range(0, fw * fh, 2):
        lo = nearest(pal16, colors[i])
        hi = nearest(pal16, colors[i + 1]) if i + 1 < len(colors) else 0
        pix[i // 2] = lo | (hi << 4)
    return fw, fh, clut, bytes(pix)


SPRITE_CLASSES = ("env_sprite", "env_glow", "env_spark", "env_explosion",
                  "cycler_sprite", "env_beam", "env_laser")

# Resident explosion sprite: weapon blasts happen on ANY map, so s_explod.spr is
# packed once (its own chunk) and loaded every map, not tied to placed entities.
EXPL_CHUNK = 3002    # WORLD.PAK extra chunk (>=3000 -> LZ4, like SFX/HUD)
EXPL_SPR = "zerogxplode.spr"  # HL's weapon-explosion fireball sheet
EXPL_FRAMES = 5      # keep it small: 5 frames of a 32px fireball is cheap VRAM
EXPL_CAP = 32        # a background flash reads fine at 32px upscaled


def build_explosion(valve, out_dir):
    """One resident single-sprite HSPR pack (s_explod.spr) -> chunk_3002."""
    p = find_spr(valve, EXPL_SPR) or find_spr(valve, "s_explod.spr")
    dec = decode_spr(p) if p else None
    if not dec:
        print("WARN: explosion sprite not found -> no animated fireball")
        return
    _, bw, bh, frames = dec
    frames = frames[:EXPL_FRAMES]
    # Force additive (blend=1): draw_billboard always Add-blends, so the CLUT
    # needs the semi-transparency bit + a black darkest cluster (crush_frame).
    crushed = [crush_frame(w, h, px, pal, 1, EXPL_CAP) for (w, h, px, pal) in frames]
    if not crushed:
        return
    frame_blobs = [struct.pack("<HH", fw, fh) + struct.pack("<16H", *clut) + pix
                   for (fw, fh, clut, pix) in crushed]
    cw, ch = crushed[0][0], crushed[0][1]
    hdr = b"HSPR" + struct.pack("<HH", 1, len(crushed))
    hdr += struct.pack("<BBHHHHH", len(crushed), 1, 0, bw, bh, cw, ch)
    open(os.path.join(out_dir, f"chunk_{EXPL_CHUNK}.psxa"), "wb").write(hdr + b"".join(frame_blobs))
    print(f"resident explosion -> chunk_{EXPL_CHUNK} ({os.path.basename(p)}, {len(crushed)} frames)")


def map_sprites(bsp):
    """Unique sprite basenames referenced by a map's sprite/beam entities."""
    d = open(bsp, "rb").read()
    eo, el = struct.unpack_from("<ii", d, 4)
    t = d[eo:eo + el].decode("latin1")
    names = []
    for b in re.findall(r"\{(.*?)\}", t, re.S):
        e = dict(re.findall(r'"([^"]+)"\s+"([^"]*)"', b))
        if e.get("classname") not in SPRITE_CLASSES:
            continue
        m = e.get("model") or e.get("texture") or ""
        if m.lower().endswith(".spr"):
            base = os.path.basename(m).lower()
            if base not in names:
                names.append(base)
    return names


def main():
    valve, maps_arg, out_dir = sys.argv[1], sys.argv[2], sys.argv[3]
    maps = maps_arg.split()
    os.makedirs(out_dir, exist_ok=True)
    manifest = []
    decoded_cache = {}
    for idx, mname in enumerate(maps):
        bsp = os.path.join(valve, "maps", mname + ".bsp")
        if not os.path.exists(bsp):
            continue
        names = map_sprites(bsp)[:MAX_SPR_PER_MAP]
        sprites, frame_blobs, first = [], [], 0
        for local_id, base in enumerate(names):
            if base not in decoded_cache:
                p = find_spr(valve, base)
                decoded_cache[base] = decode_spr(p) if p else None
            dec = decoded_cache[base]
            if not dec:
                continue
            blend, bw, bh, frames = dec
            crushed = [crush_frame(*f, blend) for f in frames]
            if not crushed:
                continue
            # Bound the atlas cost: sprites append after models per map, so cap
            # the total frame count (drop tail sprites -- graceful, like voices).
            if first + len(crushed) > MAX_FRAMES_PER_MAP:
                break
            for (fw, fh, clut, pix) in crushed:
                frame_blobs.append(struct.pack("<HH", fw, fh)
                                   + struct.pack("<16H", *clut) + pix)
            cw, ch = crushed[0][0], crushed[0][1]
            sprites.append((len(crushed), blend, first, bw, bh, cw, ch))
            manifest.append(f"{idx}|{len(sprites)-1}|{base}|{blend}|{len(crushed)}|{bw}|{bh}")
            first += len(crushed)
        if not sprites:
            continue
        hdr = b"HSPR" + struct.pack("<HH", len(sprites), len(frame_blobs))
        for (nfr, blend, ff, bw, bh, cw, ch) in sprites:
            hdr += struct.pack("<BBHHHHH", nfr, blend, ff, bw, bh, cw, ch)
        blob = hdr + b"".join(frame_blobs)
        open(os.path.join(out_dir, f"chunk_{CHUNK_BASE + idx}.psxa"), "wb").write(blob)
    open(os.path.join(out_dir, "manifest.txt"), "w").write("\n".join(manifest) + "\n")
    print(f"sprites -> {out_dir} ({len(set(m.split('|')[0] for m in manifest))} maps, "
          f"{len(manifest)} sprite entries)")
    build_explosion(valve, out_dir)


if __name__ == "__main__":
    main()

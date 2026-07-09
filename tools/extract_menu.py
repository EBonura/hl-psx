#!/usr/bin/env python3
"""Build the menu font into git-ignored data/menu/hlfont.bin.

Half-Life's WON main menu is defined by valve/640_textscheme.txt ("Primary
Button Text": FontName "Arial", FontSize 16, FgColor 255 170 0, armed white).
So the menu font IS Arial -- we rasterize the system Arial.ttf to the psx-font
BitmapFont blob the runtime builds at boot, rather than the fonts.wad HUD font
(which the engine uses for the HUD, not the menu).

Blob layout (little-endian):
  u8 glyph_w | u8 glyph_h | u16 glyph_count | u16 first_char | u16 pad
  u8 advances[glyph_count]
  u8 bitmap[glyph_count * glyph_h * ceil(glyph_w/8)]   # 1bpp, MSB=leftmost

Writes only to the git-ignored data/ tree; nothing here is committed.
"""
import os, struct

OUT = os.path.join(os.path.dirname(__file__), "..", "data", "menu")
HL = os.environ.get(
    "HL_GAME",
    os.path.expanduser("~/Library/Application Support/Steam/steamapps/common/Half-Life/valve"),
)
ARIAL = os.environ.get("MENU_FONT", "/System/Library/Fonts/Supplemental/Arial.ttf")
SIZE = 16   # matches the scheme's FontSize
GLYPH_W_CAP = 16
THRESH = 100  # anti-aliased coverage -> on/off
LOGO_W = 224  # < 256 so the 4bpp texture fits one tpage and u stays <= 255


def convert():
    from PIL import Image, ImageDraw, ImageFont

    f = ImageFont.truetype(ARIAL, SIZE)
    asc, desc = f.getmetrics()
    glyph_h = asc + desc
    first, last = 32, 126
    count = last - first + 1

    advances, cells, glyph_w = bytearray(), [], 0
    for c in range(first, last + 1):
        ch = chr(c)
        cell = Image.new("L", (SIZE + 8, glyph_h), 0)
        ImageDraw.Draw(cell).text((0, 0), ch, fill=255, font=f)
        bbox = cell.getbbox()
        glyph_w = max(glyph_w, bbox[2] if bbox else 0)
        advances.append(min(max(round(f.getlength(ch)), 1), 255))
        cells.append(cell)
    glyph_w = min(glyph_w, GLYPH_W_CAP)
    row_bytes = (glyph_w + 7) // 8

    bitmap = bytearray()
    for cell in cells:
        px = cell.load()
        for r in range(glyph_h):
            row = [0] * row_bytes
            for col in range(glyph_w):
                if px[col, r] >= THRESH:
                    row[col >> 3] |= 0x80 >> (col & 7)
            bitmap += bytes(row)

    os.makedirs(OUT, exist_ok=True)
    blob = struct.pack("<BBHHH", glyph_w, glyph_h, count, first, 0) + advances + bitmap
    open(os.path.join(OUT, "hlfont.bin"), "wb").write(blob)
    print(f"hlfont.bin: Arial {glyph_w}x{glyph_h}, {count} glyphs, {len(blob)} bytes -> {OUT}")
    preview(first, count, glyph_w, glyph_h, row_bytes, advances, bitmap)


def preview(first, count, gw, gh, rb, advances, bitmap):
    try:
        from PIL import Image
    except ImportError:
        return
    s = "New game   HALF-LIFE   Anomalous Materials"
    im = Image.new("RGB", (700, 40), (16, 16, 19))
    x = 6
    for ch in s:
        cp = ord(ch)
        if first <= cp < first + count:
            gi = cp - first
            for r in range(gh):
                for col in range(gw):
                    if bitmap[gi * gh * rb + r * rb + (col >> 3)] & (0x80 >> (col & 7)):
                        im.putpixel((x + col, 8 + r), (255, 170, 0))
            x += advances[gi]
        else:
            x += 6
    im.resize((700, 80), Image.NEAREST).save("/tmp/hlfont_preview.png")
    print("preview -> /tmp/hlfont_preview.png")


def extract_bg():
    """gfx/conback.lmp -> data/menu/bg.tex: the install's console-background image
    (the only full-screen art a Steam copy ships -- splash.bmp is WON-only),
    desaturated to grey and darkened so the orange/white menu text reads over it.
    256x240 4bpp, drawn stretched to the 320x240 screen. Blob: u16 w,h |
    u16 clut[16] | u8 pix4."""
    from PIL import Image

    d = open(os.path.join(HL, "gfx", "conback.lmp"), "rb").read()
    w, h = struct.unpack_from("<II", d, 0)
    pal_off = 8 + w * h
    pal = [(d[pal_off + i * 3], d[pal_off + i * 3 + 1], d[pal_off + i * 3 + 2]) for i in range(256)]
    im = Image.new("RGB", (w, h))
    im.putdata([pal[b] for b in d[8:8 + w * h]])
    g = im.convert("L").resize((256, 240), Image.LANCZOS)

    DARKEST = 10  # 5-bit ceiling -> ~32% brightness so menu text stays legible
    clut = [((round(i / 15 * DARKEST),) * 3) for i in range(16)]
    clut = [(v[0] << 10) | (v[1] << 5) | v[2] for v in clut]
    px = g.load()
    pix = bytearray()
    for y in range(240):
        for x in range(0, 256, 2):
            lo = px[x, y] >> 4
            hi = px[x + 1, y] >> 4
            pix.append((hi << 4) | lo)

    blob = struct.pack("<HH", 256, 240) + struct.pack("<16H", *clut) + bytes(pix)
    open(os.path.join(OUT, "bg.tex"), "wb").write(blob)
    print(f"bg.tex: 256x240 4bpp (conback), {len(blob)} bytes -> {OUT}")


def extract_logo():
    """resource/logo.tga -> data/menu/logo.tex: the real HALF-LIFE wordmark as a
    4bpp texture with a transparent (0x0000) background. Blob: u16 w,h |
    u16 clut[16] | u8 pix4. The wordmark is white, so a 16-step grey ramp CLUT
    with entry 0 = transparent is plenty."""
    from PIL import Image

    src = Image.open(os.path.join(HL, "resource", "logo.tga")).convert("RGBA")
    bg = Image.new("RGBA", src.size, (0, 0, 0, 255))
    bg.alpha_composite(src)
    h = max(1, round(LOGO_W * src.height / src.width))
    g = bg.convert("L").resize((LOGO_W, h), Image.LANCZOS)

    # 16-step grey ramp; index 0 (darkest) -> transparent 0x0000.
    clut = []
    for i in range(16):
        if i == 0:
            clut.append(0)
        else:
            v = round(i / 15 * 31)
            clut.append((v << 10) | (v << 5) | v)
    px = g.load()
    pix = bytearray()
    for y in range(h):
        for x in range(0, LOGO_W, 2):
            lo = px[x, y] >> 4
            hi = px[x + 1, y] >> 4 if x + 1 < LOGO_W else 0
            pix.append((hi << 4) | lo)  # two 4bpp texels per byte, low nibble first

    blob = struct.pack("<HH", LOGO_W, h) + struct.pack("<16H", *clut) + bytes(pix)
    open(os.path.join(OUT, "logo.tex"), "wb").write(blob)
    print(f"logo.tex: {LOGO_W}x{h} 4bpp, {len(blob)} bytes -> {OUT}")


def extract_hud():
    """sprites/640hud7.spr -> data/menu/hud.tex: a PS1-sized HUD number font +
    core status icons, packed into one 160x48 4bpp texture (digits 0-9 at
    u=d*15 row 0; icons at row 18). This is generated from the real Half-Life
    HUD sprites at 75% source size so the runtime can draw it 1:1.
    White-on-transparent grey ramp (index 0 = 0x0000 = transparent);
    the runtime tints it HEV amber. Source rects are hud.txt's `640 640hud7` ones:
    number_d at (d*24, 0, 20, 24), suit_full/empty at (0/40,24,40,40),
    cross at (80,24,32,32), divider at (240,0,2,40), pistol ammo at
    weapon_9mmhandgun.txt's (0,72,24,24), and item_battery from 640hud2."""
    from PIL import Image

    resample = Image.Resampling.LANCZOS if hasattr(Image, "Resampling") else Image.LANCZOS
    alpha_cut = 32
    d = open(os.path.join(HL, "sprites", "640hud7.spr"), "rb").read()
    off = 40
    cnt = struct.unpack_from("<h", d, off)[0]
    off += 2
    pal = [(d[off + i * 3], d[off + i * 3 + 1], d[off + i * 3 + 2]) for i in range(cnt)]
    off += cnt * 3 + 4  # palette + frame group
    _ox, _oy, fw, fh = struct.unpack_from("<iiii", d, off)
    off += 16
    sheet = d[off:off + fw * fh]
    lum = lambda i: sum(pal[i]) // 3

    src = Image.frombytes("L", (fw, fh), bytes(lum(i) for i in sheet))

    DIGIT_SRC_W, DIGIT_SRC_H = 20, 24
    DIGIT_W, DIGIT_H = 15, 18
    SUIT_W, SUIT_H = 30, 30
    CROSS_W, CROSS_H = 24, 24
    AMMO_W, AMMO_H = 18, 18
    CROSSHAIR_W, CROSSHAIR_H = 18, 18
    DIVIDER_W, DIVIDER_H = 2, 30
    BATTERY_W, BATTERY_H = 24, 24
    WEAPON_W, WEAPON_H = 80, 20
    TW, TH = 160, 188  # weapon select icons stack below the status row
    ICON_V = DIGIT_H
    SUIT_FULL_U = 0
    SUIT_EMPTY_U = 30
    HEALTH_U = 60
    AMMO_U = 84
    CROSSHAIR_U = 108
    DIVIDER_U = 132
    BATTERY_U = 136
    tex = [0] * (TW * TH)

    def blit_img(im, dx, dy):
        px = im.load()
        w, h = im.size
        for yy in range(h):
            for xx in range(w):
                a = px[xx, yy]
                tex[(dy + yy) * TW + (dx + xx)] = 0 if a < alpha_cut else max(1, min(a >> 4, 15))

    def scaled_rect(sx, sy, sw, sh, dw, dh):
        return src.crop((sx, sy, sx + sw, sy + sh)).resize((dw, dh), resample)

    for dgt in range(10):
        blit_img(scaled_rect(dgt * 24, 0, DIGIT_SRC_W, DIGIT_SRC_H, DIGIT_W, DIGIT_H), dgt * DIGIT_W, 0)
    blit_img(scaled_rect(0, 24, 40, 40, SUIT_W, SUIT_H), SUIT_FULL_U, ICON_V)
    blit_img(scaled_rect(40, 24, 40, 40, SUIT_W, SUIT_H), SUIT_EMPTY_U, ICON_V)
    blit_img(scaled_rect(80, 24, 32, 32, CROSS_W, CROSS_H), HEALTH_U, ICON_V)
    blit_img(scaled_rect(0, 72, 24, 24, AMMO_W, AMMO_H), AMMO_U, ICON_V)
    blit_img(scaled_rect(240, 0, 2, 40, DIVIDER_W, DIVIDER_H), DIVIDER_U, ICON_V)

    # HEV battery pickup icon from sprites/640hud2.spr, hud.txt's 640 rect
    # item_battery = (176,0,44,44).  Keep it compact for the pickup pulse.
    bd = open(os.path.join(HL, "sprites", "640hud2.spr"), "rb").read()
    bo = 40
    bc = struct.unpack_from("<h", bd, bo)[0]
    bo += 2
    bpal = [(bd[bo + i * 3], bd[bo + i * 3 + 1], bd[bo + i * 3 + 2]) for i in range(bc)]
    bo += bc * 3 + 4
    _bx, _by, bfw, bfh = struct.unpack_from("<iiii", bd, bo)
    bo += 16
    bsheet = bd[bo:bo + bfw * bfh]
    blum = lambda i: sum(bpal[i]) // 3
    bsrc = Image.frombytes("L", (bfw, bfh), bytes(blum(i) for i in bsheet))
    battery = bsrc.crop((176, 0, 220, 44)).resize((BATTERY_W, BATTERY_H), resample)
    blit_img(battery, BATTERY_U, ICON_V)

    # Pistol crosshair from sprites/crosshairs.spr (alphatest: idx 255 = grey
    # transparent, idx 0 = amber tick). 640 rect is (24,0,24,24).
    cd = open(os.path.join(HL, "sprites", "crosshairs.spr"), "rb").read()
    o = 40
    cc = struct.unpack_from("<h", cd, o)[0]
    o += 2 + cc * 3 + 4
    _x, _y, cfw, _cfh = struct.unpack_from("<iiii", cd, o)
    o += 16
    cs = cd[o:o + cfw * _cfh]
    mask = Image.new("L", (cfw, _cfh), 0)
    mp = mask.load()
    for yy in range(_cfh):
        for xx in range(cfw):
            mp[xx, yy] = 0 if cs[yy * cfw + xx] == 255 else 255
    crosshair = mask.crop((24, 0, 48, 24)).resize((CROSSHAIR_W, CROSSHAIR_H), resample)
    hp = crosshair.load()
    for yy in range(CROSSHAIR_H):
        for xx in range(CROSSHAIR_W):
            v = hp[xx, yy]
            tex[(ICON_V + yy) * TW + (CROSSHAIR_U + xx)] = 0 if v < alpha_cut else max(1, min(v >> 4, 15))

    # Weapon-select icons: each weapon's sprites/<name>.txt lists the 320-res
    # selected-state rect ("weapon_s 320 <sheet> x y 80 20"). Order MUST match
    # the runtime W_* constants. Two icons per atlas row, from v=48.
    WEAPON_TXT = [
        "weapon_crowbar", "weapon_9mmhandgun", "weapon_357", "weapon_9mmar",
        "weapon_shotgun", "weapon_crossbow", "weapon_rpg", "weapon_gauss",
        "weapon_egon", "weapon_hornetgun", "weapon_handgrenade", "weapon_snark",
        "weapon_tripmine", "weapon_satchel",
    ]
    sheet_cache = {}

    def sheet_lum(name):
        if name not in sheet_cache:
            sd = open(os.path.join(HL, "sprites", name + ".spr"), "rb").read()
            so = 40
            sc = struct.unpack_from("<h", sd, so)[0]
            so += 2
            spal = [(sd[so + i * 3], sd[so + i * 3 + 1], sd[so + i * 3 + 2]) for i in range(sc)]
            so += sc * 3 + 4
            _sx, _sy, sfw, sfh = struct.unpack_from("<iiii", sd, so)
            so += 16
            ss = sd[so:so + sfw * sfh]
            slum = lambda i: sum(spal[i]) // 3
            sheet_cache[name] = Image.frombytes("L", (sfw, sfh), bytes(slum(i) for i in ss))
        return sheet_cache[name]

    for wi, wname in enumerate(WEAPON_TXT):
        txt = open(os.path.join(HL, "sprites", wname + ".txt")).read()
        rect = None
        for line in txt.splitlines():
            parts = line.split()
            if len(parts) >= 7 and parts[0] == "weapon_s" and parts[1] == "320":
                rect = (parts[2], int(parts[3]), int(parts[4]), int(parts[5]), int(parts[6]))
                break
        if rect is None:
            print(f"  !! no 320 weapon_s rect for {wname}")
            continue
        sheet_name, sx, sy, sw, sh = rect
        icon = sheet_lum(sheet_name).crop((sx, sy, sx + sw, sy + sh))
        if (sw, sh) != (WEAPON_W, WEAPON_H):
            icon = icon.resize((WEAPON_W, WEAPON_H), resample)
        blit_img(icon, (wi % 2) * WEAPON_W, 48 + (wi // 2) * WEAPON_H)

    # HEV amber baked into the ramp (index 0 = transparent), so a neutral draw
    # gives amber digits. (255,170,0) -> 5551.
    amber = (255, 170, 0)
    p555 = lambda r, g, b: ((b >> 3) << 10) | ((g >> 3) << 5) | (r >> 3)
    clut = [0] + [p555(amber[0] * i // 15, amber[1] * i // 15, amber[2] * i // 15) for i in range(1, 16)]
    pix = bytearray()
    for y in range(TH):
        for x in range(0, TW, 2):
            pix.append((tex[y * TW + x + 1] << 4) | tex[y * TW + x])
    blob = struct.pack("<HH", TW, TH) + struct.pack("<16H", *clut) + bytes(pix)
    open(os.path.join(OUT, "hud.tex"), "wb").write(blob)
    print(f"hud.tex: {TW}x{TH} 4bpp (75% 640hud7 HUD atlas), {len(blob)} bytes -> {OUT}")


def pack_menu_chunk():
    """Pack bg.tex + logo.tex into one streamed WORLD.PAK chunk (menu.pak) so they
    don't sit in always-resident .rodata; the runtime streams them when the menu
    opens (like the HUD atlas). Format: u32 bg_len | u32 logo_len | bg | logo."""
    bg = open(os.path.join(OUT, "bg.tex"), "rb").read()
    logo = open(os.path.join(OUT, "logo.tex"), "rb").read()
    blob = struct.pack("<II", len(bg), len(logo)) + bg + logo
    open(os.path.join(OUT, "menu.pak"), "wb").write(blob)
    print(f"menu.pak: bg {len(bg)} + logo {len(logo)} = {len(blob)} bytes -> {OUT}")


if __name__ == "__main__":
    convert()
    for step in (extract_logo, extract_bg, extract_hud, pack_menu_chunk):
        try:
            step()
        except Exception as e:
            print(f"{step.__name__} skipped:", e)

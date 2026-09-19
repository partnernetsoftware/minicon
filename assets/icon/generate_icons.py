#!/usr/bin/env python3
"""Deterministically render the MiniCon application icon.

Writes `assets/minicon.iconset/` (the ten PNG faces macOS asks for) and, when
`iconutil` is available, compiles `assets/minicon.icns` from it. Also writes a
512x512 preview PNG for eyeballing.

Only the Python standard library is used: the rasterizer below is a small
signed-distance-field renderer and the PNG writer is `zlib` + `struct`. There
is no PIL, cairosvg or ImageMagick dependency, and no randomness or timestamp,
so a rerun reproduces byte-identical PNGs.

Usage (from the repository root):

    python3 assets/icon/generate_icons.py
    # optional, macOS only, done automatically when iconutil is present:
    iconutil -c icns assets/minicon.iconset -o assets/minicon.icns

The artwork deliberately mirrors the existing Windows icon `assets/minicon.ico`
so the product has one identity on both platforms: a deep navy rounded square
holding two overlapping tabbed panels -- cyan above, amber below -- each with a
`>` prompt and a cursor bar. At 16x16 and 32x32 that detail turns to mush, so
those faces get a simplified variant: one large cyan chevron and one amber
cursor bar. macOS picks the right face per size, which is exactly what an
`.icns` with per-size art is for.
"""

from __future__ import annotations

import math
import pathlib
import shutil
import struct
import subprocess
import sys
import zlib

# --- palette -----------------------------------------------------------------
# Sampled to match assets/minicon.ico. These five values are the whole brand
# surface of the icon; changing them changes the product's identity.
NAVY_TOP = (0x16, 0x24, 0x3A)
NAVY_BOTTOM = (0x0A, 0x13, 0x22)
PANEL_FILL = (0x10, 0x1C, 0x2E)
CYAN = (0x3A, 0xD1, 0xF0)
AMBER = (0xF2, 0xB0, 0x1E)

# --- geometry, in unit coordinates of the full canvas (y grows downward) ------
# Apple's macOS icon grid: an 824x824 rounded square with a 185px corner radius
# on a 1024px canvas, leaving the surrounding breathing room the Dock expects.
BG_HALF = 824.0 / 2.0 / 1024.0
BG_RADIUS = 185.0 / 1024.0

STROKE = 0.028  # panel outline thickness
PANEL_RADIUS = 0.045
# The tab shares the panel's corner radius so the left edge, where the two
# rounded rectangles are unioned, stays one straight unbroken line.
TAB_RADIUS = PANEL_RADIUS

# body (x0, y0, x1, y1) and tab (x0, y0, x1, y1) of each panel
TOP_BODY = (0.175, 0.245, 0.795, 0.560)
TOP_TAB = (0.175, 0.195, 0.395, 0.290)
BOTTOM_BODY = (0.205, 0.530, 0.825, 0.845)
BOTTOM_TAB = (0.205, 0.480, 0.425, 0.575)

# `>` prompt and cursor bar inside each panel
CHEVRON_RADIUS = 0.019
TOP_CHEVRON = (0.245, 0.335, 0.395, 0.055)  # x0, x1, y_center, half_height
TOP_CURSOR = (0.365, 0.475, 0.442, 0.015)  # x0, x1, y_center, half_height
BOTTOM_CHEVRON = (0.275, 0.365, 0.680, 0.055)
BOTTOM_CURSOR = (0.395, 0.505, 0.727, 0.015)

# the simplified small-size face
SMALL_CHEVRON = (0.285, 0.470, 0.455, 0.125)
SMALL_CHEVRON_RADIUS = 0.055
SMALL_CURSOR = (0.520, 0.760, 0.605, 0.050)
SMALL_CURSOR_RADIUS = 0.038

MASTER_DETAILED = 1024
MASTER_SMALL = 512

ICONSET_FACES = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]
# Faces at or below this pixel size use the simplified artwork.
SMALL_FACE_LIMIT = 32


# --- signed distance fields ---------------------------------------------------
def sd_round_rect(x: float, y: float, box: tuple, radius: float) -> float:
    """Distance to a rounded rectangle given as (x0, y0, x1, y1)."""
    x0, y0, x1, y1 = box
    cx, cy = (x0 + x1) / 2.0, (y0 + y1) / 2.0
    hw, hh = (x1 - x0) / 2.0 - radius, (y1 - y0) / 2.0 - radius
    qx, qy = abs(x - cx) - hw, abs(y - cy) - hh
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - radius


def sd_capsule(x: float, y: float, ax: float, ay: float, bx: float, by: float, radius: float) -> float:
    """Distance to a line segment thickened into a round-capped capsule."""
    pax, pay = x - ax, y - ay
    bax, bay = bx - ax, by - ay
    denom = bax * bax + bay * bay
    h = 0.0 if denom == 0.0 else min(max((pax * bax + pay * bay) / denom, 0.0), 1.0)
    return math.hypot(pax - bax * h, pay - bay * h) - radius


class Canvas:
    """Premultiplied float RGBA canvas painted by source-over compositing."""

    def __init__(self, size: int) -> None:
        self.size = size
        self.px = [0.0] * (size * size * 4)

    def paint(self, bbox: tuple, distance, color, alpha: float = 1.0) -> None:
        """Paint `color` where `distance(x, y) <= 0`, antialiased over one pixel.

        `color` is an RGB tuple or a callable of y returning one. `bbox` bounds
        the work in unit coordinates so untouched regions cost nothing.
        """
        size = self.size
        inv = 1.0 / size
        # one pixel of feather, in unit coordinates, plus a pixel of slack
        feather = inv
        x0 = max(0, int((bbox[0] - feather) * size) - 1)
        y0 = max(0, int((bbox[1] - feather) * size) - 1)
        x1 = min(size, int(math.ceil((bbox[2] + feather) * size)) + 1)
        y1 = min(size, int(math.ceil((bbox[3] + feather) * size)) + 1)
        constant_color = not callable(color)
        cr = cg = cb = 0.0
        if constant_color:
            cr, cg, cb = color[0] / 255.0, color[1] / 255.0, color[2] / 255.0
        px = self.px
        for j in range(y0, y1):
            y = (j + 0.5) * inv
            if not constant_color:
                row = color(y)
                cr, cg, cb = row[0] / 255.0, row[1] / 255.0, row[2] / 255.0
            base = j * size * 4
            for i in range(x0, x1):
                d = distance((i + 0.5) * inv, y)
                if d >= feather:
                    continue
                coverage = 0.5 - d / feather
                if coverage <= 0.0:
                    continue
                src_a = alpha * (1.0 if coverage >= 1.0 else coverage)
                o = base + i * 4
                inv_a = 1.0 - src_a
                px[o] = cr * src_a + px[o] * inv_a
                px[o + 1] = cg * src_a + px[o + 1] * inv_a
                px[o + 2] = cb * src_a + px[o + 2] * inv_a
                px[o + 3] = src_a + px[o + 3] * inv_a

    def downsample(self, target: int) -> bytes:
        """Box-filter to `target` pixels and return straight (unpremultiplied) RGBA."""
        size = self.size
        if size % target != 0:
            raise ValueError(f"master {size} is not an integer multiple of {target}")
        factor = size // target
        inv_n = 1.0 / (factor * factor)
        px = self.px
        out = bytearray(target * target * 4)
        for j in range(target):
            for i in range(target):
                r = g = b = a = 0.0
                for sj in range(j * factor, (j + 1) * factor):
                    base = sj * size * 4
                    for si in range(i * factor, (i + 1) * factor):
                        o = base + si * 4
                        r += px[o]
                        g += px[o + 1]
                        b += px[o + 2]
                        a += px[o + 3]
                r *= inv_n
                g *= inv_n
                b *= inv_n
                a *= inv_n
                o = (j * target + i) * 4
                if a <= 0.0:
                    continue
                out[o] = _byte(r / a)
                out[o + 1] = _byte(g / a)
                out[o + 2] = _byte(b / a)
                out[o + 3] = _byte(a)
        return bytes(out)


def _byte(value: float) -> int:
    return min(255, max(0, int(value * 255.0 + 0.5)))


# --- artwork ------------------------------------------------------------------
def _background(canvas: Canvas) -> None:
    box = (0.5 - BG_HALF, 0.5 - BG_HALF, 0.5 + BG_HALF, 0.5 + BG_HALF)

    def gradient(y: float):
        t = min(max((y - box[1]) / (box[3] - box[1]), 0.0), 1.0)
        return tuple(
            NAVY_TOP[c] + (NAVY_BOTTOM[c] - NAVY_TOP[c]) * t for c in range(3)
        )

    canvas.paint(box, lambda x, y: sd_round_rect(x, y, box, BG_RADIUS), gradient)


def _tab_with_skirt(body: tuple, tab: tuple) -> tuple:
    """The tab rectangle extended down into the body.

    The extension is invisible -- it lies entirely inside the body, which is
    drawn as part of the same union -- but it makes the tab and the body
    overlap by more than `STROKE`, so their inset copies still overlap and the
    outline computed from them has no gap where the two meet.
    """
    return (tab[0], tab[1], tab[2], max(tab[3], body[1] + 4.0 * STROKE))


def _panel_distance(body: tuple, tab: tuple):
    tab = _tab_with_skirt(body, tab)

    def distance(x: float, y: float) -> float:
        return min(
            sd_round_rect(x, y, body, PANEL_RADIUS),
            sd_round_rect(x, y, tab, TAB_RADIUS),
        )

    return distance


def _inset(box: tuple, amount: float) -> tuple:
    x0, y0, x1, y1 = box
    return (x0 + amount, y0 + amount, x1 - amount, y1 - amount)


def _panel_outline_distance(body: tuple, tab: tuple):
    """The panel outline, as the shape minus the shape inset by `STROKE`.

    `abs(union) - STROKE/2` would be the obvious spelling, but `min()` of two
    signed distance fields is only exact *outside* the union: where the tab
    meets the body their edges are tangent, `min()` understates the interior
    distance, and the outline visibly pinches at the join. Subtracting the
    inset shape keeps every evaluation on the exact side of both fields.
    """
    outer = _panel_distance(body, tab)
    inner_body = _inset(body, STROKE)
    inner_tab = _inset(_tab_with_skirt(body, tab), STROKE)
    inner_radius = max(PANEL_RADIUS - STROKE, 0.002)
    inner_tab_radius = max(TAB_RADIUS - STROKE, 0.002)

    def distance(x: float, y: float) -> float:
        inner = min(
            sd_round_rect(x, y, inner_body, inner_radius),
            sd_round_rect(x, y, inner_tab, inner_tab_radius),
        )
        return max(outer(x, y), -inner)

    return distance


def _chevron_distance(spec: tuple, radius: float):
    x0, x1, yc, hh = spec
    def distance(x: float, y: float) -> float:
        return min(
            sd_capsule(x, y, x0, yc - hh, x1, yc, radius),
            sd_capsule(x, y, x0, yc + hh, x1, yc, radius),
        )

    return distance


def _bbox(spec: tuple, radius: float) -> tuple:
    x0, x1, yc, hh = spec
    return (x0 - radius, yc - hh - radius, x1 + radius, yc + hh + radius)


def _draw_panel(canvas: Canvas, body: tuple, tab: tuple, accent, chevron: tuple, cursor: tuple) -> None:
    distance = _panel_distance(body, tab)
    box = (tab[0] - 0.01, tab[1] - 0.01, body[2] + 0.01, body[3] + 0.01)
    # Fill first so the lower panel occludes the upper one where they overlap.
    canvas.paint(box, distance, PANEL_FILL)
    canvas.paint(box, _panel_outline_distance(body, tab), accent)
    canvas.paint(
        _bbox(chevron, CHEVRON_RADIUS),
        _chevron_distance(chevron, CHEVRON_RADIUS),
        accent,
    )
    cx0, cx1, cyc, chh = cursor
    cursor_box = (cx0, cyc - chh, cx1, cyc + chh)
    canvas.paint(
        cursor_box,
        lambda x, y: sd_round_rect(x, y, cursor_box, chh * 0.9),
        accent,
    )


def render_detailed() -> Canvas:
    canvas = Canvas(MASTER_DETAILED)
    _background(canvas)
    _draw_panel(canvas, TOP_BODY, TOP_TAB, CYAN, TOP_CHEVRON, TOP_CURSOR)
    _draw_panel(canvas, BOTTOM_BODY, BOTTOM_TAB, AMBER, BOTTOM_CHEVRON, BOTTOM_CURSOR)
    return canvas


def render_small() -> Canvas:
    canvas = Canvas(MASTER_SMALL)
    _background(canvas)
    canvas.paint(
        _bbox(SMALL_CHEVRON, SMALL_CHEVRON_RADIUS),
        _chevron_distance(SMALL_CHEVRON, SMALL_CHEVRON_RADIUS),
        CYAN,
    )
    x0, x1, yc, hh = SMALL_CURSOR
    cursor_box = (x0, yc - hh, x1, yc + hh)
    canvas.paint(
        cursor_box,
        lambda x, y: sd_round_rect(x, y, cursor_box, SMALL_CURSOR_RADIUS),
        AMBER,
    )
    return canvas


# --- PNG ----------------------------------------------------------------------
def write_png(path: pathlib.Path, size: int, rgba: bytes) -> None:
    raw = bytearray()
    stride = size * 4
    for j in range(size):
        raw.append(0)  # filter type 0 (None), so the output never varies
        raw += rgba[j * stride : (j + 1) * stride]

    def chunk(kind: bytes, payload: bytes) -> bytes:
        body = kind + payload
        return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


# --- driver -------------------------------------------------------------------
def main() -> int:
    assets = pathlib.Path(__file__).resolve().parent.parent
    iconset = assets / "minicon.iconset"
    if iconset.exists():
        shutil.rmtree(iconset)
    iconset.mkdir(parents=True)

    detailed = render_detailed()
    small = render_small()

    preview_written = False
    for name, pixels in ICONSET_FACES:
        master = small if pixels <= SMALL_FACE_LIMIT else detailed
        rgba = master.downsample(pixels)
        write_png(iconset / name, pixels, rgba)
        print(f"  {name} ({pixels}px)")
        if pixels == 512 and not preview_written:
            write_png(pathlib.Path("/tmp/minicon-icon-preview.png"), 512, rgba)
            preview_written = True

    icns = assets / "minicon.icns"
    iconutil = shutil.which("iconutil")
    if iconutil:
        subprocess.run(
            [iconutil, "-c", "icns", str(iconset), "-o", str(icns)], check=True
        )
        print(f"wrote {icns} ({icns.stat().st_size} bytes)")
    else:
        print("iconutil not found; run it manually to produce assets/minicon.icns")
    print("preview: /tmp/minicon-icon-preview.png")
    return 0


if __name__ == "__main__":
    sys.exit(main())

"""Draw GatiMeter's icon and write res/gatimeter.ico (PNG-in-ICO, no Pillow needed).

Design: a rounded teal square with a white up arrow and an amber down arrow.
Run:  python tools/make_icon.py
"""
import math
import os
import struct
import sys
import zlib

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
SS = 4  # supersampling per axis

TEAL_TOP = (20, 184, 166)
TEAL_BOT = (15, 118, 110)
WHITE = (255, 255, 255)
AMBER = (251, 191, 36)


def rounded_rect_inside(x, y, x0, y0, x1, y1, r):
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def in_poly(x, y, pts):
    inside = False
    n = len(pts)
    j = n - 1
    for i in range(n):
        xi, yi = pts[i]
        xj, yj = pts[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def arrow(cx, top, bottom, half_w, head_h, shaft_w, up=True):
    """Arrow polygon in unit coordinates (0..1)."""
    if up:
        return [
            (cx, top), (cx + half_w, top + head_h), (cx + shaft_w, top + head_h),
            (cx + shaft_w, bottom), (cx - shaft_w, bottom), (cx - shaft_w, top + head_h),
            (cx - half_w, top + head_h),
        ]
    return [
        (cx, bottom), (cx - half_w, bottom - head_h), (cx - shaft_w, bottom - head_h),
        (cx - shaft_w, top), (cx + shaft_w, top), (cx + shaft_w, bottom - head_h),
        (cx + half_w, bottom - head_h),
    ]


def render(size):
    small = size <= 24
    margin = 0.03 if small else 0.06
    radius = 0.22
    # Arrows are thicker on small sizes so they stay readable.
    hw, sw = (0.17, 0.065) if small else (0.15, 0.05)
    up = arrow(0.34, 0.20, 0.80, hw, 0.24, sw, up=True)
    down = arrow(0.66, 0.20, 0.80, hw, 0.24, sw, up=False)
    rows = []
    n = size * SS
    for py in range(size):
        row = bytearray()
        for px in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SS):
                for sx in range(SS):
                    x = (px * SS + sx + 0.5) / n
                    y = (py * SS + sy + 0.5) / n
                    if not rounded_rect_inside(x, y, margin, margin, 1 - margin, 1 - margin, radius):
                        continue
                    t = (y - margin) / (1 - 2 * margin)
                    col = tuple(TEAL_TOP[i] * (1 - t) + TEAL_BOT[i] * t for i in range(3))
                    if in_poly(x, y, up):
                        col = WHITE
                    elif in_poly(x, y, down):
                        col = AMBER
                    acc[0] += col[0]
                    acc[1] += col[1]
                    acc[2] += col[2]
                    acc[3] += 1
            cover = acc[3] / (SS * SS)
            if acc[3]:
                r, g, b = (int(round(acc[i] / acc[3])) for i in range(3))
            else:
                r = g = b = 0
            row += bytes((r, g, b, int(round(cover * 255))))
        rows.append(bytes(row))
    return rows


def png(size, rows):
    raw = b"".join(b"\x00" + r for r in rows)

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out = os.path.join(root, "res", "gatimeter.ico")
    images = [(s, png(s, render(s))) for s in SIZES]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = b""
    data = b""
    for s, blob in images:
        dim = 0 if s >= 256 else s
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(blob), offset + len(data))
        data += blob
    with open(out, "wb") as f:
        f.write(header + entries + data)
    # Also keep a 256 px PNG for the README.
    with open(os.path.join(root, "res", "gatimeter-256.png"), "wb") as f:
        f.write(images[-1][1])
    print("wrote", out, file=sys.stderr)


if __name__ == "__main__":
    main()

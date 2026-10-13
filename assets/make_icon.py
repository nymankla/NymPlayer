"""Draws the record-player icon. Writes icon.png (256px, window icon) and icon.ico (exe icon).
Standard library only: python assets/make_icon.py"""
import math, struct, zlib
from pathlib import Path

SS = 4  # supersampling per axis, for antialiasing
WOOD, WOOD_HI = (160, 104, 58), (186, 128, 78)
VINYL, GROOVE = (20, 20, 22), (52, 52, 58)
LABEL, SPINDLE, ARM, ARM_DARK = (224, 71, 47), (230, 230, 230), (214, 216, 220), (110, 112, 118)


def seg_dist(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def color_at(x, y):
    """Colour at (x, y) in 256-unit icon space, or None for transparent."""
    # Plinth: rounded square with a lighter top half.
    m, r = 8, 40
    qx, qy = max(m + r - x, 0, x - (256 - m - r)), max(m + r - y, 0, y - (256 - m - r))
    if math.hypot(qx, qy) > r:
        return None
    c = WOOD_HI if y < 128 else WOOD
    d = math.hypot(x - 116, y - 134)
    if d <= 102:
        c = VINYL
        if 42 < d < 96 and d % 10 < 1.6:
            c = GROOVE
        if d <= 36:
            c = LABEL
        if d <= 6:
            c = SPINDLE
    # Tonearm: pivot top-right, arm down to a headshell resting on the record.
    if seg_dist(x, y, 212, 46, 196, 166) <= 5 or seg_dist(x, y, 196, 166, 170, 196) <= 9:
        c = ARM
    pd = math.hypot(x - 212, y - 46)
    if pd <= 18:
        c = ARM_DARK if pd <= 8 else ARM
    return c


def render(size):
    rows = []
    for py in range(size):
        row = bytearray([0])  # PNG filter type: none
        for px in range(size):
            r = g = b = a = 0
            for sy in range(SS):
                for sx in range(SS):
                    c = color_at((px + (sx + 0.5) / SS) * 256 / size, (py + (sy + 0.5) / SS) * 256 / size)
                    if c:
                        r, g, b, a = r + c[0], g + c[1], b + c[2], a + 1
            row += bytes([r // a, g // a, b // a, a * 255 // (SS * SS)] if a else [0, 0, 0, 0])
        rows.append(bytes(row))
    return png(size, b"".join(rows))


def png(size, raw):
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def ico(images):
    out, offset = struct.pack("<HHH", 0, 1, len(images)), 6 + 16 * len(images)
    for size, data in images:
        out += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b"".join(d for _, d in images)


if __name__ == "__main__":
    here = Path(__file__).parent
    images = [(s, render(s)) for s in (16, 24, 32, 48, 64, 256)]
    (here / "icon.png").write_bytes(images[-1][1])
    (here / "icon.ico").write_bytes(ico(images))

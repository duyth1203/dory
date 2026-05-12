#!/usr/bin/env python3
"""Find bounding box of non-black pixels in RGB PNG; print crop params for sips."""
from __future__ import annotations

import struct
import sys
import zlib
from pathlib import Path


def decode_png_rgb(path: Path) -> tuple[int, int, list[memoryview]]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"Not a PNG: {path}")
    pos = 8
    idat = b""
    w = h = 0
    while pos < len(data):
        length = struct.unpack(">I", data[pos : pos + 4])[0]
        ctype = data[pos + 4 : pos + 8]
        chunk = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if ctype == b"IHDR":
            w, h = struct.unpack(">II", chunk[:8])
            color_type = chunk[9]
            if color_type != 2:
                raise SystemExit(f"Expected RGB PNG (color type 2), got {color_type}")
            if chunk[8] != 8:
                raise SystemExit("Expected 8-bit PNG")
        elif ctype == b"IDAT":
            idat += chunk
        elif ctype == b"IEND":
            break
    raw = zlib.decompress(idat)
    bpp = 3
    rows: list[memoryview] = []
    i = 0
    prev = bytearray(w * bpp)
    buf = memoryview(raw)
    for y in range(h):
        ft = buf[i]
        i += 1
        scan = bytearray(buf[i : i + w * bpp])
        i += w * bpp
        if ft == 0:
            pass
        elif ft == 1:
            for x in range(w * bpp):
                scan[x] = (scan[x] + (scan[x - bpp] if x >= bpp else 0)) & 255
        elif ft == 2:
            for x in range(w * bpp):
                scan[x] = (scan[x] + prev[x]) & 255
        elif ft == 3:
            for x in range(w * bpp):
                left = scan[x - bpp] if x >= bpp else 0
                scan[x] = (scan[x] + ((left + prev[x]) // 2)) & 255
        elif ft == 4:
            def paeth(a: int, b: int, c: int) -> int:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                if pa <= pb and pa <= pc:
                    return a
                if pb <= pc:
                    return b
                return c

            for x in range(w * bpp):
                a = scan[x - bpp] if x >= bpp else 0
                b = prev[x]
                c = prev[x - bpp] if x >= bpp else 0
                scan[x] = (scan[x] + paeth(a, b, c)) & 255
        else:
            raise SystemExit(f"Unsupported PNG filter {ft}")
        rows.append(memoryview(scan))
        prev = scan
    return w, h, rows


def main() -> None:
    if len(sys.argv) < 2:
        print("usage: crop_icon_black_margins.py <input.png>", file=sys.stderr)
        sys.exit(2)
    threshold = 28  # sum(R,G,B) — ignore near-black JPEG flattening
    path = Path(sys.argv[1])
    w, h, rows = decode_png_rgb(path)

    min_x, min_y = w, h
    max_x, max_y = 0, 0
    for y in range(h):
        row = rows[y]
        for x in range(w):
            o = x * 3
            if int(row[o]) + int(row[o + 1]) + int(row[o + 2]) > threshold:
                min_x = min(min_x, x)
                max_x = max(max_x, x)
                min_y = min(min_y, y)
                max_y = max(max_y, y)

    if max_x < min_x:
        raise SystemExit("No non-black pixels found")

    # Square crop centered on content (dock icons expect square)
    cw = max_x - min_x + 1
    ch = max_y - min_y + 1
    side = max(cw, ch)
    cx = (min_x + max_x) // 2
    cy = (min_y + max_y) // 2
    left = max(0, cx - side // 2)
    top = max(0, cy - side // 2)
    if left + side > w:
        left = w - side
    if top + side > h:
        top = h - side
    left = max(0, left)
    top = max(0, top)

    print(f"{side} {side} {top} {left}")


if __name__ == "__main__":
    main()

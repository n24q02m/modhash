#!/usr/bin/env python3
"""Generate the BMP fixtures for `modhash-bmp` (stdlib-only Python 3).

Every fixture is built byte-by-byte from the format definition — no BMP
library is involved — so the bytes are the ground truth, not an
encoder's opinion. The tests in `modhash-bmp/tests/bmp.rs` reimplement
each builder in Rust and assert equality with these files; a reader bug
in either direction is caught by the cross-check.

Run from the fixture directory: `python gen_fixtures.py` rewrites the
files and `PROVENANCE.md` hashes deterministically.
"""

import hashlib
import struct
from pathlib import Path

HERE = Path(__file__).parent


def bmp(width, height_signed, bpp, compression, dib_size=40,
        masks=None, palette=None, pixel_data=b"", colors_used=0,
        data_pad=0, declared_size=None):
    """Assemble a BMP file.

    `height_signed` may be negative for top-down rows. `masks` are the
    three/four u32 values (R,G,B[,A]); for dib_size=40 they are written
    inline after the header, for V4/V5 they live inside the header.
    `palette` is a list of (r,g,b) tuples. `data_pad` inserts that many
    bytes between structures and the pixel data (bfOffBits tracks it).
    """
    assert dib_size in (40, 108, 124)
    has_masks = compression in (3, 6)
    dib = struct.pack("<IiiHHIIiiII", dib_size, width, height_signed, 1,
                      bpp, compression, len(pixel_data), 2835, 2835,
                      colors_used, 0)
    dib += b"\0" * (dib_size - len(dib))
    if dib_size >= 108 and has_masks:
        m = list(masks or [0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000])
        m += [0] * (4 - len(m))
        dib = dib[:40] + struct.pack("<4I", *m[:4]) + dib[56:]
    pal = b""
    if palette is not None:
        for (r, g, b) in palette:
            pal += bytes([b, g, r, 0])
    header_end = 14 + dib_size
    if dib_size == 40 and has_masks:
        n = 4 if compression == 6 else 3
        mask_bytes = struct.pack("<%dI" % n, *(masks or
                                   [0x00FF0000, 0x0000FF00, 0x000000FF,
                                    0xFF000000])[:n])
    else:
        mask_bytes = b""
    data_offset = header_end + len(mask_bytes) + len(pal) + data_pad
    body = mask_bytes + pal + b"\xAA" * data_pad + pixel_data
    size = declared_size if declared_size is not None else 14 + dib_size + len(body)
    file_header = struct.pack("<2sIHHI", b"BM", size, 0, 0, data_offset)
    return file_header + dib + body


def rle8_stream(*ops):
    """Build an RLE8 stream from ops: ('enc', n, idx), ('abs', [...]),
    ('eol',), ('delta', dx, dy), ('eof',)."""
    out = bytearray()
    for op in ops:
        if op[0] == "enc":
            out += bytes([op[1], op[2]])
        elif op[0] == "abs":
            run = bytes(op[1])
            out += bytes([0, len(run)]) + run
            if len(run) % 2 == 1:
                out += b"\x00"
        elif op[0] == "eol":
            out += b"\x00\x00"
        elif op[0] == "delta":
            out += bytes([0, 2, op[1], op[2]])
        elif op[0] == "eof":
            out += b"\x00\x01"
    return bytes(out)


# 5x4 pixel matrix, RGBA, top-left origin. Distinct channels everywhere:
# R = x*37+y*11, G = x*19+y*53, B = x*71+y*29, all mod 256.
def matrix(w=5, h=4):
    return [[(x * 37 + y * 11) % 256, (x * 19 + y * 53) % 256,
             (x * 71 + y * 29) % 256, 255]
            for y in range(h) for x in range(w)]


W, H = 5, 4
PX = matrix(W, H)


def bgr_rows(px, w, h, top_down):
    rows = []
    order = range(h) if top_down else range(h - 1, -1, -1)
    for y in order:
        row = bytearray()
        for x in range(w):
            r, g, b, a = px[y * w + x]
            row += bytes([b, g, r])
        while len(row) % 4:
            row.append(0)
        rows.append(bytes(row))
    return b"".join(rows)


def bgra_rows(px, w, h, top_down, alpha=0x40):
    rows = []
    order = range(h) if top_down else range(h - 1, -1, -1)
    for y in order:
        row = bytearray()
        for x in range(w):
            r, g, b, _ = px[y * w + x]
            row += bytes([b, g, r, alpha])
        rows.append(bytes(row))
    return b"".join(rows)


def xrgb1555_rows(px, w, h, top_down):
    """5-bit channels under masks 0x7C00/0x03E0/0x001F (R,G,B) with the
    value quantized like a real 1555 file would store it."""
    rows = []
    order = range(h) if top_down else range(h - 1, -1, -1)
    for y in order:
        row = bytearray()
        for x in range(w):
            r, g, b, _ = px[y * w + x]
            r5, g5, b5 = r >> 3, g >> 3, b >> 3
            v = (r5 << 10) | (g5 << 5) | b5
            row += struct.pack("<I", v)
        rows.append(bytes(row))
    return b"".join(rows)


def gray_palette():
    return [(i, i, i) for i in range(256)]


# RLE8 image, 6x4, palette[i] = (i,i,i). File-space rows bottom-up.
# Desired logical image (top-left origin), index values:
#   row0 (top):    1 1 1 2 3 4
#   row1:          0 0 9 9 0 0
#   row2:          0 7 7 7 0 0
#   row3 (bottom): 5 0 0 0 8 8
# File writes bottom row first (y=0 = bottom).
# Absolute runs need n>=3 (n<3 collides with EOL/EOF/delta escapes).
# y=0: enc(1,5) enc(3,0) enc(2,8) -> 5,0,0,0,8,8
# y=1: delta(1,0) enc(3,7) -> 0,7,7,7,0,0
# y=2: enc(2,0) enc(2,9) eol -> 0,0,9,9,0,0
# y=3: enc(3,1) abs(2,3,4) eof -> 1,1,1,2,3,4
RLE8_STREAM = rle8_stream(
    ("enc", 1, 5), ("enc", 3, 0), ("enc", 2, 8), ("eol",),
    ("delta", 1, 0), ("enc", 3, 7), ("eol",),
    ("enc", 2, 0), ("enc", 2, 9), ("eol",),
    ("enc", 3, 1), ("abs", [2, 3, 4]), ("eof",),
)

FIXTURES = {
    "p24_bottom_up.bmp": bmp(
        W, H, 24, 0, pixel_data=bgr_rows(PX, W, H, False)),
    "p24_top_down.bmp": bmp(
        W, -H, 24, 0, pixel_data=bgr_rows(PX, W, H, True)),
    "p32_bitfields.bmp": bmp(
        W, H, 32, 3, masks=[0x00FF0000, 0x0000FF00, 0x000000FF,
                            0xFF000000],
        pixel_data=bgra_rows(PX, W, H, False)),
    "p32_x1555.bmp": bmp(
        W, H, 32, 3, masks=[0x00007C00, 0x000003E0, 0x0000001F, 0],
        pixel_data=xrgb1555_rows(PX, W, H, False)),
    "p32_v4_header.bmp": bmp(
        W, H, 32, 3, dib_size=108,
        masks=[0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000],
        pixel_data=bgra_rows(PX, W, H, False)),
    "p32_v5_rgb.bmp": bmp(
        W, H, 32, 0, dib_size=124,
        pixel_data=bgra_rows(PX, W, H, False, alpha=0x99)),
    "rle8.bmp": bmp(
        6, 4, 8, 1, palette=gray_palette(), pixel_data=RLE8_STREAM),
}


def main():
    lines = [
        "# BMP fixture provenance for `modhash-bmp`",
        "",
        "Every `.bmp` here is generated byte-by-byte by `gen_fixtures.py`",
        "(stdlib-only Python 3) directly from the BMP format definition; no",
        "BMP library writes these files, so they are honest ground truth.",
        "Regenerating reproduces the blobs bit-for-bit:",
        "`cd tests/fixtures && python gen_fixtures.py`.",
        "",
        "| file | shape | SHA-256 |",
        "|---|---|---|",
    ]
    for name in sorted(FIXTURES):
        blob = FIXTURES[name]
        (HERE / name).write_bytes(blob)
        digest = hashlib.sha256(blob).hexdigest()
        shape = {
            "p24_bottom_up.bmp": "5x4 24-bit BI_RGB, bottom-up",
            "p24_top_down.bmp": "5x4 24-bit BI_RGB, top-down (biHeight<0)",
            "p32_bitfields.bmp": "5x4 32-bit BI_BITFIELDS ARGB masks, INFOHEADER+inline masks",
            "p32_x1555.bmp": "5x4 32-bit BI_BITFIELDS 5-bit channels (masks 0x7C00/0x03E0/0x001F), no alpha",
            "p32_v4_header.bmp": "5x4 32-bit BITMAPV4HEADER, in-header masks + alpha",
            "p32_v5_rgb.bmp": "5x4 32-bit BITMAPV5HEADER BI_RGB (alpha byte ignored -> opaque)",
            "rle8.bmp": "6x4 RLE8, gray palette, encoded+absolute+delta+EOL+EOF",
        }[name]
        lines.append(f"| `{name}` ({len(blob)} B) | {shape} | `{digest}` |")
    lines.append("")
    lines.append("The RLE8 stream decodes (top-left origin, palette index =")
    lines.append("gray value) to:")
    lines.append("")
    lines.append("```")
    lines.append("1 1 1 2 3 4")
    lines.append("0 0 9 9 0 0")
    lines.append("0 7 7 7 0 0")
    lines.append("5 0 0 0 8 8")
    lines.append("```")
    lines.append("")
    (HERE / "PROVENANCE.md").write_text("\n".join(lines), encoding="utf-8")
    for name in sorted(FIXTURES):
        print(f"wrote {name}: {len(FIXTURES[name])} bytes")


if __name__ == "__main__":
    main()

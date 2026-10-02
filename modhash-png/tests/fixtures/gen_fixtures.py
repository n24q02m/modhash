#!/usr/bin/env python3
"""Fixture generator for modhash-png tests.

Writes the .png files in this directory plus prints the manifest table
that PROVENANCE.md records. Every fixture is built from scratch with a
byte-level writer — the test crate must never depend on a working PNG
library to check its own decoder.

Stdlib only: zlib (compression + crc32) and hashlib.

    python gen_fixtures.py          # writes the .png files next to itself
"""

import hashlib
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))


def chunk(ty, data):
    body = ty + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


def ihdr(w, h, depth, colour, interlace=0):
    return chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, depth, colour, 0, 0, interlace))


def paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c


def filter_row(kind, bpp, prior, row):
    out = bytearray(len(row))
    for x in range(len(row)):
        a = row[x - bpp] if x >= bpp else 0
        b = prior[x]
        c = prior[x - bpp] if x >= bpp else 0
        if kind == 0:
            pred = 0
        elif kind == 1:
            pred = a
        elif kind == 2:
            pred = b
        elif kind == 3:
            pred = (a + b) // 2
        else:
            pred = paeth(a, b, c)
        out[x] = (row[x] - pred) & 0xFF
    return bytes(out)


ADAM7 = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4),
         (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]


def pass_dim(origin, step, extent):
    return (extent - origin + step - 1) // step if extent > origin else 0


def scanlines(w, h, depth, colour, interlaced, rows_u16, filter_fn):
    """rows_u16[y][x] -> list of channel ints; returns filtered stream bytes."""
    ch = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[colour]
    bpp = (ch * depth + 7) // 8
    passes = ADAM7 if interlaced else [(0, 0, 1, 1)]
    out = bytearray()

    def pack(vals):
        if depth == 16:
            b = bytearray()
            for v in vals:
                b += struct.pack(">H", v)
            return bytes(b)
        if depth == 8:
            return bytes(vals)
        # sub-byte packing, MSB first
        b = bytearray((len(vals) * depth + 7) // 8)
        mask = (1 << depth) - 1
        for i, v in enumerate(vals):
            v &= mask
            bit = i * depth
            b[bit >> 3] |= v << (8 - depth - (bit & 7))
        return bytes(b)

    pass_i = 0
    for (x0, y0, dx, dy) in passes:
        pw = pass_dim(x0, dx, w)
        ph = pass_dim(y0, dy, h)
        if pw == 0 or ph == 0:
            pass_i += 1
            continue
        prior = bytes(pw * ch * (2 if depth == 16 else 1) if depth >= 8 else (pw * ch * depth + 7) // 8)
        for ry in range(ph):
            y = y0 + ry * dy
            vals = []
            for rx in range(pw):
                vals.extend(rows_u16[y][x0 + rx * dx])
            row = pack(vals)
            f = filter_fn(pass_i, ry)
            out.append(f)
            out += filter_row(f, bpp, prior, row)
            prior = row
        pass_i += 1
    return bytes(out)


def png(w, h, depth, colour, rows, *, interlace=0, filters=None, extras=b"",
        palette=None, trns=None):
    """filters(pass,row)->int, default all-None."""
    filt = filters or (lambda p, r: 0)
    data = zlib.compress(
        scanlines(w, h, depth, colour, interlace, rows, filt), 9)
    out = b"\x89PNG\r\n\x1a\n" + ihdr(w, h, depth, colour, interlace)
    if palette is not None:
        out += chunk(b"PLTE", bytes(palette))
    if trns is not None:
        out += chunk(b"tRNS", trns)
    out += extras
    out += chunk(b"IDAT", data)
    out += chunk(b"IEND", b"")
    return out


MANIFEST = []


def emit(name, blob, note):
    path = os.path.join(HERE, name)
    with open(path, "wb") as fh:
        fh.write(blob)
    MANIFEST.append((name, len(blob), hashlib.sha256(blob).hexdigest()[:16], note))


# ---------------------------------------------------------------------
# Pixel sources: small fixed matrices, and one 13x11 formula image.
# ---------------------------------------------------------------------

G8_4x4 = [[[7], [15], [31], [63]],
          [[0], [128], [192], [255]],
          [[3], [1], [4], [1]],
          [[59], [26], [53], [58]]]

G1_8x2 = [[[0], [1], [0], [1], [1], [0], [0], [1]],
          [[1], [1], [0], [0], [0], [1], [1], [0]]]

G4_5x3 = [[[0], [5], [10], [15], [1]],
          [[14], [9], [4], [8], [2]],
          [[7], [7], [7], [0], [15]]]

G16_3x2 = [[[0x0000], [0x1234], [0xBEEF]],
           [[0xFFFF], [0x00FF], [0x8000]]]

RGB8_5x5 = [[[x * 51, y * 51, (x + y) * 25] for x in range(5)] for y in range(5)]

RGB16_3x2 = [[[0x0001, 0xABCD, 0xFFFF], [0x1234, 0x5678, 0x9ABC],
              [0x8000, 0x0000, 0x7FFF]],
             [[0x1111, 0x2222, 0x3333], [0xF0F0, 0x0F0F, 0x55AA],
              [0xDEAD, 0xBEEF, 0xCAFE]]]

GA8_4x4 = [[[v, 255 - v] for v in row] for row in
           [[0, 64, 128, 192], [16, 80, 144, 208],
            [32, 96, 160, 224], [48, 112, 176, 240]]]

GA16_2x2 = [[[0x0000, 0xFFFF], [0x1234, 0x5678]],
            [[0x9ABC, 0xDEF0], [0x8000, 0x0001]]]

RGBA8_4x4 = [[[x * 64, y * 64, (x + y) * 32, 255 - x * 16]
              for x in range(4)] for y in range(4)]

RGBA16_2x2 = [[[0x0000, 0x1111, 0x2222, 0xFFFF],
               [0x3333, 0x4444, 0x5555, 0x6666]],
              [[0x7777, 0x8888, 0x9999, 0xAAAA],
               [0xBBBB, 0xCCCC, 0xDDDD, 0xEEEE]]]

PAL8 = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0,
        0, 255, 255, 255, 0, 255, 17, 34, 51, 99, 88, 77]
PAL8_IDX = [[[(x + y * 2) % 8] for x in range(4)] for y in range(4)]
PAL8_TRNS = bytes([255, 128, 0])  # entry 0 opaque, 1 half, 2 clear, rest opaque

# 13 entries so a 4-bit index field can exceed the palette on hostile
# input; values stay within u8.
PAL4 = [v for i in range(13) for v in
        ((i * 19) & 0xFF, (i * 37) & 0xFF, (i * 53) & 0xFF)]
PAL4_IDX = [[[x % 13] for x in range(9)], [[(x * 3) % 13] for x in range(9)]]

G8_TRNS = [[[10], [20], [10]], [[20], [10], [30]]]  # gray=10 transparent
G8_TRNS_VAL = struct.pack(">H", 10)

RGB8_TRNS = [[[1, 2, 3], [9, 9, 9], [4, 5, 6]],
             [[9, 9, 9], [1, 2, 3], [7, 8, 9]]]  # (9,9,9) transparent
RGB8_TRNS_VAL = struct.pack(">HHH", 9, 9, 9)

PAETH_TIE = [[[40], [20]], [[50], [33]]]


def a7_px(x, y):
    return [x * 17 + y, (x * 29 + y * 13) & 0xFF, (x ^ y) * 31 & 0xFF]


A7_13x11 = [[a7_px(x, y) for x in range(13)] for y in range(11)]
A7_5x5 = [[[(x * 51 + y * 37) & 0xFF] for x in range(5)] for y in range(5)]
A7_2x2 = [[[17], [91]], [[222], [3]]]

TEXT = chunk(b"tEXt", b"Title\x00modhash-png fixture")


def main():
    emit("gray8_4x4.png",
         png(4, 4, 8, 0, G8_4x4, extras=TEXT),
         "ct0 d8, filter 0, carries an ancillary tEXt")
    emit("gray1_8x2.png", png(8, 2, 1, 0, G1_8x2), "ct0 d1, bit replication")
    emit("gray4_5x3.png", png(5, 3, 4, 0, G4_5x3), "ct0 d4, odd nibble width")
    emit("gray16_3x2.png", png(3, 2, 16, 0, G16_3x2), "ct0 d16 big-endian")
    emit("rgb8_filters_5x5.png",
         png(5, 5, 8, 2, RGB8_5x5, filters=lambda p, r: r),
         "ct2 d8, rows 0-4 use filters 0-4 respectively")
    emit("rgb16_3x2.png", png(3, 2, 16, 2, RGB16_3x2), "ct2 d16")
    emit("ga8_4x4.png", png(4, 4, 8, 4, GA8_4x4), "ct4 d8, expands to Rgba8")
    emit("ga16_2x2.png", png(2, 2, 16, 4, GA16_2x2), "ct4 d16 -> Rgba16")
    emit("rgba8_4x4.png", png(4, 4, 8, 6, RGBA8_4x4), "ct6 d8")
    emit("rgba16_2x2.png", png(2, 2, 16, 6, RGBA16_2x2), "ct6 d16")
    emit("pal8_trns_4x4.png",
         png(4, 4, 8, 3, PAL8_IDX, palette=PAL8, trns=PAL8_TRNS),
         "ct3 d8, 8-entry PLTE, tRNS[0..2]=255,128,0")
    emit("pal4_9x2.png",
         png(9, 2, 4, 3, PAL4_IDX, palette=PAL4),
         "ct3 d4, 13-entry PLTE, no tRNS -> Rgb8")
    emit("gray8_trns_3x2.png",
         png(3, 2, 8, 0, G8_TRNS, trns=G8_TRNS_VAL),
         "ct0 d8 + tRNS gray=10 -> Rgba8")
    emit("rgb8_trns_3x2.png",
         png(3, 2, 8, 2, RGB8_TRNS, trns=RGB8_TRNS_VAL),
         "ct2 d8 + tRNS rgb=(9,9,9) -> Rgba8")
    emit("paeth_tie_2x2.png",
         png(2, 2, 8, 0, PAETH_TIE, filters=lambda p, r: 4),
         "ct0 d8 all-Paeth; row1x1 has pb==pc<pa tie -> predicts b")
    emit("adam7_13x11_rgb.png",
         png(13, 11, 8, 2, A7_13x11, interlace=1,
             filters=lambda p, r: (p * 7 + r) % 5),
         "ct2 d8 Adam7, per-pass filter mix")
    emit("adam7_5x5_gray8.png",
         png(5, 5, 8, 0, A7_5x5, interlace=1,
             filters=lambda p, r: (p + r) % 5),
         "ct0 d8 Adam7, tiny dims exercise empty passes")
    emit("adam7_2x2_gray8.png",
         png(2, 2, 8, 0, A7_2x2, interlace=1),
         "ct0 d8 Adam7 2x2: passes 1-4 are empty, only 5-7 carry rows")

    for name, size, sha, note in MANIFEST:
        print(f"| {name} | {size} | {sha} | {note} |")


if __name__ == "__main__":
    main()

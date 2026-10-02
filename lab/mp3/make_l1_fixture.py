#!/usr/bin/env python3
"""Emit a synthetic MPEG-1 Layer I stereo stream for the L1 fixture.

ffmpeg has no Layer I *encoder*, so the fixture is hand-built here per
ISO/IEC 11172-3 2.4.1: 32b header, then for each section the subband
loop is outermost and the channel loop innermost (allocation 4b,
scalefactors 6b, then 12 sample groups). Joint-stereo mode never
applies here (mode 0), so `bound` equals 32 for the sample loop.
ffmpeg's own mp1 *decoder* supplies the reference PCM.
"""
import struct

def bits_to_bytes(bits):
    out = bytearray()
    acc = 0; n = 0
    for b in bits:
        acc = (acc << 1) | b; n += 1
        if n == 8:
            out.append(acc); acc = 0; n = 0
    if n:
        out.append(acc << (8 - n))
    return bytes(out)

def put(bits, v, n):
    for i in range(n - 1, -1, -1):
        bits.append((v >> i) & 1)

SR = 44100
KBPS = 384  # index 12
frames = []
for f in range(16):
    bits = []
    # header: sync(11) ver(2) layer(2) prot(1) bri(4) sri(2) pad(1) priv(1)
    #         mode(2) mode_ext(2) copy(1) orig(1) emph(2)
    pad = 1
    put(bits, 0x7FF, 11); put(bits, 3, 2); put(bits, 3, 2); put(bits, 1, 1)
    put(bits, 12, 4); put(bits, 0, 2); put(bits, pad, 1); put(bits, 0, 1)
    put(bits, 0, 2); put(bits, 0, 2); put(bits, 0, 1); put(bits, 0, 1); put(bits, 0, 2)
    # allocation: subband outermost, channel innermost, 4 bits each.
    for sb in range(32):
        for ch in range(2):
            put(bits, 2, 4)
    # scalefactors: same ordering, 6 bits each; vary by subband/channel
    # to exercise the scalefactor table.
    for sb in range(32):
        for ch in range(2):
            put(bits, (sb + ch * 3) % 40, 6)
    # samples: 12 groups; within a group subband outermost, channel
    # innermost, `alloc` bits per sample.
    for s in range(12):
        for sb in range(32):
            for ch in range(2):
                put(bits, (sb * 12 + s + ch) % 4, 2)
    body = bits_to_bytes(bits)
    frame_bytes = (12 * KBPS * 1000 // SR + pad) * 4
    frames.append(body + bytes(frame_bytes - len(body)))
open("modhash-mp3/tests/fixtures/l1_stereo.mp1", "wb").write(b"".join(frames))
print("wrote", len(b"".join(frames)), "bytes,", len(frames), "frames")

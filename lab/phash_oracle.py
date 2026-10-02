#!/usr/bin/env python3
"""Independent oracle for the modhash facade's image pHash plus the
fixture generator for `modhash` (crate #21).

PROVENANCE: Python 3.13 stdlib only (zlib, struct, math, hashlib).
No image library is used anywhere in this file: PNGs are written
byte-by-byte (filter 0 per row), WAVs are minimal PCM16 RIFF, FLACs are
verbatim-subframe streams, and the pHash implementation is written
directly from docs/algorithms/kit.md section 3 — it shares no code with
the Rust implementation it verifies, which is the point of an oracle.

What it writes (all relative to the repo root, run from anywhere):

- modhash/tests/fixtures/*.png   - known-pixel reference images, one per
                                   PNG variant the pHash path normalizes
                                   (rgb8, rgba8, gray8, rgb16, gray16,
                                   palette+tRNS, Adam7 interlaced, and a
                                   sub-32 image that exercises the
                                   box_average upscaling branch).
- modhash/tests/fixtures/base_444.png
                                 - PNG re-encode of the JPEG lane's
                                   byte-exact reference pixels
                                   (modhash-jpeg/tests/fixtures/
                                   base_444.raw); the jpeg-vs-png
                                   acceptance pair.
- modhash/tests/fixtures/tone.wav / tone.flac
                                 - the same synthesized PCM in both
                                   containers (tier-1 audio equality).
- modhash/tests/phash_vectors.rs - committed oracle OUTPUT: the exact
                                   64-bit pHash each fixture must yield.
                                   Regenerate by running this script;
                                   never edit by hand.
- fuzz/corpus/{png,jpeg,bmp,audio}/* - fuzz seeds (jpeg/bmp seeds are
                                   copies of those crates' own fixtures,
                                   whose provenance lives in their
                                   directories; png/audio seeds are
                                   generated here).
- modhash/tests/fixtures/PROVENANCE.md and fuzz/corpus/PROVENANCE.md.

pHash chain (docs/algorithms/kit.md section 3, mirrored exactly):
luma = round_half_up((299r + 587g + 114b) / 1000) clamp [0,255];
box_average to 32x32 (lo = j*n//n', hi = (j+1)*n//n', empty -> centre
((2j+1)*n)//(2*n') clamped); orthonormal DCT-II rows then columns;
keep the low 8x8; threshold = lower-middle median of the 63 non-DC
coefficients (sorted, index 31); bit i of the hash (MSB = [0][0] of the
kept block, i.e. bit 63-(8y+x)) is coeff > threshold.
"""

import hashlib
import math
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------
# pHash oracle (the part that verifies Rust)
# ---------------------------------------------------------------------


def luma(r, g, b):
    """round_half_up((299r + 587g + 114b) / 1000), clamped to [0,255]."""
    return min((299 * r + 587 * g + 114 * b + 500) // 1000, 255)


def to_luma8(pixels, w, h, channels, depth):
    """Normalize a pixel matrix to a Gray u8 matrix.

    `pixels` is a flat row-major list of channel samples. RGBA drops
    alpha; 16-bit samples reduce by >> 8. Mirrors kit.md section 2-3.
    """
    out = []
    if depth == 16:
        pixels = [v >> 8 for v in pixels]
    if channels == 1:
        return list(pixels)
    for i in range(w * h):
        r, g, b = pixels[i * channels : i * channels + 3]
        out.append(luma(r, g, b))
    return out


def box_range(j, n_in, n_out):
    """[lo, hi) source range of output slot j (resample.rs, verbatim)."""
    lo = j * n_in // n_out
    hi = (j + 1) * n_in // n_out
    if lo < hi:
        return lo, hi
    c = ((2 * j + 1) * n_in) // (2 * n_out)
    c = min(c, n_in - 1)
    return c, c + 1


def box_average(gray, w, h, out_w, out_h):
    """Integer box average (resample.rs box_average, verbatim math)."""
    out = [0] * (out_w * out_h)
    for y in range(out_h):
        y0, y1 = box_range(y, h, out_h)
        for x in range(out_w):
            x0, x1 = box_range(x, w, out_w)
            n = (y1 - y0) * (x1 - x0)
            s = 0
            for sy in range(y0, y1):
                for sx in range(x0, x1):
                    s += gray[sy * w + sx]
            out[y * out_w + x] = (2 * s + n) // (2 * n)
    return out


def dct2(x):
    """1D orthonormal DCT-II: c_k * sqrt(2/N) * sum x[n] cos(pi(2n+1)k/2N)."""
    n = len(x)
    out = []
    for k in range(n):
        ck = math.sqrt(0.5) if k == 0 else 1.0
        acc = sum(x[i] * math.cos(math.pi * (2 * i + 1) * k / (2 * n)) for i in range(n))
        out.append(ck * math.sqrt(2.0 / n) * acc)
    return out


def dct2_2d(block, w, h):
    """Separable orthonormal DCT-II, rows then columns (math/dct.rs)."""
    rows = [dct2(block[y * w : (y + 1) * w]) for y in range(h)]
    out = [[0.0] * w for _ in range(h)]
    for x in range(w):
        col = dct2([rows[y][x] for y in range(h)])
        for y in range(h):
            out[y][x] = col[y]
    return out


def lower_median(values):
    """Workspace median: sorted v[(n - 1) // 2] — lower of two middles."""
    v = sorted(values)
    return v[(len(v) - 1) // 2]


def phash(pixels, w, h, channels=3, depth=8):
    """The 64-bit image pHash of a raw pixel matrix. Pure oracle."""
    gray = to_luma8(pixels, w, h, channels, depth)
    small = box_average(gray, w, h, 32, 32)
    block = [[float(small[y * 32 + x]) for x in range(32)] for y in range(32)]
    f = dct2_2d([v for row in block for v in row], 32, 32)
    block8 = [[f[y][x] for x in range(8)] for y in range(8)]
    rest = sorted(block8[y][x] for y in range(8) for x in range(8) if (x, y) != (0, 0))
    med = lower_median(rest)
    out = 0
    for y in range(8):
        for x in range(8):
            if block8[y][x] > med:
                out |= 1 << (63 - (y * 8 + x))
    return out


# ---------------------------------------------------------------------
# Byte-level writers (no image/audio library anywhere)
# ---------------------------------------------------------------------


def _crc32(data):
    return zlib.crc32(data) & 0xFFFFFFFF


def png_bytes(pixels, w, h, colour_type, bit_depth=8, palette=None, trns=None,
              interlace=False):
    """Minimal valid PNG. `pixels` = flat samples for non-interlaced, or a
    function (pass_id) -> (pw, ph, flat pass pixels) when interlaced."""
    sig = b"\x89PNG\r\n\x1a\n"

    def chunk(ty, body):
        c = ty + body
        return struct.pack(">I", len(body)) + c + struct.pack(">I", _crc32(c))

    ihdr = struct.pack(
        ">IIBBBBB", w, h, bit_depth, colour_type, 0, 0, 1 if interlace else 0
    )
    out = bytearray(sig + chunk(b"IHDR", ihdr))
    if palette is not None:
        out += chunk(b"PLTE", bytes(palette))
    if trns is not None:
        out += chunk(b"tRNS", bytes(trns))

    ch = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[colour_type]

    def scanlines(pw, ph, flat, chn, depth):
        """Rows as (filter=0 byte + packed row bytes). Supports 8/16-bit
        multi-channel and 8-bit palette (bpp>=8 rows only)."""
        rows = bytearray()
        if depth == 8:
            for y in range(ph):
                rows.append(0)
                for x in range(pw):
                    for k in range(chn):
                        rows.append(flat[(y * pw + x) * chn + k])
        elif depth == 16:
            for y in range(ph):
                rows.append(0)
                for x in range(pw):
                    for k in range(chn):
                        v = flat[(y * pw + x) * chn + k]
                        rows += struct.pack(">H", v)
        return rows

    raw = bytearray()
    if not interlace:
        raw += scanlines(w, h, pixels, ch, bit_depth)
    else:
        # Adam7 pass grid: (x0, y0, dx, dy).
        for x0, y0, dx, dy in ADAM7:
            pw = max(0, (w - x0 + dx - 1) // dx) if w > x0 else 0
            ph = max(0, (h - y0 + dy - 1) // dy) if h > y0 else 0
            if pw == 0 or ph == 0:
                continue
            flat = []
            for y in range(ph):
                for x in range(pw):
                    src = ((y0 + y * dy) * w + (x0 + x * dx)) * ch
                    flat.extend(pixels[src : src + ch])
            raw += scanlines(pw, ph, flat, ch, bit_depth)
    out += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    out += chunk(b"IEND", b"")
    return bytes(out)


ADAM7 = ((0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4),
         (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2))


def wav_bytes(samples, rate, channels=1):
    """PCM16 RIFF/WAVE; `samples` are i32 full-range -> written >> 16."""
    data = bytearray()
    for s in samples:
        data += struct.pack("<h", (s >> 16) & 0xFFFF if False else s >> 16)
    align = channels * 2
    fmt = struct.pack("<HHIIHH", 1, channels, rate, rate * align, align, 16)

    def chunk(tag, body):
        return tag + struct.pack("<I", len(body)) + body

    body = b"WAVE" + chunk(b"fmt ", fmt) + chunk(b"data", bytes(data))
    return b"RIFF" + struct.pack("<I", len(body)) + body


class _BitW:
    """MSB-first bit writer."""

    def __init__(self):
        self.buf = bytearray()
        self.cur = 0
        self.n = 0

    def bits(self, v, n):
        for i in range(n - 1, -1, -1):
            self.cur = (self.cur << 1) | ((v >> i) & 1)
            self.n += 1
            if self.n == 8:
                self.buf.append(self.cur)
                self.cur = 0
                self.n = 0

    def pad(self):
        while self.n:
            self.bits(0, 1)

    def bytes(self):
        assert self.n == 0
        return bytes(self.buf)


def _crc8(data):
    c = 0
    for b in data:
        c ^= b
        for _ in range(8):
            c = ((c << 1) ^ 0x07) & 0xFF if c & 0x80 else (c << 1) & 0xFF
    return c


def _crc16(data):
    c = 0
    for b in data:
        c ^= b << 8
        for _ in range(8):
            c = ((c << 1) ^ 0x8005) & 0xFFFF if c & 0x8000 else (c << 1) & 0xFFFF
    return c


def _flac_streaminfo(rate, channels, bps, total):
    w = _BitW()
    w.bits(1, 1)
    w.bits(0, 7)
    w.bits(34, 24)
    w.bits(16, 16)  # min block size
    w.bits(65535, 16)  # max block size
    w.bits(0, 24)
    w.bits(0, 24)
    w.bits(rate, 20)
    w.bits(channels - 1, 3)
    w.bits(bps - 1, 5)
    w.bits(total, 36)
    for _ in range(16):
        w.bits(0, 8)
    return w.bytes()


def _flac_verbatim_frame(number, block):
    """One mono verbatim frame. block = list of i16-range ints."""
    n = len(block)
    assert 1 <= n <= 65536
    h = _BitW()
    h.bits(0x3FFE, 14)
    h.bits(0, 1)  # reserved
    h.bits(0, 1)  # fixed blocking strategy -> coded number = frame number
    h.bits(7, 4)  # block size: 16-bit extra field at end of header
    h.bits(0, 4)  # sample rate from STREAMINFO
    h.bits(0, 4)  # mono
    h.bits(0, 3)  # bps from STREAMINFO
    h.bits(0, 1)  # reserved
    # coded frame number (UTF-8 style, small values = 1 byte)
    assert number < 0x80
    h.bits(number, 8)
    h.bits(n - 1, 16)  # block_size - 1
    hdr = h.bytes()
    hdr += bytes([_crc8(hdr)])
    s = _BitW()
    s.bits(0, 1)  # pad
    s.bits(1, 6)  # verbatim
    s.bits(0, 1)  # no wasted bits
    for v in block:
        s.bits(v & 0xFFFF, 16)
    s.pad()
    body = s.bytes()
    frame = hdr + body
    return frame + struct.pack(">H", _crc16(frame))


def flac_bytes(samples16, rate, channels=1, block_size=4096):
    """FLAC stream of verbatim frames. samples16 = flat interleaved i16."""
    assert channels == 1  # our fixtures are mono
    out = bytearray(b"fLaC" + _flac_streaminfo(rate, channels, 16,
                                               len(samples16) // channels))
    num = 0
    for off in range(0, len(samples16), block_size):
        blk = samples16[off : off + block_size]
        out += _flac_verbatim_frame(num, blk)
        num += 1
        assert num < 0x80
    return bytes(out)


# ---------------------------------------------------------------------
# Fixture matrices (the known pixels the oracle hashes)
# ---------------------------------------------------------------------


def gradient(w, h):
    return [
        c
        for y in range(h)
        for x in range(w)
        for c in ((x * 7 + y * 3) % 256, (x * 5 + y * 11) % 256,
                  (x * 13 + y * 2) % 256)
    ]


def blocks(w, h):
    out = []
    for y in range(h):
        for x in range(w):
            cx, cy = x // 8, y // 8
            out += [(cx * 37 + cy * 11) % 256, (cx * 3 + cy * 53) % 256,
                    (cx * 97 + cy * 7) % 256]
    return out


def curves(w, h):
    return [
        c
        for y in range(h)
        for x in range(w)
        for c in (((x * x + y * y) // 3) % 256, (x * 9) % 256, (y * 17) % 256)
    ]


def gray_grad(w, h):
    return [(x * 9 + y * 13 + ((x // 8) ^ (y // 8)) * 60) % 256
            for y in range(h) for x in range(w)]


def gradient16(w, h):
    out = []
    for y in range(h):
        for x in range(w):
            for c in ((x * 719 + y * 313) % 65536,
                      (x * 577 + y * 1021) % 65536,
                      (x * 1237 + y * 211) % 65536):
                out.append(c)
    return out


# ---------------------------------------------------------------------
# main: write everything
# ---------------------------------------------------------------------

FIX = ROOT / "modhash" / "tests" / "fixtures"
VEC = ROOT / "modhash" / "tests" / "phash_vectors.rs"
CORP = ROOT / "fuzz" / "corpus"

# name -> (file, pixels, w, h, channels, depth) — depth/channels describe
# the pixel matrix fed to the oracle; the PNG writer gets the same.
IMAGES = []

def add_png(name, pixels, w, h, colour_type, depth=8, channels=3,
            oracle_pixels=None, oracle_channels=None, **kw):
    data = png_bytes(pixels, w, h, colour_type, depth, **kw)
    path = FIX / f"{name}.png"
    path.write_bytes(data)
    # The oracle must hash the DECODED pixels, which for palette/tRNS
    # files is the expanded RGBA, not the stored indices.
    IMAGES.append((name, oracle_pixels if oracle_pixels is not None else pixels,
                   w, h, oracle_channels if oracle_channels is not None else channels,
                   depth, data))
    return path


def main():
    FIX.mkdir(parents=True, exist_ok=True)

    # --- known-pixel PNGs, one per normalized variant ---
    add_png("phash_rgb8_48x40", gradient(48, 40), 48, 40, 2)
    px = []
    for y in range(40):
        for x in range(48):
            px += [(x * 11 + y * 7) % 256, (x * 3 + y * 5) % 256,
                   (x * 2 + y * 17) % 256, (x * 31 + y * 13) % 256]
    add_png("phash_rgba8_48x40", px, 48, 40, 6, channels=4)
    g = gray_grad(40, 32)
    add_png("phash_gray8_40x32", g, 40, 32, 0, channels=1)
    add_png("phash_rgb16_40x24", gradient16(40, 24), 40, 24, 2, depth=16)
    g16 = [v * 257 for v in gray_grad(36, 28)]  # smooth 16-bit ramp
    add_png("phash_gray16_36x28", g16, 36, 28, 0, depth=16, channels=1)
    # palette + tRNS -> decodes to Rgba8; the oracle sees the EXPANDED
    # pixels, not the stored indices.
    pal = []
    ppx = []
    for i in range(16):
        pal += [(i * 17) % 256, (i * 29 + 40) % 256, (255 - i * 11) % 256]
    for y in range(44):
        for x in range(52):
            ppx.append((x * 5 + y * 7 + (x // 6) * (y // 4)) % 16)
    trns = bytes([255] * 15 + [128])
    expanded = []
    for idx in ppx:
        expanded += pal[idx * 3 : idx * 3 + 3] + [trns[idx]]
    add_png("phash_pal8_trns_52x44", ppx, 52, 44, 3,
            channels=1, palette=pal, trns=trns,
            oracle_pixels=expanded, oracle_channels=4)
    # Adam7 interlaced RGB8
    add_png("phash_adam7_37x29", blocks(37, 29), 37, 29, 2, interlace=True)
    # smaller than 32 -> box_average upscaling branch
    add_png("phash_small_13x9", curves(13, 9), 13, 9, 2)

    # --- the jpeg-vs-png acceptance pair ---
    # base_444.raw = Pillow's byte-exact decode of the committed JPEG
    # (32x24 RGB). Re-encoding those exact pixels as PNG means both sides
    # carry identical content by construction.
    raw = (ROOT / "modhash-jpeg" / "tests" / "fixtures" / "base_444.raw").read_bytes()
    path = FIX / "base_444.png"
    path.write_bytes(png_bytes(list(raw), 32, 24, 2))
    IMAGES.append(("base_444", list(raw), 32, 24, 3, 8, path.read_bytes()))

    # --- audio fixtures: identical PCM in WAV and FLAC ---
    # Deterministic pseudo-melody: two detuned sine partials plus a slow
    # AM, in full-range i32 WAV terms (written >>16) / i16 FLAC terms.
    n = 44100  # one second
    pcm16 = []
    import random as _r

    rng = _r.Random(0x5EED)
    noise = [rng.randint(-2000, 2000) for _ in range(n)]
    for i in range(n):
        t = i / 44100.0
        v = int(9000 * math.sin(2 * math.pi * 220 * t)
                + 5000 * math.sin(2 * math.pi * 331 * t)
                + 2500 * math.sin(2 * math.pi * 97 * t) * math.sin(2 * math.pi * 0.9 * t))
        pcm16.append(max(-32768, min(32767, v + noise[i])))
    wav = wav_bytes([s << 16 for s in pcm16], 44100, 1)
    (FIX / "tone.wav").write_bytes(wav)
    (FIX / "tone.flac").write_bytes(flac_bytes(pcm16, 44100, 1))
    # a 22050 Hz sibling for the describe() rate field
    half = pcm16[::2]
    (FIX / "tone22.wav").write_bytes(
        wav_bytes([s << 16 for s in half], 22050, 1))

    # --- oracle output (the committed expectation) ---
    lines = [
        "//! GENERATED FILE - do not edit.",
        "//!",
        "//! Written by `lab/phash_oracle.py` (the independent pHash oracle).",
        "//! Each row: (fixture file under tests/fixtures/, expected pHash,",
        "//! width, height). The expected value is computed by the Python",
        "//! implementation of docs/algorithms/kit.md section 3 - the Rust",
        "//! pipeline must match it exactly.",
        "",
        "/// (fixture name, expected 64-bit pHash, width, height).",
        "#[allow(dead_code)] // also compiles standalone as a test target",
        "pub(crate) const PHASH_VECTORS: &[(&str, u64, u32, u32)] = &[",
    ]
    for name, pixels, w, h, ch, depth, _data in IMAGES:
        v = phash(pixels, w, h, ch, depth)
        lines.append(f'    ("{name}.png", 0x{v:016x}, {w}, {h}),')
    lines.append("];\n")
    VEC.write_text("\n".join(lines), encoding="utf-8", newline="\n")

    # --- fuzz corpus ---
    (CORP / "png").mkdir(parents=True, exist_ok=True)
    (CORP / "audio").mkdir(parents=True, exist_ok=True)
    for name, pixels, w, h, ch, depth, data in IMAGES[:6]:
        (CORP / "png" / f"{name}.png").write_bytes(data)
    # jpeg/bmp seeds: byte copies of those crates' own conformance
    # fixtures (provenance: their tests/fixtures directories).
    import shutil

    (CORP / "jpeg").mkdir(parents=True, exist_ok=True)
    for src in ["base_444.jpg", "base_422.jpg", "prog_420.jpg",
                "base_gray.jpg", "base_420_odd.jpg"]:
        shutil.copyfile(ROOT / "modhash-jpeg" / "tests" / "fixtures" / src,
                        CORP / "jpeg" / src)
    (CORP / "bmp").mkdir(parents=True, exist_ok=True)
    for src in ["p24_top_down.bmp", "p24_bottom_up.bmp", "p32_bitfields.bmp",
                "p32_v4_header.bmp", "rle8.bmp", "p32_x1555.bmp"]:
        shutil.copyfile(ROOT / "modhash-bmp" / "tests" / "fixtures" / src,
                        CORP / "bmp" / src)
    (CORP / "audio" / "tone.wav").write_bytes(wav)
    (CORP / "audio" / "tone.flac").write_bytes(
        flac_bytes(pcm16[: 4096 * 3], 44100, 1))
    (CORP / "audio" / "tone22.wav").write_bytes(
        (FIX / "tone22.wav").read_bytes())

    # --- provenance files ---
    def table(rows):
        return "\n".join(
            "| {} | {} | {} |".format(n, len(d), hashlib.sha256(d).hexdigest()[:16])
            for n, d in rows
        )

    prov = FIX / "PROVENANCE.md"
    rows = [(p.name, p.read_bytes()) for p in sorted(FIX.iterdir())
            if p.suffix in (".png", ".wav", ".flac")]
    prov.write_text(
        "# Fixture provenance - modhash (kit facade)\n\n"
        "Generated offline by `lab/phash_oracle.py` "
        "(Python 3.13 stdlib only: zlib/struct/math/hashlib; no image or "
        "audio library). Regenerate with `python lab/phash_oracle.py` "
        "from the repository root; the same run also rewrites "
        "`tests/phash_vectors.rs` and the `fuzz/corpus/` seeds this lane "
        "owns.\n\n"
        "PNG fixtures are written byte-by-byte (filter 0 rows; Adam7 "
        "fixture emits per-pass scanlines in spec order). `base_444.png` "
        "is a PNG re-encode of `modhash-jpeg/tests/fixtures/base_444.raw` "
        "(Pillow's byte-exact decode of `base_444.jpg`) — the same "
        "pixels in both containers.\n\n"
        "`tone.wav`/`tone.flac` carry identical PCM: `tone.wav` is "
        "minimal PCM16 RIFF; `tone.flac` is a hand-built stream of "
        "verbatim subframes (RFC 9639) — no encoder library involved.\n\n"
        "| file | bytes | sha256:16 |\n|---|---|---|\n" + table(rows) + "\n",
        encoding="utf-8",
        newline="\n",
    )

    crow = []
    for d in ("png", "jpeg", "bmp", "audio"):
        for p in sorted((CORP / d).iterdir()):
            crow.append((f"{d}/{p.name}", p.read_bytes()))
    (CORP / "PROVENANCE.md").write_text(
        "# Fuzz corpus provenance\n\n"
        "Seeds for the corpus-backed fuzz targets registered by the kit "
        "facade phase (`modhash/src/bin/fuzz.rs`).\n\n"
        "- `png/*`, `audio/*` are generated by `lab/phash_oracle.py` "
        "(same run that writes `modhash/tests/fixtures/`); `audio/` "
        "holds WAV (minimal PCM16 RIFF) and FLAC (hand-built verbatim "
        "frames) of the same synthesized tone.\n"
        "- `jpeg/*` and `bmp/*` are byte copies of the conformance "
        "fixtures in `modhash-jpeg/tests/fixtures/` and "
        "`modhash-bmp/tests/fixtures/`; their provenance files there "
        "apply (Pillow 12.3.0 / hand-rolled byte writers).\n"
        "- `mp4/minimal.mp4` predates this phase (see git history).\n\n"
        "sha256 truncated to 16 hex digits.\n\n"
        "| file | bytes | sha256:16 |\n|---|---|---|\n" + table(crow) + "\n",
        encoding="utf-8",
        newline="\n",
    )

    for name, pixels, w, h, ch, depth, _ in IMAGES:
        print(f"{name}: {w}x{h} ch{ch} d{depth} "
              f"phash=0x{phash(pixels, w, h, ch, depth):016x}")
    print(f"fixtures: {len(list(FIX.iterdir()))} files; "
          f"corpus: {sum(1 for _ in CORP.rglob('*') if _.is_file())} files")


if __name__ == "__main__":
    sys.exit(main())

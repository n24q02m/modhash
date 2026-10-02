#!/usr/bin/env python3
"""Generate the known-answer vectors for modhash-inflate.

Every expected plaintext in the generated file comes from Python's zlib, which
is an implementation this workspace does not contain. Round-tripping the crate
against itself would prove nothing; round-tripping against zlib proves the
decoder agrees with an independent one.

Regenerate with:

    python modhash-inflate/gen_vectors.py

and commit the result together with the regeneration. The output is
deterministic: the only randomness is a seeded PRNG.

The generator also records, for each vector, which DEFLATE block type it
actually exercises - read from the stream itself, not assumed from the
compression level - so the suite can assert that all three block types are
covered instead of hoping they are.
"""

import zlib
import random
import struct
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "tests" / "vectors.rs"

# Block type is the low 3 bits of the first byte of a raw DEFLATE stream.
BLOCK_NAMES = {0: "stored", 1: "fixed", 2: "dynamic"}


def raw_block_type(compressed: bytes) -> int:
    """The first block's type, read from the stream's first bits.

    DEFLATE packs bits least-significant first: BFINAL is bit 0, BTYPE is
    bits 1 and 2. Reading bits 0..2 as one field would fold BFINAL into the
    type and misreport every final fixed block as the reserved type.
    """
    if not compressed:
        raise ValueError("empty deflate stream")
    btype = (compressed[0] >> 1) & 0b11
    if btype == 3:
        raise ValueError("first block is the reserved type")
    return btype

def zlib_header(compressed: bytes) -> bytes:
    cmf, flg = compressed[0], compressed[1]
    assert cmf & 0x0F == 8, "not deflate"
    assert (cmf << 8 | flg) % 31 == 0, "bad header check bits"
    flevel = flg >> 6
    fdict = bool(flg & 0x20)
    return f"cmf={cmf:#04x} flg={flg:#04x} flevel={flevel} fdict={fdict}"


# --- the plaintexts -------------------------------------------------------
# Chosen so that each one stresses a different decoder path.
rnd = random.Random(20261002)

PLAINS = []


def add(name: str, data: bytes, why: str) -> None:
    PLAINS.append((name, data, why))


add("empty", b"", "the empty payload; a stored block with LEN 0")
add("one_byte", b"A", "the shortest non-empty payload")
add("short_run", b"a" * 10, "a single back-reference whose distance exceeds the match")
add("overlap_run", b"ab" * 50, "distance 2 with length 100: the copy must overlap itself")
add("two_symbols", b"ab", "a distance tree with only one usable code")
add("text", (b"the quick brown fox jumps over the lazy dog. " * 12), "repetitive prose: long matches, one dynamic block")
add("incompressible", bytes(rnd.randrange(256) for _ in range(300)), "random bytes: forces stored blocks even at level 9")
add("mixed", bytes(rnd.randrange(256) for _ in range(200)) + b"z" * 400 + bytes(rnd.randrange(256) for _ in range(200)),
    "random bookends around a long run: stored block then a large match")
add("long_repeat", (b"the same forty two bytes over and over!!\n" * 400), "one match repeated past 32 KiB of output")
add("large", bytes((i * 7919) % 256 for i in range(70000)), "70 KB of structured bytes spanning many blocks")

# --- the malformed streams ------------------------------------------------
# Hand-edited from known-good streams so each has exactly one defect.

BAD = []


def add_bad(name: str, data: bytes, kind: str, why: str) -> None:
    BAD.append((name, data, kind, why))


_good_stored = zlib.compress(b"stored payload" * 8, 0)
add_bad("stored_nlen_mismatch", _good_stored[:4] + bytes([0xFF, 0xFF]) + _good_stored[6:], "BadValue",
        "NLEN is not the ones' complement of LEN")

# BFINAL=1, BTYPE=3. BTYPE occupies bits 1-2, so the low three bits must be
# 0b111; 0b101 sets BTYPE=2 and would exercise a truncated dynamic header
# instead. The raw_block_type assert further down is what would catch a
# repeat of that mistake.
_reserved = bytes([0b00000111, 0b00000000])
assert ((_reserved[0] >> 1) & 0b11) == 3, "reserved vector does not actually carry BTYPE=3"
add_bad("reserved_block_type", _reserved, "BadValue", "BTYPE 3 is reserved and must be refused")

_trunc = zlib.compress(b"x" * 500, 9)
add_bad("truncated_midstream", _trunc[: len(_trunc) // 2], "Truncated", "stream ends mid-block")

_badcrc = bytearray(zlib.compress(b"checksum me" * 40, 6))
_badcrc[-1] ^= 0xFF
add_bad("bad_adler32", bytes(_badcrc), "BadValue", "the Adler-32 trailer does not match the output")

# 0x7801 is 31 * 991, so it is a perfectly valid header - zlib emits it at
# level 0. 0x7802 leaves remainder 1 and is what the name claims.
assert 0x7801 % 31 == 0, "0x7801 is a valid zlib header after all"
add_bad("zlib_bad_header_checkbits", bytes([0x78, 0x02]), "InvalidMagic",
        "CMF/FLG fails the modulo-31 check, so this is not a zlib stream")
add_bad("zlib_bad_method", bytes([0x79, 0xBB]), "InvalidMagic", "compression method is 9, not 8")

# FDICT is set on a header whose check bits are already valid, so this vector
# has exactly one defect. 0x78bc would fail modulo 31 as well, which would make
# the vector test two things at once and leave FDICT-vs-FCHECK ordering
# untestable.
assert 0x78BB % 31 == 0
_fdict = bytearray(zlib.compress(b"preset dict" * 20, 6))
_fdict[1] = 0xBB
add_bad("zlib_preset_dictionary", bytes(_fdict), "Unsupported", "FDICT set: preset dictionaries are refused")

add_bad("empty_input", b"", "Truncated", "no bytes at all")

# A genuine back-reference reaching before the start of the output, packed bit
# by bit rather than guessed at: BFINAL=1, BTYPE=fixed, one literal 'A', then
# length 3 at distance 2 - but only one byte exists, so the copy underruns.
#
# The previous version of this vector was five hand-written bytes that turned
# out to be a stored block with a bad NLEN, so it silently duplicated the
# first vector while claiming to test something else.
class _Bits:
    """DEFLATE packs bits least-significant first; Huffman codes go in MSB first."""

    def __init__(self) -> None:
        self.bits: list[int] = []

    def put(self, code: int, length: int) -> "_Bits":
        for i in range(length - 1, -1, -1):
            self.bits.append((code >> i) & 1)
        return self

    def put_lsb(self, value: int, length: int) -> "_Bits":
        """Write a non-Huffman field, which the stream reads least-bit first.

        Only Huffman codes go in most-significant-bit first. BFINAL and BTYPE
        are plain fields, so writing 1 as `01` MSB-first makes the reader see
        BTYPE 2 - a dynamic block, and a stream that then fails for a reason
        unrelated to what it was built to test.
        """
        for i in range(length):
            self.bits.append((value >> i) & 1)
        return self

    def bytes(self) -> bytes:
        out = bytearray()
        for i in range(0, len(self.bits), 8):
            byte = 0
            for j, bit in enumerate(self.bits[i:i + 8]):
                byte |= bit << j
            out.append(byte)
        return bytes(out)


def _fixed_literal(sym: int) -> tuple[int, int]:
    """(code, bit length) for a literal under the fixed Huffman table."""
    if 0 <= sym <= 143:
        return 0b00110000 + sym, 8
    if 144 <= sym <= 255:
        return 0b110010000 + sym - 144, 9
    if 256 <= sym <= 279:
        return sym - 256, 7
    return 0b11000000 + sym - 280, 8


_w = _Bits().put_lsb(1, 1).put_lsb(0b01, 2)      # BFINAL=1, BTYPE=fixed
_lit, _litlen = _fixed_literal(ord("A"))
_w.put(_lit, _litlen)
_lc, _lclen = _fixed_literal(257)                # length code 257 -> length 3
_w.put(_lc, _lclen)
_w.put(0b00001, 5)                               # distance code 1 -> distance 2
_hand = _w.bytes()
add_bad("distance_before_start", _hand, "BadValue",
        "a back-reference at distance 2 when only one byte has been produced")

# --- emit -----------------------------------------------------------------

def hexlit(data: bytes) -> str:
    return '"' + data.hex() + '"'


def rust_str(s: str) -> str:
    """A quoted, escaped Rust string literal.

    Escaping without the surrounding quotes emitted `name: empty` instead of
    `name: "empty"`, which is a syntax error rather than a bad value, so it is
    worth stating in the docstring that the quotes are part of the contract.
    """
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


lines = [
    "//! Known-answer vectors for `modhash-inflate`.",
    "//!",
    "//! Generated - see `gen_vectors.py` in the crate root for the regenerating",
    "//! command and the provenance of every expected value. Every plaintext here",
    "//! came from Python's zlib, an implementation outside this workspace.",
    "//!",
    "//! The workspace denies `missing_docs` and that lint reaches test targets,",
    "//! so every item below, including each struct field, carries a doc comment.",
    "//! A plain `//` banner is not a crate doc and does not satisfy it.",
    "//!",
    "//! This is a data module, not a test. The harness includes it through",
    "//! `#[path = \"vectors.rs\"]`, but Cargo also compiles the file as its own",
    "//! test target, where nothing references these items, so `dead_code` fires",
    "//! under the workspace's `-D warnings`. The allow below covers that build",
    "//! and is scoped to this file rather than to the crate.",
    "#![allow(dead_code)]",
    "",
    "/// A known-answer vector: `compressed` must inflate to `plain`.",
    "pub struct Vector {",
    "    /// The vector's name, which is also the test's name.",
    "    pub name: &'static str,",
    "    /// What this vector is here to exercise.",
    "    pub why: &'static str,",
    "    /// The compressed bytes, hex encoded.",
    "    pub compressed: &'static str,",
    "    /// The expected output, hex encoded.",
    "    pub plain: &'static str,",
    "    /// The DEFLATE block type the stream actually uses: `stored`,",
    "    /// `fixed` or `dynamic`, read out of the stream rather than assumed",
    "    /// from the compression level.",
    "    pub block: &'static str,",
    "}",
    "",
    "/// A stream with exactly one defect, and the error it must produce.",
    "pub struct BadVector {",
    "    /// The vector's name, which is also the test's name.",
    "    pub name: &'static str,",
    "    /// What is wrong with this stream.",
    "    pub why: &'static str,",
    "    /// The malformed bytes, hex encoded.",
    "    pub input: &'static str,",
    "    /// The `Error` variant the decoder must return, by name.",
    "    pub kind: &'static str,",
    "}",
    "",
]

lines.append("/// Every valid vector, in both raw DEFLATE and zlib framing.")
lines.append("pub const VECTORS: &[Vector] = &[")
seen_blocks = set()
for name, data, why in PLAINS:
    raw = zlib.compressobj(9, zlib.DEFLATED, -15)
    raw_c = raw.compress(data) + raw.flush()
    z = zlib.compress(data, 9)
    bt = raw_block_type(raw_c)
    seen_blocks.add(bt)
    hdr = zlib_header(z)
    lines.append("    Vector {")
    lines.append(f"        name: {rust_str(name)},")
    lines.append(f"        why: {rust_str(why)},")
    lines.append(f"        compressed: {hexlit(raw_c)},")
    lines.append(f"        plain: {hexlit(data)},")
    lines.append(f"        block: {rust_str(BLOCK_NAMES[bt])},")
    lines.append("    },")
    lines.append("    Vector {")
    lines.append(f"        name: {rust_str(name + '/zlib')},")
    lines.append(f"        why: {rust_str(why)},")
    lines.append(f"        compressed: {hexlit(z)},")
    lines.append(f"        plain: {hexlit(data)},")
    lines.append(f"        block: {rust_str(BLOCK_NAMES[bt])},")
    lines.append("    },")
    print(f"  {name:16s} raw={len(raw_c):6d}B out={len(data):6d}B block={BLOCK_NAMES[bt]:8s} zlib={hdr}")

lines.append("];")
lines.append("")
lines.append("/// Every malformed stream.")
lines.append("pub const BAD_VECTORS: &[BadVector] = &[")
for name, data, kind, why in BAD:
    lines.append("    BadVector {")
    lines.append(f"        name: {rust_str(name)},")
    lines.append(f"        why: {rust_str(why)},")
    lines.append(f"        input: {hexlit(data)},")
    lines.append(f"        kind: {rust_str(kind)},")
    lines.append("    },")
    print(f"  BAD {name:24s} {len(data):4d}B -> {kind}")
lines.append("];")
lines.append("")

seen_names = {BLOCK_NAMES[b] for b in seen_blocks}
missing = set(BLOCK_NAMES.values()) - seen_names
if missing:
    raise SystemExit(f"vector set does not exercise block types: {sorted(missing)}")

OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text("\n".join(lines), encoding="utf-8", newline="\n")
print(f"\nblock types covered: {sorted(seen_names)}")
print(f"valid vectors: {len(PLAINS) * 2}   malformed: {len(BAD)}")
print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")
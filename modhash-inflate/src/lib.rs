//! DEFLATE and zlib stream decoding (RFC 1951, RFC 1950).
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package. A hashing kit that walks PNG, ZIP, DOCX and half the web has to
//! reach the bytes inside those containers, and it cannot add a compression
//! library without breaking that rule - so the decompressor is written here,
//! once.
//!
//! This crate **decompresses only**. A compressor is not needed by any
//! consumer in the workspace, and a half-tested compressor that claims to
//! produce valid DEFLATE is worse than none. gzip (RFC 1952) is recognised
//! by [`inflate_auto`] and refused with [`Error::Unsupported`] rather than
//! mis-decoded; preset dictionaries are refused for the same reason.
//!
//! Every entry point takes a [`Limits`]. A decompression API without a size
//! ceiling is a denial-of-service primitive: a few hundred bytes of input
//! expand to gigabytes. The crate is `no_std` apart from the `alloc`
//! [`Vec`](alloc::vec::Vec) its API returns, and no allocation is ever sized
//! from a length field read out of the stream without being clamped first.
//!
//! DEFLATE packs bits least-significant bit first: the first byte's bit 0 is
//! BFINAL and bits 1-2 are BTYPE. Huffman codes alone are packed
//! most-significant bit of the code first (RFC 1951 section 3.1.1).

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;
use core::cmp::Ordering;
use modhash_primitives::{Error, Result};

/// Maximum code length in bits; RFC 1951 never exceeds this.
const MAX_BITS: usize = 15;

/// Length bases for literal/length symbols 257..=285 (RFC 1951 3.2.5).
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];

/// Extra length bits for literal/length symbols 257..=285.
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// Distance bases for distance symbols 0..=29 (RFC 1951 3.2.5).
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

/// Extra distance bits for distance symbols 0..=29.
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// The order in which the code-length code lengths are stored in a dynamic
/// block header (RFC 1951 3.2.7).
const CLCL_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Ceilings a caller imposes on one decompression call.
///
/// [`Limits`] is not optional and there is no silently permissive default: a
/// decompressor without a ceiling on what it will produce is a
/// denial-of-service primitive. [`Default`] exists for the common case - a
/// caller hashing one file - and is a *conservative* ceiling, not an
/// unlimited one.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Hard ceiling on the produced output, in bytes. Producing more than
    /// this is [`Error::TooLarge`], never an allocation attempt: every
    /// allocation derived from a length field in the stream is clamped by
    /// this value before it happens. The default is 64 MiB, room for any
    /// single member of the containers this kit walks while a zip bomb dies
    /// quietly.
    pub max_output: usize,
    /// Hard ceiling on input consumed, in bytes. An input longer than this
    /// is [`Error::TooLarge`] before a single bit is read. The default is
    /// 64 MiB.
    pub max_input: usize,
}

impl Default for Limits {
    /// A conservative ceiling, not an unlimited one: 64 MiB of output and
    /// 64 MiB of input. Forcing every caller hashing a file to invent a
    /// number gets numbers invented wrong; an unlimited default would arm
    /// every caller with a denial-of-service primitive.
    fn default() -> Self {
        Limits {
            max_output: 64 * 1024 * 1024,
            max_input: 64 * 1024 * 1024,
        }
    }
}

/// Decompress a raw RFC 1951 DEFLATE stream.
///
/// Bytes after the final block are ignored: raw DEFLATE is embedded in
/// container formats that may pad, and [`inflate_zlib`] is the entry point
/// that demands an exact end of stream.
pub fn inflate_raw(input: &[u8], limits: &Limits) -> Result<Vec<u8>> {
    check_input(input, limits)?;
    Ok(deflate_stream(input, limits)?.0)
}

/// Decompress a zlib (RFC 1950) stream: two-byte header, DEFLATE payload,
/// big-endian Adler-32 trailer.
///
/// A header whose compression method is not 8, whose window size exceeds 7,
/// or whose FCHECK bits fail the modulo-31 test is [`Error::InvalidMagic`].
/// A stream that sets FDICT is [`Error::Unsupported`]: preset dictionaries
/// would require shipping DICTID mappings for values this crate does not
/// otherwise know, so it refuses rather than guesses. The FDICT test runs
/// before the FCHECK test - a stream that declares a preset dictionary is a
/// dictionary stream whatever its check bits say. A trailer that does not
/// match Adler-32 of the produced output, or trailing bytes after the
/// trailer, is [`Error::BadValue`].
pub fn inflate_zlib(input: &[u8], limits: &Limits) -> Result<Vec<u8>> {
    check_input(input, limits)?;
    if input.len() < 2 {
        return Err(Error::truncated("zlib header", 2, input.len()));
    }
    let cmf = input[0];
    let flg = input[1];
    if cmf & 0x0f != 8 {
        return Err(Error::InvalidMagic {
            what: "zlib CMF compression method",
        });
    }
    if cmf >> 4 > 7 {
        return Err(Error::InvalidMagic {
            what: "zlib CMF window size (CINFO)",
        });
    }
    if flg & 0x20 != 0 {
        return Err(Error::Unsupported("zlib preset dictionary (FDICT)"));
    }
    if ((u16::from(cmf) << 8) | u16::from(flg)) % 31 != 0 {
        return Err(Error::InvalidMagic {
            what: "zlib FCHECK header check bits",
        });
    }
    let body = &input[2..];
    let (out, consumed) = deflate_stream(body, limits)?;
    if body.len() < consumed + 4 {
        return Err(Error::truncated(
            "zlib Adler-32 trailer",
            consumed + 4,
            body.len(),
        ));
    }
    let stored = u32::from_be_bytes([
        body[consumed],
        body[consumed + 1],
        body[consumed + 2],
        body[consumed + 3],
    ]);
    if adler32(&out) != stored {
        return Err(Error::BadValue("zlib Adler-32 trailer mismatch"));
    }
    if body.len() > consumed + 4 {
        return Err(Error::BadValue("trailing bytes after the zlib stream"));
    }
    Ok(out)
}

/// Decompress either framing, sniffing the two-byte zlib header. A stream
/// that is neither raw DEFLATE nor zlib is an error, not a guess: gzip is
/// picked out by its `1f 8b` magic and refused with
/// `Error::Unsupported("gzip")` so the caller learns what it actually has.
pub fn inflate_auto(input: &[u8], limits: &Limits) -> Result<Vec<u8>> {
    check_input(input, limits)?;
    if input.len() >= 2 && input[0] == 0x1f && input[1] == 0x8b {
        return Err(Error::Unsupported("gzip"));
    }
    if looks_like_zlib(input) {
        inflate_zlib(input, limits)
    } else {
        inflate_raw(input, limits)
    }
}

/// Adler-32 as specified by RFC 1950 section 9: two modulo-65521 running
/// sums, returned as `(B << 16) | A` with `A` starting at 1.
///
/// Exposed because the zlib wrapper's trailer check is part of this crate's
/// correctness argument, and a test that cannot reach the checksum cannot
/// check it.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    // 5552 is the largest run that cannot overflow u32 sums (NMAX, RFC 1950).
    const NMAX: usize = 5552;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(NMAX) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// Rejects input longer than [`Limits::max_input`] before anything is read.
fn check_input(input: &[u8], limits: &Limits) -> Result<()> {
    if input.len() > limits.max_input {
        return Err(Error::too_large("deflate input", limits.max_input));
    }
    Ok(())
}

/// The two bytes could be a zlib header: method 8, window at most 7, and
/// check bits that satisfy the modulo-31 test. Used by [`inflate_auto`].
fn looks_like_zlib(input: &[u8]) -> bool {
    input.len() >= 2
        && input[0] & 0x0f == 8
        && input[0] >> 4 <= 7
        && ((u16::from(input[0]) << 8) | u16::from(input[1])) % 31 == 0
}

/// LSB-first bit reader over a byte slice. Every read that runs past the end
/// of the input is [`Error::Truncated`]; nothing here can panic on short
/// input.
struct BitReader<'a> {
    data: &'a [u8],
    /// Position in bits from the start of `data`.
    bitpos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, bitpos: 0 }
    }

    /// Reads one bit.
    fn bit(&mut self) -> Result<u32> {
        let index = self.bitpos / 8;
        if index >= self.data.len() {
            return Err(Error::truncated(
                "DEFLATE stream",
                index + 1,
                self.data.len(),
            ));
        }
        let bit = (self.data[index] >> (self.bitpos % 8)) & 1;
        self.bitpos += 1;
        Ok(u32::from(bit))
    }

    /// Reads a multi-bit value, least-significant bit first (RFC 1951
    /// 3.1.1). Reading zero bits reads nothing and yields zero.
    fn bits(&mut self, count: u32) -> Result<u32> {
        let mut value = 0;
        for i in 0..count {
            value |= self.bit()? << i;
        }
        Ok(value)
    }

    /// Skips to the next byte boundary.
    fn align(&mut self) {
        self.bitpos = (self.bitpos + 7) & !7;
    }

    /// Reads `count` whole bytes. The reader must be byte-aligned.
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let start = self.bitpos / 8;
        if start + count > self.data.len() {
            return Err(Error::truncated(
                "stored block",
                start + count,
                self.data.len(),
            ));
        }
        self.bitpos += count * 8;
        Ok(&self.data[start..start + count])
    }

    /// Bytes fully or partly consumed so far.
    fn consumed_bytes(&self) -> usize {
        self.bitpos.div_ceil(8)
    }
}

/// A canonical Huffman decoding table built from a code-length vector, per
/// RFC 1951 3.2.2. Decoding is the classic two-array walk (counts per
/// length, symbols sorted by length then symbol) - iterative, no recursion,
/// no table whose size depends on attacker input.
struct Huffman {
    /// Number of codes of each length 0..=15.
    count: [u16; MAX_BITS + 1],
    /// Symbols sorted by (length, symbol).
    symbol: [u16; 288],
}

impl Huffman {
    /// Builds the table. On success returns it plus the "code space left"
    /// indicator: 0 means the length vector is exactly complete, positive
    /// means under-subscribed (incomplete), and [`Error::BadValue`] means
    /// over-subscribed.
    fn build(lengths: &[u8]) -> Result<(Huffman, u32)> {
        let mut h = Huffman {
            count: [0; MAX_BITS + 1],
            symbol: [0; 288],
        };
        for &len in lengths {
            h.count[len as usize] += 1;
        }
        let mut left: i32 = 1;
        for len in 1..=MAX_BITS {
            left <<= 1;
            left -= i32::from(h.count[len]);
            if left < 0 {
                return Err(Error::BadValue("over-subscribed Huffman code lengths"));
            }
        }
        let mut offsets = [0usize; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + h.count[len] as usize;
        }
        for (sym, &len) in lengths.iter().enumerate() {
            if len != 0 {
                h.symbol[offsets[len as usize]] = sym as u16;
                offsets[len as usize] += 1;
            }
        }
        Ok((h, left as u32))
    }

    /// Decodes one symbol, reading bits most-significant-bit-of-code first.
    /// Running out of input mid-code is [`Error::Truncated`]; a bit sequence
    /// that matches no code is [`Error::BadValue`] - never a default code.
    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_BITS {
            code |= reader.bit()? as i32;
            let count = i32::from(self.count[len]);
            if code >= first && code - count < first {
                return Ok(self.symbol[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::BadValue("Huffman code matched no entry"))
    }
}

/// Builds a tree that must be exactly complete: the code-length tree and the
/// literal/length tree both have to fill their code space.
fn build_complete(lengths: &[u8], what: &'static str) -> Result<Huffman> {
    let (tree, left) = Huffman::build(lengths)?;
    if left != 0 {
        return Err(Error::BadValue(what));
    }
    Ok(tree)
}

/// Builds the distance tree. Incomplete is tolerated in exactly one case
/// RFC 1951 section 3.2.7 spells out: a single symbol encoded in one bit
/// (one code length of one, one unused code), which real encoders emit for
/// streams with no matches. Every other under-subscription is rejected
/// rather than half-decoded.
fn build_distance(lengths: &[u8]) -> Result<Huffman> {
    let (tree, left) = Huffman::build(lengths)?;
    if left == 0 {
        return Ok(tree);
    }
    let symbols = lengths.iter().filter(|&&len| len != 0).count();
    if symbols == 1 && lengths.contains(&1) {
        return Ok(tree);
    }
    Err(Error::BadValue("incomplete distance code lengths"))
}

/// The fixed Huffman tables of RFC 1951 section 3.2.6.
fn fixed_tables() -> Result<(Huffman, Huffman)> {
    let mut literal_lengths = [0u8; 288];
    for (sym, len) in literal_lengths.iter_mut().enumerate() {
        *len = match sym {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let literal = build_complete(&literal_lengths, "incomplete fixed literal/length tree")?;
    let distance = build_complete(&[5u8; 32], "incomplete fixed distance tree")?;
    Ok((literal, distance))
}

/// Reads the dynamic table description of RFC 1951 section 3.2.7: the
/// code-length code, then the literal/length and distance code lengths
/// encoded with it, including the 16/17/18 run-length forms.
fn dynamic_tables(reader: &mut BitReader<'_>) -> Result<(Huffman, Huffman)> {
    let nlit = reader.bits(5)? as usize + 257; // 257..=288
    let ndist = reader.bits(5)? as usize + 1; // 1..=32
    let ncl = reader.bits(4)? as usize + 4; // 4..=19
    let mut cl_lengths = [0u8; 19];
    for i in 0..ncl {
        cl_lengths[CLCL_ORDER[i]] = reader.bits(3)? as u8;
    }
    let cl_tree = build_complete(&cl_lengths, "incomplete code-length tree")?;

    let total = nlit + ndist; // at most 320
    let mut lengths = [0u8; 320];
    let mut filled = 0usize;
    while filled < total {
        let sym = cl_tree.decode(reader)?;
        match sym {
            0..=15 => {
                lengths[filled] = sym as u8;
                filled += 1;
            }
            16 => {
                if filled == 0 {
                    return Err(Error::BadValue(
                        "code-length repeat with no previous length",
                    ));
                }
                let repeat = 3 + reader.bits(2)? as usize;
                let previous = lengths[filled - 1];
                filled = fill_lengths(&mut lengths, filled, previous, repeat)?;
            }
            17 => {
                let repeat = 3 + reader.bits(3)? as usize;
                filled = fill_lengths(&mut lengths, filled, 0, repeat)?;
            }
            18 => {
                let repeat = 11 + reader.bits(7)? as usize;
                filled = fill_lengths(&mut lengths, filled, 0, repeat)?;
            }
            _ => return Err(Error::BadValue("code-length symbol out of range")),
        }
    }
    let literal = build_complete(&lengths[..nlit], "incomplete literal/length code lengths")?;
    let distance = build_distance(&lengths[nlit..total])?;
    Ok((literal, distance))
}

/// Appends `count` copies of `value` to the combined length vector, refusing
/// runs that would overflow the literal/length and distance counts.
fn fill_lengths(lengths: &mut [u8; 320], filled: usize, value: u8, count: usize) -> Result<usize> {
    if filled + count > 320 {
        return Err(Error::BadValue("code-length run overflows the table"));
    }
    lengths[filled..filled + count].fill(value);
    Ok(filled + count)
}

/// Decodes one Huffman block body: literals, matches, and the end-of-block
/// symbol. Every iteration reads at least one bit, so the loop always makes
/// progress or errors.
fn inflate_block(
    reader: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    literal: &Huffman,
    distance: &Huffman,
    limits: &Limits,
) -> Result<()> {
    loop {
        let sym = literal.decode(reader)?;
        match sym.cmp(&256) {
            Ordering::Less => {
                if out.len() >= limits.max_output {
                    return Err(Error::too_large("inflated output", limits.max_output));
                }
                out.push(sym as u8);
            }
            Ordering::Equal => return Ok(()),
            Ordering::Greater => {
                let length_index = sym as usize - 257;
                if length_index >= LENGTH_BASE.len() {
                    return Err(Error::BadValue("reserved length code"));
                }
                let length = LENGTH_BASE[length_index] as usize
                    + reader.bits(u32::from(LENGTH_EXTRA[length_index]))? as usize;
                let dsym = distance.decode(reader)? as usize;
                if dsym >= DIST_BASE.len() {
                    return Err(Error::BadValue("reserved distance code"));
                }
                let dist =
                    DIST_BASE[dsym] as usize + reader.bits(u32::from(DIST_EXTRA[dsym]))? as usize;
                // A distance reaching before the start of the output is the
                // classic out-of-bounds read; it is a checked error, not one.
                if dist > out.len() {
                    return Err(Error::BadValue(
                        "back-reference before the start of the output",
                    ));
                }
                if out.len() + length > limits.max_output {
                    return Err(Error::too_large("inflated output", limits.max_output));
                }
                // A distance smaller than the length is legal and common: the
                // copy must walk forward one byte at a time so the source it
                // reads includes bytes the copy itself just wrote.
                let start = out.len() - dist;
                for i in 0..length {
                    let byte = out[start + i];
                    out.push(byte);
                }
            }
        }
    }
}

/// Decodes a raw DEFLATE stream: the block sequence and nothing after it.
/// Returns the output and how many input bytes the final block consumed.
fn deflate_stream(input: &[u8], limits: &Limits) -> Result<(Vec<u8>, usize)> {
    let mut reader = BitReader::new(input);
    let mut out = Vec::new();
    loop {
        let bfinal = reader.bits(1)?;
        let btype = reader.bits(2)?;
        match btype {
            0 => {
                reader.align();
                let len = reader.bits(16)? as usize;
                let nlen = reader.bits(16)? as u16;
                if nlen != !(len as u16) {
                    return Err(Error::BadValue(
                        "stored block NLEN is not the ones' complement of LEN",
                    ));
                }
                if out.len() + len > limits.max_output {
                    return Err(Error::too_large("inflated output", limits.max_output));
                }
                let bytes = reader.take(len)?;
                out.extend_from_slice(bytes);
            }
            1 => {
                let (literal, distance) = fixed_tables()?;
                inflate_block(&mut reader, &mut out, &literal, &distance, limits)?;
            }
            2 => {
                let (literal, distance) = dynamic_tables(&mut reader)?;
                inflate_block(&mut reader, &mut out, &literal, &distance, limits)?;
            }
            // RFC 1951 3.2.5: a compliant decoder must refuse type 3.
            _ => return Err(Error::BadValue("reserved block type 3")),
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok((out, reader.consumed_bytes()))
}

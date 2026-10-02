//! Harness for the generated known-answer corpus in `vectors.rs`, plus
//! hand-built streams for the behaviours the corpus's generating
//! implementation (Python `zlib`) cannot be made to emit.
//!
//! Every expected output in `VECTORS` came from outside this workspace; no
//! test here round-trips through this crate or through a compressor.

#[path = "vectors.rs"]
mod vectors;

use modhash_inflate::{Limits, adler32, inflate_auto, inflate_raw, inflate_zlib};
use modhash_primitives::Error;
use vectors::{BAD_VECTORS, VECTORS, Vector};

/// Decodes the lowercase-hex encoding `vectors.rs` uses for every byte
/// field.
fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "odd hex length in {s:?}");
    s.as_bytes()
        .chunks(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16).expect("hex digit");
            let lo = (pair[1] as char).to_digit(16).expect("hex digit");
            ((hi << 4) | lo) as u8
        })
        .collect()
}

/// Looks a vector up by name so tests can pin their scenario to a corpus
/// entry instead of an index.
fn vector(name: &str) -> &'static Vector {
    VECTORS
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no vector named {name}"))
}

/// The variant name of an error, for comparison against a `BadVector::kind`.
fn kind_of(err: &Error) -> &'static str {
    match err {
        Error::Truncated { .. } => "Truncated",
        Error::InvalidMagic { .. } => "InvalidMagic",
        Error::BadValue(_) => "BadValue",
        Error::Unsupported(_) => "Unsupported",
        Error::TooLarge { .. } => "TooLarge",
    }
}

/// The same structural sniff `inflate_auto` documents. Bad vectors that are
/// zlib-framed route through `inflate_zlib`; the `zlib_*` ones are routed by
/// name because the whole point of several of them is that their bytes do
/// *not* form a valid header.
fn looks_like_zlib(bytes: &[u8]) -> bool {
    bytes.len() >= 2
        && bytes[0] & 0x0f == 8
        && bytes[0] >> 4 <= 7
        && ((u16::from(bytes[0]) << 8) | u16::from(bytes[1])) % 31 == 0
}

#[test]
fn known_answer_vectors_decode_byte_for_byte() {
    for v in VECTORS {
        let compressed = unhex(v.compressed);
        let plain = unhex(v.plain);
        let limits = Limits::default();
        let out = if v.name.ends_with("/zlib") {
            inflate_zlib(&compressed, &limits)
        } else {
            inflate_raw(&compressed, &limits)
        };
        let out = out.unwrap_or_else(|e| panic!("vector {} ({}) failed: {e}", v.name, v.why));
        assert_eq!(
            out, plain,
            "vector {} ({}) decoded wrong bytes",
            v.name, v.why
        );
    }
}

#[test]
fn auto_sniff_routes_every_corpus_vector() {
    for v in VECTORS {
        let compressed = unhex(v.compressed);
        let plain = unhex(v.plain);
        let out = inflate_auto(&compressed, &Limits::default())
            .unwrap_or_else(|e| panic!("auto on {} failed: {e}", v.name));
        assert_eq!(out, plain, "auto on {}", v.name);
    }
}

#[test]
fn corpus_actually_covers_every_block_type() {
    let blocks: Vec<&str> = VECTORS.iter().map(|v| v.block).collect();
    for needed in ["stored", "fixed", "dynamic"] {
        assert!(
            blocks.contains(&needed),
            "corpus lost its {needed} coverage: a regeneration dropped a block type"
        );
    }
}

#[test]
fn malformed_streams_produce_the_named_variant() {
    for v in BAD_VECTORS {
        let input = unhex(v.input);
        let zlib_routed = v.name.starts_with("zlib_") || looks_like_zlib(&input);
        let result = if zlib_routed {
            inflate_zlib(&input, &Limits::default())
        } else {
            inflate_raw(&input, &Limits::default())
        };
        let err = result.expect_err(&format!(
            "vector {} ({}) unexpectedly decoded",
            v.name, v.why
        ));
        assert_eq!(
            kind_of(&err),
            v.kind,
            "vector {} ({}) failed for the wrong reason: {err}",
            v.name,
            v.why
        );
    }
}

#[test]
fn gzip_sniff_reports_unsupported() {
    let gz = [0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0, 0xff];
    match inflate_auto(&gz, &Limits::default()) {
        Err(Error::Unsupported("gzip")) => {}
        other => panic!("gzip magic must be Unsupported(\"gzip\"), got {other:?}"),
    }
}

#[test]
fn zlib_stream_may_not_carry_trailing_bytes() {
    let mut input = unhex(vector("one_byte/zlib").compressed);
    input.push(0x00);
    match inflate_zlib(&input, &Limits::default()) {
        Err(Error::BadValue(_)) => {}
        other => panic!("a trailing byte must be BadValue, got {other:?}"),
    }
}

#[test]
fn adler32_matches_hand_computed_values() {
    assert_eq!(adler32(b""), 1);
    assert_eq!(adler32(b"a"), 0x0062_0062);
}

// --- limits ----------------------------------------------------------------

#[test]
fn output_ceiling_is_enforced() {
    let v = vector("text/zlib");
    let compressed = unhex(v.compressed);
    let plain = unhex(v.plain);
    // Exactly at the ceiling succeeds.
    let exact = Limits {
        max_output: plain.len(),
        ..Limits::default()
    };
    assert_eq!(inflate_zlib(&compressed, &exact).unwrap(), plain);
    // One byte under the ceiling the stream needs: TooLarge, not OOM.
    let tight = Limits {
        max_output: plain.len() - 1,
        ..Limits::default()
    };
    match inflate_zlib(&compressed, &tight) {
        Err(Error::TooLarge { .. }) => {}
        other => panic!("output over the ceiling must be TooLarge, got {other:?}"),
    }
}

/// "incompressible" is one stored block, so its LEN field is a size read
/// straight out of the stream: a ceiling below it must refuse before any
/// allocation is attempted.
#[test]
fn stored_header_claiming_too_much_output_is_too_large() {
    let v = vector("incompressible/zlib");
    let compressed = unhex(v.compressed);
    let plain = unhex(v.plain);
    let tight = Limits {
        max_output: plain.len() - 1,
        ..Limits::default()
    };
    match inflate_zlib(&compressed, &tight) {
        Err(Error::TooLarge { .. }) => {}
        other => panic!("stored LEN over the ceiling must be TooLarge, got {other:?}"),
    }
}

#[test]
fn input_ceiling_is_enforced() {
    let v = vector("one_byte/zlib");
    let compressed = unhex(v.compressed);
    let tight = Limits {
        max_input: compressed.len() - 1,
        ..Limits::default()
    };
    match inflate_zlib(&compressed, &tight) {
        Err(Error::TooLarge { .. }) => {}
        other => panic!("input over the ceiling must be TooLarge, got {other:?}"),
    }
}

// --- hand-built dynamic blocks ---------------------------------------------
//
// Python's zlib never emits a one-symbol distance tree (it forces at least
// two codes), so the RFC 1951 3.2.7 special case - "if only one distance
// code is used, it is encoded using one bit" - has to be pinned with a
// stream packed here. `Bits` is the mirror of the implementation's reader:
// value fields least-significant bit first, Huffman codes most-significant
// bit of the code first (RFC 1951 3.1.1).

struct Bits {
    bytes: Vec<u8>,
    nbits: usize,
}

impl Bits {
    fn new() -> Self {
        Bits {
            bytes: Vec::new(),
            nbits: 0,
        }
    }

    fn push_bit(&mut self, bit: u32) {
        if self.nbits % 8 == 0 {
            self.bytes.push(0);
        }
        if bit != 0 {
            self.bytes[self.nbits / 8] |= 1 << (self.nbits % 8);
        }
        self.nbits += 1;
    }

    /// A value field: least-significant bit first.
    fn field(&mut self, value: u32, count: u32) {
        for i in 0..count {
            self.push_bit((value >> i) & 1);
        }
    }

    /// A Huffman code: most-significant bit of the code first.
    fn code(&mut self, value: u32, count: u32) {
        for i in (0..count).rev() {
            self.push_bit((value >> i) & 1);
        }
    }
}

/// The dynamic-block header: BFINAL, BTYPE=2, HLIT/HDIST/HCLEN, then the
/// code-length code lengths in the RFC's permuted order. HCLEN is computed
/// from the highest used entry.
fn dynamic_header(bits: &mut Bits, hlit: u32, hdist: u32, cl: &[u8; 19]) {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    bits.field(1, 1); // BFINAL
    bits.field(2, 2); // BTYPE = dynamic
    bits.field(hlit, 5);
    bits.field(hdist, 5);
    let entries = ORDER
        .iter()
        .rposition(|&sym| cl[sym] != 0)
        .map(|i| i + 1)
        .unwrap_or(4)
        .max(4);
    bits.field(entries as u32 - 4, 4);
    for &sym in &ORDER[..entries] {
        bits.field(u32::from(cl[sym]), 3);
    }
}

/// Builds a stream whose dynamic block declares: literal/length lengths
/// 88=>1, 256=>2, 257=>2 over 258 symbols (a complete tree: codes 0, 10,
/// 11), and a single distance symbol 0 with length 1 - the incomplete
/// one-symbol distance tree RFC 1951 permits. The block body is one literal
/// 'X', one match (length 3, distance 1), end-of-block: output "XXXX".
fn one_symbol_distance_stream(distance_code: u32, distance_pad_ones: u32) -> Bits {
    let mut bits = Bits::new();
    // Code-length code: symbols 1, 2, 17, 18 all length 2 => complete,
    // canonical codes 1=>00, 2=>01, 17=>10, 18=>11.
    let mut cl = [0u8; 19];
    cl[1] = 2;
    cl[2] = 2;
    cl[17] = 2;
    cl[18] = 2;
    dynamic_header(&mut bits, 1, 0, &cl); // 258 literal/length, 1 distance code
    // 258 + 1 lengths: 88 zeros, len(88)=1, 138 zeros, 29 zeros, len(256)=2,
    // len(257)=2, dist len(0)=1.
    bits.code(0b11, 2);
    bits.field(88 - 11, 7); // 18: repeat zero x88
    bits.code(0b00, 2); // 1
    bits.code(0b11, 2);
    bits.field(138 - 11, 7); // 18: x138
    bits.code(0b11, 2);
    bits.field(29 - 11, 7); // 18: x29
    bits.code(0b01, 2); // 2
    bits.code(0b01, 2); // 2
    bits.code(0b00, 2); // 1
    // Body: literal 'X' (code 0), length symbol 257 (code 11), distance
    // symbol, end-of-block symbol 256 (code 10).
    bits.code(0, 1);
    bits.code(0b11, 2);
    bits.code(distance_code, 1);
    // Ones pad the walk off the end of the one-code tree: code "1" at length
    // 1 matches nothing, and the decoder must walk lengths 2..=15 without a
    // match rather than guess a symbol.
    bits.field(distance_pad_ones, distance_pad_ones);
    bits.code(0b10, 2);
    bits
}

#[test]
fn dynamic_one_symbol_distance_tree_is_accepted() {
    let bits = one_symbol_distance_stream(0, 0);
    let out = inflate_raw(&bits.bytes, &Limits::default())
        .expect("the one-symbol distance tree is legal per RFC 1951 3.2.7");
    assert_eq!(out, b"XXXX");
}

#[test]
fn dynamic_unused_distance_code_is_bad_value() {
    // Distance code "1": the unassigned half of the one-code tree. Padded
    // with 15 ones so the decode walk runs to length 15 and fails on "no
    // code matched", not on running out of bits.
    let bits = one_symbol_distance_stream(1, 15);
    match inflate_raw(&bits.bytes, &Limits::default()) {
        Err(Error::BadValue(_)) => {}
        other => panic!("unmatched Huffman code must be BadValue, got {other:?}"),
    }
}

#[test]
fn dynamic_oversubscribed_literal_tree_is_bad_value() {
    let mut bits = Bits::new();
    // Code-length code: symbols 1 and 18, both length 1 => complete,
    // canonical codes 1=>0, 18=>1.
    let mut cl = [0u8; 19];
    cl[1] = 1;
    cl[18] = 1;
    dynamic_header(&mut bits, 0, 0, &cl); // 257 literal/length, 1 distance code
    // Lengths: 1, 1, then 254 zeros (138 + 116), then 1, 1: symbols 0, 1 and
    // 256 all claim length 1 - three halves of a one-bit code space.
    bits.code(0, 1); // 1
    bits.code(0, 1); // 1
    bits.code(1, 1);
    bits.field(138 - 11, 7); // 18: x138
    bits.code(1, 1);
    bits.field(116 - 11, 7); // 18: x116
    bits.code(0, 1); // 1 (symbol 256)
    bits.code(0, 1); // 1 (the distance symbol)
    match inflate_raw(&bits.bytes, &Limits::default()) {
        Err(Error::BadValue(_)) => {}
        other => panic!("over-subscribed code lengths must be BadValue, got {other:?}"),
    }
}

#[test]
fn dynamic_incomplete_literal_tree_is_bad_value() {
    let mut bits = Bits::new();
    // Code-length code: symbols 1=>1, 2=>2, 18=>2 => complete (canonical
    // codes 1=>0, 2=>10, 18=>11).
    let mut cl = [0u8; 19];
    cl[1] = 1;
    cl[2] = 2;
    cl[18] = 2;
    dynamic_header(&mut bits, 0, 0, &cl); // 257 literal/length, 1 distance code
    // Lengths: symbol 0=>1, symbol 1=>2, 255 zeros (138 + 117), then the
    // distance symbol's length 1. The literal tree uses three quarters of
    // its code space and is not the distance special case.
    bits.code(0, 1); // 1
    bits.code(0b10, 2); // 2
    bits.code(0b11, 2);
    bits.field(138 - 11, 7); // 18: x138
    bits.code(0b11, 2);
    bits.field(117 - 11, 7); // 18: x117
    bits.code(0, 1); // 1 (the distance symbol)
    match inflate_raw(&bits.bytes, &Limits::default()) {
        Err(Error::BadValue(_)) => {}
        other => panic!("incomplete literal tree must be BadValue, got {other:?}"),
    }
}

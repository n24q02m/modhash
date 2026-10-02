//! Known-answer tests for `modhash-primitives`.
//!
//! Every assertion pins a value that can be checked by hand against
//! `docs/algorithms/primitives.md`. Nothing here merely asserts "it did
//! not panic" or "the output is non-empty".

use std::io::Cursor;
use std::io::Read as IoRead;

use modhash_primitives::Read as Source;
use modhash_primitives::{Algorithm, Digest, Error, Format, hamming};

/// Adapts any `std::io::Read` to the kit's `no_std` source trait: an
/// I/O error surfaces as exhaustion, which `from_stream` reports as
/// `Truncated`.
struct FromIo<R: IoRead>(R);

impl<R: IoRead> Source for FromIo<R> {
    fn read_some(&mut self, buf: &mut [u8]) -> usize {
        self.0.read(buf).unwrap_or(0)
    }
}

/// A source that hands out one byte per call, to prove `from_stream`
/// follows partial reads.
struct Dribble<'a>(&'a [u8]);

impl Source for Dribble<'_> {
    fn read_some(&mut self, buf: &mut [u8]) -> usize {
        let n = self.0.len().min(buf.len()).min(1);
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        n
    }
}

// ------------------------------------------------------------- hamming

/// Identical digests: zero differing bits.
#[test]
fn hamming_identical_digests_are_zero_apart() {
    let a = Digest::<4>::from_bytes([0xa5, 0x5a, 0xff, 0x00]);
    let b = a;
    assert_eq!(hamming(&a, &b), 0);
}

/// All bits differ: exactly N * 8.
#[test]
fn hamming_all_bits_differing_is_eight_per_byte() {
    let a = Digest::<4>::from_bytes([0x00; 4]);
    let b = Digest::<4>::from_bytes([0xff; 4]);
    assert_eq!(hamming(&a, &b), 32); // 4 bytes * 8 bits
}

/// Hand-counted asymmetric case: against an all-zero digest, 0x0F
/// differs by 4 bits, 0x00 by 0, 0xFF by 8 and 0x80 by 1, so the total
/// is 4 + 0 + 8 + 1 = 13.
#[test]
fn hamming_asymmetric_hand_counted_case() {
    let a = Digest::<4>::from_bytes([0x0f, 0x00, 0xff, 0x80]);
    let b = Digest::<4>::from_bytes([0x00, 0x00, 0x00, 0x00]);
    assert_eq!(hamming(&a, &b), 13);
    // symmetric on the same hand count
    assert_eq!(hamming(&b, &a), 13);
}

/// Widths around the eight-byte stride: one full chunk (N = 8, N = 9),
/// chunk plus tail (N = 12), tail only (N = 5, N = 1).
#[test]
fn hamming_spans_u64_chunks_and_tails() {
    // N = 8: exactly one u64 chunk, 8 * 8 = 64.
    assert_eq!(
        hamming(
            &Digest::<8>::from_bytes([0xff; 8]),
            &Digest::<8>::from_bytes([0x00; 8])
        ),
        64
    );
    // N = 9: one chunk plus a one-byte tail.
    assert_eq!(
        hamming(
            &Digest::from_bytes([0xff; 9]),
            &Digest::from_bytes([0x00; 9])
        ),
        72
    );
    // N = 12: a chunk plus a four-byte tail. 0xAB = 1010_1011 (5 bits),
    // 0xCD = 1100_1101 (5 bits), so the tail adds 5 + 0 + 0 + 5 = 10
    // and the whole digest is 64 + 10 = 74.
    let a = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // chunk
        0xab, 0x00, 0x00, 0xcd, // tail
    ];
    assert_eq!(hamming(&Digest::from_bytes(a), &Digest::from_bytes([0x00; 12])), 74);
    // N = 5: tail only. 0x0F (4) + 0x80 (1) + 0x03 (2) = 7.
    assert_eq!(
        hamming(
            &Digest::from_bytes([0x0f, 0x80, 0x03, 0x00, 0x00]),
            &Digest::from_bytes([0x00; 5])
        ),
        7
    );
    // N = 1: 0xAA ^ 0x55 = 0xFF, every bit differs.
    assert_eq!(
        hamming(&Digest::<1>::from_bytes([0xaa]), &Digest::<1>::from_bytes([0x55])),
        8
    );
}

// ----------------------------------------------------------------- hex

/// Leading zeroes (0x00), high nibbles (0xFF) and both nibble shapes of
/// 0x0A / 0xA5 in one known answer.
#[test]
fn to_hex_covers_zero_bytes_and_high_nibbles() {
    let d = Digest::<4>::from_bytes([0x00, 0xff, 0x0a, 0xa5]);
    assert_eq!(&d.to_hex()[..], b"00ff0aa5");
}

/// The output is exactly two characters per byte, with leading zeroes
/// kept at both ends of the alphabet.
#[test]
fn to_hex_is_exactly_two_chars_per_byte() {
    let hex = Digest::<3>::from_bytes([0x00, 0x10, 0xf0]).to_hex();
    assert_eq!(hex.len(), 6); // the type is [u8; 6]; assert anyway
    assert_eq!(&hex[..], b"0010f0");
    assert_eq!(&Digest::<1>::from_bytes([0x00]).to_hex()[..], b"00");
    assert_eq!(&Digest::<1>::from_bytes([0xff]).to_hex()[..], b"ff");
}

/// `from_hex` inverts `to_hex` on a known digest.
#[test]
fn from_hex_round_trips_to_hex() {
    let d = Digest::<4>::from_bytes([0xde, 0xad, 0xbe, 0xef]);
    let hex = d.to_hex();
    assert_eq!(&hex[..], b"deadbeef");
    assert_eq!(Digest::<4>::from_hex(&hex).unwrap(), d);
}

/// Uppercase and mixed-case hex parse to the same digest; `to_hex`
/// still emits lowercase.
#[test]
fn from_hex_accepts_uppercase() {
    assert_eq!(
        Digest::<4>::from_hex(b"DEADBEEF").unwrap(),
        Digest::<4>::from_bytes([0xde, 0xad, 0xbe, 0xef])
    );
    assert_eq!(
        Digest::<1>::from_hex(b"fF").unwrap(),
        Digest::<1>::from_bytes([0xff])
    );
}

/// Odd length is rejected with a named reason, distinct from a mere
/// length mismatch: an odd string is not a digest at any N.
#[test]
fn from_hex_rejects_odd_length() {
    assert_eq!(
        Digest::<4>::from_hex(b"deadbee"),
        Err(Error::BadValue("hex length is odd"))
    );
    assert_eq!(
        Digest::<1>::from_hex(b"0"),
        Err(Error::BadValue("hex length is odd"))
    );
}

/// Non-hex bytes are rejected wherever they sit.
#[test]
fn from_hex_rejects_non_hex_digit() {
    assert_eq!(
        Digest::<4>::from_hex(b"deadbefg"),
        Err(Error::BadValue("hex digit"))
    );
    assert_eq!(
        Digest::<2>::from_hex(b"0 0"),
        Err(Error::BadValue("hex digit"))
    );
    assert_eq!(
        Digest::<2>::from_hex(b"zz"),
        Err(Error::BadValue("hex digit"))
    );
}

/// Wrong length: too short is `Truncated` naming what was wanted; too
/// long is a `BadValue`, not a truncation.
#[test]
fn from_hex_rejects_wrong_length() {
    assert_eq!(
        Digest::<4>::from_hex(b"deadbe"),
        Err(Error::truncated("hex digest", 8, 6))
    );
    assert_eq!(
        Digest::<4>::from_hex(b"cafebabe12"),
        Err(Error::BadValue("hex digest length"))
    );
}

// -------------------------------------------------------------- Digest

/// A slice shorter than N is `Truncated`; longer is `BadValue`.
#[test]
fn from_slice_rejects_short_and_long() {
    assert_eq!(
        Digest::<4>::from_slice(&[1, 2, 3]),
        Err(Error::truncated("digest", 4, 3))
    );
    assert_eq!(
        Digest::<4>::from_slice(&[0, 0, 0, 0, 0]),
        Err(Error::BadValue("digest length"))
    );
}

#[test]
fn from_slice_accepts_exact_length() {
    assert_eq!(
        Digest::<4>::from_slice(&[4, 3, 2, 1]).unwrap(),
        Digest::<4>::from_bytes([4, 3, 2, 1])
    );
}

/// A `std::io::Read` supplying fewer than N bytes gives `Truncated`
/// with `found` set to what it did supply.
#[test]
fn from_stream_short_std_reader_is_truncated() {
    // three of four bytes
    let mut src = FromIo(Cursor::new(&[0x01, 0x02, 0x03][..]));
    assert_eq!(
        Digest::<4>::from_stream(&mut src),
        Err(Error::truncated("digest", 4, 3))
    );
    // nothing at all
    let mut empty = FromIo(Cursor::new(&b""[..]));
    assert_eq!(
        Digest::<4>::from_stream(&mut empty),
        Err(Error::truncated("digest", 4, 0))
    );
}

/// Partial reads are followed until the digest is full.
#[test]
fn from_stream_partial_reads_still_fill() {
    let mut dribble = Dribble(&[9, 8, 7, 6]);
    assert_eq!(
        Digest::<4>::from_stream(&mut dribble).unwrap(),
        Digest::<4>::from_bytes([9, 8, 7, 6])
    );
    // and a dribble that runs dry is still Truncated, not a zero fill
    let mut short = Dribble(&[1, 2]);
    assert_eq!(
        Digest::<4>::from_stream(&mut short),
        Err(Error::truncated("digest", 4, 2))
    );
}

/// Exactly N bytes are read; the rest of the source stays unread.
#[test]
fn from_stream_reads_exactly_n_and_leaves_the_rest() {
    let mut src: &[u8] = &[5, 6, 7, 8, 9, 10];
    assert_eq!(
        Digest::<4>::from_stream(&mut src).unwrap(),
        Digest::<4>::from_bytes([5, 6, 7, 8])
    );
    assert_eq!(src, &[9, 10]);
}

/// `Default` is all-zero — including N past 32, where the standard
/// library's `Default for [T; N]` stops (the impl is manual).
#[test]
fn digest_default_is_all_zero_even_past_array_default_limit() {
    assert_eq!(Digest::<4>::default().as_bytes(), &[0u8; 4]);
    assert_eq!(Digest::<64>::default().as_bytes(), &[0u8; 64]);
}

/// Ordering is bytewise lexicographic, so digests can be index keys.
#[test]
fn digest_orders_bytewise() {
    let low = Digest::<2>::from_bytes([0x00, 0xff]);
    let high = Digest::<2>::from_bytes([0x01, 0x00]);
    assert!(low < high); // first byte decides, like [u8; 2] itself
}

// --------------------------------------------------------------- names

/// Every member round-trips through its wire form, `Display` matches
/// the wire form, and `output_len` is pinned per member.
#[test]
fn algorithm_wire_round_trip_and_output_len() {
    let table = [
        (Algorithm::Sha1, "sha1", Some(20usize)),
        (Algorithm::Sha256, "sha256", Some(32)),
        (Algorithm::Sha512, "sha512", Some(64)),
        (Algorithm::Blake3, "blake3", Some(32)),
        (Algorithm::Md5, "md5", Some(16)),
        (Algorithm::XxHash64, "xxhash64", Some(8)),
        (Algorithm::FastCdc, "fastcdc", Some(4)),
        (Algorithm::Perceptual, "perceptual", None),
        (Algorithm::Unknown, "unknown", None),
    ];
    for (member, wire, len) in table {
        assert_eq!(member.as_str(), wire);
        assert_eq!(Algorithm::from_str(wire), member);
        assert_eq!(member.output_len(), len);
        assert_eq!(format!("{member}"), wire);
    }
}

/// Unrecognised strings yield `Unknown`, never an error; the wire form
/// is lowercase and exact.
#[test]
fn algorithm_unknown_strings_yield_unknown() {
    assert_eq!(Algorithm::from_str("sha3-256"), Algorithm::Unknown);
    assert_eq!(Algorithm::from_str(""), Algorithm::Unknown);
    assert_eq!(Algorithm::from_str("SHA256"), Algorithm::Unknown);
    assert_eq!(Algorithm::from_str("sha-256"), Algorithm::Unknown);
}

/// Every member round-trips through its wire form, `Display` included.
#[test]
fn format_wire_round_trip() {
    let table = [
        (Format::Png, "png"),
        (Format::Jpeg, "jpeg"),
        (Format::Bmp, "bmp"),
        (Format::Zip, "zip"),
        (Format::Mp4, "mp4"),
        (Format::Mp3, "mp3"),
        (Format::Wav, "wav"),
        (Format::Flac, "flac"),
        (Format::Pdf, "pdf"),
        (Format::Gif, "gif"),
        (Format::Unknown, "unknown"),
    ];
    for (member, wire) in table {
        assert_eq!(member.as_str(), wire);
        assert_eq!(Format::from_str(wire), member);
        assert_eq!(format!("{member}"), wire);
    }
}

#[test]
fn format_unknown_strings_yield_unknown() {
    assert_eq!(Format::from_str("webp"), Format::Unknown);
    assert_eq!(Format::from_str("PNG"), Format::Unknown);
}

// ------------------------------------------------------------- Display

/// `Display` and `to_hex` produce the same characters, byte for byte,
/// at several widths; formatter width is honoured through `pad`.
#[test]
fn digest_display_matches_to_hex() {
    let d1 = Digest::<1>::from_bytes([0x00]);
    let d4 = Digest::<4>::from_bytes([0xff, 0x0a, 0x00, 0xa5]);
    let d8 = Digest::<8>::from_bytes([0xde, 0xad, 0xbe, 0xef, 0x00, 0x11, 0x22, 0x33]);
    // known answers
    assert_eq!(format!("{d1}"), "00");
    assert_eq!(format!("{d4}"), "ff0a00a5");
    assert_eq!(format!("{d8}"), "deadbeef00112233");
    // and structurally, so the two paths cannot drift
    assert_eq!(format!("{d1}").as_bytes(), &d1.to_hex()[..]);
    assert_eq!(format!("{d4}").as_bytes(), &d4.to_hex()[..]);
    assert_eq!(format!("{d8}").as_bytes(), &d8.to_hex()[..]);
    // formatter width goes through the standard pad path
    assert_eq!(format!("{d1:>4}"), "  00");
}

/// `Display` for `Error` is the literal prefix plus the `what` field —
/// exact strings, one per variant.
#[test]
fn error_display_is_literal_concatenation() {
    assert_eq!(
        format!("{}", Error::truncated("idat chunk", 12, 5)),
        "truncated: idat chunk"
    );
    assert_eq!(
        format!("{}", Error::InvalidMagic { what: "PNG signature" }),
        "invalid magic: PNG signature"
    );
    assert_eq!(
        format!("{}", Error::BadValue("zero denominator")),
        "bad value: zero denominator"
    );
    assert_eq!(
        format!("{}", Error::Unsupported("16-bit grey with tRNS")),
        "unsupported: 16-bit grey with tRNS"
    );
    assert_eq!(
        format!("{}", Error::too_large("inflate output", 1 << 20)),
        "too large: inflate output"
    );
}

/// The convenience constructors build exactly the named variants, and
/// `Error` is `Copy`.
#[test]
fn error_constructors_build_the_named_variants() {
    assert_eq!(
        Error::truncated("zlib stream", 10, 4),
        Error::Truncated { what: "zlib stream", needed: 10, found: 4 }
    );
    assert_eq!(
        Error::too_large("chunk table", 65536),
        Error::TooLarge { what: "chunk table", limit: 65536 }
    );
    let e = Error::truncated("s", 1, 0);
    let copied = e;
    assert_eq!(e, copied);
}

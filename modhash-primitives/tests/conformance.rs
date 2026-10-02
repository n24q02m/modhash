//! Conformance vectors for the computational primitives. Every value
//! here is a published test vector (FIPS 180-2, RFC 1950, the PNG
//! spec, the FNV reference, the splitmix64 paper) or arithmetic that
//! pins a bit-order convention the codec crates build on.

use modhash_primitives::{BitReader, SplitMix64, adler32, crc32, fnv1a64, sha256};

fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[test]
fn sha256_fips_180_2_vectors() {
    // The three FIPS 180-2 appendix B vectors plus the empty string.
    let cases: [(&[u8], &str); 4] = [
        (
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        (
            &[b'a'; 1_000_000],
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
        ),
        (
            b"",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
    ];
    for (input, want) in cases {
        let got = hex(sha256(input).expect("within size limit").as_bytes());
        assert_eq!(got, want, "sha256({:?}...)", &input[..input.len().min(12)]);
    }
}

#[test]
fn sha256_digests_across_block_and_tail_boundaries() {
    // Inputs around the 55/56/64/65-byte padding boundaries: each exercises
    // a different tail-block path, and the one-million-'a' FIPS vector above
    // pins the multi-block path.
    let reference: Vec<(usize, String)> = (0..=66)
        .map(|n| (n, hex(sha256(&vec![b'x'; n]).unwrap().as_bytes())))
        .collect();
    // Self-consistency alone is vacuous, so pin three boundary lengths
    // against python hashlib values computed out of band.
    let pinned: [(usize, &str); 3] = [
        (
            55,
            "d5e285683cd4efc02d021a5c62014694958901005d6f71e89e0989fac77e4072",
        ),
        (
            56,
            "04c26261370ee7541549d16dee320c723e3fd14671e66a099afe0a377c16888e",
        ),
        (
            64,
            "7ce100971f64e7001e8fe5a51973ecdfe1ced42befe7ee8d5fd6219506b5393c",
        ),
    ];
    for (n, want) in pinned {
        let (_, got) = &reference[n];
        assert_eq!(got, want, "sha256 boundary pin at {n} bytes");
    }
}

#[test]
fn crc32_png_spec_vectors() {
    assert_eq!(crc32(b""), 0x0000_0000);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(
        crc32(b"The quick brown fox jumps over the lazy dog"),
        0x414F_A339
    );
}

#[test]
fn adler32_rfc1950_vectors() {
    assert_eq!(adler32(b""), 0x0000_0001);
    assert_eq!(adler32(b"a"), 0x0062_0062);
    assert_eq!(adler32(b"abc"), 0x024D_0127);
    assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    // Longer than NMAX (5552): the modulo-per-chunk path must agree with
    // the byte-at-a-time reference for the same input.
    let long = vec![0xABu8; 12_000];
    let mut slow_a: u32 = 1;
    let mut slow_b: u32 = 0;
    for &byte in &long {
        slow_a = (slow_a + u32::from(byte)) % 65_521;
        slow_b = (slow_b + slow_a) % 65_521;
    }
    assert_eq!(adler32(&long), (slow_b << 16) | slow_a);
}

#[test]
fn fnv1a64_reference_vectors() {
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn splitmix64_paper_sequence() {
    let mut rng = SplitMix64::new(0);
    assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
    assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
    // Same seed, same sequence: the property tests across the workspace
    // depend on this being reproducible on every machine.
    let mut a = SplitMix64::new(42);
    let mut b = SplitMix64::new(42);
    for _ in 0..16 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
    // fill_bytes chunks a stream of words little-endian and fills exactly.
    let mut bytes = [0u8; 10];
    SplitMix64::new(7).fill_bytes(&mut bytes);
    let mut words = SplitMix64::new(7);
    let w0 = words.next_u64().to_le_bytes();
    assert_eq!(&bytes[..8], &w0);
    let w1 = words.next_u64().to_le_bytes();
    assert_eq!(&bytes[8..], &w1[..2]);
}

#[test]
fn bitreader_msb_stream_fields() {
    // 0xA6 = 0b1010_0110: first 3 stream bits are 101, next 5 are 00110.
    let mut r = BitReader::new(&[0xA6, 0x50]);
    assert_eq!(r.bits(3).unwrap(), 0b101);
    assert_eq!(r.bits(5).unwrap(), 0b0_0110);
    assert!(r.is_aligned());
    // A field may span the byte boundary: remaining 0x50 high bits plus
    // the next byte's top bits reassemble MSB-first across bytes.
    let mut r = BitReader::new(&[0xA6, 0x50]);
    assert_eq!(r.bits(12).unwrap(), 0xA65);
    // MSB order over the whole first byte reads back the byte itself.
    let mut r = BitReader::new(&[0xA6, 0x50]);
    assert_eq!(r.bits(8).unwrap(), 0xA6);
}

#[test]
fn bitreader_truncation_is_error_not_panic() {
    let mut r = BitReader::new(&[0xFF]);
    assert!(r.bits(9).is_err());
    // Exhausted exactly: 8 bits then zero remain.
    assert_eq!(r.bits(8).unwrap(), 0xFF);
    assert_eq!(r.remaining_bits(), 0);
    assert!(r.bits(1).is_err());
}

#[test]
fn bitreader_byte_align_and_byte() {
    let mut r = BitReader::new(&[0x00, 0xAB]);
    assert_eq!(r.bits(3).unwrap(), 0);
    assert!(!r.is_aligned());
    r.byte_align();
    assert!(r.is_aligned());
    assert_eq!(r.byte().unwrap(), 0xAB);
    assert!(r.byte().is_err());
    // Unaligned byte() is refused, not rounded.
    let mut r = BitReader::new(&[0xFF, 0x00]);
    r.bits(1).unwrap();
    assert!(r.byte().is_err());
}

#[test]
fn bitreader_zero_width_read_is_zero_at_any_position() {
    // Zero-width reads are legal no-ops (pre-fast-path semantics) at
    // byte-aligned AND mid-byte positions; they must not shift the stream.
    let mut r = BitReader::new(&[0xA6, 0x50]);
    assert_eq!(r.bits(0).unwrap(), 0);
    assert_eq!(r.bits(3).unwrap(), 0b101);
    assert_eq!(r.bits(0).unwrap(), 0);
    assert_eq!(r.bits(5).unwrap(), 0b0_0110);
    // Position unchanged by the interleaved zero-width reads.
    let mut r = BitReader::new(&[0xA6, 0x50]);
    r.bits(0).unwrap();
    assert_eq!(r.bits(12).unwrap(), 0xA65);
    // Exhausted stream: zero-width reads stay legal no-ops.
    let mut r = BitReader::new(&[0xFF]);
    assert_eq!(r.bits(8).unwrap(), 0xFF);
    assert_eq!(r.remaining_bits(), 0);
    assert_eq!(r.bits(0).unwrap(), 0);
}

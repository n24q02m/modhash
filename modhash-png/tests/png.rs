//! Conformance and adversarial tests for `modhash-png`.
//!
//! Known-answer checks run against the byte fixtures in `tests/fixtures/`
//! (see `PROVENANCE.md` there: files were generated offline by
//! `gen_fixtures.py`, expected pixels are declared here and in that
//! generator, never derived from the decoder). The round-trip and
//! mutation suites are deterministic: every pseudo-random choice comes
//! from `SplitMix64` with fixed seeds.

use modhash_png::{Gray, Ihdr, Image, Limits, Pixels, decode, decode_ihdr, encode};
use modhash_primitives::{Error, SplitMix64, crc32};

const L: Limits = Limits {
    max_input: 64 * 1024 * 1024,
    max_output: 256 * 1024 * 1024,
};

// ---------------------------------------------------------------------
// Fixtures (byte-embedded; see fixtures/PROVENANCE.md)
// ---------------------------------------------------------------------

const GRAY8: &[u8] = include_bytes!("fixtures/gray8_4x4.png");
const GRAY1: &[u8] = include_bytes!("fixtures/gray1_8x2.png");
const GRAY4: &[u8] = include_bytes!("fixtures/gray4_5x3.png");
const GRAY16: &[u8] = include_bytes!("fixtures/gray16_3x2.png");
const RGB8_FILTERS: &[u8] = include_bytes!("fixtures/rgb8_filters_5x5.png");
const RGB16: &[u8] = include_bytes!("fixtures/rgb16_3x2.png");
const GA8: &[u8] = include_bytes!("fixtures/ga8_4x4.png");
const GA16: &[u8] = include_bytes!("fixtures/ga16_2x2.png");
const RGBA8: &[u8] = include_bytes!("fixtures/rgba8_4x4.png");
const RGBA16: &[u8] = include_bytes!("fixtures/rgba16_2x2.png");
const PAL8_TRNS: &[u8] = include_bytes!("fixtures/pal8_trns_4x4.png");
const PAL4: &[u8] = include_bytes!("fixtures/pal4_9x2.png");
const GRAY8_TRNS: &[u8] = include_bytes!("fixtures/gray8_trns_3x2.png");
const RGB8_TRNS: &[u8] = include_bytes!("fixtures/rgb8_trns_3x2.png");
const PAETH_TIE: &[u8] = include_bytes!("fixtures/paeth_tie_2x2.png");
const ADAM7_13X11: &[u8] = include_bytes!("fixtures/adam7_13x11_rgb.png");
const ADAM7_5X5: &[u8] = include_bytes!("fixtures/adam7_5x5_gray8.png");
const ADAM7_2X2: &[u8] = include_bytes!("fixtures/adam7_2x2_gray8.png");

// ---------------------------------------------------------------------
// Test-local chunk helpers (mirrors the kit Error vocabulary)
// ---------------------------------------------------------------------

fn chunk(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(ty);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

fn ihdr_chunk(w: u32, h: u32, depth: u8, colour: u8, interlace: u8) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&w.to_be_bytes());
    d.extend_from_slice(&h.to_be_bytes());
    d.extend_from_slice(&[depth, colour, 0, 0, interlace]);
    chunk(b"IHDR", &d)
}

/// Wraps already-framed chunks into a full PNG stream.
fn stream(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

/// Rewrites the stored CRC-32 of the chunk containing byte `pos` so a
/// mutation inside a chunk's data exercises real parser paths instead of
/// dying at the integrity check. Returns false when `pos` sits outside
/// any chunk (signature).
fn repair_crc(buf: &mut [u8], pos: usize) -> bool {
    if pos < 8 {
        return false;
    }
    let mut at = 8usize;
    while at + 12 <= buf.len() {
        let len = u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]) as usize;
        let end = at + 12 + len;
        if end > buf.len() {
            return false;
        }
        if pos >= at && pos < end {
            let mut crc_input = Vec::with_capacity(4 + len);
            crc_input.extend_from_slice(&buf[at + 4..at + 8]);
            crc_input.extend_from_slice(&buf[at + 8..at + 8 + len]);
            let crc = crc32(&crc_input).to_be_bytes();
            buf[end - 4..end].copy_from_slice(&crc);
            return true;
        }
        at = end;
    }
    false
}

fn gray8_fixture_pixels() -> [u8; 16] {
    [
        7, 15, 31, 63, //
        0, 128, 192, 255, //
        3, 1, 4, 1, //
        59, 26, 53, 58,
    ]
}

// ---------------------------------------------------------------------
// Header parsing
// ---------------------------------------------------------------------

#[test]
fn ihdr_parses_from_fixtures() {
    let h = decode_ihdr(GRAY8).unwrap();
    assert_eq!(
        h,
        Ihdr {
            width: 4,
            height: 4,
            bit_depth: 8,
            colour_type: 0,
            interlaced: false
        }
    );
    let a = decode_ihdr(ADAM7_13X11).unwrap();
    assert_eq!(
        (a.width, a.height, a.colour_type, a.interlaced),
        (13, 11, 2, true)
    );
    assert_eq!(
        decode_ihdr(&GRAY8[..7]),
        Err(Error::InvalidMagic {
            what: "PNG signature"
        })
    );
}

// ---------------------------------------------------------------------
// Fixture pixel conformance
// ---------------------------------------------------------------------

#[test]
fn gray8_fixture_decodes_known_pixels() {
    let p = decode(GRAY8, &L).unwrap();
    assert_eq!(
        (p.width(), p.height(), p.colour_type(), p.bit_depth()),
        (4, 4, 0, 8)
    );
    match p.pixels() {
        Pixels::Gray8(img) => assert_eq!(img.as_slice(), &gray8_fixture_pixels()),
        other => panic!("expected Gray8, got {other:?}"),
    }
}

#[test]
fn gray1_scales_by_bit_replication() {
    let p = decode(GRAY1, &L).unwrap();
    match p.pixels() {
        Pixels::Gray8(img) => assert_eq!(
            img.as_slice(),
            &[
                0, 255, 0, 255, 255, 0, 0, 255, //
                255, 255, 0, 0, 0, 255, 255, 0
            ]
        ),
        other => panic!("expected Gray8, got {other:?}"),
    }
}

#[test]
fn gray4_scales_nibbles() {
    let p = decode(GRAY4, &L).unwrap();
    match p.pixels() {
        Pixels::Gray8(img) => assert_eq!(
            img.as_slice(),
            &[
                0, 85, 170, 255, 17, //
                238, 153, 68, 136, 34, //
                119, 119, 119, 0, 255
            ]
        ),
        other => panic!("expected Gray8, got {other:?}"),
    }
}

#[test]
fn gray16_big_endian() {
    let p = decode(GRAY16, &L).unwrap();
    match p.pixels() {
        Pixels::Gray16(img) => assert_eq!(
            img.as_slice(),
            &[0x0000, 0x1234, 0xBEEF, 0xFFFF, 0x00FF, 0x8000]
        ),
        other => panic!("expected Gray16, got {other:?}"),
    }
}

#[test]
fn rgb8_fixture_uses_all_five_filters() {
    // Row r of this fixture is coded with filter r (0-4), so one decode
    // exercises every reconstruction path.
    let p = decode(RGB8_FILTERS, &L).unwrap();
    match p.pixels() {
        Pixels::Rgb8(img) => {
            for y in 0..5usize {
                for x in 0..5usize {
                    let px = &img.as_slice()[(y * 5 + x) * 3..][..3];
                    assert_eq!(
                        px,
                        &[x as u8 * 51, y as u8 * 51, (x + y) as u8 * 25],
                        "pixel ({x},{y})"
                    );
                }
            }
        }
        other => panic!("expected Rgb8, got {other:?}"),
    }
}

#[test]
fn rgb16_fixture() {
    let p = decode(RGB16, &L).unwrap();
    match p.pixels() {
        Pixels::Rgb16(img) => assert_eq!(
            img.as_slice(),
            &[
                0x0001, 0xABCD, 0xFFFF, 0x1234, 0x5678, 0x9ABC, 0x8000, 0x0000, 0x7FFF, //
                0x1111, 0x2222, 0x3333, 0xF0F0, 0x0F0F, 0x55AA, 0xDEAD, 0xBEEF, 0xCAFE
            ]
        ),
        other => panic!("expected Rgb16, got {other:?}"),
    }
}

#[test]
fn ga8_expands_to_rgba() {
    let p = decode(GA8, &L).unwrap();
    match p.pixels() {
        Pixels::Rgba8(img) => {
            let vals = [
                0u8, 64, 128, 192, 16, 80, 144, 208, 32, 96, 160, 224, 48, 112, 176, 240,
            ];
            for (i, &g) in vals.iter().enumerate() {
                let px = &img.as_slice()[i * 4..][..4];
                assert_eq!(px, &[g, g, g, 255 - g], "pixel {i}");
            }
        }
        other => panic!("expected Rgba8, got {other:?}"),
    }
}

#[test]
fn ga16_expands_to_rgba16() {
    let p = decode(GA16, &L).unwrap();
    match p.pixels() {
        Pixels::Rgba16(img) => assert_eq!(
            img.as_slice(),
            &[
                0x0000, 0x0000, 0x0000, 0xFFFF, //
                0x1234, 0x1234, 0x1234, 0x5678, //
                0x9ABC, 0x9ABC, 0x9ABC, 0xDEF0, //
                0x8000, 0x8000, 0x8000, 0x0001
            ]
        ),
        other => panic!("expected Rgba16, got {other:?}"),
    }
}

#[test]
fn rgba8_and_rgba16_fixtures() {
    let p = decode(RGBA8, &L).unwrap();
    match p.pixels() {
        Pixels::Rgba8(img) => {
            for y in 0..4usize {
                for x in 0..4usize {
                    let px = &img.as_slice()[(y * 4 + x) * 4..][..4];
                    assert_eq!(
                        px,
                        &[
                            x as u8 * 64,
                            y as u8 * 64,
                            (x + y) as u8 * 32,
                            255 - x as u8 * 16
                        ],
                        "pixel ({x},{y})"
                    );
                }
            }
        }
        other => panic!("expected Rgba8, got {other:?}"),
    }
    let p16 = decode(RGBA16, &L).unwrap();
    match p16.pixels() {
        Pixels::Rgba16(img) => assert_eq!(
            img.as_slice(),
            &[
                0x0000, 0x1111, 0x2222, 0xFFFF, 0x3333, 0x4444, 0x5555, 0x6666, //
                0x7777, 0x8888, 0x9999, 0xAAAA, 0xBBBB, 0xCCCC, 0xDDDD, 0xEEEE
            ]
        ),
        other => panic!("expected Rgba16, got {other:?}"),
    }
}

#[test]
fn palette_decodes_with_trns_alpha() {
    let p = decode(PAL8_TRNS, &L).unwrap();
    let pal = [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [0, 255, 255],
        [255, 0, 255],
        [17, 34, 51],
        [99, 88, 77],
    ];
    let alpha = [255u8, 128, 0];
    match p.pixels() {
        Pixels::Rgba8(img) => {
            for y in 0..4usize {
                for x in 0..4usize {
                    let idx = (x + y * 2) % 8; // generator's index matrix
                    let px = &img.as_slice()[(y * 4 + x) * 4..][..4];
                    let a = alpha.get(idx).copied().unwrap_or(255);
                    let e = pal[idx];
                    assert_eq!(px, &[e[0], e[1], e[2], a], "pixel ({x},{y}) idx {idx}");
                }
            }
        }
        other => panic!("expected Rgba8, got {other:?}"),
    }
}

#[test]
fn palette_4bit_decodes_to_rgb() {
    let p = decode(PAL4, &L).unwrap();
    let pal: Vec<[u8; 3]> = (0..13)
        .map(|i| {
            [
                ((i * 19) & 0xFF) as u8,
                ((i * 37) & 0xFF) as u8,
                ((i * 53) & 0xFF) as u8,
            ]
        })
        .collect();
    match p.pixels() {
        Pixels::Rgb8(img) => {
            for (row, row_idx) in [(0usize, 0usize), (1, 1)].iter() {
                for x in 0..9usize {
                    let idx = if *row_idx == 0 { x % 13 } else { (x * 3) % 13 };
                    let px = &img.as_slice()[(*row * 9 + x) * 3..][..3];
                    assert_eq!(px, &pal[idx], "pixel ({x},{row}) idx {idx}");
                }
            }
        }
        other => panic!("expected Rgb8, got {other:?}"),
    }
}

#[test]
fn trns_gray_and_rgb_expand_alpha() {
    let g = decode(GRAY8_TRNS, &L).unwrap();
    match g.pixels() {
        Pixels::Rgba8(img) => {
            let px: Vec<[u8; 4]> = [10u8, 20, 10, 20, 10, 30]
                .iter()
                .map(|&v| [v, v, v, if v == 10 { 0 } else { 255 }])
                .collect();
            for (i, e) in px.iter().enumerate() {
                assert_eq!(&img.as_slice()[i * 4..][..4], e, "pixel {i}");
            }
        }
        other => panic!("expected Rgba8, got {other:?}"),
    }
    let r = decode(RGB8_TRNS, &L).unwrap();
    match r.pixels() {
        Pixels::Rgba8(img) => {
            let cols = [
                [1u8, 2, 3],
                [9, 9, 9],
                [4, 5, 6],
                [9, 9, 9],
                [1, 2, 3],
                [7, 8, 9],
            ];
            for (i, e) in cols.iter().enumerate() {
                let a = if *e == [9, 9, 9] { 0 } else { 255 };
                assert_eq!(
                    &img.as_slice()[i * 4..][..4],
                    &[e[0], e[1], e[2], a],
                    "pixel {i}"
                );
            }
        }
        other => panic!("expected Rgba8, got {other:?}"),
    }
}

#[test]
fn paeth_tie_breaks_to_up() {
    // Row1x1 of the fixture: a=50 b=20 c=40 -> p=30, pb=pc=10 < pa=20.
    // The spec's tie order takes b, so the coded residual 13 decodes to
    // 13+20=33; the two wrong tie orders would give 13+40=53 or 13+50=63.
    let p = decode(PAETH_TIE, &L).unwrap();
    match p.pixels() {
        Pixels::Gray8(img) => assert_eq!(img.as_slice(), &[40, 20, 50, 33]),
        other => panic!("expected Gray8, got {other:?}"),
    }
}

#[test]
fn adam7_full_and_empty_passes() {
    let p = decode(ADAM7_13X11, &L).unwrap();
    assert!(p.interlaced());
    match p.pixels() {
        Pixels::Rgb8(img) => {
            for y in 0..11usize {
                for x in 0..13usize {
                    let px = &img.as_slice()[(y * 13 + x) * 3..][..3];
                    let e = [
                        (x * 17 + y) as u8,
                        ((x * 29 + y * 13) & 0xFF) as u8,
                        (((x ^ y) * 31) & 0xFF) as u8,
                    ];
                    assert_eq!(px, &e, "pixel ({x},{y})");
                }
            }
        }
        other => panic!("expected Rgb8, got {other:?}"),
    }
    // 5x5 populates every Adam7 pass at least partially.
    let g = decode(ADAM7_5X5, &L).unwrap();
    match g.pixels() {
        Pixels::Gray8(img) => {
            for y in 0..5usize {
                for x in 0..5usize {
                    assert_eq!(
                        img.as_slice()[y * 5 + x],
                        ((x * 51 + y * 37) & 0xFF) as u8,
                        "pixel ({x},{y})"
                    );
                }
            }
        }
        other => panic!("expected Gray8, got {other:?}"),
    }
    // 2x2 leaves passes 1, 2, 3 and 4 completely empty: only passes 0,
    // 5 and 6 carry rows. A decoder that reads phantom scanlines for the
    // empty passes either mis-positions these pixels or desyncs the
    // stream.
    let t = decode(ADAM7_2X2, &L).unwrap();
    match t.pixels() {
        Pixels::Gray8(img) => assert_eq!(img.as_slice(), &[17, 91, 222, 3]),
        other => panic!("expected Gray8, got {other:?}"),
    }
}

#[test]
fn adam7_matches_noninterlaced_bytes() {
    // The 13x11 fixture decoded must equal the same pixels encoded
    // non-interlaced through the scaffolding encoder.
    let a = decode(ADAM7_13X11, &L).unwrap().into_pixels();
    let Pixels::Rgb8(img) = &a else {
        unreachable!()
    };
    let round = encode(&a, 0).unwrap();
    let b = decode(&round, &L).unwrap();
    assert_eq!(&a, b.pixels());
    let _ = img;
}

#[test]
fn decode_is_deterministic() {
    for f in [GRAY8, ADAM7_13X11, PAL8_TRNS] {
        assert_eq!(decode(f, &L).unwrap(), decode(f, &L).unwrap());
    }
}

// ---------------------------------------------------------------------
// Filters cross-check: encode the same pixels with each filter
// ---------------------------------------------------------------------

#[test]
fn every_filter_round_trips_same_pixels() {
    let mut rng = SplitMix64::new(0xF17E);
    for f in 0..5u8 {
        let img = random_image(&mut rng, f as usize);
        let bytes = encode(&img, f).unwrap();
        let back = decode(&bytes, &L).unwrap();
        assert_eq!(&img, back.pixels(), "filter {f}");
    }
}

// ---------------------------------------------------------------------
// Inversion property: pixel-inverted image round-trips and equals the
// computed complement (the "hash bits" it flips are the sample bits).
// ---------------------------------------------------------------------

#[test]
fn inverted_fixture_is_exact_complement() {
    let p = decode(GRAY8, &L).unwrap().into_pixels();
    let Pixels::Gray8(img) = p else {
        unreachable!()
    };
    let inv: Vec<u8> = img.as_slice().iter().map(|v| !v).collect();
    let inv_img = Image::<Gray, u8>::from_vec(4, 4, inv.clone()).unwrap();
    let bytes = encode(&Pixels::Gray8(inv_img), 0).unwrap();
    let back = decode(&bytes, &L).unwrap();
    match back.pixels() {
        Pixels::Gray8(i) => {
            assert_eq!(i.as_slice(), &inv[..]);
            // Every sample bit is flipped: no surviving bit in common.
            for (a, b) in img.as_slice().iter().zip(i.as_slice()) {
                assert_eq!(a & b, 0, "surviving bits after inversion");
                assert_eq!(a | b, 255);
            }
        }
        other => panic!("expected Gray8, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Round-trip: 200 seed-fixed images
// ---------------------------------------------------------------------

fn random_image(rng: &mut SplitMix64, kind: usize) -> Pixels {
    let w = 1 + (rng.next_u64() % 25) as u32;
    let h = 1 + (rng.next_u64() % 25) as u32;
    let n = (w as usize) * (h as usize);
    let mut bytes = vec![0u8; n * 8]; // enough for 4 channels of u16
    rng.fill_bytes(&mut bytes);
    match kind % 6 {
        0 => {
            let v = (0..n).map(|i| bytes[i]).collect();
            Pixels::Gray8(Image::from_vec(w, h, v).unwrap())
        }
        1 => {
            let v = (0..n)
                .map(|i| u16::from_be_bytes([bytes[2 * i], bytes[2 * i + 1]]))
                .collect();
            Pixels::Gray16(Image::from_vec(w, h, v).unwrap())
        }
        2 => {
            let v = (0..n * 3).map(|i| bytes[i]).collect();
            Pixels::Rgb8(Image::from_vec(w, h, v).unwrap())
        }
        3 => {
            let v = (0..n * 3)
                .map(|i| u16::from_be_bytes([bytes[2 * i], bytes[2 * i + 1]]))
                .collect();
            Pixels::Rgb16(Image::from_vec(w, h, v).unwrap())
        }
        4 => {
            let v = (0..n * 4).map(|i| bytes[i]).collect();
            Pixels::Rgba8(Image::from_vec(w, h, v).unwrap())
        }
        _ => {
            let v = (0..n * 4)
                .map(|i| u16::from_be_bytes([bytes[2 * i], bytes[2 * i + 1]]))
                .collect();
            Pixels::Rgba16(Image::from_vec(w, h, v).unwrap())
        }
    }
}

#[test]
fn round_trip_200_seeded_images() {
    let mut rng = SplitMix64::new(0x5EED_0002);
    for i in 0..200 {
        let kind = (rng.next_u64() % 6) as usize;
        let filter = (rng.next_u64() % 5) as u8;
        let img = random_image(&mut rng, kind);
        let bytes = encode(&img, filter).unwrap();
        let back = decode(&bytes, &L).unwrap();
        assert_eq!(
            &img,
            back.pixels(),
            "iteration {i} kind {kind} filter {filter}"
        );
    }
}

// ---------------------------------------------------------------------
// Corruption: strict prefixes always fail; single-byte mutations never
// panic.
// ---------------------------------------------------------------------

#[test]
fn every_strict_prefix_errors() {
    for f in [GRAY8, ADAM7_13X11, PAL8_TRNS] {
        for i in 0..f.len() {
            assert!(decode(&f[..i], &L).is_err(), "{i}-byte prefix decoded");
        }
    }
}

#[test]
fn ten_thousand_byte_mutations_never_panic() {
    let mut rng = SplitMix64::new(0xBADB_17E5);
    for i in 0..10_000u64 {
        let mut m = ADAM7_13X11.to_vec();
        let pos = (rng.next_u64() as usize) % m.len();
        m[pos] ^= (rng.next_u64() as u8) | 1;
        // Half the mutations get their containing chunk's CRC repaired so
        // the corrupted payload reaches the parsers, not just the CRC
        // check.
        if i % 2 == 0 {
            repair_crc(&mut m, pos);
        }
        let _ = decode(&m, &L);
    }
}

// ---------------------------------------------------------------------
// Named error paths
// ---------------------------------------------------------------------

#[test]
fn bad_signature_and_truncations() {
    let mut bad_sig = GRAY8.to_vec();
    bad_sig[0] = b'X';
    assert_eq!(
        decode(&bad_sig, &L),
        Err(Error::InvalidMagic {
            what: "PNG signature"
        })
    );
    // Chunk header cut short.
    let cut = &GRAY8[..GRAY8.len() - 3];
    assert!(decode(cut, &L).is_err());
}

#[test]
fn corrupt_chunk_crc_is_named() {
    let mut m = GRAY8.to_vec();
    // Walk chunks until the IDAT, then flip its first data byte.
    let mut at = 8usize;
    let mut flipped = false;
    while at + 12 <= m.len() {
        let len = u32::from_be_bytes([m[at], m[at + 1], m[at + 2], m[at + 3]]) as usize;
        if &m[at + 4..at + 8] == b"IDAT" {
            m[at + 8] ^= 0xFF;
            flipped = true;
            break;
        }
        at += 12 + len;
    }
    assert!(flipped, "fixture must carry an IDAT");
    assert_eq!(
        decode(&m, &L),
        Err(Error::BadValue("PNG IDAT CRC32 mismatch"))
    );
}

#[test]
fn unknown_critical_and_ancillary_chunks() {
    let ihdr = ihdr_chunk(1, 1, 8, 0, 0);
    // A 1x1 gray8 image needs 2 bytes of scanlines; the encoder
    // supplies a valid IDAT to splice foreign chunks around.
    let real = encode(&Pixels::Gray8(Image::from_vec(1, 1, vec![9]).unwrap()), 0).unwrap();
    // Build unknown CRITICAL: all four type bytes uppercase.
    let crit = chunk(b"XAUX", &[0]);
    let iend = chunk(b"IEND", &[]);
    // Reuse the real file's IDAT.
    let idat = {
        let mut at = 8;
        let mut found = Vec::new();
        while at + 12 <= real.len() {
            let len =
                u32::from_be_bytes([real[at], real[at + 1], real[at + 2], real[at + 3]]) as usize;
            if &real[at + 4..at + 8] == b"IDAT" {
                found = real[at..at + 12 + len].to_vec();
                break;
            }
            at += 12 + len;
        }
        found
    };
    let with_crit = stream(&[ihdr.clone(), crit, idat.clone(), iend.clone()]);
    assert_eq!(
        decode(&with_crit, &L),
        Err(Error::Unsupported("PNG unknown critical chunk"))
    );
    // Ancillary unknown chunk is skipped: replace with a lowercase-first
    // type — a valid file must still decode.
    let anc = chunk(b"zzZZ", &[1, 2, 3]);
    let ok = stream(&[ihdr, anc, idat, iend]);
    assert!(decode(&ok, &L).is_ok());
}

#[test]
fn reserved_bit_lowercase_type_rejected() {
    // Third type byte lowercase violates spec 5.4's reserved bit.
    let ihdr = ihdr_chunk(1, 1, 8, 0, 0);
    let bad = chunk(b"ABcD", &[0]);
    let iend = chunk(b"IEND", &[]);
    assert_eq!(
        decode(&stream(&[ihdr, bad, iend]), &L),
        Err(Error::BadValue("PNG chunk type"))
    );
}

#[test]
fn ordering_violations_are_named() {
    let ihdr = ihdr_chunk(1, 1, 8, 0, 0);
    let iend = chunk(b"IEND", &[]);
    // No IDAT.
    assert_eq!(
        decode(&stream(&[ihdr.clone(), iend.clone()]), &L),
        Err(Error::BadValue("PNG missing IDAT"))
    );
    // Missing IEND: file just stops after IDAT.
    let real = encode(&Pixels::Gray8(Image::from_vec(1, 1, vec![3]).unwrap()), 0).unwrap();
    let mut no_iend = real.clone();
    no_iend.truncate(real.len() - 12); // drop IEND chunk
    assert_eq!(
        decode(&no_iend, &L),
        Err(Error::BadValue("PNG missing IEND"))
    );
    // Trailing garbage after IEND.
    let mut trailing = real.clone();
    trailing.extend_from_slice(&[0xDE, 0xAD]);
    assert_eq!(
        decode(&trailing, &L),
        Err(Error::BadValue("PNG trailing bytes after IEND"))
    );
    // Hand-craft sig + IHDR + IDAT + tEXt + IDAT + IEND: legal PNG
    // requires consecutive IDATs.
    let idat = {
        let mut at = 8usize;
        let mut found = Vec::new();
        while at + 12 <= real.len() {
            let len =
                u32::from_be_bytes([real[at], real[at + 1], real[at + 2], real[at + 3]]) as usize;
            if &real[at + 4..at + 8] == b"IDAT" {
                found = real[at..at + 12 + len].to_vec();
                break;
            }
            at += 12 + len;
        }
        found
    };
    let stream = stream(&[
        ihdr_chunk(1, 1, 8, 0, 0),
        idat.clone(),
        chunk(b"tEXt", b"a\x00b"),
        idat,
        iend,
    ]);
    assert_eq!(
        decode(&stream, &L),
        Err(Error::BadValue("PNG non-consecutive IDAT"))
    );
}

#[test]
fn ihdr_field_validation() {
    let mk = |depth: u8, colour: u8, interlace: u8| {
        let mut d = Vec::new();
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&1u32.to_be_bytes());
        d.extend_from_slice(&[depth, colour, 0, 0, interlace]);
        chunk(b"IHDR", &d)
    };
    let iend = chunk(b"IEND", &[]);
    for (name, ih) in [
        ("bad colour type", mk(8, 5, 0)),
        ("bad depth for RGB", mk(4, 2, 0)),
        ("bad interlace", mk(8, 0, 2)),
        ("zero width", {
            let mut d = Vec::new();
            d.extend_from_slice(&0u32.to_be_bytes());
            d.extend_from_slice(&1u32.to_be_bytes());
            d.extend_from_slice(&[8, 0, 0, 0, 0]);
            chunk(b"IHDR", &d)
        }),
    ] {
        assert!(
            matches!(
                decode(&stream(&[ih, iend.clone()]), &L),
                Err(Error::BadValue(_))
            ),
            "{name}"
        );
    }
}

#[test]
fn palette_and_trns_structural_errors() {
    // Palette index out of range: ct3 d1 with a 1-entry palette but index 1 set.
    // Build: IHDR(4x1, d1, ct3), PLTE 1 entry, IDAT for row "filter0 + 0b1010...".
    let ihdr = ihdr_chunk(4, 1, 1, 3, 0);
    let plte = chunk(b"PLTE", &[9, 8, 7]);
    let iend = chunk(b"IEND", &[]);
    // Row bits: indices 0,1,0,1 -> palette has only entry 0 -> error.
    let raw = [0x00u8, 0b0101_0000u8];
    let idat = chunk(b"IDAT", &test_zlib(&raw));
    assert_eq!(
        decode(&stream(&[ihdr, plte, idat, iend.clone()]), &L),
        Err(Error::BadValue("PNG palette index out of range"))
    );

    // Missing PLTE for indexed image.
    let ihdr = ihdr_chunk(1, 1, 8, 3, 0);
    let idat = chunk(b"IDAT", &test_zlib(&[0, 0]));
    assert_eq!(
        decode(&stream(&[ihdr, idat, iend.clone()]), &L),
        Err(Error::BadValue("PNG indexed image missing PLTE"))
    );

    // tRNS on colour type 6 is forbidden.
    let ihdr = ihdr_chunk(1, 1, 8, 6, 0);
    let trns = chunk(b"tRNS", &[0, 0, 0, 0, 0, 0]);
    let idat = chunk(b"IDAT", &test_zlib(&[0, 1, 2, 3, 4]));
    assert_eq!(
        decode(&stream(&[ihdr, trns, idat, iend.clone()]), &L),
        Err(Error::BadValue("PNG tRNS in alpha image"))
    );

    // PLTE after IDAT: a truecolour file may legally carry PLTE (a
    // suggested palette) but never past the image data.
    let ihdr = ihdr_chunk(1, 1, 8, 2, 0);
    let idat = chunk(b"IDAT", &test_zlib(&[0, 0, 0, 0]));
    let plte = chunk(b"PLTE", &[1, 2, 3]);
    assert_eq!(
        decode(&stream(&[ihdr, idat, plte, iend]), &L),
        Err(Error::BadValue("PNG PLTE after IDAT"))
    );
}

#[test]
fn filter_and_length_errors() {
    // Filter byte 5 in a 1x1 gray8.
    let ihdr = ihdr_chunk(1, 1, 8, 0, 0);
    let idat = chunk(b"IDAT", &test_zlib(&[5, 42]));
    let iend = chunk(b"IEND", &[]);
    assert_eq!(
        decode(&stream(&[ihdr, idat, iend.clone()]), &L),
        Err(Error::BadValue("PNG filter type"))
    );
    // Too few scanline bytes.
    let ihdr = ihdr_chunk(2, 2, 8, 0, 0);
    let idat = chunk(b"IDAT", &test_zlib(&[0, 1, 2]));
    assert_eq!(
        decode(&stream(&[ihdr, idat, iend.clone()]), &L),
        Err(Error::BadValue("PNG decompressed length"))
    );
    // Garbage zlib.
    let ihdr = ihdr_chunk(1, 1, 8, 0, 0);
    let idat = chunk(b"IDAT", &[0xDE, 0xAD, 0xBE, 0xEF]);
    assert!(matches!(
        decode(&stream(&[ihdr, idat, iend]), &L),
        Err(Error::InvalidMagic { .. })
    ));
}

#[test]
fn encode_rejects_bad_filter() {
    let img = Pixels::Gray8(Image::from_vec(1, 1, vec![0]).unwrap());
    assert_eq!(encode(&img, 5), Err(Error::BadValue("PNG filter type")));
}

// ---------------------------------------------------------------------
// Oversize refusal: a 2^31-1-wide header never allocates.
// ---------------------------------------------------------------------

#[test]
fn absurd_dimensions_refused_before_inflate() {
    let ihdr = ihdr_chunk(0x7FFF_FFFF, 0x7FFF_FFFF, 8, 0, 0);
    let idat = chunk(b"IDAT", &test_zlib(&[0]));
    let iend = chunk(b"IEND", &[]);
    match decode(&stream(&[ihdr, idat, iend]), &L) {
        Err(Error::TooLarge { .. }) => {}
        Err(Error::BadValue(_)) => {}
        other => panic!("expected a size refusal, got {other:?}"),
    }
}

/// A stored-block zlib stream for hand-built test rows, mirroring the
/// encoder's wrapper but local so tests do not trust the code under test
/// to build its own adversarial inputs.
fn test_zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut at = 0usize;
    loop {
        let take = (data.len() - at).min(65_535);
        let last = at + take == data.len();
        out.push(if last { 1 } else { 0 });
        out.extend_from_slice(&(take as u16).to_le_bytes());
        out.extend_from_slice(&(!(take as u16)).to_le_bytes());
        out.extend_from_slice(&data[at..at + take]);
        at += take;
        if last {
            break;
        }
    }
    out.extend_from_slice(&test_adler32(data).to_be_bytes());
    out
}

/// Adler-32 recomputed in the test, not imported from the kit: a checker
/// that shares its arithmetic with the code under test checks nothing.
fn test_adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

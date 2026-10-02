//! Conformance for `modhash-wav`: every fixture below is built byte-by-byte
//! in this file, so the expected values are the bytes themselves, not an
//! encoder's opinion of them. No test round-trips through this crate.

use modhash_primitives::Error;
use modhash_wav::{SampleFormat, decode};

// ---- byte-level fixture builders -------------------------------------------

/// One complete RIFF chunk: id, u32-LE size, payload, and the pad byte that
/// RIFF requires after every odd-sized payload. The pad is part of the file
/// format, not the chunk, so `chunk` writes it unconditionally.
fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len() + 1);
    v.extend_from_slice(id);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        v.push(0);
    }
    v
}

/// A 16-byte `fmt ` chunk (the WAVEFORMAT prefix this decoder reads).
fn fmt_chunk(tag: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let block_align = channels * (bits / 8);
    let byte_rate = rate * u32::from(block_align);
    let mut v = Vec::with_capacity(16);
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&block_align.to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v
}

/// A whole file: RIFF header (size filled from the real length), then the
/// given chunks in order.
fn wav_file(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body_len: usize = 4 + chunks.iter().map(Vec::len).sum::<usize>();
    let mut v = Vec::with_capacity(8 + body_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(body_len as u32).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    for c in chunks {
        v.extend_from_slice(c);
    }
    v
}

/// A minimal well-formed file: one `fmt ` chunk then one `data` chunk.
fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
    wav_file(&[
        chunk(b"fmt ", &fmt_chunk(tag, channels, rate, bits)),
        chunk(b"data", data),
    ])
}

// ---- canonical PCM, byte-exact ---------------------------------------------

#[test]
fn pcm8_decodes_unsigned_offset_by_128() {
    // Mono 8-bit: the file stores unsigned bytes; canonical form centres
    // them. 0x00 -> -128, 0x80 -> 0, 0xFF -> 127.
    let w = decode(&wav(1, 1, 8_000, 8, &[0x00, 0x80, 0xFF, 0x01])).unwrap();
    assert_eq!(w.format(), SampleFormat::Pcm);
    assert_eq!(w.channels(), 1);
    assert_eq!(w.sample_rate(), 8_000);
    assert_eq!(w.bits_per_sample(), 8);
    assert_eq!(w.samples(), &[-128, 0, 127, -127]);
    assert_eq!(w.frames(), 4);
}

#[test]
fn pcm16_decodes_little_endian_signed() {
    // Stereo 16-bit: interleaved L,R per frame.
    let mut data = Vec::new();
    for s in [0i16, -32768, 32767, -1, 1000, -1000] {
        data.extend_from_slice(&s.to_le_bytes());
    }
    let w = decode(&wav(1, 2, 44_100, 16, &data)).unwrap();
    assert_eq!(w.channels(), 2);
    assert_eq!(w.frames(), 3);
    assert_eq!(w.samples(), &[0, -32768, 32767, -1, 1000, -1000]);
}

#[test]
fn pcm24_sign_extends_three_bytes() {
    // The pinned value from the format: bytes 00 00 80 are the most
    // negative 24-bit sample, -8_388_608. Positive max is 7F FF FF.
    let data = [
        0x00, 0x00, 0x80, // -8388608
        0xFF, 0xFF, 0x7F, //  8388607
        0x00, 0x00, 0x00, //        0
        0xFF, 0xFF, 0xFF, //       -1
        0x00, 0x01, 0x00, //      256
    ];
    let w = decode(&wav(1, 1, 48_000, 24, &data)).unwrap();
    assert_eq!(w.samples(), &[-8388608, 8388607, 0, -1, 256]);
}

#[test]
fn pcm32_decodes_little_endian_signed() {
    let mut data = Vec::new();
    for s in [i32::MIN, i32::MAX, -1, 0, 305_419_896] {
        data.extend_from_slice(&s.to_le_bytes());
    }
    let w = decode(&wav(1, 1, 96_000, 32, &data)).unwrap();
    assert_eq!(w.samples(), &[i32::MIN, i32::MAX, -1, 0, 305_419_896]);
}

#[test]
fn float32_scales_and_clamps_to_i32() {
    // Canonical float mapping: clamp [-1, 1], scale by 2^31, round half
    // away from zero; NaN -> 0.
    let mut data = Vec::new();
    for s in [0.0f32, -1.0, 1.0, 0.5, 1.5, f32::NAN] {
        data.extend_from_slice(&s.to_le_bytes());
    }
    let w = decode(&wav(3, 1, 44_100, 32, &data)).unwrap();
    assert_eq!(w.format(), SampleFormat::IeeeFloat);
    assert_eq!(w.bits_per_sample(), 32);
    assert_eq!(
        w.samples(),
        &[0, i32::MIN, i32::MAX, 1_073_741_824, i32::MAX, 0]
    );
}

#[test]
fn float64_decodes_exact_halves() {
    let mut data = Vec::new();
    for s in [0.25f64, -0.5, 1.0 / 3.0] {
        data.extend_from_slice(&s.to_le_bytes());
    }
    let w = decode(&wav(3, 1, 44_100, 64, &data)).unwrap();
    assert_eq!(w.bits_per_sample(), 64);
    // 1/3 * 2^31 = 715827882.67 -> 715827883 after rounding.
    assert_eq!(w.samples(), &[536_870_912, -1_073_741_824, 715_827_883]);
}

// ---- RIFF structure ---------------------------------------------------------

#[test]
fn odd_sized_chunk_before_data_consumes_pad_byte() {
    // A 5-byte LIST payload is followed by one pad byte. If the pad were
    // not skipped, the next "header" would start on the pad and the data
    // chunk would never be found -> missing data chunk error.
    let file = wav_file(&[
        chunk(b"fmt ", &fmt_chunk(1, 1, 44_100, 16)),
        chunk(b"LIST", b"INFOX"),
        chunk(b"data", &[0x01, 0x00, 0xFE, 0xFF]),
    ]);
    let w = decode(&file).unwrap();
    assert_eq!(w.samples(), &[1, -2]);
}

#[test]
fn unknown_chunks_are_skipped() {
    let file = wav_file(&[
        chunk(b"JUNK", &[1, 2, 3, 4]),
        chunk(b"fmt ", &fmt_chunk(1, 1, 8_000, 8)),
        chunk(b"cue ", &[9, 9, 9, 9, 9, 9]),
        chunk(b"data", &[0x80]),
        chunk(b"data", &[0x00]), // a second data chunk is ignored
    ]);
    let w = decode(&file).unwrap();
    assert_eq!(w.samples(), &[0]);
}

#[test]
fn data_chunk_before_fmt_still_decodes() {
    // Legal RIFF order is fmt-then-data, but nothing in the container
    // forbids the reverse; the walk tolerates it.
    let file = wav_file(&[
        chunk(b"data", &[0x80]),
        chunk(b"fmt ", &fmt_chunk(1, 1, 8_000, 8)),
    ]);
    let w = decode(&file).unwrap();
    assert_eq!(w.samples(), &[0]);
}

#[test]
fn fmt_extension_bytes_are_ignored() {
    // A larger fmt chunk (as WAVE_FORMAT_EXTENSIBLE files carry) keeps the
    // same 16-byte prefix; extra bytes are part of the chunk, skipped by
    // the walk.
    let mut fmt = fmt_chunk(1, 1, 44_100, 16);
    fmt.extend_from_slice(&[0xAA; 22]);
    let file = wav_file(&[chunk(b"fmt ", &fmt), chunk(b"data", &[0x05, 0x00])]);
    let w = decode(&file).unwrap();
    assert_eq!(w.samples(), &[5]);
}

#[test]
fn riff_size_field_is_not_trusted() {
    // Some writers leave the RIFF size at 0; chunk walking must reach the
    // data anyway because it is bounded by physical input, not the field.
    let mut file = wav(1, 1, 8_000, 8, &[0x80, 0x81]);
    file[4..8].copy_from_slice(&0u32.to_le_bytes());
    let w = decode(&file).unwrap();
    assert_eq!(w.samples(), &[0, 1]);
}

#[test]
fn trailing_partial_frame_is_dropped() {
    // 16-bit mono data with a dangling byte: the odd byte cannot be a
    // sample, so it is dropped rather than read as one.
    let w = decode(&wav(1, 1, 8_000, 16, &[0x05, 0x00, 0xAA])).unwrap();
    assert_eq!(w.samples(), &[5]);
    assert_eq!(w.frames(), 1);
}

#[test]
fn empty_data_chunk_decodes_to_zero_frames() {
    let w = decode(&wav(1, 1, 8_000, 16, &[])).unwrap();
    assert!(w.samples().is_empty());
    assert_eq!(w.frames(), 0);
}

// ---- refusals ---------------------------------------------------------------

#[test]
fn non_wave_riff_is_invalid_magic() {
    let mut file = wav(1, 1, 8_000, 8, &[0x80]);
    file[8..12].copy_from_slice(b"AVI ");
    assert_eq!(
        decode(&file),
        Err(Error::InvalidMagic {
            what: "wave form type"
        })
    );
}

#[test]
fn non_riff_file_is_invalid_magic() {
    let file = b"not a wave file at all.........".to_vec();
    assert_eq!(
        decode(&file),
        Err(Error::InvalidMagic {
            what: "riff signature"
        })
    );
}

#[test]
fn unsupported_format_tags_are_refused() {
    for tag in [2u16, 6, 7, 80, 85, 0xFFFE] {
        let file = wav(tag, 1, 8_000, 8, &[0x80]);
        match decode(&file) {
            Err(Error::Unsupported(_)) => {}
            other => panic!("tag {tag}: expected Unsupported, got {other:?}"),
        }
    }
}

#[test]
fn unsupported_bit_depths_are_refused() {
    // PCM depths outside 8/16/24/32 and float depths outside 32/64.
    for bits in [4u16, 12, 20, 64] {
        let file = wav(1, 1, 8_000, bits, &[0u8; 8]);
        match decode(&file) {
            Err(Error::Unsupported(_)) => {}
            other => panic!("PCM bits {bits}: expected Unsupported, got {other:?}"),
        }
    }
    for bits in [8u16, 16, 128] {
        let file = wav(3, 1, 8_000, bits, &[0u8; 16]);
        match decode(&file) {
            Err(Error::Unsupported(_)) => {}
            other => panic!("float bits {bits}: expected Unsupported, got {other:?}"),
        }
    }
}

#[test]
fn missing_required_chunks_are_named_errors() {
    let only_fmt = wav_file(&[chunk(b"fmt ", &fmt_chunk(1, 1, 8_000, 8))]);
    assert_eq!(
        decode(&only_fmt),
        Err(Error::BadValue("missing data chunk"))
    );
    let only_data = wav_file(&[chunk(b"data", &[0x80])]);
    assert_eq!(
        decode(&only_data),
        Err(Error::BadValue("missing fmt chunk"))
    );
}

#[test]
fn degenerate_fmt_fields_are_named_errors() {
    let file = |f: Vec<u8>| wav_file(&[chunk(b"fmt ", &f), chunk(b"data", &[0x80])]);
    assert_eq!(
        decode(&file(fmt_chunk(1, 0, 8_000, 8))),
        Err(Error::BadValue("zero channels"))
    );
    assert_eq!(
        decode(&file(fmt_chunk(1, 1, 0, 8))),
        Err(Error::BadValue("zero sample rate"))
    );
    let mut bad_align = fmt_chunk(1, 1, 8_000, 8);
    bad_align[12] = 7; // block_align should be 1
    assert_eq!(
        decode(&file(bad_align)),
        Err(Error::BadValue(
            "block_align differs from channels * bits_per_sample / 8"
        ))
    );
    assert_eq!(
        decode(&file(vec![1, 0, 1, 0])),
        Err(Error::truncated("fmt chunk", 16, 4))
    );
}

// ---- truncation -------------------------------------------------------------

#[test]
fn every_prefix_length_fails_clean_never_panics() {
    // A stereo 16-bit file with an odd LIST chunk: every internal structure
    // (header, chunk headers, pad byte, payloads) gets bisected somewhere in
    // this loop.
    let file = wav_file(&[
        chunk(b"fmt ", &fmt_chunk(1, 2, 44_100, 16)),
        chunk(b"LIST", b"abc"),
        chunk(b"data", &[1, 0, 2, 0, 3, 0, 4, 0]),
    ]);
    assert!(decode(&file).is_ok());
    for n in 0..file.len() {
        // Err is fine; a panic would abort this test process.
        let _ = decode(&file[..n]);
    }
}

#[test]
fn truncated_data_payload_is_truncated_not_clamped() {
    let mut file = wav(1, 1, 8_000, 16, &[1, 0, 2, 0, 3, 0, 4, 0]);
    file.truncate(file.len() - 3); // declared data size 8, only 5 present
    assert_eq!(
        decode(&file),
        Err(Error::Truncated {
            what: "riff chunk payload",
            needed: 8,
            found: 5
        })
    );
}

#[test]
fn chunk_size_running_past_eof_is_truncated() {
    // A LIST chunk claiming 1000 bytes inside a file that has ~40.
    let mut file = wav_file(&[chunk(b"fmt ", &fmt_chunk(1, 1, 8_000, 8))]);
    file.extend_from_slice(b"LIST");
    file.extend_from_slice(&1000u32.to_le_bytes());
    file.extend_from_slice(&[0u8; 10]);
    assert!(matches!(
        decode(&file),
        Err(Error::Truncated {
            what: "riff chunk payload",
            ..
        })
    ));
}

//! Decode-path tests: pixel-exact conformance against ffmpeg/x264
//! fixtures (see `fixtures/PROVENANCE.md`), determinism, hostile-input
//! robustness, and seam-level mutation guards.

use modhash_h264::{Frame, Limits, decode, decode_with_limits};
use modhash_primitives::Error;
use std::fs;

fn fixture(name: &str) -> Vec<u8> {
    fs::read(format!("tests/fixtures/{name}")).unwrap()
}

/// YUV420p planes of `n` frames at `w`x`h` out of one raw file.
fn yuv_frames(raw: &[u8], w: usize, h: usize, n: usize) -> Vec<Frame> {
    let yl = w * h;
    let cl = (w / 2) * (h / 2);
    let fl = yl + 2 * cl;
    (0..n)
        .map(|f| Frame {
            width: w as u32,
            height: h as u32,
            y: raw[f * fl..f * fl + yl].to_vec(),
            cb: raw[f * fl + yl..f * fl + yl + cl].to_vec(),
            cr: raw[f * fl + yl + cl..f * fl + fl].to_vec(),
        })
        .collect()
}

/// Pixel-exact comparison: the whole contract in one assertion.
fn assert_pixels(name: &str, w: usize, h: usize, n: usize) {
    let stream = fixture(&format!("{name}.h264"));
    let reference = fixture(&format!("{name}.yuv"));
    let frames = decode(&stream).unwrap();
    assert_eq!(frames.len(), n, "{name}: frame count");
    let want = yuv_frames(&reference, w, h, n);
    for (i, (got, exp)) in frames.iter().zip(want.iter()).enumerate() {
        assert_eq!(got.width, exp.width, "{name}[{i}] width");
        assert_eq!(got.height, exp.height, "{name}[{i}] height");
        assert_eq!(got.y, exp.y, "{name}[{i}] luma plane");
        assert_eq!(got.cb, exp.cb, "{name}[{i}] Cb plane");
        assert_eq!(got.cr, exp.cr, "{name}[{i}] Cr plane");
    }
}

#[test]
fn conformance_no_deblock_variants() {
    // Streams encoded with loop filter disabled: the reconstruction
    // path alone must be pixel-exact before the loop filter is ever
    // reached.
    assert_pixels("gd16_nd", 16, 16, 1);
    assert_pixels("grad16_nd", 16, 16, 1);
    assert_pixels("t1_nd", 32, 32, 5);
    assert_pixels("t3_nd", 16, 16, 4);
}

#[test]
fn conformance_loop_filtered_small() {
    // Full intra + deblocking pipeline on flat and gradient content.
    assert_pixels("gd16", 16, 16, 1);
    assert_pixels("grad16", 16, 16, 1);
    assert_pixels("hgrad32", 32, 32, 1);
    assert_pixels("flat16", 16, 16, 1);
    assert_pixels("flat16b", 16, 16, 1);
    assert_pixels("flat16i4", 16, 16, 1);
}

#[test]
fn conformance_ip_32x32() {
    assert_pixels("t1_32x32_ip", 32, 32, 5);
}

#[test]
fn conformance_ip_48x48() {
    assert_pixels("t2_48x48_ip", 48, 48, 5);
}

#[test]
fn conformance_tiny_16x16() {
    assert_pixels("t3_16x16", 16, 16, 4);
}

#[test]
fn conformance_sliced_80x64() {
    assert_pixels("t4_80x64_sliced", 80, 64, 6);
}

#[test]
fn conformance_i16_64x64() {
    assert_pixels("t5_64x64_i16", 64, 64, 10);
}

#[test]
fn decode_is_deterministic() {
    let stream = fixture("t2_48x48_ip.h264");
    let a = decode(&stream).unwrap();
    let b = decode(&stream).unwrap();
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.y, y.y);
        assert_eq!(x.cb, y.cb);
        assert_eq!(x.cr, y.cr);
    }
}

#[test]
fn truncated_never_panics() {
    let stream = fixture("t3_16x16.h264");
    for i in 1..stream.len() {
        let _ = decode(&stream[..i]);
    }
}

#[test]
fn bitflip_never_panics() {
    let stream = fixture("t3_16x16.h264");
    let mut s = modhash_primitives::SplitMix64::new(0x5eed);
    for _ in 0..20_000 {
        let mut bad = stream.clone();
        let i = s.next_u64() as usize % bad.len();
        bad[i] ^= 1 << (s.next_u64() % 8);
        let _ = decode(&bad);
    }
}

#[test]
fn limits_are_enforced() {
    let stream = fixture("t2_48x48_ip.h264");
    let lim = Limits {
        max_input: stream.len() - 1,
        ..Limits::default()
    };
    assert!(matches!(
        decode_with_limits(&stream, &lim),
        Err(Error::TooLarge { .. })
    ));
    let lim = Limits {
        max_frames: 0,
        ..Limits::default()
    };
    assert!(decode_with_limits(&stream, &lim).is_err());
    let lim = Limits {
        max_luma_samples: 47 * 47,
        ..Limits::default()
    };
    assert!(decode_with_limits(&stream, &lim).is_err());
}

#[test]
fn garbage_is_error_not_panic() {
    assert!(decode(&[]).is_err());
    assert!(decode(&[0, 0, 1]).is_err());
    // A reserved NAL type is skipped: Ok with zero frames, no panic.
    assert!(decode(&[0, 0, 0, 1, 0xff, 0xff]).map_or(true, |f| f.is_empty()));
}

/// High-profile streams are refused with an explicit profile name.
#[test]
fn high_profile_named_unsupported() {
    // Hand-built minimal SPS: profile_idc 110 (High 10) — P13 decodes
    // the 8-bit 4:2:0 High feature set, so the named-refusal check
    // uses a profile whose bit depth stays out of scope.
    let sps = [
        0x00, 0x00, 0x00, 0x01, 0x67, 0x6e, 0x00, 0x0a, 0xf8, 0x88, 0x80,
    ];
    match decode(&sps) {
        Err(Error::Unsupported(m)) => assert!(m.contains("High"), "message: {m}"),
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

// ---- P13 Main-profile conformance (B slices, CABAC, 8x8, weighted) ----

#[test]
fn conformance_b_slices() {
    // B-slice streams: CABAC + direct modes + B-pyramid + multi-ref.
    assert_pixels("m1_b3_64x64", 64, 64, 10);
    assert_pixels("m2_b2_refs_64x64", 64, 64, 12);
    assert_pixels("m3_b4_strat_64x48", 64, 48, 12);
    assert_pixels("m4_direct_temp_32x32", 32, 32, 8);
    assert_pixels("m5_direct_spat_32x32", 32, 32, 8);
    assert_pixels("m6_bpyr_64x64", 64, 64, 12);
}

#[test]
fn conformance_8x8_transform() {
    // High-profile streams restricted to the 8x8-transform feature:
    // inter 8x8 + I_8x8 intra prediction.
    assert_pixels("t1_8x8_64x64", 64, 64, 12);
    assert_pixels("t2_8x8_part_64x64", 64, 64, 10);
    assert_pixels("t3_i8x8_64x48", 64, 48, 12);
    assert_pixels("t4_8x8_umh_64x64", 64, 64, 12);
}

#[test]
fn conformance_weighted_pred() {
    // Explicit P-slice weighting + implicit/explicit B weighting.
    assert_pixels("w1_wp_exp_64x64", 64, 64, 10);
    assert_pixels("w2_wp_simple_64x64", 64, 64, 12);
    assert_pixels("w3_wp_b_32x32", 32, 32, 8);
}

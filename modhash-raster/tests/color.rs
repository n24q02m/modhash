//! BT.601 luma, pinning the rounding convention: `round_half_up`, not
//! truncation. `luma_bt601_rounds_half_up_not_down` is the mutation
//! proof's tripwire — it fails if `(sum + 500)/1000` becomes
//! truncation.

use modhash_raster::{Gray, Image, Rgb, Rgba, luma_bt601};

#[test]
fn gray_r_equals_g_equals_b_is_identity() {
    // 0.299 + 0.587 + 0.114 == 1 exactly, so equal channels pass through.
    for v in [0u8, 1, 100, 128, 254, 255] {
        assert_eq!(luma_bt601(v, v, v), v);
    }
}

#[test]
fn primaries_and_extremes() {
    assert_eq!(luma_bt601(255u8, 0, 0), 76); // 76.245 -> 76
    assert_eq!(luma_bt601(0u8, 255, 0), 150); // 149.685 -> 150
    assert_eq!(luma_bt601(0u8, 0, 255), 29); // 29.07 -> 29
    assert_eq!(luma_bt601(0u8, 0, 0), 0);
    assert_eq!(luma_bt601(255u8, 255, 255), 255);
}

#[test]
fn luma_bt601_rounds_half_up_not_down() {
    // The .5 boundary case: (0,0,250) -> 28.5 must go to 29; truncation
    // gives 28. Without this case a change of rounding mode stays green.
    assert_eq!(luma_bt601(0u8, 0, 250), 29);
    // A second pin well above .5 where truncation diverges too:
    // (76,0,0) -> 22.724 -> 23, not 22.
    assert_eq!(luma_bt601(76u8, 0, 0), 23);
    // And a value that truncates the same either way, so the test does
    // not degenerate to "everything rounds up": (0,0,255) -> 29.07 -> 29
    // is already covered; (1,0,0) -> 0.299 -> 0.
    assert_eq!(luma_bt601(1u8, 0, 0), 0);
}

#[test]
fn luma_of_u16_scales_with_the_sample_range() {
    // u16 white is 65535 exactly; a mid value keeps proportionality.
    assert_eq!(luma_bt601(65535u16, 65535, 65535), 65535);
    assert_eq!(luma_bt601(1000u16, 1000, 1000), 1000);
    // (0,0,1000) -> 114.0 -> 114; (0,0,999) -> 113.886 -> 114.
    assert_eq!(luma_bt601(0u16, 0, 999), 114);
}

#[test]
fn to_gray_maps_every_rgb_pixel() {
    // 2×1: red pixel and a gray pixel.
    let im: Image<Rgb> = Image::from_vec(2, 1, vec![255, 0, 0, 100, 100, 100]).unwrap();
    let gray: Image<Gray> = im.to_gray();
    assert_eq!((gray.width(), gray.height(), gray.channels()), (2, 1, 1));
    assert_eq!(gray.as_slice(), &[76, 100]);
}

#[test]
fn to_gray_on_rgba_ignores_alpha() {
    // Two identical RGB values with different alpha must give identical
    // luma — alpha is not composited.
    let im: Image<Rgba> = Image::from_vec(2, 1, vec![255, 0, 0, 0, 255, 0, 0, 255]).unwrap();
    let gray = im.to_gray();
    assert_eq!(gray.as_slice(), &[76, 76]);
}

#[test]
fn to_gray_u16_matches_the_scalar_function() {
    let im: Image<Rgb, u16> = Image::from_vec(1, 2, vec![0, 0, 999, 65535, 65535, 65535]).unwrap();
    let gray = im.to_gray();
    assert_eq!(gray.as_slice(), &[114, 65535]);
}

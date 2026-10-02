//! Buffer semantics of `Image`: construction rules, layout, indexing.

use modhash_primitives::Error;
use modhash_raster::{Gray, Image, MAX_BUFFER_BYTES, Rgb, Rgba};

#[test]
fn new_is_zeroed_and_tight() {
    let im: Image<Rgb> = Image::new(4, 3).unwrap();
    assert_eq!((im.width(), im.height(), im.channels()), (4, 3, 3));
    assert_eq!(im.sample_count(), 36);
    assert!(im.as_slice().iter().all(|&s| s == 0));
}

#[test]
fn from_vec_checks_exact_length() {
    let err = Image::<Gray>::from_vec(4, 3, vec![0u8; 11]).unwrap_err();
    assert!(matches!(err, Error::BadValue(_)));
    let err = Image::<Rgba>::from_vec(4, 3, vec![0u8; 36]).unwrap_err();
    assert!(matches!(err, Error::BadValue(_)));
    assert!(Image::<Gray>::from_vec(4, 3, vec![0u8; 12]).is_ok());
}

#[test]
fn zero_dimensions_are_bad_value() {
    assert!(matches!(Image::<Gray>::new(0, 3), Err(Error::BadValue(_))));
    assert!(matches!(Image::<Gray>::new(3, 0), Err(Error::BadValue(_))));
    assert!(matches!(
        Image::<Gray>::from_vec(0, 0, Vec::new()),
        Err(Error::BadValue(_))
    ));
}

#[test]
fn dimensions_that_overflow_usize_are_refused_not_wrapped() {
    // u32::MAX * u32::MAX * 4 bytes far exceeds the cap; the u128 check
    // must fire before any multiplication can wrap.
    assert!(matches!(
        Image::<Rgba>::new(u32::MAX, u32::MAX),
        Err(Error::TooLarge {
            what: "image buffer",
            limit: MAX_BUFFER_BYTES
        })
    ));
}

#[test]
fn the_byte_cap_binds_before_allocation() {
    // A small Vec under huge dimensions: the size check must fire first,
    // so nothing large is ever allocated.
    let err =
        Image::<Rgb, u16>::from_vec(u16::MAX as u32, u16::MAX as u32, vec![0u16; 6]).unwrap_err();
    assert!(matches!(err, Error::TooLarge { .. }));
}

#[test]
fn pixel_access_is_bounds_checked() {
    let mut im: Image<Rgb> = Image::new(2, 1).unwrap();
    im.pixel_mut(1, 0).unwrap().copy_from_slice(&[7, 8, 9]);
    assert_eq!(im.pixel(1, 0), Some(&[7u8, 8, 9][..]));
    assert_eq!(im.pixel(0, 0), Some(&[0u8, 0, 0][..]));
    assert_eq!(im.pixel(2, 0), None);
    assert_eq!(im.pixel(0, 1), None);
}

#[test]
fn u16_buffers_carry_png_wide_samples() {
    let im: Image<Gray, u16> = Image::from_vec(2, 1, vec![0x1234, 0xFFFF]).unwrap();
    assert_eq!(im.as_slice(), &[0x1234, 0xFFFF]);
    assert_eq!(im.channels(), 1);
}

#[test]
fn into_vec_returns_the_tight_buffer() {
    let data = vec![1u8, 2, 3, 4, 5, 6];
    let im: Image<Rgb> = Image::from_vec(2, 1, data.clone()).unwrap();
    assert_eq!(im.into_vec(), data);
}

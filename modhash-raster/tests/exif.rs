//! EXIF orientation: all eight tags on a 2×3 image with a distinct value
//! per pixel, plus the dihedral composition properties.

use modhash_primitives::Error;
use modhash_raster::{Gray, Image, Rgb, apply_orientation};

/// 2×3 gray image, every pixel distinct:
///
/// ```text
/// (0,0)=10 (1,0)=20
/// (0,1)=30 (1,1)=40
/// (0,2)=50 (1,2)=60
/// ```
///
/// With ws=2, hs=3 the table in `docs/algorithms/raster.md` gives these
/// layouts. Every expected buffer below is `out[y][x] = in(cx,cy)` with
/// `(cx, cy)` taken from that table — derived by hand, not generated.
fn image2x3() -> Image<Gray> {
    Image::from_vec(2, 3, vec![10, 20, 30, 40, 50, 60]).unwrap()
}

#[test]
fn orientation_1_is_identity() {
    let out = apply_orientation(&image2x3(), 1).unwrap();
    assert_eq!((out.width(), out.height()), (2, 3));
    assert_eq!(out.as_slice(), &[10, 20, 30, 40, 50, 60]);
}

#[test]
fn orientation_2_flips_horizontally() {
    // out(x,y) = in(ws−1−x, y):  rows reversed horizontally.
    //   20 10 / 40 30 / 60 50
    let out = apply_orientation(&image2x3(), 2).unwrap();
    assert_eq!((out.width(), out.height()), (2, 3));
    assert_eq!(out.as_slice(), &[20, 10, 40, 30, 60, 50]);
}

#[test]
fn orientation_3_rotates_180() {
    // out(x,y) = in(ws−1−x, hs−1−y):  60 50 / 40 30 / 20 10
    let out = apply_orientation(&image2x3(), 3).unwrap();
    assert_eq!((out.width(), out.height()), (2, 3));
    assert_eq!(out.as_slice(), &[60, 50, 40, 30, 20, 10]);
}

#[test]
fn orientation_4_flips_vertically() {
    // out(x,y) = in(x, hs−1−y):  50 60 / 30 40 / 10 20
    let out = apply_orientation(&image2x3(), 4).unwrap();
    assert_eq!((out.width(), out.height()), (2, 3));
    assert_eq!(out.as_slice(), &[50, 60, 30, 40, 10, 20]);
}

#[test]
fn orientation_5_transposes_across_the_main_diagonal() {
    // out(x,y) = in(y, x), dims swap to 3×2:  10 30 50 / 20 40 60
    let out = apply_orientation(&image2x3(), 5).unwrap();
    assert_eq!((out.width(), out.height()), (3, 2));
    assert_eq!(out.as_slice(), &[10, 30, 50, 20, 40, 60]);
}

#[test]
fn orientation_6_rotates_90_clockwise() {
    // out(x,y) = in(y, hs−1−x), dims swap to 3×2:  50 30 10 / 60 40 20
    let out = apply_orientation(&image2x3(), 6).unwrap();
    assert_eq!((out.width(), out.height()), (3, 2));
    assert_eq!(out.as_slice(), &[50, 30, 10, 60, 40, 20]);
}

#[test]
fn orientation_7_transposes_across_the_anti_diagonal() {
    // out(x,y) = in(ws−1−y, hs−1−x), dims swap to 3×2:
    //   60 40 20 / 50 30 10
    let out = apply_orientation(&image2x3(), 7).unwrap();
    assert_eq!((out.width(), out.height()), (3, 2));
    assert_eq!(out.as_slice(), &[60, 40, 20, 50, 30, 10]);
}

#[test]
fn orientation_8_rotates_270_clockwise() {
    // out(x,y) = in(ws−1−y, x), dims swap to 3×2:  20 40 60 / 10 30 50
    let out = apply_orientation(&image2x3(), 8).unwrap();
    assert_eq!((out.width(), out.height()), (3, 2));
    assert_eq!(out.as_slice(), &[20, 40, 60, 10, 30, 50]);
}

#[test]
fn orientation_moves_whole_pixels_not_channels() {
    // On an RGB buffer the three samples of a pixel travel together.
    let im: Image<Rgb> = Image::from_vec(
        2,
        1,
        vec![1, 2, 3, 4, 5, 6], // px(0,0)=1,2,3 ; px(1,0)=4,5,6
    )
    .unwrap();
    let out = apply_orientation(&im, 2).unwrap(); // flip_h
    assert_eq!(out.as_slice(), &[4, 5, 6, 1, 2, 3]);
    let out = apply_orientation(&im, 6).unwrap(); // rot90 cw: 1×2 vertical
    assert_eq!((out.width(), out.height()), (1, 2));
    assert_eq!(out.as_slice(), &[1, 2, 3, 4, 5, 6]);
}

#[test]
fn tags_outside_1_to_8_are_bad_value() {
    let im = image2x3();
    for tag in [0u8, 9, 10, 200, 255] {
        assert!(matches!(
            apply_orientation(&im, tag),
            Err(Error::BadValue(_))
        ));
    }
}

#[test]
fn rotations_compose_through_the_group() {
    // D4 is closed: rot90 twice on a square image is rot180's effect.
    let square: Image<Gray> = Image::from_vec(2, 2, vec![1, 2, 3, 4]).unwrap();
    let twice = apply_orientation(&apply_orientation(&square, 6).unwrap(), 6).unwrap();
    let once = apply_orientation(&square, 3).unwrap();
    assert_eq!(twice.as_slice(), once.as_slice());

    // rot90 then rot270 is the identity, on a non-square image too.
    let im = image2x3();
    let back = apply_orientation(&apply_orientation(&im, 6).unwrap(), 8).unwrap();
    assert_eq!(back.as_slice(), im.as_slice());

    // Flips are involutions.
    let f = apply_orientation(&apply_orientation(&im, 2).unwrap(), 2).unwrap();
    assert_eq!(f.as_slice(), im.as_slice());
}

#[test]
fn transform_methods_match_apply_orientation() {
    // The public primitives on `Image` are the same operations the tag
    // dispatch uses — pinned so the table and the methods cannot drift.
    let im = image2x3();
    let cases: [(u8, Image<Gray>); 8] = [
        (1, im.clone()),
        (2, im.flip_h()),
        (3, im.rot180()),
        (4, im.flip_v()),
        (5, im.transpose()),
        (6, im.rot90()),
        (7, im.transverse()),
        (8, im.rot270()),
    ];
    for (tag, expected) in cases {
        let out = apply_orientation(&im, tag).unwrap();
        assert_eq!(out.as_slice(), expected.as_slice(), "tag {tag}");
    }
}

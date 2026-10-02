//! The raw image buffer every decoder writes and every hash reads.
//!
//! [`Image`] is deliberately the smallest honest structure: a tight,
//! row-major, unpadded [`Vec`](alloc::vec::Vec) of samples plus `width`,
//! `height` and two marker parameters — the pixel [`Layout`] (`Gray`,
//! `Rgb`, `Rgba`) and the [`Sample`] type (`u8`, `u16`). Tight layout
//! means `stride == width * channels` is never stored and can never
//! disagree with the data; a padded buffer would need a stride field, a
//! stride argument on every constructor, and a policy for the tail bytes.
//!
//! An image is always well-formed: [`Image::new`] and [`Image::from_vec`]
//! refuse zero dimensions, wrap-around arithmetic and buffers larger than
//! [`MAX_BUFFER_BYTES`], so methods can index without re-checking.

use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::mem::size_of;

use modhash_primitives::{Error, Result};

/// Hard ceiling on a single buffer, in bytes of samples.
///
/// A decoder that trusted a 16-bit width field can be told an image is
/// 65,535×65,535; honouring that blindly is a denial-of-service
/// primitive. The ceiling is bytes, not pixels, so it binds `u8` and
/// `u16` buffers identically: `width * height * channels *
/// size_of::<T>()` must not exceed it. 1 GiB fits any photographic frame
/// the kit's decoders are asked to hash while keeping a hostile header
/// from reserving the address space.
pub const MAX_BUFFER_BYTES: usize = 1 << 30;

mod sealed {
    /// Seals [`Layout`](super::Layout) and [`Sample`](super::Sample) so
    /// downstreams cannot add a layout or sample type this crate's
    /// transforms were never validated for.
    pub trait Sealed {}
}

/// Pixel layout of an [`Image`]: how many channels one pixel occupies.
///
/// Implemented by the marker types [`Gray`], [`Rgb`] and [`Rgba`] only;
/// the trait is sealed because every transform in the crate is written
/// against a concrete channel count.
pub trait Layout: sealed::Sealed + Copy {
    /// Samples per pixel.
    const CHANNELS: usize;
}

/// Single-channel (luma) layout: one sample per pixel.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Gray;
/// Red-green-blue layout: three samples per pixel, `R G B` in order.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Rgb;
/// Red-green-blue-alpha layout: four samples per pixel, `R G B A` in
/// order.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Rgba;

impl sealed::Sealed for Gray {}
impl sealed::Sealed for Rgb {}
impl sealed::Sealed for Rgba {}
impl Layout for Gray {
    const CHANNELS: usize = 1;
}
impl Layout for Rgb {
    const CHANNELS: usize = 3;
}
impl Layout for Rgba {
    const CHANNELS: usize = 4;
}

/// One channel value of a pixel: `u8` for the 8-bit formats, `u16` for
/// PNG's 16-bit variants.
///
/// Sealed to `u8` and `u16` so arithmetic in [`crate::color`] and
/// [`crate::resample`] can widen to `u64` with a single conversion and
/// never check an upper bound again.
pub trait Sample: sealed::Sealed + Copy + Default + Eq {
    /// Widens the sample to `u64` for accumulation.
    fn to_u64(self) -> u64;
    /// Narrows a value in `0..=Self::MAX_U64` back to the sample type.
    ///
    /// Callers only pass the rounded mean of same-typed samples or a
    /// luma value already clamped to the range, so this never wraps.
    fn from_u64(v: u64) -> Self;
    /// The largest value of the type, as `u64`.
    const MAX_U64: u64;
}

impl sealed::Sealed for u8 {}
impl sealed::Sealed for u16 {}
impl Sample for u8 {
    #[inline]
    fn to_u64(self) -> u64 {
        u64::from(self)
    }
    #[inline]
    fn from_u64(v: u64) -> Self {
        v as u8
    }
    const MAX_U64: u64 = 255;
}
impl Sample for u16 {
    #[inline]
    fn to_u64(self) -> u64 {
        u64::from(self)
    }
    #[inline]
    fn from_u64(v: u64) -> Self {
        v as u16
    }
    const MAX_U64: u64 = 65_535;
}

/// A tight row-major image buffer: `width * height * L::CHANNELS`
/// samples, no padding, `stride == width * L::CHANNELS`.
///
/// `Image<Gray, u8>` is what pHash consumes; `Image<Rgb, u8>` and
/// `Image<Rgba, u8>` are what the 8-bit decoders produce;
/// `Image<_, u16>` carries PNG's 16-bit samples until a consumer
/// reduces them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image<L: Layout, T: Sample = u8> {
    width: u32,
    height: u32,
    pixels: Vec<T>,
    layout: PhantomData<L>,
}

/// Validates `width`, `height` and a buffer of `len` samples, returning
/// `len` unchanged for use sites that want it.
fn checked_len(width: u32, height: u32, channels: usize, sample_bytes: usize) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(Error::BadValue("image dimension is zero"));
    }
    // u128 keeps width*height*channels*size_of in range even at u32::MAX².
    let samples = u128::from(width) * u128::from(height) * (channels as u128);
    let bytes = samples * (sample_bytes as u128);
    if bytes > MAX_BUFFER_BYTES as u128 {
        return Err(Error::TooLarge {
            what: "image buffer",
            limit: MAX_BUFFER_BYTES,
        });
    }
    Ok(samples as usize)
}

impl<L: Layout, T: Sample> Image<L, T> {
    /// A zeroed `width`×`height` buffer.
    ///
    /// Refuses a zero dimension (`Error::BadValue`) and any buffer over
    /// [`MAX_BUFFER_BYTES`] (`Error::TooLarge`) before allocating.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let len = checked_len(width, height, L::CHANNELS, size_of::<T>())?;
        Ok(Self {
            width,
            height,
            pixels: vec![T::default(); len],
            layout: PhantomData,
        })
    }

    /// Wraps `pixels`, which must be exactly `width * height *
    /// CHANNELS` samples long.
    ///
    /// Wrong length is `Error::BadValue`; the same dimension rules as
    /// [`Image::new`] apply.
    pub fn from_vec(width: u32, height: u32, pixels: Vec<T>) -> Result<Self> {
        let len = checked_len(width, height, L::CHANNELS, size_of::<T>())?;
        if pixels.len() != len {
            return Err(Error::BadValue("buffer length != width*height*channels"));
        }
        Ok(Self {
            width,
            height,
            pixels,
            layout: PhantomData,
        })
    }

    /// Image width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Samples per pixel — [`Layout::CHANNELS`] of the layout marker.
    pub fn channels(&self) -> usize {
        L::CHANNELS
    }

    /// Total sample count: `width * height * channels`.
    pub fn sample_count(&self) -> usize {
        self.pixels.len()
    }

    /// The tight sample buffer, row-major.
    pub fn as_slice(&self) -> &[T] {
        &self.pixels
    }

    /// The tight sample buffer, row-major, mutable.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.pixels
    }

    /// Consumes the image and returns its samples.
    pub fn into_vec(self) -> Vec<T> {
        self.pixels
    }

    /// The `CHANNELS` samples of pixel `(x, y)`, or `None` out of bounds.
    ///
    /// Bounds-checking rather than panicking keeps a decoder's off-by-one
    /// in scanline bookkeeping from becoming an abort.
    pub fn pixel(&self, x: u32, y: u32) -> Option<&[T]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize) * self.width as usize * L::CHANNELS + x as usize * L::CHANNELS;
        Some(&self.pixels[i..i + L::CHANNELS])
    }

    /// The mutable `CHANNELS` samples of pixel `(x, y)`, or `None` out
    /// of bounds.
    pub fn pixel_mut(&mut self, x: u32, y: u32) -> Option<&mut [T]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize) * self.width as usize * L::CHANNELS + x as usize * L::CHANNELS;
        Some(&mut self.pixels[i..i + L::CHANNELS])
    }

    /// Rebuilds the image at `(out_w, out_h)` with `map` giving the
    /// source coordinates of each output pixel: `map(x, y) -> (cx, cy)`.
    ///
    /// Crate-internal workhorse for the eight dihedral transforms; the
    /// caller guarantees `cx < out-source width` and `cy < out-source
    /// height` for every output pixel, so indexing is direct.
    pub(crate) fn remap(
        &self,
        out_w: u32,
        out_h: u32,
        map: impl Fn(u32, u32) -> (u32, u32),
    ) -> Self {
        let c = L::CHANNELS;
        let mut out = vec![T::default(); out_w as usize * out_h as usize * c];
        let in_w = self.width as usize;
        for y in 0..out_h {
            for x in 0..out_w {
                let (cx, cy) = map(x, y);
                let src = (cy as usize) * in_w * c + cx as usize * c;
                let dst = (y as usize) * out_w as usize * c + x as usize * c;
                out[dst..dst + c].copy_from_slice(&self.pixels[src..src + c]);
            }
        }
        Self {
            width: out_w,
            height: out_h,
            pixels: out,
            layout: PhantomData,
        }
    }

    /// Mirror left↔right: `out(x, y) = in(w−1−x, y)`; size unchanged.
    /// EXIF orientation 2.
    pub fn flip_h(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.remap(w, h, |x, y| (w - 1 - x, y))
    }

    /// Mirror top↔bottom: `out(x, y) = in(x, h−1−y)`; size unchanged.
    /// EXIF orientation 4.
    pub fn flip_v(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.remap(w, h, |x, y| (x, h - 1 - y))
    }

    /// Rotate 180°: `out(x, y) = in(w−1−x, h−1−y)`; size unchanged.
    /// EXIF orientation 3.
    pub fn rot180(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.remap(w, h, |x, y| (w - 1 - x, h - 1 - y))
    }

    /// Rotate 90° clockwise: `out(x, y) = in(y, h−1−x)`; a `w×h` image
    /// becomes `h×w`. EXIF orientation 6.
    pub fn rot90(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.remap(h, w, |x, y| (y, h - 1 - x))
    }

    /// Rotate 270° clockwise (90° counter-clockwise): `out(x, y) =
    /// in(w−1−y, x)`; a `w×h` image becomes `h×w`. EXIF orientation 8.
    pub fn rot270(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.remap(h, w, |x, y| (w - 1 - y, x))
    }

    /// Reflect across the main diagonal: `out(x, y) = in(y, x)`; a
    /// `w×h` image becomes `h×w`. EXIF orientation 5.
    pub fn transpose(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.remap(h, w, |x, y| (y, x))
    }

    /// Reflect across the anti-diagonal: `out(x, y) = in(w−1−y, h−1−x)`;
    /// a `w×h` image becomes `h×w`. EXIF orientation 7.
    pub fn transverse(&self) -> Self {
        let (w, h) = (self.width, self.height);
        self.remap(h, w, |x, y| (w - 1 - y, h - 1 - x))
    }
}

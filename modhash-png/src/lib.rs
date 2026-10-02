//! PNG decoding (ISO/IEC 15948, W3C PNG specification): every colour type,
//! 1/2/4/8/16-bit depths, all five row filters and Adam7 interlacing,
//! into the kit's shared [`Image`] buffers.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package.
//!
//! Decode produces `u8` pixels for PNG files coded at 8 bits (and for the
//! sub-8-bit greyscale and indexed depths, which the specification defines
//! in terms of 8-bit output) and `u16` pixels for the 16-bit codings, big
//!-endian as PNG stores them. Colour types without a matching
//! [`Layout`](modhash_raster::Layout) expand to the smallest layout that
//! carries them losslessly: greyscale+alpha becomes `Rgba`, a `tRNS`
//! transparency on greyscale or truecolour produces `Rgba`, and indexed
//! colour is looked up through the `PLTE` palette (plus `tRNS` alpha) into
//! `Rgb`/`Rgba`.
//!
//! The stream is checked, not skimmed: every chunk CRC-32 is verified
//! ([`modhash_primitives::crc32`]), `IDAT` bytes inflate through
//! [`modhash_inflate`], chunk ordering rules (`IHDR` first, `PLTE`/`tRNS`
//! before `IDAT`, consecutive `IDAT`s, `IEND` last) are enforced, and any
//! violation is a named [`Error`], never a panic or a guess.
//!
//! A minimal encoder ([`encode`]) exists **only** so the test suite can
//! round-trip: it writes non-interlaced zlib streams of stored DEFLATE
//! blocks, one filter per file. It is test scaffolding, not a compressor.
//!
//! The crate is `no_std` apart from the `alloc` [`Vec`] its API returns.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

mod chunks;
mod encode;
mod recon;

pub use encode::encode;
pub use modhash_raster::{Gray, Image, Rgb, Rgba};

/// The 8-byte signature every PNG stream opens with.
const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Ceilings a caller imposes on one decode call.
///
/// [`Limits`] is not optional and there is no silently permissive default:
/// a decompressor without a ceiling on what it will produce is a
/// denial-of-service primitive, and PNG turns a few hundred compressed
/// bytes into a full image. [`Default`] exists for the common case — a
/// caller hashing one file — and is a *conservative* ceiling, not an
/// unlimited one.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Hard ceiling on input consumed, in bytes. An input longer than
    /// this is [`Error::TooLarge`] before a single chunk is read. The
    /// default is 64 MiB.
    pub max_input: usize,
    /// Hard ceiling on the inflated image data, in bytes (filter bytes
    /// included). Producing more than this is [`Error::TooLarge`], never
    /// an allocation attempt: the expected scanline count is computed
    /// from `IHDR` and compared before decompression starts. The default
    /// is 256 MiB.
    pub max_output: usize,
}

impl Default for Limits {
    /// A conservative ceiling: 64 MiB of file, 256 MiB of scanlines —
    /// every photograph a hashing pipeline should ever meet, while a
    /// decompression bomb dies quietly.
    fn default() -> Self {
        Limits {
            max_input: 64 * 1024 * 1024,
            max_output: 256 * 1024 * 1024,
        }
    }
}

/// The `IHDR` chunk fields: the only part of a file a caller needs before
/// deciding whether to decode.
///
/// All fields are validated against the allowed combinations in PNG
/// specification section 11.2.2 (`bit_depth`/`colour_type` table), so a
/// returned `Ihdr` always names a legal configuration.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Ihdr {
    /// Image width in pixels, 1..=2^31-1.
    pub width: u32,
    /// Image height in pixels, 1..=2^31-1.
    pub height: u32,
    /// Bit depth per sample: 1, 2, 4, 8 or 16.
    pub bit_depth: u8,
    /// Colour type code as stored: 0 grey, 2 RGB, 3 palette, 4 grey+alpha,
    /// 6 RGBA.
    pub colour_type: u8,
    /// `true` when the interlace method is Adam7 (field value 1).
    pub interlaced: bool,
}

/// Decoded pixels in a concrete layout: the variant the file's colour
/// type, bit depth and `tRNS` chunk map to.
///
/// The mapping is fixed — decoding the same file twice produces the same
/// variant — so equality of two `Pixels` values is full image equality.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pixels {
    /// Greyscale, 8-bit samples (`u8`).
    Gray8(Image<Gray, u8>),
    /// Greyscale, 16-bit samples (`u16`).
    Gray16(Image<Gray, u16>),
    /// Colour, 8-bit samples per channel.
    Rgb8(Image<Rgb, u8>),
    /// Colour, 16-bit samples per channel.
    Rgb16(Image<Rgb, u16>),
    /// Colour with alpha, 8-bit samples per channel.
    Rgba8(Image<Rgba, u8>),
    /// Colour with alpha, 16-bit samples per channel.
    Rgba16(Image<Rgba, u16>),
}

impl Pixels {
    /// Image width in pixels.
    pub fn width(&self) -> u32 {
        match self {
            Pixels::Gray8(i) => i.width(),
            Pixels::Gray16(i) => i.width(),
            Pixels::Rgb8(i) => i.width(),
            Pixels::Rgb16(i) => i.width(),
            Pixels::Rgba8(i) => i.width(),
            Pixels::Rgba16(i) => i.width(),
        }
    }

    /// Image height in pixels.
    pub fn height(&self) -> u32 {
        match self {
            Pixels::Gray8(i) => i.height(),
            Pixels::Gray16(i) => i.height(),
            Pixels::Rgb8(i) => i.height(),
            Pixels::Rgb16(i) => i.height(),
            Pixels::Rgba8(i) => i.height(),
            Pixels::Rgba16(i) => i.height(),
        }
    }

    /// Channels per pixel of the concrete layout (1, 3 or 4).
    pub fn channels(&self) -> usize {
        match self {
            Pixels::Gray8(_) | Pixels::Gray16(_) => 1,
            Pixels::Rgb8(_) | Pixels::Rgb16(_) => 3,
            Pixels::Rgba8(_) | Pixels::Rgba16(_) => 4,
        }
    }
}

/// A fully decoded PNG: the `IHDR` facts plus the pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Png {
    /// Image width in pixels.
    width: u32,
    /// Image height in pixels.
    height: u32,
    /// Bit depth the file was coded at (1, 2, 4, 8 or 16), which is *not*
    /// necessarily the depth of the returned samples: a 4-bit greyscale
    /// file still surfaces as [`Pixels::Gray8`].
    bit_depth: u8,
    /// Colour type code the file declared (0, 2, 3, 4 or 6), again not
    /// necessarily the layout the pixels landed in: type 3 surfaces as
    /// `Rgb8`/`Rgba8`.
    colour_type: u8,
    /// Whether the file was Adam7-interlaced.
    interlaced: bool,
    /// The decoded image.
    pixels: Pixels,
}

impl Png {
    /// Image width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Image height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bit depth the file declared in `IHDR`.
    pub fn bit_depth(&self) -> u8 {
        self.bit_depth
    }

    /// Colour type the file declared in `IHDR` (0, 2, 3, 4 or 6).
    pub fn colour_type(&self) -> u8 {
        self.colour_type
    }

    /// Whether the file used Adam7 interlacing.
    pub fn interlaced(&self) -> bool {
        self.interlaced
    }

    /// The decoded pixels.
    pub fn pixels(&self) -> &Pixels {
        &self.pixels
    }

    /// Consumes the `Png`, returning the decoded pixels.
    pub fn into_pixels(self) -> Pixels {
        self.pixels
    }
}

/// Parses the signature plus exactly the `IHDR` chunk — the only part of
/// a file a caller needs before deciding whether to decode.
///
/// `IHDR` must be the first chunk and must be 13 bytes (PNG specification
/// section 11.2.2); anything else is the corresponding named [`Error`].
/// Field values are validated (`width`/`height` nonzero and below 2^31,
/// legal `bit_depth`/`colour_type` combination, compression 0, filter
/// method 0, interlace 0 or 1). Later chunks are not examined here.
pub fn decode_ihdr(input: &[u8]) -> Result<Ihdr> {
    let body = chunks::first_chunk(input, b"IHDR")?;
    if body.len() != 13 {
        return Err(Error::BadValue("PNG IHDR length"));
    }
    chunks::parse_ihdr(body)
}

/// Decodes a whole PNG stream into pixels.
///
/// Layout (specification section 5): signature, `IHDR`, optional `PLTE`
/// and `tRNS`, one or more `IDAT`, `IEND`. Every chunk's CRC-32 is
/// verified; ancillary chunks are skipped by length after the check, so
/// a corrupted `tEXt` still fails the file — a hashing kit does not get
/// to pick which integrity it honours.
///
/// An unknown critical chunk is [`Error::Unsupported`]; the file may be
/// perfectly legal PNG and still undecodable without it. Everything else
/// malformed is a named [`Error`]: bad CRC, out-of-order chunks, an
/// illegal filter byte, a palette index past `PLTE`, an `IDAT` stream
/// whose inflated length is not exactly the image's scanline bytes.
pub fn decode(input: &[u8], limits: &Limits) -> Result<Png> {
    if input.len() > limits.max_input {
        return Err(Error::too_large("PNG input", limits.max_input));
    }
    let parsed = chunks::parse(input)?;
    recon::decode_image(&parsed, limits)
}

/// Internal decode result consumed by `recon`: the validated header, the
/// chunk payloads that matter, and the concatenated `IDAT` bytes.
pub(crate) struct Parsed {
    /// Validated `IHDR`.
    pub ihdr: Ihdr,
    /// `PLTE` entries, RGB triples, when the chunk was present.
    pub palette: Option<Vec<u8>>,
    /// `tRNS` payload when the chunk was present.
    pub trns: Option<Vec<u8>>,
    /// Concatenated `IDAT` payload, still zlib-compressed; `recon`
    /// inflates it after computing the exact expected size from `IHDR`.
    pub data: Vec<u8>,
}

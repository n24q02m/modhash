//! BMP decoding for 24/32-bit uncompressed and RLE8-compressed images.
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package.
//!
//! # Scope
//!
//! [`decode`] parses the `BM` file header plus a `BITMAPINFOHEADER` (40
//! bytes), `BITMAPV4HEADER` (108) or `BITMAPV5HEADER` (124) and decodes:
//!
//! - **24-bit uncompressed** (`BI_RGB`), stored `B G R` per pixel.
//! - **32-bit uncompressed** (`BI_RGB` or `BI_BITFIELDS` /
//!   `BI_ALPHABITFIELDS`). `BI_BITFIELDS` extracts every channel through
//!   the declared masks, so channel order, width and position all follow
//!   the file. `BI_RGB` uses the implicit `B G R x` byte order and, like
//!   Windows GDI, treats the fourth byte as padding: alpha is opaque.
//! - **8-bit RLE** (`BI_RLE8`) through a palette of `B G R reserved`
//!   entries. Encoded runs, absolute mode, end-of-line, delta and
//!   end-of-bitmap are all handled.
//!
//! Rows map correctly for both bottom-up (`biHeight > 0`, the common
//! form) and top-down (`biHeight < 0`) files; top-down RLE8 is rejected
//! because the format itself forbids it.
//!
//! # Canonical form
//!
//! Every accepted file becomes an [`Image<Rgba, u8>`], top-left origin,
//! `R G B A` per pixel. 24-bit files, 32-bit `BI_RGB` files and RLE8
//! palette lookups produce alpha `255`; only an explicit alpha mask
//! (`BI_BITFIELDS`/`BI_ALPHABITFIELDS` or the V4/V5 header's alpha-mask
//! field) produces real transparency. Palette "reserved" bytes are never
//! treated as alpha because the overwhelmingly common value `0` would
//! make every pixel transparent.
//!
//! # Errors and tolerance decisions (documented, load-bearing)
//!
//! - Anything outside the scope list above — `BITMAPCOREHEADER`
//!   (12-byte DIB), 1/2/4/16-bit depths, uncompressed 8-bit, `BI_RLE4`,
//!   embedded JPEG/PNG, non-`BM` OS/2 bitmap types — is
//!   [`Error::Unsupported`], named, never guessed at.
//! - `bfSize` is ignored after the magic check; the fields that actually
//!   locate data are `bfOffBits` and the DIB size.
//! - `bfOffBits` must lie at or past every structure that precedes the
//!   pixel data (DIB header, inline masks, palette); an offset that
//!   lands inside a header or palette is [`Error::BadValue`].
//! - The last uncompressed row needs only `width * bpp/8` bytes present:
//!   writers that omit final-row padding still decode. Any earlier row
//!   cut short is [`Error::Truncated`].
//! - RLE8 streams that end without an `EOF` escape decode as if `EOF`
//!   had been read; pixels never written keep the color of palette
//!   index 0, the documented RLE background. A run, absolute span or
//!   delta that writes outside the image is [`Error::BadValue`], not
//!   clipped.
//! - Channel masks must be contiguous and pairwise disjoint; a
//!   non-contiguous or overlapping mask is [`Error::BadValue`]. A zero
//!   alpha mask yields opaque pixels.
//!
//! # Dependencies
//!
//! `modhash-bmp` builds on `modhash-raster` (the shared [`Image`]
//! destination) and `modhash-primitives` (the shared [`Error`] type
//! every kit decoder returns).

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;
use modhash_primitives::{Error, Result};
use modhash_raster::{Image, Rgba};

fn truncated(what: &'static str, needed: usize, found: usize) -> Error {
    Error::truncated(what, needed, found)
}

/// `BI_RGB`: no compression.
const BI_RGB: u32 = 0;
/// `BI_RLE8`: run-length encoded 8-bit paletted.
const BI_RLE8: u32 = 1;
/// `BI_BITFIELDS`: channel masks accompany the DIB header.
const BI_BITFIELDS: u32 = 3;
/// `BI_ALPHABITFIELDS`: bitfields plus an explicit alpha mask; decoded
/// through the same mask path as `BI_BITFIELDS`.
const BI_ALPHABITFIELDS: u32 = 6;

/// `BITMAPINFOHEADER` size — the only 40-byte DIB header.
const DIB_INFOHEADER: u32 = 40;
/// `BITMAPV4HEADER` size.
const DIB_V4: u32 = 108;
/// `BITMAPV5HEADER` size.
const DIB_V5: u32 = 124;

/// What the file's compression field declared.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Compression {
    /// `BI_RGB`, uncompressed.
    Rgb,
    /// `BI_BITFIELDS` or `BI_ALPHABITFIELDS`: uncompressed pixels read
    /// through channel masks.
    Bitfields,
    /// `BI_RLE8`: 8-bit run-length encoding through a palette.
    Rle8,
}

/// Everything [`info`] reports without decoding a single pixel.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Info {
    /// Image width in pixels, always positive.
    pub width: u32,
    /// Image height in pixels, always positive.
    pub height: u32,
    /// Bits per pixel in the file: 8, 24 or 32.
    pub bits_per_pixel: u16,
    /// Declared compression.
    pub compression: Compression,
    /// `true` when `biHeight < 0`, i.e. file rows run top-to-bottom.
    pub top_down: bool,
    /// Byte offset of the pixel/RLE data within the file.
    pub data_offset: u32,
    /// DIB header size: 40, 108 or 124.
    pub dib_header_size: u32,
}

/// One channel mask decomposed for the pixel loop: `(v & mask) >>
/// shift` then scaled from `max` to `0..=255`.
#[derive(Copy, Clone, Debug)]
struct Mask {
    mask: u32,
    shift: u32,
    max: u32,
}

impl Mask {
    /// Splits a non-zero contiguous mask. A mask with holes is rejected;
    /// a zero mask is the caller's problem (absent channel).
    fn new(mask: u32) -> Result<Mask> {
        if mask == 0 {
            return Err(Error::BadValue("zero channel mask"));
        }
        let shift = mask.trailing_zeros();
        let max = mask >> shift;
        if max & (max + 1) != 0 {
            return Err(Error::BadValue("non-contiguous channel mask"));
        }
        Ok(Mask { mask, shift, max })
    }

    /// Extracts the channel from one pixel word and scales to `u8`,
    /// rounding half up so e.g. a 5-bit `max=31` field maps `16 → 132`.
    /// `u64` math keeps `max = u32::MAX` from overflowing.
    fn extract(&self, v: u32) -> u8 {
        let raw = u64::from((v & self.mask) >> self.shift);
        ((raw * 255 + u64::from(self.max) / 2) / u64::from(self.max)) as u8
    }
}

/// Little-endian field reads. Every read is bounds-checked; BMP fields
/// are fixed-width so a short read is [`Error::Truncated`].
struct Rd<'a> {
    data: &'a [u8],
}

impl Rd<'_> {
    fn u16(&self, at: usize, what: &'static str) -> Result<u16> {
        let b = self
            .data
            .get(at..at + 2)
            .ok_or_else(|| truncated(what, 2, self.data.len().saturating_sub(at)))?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&self, at: usize, what: &'static str) -> Result<u32> {
        let b = self
            .data
            .get(at..at + 4)
            .ok_or_else(|| truncated(what, 4, self.data.len().saturating_sub(at)))?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// The parsed header state the decoders run against.
struct Header {
    info: Info,
    /// `[r, g, b, a]` masks for 32-bit pixels; `a == 0` means no alpha
    /// channel in the file. Unused (all zero) for 24-bit and RLE8.
    masks: [u32; 4],
    /// Palette for RLE8 as `RGBA` quadruples (alpha forced to 255).
    palette: Vec<[u8; 4]>,
}

/// Parses the 14-byte file header plus the DIB header, masks and
/// palette. Shared by [`info`] and [`decode`].
fn parse_header(data: &[u8]) -> Result<Header> {
    // BITMAPFILEHEADER: "BM", size, reserved×2, data offset.
    if data.len() < 14 {
        return Err(truncated("file header", 14, data.len()));
    }
    if data[0..2] != *b"BM" {
        // BA/CI/CP/IC/PT are real OS/2 signatures — name them
        // unsupported rather than leaving the guess to the caller.
        return Err(match &data[0..2] {
            b"BA" | b"CI" | b"CP" | b"IC" | b"PT" => {
                Error::Unsupported("OS/2 bitmap array/icon/pointer")
            }
            _ => Error::InvalidMagic {
                what: "BM signature",
            },
        });
    }
    let rd = Rd { data };
    let data_offset = rd.u32(10, "file header")?;

    // DIB header.
    let dib = rd.u32(14, "dib header")?;
    let dib_end = match dib {
        DIB_INFOHEADER | DIB_V4 | DIB_V5 => 14usize
            .checked_add(dib as usize)
            .ok_or(Error::BadValue("dib header size overflow"))?,
        12 => return Err(Error::Unsupported("BITMAPCOREHEADER")),
        _ => return Err(Error::Unsupported("unknown DIB header size")),
    };
    if data.len() < dib_end {
        return Err(truncated("dib header", dib_end, data.len()));
    }

    let width_i = rd.u32(18, "dib header")? as i32;
    let height_i = rd.u32(22, "dib header")? as i32;
    let planes = rd.u16(26, "dib header")?;
    let bpp = rd.u16(28, "dib header")?;
    let compression_raw = rd.u32(30, "dib header")?;
    let colors_used = rd.u32(46, "dib header")?;

    if width_i <= 0 {
        return Err(Error::BadValue("image width is zero or negative"));
    }
    if height_i == 0 || height_i == i32::MIN {
        return Err(Error::BadValue("image height is zero or overflowed"));
    }
    if planes != 1 {
        return Err(Error::BadValue("planes must be 1"));
    }

    let compression = match compression_raw {
        BI_RGB => Compression::Rgb,
        BI_BITFIELDS | BI_ALPHABITFIELDS => Compression::Bitfields,
        BI_RLE8 => Compression::Rle8,
        2 => return Err(Error::Unsupported("BI_RLE4")),
        4 => return Err(Error::Unsupported("embedded JPEG (BI_JPEG)")),
        5 => return Err(Error::Unsupported("embedded PNG (BI_PNG)")),
        _ => return Err(Error::Unsupported("unknown compression value")),
    };

    let top_down = height_i < 0;
    let height = height_i.unsigned_abs();
    let width = width_i as u32;

    match (bpp, compression) {
        (24 | 32, Compression::Rgb) | (32, Compression::Bitfields) => {}
        (24, Compression::Bitfields) => {
            return Err(Error::Unsupported("24-bit BI_BITFIELDS"));
        }
        (8, Compression::Rle8) => {
            if top_down {
                return Err(Error::BadValue("top-down RLE8 is not a legal BMP"));
            }
        }
        (8, _) => return Err(Error::Unsupported("uncompressed 8-bit BMP")),
        (1 | 2 | 4 | 16, _) => return Err(Error::Unsupported("1/2/4/16-bit BMP")),
        _ => return Err(Error::Unsupported("unsupported bit depth")),
    }

    // Channel masks: only 32-bit pixels have them. `BI_RGB` carries the
    // implicit B G R x layout with no alpha channel.
    let mut masks = [0u32; 4];
    let mut structures_end = dib_end;
    if bpp == 32 && compression == Compression::Rgb {
        masks = [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0];
    }
    if bpp == 32 && compression == Compression::Bitfields {
        masks = if dib == DIB_INFOHEADER {
            // For a 40-byte header the masks are inline: 3 (or 4 for
            // BI_ALPHABITFIELDS) u32s right after it.
            let n = if compression_raw == BI_ALPHABITFIELDS {
                4
            } else {
                3
            };
            let mut m = [0u32; 4];
            for (i, slot) in m.iter_mut().enumerate().take(n) {
                *slot = rd.u32(54 + i * 4, "bitfields masks")?;
            }
            structures_end = 54 + n * 4;
            m
        } else {
            // V4/V5 keep all four masks inside the header at +40.
            [
                rd.u32(14 + 40, "bitfields masks")?,
                rd.u32(14 + 44, "bitfields masks")?,
                rd.u32(14 + 48, "bitfields masks")?,
                rd.u32(14 + 52, "bitfields masks")?,
            ]
        };
        for m in &masks[0..3] {
            Mask::new(*m)?;
        }
        if masks[3] != 0 {
            Mask::new(masks[3])?;
        }
        if masks[0] & masks[1] != 0
            || masks[0] & masks[2] != 0
            || masks[1] & masks[2] != 0
            || masks[3] & (masks[0] | masks[1] | masks[2]) != 0
        {
            return Err(Error::BadValue("overlapping channel masks"));
        }
    }

    // Palette for RLE8, stored in the file as `B G R reserved`.
    let mut palette = Vec::new();
    if bpp == 8 {
        let palette_at = dib_end;
        let count = if colors_used == 0 {
            256
        } else {
            colors_used as usize
        };
        if count > 256 {
            return Err(Error::BadValue("palette larger than 256 entries"));
        }
        let pal_bytes = count * 4;
        let pal_end = palette_at
            .checked_add(pal_bytes)
            .ok_or(Error::BadValue("palette size overflow"))?;
        if data.len() < pal_end {
            return Err(truncated("palette", pal_end, data.len()));
        }
        palette.reserve(count);
        for e in 0..count {
            let p = palette_at + e * 4;
            palette.push([data[p + 2], data[p + 1], data[p], 255]);
        }
        structures_end = pal_end;
    }

    if (data_offset as usize) < structures_end {
        return Err(Error::BadValue("pixel data offset inside headers"));
    }

    Ok(Header {
        info: Info {
            width,
            height,
            bits_per_pixel: bpp,
            compression,
            top_down,
            data_offset,
            dib_header_size: dib,
        },
        masks,
        palette,
    })
}

/// Reads only the headers, reporting what [`decode`] would see.
///
/// Every header validation `decode` performs runs here, so `info` on a
/// file `decode` would reject fails identically.
///
/// # Errors
///
/// The same [`Error`] conditions as [`decode`] minus those that only
/// pixel data can trigger.
pub fn info(data: &[u8]) -> Result<Info> {
    Ok(parse_header(data)?.info)
}

/// Decodes a whole BMP file into an [`Image<Rgba, u8>`] with a
/// top-left origin, whatever the file's row direction was.
///
/// # Errors
///
/// See the crate docs for the full tolerance table: malformed
/// structure is [`Error::Truncated`]/[`Error::BadValue`]; valid BMP
/// variants outside scope are [`Error::Unsupported`]; a declared image
/// over the raster buffer ceiling is [`Error::TooLarge`].
pub fn decode(data: &[u8]) -> Result<Image<Rgba, u8>> {
    let h = parse_header(data)?;
    let mut img =
        Image::<Rgba, u8>::new(h.info.width, h.info.height).map_err(|_| Error::TooLarge {
            what: "image buffer",
            limit: 1 << 30,
        })?;
    match h.info.compression {
        Compression::Rle8 => decode_rle8(data, &h, &mut img),
        _ => decode_uncompressed(data, &h, &mut img),
    }?;
    Ok(img)
}

/// Fills `img` from uncompressed 24/32-bit rows, honoring row direction
/// and the 4-byte row alignment.
fn decode_uncompressed(data: &[u8], h: &Header, img: &mut Image<Rgba, u8>) -> Result<()> {
    let w = h.info.width as usize;
    let height = h.info.height as usize;
    let bpp_bytes = (h.info.bits_per_pixel / 8) as usize;
    // Rows are padded to a multiple of 4 bytes. u64 keeps w*4+4 in
    // range for any width the format can declare.
    let stride = ((w as u64 * bpp_bytes as u64 + 3) & !3) as usize;
    let data_offset = h.info.data_offset as usize;
    if data_offset > data.len() {
        return Err(truncated("pixel data", data_offset, data.len()));
    }
    // The last row needs only its used bytes; final padding is optional.
    let needed = stride
        .checked_mul(height - 1)
        .and_then(|s| s.checked_add(w * bpp_bytes))
        .ok_or(Error::BadValue("pixel data size overflow"))?;
    let available = data.len() - data_offset;
    if available < needed {
        return Err(truncated("pixel data", needed, available));
    }

    // 32-bit only: declared channel masks, or the implicit B G R x
    // layout for BI_RGB (alpha absent → opaque).
    let alpha_mask = if h.masks[3] != 0 {
        Some(Mask::new(h.masks[3])?)
    } else {
        None
    };
    let masks = if h.info.bits_per_pixel == 32 {
        [
            Mask::new(h.masks[0])?,
            Mask::new(h.masks[1])?,
            Mask::new(h.masks[2])?,
        ]
    } else {
        [Mask {
            mask: 0,
            shift: 0,
            max: 1,
        }; 3]
    };

    let px = img.as_mut_slice();
    for row in 0..height {
        // Bottom-up files store the last image row first.
        let file_row = if h.info.top_down {
            row
        } else {
            height - 1 - row
        };
        let base = data_offset + file_row * stride;
        for x in 0..w {
            let p = base + x * bpp_bytes;
            let (r, g, b, a) = if bpp_bytes == 3 {
                (data[p + 2], data[p + 1], data[p], 255)
            } else {
                let v = u32::from_le_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]);
                (
                    masks[0].extract(v),
                    masks[1].extract(v),
                    masks[2].extract(v),
                    alpha_mask.map_or(255, |m| m.extract(v)),
                )
            };
            let d = (row * w + x) * 4;
            px[d] = r;
            px[d + 1] = g;
            px[d + 2] = b;
            px[d + 3] = a;
        }
    }
    Ok(())
}

/// Fills `img` from an RLE8 stream. `x`/`y` are file-space coordinates:
/// `y` counts up from the bottom of the image, matching how every RLE8
/// encoder writes. Unwritten pixels keep palette index 0.
fn decode_rle8(data: &[u8], h: &Header, img: &mut Image<Rgba, u8>) -> Result<()> {
    let w = h.info.width as usize;
    let height = h.info.height as usize;
    let start = h.info.data_offset as usize;
    if start > data.len() {
        return Err(truncated("rle data", start, data.len()));
    }
    // Pixels the stream never reaches keep the color of palette index
    // 0 — the format's background — not transparent black.
    if let Some(bg) = h.palette.first() {
        for p in img.as_mut_slice().chunks_exact_mut(4) {
            p.copy_from_slice(bg);
        }
    }
    let stream = &data[start..];
    let mut i = 0usize;
    let mut x = 0usize;
    let mut y = 0usize;

    let px = img.as_mut_slice();
    let put = |px: &mut [u8], x: usize, y: usize, idx: u8| -> Result<()> {
        if x >= w || y >= height {
            return Err(Error::BadValue("rle write past image edge"));
        }
        let color = h
            .palette
            .get(idx as usize)
            .ok_or(Error::BadValue("palette index out of range"))?;
        // File y counts from the bottom; image y from the top.
        let d = ((height - 1 - y) * w + x) * 4;
        px[d..d + 4].copy_from_slice(color);
        Ok(())
    };

    while i < stream.len() {
        let count = stream[i];
        let val = stream
            .get(i + 1)
            .copied()
            .ok_or_else(|| truncated("rle stream", 2, stream.len() - i))?;
        i += 2;
        if count != 0 {
            // Encoded mode: `count` pixels of palette index `val`.
            for _ in 0..count {
                put(px, x, y, val)?;
                x += 1;
            }
            continue;
        }
        match val {
            // End of line.
            0 => {
                x = 0;
                y += 1;
            }
            // End of bitmap. Everything after it is ignored.
            1 => break,
            // Delta: jump right `dx` and up `dy`.
            2 => {
                let dx = *stream
                    .get(i)
                    .ok_or_else(|| truncated("rle delta", 2, stream.len() - i))?;
                let dy = *stream
                    .get(i + 1)
                    .ok_or_else(|| truncated("rle delta", 2, stream.len() - i))?;
                i += 2;
                x += dx as usize;
                y += dy as usize;
                if x > w || y > height {
                    return Err(Error::BadValue("rle delta out of bounds"));
                }
            }
            // Absolute mode: `val` raw palette indices, padded to even.
            n => {
                if stream.len() - i < n as usize {
                    return Err(truncated("rle absolute run", n as usize, stream.len() - i));
                }
                for _ in 0..n {
                    put(px, x, y, stream[i])?;
                    x += 1;
                    i += 1;
                }
                // Odd absolute runs carry a pad byte. A stream that
                // simply ends is still accepted on the next pass.
                if n % 2 == 1 && i < stream.len() {
                    i += 1;
                }
            }
        }
    }
    // A stream that runs out without an EOF escape decodes as EOF.
    Ok(())
}

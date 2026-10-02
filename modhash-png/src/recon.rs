//! Scanline reconstruction: unfiltering, Adam7 interlace geometry,
//! sub-byte sample extraction and the mapping from PNG colour types to
//! the concrete [`Image`] layouts.

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};
use modhash_raster::Image;

use crate::{Ihdr, Limits, Parsed, Pixels, Png};

/// Adam7 pass geometry (specification section 8): `(x_origin, y_origin,
/// x_step, y_step)` for each of the seven passes, in pass order.
const ADAM7: [(u32, u32, u32, u32); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

/// Samples per pixel the *file* codes, before `tRNS` expansion.
fn channels(colour_type: u8) -> usize {
    match colour_type {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => unreachable!("colour type validated in IHDR"),
    }
}

/// One pass's pixel count along an axis: pixels `origin + k·step` that
/// stay below `extent`, zero when the pass starts past the image edge.
fn pass_dim(origin: u32, step: u32, extent: u32) -> u32 {
    extent.saturating_sub(origin).saturating_add(step - 1) / step
}

/// Reconstructed geometry `(pass_w, pass_h, row_bytes, total_bytes)` for
/// one pass — or the whole image treated as a single `(0, 0, 1, 1)`
/// pass. `total_bytes` includes each row's filter tag. A pass with zero
/// width *or* zero height contributes no scanlines at all: there is no
/// filter byte for a row that does not exist (specification section 8).
fn pass_shape(ihdr: &Ihdr, pass: usize) -> (u32, u32, usize, usize) {
    let (w, h) = if ihdr.interlaced {
        let (x0, y0, dx, dy) = ADAM7[pass];
        (pass_dim(x0, dx, ihdr.width), pass_dim(y0, dy, ihdr.height))
    } else {
        (ihdr.width, ihdr.height)
    };
    let bits = (w as u128) * (ihdr.bit_depth as u128) * (channels(ihdr.colour_type) as u128);
    let row_bytes = (bits.div_ceil(8)) as usize;
    let total = if w == 0 || h == 0 {
        0
    } else {
        (h as usize).saturating_mul(row_bytes + 1)
    };
    (w, h, row_bytes, total)
}

/// The transparency `tRNS` declares, decoded once per file
/// (specification section 11.3.2).
enum Trns<'a> {
    /// No `tRNS` chunk.
    None,
    /// Greyscale: the transparent value as a raw coded-domain sample.
    Value(u16),
    /// Indexed: the per-entry alpha table.
    Alpha(&'a [u8]),
    /// Truecolour: the transparent colour, three raw `u16` samples.
    Rgb(u16, u16, u16),
}

fn trns_of<'a>(ihdr: &Ihdr, trns: &'a Option<Vec<u8>>) -> Result<Trns<'a>> {
    let body = match trns {
        None => return Ok(Trns::None),
        Some(b) => b.as_slice(),
    };
    let u16_at = |i: usize| u16::from_be_bytes([body[i], body[i + 1]]);
    Ok(match ihdr.colour_type {
        0 => Trns::Value(u16_at(0)),
        2 => Trns::Rgb(u16_at(0), u16_at(2), u16_at(4)),
        3 => Trns::Alpha(body),
        _ => return Err(Error::BadValue("PNG tRNS in alpha image")),
    })
}

/// The concrete buffer kind a file decodes into.
enum Target {
    Gray8(Vec<u8>),
    Gray16(Vec<u16>),
    Rgb8(Vec<u8>),
    Rgb16(Vec<u16>),
    Rgba8(Vec<u8>),
    Rgba16(Vec<u16>),
}

/// Which target the file's colour type, depth and `tRNS` map to, with
/// the buffer allocated (zeroed) up front. Palette entries arrive as
/// `Rgb8`/`Rgba8`; grey+alpha widens to `Rgba`; a `tRNS` on colour type
/// 0 or 2 likewise produces `Rgba`. Sub-byte greyscale scales to `u8`.
fn target_of(ihdr: &Ihdr, trns: &Trns) -> Result<Target> {
    let npix = (ihdr.width as usize)
        .checked_mul(ihdr.height as usize)
        .ok_or(Error::BadValue("PNG dimension"))?;
    let target = match (ihdr.colour_type, ihdr.bit_depth, trns) {
        (0, 16, Trns::None) => Target::Gray16(alloc::vec![0; npix]),
        (0, _, Trns::None) => Target::Gray8(alloc::vec![0; npix]),
        (0, 16, Trns::Value(_)) => Target::Rgba16(alloc::vec![0; npix * 4]),
        (0, _, Trns::Value(_)) => Target::Rgba8(alloc::vec![0; npix * 4]),
        (2, 16, Trns::None) => Target::Rgb16(alloc::vec![0; npix * 3]),
        (2, _, Trns::None) => Target::Rgb8(alloc::vec![0; npix * 3]),
        (2, 16, Trns::Rgb(..)) => Target::Rgba16(alloc::vec![0; npix * 4]),
        (2, _, Trns::Rgb(..)) => Target::Rgba8(alloc::vec![0; npix * 4]),
        (3, _, Trns::None) => Target::Rgb8(alloc::vec![0; npix * 3]),
        (3, _, Trns::Alpha(_)) => Target::Rgba8(alloc::vec![0; npix * 4]),
        (4, 16, _) => Target::Rgba16(alloc::vec![0; npix * 4]),
        (4, _, _) => Target::Rgba8(alloc::vec![0; npix * 4]),
        (6, 16, _) => Target::Rgba16(alloc::vec![0; npix * 4]),
        (6, _, _) => Target::Rgba8(alloc::vec![0; npix * 4]),
        _ => unreachable!("colour type/tRNS combination validated earlier"),
    };
    Ok(target)
}

/// Decodes the parsed chunk set into pixels: computes the exact inflated
/// size from `IHDR` (before a byte of `IDAT` is decompressed), inflates,
/// unfilters and maps into the concrete [`Pixels`] buffer.
pub(crate) fn decode_image(parsed: &Parsed, limits: &Limits) -> Result<Png> {
    let ihdr = &parsed.ihdr;
    let passes = if ihdr.interlaced { 7 } else { 1 };

    let mut expected = 0usize;
    for p in 0..passes {
        let (_, _, _, total) = pass_shape(ihdr, p);
        expected = expected
            .checked_add(total)
            .ok_or(Error::BadValue("PNG scanline size"))?;
    }
    if expected > limits.max_output {
        return Err(Error::too_large("PNG image data", limits.max_output));
    }

    let raw = modhash_inflate::inflate_zlib(
        &parsed.data,
        &modhash_inflate::Limits {
            max_input: limits.max_input,
            max_output: limits.max_output,
        },
    )?;
    if raw.len() != expected {
        return Err(Error::BadValue("PNG decompressed length"));
    }

    let trns = trns_of(ihdr, &parsed.trns)?;
    let mut target = target_of(ihdr, &trns)?;

    // Unfilter pass by pass; each pass is an independent filtered image
    // whose reconstruction stride is the pixel's whole bytes.
    let depth = ihdr.bit_depth;
    let ch = channels(ihdr.colour_type);
    let bpp = (ch * depth as usize).div_ceil(8);
    let palette = parsed.palette.as_deref();
    let mut scratch: Vec<u16> = Vec::new();
    let mut off = 0usize;
    for p in 0..passes {
        let (pw, ph, row_bytes, _) = pass_shape(ihdr, p);
        if pw == 0 || ph == 0 {
            continue;
        }
        let mut prior = alloc::vec![0u8; row_bytes];
        let mut recon = alloc::vec![0u8; row_bytes];
        for ry in 0..ph {
            // The expected-length check above guarantees these slices
            // exist; `get` keeps even a sizing regression an `Err`
            // rather than a panic.
            let row = raw
                .get(off..off + 1 + row_bytes)
                .ok_or(Error::BadValue("PNG decompressed length"))?;
            let filter = row[0];
            recon.copy_from_slice(&row[1..]);
            off += 1 + row_bytes;
            unfilter(filter, bpp, &prior, &mut recon)?;
            emit_row(
                ihdr,
                &trns,
                palette,
                &mut target,
                &mut scratch,
                &recon,
                p,
                pw,
                ry,
            )?;
            prior.copy_from_slice(&recon);
        }
    }
    debug_assert_eq!(off, raw.len());

    let pixels = match target {
        Target::Gray8(v) => Pixels::Gray8(finish(ihdr, v)?),
        Target::Gray16(v) => Pixels::Gray16(finish(ihdr, v)?),
        Target::Rgb8(v) => Pixels::Rgb8(finish(ihdr, v)?),
        Target::Rgb16(v) => Pixels::Rgb16(finish(ihdr, v)?),
        Target::Rgba8(v) => Pixels::Rgba8(finish(ihdr, v)?),
        Target::Rgba16(v) => Pixels::Rgba16(finish(ihdr, v)?),
    };
    Ok(Png {
        width: ihdr.width,
        height: ihdr.height,
        bit_depth: ihdr.bit_depth,
        colour_type: ihdr.colour_type,
        interlaced: ihdr.interlaced,
        pixels,
    })
}

/// `Image::from_vec` wrapper naming the buffer it finishes.
fn finish<L: modhash_raster::Layout, T: modhash_raster::Sample>(
    ihdr: &Ihdr,
    v: Vec<T>,
) -> Result<Image<L, T>> {
    Image::from_vec(ihdr.width, ihdr.height, v)
}

/// Reconstructs one scanline in place (specification section 6, filters
/// 0-4). `bpp` is the filter stride in whole bytes; `prior` is the
/// previous reconstructed row of the same pass (zeroed on the first).
fn unfilter(f: u8, bpp: usize, prior: &[u8], recon: &mut [u8]) -> Result<()> {
    match f {
        0 => {}
        1 => {
            for x in bpp..recon.len() {
                recon[x] = recon[x].wrapping_add(recon[x - bpp]);
            }
        }
        2 => {
            for (r, p) in recon.iter_mut().zip(prior.iter()) {
                *r = r.wrapping_add(*p);
            }
        }
        3 => {
            for x in 0..recon.len() {
                let a = if x >= bpp { recon[x - bpp] } else { 0 };
                let b = prior[x];
                recon[x] = recon[x].wrapping_add(((a as u16 + b as u16) / 2) as u8);
            }
        }
        4 => {
            for x in 0..recon.len() {
                let a = if x >= bpp { recon[x - bpp] } else { 0 };
                let b = prior[x];
                let c = if x >= bpp { prior[x - bpp] } else { 0 };
                recon[x] = recon[x].wrapping_add(paeth(a, b, c));
            }
        }
        _ => return Err(Error::BadValue("PNG filter type")),
    }
    Ok(())
}

/// The Paeth predictor (specification section 6.4): nearest of `a`,
/// `b`, `c` to `a + b - c`; ties resolve to `a`, then `b`, then `c`.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

/// Extracts `w * ch` samples of `depth` bits each from one reconstructed
/// row into `scratch`. Depths 1/2/4 are packed MSB-first within each
/// byte; depth 16 is big-endian. `scratch` is resized by the caller.
fn unpack_row(depth: u8, ch: usize, w: u32, row: &[u8], out: &mut [u16]) {
    let count = (w as usize) * ch;
    match depth {
        16 => {
            for i in 0..count {
                out[i] = u16::from_be_bytes([row[2 * i], row[2 * i + 1]]);
            }
        }
        8 => {
            for i in 0..count {
                out[i] = row[i] as u16;
            }
        }
        _ => {
            let mask = (1u16 << depth) - 1;
            let d = depth as usize;
            for (i, slot) in out.iter_mut().enumerate().take(count) {
                let bit = i * d;
                *slot = ((row[bit >> 3] as u16) >> (8 - d - (bit & 7))) & mask;
            }
        }
    }
}

/// Scales a sub-8-bit sample to 8 bits by bit replication — the mapping
/// the specification's depth-scaling note produces for `1/2/4 → 8`.
fn scale8(v: u16, depth: u8) -> u16 {
    match depth {
        1 => v * 255,
        2 => v * 85,
        4 => v * 17,
        _ => v,
    }
}

/// Writes one reconstructed row into the target buffer, applying the
/// palette, `tRNS` and depth scaling. `pass`/`ry`/`pw` locate the row in
/// the final image through the Adam7 geometry (identity when the file is
/// not interlaced).
#[allow(clippy::too_many_arguments)]
fn emit_row(
    ihdr: &Ihdr,
    trns: &Trns,
    palette: Option<&[u8]>,
    target: &mut Target,
    scratch: &mut Vec<u16>,
    recon: &[u8],
    pass: usize,
    pw: u32,
    ry: u32,
) -> Result<()> {
    let depth = ihdr.bit_depth;
    let ch = channels(ihdr.colour_type);
    let (x0, y0, dx, dy) = if ihdr.interlaced {
        ADAM7[pass]
    } else {
        (0, 0, 1, 1)
    };
    let y = y0 + ry * dy;

    scratch.resize((pw as usize) * ch, 0);
    unpack_row(depth, ch, pw, recon, scratch);
    let samples: &[u16] = scratch;

    for rx in 0..pw as usize {
        let x = x0 + rx as u32 * dx;
        let base = (y as usize) * (ihdr.width as usize) + (x as usize);
        let px = &samples[rx * ch..rx * ch + ch];
        match target {
            Target::Gray16(v) => v[base] = px[0],
            Target::Gray8(v) => v[base] = scale8(px[0], depth) as u8,
            Target::Rgb16(v) => {
                v[base * 3..base * 3 + 3].copy_from_slice(&px[..3]);
            }
            Target::Rgb8(v) => {
                let (r, g, b) = rgb8(ihdr.colour_type, px, palette)?;
                v[base * 3] = r;
                v[base * 3 + 1] = g;
                v[base * 3 + 2] = b;
            }
            Target::Rgba16(v) => {
                let (r, g, b, a) = rgba16(ihdr, px, trns, palette)?;
                v[base * 4..base * 4 + 4].copy_from_slice(&[r, g, b, a]);
            }
            Target::Rgba8(v) => {
                let (r, g, b, a) = rgba8(ihdr, px, trns, palette)?;
                v[base * 4] = r;
                v[base * 4 + 1] = g;
                v[base * 4 + 2] = b;
                v[base * 4 + 3] = a;
            }
        }
    }
    Ok(())
}

/// 8-bit RGB resolution for truecolour and palette pixels.
fn rgb8(colour_type: u8, px: &[u16], palette: Option<&[u8]>) -> Result<(u8, u8, u8)> {
    match colour_type {
        2 => Ok((px[0] as u8, px[1] as u8, px[2] as u8)),
        3 => {
            let idx = px[0] as usize;
            let pal = palette.ok_or(Error::BadValue("PNG indexed image missing PLTE"))?;
            if idx * 3 + 2 >= pal.len() {
                return Err(Error::BadValue("PNG palette index out of range"));
            }
            Ok((pal[idx * 3], pal[idx * 3 + 1], pal[idx * 3 + 2]))
        }
        _ => unreachable!("rgb8 called for non-RGB colour type"),
    }
}

/// 8-bit RGBA resolution: colour types 0, 2, 3, 4 and 6 at depths below
/// 16 all land here when the target carries alpha. `tRNS` comparisons
/// happen on raw coded-domain samples, per specification 11.3.2.
fn rgba8(ihdr: &Ihdr, px: &[u16], trns: &Trns, palette: Option<&[u8]>) -> Result<(u8, u8, u8, u8)> {
    match ihdr.colour_type {
        0 => {
            let g = scale8(px[0], ihdr.bit_depth) as u8;
            let a = match trns {
                Trns::Value(tv) if px[0] == *tv => 0,
                _ => 255,
            };
            Ok((g, g, g, a))
        }
        2 => {
            let a = match trns {
                Trns::Rgb(tr, tg, tb) if px[0] == *tr && px[1] == *tg && px[2] == *tb => 0,
                _ => 255,
            };
            Ok((px[0] as u8, px[1] as u8, px[2] as u8, a))
        }
        3 => {
            let (r, g, b) = rgb8(3, px, palette)?;
            let a = match trns {
                Trns::Alpha(t) => t.get(px[0] as usize).copied().unwrap_or(255),
                _ => 255,
            };
            Ok((r, g, b, a))
        }
        4 => Ok((px[0] as u8, px[0] as u8, px[0] as u8, px[1] as u8)),
        6 => Ok((px[0] as u8, px[1] as u8, px[2] as u8, px[3] as u8)),
        _ => unreachable!("rgba8 called for incompatible colour type"),
    }
}

/// 16-bit RGBA resolution: colour types 0, 2, 4 and 6 at depth 16, with
/// `tRNS` raw-value comparison.
fn rgba16(
    ihdr: &Ihdr,
    px: &[u16],
    trns: &Trns,
    palette: Option<&[u8]>,
) -> Result<(u16, u16, u16, u16)> {
    let _ = palette;
    match ihdr.colour_type {
        0 => {
            let a = match trns {
                Trns::Value(tv) if px[0] == *tv => 0,
                _ => 0xffff,
            };
            Ok((px[0], px[0], px[0], a))
        }
        2 => {
            let a = match trns {
                Trns::Rgb(tr, tg, tb) if px[0] == *tr && px[1] == *tg && px[2] == *tb => 0,
                _ => 0xffff,
            };
            Ok((px[0], px[1], px[2], a))
        }
        4 => Ok((px[0], px[0], px[0], px[1])),
        6 => Ok((px[0], px[1], px[2], px[3])),
        _ => unreachable!("rgba16 called for incompatible colour type"),
    }
}

//! Minimal PNG encoder — **test scaffolding only**.
//!
//! The decoder needs fixtures and round-trip partners; this encoder is
//! deliberately the smallest correct writer: non-interlaced, one chosen
//! row filter for every row, and zlib streams made of uncompressed
//! (stored) DEFLATE blocks, so nothing here pretends to compress. It is
//! public because the workspace's in-crate tests live in `tests/` and
//! cannot see private items; callers should treat it as scaffolding,
//! not as a real encoder (no compression, no filter choice, no Adam7).

use alloc::vec::Vec;

use modhash_primitives::{Error, Result, crc32};

use crate::{Pixels, SIGNATURE};

/// Encodes `pixels` as a non-interlaced PNG with every row coded by
/// `filter` (0-4) and the image data wrapped in stored DEFLATE blocks.
///
/// Acceptable inputs are exactly the six [`Pixels`] variants, mapping to
/// colour types 0, 2 and 6 at depth 8 or 16. Any `filter` above 4 is
/// [`Error::BadValue`]; unlike the decoder this function is scaffolding
/// and does not synthesise palettes or `tRNS`.
pub fn encode(pixels: &Pixels, filter: u8) -> Result<Vec<u8>> {
    if filter > 4 {
        return Err(Error::BadValue("PNG filter type"));
    }
    let (width, height, depth, colour, raw) = serialize(pixels, filter);

    let mut out = Vec::new();
    out.extend_from_slice(SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(depth);
    ihdr.push(colour);
    ihdr.push(0); // compression: deflate
    ihdr.push(0); // filter method: adaptive
    ihdr.push(0); // interlace: none — scaffolding
    put_chunk(&mut out, b"IHDR", &ihdr);

    put_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    put_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// Serializes the pixels to filtered scanline bytes and reports the
/// `IHDR` fields. Rows carry one filter byte then the filtered
/// big-endian sample stream.
fn serialize(pixels: &Pixels, filter: u8) -> (u32, u32, u8, u8, Vec<u8>) {
    let (w, h, depth, colour, ch, buf) = match pixels {
        Pixels::Gray8(i) => (
            i.width(),
            i.height(),
            8u8,
            0u8,
            1usize,
            to_bytes_u8(i.as_slice()),
        ),
        Pixels::Gray16(i) => (i.width(), i.height(), 16, 0, 1, to_bytes_u16(i.as_slice())),
        Pixels::Rgb8(i) => (i.width(), i.height(), 8, 2, 3, to_bytes_u8(i.as_slice())),
        Pixels::Rgb16(i) => (i.width(), i.height(), 16, 2, 3, to_bytes_u16(i.as_slice())),
        Pixels::Rgba8(i) => (i.width(), i.height(), 8, 6, 4, to_bytes_u8(i.as_slice())),
        Pixels::Rgba16(i) => (i.width(), i.height(), 16, 6, 4, to_bytes_u16(i.as_slice())),
    };
    let sample_bytes = depth as usize / 8;
    let row_bytes = (w as usize) * ch * sample_bytes;
    let bpp = ch * sample_bytes;
    let mut out = Vec::with_capacity((row_bytes + 1) * h as usize);
    let mut prior = alloc::vec![0u8; row_bytes];
    let mut coded = alloc::vec![0u8; row_bytes];
    for y in 0..h as usize {
        let row = &buf[y * row_bytes..(y + 1) * row_bytes];
        out.push(filter);
        apply_filter(filter, bpp, &prior, row, &mut coded);
        out.extend_from_slice(&coded);
        prior.copy_from_slice(row);
    }
    (w, h, depth, colour, out)
}

fn to_bytes_u8(v: &[u8]) -> Vec<u8> {
    v.to_vec()
}

fn to_bytes_u16(v: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    for s in v {
        out.extend_from_slice(&s.to_be_bytes());
    }
    out
}

/// Inverse of the decoder's reconstruction for filters 0-4: produces
/// the bytes a decoder must undo to recover `row`.
fn apply_filter(f: u8, bpp: usize, prior: &[u8], row: &[u8], coded: &mut [u8]) {
    match f {
        0 => coded.copy_from_slice(row),
        1 => {
            for x in 0..row.len() {
                let a = if x >= bpp { row[x - bpp] } else { 0 };
                coded[x] = row[x].wrapping_sub(a);
            }
        }
        2 => {
            for x in 0..row.len() {
                coded[x] = row[x].wrapping_sub(prior[x]);
            }
        }
        3 => {
            for x in 0..row.len() {
                let a = if x >= bpp { row[x - bpp] } else { 0 };
                let b = prior[x];
                coded[x] = row[x].wrapping_sub(((a as u16 + b as u16) / 2) as u8);
            }
        }
        4 => {
            for x in 0..row.len() {
                let a = if x >= bpp { row[x - bpp] } else { 0 };
                let b = prior[x];
                let c = if x >= bpp { prior[x - bpp] } else { 0 };
                coded[x] = row[x].wrapping_sub(paeth_enc(a, b, c));
            }
        }
        _ => unreachable!("filter validated by caller"),
    }
}

/// Paeth predictor, duplicated from `recon` so the encoder is
/// self-contained scaffolding the decoder never depends on.
fn paeth_enc(a: u8, b: u8, c: u8) -> u8 {
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

/// Wraps `data` in a zlib stream of stored (uncompressed) DEFLATE
/// blocks: zlib header `0x78 0x01` (deflate, 32 KiB window, the FCHECK
/// pair for "no compression"), blocks of at most 65 535 bytes, Adler-32
/// trailer.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let blocks = data.len() / 65_535 + 1;
    let mut out = Vec::with_capacity(data.len() + blocks * 5 + 6);
    out.push(0x78);
    out.push(0x01);
    let mut at = 0usize;
    loop {
        let take = (data.len() - at).min(65_535);
        let last = at + take == data.len();
        out.push(if last { 0x01 } else { 0x00 });
        let len = take as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(&data[at..at + take]);
        at += take;
        if last {
            break;
        }
    }
    out.extend_from_slice(&modhash_inflate::adler32(data).to_be_bytes());
    out
}

/// Appends one framed chunk: length, type, data, CRC-32 over type+data.
fn put_chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(ty);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

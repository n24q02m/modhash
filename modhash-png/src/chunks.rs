//! Chunk walking: signature check, per-chunk CRC-32 verification, the
//! ordering rules of PNG specification sections 5 and 11, and field
//! validation for `IHDR`, `PLTE` and `tRNS`.

use alloc::vec::Vec;

use modhash_primitives::{Error, Result, crc32};

use crate::{Ihdr, Parsed, SIGNATURE};

/// Returns the data field of the first chunk of `input`, which must be
/// the expected `want` type (currently only `IHDR`). Checks the
/// signature, the chunk framing and the chunk's CRC-32.
pub(crate) fn first_chunk<'a>(input: &'a [u8], want: &[u8; 4]) -> Result<&'a [u8]> {
    if input.len() < 8 || &input[..8] != SIGNATURE {
        return Err(Error::InvalidMagic {
            what: "PNG signature",
        });
    }
    let mut chunks = Chunks::new(&input[8..]);
    let (ty, data) = chunks
        .next_chunk()?
        .ok_or_else(|| Error::truncated("first PNG chunk", 12, input.len().saturating_sub(8)))?;
    if ty != want {
        return Err(Error::BadValue("PNG first chunk is not IHDR"));
    }
    Ok(data)
}

/// Sequential reader over a PNG byte stream's chunk section. Each
/// successful step verifies the chunk's CRC-32 before handing the data
/// field out: a corrupted chunk is never visible to a caller.
struct Chunks<'a> {
    rest: &'a [u8],
}

impl<'a> Chunks<'a> {
    fn new(rest: &'a [u8]) -> Self {
        Chunks { rest }
    }

    /// Bytes not yet consumed. Non-zero after `IEND` means trailing
    /// garbage the decoder refuses to ignore.
    fn remaining(&self) -> usize {
        self.rest.len()
    }

    /// Reads the next `(type, data)` pair, or `None` at exact end of
    /// input. The chunk type's four bytes must be ASCII letters with the
    /// reserved (third) byte uppercase, per specification section 5.4.
    fn next_chunk(&mut self) -> Result<Option<(&'a [u8], &'a [u8])>> {
        if self.rest.is_empty() {
            return Ok(None);
        }
        if self.rest.len() < 8 {
            return Err(Error::truncated("PNG chunk header", 8, self.rest.len()));
        }
        let len = u32::from_be_bytes([self.rest[0], self.rest[1], self.rest[2], self.rest[3]]);
        let ty = &self.rest[4..8];
        if !ty.iter().all(|b| b.is_ascii_alphabetic()) || !ty[2].is_ascii_uppercase() {
            return Err(Error::BadValue("PNG chunk type"));
        }
        let total = (len as usize)
            .checked_add(12)
            .ok_or(Error::BadValue("PNG chunk length"))?;
        if self.rest.len() < total {
            return Err(Error::truncated("PNG chunk", total, self.rest.len()));
        }
        let data = &self.rest[8..8 + len as usize];
        let stored = u32::from_be_bytes([
            self.rest[8 + len as usize],
            self.rest[9 + len as usize],
            self.rest[10 + len as usize],
            self.rest[11 + len as usize],
        ]);
        // The CRC covers type bytes plus data — exactly what a caller
        // would have hashed anyway, so checking costs one pass.
        let mut crc_input = Vec::with_capacity(4 + len as usize);
        crc_input.extend_from_slice(ty);
        crc_input.extend_from_slice(data);
        if crc32(&crc_input) != stored {
            return Err(Error::BadValue(crc_what(ty)));
        }
        self.rest = &self.rest[total..];
        Ok(Some((ty, data)))
    }
}

/// Names a chunk for the CRC-failure error. The `Error` variants carry
/// `&'static str`, so the four type bytes are mapped to the chunks the
/// decoder knows and a generic phrase for everything else.
fn crc_what(ty: &[u8]) -> &'static str {
    match ty {
        b"IHDR" => "PNG IHDR CRC32 mismatch",
        b"PLTE" => "PNG PLTE CRC32 mismatch",
        b"IDAT" => "PNG IDAT CRC32 mismatch",
        b"IEND" => "PNG IEND CRC32 mismatch",
        b"tRNS" => "PNG tRNS CRC32 mismatch",
        _ => "PNG chunk CRC32 mismatch",
    }
}

/// Validates the 13 `IHDR` bytes per specification section 11.2.2.
pub(crate) fn parse_ihdr(body: &[u8]) -> Result<Ihdr> {
    debug_assert!(body.len() == 13);
    let width = u32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    let height = u32::from_be_bytes([body[4], body[5], body[6], body[7]]);
    if width == 0 || height == 0 {
        return Err(Error::BadValue("PNG zero dimension"));
    }
    if width >= (1 << 31) || height >= (1 << 31) {
        return Err(Error::BadValue("PNG dimension exceeds 2^31-1"));
    }
    let bit_depth = body[8];
    let colour_type = body[9];
    let legal = match colour_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 | 4 | 6 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        _ => return Err(Error::BadValue("PNG colour type")),
    };
    if !legal {
        return Err(Error::BadValue("PNG bit depth / colour type combination"));
    }
    if body[10] != 0 {
        return Err(Error::BadValue("PNG compression method"));
    }
    if body[11] != 0 {
        return Err(Error::BadValue("PNG filter method"));
    }
    if body[12] > 1 {
        return Err(Error::BadValue("PNG interlace method"));
    }
    Ok(Ihdr {
        width,
        height,
        bit_depth,
        colour_type,
        interlaced: body[12] == 1,
    })
}

/// Walks every chunk of `input`, enforcing ordering and gathering the
/// payloads the decoder needs. `parsed.data` is the concatenated,
/// still-compressed `IDAT` payload.
pub(crate) fn parse(input: &[u8]) -> Result<Parsed> {
    if input.len() < 8 || &input[..8] != SIGNATURE {
        return Err(Error::InvalidMagic {
            what: "PNG signature",
        });
    }
    let mut chunks = Chunks::new(&input[8..]);

    let (ty, body) = chunks
        .next_chunk()?
        .ok_or_else(|| Error::truncated("PNG IHDR chunk", 12, 0))?;
    if ty != b"IHDR" {
        return Err(Error::BadValue("PNG first chunk is not IHDR"));
    }
    if body.len() != 13 {
        return Err(Error::BadValue("PNG IHDR length"));
    }
    let ihdr = parse_ihdr(body)?;

    let mut palette: Option<Vec<u8>> = None;
    let mut trns: Option<Vec<u8>> = None;
    let mut data: Vec<u8> = Vec::new();
    let mut saw_idat = false;
    let mut idat_ended = false;
    let mut saw_iend = false;

    while let Some((ty, body)) = chunks.next_chunk()? {
        match ty {
            b"IHDR" => return Err(Error::BadValue("PNG duplicate IHDR")),
            b"PLTE" => {
                if saw_idat {
                    return Err(Error::BadValue("PNG PLTE after IDAT"));
                }
                if palette.is_some() {
                    return Err(Error::BadValue("PNG duplicate PLTE"));
                }
                if ihdr.colour_type == 0 || ihdr.colour_type == 4 {
                    return Err(Error::BadValue("PNG PLTE in greyscale image"));
                }
                if body.is_empty() || body.len() % 3 != 0 || body.len() > 768 {
                    return Err(Error::BadValue("PNG PLTE length"));
                }
                palette = Some(body.to_vec());
            }
            b"tRNS" => {
                if saw_idat {
                    return Err(Error::BadValue("PNG tRNS after IDAT"));
                }
                if trns.is_some() {
                    return Err(Error::BadValue("PNG duplicate tRNS"));
                }
                match ihdr.colour_type {
                    4 | 6 => return Err(Error::BadValue("PNG tRNS in alpha image")),
                    0 => {
                        if body.len() != 2 {
                            return Err(Error::BadValue("PNG tRNS length"));
                        }
                    }
                    2 => {
                        if body.len() != 6 {
                            return Err(Error::BadValue("PNG tRNS length"));
                        }
                    }
                    3 => {
                        let entries = palette
                            .as_ref()
                            .ok_or(Error::BadValue("PNG tRNS before PLTE"))?
                            .len()
                            / 3;
                        if body.is_empty() || body.len() > entries {
                            return Err(Error::BadValue("PNG tRNS length"));
                        }
                    }
                    _ => unreachable!("colour type validated in IHDR"),
                }
                trns = Some(body.to_vec());
            }
            b"IDAT" => {
                if idat_ended {
                    return Err(Error::BadValue("PNG non-consecutive IDAT"));
                }
                if !saw_idat && ihdr.colour_type == 3 && palette.is_none() {
                    return Err(Error::BadValue("PNG indexed image missing PLTE"));
                }
                saw_idat = true;
                data.extend_from_slice(body);
            }
            b"IEND" => {
                if !body.is_empty() {
                    return Err(Error::BadValue("PNG IEND length"));
                }
                saw_iend = true;
                break;
            }
            _ => {
                if saw_idat {
                    idat_ended = true;
                }
                if ty[0].is_ascii_uppercase() {
                    return Err(Error::Unsupported("PNG unknown critical chunk"));
                }
                // Ancillary chunk: skipped by length, already CRC-checked.
            }
        }
    }

    if !saw_iend {
        if chunks.remaining() != 0 {
            return Err(Error::truncated(
                "PNG IEND chunk",
                chunks.remaining() + 12,
                chunks.remaining(),
            ));
        }
        return Err(Error::BadValue("PNG missing IEND"));
    }
    if chunks.remaining() != 0 {
        return Err(Error::BadValue("PNG trailing bytes after IEND"));
    }
    if !saw_idat {
        return Err(Error::BadValue("PNG missing IDAT"));
    }
    Ok(Parsed {
        ihdr,
        palette,
        trns,
        data,
    })
}

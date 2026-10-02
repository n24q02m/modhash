//! MSB-first bit reader with Exp-Golomb (`ue`/`se`/`te`/`me`) decoding.
//!
//! The reader keeps a 64-bit lookahead buffer so VLC decoding can peek a
//! fixed window before committing. Every read bounds-checks: truncation
//! and oversized code words surface as [`Error`], never as a panic or a
//! wrapped shift. H.264's Exp-Golomb `ue(v)` is unbounded in theory, but
//! the syntax caps every use well below 32 bits, so [`Br::ue`] rejects
//! prefixes longer than 32 zeros.

use modhash_primitives::{Error, Result};

/// Bit-level reader over an RBSP byte slice (emulation prevention already
/// removed). Bit order is the H.264 convention: bit 0 is the MSB of
/// byte 0.
#[derive(Debug)]
pub(crate) struct Br<'a> {
    data: &'a [u8],
    /// Index of the next bit to read, counting from the MSB of byte 0.
    pos: usize,
}

impl<'a> Br<'a> {
    /// Creates a reader over `data`, positioned at bit 0.
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Br { data, pos: 0 }
    }

    /// Bits still readable.
    pub(crate) fn remaining(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    /// `true` when positioned on a byte boundary.
    pub(crate) fn is_byte_aligned(&self) -> bool {
        self.pos % 8 == 0
    }

    /// Discards bits up to the next byte boundary.
    pub(crate) fn byte_align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    /// Bit position (test-only trace helper).
    #[allow(dead_code)]
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    /// `true` when the only bits left are the RBSP stop bit and its
    /// alignment zeros — i.e. `more_rbsp_data()` is false. The tail may
    /// also be completely empty or all-zero.
    pub(crate) fn no_more_rbsp_data(&self) -> bool {
        let rem = self.remaining();
        if rem == 0 {
            return true;
        }
        // Remaining bits are the stop-bit pattern when there is exactly
        // one set bit left and it is the first remaining bit.
        let first_byte = self.pos / 8;
        let mut seen_one = false;
        for i in self.pos..self.data.len() * 8 {
            let bit = (self.data[i / 8] >> (7 - i % 8)) & 1;
            if bit == 1 {
                if seen_one || i != self.pos {
                    return false;
                }
                seen_one = true;
            }
        }
        let _ = first_byte;
        // A single 1 at the start, or all zeros: only trailing remains.
        true
    }

    /// Peeks the next `n` bits (`n <= 32`) without consuming them.
    /// The first stream bit is the MSB of the returned value.
    pub(crate) fn peek(&self, n: usize) -> Result<u32> {
        if n > 32 {
            return Err(Error::BadValue("peek width over 32"));
        }
        if n > self.remaining() {
            return Err(Error::truncated("peek bits", n, self.remaining()));
        }
        let mut v = 0u32;
        for i in 0..n {
            let bit = (self.data[(self.pos + i) / 8] >> (7 - (self.pos + i) % 8)) & 1;
            v = (v << 1) | u32::from(bit);
        }
        Ok(v)
    }

    /// Reads `n` bits (`n <= 32`), first stream bit = result MSB.
    pub(crate) fn bits(&mut self, n: usize) -> Result<u32> {
        let v = self.peek(n)?;
        self.pos += n;
        Ok(v)
    }

    /// Reads one bit.
    pub(crate) fn bit(&mut self) -> Result<bool> {
        Ok(self.bits(1)? != 0)
    }

    /// Reads one whole byte at an aligned position.
    pub(crate) fn byte(&mut self) -> Result<u8> {
        if !self.is_byte_aligned() {
            return Err(Error::BadValue("unaligned byte read"));
        }
        Ok(self.bits(8)? as u8)
    }

    /// Unsigned Exp-Golomb `ue(v)`. Rejects a leading-zero prefix longer
    /// than 31 (code_num above `u32::MAX`), which no H.264 field permits.
    pub(crate) fn ue(&mut self) -> Result<u32> {
        let mut zeros = 0usize;
        loop {
            if zeros >= self.remaining() {
                return Err(Error::truncated("ue(v) prefix", zeros + 1, zeros));
            }
            let bit = (self.data[(self.pos + zeros) / 8] >> (7 - (self.pos + zeros) % 8)) & 1;
            if bit == 1 {
                break;
            }
            zeros += 1;
            if zeros > 31 {
                return Err(Error::BadValue("ue(v) prefix over 31 zeros"));
            }
        }
        self.pos += zeros + 1;
        let suffix = if zeros == 0 { 0 } else { self.bits(zeros)? };
        Ok((1u64 << zeros) as u32 - 1 + suffix)
    }

    /// Signed Exp-Golomb `se(v)`.
    pub(crate) fn se(&mut self) -> Result<i32> {
        let k = self.ue()? as i64;
        // code_num -> value: odd k maps to (k+1)/2, even to -(k/2).
        let v = if k % 2 == 0 { -(k / 2) } else { (k + 1) / 2 };
        Ok(v as i32)
    }

    /// Truncated Exp-Golomb `te(v)`: one bit when `range` is 1, and that
    /// single bit is INVERTED per spec 9.1 (bit `0` decodes as `1`,
    /// bit `1` as `0`); a plain `ue(v)` otherwise.
    pub(crate) fn te(&mut self, range: u32) -> Result<u32> {
        if range <= 1 {
            Ok(1 - self.bits(1)?)
        } else {
            self.ue()
        }
    }
}

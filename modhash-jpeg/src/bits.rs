//! Entropy-coded bit reader.
//!
//! Inside a scan the byte stream is MSB-first with byte stuffing: a
//! literal `FF` data byte is written as `FF 00`. `FF` followed by any
//! other byte is a marker — either a restart marker (`FFD0..D7`, legal
//! mid-scan) or the marker that ends the scan. This reader surfaces both
//! cases so the scan loops can enforce restart invariants.

use modhash_primitives::{Error, Result};

/// Bit-level view over one entropy-coded region.
///
/// `pos` always sits on the first unconsumed byte of `data` (or on the
/// `0xFF` of the marker that stopped the scan).
pub struct Bits<'a> {
    data: &'a [u8],
    /// Index of the next source byte.
    pub pos: usize,
    /// Bit accumulator; `bitcnt` low bits are valid.
    acc: u64,
    /// Number of valid bits in `acc`.
    bitcnt: u32,
    /// Set when a marker was seen: `Some(marker code)` (e.g. 0xD0..D9,
    /// 0xDA) with `pos` pointing at its `0xFF` so callers can rescan.
    marker: Option<u8>,
    /// Sticky error: once set (e.g. a real marker where data was
    /// expected), every subsequent bit request fails rather than
    /// fabricating zero bits, so a truncated scan errors out instead of
    /// silently decoding gray blocks.
    stalled: bool,
}

impl<'a> Bits<'a> {
    /// Starts reading `data` at byte `pos`.
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Bits {
            data,
            pos,
            acc: 0,
            bitcnt: 0,
            marker: None,
            stalled: false,
        }
    }

    /// Resets for the next restart interval: drops every buffered bit
    /// (prefetch only ever loads bytes before the marker, so the whole
    /// accumulator is pre-marker data) and consumes the expected RSTn
    /// marker. `expected` is `n` of RSTn. Fill bytes (`FF` runs) and
    /// stray non-marker bytes before it are skipped.
    ///
    /// Returns `false` when the expected marker is absent or wrong —
    /// callers decide between lenient resync and hard failure.
    pub fn consume_rst(&mut self, expected: u8) -> bool {
        self.acc = 0;
        self.bitcnt = 0;
        // Skip stray bytes that cannot start a marker.
        while self.pos < self.data.len() && self.data[self.pos] != 0xff {
            self.pos += 1;
        }
        // Skip the 0xFF run (the marker's 0xFF plus any fill bytes).
        let mut i = self.pos;
        while i < self.data.len() && self.data[i] == 0xff {
            i += 1;
        }
        if i >= self.data.len() {
            return false;
        }
        let code = self.data[i];
        if (0xd0..=0xd7).contains(&code) && code - 0xd0 == expected {
            self.pos = i + 1;
            self.marker = None;
            self.stalled = false;
            true
        } else {
            false
        }
    }

    /// Next data byte, handling stuffing. `None` = marker or EOF.
    fn byte(&mut self) -> Option<u8> {
        if self.stalled || self.marker.is_some() {
            return None;
        }
        let b = *self.data.get(self.pos)?;
        if b != 0xff {
            self.pos += 1;
            return Some(b);
        }
        // Peek past the 0xFF run (fill bytes).
        let mut i = self.pos;
        while i < self.data.len() && self.data[i] == 0xff {
            i += 1;
        }
        if i >= self.data.len() {
            self.marker = Some(0xff); // dangling FFs then EOF
            self.pos = i;
            return None;
        }
        let code = self.data[i];
        if code == 0x00 {
            // Stuffed literal 0xFF.
            self.pos = i + 1;
            return Some(0xff);
        }
        // Real marker (RSTn or segment). Leave pos on the first 0xFF.
        self.marker = Some(code);
        None
    }

    /// Reads `n` bits (1..=16), MSB-first (first bit = MSB), per T.81.
    pub fn bits(&mut self, n: u32) -> Result<u32> {
        debug_assert!((1..=16).contains(&n));
        while self.bitcnt < n {
            match self.byte() {
                Some(b) => {
                    self.acc = (self.acc << 8) | u64::from(b);
                    self.bitcnt += 8;
                }
                None => {
                    self.stalled = true;
                    return Err(Error::Truncated {
                        what: "entropy-coded data",
                        needed: n as usize,
                        found: self.bitcnt as usize,
                    });
                }
            }
        }
        let shift = self.bitcnt - n;
        let v = (self.acc >> shift) as u32 & ((1u32 << n) - 1);
        self.bitcnt -= n;
        self.acc &= if self.bitcnt == 0 {
            0
        } else {
            (1u64 << self.bitcnt) - 1
        };
        Ok(v)
    }

    /// T.81 `RECEIVE(s)`: `s` bits.
    pub fn receive(&mut self, s: u32) -> Result<u32> {
        if s == 0 {
            return Ok(0);
        }
        self.bits(s)
    }

    /// T.81 `EXTEND(v, s)`: converts the `s`-bit magnitude into a signed
    /// value — values below `2^(s-1)` are `v - 2^s + 1`.
    pub fn extend(&mut self, s: u32) -> Result<i32> {
        let v = self.receive(s)?;
        Ok(extend(v, s))
    }
}

/// T.81 `EXTEND(v, s)` on an already-read magnitude.
#[inline]
pub fn extend(v: u32, s: u32) -> i32 {
    if s == 0 {
        return 0;
    }
    let vt = 1i32 << (s - 1);
    let v = v as i32;
    if v < vt { v - (vt << 1) + 1 } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FF 00 stuffing is unescaped to a literal 0xFF data byte;
    /// FF D9 (any non-zero byte) ends the stream at the marker.
    #[test]
    fn stuffing_and_marker_stop() {
        let data = [0x12, 0xff, 0x00, 0x34, 0xff, 0xd9, 0xaa];
        let mut b = Bits::new(&data, 0);
        assert_eq!(b.bits(8).unwrap(), 0x12);
        assert_eq!(b.bits(8).unwrap(), 0xff); // unstuffed
        assert_eq!(b.bits(8).unwrap(), 0x34);
        assert!(b.bits(1).is_err()); // at EOI marker
    }

    /// MSB-first semantics: first bit read is bit 7 of byte 0.
    #[test]
    fn msb_first_bit_order() {
        let data = [0b1011_0011];
        let mut b = Bits::new(&data, 0);
        assert_eq!(b.bits(1).unwrap(), 1);
        assert_eq!(b.bits(3).unwrap(), 0b011);
        assert_eq!(b.bits(4).unwrap(), 0b0011);
        assert!(b.bits(1).is_err());
    }

    /// Fill bytes between the data and a restart marker are skipped;
    /// the wrong RST index is refused.
    #[test]
    fn restart_marker_consumption() {
        let data = [0xab, 0xff, 0xff, 0xd1, 0x55];
        let mut b = Bits::new(&data, 0);
        assert_eq!(b.bits(8).unwrap(), 0xab);
        assert!(!b.consume_rst(0)); // wrong index
        assert!(b.consume_rst(1)); // FF FF D1 → RST1
        assert_eq!(b.bits(8).unwrap(), 0x55);
    }

    /// T.81 EXTEND known-answer pairs.
    #[test]
    fn extend_sign_magnitude() {
        assert_eq!(extend(0b011, 3), -4); // 3 - 8 + 1
        assert_eq!(extend(0b000, 3), -7); // 0 - 8 + 1
        assert_eq!(extend(0b100, 3), 4);
        assert_eq!(extend(0b111, 3), 7);
        assert_eq!(extend(0, 1), -1);
        assert_eq!(extend(1, 1), 1);
        assert_eq!(extend(0, 0), 0);
    }
}

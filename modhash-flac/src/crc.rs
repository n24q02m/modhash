//! The two checksums the FLAC frame format carries (RFC 9639 sections 9.2
//! and 9.3).
//!
//! Both are MSB-first, non-reflected CRCs with zero initial value and no
//! output inversion:
//!
//! - `crc8`: polynomial `x^8 + x^2 + x + 1` (`0x07`), one byte, over the
//!   whole frame header before the CRC byte.
//! - `crc16`: polynomial `x^16 + x^15 + x^2 + 1` (`0x8005`), two bytes,
//!   over the frame from the first sync byte to the end of padding.
//!
//! Both are computed bitwise rather than through a 256-entry table: the
//! header CRC covers ~16 bytes and the frame CRC covers at most 64 KiB, so
//! table setup would cost more code than it saves time here.
//!
//! `modhash-primitives` ships no CRC primitive, so these live in this
//! crate; a workspace checksum primitive that lands later should absorb
//! them.

/// CRC-8 of `data`: poly 0x07, init 0, no reflection, no xor-out.
pub(crate) fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC-16 of `data`: poly 0x8005, init 0, no reflection, no xor-out.
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    // The "123456789" check values are the standard known-answer vectors
    // for these CRC parameters (CRC-8/SMBUS and CRC-16/BUYPASS, which are
    // exactly the FLAC parameters: init 0, non-reflected).
    #[test]
    fn crc8_check_value() {
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc8(&[]), 0);
    }

    #[test]
    fn crc16_check_value() {
        assert_eq!(crc16(b"123456789"), 0xFEE8);
        assert_eq!(crc16(&[]), 0);
    }
}

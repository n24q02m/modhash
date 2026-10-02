//! Canonical Huffman decoding (T.81 Annex C).
//!
//! Tables are stored in the JPEG order: `counts[l]` = number of codes of
//! length `l` (1..=16), then the symbol bytes in code order. Decoding
//! walks `mincode`/`maxcode`/`valptr` exactly as T.81 `DECODE` describes.
//!
//! Also hosts the zigzag permutation: scan order position `k` maps to
//! natural (row-major) index [`ZIGZAG`]`[k]`. This table is a
//! load-bearing seam — see the mutation proofs in `tests/`.

use modhash_primitives::{Error, Result};

/// Zigzag scan order: `ZIGZAG[k]` = natural index of the `k`-th scanned
/// coefficient (T.81 Figure 5).
pub const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, //
    17, 24, 32, 25, 18, 11, 4, 5, //
    12, 19, 26, 33, 40, 48, 41, 34, //
    27, 20, 13, 6, 7, 14, 21, 28, //
    35, 42, 49, 56, 57, 50, 43, 36, //
    29, 22, 15, 23, 30, 37, 44, 51, //
    58, 59, 52, 45, 38, 31, 39, 46, //
    53, 60, 61, 54, 47, 55, 62, 63,
];

/// A canonical Huffman decoding table.
#[derive(Clone, Debug)]
pub struct Table {
    /// `counts[l-1]` = number of codes of length `l`.
    counts: [u8; 16],
    /// Symbol values in code order.
    symbols: Vec<u8>,
    /// Smallest code value of each length.
    mincode: [i32; 16],
    /// Largest code value of each length (-1 if none).
    maxcode: [i32; 16],
    /// Index into `symbols` of the first code of each length.
    valptr: [i32; 16],
}

impl Table {
    /// Builds and validates a table per T.81 `DECODE`/`Generate_size_table`.
    ///
    /// Rejects oversubscribed tables (a code prefix space exhausted before
    /// all symbols fit) and tables with more than 256 symbols.
    pub fn build(counts: [u8; 16], symbols: Vec<u8>) -> Result<Self> {
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if total > 256 {
            return Err(Error::BadValue("DHT: more than 256 symbols"));
        }
        if symbols.len() != total {
            return Err(Error::BadValue("DHT: symbol count mismatch"));
        }
        let mut mincode = [0i32; 16];
        let mut maxcode = [-1i32; 16];
        let mut valptr = [0i32; 16];
        // Canonical code walk: `code` is the first code of the current
        // length. Each level must leave room: code+count-1 < 2^l.
        let mut code: i32 = 0;
        let mut sym: i32 = 0;
        for l in 0..16 {
            let n = counts[l] as i32;
            if n > 0 {
                valptr[l] = sym;
                mincode[l] = code;
                code += n;
                sym += n;
                maxcode[l] = code - 1;
            }
            // The prefix space of length l+1 holds codes < 2^(l+1).
            if code > (1i32 << (l + 1)) {
                return Err(Error::BadValue("DHT: oversubscribed table"));
            }
            code <<= 1;
        }
        Ok(Table {
            counts,
            symbols,
            mincode,
            maxcode,
            valptr,
        })
    }

    /// T.81 `DECODE`: reads one Huffman symbol.
    pub fn decode(&self, bits: &mut crate::bits::Bits<'_>) -> Result<u8> {
        let mut code = bits.bits(1)? as i32;
        let mut l = 0usize;
        while code > self.maxcode[l] {
            code = (code << 1) | bits.bits(1)? as i32;
            l += 1;
            // maxcode of an absent length is -1, so the loop keeps
            // going through gaps. 17 iterations is impossible because
            // bits(1) never returns > 2^16 prefixes for a valid table;
            // the bound is a belt-and-suspenders for hostile tables.
            if l >= 16 {
                return Err(Error::BadValue("Huffman code longer than 16 bits"));
            }
        }
        let idx = self.valptr[l] + (code - self.mincode[l]);
        let sym = *self
            .symbols
            .get(idx as usize)
            .ok_or(Error::BadValue("Huffman symbol index out of range"))?;
        let _ = self.counts;
        Ok(sym)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::Bits;

    /// The canonical all-zeros table: one symbol `0x42` with code `0`
    /// of length 1.
    #[test]
    fn single_symbol_table() {
        let mut counts = [0u8; 16];
        counts[0] = 1;
        let t = Table::build(counts, alloc::vec![0x42]).unwrap();
        let data = [0x00]; // eight 0 bits → eight '0'-code symbols
        let mut b = Bits::new(&data, 0);
        assert_eq!(t.decode(&mut b).unwrap(), 0x42);
        assert_eq!(t.decode(&mut b).unwrap(), 0x42);
    }

    /// Oversubscribed tables (a prefix space exhausted early) must be
    /// refused at build time, not discovered as a wrong decode.
    #[test]
    fn oversubscribed_table_rejected() {
        let mut counts = [0u8; 16];
        counts[0] = 3; // three 1-bit codes: impossible
        assert!(Table::build(counts, alloc::vec![1, 2, 3]).is_err());
        let mut counts = [0u8; 16];
        counts[0] = 2;
        counts[1] = 3; // 2 + 3 needs 5 codes of len 2; only 4 exist
        assert!(Table::build(counts, alloc::vec![1, 2, 3, 4, 5]).is_err());
    }

    /// T.81 Figure C.3's example table: AC luminance, count row
    /// [0,2,1,3,3,2,4,3,5,5,4,4,0,0,1,0x7d]. Decode a known codeword:
    /// the first 2-bit code (00) maps to the first symbol.
    #[test]
    fn standard_ac_table_first_code() {
        // bits: 0,2,1,... → codes: len1 none; len2: 00,01; len3: 100;...
        let mut counts = [0u8; 16];
        counts[1] = 2; // two codes of length 2
        counts[2] = 1; // one of length 3
        let syms = alloc::vec![0xaa, 0xbb, 0xcc];
        let t = Table::build(counts, syms).unwrap();
        // "00" → 0xaa; "01" → 0xbb; "100" → 0xcc
        let data = [0b0001_1000];
        let mut b = Bits::new(&data, 0);
        assert_eq!(t.decode(&mut b).unwrap(), 0xaa);
        assert_eq!(t.decode(&mut b).unwrap(), 0xbb);
        assert_eq!(t.decode(&mut b).unwrap(), 0xcc);
    }

    /// A code of 17+ bits (no matching length) must error, not hang.
    #[test]
    fn overlong_code_errors() {
        let mut counts = [0u8; 16];
        counts[15] = 1; // single 16-bit code: 1111111111111110
        let t = Table::build(counts, alloc::vec![0x77]).unwrap();
        let data = [0xff; 4]; // all ones → code grows past 16 bits
        let mut b = Bits::new(&data, 0);
        assert!(t.decode(&mut b).is_err());
    }
}

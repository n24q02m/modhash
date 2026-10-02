//! ToUnicode and encoding CMaps (PDF 32000-1 9.10.3).
//!
//! A CMap maps *character codes* (the bytes in a `Tj`/`TJ` string) to
//! Unicode. Codes are big-endian, their length fixed per codespace range;
//! one code may map to a multi-character string and astral characters are
//! written as UTF-16 surrogate pairs inside the destination string.
//!
//! `bfchar` gives explicit pairs. `bfrange` gives a source span plus either
//! one starting destination (incremented per source code, with carry
//! propagating from the last UTF-16 code unit backwards, so a surrogate
//! start yields consecutive astral characters) or an array of destinations
//! (element `i` answers for `lo+i`).
//!
//! An [`Error`] names the failing construct; a code outside every codespace
//! or absent from the map decodes to U+FFFD, never an abort, so one bad
//! code cannot lose a page of text.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::lex::{Lexer, Tok};

/// A parsed CMap: code -> Unicode string, plus its codespace widths and
/// writing mode.
#[derive(Clone, Debug)]
pub struct CMap {
    /// Key: `byte_len << 56 | big-endian code`; value: decoded UTF-16BE dst.
    map: BTreeMap<u64, String>,
    /// Codespace lengths actually usable for tokenizing input, longest first
    /// (union of `codespacerange` widths and widths present in the map —
    /// sloppy CMaps omit ranges for codes they still use).
    lens: Vec<usize>,
    /// Writing mode: 0 horizontal, 1 vertical.
    wmode: u8,
}

fn key(code: u64, len: usize) -> u64 {
    ((len as u64) << 56) | (code & ((1u64 << 56) - 1))
}

pub(crate) fn code_of_pub(bytes: &[u8]) -> u64 {
    let mut v = 0u64;
    for &b in bytes {
        v = v << 8 | u64::from(b);
    }
    v
}

/// Decode UTF-16BE bytes to a `String`; unpaired surrogates become U+FFFD.
fn utf16be(raw: &[u8]) -> Result<String> {
    if raw.len() % 2 != 0 {
        return Err(Error::BadValue("ToUnicode destination bytes"));
    }
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .collect();
    Ok(char::decode_utf16(units)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect())
}

impl CMap {
    /// Parse a CMap stream's decoded bytes.
    pub fn parse(data: &[u8]) -> Result<CMap> {
        let mut lx = Lexer::new(data);
        let mut map = BTreeMap::new();
        let mut spaces: Vec<(u64, u64, usize)> = Vec::new();
        let mut wmode: u8 = 0;
        // operand stack: hex/literal strings and numbers awaiting a keyword
        let mut stack: Vec<Tok> = Vec::new();
        while let Some(t) = lx.next()? {
            match t {
                Tok::Str(_) | Tok::Num(_) | Tok::Name(_) | Tok::ArrOpen | Tok::ArrClose => {
                    // arrays are collected raw so bfrange's `[dst ...]` form
                    // lands on the stack as one token group
                    if matches!(t, Tok::ArrOpen) {
                        let mut arr: Vec<Vec<u8>> = Vec::new();
                        loop {
                            match lx.next()? {
                                Some(Tok::ArrClose) => break,
                                Some(Tok::Str(s)) => arr.push(s.into_owned()),
                                Some(_) => return Err(Error::BadValue("bfrange array")),
                                None => {
                                    return Err(Error::Truncated {
                                        what: "bfrange array",
                                        needed: 1,
                                        found: 0,
                                    });
                                }
                            }
                        }
                        stack.push(Tok::ArrOpen);
                        for s in arr {
                            stack.push(Tok::Str(alloc::borrow::Cow::Owned(s)));
                        }
                        stack.push(Tok::ArrClose);
                    } else {
                        stack.push(t);
                    }
                    if stack.len() > 4096 {
                        return Err(Error::BadValue("cmap operand stack"));
                    }
                }
                Tok::DictOpen | Tok::DictClose => {}
                Tok::Kw(k) => {
                    match &*k {
                        b"begincodespacerange" => {
                            let n = pop_count(&mut stack)?;
                            for _ in 0..n {
                                let (lo, hi) = take2(&mut lx, "codespacerange")?;
                                if lo.len() != hi.len() || lo.len() > 7 {
                                    return Err(Error::BadValue("codespacerange"));
                                }
                                spaces.push((code_of_pub(&lo), code_of_pub(&hi), lo.len()));
                            }
                        }
                        b"beginbfchar" => {
                            let n = pop_count(&mut stack)?;
                            for _ in 0..n {
                                let (src, dst) = take2(&mut lx, "bfchar")?;
                                add_bfchar(&mut map, &src, &dst)?;
                            }
                        }
                        b"beginbfrange" => {
                            let n = pop_count(&mut stack)?;
                            for _ in 0..n {
                                read_bfrange(&mut lx, &mut map)?;
                            }
                        }
                        b"begincidchar" | b"begincidrange" | b"beginnotdefchar"
                        | b"beginnotdefrange" => {
                            // char-code -> CID sections: irrelevant to text
                            // extraction, skip `n` pairs
                            let n = pop_count(&mut stack)?;
                            skip_cid_pairs(&mut lx, n)?;
                        }
                        b"def" => {
                            // `/WMode 1 def` style entries
                            if stack.len() >= 2 {
                                if let (Tok::Name(name), Tok::Num(v)) =
                                    (&stack[stack.len() - 2], &stack[stack.len() - 1])
                                {
                                    if name.as_ref() == b"WMode"
                                        && crate::lex::is_int(*v)
                                        && *v >= 0.0
                                        && *v <= 1.0
                                    {
                                        wmode = *v as u8;
                                    }
                                }
                            }
                            stack.clear();
                        }
                        _ => stack.clear(),
                    }
                }
            }
        }
        if map.is_empty() && spaces.is_empty() {
            return Err(Error::BadValue("CMap has no mappings"));
        }
        let mut lens: Vec<usize> = spaces.iter().map(|&(_, _, l)| l).collect();
        // union with lengths actually used by the map
        for &k in map.keys() {
            let l = (k >> 56) as usize;
            if !lens.contains(&l) {
                lens.push(l);
            }
        }
        lens.sort_unstable_by(|a, b| b.cmp(a));
        Ok(CMap { map, lens, wmode })
    }

    /// Writing mode: `true` for vertical (`/WMode 1`).
    pub fn is_vertical(&self) -> bool {
        self.wmode == 1
    }

    /// Direct lookup of one already-tokenized code.
    pub(crate) fn get(&self, code: u64, len: usize) -> Option<&str> {
        self.map.get(&key(code, len)).map(String::as_str)
    }

    /// Declared code lengths (longest first) for tokenizing raw bytes.
    pub(crate) fn lens(&self) -> &[usize] {
        &self.lens
    }

    /// Decode the longest-matching code at `bytes`' head:
    /// `(mapped text, bytes consumed)`. `None` in the text slot means the
    /// code had no mapping; `consumed` is the shortest known code width so
    /// the caller can always advance without stalling.
    pub fn lookup(&self, bytes: &[u8]) -> (Option<&str>, usize) {
        for &l in &self.lens {
            if l <= bytes.len() {
                let c = code_of_pub(&bytes[..l]);
                if let Some(s) = self.map.get(&key(c, l)) {
                    return (Some(s.as_str()), l);
                }
            }
        }
        // no hit: skip one code unit of the shortest declared width
        (None, *self.lens.last().unwrap_or(&1))
    }
}

fn pop_count(stack: &mut Vec<Tok>) -> Result<i64> {
    match stack.pop() {
        Some(Tok::Num(v)) => crate::lex::f64_as_i64(v)
            .filter(|&n| (0..=1_000_000).contains(&n))
            .ok_or(Error::BadValue("cmap section count")),
        _ => Err(Error::BadValue("cmap section count")),
    }
}

fn take2(lx: &mut Lexer, what: &'static str) -> Result<(Vec<u8>, Vec<u8>)> {
    let a = take_str(lx, what)?;
    let b = take_str(lx, what)?;
    Ok((a, b))
}

fn take_str(lx: &mut Lexer, what: &'static str) -> Result<Vec<u8>> {
    match lx.next()? {
        Some(Tok::Str(s)) => Ok(s.into_owned()),
        Some(Tok::Name(n)) => Ok(n.into_owned()),
        _ => Err(Error::BadValue(what)),
    }
}

fn add_bfchar(map: &mut BTreeMap<u64, String>, src: &[u8], dst: &[u8]) -> Result<()> {
    if src.is_empty() || src.len() > 7 {
        return Err(Error::BadValue("bfchar source"));
    }
    if dst.is_empty() {
        return Err(Error::BadValue("bfchar destination"));
    }
    map.insert(key(code_of_pub(src), src.len()), utf16be(dst)?);
    Ok(())
}

fn read_bfrange(lx: &mut Lexer, map: &mut BTreeMap<u64, String>) -> Result<()> {
    let lo = take_str(lx, "bfrange lo")?;
    let hi = take_str(lx, "bfrange hi")?;
    if lo.len() != hi.len() || lo.is_empty() || lo.len() > 7 {
        return Err(Error::BadValue("bfrange source"));
    }
    let lo_v = code_of_pub(&lo);
    let hi_v = code_of_pub(&hi);
    if hi_v < lo_v {
        return Err(Error::BadValue("bfrange range"));
    }
    let span = hi_v - lo_v;
    if span > 65536 {
        return Err(Error::TooLarge {
            what: "bfrange span",
            limit: 65536,
        });
    }
    match lx.next()? {
        Some(Tok::Str(dst0)) => {
            let units = utf16_units(&dst0)?;
            for i in 0..=span {
                let dst = inc_units(&units, i);
                let s: String = char::decode_utf16(dst)
                    .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
                    .collect();
                map.insert(key(lo_v + i, lo.len()), s);
            }
            Ok(())
        }
        Some(Tok::ArrOpen) => {
            let mut i = 0u64;
            loop {
                match lx.next()? {
                    Some(Tok::ArrClose) => break,
                    Some(Tok::Str(d)) => {
                        if i > span {
                            return Err(Error::BadValue("bfrange array length"));
                        }
                        map.insert(key(lo_v + i, lo.len()), utf16be(&d)?);
                        i += 1;
                    }
                    Some(_) => return Err(Error::BadValue("bfrange array")),
                    None => {
                        return Err(Error::Truncated {
                            what: "bfrange array",
                            needed: 1,
                            found: 0,
                        });
                    }
                }
            }
            if i != span + 1 {
                return Err(Error::BadValue("bfrange array length"));
            }
            Ok(())
        }
        _ => Err(Error::BadValue("bfrange destination")),
    }
}

fn utf16_units(raw: &[u8]) -> Result<Vec<u16>> {
    if raw.len() % 2 != 0 || raw.is_empty() {
        return Err(Error::BadValue("bfrange destination bytes"));
    }
    Ok(raw
        .chunks_exact(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .collect())
}

/// Increment the LAST UTF-16 unit of `dst` by `i`, carrying into earlier
/// units. This is the bfrange stepping rule that turns the surrogate-pair
/// start `<D840DC0B>` into consecutive astral characters
/// (U+2000B, U+2000C, ...) — the spec example in PDF 32000-1 H.3.
fn inc_units(dst: &[u16], i: u64) -> Vec<u16> {
    let mut out = dst.to_vec();
    let mut carry = i;
    for u in out.iter_mut().rev() {
        if carry == 0 {
            break;
        }
        let v = u64::from(*u) + carry;
        *u = (v & 0xFFFF) as u16;
        carry = v >> 16;
    }
    out
}

fn skip_cid_pairs(lx: &mut Lexer, n: i64) -> Result<()> {
    for _ in 0..n {
        // each entry is `src dst` where dst may be a number, a string, or
        // (in a cidrange) an array of numbers
        let _ = lx.next()?.ok_or(Error::Truncated {
            what: "cid section",
            needed: 1,
            found: 0,
        })?;
        match lx.next()? {
            Some(Tok::ArrOpen) => loop {
                match lx.next()? {
                    Some(Tok::ArrClose) => break,
                    Some(_) => {}
                    None => {
                        return Err(Error::Truncated {
                            what: "cidrange array",
                            needed: 1,
                            found: 0,
                        });
                    }
                }
            },
            Some(_) => {}
            None => {
                return Err(Error::Truncated {
                    what: "cid section",
                    needed: 1,
                    found: 0,
                });
            }
        }
    }
    Ok(())
}

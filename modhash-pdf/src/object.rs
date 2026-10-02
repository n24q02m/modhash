//! The object model: `Obj` trees plus the recursive-descent object parser.
//!
//! Objects are owned (`'static`): direct objects and object-stream members
//! live in one arena, so a [`Document`](crate::Document) can resolve any
//! indirect reference without lifetime juggling. Stream bytes stay raw
//! (undecoded) here; filters are applied at the point of use so the caller
//! decides which [`Limits`](modhash_inflate::Limits) apply.

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::lex::{Lexer, Tok, f64_as_i64, is_ws};

/// An indirect reference: object number and generation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ref {
    /// Object number.
    pub num: u32,
    /// Generation number.
    pub generation: u16,
}

/// A parsed PDF object. `Dict` keeps file order: dictionary keys repeat
/// legally and `/Index`-style array payloads rely on position, not sorting.
#[derive(Clone, Debug, PartialEq)]
pub enum Obj {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// A number. Integers and reals share one node; [`Obj::as_i64`]
    /// refuses non-integral values.
    Num(f64),
    /// A `/Name` without the leading slash, `#xx` escapes decoded.
    Name(Vec<u8>),
    /// A decoded string (literal or hex; escapes resolved).
    Str(Vec<u8>),
    /// `[` ... `]`
    Arr(Vec<Obj>),
    /// `<<` ... `>>`; keys are decoded names.
    Dict(Vec<(Vec<u8>, Obj)>),
    /// A dictionary followed by `stream` data. `data` is the raw stream
    /// content (filters NOT applied).
    Stream {
        /// The stream's dictionary.
        dict: Vec<(Vec<u8>, Obj)>,
        /// Raw stream bytes.
        data: Vec<u8>,
    },
    /// `N G R`
    Ref(Ref),
}

impl Obj {
    /// Look up `key` in a `Dict` or `Stream` dictionary.
    pub fn get(&self, key: &[u8]) -> Option<&Obj> {
        match self {
            Obj::Dict(kv) | Obj::Stream { dict: kv, .. } => {
                kv.iter().find(|(k, _)| k == key).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    /// The dictionary part of a `Dict` or `Stream`.
    pub fn dict(&self) -> Option<&[(Vec<u8>, Obj)]> {
        match self {
            Obj::Dict(kv) | Obj::Stream { dict: kv, .. } => Some(kv),
            _ => None,
        }
    }

    /// The reference if this is an indirect reference.
    pub fn as_ref(&self) -> Option<Ref> {
        match self {
            Obj::Ref(r) => Some(*r),
            _ => None,
        }
    }

    /// `f64` for `Num`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Obj::Num(v) => Some(*v),
            _ => None,
        }
    }

    /// `i64` for integer-valued `Num`.
    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().and_then(f64_as_i64)
    }

    /// `u32` for non-negative integer-valued `Num`.
    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().and_then(|v| u32::try_from(v).ok())
    }

    /// `usize` for non-negative integer-valued `Num`.
    pub fn as_usize(&self) -> Option<usize> {
        self.as_i64().and_then(|v| usize::try_from(v).ok())
    }

    /// `bool` for `Bool`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Obj::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// `&[u8]` for `Name` and `Str`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Obj::Name(b) | Obj::Str(b) => Some(b),
            _ => None,
        }
    }

    /// Array elements for `Arr`.
    pub fn as_array(&self) -> Option<&[Obj]> {
        match self {
            Obj::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// Raw stream bytes for `Stream`.
    pub fn stream_data(&self) -> Option<&[u8]> {
        match self {
            Obj::Stream { data, .. } => Some(data),
            _ => None,
        }
    }
}

/// A parsed indirect object plus a deferred stream-length fixup.
///
/// Streams whose `/Length` is an indirect reference cannot have their data
/// sliced until the referenced object is known, so [`parse_obj`] records
/// `(length_ref, data_start)` in `pending`; the
/// [`Document`](crate::Document) re-slices after resolving it.
pub(crate) struct Parsed {
    /// The parsed object (a `Stream`'s `data` may run to `endstream`
    /// rather than `/Length` until `pending` is resolved).
    pub(crate) obj: Obj,
    /// `(length_ref, data_start_offset)` for streams with indirect `/Length`.
    pub(crate) pending: Option<(Ref, usize)>,
}

const MAX_DEPTH: usize = 64;
const MAX_ELEMS: usize = 1 << 22; // 4M array/dict members

/// Parse the indirect object whose `<num> <gen> obj` header starts at
/// `offset` in `data`.
pub(crate) fn parse_obj(data: &[u8], offset: usize) -> Result<Parsed> {
    let mut lx = Lexer { data, pos: offset };
    let num = lx.expect_int("object number")?;
    let generation = lx.expect_int("generation number")?;
    if num < 0 || num > i64::from(u32::MAX) || generation < 0 || generation > i64::from(u16::MAX) {
        return Err(Error::BadValue("object number/generation"));
    }
    lx.expect_kw(b"obj", "'obj' keyword")?;
    let (obj, pending) = parse_value(&mut lx, 0, true)?;
    Ok(Parsed { obj, pending })
}

/// Parse one standalone value (object-stream members, trailer dictionaries).
/// Streams may NOT appear at value level; `pending` is always `None` except
/// when `top` allows a trailing stream.
pub(crate) fn parse_standalone(data: &[u8], offset: usize) -> Result<Parsed> {
    let mut lx = Lexer { data, pos: offset };
    let (obj, pending) = parse_value(&mut lx, 0, true)?;
    Ok(Parsed { obj, pending })
}

/// Parse one value. `top` permits a `stream` tail after a dict (only legal
/// at object level).
fn parse_value(lx: &mut Lexer, depth: usize, top: bool) -> Result<(Obj, Option<(Ref, usize)>)> {
    let t = lx.next()?.ok_or(Error::Truncated {
        what: "object",
        needed: 1,
        found: 0,
    })?;
    value_from_head(lx, t, depth, top)
}

fn value_from_head(
    lx: &mut Lexer,
    t: Tok,
    depth: usize,
    top: bool,
) -> Result<(Obj, Option<(Ref, usize)>)> {
    // the nesting cap lives here (not in parse_value) because container
    // members recurse through value_from_head directly
    if depth >= MAX_DEPTH {
        return Err(Error::BadValue("object nesting depth"));
    }
    match t {
        Tok::Num(v) => {
            // indirect reference lookahead: `N G R`
            let save = lx.pos;
            if crate::lex::is_int(v) && v >= 0.0 {
                let second = lx.next()?;
                if let Some(Tok::Num(g)) = second {
                    if crate::lex::is_int(g) && g >= 0.0 {
                        let third = lx.next()?;
                        if let Some(Tok::Kw(k)) = third {
                            if &*k == b"R" {
                                let n = f64_as_i64(v).unwrap_or(i64::MAX);
                                let gg = f64_as_i64(g).unwrap_or(i64::MAX);
                                if n > i64::from(u32::MAX) || gg > i64::from(u16::MAX) {
                                    return Err(Error::BadValue("indirect reference bounds"));
                                }
                                return Ok((
                                    Obj::Ref(Ref {
                                        num: n as u32,
                                        generation: gg as u16,
                                    }),
                                    None,
                                ));
                            }
                            lx.pos = save;
                            return Ok((Obj::Num(v), None));
                        }
                        lx.pos = save;
                        return Ok((Obj::Num(v), None));
                    }
                    lx.pos = save;
                    return Ok((Obj::Num(v), None));
                }
                lx.pos = save;
            }
            Ok((Obj::Num(v), None))
        }
        Tok::Name(b) => Ok((Obj::Name(b.into_owned()), None)),
        Tok::Str(b) => Ok((Obj::Str(b.into_owned()), None)),
        Tok::ArrOpen => {
            let mut v = Vec::new();
            loop {
                match lx.next()? {
                    Some(Tok::ArrClose) => break,
                    Some(other) => {
                        let (o, pend) = value_from_head(lx, other, depth + 1, false)?;
                        debug_assert!(pend.is_none());
                        v.push(o);
                        if v.len() > MAX_ELEMS {
                            return Err(Error::TooLarge {
                                what: "array elements",
                                limit: MAX_ELEMS,
                            });
                        }
                    }
                    None => {
                        return Err(Error::Truncated {
                            what: "array",
                            needed: 1,
                            found: 0,
                        });
                    }
                }
            }
            Ok((Obj::Arr(v), None))
        }
        Tok::DictOpen => {
            let mut kv: Vec<(Vec<u8>, Obj)> = Vec::new();
            loop {
                match lx.next()? {
                    Some(Tok::DictClose) => break,
                    Some(Tok::Name(k)) => {
                        let nt = lx.next()?.ok_or(Error::Truncated {
                            what: "dictionary value",
                            needed: 1,
                            found: 0,
                        })?;
                        let (o, pend) = value_from_head(lx, nt, depth + 1, false)?;
                        debug_assert!(pend.is_none());
                        kv.push((k.into_owned(), o));
                        if kv.len() > MAX_ELEMS {
                            return Err(Error::TooLarge {
                                what: "dict entries",
                                limit: MAX_ELEMS,
                            });
                        }
                    }
                    Some(_) => return Err(Error::BadValue("dictionary key is not a name")),
                    None => {
                        return Err(Error::Truncated {
                            what: "dictionary",
                            needed: 1,
                            found: 0,
                        });
                    }
                }
            }
            if top {
                let save = lx.pos;
                if lx.eat_kw(b"stream")? {
                    return parse_stream(lx, kv);
                }
                lx.pos = save;
            }
            Ok((Obj::Dict(kv), None))
        }
        Tok::Kw(k) => match &*k {
            b"true" => Ok((Obj::Bool(true), None)),
            b"false" => Ok((Obj::Bool(false), None)),
            b"null" => Ok((Obj::Null, None)),
            _ => Err(Error::BadValue("unexpected keyword in object")),
        },
        Tok::ArrClose | Tok::DictClose => Err(Error::BadValue("stray closer")),
    }
}

/// After `<<...>> stream <EOL>`: slice `/Length` bytes (or run to
/// `endstream` when the length is an indirect reference).
fn parse_stream(lx: &mut Lexer, dict: Vec<(Vec<u8>, Obj)>) -> Result<(Obj, Option<(Ref, usize)>)> {
    match lx.rest().first() {
        Some(&0x0D) => {
            lx.pos += 1;
            if lx.data.get(lx.pos) == Some(&0x0A) {
                lx.pos += 1;
            }
        }
        Some(&0x0A) => lx.pos += 1,
        _ => return Err(Error::BadValue("'stream' keyword not followed by EOL")),
    }
    let start = lx.pos;
    let len_obj = dict_get(&dict, b"Length").cloned();
    match len_obj {
        Some(Obj::Num(n)) => {
            let n = f64_as_i64(n).ok_or(Error::BadValue("/Length value"))?;
            if n < 0 {
                return Err(Error::BadValue("negative /Length"));
            }
            let n = n as usize;
            let end = start
                .checked_add(n)
                .ok_or(Error::BadValue("/Length overflow"))?;
            if end > lx.data.len() {
                return Err(Error::Truncated {
                    what: "stream data",
                    needed: n,
                    found: lx.data.len().saturating_sub(start),
                });
            }
            let data = lx.data[start..end].to_vec();
            lx.pos = end;
            if !consume_endstream(lx) && end != lx.data.len() {
                // a wrong length is only tolerated when data runs out at EOF
                return Err(Error::BadValue("stream without endstream"));
            }
            Ok((Obj::Stream { dict, data }, None))
        }
        Some(Obj::Ref(r)) => {
            let marker = lx.data[start..]
                .windows(9)
                .position(|w| w == b"endstream")
                .ok_or(Error::Truncated {
                    what: "stream endstream marker",
                    needed: 1,
                    found: 0,
                })?;
            let end = start + marker;
            let data = lx.data[start..end].to_vec();
            lx.pos = end;
            consume_endstream(lx);
            Ok((Obj::Stream { dict, data }, Some((r, start))))
        }
        Some(_) => Err(Error::BadValue("/Length type")),
        None => Err(Error::BadValue("stream without /Length")),
    }
}

fn consume_endstream(lx: &mut Lexer) -> bool {
    let save = lx.pos;
    lx.skip_ws();
    match lx.next() {
        Ok(Some(Tok::Kw(k))) if &*k == b"endstream" => true,
        _ => {
            lx.pos = save;
            false
        }
    }
}

pub(crate) fn dict_get<'o>(dict: &'o [(Vec<u8>, Obj)], key: &[u8]) -> Option<&'o Obj> {
    dict.iter()
        .find(|(k, _)| k.as_slice() == key)
        .map(|(_, v)| v)
}

fn dict_i64(dict: &[(Vec<u8>, Obj)], key: &[u8]) -> Option<i64> {
    dict_get(dict, key).and_then(Obj::as_i64)
}

/// Apply a stream object's `/Filter` chain to its raw bytes.
///
/// FlateDecode (zlib, with raw-deflate fallback for non-conforming files),
/// ASCII85Decode and ASCIIHexDecode are implemented. Anything else - LZW,
/// DCT, JBIG2, JPX, CCITT, RunLength, Crypt - is [`Error::Unsupported`]
/// naming the filter. `DecodeParms` predictors (PNG optimums 10-15 and
/// TIFF 2) are applied after FlateDecode.
pub fn decode_stream(obj: &Obj, limits: &modhash_inflate::Limits) -> Result<Vec<u8>> {
    let (dict, data) = match obj {
        Obj::Stream { dict, data } => (dict, data.as_slice()),
        _ => return Err(Error::BadValue("not a stream")),
    };
    let filters: Vec<Vec<u8>> = match dict_get(dict, b"Filter") {
        None | Some(Obj::Null) => Vec::new(),
        Some(Obj::Name(n)) => alloc::vec![n.clone()],
        Some(Obj::Arr(a)) => a
            .iter()
            .map(|o| match o {
                Obj::Name(n) => Ok(n.clone()),
                _ => Err(Error::BadValue("/Filter element type")),
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => return Err(Error::BadValue("/Filter type")),
    };
    let parms: Vec<&Obj> = match dict_get(dict, b"DecodeParms") {
        None | Some(Obj::Null) => Vec::new(),
        Some(p @ Obj::Dict(_)) => alloc::vec![p],
        Some(Obj::Arr(a)) => a.iter().collect(),
        Some(_) => return Err(Error::BadValue("/DecodeParms type")),
    };
    let mut cur: Vec<u8> = data.to_vec();
    for (i, f) in filters.iter().enumerate() {
        let dp = parms.get(i).copied().and_then(|o| o.dict());
        cur = match f.as_slice() {
            b"FlateDecode" | b"Fl" | b"Flate" => {
                let d = modhash_inflate::inflate_zlib(&cur, limits)
                    .or_else(|_| modhash_inflate::inflate_raw(&cur, limits))?;
                apply_predictor(d, dp)?
            }
            b"ASCIIHexDecode" | b"AHx" => ascii_hex(&cur)?,
            b"ASCII85Decode" | b"A85" => ascii85(&cur)?,
            b"Crypt" => return Err(Error::Unsupported("Crypt stream filter")),
            b"LZWDecode" | b"LZW" => return Err(Error::Unsupported("LZWDecode stream filter")),
            b"DCTDecode" | b"DCT" => return Err(Error::Unsupported("DCTDecode stream filter")),
            b"JBIG2Decode" => return Err(Error::Unsupported("JBIG2Decode stream filter")),
            b"JPXDecode" => return Err(Error::Unsupported("JPXDecode stream filter")),
            b"CCITTFaxDecode" | b"CCF" => {
                return Err(Error::Unsupported("CCITTFaxDecode stream filter"));
            }
            b"RunLengthDecode" | b"RL" => {
                return Err(Error::Unsupported("RunLengthDecode stream filter"));
            }
            _ => return Err(Error::Unsupported("unknown stream filter")),
        };
    }
    Ok(cur)
}

/// PNG/TIFF predictor pass on decoded bytes (DecodeParms `/Predictor`).
fn apply_predictor(data: Vec<u8>, parms: Option<&[(Vec<u8>, Obj)]>) -> Result<Vec<u8>> {
    let p = parms.and_then(|d| dict_i64(d, b"Predictor")).unwrap_or(1);
    if p == 1 {
        return Ok(data);
    }
    let colors = parms.and_then(|d| dict_i64(d, b"Colors")).unwrap_or(1);
    let bpc = parms
        .and_then(|d| dict_i64(d, b"BitsPerComponent"))
        .unwrap_or(8);
    let cols = parms.and_then(|d| dict_i64(d, b"Columns")).unwrap_or(1);
    if colors <= 0 || bpc <= 0 || cols <= 0 {
        return Err(Error::BadValue("DecodeParms dimensions"));
    }
    if p == 2 {
        if bpc != 8 {
            return Err(Error::Unsupported("TIFF predictor bpc != 8"));
        }
        let bpp = colors as usize;
        let rowlen = bpp
            .checked_mul(cols as usize)
            .ok_or(Error::BadValue("TIFF predictor row size"))?;
        if rowlen == 0 {
            return Err(Error::BadValue("TIFF predictor row size"));
        }
        let mut out = data;
        for row in out.chunks_exact_mut(rowlen) {
            for i in bpp..row.len() {
                let prev = row[i - bpp];
                row[i] = row[i].wrapping_add(prev);
            }
        }
        return Ok(out);
    }
    if !(10..=15).contains(&p) {
        return Err(Error::BadValue("Predictor value"));
    }
    let bits = colors
        .checked_mul(cols)
        .and_then(|c| c.checked_mul(bpc))
        .ok_or(Error::BadValue("PNG predictor row size"))?;
    let rowlen = (bits as usize).div_ceil(8);
    let bpp = ((colors as usize) * (bpc as usize)).div_ceil(8);
    if bpp == 0 || rowlen == 0 {
        return Err(Error::BadValue("PNG predictor row size"));
    }
    let mut out = Vec::with_capacity(data.len());
    let mut prev = alloc::vec![0u8; rowlen];
    let mut rows = data.chunks_exact(rowlen + 1);
    for row in rows.by_ref() {
        let (ft, line) = (row[0], &row[1..]);
        let mut cur = line.to_vec();
        match ft {
            0 => {}
            1 => {
                for i in bpp..cur.len() {
                    cur[i] = cur[i].wrapping_add(cur[i - bpp]);
                }
            }
            2 => {
                for i in 0..cur.len() {
                    cur[i] = cur[i].wrapping_add(prev[i]);
                }
            }
            3 => {
                for i in 0..cur.len() {
                    let a = if i >= bpp { cur[i - bpp] } else { 0 };
                    cur[i] = cur[i].wrapping_add(((u16::from(a) + u16::from(prev[i])) / 2) as u8);
                }
            }
            4 => {
                for i in 0..cur.len() {
                    let a = if i >= bpp { cur[i - bpp] } else { 0 };
                    let b = prev[i];
                    let c = if i >= bpp { prev[i - bpp] } else { 0 };
                    cur[i] = cur[i].wrapping_add(paeth(a, b, c));
                }
            }
            _ => return Err(Error::BadValue("PNG predictor filter type")),
        }
        out.extend_from_slice(&cur);
        prev = cur;
    }
    if !rows.remainder().is_empty() {
        return Err(Error::Truncated {
            what: "PNG predictor row",
            needed: rowlen + 1,
            found: rows.remainder().len(),
        });
    }
    Ok(out)
}

fn iabs(v: i32) -> i32 {
    if v < 0 { -v } else { v }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a, b, c) = (i32::from(a), i32::from(b), i32::from(c));
    let p = a + b - c;
    let (pa, pb, pc) = (iabs(p - a), iabs(p - b), iabs(p - c));
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

fn ascii_hex(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 2 + 1);
    let mut hi: Option<u8> = None;
    for &b in data {
        if b == b'>' {
            break;
        }
        if is_ws(b) {
            continue;
        }
        match crate::lex::hex_val(b) {
            Some(v) => match hi.take() {
                Some(h) => out.push((h << 4) | v),
                None => hi = Some(v),
            },
            None => return Err(Error::BadValue("ASCIIHex character")),
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    Ok(out)
}

fn ascii85(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 5 * 4 + 4);
    let mut group: Vec<u8> = Vec::with_capacity(5);
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        i += 1;
        if is_ws(b) {
            continue;
        }
        if b == b'~' {
            if data.get(i) == Some(&b'>') {
                break;
            }
            return Err(Error::BadValue("ASCII85 terminator"));
        }
        if b == b'z' && group.is_empty() {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if !(b'!'..=b'u').contains(&b) {
            return Err(Error::BadValue("ASCII85 character"));
        }
        group.push(b - b'!');
        if group.len() == 5 {
            let v = group.iter().fold(0u32, |a, &g| a * 85 + u32::from(g));
            out.extend_from_slice(&v.to_be_bytes());
            group.clear();
        }
    }
    if !group.is_empty() {
        let n = group.len();
        while group.len() < 5 {
            group.push(84); // 'u'
        }
        let v = group.iter().fold(0u32, |a, &g| a * 85 + u32::from(g));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    Ok(out)
}

//! Cross-reference resolution: classic `xref` tables, xref **streams**
//! (`/W` field widths + `/Index`), `/Prev` chains for incremental updates,
//! and a scan-based rebuild when the xref itself is corrupt.
//!
//! Object locations come in two shapes:
//!
//! - **plain** `offset` + `gen` — read `num gen obj` at that offset;
//! - **object-stream member** `stm` + `idx` — object `num` is the `idx`-th
//!   member of object stream `stm` (decoded on first use).
//!
//! Newer entries win: the newest trailer's table is consulted first and a
//! merge inserts only entries the newer table does not already carry.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::lex::{Lexer, Tok};
use crate::object::{Obj, decode_stream, dict_get, parse_obj, parse_standalone};

/// Where one live object lives in the file.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Loc {
    /// Byte offset of `num gen obj`.
    Plain {
        /// Byte offset of the `N G obj` header.
        offset: usize,
        /// Declared generation number.
        generation: u16,
    },
    /// Member `idx` of object stream `stm`.
    InStm {
        /// Object number of the `/ObjStm` stream.
        stm: u32,
        /// Zero-based member index.
        idx: u32,
    },
}

/// The resolved cross-reference: every used object number to its location,
/// plus the trailer dictionary of the *newest* section (merged `/Prev`
/// entries stay reachable through the map but the newest trailer's keys win).
pub(crate) struct Xref {
    /// Object number -> location.
    pub(crate) map: BTreeMap<u32, Loc>,
    /// Newest trailer dictionary (owns `/Root`, `/Encrypt`, `/Size`, ...).
    pub(crate) trailer: Vec<(Vec<u8>, Obj)>,
    /// `true` when the xref could not be parsed and was rebuilt by scanning
    /// the file for `N G obj` headers.
    pub(crate) rebuilt: bool,
}

/// `startxref` value: the byte offset the file claims its newest xref sits
/// at. `None` if no usable `startxref` marker exists.
fn find_startxref(data: &[u8]) -> Result<usize> {
    // spec wants it in the last 1024 bytes; we scan the whole tail of the
    // file backwards for the LAST occurrence to tolerate junk after %%EOF.
    let mut search_from = data.len();
    loop {
        let win = &data[..search_from];
        let pos = win
            .windows(9)
            .rposition(|w| w == b"startxref")
            .ok_or(Error::BadValue("missing startxref"))?;
        let mut lx = Lexer { data, pos: pos + 9 };
        lx.skip_ws();
        match lx.next()? {
            Some(Tok::Num(v)) if v >= 0.0 && crate::lex::is_int(v) => {
                return Ok(v as usize);
            }
            Some(_) | None => {
                // keep looking backwards for an older marker
                search_from = pos;
                if search_from == 0 {
                    return Err(Error::BadValue("startxref value"));
                }
            }
        }
    }
}

/// Parse the xref chain starting at `start`, merging `/Prev` sections and
/// `XRefStm` hybrid references (newest first; first writer wins).
pub(crate) fn read_xref(data: &[u8], limits: &modhash_inflate::Limits) -> Result<Xref> {
    let start = find_startxref(data)?;
    let mut map: BTreeMap<u32, Loc> = BTreeMap::new();
    let mut trailer: Option<Vec<(Vec<u8>, Obj)>> = None;
    let mut at = Some(start);
    let mut sections = 0usize;
    while let Some(off) = at.take() {
        sections += 1;
        if sections > 32 {
            return Err(Error::BadValue("xref /Prev chain length"));
        }
        if off >= data.len() {
            return Err(Error::Truncated {
                what: "xref section",
                needed: off + 1,
                found: data.len(),
            });
        }
        let (dict, prev, hybrid) = read_xref_at(data, off, limits, &mut map)?;
        if trailer.is_none() {
            trailer = Some(dict.clone());
        } else {
            // merge keys the newer trailer lacks (e.g. /ID often lives only
            // in the first trailer)
            for (k, v) in dict {
                if let Some(t) = trailer.as_mut() {
                    if dict_get(t, &k).is_none() {
                        t.push((k, v));
                    }
                }
            }
        }
        // hybrid /XRefStm points at a supplemental xref *stream*
        if let Some(xs_off) = hybrid {
            let (hd, _, _) = read_xref_at(data, xs_off, limits, &mut map)?;
            for (k, v) in hd {
                if dict_get(trailer.as_mut().expect("set"), &k).is_none() {
                    trailer.as_mut().expect("set").push((k, v));
                }
            }
        }
        at = prev;
    }
    let trailer = trailer.ok_or(Error::BadValue("empty xref chain"))?;
    if map.is_empty() {
        return Err(Error::BadValue("empty xref table"));
    }
    Ok(Xref {
        map,
        trailer,
        rebuilt: false,
    })
}

/// Parse one xref section at `off` — either the `xref` keyword (classic
/// table + `trailer` dict) or an xref-stream object. Returns
/// `(trailer_dict, /Prev, /XRefStm)`.
fn read_xref_at(
    data: &[u8],
    off: usize,
    limits: &modhash_inflate::Limits,
    map: &mut BTreeMap<u32, Loc>,
) -> Result<XrefResult> {
    let mut lx = Lexer { data, pos: off };
    lx.skip_ws();
    match lx.peek()? {
        Some(Tok::Kw(k)) if &*k == b"xref" => {
            lx.next()?;
            read_classic(&mut lx, map)
        }
        Some(Tok::Num(_)) => {
            // xref stream: `<num> <gen> obj <<...>> stream`
            let p = parse_obj(data, off)?;
            read_xref_stream(p, limits, map)
        }
        _ => Err(Error::BadValue("xref section head")),
    }
}

/// `(trailer dict, /Prev, /XRefStm)`.
type XrefResult = (Vec<(Vec<u8>, Obj)>, Option<usize>, Option<usize>);

/// Classic `xref` table: `start count` subsections of 20-byte records.
fn read_classic(lx: &mut Lexer, map: &mut BTreeMap<u32, Loc>) -> Result<XrefResult> {
    loop {
        lx.skip_ws();
        match lx.peek()? {
            Some(Tok::Kw(k)) if &*k == b"trailer" => {
                lx.next()?;
                let p = parse_standalone(lx.data, lx.pos)?;
                let dict = match p.obj {
                    Obj::Dict(kv) => kv,
                    _ => return Err(Error::BadValue("trailer is not a dict")),
                };
                let prev = dict_get(&dict, b"Prev").and_then(Obj::as_usize);
                let hyb = dict_get(&dict, b"XRefStm").and_then(Obj::as_usize);
                return Ok((dict, prev, hyb));
            }
            Some(Tok::Num(_)) => {
                let first = lx.expect_int("xref subsection start")?;
                let count = lx.expect_int("xref subsection count")?;
                if first < 0 || !(0..=(1i64 << 24)).contains(&count) {
                    return Err(Error::BadValue("xref subsection bounds"));
                }
                for i in 0..count as u64 {
                    let rec = read_classic_entry(lx)?;
                    if let Some((offset, generation)) = rec {
                        let num = (first as u64)
                            .checked_add(i)
                            .ok_or(Error::BadValue("xref number overflow"))?;
                        if num > u64::from(u32::MAX) {
                            return Err(Error::BadValue("xref object number"));
                        }
                        map.entry(num as u32)
                            .or_insert(Loc::Plain { offset, generation });
                    }
                }
            }
            Some(_) => return Err(Error::BadValue("xref subsection head")),
            None => {
                return Err(Error::Truncated {
                    what: "xref table",
                    needed: 1,
                    found: 0,
                });
            }
        }
    }
}

/// One 20-byte `n`/`f` record. `f` and bad lines return `None` (skipped);
/// malformed-but-present lines must not abort the whole table.
fn read_classic_entry(lx: &mut Lexer) -> Result<Option<(usize, u16)>> {
    // records are fixed-width; tokenize two ints + a flag char
    let save = lx.pos;
    let off = match lx.next()? {
        Some(Tok::Num(v)) => crate::lex::f64_as_i64(v),
        _ => {
            lx.pos = save;
            return Err(Error::BadValue("xref entry offset"));
        }
    };
    let generation = match lx.next()? {
        Some(Tok::Num(v)) => crate::lex::f64_as_i64(v),
        _ => {
            lx.pos = save;
            return Err(Error::BadValue("xref entry generation"));
        }
    };
    let flag = match lx.next()? {
        Some(Tok::Kw(k)) if k.len() == 1 => k[0],
        _ => {
            lx.pos = save;
            return Err(Error::BadValue("xref entry flag"));
        }
    };
    match (off, generation, flag) {
        (Some(o), Some(g), b'n') => {
            if o < 0 || g < 0 || g > i64::from(u16::MAX) {
                return Ok(None);
            }
            Ok(Some((o as usize, g as u16)))
        }
        (_, _, b'f') => Ok(None),
        _ => {
            lx.pos = save;
            Err(Error::BadValue("xref entry flag"))
        }
    }
}

/// Xref stream: `/W [w0 w1 w2]` + `/Index` runs over a decoded field stream.
fn read_xref_stream(
    p: crate::object::Parsed,
    limits: &modhash_inflate::Limits,
    map: &mut BTreeMap<u32, Loc>,
) -> Result<XrefResult> {
    let obj = p.obj;
    let dict = obj.dict().ok_or(Error::BadValue("xref stream dict"))?;
    // an xref stream must declare /Type /XRef
    match dict_get(dict, b"Type") {
        Some(Obj::Name(n)) if n.as_slice() == b"XRef" => {}
        _ => return Err(Error::BadValue("object at startxref is not /XRef")),
    }
    let w = dict_get(dict, b"W")
        .and_then(Obj::as_array)
        .ok_or(Error::BadValue("xref stream /W"))?;
    if w.len() != 3 {
        return Err(Error::BadValue("xref stream /W arity"));
    }
    let wv: Vec<i64> = w
        .iter()
        .map(|o| o.as_i64().ok_or(Error::BadValue("xref stream /W value")))
        .collect::<Result<_>>()?;
    for &x in &wv {
        if !(0..=8).contains(&x) {
            return Err(Error::BadValue("xref stream /W width"));
        }
    }
    let size = dict_get(dict, b"Size")
        .and_then(Obj::as_i64)
        .ok_or(Error::BadValue("xref stream /Size"))?;
    let index: Vec<(i64, i64)> = match dict_get(dict, b"Index") {
        Some(Obj::Arr(a)) => {
            if a.len() % 2 != 0 {
                return Err(Error::BadValue("xref stream /Index arity"));
            }
            let mut v = Vec::with_capacity(a.len() / 2);
            for pair in a.chunks_exact(2) {
                let lo = pair[0]
                    .as_i64()
                    .ok_or(Error::BadValue("xref stream /Index start"))?;
                let hi = pair[1]
                    .as_i64()
                    .ok_or(Error::BadValue("xref stream /Index count"))?;
                if lo < 0 || hi < 0 {
                    return Err(Error::BadValue("xref stream /Index bounds"));
                }
                v.push((lo, hi));
            }
            v
        }
        Some(_) => return Err(Error::BadValue("xref stream /Index type")),
        None => alloc::vec![(0, size)],
    };
    let data = decode_stream(&obj, limits)?;
    let rowlen = (wv[0] + wv[1] + wv[2]) as usize;
    if rowlen == 0 {
        return Err(Error::BadValue("xref stream /W row width"));
    }
    let need = index
        .iter()
        .map(|&(_, c)| c as usize)
        .try_fold(0usize, usize::checked_add)
        .and_then(|n| n.checked_mul(rowlen))
        .ok_or(Error::BadValue("xref stream entry count"))?;
    if data.len() < need {
        return Err(Error::Truncated {
            what: "xref stream entries",
            needed: need,
            found: data.len(),
        });
    }
    let mut cursor = 0usize;
    for &(first, count) in &index {
        for i in 0..count {
            let row = &data[cursor..cursor + rowlen];
            cursor += rowlen;
            let f0 = read_field(row, 0, wv[0] as usize, 1);
            let f1 = read_field(row, wv[0] as usize, wv[1] as usize, 0);
            let f2 = read_field(row, wv[0] as usize + wv[1] as usize, wv[2] as usize, 0);
            let num = first.checked_add(i).ok_or(Error::BadValue("xref num"))?;
            if num > i64::from(u32::MAX) {
                return Err(Error::BadValue("xref stream object number"));
            }
            match f0 {
                1 => {
                    if f1 > usize::MAX as u64 || f2 > u64::from(u16::MAX) {
                        continue;
                    }
                    map.entry(num as u32).or_insert(Loc::Plain {
                        offset: f1 as usize,
                        generation: f2 as u16,
                    });
                }
                2 => {
                    if f1 > u64::from(u32::MAX) || f2 > u64::from(u32::MAX) {
                        continue;
                    }
                    map.entry(num as u32).or_insert(Loc::InStm {
                        stm: f1 as u32,
                        idx: f2 as u32,
                    });
                }
                0 => {} // free
                _ => {} // type >2 reserved: ignore per spec
            }
        }
    }
    let prev = dict_get(dict, b"Prev").and_then(Obj::as_usize);
    Ok((dict.to_vec(), prev, None))
}

fn read_field(row: &[u8], at: usize, width: usize, default: u64) -> u64 {
    if width == 0 {
        return default;
    }
    let mut v = 0u64;
    for &b in &row[at..at + width] {
        v = v << 8 | u64::from(b);
    }
    v
}

/// Rebuild an xref by scanning for `N G obj` headers when `read_xref`
/// fails. The trailer is rebuilt from the first `trailer` dict found or, as
/// a last resort, from the `/Type /Catalog` object. This is a documented
/// recovery path, not a guess: every claim is verified by parsing the
/// object header at the scanned offset.
pub(crate) fn scan_xref(data: &[u8], _limits: &modhash_inflate::Limits) -> Result<Xref> {
    let mut map: BTreeMap<u32, Loc> = BTreeMap::new();
    let mut i = 0usize;
    while i + 4 < data.len() {
        // fast path: find ' obj' or 'obj' then walk back to the numbers
        if data[i].is_ascii_digit() {
            let s = i;
            let mut j = i;
            while j < data.len() && data[j].is_ascii_digit() {
                j += 1;
            }
            // need WS int WS int WS 'obj'
            if j < data.len() && crate::lex::is_ws(data[j]) {
                let mut k = j;
                while k < data.len() && crate::lex::is_ws(data[k]) {
                    k += 1;
                }
                let gs = k;
                while k < data.len() && data[k].is_ascii_digit() {
                    k += 1;
                }
                if gs < k && k < data.len() && crate::lex::is_ws(data[k]) {
                    let mut m = k;
                    while m < data.len() && crate::lex::is_ws(data[m]) {
                        m += 1;
                    }
                    if data[m..].starts_with(b"obj")
                        && (m + 3 >= data.len()
                            || crate::lex::is_ws(data[m + 3])
                            || crate::lex::is_delim(data[m + 3]))
                    {
                        let num: Option<u32> = ascii_uint(&data[s..j]);
                        let generation: Option<u16> = ascii_uint16(&data[gs..k]);
                        if let (Some(n), Some(g)) = (num, generation) {
                            // verify: the header parse must succeed, so we
                            // never claim an offset a parser would reject
                            if parse_obj(data, s).is_ok() {
                                map.insert(
                                    n,
                                    Loc::Plain {
                                        offset: s,
                                        generation: g,
                                    },
                                );
                            }
                        }
                        i = m + 3;
                        continue;
                    }
                }
            }
            i = j.max(s + 1);
        } else {
            i += 1;
        }
    }
    if map.is_empty() {
        return Err(Error::BadValue("no objects found while scanning"));
    }
    // trailer: prefer a real `trailer` dict, else synthesize from /Catalog
    let mut trailer: Option<Vec<(Vec<u8>, Obj)>> = None;
    let mut pos = 0usize;
    while let Some(off) = data[pos..].windows(7).position(|w| w == b"trailer") {
        let t_off = pos + off;
        // next non-ws char must begin a dict
        if let Ok(p) = parse_standalone(data, t_off + 7) {
            if let Obj::Dict(kv) = p.obj {
                if dict_get(&kv, b"Root").is_some() {
                    trailer = Some(kv);
                    break;
                }
                if trailer.is_none() {
                    trailer = Some(kv);
                }
            }
        }
        pos = t_off + 7;
    }
    if trailer.is_none() {
        // find the catalog object and synthesize a trailer
        let mut found: Option<u32> = None;
        for (&n, _) in map.iter() {
            if let Ok(p) = parse_obj(
                data,
                match map[&n] {
                    Loc::Plain { offset, .. } => offset,
                    Loc::InStm { .. } => continue,
                },
            ) {
                if matches!(p.obj.get(b"Type"), Some(Obj::Name(t)) if t.as_slice() == b"Catalog") {
                    found = Some(n);
                    break;
                }
            }
        }
        let root = found.ok_or(Error::BadValue("no catalog while scanning"))?;
        trailer = Some(alloc::vec![
            (
                b"Size".to_vec(),
                Obj::Num(f64::from(map.keys().max().copied().unwrap_or(0)) + 1.0)
            ),
            (
                b"Root".to_vec(),
                Obj::Ref(crate::object::Ref {
                    num: root,
                    generation: 0
                })
            ),
        ]);
    }
    Ok(Xref {
        map,
        trailer: trailer.expect("checked"),
        rebuilt: true,
    })
}

fn ascii_uint(b: &[u8]) -> Option<u32> {
    if b.is_empty() || b.len() > 10 {
        return None;
    }
    let mut v: u64 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + u64::from(c - b'0');
        if v > u64::from(u32::MAX) {
            return None;
        }
    }
    Some(v as u32)
}

fn ascii_uint16(b: &[u8]) -> Option<u16> {
    ascii_uint(b).and_then(|v| u16::try_from(v).ok())
}

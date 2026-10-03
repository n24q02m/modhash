//! Font decoding: from a font dictionary to "bytes in a `Tj` string ->
//! Unicode".
//!
//! Resolution order (PDF 32000-1 9.6.6, 9.10.2):
//!
//! 1. `/ToUnicode` CMap wins for every code it maps — this is the only
//!    reliable route for CID-keyed fonts and for non-standard encodings.
//! 2. `/Encoding`: a named encoding (`/WinAnsiEncoding`, `/MacRomanEncoding`,
//!    `/MacExpertEncoding`, `/StandardEncoding`, `/SymbolEncoding`,
//!    `/ZapfDingbatsEncoding`), an encoding dictionary (`/BaseEncoding` +
//!    `/Differences`), or (Type 0 fonts) a CMap name/`/Identity-H` or an
//!    embedded CMap stream.
//! 3. No `/Encoding`: the font's own built-in encoding — Symbol and
//!    ZapfDingbats have their own tables; the other base-14 and unknown
//!    base fonts fall back to StandardEncoding, which is what Adobe Reader
//!    does.
//!
//! `/Differences` glyph names resolve through the Adobe Glyph List table
//! (`tables::GLYPH_UNICODE`); `uniXXXX`/`uXXXXX[XX]` names decode directly.
//! An unresolvable code yields U+FFFD — the glyph is present but unmappable;
//! a *missing font entirely* is an error instead.

use alloc::string::String;
use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::cmap::{CMap, code_of_pub as code_of};
use crate::object::{Obj, Ref, decode_stream, dict_get};
use crate::tables;

/// One decoded slot of an 8-bit encoding table.
#[derive(Copy, Clone, Debug)]
pub(crate) enum Glyph {
    /// A static string from a predefined table or the AGL map.
    S(&'static str),
    /// A dynamically resolved character (`uniXXXX`/`uXXXXXX` differences).
    C(char),
}

/// How character codes map to Unicode for one font resource.
pub(crate) struct Font {
    /// Type0/CID-keyed font: codes use the CMap's codespace widths.
    pub(crate) cid: bool,
    /// Vertical writing (`/WMode 1` or `/Identity-V`): line breaks track
    /// horizontal displacement instead of vertical.
    pub(crate) vertical: bool,
    /// `/ToUnicode` cmap, when the font declares a usable one.
    pub(crate) touni: Option<CMap>,
    /// Codespace candidate lengths for CID code tokenizing (Identity-H ->
    /// 2-byte). Sorted longest-first. Only consulted for `cid` fonts.
    pub(crate) code_lens: Vec<usize>,
    /// Simple-font encoding: byte -> Unicode, after `/Differences` overlay.
    /// `None` for pure CID fonts without a byte table.
    pub(crate) enc: Option<[Option<Glyph>; 256]>,
}

/// Trait object for resolving indirect references while building a font —
/// implemented by [`Document`](crate::Document).
pub(crate) trait Resolve {
    /// Resolve one indirect reference to the object it names.
    fn deref(&self, r: Ref) -> Result<Obj>;
}

/// Build a [`Font`] from a `/Font` resource dictionary.
pub(crate) fn build<R: Resolve>(
    dict: &[(Vec<u8>, Obj)],
    res: &R,
    limits: &modhash_inflate::Limits,
) -> Result<Font> {
    let subtype = match dict_get(dict, b"Subtype") {
        Some(Obj::Name(n)) => n.as_slice(),
        _ => return Err(Error::BadValue("font /Subtype")),
    };
    let base = dict_get(dict, b"BaseFont")
        .and_then(|o| o.as_bytes())
        .unwrap_or(b"")
        .to_vec();
    let touni = load_tounicode(dict, res, limits)?;

    if subtype == b"Type0" {
        return build_cid(dict, res, limits, touni);
    }

    // simple fonts: Type1, MMType1, TrueType, Type3
    if !matches!(subtype, b"Type1" | b"MMType1" | b"TrueType" | b"Type3") {
        return Err(Error::Unsupported("font subtype"));
    }
    let mut table = simple_base_table(&base, subtype);
    if let Some(enc) = dict_get(dict, b"Encoding") {
        apply_encoding(&mut table, enc, res, limits)?;
    }
    Ok(Font {
        cid: false,
        vertical: false,
        touni,
        code_lens: alloc::vec![1],
        enc: Some(table),
    })
}

/// The default encoding for a simple font. Symbol and ZapfDingbats use
/// their own built-ins; Type3 falls through to Standard as the
/// least-wrong default (its Encoding dict is required anyway).
///
/// Every other non-symbolic base-14 or unknown base — Type1, MMType1,
/// TrueType — resolves through **WinAnsiEncoding**, not StandardEncoding.
/// The spec leaves the built-in encoding of the standard-14 fonts open
/// for bytes ≥ 0x80, and every mainstream consumer (Acrobat, pypdf,
/// MuPDF) reads them as WinAnsi: StandardEncoding marks those codes
/// `.notdef` and would silently corrupt text like `naïve` → `na<U+FFFD>ve`
/// (diff-oracle parity finding, 2026-10-03). An explicit `/Encoding` or
/// `/ToUnicode` still wins — this is only the no-Encoding fallback and
/// the base for a `/Differences` overlay.
fn simple_base_table(base: &[u8], subtype: &[u8]) -> [Option<Glyph>; 256] {
    if subtype == b"Type3" {
        return fill(tables::STANDARD_ENCODING);
    }
    match base {
        b"Symbol" => fill(tables::SYMBOL_ENCODING),
        b"ZapfDingbats" => fill(tables::ZAPFDINGBATS_ENCODING),
        _ => fill(tables::WINANSI_ENCODING),
    }
}

fn fill(t: &'static [(u8, &'static str)]) -> [Option<Glyph>; 256] {
    let mut out = [None; 256];
    for &(c, s) in t {
        out[c as usize] = Some(Glyph::S(s));
    }
    out
}

/// Apply `/Encoding` (name or dict with /BaseEncoding + /Differences).
fn apply_encoding<R: Resolve>(
    table: &mut [Option<Glyph>; 256],
    enc: &Obj,
    res: &R,
    _limits: &modhash_inflate::Limits,
) -> Result<()> {
    let enc = match enc {
        Obj::Ref(r) => res.deref(*r)?,
        other => other.clone(),
    };
    match &enc {
        Obj::Name(n) => {
            *table = named_table(n.as_slice()).ok_or(Error::Unsupported("encoding name"))?;
        }
        Obj::Dict(d) => {
            if let Some(b) = dict_get(d, b"BaseEncoding") {
                let n = b.as_bytes().ok_or(Error::BadValue("/BaseEncoding"))?;
                *table = named_table(n).ok_or(Error::Unsupported("BaseEncoding name"))?;
            }
            if let Some(Obj::Arr(diff)) = dict_get(d, b"Differences") {
                let mut code = 0usize;
                for o in diff {
                    match o {
                        Obj::Num(v) => {
                            let c = crate::lex::f64_as_i64(*v)
                                .ok_or(Error::BadValue("/Differences number"))?;
                            if !(0..=255).contains(&c) {
                                return Err(Error::BadValue("/Differences number"));
                            }
                            code = c as usize;
                        }
                        Obj::Name(n) => {
                            if code > 255 {
                                return Err(Error::BadValue("/Differences range"));
                            }
                            table[code] = glyph_lookup(n.as_slice());
                            code += 1;
                        }
                        _ => return Err(Error::BadValue("/Differences element")),
                    }
                }
            }
            // /Type /Encoding is optional on the dict; a bare Differences
            // dict is accepted.
        }
        _ => return Err(Error::BadValue("/Encoding type")),
    }
    Ok(())
}

fn named_table(name: &[u8]) -> Option<[Option<Glyph>; 256]> {
    Some(match name {
        b"StandardEncoding" => fill(tables::STANDARD_ENCODING),
        b"WinAnsiEncoding" => fill(tables::WINANSI_ENCODING),
        b"MacRomanEncoding" => fill(tables::MACROMAN_ENCODING),
        b"SymbolEncoding" => fill(tables::SYMBOL_ENCODING),
        b"ZapfDingbatsEncoding" => fill(tables::ZAPFDINGBATS_ENCODING),
        b"PDFDocEncoding" => fill(tables::PDFDOC_ENCODING),
        _ => return None,
    })
}

/// Resolve a `/Differences` glyph name to Unicode:
/// `uniXXXX` / `uXXXXX[XX]` parse directly, otherwise the AGL table.
fn glyph_lookup(name: &[u8]) -> Option<Glyph> {
    if let Some(rest) = name.strip_prefix(b"uni") {
        if rest.len() >= 4 {
            let mut v: u32 = 0;
            let mut ok = true;
            for &b in rest.iter().take(4) {
                match crate::lex::hex_val(b) {
                    Some(d) => v = v * 16 + u32::from(d),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                if let Some(c) = char::from_u32(v) {
                    return Some(Glyph::C(c));
                }
            }
        }
    }
    if let Some(rest) = name.strip_prefix(b"u") {
        if rest.len() >= 4 && rest.len() <= 6 {
            let mut v: u32 = 0;
            let mut ok = true;
            for &b in rest {
                match crate::lex::hex_val(b) {
                    Some(d) => v = v * 16 + u32::from(d),
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                if let Some(c) = char::from_u32(v) {
                    return Some(Glyph::C(c));
                }
            }
        }
    }
    tables::glyph_unicode(name).map(Glyph::S)
}

/// CID-keyed (`/Subtype /Type0`) font.
fn build_cid<R: Resolve>(
    dict: &[(Vec<u8>, Obj)],
    res: &R,
    limits: &modhash_inflate::Limits,
    touni: Option<CMap>,
) -> Result<Font> {
    // descendant fonts must exist and be a CIDFont
    match dict_get(dict, b"DescendantFonts") {
        Some(Obj::Arr(a)) if !a.is_empty() => {
            let d = match &a[0] {
                Obj::Ref(r) => res.deref(*r)?,
                other => other.clone(),
            };
            match d.get(b"Subtype") {
                Some(Obj::Name(n))
                    if n.as_slice() == b"CIDFontType0" || n.as_slice() == b"CIDFontType2" => {}
                Some(_) => return Err(Error::Unsupported("descendant font subtype")),
                None => return Err(Error::BadValue("CID font descendant")),
            }
        }
        _ => return Err(Error::BadValue("CID font /DescendantFonts")),
    }
    // /Encoding: name (Identity-H/V, predefined CMaps) or CMap stream
    let mut code_lens: Vec<usize> = alloc::vec![2];
    let mut vertical = false;
    match dict_get(dict, b"Encoding") {
        None | Some(Obj::Null) => {}
        Some(Obj::Name(n)) => match n.as_slice() {
            b"Identity-H" => {}
            b"Identity-V" => vertical = true,
            _ => {
                // predefined CMaps (UniGB-*, UniJIS-*, ...) still use 2-byte
                // codes; text extraction runs on ToUnicode, so the name
                // only fixes the code width
            }
        },
        Some(enc @ Obj::Ref(_)) | Some(enc @ Obj::Stream { .. }) => {
            let enc = match enc {
                Obj::Ref(r) => res.deref(*r)?,
                other => other.clone(),
            };
            let raw =
                decode_stream(&enc, limits).map_err(|_| Error::BadValue("CMap stream decode"))?;
            let cmap = CMap::parse(&raw).map_err(|_| Error::BadValue("CMap stream parse"))?;
            vertical = cmap.is_vertical();
            code_lens = cmap_lens(&cmap);
            if code_lens.is_empty() {
                code_lens.push(2);
            }
        }
        Some(_) => return Err(Error::BadValue("/Encoding for CID font")),
    }
    // merge ToUnicode's code lengths too (sloppy CMaps use more)
    if let Some(t) = touni.as_ref() {
        for &l in &cmap_lens(t) {
            if !code_lens.contains(&l) {
                code_lens.push(l);
            }
        }
        code_lens.sort_unstable_by(|a, b| b.cmp(a));
    }
    if touni.is_none() {
        // Without ToUnicode a CID code cannot reach Unicode. We refuse to
        // guess: CID numbers are glyph ids, not codepoints, even under
        // Identity ordering.
        return Err(Error::Unsupported("CID font without /ToUnicode"));
    }
    Ok(Font {
        cid: true,
        vertical,
        touni,
        code_lens,
        enc: None,
    })
}

/// Sorted (longest-first) code lengths of a CMap — exposed for the CID
/// path which tokenizes the input by the *Encoding* cmap's codespaces.
fn cmap_lens(c: &CMap) -> Vec<usize> {
    c.lens().to_vec()
}

/// Load `/ToUnicode` (a stream reference or inline stream).
fn load_tounicode<R: Resolve>(
    dict: &[(Vec<u8>, Obj)],
    res: &R,
    limits: &modhash_inflate::Limits,
) -> Result<Option<CMap>> {
    let o = match dict_get(dict, b"ToUnicode") {
        None | Some(Obj::Null) => return Ok(None),
        Some(o) => o.clone(),
    };
    let o = match o {
        Obj::Ref(r) => res.deref(r)?,
        other => other,
    };
    if !matches!(o, Obj::Stream { .. }) {
        return Err(Error::BadValue("/ToUnicode is not a stream"));
    }
    let raw = decode_stream(&o, limits).map_err(|_| Error::BadValue("/ToUnicode stream decode"))?;
    let cmap = CMap::parse(&raw).map_err(|_| Error::BadValue("/ToUnicode parse"))?;
    Ok(Some(cmap))
}

impl Font {
    /// Decode one string's bytes to Unicode text for this font.
    ///
    /// Unmappable codes become U+FFFD (documented choice: the byte exists,
    /// we emit the replacement char rather than guessing or dropping). With
    /// the WinAnsi fallback on non-symbolic base fonts this is rare —
    /// mostly Symbol/Zapf codes outside their tables or a corrupt stream.
    pub(crate) fn decode(&self, bytes: &[u8]) -> String {
        let mut out = String::new();
        if self.cid {
            let mut pos = 0usize;
            while pos < bytes.len() {
                // tokenize by declared code lengths, longest first
                let mut matched = false;
                if let Some(t) = self.touni.as_ref() {
                    // longest-prefix match against the cmap
                    for &l in &self.code_lens {
                        if l <= bytes.len() - pos {
                            let code = code_of(&bytes[pos..pos + l]);
                            if let Some(s) = t.get(code, l) {
                                out.push_str(s);
                                pos += l;
                                matched = true;
                                break;
                            }
                        }
                    }
                }
                if !matched {
                    // advance by the shortest declared length (or 1)
                    let step = self
                        .code_lens
                        .last()
                        .copied()
                        .unwrap_or(1)
                        .min(bytes.len() - pos);
                    out.push(char::REPLACEMENT_CHARACTER);
                    pos += step;
                }
            }
            return out;
        }
        for &b in bytes {
            if let Some(t) = self.touni.as_ref() {
                if let Some(s) = t.get(u64::from(b), 1) {
                    out.push_str(s);
                    continue;
                }
            }
            match self.enc.as_ref().and_then(|t| t[b as usize]) {
                Some(Glyph::S(s)) => out.push_str(s),
                Some(Glyph::C(c)) => out.push(c),
                None => out.push(char::REPLACEMENT_CHARACTER),
            }
        }
        out
    }
}

/// Helper used by tests and the document walker: the current font's
/// code-length list (for state display/debug).
#[allow(dead_code)]
pub(crate) fn debug_lens(f: &Font) -> &[usize] {
    &f.code_lens
}

//! Content-stream tokenizing and the text-showing state machine.
//!
//! A page's `/Contents` is one or more streams of *operands* followed by
//! *operators*. Only the text-showing and text-positioning operators
//! matter for extraction:
//!
//! - `BT`/`ET` bracket a text object. Positioning state resets per text
//!   object but the pending-line-break flag does not, so a stream split
//!   across `ET`/`BT` keeps line structure.
//! - `Tf` selects a font; `Tj`, `TJ`, `'` and `"` show strings. `'` is
//!   `T*` + `Tj`; `"` is `Tw` + `Tc` + `'`.
//! - `Td`/`TD`/`Tm`/`T*` move the text point: a non-zero vertical move ends
//!   the current line; a pure horizontal move of a large positive step or
//!   the space-width step emits a space.
//! - `TJ` arrays mix strings and displacements: a number **below -250**
//!   (moves the next glyph right by more than a quarter em) starts a new
//!   word. The threshold is documented in `docs/` and covered by fixtures.
//! - `Do` on a `/Subtype /Form` XObject recurses into its stream with the
//!   form's own resources (inheriting the page's where the form lacks any).
//!
//! Other operators are consumed and ignored, including `BI`/`ID`/`EI`
//! inline images (their binary data may contain arbitrary bytes, so `ID`
//! data is skipped by scanning for `EI` on a token boundary).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::font::{Font, Resolve};
use crate::lex::{Lexer, Tok};
use crate::object::{Obj, decode_stream, dict_get};

/// TJ kerning threshold: a TJ number strictly below `-TJ_GAP` ends a word.
const TJ_GAP: f64 = 250.0;

trait AbsManual {
    /// f64::abs is std-only under no_std.
    fn abs_manual(self) -> f64;
}
impl AbsManual for f64 {
    fn abs_manual(self) -> f64 {
        if self < 0.0 { -self } else { self }
    }
}
/// Tm/Td horizontal move (in text space units) that reads as a space.
const X_STEP: f64 = 2.0;
/// Hard cap on XObject recursion.
const MAX_DO_DEPTH: usize = 16;

/// Resources visible at one text position: `/Font` and `/XObject` dicts
/// plus inherited `/Resources` already merged by the caller.
#[derive(Clone)]
pub(crate) struct Resources {
    /// `/Font` dictionary entries.
    pub(crate) fonts: Vec<(Vec<u8>, Obj)>,
    /// `/XObject` dictionary entries.
    pub(crate) xobjects: Vec<(Vec<u8>, Obj)>,
}

impl Resources {
    pub(crate) fn from_dict(dict: Option<&[(Vec<u8>, Obj)]>) -> Resources {
        let mut fonts = Vec::new();
        let mut xobjects = Vec::new();
        if let Some(d) = dict {
            if let Some(f) = dict_get(d, b"Font").and_then(Obj::dict) {
                fonts = f.to_vec();
            }
            if let Some(x) = dict_get(d, b"XObject").and_then(Obj::dict) {
                xobjects = x.to_vec();
            }
        }
        Resources { fonts, xobjects }
    }

    fn merge(&self, over: Option<&[(Vec<u8>, Obj)]>) -> Resources {
        // form resources override page resources by name
        let other = Resources::from_dict(over);
        let mut fonts = self.fonts.clone();
        let mut xobjects = self.xobjects.clone();
        for (k, v) in other.fonts {
            fonts.retain(|(ek, _)| *ek != k);
            fonts.push((k, v));
        }
        for (k, v) in other.xobjects {
            xobjects.retain(|(ek, _)| *ek != k);
            xobjects.push((k, v));
        }
        Resources { fonts, xobjects }
    }
}

struct TextState {
    font_name: Option<Vec<u8>>,
    /// Current font is vertical-writing: Td/Tm axes swap meaning.
    vertical: bool,
    in_text: bool,
    pending_nl: bool,
    pending_sp: bool,
    /// Whether any text has been emitted for this page yet (suppresses
    /// leading breaks from the first `Td`/`Tm`).
    emitted: bool,
    /// Last `Tm` text position `(e, f)` and whether we have one, for
    /// relative positioning of the next `Tm`.
    last_tm: Option<(f64, f64)>,
}

/// Decode one page's content: font table + concatenated stream text.
///
/// `contents` is the list of content-stream objects (already deref'd and
/// decoded). Errors in any stream propagate; the caller wraps them with the
/// page index.
pub(crate) fn extract_page_text<R: Resolve>(
    streams: &[Vec<u8>],
    res: &Resources,
    resolver: &R,
    limits: &modhash_inflate::Limits,
) -> Result<String> {
    let mut out = String::new();
    let mut fonts: BTreeMap<Vec<u8>, Font> = BTreeMap::new();
    let mut st = TextState {
        font_name: None,
        vertical: false,
        in_text: false,
        pending_nl: false,
        pending_sp: false,
        emitted: false,
        last_tm: None,
    };
    for s in streams {
        run_stream(s, res, resolver, limits, &mut st, &mut fonts, &mut out, 0)?;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn run_stream<R: Resolve>(
    data: &[u8],
    res: &Resources,
    resolver: &R,
    limits: &modhash_inflate::Limits,
    st: &mut TextState,
    fonts: &mut BTreeMap<Vec<u8>, Font>,
    out: &mut String,
    depth: usize,
) -> Result<()> {
    if depth > MAX_DO_DEPTH {
        return Err(Error::TooLarge {
            what: "XObject recursion",
            limit: MAX_DO_DEPTH,
        });
    }
    let mut lx = Lexer::new(data);
    let mut ops: Vec<Tok> = Vec::new();
    while let Some(t) = lx.next()? {
        match t {
            Tok::Kw(kw) => {
                handle_op(
                    &kw, &mut ops, &mut lx, res, resolver, limits, st, fonts, out, depth,
                )?;
                ops.clear();
            }
            _ => {
                ops.push(t);
                if ops.len() > 1024 {
                    return Err(Error::BadValue("operand stack depth"));
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_op<R: Resolve>(
    kw: &[u8],
    ops: &mut Vec<Tok>,
    lx: &mut Lexer,
    res: &Resources,
    resolver: &R,
    limits: &modhash_inflate::Limits,
    st: &mut TextState,
    fonts: &mut BTreeMap<Vec<u8>, Font>,
    out: &mut String,
    depth: usize,
) -> Result<()> {
    match kw {
        b"BT" => {
            st.in_text = true;
            st.last_tm = None;
            // pending_nl survives: text split across BT/ET keeps its line
        }
        b"ET" => st.in_text = false,
        b"Tf" => {
            let name = match ops.as_slice() {
                [.., Tok::Name(n), Tok::Num(_)] => n.clone().into_owned(),
                _ => return Err(Error::BadValue("Tf operands")),
            };
            st.font_name = Some(name);
            // resolve eagerly: writing mode affects Td/Tm interpretation
            // before the next show
            st.vertical = font_for(st, res, resolver, limits, fonts)
                .map(|f| f.vertical)
                .unwrap_or(false);
        }
        b"Td" | b"TD" => {
            let (tx, ty) = two_nums(ops, "Td")?;
            position_delta(st, out, tx, ty);
        }
        b"Tm" => {
            let (a, b, c, d, e, f) = six_nums(ops)?;
            position_matrix(st, out, a, b, c, d, e, f);
        }
        b"T*" => {
            if st.emitted {
                st.pending_nl = true;
            }
        }
        b"'" => {
            // ' = T* then Tj
            let s = take_str_op(ops, "'")?;
            flush_break(st, out, true);
            show(s, st, res, resolver, limits, fonts, out)?;
        }
        b"\"" => {
            // " = Tw Tc then ' (we ignore spacing ops)
            match ops.as_slice() {
                [.., Tok::Num(_), Tok::Num(_), Tok::Str(s)] => {
                    let s = s.clone().into_owned();
                    flush_break(st, out, true);
                    show(&s, st, res, resolver, limits, fonts, out)?;
                }
                _ => return Err(Error::BadValue("\" operands")),
            }
        }
        b"Tj" => {
            let s = take_str_op(ops, "Tj")?;
            flush_break(st, out, false);
            show(s, st, res, resolver, limits, fonts, out)?;
        }
        b"TJ" => {
            let arr = match ops.last() {
                Some(Tok::ArrOpen) => None, // can't happen; arrays arrive whole
                _ => ops.last(),
            };
            // TJ operand is a raw array token group; we collect it below.
            let _ = arr;
            show_tj(ops, st, res, resolver, limits, fonts, out)?;
        }
        b"Do" => {
            let name = match ops.last() {
                Some(Tok::Name(n)) => n.clone().into_owned(),
                _ => return Err(Error::BadValue("Do operand")),
            };
            let xo = res
                .xobjects
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone());
            let xo = match xo {
                Some(Obj::Ref(r)) => Some(resolver.deref(r)?),
                other => other,
            };
            if let Some(xo) = xo {
                if let Obj::Stream { dict, .. } = &xo {
                    if matches!(dict_get(dict, b"Subtype"), Some(Obj::Name(n)) if n.as_slice() == b"Form")
                    {
                        if st.emitted {
                            st.pending_nl = true;
                        }
                        let inner = decode_stream(&xo, limits)?;
                        let fres = res.merge(dict_get(dict, b"Resources").and_then(Obj::dict));
                        run_stream(&inner, &fres, resolver, limits, st, fonts, out, depth + 1)?;
                        st.pending_nl = true;
                    }
                    // Image/other XObjects carry no extractable text
                }
            }
        }
        b"ID" => {
            // inline image: skip its raw data to the 'EI' token boundary
            skip_inline_image(lx)?;
        }
        _ => {} // every other operator: consume operands, ignore
    }
    Ok(())
}

fn two_nums(ops: &[Tok], what: &'static str) -> Result<(f64, f64)> {
    match ops {
        [.., Tok::Num(x), Tok::Num(y)] => Ok((*x, *y)),
        _ => Err(Error::BadValue(what)),
    }
}

fn six_nums(ops: &[Tok]) -> Result<(f64, f64, f64, f64, f64, f64)> {
    match ops {
        [
            ..,
            Tok::Num(a),
            Tok::Num(b),
            Tok::Num(c),
            Tok::Num(d),
            Tok::Num(e),
            Tok::Num(f),
        ] => Ok((*a, *b, *c, *d, *e, *f)),
        _ => Err(Error::BadValue("Tm operands")),
    }
}

fn take_str_op<'o>(ops: &'o mut [Tok], what: &'static str) -> Result<&'o [u8]> {
    match ops.last() {
        Some(Tok::Str(s)) => Ok(s),
        _ => Err(Error::BadValue(what)),
    }
}

/// `Td`/`TD`: a move on the line axis ends the line (ty for horizontal
/// writing, tx for vertical); a sizable move on the inline axis reads as a
/// space.
fn position_delta(st: &mut TextState, out: &mut String, tx: f64, ty: f64) {
    if !st.emitted {
        return;
    }
    let (line_move, inline_move) = if st.vertical { (tx, ty) } else { (ty, tx) };
    if line_move != 0.0 {
        st.pending_nl = true;
        st.pending_sp = false;
    } else if inline_move.abs_manual() >= X_STEP {
        st.pending_sp = true;
    }
    let _ = out;
}

/// `Tm` sets the text matrix; compare (e,f) against the previous line
/// position for the line/space decision.
#[allow(clippy::too_many_arguments)]
fn position_matrix(
    st: &mut TextState,
    _out: &mut str,
    _a: f64,
    _b: f64,
    _c: f64,
    _d: f64,
    e: f64,
    f: f64,
) {
    if let Some((le, lf)) = st.last_tm {
        if st.emitted {
            let dy = if f >= lf { f - lf } else { lf - f };
            let dx = e - le;
            let (line_move, inline_move) = if st.vertical { (dx, dy) } else { (dy, dx) };
            if line_move.abs_manual() > 0.01 {
                st.pending_nl = true;
                st.pending_sp = false;
            } else if inline_move >= X_STEP {
                st.pending_sp = true;
            }
        }
    }
    st.last_tm = Some((e, f));
}

/// Emit the pending break (`nl` = unconditional newline for `'`/`"`).
fn flush_break(st: &mut TextState, out: &mut String, nl: bool) {
    if !st.emitted {
        st.pending_nl = false;
        st.pending_sp = false;
        return;
    }
    if st.pending_nl || nl {
        if !out.ends_with('\n') {
            out.push('\n');
        }
    } else if st.pending_sp && !out.ends_with(|c: char| c.is_whitespace()) {
        out.push(' ');
    }
    st.pending_nl = false;
    st.pending_sp = false;
}

fn show<R: Resolve>(
    s: &[u8],
    st: &mut TextState,
    res: &Resources,
    resolver: &R,
    limits: &modhash_inflate::Limits,
    fonts: &mut BTreeMap<Vec<u8>, Font>,
    out: &mut String,
) -> Result<()> {
    let font = font_for(st, res, resolver, limits, fonts)?;
    let text = font.decode(s);
    if !text.is_empty() {
        out.push_str(&text);
        st.emitted = true;
    }
    Ok(())
}

fn show_tj<R: Resolve>(
    ops: &[Tok],
    st: &mut TextState,
    res: &Resources,
    resolver: &R,
    limits: &modhash_inflate::Limits,
    fonts: &mut BTreeMap<Vec<u8>, Font>,
    out: &mut String,
) -> Result<()> {
    // ops holds: ... ArrOpen, elements..., ArrClose
    let arr = match ops.last() {
        Some(Tok::ArrClose) => ops,
        _ => return Err(Error::BadValue("TJ operand")),
    };
    let font = font_for(st, res, resolver, limits, fonts)?;
    flush_break(st, out, false);
    for t in arr.iter() {
        match t {
            Tok::Str(s) => {
                let text = font.decode(s);
                if !text.is_empty() {
                    out.push_str(&text);
                    st.emitted = true;
                }
            }
            Tok::Num(n)
                if *n < -TJ_GAP && st.emitted && !out.ends_with(|c: char| c.is_whitespace()) =>
            {
                out.push(' ');
            }
            _ => {}
        }
    }
    Ok(())
}

fn font_for<'f, R: Resolve>(
    st: &TextState,
    res: &Resources,
    resolver: &R,
    limits: &modhash_inflate::Limits,
    fonts: &'f mut BTreeMap<Vec<u8>, Font>,
) -> Result<&'f Font> {
    let name = st
        .font_name
        .as_ref()
        .ok_or(Error::BadValue("text shown before Tf"))?;
    if !fonts.contains_key(name) {
        let fobj = res
            .fonts
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .ok_or(Error::BadValue("font resource missing"))?;
        let fobj = match fobj {
            Obj::Ref(r) => resolver.deref(r)?,
            other => other,
        };
        let dict = fobj.dict().ok_or(Error::BadValue("font is not a dict"))?;
        let f = crate::font::build(dict, resolver, limits)?;
        fonts.insert(name.clone(), f);
    }
    Ok(fonts.get(name).expect("just inserted"))
}

/// After `ID`: raw image data until a whitespace-delimited `EI`. The scan
/// honors the spec: data runs to the first `EI` preceded by a whitespace
/// byte and followed by whitespace/delimiter/EOF.
fn skip_inline_image(lx: &mut Lexer) -> Result<()> {
    // the lexer is positioned just after the 'ID' token; spec demands one
    // whitespace byte before data
    let rest = lx.rest();
    let mut i = 0usize;
    // skip the single whitespace separator after ID
    if i < rest.len() && crate::lex::is_ws(rest[i]) {
        i += 1;
    }
    while i + 1 < rest.len() {
        if rest[i] == b'E' && rest[i + 1] == b'I' {
            let before_ok = i == 0 || crate::lex::is_ws(rest[i - 1]);
            let after_ok = i + 2 >= rest.len()
                || crate::lex::is_ws(rest[i + 2])
                || crate::lex::is_delim(rest[i + 2]);
            if before_ok && after_ok {
                lx.pos += i + 2;
                return Ok(());
            }
        }
        i += 1;
    }
    Err(Error::Truncated {
        what: "inline image EI",
        needed: 1,
        found: 0,
    })
}

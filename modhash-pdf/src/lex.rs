//! The object-level lexer: bytes to tokens.
//!
//! A `Lexer` is a cursor over a byte slice that yields PDF tokens: numbers,
//! names, literal and hex strings, the `<<`/`>>`/`[`/`]` delimiters and
//! keywords (any other regular-token word, e.g. `obj`, `Tj`, `trailer`).
//! It knows nothing about what the tokens mean; [`crate::object`] builds
//! values out of them.
//!
//! Whitespace is NUL, TAB, LF, FF, CR and SPACE (spec Table 1); comments run
//! from `%` to end of line. End of input is `Ok(None)` everywhere; every
//! malformed construct is a named [`Error`], never a panic.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

/// One lexical token. Strings are already escape-decoded; names are
/// `#xx`-decoded.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tok {
    /// Integer or real number. Integer callers use [`f64_as_i64`].
    Num(f64),
    /// A `/Name`, decoded ( `#xx` sequences expanded).
    Name(Cow<'static, [u8]>),
    /// A `(literal)` or `<hex>` string, fully decoded to bytes.
    Str(Cow<'static, [u8]>),
    /// `[`
    ArrOpen,
    /// `]`
    ArrClose,
    /// `<<`
    DictOpen,
    /// `>>`
    DictClose,
    /// Any other regular-token word: `obj`, `stream`, `Tj`, `trailer`, ...
    Kw(Cow<'static, [u8]>),
}

/// `true` for the six PDF whitespace bytes.
pub(crate) fn is_ws(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// `true` for the ten delimiter characters that terminate a regular token.
pub(crate) fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Byte cursor over one PDF byte range.
pub(crate) struct Lexer<'a> {
    /// The source.
    pub(crate) data: &'a [u8],
    /// Current read position.
    pub(crate) pos: usize,
}

impl<'a> Lexer<'a> {
    /// Wrap a byte range.
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Lexer { data, pos: 0 }
    }

    /// Skip whitespace and `%` comments.
    pub(crate) fn skip_ws(&mut self) {
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if is_ws(b) {
                self.pos += 1;
            } else if b == b'%' {
                while self.pos < self.data.len()
                    && self.data[self.pos] != 0x0A
                    && self.data[self.pos] != 0x0D
                {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// Bytes remaining after the cursor.
    pub(crate) fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    /// Read the next token. `None` at end of input (after whitespace).
    ///
    /// Numbers are `+/-`? digits `.`?; a leading `.` is also a number.
    /// Anything else starting a regular token is a keyword.
    pub(crate) fn next(&mut self) -> Result<Option<Tok>> {
        self.skip_ws();
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        let b = self.data[self.pos];
        match b {
            b'[' => {
                self.pos += 1;
                Ok(Some(Tok::ArrOpen))
            }
            b']' => {
                self.pos += 1;
                Ok(Some(Tok::ArrClose))
            }
            b'<' => {
                if self.data.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    Ok(Some(Tok::DictOpen))
                } else {
                    self.hex_string().map(Some)
                }
            }
            b'>' => {
                if self.data.get(self.pos + 1) == Some(&b'>') {
                    self.pos += 2;
                    Ok(Some(Tok::DictClose))
                } else {
                    Err(Error::BadValue("lone '>' in object stream"))
                }
            }
            b'(' => self.literal_string().map(Some),
            b')' | b'{' | b'}' => {
                // stray closing delimiters carry no payload; surface each as
                // its own keyword so grammar checks fail on it instead of
                // returning a zero-width token that would loop the caller
                let t = self.data[self.pos..self.pos + 1].to_vec();
                self.pos += 1;
                Ok(Some(Tok::Kw(Cow::Owned(t))))
            }
            b'/' => self.name().map(Some),
            _ if b == b'+' || b == b'-' || b == b'.' || b.is_ascii_digit() => {
                self.number_or_word().map(Some)
            }
            _ => {
                // regular token: run until ws/delimiter
                let start = self.pos;
                while self.pos < self.data.len() {
                    let c = self.data[self.pos];
                    if is_ws(c) || is_delim(c) {
                        break;
                    }
                    self.pos += 1;
                }
                Ok(Some(Tok::Kw(Cow::Owned(
                    self.data[start..self.pos].to_vec(),
                ))))
            }
        }
    }

    fn name(&mut self) -> Result<Tok> {
        self.pos += 1; // '/'
        let start = self.pos;
        let mut out: Option<Vec<u8>> = None;
        while self.pos < self.data.len() {
            let c = self.data[self.pos];
            if is_ws(c) || is_delim(c) {
                break;
            }
            if c == b'#' {
                // #xx hex escape inside a name
                if out.is_none() {
                    out = Some(self.data[start..self.pos].to_vec());
                }
                let v = self
                    .data
                    .get(self.pos + 1..self.pos + 3)
                    .and_then(|h| hex_val(h[0]).zip(hex_val(h[1])))
                    .map(|(a, b)| a * 16 + b);
                match v {
                    Some(byte) => {
                        out.as_mut().expect("initialized").push(byte);
                        self.pos += 3;
                    }
                    None => {
                        return Err(Error::BadValue("name #xx escape"));
                    }
                }
            } else {
                if let Some(v) = out.as_mut() {
                    v.push(c);
                }
                self.pos += 1;
            }
        }
        let name = match out {
            Some(v) => Cow::Owned(v),
            None => Cow::Owned(self.data[start..self.pos].to_vec()),
        };
        Ok(Tok::Name(name))
    }

    fn literal_string(&mut self) -> Result<Tok> {
        self.pos += 1; // '('
        let mut out = Vec::new();
        let mut depth = 1usize;
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    if depth > 64 {
                        return Err(Error::BadValue("literal string nesting"));
                    }
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Tok::Str(Cow::Owned(out)));
                    }
                    out.push(b);
                }
                b'\\' => {
                    let n = match self.data.get(self.pos) {
                        Some(&n) => n,
                        None => {
                            return Err(Error::Truncated {
                                what: "string escape",
                                needed: 1,
                                found: 0,
                            });
                        }
                    };
                    self.pos += 1;
                    match n {
                        b'n' => out.push(0x0A),
                        b'r' => out.push(0x0D),
                        b't' => out.push(0x09),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'(' | b')' | b'\\' => out.push(n),
                        b'0'..=b'7' => {
                            let mut v = (n - b'0') as u32;
                            for _ in 0..2 {
                                match self.data.get(self.pos) {
                                    Some(&d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xFF) as u8);
                        }
                        0x0D => {
                            // line continuation; swallow LF after CR
                            if self.data.get(self.pos) == Some(&0x0A) {
                                self.pos += 1;
                            }
                        }
                        0x0A => {}
                        other => out.push(other), // spec: unknown escape = literal
                    }
                }
                _ => out.push(b),
            }
        }
        Err(Error::Truncated {
            what: "literal string",
            needed: 1,
            found: 0,
        })
    }

    fn hex_string(&mut self) -> Result<Tok> {
        self.pos += 1; // '<'
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            if b == b'>' {
                if let Some(h) = hi {
                    out.push(h << 4); // odd count: pad low nibble with 0
                }
                return Ok(Tok::Str(Cow::Owned(out)));
            }
            if is_ws(b) {
                continue;
            }
            match hex_val(b) {
                Some(v) => match hi.take() {
                    Some(h) => out.push((h << 4) | v),
                    None => hi = Some(v),
                },
                None => return Err(Error::BadValue("hex string character")),
            }
        }
        Err(Error::Truncated {
            what: "hex string",
            needed: 1,
            found: 0,
        })
    }

    fn number_or_word(&mut self) -> Result<Tok> {
        let start = self.pos;
        // number regex: [+-]? (digits | digits? '.' digits?)
        let mut is_num = true;
        let mut seen_dot = false;
        let mut seen_digit = false;
        if matches!(self.data[self.pos], b'+' | b'-') {
            self.pos += 1;
        }
        while self.pos < self.data.len() {
            let c = self.data[self.pos];
            if c.is_ascii_digit() {
                seen_digit = true;
                self.pos += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        if !seen_digit {
            is_num = false;
        }
        // a run continuing past the number is one regular token: `12Tf` is
        // the keyword "12Tf", not `12` followed by `Tf`
        if self.pos < self.data.len()
            && !is_ws(self.data[self.pos])
            && !is_delim(self.data[self.pos])
        {
            is_num = false;
        }
        // a bare sign or dot ran into a delimiter -> it's a keyword char run
        if !is_num || self.pos == start {
            // fall through to regular-token scan from the token start
            self.pos = start;
            while self.pos < self.data.len() {
                let c = self.data[self.pos];
                if is_ws(c) || is_delim(c) {
                    break;
                }
                self.pos += 1;
            }
            return Ok(Tok::Kw(Cow::Owned(self.data[start..self.pos].to_vec())));
        }
        let s = &self.data[start..self.pos];
        // PDF numbers fit in f64 for positioning; parse manually to avoid
        // str-utf8 ceremony (all bytes are ASCII here).
        let v = parse_f64(s).ok_or(Error::BadValue("number"))?;
        Ok(Tok::Num(v))
    }

    /// Peek the next token without consuming it (clones it, so callers use
    /// it only where the grammar needs lookahead).
    pub(crate) fn peek(&mut self) -> Result<Option<Tok>> {
        let save = self.pos;
        let t = self.next()?;
        self.pos = save;
        Ok(t)
    }

    /// Consume the keyword `kw` if it is next; report whether it was.
    pub(crate) fn eat_kw(&mut self, kw: &[u8]) -> Result<bool> {
        let save = self.pos;
        match self.next()? {
            Some(Tok::Kw(w)) if &*w == kw => Ok(true),
            _ => {
                self.pos = save;
                Ok(false)
            }
        }
    }

    /// Require the next token to be the keyword `kw`.
    pub(crate) fn expect_kw(&mut self, kw: &[u8], what: &'static str) -> Result<()> {
        match self.next()? {
            Some(Tok::Kw(w)) if &*w == kw => Ok(()),
            _ => Err(Error::BadValue(what)),
        }
    }

    /// Require the next token to be a non-negative integer; return it.
    pub(crate) fn expect_int(&mut self, what: &'static str) -> Result<i64> {
        match self.next()? {
            Some(Tok::Num(v)) => f64_as_i64(v).ok_or(Error::BadValue(what)),
            _ => Err(Error::BadValue(what)),
        }
    }
}

/// Hex-digit value, or `None`.
pub(crate) fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `true` when `v` is integral (f64 rounding methods are std-only).
pub(crate) fn is_int(v: f64) -> bool {
    if !v.is_finite() || !(-4.6e18..=4.6e18).contains(&v) {
        return false;
    }
    // value is integral iff truncation equals it; f64 -> i128 is lossless
    // for the range we allow
    (v as i128) as f64 == v
}

/// Integral `f64` -> `i64`, `None` for reals or out-of-range values.
pub(crate) fn f64_as_i64(v: f64) -> Option<i64> {
    // (v as i64) saturates for huge v; verify round-trip instead
    if is_int(v) && (-4.0e18..=4.0e18).contains(&v) {
        let i = v as i64;
        if i as f64 == v {
            return Some(i);
        }
    }
    None
}

/// ASCII-only `f64` parse (`[+-]?[0-9]*(.[0-9]*)?`, possibly with a second
/// stray sign we treat as an error). We do our own scan because the token
/// slice is bytes; this keeps `no_std` and locale issues out of the way.
pub(crate) fn parse_f64(s: &[u8]) -> Option<f64> {
    let mut i = 0;
    let mut neg = false;
    if let Some(&c) = s.first() {
        if c == b'+' {
            i = 1;
        } else if c == b'-' {
            neg = true;
            i = 1;
        }
    }
    let mut int: f64 = 0.0;
    let mut any = false;
    while let Some(&c) = s.get(i) {
        match c {
            b'0'..=b'9' => {
                int = int * 10.0 + f64::from(c - b'0');
                any = true;
                i += 1;
            }
            b'.' => {
                i += 1;
                let mut frac = 0.0f64;
                let mut scale = 1.0f64;
                while let Some(&d) = s.get(i) {
                    if d.is_ascii_digit() {
                        frac = frac * 10.0 + f64::from(d - b'0');
                        scale *= 10.0;
                        any = true;
                        i += 1;
                    } else {
                        return None;
                    }
                }
                let v = int + frac / scale;
                return if any {
                    Some(if neg { -v } else { v })
                } else {
                    None
                };
            }
            _ => return None,
        }
    }
    if any {
        Some(if neg { -int } else { int })
    } else {
        None
    }
}

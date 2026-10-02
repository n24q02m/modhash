//! A minimal XML tokenizer, sized for the documents the kit actually
//! meets: `word/document.xml` inside DOCX, `content.xml` inside ODF —
//! well-formed XML produced by real office software, not hostile DTDs.
//!
//! What it emits: start tags with attributes, end tags (self-closing
//! elements surface as `Start` followed by `End`), text with entities
//! decoded, and CDATA content as text. What it skips silently: the XML
//! declaration, other processing instructions, comments, and `<!DOCTYPE>`
//! declarations (internal subset included). What it refuses: unknown
//! entities, malformed names, mismatched end tags, a second root element.
//!
//! Namespaces are *not* resolved: `w:document` arrives as the literal
//! name `w:document` and `xmlns:w="…"` as an ordinary attribute, because
//! the consumers match element names verbatim and a half-resolution would
//! be worse than none.
//!
//! The reader is an [`Iterator`] of [`Result`]`<`[`XmlEvent`]`>`. Once it
//! returns `Err` or `None` it is done; a poisoned reader keeps returning
//! `None` rather than resuming a corrupt position.

use modhash_primitives::{Error, Result};

/// One token of the document.
#[derive(Clone, Debug, PartialEq)]
pub enum XmlEvent<'a> {
    /// `<name …>` — attributes in document order, values with entities
    /// already decoded.
    Start {
        /// The element name, verbatim (`prefix:local` included).
        name: &'a str,
        /// `(name, decoded value)` pairs, document order preserved.
        attrs: Vec<(&'a str, String)>,
    },
    /// `</name>` or the implied close of a self-closing `<name/>`.
    End {
        /// The element name being closed.
        name: &'a str,
    },
    /// Character data or CDATA content. Entities are already decoded in
    /// character data; CDATA content arrives verbatim.
    Text(String),
}

/// A pull tokenizer over an XML document.
///
/// `XmlReader::new("<w:t>hi</w:t>")` hands out `Start(w:t)`,
/// `Text("hi")`, `End(w:t)` and then `None`.
pub struct XmlReader<'a> {
    /// Unconsumed input.
    input: &'a str,
    /// Open element names, innermost last.
    stack: Vec<&'a str>,
    /// A `Start` from a self-closing tag produces its `End` on the next
    /// call; at most one can be outstanding because `End` carries no
    /// fields that would need a queue.
    deferred_end: Option<&'a str>,
    /// Whether the (single) root element has been seen already.
    root_seen: bool,
    /// Set after an error or a completed document: the stream is over.
    done: bool,
}

impl<'a> XmlReader<'a> {
    /// Starts reading a document. Nothing is consumed yet; the first
    /// event appears on [`next_event`](Self::next_event).
    pub fn new(input: &'a str) -> Self {
        XmlReader {
            input,
            stack: Vec::new(),
            deferred_end: None,
            root_seen: false,
            done: false,
        }
    }

    /// The next token, `None` at end of document, or a named [`Error`].
    /// Errors are sticky: every call after the first `Err` returns `None`.
    pub fn next_event(&mut self) -> Result<Option<XmlEvent<'a>>> {
        if self.done {
            return Ok(None);
        }
        match self.step() {
            Ok(e) => Ok(e),
            Err(e) => {
                self.done = true;
                Err(e)
            }
        }
    }

    /// The walking state machine behind [`next_event`](Self::next_event).
    fn step(&mut self) -> Result<Option<XmlEvent<'a>>> {
        if let Some(name) = self.deferred_end.take() {
            return Ok(Some(XmlEvent::End { name }));
        }
        if self.input.is_empty() {
            if self.stack.is_empty() {
                return Ok(None);
            }
            return Err(Error::truncated("xml document", 1, 0));
        }
        if let Some(rest) = self.input.strip_prefix('<') {
            self.input = rest;
            self.markup()
        } else {
            self.take_text()
        }
    }

    /// Consumes whatever follows the `<` already stripped: a tag, a
    /// comment/PI/doctype to skip and recurse over, or a CDATA section.
    fn markup(&mut self) -> Result<Option<XmlEvent<'a>>> {
        if self.input.starts_with("!--") {
            self.input = &self.input[3..];
            self.skip_past("-->", "xml comment")?;
            return self.step();
        }
        if self.input.starts_with('?') {
            self.input = &self.input[1..];
            self.skip_past("?>", "xml processing instruction")?;
            return self.step();
        }
        if self.input.starts_with("![CDATA[") {
            self.input = &self.input[8..];
            let end = self.input.find("]]>").ok_or_else(|| {
                Error::truncated("xml cdata", self.input.len() + 3, self.input.len())
            })?;
            let text = self.input[..end].to_string();
            self.input = &self.input[end + 3..];
            return Ok(Some(XmlEvent::Text(text)));
        }
        if self.input.starts_with('!') {
            // <!DOCTYPE …> and friends: skip to the `>` that closes the
            // declaration, tracking an internal subset's […] and quotes.
            self.input = &self.input[1..];
            self.skip_declaration()?;
            return self.step();
        }
        if self.input.starts_with('/') {
            self.input = &self.input[1..];
            return self.end_tag();
        }
        self.start_tag()
    }

    /// Consumes an end tag after `</` and checks it against the innermost
    /// open element.
    fn end_tag(&mut self) -> Result<Option<XmlEvent<'a>>> {
        let name = self.scan_name("xml end tag name")?;
        self.skip_ws();
        match self.take_char() {
            Some('>') => {}
            Some(_) => return Err(Error::BadValue("xml end tag")),
            None => return Err(Error::truncated("xml end tag", 1, 0)),
        }
        match self.stack.last() {
            Some(&open) if open == name => {
                self.stack.pop();
                Ok(Some(XmlEvent::End { name }))
            }
            _ => Err(Error::BadValue("xml end tag mismatch")),
        }
    }

    /// Consumes a start tag: name, attributes, the `>` or `/>`.
    fn start_tag(&mut self) -> Result<Option<XmlEvent<'a>>> {
        if self.stack.is_empty() && self.root_seen {
            return Err(Error::BadValue("xml second root element"));
        }
        let name = self.scan_name("xml tag name")?;
        let mut attrs: Vec<(&'a str, String)> = Vec::new();
        let self_closing;
        loop {
            self.skip_ws();
            match self.input.as_bytes().first() {
                Some(b'>') => {
                    self.input = &self.input[1..];
                    self_closing = false;
                    break;
                }
                Some(b'/') => {
                    self.input = &self.input[1..];
                    match self.take_char() {
                        Some('>') => {
                            self_closing = true;
                            break;
                        }
                        Some(_) => return Err(Error::BadValue("xml self-closing tag")),
                        None => return Err(Error::truncated("xml tag", 1, 0)),
                    }
                }
                Some(_) => {
                    let attr_name = self.scan_name("xml attribute name")?;
                    self.skip_ws();
                    match self.take_char() {
                        Some('=') => {}
                        Some(_) => return Err(Error::BadValue("xml attribute")),
                        None => return Err(Error::truncated("xml attribute", 1, 0)),
                    }
                    self.skip_ws();
                    let quote = match self.take_char() {
                        Some('"') => '"',
                        Some('\'') => '\'',
                        Some(_) => return Err(Error::BadValue("xml attribute quote")),
                        None => return Err(Error::truncated("xml attribute", 1, 0)),
                    };
                    let value = self.take_attr_value(quote)?;
                    if attrs.iter().any(|(n, _)| *n == attr_name) {
                        return Err(Error::BadValue("xml duplicate attribute"));
                    }
                    attrs.push((attr_name, value));
                }
                None => return Err(Error::truncated("xml tag", 1, 0)),
            }
        }
        if self_closing {
            self.deferred_end = Some(name);
        } else {
            self.stack.push(name);
        }
        if self.stack.len() == 1 && !self_closing {
            self.root_seen = true;
        }
        if self_closing && self.stack.is_empty() {
            // A self-closing document element is still the root.
            self.root_seen = true;
        }
        Ok(Some(XmlEvent::Start { name, attrs }))
    }

    /// Consumes a run of character data up to the next markup.
    fn take_text(&mut self) -> Result<Option<XmlEvent<'a>>> {
        if self.stack.is_empty() {
            // Outside the root only whitespace may appear; anything else
            // is a malformed document, not a text node.
            if self.input[..self.text_run_end()].trim().is_empty() {
                let run = self.text_run_end();
                self.input = &self.input[run..];
                return self.step();
            }
            return Err(Error::BadValue("xml text outside root element"));
        }
        let run = self.text_run_end();
        let chunk = &self.input[..run];
        let mut out = String::with_capacity(chunk.len());
        let mut rest = chunk;
        while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            rest = &rest[amp..];
            self.decode_entity(&mut rest, &mut out)?;
        }
        out.push_str(rest);
        self.input = &self.input[run..];
        Ok(Some(XmlEvent::Text(out)))
    }

    /// How many bytes before the next markup boundary (`<`) or EOF. Text
    /// may contain `&`, so the run stops at `<` alone.
    fn text_run_end(&self) -> usize {
        self.input.find('<').unwrap_or(self.input.len())
    }

    /// Consumes an attribute value after its opening quote, decoding
    /// entities. `<` inside a value is malformed XML.
    fn take_attr_value(&mut self, quote: char) -> Result<String> {
        let mut out = String::new();
        let mut rest = self.input;
        loop {
            let next = rest.find([quote, '<', '&']);
            match next {
                None => {
                    return Err(Error::truncated(
                        "xml attribute value",
                        rest.len() + 1,
                        rest.len(),
                    ));
                }
                Some(i) => {
                    out.push_str(&rest[..i]);
                    match rest.as_bytes()[i] {
                        b'<' => return Err(Error::BadValue("xml '<' in attribute value")),
                        b'&' => {
                            rest = &rest[i..];
                            self.decode_entity(&mut rest, &mut out)?;
                        }
                        _ => {
                            self.input = &rest[i + 1..];
                            return Ok(out);
                        }
                    }
                }
            }
        }
    }

    /// Decodes `&…;` at the head of `*rest` (which starts at the `&`)
    /// into `out` and advances `*rest` past the `;`.
    fn decode_entity(&self, rest: &mut &'a str, out: &mut String) -> Result<()> {
        let semi = rest
            .find(';')
            .ok_or(Error::BadValue("xml unterminated entity"))?;
        let body = &rest[1..semi];
        if body.is_empty() || body.contains(|c: char| c.is_whitespace() || c == '<' || c == '&') {
            return Err(Error::BadValue("xml entity"));
        }
        let decoded: char = match body {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ if body.starts_with("#x") || body.starts_with("#X") => {
                let digits = &body[2..];
                let v = u32::from_str_radix(digits, 16)
                    .map_err(|_| Error::BadValue("xml numeric character reference"))?;
                char::from_u32(v).ok_or(Error::BadValue("xml numeric character reference"))?
            }
            _ if body.starts_with('#') => {
                let digits = &body[1..];
                let v = digits
                    .parse::<u32>()
                    .map_err(|_| Error::BadValue("xml numeric character reference"))?;
                char::from_u32(v).ok_or(Error::BadValue("xml numeric character reference"))?
            }
            _ => return Err(Error::BadValue("xml unknown entity")),
        };
        out.push(decoded);
        *rest = &rest[semi + 1..];
        Ok(())
    }

    /// Scans an XML Name: a non-empty run of name characters, terminated
    /// by anything that is not one.
    fn scan_name(&mut self, what: &'static str) -> Result<&'a str> {
        let end = self
            .input
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == ':'))
            .unwrap_or(self.input.len());
        if end == 0 {
            if self.input.is_empty() {
                return Err(Error::truncated(what, 1, 0));
            }
            return Err(Error::BadValue(what));
        }
        let (name, rest) = self.input.split_at(end);
        self.input = rest;
        Ok(name)
    }

    /// Skips whitespace in place.
    fn skip_ws(&mut self) {
        self.input = self.input.trim_start();
    }

    /// Takes the first char of the input, or `None` at EOF. Char-based,
    /// not byte-based: slicing off a fixed byte width would panic on a
    /// multi-byte character.
    fn take_char(&mut self) -> Option<char> {
        let c = self.input.chars().next()?;
        self.input = &self.input[c.len_utf8()..];
        Some(c)
    }

    /// Skips to just past the first `needle`, or [`Error::Truncated`].
    fn skip_past(&mut self, needle: &str, what: &'static str) -> Result<()> {
        match self.input.find(needle) {
            Some(i) => {
                self.input = &self.input[i + needle.len()..];
                Ok(())
            }
            None => Err(Error::truncated(
                what,
                self.input.len() + needle.len(),
                self.input.len(),
            )),
        }
    }

    /// Skips a `<!…>` declaration: a `<!DOCTYPE` internal subset can hide
    /// `>` inside `[…]` or quoted strings, so the scan tracks bracket
    /// depth and quoting rather than stopping at the first `>`.
    fn skip_declaration(&mut self) -> Result<()> {
        let mut depth = 0usize;
        let mut quote: Option<char> = None;
        for (i, c) in self.input.char_indices() {
            match quote {
                Some(q) => {
                    if c == q {
                        quote = None;
                    }
                }
                None => match c {
                    '"' | '\'' => quote = Some(c),
                    '[' => depth += 1,
                    ']' => depth = depth.saturating_sub(1),
                    '>' if depth == 0 => {
                        self.input = &self.input[i + 1..];
                        return Ok(());
                    }
                    _ => {}
                },
            }
        }
        Err(Error::truncated(
            "xml declaration",
            self.input.len() + 1,
            self.input.len(),
        ))
    }
}

impl<'a> Iterator for XmlReader<'a> {
    type Item = Result<XmlEvent<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_event().transpose()
    }
}

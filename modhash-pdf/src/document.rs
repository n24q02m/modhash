//! Document assembly: xref -> trailer -> page tree -> per-page extraction.
//!
//! `Document` is lazy: objects parse on first reference and are cached, so
//! a corrupt object errors on the page that references it instead of
//! refusing the whole file. Object streams (`/ObjStm`) are decoded once and
//! their members cached under their real object numbers.
//!
//! An unresolvable xref is rebuilt by scanning the file for `N G obj`
//! headers (see `xref::scan_xref`); [`Document::xref_was_rebuilt`] reports
//! that the recovery path ran.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use modhash_primitives::{Error as KErr, Result as KResult};

use crate::content::{Resources, extract_page_text};
use crate::font::Resolve;
use crate::object::{Obj, Ref, decode_stream, dict_get, parse_obj};
use crate::xref::{Loc, Xref, read_xref, scan_xref};

/// The crate's public error: wraps the kit [`modhash_primitives::Error`]
/// with the page or object where the failure happened.
#[derive(Debug)]
pub enum Error {
    /// A document-level failure (header, xref, trailer, page tree).
    Kit(KErr),
    /// Failure attributed to one indirect object.
    Object {
        /// Object number.
        object: u32,
        /// Generation number.
        generation: u16,
        /// Underlying cause.
        cause: Box<Error>,
    },
    /// Failure attributed to one page.
    Page {
        /// Zero-based page index in document order.
        page: usize,
        /// Underlying cause.
        cause: Box<Error>,
    },
}

/// The crate's result type.
pub type Result<T, E = Error> = core::result::Result<T, E>;

impl From<KErr> for Error {
    fn from(e: KErr) -> Error {
        Error::Kit(e)
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Kit(e) => e.fmt(f),
            Error::Object {
                object,
                generation,
                cause,
            } => {
                f.write_str("object ")?;
                write_u32(f, *object)?;
                f.write_str(" ")?;
                write_u32(f, u32::from(*generation))?;
                f.write_str(": ")?;
                cause.fmt(f)
            }
            Error::Page { page, cause } => {
                f.write_str("page ")?;
                write_usize(f, *page)?;
                f.write_str(": ")?;
                cause.fmt(f)
            }
        }
    }
}

fn write_u32(f: &mut core::fmt::Formatter<'_>, v: u32) -> core::fmt::Result {
    // decimal without allocations
    let mut buf = [0u8; 10];
    let mut i = buf.len();
    let mut v = v;
    if v == 0 {
        return f.write_str("0");
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    let s = core::str::from_utf8(&buf[i..]).unwrap_or("?");
    f.write_str(s)
}

fn write_usize(f: &mut core::fmt::Formatter<'_>, v: usize) -> core::fmt::Result {
    write_u32(f, v as u32)
}

/// A page in document order: its dictionary ref and the *effective*
/// `/Resources` dictionary after inheriting down the page tree.
struct Page {
    /// Reference to the `/Page` dictionary object.
    r: Ref,
    /// Merged resources (ancestors' `/Resources` overridden by the page's).
    resources: Vec<(Vec<u8>, Obj)>,
}

/// An open PDF document.
pub struct Document<'a> {
    data: &'a [u8],
    xref: Xref,
    limits: modhash_inflate::Limits,
    pages: Vec<Page>,
    encrypt: Option<Ref>,
    cache: RefCell<BTreeMap<u32, Obj>>,
    /// decoded member lists of object streams, keyed by stream object num
    objstms: RefCell<BTreeMap<u32, Vec<(u32, Obj)>>>,
}

impl<'a> Resolve for Document<'a> {
    fn deref(&self, r: Ref) -> KResult<Obj> {
        self.resolve_obj(r).map_err(|e| match e {
            Error::Kit(k) => k,
            _ => KErr::BadValue("indirect object"),
        })
    }
}

/// Worklist item for `collect_pages`.
type PageStackItem = (Ref, Vec<(Vec<u8>, Obj)>);

impl<'a> Document<'a> {
    /// Open with default [`modhash_inflate::Limits`].
    pub fn open(data: &'a [u8]) -> Result<Document<'a>> {
        Self::open_with_limits(data, modhash_inflate::Limits::default())
    }

    /// Open with explicit decompression limits.
    pub fn open_with_limits(
        data: &'a [u8],
        limits: modhash_inflate::Limits,
    ) -> Result<Document<'a>> {
        if data.len() < 8 {
            return Err(Error::Kit(KErr::Truncated {
                what: "PDF header",
                needed: 8,
                found: data.len(),
            }));
        }
        // %PDF- may sit within the first KiB of a damaged file; binary junk
        // before the signature is tolerated by every reader
        let sig = data[..1024.min(data.len())]
            .windows(5)
            .position(|w| w == b"%PDF-")
            .ok_or(KErr::InvalidMagic {
                what: "PDF signature",
            })?;
        let _ = sig;

        let xref = match read_xref(data, &limits) {
            Ok(x) => x,
            Err(_) => scan_xref(data, &limits)?,
        };
        let trailer = &xref.trailer;

        let encrypt = dict_get(trailer, b"Encrypt").and_then(Obj::as_ref);

        let root = dict_get(trailer, b"Root")
            .and_then(Obj::as_ref)
            .ok_or(KErr::BadValue("trailer /Root"))?;

        let mut doc = Document {
            data,
            xref,
            limits,
            pages: Vec::new(),
            encrypt,
            cache: RefCell::new(BTreeMap::new()),
            objstms: RefCell::new(BTreeMap::new()),
        };
        let catalog = doc.resolve_obj(root)?;
        let catalog_dict = catalog.dict().ok_or(KErr::BadValue("/Root"))?;
        match dict_get(catalog_dict, b"Type") {
            Some(Obj::Name(n)) if n.as_slice() == b"Catalog" => {}
            _ => return Err(KErr::BadValue("/Root is not a Catalog").into()),
        }
        let pages_ref = dict_get(catalog_dict, b"Pages")
            .and_then(Obj::as_ref)
            .ok_or(KErr::BadValue("catalog /Pages"))?;
        doc.collect_pages(pages_ref, &[])?;
        if doc.pages.is_empty() {
            return Err(KErr::BadValue("empty page tree").into());
        }
        Ok(doc)
    }

    /// `true` when the xref table could not be parsed and was rebuilt by
    /// scanning for object headers.
    pub fn xref_was_rebuilt(&self) -> bool {
        self.xref.rebuilt
    }

    /// `true` when the trailer declares `/Encrypt`. Text extraction refuses
    /// encrypted documents with [`Error::Unsupported`]-bearing errors.
    pub fn is_encrypted(&self) -> bool {
        self.encrypt.is_some()
    }

    /// The `/Encrypt` object reference, if present.
    pub fn encryption(&self) -> Option<Ref> {
        self.encrypt
    }

    /// Page count.
    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    /// Resolve and parse one indirect object (public for inspection; the
    /// returned object is owned).
    pub fn object(&self, r: Ref) -> Result<Obj> {
        self.resolve_obj(r).map_err(|e| Error::Object {
            object: r.num,
            generation: r.generation,
            cause: Box::new(match e {
                Error::Kit(k) => Error::Kit(k),
                other => other,
            }),
        })
    }

    /// Extract one page's text.
    pub fn page_text(&self, page: usize) -> Result<String> {
        let p = self
            .pages
            .get(page)
            .ok_or(KErr::BadValue("page index"))
            .map_err(Error::from)?;
        if let Some(er) = self.encrypt {
            return Err(Error::Page {
                page,
                cause: Box::new(Error::Object {
                    object: er.num,
                    generation: er.generation,
                    cause: Box::new(Error::Kit(KErr::Unsupported(
                        "encrypted document (/Encrypt in trailer)",
                    ))),
                }),
            });
        }
        self.page_text_inner(p).map_err(|e| Error::Page {
            page,
            cause: Box::new(e),
        })
    }

    /// Extract the whole document's text, pages joined by `\x0c` (form
    /// feed). The first failing page aborts with [`Error::Page`].
    pub fn text(&self) -> Result<String> {
        let mut out = String::new();
        for i in 0..self.pages.len() {
            if i > 0 {
                out.push('\x0c');
            }
            out.push_str(&self.page_text(i)?);
        }
        Ok(out)
    }

    // -- internals -----------------------------------------------------------

    fn resolve_obj(&self, r: Ref) -> Result<Obj> {
        if let Some(o) = self.cache.borrow().get(&r.num) {
            return Ok(o.clone());
        }
        let loc = self
            .xref
            .map
            .get(&r.num)
            .copied()
            .ok_or(KErr::BadValue("reference to missing object"))?;
        let obj = match loc {
            Loc::Plain { offset, generation } => {
                if generation != r.generation {
                    // generation mismatch: the xref points at a different
                    // revision of this object. Readers tolerate it; we do
                    // too, since the location is still authoritative.
                }
                let p = parse_obj(self.data, offset)?;
                let mut obj = p.obj;
                if let Some((len_ref, start)) = p.pending {
                    // re-slice stream data using the resolved length
                    let len = self
                        .resolve_obj(len_ref)?
                        .as_i64()
                        .ok_or(KErr::BadValue("indirect /Length value"))?;
                    if len < 0 {
                        return Err(KErr::BadValue("negative /Length").into());
                    }
                    if let Obj::Stream { data: d, .. } = &mut obj {
                        let end = start
                            .checked_add(len as usize)
                            .and_then(|e| self.data.get(..e).map(|_| e))
                            .ok_or(KErr::Truncated {
                                what: "stream data",
                                needed: len as usize,
                                found: self.data.len().saturating_sub(start),
                            })?;
                        *d = self.data[start..end].to_vec();
                    }
                }
                obj
            }
            Loc::InStm { stm, idx } => {
                self.expand_objstm(stm)?;
                let members = self.objstms.borrow();
                let list = members
                    .get(&stm)
                    .ok_or(KErr::BadValue("object stream missing"))?;
                let (n, o) = list
                    .get(idx as usize)
                    .ok_or(KErr::BadValue("object stream index"))?;
                if *n != r.num {
                    return Err(KErr::BadValue("object stream member number").into());
                }
                o.clone()
            }
        };
        self.cache.borrow_mut().insert(r.num, obj.clone());
        Ok(obj)
    }

    /// Decode an `/ObjStm` stream and cache its members.
    fn expand_objstm(&self, stm: u32) -> Result<()> {
        if self.objstms.borrow().contains_key(&stm) {
            return Ok(());
        }
        let sobj = self.resolve_obj(Ref {
            num: stm,
            generation: 0,
        })?;
        let dict = sobj.dict().ok_or(KErr::BadValue("ObjStm dict"))?;
        let n = dict_get(dict, b"N")
            .and_then(Obj::as_usize)
            .ok_or(KErr::BadValue("ObjStm /N"))?;
        let first = dict_get(dict, b"First")
            .and_then(Obj::as_usize)
            .ok_or(KErr::BadValue("ObjStm /First"))?;
        let raw = decode_stream(&sobj, &self.limits)?;
        if first > raw.len() {
            return Err(KErr::BadValue("ObjStm /First").into());
        }
        // header: N pairs of `objnum offset` integers
        let mut lx = crate::lex::Lexer::new(&raw);
        let mut hdr: Vec<(u32, usize)> = Vec::with_capacity(n);
        for _ in 0..n {
            let num = lx.expect_int("ObjStm member number")?;
            let off = lx.expect_int("ObjStm member offset")?;
            if num < 0 || num > i64::from(u32::MAX) || off < 0 {
                return Err(KErr::BadValue("ObjStm member header").into());
            }
            let off = off as usize;
            if off >= raw.len() - first && off != 0 {
                return Err(KErr::BadValue("ObjStm member offset").into());
            }
            hdr.push((num as u32, off));
        }
        let mut members: Vec<(u32, Obj)> = Vec::with_capacity(n);
        for (i, &(num, off)) in hdr.iter().enumerate() {
            let start = first + off;
            let end = hdr.get(i + 1).map(|&(_, o)| first + o).unwrap_or(raw.len());
            if start > raw.len() || end > raw.len() || end < start {
                return Err(KErr::BadValue("ObjStm member span").into());
            }
            let slice = &raw[start..end];
            let p = crate::object::parse_standalone(slice, 0)?;
            members.push((num, p.obj));
        }
        self.objstms.borrow_mut().insert(stm, members);
        Ok(())
    }

    /// Walk the page tree collecting `/Page` nodes in document order with
    /// inherited `/Resources`.
    fn collect_pages(&mut self, node: Ref, inherited: &[(Vec<u8>, Obj)]) -> Result<()> {
        let mut stack: Vec<PageStackItem> = alloc::vec![(node, inherited.to_vec())];
        let mut visited = 0usize;
        while let Some((r, inh)) = stack.pop() {
            visited += 1;
            if visited > 1 << 16 {
                return Err(KErr::TooLarge {
                    what: "page tree nodes",
                    limit: 1 << 16,
                }
                .into());
            }
            let obj = self.resolve_obj(r)?;
            let dict = obj.dict().ok_or(KErr::BadValue("page tree node"))?;
            // resources inherit: merge inherited with node's own (node wins)
            let mut res = inh.clone();
            if let Some(rd) = dict_get(dict, b"Resources").and_then(Obj::dict) {
                for (k, v) in rd {
                    res.retain(|(ek, _)| ek != k);
                    res.push((k.clone(), v.clone()));
                }
            }
            match dict_get(dict, b"Type") {
                Some(Obj::Name(t)) if t.as_slice() == b"Page" => {
                    self.pages.push(Page { r, resources: res });
                }
                Some(Obj::Name(t)) if t.as_slice() == b"Pages" => {
                    match dict_get(dict, b"Kids").and_then(Obj::as_array) {
                        Some(kids) => {
                            // push reversed so document order pops first
                            for k in kids.iter().rev() {
                                let kr =
                                    k.as_ref().ok_or(KErr::BadValue("page tree /Kids entry"))?;
                                stack.push((kr, res.clone()));
                            }
                        }
                        None => return Err(KErr::BadValue("page tree /Kids").into()),
                    }
                }
                // a node without a recognizable type is still walked via
                // /Kids for tolerance
                _ => {
                    if let Some(kids) = dict_get(dict, b"Kids").and_then(Obj::as_array) {
                        for k in kids.iter().rev() {
                            let kr = k.as_ref().ok_or(KErr::BadValue("page tree /Kids entry"))?;
                            stack.push((kr, res.clone()));
                        }
                    } else {
                        self.pages.push(Page { r, resources: res });
                    }
                }
            }
        }
        Ok(())
    }

    fn page_text_inner(&self, p: &Page) -> Result<String> {
        let page_obj = self.resolve_obj(p.r)?;
        let dict = page_obj.dict().ok_or(KErr::BadValue("page dictionary"))?;
        // /Contents: absent -> empty text; stream ref -> one stream;
        // array of refs -> concatenated in array order
        let contents = match dict_get(dict, b"Contents") {
            None | Some(Obj::Null) => return Ok(String::new()),
            Some(o) => o.clone(),
        };
        let mut streams: Vec<Vec<u8>> = Vec::new();
        let collect = |o: &Obj, streams: &mut Vec<Vec<u8>>| -> Result<()> {
            let o = match o {
                Obj::Ref(r) => self.resolve_obj(*r)?,
                other => other.clone(),
            };
            if let Obj::Stream { .. } = o {
                let raw = decode_stream(&o, &self.limits)?;
                streams.push(raw);
                Ok(())
            } else {
                Err(KErr::BadValue("page /Contents entry").into())
            }
        };
        match &contents {
            Obj::Arr(a) => {
                for o in a {
                    collect(o, &mut streams)?;
                }
            }
            other => collect(other, &mut streams)?,
        }
        let res = Resources::from_dict(Some(&p.resources));
        extract_page_text(&streams, &res, self, &self.limits).map_err(Error::Kit)
    }
}

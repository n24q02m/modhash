//! PDF text extraction across classic xref, xref streams, cmap and CID fonts.
//!
//! Part of the `modhash` zero-dependency hashing kit: this crate depends
//! only on `modhash-primitives`, `modhash-inflate` and `modhash-text`, so the
//! whole kit resolves without a single registry package.
//!
//! # Scope
//!
//! Opens a PDF, resolves objects through the cross-reference (classic
//! tables, xref **streams** with `/W` field widths and `/Index` runs,
//! `/Prev` incremental-update chains, and `/ObjStm` object streams), walks
//! the page tree and extracts text from content streams:
//!
//! - text operators `Tj`, `TJ`, `'`, `"` plus the positioning ops `Td`,
//!   `TD`, `Tm`, `T*`; `Do` recurses into Form XObjects (own resources
//!   override the page's per name);
//! - font decoding: `/ToUnicode` CMaps (`bfchar`, `bfrange` incl. array
//!   destinations, multi-char strings and surrogate pairs), the five
//!   predefined encodings plus `/Differences` (AGL names + `uniXXXX`/
//!   `uXXXXXX`), and CID-keyed fonts (`/Type0`, `/Identity-H/-V` and
//!   embedded CMap streams);
//! - stream filters `FlateDecode` (zlib + raw-deflate fallback), `ASCII85`,
//!   `ASCIIHex` with `/DecodeParms` PNG/TIFF predictors.
//!
//! # Extraction rules (deterministic, documented)
//!
//! - A `Td`/`TD` with `ty != 0`, any `Tm` moving vertically, `T*`, `'`,
//!   `"` end the current line. `Td`/`Tm` with a pure horizontal move of
//!   >= 2 text units emits one space.
//! - `TJ` numbers below `-250` (over a quarter em leftward gap) start a new
//!   word.
//! - Pages join with `\x0c` (form feed). Unmappable codes decode as U+FFFD
//!   (the glyph exists but cannot be mapped), never silently dropped.
//!
//! # Refusals, never guesses
//!
//! - Encrypted documents (`/Encrypt` in the trailer or xref stream) refuse
//!   extraction per page with object context.
//! - CID fonts without `/ToUnicode` refuse — CID numbers are glyph ids, not
//!   codepoints.
//! - Unsupported stream filters (LZW, DCT, JBIG2, JPX, CCITT, RunLength,
//!   Crypt) refuse naming the filter.
//! - A corrupt xref is **rebuilt by scanning** for `N G obj` headers when
//!   possible ([`Document::xref_was_rebuilt`] reports it); only a file with
//!   no findable objects fails.
//! - Every page failure carries the page number ([`Error::Page`]); every
//!   object-level refusal carries the object ([`Error::Object`]).

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![no_std]

extern crate alloc;

pub mod cmap;
mod content;
mod document;
mod font;
mod lex;
mod object;
mod tables;
mod xref;

pub use document::{Document, Error, Result};
pub use object::{Obj, Ref, decode_stream};

/// Extract all text from a PDF byte buffer (pages joined by `\x0c`).
///
/// Equivalent to [`Document::open`] + [`Document::text`].
pub fn extract_text(data: &[u8]) -> Result<alloc::string::String> {
    Document::open(data)?.text()
}

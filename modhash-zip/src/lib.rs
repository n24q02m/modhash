//! ZIP container reading (PKZIP APPNOTE.TXT) plus a minimal XML reader
//! for the members inside (the kit's first consumer is DOCX:
//! `word/document.xml` in a zip).
//!
//! Part of the `modhash` zero-dependency hashing kit: every crate in this
//! workspace builds with an empty `[dependencies]` table bar the kit's own
//! path dependencies, so the whole kit resolves without a single registry
//! package. This crate reads crate `modhash-inflate` (#1) for raw DEFLATE
//! and `modhash-primitives` (#0) for CRC-32 verification and the shared
//! `Error`/`Result` vocabulary.
//!
//! # ZIP scope (APPNOTE.TXT §4.3 and §4.4)
//!
//! The reader trusts the central directory at the end of the file - the
//! structure every consumer of a zip actually honours - and only dips back
//! to each local file header to find where the payload starts. Both
//! `method 0` (stored) and `method 8` (raw DEFLATE) entries extract, and
//! every extracted byte stream is verified against the entry's CRC-32
//! before it is returned: a container reader that returns corrupt bytes
//! hands a wrong hash to the tier-1 layer above.
//!
//! Real format variants outside that scope are refused with
//! `Error::Unsupported`, never mis-decoded: ZIP64 records or sentinel
//! fields, encrypted entries (flags bit 0, strong encryption bit 6, or the
//! masked-local-header bit 13), multi-disk archives, and compression
//! methods other than 0 and 8. Data descriptors (flags bit 3) are
//! supported: the sizes and CRC are read from the central directory, and
//! the descriptor record itself is verified after the payload.
//!
//! # XML scope
//!
//! [`XmlReader`] is a well-formed-enough tokenizer for what DOCX feeds it:
//! elements with attributes, text nodes, the five predefined entities,
//! decimal and hexadecimal numeric character references, comments,
//! processing instructions and CDATA. Namespaces are passed through as
//! ordinary `prefix:local` names - consumers match `w:t` literally.
//! Anything not needed (DTD internals beyond a skipped `<!DOCTYPE>`,
//! namespace resolution, schema) is skipped or refused.

mod reader;
mod xml;

pub use reader::{ZipArchive, ZipEntry};
pub use xml::{XmlEvent, XmlReader};

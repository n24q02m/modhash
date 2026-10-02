# `modhash-zip`

ZIP container reading (stored and deflated entries, PKZIP APPNOTE.TXT) and
a minimal XML reader for the members inside — DOCX's `word/document.xml`
is the first consumer.

**Status: implemented.** `ZipArchive` parses the end-of-central-directory
record (comments up to 64 KiB handled by scanning backwards past the
signature), reads the central directory, and extracts method-0 and
method-8 members through `modhash_inflate::inflate_raw`, verifying
uncompressed size and CRC-32 (`modhash_primitives::crc32`) on every
extraction. Data descriptors (flags bit 3) are supported and verified;
ZIP64, multi-disk and encrypted entries, and other compression methods
refuse with `Error::Unsupported`. `XmlReader` is a well-formed-enough
tokenizer: elements, attributes, text, the five predefined entities,
numeric character references, comments, PIs, CDATA, and doctype skipping.

## Workspace rules

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` are not optional.
- Dependencies: `modhash-primitives` (#0), `modhash-inflate` (#1).
- Zero external dependencies is enforced by `scripts/gate_zero_dep.sh`; the
  crate ordering is enforced by `scripts/gate_dag.py`.

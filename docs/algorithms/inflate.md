# `modhash-inflate` - DEFLATE decompression

Status: specified, not yet implemented. Crate number `#1`, depends only on
`modhash-primitives`.

## Why this crate exists

Decompression sits underneath PNG, ZIP, DOCX and half the web. A hashing kit
that walks those containers has to be able to reach the bytes inside them, and
it cannot add a compression library without violating the zero-dependency
rule. So the decompressor is written here, once.

This crate **decompresses only**. It does not compress. A compressor is not
needed by any consumer in the workspace, and a half-tested compressor that
claims to produce valid DEFLATE is worse than none.

## References

- RFC 1951, *DEFLATE Compressed Data Format Specification version 1.3*
- RFC 1950, *ZLIB Compressed Data Format Specification version 3.3*
- RFC 1952, *GZIP file format* - recognised but **not** decompressed; see
  "Out of scope".

## API

```rust
/// Decompress a raw RFC 1951 DEFLATE stream.
pub fn inflate_raw(input: &[u8], limits: &Limits) -> Result<Vec<u8>>;

/// Decompress a zlib (RFC 1950) stream: header, optional preset dictionary,
/// DEFLATE payload, Adler-32 trailer.
pub fn inflate_zlib(input: &[u8], limits: &Limits) -> Result<Vec<u8>>;

/// Decompress either, sniffing the two-byte zlib header. A stream that is
/// neither raw DEFLATE nor zlib is an error, not a guess.
pub fn inflate_auto(input: &[u8], limits: &Limits) -> Result<Vec<u8>>;

/// Adler-32 as specified by RFC 1950 section 9. Exposed because the zlib
/// wrapper's trailer check is part of this crate's correctness argument, and a
/// test that cannot reach the checksum cannot check it.
pub fn adler32(data: &[u8]) -> u32;
```

### `Limits`

```rust
pub struct Limits {
    /// Hard ceiling on the produced output, in bytes. Exceeding it is
    /// `Error::TooLarge`, not an allocation failure.
    pub max_output: usize,
    /// Hard ceiling on input consumed, in bytes.
    pub max_input: usize,
}
```

`Limits` is not optional and there is no `Default` that is silently permissive.
A decompression API without a size ceiling is a denial-of-service primitive: a
few hundred bytes of input expand to gigabytes. Every entry point takes limits.

`Limits::default()` exists for convenience and is a *conservative* ceiling, not
an unlimited one. Its value is documented on the field.

Rationale for a default existing at all: the common case is a caller hashing a
file, and forcing every one of them to invent a number gets them invented wrong.

## Correctness requirements

### Block types

All three must be implemented: stored (type 0), fixed Huffman (type 1),
dynamic Huffman (type 2). **Reserved (type 3) is an error**, not a tolerated
anomaly - RFC 1951 says a compliant decoder must refuse it.

### Stored blocks

`LEN` and `NLEN` are 16-bit little-endian, and `NLEN` must be the ones'
complement of `LEN`. A mismatch is `Error::BadValue`. This check is not
optional: without it a corrupt stream yields silently wrong bytes.

### Huffman decoding

- Canonical Huffman codes, built from a code-length vector, per RFC 1951
  section 3.2.2.
- A length vector is invalid when it over-subscribes the code space
  (`Error::BadValue`). Under-subscribing is legal **only** for the single-symbol
  distance tree, which RFC 1951 explicitly permits and which real encoders emit
  for streams with no matches.
- Incomplete codes that are not the one-symbol special case are rejected rather
  than half-decoded.
- A bit sequence that matches no code is `Error::BadValue`, never a default
  code.

### Back-references

`length`/`distance` pairs copy from already-produced output. A distance that
reaches before the start of the output buffer is `Error::BadValue` - this is
the classic out-of-bounds read and it must be a bounds-checked `copy_within`,
not pointer arithmetic. Copying must handle overlap correctly, since a distance
smaller than the length is legal and common.

### zlib wrapper

- Header: `CMF` low nibble is the compression method and must be 8; `CINFO` is
  the window size and must be <= 7; `(CMF << 8 | FLG) % 31 == 0`;
  `FDICT` set means a 4-byte preset dictionary id follows, which this crate
  **rejects** with `Error::Unsupported` because supporting preset dictionaries
  requires shipping `DICTID` mappings for values it does not otherwise know.
- Trailer: Adler-32 over the produced output, big-endian, must match. A
  mismatch is `Error::BadValue`.
- Trailing bytes after the final block are an error in `inflate_zlib`. A
  truncated or padded stream is not silently accepted.

### Truncation

Input that ends mid-stream, at any depth - mid-header, mid-Huffman-code,
mid-match - is `Error::Truncated`. It must never be reported as success with
partial output, and never as a panic.

## Security requirements

This crate parses attacker-controlled input. Therefore:

- No `unsafe`. Already `forbid(unsafe_code)` at the crate root.
- No allocation without a bound derived from `Limits`. Every `Vec::with_capacity`
  derived from a length field in the stream must be clamped by `max_output`.
- No recursion proportional to input depth. The Huffman decode loop is
  iterative; a crafted code-length vector must not be able to blow the stack.
- No unbounded loop without a progress condition. Every decode iteration must
  consume input, produce output, or fall out of the loop.

## Out of scope

- **Compression.** Not implemented, not planned.
- **gzip.** Recognised by `inflate_auto` so the caller gets
  `Error::Unsupported("gzip")` rather than a confusing parse error.
- **Preset dictionaries.** Rejected explicitly, per above.

## Conformance

`tests/vectors.rs` carries known-answer vectors generated by an independent
implementation, with the generating command recorded so they can be regenerated
and re-checked. A vector is useless if nobody can say where it came from.

Round-tripping this crate against itself proves nothing; every vector's expected
output must come from outside this workspace.
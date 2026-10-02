# `modhash-primitives` - the shared vocabulary

Status: specified, not yet implemented. This is the crate every other crate in
the workspace depends on, so its surface is fixed before any consumer is
written. Adding a type later is cheap; changing one after twenty crates have
been written against it is not.

Crate number `#0`. It may not depend on anything, including another workspace
crate.

## What belongs here

Only what two or more crates would otherwise each invent their own version of:
the error type, the digest value, the distance function, and the closed sets of
names that appear in an on-disk record.

Everything else belongs to the crate that uses it. `modhash-image` owns how a
DCT works; `modhash-png` owns PNG chunk walking; neither of those belongs in a
crate numbered zero.

## Types

### `Error` and `Result<T>`

One error type for the whole kit. Every fallible operation in the workspace
returns `Result<T, Error>`, so a caller that handles errors once handles all of
them.

```rust
pub enum Error {
    /// Input ended before a structure that the format guarantees.
    Truncated { what: &'static str, needed: usize, found: usize },
    /// A magic number, signature or fixed field did not match.
    InvalidMagic { what: &'static str },
    /// A structurally valid but semantically impossible value: a zero
    /// denominator, a declared length that cannot fit, a table index past the
    /// end of the table.
    BadValue(&'static str),
    /// A real format variant this crate deliberately does not implement.
    /// This is not a failure to parse; it is a refusal to guess.
    Unsupported(&'static str),
    /// An allocation the format could request but that exceeds a configured
    /// ceiling. Always a refusal, never an OOM.
    TooLarge { what: &'static str, limit: usize },
}

pub type Result<T> = core::result::Result<T, Error>;
```

Every variant carries a `&'static str` naming the thing, so an error message
names a field rather than saying "invalid input". `Display` for `Error` must be
`no_std`-compatible: no allocation, no formatting machinery. Plain
concatenation of the literal parts is enough.

Rules:

- `Display` never allocates.
- `Error` is `Copy`, `Clone`, `Debug`, `PartialEq`, `Eq`.
- Constructors exist for the common cases so callers do not spell struct
  literals: `Error::truncated(what, needed, found)` and
  `Error::too_large(what, limit)`.

### `Digest<const N: usize>`

A fixed-width hash value. `N` is the output width in bytes.

```rust
pub struct Digest<const N: usize> { bytes: [u8; N] }
```

Required impls: `Copy`, `Clone`, `Debug`, `PartialEq`, `Eq`, `PartialOrd`,
`Ord`, `Hash`, `Default`. `Default` is all-zero and is documented as "not a
valid hash of anything", because a zeroed buffer is what a failed hash
computation tends to leave behind and it must not be mistaken for a result.

Methods:

| method | behaviour |
|---|---|
| `from_bytes([u8; N]) -> Self` | wrap, never fail |
| `as_bytes(&self) -> &[u8; N]` | borrow |
| `to_hex(self) -> [u8; 2 * N]` | lowercase hex, no allocation, no `hex` crate |
| `from_hex(&[u8]) -> Result<Self>` | rejects odd length, rejects non-hex, rejects wrong `N` |
| `from_slice(&[u8]) -> Result<Self>` | rejects wrong length |
| `from_stream(&mut impl Read) -> Result<Self>` | reads exactly `N` bytes; short read is `Truncated` |

`to_hex` is a fixed-size array, not a `String`. Hex output in this workspace is
compared byte-for-byte against specification vectors constantly; a heap
allocation per comparison is the wrong trade, and returning a `String` would
make the common case pay for the rare one.

Implement `Display` as the lowercase hex form, so `format!("{digest}")` and
`digest.to_hex()` cannot drift apart.

### Hamming distance

```rust
pub fn hamming<const N: usize>(a: &Digest<N>, b: &Digest<N>) -> u32;
```

The number of differing bits. This is the single comparison used by every
perceptual crate - image, audio, video, text, and the index layer - so it is
specified once, here.

Implementation requirement: `count_ones` on a `u64`, not a per-bit loop. A
256-bit comparison done one bit at a time is a measurable cost in the index
scan path, and this function is on its hot loop.

Accuracy requirement: it must return the exact bit count. The conformance test
is a value with a known answer, not a loopback against itself.

### `Algorithm`

A closed set of the hash functions this workspace can produce, so an index
record can say what it was computed with.

```rust
pub enum Algorithm { Sha1, Sha256, Sha512, Blake3, Md5, XxHash64, FastCdc, Perceptual, Unknown }
```

- `as_str` and `from_str` are the stable wire form: `"sha256"`, `"fastcdc"`,
  `"perceptual"`, and so on. These strings appear in on-disk records and must
  not change without a format version bump.
- `output_len` returns the digest width in bytes for the fixed-width members,
  and `None` for `Perceptual`, whose width is a property of the modality.
- `Unknown` exists so a record written by a future version can still be read.
  Parsing an unknown string yields `Unknown`, never an error.

### `Format`

A closed set of the container formats the codecs decode.

```rust
pub enum Format { Png, Jpeg, Bmp, Zip, Mp4, Mp3, Wav, Flac, Pdf, Gif, Unknown }
```

Same rules as `Algorithm`: stable `as_str` / `from_str` wire form, `Unknown` on
an unrecognised string.

The enum is the set of formats the workspace *claims* to handle. A member is
added in the phase that implements that decoder, not before - an enum listing a
format that does not decode is a lie a caller can act on.

## What this crate must not do

- Depend on anything. Enforced by `scripts/gate_zero_dep.sh`.
- Contain an algorithm. No hash function, no distance metric other than the
  one specified above, no transform.
- Allocate. `core` only; there is no `std` feature and no allocator use.
- Read the filesystem, the network, the clock or the environment. Nothing in
  this crate may make a result depend on anything but its arguments.
# Algorithm specifications

Every capability in this workspace is specified here **before** it is
implemented. There is no upstream oracle for perceptual hashing, so a
specification that two people could not independently implement to the same
bytes is a specification that has not been written yet.

A file lands here in the same phase that owns its crate, and the crate's
`tests/` directory is built from the vectors and cases the file names.

## Status

| crate | spec | state |
|---|---|---|
| `modhash-primitives` | — | not started |
| `modhash-inflate` | RFC 1951 (external) | not started |
| `modhash-unicode` | §4.1 of the design spec + UCD `NormalizationTest.txt` | not started |
| `modhash-math` | §4.3 of the design spec | not started |
| `modhash-raster` | §4.4 of the design spec | not started |
| `modhash-audio` | §4.2 of the design spec | not started |
| `modhash-fastcdc` | `fastcdc.md` | implemented |
| `modhash` | `kit.md` | implemented |
| the rest | — | not started |

## What a specification must contain

1. **Exact output.** Byte-for-byte what the function returns, for a worked
   input given in full.
2. **The vector source.** Published vectors are cited by document and section.
   Self-written vectors are labelled as such and justified.
3. **The tolerance**, where the answer is not an integer: `< 1e-9` absolute,
   or whatever the specification actually requires.
4. **The failure modes.** For a decoder: which malformed inputs must produce an
   error, and which must be rejected with a named reason. Silently
   mis-decoding is not an acceptable outcome for any input.
5. **The decisions two implementers would otherwise disagree on.** Byte order,
   rounding, tie-breaking, the definition of the median for an even-length
   input, whether a hash covers the container or only its payload.

Point 5 is the reason this directory exists. Those disagreements are where two
independent implementations of the same specification diverge, and they are
cheapest to settle on paper.

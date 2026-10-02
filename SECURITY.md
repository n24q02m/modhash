# Security Policy

## Supported versions

The project is pre-1.0. Security fixes land on `main` and are published as
the next patch release of every affected crate.

| Version | Supported |
|---|---|
| 0.1.x | yes |
| < 0.1 | no |

## Reporting a vulnerability

Open a **private** security advisory on this repository
(`Security` -> `Report a vulnerability`). Do not open a public issue for a
vulnerability that has not been triaged.

Include: the crate name and version, what the input looks like, the observed
behaviour, and the expected behaviour. A fuzz seed that reproduces it is
worth more than a prose description.

## What counts as a vulnerability here

Because this library parses untrusted input, the decoder crates are the
attack surface. These are in scope:

- **Panics on untrusted input.** A malformed file must produce an error, not
  an abort. Out-of-bounds indexing, integer overflow in a length calculation,
  and unwrapping a value the format does not guarantee are all in scope.
- **Unbounded memory or CPU.** A declared length that drives an allocation is
  a denial of service if it is not checked against the remaining input first.
  This includes the fuzz corpus format itself.
- **Hash collisions in the content-defined chunker** that break a documented
  guarantee, such as a mask table that does not produce the documented chunk
  size distribution.
- **Incorrect normalisation** producing a hash collision for two inputs the
  specification says are canonically equivalent, or the reverse.

## What is out of scope

- **The perceptual hashes are not cryptographic.** `modhash-text`, `modhash-audio`
  and `modhash-video` are designed to be collision-prone under adversarial
  input; that is what makes them useful for similarity search. Do not use them
  for access control, deduplication of untrusted archives, or any integrity
  decision. Report a collision only when two inputs the specification calls
  canonically equal do not hash equal.
- Resource exhaustion from an input the caller itself declares as enormous,
  where the caller can bound the read first.
- Vulnerabilities in crates.io dependencies — this project has none, by design.

## Fuzzing

The fuzz harness is deterministic: `cargo run --release -p modhash --bin fuzz
-- <target> <iters> <seed>` reproduces any iteration from its seed alone.
Please include the exact command line with a report.

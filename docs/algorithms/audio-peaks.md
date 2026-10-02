# `audio-peaks` — spectral-peak fingerprint (design spec §4.2)

Shazam-style landmark extraction for 44 100 Hz PCM, plus the
`peak → (t, id)` inverted table that turns matching into a histogram
vote over time offsets.

Input contract: decoded interleaved `i32` samples — the exact shape
`modhash-wav` and `modhash-flac` emit — at **44 100 Hz**. The
`signature_of_wav` / `signature_of_flac` facades refuse any other rate
(`Error::Unsupported`) because every constant below is tuned to it.

## Constants

| name          | value   | meaning                                        |
| ------------- | ------- | ---------------------------------------------- |
| `SAMPLE_RATE` | 44 100  | fixed input rate                               |
| `WINDOW`      | 4096    | samples per analysis frame (92.9 ms), Hann     |
| `HOP`         | 2048    | frame advance (46.4 ms), 50 % overlap          |
| `BIN_STRIDE`  | 8       | FFT bins folded per quantization slot          |
| `SLOT_COUNT`  | 257     | `2049` real-FFT bins → slots `0..=256`         |
| `BIN_MIN`     | 3       | first in-band bin (32 Hz lower bound)          |
| `BIN_MAX`     | 464     | last in-band bin (5000 Hz upper bound)         |
| `SLOT_MIN`    | 0       | first slot reachable by `BIN_MIN..=BIN_MAX`    |
| `SLOT_MAX`    | 58      | last slot reachable by `BIN_MAX`               |
| `TIME_RADIUS` | 4       | temporal neighbourhood = `2*4+1` = 9 frames    |
| `DELTA_TOL`   | 1 frame | match vote tolerance = ±46.4 ms                |

## Quantization — where "257 bins" comes from

A 4096-point real FFT yields bins `0..=2048`, bin `b` centred at
`b · 44100/4096 Hz` (≈ 10.77 Hz). The spec's "257 bin" surface is the
fold `slot(b) = b >> 3`: bins `0..=2047` cover slots `0..=255` and the
Nyquist bin `2048` is slot `256` alone. Inside a frame each slot's
magnitude is the **maximum squared magnitude** of its member bins —
max-pooling, not averaging, so a strong partial-bin tone keeps its
slot loud.

## Frequency band

Only bins `BIN_MIN..=BIN_MAX` (3–464, ≈ 32–5000 Hz) are folded; the
band is applied to *bins* before pooling, so slot 0 (bins 3–7) and
slot 58 (bin 464) are partially covered. Everything below ~32 Hz
(DC/rumble) and above ~5 kHz is ignored by construction.

## Peak picking

A slot `s` in frame `t` with pooled magnitude `m` becomes a peak iff:

1. `m > 0` — silence never peaks.
2. **Frequency local maximum** inside the in-band slots
   (`SLOT_MIN..=SLOT_MAX`): `m > left` and `m >= right`. Strict-left /
   non-strict-right breaks flat-top ties deterministically toward the
   lowest slot; boundary slots treat the missing neighbour as
   magnitude −1 (the `m > 0` guard already excludes silence).
3. **Temporal local maximum** over the 9-frame window `t−4..=t+4`
   clipped to `[0, frames)`: `m` must beat **every** neighbour
   (strict both sides). A plateau therefore yields no peak at all —
   the deterministic reading of "local maximum", and the reason a
   perfectly stationary spectrum produces zero landmarks.

Per spec §4.2: for each frame `t` take its first (lowest-`f`) peak and
set bit `f mod 64` of a `u64`. Distinct frames OR their bits; the
result is a compact presence mask for cheap pre-screening, not the
matching key.

## Inverted table and matching

`build_index` maps `f → sorted [(t, id)]` across all indexed
signatures (id = position in the input slice).

`match_signature` looks up every query peak `f`, and for each stored
`(t_stored, id)` casts one histogram vote at
`Δt = t_query − t_stored` (frames). Per id the **modal Δt** is the bin
with the most votes — ties resolve to the *smallest* Δt — and the
reported `votes` are the histogram mass within `modal Δt ± DELTA_TOL`
(±1 frame = ±46.4 ms, the frame-boundary rounding allowance).

Result ordering: `votes` descending, then `id` ascending — total and
deterministic. A positive `Δt` means the query content plays later
than its indexed copy (e.g. a 2 s query prefix ⇒ Δt ≈ 88200/2048 ≈ 43).

## Hostile input

* `channels == 0` ⇒ `Err(BadValue)` — division by zero would panic.
* `samples.len() % channels != 0` ⇒ `Err(BadValue)` — a partial final
  frame is a caller bug, never guessed at.
* Fewer than `WINDOW` samples ⇒ empty signature (no panic, no peaks).
* Empty signature ⇒ empty match list.
* Every allocation is proportional to input length; there is no
  length-driven index arithmetic beyond `u32` frame counts, which real
  allocations make unreachable first.

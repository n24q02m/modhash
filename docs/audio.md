# Audio modality

**Formats:** wav (PCM 8/16/24/32-bit + float), flac, mp3 (layers
I/II/III, CBR + VBR, bit reservoir).

**Tier 1** downmixes to mono `i32` (`(Σ_c s[i·ch+c]) / ch` in i64,
truncated toward zero) and hashes `rate ∥ frames ∥ PCM` — the rate is
pinned in the header, so the same PCM at different rates hashes
differently. WAV and FLAC encodings of identical PCM share the digest
(pinned by `tone.wav`/`tone.flac`).

**Tier 2** is a Shazam-style spectral-peak signature at a fixed
44 100 Hz analysis rate: 4096-sample windows, hop 2048, local peaks in
the 32–5 000 Hz band within a 9-frame window. `match` builds an inverted
peak→(t,id) table over one side and counts votes at the modal Δt;
`votes ≥ 8` is the advisory match bound.

A wrong-rate input is refused, never silently resampled: a 22 050 Hz WAV
answers `audio: unsupported` from `signature`/`describe`.

**Tier 3** is banded DTW (Sakoe–Chiba 10%, O(n·w)) as a comparator —
there are no stored tier-3 audio features.

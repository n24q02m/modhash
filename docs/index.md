# modhash documentation

A zero-dependency Rust hashing kit: canonical content hashes, perceptual
signatures, local features, similarity indexes — every crate resolves
without a single registry package.

| page | what it covers |
|---|---|
| [quickstart](quickstart.md) | build the CLI, the six commands, exit codes |
| [concepts](concepts.md) | the three tiers and what each answers |
| [image](image.md) | png/bmp/jpeg → pixels → pHash → ORB |
| [audio](audio.md) | wav/flac/mp3 → PCM → spectral-peak landmarks |
| [text](text.md) | utf8/pdf → canonical text → MinHash |
| [binary](binary.md) | zip/gif/unknown → bytes → FastCDC chunks |
| [video](video.md) | mp4/h264 — pending, honestly reported |
| [limits](limits.md) | the exact codec-scope table and current status |
| [contributing](contributing.md) | the three invariants + style rules |
| [RECEIPTS](RECEIPTS.md) | every number in these docs, with provenance |

The algorithm-level specs (the "how exactly" of each pipeline) live in
`docs/algorithms/` alongside this site.

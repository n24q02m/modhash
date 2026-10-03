# Differential lane — modhash (Rust, zero-dep) vs hashkit 0.5.0 (Python, many-dep)

Lane `wave3/diff-oracle`. User directive 2026-10-03: "so sánh tat ca python
nhieu deps voi rust zero deps" — end-to-end on one corpus.

## Provenance

- **Rust**: worktree `modhash-wt-diff` at `81b826b` (origin/main), release
  build of `modhash-cli` + `probe/` harness (this lane only).
- **Python**: `C:/Users/n24q02m-wpc/projects/.audit-hashkit-290926/arena_ws`
  — **not a git repo** (no rev to pin; content pinned by
  `results/provenance.json`: SHA-256 tree-hash of `src/hashkit` +
  `pyproject.toml` + `uv.lock`). Run via `uv run --project arena_ws`.
  hashkit 0.5.0, deps: numpy/scipy/Pillow + optional soundfile,
  opencv-python, pypdf.
- **Corpus**: 72 files — `lab/smoke-fixtures` + `corpus/gen.py` output
  (manifest in `results/manifest.tsv`).
- **ffmpeg**: present on PATH; used only as a read-only reference decoder
  (extracted YUV420p per video) — never linked, never in Cargo.

## Parity audit (per algorithm)

| axis | verdict | note |
|---|---|---|
| binary tier-1 (SHA-256 raw) | **SAME** | `rust_tier1 == py raw_sha256` on all binary formats |
| text tier-1 (canonicalize) | **SAME** | `rust canon sha == sha(mirror py canonicalize)` — NFC+casefold+strip pipeline identical |
| text tier-2 (MinHash) | DIVERGED | different perm families; sig heads differ by design — Jaccard *estimate* still comparable |
| image tier-1 (rgb8 SHA) | **SAME** ≤1024 | byte-equal on every decoded image ≤1024px; py canonical_image resizes >1024px first |
| image tier-2 (pHash) | **SAME** | mirror `lab/phash_oracle.py` (kit.md chain) == rust `image_phash` == cv2-free equivalent |
| wav/flac pcm i32 | **SAME** | `sf.read(int32) >> 16` == rust native-range i32; `wav_native` path also MATCH on 16-bit WAV |
| mono downmix | **SAME** | `trunc((l+r)/2)` in native domain byte-identical |
| mp3 decode | DIVERGED | two different mp3 decoders; `pcm_max_abs_diff` large — no equality axis, documented |
| audio tier-2 (peaks) | DIVERGED | rust: 44.1k/4096/257-bin peaks; py: 16k/1024/64-bin MinHash-like keys — BY-DESIGN |
| video Y plane | **SAME** | rust h264 decode == ffmpeg rawvideo yuv420p, byte-equal, every frame every file |
| video frame phash | **SAME** | `frame_phash(Y)` == mirror phash on same Y plane, all 32×5 frames |
| video tier-2 chain | DIVERGED | rust: 2fps sample + phash + minhash; py: cv2 all-frames + phash64 — different frame sets |
| pdf text extract | DIVERGED | spec-ambiguous 8-bit encoding: py `0xEF→ï(U+00EF)` (WinAnsi/PDFDoc), rust `→U+FFFD` (StandardEncoding `.notdef`) |
| fastcdc chunks | DIVERGED | params + digest differ (gear vs sha256, 16k/32k/64k vs 4k/8k/16k windows) — count ratio only |
| minhash(file) | DIVERGED | same class, different perms |
| tier-3 ORB | PY-ONLY-ish | py ORB(cv2) vs rust FAST-9+brief — different feature sets, no axis |
| zip member map | **SAME** | `name→sha256` sets byte-equal on all 4 zip/docx fixtures |
| zip tier-2 cdc | DIVERGED | cdc params differ (count ratio reported) |

## Coverage matrix (ingest)

- **BOTH (59)**: all png/jpg/bmp/webp/gif/img16 variants, wav+flac+mp3
  mono/short, txt/md, zip/docx, pdf, mp4(h264 baseline 64×48/128×96/mov).
- **PY-N/A (2)**: `video/high_64x48.mp4` h264-High (py cv2 decodes, rust
  refuses High profile — named unsupported), `a_frag.mp4` similar refusal.
- **RUST-N/A (7)**: 44.1kHz-only audio lane refuses `l3_48k` (48k mp3),
  `tone22`/`audio_a*` (22k wav), plus pdf/`audioonly.mp4` no-video-track —
  all **named Unsupported**, never a panic.
- **NEITHER (4)**: `minimal.mp4` (truncated), `e_mp4v` (mpeg4-part2),
  audioonly, frag — both sides refuse with named errors.

## Diff report — every mismatch investigated

| finding | verdict | evidence |
|---|---|---|
| `text_page.pdf` + `standard_default.pdf` text: rust `na<U+FFFD>ve` vs py `naïve` | **DIVERGED spec-ambiguous** | stream `(na\357ve na)`; font `/Type1 /Times-Roman` no `/Encoding`. Rust falls back StandardEncoding (0xEF unassigned→U+FFFD, documented choice `font.rs:343`); pypdf resolves via WinAnsi/PDFDoc→U+00EF. Flagged for spec owner: pick `WinAnsiEncoding` fallback for non-symbolic Type1 (matches Adobe+pypdf) or keep + document. |
| `stereo_441.wav` `pcm_mono` first-run MISMATCH | **artifact, not a bug** | comparing `(mono)>>16` vs `mono(native)` double-floored negative odd sums; native-domain downmix is byte-identical (sha `0f9c35…` == `0f9c35…`) |
| mp3 `pcm_i32`/`pcm_mono` MISMATCH ×3 | BY-DESIGN | independent mp3 decoders; not bit-exact by spec |
| `a_frag`/`audioonly`/`minimal`/`e_mp4v`/`high_64x48` | named refusals | rust `unsupported:`/`bad value:`/`truncated:` — spec-shaped errors |
| all `phash_mirror_video` first-run MISMATCH | **artifact** | my `diff.py` import `lab.phash_oracle` failed (no `__init__.py`) → `mirror=None` → all False; fixed → all MATCH |

## BY-DESIGN one-liners (for docs/limits.md)

- `audio tier-1`: rust canonical = `u32 rate ∥ u64 n ∥ i32 mono @44.1k`;
  py canonical = 16k mono int16; only 44.1k files comparable.
- `mp3`: different decoders — tier-2 peaks comparable only.
- `pdf tier-1`: rust hashes extracted text, py hashes raw bytes — decode
  axis is the only shared one.
- `fastcdc/minhash`: different window+perm+digest — count/Jaccard-estimate
  only.

## Machine outputs

- `results/matrix.md` — per-file ingest/tier1/decode/tier2 matrix (72 rows)
- `results/summary.json` — full join + counts
- `results/{py_dump,py_fp,rust_probe,rust_cli,rust_hash,rust_match,py_match,provenance}.jsonl`
- `results/dumps/` (rust intermediates) · `results/dumps_py/` (py)
  · `results/dumps_ffmpeg/` (reference YUV)

## Runner

- `probe/` — rust harness (this lane): dumps rgb8/pcm/y-planes/pdf-text/
  zip-members/tier1/cdc/signature facts per corpus file
- `run_rust.sh` — `modhash describe|hash|match` → jsonl
- `run_py.sh` — `uv run` arena `py_dump.py` (two passes: `--no-fp` fast
  decode dump, `--fp-only` slow fingerprint), `py_match.py`, provenance
- `diff.py` — the join + status matrix above
- `corpus/gen.py` — fixture generator (copies smoke-fixtures + authors
  videos/pdfs/zips/texts)

## Known gap

`results/py_fp.jsonl` is 67/72 rows — the `--fp-only` pass was killed at
1800 s on the 5 `.mp4` files (hashkit's Python `phash64` runs a per-frame
O(32²·32²) DCT loop; cv2 decode alone is fine). Nothing is lost for the
parity verdict: the video tier-2 chain is already DIVERGED by design
(different frame sets), and the rust-side Y-planes + frame-phash are
proven byte-identical against ffmpeg + the mirror oracle. Backfill by
running `py_dump.py corpus --fp-only` over `corpus/video/` only if the
py-side frame hashes are ever needed.

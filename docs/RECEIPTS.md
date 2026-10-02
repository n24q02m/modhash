# docs/RECEIPTS.md — provenance for every number in the docs

Rule (plan P18): no number without real output + commit hash.
All runs: `modhash-cli` release build at commit `fd174a8` on
x86_64-pc-windows-msvc. Reproduce: `cargo build --release -p
modhash-cli`, then the command shown.

## `modhash describe` — three tiers per modality

```text
$ modhash describe phash_rgb8_48x40.png
modality: image
format: png
tier1: 1f7637f88725b1f3c1b60fb5559486349298b2a5525bac5fb1df196651e0cb74
tier2: phash 0xc21b869527e1a74f
tier3: orb keypoints=6
image: 48x40px

$ modhash describe tone.wav
modality: audio
format: wav
tier1: 0dfdd5e83844d2756e151e5ab6c44b918b6ce1ef8f50df9474f357efe1a8cc9d
tier2: audio peaks=99 frames=20
tier3: dtw (no stored features; comparator only)
audio: 44100 Hz, 1 ch

$ modhash describe text_page.pdf
modality: text
format: pdf
tier1: 052e6e100d2d0fc00ff499968324cd8cda4e87c4e31c453f857e5a438ff96b45
tier2: minhash 128 words (3b89e1d78306d4f8 484ee5e73b18def1 e1f90dca9ea32784 5747e91c93ad264e..)
tier3: none (docx lane shares text tier-2)
text: 2 words, canonical 11 B

$ modhash describe blob.bin
modality: binary
format: unknown
tier1: 238daef3e75579c2f0ff5bb68835a74a90385154f2465de56e309bb85a39a305
tier2: fastcdc 6 unique chunks
tier3: none (no local features for binary)
binary: 49152 B

```

## `modhash match` — same PCM in wav and flac

```text
$ modhash match tone.wav tone.flac
modality: audio
score: votes=99 delta_t=0
distance: 4294967196
verdict: match
advisory: votes >= 8
```

## `modhash dedup` — two identical files cluster

```text
$ modhash dedup lab/smoke-fixtures --all   # excerpt
group 1 (exact):
  lab/smoke-fixtures\base_444.jpg
  lab/smoke-fixtures\base_444.png
group 2:
  lab/smoke-fixtures\l3_short.mp3
  lab/smoke-fixtures\tone.flac
  lab/smoke-fixtures\tone.wav
single: lab/smoke-fixtures\PROVENANCE.md
single: lab/smoke-fixtures\blob.bin
single: lab/smoke-fixtures\minimal.zip
single: lab/smoke-fixtures\p24_top_down.bmp
single: lab/smoke-fixtures\phash_rgb8_48x40.png
single: lab/smoke-fixtures\prose.txt
single: lab/smoke-fixtures\text_page.pdf
2 group(s), 5 file(s) clustered, 7 single(s), 1 skipped
```

## `modhash bench` — tier-2 signature throughput (release)

```text
$ modhash bench --iters 5
image-bmp     589878 B x  5 iters    0.012 s     240.22 MB/s
audio-wav      88244 B x  5 iters    0.051 s       8.67 MB/s
text          262144 B x  5 iters    0.151 s       8.69 MB/s
binary       4194304 B x  5 iters    0.100 s     209.75 MB/s
# wall-clock tier-2 signature throughput; release=true arch=x86_64
```

## `lab/smoke_all_formats.sh` — every landed §4.5 format end-to-end

```text
# binary: target/release/modhash.exe (modhash 0.1.0)
== formats in the §4.5 table ==
[ ok ] png (phash_rgb8_48x40.png) — image/png, tier1 1f7637f88725b1f3…
[ ok ] bmp (p24_top_down.bmp) — image/bmp, tier1 c0158e32ac8f67cc…
[ ok ] jpeg (base_444.jpg) — image/jpeg, tier1 131ae3aa0f5d3b2b…
[ ok ] wav (tone.wav) — audio/wav, tier1 0dfdd5e83844d275…
[ ok ] flac (tone.flac) — audio/flac, tier1 0dfdd5e83844d275…
[ ok ] mp3 (l3_short.mp3) — audio/mp3, tier1 ddb366c273c68009…
[ ok ] mp4 (a_64x48.mp4) — video/mp4, tier1 72a4bbfd76647b2d…
       minimal.mp4 (hand-built, fake SPS) refuses with
       "video: truncated: ue(v) prefix" (exit 1, lane named)
[ ok ] zip (minimal.zip) — binary/zip, tier1 e31853d065dd9109…
[ ok ] pdf (text_page.pdf) — text/pdf, tier1 052e6e100d2d0fc0…
== non-container rows (text and binary by content) ==
[ ok ] text (prose.txt) — text/, tier1 754d411b311d9243…
[ ok ] binary (blob.bin) — binary/, tier1 238daef3e75579c2…
== cross-format sanity ==
[ ok ] jpeg/png of identical pixels share tier1: 131ae3aa0f5d3b2b…
[ ok ] match self: exit 0
[ ok ] match distinct images: exit 1 (no-match)
[ ok ] dedup clusters the known pair
[ ok ] bench
smoke: PASS — every §4.5 format with a landed lane ran end-to-end
```

Exit code 0 at commit `fd174a8`.

## smoke_all_formats with video row (2026-10-03, main 60ba679 + smoke row update)
- `bash lab/smoke_all_formats.sh` (msys2 bash, release binary rebuilt at run): 11/11 ok rows — png/bmp/jpeg/wav/flac/mp3/**mp4(a_64x48.mp4 → video/mp4, tier1 72a4bbfd76647b2d…)**/zip/pdf/text/binary + cross-format sanity (jpeg/png tier1 equality, match self=0/distinct=1, dedup, bench). Full output captured in this receipt; script exits 0.
- mp4 fixture: byte copy of modhash/tests/fixtures/a_64x48.mp4; provenance chain in lab/smoke-fixtures/PROVENANCE.md.

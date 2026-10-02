# Video modality

**Status: implemented.** `modhash describe` on an mp4/mov reports
`modality: video` with both tiers and the video facts row.

The pipeline (spec §4.2/§6, `modhash-video`): ISO-BMFF demux
(`modhash-mp4`) → H.264 baseline decode (`modhash-h264`: CAVLC, I/P
slices, 4:2:0, deblocking) → frames sampled at a fixed 2 fps
(`frame_index = floor(2·pts/timescale)`, first frame per slot in
presentation order) → the image pHash per kept frame → MinHash-128 over
consecutive 3-frame-hash shingles. `match` is the fraction of
temporally aligned sampled frames within Hamming ≤ 10.

Refusals are named: fragmented MP4 (`moof`), non-AVC codecs (HEVC,
MPEG-4 part 2, AV1, VP9), above-baseline H.264 (CABAC, B-frames, High
profile) and every `Limits` bound are errors, never guesses.

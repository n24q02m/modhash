# Video modality

**Status: pending.** The mp4 container demuxer (`modhash-mp4`) landed —
ISO-BMFF `moov`/`trak`/sample tables — but the modality lane that turns
frames into a signature (`modhash-video` over `modhash-h264`) has not.
`modhash describe` on an mp4 reports `modality: video`, `pending: true`
and exits 0; the hashing commands exit 1 naming the lane.

The design (spec §4.2/§6): demux → H.264 decode (baseline + main:
CAVLC + CABAC, I/P/B, 4:2:0, deblocking) → fixed-rate frames (default
2 fps) → 32×32 → the image pHash → MinHash over the frame-hash sequence;
comparison is the fraction of frames matching in time order.

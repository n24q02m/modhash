# Binary modality

**Inputs:** zip (store + deflate parsing exists in `modhash-zip`; the
signature lane is raw-byte semantics), GIF, and every non-UTF-8 input
that matches no container magic — the honest default, never a guess.

**Tier 1** is SHA-256 over the raw bytes, unchanged.

**Tier 2** chunks the input content-defined (FastCDC, buzhash window 64,
min 2 KiB / avg 8 KiB / max 32 KiB, default normalization) and keeps the
set of per-chunk SHA-256 digests. Distance is exact Jaccard over the
chunk sets; `≥ 0.5` is the advisory match bound. A one-byte-prefixed
file keeps most chunk boundaries and stays clustered — the
content-defined realignment is the point.

There are no tier-3 local features for opaque bytes.

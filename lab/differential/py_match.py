#!/usr/bin/env python3
"""Pair-side dump for the differential lane (runs inside arena_ws env).

Reads corpus/pairs.tsv and emits one JSON object per pair:
  * hashkit match() verdict (kind/score/tier) when both fingerprint,
  * the comparable scalar each side exposes (hamming / jaccard / Δt
    score / frame ratio) — the value Rust's scalar can be sanity-
    checked against even though signatures are divergent.

Usage: uv run --project $ARENA_WS python py_match.py <corpus-dir>
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np

from hashkit.core import fingerprint, match
from hashkit import sim


def hamming(a: int, b: int) -> int:
    return int((a ^ b) & ((1 << 64) - 1)).bit_count()


def main() -> int:
    corpus = Path(sys.argv[1])
    for line in (corpus / "pairs.tsv").read_text().splitlines()[1:]:
        if not line.strip():
            continue
        a, b, note = line.split("\t")
        rec = {"a": a, "b": b, "note": note}
        pa, pb = corpus / a, corpus / b
        try:
            fa = fingerprint(pa, read_file=True)
            fb = fingerprint(pb, read_file=True)
            rec["mod_a"], rec["mod_b"] = fa.modality, fb.modality
            if fa.modality == fb.modality:
                v = match(fa, fb)
                rec["verdict"] = {"kind": v.kind, "score": v.score,
                                  "tier": v.tier, "reason": v.reason}
                if fa.modality == "image":
                    rec["hamming_py"] = hamming(int(fa.sim), int(fb.sim))
                elif fa.modality in ("text", "file"):
                    sa, sb = np.asarray(fa.sim), np.asarray(fb.sim)
                    rec["jaccard_est_py"] = float(np.mean(sa == sb))
                    if fa.shingles is not None and fb.shingles is not None:
                        rec["jaccard_exact_py"] = sim.jaccard_exact(
                            fa.shingles, fb.shingles)
                elif fa.modality == "video":
                    rec["video_py"] = sim.video_match(fa.sim, fb.sim)
                elif fa.modality == "audio":
                    rec["audio_py"] = sim.audio_match(fa.sim, fb.sim)
            else:
                rec["verdict"] = "modality-mismatch"
        except Exception as e:
            rec["error"] = f"{type(e).__name__}: {e}"
        sys.stdout.write(json.dumps(rec, default=str) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())

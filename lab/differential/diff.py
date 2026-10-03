#!/usr/bin/env python3
"""diff.py — the differential report engine.

Joins the Rust and Python dumps on corpus file name and emits:

  * per-format × per-modality matrix rows:
      tier1   byte-equal SHA-256 on the axis where equality is defined
              (binary raw bytes; image rgb8 when both decode; text and
              pdf via the mirrored canonicalize)
      decode  decoded-content equality: image rgb8, wav/flac i32 PCM
              (convention search), mp3 approximate, video Y planes vs
              ffmpeg, pdf text Jaccard, zip member digest sets
      tier2   the comparable scalar each side exposes (cross-hamming
              for phash, cross-jaccard estimates, BY-DESIGN markers
              where no shared axis exists — audio peaks, video frames)
      ingest  coverage: which side read the format at all

Statuses: MATCH | MISMATCH | PY-N/A | RUST-N/A | BY-DESIGN | SKIP.

Usage: python diff.py [--results DIR] [--out-md PATH]
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RES = HERE / "results"
DUMPS = RES / "dumps"
DUMPS_PY = RES / "dumps_py"
DUMPS_FF = RES / "dumps_ffmpeg"

IMG_EXT = {".png", ".jpg", ".jpeg", ".bmp", ".gif", ".webp", ".tif", ".tiff"}
AUD_EXT = {".wav", ".flac", ".mp3", ".ogg", ".m4a", ".opus"}
VID_EXT = {".mp4", ".mov", ".m4v", ".avi", ".mkv", ".webm", ".wmv"}
ZIP_EXT = {".zip", ".docx", ".jar", ".apk"}
BIN_EXTS = {".bin", ".gz", ".pdf"}


def load_jsonl(p: Path) -> list[dict]:
    if not p.exists():
        return []
    return [json.loads(l) for l in p.read_text().splitlines() if l.strip()]


def rel_of(pathstr: str) -> str:
    """Normalize a file field to corpus-relative 'dir/name'."""
    parts = Path(pathstr.replace("\\", "/")).parts
    for i, seg in enumerate(parts):
        if seg in ("img", "audio", "video", "text", "bin", "real") \
                and i + 1 < len(parts):
            return "/".join(parts[i:])
    return parts[-1]


def words_of(path: Path) -> set[str]:
    if not path.exists():
        return set()
    import unicodedata
    s = path.read_text(encoding="utf-8", errors="replace")
    return set(unicodedata.normalize("NFC", s).lower().split())


def jacc(a: set, b: set) -> float:
    if not a and not b:
        return 1.0
    if not a or not b:
        return 0.0
    return len(a & b) / len(a | b)


def hamming_hex(h1: str, h2: str) -> int:
    return bin(int(h1, 16) ^ int(h2, 16)).count("1")


# mirror of lab/phash_oracle.py loaded once
sys.path.insert(0, str(HERE.parents[1] / "lab"))
try:
    from phash_oracle import phash as mirror_phash  # type: ignore
except Exception:
    mirror_phash = None


def mirror_phash_y(y: bytes, w: int, h: int) -> str | None:
    if mirror_phash is None:
        return None
    return format(mirror_phash(list(y), w, h, 1, 8), "016x")


# ---------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--results", default=str(RES))
    ap.add_argument("--out-md", default=str(RES / "matrix.md"))
    ap.add_argument("--out-json", default=str(RES / "summary.json"))
    args = ap.parse_args()
    res = Path(args.results)

    py = {rel_of(d.get("rel") or d["file"]): d
          for d in load_jsonl(res / "py_dump.jsonl")}
    for d in load_jsonl(res / "py_fp.jsonl"):
        k = rel_of(d.get("rel") or d["file"])
        if k in py:
            py[k] = {**py[k], **{kk: vv for kk, vv in d.items()
                                 if kk.startswith(("fp_", "phash_py",
                                                   "minhash_sig", "shingles",
                                                   "audio_keys", "file_chunks",
                                                   "video_frame", "algo"))}}

    probe = {rel_of(d["file"]): d for d in load_jsonl(res / "rust_probe.jsonl")}
    rust_cli = {rel_of(d["file"]): d for d in load_jsonl(res / "rust_cli.jsonl")}
    rust_match = load_jsonl(res / "rust_match.jsonl")
    py_match = load_jsonl(res / "py_match.jsonl")

    all_files = sorted(set(py) | set(probe))
    rows: list[dict] = []
    counts: dict[str, int] = {}

    def bump(k: str):
        counts[k] = counts.get(k, 0) + 1

    for rel in all_files:
        ext = Path(rel).suffix.lower()
        r, p = probe.get(rel), py.get(rel)
        row: dict = {"file": rel, "ext": ext}
        if r:
            row["rust_modality"] = r.get("modality")
            row["rust_format"] = r.get("format")
        if p:
            row["py_modality"] = p.get("modality_py")

        # ---------------- ingest / coverage ----------------
        rust_ok = r is not None and "signature_error" not in r
        py_ok = p is not None and "fp_error" not in p
        row["ingest"] = {"rust": "ok" if rust_ok else
                         ("refused:" + (r or {}).get("signature_error", "")[:90]
                          if r else "no-dump"),
                         "py": "ok" if py_ok else
                         ("error:" + (p or {}).get("fp_error", "")[:90]
                          if p else "no-dump")}
        if rust_ok and py_ok:
            row["ingest_status"] = "BOTH"
        elif rust_ok:
            row["ingest_status"] = "PY-N/A"
        elif py_ok:
            row["ingest_status"] = "RUST-N/A"
        else:
            row["ingest_status"] = "NEITHER"
        bump("ingest_" + row["ingest_status"])

        # ---------------- tier-1 ----------------
        t1r = (r or {}).get("tier1")
        if ext in IMG_EXT:
            # image tier-1 axis = decoded rgb8 (python canonical adds a
            # >1024 resize, so only ≤1024 is an equality axis)
            if r and p and p.get("rgb8"):
                if (r.get("rgb8") or {}).get("sha256") == p["rgb8"]["sha256"]:
                    if p.get("tier1_rust_equiv") == t1r:
                        row["tier1"] = "MATCH"
                    else:
                        row["tier1"] = ("BY-DESIGN(py-resize>1024)"
                                        if p["rgb8"].get("w", 0) > 1024
                                        or p["rgb8"].get("h", 0) > 1024
                                        else "MISMATCH")
                elif r.get("rgb8"):
                    row["tier1"] = "MISMATCH"   # decoded pixels differ
                else:
                    row["tier1"] = "RUST-N/A"
            elif r is None or r.get("rgb8_error"):
                row["tier1"] = "RUST-N/A"
            else:
                row["tier1"] = "PY-N/A"
        elif ext == ".pdf":
            # rust hashes canonicalized *extracted* text; py hashes raw
            # bytes — no shared axis at tier-1 level. The decode axis
            # (extracted text) is the comparable one below.
            row["tier1"] = "BY-DESIGN(pdf: rust=text, py=bytes)"
        elif ext in {".txt", ".md"} or (r and r.get("canon")):
            if r and p and p.get("rust_canon") and r.get("canon"):
                row["tier1"] = ("MATCH" if r["canon"]["sha256"]
                                == p["rust_canon"]["sha256"] else "MISMATCH")
            else:
                row["tier1"] = "SKIP"
        elif ext in AUD_EXT or ext in VID_EXT:
            row["tier1"] = "BY-DESIGN(canonical differs)"
        else:
            # binary lane: raw byte hash — the only true SAME axis
            if t1r and p:
                row["tier1"] = ("MATCH" if t1r == p["raw_sha256"]
                                else "MISMATCH")
            elif r and t1r is None and r.get("tier1_error"):
                row["tier1"] = "RUST-N/A"
            else:
                row["tier1"] = "SKIP"
        bump("tier1_" + row["tier1"].split("(")[0])

        # ---------------- decode-exactness ----------------
        d = {}
        if ext in IMG_EXT:
            if r and r.get("rgb8") and p and p.get("rgb8"):
                same = r["rgb8"]["sha256"] == p["rgb8"]["sha256"]
                d["image_rgb8"] = "MATCH" if same else "MISMATCH"
                if not same:
                    rb = (res / "dumps" / (Path(rel).name + ".rgb"))
                    pb = (res / "dumps_py" / (Path(rel).name + ".rgb"))
                    if rb.exists() and pb.exists():
                        a, b = rb.read_bytes(), pb.read_bytes()
                        if len(a) == len(b):
                            ndiff = sum(x != y for x, y in zip(a, b))
                            d["rgb8_diff_bytes"] = ndiff
                            d["rgb8_diff_pct"] = round(100 * ndiff / len(a), 4)
            elif r and r.get("rgb8_error"):
                d["image_rgb8"] = "RUST-N/A"
            elif p and p.get("rgb8_error"):
                d["image_rgb8"] = "PY-N/A"
            else:
                d["image_rgb8"] = "SKIP"
            bump("decode_img_" + d["image_rgb8"])
        if ext in AUD_EXT:
            if r and r.get("pcm") and p and p.get("sf_i32_sha"):
                rsha = r["pcm"]["i32_sha256"]
                conv = next((k for k, v in p["sf_i32_sha"].items()
                             if v == rsha), None)
                d["pcm_i32"] = f"MATCH({conv})" if conv else "MISMATCH"
                # mono comparison (tier-1 axis): downmix must happen in
                # the *native* i32 domain (>>16 after interleave), not by
                # shifting a high-range mono — double-floor on negative
                # odd sums is a diffing artifact.
                mconv = None
                pb_raw = res / "dumps_py" / (Path(rel).name + ".sf.pcm")
                if pb_raw.exists() and r["pcm"].get("channels"):
                    import numpy as _np
                    a = _np.frombuffer(pb_raw.read_bytes(),
                                       dtype="<i4").reshape(
                        -1, int(r["pcm"]["channels"]))
                    conv_shift = {"native": 0, "shl8": 8, "shl16": 16,
                                  "shl24": 24}.get(conv or "")
                    if conv_shift:
                        n = _np.right_shift(a, conv_shift).astype(_np.int64)
                        m = ((_np.sign(n.sum(1)) * (_np.abs(n.sum(1))
                             // a.shape[1])).astype(_np.int32))
                        import hashlib as _hl
                        if _hl.sha256(m.astype("<i4").tobytes()).hexdigest() \
                                == r["pcm"]["mono_sha256"]:
                            mconv = f"native-via-{conv}"
                if mconv is None:
                    mconv = next((k for k, v in (p.get("sf_mono_i32_sha") or {})
                                 .items() if v == r["pcm"]["mono_sha256"]), None)
                d["pcm_mono"] = f"MATCH({mconv})" if mconv else "MISMATCH"
                if not conv or not mconv:
                    rb = res / "dumps" / (Path(rel).name + ".pcm")
                    pb = res / "dumps_py" / (Path(rel).name + ".sf.pcm")
                    if rb.exists() and pb.exists():
                        a, b = rb.read_bytes(), pb.read_bytes()
                        n = min(len(a), len(b))
                        if n:
                            import struct
                            diff = max(
                                abs(int.from_bytes(a[i:i+4], "little", signed=True)
                                    - int.from_bytes(b[i:i+4], "little", signed=True))
                                for i in range(0, n - n % 4, 4))
                            d["pcm_max_abs_diff"] = diff
                if ext == ".wav" and p.get("wav_native_i32_sha"):
                    d["wav_native"] = ("MATCH" if p["wav_native_i32_sha"]
                                       == rsha else "MISMATCH")
            elif r and r.get("pcm_error"):
                d["pcm_i32"] = "RUST-N/A"
            elif p and not p.get("sf_i32_sha"):
                d["pcm_i32"] = "PY-N/A"
            else:
                d["pcm_i32"] = "SKIP"
            bump("decode_aud_" + d["pcm_i32"].split("(")[0])
        if ext in VID_EXT:
            if r and r.get("decoded") and (res / "dumps_ffmpeg"
                                           / (Path(rel).name + ".yuv")).exists():
                ff = (res / "dumps_ffmpeg" / (Path(rel).name + ".yuv")).read_bytes()
                frames = r["decoded"]["frames"]
                per = []
                ok = True
                for fr in frames:
                    i, w, h = fr["i"], fr["w"], fr["h"]
                    ylen = w * h
                    off = i * (ylen + ylen // 2)
                    fy = ff[off:off + ylen]
                    match_y = (len(fy) == ylen
                               and __import__("hashlib").sha256(fy).hexdigest()
                               == fr["y_sha256"])
                    if not match_y:
                        ok = False
                    mp = mirror_phash_y(
                        (res / "dumps" / (Path(rel).name + f".f{i:03}.y")).read_bytes(),
                        w, h) if (res / "dumps" / (Path(rel).name + f".f{i:03}.y")).exists() else None
                    per.append({"i": i, "y_eq_ffmpeg": match_y,
                                "phash_eq_mirror": mp == fr["phash"]})
                d["video_y"] = "MATCH" if ok else "MISMATCH"
                d["frames"] = per
                d["phash_mirror_video"] = ("MATCH" if all(
                    f["phash_eq_mirror"] for f in per) else "MISMATCH")
            elif r and r.get("decoded_error"):
                d["video_y"] = "RUST-N/A"
            elif not (res / "dumps_ffmpeg" / (Path(rel).name + ".yuv")).exists():
                d["video_y"] = "SKIP(no-ffmpeg-ref)"
            else:
                d["video_y"] = "SKIP"
            bump("decode_vid_" + d["video_y"].split("(")[0])
        if ext == ".pdf":
            rt = res / "dumps" / (Path(rel).name + ".pdftext")
            pt = res / "dumps_py" / (Path(rel).name + ".pdftext")
            if r and r.get("pdf_text") and pt.exists() and p:
                rw = words_of(rt)
                pw = words_of(pt)
                j = jacc(rw, pw)
                same_canon = r["pdf_text"]["canon_sha256"] == \
                    (p.get("pypdf") or {}).get("canon_sha256")
                d["pdf_text_jaccard"] = round(j, 4)
                d["pdf_text"] = "MATCH" if same_canon else (
                    "CLOSE" if j > 0.95 else "MISMATCH")
                if not same_canon:
                    d["pdf_text_bytes"] = {"rust": r["pdf_text"]["bytes"],
                                           "py": pt.stat().st_size}
            elif r and r.get("pdf_text_error"):
                d["pdf_text"] = "RUST-N/A"
            elif p and p.get("pypdf_error"):
                d["pdf_text"] = "PY-N/A"
            else:
                d["pdf_text"] = "SKIP"
            bump("decode_pdf_" + d["pdf_text"])
        if ext in ZIP_EXT:
            if r and r.get("zip") and p and p.get("zip_members"):
                rm = {e["name"]: e["sha256"] for e in r["zip"]["entries"]
                      if "sha256" in e}
                pm = {m["name"]: m["sha256"] for m in p["zip_members"]}
                d["zip_members"] = "MATCH" if rm == pm else "MISMATCH"
                if rm != pm:
                    d["zip_diff"] = {"rust_only": sorted(set(rm) - set(pm)),
                                     "py_only": sorted(set(pm) - set(rm)),
                                     "content_diff": sorted(
                                         n for n in set(rm) & set(pm)
                                         if rm[n] != pm[n])}
            elif r and r.get("zip_error"):
                d["zip_members"] = "RUST-N/A"
            elif p and p.get("zip_error"):
                d["zip_members"] = "PY-N/A"
            else:
                d["zip_members"] = "SKIP"
            bump("decode_zip_" + d["zip_members"])
        if ext in BIN_EXTS or ext in ZIP_EXT:
            if r and r.get("cdc") and p and p.get("cdc_py"):
                d["cdc_chunks"] = {"rust": len(r["cdc"]["chunks"]),
                                   "py": p["cdc_py"]["count"],
                                   "ratio": round(len(r["cdc"]["chunks"])
                                                  / max(p["cdc_py"]["count"], 1), 3)}
        if ext == ".docx":
            if r and p and p.get("docx_text"):
                # rust has no docx text lane (binary routing); the only
                # axis is that both zip-decode the members identically.
                d["docx_text"] = "PY-ONLY(rust binary lane)"
        row["decode"] = d or "SKIP"

        # ---------------- tier-2 comparable scalar ----------------
        t2: dict = {}
        if ext in IMG_EXT:
            pr = (r or {}).get("phash")
            pp = (p or {}).get("phash_py")
            pm = (p or {}).get("phash_mirror")
            if pr and pp:
                t2["phash_cross_hamming"] = hamming_hex(pr, pp)
            if pr and pm:
                t2["phash_mirror_hamming"] = hamming_hex(pr, pm)
                t2["phash_status"] = ("MATCH" if pr == pm else
                                      ("CLOSE" if hamming_hex(pr, pm) <= 4
                                       else "MISMATCH"))
            elif not pr:
                t2["phash_status"] = "RUST-N/A"
            bump("t2_phash_" + t2.get("phash_status", "NA"))
        elif ext == ".pdf" or (r and r.get("minhash")):
            if r and r.get("minhash") and p and p.get("minhash_sig_head"):
                t2["minhash_head"] = {
                    "rust": r["minhash"][:4],
                    "py": p["minhash_sig_head"],
                }
            t2["minhash_status"] = "BY-DESIGN(different perm families)"
        elif ext in AUD_EXT:
            t2["audio_peaks"] = "BY-DESIGN(no shared axis)"
            if r and r.get("audio_sig"):
                t2["rust"] = r["audio_sig"]
            if p and p.get("audio_keys") is not None:
                t2["py_keys"] = p["audio_keys"]
        elif ext in VID_EXT:
            t2["video_chain"] = "BY-DESIGN(fps/grid differ)"
        elif ext in BIN_EXTS or ext in ZIP_EXT:
            t2["chunk_set"] = "BY-DESIGN(cdc params+digest differ)"
        row["tier2"] = t2 or "SKIP"
        rows.append(row)

    # ---------------- pair verdicts ----------------
    pairs = []
    rm = {(rel_of(x["a"]), rel_of(x["b"])): x for x in rust_match}
    pm = {(x["a"], x["b"]): x for x in py_match}
    for (a, b), rv in sorted(rm.items()):
        pv = pm.get((a, b)) or pm.get((b, a)) or {}
        rec = {"a": a, "b": b,
               "rust": {"matched": rv.get("matched"), "score": rv.get("score"),
                        "distance": rv.get("distance"),
                        "error": rv.get("stderr") if rv.get("cli_error") else None,
                        "modality": rv.get("modality")},
               "py": {"kind": (pv.get("verdict") or {}).get("kind")
                      if isinstance(pv.get("verdict"), dict) else pv.get("verdict"),
                      "score": (pv.get("verdict") or {}).get("score")
                      if isinstance(pv.get("verdict"), dict) else None,
                      "hamming_py": pv.get("hamming_py"),
                      "jaccard_est_py": pv.get("jaccard_est_py"),
                      "jaccard_exact_py": pv.get("jaccard_exact_py"),
                      "video_py": pv.get("video_py"),
                      "error": pv.get("error")}}
        rust_yes = rv.get("matched")
        py_kind = (pv.get("verdict") or {})
        py_yes = py_kind.get("kind") in ("exact", "near") \
            if isinstance(py_kind, dict) else None
        if rust_yes is None or py_yes is None:
            rec["agree"] = "n/a"
        else:
            rec["agree"] = ("YES" if rust_yes == (py_yes or False)
                            or (rust_yes and py_kind.get("kind") == "maybe")
                            else "PARTIAL" if py_kind.get("kind") == "maybe"
                            else "NO")
        pairs.append(rec)

    # ---------------- render ----------------
    def cell(v):
        return "" if v is None else str(v)

    lines = ["# Differential matrix — modhash (Rust, zero-dep) vs hashkit 0.5.0 (Python)",
             "",
             "Statuses: MATCH byte-equal on that axis · CLOSE small drift ·",
             "MISMATCH real divergence found · BY-DESIGN divergent by spec ·",
             "PY-N/A python cannot ingest · RUST-N/A rust refuses (named) · SKIP.",
             ""]
    lines.append("## Coverage (ingest)")
    lines += ["",
              "| file | rust modality | rust | python | status |",
              "|---|---|---|---|---|"]
    for row in rows:
        lines.append(f"| {row['file']} | {cell(row.get('rust_modality'))} | "
                     f"{row['ingest']['rust'][:60]} | {row['ingest']['py'][:60]} | "
                     f"{row['ingest_status']} |")
    lines += ["", "## Tier-1 (SHA-256 on canonical payload)", "",
              "| file | status |", "|---|---|"]
    for row in rows:
        lines.append(f"| {row['file']} | {row.get('tier1', 'SKIP')} |")
    lines += ["", "## Decode-exactness (dep decoders vs Rust decoders)", "",
              "| file | axis results |", "|---|---|"]
    for row in rows:
        d = row["decode"]
        txt = " · ".join(f"{k}={v}" if not isinstance(v, list) else
                         f"{k}=[{len(v)} frames]" for k, v in d.items()) \
            if isinstance(d, dict) else d
        lines.append(f"| {row['file']} | {txt[:200]} |")
    lines += ["", "## Tier-2 (comparable scalars)", "",
              "| file | results |", "|---|---|"]
    for row in rows:
        t = row["tier2"]
        txt = " · ".join(f"{k}={v}" for k, v in t.items()) \
            if isinstance(t, dict) else t
        lines.append(f"| {row['file']} | {txt[:200]} |")
    lines += ["", "## Pair verdicts", "",
              "| pair | rust | python | agree |", "|---|---|---|---|"]
    for pr in pairs:
        lines.append(f"| {pr['a']} ↔ {pr['b']} | {pr['rust']} | {pr['py']} | {pr['agree']} |"
                     .replace("\n", " ")[:400])

    Path(args.out_md).write_text("\n".join(lines), encoding="utf-8")
    Path(args.out_json).write_text(json.dumps(
        {"counts": counts, "rows": rows, "pairs": pairs}, indent=1),
        encoding="utf-8")
    print(json.dumps(counts, indent=1))
    print(f"rows={len(rows)} pairs={len(pairs)} -> {args.out_md}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

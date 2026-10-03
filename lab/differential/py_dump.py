#!/usr/bin/env python3
"""Python-side dumper for the differential lane.

Runs INSIDE the hashkit 0.5.0 environment (uv run --project $ARENA_WS).
Emits one JSON object per corpus file on stdout with:

  * hashkit's own verdict data (exact tier-1 string, tier-2 signature
    facts, modality detection),
  * decoded-content digests from the *dep* decoders (Pillow RGB8,
    soundfile PCM in several int32 conventions, cv2 video, pypdf text,
    zipfile member digests) — the decode-exactness axis,
  * mirror computations replicating the Rust spec on the Python-decoded
    pixels: rust-canonicalize text, rust luma/box/DCT phash — this
    separates "decoder differs" from "phash math differs".

Usage: uv run --project $ARENA_WS python py_dump.py <corpus-dir>
"""
from __future__ import annotations

import hashlib
import io
import json
import math
import sys
import unicodedata
import wave
import zipfile
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lab"))
# phash_oracle is the committed Rust-spec mirror (stdlib only).
try:
    from lab.phash_oracle import phash as mirror_phash  # type: ignore
except Exception:  # pragma: no cover
    mirror_phash = None

from hashkit import canonical as canon        # noqa: E402
from hashkit import sim                       # noqa: E402
from hashkit.core import fingerprint          # noqa: E402

AUDIO_I32_CONVENTIONS = ("native", "shl8", "shl16", "shl24")

import numpy as np
_ARGS = [a for a in sys.argv[1:] if not a.startswith("--")]
DUMP_DIR = Path(_ARGS[1]) if len(_ARGS) > 1 else None
NO_FP = "--no-fp" in sys.argv
FP_ONLY = "--fp-only" in sys.argv
if DUMP_DIR:
    DUMP_DIR.mkdir(parents=True, exist_ok=True)


def dump(name: str, data: bytes) -> None:
    if DUMP_DIR:
        (DUMP_DIR / name).write_bytes(data)


def sha(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def i32_bytes(arr: np.ndarray) -> bytes:
    return np.asarray(arr, dtype="<i4").tobytes()


def u8_bytes(arr: np.ndarray) -> bytes:
    return np.asarray(arr, dtype=np.uint8).tobytes()


# ------------------------------------------------------------- images

def pil_rgb8(path: Path) -> tuple[np.ndarray, str]:
    """Decoded RGB u8 like Rust's image_to_rgb8: gray replicated, alpha
    dropped, 16-bit channels reduced by >>8 (high byte)."""
    from PIL import Image
    im = Image.open(path)
    mode = im.mode
    if mode == "I;16" or mode == "I;16B" or mode == "I":
        a = np.asarray(im).astype(np.uint32)
        if a.dtype == np.uint16 or a.max() > 255:
            g = (a >> 8).astype(np.uint8)
        else:
            g = a.astype(np.uint8)
        return np.stack([g, g, g], axis=-1), f"{mode}->gray>>8"
    if mode in ("L", "P", "1", "LA", "PA"):
        g = np.asarray(im.convert("L"), dtype=np.uint8)
        return np.stack([g, g, g], axis=-1), f"{mode}->L"
    a = np.asarray(im.convert("RGB"), dtype=np.uint8)
    return a, f"{mode}->RGB"


def mirror_phash_of(arr_rgb: np.ndarray) -> str | None:
    """Rust-spec phash on already-decoded pixels (isolates phash math)."""
    if mirror_phash is None:
        return None
    h, w = arr_rgb.shape[:2]
    flat = arr_rgb.reshape(-1, 3).tolist()
    return format(mirror_phash(flat, w, h, 3, 8), "016x")


# ------------------------------------------------------------- audio

def wav_native_i32(path: Path) -> tuple[np.ndarray, int, int, int] | None:
    """stdlib wave → i32 in RUST convention: u8-128 / i16 / i24 sign-extend / i32."""
    try:
        with wave.open(str(path), "rb") as w:
            ch, sw, rate, n = (w.getnchannels(), w.getsampwidth(),
                               w.getframerate(), w.getnframes())
            raw = w.readframes(n)
        if sw == 1:
            a = np.frombuffer(raw, dtype=np.uint8).astype(np.int32) - 128
        elif sw == 2:
            a = np.frombuffer(raw, dtype="<i2").astype(np.int32)
        elif sw == 3:
            b = np.frombuffer(raw, dtype=np.uint8).reshape(-1, 3)
            v = (b[:, 0].astype(np.int32) | (b[:, 1].astype(np.int32) << 8)
                 | (b[:, 2].astype(np.int32) << 16))
            a = np.where(v & 0x800000, v | ~0xFFFFFF, v).astype(np.int32)
        elif sw == 4:
            a = np.frombuffer(raw, dtype="<i4").astype(np.int32)
        else:
            return None
        return a, rate, ch, sw * 8
    except Exception:
        return None


def sf_i32(path: Path) -> tuple[np.ndarray, int, int] | None:
    """soundfile → int32 left-justified; returns (arr, rate, ch)."""
    import soundfile as sf
    try:
        a, rate = sf.read(str(path), dtype="int32", always_2d=True)
    except Exception:
        try:
            f, rate = sf.read(str(path), dtype="float32", always_2d=True)
            a = np.trunc(f.astype(np.float64) * 2147483648.0).clip(
                -2147483648, 2147483647).astype(np.int32)
        except Exception:
            return None
    return a, rate, a.shape[1]


# ------------------------------------------------------------- text

def rust_canonicalize(s: str) -> str:
    """modhash_text::canonicalize mirror: NFC → to_lowercase → trim → +\\n."""
    return unicodedata.normalize("NFC", s).lower().rstrip() + "\n"


# ------------------------------------------------------------- video

def cv2_frames(path: Path) -> tuple[int, int, int, float] | None:
    """(frames, w, h, src_fps) via OpenCV, or None."""
    try:
        import cv2
        cap = cv2.VideoCapture(str(path))
        fps = cap.get(cv2.CAP_PROP_FPS) or 0.0
        n, w, h = 0, 0, 0
        while True:
            ok, fr = cap.read()
            if not ok:
                break
            if n == 0:
                h, w = fr.shape[:2]
            n += 1
        cap.release()
        return (n, w, h, fps) if n else None
    except Exception:
        return None


# ---------------------------------------------------------------- main

def dump_file(path: Path) -> dict:
    rec: dict = {"file": path.name, "rel": str(path.relative_to(path.parents[1])),
                 "ext": path.suffix.lower(), "bytes": path.stat().st_size}
    data = path.read_bytes()
    rec["raw_sha256"] = sha(data)
    try:
        rec["modality_py"] = canon.detect_modality(path)
    except Exception as e:
        rec["modality_py"] = f"error:{type(e).__name__}"
    # hashkit fingerprint (its own failure is a datum, not a crash).
    if not NO_FP:
        try:
            fp = fingerprint(path, read_file=True)
            rec["fp_exact"] = fp.exact
            rec["fp_facts"] = {
                k: (str(v) if not isinstance(v, (int, float, str)) else v)
                for k, v in (fp.meta or {}).items()}
            rec["algo"] = (fp.meta or {}).get("algo")
            if fp.modality == "image":
                rec["phash_py"] = format(int(fp.sim), "016x")
            elif fp.modality == "text":
                rec["minhash_sig_head"] = [
                    format(int(x), "016x")
                    for x in np.asarray(fp.sim)[:4]]
                rec["shingles"] = (fp.meta or {}).get("shingles")
            elif fp.modality == "audio":
                rec["audio_keys"] = int(fp.sim.get("n_keys", 0))
            elif fp.modality == "file":
                rec["file_chunks"] = (fp.meta or {}).get("chunks")
            elif fp.modality == "video":
                rec["video_frame_hashes"] = [
                    format(int(h), "016x") for h in fp.sim]
                rec["video_frames_py"] = (fp.meta or {}).get("frames")
        except Exception as e:
            rec["fp_error"] = f"{type(e).__name__}: {e}"

    ext = rec["ext"]
    # ---------- decode-exactness dumps ----------
    if FP_ONLY:
        return rec
    if ext in {".png", ".jpg", ".jpeg", ".bmp", ".gif", ".webp", ".tif", ".tiff"}:
        try:
            rgb, conv = pil_rgb8(path)
            h, w = rgb.shape[:2]
            raw = u8_bytes(rgb)
            dump(path.name + ".rgb", raw)
            rec["rgb8"] = {"w": int(w), "h": int(h), "sha256": sha(u8_bytes(rgb)),
                           "pil_conv": conv}
            rec["phash_mirror"] = mirror_phash_of(rgb)
            if max(w, h) <= 1024:
                rec["tier1_rust_equiv"] = rec["rgb8"]["sha256"]
            else:
                arr_c, _ver = canon.canonical_image(path)
                rec["tier1_py_canon"] = sha(u8_bytes(arr_c))
        except Exception as e:
            rec["rgb8_error"] = f"{type(e).__name__}: {e}"

    if ext in {".wav", ".flac", ".mp3", ".ogg", ".m4a", ".opus"}:
        if ext == ".wav":
            got = wav_native_i32(path)
            if got:
                a, rate, ch, bits = got
                rec["wav_native_i32_sha"] = sha(i32_bytes(a))
                rec["wav_rate"] = rate
                rec["wav_channels"] = ch
                rec["wav_bits"] = bits
                dump(path.name + ".native.pcm", i32_bytes(a))
                mono = a.reshape(-1, ch).astype(np.int64).sum(axis=1) // ch
        got = sf_i32(path)
        if got:
            a, rate, ch = got
            rec["sf_rate"] = rate
            rec["sf_channels"] = ch
            rec["sf_frames"] = int(a.shape[0])
            dump(path.name + ".sf.pcm", i32_bytes(a.ravel()))
            convs: dict[str, str] = {}
            for name, shift in (("native", 0), ("shl8", 8), ("shl16", 16),
                                ("shl24", 24)):
                convs[name] = sha(i32_bytes(np.right_shift(a, shift)
                                            .astype(np.int32).ravel()))
            rec["sf_i32_sha"] = convs
            # mono the way the facade does: integer mean, trunc toward 0
            mono64 = a.astype(np.int64).sum(axis=1)
            mono = (np.sign(mono64) * (np.abs(mono64) // ch)).astype(np.int32)
            rec["sf_mono_i32_sha"] = {
                "native": sha(i32_bytes(mono)),
                "shl8": sha(i32_bytes(mono >> 8)),
                "shl16": sha(i32_bytes(mono >> 16)),
                "shl24": sha(i32_bytes(mono >> 24)),
            }

    if ext in {".mp4", ".mov", ".m4v", ".avi", ".mkv", ".webm", ".wmv"}:
        got = cv2_frames(path)
        if got:
            n, w, h, fps = got
            rec["cv2"] = {"frames": n, "w": w, "h": h, "src_fps": fps}
        else:
            rec["cv2_error"] = "cv2 could not decode any frame"

    if ext == ".pdf":
        try:
            from pypdf import PdfReader
            reader = PdfReader(str(path))
            pages = [p.extract_text() or "" for p in reader.pages]
            joined = "\n".join(pages)
            dump(path.name + ".pdftext", joined.encode())
            rc = rust_canonicalize(joined)
            rec["pypdf"] = {"pages": len(pages), "text_sha256": sha(joined.encode()),
                            "canon_sha256": sha(rc.encode()),
                            "words": len(rc.split())}
            rec["tier1_rust_equiv"] = rec["pypdf"]["canon_sha256"]
        except Exception as e:
            rec["pypdf_error"] = f"{type(e).__name__}: {e}"

    if ext in {".zip", ".docx", ".jar", ".apk"}:
        try:
            with zipfile.ZipFile(path) as z:
                members = []
                for info in z.infolist():
                    payload = z.read(info)
                    members.append({"name": info.filename,
                                    "method": info.compress_type,
                                    "len": len(payload),
                                    "crc32": format(info.CRC, "08x"),
                                    "sha256": sha(payload)})
                rec["zip_members"] = members
            if ext == ".docx":
                with zipfile.ZipFile(path) as z:
                    xml = z.read("word/document.xml").decode("utf-8", "replace")
                import re
                text = " ".join(re.findall(r"<w:t[^>]*>([^<]*)</w:t>", xml))
                rec["docx_text"] = {"sha256": sha(text.encode()),
                                    "canon_sha256": sha(rust_canonicalize(text).encode()),
                                    "words": len(text.split())}
        except Exception as e:
            rec["zip_error"] = f"{type(e).__name__}: {e}"

    # FastCDC (python variant) bounds for every binary-ish file.
    if rec.get("modality_py") == "file" or ext in {".bin", ".zip", ".docx",
                                                 ".gz", ".avi", ".gif"}:
        try:
            chunks = sim.cdc_chunks(data)
            rec["cdc_py"] = {"chunks": [[o, l] for o, l in chunks],
                             "count": len(chunks)}
        except Exception as e:
            rec["cdc_py_error"] = f"{type(e).__name__}: {e}"

    # ---------- text canonicalizations ----------
    if ext in {".txt", ".md", ".csv", ".json", ".log", ".xml", ".html"} or \
            rec.get("modality_py") == "text" and ext not in {".pdf"}:
        try:
            s = data.decode("utf-8")
            rc = rust_canonicalize(s)
            dump(path.name + ".canon", rc.encode())
            rec["rust_canon"] = {"sha256": sha(rc.encode()),
                                 "words": len(rc.split()), "bytes": len(rc.encode())}
            rec["tier1_rust_equiv"] = rec["rust_canon"]["sha256"]
            pc = canon.canonical_text(s)
            rec["py_canon"] = {"sha256": sha(pc.encode()),
                               "words": len(canon.text_tokens(s))}
            rec["minhash_jaccard_sig"] = rec.get("minhash_sig_head")
        except Exception as e:
            rec["text_error"] = f"{type(e).__name__}: {e}"

    return rec


def main() -> int:
    corpus = Path(_ARGS[0]) if _ARGS else Path("corpus")
    out = sys.stdout
    for p in sorted(corpus.rglob("*")):
        if not p.is_file() or p.name in {"gen.py", "manifest.tsv", "pairs.tsv"}:
            continue
        try:
            rec = dump_file(p)
        except Exception as e:                      # never lose a row
            rec = {"file": p.name, "fatal": f"{type(e).__name__}: {e}"}
        out.write(json.dumps(rec) + "\n")
        out.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())

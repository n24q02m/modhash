#!/usr/bin/env python3

"""Fetch the corpora named in `lab/datasets/manifest.toml` into a cache
directory OUTSIDE the repository and verify their SHA-256 digests.

Stdlib only, Python >= 3.11 (tomllib). Usage:

    python lab/fetch_datasets.py [--list] [--cache DIR] [name ...]

- `--list` prints the manifest and exits.
- `--cache DIR` chooses the destination; default
  `$MODHASH_LAB_CACHE` or `%LOCALAPPDATA%/modhash-lab` /
  `~/.cache/modhash-lab`.
- With no names, every dataset is fetched; names select a subset.

Verification is unconditional: an entry whose digest mismatches is
deleted and reported, never kept. An entry with an empty `sha256` is
UNPINNED — the fetcher downloads it, prints the observed digest, and
exits nonzero until the digest is committed to the manifest. A wrong
hash is corrupted evaluation data; an unpinned hash is honestly "not
measured yet".
"""

import argparse
import hashlib
import os
import sys
import tempfile
import tomllib
import urllib.request
from pathlib import Path

MANIFEST = Path(__file__).resolve().parent / "datasets" / "manifest.toml"
UA = {"User-Agent": "modhash-lab-fetch/1.0"}


def default_cache() -> Path:
    env = os.environ.get("MODHASH_LAB_CACHE")
    if env:
        return Path(env)
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA") or (Path.home() / "AppData/Local")
        return Path(base) / "modhash-lab"
    base = os.environ.get("XDG_CACHE_HOME") or (Path.home() / ".cache")
    return Path(base) / "modhash-lab"


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fetch(ds: dict, cache: Path) -> int:
    """Returns 0 installed, 1 failure, 2 unpinned."""
    name = ds["name"]
    dest = cache / f"{name}{Path(ds['url'].split('?')[0]).suffix or '.bin'}"
    want = ds.get("sha256", "")
    tmp = cache / f".{name}.partial"
    try:
        req = urllib.request.Request(ds["url"], headers=UA)
        with urllib.request.urlopen(req, timeout=120) as r, tmp.open("wb") as f:
            for block in iter(lambda: r.read(1 << 20), b""):
                f.write(block)
    except Exception as e:  # noqa: BLE001 - report any transport failure
        print(f"{name}: fetch failed: {e}", file=sys.stderr)
        tmp.unlink(missing_ok=True)
        return 1
    got = sha256_of(tmp)
    if not want:
        print(f"{name}: UNPINNED — observed sha256 {got}", file=sys.stderr)
        print(f"{name}: commit this digest to the manifest to install", file=sys.stderr)
        tmp.unlink(missing_ok=True)
        return 2
    if got != want:
        print(f"{name}: DIGEST MISMATCH {got} != {want} — deleted", file=sys.stderr)
        tmp.unlink(missing_ok=True)
        return 1
    tmp.replace(dest)
    print(f"{name}: ok {dest} ({got[:16]}...)")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("names", nargs="*", help="dataset names (default: all)")
    ap.add_argument("--list", action="store_true", help="print manifest and exit")
    ap.add_argument("--cache", type=Path, default=default_cache(), help="cache dir")
    args = ap.parse_args()

    datasets = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))["dataset"]
    if args.list:
        for d in datasets:
            pin = "pinned" if d.get("sha256") else "UNPINNED"
            print(f"{d['name']:<24} {d['modality']:<8} {pin:<8} {d['license']}")
            print(f"  {d['url']}")
        return 0

    if args.names:
        known = {d["name"] for d in datasets}
        missing = set(args.names) - known
        if missing:
            print(f"unknown dataset(s): {', '.join(sorted(missing))}", file=sys.stderr)
            return 2
        datasets = [d for d in datasets if d["name"] in set(args.names)]

    args.cache.mkdir(parents=True, exist_ok=True)
    worst = 0
    for d in datasets:
        # 0 installed, 1 fetch/digest failure, 2 unpinned-but-downloaded.
        worst = max(worst, fetch(d, args.cache))
    return worst


if __name__ == "__main__":
    sys.exit(main())

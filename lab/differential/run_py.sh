#!/usr/bin/env bash
# Python side of the differential run.
#
#   ARENA_WS=/path/to/arena_ws lab/differential/run_py.sh
#
# Requires: uv. Runs inside the hashkit 0.5.0 environment
# (`uv run --project $ARENA_WS`; `uv sync --all-extras` once before).
# Writes:
#   results/py_dump.jsonl     per-file decoded digests + fingerprints
#   results/py_match.jsonl    per-pair match scalars
#   results/provenance.json   env + arena_ws content fingerprint
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
: "${ARENA_WS:?ARENA_WS must point at the hashkit 0.5.0 workspace}"
CORPUS="$HERE/corpus"
OUT="$HERE/results"
mkdir -p "$OUT"

uv run --project "$ARENA_WS" python "$HERE/py_dump.py" "$CORPUS" \
    "$OUT/dumps_py" > "$OUT/py_dump.jsonl"
uv run --project "$ARENA_WS" python "$HERE/py_match.py" "$CORPUS" \
    > "$OUT/py_match.jsonl"

# Provenance: arena_ws is not a git checkout, so pin the source tree
# byte-wise — SHA-256 over the sorted (relpath, file-sha256) list of
# src/hashkit + pyproject + uv.lock.
python - "$ARENA_WS" "$OUT/provenance.json" <<'PYEOF'
import hashlib, json, sys
from pathlib import Path
arena = Path(sys.argv[1])
rows = []
for p in sorted((arena / "src" / "hashkit").glob("*.py")):
    rows.append([p.name, hashlib.sha256(p.read_bytes()).hexdigest()])
for extra in ("pyproject.toml", "uv.lock", ".python-version"):
    f = arena / extra
    if f.exists():
        rows.append([extra, hashlib.sha256(f.read_bytes()).hexdigest()])
tree = hashlib.sha256(
    json.dumps(rows).encode()).hexdigest()
out = {
    "arena_ws": str(arena),
    "arena_is_git_repo": (arena / ".git").exists(),
    "arena_content_sha256": tree,
    "arena_files": rows,
}
import subprocess, datetime
def cap(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True,
                              timeout=30).stdout.strip().splitlines()[0]
    except Exception:
        return "?"
out["versions"] = {
    "uv": cap(["uv", "--version"]),
    "rustc": cap(["rustc", "--version"]),
    "ffmpeg": cap(["ffmpeg", "-version"]),
    "run_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
Path(sys.argv[2]).write_text(json.dumps(out, indent=2))
print(f"provenance: arena_content_sha256={tree[:16]}…")
PYEOF
echo "py side done -> $OUT" >&2

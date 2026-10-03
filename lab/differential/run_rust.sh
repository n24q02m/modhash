#!/usr/bin/env bash
# Rust side of the differential run.
#
#   lab/differential/run_rust.sh
#
# Requires: cargo, the corpus built by build_corpus.sh. Writes
#   results/rust_cli.jsonl   `modhash describe --json` (or stderr line)
#   results/rust_hash.jsonl  `modhash hash --json`
#   results/rust_probe.jsonl diff-probe decoded intermediates
#   results/rust_match.jsonl `modhash match --json` per pairs.tsv row
#   results/dumps/           raw decoded planes/PCM/text/frames
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
CORPUS="$HERE/corpus"
OUT="$HERE/results"
mkdir -p "$OUT/dumps"

cd "$ROOT"
cargo build --release -p modhash-cli >/dev/null 2>&1
cargo build --release --manifest-path "$HERE/probe/Cargo.toml" >/dev/null 2>&1

CLI=(cargo run --release -q -p modhash-cli --)
PROBE=(cargo run --release -q --manifest-path "$HERE/probe/Cargo.toml" --)

: > "$OUT/rust_cli.jsonl"
: > "$OUT/rust_hash.jsonl"
: > "$OUT/rust_probe.jsonl"
: > "$OUT/rust_match.jsonl"

while IFS= read -r f; do
  rel="${f#"$CORPUS/"}"
  case "$(basename "$f")" in
    gen.py|manifest.tsv|pairs.tsv) continue ;;
  esac
  echo "rust: $rel" >&2
  { "${CLI[@]}" describe "$f" --json 2>/dev/null \
      || printf '{"file": "%s", "cli_error": true, "stderr": "%s"}\n' \
           "$rel" "$("${CLI[@]}" describe "$f" 2>&1 | tr '"' "'")"
  } >> "$OUT/rust_cli.jsonl"
  { "${CLI[@]}" hash "$f" --json 2>/dev/null \
      || printf '{"file": "%s", "cli_error": true}\n' "$rel"
  } >> "$OUT/rust_hash.jsonl"
  "${PROBE[@]}" "$f" --dump-dir "$OUT/dumps" >> "$OUT/rust_probe.jsonl"
done < <(find "$CORPUS" -type f | sort)

# pair matching
tail -n +2 "$CORPUS/pairs.tsv" | while IFS=$'\t' read -r a b note; do
  [ -z "$a" ] && continue
  "${CLI[@]}" match "$CORPUS/$a" "$CORPUS/$b" --json 2>/dev/null \
    || printf '{"a": "%s", "b": "%s", "cli_error": true, "note": "%s"}\n' \
         "$a" "$b" "$note"
done >> "$OUT/rust_match.jsonl"

echo "rust side done -> $OUT" >&2

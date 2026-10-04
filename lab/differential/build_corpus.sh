#!/usr/bin/env bash
# Build the differential corpus deterministically.
#
#   lab/differential/build_corpus.sh
#
# Copies committed fixtures + generates synthetic inputs via gen.py and
# writes corpus/manifest.tsv (sha256 of every file). ARENA_WS may point
# at the hashkit 0.5.0 workspace to add the "real files" slice; files
# whose source is absent are recorded as missing-skip, not faked.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
: "${ARENA_WS:=arena_ws}"
export ARENA_WS
python "$HERE/corpus/gen.py"

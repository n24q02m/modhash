#!/usr/bin/env bash
# lab/smoke_all_formats.sh — end-to-end CLI run for every format in the
# design spec §4.5 codec table whose lane has landed on main.
#
#   bash lab/smoke_all_formats.sh [path-to-modhash-binary]
#
# Default binary is target/release/modhash(.exe); it is built with
# `cargo build --release -p modhash-cli` when missing, so a bare run is
# enough after a clone.
#
# A format whose crate has NOT landed (today: mp4 video — the facade's
# video lane is pending on modhash-h264/modhash-video) is printed as
# `[SKIP pending: <fmt>]`, never faked as a pass. Any other failure is a
# real failure and exits nonzero.
#
# Everything the script hashes lives in `lab/smoke-fixtures/` — committed
# bytes, no downloads, no generation at run time.

set -u
set -o pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2

BIN="${1:-}"
if [ -z "$BIN" ]; then
    BIN="target/release/modhash"
    [ -x "$BIN.exe" ] && BIN="$BIN.exe"
    if [ ! -x "$BIN" ]; then
        echo "# building modhash (release)…"
        cargo build --release -p modhash-cli || exit 2
        [ -x "$BIN.exe" ] && BIN="$BIN.exe"
    fi
fi
[ -x "$BIN" ] || { echo "smoke: $BIN missing and could not be built"; exit 2; }
echo "# binary: $BIN ($("$BIN" version))"

FX="lab/smoke-fixtures"
fail=0

# ok_row <fmt> <file> <expected modality> [expected format]
# Runs `describe` and `hash`; asserts exit 0, the three tier lines, and
# the modality (and format when given) the detection table promises.
ok_row() {
    local fmt="$1" file="$2" modality="$3" format="${4:-}"
    local desc hash rc
    desc="$("$BIN" describe "$FX/$file" 2>&1)"; rc=$?
    if [ $rc -ne 0 ]; then
        echo "[FAIL] $fmt ($file): describe exited $rc"; echo "$desc" | sed 's/^/       /'
        fail=1; return
    fi
    for t in "modality: $modality" "tier1:" "tier2:" "tier3:"; do
        if ! grep -qF "$t" <<<"$desc"; then
            echo "[FAIL] $fmt ($file): describe missing \`$t\`"; echo "$desc" | sed 's/^/       /'
            fail=1; return
        fi
    done
    if [ -n "$format" ] && ! grep -qF "format: $format" <<<"$desc"; then
        echo "[FAIL] $fmt ($file): expected format $format"; echo "$desc" | sed 's/^/       /'
        fail=1; return
    fi
    hash="$("$BIN" hash "$FX/$file" 2>&1)"; rc=$?
    if [ $rc -ne 0 ]; then
        echo "[FAIL] $fmt ($file): hash exited $rc"; echo "$hash" | sed 's/^/       /'
        fail=1; return
    fi
    if ! grep -qE '^[0-9a-f]{64}$' <<<"$hash"; then
        echo "[FAIL] $fmt ($file): hash did not print a 64-hex digest"; echo "$hash" | sed 's/^/       /'
        fail=1; return
    fi
    echo "[ ok ] $fmt ($file) — $modality/$format, tier1 ${hash:0:16}…"
}

# pending_row <fmt> <file> <modality>
# A detected-but-unlanded lane: describe must report it honestly
# (pending:true + the lane named) and hash must refuse with exit 1.
pending_row() {
    local fmt="$1" file="$2" modality="$3"
    local desc rc
    desc="$("$BIN" describe "$FX/$file" 2>&1)"; rc=$?
    if [ $rc -eq 0 ] && grep -qF "pending: true" <<<"$desc" && grep -qF "modality: $modality" <<<"$desc"; then
        echo "[SKIP pending: $fmt] — describe reports pending:$modality ($file)"
    else
        echo "[FAIL] $fmt ($file): expected pending:true on describe, got rc=$rc"
        echo "$desc" | sed 's/^/       /'
        fail=1
        return
    fi
    if "$BIN" hash "$FX/$file" >/dev/null 2>&1; then
        echo "[FAIL] $fmt ($file): hash must refuse a pending lane"
        fail=1; return
    fi
    echo "       hash refuses the pending lane (exit nonzero, lane named)"
}

echo "== formats in the §4.5 table =="

# §4.5 row          fixture                          modality  format
ok_row png          phash_rgb8_48x40.png             image     png
ok_row bmp          p24_top_down.bmp                 image     bmp
ok_row jpeg         base_444.jpg                     image     jpeg
ok_row wav          tone.wav                         audio     wav
ok_row flac         tone.flac                        audio     flac
ok_row mp3          l3_short.mp3                     audio     mp3
# mp4/h264: the demux crate landed but the video modality lane
# (modhash-video on modhash-h264) has not — honest pending, not a pass.
pending_row mp4     minimal.mp4                     video
ok_row zip          minimal.zip                      binary    zip
ok_row pdf          text_page.pdf                    text      pdf

echo "== non-container rows (text and binary by content) =="
ok_row text         prose.txt                        text
ok_row binary       blob.bin                         binary

echo "== cross-format sanity =="
# Same pixels in two containers share the tier-1 digest — the §2
# "normalized content" contract, checked end-to-end through the CLI.
a="$("$BIN" hash "$FX/base_444.png")"
b="$("$BIN" hash "$FX/base_444.jpg")"
if [ "$a" = "$b" ]; then
    echo "[ ok ] jpeg/png of identical pixels share tier1: ${a:0:16}…"
else
    echo "[FAIL] jpeg/png tier-1 mismatch: $a vs $b"; fail=1
fi

# match: a file against itself must match (exit 0); two distinct images
# must not (exit 1 — grep convention).
if "$BIN" match "$FX/phash_rgb8_48x40.png" "$FX/phash_rgb8_48x40.png" >/dev/null 2>&1; then
    echo "[ ok ] match self: exit 0"
else
    echo "[FAIL] match self did not exit 0"; fail=1
fi
if "$BIN" match "$FX/phash_rgb8_48x40.png" "$FX/base_444.png" >/dev/null 2>&1; then
    echo "[FAIL] match distinct images exited 0"; fail=1
else
    echo "[ ok ] match distinct images: exit 1 (no-match)"
fi

# dedup: a scratch dir with one known duplicate pair must cluster it.
D="$(mktemp -d "${TMPDIR:-/tmp}/modhash-smoke.XXXXXX")"
trap 'rm -rf "$D"' EXIT
cp "$FX/phash_rgb8_48x40.png" "$D/dup_a.png"
cp "$FX/phash_rgb8_48x40.png" "$D/dup_b.png"
cp "$FX/base_444.png" "$D/other.png"
dedup_out="$("$BIN" dedup "$D" 2>&1)"; rc=$?
if [ $rc -eq 0 ] && grep -qF "1 group(s)" <<<"$dedup_out" \
    && grep -qF "dup_a.png" <<<"$dedup_out" && grep -qF "dup_b.png" <<<"$dedup_out"; then
    echo "[ ok ] dedup clusters the known pair"
else
    echo "[FAIL] dedup output unexpected (rc=$rc)"; echo "$dedup_out" | sed 's/^/       /'; fail=1
fi

# bench runs on the release binary it was asked to measure — one
# iteration is the smoke, the lab bench is the measurement.
if "$BIN" bench --iters 1 >/dev/null 2>&1; then
    echo "[ ok ] bench"
else
    echo "[FAIL] bench --iters 1"; fail=1
fi

if [ $fail -eq 0 ]; then
    echo "smoke: PASS — every §4.5 format with a landed lane ran end-to-end"
else
    echo "smoke: FAIL"
fi
exit $fail

#!/usr/bin/env bash
# Publish the modhash crates to crates.io in leaf-to-root order.
#
# Order is the design spec §3 table. A crate that already has a release on the
# registry is skipped, so re-running after a partial failure resumes cleanly.
# The FIRST failure stops the run: shipping a half-published dependency chain
# is worse than shipping nothing.
#
# Usage: scripts/publish.sh [--dry-run]

set -euo pipefail
cd "$(dirname "$0")/.."

DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

# Leaf-to-root: the exact reverse of the spec §3 numbering.
ORDER=(
  modhash-primitives modhash-inflate modhash-unicode modhash-math
  modhash-raster modhash-png modhash-bmp modhash-jpeg modhash-text
  modhash-fastcdc modhash-wav modhash-flac modhash-mp3 modhash-audio
  modhash-mp4 modhash-h264 modhash-video modhash-zip modhash-pdf
  modhash-tier3 modhash-index modhash modhash-cli
)

probe_registry() {
  # 200 => the crate name is already taken on crates.io.
  curl -sS -o /dev/null -w '%{http_code}' \
    -H 'User-Agent: modhash-publish' \
    "https://crates.io/api/v1/crates/$1"
}

for crate in "${ORDER[@]}"; do
  version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$crate/Cargo.toml" | head -1)
  status=$(probe_registry "$crate")

  if [[ "$status" == "200" ]]; then
    echo "SKIP  $crate $version (name already on crates.io)"
    continue
  fi

  if [[ "$DRY_RUN" == "1" ]]; then
    echo "DRY   $crate $version (registry status $status) would publish"
    continue
  fi

  echo "PUBLISH $crate $version"
  if ! cargo publish -p "$crate"; then
    echo "FAILED at $crate $version - stopping, nothing after this point is published" >&2
    exit 1
  fi
done

echo "publish run complete"

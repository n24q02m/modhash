#!/usr/bin/env python3
"""Enforce the modhash workspace crate ordering (design spec §3).

The workspace rule is absolute: a crate may only depend on a crate with a
LOWER order number. That single rule subsumes "no dependency cycles", so the
gate checks edges, not graph shape.

The order table below is the table from the design spec, transcribed once. A
crate missing from it is a gate failure, not a silently accepted crate.
The gate also checks COMPLETENESS. The ordering rule alone cannot catch a
missing edge: an empty dependency list trivially satisfies "every edge points
down", so a crate that was supposed to build on modhash-primitives shipped
with no dependency at all and the gate stayed green. EXPECTED_EDGES below is
the declared design graph, and the actual graph must match it exactly.

Amending EXPECTED_EDGES is allowed but must be deliberate: an implementer who
does not need a declared edge removes it in the same change and says why in
the pull request, so the table stays the design record rather than a wish.

Exit codes: 0 = clean, 1 = rule violated or table drift, other = cargo failed.
"""

import json
import subprocess
import sys

# order -> crate name, straight from the design spec §3 table.
ORDER = {
    "modhash-primitives": 0,
    "modhash-inflate": 1,
    "modhash-unicode": 2,
    "modhash-math": 3,
    "modhash-raster": 4,
    "modhash-png": 5,
    "modhash-bmp": 6,
    "modhash-jpeg": 7,
    "modhash-text": 8,
    "modhash-fastcdc": 9,
    "modhash-wav": 10,
    "modhash-flac": 11,
    "modhash-mp3": 12,
    "modhash-audio": 13,
    "modhash-mp4": 14,
    "modhash-h264": 15,
    "modhash-video": 16,
    "modhash-zip": 17,
    "modhash-pdf": 18,
    "modhash-tier3": 19,
    "modhash-index": 20,
    "modhash": 21,
    "modhash-cli": 22,
}

# The declared design graph, from the spec §3 table plus the shared-vocabulary
# edges it implies: modhash-primitives exists to be used by the whole kit, and
# modhash-cli is the command front end for the kit crate.
EXPECTED_EDGES = {
    "modhash-primitives": [],
    "modhash-inflate": ["modhash-primitives"],
    "modhash-unicode": ["modhash-primitives"],
    "modhash-math": ["modhash-primitives"],
    "modhash-raster": ["modhash-primitives"],
    "modhash-png": ["modhash-inflate", "modhash-primitives", "modhash-raster"],
    "modhash-bmp": ["modhash-primitives", "modhash-raster"],
    "modhash-jpeg": ["modhash-math", "modhash-primitives", "modhash-raster"],
    "modhash-text": ["modhash-primitives", "modhash-unicode"],
    "modhash-fastcdc": ["modhash-primitives"],
    "modhash-wav": ["modhash-primitives"],
    "modhash-flac": ["modhash-primitives"],
    "modhash-mp3": ["modhash-primitives"],
    "modhash-audio": ["modhash-math", "modhash-wav", "modhash-flac"],
    "modhash-mp4": ["modhash-primitives"],
    "modhash-h264": ["modhash-primitives", "modhash-math", "modhash-raster"],
    "modhash-video": ["modhash-primitives", "modhash-raster", "modhash-text", "modhash-mp4", "modhash-h264"],
    "modhash-zip": ["modhash-primitives", "modhash-inflate"],
    "modhash-pdf": ["modhash-primitives", "modhash-inflate", "modhash-text"],
    "modhash-tier3": ["modhash-math", "modhash-raster"],
    "modhash-index": ["modhash-primitives"],
    "modhash": [
        "modhash-audio", "modhash-bmp", "modhash-fastcdc", "modhash-flac",
        "modhash-h264", "modhash-index", "modhash-inflate", "modhash-jpeg",
        "modhash-math", "modhash-mp3", "modhash-mp4", "modhash-pdf",
        "modhash-png", "modhash-primitives", "modhash-raster",
        "modhash-text", "modhash-tier3", "modhash-unicode", "modhash-video",
        "modhash-wav", "modhash-zip",
    ],
    "modhash-cli": ["modhash"],
}


def load_metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--offline"],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(out.stdout)


def main():
    meta = load_metadata()
    failures = []

    members = {p["name"] for p in meta["packages"] if p["id"] in meta["workspace_members"]}
    if members != set(ORDER):
        for extra in sorted(members - set(ORDER)):
            failures.append(f"crate not in the spec order table: {extra}")
        for missing in sorted(set(ORDER) - members):
            failures.append(f"crate in the order table but absent from the workspace: {missing}")

    for pkg in meta["packages"]:
        name = pkg["name"]
        if name not in ORDER:
            continue
        actual = {d["name"] for d in pkg["dependencies"]}
        declared = set(EXPECTED_EDGES.get(name, []))

        for missing in sorted(declared - actual):
            failures.append(
                f"{name} (#{ORDER[name]}) is missing a declared dependency on {missing}; "
                f"the crate table says it builds on it"
            )
        for extra in sorted(actual - declared):
            failures.append(
                f"{name} (#{ORDER[name]}) declares {extra}, which the crate table does not list"
            )

        for dep in sorted(actual):
            if dep not in ORDER:
                failures.append(f"{name} ({ORDER[name]}) depends on non-workspace crate {dep}")
            elif dep == name:
                failures.append(f"{name} ({ORDER[name]}) depends on itself")
            elif ORDER[dep] >= ORDER[name]:
                failures.append(
                    f"{name} (#{ORDER[name]}) depends on {dep} (#{ORDER[dep]}); "
                    f"a crate may only depend on a LOWER-numbered crate"
                )

    if failures:
        print("DAG gate FAILED:")
        for f in failures:
            print(f"  - {f}")
        return 1

    print(
        f"DAG gate OK: {len(members)} crates, "
        f"{sum(len(v) for v in EXPECTED_EDGES.values())} declared edges all present "
        f"and all pointing to a lower order number"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Enforce the modhash workspace crate ordering (design spec §3).

The workspace rule is absolute: a crate may only depend on a crate with a
LOWER order number. That single rule subsumes "no dependency cycles", so the
gate checks edges, not graph shape.

The order table below is the table from the design spec, transcribed once. A
crate missing from it is a gate failure, not a silently accepted crate.

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
        for dep in pkg["dependencies"]:
            if dep["name"] not in ORDER:
                failures.append(f"{name} ({ORDER[name]}) depends on non-workspace crate {dep['name']}")
                continue
            if dep["name"] == name:
                failures.append(f"{name} ({ORDER[name]}) depends on itself")
            elif ORDER[dep["name"]] >= ORDER[name]:
                failures.append(
                    f"{name} (#{ORDER[name]}) depends on {dep['name']} (#{ORDER[dep['name']]}); "
                    f"a crate may only depend on a LOWER-numbered crate"
                )

    if failures:
        print("DAG gate FAILED:")
        for f in failures:
            print(f"  - {f}")
        return 1

    print(f"DAG gate OK: {len(members)} crates, all edges point to a lower order number")
    return 0


if __name__ == "__main__":
    sys.exit(main())

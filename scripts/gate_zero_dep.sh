#!/usr/bin/env bash
# Zero-dependency gate for the modhash workspace.
#
# The body below is the command published in the design spec (§2) and is kept
# byte-identical on purpose: the gate must be the thing the spec measured, not
# a re-implementation of it. Its discriminating power was proven by adding one
# `itoa` dev-dependency to a two-crate workspace (exit 1, `registry crates:
# ['itoa']`) against the same command run on an internal-only workspace
# (exit 0, `registry crates: []`).
#
# Exit codes: 0 = every resolved package is a workspace member, 1 = at least one
# registry package leaked in, other = cargo/python failed.

set -euo pipefail

cd "$(dirname "$0")/.."

cargo metadata --format-version 1 --offline \
 | python -c "import json,sys;r=json.load(sys.stdin);\
ext=sorted({p['name'] for p in r['packages'] if p.get('source')});\
print('workspace crates:',len(r['workspace_members']),'| registry crates:',ext);\
sys.exit(1 if ext else 0)"

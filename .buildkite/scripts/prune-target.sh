#!/usr/bin/env bash
# Trims target/ down to third-party dependency artifacts before it's saved to
# the Buildkite cache (the same idea as Swatinem/rust-cache's cleanup):
#
#   * workspace crates always rebuild anyway (a fresh checkout gives every
#     source file a new mtime), so their artifacts are dead weight;
#   * incremental/ is unused (CARGO_INCREMENTAL=0) and doc/, package/ and
#     nextest/ are per-run output;
#   * final binaries at the top of each profile dir are rebuilt every run.
#
# Keeps each cache entry to roughly the size of the dependency build.
set -euo pipefail

target=${CARGO_TARGET_DIR:-target}
[ -d "$target" ] || exit 0

before=$(du -sh "$target" | cut -f1)

# Matching is done in Python (already needed to read cargo metadata) so it's
# exact and identical on Linux and macOS: an artifact belongs to the
# workspace only if its name is `<workspace name>-<16 hex hash>`, so e.g. the
# crates.io crate atuin-vt100 survives even though `atuin` is a workspace
# crate.
cargo metadata --no-deps --format-version 1 --offline | python3 -c '
import json, os, re, shutil, sys

target = sys.argv[1]
meta = json.load(sys.stdin)
names = set()
for pkg in meta["packages"]:
    names |= {pkg["name"], pkg["name"].replace("-", "_")}
    names |= {t["name"].replace("-", "_") for t in pkg["targets"]}

artifact = re.compile(r"^(?:lib)?(?P<name>.+?)-[0-9a-f]{16}(?:\..*)?$")

def remove(path):
    if os.path.isdir(path) and not os.path.islink(path):
        shutil.rmtree(path)
    else:
        os.remove(path)

for per_run in ("doc", "package", "nextest", "tmp"):
    path = os.path.join(target, per_run)
    if os.path.exists(path):
        remove(path)

for profile in os.listdir(target):
    profile_dir = os.path.join(target, profile)
    if not os.path.isdir(os.path.join(profile_dir, "deps")):
        continue
    for per_run in ("incremental", "examples"):
        path = os.path.join(profile_dir, per_run)
        if os.path.exists(path):
            remove(path)
    # Top-level outputs (binaries, .d files) are copies of deps/ artifacts.
    for entry in os.listdir(profile_dir):
        path = os.path.join(profile_dir, entry)
        if os.path.isfile(path) or os.path.islink(path):
            os.remove(path)
    for sub in ("deps", ".fingerprint", "build"):
        sub_dir = os.path.join(profile_dir, sub)
        if not os.path.isdir(sub_dir):
            continue
        for entry in os.listdir(sub_dir):
            match = artifact.match(entry)
            if match and match.group("name") in names:
                remove(os.path.join(sub_dir, entry))
' "$target"

echo "Pruned $target: $before -> $(du -sh "$target" | cut -f1)"

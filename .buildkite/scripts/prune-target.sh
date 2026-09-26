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

# Workspace package names (for .fingerprint/ and build/ dirs, which use the
# package name) and target names with - -> _ (for deps/ artifacts).
names=$(cargo metadata --no-deps --format-version 1 --offline | python3 -c '
import json, sys
meta = json.load(sys.stdin)
names = set()
for pkg in meta["packages"]:
    names.add(pkg["name"])
    names.add(pkg["name"].replace("-", "_"))
    for tgt in pkg["targets"]:
        names.add(tgt["name"].replace("-", "_"))
print("\n".join(sorted(names)))
')

rm -rf "$target"/{doc,package,nextest,tmp}

for profile in "$target"/*/; do
  profile=${profile%/}
  [ -d "$profile/deps" ] || continue
  rm -rf "$profile/incremental" "$profile/examples"
  # Top-level outputs (binaries, .d files) are copies of deps/ artifacts.
  find "$profile" -maxdepth 1 -type f -delete
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    rm -rf "$profile/.fingerprint/$name"-* "$profile/build/$name"-* \
      "$profile/deps/$name"-* "$profile/deps/lib$name"-*
  done <<<"$names"
done

echo "Pruned $target: $before -> $(du -sh "$target" | cut -f1)"

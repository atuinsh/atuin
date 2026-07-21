"""Prune logic for mike-versioned docs prereleases.

The docs site publishes stable releases under their `major.minor` id (kept
permanently) and every prerelease (e.g. `18.18.0-beta.1`) under its full
version id, titled "(pre-release)". Prerelease previews are transient: they are
removed once a version with an equal-or-greater base version appears, so only
the newest upcoming preview is ever shown.

`base_version` is the `(major, minor, patch)` of a version, ignoring any
`-beta.N`/`-rc.N` prerelease suffix. The prune rule is: delete a prerelease `P`
when `base(P) <= boundary`, where `boundary` is the base of either
  - a just-released stable version (corresponding-or-later release), or
  - a just-deployed newer prerelease (supersedes older previews).

CLI (reads `mike list --json` on stdin, prints ids to delete, one per line):

    mike list --json | python prerelease_docs.py <boundary> [--exclude <id>]
"""

import argparse
import json
import sys


def base_version(version: str) -> tuple[int, int, int]:
    """Return (major, minor, patch) for a version, ignoring any prerelease suffix.

    Raises ValueError if the numeric core is not a dotted integer version.
    """
    core = version.lstrip("v").split("-", 1)[0].split("+", 1)[0]
    parts = core.split(".")
    if not (1 <= len(parts) <= 3):
        raise ValueError(f"not a version: {version!r}")
    nums = [int(p) for p in parts]  # raises ValueError on non-numeric parts
    while len(nums) < 3:
        nums.append(0)
    return (nums[0], nums[1], nums[2])


def is_prerelease(version: str) -> bool:
    """True if the version carries a prerelease suffix (e.g. `18.18.0-beta.1`)."""
    return "-" in version.lstrip("v")


def prereleases_to_delete(versions, boundary: str, exclude: str | None = None):
    """Ids of prerelease versions superseded by `boundary`.

    A prerelease is superseded when its base version is <= base(boundary). The
    `exclude` id (e.g. the prerelease just deployed) is never returned.
    Non-prerelease ids (stable minors, `main`) and unparsable ids are ignored.
    """
    boundary_base = base_version(boundary)
    doomed = []
    for entry in versions:
        vid = entry["version"] if isinstance(entry, dict) else entry
        if vid == exclude or not is_prerelease(vid):
            continue
        try:
            if base_version(vid) <= boundary_base:
                doomed.append(vid)
        except ValueError:
            continue  # stray non-semver id -> leave it alone
    return doomed


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("boundary", help="version whose base sets the prune cutoff")
    parser.add_argument("--exclude", default=None, help="id to never delete")
    args = parser.parse_args(argv)

    raw = sys.stdin.read().strip()
    versions = json.loads(raw) if raw else []
    for vid in prereleases_to_delete(versions, args.boundary, args.exclude):
        print(vid)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

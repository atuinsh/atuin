"""
Given `mike list --json` on stdin and a boundary version, print the mike
version ids whose docs should be pruned (one per line).

- Docs are published `major.minor`, for each stable release, and kept forever.
- Only the latest pre-release docs are kept: if a `.beta.1` exists and a
  `.beta.2` is deployed, the old `.beta.1` is pruned.
- When a stable release (or any later version) lands, every pre-release with a
  base version <= it is pruned.

Usage:
    mike list --json | python filter_dead_vers.py <boundary> [--exclude <id>]
"""

import argparse
import json
import sys
from collections.abc import Iterable, Mapping

from common import Version


def versions_to_prune(
    entries: Iterable[Mapping[str, object]],
    boundary: str,
    exclude: str | None = None,
) -> list[str]:
    """Return the prerelease ids (from `mike list --json` entries) superseded by `boundary`.

    A prerelease is superseded when its base version is <= the base of
    `boundary`. `exclude` (e.g. the id just deployed) is never returned. Entries
    that are stable releases, `main`, or otherwise unparsable are left alone.
    """
    boundary_base = Version.from_str(boundary).base

    doomed: list[str] = []
    for entry in entries:
        version_id = str(entry["version"])
        if version_id == exclude:
            continue
        try:
            version = Version.from_mike(entry)
        except ValueError:
            continue  # not a version we manage (e.g. "main") -> never prune
        if version.is_prerelease and version.base <= boundary_base:
            doomed.append(version_id)
    return doomed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("boundary", help="version whose base sets the prune cutoff")
    parser.add_argument("--exclude", default=None, help="version id to never prune")
    args = parser.parse_args(argv)

    raw = sys.stdin.read().strip()
    entries = json.loads(raw) if raw else []
    for version_id in versions_to_prune(entries, args.boundary, args.exclude):
        print(version_id)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

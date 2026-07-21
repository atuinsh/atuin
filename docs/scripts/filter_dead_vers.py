"""
Given `mike list --json` on stdin and a boundary version, print the mike
version ids whose docs should be pruned (one per line).

- Docs are published `major.minor`, for each stable release, and kept forever.
- Only the latest pre-release docs are kept: if a `.beta.1` exists and a
  `.beta.2` is deployed, the old `.beta.1` is pruned.
- When a stable release (or any later version) lands, every pre-release with a
  base version <= it is pruned.

Usage:
    mike list --json | python filter_dead_vers.py <boundary>
"""

import argparse
import json
import sys
from collections.abc import Iterable, Mapping

from common import Version


def _parse_or_none(entry: Mapping[str, object]) -> Version | None:
    try:
        return Version.from_mike(entry)
    except ValueError:
        return None  # not a version we manage (e.g. "main") -> never prune


def versions_to_prune(
    entries: Iterable[Mapping[str, object]],
    boundary: str,
) -> list[str]:
    boundary_version = Version.from_str(boundary)

    def superseded(entry: Mapping[str, object]) -> bool:
        version = _parse_or_none(entry)
        return (
            version is not None
            and version.is_prerelease
            and version < boundary_version
        )

    return [str(entry["version"]) for entry in filter(superseded, entries)]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("boundary", help="version that sets the prune cutoff")
    args = parser.parse_args(argv)

    raw = sys.stdin.read().strip()
    entries = json.loads(raw) if raw else []
    sys.stdout.writelines(
        f"{version_id}\n"
        for version_id in versions_to_prune(entries, args.boundary)
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

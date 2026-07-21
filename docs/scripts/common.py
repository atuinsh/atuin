"""Version model shared by the docs-deploy helper scripts.

`BaseVersion` is the `(major, minor, patch)` triple; `Version` adds an optional
`-beta.N` prerelease number. Only `-beta.N` prereleases are recognised (that is
all Atuin ships); any other suffix fails to parse.
"""

from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from functools import total_ordering
from typing import Self
import re

# Stable docs are published under a `major.minor` id (patch omitted); releases
# and prereleases carry the full `major.minor.patch[-beta.N]`.
_version_re = re.compile(r"^v?(\d+)\.(\d+)(?:\.(\d+))?(?:-beta\.(\d+))?$")


@dataclass(frozen=True, order=True)
class BaseVersion:
    major: int
    minor: int
    patch: int

    def __iter__(self) -> Iterator[int]:
        return iter((self.major, self.minor, self.patch))


@total_ordering
@dataclass(frozen=True, eq=False)
class Version:
    base: BaseVersion
    # The `-beta.N` number, or None for a stable release.
    pre_release: int | None

    @classmethod
    def from_str(cls, as_str: str) -> Self:
        match = _version_re.match(as_str)
        if match is None:
            raise ValueError(f"{as_str!r} is not a supported version string")

        major, minor, patch, pre = match.groups()
        return cls(
            base=BaseVersion(int(major), int(minor), int(patch) if patch else 0),
            pre_release=int(pre) if pre is not None else None,
        )

    @classmethod
    def from_mike(cls, entry: Mapping[str, object]) -> Self:
        """Build a Version from a `mike list --json` entry (a `{"version": ...}` mapping)."""
        return cls.from_str(str(entry["version"]))

    @property
    def is_prerelease(self) -> bool:
        return self.pre_release is not None

    def _key(self) -> tuple[int, int, int, float]:
        # A stable release (None) sorts after every prerelease of the same base,
        # matching semver (`18.18.0` > `18.18.0-beta.9`).
        pre = self.pre_release if self.pre_release is not None else float("inf")
        return (self.base.major, self.base.minor, self.base.patch, pre)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Version):
            return NotImplemented
        return self._key() == other._key()

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, Version):
            return NotImplemented
        return self._key() < other._key()

    def __hash__(self) -> int:
        return hash(self._key())

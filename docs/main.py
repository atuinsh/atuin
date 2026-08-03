"""mkdocs-macros hooks exposing repo facts to the docs.

Keeps version numbers quoted in the docs in sync with their source of truth in
the repo, so they can't drift. Currently exposes `{{ msrv }}`, read from the
`channel` pinned in `rust-toolchain.toml`.
"""

import tomllib
from pathlib import Path

# The docs project lives in `docs/`; the repo root is its parent.
_REPO_ROOT = Path(__file__).resolve().parent.parent
_RUST_TOOLCHAIN = _REPO_ROOT / "rust-toolchain.toml"


def _read_msrv() -> str:
    with _RUST_TOOLCHAIN.open("rb") as f:
        return tomllib.load(f)["toolchain"]["channel"]


def define_env(env):
    env.variables["msrv"] = _read_msrv()

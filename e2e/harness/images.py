"""Build the docker images the tests run in.

The base image compiles `atuin` + `atuin-server` from the repo source (see
images/Dockerfile.base) so we never depend on host/target compatibility -- the
host is macOS, the containers are Linux. Per-shell images layer a shell + rc on
top of the base.

Images are built once and reused. Set ATUIN_E2E_REBUILD=1 to force a rebuild
(e.g. after changing atuin source).
"""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
IMAGES_DIR = Path(__file__).resolve().parents[1] / "images"

BASE_IMAGE = "atuin-e2e-base:local"
SHELLS = ("zsh", "bash", "fish", "nu")

_BUILD_ENV = {**os.environ, "DOCKER_BUILDKIT": "1"}


def _rebuild_requested() -> bool:
    return os.environ.get("ATUIN_E2E_REBUILD") == "1"


def image_exists(tag: str) -> bool:
    return (
        subprocess.run(
            ["docker", "image", "inspect", tag],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode
        == 0
    )


def _build(tag: str, dockerfile: Path, context: Path) -> None:
    subprocess.run(
        ["docker", "build", "-t", tag, "-f", str(dockerfile), "."],
        cwd=context,
        env=_BUILD_ENV,
        check=True,
    )


def ensure_base() -> str:
    """Build the base image if missing (or if a rebuild was requested)."""
    if _rebuild_requested() or not image_exists(BASE_IMAGE):
        _build(BASE_IMAGE, IMAGES_DIR / "Dockerfile.base", REPO_ROOT)
    return BASE_IMAGE


def shell_image(shell: str) -> str:
    return f"atuin-e2e-{shell}:local"


def ensure_shell(shell: str) -> str:
    """Build a per-shell image (and the base it depends on) if missing."""
    if shell not in SHELLS:
        raise ValueError(f"unknown shell {shell!r}; expected one of {SHELLS}")
    ensure_base()
    tag = shell_image(shell)
    if _rebuild_requested() or not image_exists(tag):
        _build(tag, IMAGES_DIR / f"Dockerfile.{shell}", IMAGES_DIR)
    return tag

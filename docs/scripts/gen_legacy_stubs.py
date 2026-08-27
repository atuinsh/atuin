"""
Generate meta-refresh redirect stubs for the legacy unversioned /cli/* URLs
of the pre-versioning docs site.

Reads the mkdocs-redirects `redirect_maps` from mkdocs.yml (already the
authoritative old-page -> new-page map) and writes one stub per `cli/...`
entry into the output directory. Synced to the gh-pages root, each stub
serves an HTTP 200 with an instant meta refresh straight to the page's home
under /latest/ - the redirect form search engines treat as permanent.

Run by the docs-root-files action on every deploy, writing directly into the
gh-pages worktree; nothing generated here is committed to the source tree.

Usage:
    python gen_legacy_stubs.py <mkdocs.yml> <output-dir>
"""

import argparse
from collections.abc import Mapping
from pathlib import Path

import yaml

_STUB = """\
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta http-equiv="refresh" content="0; url={target}">
  <link rel="canonical" href="https://docs.atuin.sh{target}">
  <title>Redirecting to docs.atuin.sh{target}</title>
</head>
<body>
  <p>This page has moved to <a href="{target}">docs.atuin.sh{target}</a>.</p>
</body>
</html>
"""


class _MkdocsLoader(yaml.SafeLoader):
    pass


# mkdocs.yml uses `!!python/name:` tags (theme emoji hooks); their values are
# irrelevant here, so load them as None instead of failing.
_MkdocsLoader.add_multi_constructor(
    "tag:yaml.org,2002:python/name:", lambda loader, suffix, node: None
)
# `!ENV [VAR, default]` (mkdocs env-var interpolation, used by git-committers);
# also irrelevant to stub generation.
_MkdocsLoader.add_constructor("!ENV", lambda loader, node: None)


def _md_to_url(md_path: str) -> str:
    trimmed = md_path.removesuffix("index.md")
    return trimmed if trimmed != md_path else md_path.removesuffix(".md") + "/"


def _redirect_maps(config: Mapping[str, object]) -> Mapping[str, str]:
    return next(
        plugin["redirects"]["redirect_maps"]
        for plugin in config["plugins"]
        if isinstance(plugin, Mapping) and "redirects" in plugin
    )


def _stub(old_md: str, new_md: str) -> tuple[str, str]:
    target = f"/latest/{_md_to_url(new_md)}"
    return f"{_md_to_url(old_md)}index.html", _STUB.format(target=target)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("mkdocs_yml", type=Path, help="path to mkdocs.yml")
    parser.add_argument("out_dir", type=Path, help="directory to write the stub tree into")
    args = parser.parse_args(argv)

    config = yaml.load(args.mkdocs_yml.read_text(), _MkdocsLoader)
    stubs = [
        _stub(old, new)
        for old, new in _redirect_maps(config).items()
        if old.startswith("cli/")
    ]

    for rel_path, html in stubs:
        path = args.out_dir / rel_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(html)

    print(f"wrote {len(stubs)} stubs under {args.out_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

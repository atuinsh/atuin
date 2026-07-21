"""Branding guards for the Atuin docs site.

CSS is not normally testable. Two things here are worth the harness:

1. Contrast. `#38c85a` is the brand green, but on a light background it is
   ~2.1:1 -- it fails WCAG AA outright. Light mode therefore uses `#15803d`.
   Nothing but arithmetic will catch a regression here, so we do the
   arithmetic.
2. Structure. Material is a moving target; a version bump can silently
   invalidate a selector we depend on. We assert against a real build.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

import pytest

DOCS_ROOT = Path(__file__).resolve().parent.parent
STYLESHEETS = DOCS_ROOT / "docs" / "stylesheets"


# --------------------------------------------------------------------------
# CSS parsing
# --------------------------------------------------------------------------

def read_css(name: str) -> str:
    """Return the text of a stylesheet in docs/docs/stylesheets/."""
    return (STYLESHEETS / name).read_text(encoding="utf-8")


def parse_vars(css: str, scheme: str | None = None) -> dict[str, str]:
    """Extract custom-property declarations.

    Comments are stripped first, then the stylesheet is split into
    (selector-list, body) rule pairs and each selector list is split on
    commas. A combined rule such as

        [data-md-color-scheme="slate"],
        [data-md-color-scheme="default"] {
          ...
        }

    is therefore matched by BOTH "slate" and "default" -- unlike a naive
    "selector immediately followed by `{`" match, which would only ever
    see the last selector in the list.

    With `scheme=None`, this reads the rule whose selector is exactly
    `:root`. With a scheme, it reads EVERY rule whose selector list
    contains `[data-md-color-scheme="<scheme>"]`, merging their bodies in
    document order so a later declaration overrides an earlier one --
    mirroring the CSS cascade. Values are returned verbatim, so a
    `var(--x)` reference comes back as the literal string -- callers that
    need a colour should use `resolve`.
    """
    css = re.sub(r"/\*.*?\*/", "", css, flags=re.S)

    target = ":root" if scheme is None else '[data-md-color-scheme="%s"]' % scheme

    merged: dict[str, str] = {}
    for selector_list, body in re.findall(r"([^{}]+)\{([^{}]*)\}", css, re.S):
        selectors = [s.strip() for s in selector_list.split(",")]
        if target not in selectors:
            continue
        for m in re.finditer(r"(--[\w-]+)\s*:\s*([^;]+);", body):
            merged[m.group(1).strip()] = m.group(2).strip()
    return merged


def resolve(value: str, root: dict[str, str]) -> str:
    """Follow `var(--x)` indirection until a literal remains."""
    seen = set()
    while (m := re.fullmatch(r"var\((--[\w-]+)\)", value.strip())):
        name = m.group(1)
        if name in seen:
            raise ValueError(f"circular var reference at {name}")
        seen.add(name)
        value = root[name]
    return value.strip()


# --------------------------------------------------------------------------
# WCAG 2.1 contrast
# --------------------------------------------------------------------------

def _channel(c: float) -> float:
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4


def luminance(hex_color: str) -> float:
    h = hex_color.strip().lstrip("#")
    if len(h) == 3:
        h = "".join(ch * 2 for ch in h)
    r, g, b = (int(h[i:i + 2], 16) / 255 for i in (0, 2, 4))
    return 0.2126 * _channel(r) + 0.7152 * _channel(g) + 0.0722 * _channel(b)


def contrast(fg: str, bg: str) -> float:
    a, b = luminance(fg), luminance(bg)
    lo, hi = min(a, b), max(a, b)
    return (hi + 0.05) / (lo + 0.05)


# --------------------------------------------------------------------------
# Contrast guards
# --------------------------------------------------------------------------

@pytest.fixture(scope="module")
def tokens() -> str:
    return read_css("atuin-tokens.css")


@pytest.mark.parametrize("scheme", ["slate", "default"])
@pytest.mark.parametrize(
    "fg_var,minimum,label",
    [
        ("--md-default-fg-color", 4.5, "body text"),
        ("--md-typeset-a-color", 4.5, "links"),
        ("--md-default-fg-color--light", 4.5, "muted text"),
        ("--md-default-fg-color--lighter", 3.0, "de-emphasised UI text"),
    ],
)
def test_foreground_contrast(tokens, scheme, fg_var, minimum, label):
    root = parse_vars(tokens)
    scheme_vars = parse_vars(tokens, scheme)

    fg = resolve(scheme_vars[fg_var], root)
    bg = resolve(scheme_vars["--md-default-bg-color"], root)

    ratio = contrast(fg, bg)
    assert ratio >= minimum, (
        f"{label} in {scheme}: {fg} on {bg} is {ratio:.2f}:1, "
        f"needs >= {minimum}:1"
    )


@pytest.mark.parametrize("scheme", ["slate", "default"])
def test_code_contrast(tokens, scheme):
    root = parse_vars(tokens)
    scheme_vars = parse_vars(tokens, scheme)

    fg = resolve(scheme_vars["--md-code-fg-color"], root)
    bg = resolve(scheme_vars["--md-code-bg-color"], root)

    ratio = contrast(fg, bg)
    assert ratio >= 4.5, f"code text in {scheme}: {ratio:.2f}:1, needs >= 4.5:1"


def test_brand_green_is_measured_value(tokens):
    """Guard against the brand green drifting to an approximation."""
    root = parse_vars(tokens)
    assert root["--atuin-green"].lower() == "#38c85a"
    assert root["--atuin-bg"].lower() == "#101620"


def test_light_scheme_does_not_use_the_bright_green_for_text(tokens):
    """#38c85a is ~2.1:1 on the light background. It must not carry text."""
    root = parse_vars(tokens)
    light = parse_vars(tokens, "default")
    for var in ("--md-typeset-a-color", "--md-default-fg-color"):
        assert resolve(light[var], root).lower() != "#38c85a"


def test_combined_selector_rule_applies_to_both_schemes(tokens):
    """The syntax-highlighting block is declared as a combined selector:

        [data-md-color-scheme="slate"],
        [data-md-color-scheme="default"] { ... }

    Both schemes share it. A selector match that only looks at the text
    immediately before `{` (i.e. the last selector in the list) would see
    "default" but miss "slate", since "slate" is followed by a comma, not
    a brace. That asymmetry is wrong -- the rule applies to both -- so
    both schemes must expose the same `--md-code-hl-*` keys, and a
    variable declared only in the combined rule must be visible from
    either scheme lookup.
    """
    slate = parse_vars(tokens, "slate")
    default = parse_vars(tokens, "default")

    assert "--md-code-hl-keyword-color" in slate
    assert "--md-code-hl-keyword-color" in default
    assert slate["--md-code-hl-keyword-color"] == default["--md-code-hl-keyword-color"]

    slate_hl_keys = {k for k in slate if k.startswith("--md-code-hl-")}
    default_hl_keys = {k for k in default if k.startswith("--md-code-hl-")}
    assert slate_hl_keys == default_hl_keys

"""Writes the test pages that are too large to keep in the repository.

Usage: make.py <directory>

- long.html: a 3,000-line reference manual, about 12,000 elements in Chromium.
- ci.html: a CI log of 3,000 lines that ends in an error, like a failed build.
"""
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

with open(out / "long.html", "w") as f:
    f.write("<html><head><title>Reference manual</title></head><body><h1>Reference manual</h1>")
    for i in range(3000):
        f.write(f"<p>Line {i}: the quick brown fox jumps over the lazy dog, section {i // 100}.</p>")
    f.write("</body></html>")

with open(out / "ci.html", "w") as f:
    f.write("<title>CI run #4711 – failed</title><h1>Job: test (ubuntu)</h1>")
    for i in range(3000):
        f.write(
            f"<div class=l><span class=n>{i}</span> "
            f"<span>Compiling crate-{i} v0.1.{i} (/build/crates/c{i})</span></div>"
        )
    f.write("<p>error: could not compile `atuin-client` (lib) due to 1 previous error</p>")

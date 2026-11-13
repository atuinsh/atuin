"""Builds Atuin Demo Mono: IBM Plex Mono with the symbols coding agents draw
that Plex lacks (⏺ ● ⎿ ✻ ⏵ and the rest) taken from Noto Sans Symbols 2,
each scaled into one cell.

Without them, betamax falls back per glyph, and on macOS it falls back to
Apple Color Emoji, so Claude Code's ⏺ renders as an emoji. Both fonts are
under the SIL OFL, which keeps the name "Plex" for IBM's own, hence the name.

Usage: python build.py <IBMPlexMono-Regular.ttf> <IBMPlexMono-Bold.ttf>
         <NotoSansSymbols2-Regular.ttf>
Writes AtuinDemoMono-Regular.ttf and AtuinDemoMono-Bold.ttf beside this file.
"""

import sys
from pathlib import Path

from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

RANGES = [
    (0x2190, 0x21FF),  # arrows
    (0x2300, 0x23FF),  # technical: ⏺ ⎿ ⏵
    (0x25A0, 0x25FF),  # geometric shapes: ●
    (0x2600, 0x26FF),  # miscellaneous symbols
    (0x2700, 0x27BF),  # dingbats: ✻ ✢ ✶
    (0x2B00, 0x2BFF),  # more arrows and shapes
]
# Of the cell, the most a symbol may fill, so it doesn't touch its neighbours.
FILL = 0.9


def build(plex_path: Path, symbols: TTFont, out: Path, style: str) -> int:
    plex = TTFont(plex_path)
    cell = plex["hmtx"]["zero"][0]
    scale_em = plex["head"].unitsPerEm / symbols["head"].unitsPerEm
    plex_cmap = plex.getBestCmap()
    sym_cmap = symbols.getBestCmap()
    sym_glyphs = symbols.getGlyphSet()
    glyf, hmtx = plex["glyf"], plex["hmtx"]
    order = list(plex.getGlyphOrder())

    added = 0
    for start, end in RANGES:
        for code in range(start, end + 1):
            if code in plex_cmap or code not in sym_cmap:
                continue
            src = sym_cmap[code]
            bounds = symbols["glyf"][src]
            bounds.recalcBounds(symbols["glyf"])
            if not hasattr(bounds, "xMin"):
                continue
            width = (bounds.xMax - bounds.xMin) * scale_em
            scale = scale_em * min(1.0, cell * FILL / width) if width else scale_em
            dx = (cell - (bounds.xMax - bounds.xMin) * scale) / 2 - bounds.xMin * scale
            pen = TTGlyphPen(None)
            sym_glyphs[src].draw(TransformPen(pen, (scale, 0, 0, scale, dx, 0)))
            name = f"uni{code:04X}"
            glyph = pen.glyph()
            glyph.recalcBounds(glyf)
            glyf[name] = glyph
            hmtx[name] = (cell, getattr(glyph, "xMin", 0))
            order.append(name)
            for table in plex["cmap"].tables:
                if table.isUnicode():
                    table.cmap[code] = name
            added += 1
    plex.setGlyphOrder(order)
    glyf.glyphOrder = order
    plex["maxp"].numGlyphs = len(order)

    family = "Atuin Demo Mono"
    for record in plex["name"].names:
        match record.nameID:
            case 1 | 16:
                record.string = family
            case 3:
                record.string = f"{family} {style}"
            case 4:
                record.string = f"{family} {style}"
            case 6:
                record.string = f"AtuinDemoMono-{style}"
    plex.save(out)
    return added


def main() -> None:
    regular, bold, noto = (Path(p) for p in sys.argv[1:4])
    symbols = TTFont(noto)
    here = Path(__file__).parent
    for path, style in ((regular, "Regular"), (bold, "Bold")):
        added = build(path, symbols, here / f"AtuinDemoMono-{style}.ttf", style)
        print(f"AtuinDemoMono-{style}.ttf: {added} symbols")


if __name__ == "__main__":
    main()

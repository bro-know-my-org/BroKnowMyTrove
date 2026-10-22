#!/usr/bin/env python3
"""Regenerate the bundled UI fallback; requires fonttools==4.60.1."""
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
FONT_DIR = ROOT / "assets/fonts"


def main():
    # Include comments and identifiers too: a conservative superset of literal
    # UI text, including both locales and punctuation. User text is not bundled.
    codepoints = set(range(0x20, 0x7F))
    for crate in ("vault-gui", "vault-i18n"):
        for source in sorted((ROOT / "crates" / crate / "src").rglob("*.rs")):
            codepoints.update(ord(ch) for ch in source.read_text(encoding="utf-8")
                              if not ch.isspace() and not ch.isascii())

    font = TTFont(FONT_DIR / "NotoSansCJK-Regular.ttc", fontNumber=2,
                  recalcTimestamp=False)
    options = subset.Options()
    options.name_IDs = ["*"]  # Preserve the original copyright and license.
    options.name_languages = ["*"]
    subsetter = subset.Subsetter(options=options)
    subsetter.populate(unicodes=sorted(codepoints))
    subsetter.subset(font)

    # Give the modified font its own family and PostScript name.
    names = {1: "BKMT Sans UI", 2: "Regular", 3: "BKMT Sans UI Regular",
             4: "BKMT Sans UI Regular", 6: "BkmtSansUI-Regular",
             16: "BKMT Sans UI", 17: "Regular"}
    for record in font["name"].names:
        if record.nameID in names:
            record.string = names[record.nameID].encode(record.getEncoding())
    cff = font["CFF "]
    cff.cff.fontNames = ["BkmtSansUI-Regular"]
    cff.cff.topDictIndex[0].FamilyName = "BKMT Sans UI"
    cff.cff.topDictIndex[0].FullName = "BKMT Sans UI Regular"

    output = FONT_DIR / "BkmtSansUI.otf"
    font.save(output)
    print(f"{output.name}: {output.stat().st_size:,} bytes, "
          f"{len(font.getBestCmap())} mapped characters")


if __name__ == "__main__":
    main()

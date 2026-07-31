#!/usr/bin/env python3
"""Regenerate frontend/icons/ from the app's icon source.

Run from the repo root:  python3 tools/make-icons.py

The output is committed rather than generated during `build-web.sh`, so a build
does not depend on which image tools happen to be installed. Re-run this only
when the source icon changes.

Sources, and why these and not the others in build/:

  build/iconComp.png            the current *composited* icon (purple CM mark).
                                "Comp" as in composited — this is the flattened
                                render of CAIDashboard.icon.
  build/CAIDashboard.icon/      Apple Icon Composer source: a glyph plus a fill
                                colour, assembled by the OS. Assets/Image.png is
                                the glyph alone, so its alpha is what to use when
                                compositing a new background.
  build/icon.png                OLD — a blue globe, superseded. Do not use.
  build/iconNew.png             superseded by iconComp.png, and only 960px.
"""
import os
import sys

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "frontend", "icons")

COMP = os.path.join(ROOT, "build", "iconComp.png")
GLYPH = os.path.join(ROOT, "build", "CAIDashboard.icon", "Assets", "Image.png")

# From CAIDashboard.icon/icon.json:
#   fill        display-p3(0.47461, 0.41968, 0.93701)
#   glyph layer display-p3(0.94245, 0.96783, 0.92085)
BG = (121, 107, 239, 255)
FG = (240, 247, 235, 255)

# Fraction of the canvas the glyph spans in the maskable icon. A maskable icon is
# cropped to a circle or squircle by the OS, so the mark has to stay inside the
# inner ~80%; 0.62 leaves comfortable room at the corners of a circular crop.
SAFE = 0.62
SIZES = (128, 192, 256, 512)


def main():
    for path in (COMP, GLYPH):
        if not os.path.exists(path):
            sys.exit(f"missing source: {path}")
    os.makedirs(OUT, exist_ok=True)

    # "any" icons keep iconComp's own rounded-square shape and its transparent
    # margin, which is what a plain (non-masked) icon slot expects.
    comp = Image.open(COMP).convert("RGBA")
    for size in SIZES:
        comp.resize((size, size), Image.LANCZOS).save(
            os.path.join(OUT, f"icon-{size}.png")
        )

    # The maskable icon is a genuinely different asset, not the same file
    # relabelled: declaring a rounded square with transparent margins as
    # "maskable" gets its corners clipped and the mark looks inset.
    glyph = Image.open(GLYPH).convert("RGBA")
    alpha = glyph.getchannel("A")
    # Crop to the actual ink, so the safe-zone maths is about the mark rather
    # than whatever padding the source carries.
    box = alpha.getbbox()
    alpha = alpha.crop(box)

    size = 512
    w, h = alpha.size
    scale = (size * SAFE) / max(w, h)
    gw, gh = max(1, round(w * scale)), max(1, round(h * scale))
    alpha = alpha.resize((gw, gh), Image.LANCZOS)

    canvas = Image.new("RGBA", (size, size), BG)
    canvas.paste(Image.new("RGBA", (gw, gh), FG), ((size - gw) // 2, (size - gh) // 2), alpha)
    canvas.save(os.path.join(OUT, "icon-maskable-512.png"))

    for name in sorted(os.listdir(OUT)):
        print(f"  {name:26} {os.path.getsize(os.path.join(OUT, name)):>7} bytes")


main()

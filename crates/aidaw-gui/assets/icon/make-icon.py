"""Render the AIDAW app icon from the SVGs in this folder.

    python3 make-icon.py   (needs: pip install cairosvg pillow)

aidaw.svg is the full icon; aidaw-small.svg is a simpler drawing that stays
readable at 16-32 px. Writes aidaw.ico (embedded in the .exe files by
build.rs) and aidaw-256.png (the window/taskbar icon).
"""
import io

import cairosvg
from PIL import Image


def render(svg, size):
    png = cairosvg.svg2png(url=svg, output_width=size, output_height=size)
    return Image.open(io.BytesIO(png)).convert("RGBA")


sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
frames = [render("aidaw-small.svg" if s <= 32 else "aidaw.svg", s) for s in sizes]
big = frames[-1]
big.save("aidaw-256.png", optimize=True)
big.save("aidaw.ico", sizes=[(s, s) for s in sizes], append_images=frames[:-1])

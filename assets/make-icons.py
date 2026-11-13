"""Draw the launcher icons from the favicon's design (ui/favicon.svg).

Needs Pillow, and iconutil for the Mac file:  python3 assets/make-icons.py
"""
import os
import subprocess
import tempfile

from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
BARS = ["#16a394", "#8a63e6", "#e0812b"]


def draw(size, margin):
    """The 32-unit favicon, scaled into a square with `margin` empty units around it."""
    scale = 4  # drawn large and shrunk, for smooth edges
    px = size * scale
    unit = px / (32 + 2 * margin)
    img = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    def rect(x, y, w, h, r, fill):
        x0, y0 = (x + margin) * unit, (y + margin) * unit
        d.rounded_rectangle([x0, y0, x0 + w * unit, y0 + h * unit], radius=r * unit, fill=fill)

    rect(0, 0, 32, 32, 7, "#16202b")
    for i, colour in enumerate(BARS):
        rect(7 + 7 * i, 8, 4, 16, 2, colour)
    return img.resize((size, size), Image.LANCZOS)


# Windows and Linux icons fill the square; Mac icons leave a margin around the tile.
draw(256, 0).save(os.path.join(HERE, "icon.png"), optimize=True)
draw(256, 0).save(os.path.join(HERE, "icon.ico"), sizes=[(s, s) for s in (16, 24, 32, 48, 64, 256)])

with tempfile.TemporaryDirectory() as tmp:
    iconset = os.path.join(tmp, "icon.iconset")
    os.mkdir(iconset)
    for s in (16, 32, 128, 256, 512):
        draw(s, 3.9).save(os.path.join(iconset, f"icon_{s}x{s}.png"), optimize=True)
        draw(s * 2, 3.9).save(os.path.join(iconset, f"icon_{s}x{s}@2x.png"), optimize=True)
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(HERE, "icon.icns")], check=True)

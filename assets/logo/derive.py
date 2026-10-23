#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = ["pillow>=10", "numpy"]
# ///
"""Derives, from the renderings in this folder, the small files that show the mark where fwd shows
itself, and writes them where each is used:

- extensions/vscode/media/icon.png, the VS Code extension's icon (package.json's `icon`, which VS
  Code takes as a PNG of at least 128 by 128 pixels, 256 by 256 for Retina screens): the tile
  rendering, cut to the tile, 256 by 256 pixels, so that the extension's page, which shows it at
  128, stays sharp on a Retina screen, with the white around its rounded corners made transparent
  so that no white square shows on a dark theme.
- crates/fwd-web/assets/mark.png, the page's mark: the mark alone, cut to the drawing and centred
  in a square of 96 by 96 pixels, four times the size the page's header shows it at, so that it
  stays sharp on any screen. Trunk copies it beside the page (crates/fwd-web/index.html), where it
  is the page's favicon, the mark beside the name in the page's header (app.css), and the icon on
  the tab of the page's panel in VS Code.

Run it again whenever a rendering changes, from anywhere, and commit what it writes:

    uv run assets/logo/derive.py

or `python3 assets/logo/derive.py` with Pillow and NumPy installed. Pillow reads the WebP
renderings and resizes with its Lanczos filter. The renderings are sRGB, so the PNGs carry no
colour profile, and browsers read them as sRGB.
"""

from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent


def main() -> None:
    save(tile(), 256, REPO / "extensions/vscode/media/icon.png")
    mark = Image.open(HERE / "fwdcentaur-logo.webp").convert("RGBA")
    save(mark, 96, REPO / "crates/fwd-web/assets/mark.png")


def tile() -> Image.Image:
    """The tile rendering with the white around the tile transparent: what a flood from a corner
    reaches through white. The cut is made at full size, so the resize smooths the tile's edge
    again, as it smooths every other."""
    rgb = Image.open(HERE / "fwdcentaur-app-icon.webp").convert("RGB")
    flooded = rgb.copy()
    ImageDraw.floodfill(flooded, (0, 0), (255, 0, 255), thresh=12)
    outside = np.all(np.asarray(flooded) == (255, 0, 255), axis=2)
    alpha = np.where(outside, 0, 255).astype(np.uint8)
    return Image.fromarray(np.dstack([np.asarray(rgb), alpha]), "RGBA")


def save(image: Image.Image, size: int, path: Path) -> None:
    """Cuts `image` to what it draws, centres that in a transparent square, and writes the square
    at `size` pixels a side as a PNG with no metadata. What it draws is where it is more than 3 %
    opaque: the transparent rendering has specks fainter than that up to sixty pixels out from the
    drawing."""
    drawn = image.crop(image.getchannel("A").point(lambda a: 255 if a > 8 else 0).getbbox())
    side = max(drawn.size)
    square = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    square.paste(drawn, ((side - drawn.width) // 2, (side - drawn.height) // 2))
    small = square.resize((size, size), Image.Resampling.LANCZOS)
    path.parent.mkdir(parents=True, exist_ok=True)
    # A new image from the pixels alone: the rendering's colour profile stays behind.
    Image.fromarray(np.asarray(small), "RGBA").save(path, optimize=True)
    print(f"{path.relative_to(REPO)}: {size} by {size} pixels, {path.stat().st_size} bytes")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Render the Cutting Board application icon from its master artwork.

Run this only when ``assets/app-icon-source.png`` changes. It needs Pillow;
the application itself needs nothing, because every result is committed as a
plain PNG under ``assets/``.

The master is an opaque square: a white rounded slab of line art sitting on a
black field. Desktop shells expect the icon to be the slab alone, so the field
outside it is cut to transparency here rather than baked into the artwork.

    ./scripts/build-app-icon.py
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "assets" / "app-icon-source.png"
ASSET_DIR = ROOT / "assets"

SIZES = (32, 48, 64, 128, 256, 512)
# The icon Tk loads for the window and the .desktop entry; the hicolor theme
# gets the sized variants instead.
DEFAULT_SIZE = 256
DEFAULT_NAME = "cutting-board.png"


def main() -> int:
    master = Image.open(SOURCE).convert("L")
    mask = _slab_mask(master)
    art = master.convert("RGB")

    for size in SIZES:
        icon = art.resize((size, size), Image.LANCZOS)
        icon.putalpha(mask.resize((size, size), Image.LANCZOS))
        icon.save(ASSET_DIR / f"cutting-board-{size}.png", optimize=True)
        print(f"icon  cutting-board-{size}.png")

    default = art.resize((DEFAULT_SIZE, DEFAULT_SIZE), Image.LANCZOS)
    default.putalpha(mask.resize((DEFAULT_SIZE, DEFAULT_SIZE), Image.LANCZOS))
    default.save(ASSET_DIR / DEFAULT_NAME, optimize=True)
    print(f"icon  {DEFAULT_NAME}")
    return 0


def _slab_mask(master: Image.Image) -> Image.Image:
    """An alpha mask covering the rounded slab and nothing outside it.

    The slab is found rather than measured: flooding the black field inward
    from a corner stops at the slab's edge, and every black line *inside* the
    slab is enclosed by white, so the flood never reaches it. That leaves the
    white body, which is then grown back over the black rim the flood ate.
    """
    body = master.point(lambda value: 255 if value > 128 else 0)

    flood = body.copy()
    ImageDraw.floodfill(flood, (0, 0), 128)
    # Anything the flood repainted is field; the rest is slab.
    mask = flood.point(lambda value: 0 if value == 128 else 255)

    rim = _rim_thickness(mask)
    if rim:
        mask = mask.filter(ImageFilter.MaxFilter(rim * 2 + 1))
    return mask


def _rim_thickness(mask: Image.Image) -> int:
    """How much black the flood ate: the gap between slab and canvas edge."""
    width, height = mask.size
    left, top, right, bottom = mask.getbbox()
    return min(left, top, width - right, height - bottom)


if __name__ == "__main__":
    raise SystemExit(main())

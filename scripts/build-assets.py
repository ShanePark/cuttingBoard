#!/usr/bin/env python3
"""Render the icon and tile artwork bundled with Cutting Board.

Run this only when the artwork catalog changes. It needs network access,
librsvg (via PyGObject), pycairo and Pillow; the application itself needs
none of them because every result is committed as a plain PNG under
``assets/``.

Brand marks come from Simple Icons (CC0 1.0). Each mark is a single path,
so it is rasterised to an alpha mask and then tinted with a colour chosen
for legibility on the dark canvas rather than the raw brand hex.

    ./scripts/build-assets.py
"""

from __future__ import annotations

import struct
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

import cairo
import gi
from PIL import Image, ImageDraw, ImageFilter

gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg  # noqa: E402 - must follow require_version

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets"
ICON_DIR = ASSETS / "icons"
UI_DIR = ASSETS / "ui"
WINDOW_ICON_NAME = "window-icon.argb"
CDN = "https://cdn.jsdelivr.net/npm/simple-icons@13/icons/{slug}.svg"

ICON_SIZES = (48, 96)

# tech id -> (Simple Icons slug, tint). The tint is picked for contrast on
# #0B0E14; near-black brand colours are replaced with a light neutral.
CATALOG: dict[str, tuple[str, str]] = {
    "spring": ("spring", "#6DB33F"),
    "react": ("react", "#61DAFB"),
    "vite": ("vite", "#A78BFA"),
    "nextjs": ("nextdotjs", "#E6EDF3"),
    "nuxt": ("nuxtdotjs", "#00DC82"),
    "vue": ("vuedotjs", "#4FC08D"),
    "angular": ("angular", "#F0526D"),
    "svelte": ("svelte", "#FF3E00"),
    "astro": ("astro", "#C9A9FF"),
    "remix": ("remix", "#E6EDF3"),
    "node": ("nodedotjs", "#7FC15E"),
    "deno": ("deno", "#E6EDF3"),
    "bun": ("bun", "#FBF0DF"),
    "webpack": ("webpack", "#8DD6F9"),
    "storybook": ("storybook", "#FF4785"),
    "python": ("python", "#FFD845"),
    "django": ("django", "#44B78B"),
    "flask": ("flask", "#E6EDF3"),
    "fastapi": ("fastapi", "#05BFA6"),
    "jupyter": ("jupyter", "#F37626"),
    "java": ("openjdk", "#E6EDF3"),
    "gradle": ("gradle", "#8FD3E8"),
    "maven": ("apachemaven", "#E05A78"),
    "kotlin": ("kotlin", "#A98BFF"),
    "tomcat": ("apachetomcat", "#F8DC75"),
    "rust": ("rust", "#FF8A5B"),
    "go": ("go", "#00ADD8"),
    "dotnet": ("dotnet", "#A78BFA"),
    "php": ("php", "#8E93D6"),
    "laravel": ("laravel", "#FF4536"),
    "ruby": ("ruby", "#E5544B"),
    "rails": ("rubyonrails", "#F0413E"),
    "elixir": ("elixir", "#C4A7E7"),
    "postgresql": ("postgresql", "#6C8FEB"),
    "mysql": ("mysql", "#6FA8D0"),
    "mariadb": ("mariadb", "#D08A6E"),
    "mongodb": ("mongodb", "#47A248"),
    "redis": ("redis", "#FF6152"),
    "sqlite": ("sqlite", "#7FC8E8"),
    "elasticsearch": ("elasticsearch", "#FEC514"),
    "rabbitmq": ("rabbitmq", "#FF7A2F"),
    "kafka": ("apachekafka", "#E6EDF3"),
    "solr": ("apachesolr", "#E0684F"),
    "minio": ("minio", "#E0526B"),
    "keycloak": ("keycloak", "#7FA8D9"),
    "grafana": ("grafana", "#F46800"),
    "prometheus": ("prometheus", "#E6522C"),
    "nginx": ("nginx", "#4ACF7F"),
    "caddy": ("caddy", "#4FA8DC"),
    "traefik": ("traefikproxy", "#5FC5C0"),
    "docker": ("docker", "#2496ED"),
    "kubernetes": ("kubernetes", "#5B8DEF"),
    "android": ("android", "#3DDC84"),
    "ollama": ("ollama", "#E6EDF3"),
    "supabase": ("supabase", "#3ECF8E"),
    "firebase": ("firebase", "#FFA000"),
    "graphql": ("graphql", "#E535AB"),
    "electron": ("electron", "#9FEAF9"),
    "flutter": ("flutter", "#54C5F8"),
}

# Drawn locally rather than fetched: neither an unidentified server nor a
# forwarded SSH port has a brand mark to borrow.
GENERIC = "service"
DRAWN = ("service", "ssh")


def main() -> int:
    ICON_DIR.mkdir(parents=True, exist_ok=True)
    UI_DIR.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as cache_dir:
        cache = Path(cache_dir)
        failed: list[str] = []
        for tech, (slug, tint) in sorted(CATALOG.items()):
            try:
                svg = _download(slug, cache)
            except urllib.error.URLError as exc:
                failed.append(f"{tech} ({slug}): {exc}")
                continue
            for size in ICON_SIZES:
                mask = _rasterise(svg, size)
                _tint(mask, tint).save(ICON_DIR / f"{tech}-{size}.png")
            print(f"icon  {tech:<14} <- simple-icons/{slug}")

    for size in ICON_SIZES:
        _generic_icon(size).save(ICON_DIR / f"{GENERIC}-{size}.png")
        _ssh_icon(size).save(ICON_DIR / f"ssh-{size}.png")
    for name in DRAWN:
        print(f"icon  {name:<14} <- drawn")

    _build_tiles()
    _build_power_glyphs()
    _build_window_icon()

    if failed:
        print("\nFAILED — these marks are unavailable and fall back to a generic glyph:")
        for item in failed:
            print(f"  {item}")
        return 1
    print(f"\n{len(CATALOG) + len(DRAWN)} marks, {len(ICON_SIZES)} sizes, tiles and glyphs written")
    return 0


def _download(slug: str, cache: Path) -> Path:
    target = cache / f"{slug}.svg"
    if target.exists():
        return target
    request = urllib.request.Request(
        CDN.format(slug=slug),
        headers={"User-Agent": "cutting-board-asset-build"},
    )
    with urllib.request.urlopen(request, timeout=20) as response:  # noqa: S310 - fixed CDN host
        payload = response.read()
    if not payload.lstrip().startswith(b"<svg"):
        raise urllib.error.URLError("response was not an SVG")
    target.write_bytes(payload)
    return target


def _rasterise(svg: Path, size: int) -> Image.Image:
    """Render the mark with librsvg and return it as an RGBA image."""
    handle = Rsvg.Handle.new_from_data(svg.read_bytes())
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, size, size)
    context = cairo.Context(surface)
    viewport = Rsvg.Rectangle()
    viewport.x = 0.0
    viewport.y = 0.0
    viewport.width = float(size)
    viewport.height = float(size)
    handle.render_document(context, viewport)
    surface.flush()

    # Cairo hands back premultiplied BGRA; Pillow's "BGRa" raw mode undoes both.
    return Image.frombuffer(
        "RGBA",
        (size, size),
        bytes(surface.get_data()),
        "raw",
        "BGRa",
        surface.get_stride(),
        1,
    )


def _tint(mask: Image.Image, colour: str) -> Image.Image:
    solid = Image.new("RGBA", mask.size, _rgb(colour) + (255,))
    solid.putalpha(mask.getchannel("A"))
    return solid


def _generic_icon(size: int) -> Image.Image:
    """A stacked-server glyph for services with no recognised brand."""
    scale = 4
    edge = size * scale
    image = Image.new("RGBA", (edge, edge), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    colour = (125, 133, 144, 255)
    accent = (34, 211, 238, 255)

    margin = edge * 0.14
    width = edge - margin * 2
    height = width * 0.26
    gap = height * 0.34
    top = (edge - (height * 3 + gap * 2)) / 2
    radius = height * 0.3
    for index in range(3):
        y = top + index * (height + gap)
        draw.rounded_rectangle(
            (margin, y, margin + width, y + height),
            radius=radius,
            outline=colour,
            width=int(edge * 0.028),
        )
        dot = height * 0.22
        cx = margin + width - height * 0.55
        cy = y + height / 2
        draw.ellipse((cx - dot, cy - dot, cx + dot, cy + dot), fill=accent if index == 0 else colour)
    return image.resize((size, size), Image.LANCZOS)


def _ssh_icon(size: int) -> Image.Image:
    """A terminal window with a prompt: a forwarded shell, not a brand."""
    scale = 4
    edge = size * scale
    image = Image.new("RGBA", (edge, edge), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    colour = (183, 194, 207, 255)
    accent = (34, 211, 238, 255)

    stroke = int(edge * 0.055)
    box = (edge * 0.10, edge * 0.18, edge * 0.90, edge * 0.82)
    draw.rounded_rectangle(box, radius=edge * 0.10, outline=colour, width=stroke)

    # chevron
    left, top = edge * 0.26, edge * 0.40
    draw.line(
        [(left, top), (left + edge * 0.13, edge * 0.50), (left, edge * 0.60)],
        fill=accent,
        width=stroke,
        joint="curve",
    )
    # underscore
    draw.line(
        (edge * 0.50, edge * 0.61, edge * 0.72, edge * 0.61),
        fill=colour,
        width=stroke,
    )
    return image.resize((size, size), Image.LANCZOS)


TILE_WIDTH = 176
TILE_HEIGHT = 152
TILE_PAD = 14  # room for the hover glow


def _build_tiles() -> None:
    specs = {
        "tile-idle": {
            "top": "#141A24",
            "bottom": "#0E131B",
            "border": "#1F2733",
            "glow": None,
        },
        "tile-hover": {
            "top": "#1A2230",
            "bottom": "#121926",
            "border": "#22D3EE",
            "glow": "#22D3EE",
        },
        "tile-armed": {
            "top": "#251722",
            "bottom": "#1A1019",
            "border": "#FB7185",
            "glow": "#FB7185",
        },
    }
    for name, spec in specs.items():
        _tile(spec).save(UI_DIR / f"{name}.png")
        print(f"tile  {name}")


def _tile(spec: dict[str, str | None]) -> Image.Image:
    scale = 3
    width = (TILE_WIDTH + TILE_PAD * 2) * scale
    height = (TILE_HEIGHT + TILE_PAD * 2) * scale
    pad = TILE_PAD * scale
    radius = 18 * scale
    box = (pad, pad, width - pad, height - pad)

    image = Image.new("RGBA", (width, height), (0, 0, 0, 0))

    glow = spec["glow"]
    if glow:
        halo = Image.new("RGBA", (width, height), (0, 0, 0, 0))
        ImageDraw.Draw(halo).rounded_rectangle(box, radius=radius, fill=_rgb(glow) + (85,))
        halo = halo.filter(ImageFilter.GaussianBlur(pad * 0.55))
        image.alpha_composite(halo)

    body = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    gradient = _vertical_gradient(width, height, str(spec["top"]), str(spec["bottom"]))
    shape = Image.new("L", (width, height), 0)
    ImageDraw.Draw(shape).rounded_rectangle(box, radius=radius, fill=255)
    body.paste(gradient, (0, 0), shape)

    ImageDraw.Draw(body).rounded_rectangle(
        box,
        radius=radius,
        outline=_rgb(str(spec["border"])) + (255,),
        width=max(1, int(scale * 1.2)),
    )
    image.alpha_composite(body)

    # A thin highlight along the top edge sells the raised, glassy surface.
    sheen = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    ImageDraw.Draw(sheen).rounded_rectangle(
        (box[0] + radius * 0.4, box[1], box[2] - radius * 0.4, box[1] + scale * 2),
        radius=scale,
        fill=(255, 255, 255, 26),
    )
    image.alpha_composite(sheen)

    return image.resize(
        (TILE_WIDTH + TILE_PAD * 2, TILE_HEIGHT + TILE_PAD * 2),
        Image.LANCZOS,
    )


def _vertical_gradient(width: int, height: int, top: str, bottom: str) -> Image.Image:
    start, end = _rgb(top), _rgb(bottom)
    column = Image.new("RGB", (1, height))
    pixels = column.load()
    assert pixels is not None
    for y in range(height):
        ratio = y / max(1, height - 1)
        pixels[0, y] = tuple(  # type: ignore[assignment]
            round(start[channel] + (end[channel] - start[channel]) * ratio) for channel in range(3)
        )
    return column.resize((width, height), Image.BILINEAR).convert("RGBA")


POWER_SIZE = 26


def _build_power_glyphs() -> None:
    variants = {
        "power-idle": ("#7D8590", None),
        "power-hot": ("#FF7A8A", "#FB7185"),
    }
    for name, (colour, glow) in variants.items():
        _power_glyph(colour, glow).save(UI_DIR / f"{name}.png")
        print(f"glyph {name}")


def _power_glyph(colour: str, glow: str | None) -> Image.Image:
    """IEC 5009 power symbol: a broken ring with a vertical stem."""
    scale = 8
    edge = POWER_SIZE * scale
    image = Image.new("RGBA", (edge, edge), (0, 0, 0, 0))

    if glow:
        halo = Image.new("RGBA", (edge, edge), (0, 0, 0, 0))
        ImageDraw.Draw(halo).ellipse(
            (edge * 0.12, edge * 0.12, edge * 0.88, edge * 0.88),
            fill=_rgb(glow) + (70,),
        )
        image.alpha_composite(halo.filter(ImageFilter.GaussianBlur(edge * 0.09)))

    draw = ImageDraw.Draw(image)
    stroke = int(edge * 0.10)
    inset = edge * 0.24
    # The arc starts and ends short of vertical, leaving the classic gap.
    draw.arc(
        (inset, inset, edge - inset, edge - inset),
        start=-63,
        end=243,
        fill=_rgb(colour) + (255,),
        width=stroke,
    )
    centre = edge / 2
    draw.line(
        (centre, edge * 0.16, centre, edge * 0.46),
        fill=_rgb(colour) + (255,),
        width=stroke,
    )
    return image.resize((POWER_SIZE, POWER_SIZE), Image.LANCZOS)


# Sizes offered to the window manager: the title bar draws the small ones and
# the task switcher the large. 256 is left out to keep the blob small.
WINDOW_ICON_SIZES = (32, 48, 64, 128)


def _build_window_icon() -> None:
    """Pack the application icon into the exact _NET_WM_ICON payload.

    Tk 8.6 does not publish that property, so the app writes it itself at
    startup. Decoding PNGs at runtime would mean either a Pillow dependency or
    a hand-rolled decoder, so the pixels are laid out here instead: pairs of
    width and height followed by the ARGB pixels, as little-endian uint32.
    """
    payload = bytearray()
    for size in WINDOW_ICON_SIZES:
        source = ASSETS / f"cutting-board-{size}.png"
        image = Image.open(source).convert("RGBA").resize((size, size), Image.LANCZOS)
        payload += struct.pack("<II", size, size)
        for red, green, blue, alpha in image.getdata():
            payload += struct.pack("<I", (alpha << 24) | (red << 16) | (green << 8) | blue)
    (ASSETS / WINDOW_ICON_NAME).write_bytes(bytes(payload))
    print(f"icon  window blob   <- {len(WINDOW_ICON_SIZES)} sizes, {len(payload)} bytes")


def _rgb(value: str) -> tuple[int, int, int]:
    value = value.lstrip("#")
    return int(value[0:2], 16), int(value[2:4], 16), int(value[4:6], 16)


if __name__ == "__main__":
    raise SystemExit(main())

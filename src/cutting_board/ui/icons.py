from __future__ import annotations

import tkinter as tk
from pathlib import Path


class IconStore:
    """Load and cache the PNG artwork committed under ``assets/``.

    Tk images are collected as soon as the last Python reference goes away, so
    every image handed out here is retained for the lifetime of the store.
    """

    def __init__(self, assets_dir: Path | None) -> None:
        self._assets_dir = assets_dir
        self._cache: dict[str, tk.PhotoImage | None] = {}

    @property
    def available(self) -> bool:
        return self._assets_dir is not None

    def tech(self, tech: str, size: int = 48) -> tk.PhotoImage | None:
        """The brand mark for a technology, falling back to a generic glyph."""
        image = self._load(f"icons/{tech}-{size}.png")
        if image is None and tech != "service":
            image = self._load(f"icons/service-{size}.png")
        return image

    def ui(self, name: str) -> tk.PhotoImage | None:
        return self._load(f"ui/{name}.png")

    def app(self, size: int | None = None) -> tk.PhotoImage | None:
        """The application icon, either the default copy or a published size."""
        if size is None:
            return self._load("cutting-board.png")
        return self._load(f"cutting-board-{size}.png")

    def _load(self, relative: str) -> tk.PhotoImage | None:
        if relative in self._cache:
            return self._cache[relative]
        image: tk.PhotoImage | None = None
        if self._assets_dir is not None:
            path = self._assets_dir / relative
            if path.is_file():
                try:
                    image = tk.PhotoImage(file=str(path))
                except tk.TclError:
                    image = None
        self._cache[relative] = image
        return image

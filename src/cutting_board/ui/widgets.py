from __future__ import annotations

import tkinter as tk
from collections.abc import Callable

from cutting_board.models import ServiceSnapshot
from cutting_board.presentation import FRESH_UPTIME_SECONDS, format_uptime_compact
from cutting_board.scanner.docker import ContainerInfo
from cutting_board.ui import theme
from cutting_board.ui.icons import IconStore

ICON_SIZE = 48
MAX_CHIPS = 3

# Who started a service is only worth a badge when the answer is interesting.
# A shell or an init system launched most of what is on the board, so badging
# those would just add a label to every tile.
ORIGIN_COLOURS = {"agent": theme.VIOLET, "ide": theme.ACCENT}

# X11 reports the wheel as button presses, one per notch; Windows and macOS
# send <MouseWheel> carrying a delta instead. The horizontal wheel (Button-6
# and Button-7 on X11) is deliberately left alone: this area only scrolls
# vertically, and claiming those events would turn a sideways flick on a
# trackpad into unexpected vertical movement.
WHEEL_SEQUENCES = ("<MouseWheel>", "<Button-4>", "<Button-5>")

# One notch travels a little over a third of a tile, so a flick reads as a
# glide rather than the jump a canvas "unit" would give (a unit is a tenth of
# the viewport, which grows with the window).
WHEEL_STEP_PIXELS = 54


class ScrollArea(tk.Frame):
    """A vertically scrolling canvas that hosts the project sections."""

    def __init__(self, master: tk.Misc) -> None:
        super().__init__(master, bg=theme.CANVAS)
        self.canvas = tk.Canvas(
            self,
            bg=theme.CANVAS,
            highlightthickness=0,
            bd=0,
            takefocus=False,
        )
        self.scrollbar = tk.Scrollbar(
            self,
            orient="vertical",
            command=self.canvas.yview,
            width=8,
            troughcolor=theme.CANVAS,
            bg=theme.BORDER,
            activebackground=theme.ACCENT_DIM,
            bd=0,
            highlightthickness=0,
            relief="flat",
        )
        self.body = tk.Frame(self.canvas, bg=theme.CANVAS)
        self._window = self.canvas.create_window((0, 0), window=self.body, anchor="nw")

        self.canvas.configure(yscrollcommand=self._on_scroll)
        self.canvas.pack(side="left", fill="both", expand=True)

        self.body.bind("<Configure>", self._on_body_configure)
        self.canvas.bind("<Configure>", self._on_canvas_configure)

        # Tk delivers a mouse event to the widget under the pointer and runs
        # only that widget's bind tags; unlike a browser it never bubbles the
        # event up to the parents. Binding the tiles one by one therefore
        # leaves every section header, label and gap between tiles dead, and
        # the board throws its children away and builds new ones on every
        # scan, so such bindings would have to be renewed forever.
        #
        # A widget's bind tags include the path name of its toplevel, so a
        # single binding there sees the wheel wherever the pointer is inside
        # the window; _owns() then decides whether the event happened over
        # this area. Nothing has to be re-bound when the tiles are rebuilt.
        self._toplevel = self.winfo_toplevel()
        self._wheel_bindings: list[tuple[str, str]] = [
            (sequence, self._toplevel.bind(sequence, self._on_wheel, add="+"))
            for sequence in WHEEL_SEQUENCES
        ]
        self.bind("<Destroy>", self._on_destroy, add="+")

    def _on_scroll(self, first: str, last: str) -> None:
        # Keep the chrome clean: the scrollbar only exists when it is needed.
        if float(first) <= 0.0 and float(last) >= 1.0:
            self.scrollbar.pack_forget()
        else:
            self.scrollbar.pack(side="right", fill="y", padx=(2, 0))
        self.scrollbar.set(first, last)

    def _on_body_configure(self, _event: tk.Event) -> None:
        self.canvas.configure(scrollregion=self.canvas.bbox("all"))

    def _on_canvas_configure(self, event: tk.Event) -> None:
        self.canvas.itemconfigure(self._window, width=event.width)

    # ----------------------------------------------------------------- wheel

    def _on_wheel(self, event: tk.Event) -> None:
        if not self._owns(getattr(event, "widget", None)):
            return
        notches = _wheel_notches(event)
        if notches:
            self._scroll_pixels(notches * WHEEL_STEP_PIXELS)

    def _owns(self, widget: object) -> bool:
        """Is `widget` this area or something drawn inside it?

        The wheel is bound on the whole window, so the header, the footer and
        anything else outside this frame has to be filtered out. Comparing
        Tk path names is enough and, unlike winfo_containing, it costs no
        round trip and is unaffected by the pointer sitting over a child that
        was created since the event was queued.
        """
        if widget is None:
            return False
        own = str(self)
        path = str(widget)
        return path == own or path.startswith(own + ".")

    def _scroll_pixels(self, pixels: float) -> None:
        first, last = self.canvas.yview()
        if first <= 0.0 and last >= 1.0:
            # Everything already fits, which is the same test that hides the
            # scrollbar: the wheel must not nudge a board that has nowhere to go.
            return
        height = self._region_height()
        if height <= 0:
            return
        # yview works in fractions of the scroll region, and clamping here
        # keeps a fast flick from running past the first or the last tile.
        span = last - first
        target = min(max(first + pixels / height, 0.0), max(0.0, 1.0 - span))
        self.canvas.yview_moveto(target)

    def _region_height(self) -> float:
        region = str(self.canvas.cget("scrollregion")).split()
        if len(region) != 4:
            return 0.0
        try:
            return float(region[3]) - float(region[1])
        except ValueError:
            return 0.0

    def _on_destroy(self, event: tk.Event) -> None:
        # The wheel bindings live on the toplevel, which outlives this frame,
        # so they have to be taken back or they would fire against a dead
        # canvas. Children report their own destruction through the same tag.
        if event.widget is not self:
            return
        for sequence, funcid in self._wheel_bindings:
            try:
                self._toplevel.unbind(sequence, funcid)
            except tk.TclError:
                pass  # the whole window is going away; the binding goes with it
        self._wheel_bindings = []

    def bind_wheel_recursive(self, widget: tk.Misc) -> None:
        """Kept for callers that used to re-bind every freshly built child.

        The wheel is bound once on the toplevel now, so newly created tiles
        are already covered and there is nothing left to do here.
        """
        del widget


class SectionHeader(tk.Frame):
    """Project name and path above a run of tiles."""

    def __init__(
        self,
        master: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        title: str,
        path: str | None,
        accent: str = theme.TEXT_DIM,
    ) -> None:
        super().__init__(master, bg=theme.CANVAS)

        row = tk.Frame(self, bg=theme.CANVAS)
        row.pack(fill="x", padx=theme.TILE_PAD + 4)

        tk.Frame(row, bg=accent, width=3, height=13).pack(side="left", padx=(0, 9))
        tk.Label(
            row,
            text=title.upper(),
            bg=theme.CANVAS,
            fg=theme.TEXT,
            font=fonts["section"],
        ).pack(side="left")
        if path:
            tk.Label(
                row,
                text=_shorten_path(path),
                bg=theme.CANVAS,
                fg=theme.TEXT_DIM,
                font=fonts["section_path"],
            ).pack(side="left", padx=(10, 0))

        tk.Frame(self, bg=theme.HAIRLINE, height=1).pack(
            fill="x", padx=theme.TILE_PAD + 4, pady=(7, 0)
        )


class ServiceTile(tk.Canvas):
    """One service, shown as a brand mark with its ports.

    Everything else the scanner knows lives behind the tile: clicking it opens
    the detail dialog, so the board itself stays readable at a glance.
    """

    def __init__(
        self,
        master: tk.Misc,
        *,
        service: ServiceSnapshot,
        fonts: dict[str, tuple[str, int, str]],
        icons: IconStore,
        busy: bool,
        on_details: Callable[[ServiceSnapshot], None],
        on_terminate: Callable[[ServiceSnapshot], None],
        on_open: Callable[[ServiceSnapshot], None],
    ) -> None:
        super().__init__(
            master,
            width=theme.TILE_SPAN,
            height=theme.TILE_HEIGHT + theme.TILE_PAD * 2,
            bg=theme.CANVAS,
            highlightthickness=0,
            bd=0,
            takefocus=False,
        )
        self.service = service
        self._fonts = fonts
        self._icons = icons
        self._busy = busy
        self._on_details = on_details
        self._on_terminate = on_terminate
        self._on_open = on_open
        self._over_power = False
        # Where the uptime line sits and which canvas items draw it, so it can
        # be refreshed without touching the rest of the tile.
        self._uptime_at: tuple[float, float] | None = None
        self._uptime_items: tuple[int, int] | None = None

        self._background = self.create_image(0, 0, anchor="nw", image=icons.ui("tile-idle"))
        self._draw()

        self.bind("<Enter>", self._on_enter)
        self.bind("<Leave>", self._on_leave)
        self.bind("<Motion>", self._on_motion)
        self.bind("<Button-1>", self._on_click)

    # ------------------------------------------------------------------ draw

    def _draw(self) -> None:
        service = self.service
        pad = theme.TILE_PAD
        centre = theme.TILE_SPAN / 2
        accent = theme.CATEGORY_COLORS.get(service.category.value, theme.TEXT_MUTED)

        self._draw_origin(pad)

        icon = self._icons.tech(service.tech, ICON_SIZE)
        if icon is not None:
            self.create_image(centre, pad + 44, image=icon)
        else:
            self.create_text(
                centre,
                pad + 44,
                text=service.tech[:2].upper(),
                fill=accent,
                font=self._fonts["section"],
            )

        self.create_text(
            centre,
            pad + 88,
            text=_ellipsis(service.display_name, 20),
            fill=theme.TEXT if not self._busy else theme.TEXT_DIM,
            font=self._fonts["tile_name"],
            anchor="center",
        )

        self._draw_chips(centre, pad + 112, accent)

        if self._busy:
            self.create_text(
                centre,
                pad + 134,
                text="stopping…",
                fill=theme.WARNING,
                font=self._fonts["tile_meta"],
            )
        else:
            self._draw_uptime(centre, pad + 134)

        self._power = self.create_image(
            theme.TILE_SPAN - pad - 16,
            pad + 16,
            image=self._icons.ui("power-idle"),
            state="hidden",
        )

    def _draw_origin(self, pad: int) -> None:
        """A badge naming the tool that started the service, when one is known."""
        colour = ORIGIN_COLOURS.get(self.service.origin_kind)
        label = self.service.origin_label
        if colour is None or not label:
            return
        width = 14 + _text_width(label)
        left = pad + 8
        top = pad + 8
        theme.rounded_rect(
            self,
            left,
            top,
            left + width,
            top + 17,
            5,
            fill=theme.SURFACE_ALT,
            outline=colour,
        )
        self.create_text(
            left + width / 2,
            top + 9,
            text=label,
            fill=colour,
            font=self._fonts["chip"],
        )

    def _draw_uptime(self, centre: float, y: float) -> None:
        """How long the service has been up, as its own quiet line.

        A freshly started process is the one a developer is most likely to be
        looking for, so the first few minutes are tinted; after that the line
        recedes into the tile.
        """
        self._uptime_at = (centre, y)
        process = self.service.process
        if process is None:
            return
        text = format_uptime_compact(process.uptime_seconds)
        if not text:
            return
        colour = self._uptime_colour(process.uptime_seconds)
        left = centre - _uptime_width(text) / 2 - 9
        dot = self.create_oval(left, y - 2, left + 4, y + 2, fill=colour, outline="")
        label = self.create_text(
            centre + 3,
            y,
            text=text,
            fill=colour,
            font=self._fonts["tile_meta"],
            anchor="center",
        )
        self._uptime_items = (dot, label)

    @staticmethod
    def _uptime_colour(seconds: int | None) -> str:
        return theme.ACCENT if (seconds or 0) < FRESH_UPTIME_SECONDS else theme.TEXT_DIM

    # ---------------------------------------------------------------- refresh

    def adopt(self, service: ServiceSnapshot) -> None:
        """Point the tile at the newest snapshot of the service it already shows.

        The board keeps its tiles across a scan that changed nothing it draws,
        but the snapshot behind them is a new object every scan. Replacing the
        reference here is what keeps a click opening the detail dialog on
        current numbers instead of the ones the tile was built from.
        """
        self.service = service
        self._refresh_uptime()

    def _refresh_uptime(self) -> None:
        """Repaint only the uptime line, leaving the rest of the tile alone.

        Uptime is the one thing on a tile that moves on every scan, so it is
        updated in place rather than by rebuilding the tile.
        """
        if self._busy or self._uptime_at is None:
            return
        centre, y = self._uptime_at
        process = self.service.process
        text = format_uptime_compact(process.uptime_seconds) if process else ""
        if not text:
            if self._uptime_items is not None:
                self.delete(*self._uptime_items)
                self._uptime_items = None
            return
        if self._uptime_items is None:  # nothing to show before, something now
            self._draw_uptime(centre, y)
            return
        dot, label = self._uptime_items
        colour = self._uptime_colour(process.uptime_seconds if process else None)
        left = centre - _uptime_width(text) / 2 - 9
        self.coords(dot, left, y - 2, left + 4, y + 2)
        self.itemconfigure(dot, fill=colour)
        self.itemconfigure(label, text=text, fill=colour)

    def _draw_chips(self, centre: float, y: float, accent: str) -> None:
        ports = self.service.unique_ports
        if not ports:
            return
        labels = [str(port) for port in ports[:MAX_CHIPS]]
        overflow = len(ports) - len(labels)
        if overflow > 0:
            labels.append(f"+{overflow}")

        font = self._fonts["chip"]
        widths = [max(30, 11 + 7 * len(label)) for label in labels]
        total = sum(widths) + 5 * (len(widths) - 1)
        x = centre - total / 2
        for label, width in zip(labels, widths):
            theme.rounded_rect(
                self,
                x,
                y - 9,
                x + width,
                y + 9,
                6,
                fill=theme.SURFACE_ALT,
                outline=accent if label[0] != "+" else theme.BORDER,
            )
            self.create_text(
                x + width / 2,
                y,
                text=label,
                fill=accent if label[0] != "+" else theme.TEXT_DIM,
                font=font,
            )
            x += width + 5

    # ----------------------------------------------------------- interaction

    def _on_enter(self, _event: tk.Event) -> None:
        self.itemconfigure(self._background, image=self._icons.ui("tile-hover"))
        if self.service.can_terminate and not self._busy:
            self.itemconfigure(self._power, state="normal")

    def _on_leave(self, _event: tk.Event) -> None:
        self._over_power = False
        self.itemconfigure(self._background, image=self._icons.ui("tile-idle"))
        self.itemconfigure(self._power, image=self._icons.ui("power-idle"), state="hidden")

    def _on_motion(self, event: tk.Event) -> None:
        if not self.service.can_terminate or self._busy:
            return
        over = self._in_power_zone(event.x, event.y)
        if over == self._over_power:
            return
        self._over_power = over
        self.itemconfigure(
            self._background,
            image=self._icons.ui("tile-armed" if over else "tile-hover"),
        )
        self.itemconfigure(
            self._power,
            image=self._icons.ui("power-hot" if over else "power-idle"),
        )
        self.configure(cursor="hand2" if over else "")

    def _on_click(self, event: tk.Event) -> None:
        if self._busy:
            return
        if self.service.can_terminate and self._in_power_zone(event.x, event.y):
            self._on_terminate(self.service)
            return
        if self.service.browser_url() and event.state & 0x0004:  # Ctrl-click
            self._on_open(self.service)
            return
        self._on_details(self.service)

    def _in_power_zone(self, x: int, y: int) -> bool:
        pad = theme.TILE_PAD
        cx = theme.TILE_SPAN - pad - 16
        cy = pad + 16
        return abs(x - cx) <= 17 and abs(y - cy) <= 17


class ContainerTile(tk.Canvas):
    """One Docker container, drawn like a service tile.

    Containers are shown but never acted on: stopping one is an operation on
    shared infrastructure, and the board is a place to see what is running.
    """

    def __init__(
        self,
        master: tk.Misc,
        *,
        container: ContainerInfo,
        tech: str,
        fonts: dict[str, tuple[str, int, str]],
        icons: IconStore,
        on_details: Callable[[ContainerInfo], None],
    ) -> None:
        super().__init__(
            master,
            width=theme.TILE_SPAN,
            height=theme.TILE_HEIGHT + theme.TILE_PAD * 2,
            bg=theme.CANVAS,
            highlightthickness=0,
            bd=0,
            takefocus=False,
        )
        self.container = container
        self._tech = tech
        self._fonts = fonts
        self._icons = icons
        self._on_details = on_details

        self._background = self.create_image(0, 0, anchor="nw", image=icons.ui("tile-idle"))
        self._draw()

        self.bind("<Enter>", self._on_enter)
        self.bind("<Leave>", self._on_leave)
        self.bind("<Button-1>", lambda _event: self._on_details(self.container))
        self.configure(cursor="hand2")

    def _draw(self) -> None:
        container = self.container
        pad = theme.TILE_PAD
        centre = theme.TILE_SPAN / 2
        running = container.running
        accent = theme.OK if running else theme.TEXT_DIM

        icon = self._icons.tech(self._tech, ICON_SIZE)
        if icon is not None:
            self.create_image(centre, pad + 44, image=icon)

        # A stopped container is still worth seeing, but it must not compete
        # with the running ones for attention.
        self.create_text(
            centre,
            pad + 88,
            text=_ellipsis(container.name, 20),
            fill=theme.TEXT if running else theme.TEXT_MUTED,
            font=self._fonts["tile_name"],
            anchor="center",
        )

        self._draw_port_chips(centre, pad + 112, theme.VIOLET if running else theme.BORDER)

        if container.status:
            self.create_text(
                centre,
                pad + 134,
                text=_ellipsis(container.status, 22),
                fill=accent,
                font=self._fonts["tile_meta"],
                anchor="center",
            )

        self.create_oval(pad + 12, pad + 12, pad + 18, pad + 18, fill=accent, outline="")

    def _draw_port_chips(self, centre: float, y: float, accent: str) -> None:
        ports = self.container.ports
        if not ports:
            return
        labels = [str(port) for port in ports[:MAX_CHIPS]]
        overflow = len(ports) - len(labels)
        if overflow > 0:
            labels.append(f"+{overflow}")

        widths = [max(30, 11 + 7 * len(label)) for label in labels]
        total = sum(widths) + 5 * (len(widths) - 1)
        x = centre - total / 2
        for label, width in zip(labels, widths):
            theme.rounded_rect(
                self,
                x,
                y - 9,
                x + width,
                y + 9,
                6,
                fill=theme.SURFACE_ALT,
                outline=accent if label[0] != "+" else theme.BORDER,
            )
            self.create_text(
                x + width / 2,
                y,
                text=label,
                fill=accent if label[0] != "+" else theme.TEXT_DIM,
                font=self._fonts["chip"],
            )
            x += width + 5

    def _on_enter(self, _event: tk.Event) -> None:
        self.itemconfigure(self._background, image=self._icons.ui("tile-hover"))

    def _on_leave(self, _event: tk.Event) -> None:
        self.itemconfigure(self._background, image=self._icons.ui("tile-idle"))


def _wheel_notches(event: tk.Event) -> int:
    """How far the wheel turned, positive downwards.

    X11 sends one button press per notch and no delta at all, so the button
    number is the only signal there. Windows reports a delta in multiples of
    120, while macOS reports small counts that must not be rounded away.
    """
    num = getattr(event, "num", None)
    if num == 4:
        return -1
    if num == 5:
        return 1
    try:
        delta = int(getattr(event, "delta", 0) or 0)
    except (TypeError, ValueError):
        return 0
    if not delta:
        return 0
    if abs(delta) >= 120:
        return -(delta // 120)
    return -1 if delta > 0 else 1


def _text_width(text: str) -> float:
    """Rough pixel width of a short chip label at the tile font size."""
    return sum(9.0 if character > "\u007f" else 5.5 for character in text)


def _uptime_width(text: str) -> float:
    """Rough pixel width of the uptime line.

    Measuring through a font object would need a live widget for every tile;
    the dot beside the text only has to sit close, so a per-character estimate
    that accounts for the wider Hangul glyphs is enough.
    """
    return sum(9.0 if character > "\u007f" else 5.0 for character in text)


def _ellipsis(text: str, limit: int) -> str:
    text = text.strip()
    return text if len(text) <= limit else text[: limit - 1] + "…"


def _shorten_path(path: str, limit: int = 44) -> str:
    import os

    home = os.path.expanduser("~")
    if path.startswith(home):
        path = "~" + path[len(home) :]
    if len(path) <= limit:
        return path
    parts = path.split("/")
    return ".../" + "/".join(parts[-2:]) if len(parts) > 2 else path[-limit:]

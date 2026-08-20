from __future__ import annotations

import tkinter as tk
from collections.abc import Callable
from dataclasses import dataclass
from ipaddress import ip_address
from tkinter import font as tkfont
from urllib.parse import urlsplit

from cutting_board.models import ServiceSnapshot
from cutting_board.presentation import FRESH_UPTIME_SECONDS, format_uptime_compact
from cutting_board.scanner.docker import ContainerInfo
from cutting_board.ui import theme
from cutting_board.ui.icons import IconStore

# The committed technology marks are 48 px; in the horizontal card they take
# roughly forty pixels of visual weight without dominating the copy.
ICON_SIZE = 48
MAX_CHIPS = 2

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


@dataclass(slots=True)
class _CanvasAction:
    key: str
    bounds: tuple[float, float, float, float]
    shape: int | None
    visuals: tuple[tuple[int, str], ...]
    enabled: bool


@dataclass(frozen=True, slots=True)
class _ActionStyle:
    fill: str
    outline: str
    foreground: str
    width: int


@dataclass(frozen=True, slots=True)
class _CardStyle:
    fill: str
    outline: str
    width: int


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
        accent: str | None = None,
    ) -> None:
        super().__init__(master, bg=theme.CANVAS)
        accent = _section_accent(accent)

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
    """A compact service card with visible, keyboard-accessible actions."""

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
            takefocus=True,
            cursor="hand2",
        )
        self.service = service
        self._fonts = fonts
        self._icons = icons
        self._busy = busy
        self._on_details = on_details
        self._on_terminate = on_terminate
        self._on_open = on_open
        self._hovered = False
        self._focused = False
        self._hovered_action: str | None = None
        self._selected_action: str | None = None
        self._actions: list[_CanvasAction] = []
        # Where the uptime line sits and which canvas items draw it, so it can
        # be refreshed without touching the rest of the tile.
        self._uptime_at: tuple[float, float] | None = None
        self._uptime_items: tuple[int, int] | None = None

        self._background = theme.rounded_rect(
            self,
            theme.TILE_PAD,
            6,
            theme.TILE_SPAN - theme.TILE_PAD,
            theme.TILE_HEIGHT + theme.TILE_PAD + 2,
            theme.CARD_RADIUS,
            fill=theme.SURFACE,
            outline=theme.HAIRLINE,
            width=1,
        )
        self._draw()

        self.bind("<Enter>", self._on_enter)
        self.bind("<Leave>", self._on_leave)
        self.bind("<Motion>", self._on_motion)
        self.bind("<Button-1>", self._on_click)
        self.bind("<FocusIn>", self._on_focus_in)
        self.bind("<FocusOut>", self._on_focus_out)
        self.bind("<Return>", self._on_activate)
        self.bind("<space>", self._on_activate)
        self.bind("<Left>", lambda _event: self._cycle_action(-1))
        self.bind("<Right>", lambda _event: self._cycle_action(1))

    # ------------------------------------------------------------------ draw

    def _draw(self) -> None:
        service = self.service
        browser_url = service.browser_url()
        pad = theme.TILE_PAD
        card_left = pad
        card_right = theme.TILE_SPAN - pad
        content_x = card_left + 78
        accent = theme.CATEGORY_COLORS.get(service.category.value, theme.TEXT_MUTED)

        icon_left = card_left + 10
        icon_top = 20
        icon_right = icon_left + theme.ICON_WELL_SIZE
        icon_bottom = icon_top + theme.ICON_WELL_SIZE
        theme.rounded_rect(
            self,
            icon_left,
            icon_top,
            icon_right,
            icon_bottom,
            theme.ICON_WELL_RADIUS,
            fill=theme.SURFACE_ALT,
            outline="",
        )

        icon = self._icons.tech(service.tech, ICON_SIZE)
        if icon is not None:
            self.create_image(
                (icon_left + icon_right) / 2,
                (icon_top + icon_bottom) / 2,
                image=icon,
            )
        else:
            self.create_text(
                (icon_left + icon_right) / 2,
                (icon_top + icon_bottom) / 2,
                text=service.tech[:2].upper(),
                fill=accent,
                font=self._fonts["section"],
            )

        title_right = card_right - theme.SPACE_MD
        if service.can_terminate:
            title_right -= theme.CONTROL_SIZE + theme.SPACE_SM

        self.create_text(
            content_x,
            26,
            text=_ellipsis_to_width(
                service.display_name,
                title_right - content_x,
                tkfont.Font(root=self, font=self._fonts["tile_name"]).measure,
            ),
            fill=theme.TEXT if not self._busy else theme.TEXT_MUTED,
            font=self._fonts["tile_name"],
            anchor="w",
        )

        if self._busy:
            self.create_text(
                content_x,
                54,
                text="중지 중…",
                fill=theme.WARNING,
                font=self._fonts["tile_meta"],
                anchor="w",
            )
        else:
            self._draw_uptime(content_x, 54)

        self._draw_origin(card_right - theme.SPACE_MD, 54)
        self._draw_chips(content_x, 82, accent)

        details_hint = self.create_text(
            card_right - theme.SPACE_MD,
            110,
            text="↵ 상세",
            fill=theme.TEXT_MUTED,
            font=self._fonts["tile_meta"],
            anchor="e",
            state="hidden",
        )
        self._actions.append(
            _CanvasAction(
                key="details",
                bounds=(-1, -1, -1, -1),
                shape=None,
                visuals=((details_hint, "fill"),),
                enabled=True,
            )
        )
        if browser_url:
            self._draw_link_action(
                content_x,
                110,
                browser_url,
                max_width=card_right - content_x - 72,
            )
        if service.can_terminate:
            self._draw_power_action(card_right, disabled=self._busy)

    def _draw_origin(self, right: float, y: float) -> None:
        """A badge naming the tool that started the service, when one is known."""
        colour = _origin_colour(self.service.origin_kind)
        label = self.service.origin_label
        if colour is None or not label:
            return
        width = 24 + _text_width(label)
        left = right - width
        theme.rounded_rect(
            self,
            left,
            y - 9,
            right,
            y + 9,
            6,
            fill=theme.SURFACE_ALT,
            outline="",
        )
        self.create_oval(
            left + 7,
            y - 2,
            left + 11,
            y + 2,
            fill=colour,
            outline="",
        )
        self.create_text(
            left + 15,
            y,
            text=label,
            fill=theme.TEXT_MUTED,
            font=self._fonts["chip"],
            anchor="w",
        )

    def _draw_uptime(self, x: float, y: float) -> None:
        """How long the service has been up, as its own quiet line.

        A freshly started process is the one a developer is most likely to be
        looking for, so the first few minutes are tinted; after that the line
        recedes into the tile.
        """
        self._uptime_at = (x, y)
        process = self.service.process
        if process is None:
            return
        text = format_uptime_compact(process.uptime_seconds)
        if not text:
            return
        text = f"실행 {text}"
        colour = self._uptime_colour(process.uptime_seconds)
        dot = self.create_oval(x, y - 3, x + 6, y + 3, fill=colour, outline="")
        label = self.create_text(
            x + 11,
            y,
            text=text,
            fill=colour,
            font=self._fonts["tile_meta"],
            anchor="w",
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
        x, y = self._uptime_at
        process = self.service.process
        text = format_uptime_compact(process.uptime_seconds) if process else ""
        if not text:
            if self._uptime_items is not None:
                self.delete(*self._uptime_items)
                self._uptime_items = None
            return
        if self._uptime_items is None:  # nothing to show before, something now
            self._draw_uptime(x, y)
            return
        dot, label = self._uptime_items
        colour = self._uptime_colour(process.uptime_seconds if process else None)
        text = f"실행 {text}"
        self.coords(dot, x, y - 3, x + 6, y + 3)
        self.itemconfigure(dot, fill=colour)
        self.itemconfigure(label, text=text, fill=colour)

    def _draw_chips(self, x: float, y: float, accent: str) -> None:
        ports = self.service.unique_ports
        if not ports:
            self.create_text(
                x,
                y,
                text="포트 정보 없음",
                fill=theme.TEXT_DIM,
                font=self._fonts["tile_meta"],
                anchor="w",
            )
            return
        labels = _port_badge_labels(ports)

        self.create_oval(x, y - 2, x + 4, y + 2, fill=accent, outline="")
        x += 10
        font = self._fonts["chip"]
        widths = [max(34, 16 + _text_width(label)) for label in labels]
        for label, width in zip(labels, widths):
            theme.rounded_rect(
                self,
                x,
                y - 9,
                x + width,
                y + 9,
                6,
                fill=theme.SURFACE_ALT,
                outline="",
            )
            self.create_text(
                x + width / 2,
                y,
                text=label,
                fill=theme.TEXT_DIM if label.startswith("+") else theme.TEXT_MUTED,
                font=font,
            )
            x += width + 6

    def _draw_link_action(
        self,
        x: float,
        y: float,
        url: str,
        *,
        max_width: float,
    ) -> None:
        font = tkfont.Font(root=self, font=self._fonts["tile_meta"])
        label = _ellipsis_to_width(_browser_link_label(url), max_width, font.measure)
        width = max(1.0, float(font.measure(label)))
        bounds = (x - 4, y - 12, x + width + 4, y + 12)
        shape = theme.rounded_rect(
            self,
            *bounds,
            theme.SPACE_SM,
            fill=theme.SURFACE,
            outline="",
        )
        label_item = self.create_text(
            x,
            y,
            text=label,
            fill=theme.ACCENT,
            font=self._fonts["tile_meta"],
            anchor="w",
        )
        underline = self.create_line(
            x,
            y + 8,
            x + width,
            y + 8,
            fill=theme.ACCENT_HOVER,
            width=1,
            state="hidden",
        )
        self._actions.append(
            _CanvasAction(
                key="open",
                bounds=bounds,
                shape=shape,
                visuals=((label_item, "fill"), (underline, "fill")),
                enabled=True,
            )
        )

    def _draw_power_action(self, card_right: float, *, disabled: bool) -> None:
        hit_bounds, visual_bounds, centre = _power_action_geometry(card_right)
        cx, cy = centre
        icon_radius = theme.CONTROL_ICON_SIZE / 2
        shape = self.create_oval(
            *visual_bounds,
            fill=theme.SURFACE_ALT,
            outline="",
            width=1,
        )
        arc = self.create_arc(
            cx - icon_radius + 2,
            cy - icon_radius + 2,
            cx + icon_radius - 2,
            cy + icon_radius - 2,
            start=135,
            extent=270,
            style="arc",
            outline=theme.TEXT_DIM if disabled else theme.DANGER,
            width=2,
        )
        stem = self.create_line(
            cx,
            cy - icon_radius,
            cx,
            cy,
            fill=theme.TEXT_DIM if disabled else theme.DANGER,
            width=2,
            capstyle="round",
        )
        self._actions.append(
            _CanvasAction(
                key="terminate",
                bounds=hit_bounds,
                shape=shape,
                visuals=((arc, "outline"), (stem, "fill")),
                enabled=not disabled,
            )
        )

    # ----------------------------------------------------------- interaction

    def _on_enter(self, _event: tk.Event) -> None:
        self._hovered = True
        self._paint_card()
        self._paint_actions()

    def _on_leave(self, _event: tk.Event) -> None:
        self._hovered = False
        self._set_hovered_action(None)
        self._paint_card()
        self._paint_actions()

    def _on_motion(self, event: tk.Event) -> None:
        action = self._action_at(event.x, event.y)
        self._set_hovered_action(action.key if action is not None else None)

    def _on_click(self, event: tk.Event) -> None:
        self.focus_set()
        action = self._action_at(event.x, event.y)
        if action is not None:
            if action.enabled:
                self._selected_action = action.key
                self._paint_actions()
                self._invoke_action(action.key)
            return
        if self.service.browser_url() and event.state & 0x0004:  # Ctrl-click
            self._on_open(self.service)
            return
        self._selected_action = "details"
        self._paint_actions()
        self._on_details(self.service)

    def _in_power_zone(self, x: int, y: int) -> bool:
        action = self._action_at(x, y)
        return action is not None and action.enabled and action.key == "terminate"

    def _action_at(self, x: float, y: float) -> _CanvasAction | None:
        for action in self._actions:
            left, top, right, bottom = action.bounds
            if left <= x <= right and top <= y <= bottom:
                return action
        return None

    def _set_hovered_action(self, key: str | None) -> None:
        if key == self._hovered_action:
            return
        self._hovered_action = key
        self._paint_actions()

    def _paint_actions(self) -> None:
        for action in self._actions:
            hovered = action.enabled and action.key == self._hovered_action
            selected = action.enabled and self._focused and action.key == self._selected_action
            if action.shape is None:
                for item, _option in action.visuals:
                    self.itemconfigure(
                        item,
                        state="normal" if selected else "hidden",
                    )
                continue
            style = _action_style(
                action.key,
                enabled=action.enabled,
                hovered=hovered,
                selected=selected,
                card_hovered=self._hovered,
            )
            self.itemconfigure(
                action.shape,
                fill=style.fill,
                outline=style.outline,
                width=style.width,
            )
            for item, option in action.visuals:
                self.itemconfigure(item, **{option: style.foreground})
            if action.key == "open" and len(action.visuals) > 1:
                self.itemconfigure(
                    action.visuals[1][0],
                    state="normal" if hovered or selected else "hidden",
                )

    def _on_focus_in(self, _event: tk.Event) -> None:
        self._focused = True
        self._selected_action = next(
            (action.key for action in self._actions if action.enabled),
            None,
        )
        self._paint_card()
        self._paint_actions()

    def _on_focus_out(self, _event: tk.Event) -> None:
        self._focused = False
        self._paint_card()
        self._paint_actions()

    def _on_activate(self, _event: tk.Event) -> str:
        selected = self._selected_action or "details"
        self._invoke_action(selected)
        return "break"

    def _cycle_action(self, step: int) -> str:
        keys = tuple(action.key for action in self._actions if action.enabled)
        self._selected_action = _next_action_key(keys, self._selected_action, step)
        self._paint_actions()
        return "break"

    def _invoke_action(self, key: str) -> None:
        if key == "terminate":
            if not self._busy and self.service.can_terminate:
                self._on_terminate(self.service)
            return
        if key == "open":
            if self.service.browser_url():
                self._on_open(self.service)
            return
        self._on_details(self.service)

    def _paint_card(self) -> None:
        style = _card_style(hovered=self._hovered, focused=self._focused)
        self.itemconfigure(
            self._background,
            fill=style.fill,
            outline=style.outline,
            width=style.width,
        )


class ContainerTile(tk.Canvas):
    """A read-only Docker card whose surface opens its details."""

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
            takefocus=True,
            cursor="hand2",
        )
        self.container = container
        self._tech = tech
        self._fonts = fonts
        self._icons = icons
        self._on_details = on_details
        self._hovered = False
        self._focused = False

        self._background = theme.rounded_rect(
            self,
            theme.TILE_PAD,
            6,
            theme.TILE_SPAN - theme.TILE_PAD,
            theme.TILE_HEIGHT + theme.TILE_PAD + 2,
            theme.CARD_RADIUS,
            fill=theme.SURFACE,
            outline=theme.HAIRLINE,
            width=1,
        )
        self._draw()

        self.bind("<Enter>", self._on_enter)
        self.bind("<Leave>", self._on_leave)
        self.bind("<Button-1>", self._on_click)
        self.bind("<FocusIn>", self._on_focus_in)
        self.bind("<FocusOut>", self._on_focus_out)
        self.bind("<Return>", self._on_activate)
        self.bind("<space>", self._on_activate)
        self.bind("<Left>", self._keep_details_selected)
        self.bind("<Right>", self._keep_details_selected)

    def _draw(self) -> None:
        container = self.container
        pad = theme.TILE_PAD
        card_left = pad
        card_right = theme.TILE_SPAN - pad
        content_x = card_left + 78
        running = container.running
        accent = theme.OK if running else theme.TEXT_DIM

        icon_left = card_left + 10
        icon_top = 20
        icon_right = icon_left + theme.ICON_WELL_SIZE
        icon_bottom = icon_top + theme.ICON_WELL_SIZE
        theme.rounded_rect(
            self,
            icon_left,
            icon_top,
            icon_right,
            icon_bottom,
            theme.ICON_WELL_RADIUS,
            fill=theme.SURFACE_ALT,
            outline="",
        )

        icon = self._icons.tech(self._tech, ICON_SIZE)
        if icon is not None:
            self.create_image(
                (icon_left + icon_right) / 2,
                (icon_top + icon_bottom) / 2,
                image=icon,
            )
        else:
            self.create_text(
                (icon_left + icon_right) / 2,
                (icon_top + icon_bottom) / 2,
                text="DK",
                fill=accent,
                font=self._fonts["section"],
            )

        # A stopped container is still worth seeing, but it must not compete
        # with the running ones for attention.
        self.create_text(
            content_x,
            26,
            text=_ellipsis_to_width(
                container.name,
                card_right - theme.SPACE_MD - content_x,
                tkfont.Font(root=self, font=self._fonts["tile_name"]).measure,
            ),
            fill=theme.TEXT if running else theme.TEXT_MUTED,
            font=self._fonts["tile_name"],
            anchor="w",
        )

        state_text = "실행 중" if running else "중지됨"
        if container.status:
            state_text += f" · {_ellipsis(container.status, 22)}"
        self.create_oval(content_x, 51, content_x + 6, 57, fill=accent, outline="")
        self.create_text(
            content_x + 11,
            54,
            text=state_text,
            fill=accent,
            font=self._fonts["tile_meta"],
            anchor="w",
        )

        self._draw_port_chips(content_x, 82, theme.VIOLET if running else theme.TEXT_DIM)
        self._details_hint = self.create_text(
            card_right - theme.SPACE_MD,
            110,
            text="↵ 상세",
            fill=theme.TEXT_MUTED,
            font=self._fonts["tile_meta"],
            anchor="e",
            state="hidden",
        )

    def _draw_port_chips(self, x: float, y: float, accent: str) -> None:
        ports = self.container.ports
        if not ports:
            self.create_text(
                x,
                y,
                text="공개 포트 없음",
                fill=theme.TEXT_DIM,
                font=self._fonts["tile_meta"],
                anchor="w",
            )
            return
        labels = _port_badge_labels(ports)

        self.create_oval(x, y - 2, x + 4, y + 2, fill=accent, outline="")
        x += 10
        widths = [max(34, 16 + _text_width(label)) for label in labels]
        for label, width in zip(labels, widths):
            theme.rounded_rect(
                self,
                x,
                y - 9,
                x + width,
                y + 9,
                6,
                fill=theme.SURFACE_ALT,
                outline="",
            )
            self.create_text(
                x + width / 2,
                y,
                text=label,
                fill=theme.TEXT_DIM if label.startswith("+") else theme.TEXT_MUTED,
                font=self._fonts["chip"],
            )
            x += width + 6

    def _on_enter(self, _event: tk.Event) -> None:
        self._hovered = True
        self._paint_card()

    def _on_leave(self, _event: tk.Event) -> None:
        self._hovered = False
        self._paint_card()

    def _on_click(self, event: tk.Event) -> None:
        del event
        self.focus_set()
        self._on_details(self.container)

    def _on_activate(self, _event: tk.Event) -> str:
        self._on_details(self.container)
        return "break"

    def _keep_details_selected(self, _event: tk.Event) -> str:
        return "break"

    def _on_focus_in(self, _event: tk.Event) -> None:
        self._focused = True
        self._paint_card()
        self.itemconfigure(self._details_hint, state="normal")

    def _on_focus_out(self, _event: tk.Event) -> None:
        self._focused = False
        self._paint_card()
        self.itemconfigure(self._details_hint, state="hidden")

    def _paint_card(self) -> None:
        style = _card_style(hovered=self._hovered, focused=self._focused)
        self.itemconfigure(
            self._background,
            fill=style.fill,
            outline=style.outline,
            width=style.width,
        )


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


def _origin_colour(kind: str) -> str | None:
    """Resolve launcher colours after the active palette has been applied."""
    return {"agent": theme.VIOLET, "ide": theme.ACCENT}.get(kind)


def _port_badge_labels(ports: tuple[int, ...]) -> tuple[str, ...]:
    """Keep the quiet port row numeric, compact, and truthful."""
    if len(ports) > MAX_CHIPS:
        return str(ports[0]), f"+{len(ports) - 1}"
    return tuple(str(port) for port in ports)


def _browser_link_label(url: str) -> str:
    """Turn a browser URL into a compact destination without changing it."""
    value = url.strip()
    try:
        parsed = urlsplit(value)
        host = parsed.hostname
        port = parsed.port
    except ValueError:
        return value
    if not host:
        return value

    display_host = host
    normalized_host = host.casefold().rstrip(".")
    if normalized_host in {"localhost", "localhost.localdomain", "*", "+"} or normalized_host.endswith(
        ".localhost"
    ):
        display_host = "localhost"
    else:
        try:
            address = ip_address(normalized_host)
        except ValueError:
            address = None
        if address is not None and (address.is_loopback or address.is_unspecified):
            display_host = "localhost"
        elif ":" in display_host:
            display_host = f"[{display_host}]"

    destination = display_host
    if port is not None:
        destination += f":{port}"
    if parsed.path and parsed.path != "/":
        destination += parsed.path
    if parsed.query:
        destination += f"?{parsed.query}"
    if parsed.fragment:
        destination += f"#{parsed.fragment}"
    return destination


def _section_accent(accent: str | None) -> str:
    """Resolve the optional section accent at widget construction time."""
    return theme.TEXT_DIM if accent is None else accent


def _ellipsis_to_width(
    text: str,
    max_width: float,
    measure: Callable[[str], int],
) -> str:
    """Fit one line to a measured pixel width, preserving a visible ellipsis."""
    text = text.strip()
    if not text or max_width <= 0:
        return ""
    if measure(text) <= max_width:
        return text

    suffix = "…"
    if measure(suffix) > max_width:
        return ""
    low = 0
    high = len(text)
    while low < high:
        middle = (low + high + 1) // 2
        candidate = text[:middle].rstrip() + suffix
        if measure(candidate) <= max_width:
            low = middle
        else:
            high = middle - 1
    return text[:low].rstrip() + suffix


def _next_action_key(
    keys: tuple[str, ...],
    selected: str | None,
    step: int,
) -> str | None:
    """Cycle through enabled canvas actions without depending on a Tk display."""
    if not keys:
        return None
    if selected not in keys:
        return keys[0]
    index = keys.index(selected)
    return keys[(index + step) % len(keys)]


def _power_action_geometry(
    card_right: float,
) -> tuple[
    tuple[float, float, float, float],
    tuple[float, float, float, float],
    tuple[float, float],
]:
    """Return a 36 px hit target around the smaller circular control."""
    centre_x = card_right - theme.SPACE_MD - theme.CONTROL_HIT_SIZE / 2
    centre_y = 26.0
    hit_radius = theme.CONTROL_HIT_SIZE / 2
    visual_radius = theme.CONTROL_RADIUS
    return (
        (
            centre_x - hit_radius,
            centre_y - hit_radius,
            centre_x + hit_radius,
            centre_y + hit_radius,
        ),
        (
            centre_x - visual_radius,
            centre_y - visual_radius,
            centre_x + visual_radius,
            centre_y + visual_radius,
        ),
        (centre_x, centre_y),
    )


def _action_style(
    key: str,
    *,
    enabled: bool,
    hovered: bool,
    selected: bool,
    card_hovered: bool,
) -> _ActionStyle:
    """Resolve action states without relying on a live Tk display."""
    card_fill = theme.SURFACE_HOVER if card_hovered else theme.SURFACE
    if key == "terminate":
        if not enabled:
            return _ActionStyle(theme.SURFACE_ALT, "", theme.TEXT_DIM, 1)
        if selected:
            return _ActionStyle(theme.SURFACE_ALT, theme.ACCENT, theme.DANGER, 2)
        if hovered:
            return _ActionStyle(theme.SURFACE_HOVER, theme.BORDER, theme.DANGER, 1)
        return _ActionStyle(theme.SURFACE_ALT, "", theme.DANGER, 1)

    if hovered or selected:
        return _ActionStyle(card_fill, "", theme.ACCENT_HOVER, 1)
    return _ActionStyle(card_fill, "", theme.ACCENT, 1)


def _card_style(*, hovered: bool, focused: bool) -> _CardStyle:
    """Keep keyboard focus inside actions instead of selecting the card."""
    del focused
    if hovered:
        return _CardStyle(theme.SURFACE_HOVER, theme.BORDER, 1)
    return _CardStyle(theme.SURFACE, theme.HAIRLINE, 1)


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

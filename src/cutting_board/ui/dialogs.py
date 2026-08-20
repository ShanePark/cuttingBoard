from __future__ import annotations

import platform
import tkinter as tk
from collections.abc import Callable
from pathlib import Path

from cutting_board.constants import APP_NAME, APP_VERSION
from cutting_board.models import ServiceSnapshot
from cutting_board.presentation import format_bytes, format_cpu, format_duration
from cutting_board.scanner.docker import ContainerInfo
from cutting_board.ui import theme
from cutting_board.ui.icons import IconStore

# The scan interval offered to the user. Every value sits inside the range
# ``services.settings._clamp_interval`` allows, so a pick is always kept.
SCAN_INTERVAL_CHOICES: tuple[tuple[str, float], ...] = (
    ("1 sec", 1.0),
    ("2 sec", 2.0),
    ("5 sec", 5.0),
    ("10 sec", 10.0),
)

THEME_MODE_CHOICES: tuple[tuple[str, str], ...] = (
    ("System", "system"),
    ("Dark", "dark"),
    ("Light", "light"),
)

SCRIM_ALPHA = 0.26


class ContainerDetailDialog(tk.Toplevel):
    """Everything docker reports about one container."""

    def __init__(
        self,
        parent: tk.Misc,
        *,
        container: ContainerInfo,
        tech: str,
        fonts: dict[str, tuple[str, int, str]],
        icons: IconStore,
    ) -> None:
        backdrop = ModalBackdrop(parent)
        super().__init__(backdrop.window, bg=theme.CANVAS)
        self._modal_backdrop = backdrop
        self.container = container
        self._tech = tech
        self._fonts = fonts
        self._icons = icons

        self.title(container.name)
        self.resizable(False, False)
        self.transient(backdrop.window)

        self._build()

        self.update_idletasks()
        configure_detail_dismiss(self, backdrop)

    def _build(self) -> None:
        container = self.container
        accent = theme.OK if container.running else theme.TEXT_MUTED

        head = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=18)
        head.pack(fill="x")

        icon = self._icons.tech(self._tech, 96)
        if icon is not None:
            tk.Label(head, image=icon, bg=theme.SURFACE).pack(side="left", padx=(0, 18))
            self._icon_ref = icon

        titles = tk.Frame(head, bg=theme.SURFACE)
        titles.pack(side="left", fill="both", expand=True)
        tk.Label(
            titles,
            text=container.name,
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=(self._fonts["body_bold"][0], 15, "bold"),
            anchor="w",
        ).pack(fill="x")
        tk.Label(
            titles,
            text=container.state.upper(),
            bg=theme.SURFACE,
            fg=accent,
            font=self._fonts["label"],
            anchor="w",
        ).pack(fill="x", pady=(3, 0))
        if container.compose_project:
            tk.Label(
                titles,
                text=container.compose_project,
                bg=theme.SURFACE,
                fg=theme.TEXT_MUTED,
                font=self._fonts["small"],
                anchor="w",
            ).pack(fill="x", pady=(7, 0))

        tk.Frame(self, bg=theme.HAIRLINE, height=1).pack(fill="x")

        body = tk.Frame(self, bg=theme.CANVAS, padx=22, pady=18)
        body.pack(fill="both", expand=True)

        _section(body, "CONTAINER", self._fonts)
        _row(body, "Image", container.image, self._fonts, mono=True)
        _row(body, "ID", container.id, self._fonts, mono=True)
        _row(body, "Status", container.status or container.state, self._fonts)
        _row(body, "Created", container.created_at or "—", self._fonts)
        if container.compose_service:
            _row(body, "Service", container.compose_service, self._fonts)

        _section(body, "PORTS", self._fonts)
        if container.ports:
            for port in container.ports:
                _row(body, str(port), f"localhost:{port}", self._fonts, mono=True)
        else:
            _row(body, "—", "No published ports", self._fonts)

        bar = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=14)
        bar.pack(fill="x", side="bottom")
        close = tk.Label(
            bar,
            text="Close",
            bg=theme.SURFACE_ALT,
            fg=theme.TEXT_MUTED,
            font=self._fonts["small"],
            padx=16,
            pady=7,
            cursor="hand2",
        )
        close.bind("<Button-1>", lambda _event: self.destroy())
        close.pack(side="right")


def _section(parent: tk.Misc, title: str, fonts: dict[str, tuple[str, int, str]]) -> None:
    tk.Label(
        parent,
        text=title,
        bg=theme.CANVAS,
        fg=theme.TEXT_DIM,
        font=fonts["label"],
        anchor="w",
    ).pack(fill="x", pady=(16, 6))


def _row(
    parent: tk.Misc,
    label: str,
    value: str,
    fonts: dict[str, tuple[str, int, str]],
    *,
    mono: bool = False,
) -> None:
    row = tk.Frame(parent, bg=theme.CANVAS)
    row.pack(fill="x", pady=1)
    tk.Label(
        row,
        text=label,
        bg=theme.CANVAS,
        fg=theme.TEXT_DIM,
        font=fonts["small"],
        width=12,
        anchor="w",
    ).pack(side="left")
    tk.Label(
        row,
        text=value,
        bg=theme.CANVAS,
        fg=theme.TEXT,
        font=fonts["mono"] if mono else fonts["small"],
        anchor="w",
        justify="left",
        wraplength=430,
    ).pack(side="left", fill="x", expand=True)


def _centred_modal_geometry(
    *,
    parent_x: int,
    parent_y: int,
    parent_width: int,
    parent_height: int,
    dialog_width: int,
    dialog_height: int,
) -> str:
    """Return an exact parent-centred position in the global desktop space."""
    x, y = _centred_modal_position(
        parent_x=parent_x,
        parent_y=parent_y,
        parent_width=parent_width,
        parent_height=parent_height,
        dialog_width=dialog_width,
        dialog_height=dialog_height,
    )
    return f"{x:+d}{y:+d}"


def _centred_modal_position(
    *,
    parent_x: int,
    parent_y: int,
    parent_width: int,
    parent_height: int,
    dialog_width: int,
    dialog_height: int,
) -> tuple[int, int]:
    x = parent_x + (parent_width - dialog_width) // 2
    y = parent_y + (parent_height - dialog_height) // 2
    return x, y


class ModalBackdrop:
    """A dim modal scrim that physically blocks the parent client area."""

    def __init__(
        self,
        parent: tk.Misc,
        *,
        _window_factory: Callable[..., tk.Toplevel] | None = None,
    ) -> None:
        self.parent = parent.winfo_toplevel()
        factory = _window_factory or tk.Toplevel
        self.window = factory(self.parent, bg="#000000")
        self._dialog: tk.Toplevel | None = None
        self._on_outside: Callable[[], None] | None = None
        self._parent_bindings: dict[str, str] = {}
        self._closed = False

        self.window.withdraw()
        self.window.overrideredirect(True)
        self.window.transient(self.parent)
        self.window.attributes("-alpha", SCRIM_ALPHA)
        self.window.configure(cursor="arrow")
        self.window.bind("<Button-1>", self._handle_click, add="+")
        self.window.bind("<Destroy>", self._handle_backdrop_destroy, add="+")

    def activate(
        self,
        dialog: tk.Toplevel,
        *,
        on_outside: Callable[[], None] | None,
    ) -> None:
        """Show the scrim and layer the interactive dialog above it."""
        if self._closed:
            return
        self._dialog = dialog
        self._on_outside = on_outside
        self._bind_parent("<Configure>", self._handle_parent_configure)
        self._bind_parent("<Destroy>", self._handle_parent_destroy)
        dialog.bind("<Destroy>", self._handle_dialog_destroy, add="+")
        self._sync_scrim_geometry()
        self.window.deiconify()
        self.window.update_idletasks()
        try:
            self.window.wait_visibility()
        except tk.TclError:
            pass
        self.window.lift(self.parent)
        self._show_dialog()
        try:
            dialog.after_idle(self._show_dialog)
        except tk.TclError:
            pass

    def close(self) -> None:
        """Destroy the scrim and remove parent bindings exactly once."""
        if self._closed:
            return
        self._closed = True
        self._cleanup_parent_bindings()
        try:
            if self.window.winfo_exists():
                self.window.destroy()
        except tk.TclError:
            pass

    def _bind_parent(self, sequence: str, handler: Callable[[tk.Event], object]) -> None:
        binding_id = self.parent.bind(sequence, handler, add="+")
        if binding_id is not None:
            self._parent_bindings[sequence] = binding_id

    def _cleanup_parent_bindings(self) -> None:
        for sequence, binding_id in self._parent_bindings.items():
            try:
                self.parent.unbind(sequence, binding_id)
            except tk.TclError:
                pass
        self._parent_bindings.clear()

    def _handle_click(self, _event: tk.Event) -> str:
        if not self._closed and self._on_outside is not None:
            self._on_outside()
        return "break"

    def _handle_parent_configure(self, event: tk.Event) -> None:
        if getattr(event, "widget", None) is self.parent:
            self._sync_scrim_geometry()
            if self._dialog is not None:
                self._dialog.lift(self.window)
            self._position_dialog()

    def _handle_parent_destroy(self, event: tk.Event) -> None:
        if getattr(event, "widget", None) is self.parent:
            self.close()

    def _handle_dialog_destroy(self, event: tk.Event) -> None:
        if getattr(event, "widget", None) is self._dialog:
            self.close()

    def _handle_backdrop_destroy(self, event: tk.Event) -> None:
        if getattr(event, "widget", None) is self.window:
            self._closed = True
            self._cleanup_parent_bindings()

    def _sync_scrim_geometry(self) -> None:
        if self._closed:
            return
        try:
            width = max(1, self.parent.winfo_width())
            height = max(1, self.parent.winfo_height())
            x = self.parent.winfo_rootx()
            y = self.parent.winfo_rooty()
            self.window.geometry(f"{width}x{height}+{x}+{y}")
            self.window.lift(self.parent)
        except tk.TclError:
            pass

    def _position_dialog(self) -> None:
        if self._closed or self._dialog is None:
            return
        try:
            self._dialog.update_idletasks()
            dialog_width = self._dialog.winfo_width()
            dialog_height = self._dialog.winfo_height()
            target_x, target_y = _centred_modal_position(
                parent_x=self.parent.winfo_rootx(),
                parent_y=self.parent.winfo_rooty(),
                parent_width=self.parent.winfo_width(),
                parent_height=self.parent.winfo_height(),
                dialog_width=dialog_width,
                dialog_height=dialog_height,
            )
            self._dialog.geometry(f"{target_x:+d}{target_y:+d}")
            self._dialog.update_idletasks()

            # On macOS, wm geometry positions the outer frame while winfo_root*
            # reports the client area. Correct the title-bar inset after map.
            frame_x = self._dialog.winfo_x() + target_x - self._dialog.winfo_rootx()
            frame_y = self._dialog.winfo_y() + target_y - self._dialog.winfo_rooty()
            self._dialog.geometry(f"{frame_x:+d}{frame_y:+d}")
            self._dialog.update_idletasks()
        except tk.TclError:
            pass

    def _show_dialog(self) -> None:
        """Map and raise the modal after its withdrawn scrim is visible."""
        if self._closed or self._dialog is None:
            return
        try:
            self._dialog.deiconify()
            self._dialog.lift(self.window)
            self._position_dialog()
            self._dialog.focus_set()
        except tk.TclError:
            pass


def configure_detail_dismiss(dialog: tk.Toplevel, backdrop: ModalBackdrop) -> None:
    """Give a read-only detail popup Escape and scrim-click dismissal."""
    dialog.bind("<Escape>", lambda _event: dialog.destroy())
    backdrop.activate(dialog, on_outside=dialog.destroy)


class ServiceDetailDialog(tk.Toplevel):
    """Everything the scanner knows about one service.

    The board deliberately shows almost no text, so this is where the command
    line, working directory and per-endpoint detail live.
    """

    def __init__(
        self,
        parent: tk.Misc,
        *,
        service: ServiceSnapshot,
        fonts: dict[str, tuple[str, int, str]],
        icons: IconStore,
        on_open: Callable[[ServiceSnapshot], None],
        on_terminate: Callable[[ServiceSnapshot], None],
    ) -> None:
        backdrop = ModalBackdrop(parent)
        super().__init__(backdrop.window, bg=theme.CANVAS)
        self._modal_backdrop = backdrop
        self.service = service
        self._fonts = fonts
        self._icons = icons
        self._on_open = on_open
        self._on_terminate = on_terminate

        self.title(service.display_name)
        self.configure(padx=0, pady=0)
        self.resizable(False, False)
        self.transient(backdrop.window)

        self._build()

        self.update_idletasks()
        configure_detail_dismiss(self, backdrop)

    # --------------------------------------------------------------- layout

    def _build(self) -> None:
        service = self.service
        accent = theme.CATEGORY_COLORS.get(service.category.value, theme.TEXT_MUTED)

        head = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=18)
        head.pack(fill="x")

        icon = self._icons.tech(service.tech, 96)
        if icon is not None:
            tk.Label(head, image=icon, bg=theme.SURFACE).pack(side="left", padx=(0, 18))
            self._icon_ref = icon

        titles = tk.Frame(head, bg=theme.SURFACE)
        titles.pack(side="left", fill="both", expand=True)
        tk.Label(
            titles,
            text=service.display_name,
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=(self._fonts["body_bold"][0], 15, "bold"),
            anchor="w",
        ).pack(fill="x")
        tk.Label(
            titles,
            text=service.category.value.upper(),
            bg=theme.SURFACE,
            fg=accent,
            font=self._fonts["label"],
            anchor="w",
        ).pack(fill="x", pady=(3, 0))
        if service.project:
            tk.Label(
                titles,
                text=service.project.name,
                bg=theme.SURFACE,
                fg=theme.TEXT_MUTED,
                font=self._fonts["small"],
                anchor="w",
            ).pack(fill="x", pady=(7, 0))

        tk.Frame(self, bg=theme.HAIRLINE, height=1).pack(fill="x")

        body = tk.Frame(self, bg=theme.CANVAS, padx=22, pady=18)
        body.pack(fill="both", expand=True)

        self._section(body, "ENDPOINTS")
        for endpoint in service.endpoints:
            self._row(
                body,
                f"{endpoint.protocol} {endpoint.family}",
                f"{endpoint.display_address}:{endpoint.port}  ·  {endpoint.scope.value}",
            )

        process = service.process
        if process is not None:
            self._section(body, "PROCESS")
            self._row(body, "PID", str(process.pid))
            if process.ppid is not None:
                self._row(body, "PPID", str(process.ppid))
            self._row(body, "Name", process.name)
            self._row(body, "User", process.username or "—")
            if service.origin_label:
                self._row(body, "Started by", service.origin_label)
            self._row(body, "Uptime", format_duration(process.uptime_seconds))
            self._row(body, "CPU", format_cpu(process.cpu_percent))
            self._row(body, "Memory", format_bytes(process.memory_bytes))
            if process.cwd:
                self._row(body, "Working dir", process.cwd, mono=True)
            if process.executable:
                self._row(body, "Executable", process.executable, mono=True)

            self._section(body, "COMMAND")
            command = tk.Text(
                body,
                height=min(8, max(2, len(process.command_display) // 74 + 1)),
                width=74,
                bg=theme.SURFACE,
                fg=theme.TEXT_MUTED,
                font=self._fonts["mono"],
                relief="flat",
                wrap="char",
                padx=12,
                pady=10,
                highlightthickness=1,
                highlightbackground=theme.BORDER,
                insertbackground=theme.ACCENT,
            )
            command.insert("1.0", process.command_display or "—")
            command.configure(state="disabled")
            command.pack(fill="x", pady=(4, 0))
        else:
            self._section(body, "PROCESS")
            self._row(body, "Status", "Owning process unavailable")

        if service.warnings:
            self._section(body, "NOTES")
            for warning in service.warnings:
                self._row(body, "•", warning)

        self._build_actions()

    def _build_actions(self) -> None:
        bar = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=14)
        bar.pack(fill="x", side="bottom")

        self._button(bar, "Close", theme.TEXT_MUTED, self.destroy).pack(side="right")

        if self.service.can_terminate:
            self._button(
                bar,
                "Stop",
                theme.DANGER,
                self._terminate,
            ).pack(side="right", padx=(0, 8))

        if self.service.browser_url():
            self._button(
                bar,
                "Open in Browser",
                theme.ACCENT,
                lambda: self._on_open(self.service),
            ).pack(side="left")

    def _terminate(self) -> None:
        service = self.service
        self.destroy()
        self._on_terminate(service)

    # --------------------------------------------------------------- pieces

    def _section(self, parent: tk.Misc, title: str) -> None:
        tk.Label(
            parent,
            text=title,
            bg=theme.CANVAS,
            fg=theme.TEXT_DIM,
            font=self._fonts["label"],
            anchor="w",
        ).pack(fill="x", pady=(16, 6))

    def _row(self, parent: tk.Misc, label: str, value: str, *, mono: bool = False) -> None:
        row = tk.Frame(parent, bg=theme.CANVAS)
        row.pack(fill="x", pady=1)
        tk.Label(
            row,
            text=label,
            bg=theme.CANVAS,
            fg=theme.TEXT_DIM,
            font=self._fonts["small"],
            width=12,
            anchor="w",
        ).pack(side="left")
        tk.Label(
            row,
            text=value,
            bg=theme.CANVAS,
            fg=theme.TEXT,
            font=self._fonts["mono"] if mono else self._fonts["small"],
            anchor="w",
            justify="left",
            wraplength=430,
        ).pack(side="left", fill="x", expand=True)

    def _button(
        self,
        parent: tk.Misc,
        text: str,
        colour: str,
        command: Callable[[], None],
    ) -> tk.Label:
        button = tk.Label(
            parent,
            text=text,
            bg=theme.SURFACE_ALT,
            fg=colour,
            font=self._fonts["small"],
            padx=16,
            pady=7,
            cursor="hand2",
        )
        button.bind("<Button-1>", lambda _event: command())
        button.bind("<Enter>", lambda _event: button.configure(bg=theme.BORDER))
        button.bind("<Leave>", lambda _event: button.configure(bg=theme.SURFACE_ALT))
        return button

class ConfirmDialog(tk.Toplevel):
    """A themed yes/no modal.

    The Tk message box is drawn by the platform in its own grey palette, which
    is jarring against the board. This asks the same question in the board's
    own colours, with the brand mark of the service being acted on.
    """

    def __init__(
        self,
        parent: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        icons: IconStore,
        title: str,
        headline: str,
        meta: str,
        question: str,
        confirm_label: str,
        tech: str = "service",
    ) -> None:
        backdrop = ModalBackdrop(parent)
        super().__init__(backdrop.window, bg=theme.BORDER)
        self._modal_backdrop = backdrop
        self.confirmed = False
        self._fonts = fonts
        self._icons = icons

        self.title(title)
        self.resizable(False, False)
        self.transient(backdrop.window)
        self.configure(padx=1, pady=1)  # the 1px border is the parent background

        self._build(headline, meta, question, confirm_label, tech)

        self.bind("<Escape>", lambda _event: self._answer(False))
        self.bind("<Return>", lambda _event: self._answer(True))
        self.update_idletasks()
        backdrop.activate(self, on_outside=lambda: self._answer(False))
        self._confirm_button.focus_set()

    # --------------------------------------------------------------- layout

    def _build(
        self,
        headline: str,
        meta: str,
        question: str,
        confirm_label: str,
        tech: str,
    ) -> None:
        body = tk.Frame(self, bg=theme.SURFACE, padx=26, pady=24)
        body.pack(fill="both", expand=True)

        head = tk.Frame(body, bg=theme.SURFACE)
        head.pack(fill="x")

        icon = self._icons.tech(tech, 48)
        if icon is not None:
            tk.Label(head, image=icon, bg=theme.SURFACE).pack(side="left", padx=(0, 16))
            self._icon_ref = icon

        titles = tk.Frame(head, bg=theme.SURFACE)
        titles.pack(side="left", fill="both", expand=True)
        tk.Label(
            titles,
            text=headline,
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=(self._fonts["body_bold"][0], 13, "bold"),
            anchor="w",
        ).pack(fill="x")
        tk.Label(
            titles,
            text=meta,
            bg=theme.SURFACE,
            fg=theme.TEXT_DIM,
            font=self._fonts["tile_meta"],
            anchor="w",
        ).pack(fill="x", pady=(4, 0))

        tk.Label(
            body,
            text=question,
            bg=theme.SURFACE,
            fg=theme.TEXT_MUTED,
            font=self._fonts["small"],
            anchor="w",
            justify="left",
            wraplength=330,
        ).pack(fill="x", pady=(20, 0))

        actions = tk.Frame(body, bg=theme.SURFACE)
        actions.pack(fill="x", pady=(22, 0))

        self._confirm_button = self._action(
            actions,
            confirm_label,
            fg=theme.ON_ACCENT,
            bg=theme.DANGER,
            hover="#FF8CA0",
            command=lambda: self._answer(True),
        )
        self._confirm_button.pack(side="right")
        self._action(
            actions,
            "Cancel",
            fg=theme.TEXT_MUTED,
            bg=theme.SURFACE_ALT,
            hover=theme.BORDER,
            command=lambda: self._answer(False),
        ).pack(side="right", padx=(0, 8))

    def _action(
        self,
        parent: tk.Misc,
        text: str,
        *,
        fg: str,
        bg: str,
        hover: str,
        command: Callable[[], None],
    ) -> tk.Label:
        button = tk.Label(
            parent,
            text=text,
            bg=bg,
            fg=fg,
            font=self._fonts["body_bold"],
            padx=20,
            pady=9,
            cursor="hand2",
            highlightthickness=0,
            takefocus=True,
        )
        button.bind("<Button-1>", lambda _event: command())
        button.bind("<space>", lambda _event: command())
        button.bind("<Enter>", lambda _event: button.configure(bg=hover))
        button.bind("<Leave>", lambda _event: button.configure(bg=bg))
        return button

    # --------------------------------------------------------------- answer

    def _answer(self, confirmed: bool) -> None:
        self.confirmed = confirmed
        self.destroy()

def ask_confirmation(
    parent: tk.Misc,
    *,
    fonts: dict[str, tuple[str, int, str]],
    icons: IconStore,
    title: str,
    headline: str,
    meta: str,
    question: str,
    confirm_label: str,
    tech: str = "service",
) -> bool:
    """Show :class:`ConfirmDialog` and block until it is answered."""
    dialog = ConfirmDialog(
        parent,
        fonts=fonts,
        icons=icons,
        title=title,
        headline=headline,
        meta=meta,
        question=question,
        confirm_label=confirm_label,
        tech=tech,
    )
    parent.wait_window(dialog)
    return dialog.confirmed


class SettingsDialog(tk.Toplevel):
    """What the board is, and how often it looks.

    The window chrome deliberately carries no version string, so this is the
    one place the build, the interpreter and the settings file are named.
    """

    def __init__(
        self,
        parent: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        icons: IconStore,
        settings_path: Path,
        scan_interval_seconds: float,
        theme_mode: str,
        on_interval_change: Callable[[float], None],
        on_theme_change: Callable[[str], None],
    ) -> None:
        backdrop = ModalBackdrop(parent)
        super().__init__(backdrop.window, bg=theme.CANVAS)
        self._modal_backdrop = backdrop
        self._fonts = fonts
        self._icons = icons
        self._settings_path = settings_path
        self._interval = float(scan_interval_seconds)
        self._theme_mode = theme_mode
        self._on_interval_change = on_interval_change
        self._on_theme_change = on_theme_change
        self._chips: list[tuple[float, tk.Canvas, int, int]] = []
        self._theme_chips: list[tuple[str, tk.Canvas, int, int]] = []

        self.title("Settings")
        self.resizable(False, False)
        self.transient(backdrop.window)

        self._build()

        self.bind("<Escape>", lambda _event: self.destroy())
        self.update_idletasks()
        backdrop.activate(self, on_outside=self.destroy)

    # --------------------------------------------------------------- layout

    def _build(self) -> None:
        head = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=18)
        head.pack(fill="x")

        icon = self._icons.app(64)
        if icon is not None:
            tk.Label(head, image=icon, bg=theme.SURFACE).pack(side="left", padx=(0, 18))
            self._icon_ref = icon

        titles = tk.Frame(head, bg=theme.SURFACE)
        titles.pack(side="left", fill="both", expand=True)
        tk.Label(
            titles,
            text=APP_NAME,
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=(self._fonts["body_bold"][0], 15, "bold"),
            anchor="w",
        ).pack(fill="x")
        tk.Label(
            titles,
            text="SETTINGS",
            bg=theme.SURFACE,
            fg=theme.ACCENT,
            font=self._fonts["label"],
            anchor="w",
        ).pack(fill="x", pady=(3, 0))

        tk.Frame(self, bg=theme.HAIRLINE, height=1).pack(fill="x")

        body = tk.Frame(self, bg=theme.CANVAS, padx=22, pady=18)
        body.pack(fill="both", expand=True)

        _section(body, "INFORMATION", self._fonts)
        _row(body, "Version", APP_VERSION, self._fonts, mono=True)
        _row(body, "Python", platform.python_version(), self._fonts, mono=True)
        _row(body, "Platform", f"{platform.system()} {platform.release()}", self._fonts)
        _row(body, "Settings file", _home_relative(self._settings_path), self._fonts, mono=True)

        _section(body, "SETTINGS", self._fonts)
        self._build_interval_row(body)
        self._build_theme_row(body)

        bar = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=14)
        bar.pack(fill="x", side="bottom")
        close = tk.Label(
            bar,
            text="Close",
            bg=theme.SURFACE_ALT,
            fg=theme.TEXT_MUTED,
            font=self._fonts["small"],
            padx=16,
            pady=7,
            cursor="hand2",
        )
        close.bind("<Button-1>", lambda _event: self.destroy())
        close.pack(side="right")

    def _build_interval_row(self, parent: tk.Misc) -> None:
        row = tk.Frame(parent, bg=theme.CANVAS)
        row.pack(fill="x", pady=(4, 2))
        tk.Label(
            row,
            text="Scan interval",
            bg=theme.CANVAS,
            fg=theme.TEXT_DIM,
            font=self._fonts["small"],
            width=12,
            anchor="w",
        ).pack(side="left")

        for label, value in SCAN_INTERVAL_CHOICES:
            width = 24 + _chip_text_width(label)
            chip = tk.Canvas(
                row,
                width=width,
                height=24,
                bg=theme.CANVAS,
                highlightthickness=0,
                bd=0,
                cursor="hand2",
            )
            chip.pack(side="left", padx=(0, 6))
            shape = theme.rounded_rect(
                chip,
                1,
                1,
                width - 1,
                23,
                7,
                fill=theme.SURFACE_ALT,
                outline=theme.BORDER,
            )
            text = chip.create_text(
                width / 2,
                12,
                text=label,
                fill=theme.TEXT_MUTED,
                font=self._fonts["small"],
            )

            def choose_interval(_event: tk.Event, choice: float = value) -> None:
                self._choose_interval(choice)

            chip.bind("<Button-1>", choose_interval)
            self._chips.append((value, chip, shape, text))

        self._paint_chips()

    def _build_theme_row(self, parent: tk.Misc) -> None:
        row = tk.Frame(parent, bg=theme.CANVAS)
        row.pack(fill="x", pady=(10, 2))
        tk.Label(
            row,
            text="Theme",
            bg=theme.CANVAS,
            fg=theme.TEXT_DIM,
            font=self._fonts["small"],
            width=12,
            anchor="w",
        ).pack(side="left")

        for label, value in THEME_MODE_CHOICES:
            width = 24 + _chip_text_width(label)
            chip = tk.Canvas(
                row,
                width=width,
                height=24,
                bg=theme.CANVAS,
                highlightthickness=0,
                bd=0,
                cursor="hand2",
            )
            chip.pack(side="left", padx=(0, 6))
            shape = theme.rounded_rect(
                chip,
                1,
                1,
                width - 1,
                23,
                7,
                fill=theme.SURFACE_ALT,
                outline=theme.BORDER,
            )
            text = chip.create_text(
                width / 2,
                12,
                text=label,
                fill=theme.TEXT_MUTED,
                font=self._fonts["small"],
            )

            def choose_theme(_event: tk.Event, choice: str = value) -> None:
                self._choose_theme(choice)

            chip.bind("<Button-1>", choose_theme)
            self._theme_chips.append((value, chip, shape, text))

        self._paint_theme_chips()

    # --------------------------------------------------------------- choice

    def _choose_interval(self, seconds: float) -> None:
        if seconds == self._interval:
            return
        self._interval = seconds
        self._paint_chips()
        self._on_interval_change(seconds)

    def _choose_theme(self, mode: str) -> None:
        if mode == self._theme_mode:
            return
        self._theme_mode = mode
        self.destroy()
        self._on_theme_change(mode)

    def _paint_chips(self) -> None:
        for value, chip, shape, text in self._chips:
            selected = value == self._interval
            chip.itemconfigure(shape, outline=theme.ACCENT if selected else theme.BORDER)
            chip.itemconfigure(text, fill=theme.ACCENT if selected else theme.TEXT_MUTED)

    def _paint_theme_chips(self) -> None:
        for value, chip, shape, text in self._theme_chips:
            selected = value == self._theme_mode
            chip.itemconfigure(shape, outline=theme.ACCENT if selected else theme.BORDER)
            chip.itemconfigure(text, fill=theme.ACCENT if selected else theme.TEXT_MUTED)


def _home_relative(path: Path) -> str:
    """A path with the home directory folded back into ``~``.

    The settings file always lives under the user's home, and the untruncated
    absolute form wraps over several lines in the dialog.
    """
    try:
        return f"~/{path.relative_to(Path.home())}"
    except ValueError:
        return str(path)


def _chip_text_width(label: str) -> int:
    """Rough pixel width of a short chip label, allowing for wider glyphs."""
    return int(sum(11 if character > "\u007f" else 6 for character in label))

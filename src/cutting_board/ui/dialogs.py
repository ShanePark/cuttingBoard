from __future__ import annotations

import platform
import tkinter as tk
from collections.abc import Callable
from pathlib import Path

from cutting_board.constants import APP_NAME, APP_VERSION
from cutting_board.models import ServiceSnapshot
from cutting_board.scanner.docker import ContainerInfo
from cutting_board.presentation import format_bytes, format_cpu, format_duration
from cutting_board.ui import theme
from cutting_board.ui.icons import IconStore

# The scan interval offered to the user. Every value sits inside the range
# ``services.settings._clamp_interval`` allows, so a pick is always kept.
SCAN_INTERVAL_CHOICES: tuple[tuple[str, float], ...] = (
    ("1초", 1.0),
    ("2초", 2.0),
    ("5초", 5.0),
    ("10초", 10.0),
)


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
        super().__init__(parent, bg=theme.CANVAS)
        self.container = container
        self._tech = tech
        self._fonts = fonts
        self._icons = icons

        self.title(container.name)
        self.resizable(False, False)
        self.transient(parent.winfo_toplevel())

        self._build()

        self.bind("<Escape>", lambda _event: self.destroy())
        self.update_idletasks()
        _centre_on(self, parent)
        self.grab_set()
        self.focus_set()
        dismiss_on_outside_click(self, self.destroy)

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
        _row(body, "이미지", container.image, self._fonts, mono=True)
        _row(body, "ID", container.id, self._fonts, mono=True)
        _row(body, "상태", container.status or container.state, self._fonts)
        _row(body, "생성", container.created_at or "—", self._fonts)
        if container.compose_service:
            _row(body, "서비스", container.compose_service, self._fonts)

        _section(body, "PORTS", self._fonts)
        if container.ports:
            for port in container.ports:
                _row(body, str(port), f"localhost:{port}", self._fonts, mono=True)
        else:
            _row(body, "—", "게시된 포트 없음", self._fonts)

        bar = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=14)
        bar.pack(fill="x", side="bottom")
        close = tk.Label(
            bar,
            text="닫기",
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


def _centre_on(dialog: tk.Toplevel, parent: tk.Misc) -> None:
    top = parent.winfo_toplevel()
    x = top.winfo_rootx() + (top.winfo_width() - dialog.winfo_width()) // 2
    y = top.winfo_rooty() + (top.winfo_height() - dialog.winfo_height()) // 3
    dialog.geometry(f"+{max(0, x)}+{max(0, y)}")


def dismiss_on_outside_click(dialog: tk.Toplevel, on_dismiss: Callable[[], None]) -> None:
    """Close `dialog` when the pointer is pressed anywhere outside it.

    The dialog holds an application grab, so a press over the board behind it
    is redirected here rather than reaching the board. The pointer's root
    coordinates are compared against the dialog's own rectangle, because the
    widget the event names is the grab holder and not the widget that was
    actually under the pointer.
    """

    def handle(event: tk.Event) -> None:
        left = dialog.winfo_rootx()
        top = dialog.winfo_rooty()
        inside_x = left <= event.x_root < left + dialog.winfo_width()
        inside_y = top <= event.y_root < top + dialog.winfo_height()
        if not (inside_x and inside_y):
            on_dismiss()

    dialog.bind("<Button-1>", handle, add="+")


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
        super().__init__(parent, bg=theme.CANVAS)
        self.service = service
        self._fonts = fonts
        self._icons = icons
        self._on_open = on_open
        self._on_terminate = on_terminate

        self.title(service.display_name)
        self.configure(padx=0, pady=0)
        self.resizable(False, False)
        self.transient(parent.winfo_toplevel())

        self._build()

        self.bind("<Escape>", lambda _event: self.destroy())
        self.update_idletasks()
        self._centre_on(parent)
        self.grab_set()
        self.focus_set()
        dismiss_on_outside_click(self, self.destroy)

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
            self._row(body, "이름", process.name)
            self._row(body, "사용자", process.username or "—")
            if service.origin_label:
                self._row(body, "실행 주체", service.origin_label)
            self._row(body, "실행 시간", format_duration(process.uptime_seconds))
            self._row(body, "CPU", format_cpu(process.cpu_percent))
            self._row(body, "메모리", format_bytes(process.memory_bytes))
            if process.cwd:
                self._row(body, "작업 경로", process.cwd, mono=True)
            if process.executable:
                self._row(body, "실행 파일", process.executable, mono=True)

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
            self._row(body, "상태", "소유 프로세스를 확인할 수 없음")

        if service.warnings:
            self._section(body, "NOTES")
            for warning in service.warnings:
                self._row(body, "•", warning)

        self._build_actions()

    def _build_actions(self) -> None:
        bar = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=14)
        bar.pack(fill="x", side="bottom")

        self._button(bar, "닫기", theme.TEXT_MUTED, self.destroy).pack(side="right")

        if self.service.can_terminate:
            self._button(
                bar,
                "종료",
                theme.DANGER,
                self._terminate,
            ).pack(side="right", padx=(0, 8))

        if self.service.browser_url():
            self._button(
                bar,
                "브라우저에서 열기",
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

    def _centre_on(self, parent: tk.Misc) -> None:
        top = parent.winfo_toplevel()
        x = top.winfo_rootx() + (top.winfo_width() - self.winfo_width()) // 2
        y = top.winfo_rooty() + (top.winfo_height() - self.winfo_height()) // 3
        self.geometry(f"+{max(0, x)}+{max(0, y)}")


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
        super().__init__(parent, bg=theme.BORDER)
        self.confirmed = False
        self._fonts = fonts
        self._icons = icons

        self.title(title)
        self.resizable(False, False)
        self.transient(parent.winfo_toplevel())
        self.configure(padx=1, pady=1)  # the 1px border is the parent background

        self._build(headline, meta, question, confirm_label, tech)

        self.bind("<Escape>", lambda _event: self._answer(False))
        self.bind("<Return>", lambda _event: self._answer(True))
        self.update_idletasks()
        self._centre_on(parent)
        self.grab_set()
        self._confirm_button.focus_set()
        # Clicking away from a question is a refusal, never a confirmation.
        dismiss_on_outside_click(self, lambda: self._answer(False))

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
            fg=theme.CANVAS,
            bg=theme.DANGER,
            hover="#FF8CA0",
            command=lambda: self._answer(True),
        )
        self._confirm_button.pack(side="right")
        self._action(
            actions,
            "취소",
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

    def _centre_on(self, parent: tk.Misc) -> None:
        top = parent.winfo_toplevel()
        x = top.winfo_rootx() + (top.winfo_width() - self.winfo_width()) // 2
        y = top.winfo_rooty() + (top.winfo_height() - self.winfo_height()) // 3
        self.geometry(f"+{max(0, x)}+{max(0, y)}")


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
        on_interval_change: Callable[[float], None],
    ) -> None:
        super().__init__(parent, bg=theme.CANVAS)
        self._fonts = fonts
        self._icons = icons
        self._settings_path = settings_path
        self._interval = float(scan_interval_seconds)
        self._on_interval_change = on_interval_change
        self._chips: list[tuple[float, tk.Canvas, int, int]] = []

        self.title("설정")
        self.resizable(False, False)
        self.transient(parent.winfo_toplevel())

        self._build()

        self.bind("<Escape>", lambda _event: self.destroy())
        self.update_idletasks()
        _centre_on(self, parent)
        self.grab_set()
        self.focus_set()
        dismiss_on_outside_click(self, self.destroy)

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

        _section(body, "정보", self._fonts)
        _row(body, "버전", APP_VERSION, self._fonts, mono=True)
        _row(body, "파이썬", platform.python_version(), self._fonts, mono=True)
        _row(body, "플랫폼", f"{platform.system()} {platform.release()}", self._fonts)
        _row(body, "설정 파일", _home_relative(self._settings_path), self._fonts, mono=True)

        _section(body, "설정", self._fonts)
        self._build_interval_row(body)

        bar = tk.Frame(self, bg=theme.SURFACE, padx=22, pady=14)
        bar.pack(fill="x", side="bottom")
        close = tk.Label(
            bar,
            text="닫기",
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
            text="스캔 주기",
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
            chip.bind("<Button-1>", lambda _event, choice=value: self._choose_interval(choice))
            self._chips.append((value, chip, shape, text))

        self._paint_chips()

    # --------------------------------------------------------------- choice

    def _choose_interval(self, seconds: float) -> None:
        if seconds == self._interval:
            return
        self._interval = seconds
        self._paint_chips()
        self._on_interval_change(seconds)

    def _paint_chips(self) -> None:
        for value, chip, shape, text in self._chips:
            selected = value == self._interval
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
    """Rough pixel width of a short chip label, allowing for wider Hangul."""
    return int(sum(11 if character > "\u007f" else 6 for character in label))

from __future__ import annotations

import tkinter as tk
from collections.abc import Callable, Sequence
from dataclasses import dataclass

from cutting_board.ui import theme
from cutting_board.ui.widgets import ScrollArea


@dataclass(frozen=True, slots=True)
class LaunchTaskView:
    """Display-only state for one configured task.

    The launch controller owns process state. This small view model keeps the
    Tk layer independent from that controller and makes ownership explicit:
    an externally detected process can be shown without exposing a stop action.
    """

    name: str
    cwd: str
    command: str
    state: str = "stopped"
    expected_port: int | None = None
    message: str | None = None
    can_start: bool = True
    can_stop: bool = False
    external: bool = False


@dataclass(frozen=True, slots=True)
class LaunchProfileView:
    """Display-only state for one registered launch profile."""

    id: str
    name: str
    project_root: str
    tasks: tuple[LaunchTaskView, ...]
    can_start: bool = True
    can_stop: bool = False
    can_edit: bool = True
    can_delete: bool = True


@dataclass(frozen=True, slots=True)
class LaunchProfileCallbacks:
    """Actions supplied by the window/controller integration layer."""

    on_add: Callable[[], None]
    on_start_profile: Callable[[str], None]
    on_stop_profile: Callable[[str], None]
    on_edit_profile: Callable[[str], None]
    on_delete_profile: Callable[[str], None]
    on_start_task: Callable[[str, str], None]
    on_stop_task: Callable[[str, str], None]
    on_show_logs: Callable[[str, str], None]


@dataclass(frozen=True, slots=True)
class StatePresentation:
    label: str
    colour: str


@dataclass(frozen=True, slots=True)
class ContextAction:
    key: str
    label: str


@dataclass(frozen=True, slots=True)
class ActionInsets:
    horizontal: int
    vertical: int


PROFILE_BORDER_WIDTH = 1
PROFILE_INSET = (18, 16)
TASK_BORDER_WIDTH = 1
TASK_INSET = (14, 11)


_STATE_LABELS: dict[str, str] = {
    "stopped": "Stopped",
    "starting": "Starting",
    "running": "Running",
    "stopping": "Stopping",
    "failed": "Failed",
    "external": "Running externally",
}


def state_presentation(state: str, *, external: bool = False) -> StatePresentation:
    """Return the English label and colour used for a task state."""
    key = "external" if external else state.strip().lower()
    colours = {
        "stopped": theme.TEXT_DIM,
        "starting": theme.WARNING,
        "running": theme.OK,
        "stopping": theme.WARNING,
        "failed": theme.DANGER,
        "external": theme.VIOLET,
    }
    return StatePresentation(
        _STATE_LABELS.get(key, state or "Checking status"),
        colours.get(key, theme.TEXT_MUTED),
    )


def profile_primary_action(profile: LaunchProfileView) -> ContextAction | None:
    """Choose the one group action that best matches the current profile state."""
    if profile.can_start:
        label = "▶ Start Remaining" if profile.can_stop else "▶ Start All"
        return ContextAction("start", label)
    if profile.can_stop:
        return ContextAction("stop", "■ Stop All")
    return None


def task_primary_action(task: LaunchTaskView) -> ContextAction | None:
    """Show at most one lifecycle action for a task."""
    if task.can_stop:
        return ContextAction("stop", "■ Stop")
    if task.can_start:
        return ContextAction("start", "▶ Start")
    return None


def _middle_ellipsis(value: str, max_characters: int) -> str:
    """Preserve both ends of long paths and commands within a stable budget."""
    if max_characters < 5 or len(value) <= max_characters:
        return value
    left = (max_characters - 1) // 2
    right = max_characters - left - 1
    return f"{value[:left]}…{value[-right:]}"


def _task_count_label(count: int) -> str:
    """Format a task count with the correct singular or plural noun."""
    noun = "Task" if count == 1 else "Tasks"
    return f"{count} {noun}"


def _profiles_changed(
    previous: tuple[LaunchProfileView, ...],
    current: tuple[LaunchProfileView, ...],
    *,
    rendered: bool,
) -> bool:
    """Return whether a panel needs a destructive card rebuild."""
    return not rendered or previous != current


def _action_insets(*, compact: bool) -> ActionInsets:
    """Keep controls compact while preserving a comfortable pointer target."""
    return ActionInsets(9, 4) if compact else ActionInsets(13, 7)


class ActionButton(tk.Label):
    """A themed action that works with pointer, Return, and Space."""

    def __init__(
        self,
        master: tk.Misc,
        *,
        text: str,
        fonts: dict[str, tuple[str, int, str]],
        command: Callable[[], None],
        foreground: str | None = None,
        background: str | None = None,
        hover: str | None = None,
        enabled: bool = True,
        compact: bool = False,
    ) -> None:
        foreground = theme.TEXT if foreground is None else foreground
        background = theme.SURFACE_ALT if background is None else background
        hover = theme.SURFACE_HOVER if hover is None else hover
        self._command = command
        self._normal_bg = background
        self._hover_bg = hover
        self._enabled = enabled
        insets = _action_insets(compact=compact)
        super().__init__(
            master,
            text=text,
            bg=background if enabled else theme.SURFACE,
            fg=foreground if enabled else theme.TEXT_DIM,
            font=fonts["small"] if compact else fonts["body_bold"],
            padx=insets.horizontal,
            pady=insets.vertical,
            cursor="hand2" if enabled else "arrow",
            takefocus=enabled,
            highlightthickness=1,
            highlightbackground=background if enabled else theme.SURFACE,
            highlightcolor=theme.ACCENT,
        )
        self.bind("<Button-1>", self._activate)
        self.bind("<Return>", self._activate)
        self.bind("<space>", self._activate)
        self.bind("<Enter>", self._enter)
        self.bind("<Leave>", self._leave)

    def _activate(self, _event: tk.Event) -> str:
        if self._enabled:
            self._command()
        return "break"

    def _enter(self, _event: tk.Event) -> None:
        if self._enabled:
            self.configure(bg=self._hover_bg)

    def _leave(self, _event: tk.Event) -> None:
        if self._enabled:
            self.configure(bg=self._normal_bg)


class LaunchProfilesPanel(tk.Frame):
    """Scrollable registered-profile surface for the ``Launch Profiles`` tab."""

    def __init__(
        self,
        master: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        callbacks: LaunchProfileCallbacks,
    ) -> None:
        super().__init__(master, bg=theme.CANVAS)
        self._fonts = fonts
        self._callbacks = callbacks
        self._profiles: tuple[LaunchProfileView, ...] = ()
        self._rendered = False
        self._build_header()
        self._scroll = ScrollArea(self)
        self._scroll.pack(fill="both", expand=True)

    @property
    def profiles(self) -> tuple[LaunchProfileView, ...]:
        return self._profiles

    def render(self, profiles: Sequence[LaunchProfileView]) -> None:
        """Replace profile cards with a new immutable snapshot."""
        next_profiles = tuple(profiles)
        if not _profiles_changed(self._profiles, next_profiles, rendered=self._rendered):
            return
        self._profiles = next_profiles
        self._rendered = True
        for child in self._scroll.body.winfo_children():
            child.destroy()
        if not self._profiles:
            self._build_empty_state()
            return
        for profile in self._profiles:
            ProfileCard(
                self._scroll.body,
                fonts=self._fonts,
                profile=profile,
                callbacks=self._callbacks,
            ).pack(fill="x", padx=18, pady=(10, 4))

    def _build_header(self) -> None:
        header = tk.Frame(self, bg=theme.CANVAS, padx=20, pady=14)
        header.pack(fill="x")
        copy = tk.Frame(header, bg=theme.CANVAS)
        copy.pack(side="left", fill="x", expand=True)
        tk.Label(
            copy,
            text="Launch Profiles",
            bg=theme.CANVAS,
            fg=theme.TEXT,
            font=self._fonts["section"],
            anchor="w",
        ).pack(fill="x")
        tk.Label(
            copy,
            text="Run backend, frontend, and auto-build tasks together.",
            bg=theme.CANVAS,
            fg=theme.TEXT_MUTED,
            font=self._fonts["small"],
            anchor="w",
        ).pack(fill="x", pady=(3, 0))
        ActionButton(
            header,
            text="＋ Add",
            fonts=self._fonts,
            command=self._callbacks.on_add,
            foreground=theme.ON_ACCENT,
            background=theme.ACCENT,
            hover=theme.ACCENT_HOVER,
        ).pack(side="right", padx=(14, 0))

    def _build_empty_state(self) -> None:
        empty = tk.Frame(self._scroll.body, bg=theme.SURFACE, padx=28, pady=34)
        empty.pack(fill="x", padx=18, pady=18)
        tk.Label(
            empty,
            text="No launch profiles yet",
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=self._fonts["section"],
        ).pack()
        tk.Label(
            empty,
            text="Add a project and run commands to start and stop them together without an IDE.",
            bg=theme.SURFACE,
            fg=theme.TEXT_MUTED,
            font=self._fonts["body"],
        ).pack(pady=(7, 15))
        ActionButton(
            empty,
            text="Add First Profile",
            fonts=self._fonts,
            command=self._callbacks.on_add,
            foreground=theme.ACCENT,
        ).pack()


class ProfileCard(tk.Frame):
    """One profile and its individually controllable tasks."""

    def __init__(
        self,
        master: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        profile: LaunchProfileView,
        callbacks: LaunchProfileCallbacks,
    ) -> None:
        super().__init__(
            master,
            bg=theme.HAIRLINE,
            bd=0,
            highlightthickness=0,
            padx=PROFILE_BORDER_WIDTH,
            pady=PROFILE_BORDER_WIDTH,
        )
        self._fonts = fonts
        self._profile = profile
        self._callbacks = callbacks
        self._content = tk.Frame(
            self,
            bg=theme.SURFACE,
            padx=PROFILE_INSET[0],
            pady=PROFILE_INSET[1],
        )
        self._content.pack(fill="both", expand=True)
        self._build()

    def _build(self) -> None:
        profile = self._profile
        header = tk.Frame(self._content, bg=theme.SURFACE)
        header.pack(fill="x")
        titles = tk.Frame(header, bg=theme.SURFACE)
        titles.pack(fill="x", expand=True)
        title_line = tk.Frame(titles, bg=theme.SURFACE)
        title_line.pack(fill="x")
        tk.Label(
            title_line,
            text=profile.name,
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=self._fonts["section"],
            anchor="w",
        ).pack(side="left", fill="x", expand=True)
        tk.Label(
            title_line,
            text=_task_count_label(len(profile.tasks)),
            bg=theme.SURFACE,
            fg=theme.TEXT_DIM,
            font=self._fonts["small"],
            anchor="e",
        ).pack(side="right", padx=(12, 0))
        tk.Label(
            titles,
            text=_middle_ellipsis(profile.project_root, 120),
            bg=theme.SURFACE,
            fg=theme.TEXT_DIM,
            font=self._fonts["mono"],
            anchor="w",
            justify="left",
            wraplength=460,
        ).pack(fill="x", pady=(3, 0))

        actions = tk.Frame(self._content, bg=theme.SURFACE)
        actions.pack(fill="x", pady=(14, 0))
        primary = profile_primary_action(profile)
        if primary is not None:
            if primary.key == "start":
                command = lambda: self._callbacks.on_start_profile(profile.id)
            else:
                command = lambda: self._callbacks.on_stop_profile(profile.id)
            self._button(
                actions,
                primary.label,
                command,
                primary=True,
                enabled=True,
            ).pack(side="left")

        secondary = tk.Frame(actions, bg=theme.SURFACE)
        secondary.pack(side="right")
        if profile.can_edit:
            self._button(
                secondary,
                "Edit",
                lambda: self._callbacks.on_edit_profile(profile.id),
                enabled=True,
            ).pack(side="left")
        if profile.can_delete:
            self._button(
                secondary,
                "Delete",
                lambda: self._callbacks.on_delete_profile(profile.id),
                colour=theme.DANGER,
                enabled=True,
            ).pack(side="left", padx=(8, 0))

        separator = tk.Frame(self._content, bg=theme.HAIRLINE, height=1)
        separator.pack(fill="x", pady=(14, 5))
        for task in profile.tasks:
            TaskRow(
                self._content,
                fonts=self._fonts,
                profile_id=profile.id,
                task=task,
                callbacks=self._callbacks,
            ).pack(fill="x", pady=(6, 0))

    def _button(
        self,
        parent: tk.Misc,
        label: str,
        command: Callable[[], None],
        *,
        colour: str | None = None,
        primary: bool = False,
        enabled: bool,
    ) -> ActionButton:
        foreground = theme.ON_ACCENT if primary else theme.TEXT_MUTED if colour is None else colour
        return ActionButton(
            parent,
            text=label,
            fonts=self._fonts,
            command=command,
            foreground=foreground,
            background=theme.ACCENT if primary else theme.SURFACE,
            hover=theme.ACCENT_HOVER if primary else theme.SURFACE_HOVER,
            enabled=enabled,
            compact=True,
        )


class TaskRow(tk.Frame):
    """Readable status and explicit controls for one configured task."""

    def __init__(
        self,
        master: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        profile_id: str,
        task: LaunchTaskView,
        callbacks: LaunchProfileCallbacks,
    ) -> None:
        super().__init__(
            master,
            bg=theme.HAIRLINE,
            bd=0,
            highlightthickness=0,
            padx=TASK_BORDER_WIDTH,
            pady=TASK_BORDER_WIDTH,
        )
        self._fonts = fonts
        self._profile_id = profile_id
        self._task = task
        self._callbacks = callbacks
        self._content = tk.Frame(
            self,
            bg=theme.SURFACE_ALT,
            padx=TASK_INSET[0],
            pady=TASK_INSET[1],
        )
        self._content.pack(fill="both", expand=True)
        self._build()

    def _build(self) -> None:
        task = self._task
        presentation = state_presentation(task.state, external=task.external)

        identity = tk.Frame(self._content, bg=theme.SURFACE_ALT)
        identity.pack(fill="x")
        title = tk.Frame(identity, bg=theme.SURFACE_ALT)
        title.pack(fill="x")
        name = tk.Label(
            title,
            text=_middle_ellipsis(task.name, 72),
            bg=theme.SURFACE_ALT,
            fg=theme.TEXT,
            font=self._fonts["body_bold"],
            anchor="w",
            justify="left",
            wraplength=460,
        )
        name.pack(fill="x")

        status = tk.Frame(identity, bg=theme.SURFACE_ALT)
        status.pack(fill="x", pady=(4, 0))
        tk.Label(
            status,
            text=f"● {presentation.label}",
            bg=theme.SURFACE_ALT,
            fg=presentation.colour,
            font=self._fonts["small"],
            anchor="w",
        ).pack(side="left")
        if task.expected_port is not None:
            tk.Label(
                status,
                text=f"localhost:{task.expected_port}",
                bg=theme.SURFACE_ALT,
                fg=theme.ACCENT,
                font=self._fonts["mono"],
                anchor="w",
            ).pack(side="left", padx=(10, 0))

        meta = task.message or (
            f"{_middle_ellipsis(task.cwd, 54)}  ·  {_middle_ellipsis(task.command, 100)}"
        )
        meta_label = tk.Label(
            identity,
            text=meta,
            bg=theme.SURFACE_ALT,
            fg=theme.TEXT_DIM,
            font=self._fonts["small"],
            anchor="w",
            justify="left",
            wraplength=460,
        )
        meta_label.pack(fill="x", pady=(5, 0))
        identity.bind(
            "<Configure>",
            lambda event: (
                name.configure(wraplength=max(180, event.width - 8)),
                meta_label.configure(wraplength=max(180, event.width - 8)),
            ),
        )

        actions = tk.Frame(self._content, bg=theme.SURFACE_ALT)
        actions.pack(fill="x", pady=(10, 0))
        primary = task_primary_action(task)
        if primary is not None:
            if primary.key == "start":
                command = lambda: self._callbacks.on_start_task(self._profile_id, task.name)
            else:
                command = lambda: self._callbacks.on_stop_task(self._profile_id, task.name)
            ActionButton(
                actions,
                text=primary.label,
                fonts=self._fonts,
                command=command,
                foreground=theme.OK if primary.key == "start" else theme.WARNING,
                background=theme.SURFACE_ALT,
                compact=True,
            ).pack(side="left")
        ActionButton(
            actions,
            text="≡ Logs",
            fonts=self._fonts,
            command=lambda: self._callbacks.on_show_logs(self._profile_id, task.name),
            foreground=theme.ACCENT,
            background=theme.SURFACE_ALT,
            compact=True,
        ).pack(side="right")

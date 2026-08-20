from __future__ import annotations

import math
import queue
import time
import tkinter as tk
import uuid
from collections.abc import Callable
from concurrent.futures import Future, ThreadPoolExecutor
from pathlib import Path
from tkinter import font as tkfont
from typing import Any, ClassVar

from cutting_board.constants import APP_NAME
from cutting_board.controller import ScanController
from cutting_board.launch_models import (
    LaunchProfile,
    LaunchState,
    LaunchTask,
    ManagedTaskSnapshot,
)
from cutting_board.models import ServiceSnapshot, TerminationResult, WorkspaceSnapshot
from cutting_board.presentation import (
    ServiceGroup,
    container_count,
    container_services,
    group_services,
    visible_services,
)
from cutting_board.scanner.classifier import classify_image
from cutting_board.scanner.docker import ContainerInfo, ContainerListing, list_containers
from cutting_board.services.browser import open_url
from cutting_board.services.settings import SettingsStore, UISettings
from cutting_board.ui import theme
from cutting_board.ui.dialogs import (
    ContainerDetailDialog,
    ServiceDetailDialog,
    SettingsDialog,
    ask_confirmation,
)
from cutting_board.ui.icons import IconStore
from cutting_board.ui.launch_dialogs import (
    LaunchLogDialog,
    LaunchProfileDraft,
    LaunchTaskDraft,
    ask_launch_profile,
)
from cutting_board.ui.launch_widgets import (
    LaunchProfileCallbacks,
    LaunchProfilesPanel,
    LaunchProfileView,
    LaunchTaskView,
)
from cutting_board.ui.widgets import ContainerTile, ScrollArea, SectionHeader, ServiceTile
from cutting_board.ui.window_icon import BLOB_NAME, apply_window_icon

POLL_INTERVAL_MS = 120
HEADER_HEIGHT = 56
SETTINGS_HIT_TARGET = 36

TAB_SERVICES = "services"
TAB_DOCKER = "docker"
TAB_LAUNCH = "launch"

# Shelling out to the docker CLI is far more expensive than a port scan, so the
# container list is refreshed on its own slower clock instead of once per scan,
# and slower still while the Docker tab is not the one being looked at — the
# only thing the other tab needs from it is the number beside its name.
DOCKER_REFRESH_SECONDS = 6.0
DOCKER_IDLE_REFRESH_SECONDS = 45.0

LOOSE_GROUP = "단독 컨테이너"
STOPPED_GROUP = "중지됨"


class _ToolbarTab(tk.Canvas):
    """A rounded, borderless toolbar tab with explicit focus feedback."""

    def __init__(
        self,
        parent: tk.Misc,
        *,
        text: str,
        font: tuple[str, int, str],
        command: Callable[[], None],
    ) -> None:
        super().__init__(
            parent,
            width=64,
            height=32,
            bg=theme.SURFACE_ALT,
            highlightthickness=0,
            bd=0,
            takefocus=True,
            cursor="hand2",
        )
        self._font = font
        self._font_measure = tkfont.Font(root=parent, font=font)
        self._text = text
        self._active = False
        self._hovered = False
        self._focused = False
        self.accessible_name = text
        _bind_action(self, command)
        self.bind("<Enter>", lambda _event: self._set_hovered(True))
        self.bind("<Leave>", lambda _event: self._set_hovered(False))
        self.bind("<FocusIn>", lambda _event: self._set_focused(True))
        self.bind("<FocusOut>", lambda _event: self._set_focused(False))
        self._paint()

    def set_presentation(self, text: str, *, active: bool) -> None:
        self._text = text
        self.accessible_name = text
        self._active = active
        self._paint()

    def _set_hovered(self, value: bool) -> None:
        self._hovered = value
        self._paint()

    def _set_focused(self, value: bool) -> None:
        self._focused = value
        self._paint()

    def _paint(self) -> None:
        width = max(54, self._font_measure.measure(self._text) + 24)
        self.configure(width=width)
        self.delete("all")
        fill, outline = _segmented_surface_colours(
            self._hovered,
            self._focused,
            self._active,
        )
        theme.rounded_rect(
            self,
            1,
            1,
            width - 1,
            31,
            9,
            fill=fill,
            outline=outline,
        )
        foreground = theme.TEXT if self._active or self._hovered or self._focused else theme.TEXT_DIM
        self.create_text(
            width / 2,
            16,
            text=self._text,
            fill=foreground,
            font=self._font,
        )


class _SegmentedTabBar(tk.Canvas):
    """One rounded toolbar surface containing all navigation segments."""

    def __init__(self, parent: tk.Misc) -> None:
        super().__init__(
            parent,
            width=180,
            height=36,
            bg=theme.CANVAS,
            highlightthickness=0,
            bd=0,
        )
        self._items: list[tuple[int, _ToolbarTab]] = []

    def add_tab(
        self,
        *,
        text: str,
        font: tuple[str, int, str],
        command: Callable[[], None],
    ) -> _ToolbarTab:
        tab = _ToolbarTab(self, text=text, font=font, command=command)
        window = self.create_window(2, 2, anchor="nw", window=tab)
        self._items.append((window, tab))
        self.layout_tabs()
        return tab

    def layout_tabs(self) -> None:
        x = 2
        for window, tab in self._items:
            self.coords(window, x, 2)
            x += int(float(tab.cget("width")))
        width = x + 2
        self.configure(width=width)
        self.delete("bar-surface")
        theme.rounded_rect(
            self,
            0,
            0,
            width,
            36,
            10,
            fill=theme.SURFACE_ALT,
            outline=theme.SURFACE_ALT,
            tags="bar-surface",
        )
        self.tag_lower("bar-surface")


class _SettingsGear(tk.Canvas):
    """An icon-only settings control sized for a comfortable toolbar target."""

    accessible_name = "설정"

    def __init__(self, parent: tk.Misc, *, command: Callable[[], None]) -> None:
        super().__init__(
            parent,
            width=SETTINGS_HIT_TARGET,
            height=SETTINGS_HIT_TARGET,
            bg=theme.CANVAS,
            highlightthickness=0,
            bd=0,
            takefocus=True,
            cursor="hand2",
        )
        self._hovered = False
        self._focused = False
        _bind_action(self, command)
        self.bind("<Enter>", lambda _event: self._set_hovered(True))
        self.bind("<Leave>", lambda _event: self._set_hovered(False))
        self.bind("<FocusIn>", lambda _event: self._set_focused(True))
        self.bind("<FocusOut>", lambda _event: self._set_focused(False))
        self._paint()

    def _set_hovered(self, value: bool) -> None:
        self._hovered = value
        self._paint()

    def _set_focused(self, value: bool) -> None:
        self._focused = value
        self._paint()

    def _paint(self) -> None:
        self.delete("all")
        fill, outline = _toolbar_surface_colours(self._hovered, self._focused)
        theme.rounded_rect(
            self,
            1,
            1,
            35,
            35,
            11,
            fill=fill,
            outline=outline,
        )
        gear_colour = theme.TEXT if self._hovered or self._focused else theme.TEXT_MUTED
        self.create_polygon(
            _gear_polygon_points(SETTINGS_HIT_TARGET),
            fill=gear_colour,
            outline="",
        )
        self.create_oval(14.5, 14.5, 21.5, 21.5, fill=fill, outline="")


class CuttingBoardWindow:
    """The board: every development service running right now, as tiles."""

    _TAB_LABELS: ClassVar[dict[str, str]] = {
        TAB_SERVICES: "서비스",
        TAB_DOCKER: "Docker",
        TAB_LAUNCH: "실행 구성",
    }

    def __init__(
        self,
        root: tk.Tk,
        *,
        controller: ScanController | None,
        launch_controller: Any,
        terminator: Any,
        settings_store: SettingsStore,
        initial_snapshot: WorkspaceSnapshot | None = None,
        assets_dir: Path | None = None,
        auto_close_seconds: float | None = None,
        container_source: Callable[[], ContainerListing] = list_containers,
    ) -> None:
        self.root = root
        self.controller = controller
        self.launch_controller = launch_controller
        self.terminator = terminator
        self.settings_store = settings_store
        self.settings: UISettings = settings_store.load()
        # Injected so --demo can show containers without a Docker daemon.
        self.container_source = container_source
        self._assets_dir = assets_dir
        self.icons = IconStore(assets_dir)

        self.snapshot: WorkspaceSnapshot | None = initial_snapshot
        self.busy_pids: set[int] = set()
        self.executor = ThreadPoolExecutor(max_workers=2, thread_name_prefix="cutting-board-action")
        self.action_results: queue.Queue[tuple[Future[TerminationResult], ServiceSnapshot, bool]] = (
            queue.Queue()
        )
        self.tab = TAB_SERVICES
        self.containers: ContainerListing | None = None
        self.container_results: queue.Queue[ContainerListing] = queue.Queue()
        self.launch_action_results: queue.Queue[tuple[Future[Any], str]] = queue.Queue()
        self._containers_requested_at = 0.0
        self._containers_in_flight = False
        self._columns = 0
        # What the scroll body was last built from; see _draw_body.
        self._body_signature: tuple[Any, ...] | None = None
        self._body_columns = 0
        self._tiles: list[ServiceTile] = []
        self._toast: tk.Frame | None = None
        self._toast_job: str | None = None
        self._window_icons: tuple[tk.PhotoImage, ...] = ()
        self._closing = False

        root.title(APP_NAME)
        root.geometry(self.settings.window_geometry)
        root.minsize(560, 420)
        theme.apply_palette(self.settings.theme_mode)
        self.fonts = theme.configure_theme(root)
        self._load_window_icon()

        self._build_layout()
        root.protocol("WM_DELETE_WINDOW", self.close)

        if self.controller is not None:
            self.controller.start()
        # The loop runs even with no scanner behind it: the Docker tab feeds
        # itself through the same queue and has to be drained either way.
        self.root.after(POLL_INTERVAL_MS, self._poll_events)
        self.render()

        if auto_close_seconds is not None:
            self.root.after(
                max(100, int(auto_close_seconds * 1000)),
                lambda: self.close(notify=False),
            )

    # --------------------------------------------------------------- layout

    def _load_window_icon(self) -> None:
        """Hand the window manager the application icon.

        This is what the title bar, the task switcher and the dock draw, so
        the board itself does not repeat the name or the mark in its header.
        Several sizes are offered because the window manager picks whichever
        one fits the surface it is drawing.
        """
        sizes = (None, 128, 64, 48, 32)
        self._window_icons = tuple(
            image for image in (self.icons.app(size) for size in sizes) if image is not None
        )
        if self._window_icons:
            try:
                self.root.iconphoto(True, *self._window_icons)
            except tk.TclError:
                pass
        if self._assets_dir is not None:
            # Tk only sets the legacy icon hint, which the desktop ignores, so
            # the EWMH property is published once the window manager has had a
            # chance to reparent the window.
            self.root.bind("<Map>", self._publish_window_icon, add="+")

    def _publish_window_icon(self, _event: tk.Event) -> None:
        self.root.unbind("<Map>")
        assert self._assets_dir is not None
        self.root.after(150, lambda: apply_window_icon(self.root, self._assets_dir / BLOB_NAME))

    def _build_layout(self) -> None:
        self.root.configure(background=theme.CANVAS)

        header = tk.Frame(self.root, bg=theme.CANVAS, height=HEADER_HEIGHT)
        header.pack(fill="x", side="top")
        header.pack_propagate(False)

        # The title bar already carries the name and the icon, so the header
        # only says which board is open and what is on it.
        self._tab_bar = _SegmentedTabBar(header)
        self._tab_bar.pack(side="left", padx=(theme.TILE_PAD + 8, 0), pady=10)

        self._tabs: dict[str, _ToolbarTab] = {}
        for key, label in (
            (TAB_SERVICES, "서비스"),
            (TAB_DOCKER, "Docker"),
            (TAB_LAUNCH, "실행 구성"),
        ):
            self._tabs[key] = self._tab_bar.add_tab(
                text=label,
                font=self.fonts["label"],
                command=lambda target=key: self._select_tab(target),
            )

        right = tk.Frame(header, bg=theme.CANVAS)
        right.pack(side="right", padx=(0, theme.TILE_PAD + 8))

        self._build_settings_button(right).pack(side="right")

        tk.Frame(self.root, bg=theme.HAIRLINE, height=1).pack(fill="x", side="top")

        footer = tk.Frame(self.root, bg=theme.CANVAS, height=26)
        footer.pack(fill="x", side="bottom")
        footer.pack_propagate(False)
        self.status = tk.Label(
            footer,
            text="",
            bg=theme.CANVAS,
            fg=theme.TEXT_DIM,
            font=self.fonts["tile_meta"],
        )
        self.status.pack(side="left", padx=(theme.TILE_PAD + 12, 0))

        self.scroll = ScrollArea(self.root)
        self.scroll.pack(fill="both", expand=True, side="top")
        self.scroll.canvas.bind("<Configure>", self._on_resize, add="+")
        self.launch_panel = LaunchProfilesPanel(
            self.root,
            fonts=self.fonts,
            callbacks=LaunchProfileCallbacks(
                on_add=self._add_launch_profile,
                on_start_profile=self._start_launch_profile,
                on_stop_profile=self._stop_launch_profile,
                on_edit_profile=self._edit_launch_profile,
                on_delete_profile=self._delete_launch_profile,
                on_start_task=self._start_launch_task,
                on_stop_task=self._stop_launch_task,
                on_show_logs=self._show_launch_logs,
            ),
        )

    def _build_settings_button(self, parent: tk.Misc) -> _SettingsGear:
        """Build a visible, keyboard-operable settings affordance."""
        return _SettingsGear(parent, command=self._show_settings)

    def _select_tab(self, key: str) -> None:
        if key == self.tab:
            return
        self.tab = key
        self.scroll.canvas.yview_moveto(0.0)
        self._pack_tab_body(key)
        if key == TAB_DOCKER:
            self._request_containers(force=True)
        self.render()

    def _pack_tab_body(self, key: str) -> None:
        if key == TAB_LAUNCH:
            self.scroll.pack_forget()
            self.launch_panel.pack(fill="both", expand=True, side="top")
        else:
            self.launch_panel.pack_forget()
            self.scroll.pack(fill="both", expand=True, side="top")

    def _paint_tabs(self, counts: dict[str, int]) -> None:
        for key, text in self._tabs.items():
            active = key == self.tab
            count = counts.get(key)
            suffix = "" if count is None else f"  {count}"
            text.set_presentation(
                f"{self._TAB_LABELS[key]}{suffix}",
                active=active,
            )
        self._tab_bar.layout_tabs()

    def _on_resize(self, event: tk.Event) -> None:
        columns = self._columns_for(event.width)
        if columns != self._columns:
            self._columns = columns
            self.render()

    @staticmethod
    def _columns_for(width: int) -> int:
        usable = max(0, width - (theme.TILE_PAD + 4) * 2)
        return max(1, usable // (theme.TILE_SPAN + theme.GRID_GUTTER))

    # --------------------------------------------------------------- events

    def refresh(self) -> None:
        if self.controller is not None:
            self.controller.refresh()

    def _poll_events(self) -> None:
        if self.controller is not None:
            while True:
                try:
                    event = self.controller.events.get_nowait()
                except queue.Empty:
                    break
                if event.snapshot is not None:
                    self._accept_snapshot(event.snapshot)
                elif event.error:
                    self._show_toast(event.error, error=True)
        launch_changed = False
        while True:
            try:
                self.launch_controller.events.get_nowait()
            except queue.Empty:
                break
            launch_changed = True
        if launch_changed:
            self.render()
        self._drain_action_results()
        self._drain_launch_action_results()
        self._drain_container_results()
        self._request_containers()
        self.root.after(POLL_INTERVAL_MS, self._poll_events)

    def _accept_snapshot(self, snapshot: WorkspaceSnapshot) -> None:
        self.snapshot = snapshot
        self.render()

    # --------------------------------------------------------------- render

    def render(self) -> None:
        snapshot = self.snapshot
        if snapshot is None:
            self._paint_tabs({TAB_LAUNCH: len(self.launch_controller.profiles)})
            if self.tab == TAB_LAUNCH:
                self.status.configure(text="서비스를 찾는 중")
                self._render_launch_profiles()
                return
            self._draw_body(("loading",), lambda: self._render_empty("서비스를 찾는 중"))
            return

        services = visible_services(snapshot)
        self._paint_tabs(
            {
                TAB_SERVICES: len(services),
                TAB_DOCKER: self._docker_tab_count(snapshot),
                TAB_LAUNCH: len(self.launch_controller.profiles),
            }
        )
        self.status.configure(
            text=f"리스너 {snapshot.endpoint_count}개  ·  {snapshot.scan_duration_ms} ms"
        )

        if self.tab == TAB_DOCKER:
            self._render_docker(snapshot)
        elif self.tab == TAB_LAUNCH:
            self._render_launch_profiles()
        else:
            self._render_services(services)

    # ------------------------------------------------------------ body reuse

    def _draw_body(self, signature: tuple[Any, ...], build: Callable[[], None]) -> None:
        """Rebuild the scroll body only when what it draws has actually changed.

        A scan runs every couple of seconds and nearly always finds the same
        services, but tearing the body down and building it again makes the
        whole board flash and throws the scroll position away with it. The
        signature covers everything the sections and the tiles paint and
        deliberately leaves out uptime, CPU and memory, which move on every
        scan; those are pushed onto the surviving tiles instead.

        The column count is part of the comparison because it is the grid
        layout rather than the data: a resize that fits another tile per row
        has to rebuild even though nothing about the services changed.
        """
        columns = self._columns or self._columns_for(self.scroll.canvas.winfo_width() or 1200)
        if signature == self._body_signature and columns == self._body_columns:
            self._refresh_tiles()
            return

        for child in self.scroll.body.winfo_children():
            child.destroy()
        self._tiles = []
        self._body_signature = signature
        self._body_columns = columns
        build()
        tk.Frame(self.scroll.body, bg=theme.CANVAS, height=theme.TILE_PAD).pack(fill="x")

    def _refresh_tiles(self) -> None:
        """Hand the surviving tiles the snapshot the latest scan produced."""
        snapshot = self.snapshot
        if snapshot is None:
            return
        current = {service.id: service for service in snapshot.services}
        for tile in self._tiles:
            service = current.get(tile.service.id)
            if service is not None:
                tile.adopt(service)

    def _tile_signature(self, service: ServiceSnapshot) -> tuple[Any, ...]:
        """Everything about a service that changes what its tile looks like."""
        process = service.process
        return (
            service.id,
            service.display_name,
            service.tech,
            service.unique_ports,
            service.category.value,
            service.status.value,
            bool(service.warnings),
            service.origin_kind,
            service.origin_label,
            service.can_terminate,
            process is not None and process.pid in self.busy_pids,
        )

    def _render_services(self, services: tuple[ServiceSnapshot, ...]) -> None:
        groups = group_services(services)
        if not groups:
            message = "실행 중인 개발 서비스 없음"
            self._draw_body(("empty", message), lambda: self._render_empty(message))
            return

        signature = (
            "services",
            tuple(
                (
                    group.name,
                    group.path,
                    group.system,
                    tuple(self._tile_signature(service) for service in group.services),
                )
                for group in groups
            ),
        )
        self._draw_body(signature, lambda: self._build_service_sections(groups))

    # ----------------------------------------------------------- launch tab

    def _render_launch_profiles(self) -> None:
        self.launch_panel.render(self._launch_profile_views())

    def _launch_profile_views(self) -> tuple[LaunchProfileView, ...]:
        views: list[LaunchProfileView] = []
        for profile in self.launch_controller.profiles:
            task_views = tuple(self._launch_task_view(profile, task) for task in profile.tasks)
            can_stop = any(task.can_stop for task in task_views)
            views.append(
                LaunchProfileView(
                    id=profile.id,
                    name=profile.name,
                    project_root=profile.project_root,
                    tasks=task_views,
                    can_start=any(task.can_start for task in task_views),
                    can_stop=can_stop,
                    can_edit=not can_stop,
                    can_delete=not can_stop,
                )
            )
        return tuple(views)

    def _launch_task_view(self, profile: LaunchProfile, task: LaunchTask) -> LaunchTaskView:
        snapshot = self.launch_controller.snapshot(profile.id, task.name)
        owned = self._is_owned_snapshot(snapshot)
        external = not owned and self._expected_listener_is_external(profile, task)
        stopped = snapshot.state in {LaunchState.STOPPED, LaunchState.FAILED} and not owned
        message = snapshot.message
        if external:
            message = "외부에서 실행한 프로세스입니다. Cutting Board에서는 종료하지 않습니다."
        return LaunchTaskView(
            name=task.name,
            cwd=task.cwd,
            command=task.command,
            state=snapshot.state.value,
            expected_port=task.expected_port,
            message=message,
            can_start=stopped and not external,
            can_stop=owned,
            external=external,
        )

    @staticmethod
    def _is_owned_snapshot(snapshot: ManagedTaskSnapshot) -> bool:
        return (
            snapshot.state in {LaunchState.STARTING, LaunchState.RUNNING, LaunchState.STOPPING}
            or snapshot.main_pid is not None
            or snapshot.watch_pid is not None
        )

    def _expected_listener_is_external(self, profile: LaunchProfile, task: LaunchTask) -> bool:
        snapshot = self.snapshot
        if snapshot is None or task.expected_port is None:
            return False
        root = Path(profile.project_root).expanduser().resolve(strict=False)
        return any(
            task.expected_port in service.unique_ports
            and service.project is not None
            and Path(service.project.root_path).expanduser().resolve(strict=False) == root
            for service in snapshot.services
        )

    def _add_launch_profile(self) -> None:
        draft = ask_launch_profile(self.root, fonts=self.fonts)
        if draft is None:
            return
        self._save_launch_profile(self._profile_from_draft(uuid.uuid4().hex, draft))

    def _edit_launch_profile(self, profile_id: str) -> None:
        profile = self.launch_controller.profile(profile_id)
        draft = ask_launch_profile(
            self.root,
            fonts=self.fonts,
            profile=self._draft_from_profile(profile),
        )
        if draft is None:
            return
        self._save_launch_profile(self._profile_from_draft(profile.id, draft))

    def _save_launch_profile(self, profile: LaunchProfile) -> None:
        try:
            self.launch_controller.save_profile(profile)
        except (KeyError, OSError, RuntimeError, ValueError) as exc:
            self._show_toast(f"실행 구성을 저장하지 못했습니다: {exc}", error=True)
            return
        self._show_toast("실행 구성을 저장했습니다.")
        self.render()

    def _delete_launch_profile(self, profile_id: str) -> None:
        profile = self.launch_controller.profile(profile_id)
        confirmed = ask_confirmation(
            self.root,
            fonts=self.fonts,
            icons=self.icons,
            title="실행 구성 삭제",
            headline=profile.name,
            meta=profile.project_root,
            question="이 실행 구성과 저장된 명령을 삭제합니다.",
            confirm_label="삭제",
        )
        if not confirmed:
            return
        try:
            self.launch_controller.delete_profile(profile_id)
        except (KeyError, OSError, RuntimeError, ValueError) as exc:
            self._show_toast(f"실행 구성을 삭제하지 못했습니다: {exc}", error=True)
            return
        self._show_toast("실행 구성을 삭제했습니다.")
        self.render()

    def _start_launch_profile(self, profile_id: str) -> None:
        profile_view = next(view for view in self._launch_profile_views() if view.id == profile_id)
        task_names = tuple(task.name for task in profile_view.tasks if task.can_start)
        if not task_names:
            return
        self._submit_launch_action(
            "실행 구성을 시작하지 못했습니다",
            lambda: tuple(
                self.launch_controller.start_task(profile_id, task_name)
                for task_name in task_names
            ),
        )

    def _stop_launch_profile(self, profile_id: str) -> None:
        self._submit_launch_action(
            "실행 구성을 종료하지 못했습니다",
            lambda: self.launch_controller.stop_profile(profile_id),
        )

    def _start_launch_task(self, profile_id: str, task_name: str) -> None:
        profile = self.launch_controller.profile(profile_id)
        task = profile.task(task_name)
        if self._expected_listener_is_external(profile, task):
            self._show_toast("외부 실행 중인 작업은 중복 실행하지 않습니다.", error=True)
            return
        self._submit_launch_action(
            f"{task_name} 작업을 시작하지 못했습니다",
            lambda: self.launch_controller.start_task(profile_id, task_name),
        )

    def _stop_launch_task(self, profile_id: str, task_name: str) -> None:
        snapshot = self.launch_controller.snapshot(profile_id, task_name)
        if not self._is_owned_snapshot(snapshot):
            self._show_toast("Cutting Board에서 실행한 작업만 종료할 수 있습니다.", error=True)
            return
        self._submit_launch_action(
            f"{task_name} 작업을 종료하지 못했습니다",
            lambda: self.launch_controller.stop_task(profile_id, task_name),
        )

    def _submit_launch_action(self, failure_message: str, action: Callable[[], Any]) -> None:
        future = self.executor.submit(action)
        future.add_done_callback(
            lambda completed: self.launch_action_results.put((completed, failure_message))
        )

    def _drain_launch_action_results(self) -> None:
        changed = False
        while True:
            try:
                future, failure_message = self.launch_action_results.get_nowait()
            except queue.Empty:
                break
            changed = True
            try:
                future.result()
            except Exception as exc:  # noqa: BLE001 - background action boundary
                self._show_toast(f"{failure_message}: {exc}", error=True)
        if changed:
            self.refresh()
            self.render()

    def _show_launch_logs(self, profile_id: str, task_name: str) -> None:
        profile = self.launch_controller.profile(profile_id)
        snapshot = self.launch_controller.snapshot(profile_id, task_name)
        LaunchLogDialog(
            self.root,
            fonts=self.fonts,
            profile_name=profile.name,
            task_name=task_name,
            lines=snapshot.logs,
        )

    @staticmethod
    def _draft_from_profile(profile: LaunchProfile) -> LaunchProfileDraft:
        return LaunchProfileDraft(
            name=profile.name,
            project_root=profile.project_root,
            tasks=tuple(
                LaunchTaskDraft(
                    name=task.name,
                    cwd=task.cwd,
                    command=task.command,
                    expected_port=task.expected_port,
                    watch_command=task.watch_command,
                )
                for task in profile.tasks
            ),
        )

    @staticmethod
    def _profile_from_draft(profile_id: str, draft: LaunchProfileDraft) -> LaunchProfile:
        return LaunchProfile(
            id=profile_id,
            name=draft.name,
            project_root=draft.project_root,
            tasks=tuple(
                LaunchTask(
                    name=task.name,
                    cwd=task.cwd,
                    command=task.command,
                    expected_port=task.expected_port,
                    watch_command=task.watch_command,
                )
                for task in draft.tasks
            ),
        )

    def _build_service_sections(self, groups: tuple[ServiceGroup, ...]) -> None:
        for group in groups:
            section = tk.Frame(self.scroll.body, bg=theme.CANVAS)
            section.pack(fill="x", pady=(16, 0))
            SectionHeader(
                section,
                fonts=self.fonts,
                title=group.name,
                path=group.path,
                accent=theme.VIOLET if group.system else theme.ACCENT,
            ).pack(fill="x")
            self._build_tile_grid(section, group.services)

    def _build_tile_grid(self, section: tk.Frame, services: tuple[ServiceSnapshot, ...]) -> None:
        columns = self._body_columns
        grid = tk.Frame(section, bg=theme.CANVAS)
        grid.pack(fill="x", padx=theme.TILE_PAD - 6, pady=(2, 0))
        for index, service in enumerate(services):
            tile = ServiceTile(
                grid,
                service=service,
                fonts=self.fonts,
                icons=self.icons,
                busy=service.process is not None and service.process.pid in self.busy_pids,
                on_details=self._show_details,
                on_terminate=self._request_termination,
                on_open=self._open_service,
            )
            tile.grid(
                row=index // columns,
                column=index % columns,
                sticky=self._tile_grid_sticky(columns),
            )
            self._tiles.append(tile)

    @staticmethod
    def _tile_grid_sticky(columns: int) -> str:
        del columns
        return "w"

    # ---------------------------------------------------------- docker tab

    def _request_containers(self, *, force: bool = False) -> None:
        """Ask docker for the container list, at most once per refresh window."""
        if self._containers_in_flight:
            return
        interval = (
            DOCKER_REFRESH_SECONDS if self.tab == TAB_DOCKER else DOCKER_IDLE_REFRESH_SECONDS
        )
        now = time.monotonic()
        if not force and now - self._containers_requested_at < interval:
            return
        self._containers_requested_at = now
        self._containers_in_flight = True
        future = self.executor.submit(self.container_source)
        future.add_done_callback(self._on_containers)

    def _on_containers(self, future: Future[ContainerListing]) -> None:
        try:
            listing = future.result()
        except Exception:  # noqa: BLE001 - background integration boundary
            listing = ContainerListing.unavailable("Docker 정보를 가져오지 못했습니다.")
        self.container_results.put(listing)

    def _drain_container_results(self) -> None:
        latest: ContainerListing | None = None
        while True:
            try:
                latest = self.container_results.get_nowait()
            except queue.Empty:
                break
        if latest is None:
            return
        self._containers_in_flight = False
        changed = latest != self.containers
        self.containers = latest
        if changed:
            self.render()

    def _docker_tab_count(self, snapshot: WorkspaceSnapshot) -> int:
        listing = self.containers
        if listing is not None and listing.available:
            return sum(1 for item in listing.containers if item.running)
        return container_count(snapshot)

    def _render_docker(self, snapshot: WorkspaceSnapshot) -> None:
        listing = self.containers
        if listing is None:
            message = "컨테이너를 찾는 중"
            self._draw_body(("containers-pending",), lambda: self._render_empty(message))
            return
        if not listing.available:
            message = listing.message or "Docker를 사용할 수 없습니다"
            services = container_services(snapshot)
            signature = (
                "docker-unavailable",
                message,
                tuple(self._tile_signature(service) for service in services),
            )
            self._draw_body(signature, lambda: self._build_port_fallback(message, services))
            return
        if not listing.containers:
            message = listing.message or "컨테이너가 없습니다"
            self._draw_body(("containers-none", message), lambda: self._render_empty(message))
            return

        groups = _group_containers(listing.containers)
        signature = (
            "docker",
            tuple(
                (project, tuple(_container_signature(item) for item in containers))
                for project, containers in groups
            ),
        )
        self._draw_body(signature, lambda: self._build_container_sections(groups))

    def _build_container_sections(self, groups: list[tuple[str, list[ContainerInfo]]]) -> None:
        columns = self._body_columns
        for project, containers in groups:
            section = tk.Frame(self.scroll.body, bg=theme.CANVAS)
            section.pack(fill="x", pady=(16, 0))
            SectionHeader(
                section,
                fonts=self.fonts,
                title=project,
                path=None,
                accent=theme.TEXT_DIM if project == STOPPED_GROUP else theme.VIOLET,
            ).pack(fill="x")

            grid = tk.Frame(section, bg=theme.CANVAS)
            grid.pack(fill="x", padx=theme.TILE_PAD - 6, pady=(2, 0))
            for index, container in enumerate(containers):
                ContainerTile(
                    grid,
                    container=container,
                    tech=classify_image(container.image).tech,
                    fonts=self.fonts,
                    icons=self.icons,
                    on_details=self._show_container_details,
                ).grid(
                    row=index // columns,
                    column=index % columns,
                    sticky=self._tile_grid_sticky(columns),
                )

    def _build_port_fallback(self, message: str, services: tuple[ServiceSnapshot, ...]) -> None:
        """What is still knowable when the docker CLI cannot be reached.

        The published ports show up as ordinary processes, so the board can
        still say something more useful than an error on its own.
        """
        self._render_empty(message)
        if not services:
            return
        section = tk.Frame(self.scroll.body, bg=theme.CANVAS)
        section.pack(fill="x", pady=(16, 0))
        SectionHeader(
            section,
            fonts=self.fonts,
            title="게시된 포트",
            path=None,
            accent=theme.VIOLET,
        ).pack(fill="x")
        self._build_tile_grid(section, services)

    def _show_container_details(self, container: ContainerInfo) -> None:
        ContainerDetailDialog(
            self.root,
            container=container,
            tech=classify_image(container.image).tech,
            fonts=self.fonts,
            icons=self.icons,
        )

    def _render_empty(self, title: str) -> None:
        holder = tk.Frame(self.scroll.body, bg=theme.CANVAS)
        holder.pack(fill="both", expand=True, pady=90)
        tk.Label(
            holder,
            text="◆",
            bg=theme.CANVAS,
            fg=theme.BORDER,
            font=(self.fonts["wordmark"][0], 34, "bold"),
        ).pack()
        tk.Label(
            holder,
            text=title,
            bg=theme.CANVAS,
            fg=theme.TEXT_DIM,
            font=self.fonts["empty"],
        ).pack(pady=(10, 0))

    # ---------------------------------------------------------- interactions

    def _open_service(self, service: ServiceSnapshot) -> None:
        url = service.browser_url()
        if url:
            open_url(url)

    def _show_details(self, service: ServiceSnapshot) -> None:
        ServiceDetailDialog(
            self.root,
            service=service,
            fonts=self.fonts,
            icons=self.icons,
            on_open=self._open_service,
            on_terminate=self._request_termination,
        )

    def _show_settings(self) -> None:
        SettingsDialog(
            self.root,
            fonts=self.fonts,
            icons=self.icons,
            settings_path=self.settings_store.path,
            scan_interval_seconds=self.settings.scan_interval_seconds,
            theme_mode=self.settings.theme_mode,
            on_interval_change=self._apply_scan_interval,
            on_theme_change=self._apply_theme_mode,
        )

    def _apply_scan_interval(self, seconds: float) -> None:
        """Persist a new scan interval and hand it to the running scan loop."""
        self.settings.scan_interval_seconds = seconds
        self.settings_store.save(self.settings)
        if self.controller is not None:  # --demo runs without a scanner behind it
            self.controller.set_interval(seconds)

    def _apply_theme_mode(self, mode: str) -> None:
        """Persist a palette choice and rebuild widgets without touching runtimes."""
        if mode == self.settings.theme_mode:
            return
        self.settings.theme_mode = mode
        self.settings_store.save(self.settings)
        self._rebuild_for_theme()

    def _rebuild_for_theme(self) -> None:
        selected_tab = self.tab
        if self._toast_job is not None:
            try:
                self.root.after_cancel(self._toast_job)
            except tk.TclError:
                pass
        self._toast = None
        self._toast_job = None
        for child in self.root.winfo_children():
            child.destroy()

        theme.apply_palette(self.settings.theme_mode)
        self.fonts = theme.configure_theme(self.root)
        self._columns = 0
        self._body_columns = 0
        self._body_signature = None
        self._tiles = []
        self._build_layout()
        self.tab = selected_tab
        self._pack_tab_body(selected_tab)
        self.render()

    def _request_termination(self, service: ServiceSnapshot) -> None:
        process = service.process
        if process is None or not service.can_terminate or process.pid in self.busy_pids:
            return
        ports = "  ·  ".join(f":{port}" for port in service.unique_ports)
        confirmed = ask_confirmation(
            self.root,
            fonts=self.fonts,
            icons=self.icons,
            title="종료",
            headline=service.display_name,
            meta=f"PID {process.pid}  ·  {ports}",
            question="SIGTERM을 보내 이 서비스를 종료합니다.",
            confirm_label="종료",
            tech=service.tech,
        )
        if confirmed:
            self._submit_termination(service, force=False)

    def _submit_termination(self, service: ServiceSnapshot, *, force: bool) -> None:
        process = service.process
        if process is None:
            return
        self.busy_pids.add(process.pid)
        self.render()
        future = self.executor.submit(
            self.terminator.terminate,
            process.pid,
            process.create_time,
            force=force,
        )
        future.add_done_callback(
            lambda completed, target=service, requested=force: self.action_results.put(
                (completed, target, requested)
            )
        )

    def _drain_action_results(self) -> None:
        while True:
            try:
                future, service, force = self.action_results.get_nowait()
            except queue.Empty:
                return
            self._handle_termination_result(future, service, force)

    def _handle_termination_result(
        self,
        future: Future[TerminationResult],
        service: ServiceSnapshot,
        force: bool,
    ) -> None:
        process = service.process
        if process:
            self.busy_pids.discard(process.pid)
        try:
            result = future.result()
        except Exception as exc:  # noqa: BLE001 - background integration boundary
            self._show_toast(f"종료 실패: {exc}", error=True)
            self.render()
            return

        self.refresh()

        if result.status == "still_running" and not force:
            self.render()
            confirmed = ask_confirmation(
                self.root,
                fonts=self.fonts,
                icons=self.icons,
                title="강제 종료",
                headline=service.display_name,
                meta=f"PID {process.pid}  ·  SIGTERM 무응답" if process else "SIGTERM 무응답",
                question="아직 실행 중입니다. SIGKILL로 강제 종료할까요?",
                confirm_label="강제 종료",
                tech=service.tech,
            )
            if confirmed:
                self._submit_termination(service, force=True)
            return

        self._show_toast(result.message, error=not result.success)
        self.render()

    # ---------------------------------------------------------------- toast

    def _show_toast(self, message: str, *, error: bool = False) -> None:
        if self._toast is not None and self._toast.winfo_exists():
            self._toast.destroy()
        if self._toast_job is not None:
            try:
                self.root.after_cancel(self._toast_job)
            except tk.TclError:
                pass

        accent = theme.DANGER if error else theme.ACCENT
        toast = tk.Frame(self.root, bg=theme.SURFACE_ALT, highlightthickness=1)
        toast.configure(highlightbackground=accent, highlightcolor=accent)
        tk.Frame(toast, bg=accent, width=3).pack(side="left", fill="y")
        tk.Label(
            toast,
            text=message,
            bg=theme.SURFACE_ALT,
            fg=theme.TEXT,
            font=self.fonts["small"],
            wraplength=420,
            justify="left",
            padx=12,
            pady=9,
        ).pack(side="left")
        toast.place(relx=0.5, rely=1.0, anchor="s", y=-40)
        self._toast = toast
        self._toast_job = self.root.after(4200, self._clear_toast)

    def _clear_toast(self) -> None:
        self._toast_job = None
        if self._toast is not None and self._toast.winfo_exists():
            self._toast.destroy()
        self._toast = None

    # ---------------------------------------------------------------- close

    def close(self, *, notify: bool = True) -> None:
        if self._closing:
            return
        if notify and self._owned_launch_tasks():
            confirmed = ask_confirmation(
                self.root,
                fonts=self.fonts,
                icons=self.icons,
                title="Cutting Board 종료",
                headline="실행 중인 작업이 있습니다",
                meta="앱이 시작한 작업만 종료합니다.",
                question="Cutting Board에서 실행한 프로세스가 모두 종료됩니다.",
                confirm_label="종료",
            )
            if not confirmed:
                return
        self._closing = True
        try:
            self.settings.window_geometry = self.root.winfo_geometry()
        except tk.TclError:
            pass
        self.settings_store.save(self.settings)
        self.launch_controller.close()
        if self.controller is not None:
            self.controller.close()
        self.executor.shutdown(wait=False, cancel_futures=True)
        try:
            self.root.destroy()
        except tk.TclError:
            pass

    def _owned_launch_tasks(self) -> tuple[ManagedTaskSnapshot, ...]:
        """Return tasks that may still have app-owned process groups."""
        active_states = {
            LaunchState.STARTING,
            LaunchState.RUNNING,
            LaunchState.STOPPING,
        }
        return tuple(
            snapshot
            for snapshot in self.launch_controller.snapshots()
            if snapshot.state in active_states
            or snapshot.main_pid is not None
            or snapshot.watch_pid is not None
        )


def _bind_action(widget: tk.Misc, command: Callable[[], None]) -> None:
    """Give a custom toolbar control the activation behavior of a button."""

    def activate(_event: tk.Event) -> str:
        if getattr(_event, "num", None) == 1:
            widget.focus_set()
        command()
        return "break"

    for sequence in ("<Button-1>", "<Return>", "<space>"):
        widget.bind(sequence, activate)


def _toolbar_surface_colours(hovered: bool, focused: bool) -> tuple[str, str]:
    fill = theme.SURFACE_ALT if hovered or focused else theme.CANVAS
    if focused:
        return fill, theme.ACCENT
    return fill, theme.BORDER if hovered else theme.CANVAS


def _segmented_surface_colours(
    hovered: bool,
    focused: bool,
    active: bool,
) -> tuple[str, str]:
    if active:
        fill = theme.SURFACE
    elif hovered:
        fill = theme.SURFACE_HOVER
    else:
        fill = theme.SURFACE_ALT
    return fill, theme.ACCENT if focused else fill


def _gear_polygon_points(size: int) -> tuple[float, ...]:
    centre = size / 2
    points: list[float] = []
    for index in range(8):
        base = 2 * math.pi * index / 8
        for radius, offset in ((7.5, -0.30), (10.8, -0.17), (10.8, 0.17), (7.5, 0.30)):
            points.append(centre + math.cos(base + offset) * radius)
            points.append(centre + math.sin(base + offset) * radius)
    return tuple(points)


def _container_signature(container: ContainerInfo) -> tuple[Any, ...]:
    """Everything about a container that changes what its tile looks like.

    The image is in here because it picks the brand mark, not because the tile
    prints it.
    """
    return (
        container.id,
        container.name,
        container.image,
        container.state,
        container.status,
        container.ports,
    )


def _group_containers(
    containers: tuple[ContainerInfo, ...],
) -> list[tuple[str, list[ContainerInfo]]]:
    """Compose projects first, then loose containers, then whatever is stopped.

    Stopped containers are pulled out into their own trailing group rather
    than mixed into the project they belong to. That keeps every project
    section a list of things that are actually up, so the number beside the
    tab — which counts running containers — matches what is on the board.
    """
    running: dict[str, list[ContainerInfo]] = {}
    stopped: list[ContainerInfo] = []
    for container in containers:
        if not container.running:
            stopped.append(container)
            continue
        running.setdefault(container.compose_project or LOOSE_GROUP, []).append(container)

    for items in running.values():
        items.sort(key=lambda item: item.name.casefold())
    groups = sorted(
        running.items(),
        key=lambda entry: (entry[0] == LOOSE_GROUP, entry[0].casefold()),
    )
    if stopped:
        stopped.sort(key=lambda item: item.name.casefold())
        groups.append((STOPPED_GROUP, stopped))
    return groups

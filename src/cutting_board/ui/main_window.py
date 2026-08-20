from __future__ import annotations

import math
import queue
import time
import tkinter as tk
from collections.abc import Callable
from concurrent.futures import Future, ThreadPoolExecutor
from pathlib import Path
from typing import Any

from cutting_board.constants import APP_NAME
from cutting_board.controller import ScanController
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
from cutting_board.ui.widgets import ContainerTile, ScrollArea, SectionHeader, ServiceTile
from cutting_board.ui.window_icon import BLOB_NAME, apply_window_icon

POLL_INTERVAL_MS = 120

TAB_SERVICES = "services"
TAB_DOCKER = "docker"

# Shelling out to the docker CLI is far more expensive than a port scan, so the
# container list is refreshed on its own slower clock instead of once per scan,
# and slower still while the Docker tab is not the one being looked at — the
# only thing the other tab needs from it is the number beside its name.
DOCKER_REFRESH_SECONDS = 6.0
DOCKER_IDLE_REFRESH_SECONDS = 45.0

LOOSE_GROUP = "단독 컨테이너"
STOPPED_GROUP = "중지됨"


class CuttingBoardWindow:
    """The board: every development service running right now, as tiles."""

    _TAB_LABELS = {TAB_SERVICES: "SERVICES", TAB_DOCKER: "DOCKER"}

    def __init__(
        self,
        root: tk.Tk,
        *,
        controller: ScanController | None,
        terminator: Any,
        settings_store: SettingsStore,
        initial_snapshot: WorkspaceSnapshot | None = None,
        assets_dir: Path | None = None,
        auto_close_seconds: float | None = None,
        container_source: Callable[[], ContainerListing] = list_containers,
    ) -> None:
        self.root = root
        self.controller = controller
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

        root.title(APP_NAME)
        root.geometry(self.settings.window_geometry)
        root.minsize(560, 420)
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
            self.root.after(max(100, int(auto_close_seconds * 1000)), self.close)

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

        header = tk.Frame(self.root, bg=theme.CANVAS, height=52)
        header.pack(fill="x", side="top")
        header.pack_propagate(False)

        # The title bar already carries the name and the icon, so the header
        # only says which board is open and what is on it.
        left = tk.Frame(header, bg=theme.CANVAS)
        left.pack(side="left", padx=(theme.TILE_PAD + 6, 0), fill="y")

        self._tabs: dict[str, tuple[tk.Frame, tk.Label, tk.Frame]] = {}
        for key, label in ((TAB_SERVICES, "SERVICES"), (TAB_DOCKER, "DOCKER")):
            self._tabs[key] = self._build_tab(left, key, label)

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

    def _build_settings_button(self, parent: tk.Misc) -> tk.Canvas:
        """A gear drawn from canvas primitives, in the style of the other marks.

        The body is one polygon whose radius alternates between the tip and
        the root of a tooth; the bore is a disc painted back in the canvas
        colour, because Tk has no way to cut a hole out of a polygon.
        """
        size = 20
        centre = size / 2
        button = tk.Canvas(
            parent,
            width=size,
            height=size,
            bg=theme.CANVAS,
            highlightthickness=0,
            bd=0,
            cursor="hand2",
        )

        teeth = 8
        points: list[float] = []
        for index in range(teeth):
            base = 2 * math.pi * index / teeth
            # Half-widths in radians: the tip is narrower than the root, so the
            # flanks of each tooth slope outwards.
            for radius, offset in ((6.6, -0.30), (9.4, -0.17), (9.4, 0.17), (6.6, 0.30)):
                points.append(centre + math.cos(base + offset) * radius)
                points.append(centre + math.sin(base + offset) * radius)
        button.create_polygon(points, fill=theme.TEXT_DIM, outline="", tags="gear")
        button.create_oval(
            centre - 3.8,
            centre - 3.8,
            centre + 3.8,
            centre + 3.8,
            fill=theme.CANVAS,
            outline="",
        )

        button.bind("<Enter>", lambda _event: button.itemconfigure("gear", fill=theme.ACCENT))
        button.bind("<Leave>", lambda _event: button.itemconfigure("gear", fill=theme.TEXT_DIM))
        button.bind("<Button-1>", lambda _event: self._show_settings())
        return button

    def _build_tab(
        self,
        parent: tk.Misc,
        key: str,
        label: str,
    ) -> tuple[tk.Frame, tk.Label, tk.Frame]:
        """One entry in the tab strip: a label over its own underline."""
        holder = tk.Frame(parent, bg=theme.CANVAS)
        holder.pack(side="left", fill="y", padx=(0, 4))

        text = tk.Label(
            holder,
            text=label,
            bg=theme.CANVAS,
            fg=theme.TEXT_DIM,
            font=self.fonts["label"],
            cursor="hand2",
            padx=10,
            pady=6,
        )
        text.pack(side="top", pady=(12, 0))

        underline = tk.Frame(holder, bg=theme.CANVAS, height=2)
        underline.pack(side="top", fill="x", pady=(6, 0))

        for widget in (holder, text):
            widget.bind("<Button-1>", lambda _event, target=key: self._select_tab(target))
        return holder, text, underline

    def _select_tab(self, key: str) -> None:
        if key == self.tab:
            return
        self.tab = key
        self.scroll.canvas.yview_moveto(0.0)
        if key == TAB_DOCKER:
            self._request_containers(force=True)
        self.render()

    def _paint_tabs(self, counts: dict[str, int]) -> None:
        for key, (_holder, text, underline) in self._tabs.items():
            active = key == self.tab
            count = counts.get(key)
            suffix = "" if count is None else f"  {count}"
            text.configure(
                text=f"{self._TAB_LABELS[key]}{suffix}",
                fg=theme.TEXT if active else theme.TEXT_DIM,
            )
            underline.configure(bg=theme.ACCENT if active else theme.CANVAS)

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
        self._drain_action_results()
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
            self._paint_tabs({})
            self._draw_body(("loading",), lambda: self._render_empty("서비스를 찾는 중"))
            return

        services = visible_services(snapshot)
        self._paint_tabs(
            {
                TAB_SERVICES: len(services),
                TAB_DOCKER: self._docker_tab_count(snapshot),
            }
        )
        self.status.configure(
            text=f"{snapshot.scan_duration_ms} ms  ·  {len(snapshot.services)} listeners seen"
        )

        if self.tab == TAB_DOCKER:
            self._render_docker(snapshot)
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
            tile.grid(row=index // columns, column=index % columns, sticky="w")
            self._tiles.append(tile)

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
        except Exception:  # list_containers is total, but a thread must not die silently
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
                ).grid(row=index // columns, column=index % columns, sticky="w")

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
            on_interval_change=self._apply_scan_interval,
        )

    def _apply_scan_interval(self, seconds: float) -> None:
        """Persist a new scan interval and hand it to the running scan loop."""
        self.settings.scan_interval_seconds = seconds
        self.settings_store.save(self.settings)
        if self.controller is not None:  # --demo runs without a scanner behind it
            self.controller.set_interval(seconds)

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
        except Exception as exc:  # the board must survive a failed action
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

    def close(self) -> None:
        try:
            self.settings.window_geometry = self.root.winfo_geometry()
        except tk.TclError:
            pass
        self.settings_store.save(self.settings)
        if self.controller is not None:
            self.controller.close()
        self.executor.shutdown(wait=False, cancel_futures=True)
        try:
            self.root.destroy()
        except tk.TclError:
            pass


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

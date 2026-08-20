from __future__ import annotations

import argparse
import json
import os
import platform
import signal
import sys
import time
from pathlib import Path
from typing import Sequence

from cutting_board import __version__
from cutting_board.constants import APP_NAME, DEFAULT_SCAN_INTERVAL_SECONDS
from cutting_board.controller import ScanController
from cutting_board.demo import DemoProcessTerminator, demo_containers, demo_snapshot
from cutting_board.models import WorkspaceSnapshot
from cutting_board.presentation import visible_services
from cutting_board.scanner.docker import list_containers
from cutting_board.scanner.linux import LinuxServiceScanner
from cutting_board.services.settings import SettingsStore
from cutting_board.services.termination import ProcessTerminator


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="cutting-board",
        description="IDE-independent local Services workspace for Ubuntu.",
    )
    parser.add_argument("--version", action="version", version=f"{APP_NAME} {__version__}")
    parser.add_argument(
        "--snapshot",
        action="store_true",
        help="scan once without opening the GUI",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="with --snapshot, print machine-readable JSON",
    )
    parser.add_argument(
        "--containers",
        action="store_true",
        help="with --snapshot, also list Docker and Podman plumbing",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        dest="include_all",
        help="with --snapshot, list every TCP listener including desktop and system ones",
    )
    parser.add_argument(
        "--demo",
        action="store_true",
        help="open the GUI with deterministic demonstration data",
    )
    parser.add_argument(
        "--auto-close",
        type=float,
        metavar="SECONDS",
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--scan-interval",
        type=float,
        metavar="SECONDS",
        help="override the saved scan interval for this run",
    )
    parser.add_argument(
        "--settings-file",
        type=Path,
        help=argparse.SUPPRESS,
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if not sys.platform.startswith("linux"):
        print(
            f"{APP_NAME} {__version__} currently supports Linux only; "
            "the scanner boundary is prepared for a future macOS implementation.",
            file=sys.stderr,
        )
        return 2

    if args.snapshot:
        return _run_snapshot(
            containers=args.containers, include_all=args.include_all, as_json=args.json
        )

    return _run_gui(args)


def _run_snapshot(*, containers: bool, include_all: bool, as_json: bool) -> int:
    scanner = LinuxServiceScanner()
    snapshot = scanner.scan()
    if include_all:
        services = snapshot.services
    else:
        services = visible_services(snapshot, show_containers=containers)
    if as_json:
        payload = snapshot.to_dict()
        keep = {service.id for service in services}
        payload["services"] = [item for item in payload["services"] if item["id"] in keep]
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        _print_snapshot(snapshot, services)
    return 0 if not snapshot.errors else 1


def _print_snapshot(snapshot: WorkspaceSnapshot, services: tuple) -> None:
    print(f"{APP_NAME} {__version__} · {platform.platform()}")
    print(
        f"scanned {len(snapshot.services)} service(s), {snapshot.endpoint_count} endpoint(s) "
        f"in {snapshot.scan_duration_ms} ms"
    )
    if snapshot.errors:
        for error in snapshot.errors:
            print(f"warning: {error}", file=sys.stderr)
    if not services:
        print("No development services are running.")
        return
    print(f"{'PROJECT':20} {'SERVICE':22} {'TECH':12} {'PID':>7} {'PORTS':14} COMMAND")
    print("-" * 116)
    for service in services:
        project = service.project.name if service.project else "(unassigned)"
        pid = str(service.process.pid) if service.process else "?"
        ports = ",".join(str(port) for port in service.unique_ports)
        command = service.process.command_display if service.process else "owner unknown"
        print(
            f"{project[:20]:20} {service.display_name[:22]:22} {service.tech[:12]:12} "
            f"{pid:>7} {ports[:14]:14} {command[:32]}"
        )


def _run_gui(args: argparse.Namespace) -> int:
    try:
        import tkinter as tk
    except ImportError:
        print(
            "Tkinter is not installed. On Ubuntu 24.04 run: sudo apt install python3-tk",
            file=sys.stderr,
        )
        return 3

    from cutting_board.ui.main_window import CuttingBoardWindow

    try:
        # The class name has to be handed to Tk up front: it becomes WM_CLASS,
        # which is how the desktop shell ties the window to the .desktop entry
        # and therefore to the application icon.
        root = tk.Tk(className="CuttingBoard")
    except tk.TclError as exc:
        print(f"Could not open the desktop display: {exc}", file=sys.stderr)
        return 4

    settings_store = SettingsStore(args.settings_file)
    settings = settings_store.load()
    interval = args.scan_interval if args.scan_interval is not None else settings.scan_interval_seconds
    interval = max(0.75, min(30.0, float(interval or DEFAULT_SCAN_INTERVAL_SECONDS)))
    settings.scan_interval_seconds = interval
    settings_store.save(settings)

    if args.demo:
        controller = None
        initial_snapshot = demo_snapshot()
        terminator = DemoProcessTerminator()
        container_source = demo_containers
    else:
        controller = ScanController(LinuxServiceScanner(), interval_seconds=interval)
        initial_snapshot = None
        terminator = ProcessTerminator()
        container_source = list_containers

    window = CuttingBoardWindow(
        root,
        controller=controller,
        terminator=terminator,
        settings_store=settings_store,
        initial_snapshot=initial_snapshot,
        assets_dir=find_assets_dir(),
        auto_close_seconds=args.auto_close,
        container_source=container_source,
    )
    _close_on_signal(window)
    try:
        root.mainloop()
    except KeyboardInterrupt:
        try:
            root.destroy()
        except tk.TclError:
            pass
    return 0


def _close_on_signal(window: "CuttingBoardWindow") -> None:
    """Shut down cleanly when the process is asked to stop.

    Tk never returns to Python while it is inside ``mainloop``, so a signal
    handler would normally be deferred indefinitely. The window's event pump
    runs on a timer, which gives the interpreter a moment to run the handler,
    so the settings — the window geometry above all — are still saved when a
    session ends or a development watcher restarts the process.
    """

    def handle(signum: int, frame: object) -> None:
        del signum, frame
        window.close()

    for number in (signal.SIGTERM, signal.SIGINT):
        try:
            signal.signal(number, handle)
        except (OSError, ValueError):
            # Not the main thread, or a platform without that signal.
            pass


def find_assets_dir() -> Path | None:
    candidates: list[Path] = []
    override = os.environ.get("CUTTING_BOARD_ASSETS")
    if override:
        candidates.append(Path(override).expanduser())
    candidates.extend(
        [
            Path("/usr/share/cutting-board/assets"),
            Path(__file__).resolve().parents[2] / "assets",
            Path(__file__).resolve().parent / "assets",
        ]
    )
    for candidate in candidates:
        if (candidate / "cutting-board.png").is_file():
            return candidate
    return None


if __name__ == "__main__":
    raise SystemExit(main())

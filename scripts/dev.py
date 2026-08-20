#!/usr/bin/env python3
"""Run the board and restart it whenever a source file changes.

Tk has no hot-reload story worth trusting: reloading a module leaves live
widgets bound to the old classes, and the scan thread and icon caches would
have to be rebuilt anyway. Restarting the process is the honest version of the
same thing, and it takes about a second.

The window reopens where it was, because the application saves its geometry on
the way out and this script asks it to stop with SIGTERM rather than killing
it. Arguments are passed straight through, so `dev.py --demo` works.

No dependency: `inotify` tooling is not installed on the target machine, and a
poll over a few dozen files costs nothing next to a scan sweep.
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WATCHED_DIRECTORIES = (ROOT / "src" / "cutting_board", ROOT / "assets")
WATCHED_SUFFIXES = (".py", ".png", ".argb")
POLL_SECONDS = 0.2
DEBOUNCE_SECONDS = 0.35
STOP_GRACE_SECONDS = 8.0


@dataclass(frozen=True, slots=True)
class FileStamp:
    """The cheap file identity needed by the polling watcher."""

    modified_ns: int
    size: int


@dataclass(slots=True)
class ChangeDebouncer:
    """Collect file changes until saves have been quiet for a short period."""

    delay_seconds: float
    _paths: set[Path] = field(default_factory=set)
    _last_change_at: float | None = None

    def observe(self, paths: Iterable[Path], now: float) -> list[Path]:
        changed = tuple(paths)
        if changed:
            self._paths.update(changed)
            self._last_change_at = now

        if self._last_change_at is None or now - self._last_change_at < self.delay_seconds:
            return []

        ready = sorted(self._paths)
        self._paths.clear()
        self._last_change_at = None
        return ready


def snapshot(
    directories: Iterable[Path] = WATCHED_DIRECTORIES,
    suffixes: Iterable[str] = WATCHED_SUFFIXES,
) -> dict[Path, FileStamp]:
    """Modification times of everything worth restarting for."""
    stamps: dict[Path, FileStamp] = {}
    watched_suffixes = frozenset(suffixes)
    for directory in directories:
        if not directory.is_dir():
            continue
        for path in directory.rglob("*"):
            if path.suffix not in watched_suffixes or "__pycache__" in path.parts:
                continue
            try:
                status = path.stat()
                stamps[path] = FileStamp(status.st_mtime_ns, status.st_size)
            except OSError:
                # Vanished between the walk and the stat; the next poll settles it.
                continue
    return stamps


def changes(before: dict[Path, FileStamp], after: dict[Path, FileStamp]) -> list[Path]:
    touched = [path for path, stamp in after.items() if before.get(path) != stamp]
    touched.extend(path for path in before if path not in after)
    return sorted(touched)


def app_arguments(arguments: list[str]) -> list[str]:
    """Accept either direct app flags or the conventional ``--`` separator."""
    if arguments[:1] == ["--"]:
        return arguments[1:]
    return arguments


def start(arguments: list[str]) -> subprocess.Popen[bytes]:
    environment = dict(os.environ)
    existing = environment.get("PYTHONPATH")
    environment["PYTHONPATH"] = f"{ROOT / 'src'}{os.pathsep + existing if existing else ''}"
    return subprocess.Popen(
        [sys.executable, "-m", "cutting_board", *arguments],
        env=environment,
        cwd=ROOT,
    )


def stop(process: subprocess.Popen[bytes]) -> None:
    """Ask the window to close, so it saves its geometry before it goes."""
    if process.poll() is not None:
        return
    process.send_signal(signal.SIGTERM)
    try:
        process.wait(timeout=STOP_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        print("child did not stop after SIGTERM; killing the exact child process", file=sys.stderr)
        process.kill()
        process.wait()


def _stop_on_signal() -> None:
    """Turn a SIGTERM into the same unwind Ctrl-C already gets.

    Without this the watcher dies where it stands and leaves the window it
    started running with nothing watching it.
    """

    def handle(signum: int, frame: object) -> None:
        del signum, frame
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, handle)


def main(arguments: list[str]) -> int:
    _stop_on_signal()
    print(f"watching {', '.join(str(d.relative_to(ROOT)) for d in WATCHED_DIRECTORIES)}")
    stamps = snapshot()
    debouncer = ChangeDebouncer(DEBOUNCE_SECONDS)
    child_arguments = app_arguments(arguments)
    process: subprocess.Popen[bytes] | None = start(child_arguments)
    reported_exit = False

    try:
        while True:
            time.sleep(POLL_SECONDS)

            if process is not None and (code := process.poll()) is not None:
                if code == 0:
                    # The window was closed on purpose, so stop watching too.
                    return 0
                if not reported_exit:
                    # A crash, most likely a syntax or import error in the latest save.
                    print(f"child exited with {code}; waiting for a source change", flush=True)
                    reported_exit = True
                process = None

            current = snapshot()
            touched = changes(stamps, current)
            stamps = current
            ready = debouncer.observe(touched, time.monotonic())
            if not ready:
                continue
            names = ", ".join(str(path.relative_to(ROOT)) for path in ready[:3])
            extra = f" (+{len(ready) - 3})" if len(ready) > 3 else ""
            print(f"restarting — {names}{extra}", flush=True)
            if process is not None:
                stop(process)
            process = start(child_arguments)
            reported_exit = False
    except KeyboardInterrupt:
        return 0
    finally:
        if process is not None:
            stop(process)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

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
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WATCHED_DIRECTORIES = (ROOT / "src", ROOT / "assets")
WATCHED_SUFFIXES = (".py", ".png", ".argb")
POLL_SECONDS = 0.4
STOP_GRACE_SECONDS = 3.0


def snapshot() -> dict[Path, float]:
    """Modification times of everything worth restarting for."""
    stamps: dict[Path, float] = {}
    for directory in WATCHED_DIRECTORIES:
        if not directory.is_dir():
            continue
        for path in directory.rglob("*"):
            if path.suffix not in WATCHED_SUFFIXES or "__pycache__" in path.parts:
                continue
            try:
                stamps[path] = path.stat().st_mtime
            except OSError:
                # Vanished between the walk and the stat; the next poll settles it.
                continue
    return stamps


def changes(before: dict[Path, float], after: dict[Path, float]) -> list[Path]:
    touched = [path for path, stamp in after.items() if before.get(path) != stamp]
    touched.extend(path for path in before if path not in after)
    return sorted(touched)


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
    process = start(arguments)

    try:
        while True:
            time.sleep(POLL_SECONDS)

            code = process.poll()
            if code is not None:
                if code == 0:
                    # The window was closed on purpose, so stop watching too.
                    return 0
                # A crash, most likely a syntax error in what was just saved.
                # Keep watching: the next save is probably the fix.
                print(f"exited with {code}; waiting for a change", flush=True)
                while process.poll() is not None:
                    time.sleep(POLL_SECONDS)
                    current = snapshot()
                    touched = changes(stamps, current)
                    if touched:
                        stamps = current
                        print(f"restarting — {touched[0].relative_to(ROOT)}", flush=True)
                        process = start(arguments)
                continue

            current = snapshot()
            touched = changes(stamps, current)
            if not touched:
                continue
            stamps = current
            names = ", ".join(str(path.relative_to(ROOT)) for path in touched[:3])
            extra = f" (+{len(touched) - 3})" if len(touched) > 3 else ""
            print(f"restarting — {names}{extra}", flush=True)
            stop(process)
            process = start(arguments)
    except KeyboardInterrupt:
        return 0
    finally:
        stop(process)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

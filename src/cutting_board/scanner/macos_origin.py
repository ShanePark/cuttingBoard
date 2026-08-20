"""macOS process readers for the shared launcher-origin detector."""

from __future__ import annotations

from collections.abc import Callable, Mapping

import psutil

from cutting_board.scanner.origin import Origin, OriginDetector, ProcessEntry


def read_process_entry(
    pid: int,
    process_factory: Callable[[int], psutil.Process] = psutil.Process,
) -> ProcessEntry | None:
    """Read the process fields used by ``OriginDetector`` through psutil."""
    try:
        process = process_factory(pid)
        return ProcessEntry(
            name=process.name(),
            ppid=process.ppid(),
            start_ticks=int(process.create_time() * 1000),
            command=tuple(process.cmdline()),
        )
    except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess, OSError, ValueError):
        return None


def read_process_environ(
    pid: int,
    process_factory: Callable[[int], psutil.Process] = psutil.Process,
) -> Mapping[str, str] | None:
    """Read a process environment when macOS permits it."""
    try:
        return {str(name): str(value) for name, value in process_factory(pid).environ().items()}
    except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess, OSError):
        return None


_DETECTOR = OriginDetector(read_process_entry, read_process_environ)


def detect_origin(pid: int, *, create_time: float | None = None) -> Origin:
    """Resolve a launcher using the shared rules and macOS psutil readers."""
    return _DETECTOR.detect(pid, create_time=create_time)


def clear_cache() -> None:
    """Forget cached macOS launcher answers."""
    _DETECTOR.clear_cache()

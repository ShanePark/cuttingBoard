from __future__ import annotations

import queue
import threading
from dataclasses import dataclass

from cutting_board.constants import MAX_SCAN_INTERVAL_SECONDS, MIN_SCAN_INTERVAL_SECONDS
from cutting_board.models import WorkspaceSnapshot
from cutting_board.scanner.base import ServiceScanner


@dataclass(frozen=True, slots=True)
class ScanEvent:
    snapshot: WorkspaceSnapshot | None = None
    error: str | None = None


class ScanController:
    """Run scans on one background thread for exactly the GUI lifetime."""

    def __init__(self, scanner: ServiceScanner, interval_seconds: float) -> None:
        self._scanner = scanner
        # Guards the interval alone: it is the one field the GUI thread writes
        # while the worker thread is reading it.
        self._interval_lock = threading.Lock()
        self._interval_seconds = interval_seconds
        self._events: queue.Queue[ScanEvent] = queue.Queue(maxsize=4)
        self._stop = threading.Event()
        self._wake = threading.Event()
        self._thread = threading.Thread(target=self._run, name="cutting-board-scanner", daemon=True)
        self._started = False

    @property
    def events(self) -> queue.Queue[ScanEvent]:
        return self._events

    def start(self) -> None:
        if self._started:
            return
        self._started = True
        self._thread.start()

    def refresh(self) -> None:
        self._wake.set()

    def set_interval(self, seconds: float) -> None:
        """Adopt a new scan interval, clamped to the supported range.

        The worker is parked on ``_wake`` for whatever the old interval was, so
        it is nudged awake: the new interval applies from now instead of once
        the outstanding sleep expires.
        """
        clamped = min(MAX_SCAN_INTERVAL_SECONDS, max(MIN_SCAN_INTERVAL_SECONDS, float(seconds)))
        with self._interval_lock:
            if clamped == self._interval_seconds:
                return
            self._interval_seconds = clamped
        self._wake.set()

    @property
    def interval_seconds(self) -> float:
        with self._interval_lock:
            return self._interval_seconds

    def close(self, timeout: float = 3.0) -> None:
        self._stop.set()
        self._wake.set()
        if self._started and self._thread.is_alive():
            self._thread.join(timeout=timeout)

    def _run(self) -> None:
        while not self._stop.is_set():
            try:
                event = ScanEvent(snapshot=self._scanner.scan())
            except Exception as exc:  # UI must survive unexpected scanner failures
                event = ScanEvent(error=f"Scan failed: {exc}")
            self._put_latest(event)
            self._wake.wait(self.interval_seconds)
            self._wake.clear()

    def _put_latest(self, event: ScanEvent) -> None:
        try:
            self._events.put_nowait(event)
            return
        except queue.Full:
            pass
        try:
            self._events.get_nowait()
        except queue.Empty:
            pass
        try:
            self._events.put_nowait(event)
        except queue.Full:
            pass

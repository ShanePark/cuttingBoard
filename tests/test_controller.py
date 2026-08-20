from __future__ import annotations

import queue
import threading
import time
import unittest

from cutting_board.constants import MAX_SCAN_INTERVAL_SECONDS, MIN_SCAN_INTERVAL_SECONDS
from cutting_board.controller import ScanController, ScanEvent
from tests.helpers import make_snapshot


class CountingScanner:
    def __init__(self) -> None:
        self.calls = 0
        self.called = threading.Event()

    def scan(self):
        self.calls += 1
        self.called.set()
        return make_snapshot()


class ControllerTests(unittest.TestCase):
    def test_start_refresh_and_close_follow_gui_lifetime_contract(self) -> None:
        scanner = CountingScanner()
        controller = ScanController(scanner, interval_seconds=30)
        controller.start()
        self.assertTrue(scanner.called.wait(timeout=2))
        first = controller.events.get(timeout=2)
        self.assertIsNotNone(first.snapshot)

        scanner.called.clear()
        controller.refresh()
        self.assertTrue(scanner.called.wait(timeout=2))
        second = controller.events.get(timeout=2)
        self.assertIsNotNone(second.snapshot)

        controller.close(timeout=2)
        calls_after_close = scanner.calls
        time.sleep(0.05)
        self.assertEqual(scanner.calls, calls_after_close)

    def test_queue_keeps_a_recent_event_when_full(self) -> None:
        scanner = CountingScanner()
        controller = ScanController(scanner, interval_seconds=30)
        for _ in range(10):
            controller._put_latest(ScanEvent(snapshot=make_snapshot()))
        events = []
        while True:
            try:
                events.append(controller.events.get_nowait())
            except queue.Empty:
                break
        self.assertGreaterEqual(len(events), 1)
        self.assertLessEqual(len(events), 4)

    def test_set_interval_clamps_to_the_supported_range(self) -> None:
        controller = ScanController(CountingScanner(), interval_seconds=5)

        controller.set_interval(0.01)
        self.assertEqual(controller.interval_seconds, MIN_SCAN_INTERVAL_SECONDS)

        controller.set_interval(9000)
        self.assertEqual(controller.interval_seconds, MAX_SCAN_INTERVAL_SECONDS)

        controller.set_interval(3)
        self.assertEqual(controller.interval_seconds, 3.0)

    def test_set_interval_wakes_the_worker_so_the_new_clock_applies_now(self) -> None:
        scanner = CountingScanner()
        controller = ScanController(scanner, interval_seconds=30)
        controller.start()
        self.assertTrue(scanner.called.wait(timeout=2))

        scanner.called.clear()
        controller.set_interval(1)
        self.assertTrue(scanner.called.wait(timeout=2))
        controller.close(timeout=2)


if __name__ == "__main__":
    unittest.main()

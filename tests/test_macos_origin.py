from __future__ import annotations

import unittest

import psutil

from cutting_board.scanner.macos_origin import read_process_entry, read_process_environ


class FakeProcess:
    def name(self) -> str:
        return "node"

    def ppid(self) -> int:
        return 12

    def create_time(self) -> float:
        return 123.456

    def cmdline(self) -> list[str]:
        return ["node", "server.js"]

    def environ(self) -> dict[str, str]:
        return {"TERM_PROGRAM": "ghostty"}


class VanishedProcess:
    def __init__(self) -> None:
        raise psutil.NoSuchProcess(42)


class MacOSOriginReaderTests(unittest.TestCase):
    def test_process_reader_builds_shared_entry(self) -> None:
        entry = read_process_entry(42, lambda _pid: FakeProcess())  # type: ignore[arg-type]
        self.assertIsNotNone(entry)
        assert entry is not None
        self.assertEqual(entry.name, "node")
        self.assertEqual(entry.ppid, 12)
        self.assertEqual(entry.start_ticks, 123456)
        self.assertEqual(entry.command, ("node", "server.js"))

    def test_environment_reader_returns_psutil_environment(self) -> None:
        environ = read_process_environ(42, lambda _pid: FakeProcess())  # type: ignore[arg-type]
        self.assertEqual(environ, {"TERM_PROGRAM": "ghostty"})

    def test_readers_tolerate_a_vanished_process(self) -> None:
        self.assertIsNone(read_process_entry(42, lambda _pid: VanishedProcess()))  # type: ignore[arg-type]
        self.assertIsNone(read_process_environ(42, lambda _pid: VanishedProcess()))  # type: ignore[arg-type]


if __name__ == "__main__":
    unittest.main()

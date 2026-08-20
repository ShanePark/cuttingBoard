from __future__ import annotations

import os
import socket
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import psutil

from cutting_board.models import EndpointScope, Ownership
from cutting_board.scanner.linux import LinuxServiceScanner


class FakeMemory:
    rss = 64 * 1024 * 1024


class FakeUids:
    effective = os.geteuid()


class FakeProcess:
    pid = 4242

    def create_time(self) -> float:
        return 1000.0

    def name(self) -> str:
        return "node"

    def cmdline(self) -> list[str]:
        return ["node", "node_modules/vite/bin/vite.js"]

    def exe(self) -> str:
        return "/usr/bin/node"

    def cwd(self) -> str:
        return self.working_directory

    def username(self) -> str:
        return "developer"

    def ppid(self) -> int:
        return 10

    def uids(self) -> FakeUids:
        return FakeUids()

    def memory_info(self) -> FakeMemory:
        return FakeMemory()

    def cpu_percent(self, interval: float | None = None) -> float:
        return 2.5


class ScannerTests(unittest.TestCase):
    def test_same_pid_is_grouped_and_enriched(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".git").mkdir()
            (root / "package.json").write_text('{"name":"demo-web"}', encoding="utf-8")
            FakeProcess.working_directory = str(root)
            connections = [
                SimpleNamespace(
                    status=psutil.CONN_LISTEN,
                    laddr=("127.0.0.1", 5173),
                    family=socket.AF_INET,
                    pid=4242,
                ),
                SimpleNamespace(
                    status=psutil.CONN_LISTEN,
                    laddr=("0.0.0.0", 5174),
                    family=socket.AF_INET,
                    pid=4242,
                ),
                SimpleNamespace(
                    status=psutil.CONN_ESTABLISHED,
                    laddr=("127.0.0.1", 60000),
                    family=socket.AF_INET,
                    pid=4242,
                ),
            ]
            scanner = LinuxServiceScanner(
                net_connections=lambda **_kwargs: connections,
                process_factory=lambda _pid: FakeProcess(),
            )

            first = scanner.scan()
            second = scanner.scan()

            self.assertEqual(len(first.services), 1)
            service = second.services[0]
            self.assertEqual([endpoint.port for endpoint in service.endpoints], [5173, 5174])
            self.assertEqual(service.project.name if service.project else None, "demo-web")
            self.assertEqual(service.display_name, "Vite")
            self.assertEqual(service.ownership, Ownership.CURRENT_USER)
            self.assertTrue(service.can_terminate)
            self.assertEqual(service.process.cpu_percent if service.process else None, 2.5)
            self.assertIn("모든 네트워크 인터페이스에서 수신 중", service.warnings)
            self.assertEqual(service.endpoints[0].scope, EndpointScope.LOOPBACK)

    def test_pidless_global_row_is_enriched_from_current_user_process_scan(self) -> None:
        global_rows = [
            SimpleNamespace(
                status=psutil.CONN_LISTEN,
                laddr=("127.0.0.1", 7555),
                family=socket.AF_INET,
                pid=None,
            )
        ]
        process_rows = [
            SimpleNamespace(
                status=psutil.CONN_LISTEN,
                laddr=("127.0.0.1", 7555),
                family=socket.AF_INET,
                pid=4242,
            )
        ]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".git").mkdir()
            FakeProcess.working_directory = str(root)
            scanner = LinuxServiceScanner(
                net_connections=lambda **_kwargs: global_rows,
                process_factory=lambda _pid: FakeProcess(),
            )
            scanner._read_connections_per_process = lambda: process_rows  # type: ignore[method-assign]
            snapshot = scanner.scan()
        self.assertEqual(len(snapshot.services), 1)
        self.assertEqual(snapshot.services[0].process.pid if snapshot.services[0].process else None, 4242)

    def test_unknown_pid_is_not_dropped(self) -> None:
        connections = [
            SimpleNamespace(
                status=psutil.CONN_LISTEN,
                laddr=("127.0.0.1", 7000),
                family=socket.AF_INET,
                pid=None,
            )
        ]
        scanner = LinuxServiceScanner(
            net_connections=lambda **_kwargs: connections,
            process_factory=lambda _pid: FakeProcess(),
        )
        scanner._read_connections_per_process = lambda: []  # type: ignore[method-assign]
        snapshot = scanner.scan()
        self.assertEqual(len(snapshot.services), 1)
        self.assertIsNone(snapshot.services[0].process)
        self.assertFalse(snapshot.services[0].can_terminate)
        self.assertEqual(snapshot.services[0].lowest_port, 7000)

    def test_snapshot_dictionary_is_json_safe(self) -> None:
        connections = [
            SimpleNamespace(
                status=psutil.CONN_LISTEN,
                laddr=("127.0.0.1", 7001),
                family=socket.AF_INET,
                pid=None,
            )
        ]
        scanner = LinuxServiceScanner(net_connections=lambda **_kwargs: connections)
        scanner._read_connections_per_process = lambda: []  # type: ignore[method-assign]
        payload = scanner.scan().to_dict()
        self.assertEqual(payload["services"][0]["endpoints"][0]["scope"], "loopback")


if __name__ == "__main__":
    unittest.main()

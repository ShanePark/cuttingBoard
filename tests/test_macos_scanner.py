from __future__ import annotations

import os
import socket
import subprocess
import tempfile
import unittest
from collections.abc import Sequence
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path
from unittest import mock

from cutting_board import app
from cutting_board.models import EndpointScope, Ownership
from cutting_board.scanner.linux import LinuxServiceScanner
from cutting_board.scanner.macos import LSOF_PATH, MacOSServiceScanner, parse_lsof_output


class FakeMemory:
    rss = 32 * 1024 * 1024


class FakeUids:
    effective = os.geteuid()


class FakeProcess:
    def create_time(self) -> float:
        return 1000.0

    def name(self) -> str:
        return "node"

    def cmdline(self) -> list[str]:
        return ["node", "node_modules/vite/bin/vite.js"]

    def exe(self) -> str:
        return "/usr/local/bin/node"

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
        return 1.5


LSOF_OUTPUT = """\
p4242
f10
tIPv4
n127.0.0.1:5173
f11
tIPv4
n*:5174
f12
tIPv6
n[::1]:5175
f13
tIPv6
n*:5176
p5252
f8
tIPv4
nlocalhost:8000
"""


class LsofParserTests(unittest.TestCase):
    def test_parses_ipv4_ipv6_wildcard_and_localhost(self) -> None:
        connections = parse_lsof_output(LSOF_OUTPUT)

        self.assertEqual(
            [(item.pid, item.family, item.laddr) for item in connections],
            [
                (4242, socket.AF_INET, ("127.0.0.1", 5173)),
                (4242, socket.AF_INET, ("0.0.0.0", 5174)),
                (4242, socket.AF_INET6, ("::1", 5175)),
                (4242, socket.AF_INET6, ("::", 5176)),
                (5252, socket.AF_INET, ("127.0.0.1", 8000)),
            ],
        )

    def test_ignores_malformed_records(self) -> None:
        output = "pbad\ntIPv4\nn127.0.0.1:3000\np42\nnmissing-port\nn*:0\nn*:70000\n"
        self.assertEqual(parse_lsof_output(output), [])


class MacOSScannerTests(unittest.TestCase):
    def test_groups_lsof_rows_and_reuses_process_enrichment(self) -> None:
        commands: list[tuple[str, ...]] = []

        def run_lsof(command: Sequence[str]) -> subprocess.CompletedProcess[str]:
            commands.append(tuple(command))
            return subprocess.CompletedProcess(command, 0, stdout=LSOF_OUTPUT, stderr="")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".git").mkdir()
            (root / "package.json").write_text('{"name":"mac-web"}', encoding="utf-8")
            FakeProcess.working_directory = str(root)
            scanner = MacOSServiceScanner(
                lsof_runner=run_lsof,
                process_factory=lambda _pid: FakeProcess(),  # type: ignore[arg-type]
            )
            snapshot = scanner.scan()

        self.assertEqual(commands[0][0], LSOF_PATH)
        service = next(item for item in snapshot.services if item.process and item.process.pid == 4242)
        self.assertEqual(service.project.name if service.project else None, "mac-web")
        self.assertEqual(service.display_name, "Vite")
        self.assertEqual(service.ownership, Ownership.CURRENT_USER)
        self.assertTrue(service.can_terminate)
        self.assertEqual(service.unique_ports, (5173, 5174, 5175, 5176))
        self.assertEqual(
            [endpoint.scope for endpoint in service.endpoints],
            [
                EndpointScope.LOOPBACK,
                EndpointScope.WILDCARD,
                EndpointScope.LOOPBACK,
                EndpointScope.WILDCARD,
            ],
        )

    def test_no_lsof_matches_is_an_empty_successful_scan(self) -> None:
        scanner = MacOSServiceScanner(
            lsof_runner=lambda command: subprocess.CompletedProcess(
                command, 1, stdout="", stderr=""
            )
        )
        snapshot = scanner.scan()
        self.assertEqual(snapshot.services, ())
        self.assertEqual(snapshot.errors, ())

    def test_lsof_failure_is_reported_without_raising(self) -> None:
        scanner = MacOSServiceScanner(
            lsof_runner=lambda command: subprocess.CompletedProcess(
                command, 2, stdout="", stderr="permission denied"
            )
        )
        snapshot = scanner.scan()
        self.assertEqual(snapshot.services, ())
        self.assertIn("permission denied", snapshot.errors[0])

    def test_lsof_exit_one_with_an_error_is_not_treated_as_no_matches(self) -> None:
        scanner = MacOSServiceScanner(
            lsof_runner=lambda command: subprocess.CompletedProcess(
                command, 1, stdout="", stderr="permission denied"
            )
        )
        snapshot = scanner.scan()
        self.assertEqual(snapshot.services, ())
        self.assertIn("permission denied", snapshot.errors[0])

    def test_platform_factory_selects_macos_and_preserves_unsupported_result(self) -> None:
        self.assertIsInstance(app._scanner_for_platform("darwin"), MacOSServiceScanner)
        self.assertIsInstance(app._scanner_for_platform("linux"), LinuxServiceScanner)
        self.assertIsNone(app._scanner_for_platform("win32"))

        stderr = StringIO()
        with mock.patch.object(app.sys, "platform", "win32"), redirect_stderr(stderr):
            result = app.main(["--snapshot"])
        self.assertEqual(result, 2)
        self.assertIn("not supported", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()

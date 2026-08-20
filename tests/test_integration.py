from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from cutting_board.scanner.linux import LinuxServiceScanner
from cutting_board.scanner.macos import MacOSServiceScanner
from cutting_board.services.termination import ProcessTerminator

_LISTENER_CODE = r"""
import signal
import socket
import time
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("127.0.0.1", 0))
sock.listen()
print(sock.getsockname()[1], flush=True)
while True:
    time.sleep(1)
"""


class LiveScannerIntegrationTests(unittest.TestCase):
    def test_discover_project_and_terminate_real_listener(self) -> None:
        if not (sys.platform.startswith("linux") or sys.platform == "darwin"):
            self.skipTest("Linux/macOS integration test")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "agent-workspace"
            cwd = root / "apps" / "api"
            cwd.mkdir(parents=True)
            (root / ".git").mkdir()
            (root / "package.json").write_text(
                json.dumps({"name": "integration-workspace"}),
                encoding="utf-8",
            )
            process = subprocess.Popen(
                [sys.executable, "-u", "-c", _LISTENER_CODE],
                cwd=cwd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                start_new_session=True,
            )
            try:
                assert process.stdout is not None
                port = int(process.stdout.readline().strip())
                scanner = MacOSServiceScanner() if sys.platform == "darwin" else LinuxServiceScanner()
                service = None
                for _attempt in range(20):
                    snapshot = scanner.scan()
                    service = next(
                        (
                            item
                            for item in snapshot.services
                            if item.process is not None
                            and item.process.pid == process.pid
                            and port in {endpoint.port for endpoint in item.endpoints}
                        ),
                        None,
                    )
                    if service is not None:
                        break
                    time.sleep(0.1)

                self.assertIsNotNone(service, f"listener PID {process.pid} port {port} was not discovered")
                assert service is not None and service.process is not None
                self.assertEqual(service.project.name if service.project else None, "integration-workspace")
                self.assertTrue(service.can_terminate)

                result = ProcessTerminator().terminate(
                    service.process.pid,
                    service.process.create_time,
                    timeout_seconds=2.0,
                )
                self.assertTrue(result.success, result)
                process.wait(timeout=3)
                self.assertIsNotNone(process.returncode)
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=2)
                if process.stdout is not None:
                    process.stdout.close()
                if process.stderr is not None:
                    process.stderr.close()


if __name__ == "__main__":
    unittest.main()

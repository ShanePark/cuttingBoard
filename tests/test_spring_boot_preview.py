from __future__ import annotations

import os
import socket
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import psutil

from cutting_board.models import Endpoint, EndpointScope
from cutting_board.scanner.linux import LinuxServiceScanner
from cutting_board.scanner.spring_boot import resolve_spring_boot_browser_url


def endpoint(port: int) -> Endpoint:
    return Endpoint("IPv4", "127.0.0.1", port, EndpointScope.LOOPBACK)


class SpringBootPreviewTests(unittest.TestCase):
    def test_ignores_livereload_port_and_appends_properties_context_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "application.properties").write_text(
                "server.port=48080\nserver.servlet.context-path=/oasis\n",
                encoding="utf-8",
            )

            url = resolve_spring_boot_browser_url(
                command=("java", "org.springframework.boot.loader.launch.JarLauncher"),
                environment={},
                cwd=str(root),
                project_root=str(root),
                endpoints=(endpoint(35729), endpoint(48080)),
            )

        self.assertEqual(url, "http://localhost:48080/oasis/")

    def test_active_profile_yaml_and_environment_placeholders_are_resolved(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            resources = Path(directory) / "src/main/resources"
            resources.mkdir(parents=True)
            (resources / "application.properties").write_text(
                "spring.profiles.active=local\n",
                encoding="utf-8",
            )
            (resources / "application-local.yml").write_text(
                "server:\n  port: 48080\n  servlet:\n    context-path: ${APP_CONTEXT:/}\n",
                encoding="utf-8",
            )

            url = resolve_spring_boot_browser_url(
                command=("java", "-jar", "app.jar"),
                environment={"APP_CONTEXT": "/oasis"},
                cwd=directory,
                project_root=directory,
                endpoints=(endpoint(35729), endpoint(48080)),
            )

        self.assertEqual(url, "http://localhost:48080/oasis/")

    def test_command_line_overrides_environment_and_config(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "application.properties").write_text(
                "server.port=8080\nserver.servlet.context-path=/config\n",
                encoding="utf-8",
            )

            url = resolve_spring_boot_browser_url(
                command=(
                    "java",
                    "-Dserver.port=48080",
                    "--server.servlet.context-path=/command",
                ),
                environment={
                    "SERVER_PORT": "8081",
                    "SERVER_SERVLET_CONTEXT_PATH": "/environment",
                },
                cwd=str(root),
                project_root=str(root),
                endpoints=(endpoint(8080), endpoint(8081), endpoint(48080)),
            )

        self.assertEqual(url, "http://localhost:48080/command/")

    def test_configured_custom_livereload_and_management_ports_are_excluded(self) -> None:
        url = resolve_spring_boot_browser_url(
            command=("java", "-jar", "app.jar"),
            environment={
                "SERVER_PORT": "48080",
                "SPRING_DEVTOOLS_LIVERELOAD_PORT": "36000",
                "MANAGEMENT_SERVER_PORT": "48081",
            },
            cwd=None,
            project_root=None,
            endpoints=(endpoint(36000), endpoint(48081), endpoint(48080)),
        )
        self.assertEqual(url, "http://localhost:48080")

    def test_livereload_listener_alone_has_no_browser_url(self) -> None:
        self.assertIsNone(
            resolve_spring_boot_browser_url(
                command=("java", "-jar", "app.jar"),
                environment={},
                cwd=None,
                project_root=None,
                endpoints=(endpoint(35729),),
            )
        )


class _Memory:
    rss = 32 * 1024 * 1024


class _Uids:
    effective = os.geteuid()


class _SpringProcess:
    pid = 4242

    def __init__(self, cwd: str) -> None:
        self._cwd = cwd

    def create_time(self) -> float:
        return 1000.0

    def name(self) -> str:
        return "java"

    def cmdline(self) -> list[str]:
        return ["java", "-jar", "spring-boot-app.jar"]

    def exe(self) -> str:
        return "/usr/bin/java"

    def cwd(self) -> str:
        return self._cwd

    def username(self) -> str:
        return "developer"

    def ppid(self) -> int:
        return 10

    def uids(self) -> _Uids:
        return _Uids()

    def memory_info(self) -> _Memory:
        return _Memory()

    def cpu_percent(self, interval: float | None = None) -> float:
        return 1.0

    def environ(self) -> dict[str, str]:
        return {"SERVER_SERVLET_CONTEXT_PATH": "/oasis"}


class SpringBootScannerIntegrationTests(unittest.TestCase):
    def test_scanner_attaches_resolved_browser_url_to_spring_service(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".git").mkdir()
            connections = [
                SimpleNamespace(
                    status=psutil.CONN_LISTEN,
                    laddr=("127.0.0.1", port),
                    family=socket.AF_INET,
                    pid=4242,
                )
                for port in (35729, 48080)
            ]
            scanner = LinuxServiceScanner(
                net_connections=lambda **_kwargs: connections,
                process_factory=lambda _pid: _SpringProcess(directory),
            )

            service = scanner.scan().services[0]

        self.assertEqual(service.display_name, "Spring Boot")
        self.assertEqual(service.browser_url(), "http://localhost:48080/oasis/")


if __name__ == "__main__":
    unittest.main()

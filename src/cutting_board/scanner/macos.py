"""TCP listener discovery for macOS."""

from __future__ import annotations

import socket
import subprocess
from collections.abc import Callable, Sequence
from types import SimpleNamespace

import psutil

from cutting_board.scanner.linux import LinuxServiceScanner
from cutting_board.scanner.macos_origin import detect_origin
from cutting_board.scanner.project import ProjectResolver

LSOF_PATH = "/usr/sbin/lsof"
LsofResult = subprocess.CompletedProcess[str]
LsofRunner = Callable[[Sequence[str]], LsofResult]


def _run_lsof(command: Sequence[str]) -> LsofResult:
    return subprocess.run(command, capture_output=True, text=True, check=False)


class MacOSServiceScanner(LinuxServiceScanner):
    """Discover macOS listeners with lsof and reuse the shared enrichment."""

    def __init__(
        self,
        project_resolver: ProjectResolver | None = None,
        lsof_runner: LsofRunner | None = None,
        process_factory: Callable[[int], psutil.Process] | None = None,
    ) -> None:
        super().__init__(
            project_resolver=project_resolver,
            process_factory=process_factory,
            origin_detector=detect_origin,
        )
        self._lsof_runner = lsof_runner or _run_lsof

    def _read_connections(self, errors: list[str]) -> list[SimpleNamespace]:
        command = (LSOF_PATH, "-nP", "-a", "-iTCP", "-sTCP:LISTEN", "-Fptn")
        try:
            result = self._lsof_runner(command)
        except OSError as exc:
            errors.append(f"macOS listener lookup failed: {exc}")
            return []

        # lsof exits with 1 when its selection matched no files. That is an
        # empty workspace, not a scan failure.
        no_matches = result.returncode == 1 and not result.stdout.strip() and not result.stderr.strip()
        if result.returncode != 0 and not no_matches:
            detail = result.stderr.strip() or f"exit code {result.returncode}"
            errors.append(f"macOS listener lookup failed: {detail}")
        return parse_lsof_output(result.stdout)


def parse_lsof_output(output: str) -> list[SimpleNamespace]:
    """Turn lsof field output into psutil-compatible listener rows."""
    connections: list[SimpleNamespace] = []
    pid: int | None = None
    family: socket.AddressFamily | None = None

    for raw_line in output.splitlines():
        if not raw_line:
            continue
        field, value = raw_line[0], raw_line[1:]
        if field == "p":
            try:
                candidate = int(value)
            except ValueError:
                pid = None
            else:
                pid = candidate if candidate > 0 else None
        elif field == "f":
            family = None
        elif field == "t":
            if value == "IPv6":
                family = socket.AF_INET6
            elif value == "IPv4":
                family = socket.AF_INET
        elif field == "n" and pid is not None:
            endpoint = _parse_endpoint(value, family)
            if endpoint is None:
                continue
            address, port, resolved_family = endpoint
            connections.append(
                SimpleNamespace(
                    status="LISTEN",
                    laddr=(address, port),
                    family=resolved_family,
                    pid=pid,
                )
            )
    return connections


def _parse_endpoint(
    value: str,
    family: socket.AddressFamily | None,
) -> tuple[str, int, socket.AddressFamily] | None:
    if value.startswith("["):
        close = value.rfind("]:")
        if close < 0:
            return None
        address, port_text = value[1:close], value[close + 2 :]
    else:
        try:
            address, port_text = value.rsplit(":", 1)
        except ValueError:
            return None

    try:
        port = int(port_text)
    except ValueError:
        return None
    if not 0 < port <= 65535:
        return None

    resolved_family = family or (socket.AF_INET6 if ":" in address else socket.AF_INET)
    if address == "*":
        address = "::" if resolved_family == socket.AF_INET6 else "0.0.0.0"
    elif address.casefold() == "localhost":
        address = "::1" if resolved_family == socket.AF_INET6 else "127.0.0.1"
    return address, port, resolved_family

from __future__ import annotations

import getpass
import ipaddress
import os
import pwd
import shlex
import socket
import time
from collections import defaultdict
from collections.abc import Callable
from types import SimpleNamespace
from typing import Any, TypeVar

import psutil

from cutting_board.constants import SENSITIVE_FLAGS
from cutting_board.models import (
    Endpoint,
    EndpointScope,
    Ownership,
    ProcessInfo,
    ServiceSnapshot,
    ServiceStatus,
    WorkspaceSnapshot,
)
from cutting_board.scanner.classifier import classify_service
from cutting_board.scanner.origin import UNKNOWN_ORIGIN, detect_origin
from cutting_board.scanner.project import ProjectResolver
from cutting_board.scanner.relevance import Relevance, relevance_of

T = TypeVar("T")


class LinuxServiceScanner:
    """Discover TCP listeners and enrich them with Linux process metadata."""

    def __init__(
        self,
        project_resolver: ProjectResolver | None = None,
        net_connections: Callable[..., list[Any]] | None = None,
        process_factory: Callable[[int], psutil.Process] | None = None,
    ) -> None:
        self._resolver = project_resolver or ProjectResolver()
        self._net_connections = net_connections or psutil.net_connections
        self._process_factory = process_factory or psutil.Process
        self._current_uid = os.geteuid()
        self._current_username = self._username_for_uid(self._current_uid)
        self._process_cache: dict[tuple[int, int], psutil.Process] = {}
        self._cpu_primed: set[tuple[int, int]] = set()

    def scan(self) -> WorkspaceSnapshot:
        started = time.monotonic()
        errors: list[str] = []
        try:
            connections = self._read_connections(errors)
        except Exception as exc:  # defensive boundary around OS inspection
            errors.append(f"리스너 스캔 실패: {exc}")
            connections = []

        grouped: dict[int | str, set[Endpoint]] = defaultdict(set)
        for connection in connections:
            endpoint = self._to_endpoint(connection)
            if endpoint is None:
                continue
            pid = getattr(connection, "pid", None)
            key: int | str = pid if isinstance(pid, int) and pid > 0 else f"endpoint:{endpoint.key}"
            grouped[key].add(endpoint)

        services: list[ServiceSnapshot] = []
        active_process_keys: set[tuple[int, int]] = set()
        for key, endpoint_set in grouped.items():
            endpoints = tuple(sorted(endpoint_set, key=lambda item: (item.port, item.address, item.family)))
            if isinstance(key, int):
                service, process_key = self._service_for_pid(key, endpoints)
                if process_key is not None:
                    active_process_keys.add(process_key)
                services.append(service)
            else:
                services.append(self._unknown_service(str(key), endpoints))

        self._process_cache = {
            key: process for key, process in self._process_cache.items() if key in active_process_keys
        }
        self._cpu_primed.intersection_update(active_process_keys)

        services.sort(key=self._service_sort_key)
        duration_ms = int((time.monotonic() - started) * 1000)
        return WorkspaceSnapshot(
            scanned_at=time.time(),
            scan_duration_ms=duration_ms,
            services=tuple(services),
            current_username=self._current_username,
            current_uid=self._current_uid,
            errors=tuple(dict.fromkeys(errors)),
        )

    def _read_connections(self, errors: list[str]) -> list[Any]:
        try:
            system_connections = list(self._net_connections(kind="tcp"))
        except psutil.AccessDenied:
            errors.append("시스템 전체 소켓 정보 접근이 제한되어 확인 가능한 프로세스만 표시합니다.")
            return self._read_connections_per_process()

        # Linux can return listener rows while withholding their PID. Enrich only
        # those rows from processes the current user is allowed to inspect.
        unresolved_ports = {
            key[2]
            for item in system_connections
            if getattr(item, "pid", None) is None
            and (key := self._connection_endpoint_key(item)) is not None
        }
        if not unresolved_ports:
            return system_connections

        # Walking every process is expensive, and it can only ever resolve a
        # socket this user owns. The kernel publishes the owning UID, so skip
        # the walk when every hidden listener belongs to somebody else.
        # Only skip when every hidden listener is positively known to belong to
        # somebody else. A port missing from the table is treated as unknown,
        # so the walk still runs rather than silently dropping an owner.
        socket_uids = self._listener_uids()
        if all(
            socket_uids.get(port, self._current_uid) != self._current_uid
            for port in unresolved_ports
        ):
            return system_connections

        process_connections = self._read_connections_per_process()
        owned_by_endpoint: dict[tuple[Any, str, int], Any] = {}
        for item in process_connections:
            endpoint_key = self._connection_endpoint_key(item)
            if endpoint_key is not None and getattr(item, "pid", None):
                owned_by_endpoint[endpoint_key] = item

        merged: list[Any] = []
        seen_process_rows: set[tuple[Any, str, int, int]] = set()
        for item in system_connections:
            endpoint_key = self._connection_endpoint_key(item)
            replacement = owned_by_endpoint.get(endpoint_key) if endpoint_key is not None else None
            selected = replacement if getattr(item, "pid", None) is None and replacement is not None else item
            pid = getattr(selected, "pid", None)
            if endpoint_key is not None and isinstance(pid, int):
                dedupe_key = (*endpoint_key, pid)
                if dedupe_key in seen_process_rows:
                    continue
                seen_process_rows.add(dedupe_key)
            merged.append(selected)
        return merged

    @staticmethod
    def _connection_endpoint_key(connection: Any) -> tuple[Any, str, int] | None:
        if getattr(connection, "status", None) not in {psutil.CONN_LISTEN, "LISTEN"}:
            return None
        local_address = getattr(connection, "laddr", None)
        if not local_address:
            return None
        try:
            address_value = getattr(local_address, "ip", None)
            port_value = getattr(local_address, "port", None)
            if address_value is None:
                address_value = local_address[0]
            if port_value is None:
                port_value = local_address[1]
            address = str(address_value)
            port = int(port_value)
        except (AttributeError, IndexError, TypeError, ValueError):
            return None
        return getattr(connection, "family", None), address, port

    @staticmethod
    def _listener_uids() -> dict[int, int]:
        """Map each listening TCP port to the UID that owns the socket.

        ``/proc/net/tcp`` exposes the owner of every socket even when the
        process behind it is not readable, which is exactly the information
        psutil withholds when it reports a listener with no PID.
        """
        listen_state = "0A"
        owners: dict[int, int] = {}
        for name in ("/proc/net/tcp", "/proc/net/tcp6"):
            try:
                with open(name, encoding="ascii") as handle:
                    next(handle, None)  # column header
                    for line in handle:
                        fields = line.split()
                        if len(fields) < 8 or fields[3] != listen_state:
                            continue
                        try:
                            port = int(fields[1].rsplit(":", 1)[1], 16)
                            owners.setdefault(port, int(fields[7]))
                        except (IndexError, ValueError):
                            continue
            except OSError:
                continue
        return owners

    def _read_connections_per_process(self) -> list[Any]:
        connections: list[Any] = []
        # No attrs: psutil.as_dict() treats an empty list as "collect
        # everything", which reads memory_maps() for every process and turns a
        # sub-second sweep into several seconds. Only uids() is needed here.
        for process in psutil.process_iter():
            try:
                if int(process.uids().effective) != self._current_uid:
                    continue
                getter = getattr(process, "net_connections", None) or getattr(process, "connections")
                for connection in getter(kind="tcp"):
                    if getattr(connection, "pid", None) is None:
                        connection = SimpleNamespace(
                            family=getattr(connection, "family", None),
                            type=getattr(connection, "type", None),
                            laddr=getattr(connection, "laddr", None),
                            raddr=getattr(connection, "raddr", None),
                            status=getattr(connection, "status", None),
                            pid=process.pid,
                        )
                    connections.append(connection)
            except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess, OSError):
                continue
        return connections

    @staticmethod
    def _to_endpoint(connection: Any) -> Endpoint | None:
        status = getattr(connection, "status", None)
        if status not in {psutil.CONN_LISTEN, "LISTEN"}:
            return None
        local_address = getattr(connection, "laddr", None)
        if not local_address:
            return None
        try:
            address_value = getattr(local_address, "ip", None)
            port_value = getattr(local_address, "port", None)
            if address_value is None:
                address_value = local_address[0]
            if port_value is None:
                port_value = local_address[1]
            address = str(address_value)
            port = int(port_value)
        except (AttributeError, IndexError, TypeError, ValueError):
            return None
        family_value = getattr(connection, "family", None)
        family = "IPv6" if family_value == socket.AF_INET6 else "IPv4"
        return Endpoint(
            family=family,
            address=address,
            port=port,
            scope=_endpoint_scope(address),
        )

    def _service_for_pid(
        self, pid: int, endpoints: tuple[Endpoint, ...]
    ) -> tuple[ServiceSnapshot, tuple[int, int] | None]:
        warnings: list[str] = []
        if any(endpoint.scope == EndpointScope.WILDCARD for endpoint in endpoints):
            warnings.append("모든 네트워크 인터페이스에서 수신 중")

        try:
            process = self._process_factory(pid)
            create_time = float(process.create_time())
        except psutil.NoSuchProcess:
            return self._unknown_service(f"pid:{pid}", endpoints, pid=pid), None
        except (psutil.AccessDenied, psutil.ZombieProcess, OSError) as exc:
            warnings.append(f"프로세스 정보 접근 제한: {type(exc).__name__}")
            return self._unknown_service(f"pid:{pid}", endpoints, pid=pid, warnings=warnings), None

        process_key = (pid, int(create_time * 1000))
        cached_process = self._process_cache.get(process_key)
        if cached_process is None:
            cached_process = process
            self._process_cache[process_key] = cached_process

        name = self._safe(cached_process.name, f"PID {pid}")
        command = tuple(self._safe(cached_process.cmdline, []))
        executable = self._safe(cached_process.exe, None)
        cwd = self._safe(cached_process.cwd, None)
        username = self._safe(cached_process.username, None)
        ppid = self._safe(cached_process.ppid, None)
        uid = self._read_uid(cached_process)
        memory_bytes = self._read_memory(cached_process)
        uptime_seconds = max(0, int(time.time() - create_time))
        cpu_percent = self._read_cpu(cached_process, process_key)

        missing = []
        if not command:
            missing.append("명령")
        if cwd is None:
            missing.append("작업 디렉터리")
        if executable is None:
            missing.append("실행 파일")
        if missing:
            warnings.append("확인 불가: " + ", ".join(missing))

        ownership = self._ownership(uid)
        can_terminate = (
            ownership == Ownership.CURRENT_USER and pid not in {1, os.getpid()} and pid > 1
        )
        project_resolution = self._resolver.resolve(cwd)
        project = project_resolution.project if project_resolution else None
        package_name = project_resolution.package_name if project_resolution else None
        project_name = project.name if project else None
        classification = classify_service(
            process_name=name,
            command=command,
            executable=executable,
            package_name=package_name,
            project_name=project_name,
        )
        relevance = relevance_of(
            ownership=ownership,
            process_name=name,
            executable=executable,
            command=command,
            tech=classification.tech,
            specific=classification.specific,
            has_project=project is not None,
        )
        # Only worth asking for services that made it onto the board; noise is
        # discarded before anything is rendered.
        origin = (
            detect_origin(pid, create_time=create_time)
            if relevance is not Relevance.NOISE
            else UNKNOWN_ORIGIN
        )

        process_info = ProcessInfo(
            pid=pid,
            create_time=create_time,
            ppid=ppid,
            name=name,
            executable=executable,
            command=command,
            command_display=redact_command(command),
            cwd=cwd,
            username=username,
            uid=uid,
            uptime_seconds=uptime_seconds,
            cpu_percent=cpu_percent,
            memory_bytes=memory_bytes,
        )
        status = ServiceStatus.HEALTHY if not missing else ServiceStatus.LIMITED
        return (
            ServiceSnapshot(
                id=f"process:{pid}:{process_key[1]}",
                display_name=classification.name,
                category=classification.category,
                endpoints=endpoints,
                process=process_info,
                project=project,
                ownership=ownership,
                can_terminate=can_terminate,
                status=status,
                warnings=tuple(warnings),
                tech=classification.tech,
                relevance=relevance.value,
                origin_id=origin.id,
                origin_label=origin.label,
                origin_kind=origin.kind.value,
            ),
            process_key,
        )

    def _unknown_service(
        self,
        key: str,
        endpoints: tuple[Endpoint, ...],
        pid: int | None = None,
        warnings: list[str] | None = None,
    ) -> ServiceSnapshot:
        warning_items = list(warnings or [])
        warning_items.append("소유 프로세스를 확인할 수 없음")
        if any(endpoint.scope == EndpointScope.WILDCARD for endpoint in endpoints):
            warning_items.append("Listens on all network interfaces")
        port_label = ", ".join(str(endpoint.port) for endpoint in endpoints)
        return ServiceSnapshot(
            id=f"unknown:{key}",
            display_name=f"Unidentified listener · {port_label}",
            category=classify_service("", (), None, None, None).category,
            endpoints=endpoints,
            process=None,
            project=None,
            ownership=Ownership.UNKNOWN,
            can_terminate=False,
            status=ServiceStatus.UNKNOWN_PROCESS,
            warnings=tuple(dict.fromkeys(warning_items)),
            tech="service",
            relevance=Relevance.NOISE.value,
        )

    def _read_cpu(self, process: psutil.Process, key: tuple[int, int]) -> float | None:
        try:
            value = float(process.cpu_percent(interval=None))
            if key not in self._cpu_primed:
                self._cpu_primed.add(key)
                return None
            return max(0.0, value)
        except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess, OSError):
            return None

    @staticmethod
    def _read_uid(process: psutil.Process) -> int | None:
        try:
            return int(process.uids().effective)
        except (AttributeError, psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess, OSError):
            return None

    @staticmethod
    def _read_memory(process: psutil.Process) -> int | None:
        try:
            return int(process.memory_info().rss)
        except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess, OSError):
            return None

    @staticmethod
    def _safe(call: Callable[[], T], default: T) -> T:
        try:
            return call()
        except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess, OSError, ValueError):
            return default

    def _ownership(self, uid: int | None) -> Ownership:
        if uid is None:
            return Ownership.UNKNOWN
        if uid == self._current_uid:
            return Ownership.CURRENT_USER
        return Ownership.OTHER_USER

    @staticmethod
    def _service_sort_key(service: ServiceSnapshot) -> tuple[int, str, int, str]:
        ownership_rank = {
            Ownership.CURRENT_USER: 0,
            Ownership.UNKNOWN: 1,
            Ownership.OTHER_USER: 2,
        }[service.ownership]
        project_name = service.project.name.casefold() if service.project else "~unassigned"
        return ownership_rank, project_name, service.lowest_port, service.display_name.casefold()

    @staticmethod
    def _username_for_uid(uid: int) -> str:
        try:
            return pwd.getpwuid(uid).pw_name
        except (KeyError, OSError):
            return getpass.getuser()


def _endpoint_scope(address: str) -> EndpointScope:
    if address in {"0.0.0.0", "::", "0:0:0:0:0:0:0:0"}:
        return EndpointScope.WILDCARD
    try:
        parsed = ipaddress.ip_address(address)
    except ValueError:
        return EndpointScope.UNKNOWN
    if parsed.is_loopback:
        return EndpointScope.LOOPBACK
    return EndpointScope.INTERFACE


def redact_command(command: tuple[str, ...]) -> str:
    if not command:
        return ""
    redacted: list[str] = []
    hide_next = False
    for token in command:
        if hide_next:
            redacted.append("••••")
            hide_next = False
            continue

        lowered = token.casefold()
        if lowered in SENSITIVE_FLAGS:
            redacted.append(token)
            hide_next = True
            continue

        if "=" in token:
            key, value = token.split("=", 1)
            normalized_key = key.casefold().replace("_", "-")
            if normalized_key in SENSITIVE_FLAGS or any(
                word in normalized_key for word in ("token", "password", "secret", "api-key")
            ):
                redacted.append(f"{key}=••••")
                continue
            if value and key.isupper() and any(
                word in normalized_key for word in ("credential", "auth", "database-url")
            ):
                redacted.append(f"{key}=••••")
                continue

        redacted.append(token)

    rendered = shlex.join(redacted)
    if len(rendered) > 800:
        return rendered[:797] + "..."
    return rendered

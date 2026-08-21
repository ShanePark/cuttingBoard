from __future__ import annotations

from dataclasses import asdict, dataclass, field
from enum import Enum
from typing import Any


class EndpointScope(str, Enum):
    LOOPBACK = "loopback"
    WILDCARD = "wildcard"
    INTERFACE = "interface"
    UNKNOWN = "unknown"


class ServiceCategory(str, Enum):
    WEB = "web"
    API = "api"
    DATABASE = "database"
    CACHE = "cache"
    PROXY = "proxy"
    RUNTIME = "runtime"
    OTHER = "other"


class Ownership(str, Enum):
    CURRENT_USER = "current_user"
    OTHER_USER = "other_user"
    UNKNOWN = "unknown"


class ServiceStatus(str, Enum):
    HEALTHY = "healthy"
    LIMITED = "limited"
    UNKNOWN_PROCESS = "unknown_process"


@dataclass(frozen=True, slots=True)
class Endpoint:
    family: str
    address: str
    port: int
    scope: EndpointScope
    protocol: str = "TCP"

    @property
    def key(self) -> str:
        return f"{self.protocol}:{self.family}:{self.address}:{self.port}"

    @property
    def display_address(self) -> str:
        if self.address in {"0.0.0.0", "::"}:
            return "all interfaces"
        return self.address


@dataclass(frozen=True, slots=True)
class ProjectInfo:
    id: str
    name: str
    root_path: str
    detection_source: str
    package_name: str | None = None
    package_path: str | None = None


@dataclass(frozen=True, slots=True)
class ProcessInfo:
    pid: int
    create_time: float
    ppid: int | None
    name: str
    executable: str | None
    command: tuple[str, ...]
    command_display: str
    cwd: str | None
    username: str | None
    uid: int | None
    uptime_seconds: int | None
    cpu_percent: float | None
    memory_bytes: int | None


@dataclass(frozen=True, slots=True)
class ServiceSnapshot:
    id: str
    display_name: str
    category: ServiceCategory
    endpoints: tuple[Endpoint, ...]
    process: ProcessInfo | None
    project: ProjectInfo | None
    ownership: Ownership
    can_terminate: bool
    status: ServiceStatus
    warnings: tuple[str, ...] = field(default_factory=tuple)
    tech: str = "service"
    """Artwork id for the brand mark shown on the tile."""
    relevance: str = "dev"
    """Why the listener is on the board; see scanner.relevance.Relevance."""
    origin_id: str = ""
    """Stable slug for whatever launched the process; see scanner.origin."""
    origin_label: str = ""
    """The short name shown on the tile badge; empty when nothing was found."""
    origin_kind: str = "unknown"
    """Category of the launcher: agent, ide, terminal, system or unknown."""
    browser_url_hint: str | None = None
    """Locally-derived URL when a framework needs more than a port guess."""

    @property
    def is_system(self) -> bool:
        return self.ownership == Ownership.OTHER_USER

    @property
    def unique_ports(self) -> tuple[int, ...]:
        """Ports without the IPv4/IPv6 duplicate of a dual-stack listener."""
        return tuple(dict.fromkeys(endpoint.port for endpoint in self.endpoints))

    @property
    def lowest_port(self) -> int:
        return min((endpoint.port for endpoint in self.endpoints), default=65536)

    @property
    def searchable_text(self) -> str:
        values: list[str] = [self.display_name, self.category.value]
        values.extend(str(endpoint.port) for endpoint in self.endpoints)
        values.extend(endpoint.address for endpoint in self.endpoints)
        if self.project:
            values.extend([self.project.name, self.project.root_path])
            if self.project.package_name:
                values.append(self.project.package_name)
        if self.process:
            values.extend(
                [
                    str(self.process.pid),
                    self.process.name,
                    self.process.command_display,
                    self.process.cwd or "",
                    self.process.executable or "",
                    self.process.username or "",
                ]
            )
        return " ".join(values).casefold()

    def browser_url(self) -> str | None:
        if self.browser_url_hint:
            return self.browser_url_hint
        web_ports = {
            80, 443, 3000, 3001, 3100, 4000, 4200, 5000, 5173,
            5174, 8000, 8001, 8080, 8081, 8443, 8888, 9000, 9443,
        }
        if self.category not in {ServiceCategory.WEB, ServiceCategory.API, ServiceCategory.PROXY} and not any(
            endpoint.port in web_ports for endpoint in self.endpoints
        ):
            return None
        endpoint = next(
            (
                item
                for item in self.endpoints
                if item.scope in {EndpointScope.LOOPBACK, EndpointScope.WILDCARD}
            ),
            self.endpoints[0] if self.endpoints else None,
        )
        if endpoint is None:
            return None
        scheme = "https" if endpoint.port in {443, 8443, 9443} else "http"
        return f"{scheme}://localhost:{endpoint.port}"


@dataclass(frozen=True, slots=True)
class WorkspaceSnapshot:
    scanned_at: float
    scan_duration_ms: int
    services: tuple[ServiceSnapshot, ...]
    current_username: str
    current_uid: int
    errors: tuple[str, ...] = field(default_factory=tuple)

    @property
    def project_count(self) -> int:
        return len(
            {
                service.project.id
                for service in self.services
                if service.project and not service.is_system
            }
        )

    @property
    def endpoint_count(self) -> int:
        return sum(len(service.endpoints) for service in self.services)

    @property
    def system_service_count(self) -> int:
        return sum(1 for service in self.services if service.is_system)

    def to_dict(self) -> dict[str, Any]:
        payload = _json_safe(asdict(self))
        for service in payload.get("services", []):
            process = service.get("process") if isinstance(service, dict) else None
            if isinstance(process, dict):
                process.pop("command", None)
        return payload


@dataclass(frozen=True, slots=True)
class TerminationResult:
    status: str
    message: str
    pid: int
    force: bool

    @property
    def success(self) -> bool:
        return self.status in {"terminated", "already_exited"}


def _json_safe(value: Any) -> Any:
    if isinstance(value, Enum):
        return value.value
    if isinstance(value, dict):
        return {str(key): _json_safe(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [_json_safe(item) for item in value]
    return value

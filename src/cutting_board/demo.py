from __future__ import annotations

import os
import time

from cutting_board.models import (
    Endpoint,
    EndpointScope,
    Ownership,
    ProcessInfo,
    ProjectInfo,
    ServiceCategory,
    ServiceSnapshot,
    ServiceStatus,
    TerminationResult,
    WorkspaceSnapshot,
)
from cutting_board.scanner.docker import ContainerInfo, ContainerListing
from cutting_board.scanner.origin import CLAUDE, CODEX, UNKNOWN_ORIGIN, VSCODE, Origin
from cutting_board.services.termination import ProcessTerminator


class DemoProcessTerminator(ProcessTerminator):
    """Keep demo controls representative without ever signaling a real PID."""

    def terminate(
        self,
        pid: int,
        expected_create_time: float,
        *,
        force: bool = False,
        timeout_seconds: float = 1.5,
    ) -> TerminationResult:
        del expected_create_time, timeout_seconds
        return TerminationResult(
            status="protected",
            message="데모 데이터에서는 실제 프로세스를 종료하지 않습니다.",
            pid=pid,
            force=force,
        )


def demo_containers() -> ContainerListing:
    """A stand-in for the docker CLI, so --demo never touches the real daemon."""
    return ContainerListing(
        containers=(
            ContainerInfo(
                id="9c2f41ab73de",
                name="shop-platform-postgres-1",
                image="postgres:16-alpine",
                state="running",
                status="Up 4 hours (healthy)",
                ports=(5432,),
                created_at="2026-08-20 09:12:33 +0900 KST",
                compose_project="shop-platform",
                compose_service="postgres",
            ),
            ContainerInfo(
                id="4ab80cf1d925",
                name="shop-platform-redis-1",
                image="redis:7-alpine",
                state="running",
                status="Up 4 hours",
                ports=(6379,),
                created_at="2026-08-20 09:12:33 +0900 KST",
                compose_project="shop-platform",
                compose_service="redis",
            ),
            ContainerInfo(
                id="1de77b0c4a3f",
                name="shop-platform-minio-1",
                image="minio/minio",
                state="running",
                status="Up 4 hours",
                ports=(9000, 9001),
                created_at="2026-08-20 09:12:33 +0900 KST",
                compose_project="shop-platform",
                compose_service="minio",
            ),
            ContainerInfo(
                id="6b1f0ad82c47",
                name="mailhog",
                image="mailhog/mailhog",
                state="exited",
                status="Exited (0) 2 hours ago",
                ports=(),
                created_at="2026-08-19 22:04:10 +0900 KST",
                compose_project=None,
                compose_service=None,
            ),
        )
    )


# Launchers for the demo board. Most services keep the unknown origin so the
# screenshots show both states rather than a badge on every tile.
_DEMO_ORIGINS = {
    "vite": CLAUDE,
    "mock": CLAUDE,
    "spring": CODEX,
    "uvicorn": CODEX,
    "storybook": VSCODE,
}


def demo_snapshot() -> WorkspaceSnapshot:
    """Return deterministic data used by screenshots and GUI smoke tests."""
    now = time.time()
    uid = os.geteuid()
    username = "developer"

    shop = ProjectInfo(
        id="project:demo-shop",
        name="shop-platform",
        root_path="/home/developer/work/shop-platform",
        detection_source=".git",
        package_name="shop-platform",
        package_path="/home/developer/work/shop-platform",
    )
    billing = ProjectInfo(
        id="project:demo-billing",
        name="billing-api",
        root_path="/home/developer/work/billing-api",
        detection_source=".git",
    )

    ml = ProjectInfo(
        id="project:demo-ml",
        name="ml-pipeline",
        root_path="/home/developer/work/ml-pipeline",
        detection_source="pyproject.toml",
    )

    # A spread of stacks, so the demo shows what the board looks like when a
    # few projects are running at once.
    specs = (
        ("vite", "Vite", "vite", ServiceCategory.WEB, shop, 18342, "node",
         ("node", "node_modules/vite/bin/vite.js", "--host", "127.0.0.1"),
         "apps/web", ((5173, EndpointScope.LOOPBACK),), 1.2, 184, 18 * 60),
        ("storybook", "Storybook", "storybook", ServiceCategory.WEB, shop, 18355, "node",
         ("node", "node_modules/.bin/storybook", "dev", "-p", "6006"),
         "apps/web", ((6006, EndpointScope.LOOPBACK),), 0.8, 210, 14 * 60),
        ("mock", "mock-api", "node", ServiceCategory.API, shop, 18420, "node",
         ("pnpm", "run", "mock"),
         "apps/mock-api", ((3100, EndpointScope.LOOPBACK),), 0.4, 72, 12 * 60),
        ("spring", "Spring Boot", "spring", ServiceCategory.API, billing, 18721, "java",
         ("./gradlew", "bootRun"),
         "", ((8080, EndpointScope.WILDCARD), (8081, EndpointScope.LOOPBACK)), 3.4, 512, 9 * 60),
        ("postgres", "PostgreSQL", "postgresql", ServiceCategory.DATABASE, billing, 19044, "postgres",
         ("postgres", "-D", "/home/developer/work/billing-api/.data/pg"),
         ".data/pg", ((5432, EndpointScope.LOOPBACK),), 0.3, 96, 46 * 60),
        ("redis", "Redis", "redis", ServiceCategory.CACHE, billing, 19102, "redis-server",
         ("redis-server", "127.0.0.1:6379"),
         "", ((6379, EndpointScope.LOOPBACK),), 0.1, 16, 31 * 60),
        ("uvicorn", "Uvicorn", "fastapi", ServiceCategory.API, ml, 20455, "python3",
         ("python3", "-m", "uvicorn", "serve:app", "--port", "8000"),
         "", ((8000, EndpointScope.LOOPBACK),), 2.6, 340, 5 * 60),
        ("jupyter", "Jupyter", "jupyter", ServiceCategory.RUNTIME, ml, 20470, "python3",
         ("jupyter-lab", "--port", "8888"),
         "notebooks", ((8888, EndpointScope.LOOPBACK),), 0.9, 288, 52 * 60),
        ("cargo", "Rust service", "rust", ServiceCategory.RUNTIME, None, 21880, "edge-proxy",
         ("cargo", "run", "--release"),
         "", ((7700, EndpointScope.LOOPBACK),), 0.7, 44, 3 * 60),
    )

    services = tuple(
        _service(
            identity=identity,
            name=name,
            tech=tech,
            category=category,
            project=project,
            pid=pid,
            ppid=pid - 40,
            process_name=process_name,
            command=command,
            cwd="/".join(
                part for part in ((project.root_path if project else "/home/developer/work/edge"), suffix) if part
            ),
            ports=tuple(("127.0.0.1" if scope is EndpointScope.LOOPBACK else "0.0.0.0", port, scope)
                        for port, scope in ports),
            uid=uid,
            username=username,
            cpu=cpu,
            memory=memory_mb * 1024 * 1024,
            uptime=uptime,
            warnings=("모든 네트워크 인터페이스에서 수신 중",)
            if any(scope is EndpointScope.WILDCARD for _, scope in ports)
            else (),
            origin=_DEMO_ORIGINS.get(identity, UNKNOWN_ORIGIN),
        )
        for identity, name, tech, category, project, pid, process_name, command, suffix, ports, cpu, memory_mb, uptime in specs
    ) + (
        _service(
            identity="docker-pg",
            name="Docker port proxy",
            tech="docker",
            category=ServiceCategory.PROXY,
            project=None,
            pid=4102,
            ppid=1,
            process_name="docker-proxy",
            command=("/usr/bin/docker-proxy", "-container-ip", "172.17.0.3", "-container-port", "5432"),
            cwd="/",
            ports=(("0.0.0.0", 55432, EndpointScope.WILDCARD),),
            uid=uid,
            username=username,
            cpu=0.0,
            memory=8 * 1024 * 1024,
            uptime=3 * 60 * 60,
            relevance="container",
        ),
    )

    return WorkspaceSnapshot(
        scanned_at=now,
        scan_duration_ms=63,
        services=services,
        current_username=username,
        current_uid=uid,
        errors=(),
    )


def _service(
    *,
    identity: str,
    name: str,
    tech: str,
    category: ServiceCategory,
    project: ProjectInfo | None,
    pid: int,
    ppid: int,
    process_name: str,
    command: tuple[str, ...],
    cwd: str,
    ports: tuple[tuple[str, int, EndpointScope], ...],
    uid: int,
    username: str,
    cpu: float,
    memory: int,
    uptime: int,
    warnings: tuple[str, ...] = (),
    ownership: Ownership = Ownership.CURRENT_USER,
    can_terminate: bool = True,
    relevance: str = "dev",
    origin: Origin = UNKNOWN_ORIGIN,
) -> ServiceSnapshot:
    endpoints = tuple(
        Endpoint(
            family="IPv6" if ":" in address else "IPv4",
            address=address,
            port=port,
            scope=scope,
        )
        for address, port, scope in ports
    )
    process = ProcessInfo(
        pid=pid,
        create_time=time.time() - uptime,
        ppid=ppid,
        name=process_name,
        executable=f"/usr/bin/{process_name}",
        command=command,
        command_display=" ".join(command),
        cwd=cwd,
        username=username,
        uid=uid,
        uptime_seconds=uptime,
        cpu_percent=cpu,
        memory_bytes=memory,
    )
    return ServiceSnapshot(
        id=f"demo:{identity}",
        display_name=name,
        category=category,
        endpoints=endpoints,
        process=process,
        project=project,
        ownership=ownership,
        can_terminate=can_terminate,
        status=ServiceStatus.HEALTHY,
        warnings=warnings,
        tech=tech,
        relevance=relevance,
        origin_id=origin.id,
        origin_label=origin.label,
        origin_kind=origin.kind.value,
    )

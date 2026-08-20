from __future__ import annotations

import os
import time
from pathlib import Path

from cutting_board.models import (
    Endpoint,
    EndpointScope,
    Ownership,
    ProcessInfo,
    ProjectInfo,
    ServiceCategory,
    ServiceSnapshot,
    ServiceStatus,
    WorkspaceSnapshot,
)


def make_service(
    *,
    service_id: str = "service:1",
    name: str = "Vite",
    port: int = 5173,
    project: ProjectInfo | None = None,
    ownership: Ownership = Ownership.CURRENT_USER,
    pid: int = 1234,
    tech: str = "vite",
    relevance: str = "dev",
) -> ServiceSnapshot:
    process = ProcessInfo(
        pid=pid,
        create_time=time.time() - 120,
        ppid=1,
        name="node",
        executable="/usr/bin/node",
        command=("pnpm", "run", "dev"),
        command_display="pnpm run dev",
        cwd=project.root_path if project else "/tmp/scratch",
        username="developer",
        uid=os.geteuid(),
        uptime_seconds=120,
        cpu_percent=1.25,
        memory_bytes=128 * 1024 * 1024,
    )
    return ServiceSnapshot(
        id=service_id,
        display_name=name,
        category=ServiceCategory.WEB,
        endpoints=(
            Endpoint(
                family="IPv4",
                address="127.0.0.1",
                port=port,
                scope=EndpointScope.LOOPBACK,
            ),
        ),
        process=process,
        project=project,
        ownership=ownership,
        can_terminate=ownership == Ownership.CURRENT_USER,
        status=ServiceStatus.HEALTHY,
        tech=tech,
        relevance=relevance,
    )


def make_project(name: str = "shop-web", path: str = "/tmp/shop-web") -> ProjectInfo:
    return ProjectInfo(
        id=f"project:{name}",
        name=name,
        root_path=path,
        detection_source=".git",
    )


def make_snapshot(*services: ServiceSnapshot) -> WorkspaceSnapshot:
    return WorkspaceSnapshot(
        scanned_at=time.time(),
        scan_duration_ms=10,
        services=tuple(services),
        current_username="developer",
        current_uid=os.geteuid(),
    )

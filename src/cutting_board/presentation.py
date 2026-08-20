from __future__ import annotations

from dataclasses import dataclass

from cutting_board.models import Ownership, ServiceSnapshot, WorkspaceSnapshot
from cutting_board.scanner.relevance import Relevance


@dataclass(frozen=True, slots=True)
class ServiceGroup:
    id: str
    name: str
    path: str | None
    source: str | None
    services: tuple[ServiceSnapshot, ...]
    system: bool = False
    unassigned: bool = False


def visible_services(
    snapshot: WorkspaceSnapshot,
    *,
    show_containers: bool = False,
    query: str = "",
) -> tuple[ServiceSnapshot, ...]:
    """The development services worth putting on the board.

    Desktop applications, system daemons and listeners owned by other users
    are classified as noise by the scanner and never appear. Container
    plumbing is real development infrastructure but rarely what a developer is
    looking for, so it is opt-in.
    """
    normalized_query = query.strip().casefold()
    result = []
    for service in snapshot.services:
        if service.relevance == Relevance.NOISE.value:
            continue
        if service.relevance == Relevance.CONTAINER.value and not show_containers:
            continue
        if normalized_query and normalized_query not in service.searchable_text:
            continue
        result.append(service)
    return tuple(result)


def container_services(snapshot: WorkspaceSnapshot) -> tuple[ServiceSnapshot, ...]:
    """The listeners that belong to container plumbing.

    These are what the Docker tab can show on a machine where the Docker CLI
    is not reachable: the published ports are still visible as processes even
    when the containers behind them cannot be enumerated.
    """
    return tuple(
        sorted(
            (item for item in snapshot.services if item.relevance == Relevance.CONTAINER.value),
            key=lambda item: (item.lowest_port, item.display_name.casefold()),
        )
    )


def container_count(snapshot: WorkspaceSnapshot) -> int:
    return sum(1 for item in snapshot.services if item.relevance == Relevance.CONTAINER.value)


def group_services(services: tuple[ServiceSnapshot, ...]) -> tuple[ServiceGroup, ...]:
    buckets: dict[str, list[ServiceSnapshot]] = {}
    metadata: dict[str, tuple[str, str | None, str | None, bool, bool]] = {}

    for service in services:
        if service.relevance == Relevance.CONTAINER.value:
            group_id = "containers"
            metadata[group_id] = ("Containers", None, None, True, False)
        elif service.project is not None:
            group_id = service.project.id
            metadata[group_id] = (
                service.project.name,
                service.project.root_path,
                service.project.detection_source,
                False,
                False,
            )
        else:
            group_id = "unassigned"
            metadata[group_id] = ("Other", None, None, False, True)
        buckets.setdefault(group_id, []).append(service)

    groups: list[ServiceGroup] = []
    for group_id, items in buckets.items():
        name, path, source, system, unassigned = metadata[group_id]
        sorted_items = tuple(sorted(items, key=lambda item: (item.lowest_port, item.display_name.casefold())))
        groups.append(
            ServiceGroup(
                id=group_id,
                name=name,
                path=path,
                source=source,
                services=sorted_items,
                system=system,
                unassigned=unassigned,
            )
        )

    def key(group: ServiceGroup) -> tuple[int, str]:
        if group.system:
            rank = 2
        elif group.unassigned:
            rank = 1
        else:
            rank = 0
        return rank, group.name.casefold()

    groups.sort(key=key)
    return tuple(groups)


def format_bytes(value: int | None) -> str:
    if value is None:
        return "—"
    units = ("B", "KB", "MB", "GB", "TB")
    amount = float(value)
    for unit in units:
        if amount < 1024 or unit == units[-1]:
            if unit in {"B", "KB"}:
                return f"{amount:.0f} {unit}"
            return f"{amount:.1f} {unit}"
        amount /= 1024
    return f"{value} B"


def format_duration(seconds: int | None) -> str:
    if seconds is None:
        return "—"
    seconds = max(0, seconds)
    if seconds < 60:
        return _format_count(seconds, "second")
    minutes, _ = divmod(seconds, 60)
    if minutes < 60:
        return _format_count(minutes, "minute")
    hours, minutes = divmod(minutes, 60)
    if hours < 24:
        return (
            f"{_format_count(hours, 'hour')} {_format_count(minutes, 'minute')}"
            if minutes
            else _format_count(hours, "hour")
        )
    days, hours = divmod(hours, 24)
    return (
        f"{_format_count(days, 'day')} {_format_count(hours, 'hour')}"
        if hours
        else _format_count(days, "day")
    )


def _format_count(value: int, unit: str) -> str:
    return f"{value} {unit if value == 1 else unit + 's'}"


# A service younger than this is still warming up, and the board tints its
# uptime so a just-restarted process is obvious at a glance.
FRESH_UPTIME_SECONDS = 300


def format_uptime_compact(seconds: int | None) -> str:
    """Uptime short enough for a tile: at most two units, never wrapping."""
    if seconds is None:
        return ""
    seconds = max(0, seconds)
    if seconds < 60:
        return f"{seconds}s"
    minutes, _ = divmod(seconds, 60)
    if minutes < 60:
        return f"{minutes}m"
    hours, minutes = divmod(minutes, 60)
    if hours < 24:
        return f"{hours}h {minutes}m" if minutes else f"{hours}h"
    days, hours = divmod(hours, 24)
    return f"{days}d {hours}h" if hours else f"{days}d"


def format_cpu(value: float | None) -> str:
    return "Calculating" if value is None else f"{value:.1f}%"


def endpoint_label(service: ServiceSnapshot) -> str:
    return ", ".join(str(endpoint.port) for endpoint in service.endpoints)

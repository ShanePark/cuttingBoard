from __future__ import annotations

import json
import subprocess
from collections.abc import Callable, Sequence
from dataclasses import dataclass

# `docker ps` is asked for JSON per line rather than a table so that field
# order and column widths cannot change the parse. `--no-trunc` keeps labels
# and names intact; the id is shortened here instead, because the CLI would
# otherwise truncate the fields we actually read.
PS_COMMAND: tuple[str, ...] = ("docker", "ps", "--all", "--no-trunc", "--format", "{{json .}}")

COMPOSE_PROJECT_LABEL = "com.docker.compose.project"
COMPOSE_SERVICE_LABEL = "com.docker.compose.service"

_SHORT_ID_LENGTH = 12
_MAX_HINT_LENGTH = 160

# A published range such as `-p 8000-8100:8000-8100` arrives as a single entry.
# Expanding it is right for the handful of ports a dev stack publishes, but a
# pathological range would flood the tile, so expansion stops after this many.
_MAX_RANGE_PORTS = 64

_NO_CLI_MESSAGE = "Docker CLI를 찾을 수 없습니다."
_LAUNCH_FAILED_MESSAGE = "Docker CLI를 실행할 수 없습니다."
_TIMEOUT_MESSAGE = "Docker가 제한 시간 안에 응답하지 않았습니다."
_DAEMON_MESSAGE = "Docker 데몬에 연결할 수 없습니다."
_PERMISSION_MESSAGE = "Docker 소켓에 접근할 권한이 없습니다."
_FAILED_MESSAGE = "Docker 명령이 실패했습니다."
_EMPTY_MESSAGE = "컨테이너가 없습니다."

# Takes the command and a timeout in seconds. Exists so tests can drive the
# parser and the failure paths without a docker installation.
CommandRunner = Callable[[Sequence[str], float], "subprocess.CompletedProcess[str]"]


@dataclass(frozen=True, slots=True)
class ContainerInfo:
    """One container as `docker ps` reports it.

    Every field is best effort: an older CLI may omit `State`, a container may
    carry no compose labels, and a stopped container publishes no ports. The
    fields are therefore always present but may be empty rather than absent,
    so the UI never has to guard against a missing attribute.
    """

    id: str
    name: str
    image: str
    state: str
    status: str
    ports: tuple[int, ...]
    created_at: str
    compose_project: str | None = None
    compose_service: str | None = None

    @property
    def running(self) -> bool:
        return self.state == "running"


@dataclass(frozen=True, slots=True)
class ContainerListing:
    """The containers, plus why there are none when there are none.

    An empty tuple is ambiguous on its own — no docker, no daemon, no
    permission and no containers all look alike — so `available` separates
    "docker answered" from "docker could not be asked", and `message` carries
    the sentence the tab shows in place of tiles.
    """

    containers: tuple[ContainerInfo, ...] = ()
    available: bool = True
    message: str | None = None

    @classmethod
    def unavailable(cls, message: str) -> ContainerListing:
        return cls(containers=(), available=False, message=message)


# Remembers what the last listing learned so `docker_available` does not have
# to spawn a second process. A plain module global is enough: the value is a
# single bool written by one worker thread and read by the UI thread, and a
# stale answer only costs a tab that appears one refresh late.
_availability_cache: bool | None = None


def list_containers(timeout: float = 2.0, *, runner: CommandRunner | None = None) -> ContainerListing:
    """Every container docker knows about, or the reason we could not ask.

    Never raises and never blocks longer than `timeout`: this runs on a worker
    thread on machines that may have no docker binary, no running daemon or no
    permission on the socket, and a hung CLI would leak the thread.
    """
    global _availability_cache
    listing = _list(timeout, runner if runner is not None else _run_subprocess)
    _availability_cache = listing.available
    return listing


def docker_available(timeout: float = 2.0) -> bool:
    """Whether docker answered us, for deciding if the tab is worth showing.

    Reuses what the last `list_containers` call found and only shells out when
    nothing has asked yet.
    """
    if _availability_cache is not None:
        return _availability_cache
    return list_containers(timeout=timeout).available


def _list(timeout: float, runner: CommandRunner) -> ContainerListing:
    try:
        completed = runner(PS_COMMAND, timeout)
    except FileNotFoundError:
        # No docker binary on PATH: the common case on a machine that simply
        # does not use containers, and not worth reporting as an error.
        return ContainerListing.unavailable(_NO_CLI_MESSAGE)
    except subprocess.TimeoutExpired:
        return ContainerListing.unavailable(_TIMEOUT_MESSAGE)
    except OSError as error:
        # A broken PATH entry, an exec failure or a dead socket file.
        return ContainerListing.unavailable(f"{_LAUNCH_FAILED_MESSAGE} ({error})")

    if completed.returncode != 0:
        return ContainerListing.unavailable(_failure_message(_as_text(completed.stderr)))

    containers = _parse_lines(_as_text(completed.stdout))
    if not containers:
        return ContainerListing(containers=(), available=True, message=_EMPTY_MESSAGE)
    return ContainerListing(containers=containers, available=True)


def _run_subprocess(command: Sequence[str], timeout: float) -> subprocess.CompletedProcess[str]:
    # `check=False` because a non-zero exit is a message for the user, not an
    # exception; `errors="replace"` because a container name may hold bytes
    # that are not valid UTF-8 and one bad byte must not lose the whole list.
    return subprocess.run(
        list(command),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        check=False,
    )


def _failure_message(stderr: str) -> str:
    """Turn docker's exit into a sentence, keeping the first line of stderr.

    The two failures worth naming are a stopped daemon and a socket the user
    may not touch, because each has a different fix. Everything else keeps the
    generic headline and leans on the hint.
    """
    hint = _first_line(stderr)
    lowered = hint.casefold()
    if "permission denied" in lowered:
        # Checked before the daemon case: the permission error also mentions
        # connecting to the daemon, but the daemon is running just fine.
        headline = _PERMISSION_MESSAGE
    elif "cannot connect to the docker daemon" in lowered or "daemon running" in lowered:
        headline = _DAEMON_MESSAGE
    else:
        headline = _FAILED_MESSAGE
    return f"{headline} ({hint})" if hint else headline


def _first_line(text: str) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped:
            if len(stripped) > _MAX_HINT_LENGTH:
                return stripped[:_MAX_HINT_LENGTH].rstrip() + "…"
            return stripped
    return ""


def _parse_lines(stdout: str) -> tuple[ContainerInfo, ...]:
    """Parse the JSON-per-line output, dropping only the lines that are broken.

    A warning printed on stdout by a plugin, or output truncated mid-line, must
    cost that one container and not the whole listing.
    """
    containers: list[ContainerInfo] = []
    for line in stdout.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        try:
            payload = json.loads(stripped)
        except ValueError:
            continue
        if not isinstance(payload, dict):
            continue
        container = _parse_container(payload)
        if container is not None:
            containers.append(container)
    return tuple(containers)


def _parse_container(payload: dict[str, object]) -> ContainerInfo | None:
    identifier = _as_text(payload.get("ID"))[:_SHORT_ID_LENGTH]
    # A container can carry several names; docker lists them comma separated
    # and the first one is the one it prints everywhere else.
    name = _as_text(payload.get("Names")).split(",")[0].strip()
    if not identifier and not name:
        return None

    labels = _parse_labels(_as_text(payload.get("Labels")))
    status = _as_text(payload.get("Status"))
    return ContainerInfo(
        id=identifier,
        name=name or identifier,
        image=_as_text(payload.get("Image")),
        state=_as_text(payload.get("State")).casefold() or _state_from_status(status),
        status=status,
        ports=_parse_ports(_as_text(payload.get("Ports"))),
        # Kept as docker printed it ("2026-08-20 09:12:33 +0900 KST"): the
        # trailing zone name is not portable to parse, and `Status` already
        # carries the uptime the tile wants to show.
        created_at=_as_text(payload.get("CreatedAt")),
        compose_project=labels.get(COMPOSE_PROJECT_LABEL) or None,
        compose_service=labels.get(COMPOSE_SERVICE_LABEL) or None,
    )


def _state_from_status(status: str) -> str:
    """Recover the state from the human status for CLIs without a `State` field.

    Docker only added `.State` to the `ps` format in 20.10, and the tab groups
    running containers apart from stopped ones, so guessing beats "unknown".
    """
    lowered = status.casefold()
    if "(paused)" in lowered:
        return "paused"
    first = lowered.split(" ", 1)[0]
    if first.startswith("up"):
        return "running"
    known = ("exited", "created", "restarting", "removing", "paused", "dead")
    return first if first in known else "unknown"


def _parse_labels(raw: str) -> dict[str, str]:
    """Split the `key=value,key=value` blob docker renders for `.Labels`.

    Docker joins labels with commas without escaping them, so a value that
    itself contains a comma is unrecoverable. Fragments without a `=` are
    dropped rather than guessed at; the compose labels we care about never
    contain commas.
    """
    labels: dict[str, str] = {}
    for entry in raw.split(","):
        key, separator, value = entry.partition("=")
        if not separator:
            continue
        stripped_key = key.strip()
        if stripped_key:
            labels[stripped_key] = value.strip()
    return labels


def _parse_ports(raw: str) -> tuple[int, ...]:
    """Host ports from `0.0.0.0:5432->5432/tcp, :::5432->5432/tcp`.

    Only the left of `->` matters: that is what a developer can reach from the
    host. A dual-stack publish lists the same port twice, once per address
    family, so the result is a set — the tab shows ports, not bindings.
    """
    ports: set[int] = set()
    for entry in raw.split(","):
        stripped = entry.strip()
        if "->" not in stripped:
            # `8080/tcp` means merely exposed by the image: nothing is
            # published, so there is nothing to reach from the host.
            continue
        host_side = stripped.split("->", 1)[0].strip()
        # Drops the address, whether it is `0.0.0.0`, `:::` or `[::]:`.
        ports.update(_expand_port_range(host_side.rsplit(":", 1)[-1]))
    return tuple(sorted(ports))


def _expand_port_range(token: str) -> tuple[int, ...]:
    start_text, separator, end_text = token.partition("-")
    start = _port_number(start_text)
    if start is None:
        return ()
    if not separator:
        return (start,)
    end = _port_number(end_text)
    if end is None or end < start:
        return (start,)
    return tuple(range(start, min(end, start + _MAX_RANGE_PORTS - 1) + 1))


def _port_number(token: str) -> int | None:
    stripped = token.strip()
    if not stripped.isdigit():
        return None
    value = int(stripped)
    return value if 0 < value <= 65535 else None


def _as_text(value: object) -> str:
    """`{{json .}}` fields are strings, but a null or a number must not crash."""
    return value.strip() if isinstance(value, str) else ""

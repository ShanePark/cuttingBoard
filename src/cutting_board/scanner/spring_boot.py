from __future__ import annotations

import re
import shlex
from collections.abc import Mapping, Sequence
from pathlib import Path

from cutting_board.models import Endpoint

_CONFIG_NAMES = ("application.properties", "application.yml", "application.yaml")
_ENVIRONMENT_PROPERTIES = {
    "SERVER_PORT": "server.port",
    "SERVER_SERVLET_CONTEXT_PATH": "server.servlet.context-path",
    "SPRING_PROFILES_ACTIVE": "spring.profiles.active",
    "SPRING_DEVTOOLS_LIVERELOAD_PORT": "spring.devtools.livereload.port",
    "MANAGEMENT_SERVER_PORT": "management.server.port",
    "SERVER_SSL_ENABLED": "server.ssl.enabled",
}
_LIVERELOAD_PORT = 35729
_PLACEHOLDER = re.compile(r"\$\{([^}:]+)(?::([^}]*))?\}")


def resolve_spring_boot_browser_url(
    *,
    command: Sequence[str],
    environment: Mapping[str, str] | None,
    cwd: str | None,
    project_root: str | None,
    endpoints: Sequence[Endpoint],
) -> str | None:
    """Resolve a Spring Boot browser URL from process and project evidence."""
    env = {str(name): str(value) for name, value in (environment or {}).items()}
    command_properties = _command_properties(command)
    environment_properties = {
        property_name: env[name]
        for name, property_name in _ENVIRONMENT_PROPERTIES.items()
        if name in env
    }

    locations = _config_locations(cwd, project_root)
    base_properties = _load_properties(locations, ())
    active_profiles = _profiles(
        command_properties.get(
            "spring.profiles.active",
            environment_properties.get(
                "spring.profiles.active",
                base_properties.get("spring.profiles.active", ""),
            ),
        )
    )
    properties = _load_properties(locations, active_profiles)
    properties.update(environment_properties)
    properties.update(command_properties)
    properties = {
        name: _resolve_placeholders(value, env)
        for name, value in properties.items()
    }

    configured_port = _port(properties.get("server.port"))
    excluded_ports = {_LIVERELOAD_PORT}
    for property_name in ("spring.devtools.livereload.port", "management.server.port"):
        if (port := _port(properties.get(property_name))) is not None:
            excluded_ports.add(port)

    listening_ports = tuple(dict.fromkeys(endpoint.port for endpoint in endpoints))
    candidates = tuple(port for port in listening_ports if port not in excluded_ports)
    if not candidates:
        return None
    if configured_port in candidates:
        selected_port = configured_port
    elif 8080 in candidates:
        selected_port = 8080
    else:
        selected_port = candidates[0]

    ssl_enabled = properties.get("server.ssl.enabled", "").strip().casefold() == "true"
    scheme = "https" if ssl_enabled or selected_port in {443, 8443, 9443} else "http"
    context_path = properties.get(
        "server.servlet.context-path",
        properties.get("server.context-path", ""),
    )
    return f"{scheme}://localhost:{selected_port}{_normalize_context_path(context_path)}"


def _command_properties(command: Sequence[str]) -> dict[str, str]:
    properties: dict[str, str] = {}
    tokens: list[str] = []
    for token in command:
        tokens.append(str(token))
        if str(token).startswith("--args="):
            try:
                tokens.extend(shlex.split(str(token).partition("=")[2]))
            except ValueError:
                continue

    index = 0
    while index < len(tokens):
        token = tokens[index]
        prefix = ""
        if token.startswith("--"):
            prefix = "--"
        elif token.startswith("-D"):
            prefix = "-D"
        if prefix:
            option = token[len(prefix) :]
            if "=" in option:
                name, value = option.split("=", 1)
                properties[name] = value
            elif index + 1 < len(tokens) and not tokens[index + 1].startswith("-"):
                properties[option] = tokens[index + 1]
                index += 1
        index += 1
    return properties


def _config_locations(cwd: str | None, project_root: str | None) -> tuple[Path, ...]:
    roots: list[Path] = []
    for value in (project_root, cwd):
        if not value:
            continue
        try:
            root = Path(value).resolve(strict=False)
        except (OSError, RuntimeError, ValueError):
            continue
        if root not in roots:
            roots.append(root)

    locations: list[Path] = []
    for root in roots:
        for location in (root / "src/main/resources", root, root / "config"):
            if location not in locations:
                locations.append(location)
    return tuple(locations)


def _load_properties(locations: Sequence[Path], active_profiles: Sequence[str]) -> dict[str, str]:
    properties: dict[str, str] = {}
    for location in locations:
        for name in _CONFIG_NAMES:
            properties.update(_read_config(location / name, active_profiles))
        for profile in active_profiles:
            for suffix in ("properties", "yml", "yaml"):
                properties.update(
                    _read_config(location / f"application-{profile}.{suffix}", active_profiles)
                )
    return properties


def _read_config(path: Path, active_profiles: Sequence[str]) -> dict[str, str]:
    try:
        if not path.is_file() or path.stat().st_size > 1_000_000:
            return {}
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return {}
    if path.suffix == ".properties":
        return _parse_properties(text)
    return _parse_yaml(text, active_profiles)


def _parse_properties(text: str) -> dict[str, str]:
    properties: dict[str, str] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line or line.startswith(("#", "!")):
            continue
        match = re.match(r"([^:=\s]+)(?:\s*(?:=|:)\s*|\s+)(.*)", line)
        if match:
            properties[match.group(1).strip()] = match.group(2).strip()
    return properties


def _parse_yaml(text: str, active_profiles: Sequence[str]) -> dict[str, str]:
    properties: dict[str, str] = {}
    active = set(active_profiles)
    for raw_document in re.split(r"(?m)^---\s*(?:#.*)?$", text):
        document = _flatten_yaml_document(raw_document)
        selector = document.get("spring.config.activate.on-profile") or document.get("spring.profiles")
        if selector and not active.intersection(_profiles(selector)):
            continue
        properties.update(document)
    return properties


def _flatten_yaml_document(text: str) -> dict[str, str]:
    properties: dict[str, str] = {}
    parents: list[tuple[int, str]] = []
    for raw_line in text.splitlines():
        stripped = raw_line.lstrip(" ")
        if not stripped or stripped.startswith("#") or ":" not in stripped:
            continue
        indent = len(raw_line) - len(stripped)
        key, raw_value = stripped.split(":", 1)
        key = key.strip()
        if not key:
            continue
        while parents and parents[-1][0] >= indent:
            parents.pop()
        value = _yaml_scalar(raw_value)
        full_key = ".".join([*(name for _, name in parents), key])
        if value is None:
            parents.append((indent, key))
        else:
            properties[full_key] = value
    return properties


def _yaml_scalar(raw_value: str) -> str | None:
    value = raw_value.strip()
    if not value:
        return None
    if value[0] in {'"', "'"} and len(value) >= 2 and value[-1] == value[0]:
        return value[1:-1]
    return value.split(" #", 1)[0].strip()


def _profiles(value: str) -> tuple[str, ...]:
    normalized = value.strip().removeprefix("[").removesuffix("]")
    return tuple(
        item.strip().strip("'\"")
        for item in normalized.split(",")
        if item.strip().strip("'\"")
    )


def _port(value: str | None) -> int | None:
    try:
        port = int(value) if value is not None else 0
    except (TypeError, ValueError):
        return None
    return port if 0 < port <= 65535 else None


def _resolve_placeholders(value: str, environment: Mapping[str, str]) -> str:
    def replace(match: re.Match[str]) -> str:
        name, default = match.group(1), match.group(2)
        return environment.get(name, default if default is not None else match.group(0))

    return _PLACEHOLDER.sub(replace, value)


def _normalize_context_path(value: str) -> str:
    path = value.strip()
    if not path or path == "/":
        return ""
    return f"/{path.strip('/')}/"

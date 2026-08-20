from __future__ import annotations

import json
import os
import tempfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

from cutting_board.constants import (
    APP_SLUG,
    DEFAULT_SCAN_INTERVAL_SECONDS,
    MAX_SCAN_INTERVAL_SECONDS,
    MIN_SCAN_INTERVAL_SECONDS,
)


@dataclass(slots=True)
class UISettings:
    show_system_services: bool = False
    window_geometry: str = "1280x820"
    scan_interval_seconds: float = DEFAULT_SCAN_INTERVAL_SECONDS
    collapsed_project_ids: list[str] = field(default_factory=list)
    theme_mode: str = "dark"

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> UISettings:
        interval = _clamp_interval(data.get("scan_interval_seconds", DEFAULT_SCAN_INTERVAL_SECONDS))
        collapsed = data.get("collapsed_project_ids", [])
        if not isinstance(collapsed, list):
            collapsed = []
        return cls(
            show_system_services=bool(data.get("show_system_services", False)),
            window_geometry=_safe_geometry(data.get("window_geometry")),
            scan_interval_seconds=interval,
            collapsed_project_ids=[str(item) for item in collapsed if isinstance(item, str)],
            theme_mode=_safe_theme_mode(data.get("theme_mode")),
        )


class SettingsStore:
    def __init__(self, path: Path | None = None) -> None:
        self.path = path or _default_settings_path()

    def load(self) -> UISettings:
        try:
            data = json.loads(self.path.read_text(encoding="utf-8"))
            if isinstance(data, dict):
                return UISettings.from_dict(data)
        except (FileNotFoundError, PermissionError, OSError, UnicodeError, json.JSONDecodeError):
            pass
        return UISettings()

    def save(self, settings: UISettings) -> None:
        payload = json.dumps(asdict(settings), ensure_ascii=False, indent=2) + "\n"
        temporary_path: Path | None = None
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(
                "w",
                encoding="utf-8",
                dir=self.path.parent,
                prefix="settings-",
                suffix=".tmp",
                delete=False,
            ) as handle:
                handle.write(payload)
                temporary_path = Path(handle.name)
            os.replace(temporary_path, self.path)
        except OSError:
            if temporary_path is not None:
                try:
                    temporary_path.unlink(missing_ok=True)
                except OSError:
                    pass


def _default_settings_path() -> Path:
    config_home = os.environ.get("XDG_CONFIG_HOME")
    base = Path(config_home).expanduser() if config_home else Path.home() / ".config"
    return base / APP_SLUG / "settings.json"


def _safe_geometry(value: Any) -> str:
    if not isinstance(value, str):
        return "1280x820"
    # Tk geometry: WIDTHxHEIGHT optionally followed by signed X/Y offsets.
    head = value.split("+", 1)[0].split("-", 1)[0]
    if "x" not in head:
        return "1280x820"
    try:
        width, height = (int(part) for part in head.split("x", 1))
    except ValueError:
        return "1280x820"
    if width < 900 or height < 600:
        return "1280x820"
    return value


def _clamp_interval(value: Any) -> float:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return DEFAULT_SCAN_INTERVAL_SECONDS
    return min(MAX_SCAN_INTERVAL_SECONDS, max(MIN_SCAN_INTERVAL_SECONDS, parsed))


def _safe_theme_mode(value: Any) -> str:
    return value if value in {"dark", "light", "system"} else "dark"

from __future__ import annotations

from cutting_board import __version__

APP_NAME = "Cutting Board"
APP_SLUG = "cutting-board"
APP_VERSION = __version__
DEFAULT_SCAN_INTERVAL_SECONDS = 2.0
MIN_SCAN_INTERVAL_SECONDS = 0.75
MAX_SCAN_INTERVAL_SECONDS = 30.0

PROJECT_MARKERS: tuple[str, ...] = (
    ".git",
    "pnpm-workspace.yaml",
    "package.json",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "Cargo.toml",
    "pyproject.toml",
    "go.mod",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
)

SENSITIVE_FLAGS: frozenset[str] = frozenset(
    {
        "--token",
        "--access-token",
        "--auth-token",
        "--api-key",
        "--apikey",
        "--password",
        "--passwd",
        "--secret",
        "--client-secret",
        "--database-url",
    }
)

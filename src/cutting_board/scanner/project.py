from __future__ import annotations

import hashlib
import json
import os
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path

from cutting_board.constants import PROJECT_MARKERS
from cutting_board.models import ProjectInfo


@dataclass(frozen=True, slots=True)
class ProjectResolution:
    project: ProjectInfo
    package_name: str | None
    package_path: str | None


def _default_excluded_roots() -> frozenset[Path]:
    """Directories that hold many unrelated things and are never a project.

    A dotfiles repository in ``$HOME`` is common, and without this every
    desktop application started from the home directory would be filed under
    a project named after the user. ``/tmp`` behaves the same way once any
    tool leaves a marker there.
    """
    roots = {Path("/"), Path("/tmp"), Path("/var/tmp"), Path("/usr"), Path("/opt"), Path("/etc")}
    try:
        roots.add(Path.home().resolve(strict=False))
    except (OSError, RuntimeError):
        pass
    return frozenset(roots)


class ProjectResolver:
    """Infer project context from a process working directory.

    A Git root is preferred as the project group. When there is no Git root,
    the nearest supported project marker is used. The nearest package.json is
    retained separately so monorepo services can still have useful names.
    """

    def __init__(
        self,
        cache_size: int = 512,
        excluded_roots: frozenset[Path] | None = None,
    ) -> None:
        self._cache_size = cache_size
        self._excluded_roots = (
            _default_excluded_roots() if excluded_roots is None else excluded_roots
        )
        self._cache: OrderedDict[str, ProjectResolution | None] = OrderedDict()

    def resolve(self, cwd: str | None) -> ProjectResolution | None:
        if not cwd:
            return None
        normalized = self._normalize(cwd)
        if normalized is None:
            return None
        key = str(normalized)
        if key in self._cache:
            value = self._cache.pop(key)
            self._cache[key] = value
            return value

        value = self._resolve_uncached(normalized)
        # Do not cache a miss: agents may create project markers after a
        # process has already started and the next scan should pick them up.
        if value is not None:
            self._cache[key] = value
            while len(self._cache) > self._cache_size:
                self._cache.popitem(last=False)
        return value

    @staticmethod
    def _normalize(cwd: str) -> Path | None:
        try:
            path = Path(cwd).expanduser()
            if not path.is_absolute():
                path = Path(os.path.abspath(path))
            return path.resolve(strict=False)
        except (OSError, RuntimeError, ValueError):
            return None

    def _resolve_uncached(self, cwd: Path) -> ProjectResolution | None:
        ancestors = [cwd, *cwd.parents]
        git_root: Path | None = None
        nearest_marker_root: Path | None = None
        nearest_marker: str | None = None
        package_root: Path | None = None
        package_name: str | None = None

        for directory in ancestors:
            if directory in self._excluded_roots:
                break

            if package_root is None and (directory / "package.json").is_file():
                package_root = directory
                package_name = self._read_package_name(directory / "package.json")

            if git_root is None and (directory / ".git").exists():
                git_root = directory

            if nearest_marker_root is None:
                for marker in PROJECT_MARKERS:
                    try:
                        if (directory / marker).exists():
                            nearest_marker_root = directory
                            nearest_marker = marker
                            break
                    except OSError:
                        continue

        root = git_root or nearest_marker_root
        if root is None:
            return None

        source = ".git" if git_root is not None else (nearest_marker or "marker")
        root_package_name = self._read_package_name(root / "package.json")
        project_name = root_package_name or root.name or str(root)
        project_id = hashlib.sha1(str(root).encode("utf-8"), usedforsecurity=False).hexdigest()[:16]

        project = ProjectInfo(
            id=f"project:{project_id}",
            name=project_name,
            root_path=str(root),
            detection_source=source,
            package_name=package_name,
            package_path=str(package_root) if package_root else None,
        )
        return ProjectResolution(
            project=project,
            package_name=package_name,
            package_path=str(package_root) if package_root else None,
        )

    @staticmethod
    def _read_package_name(path: Path) -> str | None:
        try:
            if not path.is_file() or path.stat().st_size > 1_000_000:
                return None
            data = json.loads(path.read_text(encoding="utf-8"))
            value = data.get("name")
            if isinstance(value, str) and value.strip():
                return value.strip()
        except (OSError, UnicodeError, json.JSONDecodeError, TypeError):
            return None
        return None

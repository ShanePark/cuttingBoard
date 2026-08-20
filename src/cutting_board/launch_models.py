from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path
from typing import Any


class LaunchState(str, Enum):
    STOPPED = "stopped"
    STARTING = "starting"
    RUNNING = "running"
    STOPPING = "stopping"
    FAILED = "failed"


@dataclass(frozen=True, slots=True)
class LaunchTask:
    name: str
    cwd: str
    command: str
    expected_port: int | None = None
    watch_command: str | None = None

    def __post_init__(self) -> None:
        _require_text(self.name, "작업 이름")
        _require_text(self.cwd, "작업 폴더")
        _require_text(self.command, "실행 명령")
        if self.expected_port is not None:
            if isinstance(self.expected_port, bool) or not isinstance(self.expected_port, int):
                raise ValueError("예상 포트는 숫자여야 합니다.")
            if not 1 <= self.expected_port <= 65535:
                raise ValueError("예상 포트는 1~65535 범위여야 합니다.")
        if self.watch_command is not None:
            _require_text(self.watch_command, "자동 빌드 명령")

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LaunchTask:
        try:
            return cls(
                name=data["name"],
                cwd=data["cwd"],
                command=data["command"],
                expected_port=data.get("expected_port"),
                watch_command=data.get("watch_command"),
            )
        except KeyError as exc:
            raise ValueError(f"작업 설정에 {exc.args[0]} 항목이 없습니다.") from exc

    def to_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "cwd": self.cwd,
            "command": self.command,
            "expected_port": self.expected_port,
            "watch_command": self.watch_command,
        }


@dataclass(frozen=True, slots=True)
class LaunchProfile:
    id: str
    name: str
    project_root: str
    tasks: tuple[LaunchTask, ...] = field(default_factory=tuple)

    def __post_init__(self) -> None:
        _require_text(self.id, "프로필 ID")
        _require_text(self.name, "프로필 이름")
        _require_text(self.project_root, "프로젝트 폴더")
        root = Path(self.project_root).expanduser()
        if not root.is_absolute():
            raise ValueError("프로젝트 폴더는 절대 경로여야 합니다.")
        if not self.tasks:
            raise ValueError("프로필에는 작업이 하나 이상 필요합니다.")
        names: set[str] = set()
        for task in self.tasks:
            if not isinstance(task, LaunchTask):
                raise TypeError("프로필 작업 형식이 올바르지 않습니다.")
            normalized_name = task.name.strip().casefold()
            if normalized_name in names:
                raise ValueError(f"작업 이름이 중복되었습니다: {task.name}")
            names.add(normalized_name)
            self.task_cwd(task)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> LaunchProfile:
        try:
            raw_tasks = data["tasks"]
            if not isinstance(raw_tasks, list):
                raise TypeError("프로필의 작업 목록 형식이 올바르지 않습니다.")
            return cls(
                id=data["id"],
                name=data["name"],
                project_root=data["project_root"],
                tasks=tuple(
                    LaunchTask.from_dict(item)
                    if isinstance(item, dict)
                    else _invalid_task()
                    for item in raw_tasks
                ),
            )
        except KeyError as exc:
            raise ValueError(f"프로필 설정에 {exc.args[0]} 항목이 없습니다.") from exc

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "name": self.name,
            "project_root": self.project_root,
            "tasks": [task.to_dict() for task in self.tasks],
        }

    def task(self, name: str) -> LaunchTask:
        for task in self.tasks:
            if task.name == name:
                return task
        raise KeyError(name)

    def task_cwd(self, task: LaunchTask) -> Path:
        root = Path(self.project_root).expanduser().resolve(strict=False)
        configured = Path(task.cwd).expanduser()
        resolved = (
            configured.resolve(strict=False)
            if configured.is_absolute()
            else (root / configured).resolve(strict=False)
        )
        try:
            resolved.relative_to(root)
        except ValueError as exc:
            raise ValueError(f"작업 폴더는 프로젝트 안에 있어야 합니다: {task.cwd}") from exc
        return resolved


@dataclass(frozen=True, slots=True)
class ManagedTaskSnapshot:
    profile_id: str
    task_name: str
    state: LaunchState
    main_pid: int | None = None
    watch_pid: int | None = None
    expected_port: int | None = None
    logs: tuple[str, ...] = field(default_factory=tuple)
    message: str | None = None


@dataclass(frozen=True, slots=True)
class LaunchEvent:
    snapshot: ManagedTaskSnapshot


def _require_text(value: object, label: str) -> None:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{label}을(를) 입력해 주세요.")
    if "\x00" in value:
        raise ValueError(f"{label}에 사용할 수 없는 문자가 있습니다.")


def _invalid_task() -> LaunchTask:
    raise TypeError("프로필의 작업 형식이 올바르지 않습니다.")

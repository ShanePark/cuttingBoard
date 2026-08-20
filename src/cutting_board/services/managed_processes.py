from __future__ import annotations

import os
import pwd
import queue
import signal
import subprocess
import sys
import threading
import time
from collections import deque
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import psutil

from cutting_board.launch_models import (
    LaunchEvent,
    LaunchProfile,
    LaunchState,
    LaunchTask,
    ManagedTaskSnapshot,
)


class ManagedProcessError(RuntimeError):
    pass


def resolve_shell_path(
    *,
    platform: str | None = None,
    login_shell_provider: Callable[[], str | None] | None = None,
    is_executable: Callable[[str], bool] | None = None,
) -> str:
    """Choose a login-capable shell without adding a Linux zsh dependency."""

    current_platform = sys.platform if platform is None else platform
    executable_check = is_executable or _is_executable_file
    if current_platform == "darwin" and executable_check("/bin/zsh"):
        return "/bin/zsh"

    provider = login_shell_provider or _current_login_shell
    candidate = provider()
    if _is_safe_shell(candidate, executable_check):
        assert candidate is not None
        return candidate
    return "/bin/sh"


@dataclass(slots=True)
class _OwnedProcess:
    role: str
    handle: Any
    pid: int
    pgid: int
    create_time: float
    uid: int


@dataclass(slots=True)
class _TaskRuntime:
    profile_id: str
    task: LaunchTask
    state: LaunchState = LaunchState.STOPPED
    processes: dict[str, _OwnedProcess] = field(default_factory=dict)
    logs: deque[str] = field(default_factory=deque)
    message: str | None = None


class ManagedProcessRunner:
    """Own process groups started from launch profiles for the app lifetime."""

    def __init__(
        self,
        *,
        popen_factory: Callable[..., Any] | None = None,
        process_factory: Callable[[int], Any] | None = None,
        killpg: Callable[[int, int], None] | None = None,
        getpgid: Callable[[int], int] | None = None,
        process_iter: Callable[[], Iterable[Any]] | None = None,
        current_uid: int | None = None,
        own_pid: int | None = None,
        shell_path: str | None = None,
        max_log_lines: int = 500,
        stop_timeout_seconds: float = 2.0,
    ) -> None:
        if max_log_lines < 1:
            raise ValueError("로그 보관 줄 수는 1 이상이어야 합니다.")
        if stop_timeout_seconds < 0:
            raise ValueError("종료 대기 시간은 0 이상이어야 합니다.")
        self._popen = popen_factory or subprocess.Popen
        self._process_factory = process_factory or psutil.Process
        self._killpg = killpg or os.killpg
        self._getpgid = getpgid or os.getpgid
        self._process_iter = process_iter or psutil.process_iter
        self._current_uid = os.geteuid() if current_uid is None else current_uid
        self._own_pid = os.getpid() if own_pid is None else own_pid
        self._shell_path = resolve_shell_path() if shell_path is None else shell_path
        self._max_log_lines = max_log_lines
        self._stop_timeout_seconds = stop_timeout_seconds
        self._lock = threading.RLock()
        self._runtimes: dict[tuple[str, str], _TaskRuntime] = {}
        self._events: queue.Queue[LaunchEvent] = queue.Queue(maxsize=128)
        self._closed = False

    @property
    def events(self) -> queue.Queue[LaunchEvent]:
        return self._events

    def start_task(self, profile: LaunchProfile, task_name: str) -> ManagedTaskSnapshot:
        task = profile.task(task_name)
        key = (profile.id, task.name)
        with self._lock:
            if self._closed:
                raise ManagedProcessError("실행 관리자가 이미 종료되었습니다.")
            runtime = self._runtimes.get(key)
            if runtime is not None and self._has_live_process(runtime):
                raise ManagedProcessError(f"이미 실행 중인 작업입니다: {task.name}")
            runtime = _TaskRuntime(
                profile_id=profile.id,
                task=task,
                state=LaunchState.STARTING,
                logs=deque(maxlen=self._max_log_lines),
                message="작업을 시작하고 있습니다.",
            )
            self._runtimes[key] = runtime
            self._emit_locked(runtime)

        cwd = profile.task_cwd(task)
        if not cwd.is_dir():
            return self._fail_start(runtime, f"작업 폴더를 찾을 수 없습니다: {cwd}")

        try:
            self._spawn(runtime, "main", task.command, cwd)
            with self._lock:
                if runtime.state != LaunchState.STARTING:
                    return self._snapshot_locked(runtime)
            if task.watch_command is not None:
                self._spawn(runtime, "watch", task.watch_command, cwd)
        except (
            OSError,
            psutil.Error,
            ManagedProcessError,
            TypeError,
            ValueError,
            AttributeError,
        ) as exc:
            self._stop_runtime(runtime, timeout_seconds=self._stop_timeout_seconds)
            return self._fail_start(runtime, f"작업을 시작하지 못했습니다: {exc}")

        with self._lock:
            if runtime.state != LaunchState.STARTING:
                return self._snapshot_locked(runtime)
            runtime.state = LaunchState.RUNNING
            runtime.message = "작업이 실행 중입니다."
            return self._emit_locked(runtime)

    def start_profile(self, profile: LaunchProfile) -> tuple[ManagedTaskSnapshot, ...]:
        snapshots: list[ManagedTaskSnapshot] = []
        for task in profile.tasks:
            snapshots.append(self.start_task(profile, task.name))
        return tuple(snapshots)

    def stop_task(
        self,
        profile_id: str,
        task_name: str,
        *,
        timeout_seconds: float | None = None,
    ) -> ManagedTaskSnapshot:
        key = (profile_id, task_name)
        with self._lock:
            runtime = self._runtimes.get(key)
            if runtime is None:
                return ManagedTaskSnapshot(
                    profile_id=profile_id,
                    task_name=task_name,
                    state=LaunchState.STOPPED,
                    message="이미 종료된 작업입니다.",
                )
        clean = self._stop_runtime(
            runtime,
            timeout_seconds=self._stop_timeout_seconds if timeout_seconds is None else timeout_seconds,
        )
        with self._lock:
            if not clean or self._has_live_process(runtime):
                runtime.state = LaunchState.FAILED
                runtime.message = "일부 프로세스를 안전하게 종료하지 못했습니다."
            else:
                runtime.state = LaunchState.STOPPED
                runtime.message = "작업을 종료했습니다."
            return self._emit_locked(runtime)

    def stop_profile(
        self,
        profile_id: str,
        *,
        timeout_seconds: float | None = None,
    ) -> tuple[ManagedTaskSnapshot, ...]:
        with self._lock:
            names = [name for candidate_id, name in self._runtimes if candidate_id == profile_id]
        return tuple(
            self.stop_task(profile_id, name, timeout_seconds=timeout_seconds)
            for name in names
        )

    def snapshot(self, profile_id: str, task_name: str) -> ManagedTaskSnapshot:
        with self._lock:
            runtime = self._runtimes.get((profile_id, task_name))
            if runtime is None:
                return ManagedTaskSnapshot(
                    profile_id=profile_id,
                    task_name=task_name,
                    state=LaunchState.STOPPED,
                )
            return self._snapshot_locked(runtime)

    def snapshots(self, profile_id: str | None = None) -> tuple[ManagedTaskSnapshot, ...]:
        with self._lock:
            runtimes = (
                runtime
                for (candidate_id, _name), runtime in self._runtimes.items()
                if profile_id is None or candidate_id == profile_id
            )
            return tuple(self._snapshot_locked(runtime) for runtime in runtimes)

    def is_profile_active(self, profile_id: str) -> bool:
        with self._lock:
            return any(
                candidate_id == profile_id and self._has_live_process(runtime)
                for (candidate_id, _name), runtime in self._runtimes.items()
            )

    def close(self, timeout_seconds: float | None = None) -> None:
        with self._lock:
            if self._closed:
                return
            self._closed = True
            keys = tuple(self._runtimes)
        timeout = self._stop_timeout_seconds if timeout_seconds is None else timeout_seconds
        for profile_id, task_name in keys:
            self.stop_task(profile_id, task_name, timeout_seconds=timeout)

    def _spawn(
        self,
        runtime: _TaskRuntime,
        role: str,
        command: str,
        cwd: Path,
    ) -> _OwnedProcess:
        handle = self._popen(
            [self._shell_path, "-lc", command],
            cwd=str(cwd),
            start_new_session=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            errors="replace",
            bufsize=1,
            shell=False,
        )
        pid = int(handle.pid)
        try:
            process = self._process_factory(pid)
            create_time = float(process.create_time())
            uid = int(process.uids().effective)
            pgid = int(self._getpgid(pid))
            if pid in {1, self._own_pid} or uid != self._current_uid or pgid != pid:
                raise ManagedProcessError("시작한 프로세스의 소유권을 안전하게 확인할 수 없습니다.")
        except (
            OSError,
            psutil.Error,
            ManagedProcessError,
            TypeError,
            ValueError,
            AttributeError,
        ):
            self._cleanup_unverified(handle, pid)
            raise
        owned = _OwnedProcess(
            role=role,
            handle=handle,
            pid=pid,
            pgid=pgid,
            create_time=create_time,
            uid=uid,
        )
        with self._lock:
            runtime.processes[role] = owned
        reader = threading.Thread(
            target=self._read_output,
            args=(runtime, owned),
            name=f"cutting-board-{runtime.profile_id}-{runtime.task.name}-{role}-log",
            daemon=True,
        )
        reader.start()
        monitor = threading.Thread(
            target=self._monitor_process,
            args=(runtime, owned),
            name=f"cutting-board-{runtime.profile_id}-{runtime.task.name}-{role}-wait",
            daemon=True,
        )
        monitor.start()
        return owned

    def _read_output(self, runtime: _TaskRuntime, process: _OwnedProcess) -> None:
        stream = process.handle.stdout
        if stream is None:
            return
        label = "실행" if process.role == "main" else "자동 빌드"
        try:
            for raw_line in stream:
                line = raw_line.rstrip("\r\n")
                if not line:
                    continue
                if len(line) > 8000:
                    line = f"{line[:7999]}…"
                with self._lock:
                    runtime.logs.append(f"[{label}] {line}")
                    self._emit_locked(runtime)
        except (OSError, ValueError):
            return
        finally:
            try:
                stream.close()
            except OSError:
                pass

    def _monitor_process(self, runtime: _TaskRuntime, process: _OwnedProcess) -> None:
        try:
            return_code = int(process.handle.wait())
        except (
            OSError,
            psutil.Error,
            ManagedProcessError,
            TypeError,
            ValueError,
            AttributeError,
        ):
            return_code = -1
        with self._lock:
            if runtime.state in {LaunchState.STARTING, LaunchState.RUNNING}:
                runtime.state = LaunchState.FAILED if return_code else LaunchState.STOPPED
                process_label = "자동 빌드 프로세스" if process.role == "watch" else "실행 프로세스"
                if return_code:
                    runtime.message = (
                        f"{process_label}가 종료 코드 {return_code}(으)로 종료되었습니다."
                    )
                else:
                    runtime.message = f"{process_label}가 종료되었습니다."
                runtime.logs.append(f"[상태] {runtime.message}")
            self._emit_locked(runtime)

    def _stop_runtime(self, runtime: _TaskRuntime, timeout_seconds: float) -> bool:
        with self._lock:
            runtime.state = LaunchState.STOPPING
            runtime.message = "작업을 종료하고 있습니다."
            processes = tuple(runtime.processes.values())
            self._emit_locked(runtime)

        verified_groups: set[int] = set()
        clean = True
        for process in processes:
            if self._signal_owned(runtime, process, signal.SIGTERM):
                verified_groups.add(process.pgid)
            else:
                clean = False

        deadline = time.monotonic() + max(0.0, timeout_seconds)
        for process in processes:
            remaining = max(0.0, deadline - time.monotonic())
            try:
                process.handle.wait(timeout=remaining)
            except (subprocess.TimeoutExpired, TimeoutError):
                pass
            except OSError:
                clean = False

        for process in processes:
            if process.pgid not in verified_groups:
                continue
            has_members, safe = self._group_membership(process)
            if has_members and safe:
                if not self._signal_verified_group(runtime, process, signal.SIGKILL):
                    clean = False
            elif has_members:
                clean = False

        for process in processes:
            try:
                process.handle.wait(timeout=max(0.1, min(1.0, timeout_seconds)))
            except (OSError, subprocess.SubprocessError, TimeoutError):
                clean = False

        group_deadline = time.monotonic() + max(0.1, min(1.0, timeout_seconds))
        while True:
            remaining = False
            unsafe = False
            for process in processes:
                if process.pgid not in verified_groups:
                    continue
                has_members, safe = self._group_membership(process)
                remaining = remaining or (has_members and safe)
                unsafe = unsafe or (has_members and not safe)
            if unsafe:
                return False
            if not remaining:
                return clean
            if time.monotonic() >= group_deadline:
                return False
            time.sleep(0.01)

    def _signal_owned(self, runtime: _TaskRuntime, owned: _OwnedProcess, sig: int) -> bool:
        if not self._ownership_matches(owned):
            with self._lock:
                runtime.logs.append(
                    f"[안전] PID {owned.pid}의 소유권이 달라 종료 신호를 보내지 않았습니다."
                )
                runtime.state = LaunchState.FAILED
                runtime.message = "프로세스 소유권이 달라 안전하게 종료하지 못했습니다."
                self._emit_locked(runtime)
            return False
        return self._signal_verified_group(runtime, owned, sig)

    def _signal_verified_group(
        self,
        runtime: _TaskRuntime,
        owned: _OwnedProcess,
        sig: int,
    ) -> bool:
        try:
            self._killpg(owned.pgid, sig)
            return True
        except ProcessLookupError:
            return True
        except (PermissionError, OSError) as exc:
            with self._lock:
                runtime.logs.append(f"[오류] 프로세스 종료 실패: {exc}")
                runtime.state = LaunchState.FAILED
                runtime.message = "프로세스를 종료하지 못했습니다."
                self._emit_locked(runtime)
            return False

    def _cleanup_unverified(self, handle: Any, pid: int) -> None:
        if pid <= 1 or pid == self._own_pid:
            return
        try:
            self._killpg(pid, signal.SIGTERM)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        try:
            handle.wait(timeout=self._stop_timeout_seconds)
        except (OSError, subprocess.SubprocessError, TimeoutError):
            pass
        try:
            self._killpg(pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError, OSError):
            pass
        try:
            handle.wait(timeout=max(0.1, min(1.0, self._stop_timeout_seconds)))
        except (OSError, subprocess.SubprocessError, TimeoutError):
            return

    def _ownership_matches(self, owned: _OwnedProcess) -> bool:
        if owned.pid <= 1 or owned.pid == self._own_pid or owned.uid != self._current_uid:
            return False
        try:
            process = self._process_factory(owned.pid)
            create_time = float(process.create_time())
            uid = int(process.uids().effective)
            pgid = int(self._getpgid(owned.pid))
        except (psutil.Error, OSError, ValueError, AttributeError):
            return False
        return (
            abs(create_time - owned.create_time) <= 0.5
            and uid == self._current_uid
            and pgid == owned.pgid
        )

    def _group_membership(self, owned: _OwnedProcess) -> tuple[bool, bool]:
        try:
            own_pgid = int(self._getpgid(self._own_pid))
        except OSError:
            own_pgid = -1
        if owned.pgid <= 1 or owned.pgid == own_pgid:
            return True, False

        has_members = False
        try:
            candidates = self._process_iter()
            for process in candidates:
                pid = int(process.pid)
                try:
                    if int(self._getpgid(pid)) != owned.pgid:
                        continue
                except (psutil.NoSuchProcess, ProcessLookupError, OSError):
                    continue
                try:
                    if process.status() == psutil.STATUS_ZOMBIE:
                        continue
                    uid = int(process.uids().effective)
                    create_time = float(process.create_time())
                except psutil.NoSuchProcess:
                    continue
                except (psutil.Error, OSError, ValueError, AttributeError):
                    return True, False
                has_members = True
                if uid != self._current_uid or create_time < owned.create_time - 0.5:
                    return True, False
        except (psutil.Error, OSError):
            return True, False
        return has_members, True

    def _fail_start(self, runtime: _TaskRuntime, message: str) -> ManagedTaskSnapshot:
        with self._lock:
            runtime.state = LaunchState.FAILED
            runtime.message = message
            runtime.logs.append(f"[오류] {message}")
            return self._emit_locked(runtime)

    @staticmethod
    def _has_live_process(runtime: _TaskRuntime) -> bool:
        return any(process.handle.poll() is None for process in runtime.processes.values())

    def _snapshot_locked(self, runtime: _TaskRuntime) -> ManagedTaskSnapshot:
        main = runtime.processes.get("main")
        watch = runtime.processes.get("watch")
        return ManagedTaskSnapshot(
            profile_id=runtime.profile_id,
            task_name=runtime.task.name,
            state=runtime.state,
            main_pid=main.pid if main is not None and main.handle.poll() is None else None,
            watch_pid=watch.pid if watch is not None and watch.handle.poll() is None else None,
            expected_port=runtime.task.expected_port,
            logs=tuple(runtime.logs),
            message=runtime.message,
        )

    def _emit_locked(self, runtime: _TaskRuntime) -> ManagedTaskSnapshot:
        snapshot = self._snapshot_locked(runtime)
        if self._runtimes.get((runtime.profile_id, runtime.task.name)) is not runtime:
            return snapshot
        event = LaunchEvent(snapshot=snapshot)
        try:
            self._events.put_nowait(event)
            return snapshot
        except queue.Full:
            pass
        try:
            self._events.get_nowait()
        except queue.Empty:
            pass
        try:
            self._events.put_nowait(event)
        except queue.Full:
            pass
        return snapshot


ManagedProcessManager = ManagedProcessRunner


def _current_login_shell() -> str | None:
    try:
        configured = pwd.getpwuid(os.geteuid()).pw_shell
    except (KeyError, OSError):
        configured = ""
    return configured or os.environ.get("SHELL")


def _is_executable_file(path: str) -> bool:
    return os.path.isfile(path) and os.access(path, os.X_OK)


def _is_safe_shell(
    candidate: str | None,
    is_executable: Callable[[str], bool],
) -> bool:
    if not candidate or "\x00" in candidate or not os.path.isabs(candidate):
        return False
    if Path(candidate).name.casefold() in {"false", "nologin"}:
        return False
    return is_executable(candidate)

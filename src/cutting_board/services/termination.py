from __future__ import annotations

import os
from collections.abc import Callable

import psutil

from cutting_board.models import TerminationResult


class ProcessTerminator:
    """Terminate a process after validating owner and creation time."""

    def __init__(
        self,
        process_factory: Callable[[int], psutil.Process] | None = None,
        current_uid: int | None = None,
        own_pid: int | None = None,
    ) -> None:
        self._process_factory = process_factory or psutil.Process
        self._current_uid = os.geteuid() if current_uid is None else current_uid
        self._own_pid = os.getpid() if own_pid is None else own_pid

    def terminate(
        self,
        pid: int,
        expected_create_time: float,
        *,
        force: bool = False,
        timeout_seconds: float = 1.5,
    ) -> TerminationResult:
        if pid in {1, self._own_pid} or pid <= 1:
            return TerminationResult(
                status="protected",
                message="보호된 프로세스는 종료하지 않습니다.",
                pid=pid,
                force=force,
            )

        try:
            process = self._process_factory(pid)
            actual_create_time = float(process.create_time())
        except psutil.NoSuchProcess:
            return TerminationResult(
                status="already_exited",
                message="프로세스가 이미 종료되었습니다.",
                pid=pid,
                force=force,
            )
        except (psutil.AccessDenied, psutil.ZombieProcess, OSError) as exc:
            return TerminationResult(
                status="permission_denied",
                message=f"프로세스 정보를 확인할 수 없습니다: {type(exc).__name__}.",
                pid=pid,
                force=force,
            )

        if abs(actual_create_time - expected_create_time) > 0.5:
            return TerminationResult(
                status="pid_reused",
                message="PID가 다른 프로세스에 재사용되어 종료하지 않았습니다.",
                pid=pid,
                force=force,
            )

        try:
            effective_uid = int(process.uids().effective)
        except (AttributeError, psutil.NoSuchProcess):
            return TerminationResult(
                status="already_exited",
                message="프로세스가 이미 종료되었습니다.",
                pid=pid,
                force=force,
            )
        except (psutil.AccessDenied, psutil.ZombieProcess, OSError):
            return TerminationResult(
                status="permission_denied",
                message="프로세스 소유자를 확인할 수 없습니다.",
                pid=pid,
                force=force,
            )

        if effective_uid != self._current_uid:
            return TerminationResult(
                status="permission_denied",
                message="현재 사용자 소유 프로세스만 종료할 수 있습니다.",
                pid=pid,
                force=force,
            )

        try:
            if force:
                process.kill()
            else:
                process.terminate()
            process.wait(timeout=timeout_seconds)
            return TerminationResult(
                status="terminated",
                message="프로세스를 강제 종료했습니다." if force else "프로세스를 종료했습니다.",
                pid=pid,
                force=force,
            )
        except psutil.NoSuchProcess:
            return TerminationResult(
                status="terminated",
                message="프로세스가 종료되었습니다.",
                pid=pid,
                force=force,
            )
        except psutil.TimeoutExpired:
            return TerminationResult(
                status="still_running",
                message=(
                    "프로세스가 SIGTERM에 응답하지 않고 계속 실행 중입니다."
                    if not force
                    else "SIGKILL 이후에도 프로세스가 실행 중인 것으로 표시됩니다."
                ),
                pid=pid,
                force=force,
            )
        except psutil.AccessDenied:
            return TerminationResult(
                status="permission_denied",
                message="종료 신호를 보낼 권한이 없습니다.",
                pid=pid,
                force=force,
            )
        except OSError as exc:
            return TerminationResult(
                status="error",
                message=f"프로세스 종료 실패: {exc}",
                pid=pid,
                force=force,
            )

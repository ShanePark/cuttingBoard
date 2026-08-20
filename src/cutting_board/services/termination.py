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
                message="Protected processes will not be terminated.",
                pid=pid,
                force=force,
            )

        try:
            process = self._process_factory(pid)
            actual_create_time = float(process.create_time())
        except psutil.NoSuchProcess:
            return TerminationResult(
                status="already_exited",
                message="The process has already exited.",
                pid=pid,
                force=force,
            )
        except (psutil.AccessDenied, psutil.ZombieProcess, OSError) as exc:
            return TerminationResult(
                status="permission_denied",
                message=f"Could not inspect process information: {type(exc).__name__}.",
                pid=pid,
                force=force,
            )

        if abs(actual_create_time - expected_create_time) > 0.5:
            return TerminationResult(
                status="pid_reused",
                message="The PID was reused by another process; no signal was sent.",
                pid=pid,
                force=force,
            )

        try:
            effective_uid = int(process.uids().effective)
        except (AttributeError, psutil.NoSuchProcess):
            return TerminationResult(
                status="already_exited",
                message="The process has already exited.",
                pid=pid,
                force=force,
            )
        except (psutil.AccessDenied, psutil.ZombieProcess, OSError):
            return TerminationResult(
                status="permission_denied",
                message="Could not determine the process owner.",
                pid=pid,
                force=force,
            )

        if effective_uid != self._current_uid:
            return TerminationResult(
                status="permission_denied",
                message="Only processes owned by the current user can be terminated.",
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
                message="The process was forcefully terminated." if force else "The process was terminated.",
                pid=pid,
                force=force,
            )
        except psutil.NoSuchProcess:
            return TerminationResult(
                status="terminated",
                message="The process has exited.",
                pid=pid,
                force=force,
            )
        except psutil.TimeoutExpired:
            return TerminationResult(
                status="still_running",
                message=(
                    "The process did not respond to SIGTERM and is still running."
                    if not force
                    else "The process still appears to be running after SIGKILL."
                ),
                pid=pid,
                force=force,
            )
        except psutil.AccessDenied:
            return TerminationResult(
                status="permission_denied",
                message="Permission denied while sending the termination signal.",
                pid=pid,
                force=force,
            )
        except OSError as exc:
            return TerminationResult(
                status="error",
                message=f"Failed to terminate the process: {exc}",
                pid=pid,
                force=force,
            )

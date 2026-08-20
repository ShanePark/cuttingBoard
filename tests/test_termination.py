from __future__ import annotations

import os
import unittest

import psutil

from cutting_board.services.termination import ProcessTerminator


class FakeUids:
    def __init__(self, effective: int) -> None:
        self.effective = effective


class FakeProcess:
    def __init__(
        self,
        *,
        create_time: float = 100.0,
        uid: int | None = None,
        timeout: bool = False,
    ) -> None:
        self._create_time = create_time
        self._uid = os.geteuid() if uid is None else uid
        self.timeout = timeout
        self.terminated = False
        self.killed = False

    def create_time(self) -> float:
        return self._create_time

    def uids(self) -> FakeUids:
        return FakeUids(self._uid)

    def terminate(self) -> None:
        self.terminated = True

    def kill(self) -> None:
        self.killed = True

    def wait(self, timeout: float) -> None:
        if self.timeout:
            raise psutil.TimeoutExpired(timeout, pid=444)


class ProcessTerminatorTests(unittest.TestCase):
    def test_term_then_success(self) -> None:
        process = FakeProcess()
        result = ProcessTerminator(lambda _pid: process, own_pid=999).terminate(444, 100.0)
        self.assertTrue(result.success)
        self.assertTrue(process.terminated)
        self.assertFalse(process.killed)

    def test_force_uses_kill(self) -> None:
        process = FakeProcess()
        result = ProcessTerminator(lambda _pid: process, own_pid=999).terminate(444, 100.0, force=True)
        self.assertTrue(result.success)
        self.assertTrue(process.killed)

    def test_pid_reuse_prevents_signal(self) -> None:
        process = FakeProcess(create_time=200.0)
        result = ProcessTerminator(lambda _pid: process, own_pid=999).terminate(444, 100.0)
        self.assertEqual(result.status, "pid_reused")
        self.assertFalse(process.terminated)

    def test_other_user_is_rejected(self) -> None:
        process = FakeProcess(uid=os.geteuid() + 1)
        result = ProcessTerminator(lambda _pid: process, own_pid=999).terminate(444, 100.0)
        self.assertEqual(result.status, "permission_denied")
        self.assertFalse(process.terminated)

    def test_timeout_requests_force_kill(self) -> None:
        process = FakeProcess(timeout=True)
        result = ProcessTerminator(lambda _pid: process, own_pid=999).terminate(444, 100.0)
        self.assertEqual(result.status, "still_running")

    def test_own_process_is_protected(self) -> None:
        result = ProcessTerminator(lambda _pid: FakeProcess(), own_pid=444).terminate(444, 100.0)
        self.assertEqual(result.status, "protected")

    def test_already_exited_is_successful(self) -> None:
        def missing(_pid: int):
            raise psutil.NoSuchProcess(444)

        result = ProcessTerminator(missing, own_pid=999).terminate(444, 100.0)
        self.assertEqual(result.status, "already_exited")
        self.assertTrue(result.success)


if __name__ == "__main__":
    unittest.main()

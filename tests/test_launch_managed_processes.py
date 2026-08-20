from __future__ import annotations

import io
import os
import shlex
import signal
import subprocess
import sys
import tempfile
import textwrap
import threading
import time
import unittest
from pathlib import Path
from typing import Any

import psutil

from cutting_board.launch_models import LaunchProfile, LaunchState, LaunchTask
from cutting_board.services.managed_processes import (
    ManagedProcessError,
    ManagedProcessRunner,
    resolve_shell_path,
)


class FakeUids:
    def __init__(self, effective: int) -> None:
        self.effective = effective


class FakePsProcess:
    def __init__(self, pid: int, uid: int = 501) -> None:
        self.pid = pid
        self.uid = uid
        self.created = float(pid)

    def create_time(self) -> float:
        return self.created

    def uids(self) -> FakeUids:
        return FakeUids(self.uid)

    def status(self) -> str:
        return psutil.STATUS_RUNNING


class FakePopen:
    def __init__(self, pid: int, output: str = "", *, ignores_term: bool = False) -> None:
        self.pid = pid
        self.stdout = io.StringIO(output)
        self.return_code: int | None = None
        self.ignores_term = ignores_term
        self.finished = threading.Event()

    def poll(self) -> int | None:
        return self.return_code

    def wait(self, timeout: float | None = None) -> int:
        if self.return_code is None and not self.finished.wait(timeout):
            raise subprocess.TimeoutExpired(["fake"], timeout)
        assert self.return_code is not None
        return self.return_code

    def finish(self, return_code: int) -> None:
        self.return_code = return_code
        self.finished.set()


class FakeSpawner:
    def __init__(self, outputs: list[str] | None = None, *, ignores_term: bool = False) -> None:
        self.outputs = outputs or []
        self.ignores_term = ignores_term
        self.calls: list[tuple[list[str], dict[str, Any]]] = []
        self.handles: dict[int, FakePopen] = {}
        self.processes: dict[int, FakePsProcess] = {}
        self.next_pid = 7100

    def popen(self, argv: list[str], **kwargs: Any) -> FakePopen:
        pid = self.next_pid
        self.next_pid += 1
        output = self.outputs[len(self.calls)] if len(self.calls) < len(self.outputs) else ""
        handle = FakePopen(pid, output, ignores_term=self.ignores_term)
        self.calls.append((argv, kwargs))
        self.handles[pid] = handle
        self.processes[pid] = FakePsProcess(pid)
        return handle

    def process(self, pid: int) -> FakePsProcess:
        return self.processes[pid]

    def killpg(self, pgid: int, sig: int) -> None:
        handle = self.handles[pgid]
        if sig == signal.SIGTERM and handle.ignores_term:
            return
        handle.finish(-sig)


def make_profile(root: Path, *, watch: bool = False) -> LaunchProfile:
    return LaunchProfile(
        id="dutypark",
        name="Dutypark",
        project_root=str(root),
        tasks=(
            LaunchTask(
                name="backend",
                cwd=".",
                command="JAVA_HOME=/opt/jdk ./gradlew bootRun --args='--spring.profiles.active=dev'",
                expected_port=8080,
                watch_command="./gradlew classes --continuous" if watch else None,
            ),
            LaunchTask(
                name="frontend",
                cwd=".",
                command="/opt/homebrew/bin/npm run dev",
                expected_port=5173,
            ),
        ),
    )


def make_runner(spawner: FakeSpawner, **kwargs: Any) -> ManagedProcessRunner:
    return ManagedProcessRunner(
        popen_factory=spawner.popen,
        process_factory=spawner.process,
        killpg=spawner.killpg,
        getpgid=lambda pid: pid,
        process_iter=lambda: (
            process
            for pid, process in spawner.processes.items()
            if spawner.handles[pid].poll() is None
        ),
        current_uid=501,
        own_pid=9999,
        stop_timeout_seconds=0.01,
        **kwargs,
    )


class ShellResolutionTests(unittest.TestCase):
    def test_macos_prefers_zsh_when_it_is_available(self) -> None:
        selected = resolve_shell_path(
            platform="darwin",
            login_shell_provider=lambda: "/bin/bash",
            is_executable=lambda path: path in {"/bin/zsh", "/bin/bash"},
        )
        self.assertEqual(selected, "/bin/zsh")

    def test_linux_uses_the_current_users_login_shell(self) -> None:
        selected = resolve_shell_path(
            platform="linux",
            login_shell_provider=lambda: "/bin/bash",
            is_executable=lambda path: path == "/bin/bash",
        )
        self.assertEqual(selected, "/bin/bash")

    def test_unsafe_or_unavailable_login_shell_falls_back_to_sh(self) -> None:
        for candidate in ("zsh", "/usr/sbin/nologin", "/missing/shell", "bad\x00shell"):
            with self.subTest(candidate=candidate):
                selected = resolve_shell_path(
                    platform="linux",
                    login_shell_provider=lambda candidate=candidate: candidate,
                    is_executable=lambda path: path in {"/usr/sbin/nologin"},
                )
                self.assertEqual(selected, "/bin/sh")


class ManagedProcessRunnerTests(unittest.TestCase):
    def test_uses_login_zsh_argv_and_process_group_contract(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner()
            runner = make_runner(spawner, shell_path="/bin/zsh")
            profile = make_profile(Path(directory))

            runner.start_profile(profile)

            self.assertEqual(len(spawner.calls), 2)
            self.assertEqual(
                spawner.calls[0][0],
                ["/bin/zsh", "-lc", profile.tasks[0].command],
            )
            self.assertEqual(
                spawner.calls[1][0],
                ["/bin/zsh", "-lc", "/opt/homebrew/bin/npm run dev"],
            )
            for _argv, options in spawner.calls:
                self.assertEqual(options["cwd"], str(Path(directory).resolve()))
                self.assertTrue(options["start_new_session"])
                self.assertIs(options["stderr"], subprocess.STDOUT)
                self.assertFalse(options["shell"])
            runner.close()

    def test_duplicate_start_is_prevented(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner()
            runner = make_runner(spawner)
            profile = make_profile(Path(directory))
            runner.start_task(profile, "backend")

            with self.assertRaisesRegex(ManagedProcessError, "이미 실행 중"):
                runner.start_task(profile, "backend")
            self.assertEqual(len(spawner.calls), 1)
            runner.close()

    def test_main_and_watch_have_bounded_merged_logs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner(["one\ntwo\n", "watch-one\nwatch-two\n"])
            runner = make_runner(spawner, max_log_lines=3)
            profile = make_profile(Path(directory), watch=True)
            runner.start_task(profile, "backend")

            deadline = time.monotonic() + 1
            snapshot = runner.snapshot(profile.id, "backend")
            while len(snapshot.logs) < 3 and time.monotonic() < deadline:
                snapshot = runner.snapshot(profile.id, "backend")
                time.sleep(0.005)

            self.assertEqual(len(snapshot.logs), 3)
            self.assertTrue(any(line.startswith("[실행]") for line in snapshot.logs))
            self.assertTrue(any(line.startswith("[자동 빌드]") for line in snapshot.logs))
            self.assertIsNotNone(snapshot.main_pid)
            self.assertIsNotNone(snapshot.watch_pid)
            runner.close()

    def test_watch_failure_is_identifiable_while_main_remains_stoppable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner()
            runner = make_runner(spawner)
            profile = make_profile(Path(directory), watch=True)
            runner.start_task(profile, "backend")
            watch = spawner.handles[7101]

            watch.finish(7)
            deadline = time.monotonic() + 1
            snapshot = runner.snapshot(profile.id, "backend")
            while snapshot.state != LaunchState.FAILED and time.monotonic() < deadline:
                snapshot = runner.snapshot(profile.id, "backend")
                time.sleep(0.005)

            self.assertEqual(snapshot.state, LaunchState.FAILED)
            self.assertIn("자동 빌드 프로세스", snapshot.message or "")
            self.assertIsNone(spawner.handles[7100].poll())
            stopped = runner.stop_task(profile.id, "backend")
            self.assertEqual(stopped.state, LaunchState.STOPPED)
            self.assertIsNotNone(spawner.handles[7100].poll())
            self.assertIsNone(stopped.main_pid)
            self.assertIsNone(stopped.watch_pid)

    def test_stop_escalates_from_term_to_kill(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner(ignores_term=True)
            signals: list[tuple[int, int]] = []

            def killpg(pgid: int, sig: int) -> None:
                signals.append((pgid, sig))
                spawner.killpg(pgid, sig)

            runner = ManagedProcessRunner(
                popen_factory=spawner.popen,
                process_factory=spawner.process,
                killpg=killpg,
                getpgid=lambda pid: pid,
                process_iter=lambda: (
                    process
                    for pid, process in spawner.processes.items()
                    if spawner.handles[pid].poll() is None
                ),
                current_uid=501,
                own_pid=9999,
                stop_timeout_seconds=0.01,
            )
            profile = make_profile(Path(directory))
            runner.start_task(profile, "backend")

            snapshot = runner.stop_task(profile.id, "backend")

            self.assertEqual(
                signals,
                [(7100, signal.SIGTERM), (7100, signal.SIGKILL)],
            )
            self.assertEqual(snapshot.state, LaunchState.STOPPED)

    def test_pid_reuse_prevents_group_signal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner()
            signals: list[tuple[int, int]] = []
            runner = ManagedProcessRunner(
                popen_factory=spawner.popen,
                process_factory=spawner.process,
                killpg=lambda pgid, sig: signals.append((pgid, sig)),
                getpgid=lambda pid: pid,
                process_iter=lambda: spawner.processes.values(),
                current_uid=501,
                own_pid=9999,
                stop_timeout_seconds=0,
            )
            profile = make_profile(Path(directory))
            runner.start_task(profile, "backend")
            spawner.processes[7100].created += 10

            snapshot = runner.stop_task(profile.id, "backend")

            self.assertEqual(signals, [])
            self.assertEqual(snapshot.state, LaunchState.FAILED)
            spawner.handles[7100].finish(0)

    def test_failed_spawn_verification_cleans_the_new_process_group(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner()
            signals: list[tuple[int, int]] = []

            def killpg(pgid: int, sig: int) -> None:
                signals.append((pgid, sig))
                spawner.killpg(pgid, sig)

            runner = ManagedProcessRunner(
                popen_factory=spawner.popen,
                process_factory=lambda _pid: (_ for _ in ()).throw(OSError("unavailable")),
                killpg=killpg,
                getpgid=lambda pid: pid,
                process_iter=lambda: (),
                current_uid=501,
                own_pid=9999,
                stop_timeout_seconds=0.01,
            )

            snapshot = runner.start_task(make_profile(Path(directory)), "backend")

            self.assertEqual(snapshot.state, LaunchState.FAILED)
            self.assertEqual(
                signals,
                [(7100, signal.SIGTERM), (7100, signal.SIGKILL)],
            )
            self.assertIsNotNone(spawner.handles[7100].poll())

    def test_kills_owned_group_child_even_after_leader_exits(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner()
            child = FakePsProcess(7200)
            child.created = 7200.0
            child_alive = True
            signals: list[tuple[int, int]] = []

            def killpg(pgid: int, sig: int) -> None:
                nonlocal child_alive
                signals.append((pgid, sig))
                if sig == signal.SIGTERM:
                    spawner.handles[pgid].finish(-sig)
                else:
                    child_alive = False

            def process_iter():
                if child_alive:
                    yield child
                if spawner.handles[7100].poll() is None:
                    yield spawner.processes[7100]

            runner = ManagedProcessRunner(
                popen_factory=spawner.popen,
                process_factory=spawner.process,
                killpg=killpg,
                getpgid=lambda pid: 7100 if pid == 7200 else pid,
                process_iter=process_iter,
                current_uid=501,
                own_pid=9999,
                stop_timeout_seconds=0.01,
            )
            profile = make_profile(Path(directory))
            runner.start_task(profile, "backend")

            snapshot = runner.stop_task(profile.id, "backend")

            self.assertEqual(
                signals,
                [(7100, signal.SIGTERM), (7100, signal.SIGKILL)],
            )
            self.assertFalse(child_alive)
            self.assertEqual(snapshot.state, LaunchState.STOPPED)

    def test_close_is_idempotent_stops_only_owned_tasks_and_rejects_start(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            spawner = FakeSpawner()
            runner = make_runner(spawner)
            profile = make_profile(Path(directory))
            runner.start_task(profile, "backend")

            runner.close()
            runner.close()

            self.assertIsNotNone(spawner.handles[7100].poll())
            with self.assertRaisesRegex(ManagedProcessError, "이미 종료"):
                runner.start_task(profile, "frontend")

    def test_missing_working_directory_is_failed_without_spawning(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            profile = LaunchProfile(
                id="missing",
                name="Missing",
                project_root=str(root),
                tasks=(LaunchTask(name="web", cwd="missing", command="run"),),
            )
            spawner = FakeSpawner()
            runner = make_runner(spawner)

            snapshot = runner.start_task(profile, "web")

            self.assertEqual(snapshot.state, LaunchState.FAILED)
            self.assertIn("찾을 수 없습니다", snapshot.message or "")
            self.assertEqual(spawner.calls, [])

    @unittest.skipUnless(os.name == "posix" and Path("/bin/zsh").exists(), "POSIX process groups required")
    def test_real_process_group_kills_term_ignoring_child_after_leader_exits(self) -> None:
        child_helper = (
            "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
            'print("CHILD_READY", flush=True); time.sleep(60)'
        )
        helper = textwrap.dedent(
            f"""\
            import os, signal, subprocess, sys, time
            child = subprocess.Popen([sys.executable, '-c', {child_helper!r}])
            print(f'READY {{os.getpid()}} {{os.getpgrp()}} {{child.pid}}', flush=True)
            signal.signal(signal.SIGTERM, lambda *_args: sys.exit(0))
            time.sleep(60)
            """
        )
        command = f"{shlex.quote(sys.executable)} -c {shlex.quote(helper)}"
        child_pid: int | None = None
        pgid: int | None = None

        def child_processes() -> tuple[psutil.Process, ...]:
            if child_pid is None:
                return ()
            try:
                return (psutil.Process(child_pid),)
            except psutil.NoSuchProcess:
                return ()

        runner = ManagedProcessRunner(
            stop_timeout_seconds=0.15,
            process_iter=child_processes,
        )
        try:
            with tempfile.TemporaryDirectory() as directory:
                profile = LaunchProfile(
                    id="real-group",
                    name="Real group",
                    project_root=directory,
                    tasks=(LaunchTask(name="helper", cwd=".", command=command),),
                )
                started = runner.start_task(profile, "helper")
                self.assertEqual(started.state, LaunchState.RUNNING)
                pgid = started.main_pid

                deadline = time.monotonic() + 3
                ready_line = ""
                while time.monotonic() < deadline:
                    snapshot = runner.snapshot(profile.id, "helper")
                    ready_line = next((line for line in snapshot.logs if "READY " in line), "")
                    child_ready = any("CHILD_READY" in line for line in snapshot.logs)
                    if ready_line and child_ready:
                        break
                    time.sleep(0.01)
                self.assertTrue(ready_line)
                self.assertTrue(child_ready)
                _prefix, _ready, leader_text, group_text, child_text = ready_line.split()
                child_pid = int(child_text)
                self.assertEqual(int(leader_text), pgid)
                self.assertEqual(int(group_text), pgid)
                self.assertEqual(os.getpgid(child_pid), pgid)

                stopped = runner.stop_task(profile.id, "helper")
                self.assertEqual(stopped.state, LaunchState.STOPPED)
                deadline = time.monotonic() + 2
                while child_pid is not None and psutil.pid_exists(child_pid) and time.monotonic() < deadline:
                    try:
                        if psutil.Process(child_pid).status() == psutil.STATUS_ZOMBIE:
                            break
                    except psutil.NoSuchProcess:
                        break
                    time.sleep(0.01)
                if child_pid is not None and psutil.pid_exists(child_pid):
                    self.assertEqual(psutil.Process(child_pid).status(), psutil.STATUS_ZOMBIE)
        finally:
            runner.close(timeout_seconds=0.1)
            if pgid is not None and pgid != os.getpgrp():
                try:
                    os.killpg(pgid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


if __name__ == "__main__":
    unittest.main()

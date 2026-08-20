from __future__ import annotations

import queue
import tempfile
import unittest
from collections.abc import Iterable
from pathlib import Path

from cutting_board.launch_controller import LaunchController
from cutting_board.launch_models import (
    LaunchEvent,
    LaunchProfile,
    LaunchState,
    LaunchTask,
    ManagedTaskSnapshot,
)
from cutting_board.services.launch_profiles import LaunchProfileStoreError
from cutting_board.services.managed_processes import ManagedProcessError


def make_profile(root: Path, profile_id: str = "profile") -> LaunchProfile:
    return LaunchProfile(
        id=profile_id,
        name=profile_id,
        project_root=str(root),
        tasks=(LaunchTask(name="web", cwd=".", command="npm run dev", expected_port=5173),),
    )


class FakeStore:
    def __init__(self, profiles: tuple[LaunchProfile, ...]) -> None:
        self.saved = profiles
        self.fail = False

    def load(self) -> tuple[LaunchProfile, ...]:
        return self.saved

    def save(self, profiles: Iterable[LaunchProfile]) -> tuple[LaunchProfile, ...]:
        if self.fail:
            raise LaunchProfileStoreError("save failed")
        self.saved = tuple(profiles)
        return self.saved


class FakeRunner:
    def __init__(self) -> None:
        self.events: queue.Queue[LaunchEvent] = queue.Queue()
        self.active: set[str] = set()
        self.calls: list[tuple[str, ...]] = []
        self.closed = 0

    def is_profile_active(self, profile_id: str) -> bool:
        return profile_id in self.active

    def start_task(self, profile: LaunchProfile, task_name: str) -> ManagedTaskSnapshot:
        self.calls.append(("start_task", profile.id, task_name))
        self.active.add(profile.id)
        return ManagedTaskSnapshot(profile.id, task_name, LaunchState.RUNNING)

    def start_profile(self, profile: LaunchProfile) -> tuple[ManagedTaskSnapshot, ...]:
        self.calls.append(("start_profile", profile.id))
        self.active.add(profile.id)
        return tuple(
            ManagedTaskSnapshot(profile.id, task.name, LaunchState.RUNNING)
            for task in profile.tasks
        )

    def stop_task(self, profile_id: str, task_name: str) -> ManagedTaskSnapshot:
        self.calls.append(("stop_task", profile_id, task_name))
        self.active.discard(profile_id)
        return ManagedTaskSnapshot(profile_id, task_name, LaunchState.STOPPED)

    def stop_profile(self, profile_id: str) -> tuple[ManagedTaskSnapshot, ...]:
        self.calls.append(("stop_profile", profile_id))
        self.active.discard(profile_id)
        return ()

    def snapshot(self, profile_id: str, task_name: str) -> ManagedTaskSnapshot:
        state = LaunchState.RUNNING if profile_id in self.active else LaunchState.STOPPED
        return ManagedTaskSnapshot(profile_id, task_name, state)

    def snapshots(self, profile_id: str | None = None) -> tuple[ManagedTaskSnapshot, ...]:
        return tuple(
            ManagedTaskSnapshot(candidate, "web", LaunchState.RUNNING)
            for candidate in self.active
            if profile_id is None or candidate == profile_id
        )

    def close(self) -> None:
        self.closed += 1
        self.active.clear()


class LaunchControllerTests(unittest.TestCase):
    def test_crud_updates_memory_only_after_store_success(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = make_profile(root, "first")
            second = make_profile(root, "second")
            store = FakeStore((first,))
            controller = LaunchController(store=store, runner=FakeRunner())

            self.assertEqual(controller.save_profile(second), (first, second))
            store.fail = True
            replacement = LaunchProfile(
                id="first",
                name="changed",
                project_root=str(root),
                tasks=first.tasks,
            )
            with self.assertRaises(LaunchProfileStoreError):
                controller.save_profile(replacement)
            self.assertEqual(controller.profiles, (first, second))
            store.fail = False
            self.assertTrue(controller.delete_profile("first"))
            self.assertFalse(controller.delete_profile("missing"))

    def test_start_stop_and_event_queue_are_forwarded(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            profile = make_profile(Path(directory))
            runner = FakeRunner()
            controller = LaunchController(store=FakeStore((profile,)), runner=runner)

            started = controller.start_task(profile.id, "web")
            self.assertEqual(started.state, LaunchState.RUNNING)
            self.assertEqual(controller.snapshot(profile.id, "web").state, LaunchState.RUNNING)
            self.assertIs(controller.events, runner.events)
            stopped = controller.stop_task(profile.id, "web")
            self.assertEqual(stopped.state, LaunchState.STOPPED)
            self.assertEqual(
                runner.calls,
                [("start_task", profile.id, "web"), ("stop_task", profile.id, "web")],
            )

    def test_running_profile_cannot_be_changed_or_deleted(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            profile = make_profile(Path(directory))
            runner = FakeRunner()
            runner.active.add(profile.id)
            controller = LaunchController(store=FakeStore((profile,)), runner=runner)

            with self.assertRaisesRegex(ManagedProcessError, "edited"):
                controller.save_profile(profile)
            with self.assertRaisesRegex(ManagedProcessError, "deleted"):
                controller.delete_profile(profile.id)

    def test_unregistered_profile_is_not_started(self) -> None:
        controller = LaunchController(store=FakeStore(()), runner=FakeRunner())
        with self.assertRaisesRegex(KeyError, "Unknown launch profile"):
            controller.start_profile("missing")

    def test_close_is_idempotent_and_rejects_new_starts(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            profile = make_profile(Path(directory))
            runner = FakeRunner()
            controller = LaunchController(store=FakeStore((profile,)), runner=runner)

            controller.close()
            controller.close()

            self.assertEqual(runner.closed, 1)
            with self.assertRaisesRegex(ManagedProcessError, "already closed"):
                controller.start_task(profile.id, "web")


if __name__ == "__main__":
    unittest.main()

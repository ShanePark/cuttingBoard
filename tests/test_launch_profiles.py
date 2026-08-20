from __future__ import annotations

import json
import stat
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from cutting_board.launch_models import LaunchProfile, LaunchTask
from cutting_board.services.launch_profiles import LaunchProfileStore, LaunchProfileStoreError


def make_profile(root: Path, profile_id: str = "dutypark") -> LaunchProfile:
    return LaunchProfile(
        id=profile_id,
        name="Dutypark Development",
        project_root=str(root),
        tasks=(
            LaunchTask(
                name="backend",
                cwd="backend",
                command="JAVA_HOME=/opt/jdk ./gradlew bootRun --args='--spring.profiles.active=dev'",
                expected_port=8080,
                watch_command="./gradlew classes --continuous",
            ),
            LaunchTask(
                name="frontend",
                cwd="frontend",
                command="/opt/homebrew/bin/npm run dev",
                expected_port=5173,
            ),
        ),
    )


class LaunchModelTests(unittest.TestCase):
    def test_profile_resolves_relative_task_directories(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            profile = make_profile(root)
            self.assertEqual(profile.task_cwd(profile.tasks[0]), (root / "backend").resolve())
            self.assertIsNone(
                LaunchTask(name="worker", cwd=".", command="./run-worker").expected_port
            )

    def test_validation_rejects_invalid_fields_and_project_escape(self) -> None:
        with self.assertRaisesRegex(ValueError, "Task name"):
            LaunchTask(name=" ", cwd=".", command="run")
        with self.assertRaisesRegex(ValueError, "between 1 and 65535"):
            LaunchTask(name="web", cwd=".", command="run", expected_port=70000)
        with self.assertRaisesRegex(ValueError, "integer"):
            LaunchTask(name="web", cwd=".", command="run", expected_port=True)
        with self.assertRaisesRegex(ValueError, "Auto-build command"):
            LaunchTask(name="web", cwd=".", command="run", watch_command=" ")
        with tempfile.TemporaryDirectory() as directory, self.assertRaisesRegex(
            ValueError, "inside the project folder"
        ):
            LaunchProfile(
                id="escape",
                name="Escape",
                project_root=directory,
                tasks=(LaunchTask(name="web", cwd="../outside", command="run"),),
            )

    def test_validation_rejects_duplicate_task_names(self) -> None:
        with tempfile.TemporaryDirectory() as directory, self.assertRaisesRegex(ValueError, "Duplicate"):
            LaunchProfile(
                id="duplicate",
                name="Duplicate",
                project_root=directory,
                tasks=(
                    LaunchTask(name="Web", cwd=".", command="one"),
                    LaunchTask(name="web", cwd=".", command="two"),
                ),
            )


class LaunchProfileStoreTests(unittest.TestCase):
    def test_round_trip_is_atomic_and_private(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "backend").mkdir()
            (root / "frontend").mkdir()
            path = root / "config" / "launch_profiles.json"
            store = LaunchProfileStore(path)
            expected = (make_profile(root),)

            self.assertEqual(store.save(expected), expected)
            self.assertEqual(store.load(), expected)
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            self.assertEqual(list(path.parent.glob("*.tmp")), [])

    def test_upsert_and_delete_preserve_other_profiles(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            store = LaunchProfileStore(root / "launch_profiles.json")
            first = make_profile(root, "first")
            second = make_profile(root, "second")
            store.save((first, second))
            replacement = LaunchProfile(
                id="first",
                name="Updated",
                project_root=str(root),
                tasks=(LaunchTask(name="worker", cwd=".", command="./worker"),),
            )

            self.assertEqual(store.upsert(replacement), (replacement, second))
            self.assertEqual(store.delete("first"), (second,))

    def test_corrupt_or_unsupported_json_is_reported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "launch_profiles.json"
            path.write_text("{broken", encoding="utf-8")
            with self.assertRaisesRegex(LaunchProfileStoreError, "Unable to read"):
                LaunchProfileStore(path).load()
            path.write_text(json.dumps({"version": 2, "profiles": []}), encoding="utf-8")
            with self.assertRaisesRegex(LaunchProfileStoreError, "Unsupported"):
                LaunchProfileStore(path).load()

    def test_duplicate_profile_ids_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            profile = make_profile(root)
            with self.assertRaisesRegex(ValueError, "Duplicate"):
                LaunchProfileStore(root / "launch_profiles.json").save((profile, profile))

    def test_replace_failure_keeps_previous_file_and_cleans_temporary_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "launch_profiles.json"
            path.write_text('{"version": 1, "profiles": []}\n', encoding="utf-8")
            original = path.read_bytes()
            with patch(
                "cutting_board.services.launch_profiles.os.replace", side_effect=OSError
            ), self.assertRaisesRegex(LaunchProfileStoreError, "Unable to save"):
                LaunchProfileStore(path).save((make_profile(root),))
            self.assertEqual(path.read_bytes(), original)
            self.assertEqual(list(root.glob("*.tmp")), [])


if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from scripts.dev import ChangeDebouncer, app_arguments, changes, snapshot


class SnapshotTests(unittest.TestCase):
    def test_snapshot_tracks_only_selected_source_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            source = root / "module.py"
            ignored = root / "settings.json"
            cache = root / "__pycache__"
            cache.mkdir()
            source.write_text("first\n", encoding="utf-8")
            ignored.write_text("{}\n", encoding="utf-8")
            (cache / "module.py").write_text("cached\n", encoding="utf-8")

            before = snapshot((root,), (".py",))
            source.write_text("second and longer\n", encoding="utf-8")
            os.utime(source, None)
            after = snapshot((root,), (".py",))

            self.assertEqual(list(before), [source])
            self.assertEqual(changes(before, after), [source])


class ChangeDebouncerTests(unittest.TestCase):
    def test_batches_changes_until_the_quiet_period_ends(self) -> None:
        debouncer = ChangeDebouncer(delay_seconds=0.3)
        first = Path("first.py")
        second = Path("second.py")

        self.assertEqual(debouncer.observe([first], now=1.0), [])
        self.assertEqual(debouncer.observe([second], now=1.2), [])
        self.assertEqual(debouncer.observe([], now=1.49), [])
        self.assertEqual(debouncer.observe([], now=1.5), [first, second])
        self.assertEqual(debouncer.observe([], now=2.0), [])


class AppArgumentsTests(unittest.TestCase):
    def test_accepts_direct_application_flags(self) -> None:
        self.assertEqual(app_arguments(["--demo", "--auto-close", "1"]), ["--demo", "--auto-close", "1"])

    def test_removes_a_leading_separator(self) -> None:
        self.assertEqual(app_arguments(["--", "--demo"]), ["--demo"])


if __name__ == "__main__":
    unittest.main()

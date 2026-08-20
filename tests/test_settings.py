from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from cutting_board.services.settings import SettingsStore, UISettings


class SettingsTests(unittest.TestCase):
    def test_round_trip(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "settings.json"
            store = SettingsStore(path)
            expected = UISettings(
                show_system_services=True,
                window_geometry="1200x700+10+20",
                scan_interval_seconds=4.0,
                collapsed_project_ids=["project:a"],
            )
            store.save(expected)
            self.assertEqual(store.load(), expected)

    def test_invalid_values_fall_back_or_clamp(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "settings.json"
            path.write_text(
                json.dumps(
                    {
                        "window_geometry": "20x10",
                        "scan_interval_seconds": 999,
                        "collapsed_project_ids": "wrong",
                    }
                ),
                encoding="utf-8",
            )
            settings = SettingsStore(path).load()
            self.assertEqual(settings.window_geometry, "1280x820")
            self.assertEqual(settings.scan_interval_seconds, 30.0)
            self.assertEqual(settings.collapsed_project_ids, [])

    def test_corrupt_json_returns_defaults(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "settings.json"
            path.write_text("{not-json", encoding="utf-8")
            self.assertEqual(SettingsStore(path).load(), UISettings())

    def test_unwritable_config_directory_does_not_crash_the_app(self) -> None:
        store = SettingsStore(Path("/unwritable/settings.json"))
        with patch.object(Path, "mkdir", side_effect=PermissionError("denied")):
            store.save(UISettings())


if __name__ == "__main__":
    unittest.main()

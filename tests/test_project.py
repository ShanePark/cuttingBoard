from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from cutting_board.scanner.project import ProjectResolver


class ProjectResolverTests(unittest.TestCase):
    def test_git_root_groups_nested_package(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "workspace"
            package = root / "apps" / "web"
            package.mkdir(parents=True)
            (root / ".git").mkdir()
            (root / "package.json").write_text(json.dumps({"name": "platform"}), encoding="utf-8")
            (package / "package.json").write_text(json.dumps({"name": "storefront"}), encoding="utf-8")

            resolution = ProjectResolver().resolve(str(package))

            self.assertIsNotNone(resolution)
            assert resolution is not None
            self.assertEqual(resolution.project.root_path, str(root))
            self.assertEqual(resolution.project.name, "platform")
            self.assertEqual(resolution.package_name, "storefront")
            self.assertEqual(resolution.project.detection_source, ".git")

    def test_nearest_marker_is_used_without_git(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "service"
            nested = root / "src" / "main"
            nested.mkdir(parents=True)
            (root / "pom.xml").write_text("<project/>", encoding="utf-8")

            resolution = ProjectResolver().resolve(str(nested))

            self.assertIsNotNone(resolution)
            assert resolution is not None
            self.assertEqual(resolution.project.root_path, str(root))
            self.assertEqual(resolution.project.name, "service")
            self.assertEqual(resolution.project.detection_source, "pom.xml")

    def test_no_marker_returns_none(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.assertIsNone(ProjectResolver().resolve(directory))

    def test_a_previous_miss_does_not_hide_a_new_project_marker(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            resolver = ProjectResolver()
            self.assertIsNone(resolver.resolve(str(root)))
            (root / ".git").mkdir()
            resolution = resolver.resolve(str(root))
            self.assertIsNotNone(resolution)
            assert resolution is not None
            self.assertEqual(resolution.project.root_path, str(root))


if __name__ == "__main__":
    unittest.main()

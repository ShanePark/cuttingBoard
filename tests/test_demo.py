from __future__ import annotations

import json
import unittest

from cutting_board.demo import demo_snapshot


class DemoTests(unittest.TestCase):
    def test_demo_snapshot_is_complete_and_json_serializable(self) -> None:
        snapshot = demo_snapshot()
        self.assertGreaterEqual(snapshot.project_count, 2)
        self.assertGreaterEqual(len(snapshot.services), 5)
        self.assertIn("Spring Boot", {item.display_name for item in snapshot.services})
        payload = snapshot.to_dict()
        json.dumps(payload)
        self.assertNotIn("command", payload["services"][0]["process"])

    def test_demo_snapshot_shows_both_known_and_unknown_launchers(self) -> None:
        # The screenshots are taken from this data, so it has to exercise the
        # badge and its absence rather than only one of the two.
        services = {item.display_name: item for item in demo_snapshot().services}
        self.assertEqual(services["Vite"].origin_kind, "agent")
        self.assertEqual(services["Vite"].origin_label, "Claude")
        self.assertEqual(services["Storybook"].origin_kind, "ide")
        self.assertEqual(services["Jupyter"].origin_kind, "unknown")
        self.assertEqual(services["Jupyter"].origin_label, "")


if __name__ == "__main__":
    unittest.main()

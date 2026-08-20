from __future__ import annotations

import unittest

from cutting_board.presentation import (
    container_count,
    format_bytes,
    format_cpu,
    format_duration,
    group_services,
    visible_services,
)
from tests.helpers import make_project, make_service, make_snapshot


class PresentationTests(unittest.TestCase):
    def test_noise_is_never_visible(self) -> None:
        project = make_project()
        dev_service = make_service(project=project)
        noise = make_service(
            service_id="service:noise",
            name="ulauncher",
            port=5054,
            relevance="noise",
        )
        snapshot = make_snapshot(dev_service, noise)

        self.assertEqual(visible_services(snapshot), (dev_service,))
        # Noise stays hidden even when containers are asked for.
        self.assertEqual(visible_services(snapshot, show_containers=True), (dev_service,))

    def test_containers_are_hidden_until_asked_for(self) -> None:
        dev_service = make_service(project=make_project())
        container = make_service(
            service_id="service:container",
            name="Docker port proxy",
            port=54321,
            tech="docker",
            relevance="container",
        )
        snapshot = make_snapshot(dev_service, container)

        self.assertEqual(visible_services(snapshot), (dev_service,))
        self.assertEqual(
            visible_services(snapshot, show_containers=True),
            (dev_service, container),
        )
        self.assertEqual(container_count(snapshot), 1)

    def test_search_matches_port_command_and_project(self) -> None:
        project = make_project("checkout")
        service = make_service(project=project, port=4317)
        snapshot = make_snapshot(service)
        self.assertEqual(visible_services(snapshot, query="4317"), (service,))
        self.assertEqual(visible_services(snapshot, query="pnpm run dev"), (service,))
        self.assertEqual(visible_services(snapshot, query="CHECKOUT"), (service,))

    def test_group_order_is_project_then_unassigned_then_containers(self) -> None:
        project = make_project("a-project")
        groups = group_services(
            (
                make_service(service_id="s2", project=None, port=8000),
                make_service(service_id="s3", relevance="container", port=54321),
                make_service(service_id="s1", project=project, port=3000),
            )
        )
        self.assertEqual([group.name for group in groups], ["a-project", "Other", "Containers"])

    def test_formatters(self) -> None:
        self.assertEqual(format_bytes(1024 * 1024), "1.0 MB")
        self.assertEqual(format_duration(3660), "1 hour 1 minute")
        self.assertEqual(format_cpu(None), "Calculating")
        self.assertEqual(format_cpu(2.345), "2.3%")


if __name__ == "__main__":
    unittest.main()

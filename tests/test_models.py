from __future__ import annotations

import unittest
from dataclasses import replace

from cutting_board.models import ServiceCategory
from tests.helpers import make_service


class ModelTests(unittest.TestCase):
    def test_api_uses_https_on_known_tls_port(self) -> None:
        service = replace(make_service(port=8443), category=ServiceCategory.API)
        self.assertEqual(service.browser_url(), "https://localhost:8443")

    def test_database_has_no_browser_action(self) -> None:
        service = replace(make_service(port=5432), category=ServiceCategory.DATABASE)
        self.assertIsNone(service.browser_url())

    def test_unknown_custom_tcp_port_has_no_browser_action(self) -> None:
        service = replace(make_service(port=45678), category=ServiceCategory.OTHER)
        self.assertIsNone(service.browser_url())

    def test_known_dev_port_is_browser_candidate_even_if_unclassified(self) -> None:
        service = replace(make_service(port=3000), category=ServiceCategory.OTHER)
        self.assertEqual(service.browser_url(), "http://localhost:3000")


if __name__ == "__main__":
    unittest.main()

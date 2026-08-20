from __future__ import annotations

import unittest

from cutting_board.models import ServiceCategory
from cutting_board.scanner.classifier import classify_service
from cutting_board.scanner.linux import redact_command


class ClassifierTests(unittest.TestCase):
    def test_vite_in_monorepo_uses_package_name(self) -> None:
        result = classify_service(
            "node",
            ("node", "node_modules/vite/bin/vite.js"),
            "/usr/bin/node",
            "storefront",
            "shop-platform",
        )
        self.assertEqual(result.name, "storefront · Vite")
        self.assertEqual(result.category, ServiceCategory.WEB)
        self.assertEqual(result.tech, "vite")
        self.assertTrue(result.specific)

    def test_spring_boot_is_api(self) -> None:
        result = classify_service(
            "java", ("./gradlew", "bootRun"), "/usr/bin/java", None, "billing-api"
        )
        self.assertEqual(result.name, "Spring Boot")
        self.assertEqual(result.category, ServiceCategory.API)
        self.assertEqual(result.tech, "spring")

    def test_jar_version_is_removed_from_display_name(self) -> None:
        result = classify_service(
            "java",
            ("java", "-jar", "inventory-service-1.4.2.jar"),
            "/usr/bin/java",
            None,
            None,
        )
        self.assertEqual(result.name, "inventory-service")
        self.assertEqual(result.category, ServiceCategory.API)
        self.assertEqual(result.tech, "java")

    def test_classpath_dependency_does_not_steal_the_identity(self) -> None:
        """A Spring service shipping the JDBC driver is not PostgreSQL."""
        result = classify_service(
            "java",
            (
                "java",
                "-cp",
                "/app/lib/postgresql-42.7.4.jar:/app/lib/spring-boot-3.2.0.jar",
                "org.springframework.boot.loader.JarLauncher",
            ),
            "/usr/bin/java",
            None,
            "billing-api",
        )
        self.assertEqual(result.tech, "spring")
        self.assertEqual(result.category, ServiceCategory.API)

    def test_needles_only_match_on_word_boundaries(self) -> None:
        """A directory named ``invite-app`` must not be read as Vite."""
        result = classify_service(
            "node",
            ("node", "/home/dev/invite-app/node_modules/next/dist/bin/next", "dev"),
            "/usr/bin/node",
            None,
            "invite-app",
        )
        self.assertEqual(result.name, "Next.js")
        self.assertEqual(result.tech, "nextjs")

    def test_unrecognised_runtime_is_not_specific(self) -> None:
        result = classify_service("node", ("node", "server.js"), "/usr/bin/node", None, None)
        self.assertFalse(result.specific)
        self.assertEqual(result.tech, "node")

    def test_command_redacts_separate_and_inline_secrets(self) -> None:
        rendered = redact_command(
            (
                "server",
                "--token",
                "top-secret",
                "API_KEY=another-secret",
                "--port=8080",
            )
        )
        self.assertNotIn("top-secret", rendered)
        self.assertNotIn("another-secret", rendered)
        self.assertIn("••••", rendered)
        self.assertIn("--port=8080", rendered)

    def test_json_export_never_contains_raw_secret_tokens(self) -> None:
        from dataclasses import replace
        from cutting_board.demo import demo_snapshot

        snapshot = demo_snapshot()
        first = snapshot.services[0]
        assert first.process is not None
        secret_process = replace(
            first.process,
            command=("server", "--token", "raw-secret"),
            command_display="server --token '••••'",
        )
        secret_service = replace(first, process=secret_process)
        exported = replace(snapshot, services=(secret_service,)).to_dict()
        rendered = str(exported)
        self.assertNotIn("raw-secret", rendered)
        self.assertNotIn("'command':", rendered)


if __name__ == "__main__":
    unittest.main()

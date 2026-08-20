from __future__ import annotations

import unittest

from cutting_board.models import Ownership
from cutting_board.scanner.classifier import classify_service
from cutting_board.scanner.relevance import Relevance, relevance_of

GRADLE_HOME = "/home/dev/.gradle/wrapper/dists/gradle-8.14.3/ab12cd/gradle-8.14.3"

# The classpath a Spring Boot service gets from ./gradlew bootRun: every jar
# sits under ~/.gradle, so "gradle" appears many times over without the
# process being a build daemon.
BOOT_CLASSPATH = ":".join(
    (
        "/home/dev/projects/billing/build/classes/java/main",
        "/home/dev/projects/billing/build/resources/main",
        "/home/dev/.gradle/caches/modules-2/files-2.1/org.springframework.boot"
        "/spring-boot/2.7.18/f6dbdd8/spring-boot-2.7.18.jar",
        "/home/dev/.gradle/caches/modules-2/files-2.1/org.postgresql"
        "/postgresql/42.7.4/264310f/postgresql-42.7.4.jar",
    )
)


def classify_and_rank(
    process_name: str,
    command: tuple[str, ...],
    executable: str | None = "/usr/bin/java",
    *,
    has_project: bool = True,
) -> Relevance:
    """Rank a listener the way the scanner does, classifier included.

    The daemon rule has to survive the classifier recognising the process, so
    the two steps are exercised together rather than with a hand-set flag.
    """
    classification = classify_service(process_name, command, executable, None, None)
    return relevance_of(
        ownership=Ownership.CURRENT_USER,
        process_name=process_name,
        executable=executable,
        command=command,
        tech=classification.tech,
        specific=classification.specific,
        has_project=has_project,
    )


class BuildDaemonRelevanceTests(unittest.TestCase):
    def test_gradle_daemon_is_noise(self) -> None:
        command = (
            "/usr/lib/jvm/java-17/bin/java",
            "-Xmx2048m",
            "-cp",
            f"{GRADLE_HOME}/lib/gradle-daemon-main-8.14.3.jar",
            f"-javaagent:{GRADLE_HOME}/lib/agents/gradle-instrumentation-agent-8.14.3.jar",
            "org.gradle.launcher.daemon.bootstrap.GradleDaemon",
            "8.14.3",
        )
        self.assertEqual(classify_and_rank("java", command), Relevance.NOISE)

    def test_gradle_daemon_keeps_its_name(self) -> None:
        """The rule hides the daemon; naming it is still the classifier's job."""
        command = (
            "java",
            "-cp",
            f"{GRADLE_HOME}/lib/gradle-daemon-main-8.14.3.jar",
            "org.gradle.launcher.daemon.bootstrap.GradleDaemon",
        )
        self.assertEqual(classify_service("java", command, "/usr/bin/java", None, None).name, "Gradle Daemon")

    def test_gradle_daemon_launcher_jar_alone_is_noise(self) -> None:
        """The jar is enough; the main class need not survive on argv."""
        command = ("java", "-cp", f"{GRADLE_HOME}/lib/gradle-daemon-main-8.14.3.jar", "8.14.3")
        self.assertEqual(classify_and_rank("java", command), Relevance.NOISE)

    def test_gradle_worker_is_noise(self) -> None:
        command = (
            "java",
            "-cp",
            f"{GRADLE_HOME}/lib/gradle-worker.jar",
            "org.gradle.process.internal.worker.GradleWorkerMain",
            "'Gradle Test Executor 3'",
        )
        self.assertEqual(classify_and_rank("java", command), Relevance.NOISE)

    def test_kotlin_compile_daemon_is_noise(self) -> None:
        command = (
            "java",
            "-cp",
            "/home/dev/.gradle/caches/kotlin-daemon/kotlin-daemon-embeddable-1.9.24.jar",
            "org.jetbrains.kotlin.daemon.KotlinCompileDaemon",
        )
        self.assertEqual(classify_and_rank("java", command), Relevance.NOISE)

    def test_maven_daemon_main_class_is_noise(self) -> None:
        command = (
            "java",
            "-classpath",
            "/home/dev/.m2/mvnd/lib/mvnd-daemon-1.0.2.jar",
            "org.apache.maven.cli.DaemonMavenCli",
        )
        self.assertEqual(classify_and_rank("java", command), Relevance.NOISE)

    def test_mvnd_server_main_class_is_noise(self) -> None:
        command = ("java", "-cp", "/opt/mvnd/lib/mvnd-common.jar", "org.mvndaemon.mvnd.daemon.Server")
        self.assertEqual(classify_and_rank("java", command), Relevance.NOISE)

    def test_mvnd_executable_is_noise(self) -> None:
        command = ("/home/dev/.sdkman/candidates/mvnd/current/bin/mvnd", "clean", "install")
        self.assertEqual(
            classify_and_rank("mvnd", command, "/home/dev/.sdkman/candidates/mvnd/current/bin/mvnd"),
            Relevance.NOISE,
        )

    def test_nailgun_server_is_noise(self) -> None:
        command = ("java", "-cp", "/home/dev/.cache/nailgun/nailgun-server-1.0.0.jar", "com.facebook.nailgun.NGServer")
        self.assertEqual(classify_and_rank("java", command), Relevance.NOISE)

    def test_legacy_nailgun_server_is_noise(self) -> None:
        command = ("java", "-cp", "/opt/nailgun/nailgun.jar", "com.martiansoftware.nailgun.NGServer")
        self.assertEqual(classify_and_rank("java", command), Relevance.NOISE)


class ApplicationsLaunchedByGradleTests(unittest.TestCase):
    def test_application_on_a_gradle_classpath_is_dev(self) -> None:
        """Every jar comes from ~/.gradle; the process is still the service."""
        command = ("/usr/lib/jvm/java-17/bin/java", "-cp", BOOT_CLASSPATH, "com.acme.billing.Application")
        self.assertEqual(classify_and_rank("java", command), Relevance.DEV)

    def test_application_without_a_project_is_still_dev(self) -> None:
        """Spring Boot is recognised, so the daemon rule must not steal it."""
        command = ("/usr/lib/jvm/java-17/bin/java", "-cp", BOOT_CLASSPATH, "com.acme.billing.Application")
        self.assertEqual(classify_and_rank("java", command, has_project=False), Relevance.DEV)

    def test_gradlew_boot_run_is_dev(self) -> None:
        command = ("./gradlew", "bootRun")
        self.assertEqual(classify_and_rank("java", command, "/usr/bin/java"), Relevance.DEV)

    def test_gradle_wrapper_script_is_dev(self) -> None:
        """The wrapper below a real service is not the daemon it talks to."""
        command = ("/bin/sh", "/home/dev/projects/billing/gradlew", "bootRun", "--continuous")
        self.assertEqual(classify_and_rank("sh", command, "/bin/sh"), Relevance.DEV)

    def test_gradle_library_on_a_classpath_is_not_a_marker(self) -> None:
        """An app that embeds Gradle's own jars is still an app."""
        classpath = (
            f"{BOOT_CLASSPATH}"
            ":/home/dev/.gradle/caches/modules-2/files-2.1/org.gradle/gradle-tooling-api-8.14.3.jar"
            ":/home/dev/.gradle/caches/modules-2/files-2.1/org.gradle/gradle-core-8.14.3.jar"
        )
        command = ("/usr/lib/jvm/java-17/bin/java", "-cp", classpath, "com.acme.billing.Application")
        self.assertEqual(classify_and_rank("java", command), Relevance.DEV)


class UnrelatedRelevanceTests(unittest.TestCase):
    def test_other_users_stay_noise(self) -> None:
        self.assertEqual(
            relevance_of(
                ownership=Ownership.OTHER_USER,
                process_name="java",
                executable="/usr/bin/java",
                command=("java", "-cp", BOOT_CLASSPATH, "com.acme.billing.Application"),
                tech="spring",
                specific=True,
                has_project=True,
            ),
            Relevance.NOISE,
        )

    def test_container_runtime_still_wins_over_the_daemon_rule(self) -> None:
        self.assertEqual(classify_and_rank("dockerd", ("dockerd",), "/usr/bin/dockerd"), Relevance.CONTAINER)


if __name__ == "__main__":
    unittest.main()

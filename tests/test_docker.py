from __future__ import annotations

import json
import subprocess
import unittest
from collections.abc import Sequence
from unittest import mock

from cutting_board.scanner import docker
from cutting_board.scanner.docker import ContainerInfo, docker_available, list_containers


def ps_line(
    *,
    container_id: str = "9f1c2d3e4b5a6c7d8e9f0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3e4f5",
    names: str = "shop-db-1",
    image: str = "postgres:16",
    state: str = "running",
    status: str = "Up 3 hours",
    ports: str = "",
    labels: str = "",
    created_at: str = "2026-08-20 09:12:33 +0900 KST",
) -> str:
    """One line of `docker ps --format '{{json .}}'` output."""
    return json.dumps(
        {
            "ID": container_id,
            "Names": names,
            "Image": image,
            "State": state,
            "Status": status,
            "Ports": ports,
            "Labels": labels,
            "CreatedAt": created_at,
            "Command": '"docker-entrypoint.s…"',
        }
    )


class FakeRunner:
    """Stands in for the docker CLI: replays canned output or raises."""

    def __init__(
        self,
        *,
        stdout: str = "",
        stderr: str = "",
        returncode: int = 0,
        error: BaseException | None = None,
    ) -> None:
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode
        self.error = error
        self.calls: list[tuple[tuple[str, ...], float]] = []

    def __call__(self, command: Sequence[str], timeout: float) -> subprocess.CompletedProcess[str]:
        self.calls.append((tuple(command), timeout))
        if self.error is not None:
            raise self.error
        return subprocess.CompletedProcess(list(command), self.returncode, self.stdout, self.stderr)


class DockerListingTests(unittest.TestCase):
    def setUp(self) -> None:
        # The availability cache lives on the module, so each test starts from
        # a machine we know nothing about yet.
        docker._availability_cache = None

    def test_two_containers_are_parsed_with_their_compose_project(self) -> None:
        runner = FakeRunner(
            stdout="\n".join(
                (
                    ps_line(
                        names="shop-db-1",
                        image="postgres:16",
                        ports="0.0.0.0:5432->5432/tcp",
                        labels="com.docker.compose.project=shop,com.docker.compose.service=db",
                    ),
                    ps_line(
                        container_id="aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899",
                        names="shop-redis-1",
                        image="redis:7",
                        state="exited",
                        status="Exited (0) 2 minutes ago",
                        labels="com.docker.compose.project=shop,com.docker.compose.service=cache",
                    ),
                )
            )
        )

        listing = list_containers(runner=runner)

        self.assertTrue(listing.available)
        self.assertIsNone(listing.message)
        self.assertEqual(len(listing.containers), 2)

        database, cache = listing.containers
        self.assertEqual(database.id, "9f1c2d3e4b5a")
        self.assertEqual(database.name, "shop-db-1")
        self.assertEqual(database.image, "postgres:16")
        self.assertEqual(database.state, "running")
        self.assertEqual(database.status, "Up 3 hours")
        self.assertEqual(database.ports, (5432,))
        self.assertEqual(database.created_at, "2026-08-20 09:12:33 +0900 KST")
        self.assertEqual(database.compose_project, "shop")
        self.assertEqual(database.compose_service, "db")
        self.assertTrue(database.running)

        self.assertEqual(cache.state, "exited")
        self.assertFalse(cache.running)
        self.assertEqual(cache.compose_project, "shop")
        self.assertEqual({item.compose_project for item in listing.containers}, {"shop"})

    def test_dual_stack_publish_collapses_to_one_host_port(self) -> None:
        runner = FakeRunner(stdout=ps_line(ports="0.0.0.0:5432->5432/tcp, :::5432->5432/tcp"))

        listing = list_containers(runner=runner)

        self.assertEqual(listing.containers[0].ports, (5432,))

    def test_bracketed_ipv6_and_several_ports_are_sorted(self) -> None:
        runner = FakeRunner(
            stdout=ps_line(ports="[::]:8080->80/tcp, 0.0.0.0:8080->80/tcp, 127.0.0.1:3000->3000/tcp")
        )

        listing = list_containers(runner=runner)

        self.assertEqual(listing.containers[0].ports, (3000, 8080))

    def test_container_without_published_ports(self) -> None:
        runner = FakeRunner(stdout=ps_line(ports=""))

        listing = list_containers(runner=runner)

        self.assertEqual(listing.containers[0].ports, ())

    def test_exposed_but_unpublished_port_is_not_reachable(self) -> None:
        runner = FakeRunner(stdout=ps_line(ports="8080/tcp"))

        listing = list_containers(runner=runner)

        self.assertEqual(listing.containers[0].ports, ())

    def test_published_range_is_expanded(self) -> None:
        runner = FakeRunner(stdout=ps_line(ports="0.0.0.0:8000-8002->8000-8002/tcp"))

        listing = list_containers(runner=runner)

        self.assertEqual(listing.containers[0].ports, (8000, 8001, 8002))

    def test_absurd_range_does_not_flood_the_listing(self) -> None:
        runner = FakeRunner(stdout=ps_line(ports="0.0.0.0:1-65535->1-65535/tcp"))

        listing = list_containers(runner=runner)

        self.assertEqual(len(listing.containers[0].ports), 64)

    def test_container_without_compose_labels(self) -> None:
        runner = FakeRunner(stdout=ps_line(names="scratch", labels="maintainer=someone"))

        container = list_containers(runner=runner).containers[0]

        self.assertIsNone(container.compose_project)
        self.assertIsNone(container.compose_service)

    def test_state_is_recovered_from_status_when_the_cli_omits_it(self) -> None:
        runner = FakeRunner(
            stdout="\n".join(
                (
                    ps_line(state="", status="Up 12 seconds"),
                    ps_line(container_id="b" * 64, names="old", state="", status="Exited (137) 1 day ago"),
                    ps_line(container_id="c" * 64, names="held", state="", status="Up 2 hours (Paused)"),
                )
            )
        )

        states = [item.state for item in list_containers(runner=runner).containers]

        self.assertEqual(states, ["running", "exited", "paused"])

    def test_malformed_line_does_not_lose_the_good_ones(self) -> None:
        runner = FakeRunner(
            stdout="\n".join(
                (
                    ps_line(names="good-one"),
                    "{not json at all",
                    "",
                    '"a bare string"',
                    ps_line(container_id="d" * 64, names="good-two"),
                )
            )
        )

        listing = list_containers(runner=runner)

        self.assertTrue(listing.available)
        self.assertEqual([item.name for item in listing.containers], ["good-one", "good-two"])

    def test_empty_output_reports_no_containers_but_stays_available(self) -> None:
        listing = list_containers(runner=FakeRunner(stdout="\n"))

        self.assertTrue(listing.available)
        self.assertEqual(listing.containers, ())
        self.assertEqual(listing.message, "컨테이너가 없습니다.")

    def test_missing_docker_binary(self) -> None:
        runner = FakeRunner(error=FileNotFoundError(2, "No such file or directory", "docker"))

        listing = list_containers(runner=runner)

        self.assertFalse(listing.available)
        self.assertEqual(listing.containers, ())
        self.assertEqual(listing.message, "Docker CLI를 찾을 수 없습니다.")

    def test_daemon_not_running_keeps_the_stderr_hint(self) -> None:
        stderr = (
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. "
            "Is the docker daemon running?\n"
        )
        runner = FakeRunner(returncode=1, stderr=stderr)

        listing = list_containers(runner=runner)

        self.assertFalse(listing.available)
        assert listing.message is not None
        self.assertIn("Docker 데몬에 연결할 수 없습니다.", listing.message)
        self.assertIn("Cannot connect to the Docker daemon", listing.message)

    def test_permission_denied_is_reported_as_a_permission_problem(self) -> None:
        stderr = "Got permission denied while trying to connect to the Docker daemon socket\n"
        runner = FakeRunner(returncode=1, stderr=stderr)

        listing = list_containers(runner=runner)

        self.assertFalse(listing.available)
        assert listing.message is not None
        self.assertIn("Docker 소켓에 접근할 권한이 없습니다.", listing.message)
        self.assertIn("permission denied", listing.message)

    def test_unrecognised_failure_falls_back_to_the_generic_message(self) -> None:
        runner = FakeRunner(returncode=125, stderr="unknown flag: --all\nSee 'docker ps --help'.\n")

        listing = list_containers(runner=runner)

        self.assertFalse(listing.available)
        assert listing.message is not None
        self.assertIn("Docker 명령이 실패했습니다.", listing.message)
        # Only the first line of stderr, so the tab shows one readable sentence.
        self.assertIn("unknown flag: --all", listing.message)
        self.assertNotIn("--help", listing.message)

    def test_failure_without_stderr_still_has_a_message(self) -> None:
        listing = list_containers(runner=FakeRunner(returncode=1, stderr=""))

        self.assertFalse(listing.available)
        self.assertEqual(listing.message, "Docker 명령이 실패했습니다.")

    def test_long_stderr_hint_is_trimmed(self) -> None:
        runner = FakeRunner(returncode=1, stderr="x" * 400)

        listing = list_containers(runner=runner)

        assert listing.message is not None
        self.assertLess(len(listing.message), 220)
        self.assertIn("…", listing.message)

    def test_timeout_never_escapes(self) -> None:
        runner = FakeRunner(error=subprocess.TimeoutExpired(cmd="docker ps", timeout=2.0))

        listing = list_containers(runner=runner)

        self.assertFalse(listing.available)
        self.assertEqual(listing.containers, ())
        self.assertEqual(listing.message, "Docker가 제한 시간 안에 응답하지 않았습니다.")

    def test_os_error_never_escapes(self) -> None:
        runner = FakeRunner(error=PermissionError(13, "Permission denied"))

        listing = list_containers(runner=runner)

        self.assertFalse(listing.available)
        assert listing.message is not None
        self.assertIn("Docker CLI를 실행할 수 없습니다.", listing.message)

    def test_timeout_is_handed_to_the_runner(self) -> None:
        runner = FakeRunner(stdout=ps_line())

        list_containers(0.5, runner=runner)

        self.assertEqual(len(runner.calls), 1)
        command, timeout = runner.calls[0]
        self.assertEqual(timeout, 0.5)
        self.assertEqual(command[:2], ("docker", "ps"))
        self.assertIn("--all", command)
        self.assertIn("{{json .}}", command)


class DockerAvailabilityTests(unittest.TestCase):
    def setUp(self) -> None:
        docker._availability_cache = None

    def test_availability_reuses_the_last_listing(self) -> None:
        runner = FakeRunner(stdout=ps_line())

        self.assertTrue(list_containers(runner=runner).available)
        self.assertTrue(docker_available())
        # The cached answer must not cost a second process.
        self.assertEqual(len(runner.calls), 1)

    def test_availability_remembers_a_machine_without_docker(self) -> None:
        runner = FakeRunner(error=FileNotFoundError(2, "No such file or directory", "docker"))

        list_containers(runner=runner)

        self.assertFalse(docker_available())
        self.assertEqual(len(runner.calls), 1)

    def test_availability_asks_docker_when_nothing_has_asked_yet(self) -> None:
        with mock.patch.object(docker, "_run_subprocess") as run:
            run.return_value = subprocess.CompletedProcess(list(docker.PS_COMMAND), 0, ps_line(), "")
            self.assertTrue(docker_available())
        self.assertEqual(run.call_count, 1)


class DefaultRunnerTests(unittest.TestCase):
    """The real subprocess call, without needing docker to be installed."""

    def setUp(self) -> None:
        docker._availability_cache = None

    def test_subprocess_is_called_with_a_timeout_and_without_check(self) -> None:
        with mock.patch("subprocess.run") as run:
            run.return_value = subprocess.CompletedProcess(list(docker.PS_COMMAND), 0, ps_line(), "")
            listing = list_containers(1.5)

        self.assertEqual(len(listing.containers), 1)
        run.assert_called_once()
        args, kwargs = run.call_args
        self.assertEqual(args[0], list(docker.PS_COMMAND))
        self.assertEqual(kwargs["timeout"], 1.5)
        self.assertFalse(kwargs["check"])
        self.assertTrue(kwargs["capture_output"])

    def test_a_missing_binary_from_the_real_runner_is_handled(self) -> None:
        with mock.patch("subprocess.run", side_effect=FileNotFoundError()):
            listing = list_containers()

        self.assertFalse(listing.available)
        self.assertEqual(listing.message, "Docker CLI를 찾을 수 없습니다.")


class ContainerInfoTests(unittest.TestCase):
    def test_container_info_is_frozen(self) -> None:
        container = ContainerInfo(
            id="abc123",
            name="db",
            image="postgres:16",
            state="running",
            status="Up 3 hours",
            ports=(5432,),
            created_at="2026-08-20 09:12:33 +0900 KST",
        )

        with self.assertRaises(Exception):
            container.name = "other"  # type: ignore[misc]


if __name__ == "__main__":
    unittest.main()

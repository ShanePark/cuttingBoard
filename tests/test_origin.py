from __future__ import annotations

import os
import unittest

from cutting_board.scanner import origin as origin_module
from cutting_board.scanner.origin import (
    Origin,
    OriginDetector,
    OriginKind,
    OriginSignal,
    ProcessEntry,
    clear_cache,
    detect_origin,
)


def chain(*nodes: tuple[int, str, tuple[str, ...]]) -> dict[int, ProcessEntry]:
    """Build a child-to-parent process chain; the last node is the root."""
    entries: dict[int, ProcessEntry] = {}
    for index, (pid, name, command) in enumerate(nodes):
        parent = nodes[index + 1][0] if index + 1 < len(nodes) else 0
        entries[pid] = ProcessEntry(
            name=name,
            ppid=parent,
            start_ticks=1000 + index,
            command=command,
        )
    return entries


class FakeProcesses:
    """A readable process table that records which PIDs were inspected."""

    def __init__(self, entries: dict[int, ProcessEntry]) -> None:
        self.entries = entries
        self.reads: list[int] = []

    def __call__(self, pid: int) -> ProcessEntry | None:
        self.reads.append(pid)
        return self.entries.get(pid)


class FakeEnvirons:
    def __init__(self, environs: dict[int, dict[str, str]] | None = None) -> None:
        self.environs = environs or {}
        self.reads: list[int] = []

    def __call__(self, pid: int) -> dict[str, str] | None:
        self.reads.append(pid)
        return self.environs.get(pid)


def detector(
    entries: dict[int, ProcessEntry],
    environs: dict[int, dict[str, str]] | None = None,
) -> tuple[OriginDetector, FakeProcesses, FakeEnvirons]:
    processes = FakeProcesses(entries)
    environments = FakeEnvirons(environs)
    return OriginDetector(processes, environments), processes, environments


SNAPSHOT_SHELL = (
    "/usr/bin/zsh",
    "-c",
    "source /home/dev/.claude/shell-snapshots/snapshot-zsh-1787208668533-c05t48.sh",
)


class AncestryTests(unittest.TestCase):
    def test_claude_code_ancestry(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "bash", ("/bin/bash",)),
                (902, "claude", ("/home/dev/.config/Claude/claude-code/2.1.234/claude", "--verbose")),
                (903, "gnome-shell", ("/usr/bin/gnome-shell",)),
                (904, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        result = probe.detect(900)
        self.assertEqual(result.id, "claude")
        self.assertEqual(result.label, "Claude")
        self.assertIs(result.kind, OriginKind.AGENT)
        self.assertIs(result.signal, OriginSignal.ANCESTRY)

    def test_shell_snapshot_argv_alone_identifies_claude_code(self) -> None:
        """The snapshot path outlives the claude process that wrote it."""
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "zsh", SNAPSHOT_SHELL),
                (902, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        result = probe.detect(900)
        self.assertEqual(result.id, "claude")
        self.assertIs(result.kind, OriginKind.AGENT)

    def test_codex_ancestry(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "zsh", ("/usr/bin/zsh", "-lc", "npm run dev")),
                (902, "codex", ("/usr/lib/chatgpt/resources/codex", "app-server")),
                (903, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        result = probe.detect(900)
        self.assertEqual(result.id, "codex")
        self.assertIs(result.kind, OriginKind.AGENT)

    def test_vscode_ancestry_is_an_ide(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "bash", ("/bin/bash",)),
                (902, "code", ("/usr/share/code/code", ".")),
                (903, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        result = probe.detect(900)
        self.assertIs(result.kind, OriginKind.IDE)
        self.assertEqual(result.id, "vscode")
        self.assertEqual(result.label, "VS Code")

    def test_jetbrains_is_recognised_through_its_java_main_class(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "java", ("/opt/jdk/bin/java", "-cp", "/opt/idea/app.jar", "com.intellij.idea.Main")),
                (902, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        result = probe.detect(900)
        self.assertIs(result.kind, OriginKind.IDE)
        self.assertEqual(result.id, "jetbrains")

    def test_terminal_ancestry(self) -> None:
        """comm is truncated to 15 characters, so the short form must match."""
        probe, _, _ = detector(
            chain(
                (900, "python3", ("python3", "-m", "http.server")),
                (901, "zsh", ("/usr/bin/zsh",)),
                (902, "gnome-terminal-", ()),
                (903, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        result = probe.detect(900)
        self.assertIs(result.kind, OriginKind.TERMINAL)
        self.assertEqual(result.id, "terminal")
        self.assertEqual(result.label, "터미널")

    def test_systemd_only_ancestry_is_system(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "postgres", ("/usr/lib/postgresql/16/bin/postgres",)),
                (901, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        result = probe.detect(900)
        self.assertIs(result.kind, OriginKind.SYSTEM)
        self.assertEqual(result.id, "system")

    def test_reaching_the_root_without_a_match_is_system(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "tini", ("/sbin/tini", "--")),
            )
        )
        self.assertIs(probe.detect(900).kind, OriginKind.SYSTEM)

    def test_nearest_launcher_wins_over_the_one_above_it(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "zsh", SNAPSHOT_SHELL),
                (902, "claude", ("/home/dev/.local/bin/claude",)),
                (903, "ghostty", ("/usr/bin/ghostty",)),
                (904, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        self.assertEqual(probe.detect(900).id, "claude")

    def test_ancestry_deeper_than_the_limit_is_unknown(self) -> None:
        nodes = [(900 + step, "node", ("node", "worker.js")) for step in range(20)]
        nodes.append((999, "systemd", ("/usr/lib/systemd/systemd", "--user")))
        probe, processes, _ = detector(chain(*nodes))
        result = probe.detect(900)
        self.assertIs(result.kind, OriginKind.UNKNOWN)
        self.assertLessEqual(len(processes.reads), origin_module._ANCESTRY_LIMIT)

    def test_unknown_origin_has_an_empty_slug_and_label(self) -> None:
        probe, _, _ = detector({})
        result = probe.detect(900)
        self.assertIs(result.kind, OriginKind.UNKNOWN)
        self.assertEqual(result.id, "")
        self.assertEqual(result.label, "")
        self.assertFalse(result.known)


class EnvironmentTests(unittest.TestCase):
    def test_environ_marker_without_matching_ancestry(self) -> None:
        """A detached dev server keeps the agent's environment, not its parent."""
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            ),
            {900: {"CLAUDECODE": "1", "PATH": "/usr/bin"}},
        )
        result = probe.detect(900)
        self.assertEqual(result.id, "claude")
        self.assertIs(result.kind, OriginKind.AGENT)
        self.assertIs(result.signal, OriginSignal.ENVIRONMENT)

    def test_codex_environ_marker(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            ),
            {900: {"CODEX_SESSION_ID": "01a01cb1"}},
        )
        self.assertEqual(probe.detect(900).id, "codex")

    def test_chatgpt_desktop_variables_are_not_a_codex_session(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            ),
            {900: {"BROWSER_USE_CODEX_APP_VERSION": "26.814.41957"}},
        )
        self.assertIs(probe.detect(900).kind, OriginKind.SYSTEM)

    def test_term_program_value_identifies_a_terminal(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            ),
            {900: {"TERM_PROGRAM": "ghostty"}},
        )
        result = probe.detect(900)
        self.assertIs(result.kind, OriginKind.TERMINAL)
        self.assertIs(result.signal, OriginSignal.ENVIRONMENT)

    def test_jetbrains_terminal_emulator_marker(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            ),
            {900: {"TERMINAL_EMULATOR": "JetBrains-JediTerm"}},
        )
        self.assertEqual(probe.detect(900).id, "jetbrains")

    def test_agent_environment_outranks_a_terminal_ancestry(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "ghostty", ("/usr/bin/ghostty",)),
            ),
            {900: {"CLAUDECODE": "1", "TERM_PROGRAM": "ghostty"}},
        )
        self.assertEqual(probe.detect(900).id, "claude")

    def test_ancestry_wins_when_the_environment_is_no_more_specific(self) -> None:
        probe, _, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "code", ("/usr/share/code/code",)),
            ),
            {900: {"TERM_PROGRAM": "ghostty"}},
        )
        result = probe.detect(900)
        self.assertEqual(result.id, "vscode")
        self.assertIs(result.signal, OriginSignal.ANCESTRY)

    def test_agent_ancestry_skips_the_environ_read(self) -> None:
        probe, _, environments = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "claude", ("/home/dev/.local/bin/claude",)),
            ),
            {900: {"TERM_PROGRAM": "ghostty"}},
        )
        self.assertEqual(probe.detect(900).id, "claude")
        self.assertEqual(environments.reads, [])


class FailureTests(unittest.TestCase):
    def test_process_vanishing_mid_walk_is_unknown(self) -> None:
        entries = chain(
            (900, "node", ("node", "server.js")),
            (901, "zsh", ("/usr/bin/zsh",)),
            (902, "claude", ("/home/dev/.local/bin/claude",)),
        )
        del entries[902]  # reaped between the child read and the parent read
        probe, _, _ = detector(entries)
        result = probe.detect(900)
        self.assertIs(result.kind, OriginKind.UNKNOWN)

    def test_target_process_missing_is_unknown(self) -> None:
        probe, _, _ = detector({})
        self.assertIs(probe.detect(4242).kind, OriginKind.UNKNOWN)

    def test_reader_raising_is_contained(self) -> None:
        def explode(pid: int) -> ProcessEntry | None:
            raise OSError("gone")

        probe = OriginDetector(explode, FakeEnvirons())
        self.assertIs(probe.detect(900).kind, OriginKind.UNKNOWN)

    def test_permission_denied_environ_still_yields_the_ancestry(self) -> None:
        def denied(pid: int) -> dict[str, str] | None:
            raise PermissionError(pid)

        probe = OriginDetector(
            FakeProcesses(
                chain(
                    (900, "node", ("node", "server.js")),
                    (901, "kitty", ("/usr/bin/kitty",)),
                )
            ),
            denied,
        )
        self.assertIs(probe.detect(900).kind, OriginKind.TERMINAL)

    def test_non_positive_pid_is_unknown(self) -> None:
        probe, processes, _ = detector({})
        self.assertIs(probe.detect(0).kind, OriginKind.UNKNOWN)
        self.assertEqual(processes.reads, [])


class CacheTests(unittest.TestCase):
    def test_repeat_lookup_does_not_walk_again(self) -> None:
        probe, processes, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "zsh", SNAPSHOT_SHELL),
                (902, "systemd", ("/usr/lib/systemd/systemd", "--user")),
            )
        )
        first = probe.detect(900, create_time=1_700_000_000.5)
        walked = len(processes.reads)
        self.assertGreater(walked, 1)

        second = probe.detect(900, create_time=1_700_000_000.5)
        self.assertEqual(second, first)
        self.assertEqual(len(processes.reads), walked)

    def test_a_recycled_pid_misses_the_cache(self) -> None:
        probe, processes, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "ghostty", ("/usr/bin/ghostty",)),
            )
        )
        probe.detect(900, create_time=1_700_000_000.5)
        walked = len(processes.reads)
        probe.detect(900, create_time=1_700_009_999.5)
        self.assertGreater(len(processes.reads), walked)

    def test_siblings_reuse_the_cached_answer_for_a_shared_parent(self) -> None:
        entries = chain(
            (900, "node", ("node", "one.js")),
            (901, "zsh", SNAPSHOT_SHELL),
            (902, "gnome-shell", ("/usr/bin/gnome-shell",)),
            (903, "systemd", ("/usr/lib/systemd/systemd", "--user")),
        )
        entries[910] = ProcessEntry(name="node", ppid=901, start_ticks=1010, command=("node", "two.js"))
        probe, processes, _ = detector(entries)

        self.assertEqual(probe.detect(900).id, "claude")
        processes.reads.clear()
        self.assertEqual(probe.detect(910).id, "claude")
        # Only the sibling itself and the shared shell, whose answer is cached.
        self.assertEqual(processes.reads, [910, 901])

    def test_a_fruitless_chain_is_not_rewalked_for_every_sibling(self) -> None:
        entries = chain(
            (900, "node", ("node", "one.js")),
            (901, "sh", ("/bin/sh",)),
            (902, "supervisor", ("/usr/bin/supervisor",)),
        )
        del entries[902]
        entries[910] = ProcessEntry(name="node", ppid=901, start_ticks=1010, command=("node", "two.js"))
        probe, processes, _ = detector(entries)

        self.assertIs(probe.detect(900).kind, OriginKind.UNKNOWN)
        processes.reads.clear()
        self.assertIs(probe.detect(910).kind, OriginKind.UNKNOWN)
        self.assertEqual(processes.reads, [910, 901])

    def test_cache_is_bounded(self) -> None:
        entries = {
            pid: ProcessEntry(name="node", ppid=0, start_ticks=pid, command=("node", "server.js"))
            for pid in range(1, 2000)
        }
        probe, _, _ = detector(entries)
        for pid in entries:
            probe.detect(pid, create_time=float(pid))
        self.assertLessEqual(len(probe._results), origin_module._CACHE_LIMIT)
        self.assertLessEqual(len(probe._ancestry), origin_module._CACHE_LIMIT)

    def test_clear_cache_forces_another_walk(self) -> None:
        probe, processes, _ = detector(
            chain(
                (900, "node", ("node", "server.js")),
                (901, "ghostty", ("/usr/bin/ghostty",)),
            )
        )
        probe.detect(900, create_time=1.0)
        walked = len(processes.reads)
        probe.clear_cache()
        probe.detect(900, create_time=1.0)
        self.assertGreater(len(processes.reads), walked)


class ModuleApiTests(unittest.TestCase):
    def tearDown(self) -> None:
        clear_cache()

    def test_detect_origin_reads_this_machine_without_raising(self) -> None:
        result = detect_origin(os.getpid())
        self.assertIsInstance(result, Origin)
        self.assertIn(result.kind, set(OriginKind))

    def test_detect_origin_on_a_dead_pid_is_unknown(self) -> None:
        # 4194304 is above the default pid_max, so it can never be assigned.
        self.assertIs(detect_origin(4_194_304).kind, OriginKind.UNKNOWN)

    def test_detect_origin_uses_the_shared_cache(self) -> None:
        pid = os.getpid()
        first = detect_origin(pid, create_time=1.0)
        self.assertEqual(detect_origin(pid, create_time=1.0), first)
        clear_cache()
        self.assertEqual(detect_origin(pid, create_time=1.0), first)

    def test_readers_tolerate_an_unreadable_process(self) -> None:
        self.assertIsNone(origin_module.read_process_entry(4_194_304))
        self.assertIsNone(origin_module.read_process_environ(4_194_304))

    def test_reading_this_process_returns_a_usable_entry(self) -> None:
        entry = origin_module.read_process_entry(os.getpid())
        self.assertIsNotNone(entry)
        assert entry is not None
        self.assertEqual(entry.ppid, os.getppid())
        self.assertGreater(entry.start_ticks, 0)
        self.assertTrue(entry.command)

    def test_reading_this_environment_returns_the_real_variables(self) -> None:
        environ = origin_module.read_process_environ(os.getpid())
        self.assertIsNotNone(environ)
        assert environ is not None
        self.assertIn("PATH", environ)
        self.assertTrue(all(name and "\0" not in name for name in environ))


if __name__ == "__main__":
    unittest.main()

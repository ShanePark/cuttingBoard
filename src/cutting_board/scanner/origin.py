"""Attribute a listening process to whatever launched it.

Two independent signals answer the question, and they fail in different ways,
so both are read:

* Process ancestry names the launcher directly, but it evaporates the moment
  the launcher exits and the service is reparented to init.
* The inherited environment survives ``setsid``, daemonisation and the death
  of the launcher, which is exactly the case ancestry cannot see.

Everything is read straight from ``/proc``. psutil returns the same data and
is already a dependency, but collecting name and ppid for 400 processes
measured 40.6 ms through psutil against 6.1 ms through ``/proc`` here, and
this runs for every listener on a sweep that has a two second budget and
currently finishes in 86-180 ms.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass, replace
from enum import Enum
from pathlib import Path


class OriginKind(str, Enum):
    """Who put a listening process on this machine."""

    AGENT = "agent"
    """An AI coding agent: Claude Code, Codex, Cursor and friends."""

    IDE = "ide"
    """An editor or IDE, including its integrated terminal."""

    TERMINAL = "terminal"
    """A terminal emulator or multiplexer the user typed into."""

    SYSTEM = "system"
    """init, a systemd unit, or anything else that supervises the machine."""

    UNKNOWN = "unknown"
    """No evidence survived. The badge is simply not drawn."""


class OriginSignal(str, Enum):
    """Which evidence produced an origin, for tooltips and diagnosis."""

    ANCESTRY = "ancestry"
    ENVIRONMENT = "environment"
    NONE = "none"


@dataclass(frozen=True, slots=True)
class Origin:
    """The launcher behind a service, ready to render as a badge.

    ``id`` is a stable slug that badge artwork and tests key off; ``label``
    is the string the tile shows, kept short because it renders at about 9px
    on a 176px wide tile. Korean is used only where no product name exists.
    """

    kind: OriginKind
    id: str
    label: str
    signal: OriginSignal = OriginSignal.NONE

    @property
    def known(self) -> bool:
        return self.kind is not OriginKind.UNKNOWN


UNKNOWN_ORIGIN = Origin(OriginKind.UNKNOWN, "", "")

# Every emittable origin. Ancestry and environment rules share these so a
# slug can never drift between the two tables.
CLAUDE = Origin(OriginKind.AGENT, "claude", "Claude")
CODEX = Origin(OriginKind.AGENT, "codex", "Codex")
CURSOR = Origin(OriginKind.AGENT, "cursor", "Cursor")
WINDSURF = Origin(OriginKind.AGENT, "windsurf", "Windsurf")
COPILOT = Origin(OriginKind.AGENT, "copilot", "Copilot")
AIDER = Origin(OriginKind.AGENT, "aider", "Aider")
GEMINI = Origin(OriginKind.AGENT, "gemini", "Gemini")
VSCODE = Origin(OriginKind.IDE, "vscode", "VS Code")
JETBRAINS = Origin(OriginKind.IDE, "jetbrains", "JetBrains")
TERMINAL = Origin(OriginKind.TERMINAL, "terminal", "터미널")
SYSTEM = Origin(OriginKind.SYSTEM, "system", "시스템")

# Cursor and Windsurf are editors, but they are here because a service they
# start was started by their agent, not by a human pressing Run. The badge is
# answering "did a model do this?", so they sit with the agents.
_RANK = {
    OriginKind.AGENT: 0,
    OriginKind.IDE: 1,
    OriginKind.TERMINAL: 2,
    OriginKind.SYSTEM: 3,
    OriginKind.UNKNOWN: 4,
}

_ANCESTRY_LIMIT = 12
_CACHE_LIMIT = 512

# stat needs only its first 22 fields; cmdline and environ are capped so a
# process with a pathological argv or environment cannot stall a sweep.
_STAT_BYTES = 512
_CMDLINE_BYTES = 8192
_ENVIRON_BYTES = 65536


@dataclass(frozen=True, slots=True)
class ProcessEntry:
    """The parts of ``/proc/<pid>`` that can identify a launcher."""

    name: str
    ppid: int
    start_ticks: int
    command: tuple[str, ...] = ()


def _comm_names(*names: str) -> frozenset[str]:
    """Process names plus the forms ``/proc/<pid>/stat`` actually reports.

    The kernel truncates a userspace comm to 15 characters, so
    ``gnome-terminal-server`` arrives as ``gnome-terminal-``. argv usually
    carries the full name, but a zombie or a process with a cleared argv has
    nothing but comm, so both spellings are matched.
    """
    return frozenset(names) | {name[:15] for name in names}


class _Launcher:
    """A program that, seen in an ancestry, explains the service below it.

    ``fingerprints`` are matched against the whole argv, lowercased. They
    exist for launchers that leave a trace in a *shell's* arguments rather
    than running as a recognisable process of their own, so they must be
    path fragments distinctive enough that a checkout directory cannot
    impersonate them.
    """

    __slots__ = ("names", "fingerprints", "origin")

    def __init__(
        self,
        names: frozenset[str],
        fingerprints: tuple[str, ...],
        origin: Origin,
    ) -> None:
        self.names = names
        self.fingerprints = fingerprints
        self.origin = origin


# Order is precedence within a single ancestor: agents, then IDEs, then
# terminals, then the supervisor. Across ancestors the nearest match wins,
# which already gives the right answer, because a launcher started by another
# launcher is the closer parent.
_LAUNCHERS: tuple[_Launcher, ...] = (
    _Launcher(
        _comm_names("claude", "claude-code"),
        # Claude Code sources a snapshot of the user's shell into every
        # command it runs. That path stays in the shell's argv after the
        # claude process itself is gone, which is the only evidence left
        # once a dev server outlives the session that started it.
        (".claude/shell-snapshots/", ".claude/local/claude"),
        CLAUDE,
    ),
    _Launcher(_comm_names("codex", "codex-cli"), ("/.codex/",), CODEX),
    _Launcher(_comm_names("cursor", "cursor-agent"), ("/.cursor-server/",), CURSOR),
    _Launcher(
        _comm_names("windsurf", "windsurf-next"),
        ("/.windsurf-server/", "/.codeium/windsurf/"),
        WINDSURF,
    ),
    # `gh copilot ...` runs as gh, so the subcommand has to count as a name.
    _Launcher(_comm_names("copilot", "copilot-cli"), ("/.copilot/",), COPILOT),
    _Launcher(_comm_names("aider"), (), AIDER),
    _Launcher(_comm_names("gemini", "gemini-cli"), ("/.gemini/",), GEMINI),
    _Launcher(
        _comm_names("code", "code-insiders", "code-oss", "codium", "vscodium"),
        ("/.vscode-server/", "extensionhostprocess.js"),
        VSCODE,
    ),
    _Launcher(
        _comm_names(
            "idea",
            "idea.sh",
            "pycharm",
            "pycharm.sh",
            "webstorm",
            "goland",
            "clion",
            "rider",
            "rubymine",
            "phpstorm",
            "datagrip",
            "rustrover",
            "studio",
            "android-studio",
            "jetbrains-toolbox",
            "jetbrainsd",
        ),
        # JetBrains IDEs run as a plain `java`, so the main class is the
        # only thing that names them.
        ("com.intellij.idea.main", "/jetbrains/toolbox/", "idea_rt.jar"),
        JETBRAINS,
    ),
    _Launcher(
        _comm_names(
            "gnome-terminal-server",
            "konsole",
            "kitty",
            "alacritty",
            "wezterm",
            "wezterm-gui",
            "tmux",
            "tmux: server",
            "screen",
            "xterm",
            "terminator",
            "ghostty",
            "xfce4-terminal",
            "mate-terminal",
            "deepin-terminal",
            "lxterminal",
            "qterminal",
            "tilix",
            "urxvt",
            "rxvt",
            "foot",
            "sakura",
            "guake",
            "yakuake",
            "contour",
            "hyper",
            "warp-terminal",
        ),
        (),
        TERMINAL,
    ),
    _Launcher(_comm_names("systemd", "init"), (), SYSTEM),
)


class _EnvironmentRule:
    """Evidence that survives in an inherited environment.

    ``keys`` are variables whose mere presence is proof; their values are
    never compared. ``values`` are variables that several launchers set, so
    only a specific value identifies one.
    """

    __slots__ = ("keys", "values", "origin")

    def __init__(
        self,
        keys: frozenset[str],
        values: tuple[tuple[str, frozenset[str]], ...],
        origin: Origin,
    ) -> None:
        self.keys = keys
        self.values = values
        self.origin = origin


# Ordered by how specific the answer is, because a Claude Code session run
# inside a VS Code terminal inside Ghostty carries all three sets of markers
# and the innermost one is the true launcher.
#
# ANTHROPIC_* and OPENAI_* are deliberately absent: a user can export those
# globally, which would badge every service on the board.
_ENVIRONMENT_RULES: tuple[_EnvironmentRule, ...] = (
    # All five confirmed on this machine in a Claude Code spawned shell.
    _EnvironmentRule(
        frozenset(
            {
                "CLAUDECODE",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_CODE_EXECPATH",
                "CLAUDE_PROJECT_DIR",
            }
        ),
        (),
        CLAUDE,
    ),
    # CODEX_HOME, CODEX_CLI_PATH, CODEX_SESSION_ID, CODEX_THREAD_ID and
    # CODEX_SANDBOX_NETWORK_DISABLED were all observed here. The names are
    # matched exactly so the ChatGPT desktop app's BROWSER_USE_CODEX_APP_*
    # variables cannot be mistaken for a Codex session.
    _EnvironmentRule(
        frozenset(
            {
                "CODEX_HOME",
                "CODEX_CLI_PATH",
                "CODEX_SESSION_ID",
                "CODEX_THREAD_ID",
                "CODEX_SANDBOX",
                "CODEX_SANDBOX_NETWORK_DISABLED",
            }
        ),
        (),
        CODEX,
    ),
    # Not observed here; Cursor is not installed on this machine.
    _EnvironmentRule(frozenset({"CURSOR_TRACE_ID", "CURSOR_AGENT"}), (), CURSOR),
    # VSCODE_PID, VSCODE_IPC_HOOK, VSCODE_CWD and VSCODE_NLS_CONFIG were
    # observed. VSCODE_INJECTION, VSCODE_GIT_IPC_HANDLE and
    # TERM_PROGRAM=vscode belong to the integrated terminal, which was not
    # open here, so they are carried on documentation alone.
    _EnvironmentRule(
        frozenset(
            {
                "VSCODE_PID",
                "VSCODE_IPC_HOOK",
                "VSCODE_CWD",
                "VSCODE_NLS_CONFIG",
                "VSCODE_INJECTION",
                "VSCODE_GIT_IPC_HANDLE",
            }
        ),
        (("TERM_PROGRAM", frozenset({"vscode"})),),
        VSCODE,
    ),
    # Not observed here; no JetBrains terminal was open.
    _EnvironmentRule(
        frozenset({"IDEA_INITIAL_DIRECTORY"}),
        (("TERMINAL_EMULATOR", frozenset({"jetbrains-jediterm"})),),
        JETBRAINS,
    ),
    # GHOSTTY_RESOURCES_DIR and TERM_PROGRAM=ghostty were observed. The other
    # emulators were not running, so their markers come from documentation.
    _EnvironmentRule(
        frozenset(
            {
                "GHOSTTY_RESOURCES_DIR",
                "KITTY_WINDOW_ID",
                "ALACRITTY_WINDOW_ID",
                "ALACRITTY_SOCKET",
                "WEZTERM_PANE",
                "WEZTERM_EXECUTABLE",
                "TMUX",
                "KONSOLE_VERSION",
                "GNOME_TERMINAL_SCREEN",
                "GNOME_TERMINAL_SERVICE",
                "TERMINATOR_UUID",
                "XTERM_VERSION",
                "VTE_VERSION",
            }
        ),
        (
            (
                "TERM_PROGRAM",
                frozenset(
                    {
                        "ghostty",
                        "kitty",
                        "alacritty",
                        "wezterm",
                        "tmux",
                        "konsole",
                        "gnome-terminal",
                        "warpterminal",
                        "hyper",
                        "contour",
                        "rio",
                    }
                ),
            ),
        ),
        TERMINAL,
    ),
)


def read_process_entry(pid: int) -> ProcessEntry | None:
    """Read name, parent and creation stamp for one process, or None."""
    try:
        with open(f"/proc/{pid}/stat", "rb") as handle:
            raw = handle.read(_STAT_BYTES)
        with open(f"/proc/{pid}/cmdline", "rb") as handle:
            argv = handle.read(_CMDLINE_BYTES)
    except OSError:
        return None

    try:
        # comm is parenthesised and may itself contain parentheses or spaces,
        # so the field split can only start after its closing bracket.
        close = raw.rindex(b")")
        name = raw[raw.index(b"(") + 1 : close].decode("utf-8", "replace")
        fields = raw[close + 2 :].split(b" ")
        # stat numbers its fields from one and comm occupies field two, so
        # field N sits at fields[N - 3]: ppid is field 4, starttime field 22.
        ppid = int(fields[1])
        start_ticks = int(fields[19])
    except (IndexError, ValueError):
        return None

    command = tuple(token.decode("utf-8", "replace") for token in argv.split(b"\0") if token)
    return ProcessEntry(name=name, ppid=ppid, start_ticks=start_ticks, command=command)


def read_process_environ(pid: int) -> Mapping[str, str] | None:
    """Read the environment a process inherited, or None when it is hidden.

    Only processes owned by this user are readable; everything else raises
    PermissionError, which is an OSError like a vanished process is.
    """
    try:
        with open(f"/proc/{pid}/environ", "rb") as handle:
            raw = handle.read(_ENVIRON_BYTES)
    except OSError:
        return None

    environ: dict[str, str] = {}
    for item in raw.split(b"\0"):
        name, separator, value = item.partition(b"=")
        if separator:
            environ[name.decode("utf-8", "replace")] = value.decode("utf-8", "replace")
    return environ


class OriginDetector:
    """Resolves the launcher behind a process, caching what it learns.

    The two readers are injected so tests can describe a process tree
    instead of having to create one.
    """

    __slots__ = ("_read_process", "_read_environ", "_results", "_ancestry")

    def __init__(
        self,
        read_process: Callable[[int], ProcessEntry | None] | None = None,
        read_environ: Callable[[int], Mapping[str, str] | None] | None = None,
    ) -> None:
        self._read_process = read_process or read_process_entry
        self._read_environ = read_environ or read_process_environ
        self._results: dict[tuple[int, int], Origin] = {}
        # Keyed by ancestor rather than by service: siblings started from one
        # shell share a chain, and re-walking it per listener is the whole
        # cost of this module.
        self._ancestry: dict[tuple[int, int], Origin] = {}

    def clear_cache(self) -> None:
        self._results.clear()
        self._ancestry.clear()

    def detect(self, pid: int, *, create_time: float | None = None) -> Origin:
        try:
            return self._detect(pid, create_time)
        except Exception:  # defensive boundary around OS inspection
            return UNKNOWN_ORIGIN

    def _detect(self, pid: int, create_time: float | None) -> Origin:
        if pid <= 0:
            return UNKNOWN_ORIGIN

        # create_time and /proc's starttime mark the same instant by two
        # different clocks, so either turns a recycled PID into a miss. The
        # caller's value is preferred because it answers without any read.
        key = (pid, int(create_time * 1000)) if create_time is not None else None
        if key is not None:
            hit = self._results.get(key)
            if hit is not None:
                return hit

        entry = self._read_process(pid)
        if entry is None:
            return UNKNOWN_ORIGIN
        if key is None:
            key = (pid, entry.start_ticks)
            hit = self._results.get(key)
            if hit is not None:
                return hit

        origin = self._resolve(pid, entry)
        _store(self._results, key, origin)
        return origin

    def _resolve(self, pid: int, entry: ProcessEntry) -> Origin:
        ancestry = self._walk(pid, entry)
        if ancestry is not None and ancestry.kind is OriginKind.AGENT:
            return ancestry

        environment = self._from_environment(pid)
        if environment is None:
            return ancestry or UNKNOWN_ORIGIN
        if ancestry is None:
            return environment
        # A detached dev server reparents to init, so ancestry says SYSTEM
        # while the inherited environment still names the agent that started
        # it. The more specific answer wins, ties going to ancestry.
        return environment if _RANK[environment.kind] < _RANK[ancestry.kind] else ancestry

    def _walk(self, pid: int, entry: ProcessEntry | None) -> Origin | None:
        pending: list[tuple[int, int]] = []
        found: Origin | None = None
        current = pid

        for _ in range(_ANCESTRY_LIMIT):
            if entry is None:
                entry = self._read_process(current)
            if entry is None:
                break

            key = (current, entry.start_ticks)
            cached = self._ancestry.get(key)
            if cached is not None:
                # UNKNOWN doubles as the "walked this, found nothing" note so
                # a fruitless chain is not re-walked for every sibling.
                found = None if cached.kind is OriginKind.UNKNOWN else cached
                break

            match = _match_launcher(entry)
            if match is not None:
                found = match
                break

            pending.append(key)
            parent = entry.ppid
            if parent <= 0 or parent == current:
                # The top of the tree with nothing recognised on the way: the
                # service belongs to whatever supervises the machine.
                found = replace(SYSTEM, signal=OriginSignal.ANCESTRY)
                break
            current = parent
            entry = None

        resolved = found or UNKNOWN_ORIGIN
        for key in pending:
            _store(self._ancestry, key, resolved)
        return found

    def _from_environment(self, pid: int) -> Origin | None:
        try:
            environ = self._read_environ(pid)
        except OSError:
            # A hidden environment is not a reason to throw away a perfectly
            # good ancestry, so this signal fails on its own.
            return None
        if not environ:
            return None
        for rule in _ENVIRONMENT_RULES:
            if any(name in environ for name in rule.keys):
                return replace(rule.origin, signal=OriginSignal.ENVIRONMENT)
            for name, accepted in rule.values:
                value = environ.get(name)
                if value is not None and value.casefold() in accepted:
                    return replace(rule.origin, signal=OriginSignal.ENVIRONMENT)
        return None


def _match_launcher(entry: ProcessEntry) -> Origin | None:
    names = {entry.name.casefold()}
    command = entry.command
    if command:
        names.add(Path(command[0]).name.casefold())
        # `node /path/to/claude.js` is Claude Code and `gh copilot` is
        # Copilot, so the first real argument counts as identity too.
        if len(command) > 1 and not command[1].startswith("-"):
            names.add(Path(command[1]).name.casefold())

    haystack = " ".join(command).casefold() if command else ""
    for launcher in _LAUNCHERS:
        if names & launcher.names:
            return replace(launcher.origin, signal=OriginSignal.ANCESTRY)
        if haystack and any(mark in haystack for mark in launcher.fingerprints):
            return replace(launcher.origin, signal=OriginSignal.ANCESTRY)
    return None


def _store(cache: dict[tuple[int, int], Origin], key: tuple[int, int], value: Origin) -> None:
    """Insert under a bound, dropping the oldest half when it is reached.

    Eviction is by insertion order rather than by use: entries age out with
    the processes they describe, and evicting a live one costs a single walk.
    """
    if len(cache) >= _CACHE_LIMIT:
        for stale in list(cache)[: _CACHE_LIMIT // 2]:
            del cache[stale]
    cache[key] = value


_DETECTOR = OriginDetector()


def detect_origin(pid: int, *, create_time: float | None = None) -> Origin:
    """Say who launched ``pid``, never raising and never blocking.

    ``create_time`` is the process creation stamp the caller already holds.
    Passing it lets a repeat lookup answer without touching ``/proc`` at all,
    and keeps a recycled PID from inheriting the previous tenant's badge.
    """
    return _DETECTOR.detect(pid, create_time=create_time)


def clear_cache() -> None:
    """Forget every cached answer. Tests need this; the scanner does not."""
    _DETECTOR.clear_cache()

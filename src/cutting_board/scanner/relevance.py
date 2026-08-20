from __future__ import annotations

from enum import Enum
from pathlib import Path

from cutting_board.models import Ownership


class Relevance(str, Enum):
    """Why a listener does or does not belong on the board."""

    DEV = "dev"
    """A development service: always shown."""

    CONTAINER = "container"
    """Docker, Podman or Kubernetes plumbing: hidden unless asked for."""

    NOISE = "noise"
    """Desktop apps, system daemons and listeners owned by other users."""


CONTAINER_TECH = frozenset({"docker", "kubernetes"})

CONTAINER_NAMES = frozenset(
    {
        "docker-proxy",
        "dockerd",
        "docker",
        "containerd",
        "containerd-shim",
        "podman",
        "conmon",
        "rootlesskit",
        "slirp4netns",
        "k3s",
        "kubelet",
        "minikube",
    }
)

# Desktop and consumer software that opens a port for its own purposes.
# The project rule below already hides most of it; this list also covers the
# case where such an app happens to be started from inside a repository.
DESKTOP_APPS = frozenset(
    {
        "ulauncher",
        "kdeconnectd",
        "kdeconnect-indicator",
        "dropbox",
        "insync",
        "nextcloud",
        "megasync",
        "syncthing",
        "spotify",
        "steam",
        "steamwebhelper",
        "discord",
        "slack",
        "telegram-desktop",
        "signal-desktop",
        "element-desktop",
        "zoom",
        "teams",
        "skype",
        "github-desktop",
        "jetbrains-toolbox",
        "anydesk",
        "teamviewer",
        "rustdesk",
        "barrier",
        "synergy",
        "obs",
        "kdeinit5",
        "plasmashell",
        "gnome-shell",
        "evolution",
        "thunderbird",
        "firefox",
        "chrome",
        "chromium",
        "brave",
        "vivaldi",
        "opera",
        "transmission",
        "qbittorrent",
        "deluge",
        "vlc",
        "kodi",
        "warp",
        "1password",
        "bitwarden",
        "keepassxc",
    }
)

# Build and compile daemons keep a socket open only so their own client can
# reach them. The classifier names them correctly — "Gradle Daemon" is what a
# Gradle daemon is — but they are build plumbing, not a service to visit.
#
# Only a daemon's own entry point counts as a marker: its main class, or the
# launcher jar that carries nothing else. The bare word "gradle" would also
# match the application the daemon forked, whose classpath is full of
# ~/.gradle/caches paths and which is exactly what the board exists to show.
BUILD_DAEMON_MAIN_CLASSES = frozenset(
    {
        "org.gradle.launcher.daemon.bootstrap.gradledaemon",
        "org.gradle.process.internal.worker.gradleworkermain",
        "org.jetbrains.kotlin.daemon.kotlincompiledaemon",
        "org.apache.maven.cli.daemonmavencli",
        "org.mvndaemon.mvnd.daemon.server",
        "com.facebook.nailgun.ngserver",
        "com.martiansoftware.nailgun.ngserver",
    }
)

# Jar basenames, compared by prefix because a version suffix usually follows.
BUILD_DAEMON_JARS = ("gradle-daemon-main", "gradle-worker")

# Program names that are a build daemon whatever the rest of argv says.
BUILD_DAEMON_NAMES = frozenset({"mvnd"})

# Executables here belong to the distribution, not to a checkout.
SYSTEM_PREFIXES = (
    "/usr/lib",
    "/usr/libexec",
    "/usr/share",
    "/usr/sbin",
    "/opt",
    "/snap",
    "/var/lib/flatpak",
    "/var/lib/snapd",
)


def relevance_of(
    *,
    ownership: Ownership,
    process_name: str,
    executable: str | None,
    command: tuple[str, ...],
    tech: str,
    specific: bool,
    has_project: bool,
) -> Relevance:
    """Decide whether a listener is development work worth showing.

    ``specific`` means the classifier recognised an actual framework, daemon
    or tool rather than falling back to the bare runtime name. A listener is
    development work when it was recognised, or when it runs out of a real
    project checkout. Everything else is a desktop app or a system daemon.

    Build and compile daemons are the exception the ``specific`` shortcut
    cannot express: they are recognised, and named well, yet nobody visits
    them. They are ruled out before the shortcut runs.
    """
    if ownership is not Ownership.CURRENT_USER:
        return Relevance.NOISE

    names = _candidate_names(process_name, executable, command)

    if tech in CONTAINER_TECH or names & CONTAINER_NAMES:
        return Relevance.CONTAINER

    if names & DESKTOP_APPS:
        return Relevance.NOISE

    if names & BUILD_DAEMON_NAMES or _is_build_daemon(command):
        return Relevance.NOISE

    if specific:
        return Relevance.DEV

    if executable and any(executable.startswith(prefix) for prefix in SYSTEM_PREFIXES):
        return Relevance.NOISE

    return Relevance.DEV if has_project else Relevance.NOISE


def _is_build_daemon(command: tuple[str, ...]) -> bool:
    """Whether argv carries a build or compile daemon's own entry point."""
    for token in command:
        lowered = token.casefold()
        if lowered in BUILD_DAEMON_MAIN_CLASSES:
            return True
        # A classpath packs many entries into a single argument, so each one
        # has to be weighed on its own rather than as a substring of the whole.
        for entry in lowered.split(":"):
            name = entry.rsplit("/", 1)[-1]
            if name.endswith(".jar") and name.startswith(BUILD_DAEMON_JARS):
                return True
    return False


def _candidate_names(
    process_name: str,
    executable: str | None,
    command: tuple[str, ...],
) -> set[str]:
    """Names that could identify the program, including interpreted scripts.

    ``/usr/bin/python3 /usr/bin/ulauncher`` is ulauncher, not Python, so the
    first few arguments count as identity too.
    """
    names = {process_name.casefold()}
    if executable:
        names.add(Path(executable).name.casefold())
    for token in command[1:3]:
        if token.startswith("-"):
            continue
        names.add(Path(token).name.casefold())
    return {name for name in names if name}

# Cutting Board

A local development services board for Ubuntu, independent of any IDE.

Cutting Board watches the TCP listeners on your machine, works out which of them
are development services and which are noise, and draws the survivors as tiles
grouped by project. Nothing has to register with it: start a dev server from a
terminal, an agent or an IDE and it appears on the next scan. The scanner starts
with the window and stops with it — there is no daemon and no autostart.

![The Cutting Board services board](assets/cutting-board-screenshot.png)

The application's user interface is in Korean. This documentation is in English.

## Why

Running several coding agents in parallel leaves invisible processes behind:
a Vite server here, a Spring Boot app there, a Postgres container, a stray
Storybook. `ss -tlnp` tells you a port is taken; it does not tell you which
project it belongs to or whether you still need it. Cutting Board answers those
two questions at a glance and lets you stop the process without hunting for the
terminal that started it.

## Install

Build the Debian package from the checkout and install it:

```bash
sudo apt install dpkg-dev
./scripts/build-deb.sh
sudo apt install ./dist/cutting-board_0.1.0_all.deb
cutting-board
```

`./scripts/install-ubuntu.sh` does the same in one step, building the package
first if it is not there yet. The package is a build output and is not kept in
the repository; the `ubuntu-24.04` workflow also uploads one as a build
artifact.

`apt install ./...deb` pulls in the declared dependencies (`python3`,
`python3-tk`, `python3-psutil`). Uninstall with `sudo apt remove cutting-board`.

## Run from source

```bash
sudo apt install python3 python3-tk python3-psutil

./scripts/run.sh            # the real board
./scripts/run.sh --demo     # deterministic demonstration data
./scripts/dev.py            # the same, restarted whenever a source file changes
```

`dev.py` watches `src/` and `assets/` and restarts the process about a second
after a save, passing its own arguments through (`./scripts/dev.py --demo`). It
is not hot reloading: Tk keeps live widgets bound to the classes they were built
from, so reloading a module in place would leave half the window running old
code. Restarting is the honest version, and the window reopens where it was
because the application saves its geometry when it is asked to stop. A syntax
error does not end the session — the watcher reports the exit code and waits for
the next save.

`--demo` is entirely self-contained: it renders a fixed snapshot, its stop
buttons never signal a real PID, and its Docker tab is fed by `demo_containers()`
rather than the Docker daemon. It is safe on a machine with no Docker installed
and is what the GUI smoke test drives.

Python 3.10 or newer, plus `psutil`. Only `psutil` is a third-party runtime
dependency; Tkinter ships with the distribution's Python.

## Command line

```
cutting-board                        open the board
cutting-board --demo                 open the board with demonstration data
cutting-board --version              print name and version
cutting-board --scan-interval 4      override the saved scan interval (0.75-30 s)

cutting-board --snapshot             scan once, print a table, do not open the GUI
cutting-board --snapshot --json      the same scan as machine-readable JSON
cutting-board --snapshot --containers  also list container plumbing
cutting-board --snapshot --all       list every TCP listener, noise included
```

`--json`, `--containers` and `--all` only mean anything alongside `--snapshot`.
The plain-text snapshot prints one row per service with the project, service
name, technology, PID, ports and redacted command. The JSON form contains the
same set of services with plain strings and JSON primitives, never enum objects,
and never the raw `command` token array.

Exit codes: `0` success, `1` the scan produced warnings, `2` not Linux, `3`
Tkinter is missing, `4` no display could be opened.

## What reaches the board, and what does not

Most listening sockets on a desktop are not development work. Every listener is
classified once, in `src/cutting_board/scanner/relevance.py`, into one of three
buckets:

| Bucket | Meaning | Shown |
|---|---|---|
| `dev` | A recognised framework, daemon or tool, or an unrecognised process running out of a real project checkout | Services tab |
| `container` | Docker, Podman or Kubernetes plumbing — `docker-proxy`, `dockerd`, `containerd`, `k3s`, and friends | Docker tab, and `--snapshot --containers` |
| `noise` | Everything else | Never |

The rules, in order:

1. A listener owned by another UID is noise. Cutting Board only ever shows and
   signals processes belonging to the user running it.
2. Container runtimes and proxies are `container`.
3. Known desktop applications are noise even when started from inside a
   repository — ulauncher, github-desktop, jetbrains-toolbox, browsers, chat
   clients, sync agents and the rest of the list in `relevance.py`.
4. Build-tool daemons are noise. A Gradle daemon, a Gradle worker, the Kotlin
   compile daemon, the Maven daemon and Nailgun all open a TCP port for their
   own client protocol and then sit there for hours after the build finished;
   none of them is a service anybody visits. They are matched on their main
   class or launcher jar, never on the word "gradle" — the application that
   `./gradlew bootRun` forks is exactly what the board is for.
5. If the classifier recognised an actual framework or daemon, it is `dev`.
6. An unrecognised executable living under a distribution prefix (`/usr/lib`,
   `/usr/libexec`, `/usr/sbin`, `/opt`, `/snap`, the flatpak and snapd state
   directories) is a system daemon, so it is noise.
7. Anything left is `dev` if it runs out of a detected project, and noise if it
   does not.

There is no "include system services" switch and no search box: the point of
the board is that it is short enough not to need either. `--snapshot --all`
prints the unfiltered list when you need to check what was dropped.

## The board

Tiles are grouped into sections by project, and sorted within a section by
lowest port. Each tile carries the brand mark, the service name, up to three
port chips, and how long the process has been up. Uptime under five minutes is
tinted with the accent colour, so a just-restarted service stands out. A tile
also carries a badge when the board can tell which tool launched the service —
see below.

Everything else the scanner knows — endpoints and their scope, PID and PPID,
user, CPU, resident memory, working directory, executable, the redacted command
line and any warnings — lives in the detail dialog, one click away.

- **Click a tile** to open its detail dialog.
- **Ctrl-click** a tile whose service looks like a web endpoint to open it in
  the default browser.
- **Hover** a tile you own to reveal the power glyph in its corner; clicking the
  glyph asks for confirmation and then stops the service.
- Dialogs close on Escape, on the close button, or on a click outside them.
- **The cog in the header** opens settings: the version, the Python and platform
  it is running on, where the settings file lives, and the scan interval. The
  version deliberately appears nowhere else — not in the title bar, not in the
  footer.

The grid reflows to the window width. The window remembers its geometry in the
settings file.

The board holds still. A scan lands every two seconds, but a render only redraws
when something a tile actually paints has changed; otherwise the existing tiles
are left alone and only the uptime line is refreshed in place. Without that the
whole board would be destroyed and rebuilt twice a minute, which reads as a
flicker and loses your scroll position every time.

## Who started it

When an agent or an editor started a service, the tile says so with a small
badge in its top-left corner — violet for a coding agent, cyan for an IDE.
Claude, Codex, Cursor, Windsurf, Copilot, Aider, Gemini, VS Code and JetBrains
are recognised. A service you started yourself in a terminal gets no badge:
that is the ordinary case, and labelling it would put a badge on everything.

Two signals answer the question. The first walks the process tree upwards
looking for a launcher it knows. The second reads the environment the process
inherited and looks for marker variables such as `CLAUDECODE` or
`CODEX_SESSION_ID` — only their names, never their values.

The second signal is what makes a detached server attributable. A Gradle daemon
or a backgrounded dev server reparents to init the moment it daemonises, so its
ancestry says nothing more than "the system started it", while the environment
it inherited still names the agent that did.

`ANTHROPIC_*` and `OPENAI_*` are deliberately ignored, because a globally
exported API key would otherwise badge every service on the board.

## Docker tab

Docker is a tab of its own, not a filter. It shells out to the Docker CLI:

```
docker ps --all --no-trunc --format '{{json .}}'
```

Containers are grouped by their `com.docker.compose.project` label, with
standalone containers last and running containers first inside each group. The
brand mark comes from the image name, so `postgres:16-alpine` gets the
PostgreSQL mark. Published host ports are read from the left of each `->` entry,
with the dual-stack duplicate collapsed away; a port that is merely exposed by
the image is not shown, because nothing on the host can reach it.

The call never raises and never blocks for more than two seconds. When Docker is
missing, stopped, or the socket is not readable, the tab says so in that many
words and falls back to listing the container-owned listeners the port scanner
found on its own. The container list refreshes every 6 seconds while the tab is
open and every 45 seconds while it is not, because a `docker ps` is far more
expensive than a port sweep.

## Project detection

The working directory of a listening process is walked upwards. A `.git` root
wins; failing that, the nearest directory holding a supported marker is used.
The nearest `package.json` is remembered separately, so a service in a monorepo
can still be named after its own package.

Supported markers: `.git`, `pnpm-workspace.yaml`, `package.json`, `pom.xml`,
`build.gradle{,.kts}`, `settings.gradle{,.kts}`, `Cargo.toml`, `pyproject.toml`,
`go.mod`, `docker-compose.y{a,}ml`, `compose.y{a,}ml`.

`/`, `/tmp`, `/var/tmp`, `/usr`, `/opt`, `/etc` and `$HOME` are never project
roots. A dotfiles repository in the home directory is common enough that without
this rule every application started from `~` would be filed under a project
named after the user.

Services with no detected project are grouped under a catch-all section rather
than being hidden.

## Stopping a service

The stop action sends `SIGTERM` to the PID that actually owns the port and waits
1.5 seconds. If the process is still alive, a second confirmation offers
`SIGKILL`. Before either signal is sent, the terminator re-checks that:

- the PID is not 1, not Cutting Board itself, and greater than 1;
- the process creation time still matches the one recorded during the scan, so a
  recycled PID cannot be signalled by mistake;
- the effective UID still matches the current user.

Only the listening process is stopped. Process trees, restarts and log capture
are out of scope.

## Assets

The board draws brand marks, tile chrome and power glyphs from PNGs committed
under `assets/`. They are produced by a build-time script that the application
itself never runs:

```bash
./scripts/build-assets.py      # brand marks, tiles, glyphs, window icon blob
./scripts/build-app-icon.py    # application icon sizes from the master artwork
```

`build-assets.py` needs network access, Pillow, pycairo and PyGObject with
librsvg; `build-app-icon.py` needs only Pillow. Neither is a runtime dependency.
`build-assets.py` downloads 59 Simple Icons marks, rasterises each with librsvg,
tints it for legibility on the near-black canvas, draws two further glyphs
locally (`service` and `ssh`, which have no brand to borrow), renders the three
tile states and the two power glyphs, and packs the application icon into the
`_NET_WM_ICON` payload described below. Output lands in `assets/icons/` (61
marks at 48 px and 96 px), `assets/ui/` and `assets/window-icon.argb`. Run it
only when the catalogue changes.

Brand marks are from [Simple Icons](https://github.com/simple-icons/simple-icons),
released under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/).
Each mark remains the trademark of the project it represents and is used here
only to identify that technology.

## Window icon

Tk 8.6 sets only the legacy WM_HINTS icon pixmap on X11, which modern desktops
ignore, so `src/cutting_board/ui/window_icon.py` writes the `_NET_WM_ICON`
property itself through ctypes and Xlib once the window manager has reparented
the window. The whole path is best effort — no X11, no library or no artwork
simply leaves the window as Tk left it. GNOME does not draw application icons in
title bars, so the visible effect is in the task switcher and the dock.

## Project layout

```text
src/cutting_board/
├─ app.py               CLI parsing, platform check, GUI assembly
├─ controller.py        the scan thread and its latest-event queue
├─ constants.py         name, version, interval bounds, markers, secret flags
├─ models.py            immutable snapshot dataclasses
├─ presentation.py      visibility rules, grouping, display formatting
├─ demo.py              deterministic data for --demo and smoke tests
├─ scanner/
│  ├─ base.py           the ServiceScanner protocol
│  ├─ linux.py          TCP listeners plus process metadata
│  ├─ classifier.py     what a process or an image is running
│  ├─ relevance.py      dev / container / noise
│  ├─ origin.py         which tool launched a process
│  └─ docker.py         the docker ps wrapper
├─ services/
│  ├─ termination.py    validated SIGTERM and SIGKILL
│  ├─ settings.py       atomic XDG JSON settings
│  └─ browser.py        opening a local URL
└─ ui/
   ├─ main_window.py    tabs, board rendering, event pump
   ├─ widgets.py        scroll area, section headers, tiles
   ├─ dialogs.py        detail and confirmation dialogs
   ├─ theme.py          palette, metrics, fonts
   ├─ icons.py          PNG loading and caching
   └─ window_icon.py    _NET_WM_ICON via ctypes/Xlib

assets/                 committed artwork; assets/icons is generated
scripts/                run, watch-and-restart, verify, package, asset builds
packaging/              Debian control and desktop entry
tests/                  unit tests and a live TCP integration test
SPEC.md                 the detailed specification
CONTRIBUTING.md         development setup and conventions
CHANGELOG.md            release notes
TEST_REPORT.md          the last recorded verification run
AGENTS.md               working rules for coding agents on this repository
```

## Tests

```bash
PYTHONPATH=src python3 -m unittest discover -s tests
./scripts/verify.sh          # the full gate: compile, tests, snapshot, GUI, package
```

`verify.sh` needs `dpkg-dev`, plus either `xvfb-run` or a display it can already
reach for the GUI stages; with neither it stops there rather than reporting a
pass it did not earn. The same script runs on an `ubuntu-24.04` GitHub Actions
runner.

## Privacy and permissions

- No network requests, no telemetry, no accounts, no cloud storage.
- Run it as your normal user; `sudo` is neither required nor recommended.
- Only `${XDG_CONFIG_HOME:-~/.config}/cutting-board/settings.json` is written,
  and it holds window geometry and the scan interval — never process history.
- Snapshots exist in memory for the lifetime of the process.
- Common secrets on a command line (`--token`, `--api-key`, `--password`,
  `KEY=VALUE` pairs whose key mentions a token, password, secret, api-key,
  credential, auth or database URL) are masked before the command is displayed
  or exported.
- Processes owned by other users are never signalled and never shown.
- Launcher attribution reads `/proc/<pid>/environ`, but only checks whether
  certain variable names are present. No environment value is kept, shown or
  exported.

## Limitations

- Linux only. The scanner boundary is in place for a future macOS
  implementation, but none exists yet.
- TCP `LISTEN` sockets only. UDP services and portless workers are invisible.
- The host network namespace only. A port that exists solely inside a container
  network is not visible to the port scanner; the Docker tab covers published
  ports instead.
- No log capture, no starting or restarting services, no health checks.
- Names and projects are inferred from live process state, so an unusual launcher
  may land in the catch-all group.
- Stopping a service signals the listening process, not its process tree.

## Licence

MIT — see [LICENSE](LICENSE). Brand marks under `assets/icons/` are from Simple
Icons under CC0 1.0, as described above.

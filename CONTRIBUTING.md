# Contributing

## Language policy

**Documentation is written in English.** That covers Markdown files, docstrings,
code comments and commit messages.

**User-facing strings in the application are Korean.** Anything the user reads in
the window — labels, buttons, dialog text, toasts, warnings attached to a
service, the messages in `TerminationResult` and `ContainerListing` — stays
Korean. Section headings drawn as small-caps labels (`ENDPOINTS`, `PROCESS`,
`SERVICES`, `DOCKER`) are English by design and are part of the visual style.

The two rules do not overlap: a Korean string in a docstring is wrong, and an
English sentence shown to the user is wrong.

## Setup

Ubuntu 24.04:

```bash
sudo apt update
sudo apt install python3 python3-tk python3-psutil xvfb dpkg-dev
```

`python3-psutil` is the only third-party runtime dependency; `xvfb` and
`dpkg-dev` are needed by the verification script, not by the application.

Run from the checkout:

```bash
./scripts/run.sh              # the real board
./scripts/run.sh --demo       # deterministic data, no live scanning, no Docker
./scripts/run.sh --snapshot   # headless, prints a table
./scripts/dev.py              # restarts the window on every save; takes the same flags
```

`--demo` is self-contained: fixed services, a fixed container listing from
`demo_containers()`, and a terminator that never signals a real PID. Use it for
UI work, and use it when you have no Docker daemon.

While iterating on the interface, `./scripts/dev.py` is usually what you want:
it polls `src/` and `assets/` and restarts the process a moment after a save.
It is not hot reloading — Tk widgets stay bound to the classes that built them,
so an in-place `importlib.reload` would leave a window running two versions of
itself at once. The restart is fast because the application saves its geometry
on the way out and the watcher stops it with SIGTERM rather than killing it, so
the window comes back the same size in the same place.

## Tests

```bash
PYTHONPATH=src python3 -m unittest discover -s tests
PYTHONPATH=src python3 -m unittest discover -s tests -v      # per-test names
PYTHONPATH=src python3 -m unittest tests.test_docker -v      # one module
```

The suite is standard-library `unittest` — there is no pytest, and no test
dependency to install. It must stay that way so a clean Ubuntu box can run it.

The full release gate:

```bash
./scripts/verify.sh
```

It compiles everything, re-parses the source against Python 3.10 syntax, runs
the suite with `-W error::ResourceWarning`, validates the JSON snapshot schema
against the live host, drives the GUI twice, builds the `.deb`, re-runs the GUI
from the extracted package, and checks source hygiene.

The GUI stages want `xvfb-run`. Without it the script falls back to a `$DISPLAY`
it can already reach, checked with `xdpyinfo`, so a developer on their own
desktop can still run the gate. With neither, it stops with an error rather than
skipping those stages.

Note that stage 7 greps `src/`, `tests/`, `README.md` and `SPEC.md` for the
literal strings `TODO`, `FIXME` and `example.invalid`, and fails the build if it
finds any. Describe unfinished work in prose in those files.

### What to test

- Pure inference and formatting get unit tests: the classifier, relevance,
  project resolution, presentation, settings, the Docker parser.
- OS behaviour gets an integration test driven by a short-lived child process,
  as in `tests/test_integration.py`. Never write a test that signals a process
  the test did not itself create.
- Docker code is tested through the injected `CommandRunner`, never against a
  real daemon. `tests/test_docker.py` shows the pattern for both parsing and
  every failure path.
- GUI changes must keep the `--demo --auto-close` Xvfb path working. There is no
  automated assertion on rendering, so state the manual checks you performed in
  the pull request.

`tests/helpers.py` builds `ServiceSnapshot`, `ProjectInfo` and
`WorkspaceSnapshot` fixtures; prefer it over hand-rolling a snapshot.

## Code style

- Every module starts with `from __future__ import annotations`.
- Full type hints on every function, method and dataclass field. `pyproject.toml`
  configures mypy in strict mode.
- Snapshot types are `@dataclass(frozen=True, slots=True)`. The scan thread
  publishes finished, immutable objects; nothing downstream mutates them.
- Keyword-only arguments for anything a caller could confuse — most constructors
  in `ui/` take `*` before their parameters.
- Line length 110, target `py310` (see the `[tool.ruff]` section). The code must
  parse under Python 3.10 even though it is developed on 3.12; `verify.sh`
  enforces this with `ast.parse(feature_version=(3, 10))`.
- British spelling in prose and comments: *rasterise*, *colour*, *centre*,
  *normalise*. Identifiers already using American spellings stay as they are.

### Comments explain *why*

The repository's convention is that a comment carries the reason, the constraint
or the trap — never a restatement of the code. Compare:

```python
# Bad: says what the next line already says.
# Skip processes owned by other users.
if int(process.uids().effective) != self._current_uid:
    continue
```

```python
# Good: the reason the argument is absent at all.
# No attrs: psutil.as_dict() treats an empty list as "collect
# everything", which reads memory_maps() for every process and turns a
# sub-second sweep into several seconds. Only uids() is needed here.
for process in psutil.process_iter():
```

If a line looks wrong but is deliberate, that is exactly where a comment belongs.
`ScrollArea`'s toplevel wheel binding, the `/proc/net/tcp` UID shortcut and the
classifier's dependency-value exclusion are all worked examples.

## Design rules

1. **Never hide a real listener to make the board look tidy.** Filtering happens
   in `relevance.py` and must be justifiable; when inference fails, prefer
   showing the service with an honest label over dropping it. `--snapshot --all`
   must always be able to show everything.
2. **Tk widgets are touched only from the main thread.** Worker threads
   communicate through `queue.Queue`, drained by the `after()` pump in
   `main_window.py`.
3. **The scanner returns a finished, immutable `WorkspaceSnapshot`.** No partial
   updates and no callbacks into the UI.
4. **OS boundaries never raise.** Wrap them and turn failures into snapshot
   warnings or an unavailable listing. The board must survive a scan that goes
   wrong.
5. **Platform-specific code stays inside `scanner/` and the OS service
   adapters.** No `sys.platform` checks in `presentation.py` or `ui/`.
6. **Never bypass the pre-signal validation** of PID, creation time and UID in
   `services/termination.py`.
7. **Display `command_display`, never `command`.** The raw argv exists for
   classification only and is stripped from exported JSON.
8. **No feature may require a service to register itself** with Cutting Board.
   Discovery stays unconditional.

## Adding a classifier rule

Rules live in `src/cutting_board/scanner/classifier.py`. Pick the tier by the
quality of the evidence, not by convenience:

- `_DAEMON_RULES` — the program *is* the daemon, identified by process name or
  executable basename.
- `_ARGV_RULES` — identified by an argument, with classpath-style dependency
  values excluded.
- `_FRAMEWORK_RULES` — identifiable only from a launcher or framework artefact
  that may appear in a dependency value. Never put a driver or a client library
  here; that is what made a Spring Boot service look like PostgreSQL.

Needles match on word boundaries, so choose a needle that cannot appear as a
fragment of an unrelated name. Then add tests for:

- the expected display name, category and `tech`;
- a nearby command that must **not** match;
- the monorepo case, if a package name should appear in the label.

A new `tech` id also needs artwork: add it to `CATALOG` in
`scripts/build-assets.py` with a Simple Icons slug and a tint that reads on
`#0B0E14`, then re-run the script (see below) and commit the PNGs. Without
artwork the tile falls back to the generic `service` glyph, which is acceptable
but plain.

## Rebuilding assets

Only when the artwork catalogue changes:

```bash
./scripts/build-assets.py     # brand marks, tile states, power glyphs, icon blob
./scripts/build-app-icon.py   # application icon sizes from the master artwork
```

`build-assets.py` needs network access plus Pillow, pycairo and PyGObject with
librsvg. `build-app-icon.py` needs only Pillow. Neither is a runtime dependency,
and neither should ever become one — the application reads plain committed PNGs.
Commit the regenerated files.

## Adding a platform

1. Implement the `ServiceScanner` protocol from `scanner/base.py`.
2. Build the same `Endpoint` and `WorkspaceSnapshot` shapes from that platform's
   socket information.
3. Isolate signal and ownership differences in a separate service, alongside
   `services/termination.py`.
4. Select the implementation in `app.py`.
5. Add a live listener integration test and a GUI smoke run for the platform.

The presentation and UI layers must not learn which platform they are on.

## Releasing

1. Align the version in `src/cutting_board/__init__.py` (the single source —
   `constants.py` and the packaging scripts read it) and `pyproject.toml`.
2. Update `CHANGELOG.md`.
3. Run `./scripts/verify.sh`.
4. Update `TEST_REPORT.md` with the results you actually observed.
5. Re-run `./scripts/build-deb.sh` and attach the `.deb` to the release. `dist/`
   is ignored by Git — the package is a build output, not a tracked file.

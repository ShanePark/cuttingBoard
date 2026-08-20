# Test Report — Cutting Board 0.1.0

| | |
|---|---|
| Date | 2026-08-20 |
| Test suite | **122 tests, OK** (no failures, errors or skips) |
| Suite runtime | 0.25–0.38 s |
| Byte-compilation | PASS |
| Python 3.10 syntax compatibility | PASS (42 files) |
| Headless JSON snapshot | PASS |
| GUI smoke | PASS — on an offscreen display, not Xvfb; see §1 |
| Debian package build and extraction | PASS |
| `scripts/verify.sh` end to end | **PASS, all seven stages** |
| GUI behaviour (hover, wheel, dialogs) | **no unittest coverage**; exercised by scripted probes, see §5 |

This report records what was actually executed on the host below. Stages that
were not run are named as such rather than being carried over from a previous
report.

## 1. Environment

```text
OS        Ubuntu 24.04.4 LTS
Kernel    Linux 6.8.0-138-generic x86_64
Python    3.12.3
Tk        8.6
psutil    5.9.8
dpkg-deb  1.22.6
xvfb-run  not installed
display   mutter --headless --virtual-monitor 1400x900, via Xwayland on :0
```

This is the release target platform, so there is no cross-distribution gap to
qualify. `xvfb-run` is absent, so the GUI stages ran on an offscreen display
served by a headless `mutter` instead — a real window manager rather than a bare
X server, which is closer to the desktop the application ships to. `verify.sh`
prefers `xvfb-run` and falls back to any `$DISPLAY` it can reach, so no change
to the script was needed beyond that fallback.

## 2. What was run

`./scripts/verify.sh` was run end to end and **all seven stages passed**. Each
stage below reports what that run did.

### Stage 1 — compilation and syntax compatibility · PASS

```bash
python3 -m compileall -q src tests
```

All modules compiled. Every file under `src/` and `tests/` was then re-parsed
with `ast.parse(..., feature_version=(3, 10))`: 42 files, all accepted. The code
is developed on 3.12 and must remain parseable by the 3.10 the package declares.

### Stage 2 — unit and live integration tests · PASS

```bash
PYTHONPATH=src python3 -m unittest discover -s tests
Ran 122 tests in 0.247s
OK
```

Re-run with `-W error::ResourceWarning`, as `verify.sh` does, with the same
result: 122 tests, OK. No socket, file or subprocess is left unclosed.

### Stage 3 — headless JSON snapshot · PASS

```bash
PYTHONPATH=src python3 -m cutting_board --snapshot --json
```

Exit code 0, no scanner warnings. The payload carried `scanned_at`,
`scan_duration_ms`, `services`, `current_uid` and `errors`; `services` was a
list; every value was a JSON primitive rather than an enum object. It was also
asserted directly that **no exported process object contains a raw `command`
key** — the redaction path in `WorkspaceSnapshot.to_dict()` holds.

4 visible services in 84 ms on a live host.

The plain-text form was exercised as well and rendered the project, service,
tech, PID, port and redacted-command columns correctly against real listeners.

### Stage 4 — GUI smoke · PASS

Both passes ran on the offscreen display described in §1:

```bash
python3 -m cutting_board --demo --auto-close 1
python3 -m cutting_board --auto-close 1
```

Each constructed the window, rendered a board, loaded the icons, ran the main
loop and shut down without an exception — the demo one against fixed data, the
other against the live scanner and the real Docker daemon.

### Stages 5 and 6 — Debian package · PASS

`dist/cutting-board_0.1.0_all.deb` was rebuilt from the current tree
(about 290 kB) and its contents and permissions asserted. It was then extracted
and both `--version` and one more GUI pass were run against the packaged code
and assets alone, proving the package carries every module and every generated
asset the application needs.

### Stage 7 — source hygiene · PASS, with one note

No `TODO`, `FIXME` or `example.invalid` marker exists in `src/`, `tests/`,
`README.md` or `SPEC.md`.

The bytecode check reports stray `__pycache__` content in the working tree.
This is an artefact of ordinary local test runs and not a defect: `verify.sh`
deletes those directories in its first lines and redirects new bytecode through
`PYTHONPYCACHEPREFIX`, so the same check passes inside a real script run.

## 3. Automated coverage

| Module | Tests | Subject |
|---|---:|---|
| `test_docker.py` | 26 | `docker ps` parsing, compose labels, port publishing and ranges, dual-stack collapse, state recovery, malformed lines, and every failure path — missing binary, dead daemon, permission denied, timeout, `OSError`, empty output — plus the availability cache and the real runner's arguments |
| `test_classifier.py` | 8 | tier precedence, monorepo package names, jar version stripping, classpath dependencies not stealing identity, word-boundary matching, non-specific runtime fallback, command redaction, and the absence of raw secrets in exported JSON |
| `test_termination.py` | 7 | SIGTERM success, forced kill, PID reuse refusal, foreign UID refusal, timeout escalation, self-protection, already-exited |
| `test_relevance.py` | 17 | build-daemon markers demoted to noise — main class, launcher jar, legacy Nailgun — against the real classifier rather than a hand-set flag, plus the cases that must stay `dev`: an application on a long `~/.gradle` classpath, one embedding the Gradle tooling jars, `./gradlew bootRun` and its shell-wrapped form; container precedence is asserted too |
| `test_presentation.py` | 5 | noise never visible, containers opt-in, the retained query filter, group ordering, formatters |
| `test_project.py` | 4 | Git root grouping a nested package, nearest marker without Git, no marker returning `None`, and a previous miss not masking a marker created later |
| `test_scanner.py` | 4 | endpoints of one PID grouped and enriched, pidless rows enriched from the per-process walk, unknown PIDs preserved rather than dropped, JSON-safe snapshots |
| `test_models.py` | 4 | browser-URL eligibility, TLS port scheme selection, no button for databases or unknown ports |
| `test_settings.py` | 4 | round trip, clamping and fallback of invalid values, corrupt JSON, unwritable config directory |
| `test_controller.py` | 4 | thread start / manual refresh / close lifetime contract, latest-wins queue behaviour, interval clamping, and the worker waking on an interval change |
| `test_integration.py` | 1 | the live test described below |
| `test_demo.py` | 2 | the demo snapshot is complete and JSON-serialisable, and carries both attributed and unattributed services |
| `test_origin.py` | 36 | launcher attribution: ancestry matching including truncated comm names, environment markers, the rank rules that settle a disagreement, the detached-daemon case, cache keying by start time, and every failure path — dead PID, hidden `/proc`, a reader that raises |

### The live integration test

`test_integration.py` starts a real Python TCP server inside a temporary Git
checkout and asserts that the scanner finds the kernel-assigned port, attributes
the correct PID, infers the temporary project's name, marks the service as
terminable by the current user, and that `ProcessTerminator` actually delivers
`SIGTERM` so the child exits and the port is released. It is the only test that
signals a process, and it only ever signals a process it created.

## 4. Live measurements

Twelve consecutive sweeps with one reused `LinuxServiceScanner`:

```text
duration   80–102 ms  (95, 80, 102, 88, 81, 80, 81, 80, 81, 81, 82, 84)
listeners  36 seen, 4 classified as development services
errors     none
```

This is comfortably inside the 2-second default interval, and scanning runs on a
background thread rather than the Tk main thread. For context, before the two
scanner fixes described in `SPEC.md` §15 a sweep on comparable hardware took
roughly 9,900–17,500 ms.

Relevance filtering is doing real work on this host: 36 listeners observed, 4
shown. The remainder were desktop applications, system daemons and sockets owned
by other users.

## 5. Not covered by automated tests

None of the following is covered by the unittest suite. Most were exercised in
this session by scripted probes driving a real Tk instance on the offscreen
display, which is recorded here as what it is: evidence weaker than a test in
the suite, stronger than nothing.

Verified by probe:

- Mouse-wheel scrolling — eight assertions covering the clamp at the first and
  last tile, a widget created after construction scrolling without a re-bind,
  the wheel over the header being ignored, a no-op when the content already
  fits, the scrollbar hiding in that case, the toplevel binding being released
  on destroy, and no error after the area is destroyed. All pass.
- Dismissal on an outside click for `ServiceDetailDialog`,
  `ContainerDetailDialog`, `SettingsDialog` and `ConfirmDialog`, each also
  asserted to survive a click inside itself. For the confirmation, an outside
  click resolves to a refusal. All pass.
- The scan-interval chips: clicking one writes the settings file, updates the
  window's settings object and calls through to `ScanController`, with the value
  observed on all three. Escape then closes the dialog.
- Board rendering, both tabs, the launcher badges, the settings dialog and the
  detail dialogs were captured as screenshots and inspected.
- The repaint gate — 46 assertions on Tk widget path names, which are never
  reused, so identical names prove the same widgets survived. Tiles are
  identical across an unchanged scan fed a fresh snapshot object; they are all
  replaced when a service appears, disappears or changes port; the uptime text
  repaints in place with the fresh/stale tint switching at
  `FRESH_UPTIME_SECONDS`; a scroll position parked mid-board survives a scan; a
  narrower window rebuilds and a wider one rebuilds back; the busy state flips
  both ways; a toast survives a skipped render; and a click on a reused tile
  hands the dialog the newest snapshot rather than the one the tile was built
  from. All pass.
- The same against the live scanner and the real Docker daemon: **0 rebuilds in
  5 renders on the services tab and 6 on the Docker tab**, with 12 running
  containers across 3 compose groups.
- `scripts/dev.py`: a source change restarts the child process, SIGTERM on the
  watcher takes the window down with it, and a direct SIGTERM to the
  application exits cleanly having saved `window_geometry`.

Still unverified, and needing a human at a real desktop session:

- Tile hover and armed states, and the power-glyph hit zone.
- Toast display and expiry.
- `_NET_WM_ICON` publication and how the icon appears in the task switcher and
  dock — the property is written, but only a desktop shell can show the result.
- Font fallback when the preferred families are absent.
- Whether the offscreen compositor renders identically to a real GNOME session.

The README's `assets/cutting-board-screenshot.png` is an illustrative demo-board
screenshot. The GUI smoke test in `verify.sh` stage 4 asserts nothing about
rendering; it proves only that the window constructs, runs and closes without an
exception.

## 6. Security properties asserted by tests

- A process owned by another UID is never signalled (`test_termination.py`).
- PID 1 and Cutting Board's own PID are protected.
- A mismatched creation time blocks the signal, so a recycled PID cannot be
  killed by mistake.
- `SIGKILL` is a separate path requiring an explicit force flag; nothing
  escalates automatically.
- Secrets on a command line are masked in `command_display`
  (`test_classifier.py`).
- Exported JSON contains no raw `command` array — asserted both in the unit
  tests and against the live snapshot in stage 3.
- The only subprocess the application spawns is `docker ps`, with a fixed
  argument list and a timeout, and its failure paths are exhaustively tested
  without a daemon.

## 7. Outstanding

The whole gate has now run here, so what remains is what no headless run can
answer. The `ubuntu-24.04` GitHub Actions workflow runs the same script with
`xvfb-run` present and uploads the resulting package.

Confirm on a real desktop session:

- Cutting Board appears in the Ubuntu application menu and its icon is drawn in
  the task switcher and dock;
- the window text and fonts render correctly under Wayland/XWayland;
- real Node, Java and Python development servers are attributed to the right
  PIDs and projects;
- the power glyph stops a test server owned by the current user;
- the Docker tab lists containers, and degrades with a message on a machine
  where the daemon is stopped.

## 8. Verdict

All seven stages of `scripts/verify.sh` pass on this host: 122 tests,
compilation, 3.10 syntax compatibility, the live headless snapshot and its
redaction guarantee, the GUI smoke passes, the `.deb` build, and the same GUI
run again from the extracted package. The scanner's performance target holds at
80–102 ms per sweep.

What is not established is how the interface behaves under a hand on a real
desktop: hover and armed tile states, the toast, the icon in the shell, and font
fallback. Those are listed in §5 and remain the release's open risk.

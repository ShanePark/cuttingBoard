# `cutting-board control` review — 2026-11-03

This document reviews seven requests from the oasis26 session, which drove a dev environment using
only `cutting-board control` on version 0.6.0. It checks each report against the code and records
what will be built and what was dropped.

Evidence is code reading, read-only commands (`control list/status`, `ss`, `docker ps/inspect`,
`docker compose config`) and analysis of the real task logs. No service was restarted during the
review, so anything that needs a live run is marked **unverified**.

## Scope

Implementation order:

| Order | Work | Item | Size |
|-------|------|------|------|
| 1 | Specific container failure codes, misclassification fix | 3 | S |
| 2 | `control wait`, `logs --current-run` | 1 | M |
| 3 | `control show` | 2 | S |
| 4 | Log safety and noise fixes | 7, side findings | S |
| 5 | `control update` | 2 | M |

### Dropped

| Request | Why |
|---------|-----|
| Recreate a deleted container (item 4) | Rare, and a wrong project name yields an empty database. `container_missing` tells the caller what happened; running `docker compose up` by hand stays the recovery |
| Compose down changes (item 4) | The dialog already says containers are removed, and the oasis26 removal probably came from a terminal |
| `start --profile --all`, reordering, `depends_on` (item 5) | Needs a new reorder UI to be correct. A caller can already loop over `start` and `wait` |
| `status --profile` (item 5) | `list` already returns this |
| Prepare cache filter, `prepare_cache`, `phase` (item 6) | One wasted no-op build per frontend rebuild, and a false hit would be worse than the miss |
| Daemonizing detection (item 7) | Affects a single task, and removing `-f` from it solves the case. Static matching gives false alarms |
| `--wait-ready`, `port_listening`, `run_id`, HTTP probe (item 1) | `wait` covers these. A status connect would touch every task on each `list` |
| Task rename and offline `show` | Rename orphans logs and breaks references; the CLI already needs the app running |
| Log rotation, `safe_name` collisions, global profile lease, `gradle` parent directory bug | Low impact, unrelated to the request |

## Where the reports differ from the code

| Report | What the code shows |
|--------|---------------------|
| `task update` must refresh in-memory state | The backend holds no profile state; every read goes to the file (`lib.rs:635-641`) |
| Running tasks pick up edits at next start | The UI refuses to save a profile with any active task (`lib.rs:252-259`). Editing a running task's name, port or cwd would lose track of its process |
| Changing `command` changes what runs | Not for prepared Spring tasks: Gradle runs `bootRun` and takes only the active profiles from the command (`prepare.rs:676-704`) |
| UI compose down removed the container | Unlikely. The button only appears for projects with two or more containers (`docker-groups.ts:41`), and oasis26's compose file has one service. The actual path is **unverified** |
| Cache miss comes from generated files | Half right. The fingerprint is also computed before the build and saved after it (`prepare.rs:118,183`), so the next restart misses once. It is not a permanent miss |
| The nine blank log lines are odd `ssh -f` output | They are left by `sanitize_task_log`: it removes each start marker but not the newline written before it (`launch.rs:385`) |
| Changing the sanitizer may affect the UI | The UI reads raw logs and never calls the sanitizer. Changing the marker text is what would break it (`restart-progress.ts:184-191`, `logs.rs:272,287`) |

## Design

### 1. Failure reasons (item 3)

The reason is lost in three places in `control_service.rs`: `operation_error` (`:846`) for `start`,
`safe_snapshot_message` (`:1021`) for `status`, and `safe_read_error` (`:1002`) for `logs`.

`operation_error` also misclassifies by matching substrings of the whole message:

| Input | Result | Problem |
|-------|--------|---------|
| Missing container named `maven-db` | `prepare_failed` | The name matched "maven" |
| `docker start` failure | `spawn_failed` | Advises checking the working directory and executable |
| `docker restart` failure | `operation_failed` | "Could not restart" misses the pattern |
| `stop` on a missing container | `stop_failed` | `stop_profile` silently skips the same case (`containers.rs:76-82`); this stays a failure, with a clear code |

Use typed errors instead of substring matching: `container_missing`, `docker_unavailable`,
`container_action_failed`. Pass the container reason through `status`. Add the port number to
`port_in_use` and the Docker status to `process_exited`.

Masking is deliberate and pinned by tests (`control_service.rs:1213,1236`), so use an allowlist:
container names, port numbers, and the last line of Docker stderr with a length limit. Paths and
command lines stay blocked. Prefer new codes and messages over new fields, because `ControlError` uses
`deny_unknown_fields` and a new field would break older CLIs (`control.rs:72`).

### 2. Readiness (item 1)

`restart` returns about 80 ms after spawn (`launch.rs:459-460`), and `running` only means the process
is alive. Logs are append-only and start markers are filtered out, so grepping `logs` for
`Started <MainClass>` matches lines from the previous run.

Measured on the real logs: at spawn, the previous run's `Started` line was still in the 64 KiB tail
in 146 of 275 starts (53%). Spring's reported startup had a median of 22 s and a p90 of 36 s.

```
cutting-board control wait --profile <ID> --task <NAME> [--log-pattern <TEXT>] [--timeout-seconds N]
cutting-board control logs ... [--current-run]
```

- A separate `wait` can resume after a restart timeout, can wait on a restart started elsewhere, and
  keeps `restart` exit codes unambiguous. Timeout defaults to 120 s, range 1–300.
- The CLI loops about once per second; a new one-shot `Readiness` request decides in the backend.
  Probing from the client would give wrong answers in a sandbox, and blocking on the server would
  hold one of 16 connection slots.
- Decision order: operation in flight → wait. Operation failed since this run → fail with its code.
  Exited → fail. Container with a healthcheck → use health. `--log-pattern` → match in the current
  run only, replacing the port check. Otherwise the port must accept a connection and the listener
  must not predate this run. No port and no pattern → `readiness_unavailable`.
- A container without a healthcheck counts as ready on a port connection, labelled `container_port`.
- The failed-operation check is essential: when prepare fails the old process keeps its port
  (`launch.rs:534-536`), so a port check alone would report the old process as ready.
- Use a connect-only probe. `port_is_occupied` binds the port and could collide with a booting
  service. Connecting also covers docker-proxy, which the scanner cannot see.
- Run boundary: the last start marker in the raw log, which survives an app restart. If the marker
  has scrolled out of the tail, the whole tail counts as the current run; 97 of 267 starts produce
  more than 64 KiB before `Started`. Processes started outside Cutting Board have no boundary, so
  `--log-pattern` returns `run_boundary_unknown`.
- Container logs have no boundary either, so `--current-run` and `--log-pattern` are rejected for them.
- Exit codes reuse the existing values: 0 ready, 2 no signal, 4 unknown task, 5 timeout, 6 exited or
  failed, 7 app unavailable.

**Known limit:** straight after an app restart, a task that has not bound its port yet is not
rediscovered, so `wait` reports `not_running`. Document this.

### 3. `control show` (item 2)

```
cutting-board control show --profile <ID> --task <NAME>
```

Returns the saved `name`, `cwd`, `resolved_cwd`, `command`, `expected_port`, `container` and `prepare`,
plus `launch_source` (`saved_command | prepare | container`), which exposes the trap where a saved
command is not what runs. Values are printed raw, on explicit request only, and documented like `logs`.
Masking would break round trips: masked values have already been saved back as real commands once
(`storage/migration.rs:17-60`). `list` and `status` keep omitting these fields.

### 4. Log safety and noise

| Finding | Location | Change |
|---------|----------|--------|
| A marker cut by the 64 KiB tail no longer starts with `=== Cutting Board `, so the rest of its command line can pass the filter (**may expose secrets; not reproduced**) | `logs.rs:117-118`, `control_service.rs:1006-1011` | Drop the partial first line of a truncated tail in the control path |
| A prepare marker appended after partial output has the same leak (not reproduced) | `prepare.rs:938-947` | Start the marker on a new line |
| `sanitize_task_log` leaves a blank line per start | `launch.rs:385` | Drop a blank line directly before a start marker; real blank output stays |
| `control logs` ignores the real log source of external tasks and can return stale text | `lib.rs:462`, `launch.rs:132-167` | Use the same source the UI uses |

### 5. `control update` (item 2)

```
cutting-board control update --profile <ID> --task <NAME> [--command S] [--cwd S]
  [--expected-port N | --clear-expected-port] [--clear-prepare] [--timeout-seconds N]
```

- Socket access is enough to use it. Same-user callers can already write the file and call `start`, so
  the OS boundary is unchanged. A caller allowed to use the socket but not to write files gains the
  ability to store a command that runs later; this is accepted because sandboxing is not treated as a
  security boundary here. `README.md:100,122` must state the new contract.
- Reuse the UI's save path under the profile lease (`lib.rs:243-261`). Refresh the scan first, and use
  `run_action` so a lost response is handled.
- Refuse edits to an active target task. Stopped tasks can be edited while their siblings run; the UI
  keeps its stricter rule.
- Return old and new values, plus a warning when the task takes the prepare path.
- Add checks the storage layer lacks: reject port 0, reject command or prepare on container tasks.
- **Required UI fix:** the open edit form saves the whole profile, so it silently reverts any CLI change
  made after the form was opened. `saveProfileFromModal` should re-read and refuse when the profile has
  changed.

## Verification plan

Each change extends the existing tests in its module: `cli.rs`, `control.rs`, `control_service.rs`,
`launch.rs`, `launch/logs.rs`, `launch/prepare.rs`, `launch/containers.rs`, `lib.rs`, and
`tests/*.test.ts` for the UI change. The tests that pin the masking contract stay as they are. Tests use
temporary config directories and never touch the real `launch-profiles.json`. `README.md:96-133` is
updated with each command.

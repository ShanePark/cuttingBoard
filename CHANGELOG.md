# Changelog

Notable user-facing changes to Cutting Board. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Versions follow `0.MINOR.PATCH`: MINOR for features or control CLI contract changes, PATCH for fixes only.

## 0.7.0 - 2026-11-03

First versioned release. It covers the changes since 0.6.0; earlier versions are not recorded.

### Added

- `cutting-board control`: a local CLI that lists, starts, stops and restarts saved launch tasks and reads their logs, printing JSON and talking to the running app over a private per-user socket.
- `cutting-board help` prints the full control reference: commands, responses, exit codes and how `wait` decides. `control help` and `--help` after any control command print it too.
- `control wait`: waits until a task is ready, judged by Docker health, a port connection, or a `--log-pattern` match in the current run, and fails when the run has already failed. It reports `waiting_for: "prepare"` while a build runs and checks only the task it waits for. An app that quits during the wait is reported as `unavailable` (exit code `7`) about 5 seconds later, while a slow or busy app counts as a timeout and a quick app restart does not end the wait.
- `control show` returns a task's saved values. `control update` changes a stopped task's command, working directory, expected port or prepare setting. Both return `resolved`, the build and launch commands a start would really run, including when project detection chose them. Like `command`, it shows saved values unmasked.
- `control logs --current-run` returns only what the latest start wrote.
- `control logs --prepare` returns the output of a task's latest prepare step, with its state, times and failure reason, and a `prepare_failed` error points to it. It also explains a first start that failed to build and a failed restart of a process Cutting Board did not start.
- An operation's `prepare` field, shown by `control operation`, `status` and `list`, says whether a build is running, skipped, succeeded or failed.
- Launch profiles on the board: cards with start, stop and restart actions, confirmation prompts, task details, opening a task in the browser, and running a group's Docker containers from its profile.
- Project-aware lifecycle for Spring tasks, which builds before starting and shows active Spring profiles. A failed build during a restart leaves the old process running.
- Docker controls in the app, related containers shown on service cards, container logs, opening a container's web ports in the browser, and stopping a Compose group with `compose down`.
- Live consoles for services and launch tasks, including tasks started outside Cutting Board, in a resizable dock with search, keyboard navigation, text selection and manual spacing.
- Service and launch restart actions, and live CPU and memory usage.
- In-app updates that rebuild from the local source checkout, on macOS and Linux, with blocking progress and live build output.

### Changed

- New interface: a redesigned service board with neutral floating panels and window controls in the toolbar. Board columns stay steady as cards come and go.
- Board groups show card counts, and larger groups and active launch groups come first. Idle launch groups and stopped tasks are marked at a glance.
- Settings changes apply immediately. Launch groups and profile creation are simpler.
- The control CLI reports container failures with typed codes `container_missing`, `docker_unavailable` and `container_action_failed` rather than generic or misleading ones, and `status` includes the container reason.
- Process refreshes and rolling consoles do less background work.

### Fixed

- The profile edit form refuses to save over a change made after it opened, such as one from `control update`. The check runs at the moment of writing, so a change made while the save is under way is not overwritten either.
- Listing containers gives up after 10 seconds, so a hung Docker daemon no longer blocks container views or `control` status requests indefinitely. `control wait` on a task that is not a container does not depend on Docker at all.
- Stopping a profile while Docker is unavailable or hung reports that its containers may still be running, instead of claiming success. Its other tasks are still stopped.
- `control logs` reads the same output as the app for processes started outside Cutting Board, no longer returns part of a cut-off start marker, and drops the blank line each start left behind. A process with many open files no longer loses its log source.
- `control logs`, `start`, `stop` and `restart` say which of the profile and the task does not exist, and `logs` on an unknown profile returns `unknown_profile`.
- A task whose build could not be planned leaves the reason in its log.
- macOS privacy permissions persist across updates through stable app signing.
- Launches keep their environment and Spring profiles when started through the project's native tooling.
- Uptime resets after a launch restart and is kept while a task is restarting.
- Self-update builds use the configured shell.
- Services are named after their working directory project, Maven modules after their own `artifactId`, and registered submodules group under their superproject. Services without a project stay out of container groups.
- A launch profile can be deleted while it runs.
- Long service and group names stay within their cards and headers without shrinking icons, and the workspace list keeps its scroll position across re-renders.
- Slow service log loading no longer blocks the interface, and saving window geometry no longer runs on the event loop.

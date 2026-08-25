## Implementation

- Read the relevant code, tests, and local instructions before editing.
- Before implementation, ask clarifying questions about any remaining material ambiguity that affects scope, behavior, interfaces, or acceptance criteria. Do not proceed on material assumptions.
- Make the smallest, simplest change that fully satisfies the request.
- Do not invent requirements, broaden scope, or add speculative abstractions or dependencies.
- Follow existing conventions and preserve unrelated behavior and work.
- Write clear code. Comment only non-obvious rationale, invariants, or constraints.

## Language

- Documentation is written in English. This covers Markdown files, docstrings, code comments, and commit messages.
- User-facing strings in the application itself are English.

## Delegation and Parallel Work

- The main agent's primary role is orchestration: planning, decomposition, delegation, coordination, review, user communication, and handling additional work—not hands-on execution.
- Delegate repository exploration, implementation, testing, and verification to subagents by default. Do not occupy the main agent with substantial work that can be delegated.
- Maximize safe parallelism across independent workstreams.
- The main agent may directly handle only brief, local tasks that do not benefit from delegation, as well as integration, conflict resolution, or work that requires its broader context.
- Give each delegated task a single owner with explicit scope, deliverables, dependencies, and verification criteria.
- Parallelize only independent work. Concurrent writes must not overlap in files, mutable state, or contracts.
- Shared files and cross-cutting contracts must have a single owner.
- The main agent remains accountable for integration, review of the final diff and verification results, and the accuracy of the completion report.

## Completion

- Run verification proportional to the change, starting with focused checks and expanding based on risk and blast radius.
- Never weaken or bypass tests or checks to make a change pass.
- Do not claim completion without relevant verification. State exactly what was not verified and why.
- Distinguish failures caused by the current change from pre-existing failures.
- Report what changed, the checks run and their results, unverified areas, and remaining risks or assumptions.

## macOS Development

- At the end of each development session on macOS, run a fresh application build and restart the app from that build. Prefer `npm run tauri build -- --bundles app` for the local restart workflow; use `npm run tauri build` (or the equivalent `make build` target) when all distributables are required.
- Treat `~/Applications/Cutting Board.app` as the stable installed app path used by the Dock. After a successful build, force-quit every running `Cutting Board` instance, sync the fresh bundle from `src-tauri/target/release/bundle/macos/Cutting Board.app` to that stable path, launch the stable path with `open -n`, and verify that the new instance is running from the stable path. This restart is part of the task and must be completed automatically after bundle changes; do not stop after building, leave the app running from the transient build directory, or leave an older instance running.

## Linux Development

- At the end of each development session on Linux, rebuild the debug binary and restart the running development app so the change can be checked in the real app. This restart is part of the task; do not leave an older instance running or stop after building.
- Build with `cargo build --manifest-path src-tauri/Cargo.toml`, which produces `src-tauri/target/debug/cutting-board`.
- Stop every running instance first with `pkill -TERM -f 'src-tauri/target/debug/cutting-board'`. The launcher focuses an existing window instead of starting a second instance, so a stale process silently blocks the restart.
- Relaunch detached with `setsid ~/.local/bin/cutting-board-dev >/dev/null 2>&1 </dev/null &` when that launcher is present; it starts Vite on port 1420 when the port is free, runs the debug binary against it, and stops the server it started on exit. Without the launcher, use `npm run tauri dev`.
- Verify the restart: the process list shows the launcher, Vite, and the debug binary, and `wmctrl -lx` lists a `cutting-board.Cutting-board` window. Vite writes to `~/.local/state/cutting-board/vite.log`.
- The debug binary loads the frontend from the Vite dev server, so frontend-only edits reach the running app through hot reload. A full restart is still expected once the work is done.

## Git

- Do not perform version-control operations that change local or remote repository state unless explicitly requested.
- Before any requested version-control write, inspect the working tree and relevant diffs.
- Commit only changes made for the current task.
- Treat pre-existing changes as user-owned. Never discard, overwrite, stage, or commit unrelated work.

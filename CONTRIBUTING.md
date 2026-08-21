# Contributing to Cutting Board

Cutting Board is a Tauri 2 desktop application with a Rust native core and a strict TypeScript/Vite interface. Contributions must preserve the behavior and visual contract in `SPEC.md`.

## Environment

Install:

- Node.js 22 or later
- current stable Rust toolchain with `rustfmt`
- Tauri 2 platform prerequisites
- `lsof` (Linux may fall back to `ss`)
- Docker CLI for manual Docker-tab testing

On Ubuntu, install the same WebKitGTK and desktop integration packages listed in `.github/workflows/ci.yml`.

## Setup

```bash
npm install
npm run tauri dev
```

Deterministic UI data is available without local services:

```bash
npm run tauri dev -- -- --demo
```

## Required checks

Before opening a pull request, run:

```bash
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
npm run tauri build
```

The pull request CI repeats these checks on Ubuntu and macOS and uploads the generated bundles.

## Architecture rules

- Keep process, filesystem, Docker, signal, and launch ownership logic in Rust.
- Keep the TypeScript layer presentation-focused and call native behavior only through typed functions in `src/api.ts`.
- Add every new Tauri command to the explicit invoke handler and expose only the minimum capability permission required by the webview.
- Do not add a Python sidecar, Node sidecar, shell daemon, cloud service, telemetry client, account system, or login-startup behavior.
- Do not write into detected project directories.
- Persist settings and profiles with an atomic replacement; never partially overwrite the only copy.
- Treat process command lines and logs as potentially sensitive.

## Scanner changes

Scanner changes require fixture-level unit tests for parsing and classification. Preserve these invariants:

1. another user's listeners are invisible;
2. known operating-system noise is invisible;
3. Docker plumbing is separated from development services;
4. project attribution never mutates the project;
5. command arguments are redacted before serialization;
6. duplicate listeners for one PID do not create duplicate cards;
7. scan failure is visible and non-destructive.

A new technology classifier should map to an existing icon in `assets/icons/` or deliberately use the generic fallback. Add both 48 px and 96 px artwork only when licensing permits it.

## Termination changes

Termination is safety-critical. A stop implementation must retain all of the following at action time:

- immutable service identity from the last scan;
- current UID verification;
- live PID lookup;
- process creation-time comparison to prevent PID reuse;
- rejection of PID 1 and Cutting Board itself;
- graceful signal before forceful signal;
- no destructive action in demonstration mode.

Do not broaden termination to arbitrary PIDs, container IDs, external launch tasks, parent terminals, or IDEs.

## Launch Profile changes

A task is owned only when Cutting Board created its process session and still tracks it in memory. An expected port occupied by an attributed external process must be presented as external, not adopted. Stop the process group owned by the task, not unrelated listeners that happen to use the same project directory.

Profile and task names must remain bounded and task names unique within a profile. Commands are user-authored shell input; do not transform them silently.

## Interface changes

The current spatial and semantic system is intentional. In particular:

- toolbar: 56 px plus one hairline
- service grid cell: 284 × 136
- visible service card: 268 × 124, 16 px radius
- technology well: 56 px, 14 px radius, 48 px artwork
- action hit targets: at least 36 px where the current design specifies them
- dark/light colors: exact values in `SPEC.md`
- no duplicated in-content logo/wordmark
- keyboard cards: focus, Left/Right action cycling, Enter/Space activation

Test at the 560 × 420 minimum size, common laptop sizes, high-DPI macOS, and Linux WebKitGTK. Avoid UI libraries that reset typography, spacing, native scrolling, or focus behavior.

## Commits and pull requests

Keep commits focused. Explain behavior changes, safety implications, affected platforms, and manual test coverage. Include screenshots for visible changes and logs for platform-specific scanner or bundle failures. Do not commit `node_modules`, `dist`, `src-tauri/target`, generated Tauri schemas, application settings, profiles, or launch logs.

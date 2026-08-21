# Cutting Board

Cutting Board is a native desktop board for local development services. It discovers TCP listeners owned by the current user, groups them by project, identifies common frameworks and infrastructure, shows Docker containers, and owns optional launch profiles for starting and stopping project commands.

The application is implemented with **Tauri 2**, a strict TypeScript/Vite webview, and a Rust native core.

![Cutting Board](assets/cutting-board-screenshot.png)

## Current interface

The Tauri interface preserves the existing Cutting Board design:

- 56 px toolbar with segmented `Services`, `Docker`, and `Launch Profiles` tabs
- semantic dark, light, and system palettes
- 284 × 136 grid cells containing 268 × 124 rounded service cards
- 56 px technology wells, live uptime, launcher origin, compact port chips, browser links, and guarded stop controls
- project section headings, responsive columns, keyboard focus, arrow-key action selection, and modal detail views
- launch profile cards with task state, expected-port ownership, logs, and explicit start/stop actions

## Native capabilities

### Service discovery

The Rust scanner reads listening TCP sockets with `lsof` on macOS and Linux, falling back to `ss` on Linux. It joins listeners to live process metadata, rejects listeners owned by another user, suppresses known operating-system noise, and keeps Docker plumbing separate from user-facing services.

Project ownership is inferred by walking from the process working directory and command paths to markers such as `.git`, `package.json`, `pyproject.toml`, `Cargo.toml`, `go.mod`, `pom.xml`, Gradle files, and Compose files. Technology and category classification cover common web runtimes, API frameworks, databases, caches, and proxies.

### Safe termination

A service can be stopped only when all of the following remain true at action time:

1. the service was marked as a user-owned development process during the last scan;
2. its PID still exists;
3. its process creation time still matches, preventing PID-reuse mistakes;
4. its UID still matches the current user;
5. the target is not PID 1 or Cutting Board itself.

Cutting Board sends `SIGTERM`, waits for a graceful exit, and uses `SIGKILL` only when the validated process ignores the initial signal. Demonstration mode never exposes destructive actions.

### Docker

The Docker tab reads `docker ps -a`, including state, status, image, published host ports, and Compose project/service labels. If the Docker CLI is unavailable, container listener processes discovered by the socket scanner remain visible as a read-only fallback.

### Launch Profiles

Profiles are local JSON records containing a project root and one or more named shell tasks. Cutting Board starts each task in a dedicated process session, redirects output to a local log, tracks ownership, and stops the whole owned process group. A process already listening on a task's expected port is displayed as external and is never stopped by the profile manager.

Settings, profiles, and logs are stored in the operating system's application configuration directory. There is no account, telemetry, cloud sync, or login-startup behavior.

## Development

### Prerequisites

- Node.js 22 or later
- current stable Rust toolchain
- Tauri 2 platform prerequisites
- Linux: WebKitGTK 4.1 development packages and related system libraries
- `lsof`; Linux may use `ss` as a fallback
- Docker CLI only for the Docker tab

### Commands

```bash
npm install
npm run tauri dev
npm run check
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
npm run tauri build
```

Equivalent Make targets are available:

```bash
make install
make dev
make check
make test
make build
```

### Demonstration mode

```bash
npm run tauri dev -- -- --demo
```

The native binary also supports:

```text
cutting-board --demo
cutting-board --auto-close-seconds 5
cutting-board --help
cutting-board --version
```

## Repository layout

```text
src/                         TypeScript presentation and command client
src-tauri/src/               Rust scanner, Docker, persistence, launch manager, Tauri commands
src-tauri/capabilities/      least-privilege Tauri capability declaration
assets/                      application icon source and reference screenshot
public/icons/                generated UI and unsupported-technology PNG fallbacks
.github/workflows/ci.yml     Ubuntu and macOS type-check, test, and bundle jobs
SPEC.md                      normative behavior and design contract
TAURI_MIGRATION.md          Tauri architecture map
```

## Security and privacy

Process command lines can contain credentials. The scanner redacts common password, token, secret, API-key, authorization, and credential arguments before returning process details to the webview. All native actions are exposed through typed Tauri commands; the webview receives only the dialog and URL-opener permissions declared in `src-tauri/capabilities/default.json`.

Launch commands are intentionally user-authored shell commands. Review a profile before starting it. Logs remain local and may contain output produced by the launched program.

## License

MIT. See [LICENSE](LICENSE).

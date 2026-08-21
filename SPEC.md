# Cutting Board — Tauri Product Specification

This document is the normative contract for the Tauri implementation of Cutting Board. The migration changes the runtime and implementation language, not the product behavior, information hierarchy, safety model, or visual identity.

## 1. Product boundary

Cutting Board is a local, single-user desktop utility. It answers three questions:

1. Which development services are listening now?
2. Which Docker containers exist and which host ports do they publish?
3. Which saved project tasks can Cutting Board start and stop as their owner?

It does not provide remote monitoring, cloud sync, authentication, telemetry, automatic login startup, container mutation, or arbitrary system-process administration.

## 2. Runtime architecture

The application shall use:

- Tauri 2 for the desktop shell, command boundary, permissions, window lifecycle, and bundles;
- Rust for socket discovery, process inspection, classification, project attribution, Docker integration, termination, persistence, and managed launch processes;
- strict TypeScript and Vite for presentation and interaction;
- the existing `assets/` artwork without visual substitution.

The shipped application shall not require Python, Tk, Tcl, a Python virtual environment, or a Python sidecar.

The webview may call only registered Tauri commands. Its capability set shall remain limited to Tauri core defaults, local file/folder dialogs, and opening validated URLs with the operating system.

## 3. Main window

### 3.1 Window contract

- title: `Cutting Board`
- default content size: 1080 × 720
- minimum content size: 560 × 420
- resizable: yes
- initial position: centered unless a saved position exists
- saved geometry: restored on the next launch
- closing the window: stop every process group owned by the launch manager, persist geometry, then exit

### 3.2 Toolbar

The top toolbar is 56 px high plus a 1 px lower hairline. It contains:

- a left-aligned segmented tab surface with `Services`, `Docker`, and `Launch Profiles`;
- a count in every tab label;
- one right-aligned, 36 × 36 settings target with a gear glyph;
- no duplicated application logo or wordmark inside the content area because the native window already carries the application identity.

The bottom status bar is 26 px high. It shows listener count and scan duration. Non-fatal scanner warnings may appear at the right edge.

## 4. Visual system

### 4.1 Dark palette

| Token | Value |
|---|---|
| canvas | `#0E0E11` |
| surface | `#1C1C1E` |
| surface-alt | `#2C2C2E` |
| surface-hover | `#343438` |
| hairline | `#38383A` |
| border | `#48484A` |
| text | `#F5F5F7` |
| text-muted | `#C7C7CC` |
| text-dim | `#9EA0A5` |
| accent | `#60ADFF` |
| accent-dim | `#173A5E` |
| accent-hover | `#88C3FF` |
| on-accent | `#0E0E11` |
| violet | `#D38BFF` |
| danger | `#FF756E` |
| danger-dim | `#5C2422` |
| ok | `#30D158` |
| warning | `#FFD60A` |

Category accents are web `#60ADFF`, API `#D38BFF`, database `#85B8FF`, cache `#FF9F5A`, proxy `#30D158`, runtime/other `#9EA0A5`.

### 4.2 Light palette

| Token | Value |
|---|---|
| canvas | `#F2F2F7` |
| surface | `#FFFFFF` |
| surface-alt | `#E9E9EE` |
| surface-hover | `#E5E5EA` |
| hairline | `#D1D1D6` |
| border | `#C7C7CC` |
| text | `#1C1C1E` |
| text-muted | `#3A3A3C` |
| text-dim | `#636366` |
| accent | `#005EB8` |
| accent-dim | `#D9ECFF` |
| accent-hover | `#0053A6` |
| on-accent | `#FFFFFF` |
| violet | `#783399` |
| danger | `#C81D25` |
| danger-dim | `#FCE8E7` |
| ok | `#1B6F30` |
| warning | `#8A5100` |

Category accents are web `#005EB8`, API `#783399`, database `#1D4ED8`, cache `#9A350E`, proxy `#1B6F30`, runtime `#475569`, other `#636366`.

`system` theme follows the operating-system color scheme. Failure to determine a native scheme shall use the dark palette.

### 4.3 Typography

Preferred proportional families, in order, are SF Pro Text, Apple SD Gothic Neo, Helvetica Neue, Inter, Ubuntu, Noto Sans CJK KR, Noto Sans, and the platform sans-serif. Preferred monospaced families are SF Mono, Menlo, JetBrains Mono, Ubuntu Mono, Noto Sans Mono, and the platform monospace.

The interface uses compact 10–12 px metadata and labels, 12 px section/card titles, and larger text only in modal identity or empty-state headings.

## 5. Service board

### 5.1 Section layout

Services are grouped by detected project. Project groups sort alphabetically, then the unassigned `Other` group. A section contains:

- a 3 × 13 accent bar;
- uppercase project name;
- shortened monospaced path on the same line when available;
- a 1 px hairline 7 px below the heading;
- a responsive fixed-cell grid.

Grid cells are 284 × 136 with 6 px gutters. The visible rounded card inside each cell is 268 × 124 with a 16 px radius.

### 5.2 Service card

A card contains:

- a 56 × 56, 14 px-radius technology well containing the existing 48 px artwork;
- a single-line service name;
- live uptime in `Running 4m 12s` form with a 6 px status dot;
- accent treatment for services younger than 300 seconds;
- optional `Agent` or IDE origin badge;
- at most two compact port chips; more than two ports become the first port plus `+N`;
- `No port information` when no endpoint can be represented;
- a compact browser destination without the scheme when a safe URL exists;
- a circular 30 px visible stop control inside a 36 px pointer target when termination is allowed;
- a hidden `↵ Details` hint that appears for keyboard selection.

Hover changes the card to `surface-hover` with a `border` outline. Keyboard focus uses `accent`. The card opens details. The link opens the browser. The power control asks for confirmation and then invokes guarded termination.

Left and right arrow keys cycle enabled card actions. Enter and Space invoke the selected action. Pointer controls shall not require keyboard focus to be usable.

### 5.3 Details

The service detail view shows identity, technology/category, status, launcher origin, project, PID, executable, working directory, redacted command line, CPU, memory, uptime, warnings, and every listening endpoint. The view shall never expose a command-line secret that the scanner recognized as a password, token, secret, API key, authorization value, or credential.

## 6. Discovery and classification

### 6.1 Listener source

On macOS and Linux, the primary source is:

```text
lsof -nP -iTCP -sTCP:LISTEN -FpcuPn
```

Linux may fall back to `ss -H -ltnp`. A listener record contains PID, UID when available, process name, address, family, port, TCP protocol, and a scope of `loopback`, `wildcard`, or `lan`.

The scanner aggregates every TCP listener belonging to the same PID into one service and deduplicates repeated IPv4/IPv6 representations.

### 6.2 Process join

The scanner joins listeners to live process metadata from Rust. It may expose only redacted command text. A process model includes PID, parent PID, name, executable, working directory, creation time, live uptime, CPU, memory, and UID when available.

Listeners owned by another user are not visible. Known operating-system daemons and desktop infrastructure are classified as noise. Docker proxy/plumbing processes are classified as container infrastructure and are excluded from the Services tab.

### 6.3 Project attribution

Starting from the process working directory and valid absolute command paths, scan toward the filesystem root for the first project marker:

- `.git`
- `package.json`
- `pyproject.toml`
- `Cargo.toml`
- `go.mod`
- `pom.xml`
- `build.gradle` / `build.gradle.kts`
- `docker-compose.yml` / `compose.yml`

When possible, derive the project name from the marker's own metadata. Otherwise use the directory name. Project identity is a stable hash of the canonical root path.

### 6.4 Technology and category

At minimum, recognize Spring Boot, Next.js, Vite, Nuxt, Angular, Django, FastAPI, Flask, Rails, PostgreSQL, MySQL/MariaDB, MongoDB, Redis, Memcached, Elasticsearch, RabbitMQ, nginx, Caddy, Node.js, Deno, Bun, Python, Java, .NET, Rust, Go, PHP, Ruby, and Docker.

Categories are `web`, `api`, `database`, `cache`, `proxy`, `runtime`, and `other`. Unknown user-owned processes may be considered development services when they belong to a detected project or listen on a common development/high ephemeral port.

### 6.5 Browser destination

Only web, API, proxy, and suitable runtime services receive a browser URL. Loopback is preferred, wildcard addresses become `localhost`, IPv6 literals use brackets, and ports 443/8443 or explicit HTTPS arguments use `https`. Spring context-path arguments are appended when present.

### 6.6 Origin

Inspect the process ancestry for known agents, IDEs, and terminals. Supported labels include Agent/Claude Code/Aider, VS Code, Cursor, IntelliJ IDEA, PyCharm, WebStorm, Android Studio, Zed, Terminal, iTerm2, WezTerm, Alacritty, Kitty, Ghostty, and Konsole. Unknown ancestry remains visually quiet.

## 7. Termination safety

A stop action is available only for a current-user development process with a live process record. At action time the Rust core shall:

1. look up the immutable identity captured by the last scan;
2. reject PID 0/1 and Cutting Board's own PID;
3. verify current-user ownership again;
4. reload process metadata;
5. verify the process creation time still equals the scanned creation time;
6. send `SIGTERM` to that PID;
7. wait approximately two seconds;
8. send `SIGKILL` only if the validated process remains alive.

A changed/reused PID, missing identity, ownership mismatch, or demonstration mode must produce an error without sending a signal.

## 8. Docker tab

The Docker tab is read-only. It invokes `docker ps -a` and shows container ID, name, image, state, status, published host ports, Compose project, and Compose service. Containers group by Compose project; standalone containers appear last. Running containers precede stopped containers.

Cards use the same geometry as service cards. Stopped cards are visually muted. Clicking or pressing Enter/Space opens container details. Cutting Board does not start, stop, remove, or exec into containers.

If Docker cannot be queried, show the error and, when available, show container listener processes from the service scanner as a fallback.

## 9. Launch Profiles

### 9.1 Persistence

A profile contains:

- stable ID;
- display name, maximum 80 characters;
- absolute project root;
- one or more tasks.

A task contains a unique name, working directory (absolute or relative to the project root), shell command, and optional expected TCP port. Profiles are written atomically as local JSON.

### 9.2 Ownership

Starting a task shall:

1. validate profile/task existence and task directory;
2. reject demonstration mode;
3. reject an expected port already served by an external process attributed to the same project;
4. start `/bin/sh -lc <command>` on Unix in a new session/process group;
5. set `CUTTING_BOARD_MANAGED=1`;
6. redirect stdout/stderr to a local append-only task log;
7. record PID, start time, state, and ownership in memory.

Only owned process groups may be stopped. Stopping sends `SIGTERM` to the group, waits, then uses `SIGKILL` if necessary. Closing Cutting Board stops all active owned groups. Externally detected tasks show `Running externally` and expose neither a stop nor an ownership claim.

### 9.3 Presentation

The tab contains a header, explanatory copy, and `＋ Add`. Each profile card contains name, project path, task count, one primary group action, Edit/Delete when no task is active, and task rows.

Task states are `Stopped`, `Starting`, `Running`, `Stopping`, `Failed`, and `Running externally`, with dim, warning, green, danger, or violet semantic colors. Every task provides Logs; lifecycle controls appear only when valid.

## 10. Settings and persistence

Settings contain theme mode, scan interval (500–60,000 ms), and window geometry. Invalid or old values normalize safely. Writes use a temporary file, `fsync`, and atomic replacement. Corrupt files produce a user-visible error rather than silent data loss.

Application data belongs in the platform application-configuration directory. Cutting Board shall not write project files.

## 11. CLI

The native executable supports:

- `--demo`
- `--auto-close-seconds N`
- `--help` / `-h`
- `--version` / `-V`

Unknown options exit with status 2. Demonstration mode returns deterministic services, containers, and profiles and disables every mutation.

## 12. Build and verification

The repository shall pass on Ubuntu 24.04 and current macOS runners:

```text
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
npm run tauri build
```

CI uploads generated bundles from `src-tauri/target/release/bundle/`.

Acceptance requires no Python runtime files, Python dependency manifests, Tkinter UI, or legacy Python packaging scripts in the migration branch.

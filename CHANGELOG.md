# Changelog

## 0.1.0 — 2026-08-20

First release, targeting Ubuntu 24.04.

- Automatic discovery of TCP IPv4/IPv6 `LISTEN` sockets, swept every 2 seconds
- Endpoints of one PID collapsed into a single service
- Project inference from the process working directory and a marker walk
- Service identification from process name, argv, executable and package name
- Noise filtering: every listener is classified `dev`, `container` or `noise`,
  and only development work reaches the board
- Tile board on a near-black canvas with a single cyan accent, grouped by
  project and reflowing to the window width
- Brand marks for 61 technologies, drawn from Simple Icons (CC0 1.0)
- Docker tab listing containers grouped by compose project, with a port
  fallback when the Docker CLI cannot be reached
- Uptime and ports on the tile; everything else in a detail dialog
- Launcher attribution from process ancestry and inherited environment markers,
  badging services started by a coding agent or an IDE
- Open a web service in the default browser
- Validated `SIGTERM`, with `SIGKILL` offered only after a timeout and a second
  confirmation
- Masking of common secrets in displayed and exported command lines
- Headless snapshot CLI with plain-text and JSON output
- JSON settings under XDG, written atomically, behind a settings dialog that
  applies a new scan interval to the running scan loop immediately
- Debian package, desktop entry and application icon
- A watch-and-restart development runner, `scripts/dev.py`
- Unit tests, a live TCP integration test, a headless GUI smoke test and a
  package smoke test

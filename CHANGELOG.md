# Changelog

All notable changes to Cutting Board are documented here.

## [0.6.0] - Unreleased

### Changed

- Established the desktop application on Tauri 2, Rust, strict TypeScript, and Vite.
- Preserved the existing dark/light/system palettes, toolbar, project sections, fixed service-card geometry, technology artwork, keyboard action model, details, settings, Docker tab, and Launch Profiles layout.
- Moved TCP listener discovery, process metadata, relevance filtering, technology classification, project attribution, origin detection, Spring context-path handling, and command redaction into Rust.
- Reimplemented guarded service termination with UID and process-creation-time validation before signaling a PID.
- Reimplemented Docker enumeration as a native command with read-only Compose grouping and listener fallback.
- Reimplemented launch profiles with atomic JSON persistence, owned process sessions, task logs, external-port detection, and shutdown cleanup.
- Added Tauri bundles and Ubuntu/macOS CI verification.

### Security

- The webview now has an explicit least-privilege Tauri capability file.
- Process command arguments recognized as passwords, tokens, secrets, API keys, authorization values, or credentials are redacted before they cross the command boundary.
- Demonstration mode disables every mutation.

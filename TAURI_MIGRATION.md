# Tauri migration map

Cutting Board 0.6 replaces the Python/Tkinter runtime with a native Tauri 2 application while preserving the product and interface contract in `SPEC.md`.

| Concern | Tauri implementation |
|---|---|
| Desktop shell and permissions | `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json` |
| TCP listener and process discovery | `src-tauri/src/scanner.rs` |
| Safe process termination | `src-tauri/src/lib.rs` |
| Docker read-only discovery | `src-tauri/src/docker.rs` |
| Settings and launch-profile persistence | `src-tauri/src/storage.rs` |
| Owned launch process groups and logs | `src-tauri/src/launch.rs` |
| Typed native command client | `src/api.ts`, `src/types.ts` |
| Existing board UI and design tokens | `src/main.ts`, `src/styles.css`, `assets/` |
| Cross-platform verification and bundles | `.github/workflows/ci.yml` |

Required release checks:

```bash
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
npm run tauri build
```

The migration intentionally retains no Python runtime or sidecar dependency. Process, filesystem, Docker, signal, and launch ownership operations remain behind typed Rust commands and an explicit least-privilege Tauri capability declaration.

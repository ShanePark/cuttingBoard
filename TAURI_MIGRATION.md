# Tauri architecture map

Cutting Board is a native Tauri 2 application. The product and interface contract lives in `SPEC.md`.

| Concern | Tauri implementation |
|---|---|
| Desktop shell and permissions | `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json` |
| TCP listener and process discovery | `src-tauri/src/scanner.rs` |
| Safe process termination | `src-tauri/src/lib.rs` |
| Docker read-only discovery | `src-tauri/src/docker.rs` |
| Settings and launch-profile persistence | `src-tauri/src/storage.rs` |
| Owned launch process groups and logs | `src-tauri/src/launch.rs` |
| Typed native command client | `src/api.ts`, `src/types.ts` |
| Board UI, design tokens, and licensed runtime artwork | `src/main.ts`, `src/styles.css`, `public/icons/` |
| Cross-platform verification and bundles | `.github/workflows/ci.yml` |

Required release checks:

```bash
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --all-targets
npm run tauri build
```

The application has no external runtime or sidecar dependency. Process, filesystem, Docker, signal, and launch ownership operations remain behind typed Rust commands and an explicit least-privilege Tauri capability declaration.

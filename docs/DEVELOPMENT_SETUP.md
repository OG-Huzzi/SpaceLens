# SpaceLens — Development Setup

Reproducibility contract for this repository. Any agent or developer cloning
`https://github.com/OG-Huzzi/SpaceLens` should be able to reproduce the
environment from this document alone.

## Prerequisites

| Tool | Version used | Notes |
|---|---|---|
| Windows | 11 Pro x64 | dev machine reference |
| Git | 2.53.0.windows.2 | line endings governed by `.gitattributes` |
| Rust (GNU) | 1.98.1 `stable-x86_64-pc-windows-gnu` | installed via rustup, homes on D: |
| Node.js | 24.11.1 | |
| npm | 11.6.2 | cache redirected to D: |
| GCC (MSYS2) | 15.2.0 | needed to link `rusqlite` (bundled SQLite) |
| WebView2 | v152.0.4191.66 | Tauri runtime on Windows |
| Tauri CLI | 2.11.4 | via `@tauri-apps/cli` devDependency |

MSVC + Windows SDK are **not** installed (see limitations).

## Resource constraints (8 GB RAM, C: nearly full)

- **Never install tools on C:.** C: had <1 GB free at Phase 0.5 start; all
  SpaceLens development data lives on D:.
- **Rust homes on D:** — user env vars `CARGO_HOME=D:\.cargo`,
  `RUSTUP_HOME=D:\.rustup`, and `D:\.cargo\bin` on the user PATH.
- **npm cache on D:** — user `.npmrc` sets `cache=D:\.npm-cache`.
- **Build parallelism pinned to 2** — `.cargo/config.toml` in this repo sets
  `build.jobs = 2`. Do not raise it on 8 GB machines.
- Never relocate toolchains blindly; the configuration above is per-user and
  does not move any globally installed software.

## Commands

```sh
# Rust core (format + tests)
cargo fmt --check
cargo test -j 2 -p spacelens-core

# Frontend (typecheck + production build)
npm install        # only if node_modules is missing
npm run build      # tsc --noEmit && vite build

# Tauri
npx tauri --version
```

`src-tauri` is a config-level contract in Phase 0/0.5 and is NOT a workspace
member; it first compiles in MSVC CI (Phase 1+). See
`progress/PHASE_0_STATUS.md` and `progress/PHASE_0_5_STATUS.md`.

## Known Windows limitations

1. **No MSVC / Windows SDK** — a native `tauri build` cannot run on this
   machine. Verify locally with: fmt, `cargo test -j 2 -p spacelens-core`,
   `npm run build`, Tauri CLI + config JSON validation. Full Tauri compile and
   bundling must happen in MSVC CI.
2. **C: free space is small** — keep caches/artifacts on D:; check
   `Get-PSDrive C` before heavy installs.
3. macOS/Linux behavior is contract-only until CI runners exist.

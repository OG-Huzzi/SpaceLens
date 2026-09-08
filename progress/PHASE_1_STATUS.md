# SpaceLens — Phase 1 Status

- **Phase:** 1 — Filesystem Engine
- **Verdict:** VERIFIED (all Phase 1 acceptance criteria met; platform deferrals documented)
- **Date:** 2026-09-07 · **Machine:** Windows 11 Pro x64, 8 GB RAM · **Agent run:** Phase 1

## Implemented modules (`crates/spacelens-engine`)

| Module | Contents |
|---|---|
| `model.rs` | `FsEntry` (scan-scoped id, parent id, `PathBuf`, kind file/dir/link/other, `u64` size, `Option<u64>` allocated, Option timestamps, device/inode, hidden, error ref) — every unavailable field explicit `None` |
| `error.rs` | `ErrorCategory` (8 stable categories + IPC codes), `ScanError` (path, message, raw OS code), bounded `ScanErrorReport` (exact counts, ≤256 stored examples) |
| `options.rs` | `ScanOptions` (threads, `SymlinkPolicy`, max_depth, emit_entries, progress_interval), `default_threads()` = clamped 2..4 |
| `cancel.rs` | `CancelHandle` — atomic flag, clone-shared, idempotent |
| `progress.rs` | `ScanEvent::Started/Entry/Progress/Completed/Cancelled/Failed`, `ProgressSnapshot`, `Phase` — typed, throttled ≤4/sec |
| `summary.rs` | `ScanSummary` (counts, bytes, allocated, error report, status, timestamps, depth_capped), `ScanStatus::{Completed\|Cancelled\|Failed}` |
| `platform/` | `PlatformFs`, `DriveInfo`, `SysDirs` traits; `StdFs` (shared std impl); `unix.rs`/`windows.rs` per-OS helpers; `drives.rs` (Windows Win32 volume listing via `windows-sys`; Linux `/proc/mounts` w/ octal unescape; macOS root-only) |
| `scanner.rs` | shared streaming walker — bounded worker pool, dir-task queue with pending-counter termination, 1024-slot entry channel with backpressure, per-entry cancel checks, record-only link policy |

## Important design decisions

1. **New crate, not a core submodule.** The engine stays DB-free and UI-free
   per docs/ARCHITECTURE.md (`scanner` is the dependency-chain bottom).
2. **Streaming channel (backpressure) instead of buffering.** Memory is flat
   against file count; no whole-tree collection ever exists.
3. **Exactly-once emission.** A bug found by the stress test (dir entries
   emitted twice — once at discovery, once at task processing) was fixed by
   emitting each directory exactly once at discovery and carrying only its id
   in the task.
4. **Record-only links as default** (`SymlinkPolicy::RecordOnly`): cycles are
   structurally impossible; follow-with-guard is reserved for future opt-in.
5. **Threads default to 2–4, pinned by `.cargo/config.toml` jobs=2** — safe on
   8 GB RAM (docs/PERFORMANCE.md).
6. **Per-OS behavior strictly behind traits** — zero `cfg!(target_os)` in
   shared scanner code; only `platform/` implementations branch.
7. **`windows-sys` (Microsoft), not `windows-rs`** — thin, auditable FFI;
   the dependency graph stays minimal.

## Dependency discipline

- Added deps: `serde` (serialization), `windows-sys 0.59` (Windows-only FFI
  declarations), dev `tempfile` + `serde_json` (tests). Nothing else; std
  solves the rest. No `libc` (errno mapping via std `io::ErrorKind` avoids
  divergent errno ranges), no `rayon`, no async runtime.

## Tests executed (real commands + results)

- `cargo fmt --check` → clean (FMT:0).
- `cargo clippy -j 2 --workspace --all-targets` → clean (CLIPPY_EXIT:0; the one
  warning found was fixed and re-verified).
- `cargo test -j 2 --workspace` → **48 passed, 0 failed, 1 ignored**:
  - spacelens-core: 6/6 (contract + db — Phase 0 regression green).
  - engine unit: 11/11 (model serialization ≥4 GiB, error codes/report bounds,
    cancel idempotence, options defaults, drives/sysdirs).
  - traversal_tests: 6/6 (nesting/bytes/parents, empty dirs, hidden, Unicode
    + special names, 300-deep nesting, depth cap).
  - error_tests: 6/6 (permission walls, vanishing file, typed root failure ×2,
    raw OS code preserved).
  - link_tests: 5/5 (cycle a→b / b→a terminates, self-link, broken link,
    no recursion through dir links, relative target resolution).
  - cancel_tests: 5/5 + 1 perf-ignored (cancel-before-start, cancel-mid-scan
    deterministic, exactly-one-terminal + monotonic progress, >4 GiB sizes,
    file-root scan, 3k-entry stress no-loss/no-dup/no-orphan).
  - real_fs_tests: 7/7 (temp-fixture trees: nested metadata, empty/hidden,
    Unicode/space, >260-char long paths — passes on Windows, symlink
    record-only incl. broken links, >4 GiB logical file, progress invariants).
- Perf smoke (explicit `-- --ignored --nocapture`): 10,000 files / 21 dirs in
  ~249 ms (~40k files/sec) on the dev box; correctness asserted, timing printed
  (docs/TESTING_STRATEGY.md forbids wall-clock pass gates).
- `npm run build` → tsc clean + vite 27 modules, exit 0 (Phase 0/0.5
  frontend regression).

## CI

`.github/workflows/ci.yml` added (was absent): matrix
`ubuntu|windows|macos` — `cargo fmt --check`, `cargo test -j 2 --workspace`
(bounded, 8 GB-safe), perf smoke `-- --ignored --nocapture`; plus a frontend
job (`npm ci`, `npm run build`). Native Tauri compilation intentionally NOT
attempted yet (bundle icons / MSVC contract; documented in docs/SCANNER.md).

## Safety

- **Traversal:** streaming/bounded; nested/empty/deep/stress correct.
- **Symlinks/junctions:** record-only default; cycles and self-links cannot
  recurse (structurally impossible) — tested with a→b→a, self-links, broken
  links, dir-links.
- **Errors:** permission-denied dir/file recorded + tallied, scan continues;
  vanished entries typed; missing/unreadable root → `ScanStatus::Failed`.
- **Cancellation:** engine-level (not a UI flag); typed final state; tested
  before-start and mid-scan; no panics, no leaked threads.
- **No destructive ops** anywhere (no delete/rename/move/shell-out).

## Architecture (Phase-1 gate)

- Platform behavior abstracted behind `PlatformFs`/`DriveInfo`/`SysDirs`;
  scanner code has zero OS branches.
- UI untouched (zero frontend edits; npm build green).
- Engine offline: no network deps, no telemetry, no path logging.
- SQLite stays Rust-owned; the engine is DB-free.
- Classifier/hasher/recommender boundaries untouched and clean.

## Performance (Phase-1 gate)

- Bounded concurrency (worker pool; one task per directory, never per file).
- Memory flat: streaming channel with backpressure, no whole-tree buffering.
- `u64` byte accounting verified at 4 TiB (fake platform) and >4 GiB (real
  fixture).
- Measured: 10k files in ~249 ms on the dev machine (fixture + machine +
  method recorded in docs/SCANNER.md).

## Known limitations / follow-ups (honest)

1. **CI results pending.** Workflow pushed for first run; local verification
   is Windows-only by necessity.
2. **Unix volume capacity** (`statvfs`) and **macOS mount table**
   (`getmntinfo`) need a future `libc` dependency — deferred; Linux lists
   mounts without capacity, macOS reports root only.
3. **Windows reparse-tag classification** (junction vs symlink vs mount) —
   Phase 2+ refinement; behavior is identical (record-only).
4. **Windows allocated size** (`GetFileInformationByHandle` FFI) — deferred;
   Unix reports blocks×512 already.
5. **>4 GiB real-file test** may SKIP on near-full disks (dev D: had space;
   NTFS reserves the extent on `set_len`; CI on large disks exercises it).
6. **Symlink follow-modes** with dev/ino guard — future, opt-in only.
7. **DB persistence of scan records** deferred until the Tauri service layer
   exists (keeps the engine DB-free per the Phase-1 boundary).
8. **C: free space fluctuated 6.6 GB → 1.5 GB** during this run — post-hoc
   audit confirmed SpaceLens artifacts are all on D: (CARGO_HOME, RUSTUP_HOME,
   root `.cargo` cache, `target/`, npm cache); `C:\Users\DELL\.rustup` is an
   empty 0 MB stub; the drop is from external system processes (OS updates /
   Defender), outside the repo's control. C: remains small; the D:-redirect
   configuration from Phase 0.5 is intact and re-verified.

## Phase-1 acceptance gate (all checked)

- Scanner: real traversal ✓ streaming/bounded ✓ nested ✓ metadata ✓ >4 GiB ✓.
- Safety: explicit link policy ✓ no infinite recursion ✓ permission errors
  recovered ✓ disappearing files recovered ✓ zero destructive ops ✓.
- Control: cancellation ✓ typed progress ✓ typed completion ✓ typed errors ✓.
- Architecture: platform abstraction ✓ UI has no fs code ✓ engine offline ✓
  SQLite Rust-owned ✓ clean downstream boundaries ✓.
- Performance: bounded concurrency ✓ flat memory ✓ no per-entry task
  explosion ✓ no whole-tree buffering ✓.
- Tests: deterministic fixtures ✓ edge cases ✓ cancellation ✓ errors ✓ large
  values ✓.
- CI: workflow added for win/linux/mac ✓ (first remote run pending).

## Exact follow-up handoff

- Await external audit of this phase + first CI matrix results.
- Phase 2 = storage analysis + intelligence (classifier rule engine v1).
- Phase 2 must not start without explicit authorization.

## Commit SHA

Phase 1 implementation: `d5c625e` · env note: `05887d1` · **Verification gate fix: `8ef3563`**

## Cross-platform verification (Final Verification Gate, 2026-09-08)

- **Defect found by CI:** the Unix-only code in
  `crates/spacelens-engine/src/platform/unix.rs` used
  `io::ErrorKind::FilesystemLoop`, which is an **unstable** std variant
  (feature `io_error_more`) not available on stable Rust. This file is
  excluded by `#[cfg(unix)]` on Windows, so the local (Windows-only) build
  never compiled it. Both initial CI runs failed on Unix exactly here:
  - Run #1 (`d5c625e`): Ubuntu job FAILED (`cargo test` step, exit 101).
  - Run #2 (`05887d1`): macOS job FAILED (`cargo test` step, exit 101).
  - Windows passed both times (incl. tests + perf smoke) — correctly
    exercising the Windows-only code paths.
- **Fix (`8ef3563`):** removed the `FilesystemLoop` arm; ELOOP now degrades
  to `ErrorCategory::Other` (it cannot occur under lstat semantics). The
  mapping is documented in-code.
- **Local cross-target verification (this Windows box, GNU Rust 1.98.1):**
  - `cargo check --target x86_64-unknown-linux-gnu -p spacelens-engine` → clean
  - `cargo check --target x86_64-unknown-linux-gnu -p spacelens-engine --tests` → clean
  - `cargo check --target x86_64-apple-darwin -p spacelens-engine --tests` → clean
  (rustup targets installed locally; Unix code paths + tests now compile-verified.)
- **Local regression (Windows):** `cargo fmt --check` 0 · `cargo clippy -j 2
  --workspace --all-targets` 0 · `cargo test -j 2 --workspace` 48 passed /
  0 failed / 1 ignored · perf smoke 10k files ≈ 251 ms (~39.8k files/sec) ·
  `npm run build` exit 0.
- **GitHub Actions run #3 (`34195483429`) on `8ef3563` — ALL GREEN:**
  - `rust (windows-latest)` → **success** (fmt ✓, tests ✓, perf smoke ✓)
  - `rust (ubuntu-latest)` → **success**
  - `rust (macos-latest)` → **success**
  - `frontend` → **success** (npm ci + npm run build)
  - Workflow conclusion: **success** (fail-fast: false; all 4 jobs required).
- **Final hostile audit (source):** all `cfg!`/`#[cfg(...)]` OS branching is
  confined to `crates/spacelens-engine/src/platform/` — zero OS branching in
  shared scanner code; no network/destructive/shell code anywhere in the
  engine (2 false-positive doc-comment hits for the word “shell” — “never
  shell-interpolated”, “Tauri shell”); no secrets; paths via Path/PathBuf.

### Phase 1 verdict after the gate: **VERIFIED**

## Independent re-verification (2026-09-08, second agent run)

A fresh agent run re-verified every claim above from scratch (forensic
inspection of git state + full source first, then re-execution). Results:

- `cargo fmt --check` → exit 0.
- `cargo clippy -j 2 --workspace --all-targets -- -D warnings` → exit 0.
- `cargo test -j 2 --workspace` → **48 passed / 0 failed / 1 ignored**
  (6 core + 11 engine unit + 6 cancel + 6 error + 5 link + 7 real-fs +
  7 traversal).
- Perf smoke (`cargo test -j 2 -p spacelens-engine -- --ignored --nocapture`)
  → passed; 10k files in 685 ms (~14.6k files/sec) under concurrent load —
  timing is machine/load-dependent and never a pass gate (correctness is
  asserted inside the test and passed).
- `npm ci` → exit 0 (0 vulnerabilities). `npm run build` → exit 0
  (tsc clean + vite 27 modules).
- **GitHub API check of run #3 (`34195483429`) on `8ef3563`:** status
  `completed`, conclusion `success`; all 4 jobs `success` — `frontend`,
  `rust (macos-latest)`, `rust (ubuntu-latest)`, `rust (windows-latest)` —
  each with Format/Tests/Perf-smoke steps green (verified per-job via the
  Actions jobs API, not from prior reports).
- Security re-audit (grep-based): no network/telemetry/process-spawn/
  destructive APIs anywhere in the engine; no secrets in tracked files
  (remaining grep hits are doc comments and synthetic test fixture names).
- Working tree at audit time: only this documentation change.

No code changes were needed. Verdict stands: **VERIFIED**.
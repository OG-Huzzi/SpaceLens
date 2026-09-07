# SpaceLens — Scanner Architecture (Phase 1)

Status: Phase 1. Describes the filesystem engine as implemented in
`crates/spacelens-engine`. Extends, does not replace, `docs/ARCHITECTURE.md`.

## Where the scanner sits

```
React/TS UI (untouched in Phase 1)
        ↕ typed IPC (Phase 8/9)
Tauri shell (not compiled yet — MSVC CI)
        ↕
spacelens-engine   ← this phase (pure; no DB, no UI, no network)
        ↕ platform traits
StdFs / DriveInfo / SysDirs impls (std + thin windows-sys FFI)
```

The engine is the bottom of the dependency chain (docs/ARCHITECTURE.md):
`scanner → classifier → recommender → planner → safety → executor`. It knows
nothing of categories, recommendations, SQLite, or the UI. Later phases plug
in downstream without touching it.

## Modules

| Module | Responsibility |
|---|---|
| `model` | `FsEntry`, `EntryKind`, `LinkInfo` — normalized, `u64` sizes, explicit `Option` for unavailable fields |
| `error` | `ErrorCategory` (stable codes), `ScanError`, bounded `ScanErrorReport` |
| `options` | `ScanOptions`, `SymlinkPolicy`, conservative thread defaults |
| `cancel` | `CancelHandle` — atomic cooperative cancellation |
| `progress` | `ScanEvent`, `ProgressSnapshot`, `Phase` — typed, throttled |
| `summary` | `ScanSummary`, `ScanStatus` (Completed/Cancelled/Failed) |
| `platform` | `PlatformFs`, `DriveInfo`, `SysDirs` traits + per-OS impls |
| `scanner` | the shared streaming walker (bounded concurrency) |

## Traversal policy

- Streaming: entries are delivered to the caller through a bounded channel
  (1024-slot, backpressure via blocking sends). The engine never buffers file
  entries; memory stays flat against file count.
- Work model: fixed pool of `ScanOptions::threads` workers (default
  `min(2..4, cpus)`) pulling directory tasks from a shared queue. One task per
  *directory*, never per file. Termination: `pending == 0` (queued + in-flight
  == 0), no polling, no busy-wait.
- Depth: unlimited by default (safe once links are not followed — the
  directory graph is a finite DAG); an explicit `max_depth` cap is available
  and flags `summary.depth_capped`.

## Symlink / junction / reparse-point policy

**Default (`SymlinkPolicy::RecordOnly`): links are recorded, never followed.**

- A link is emitted as `EntryKind::Link` with its target (as the OS reports
  it — may be relative) and a `broken` flag (target stat → NotFound).
- Windows junctions/mount points/symlinks all carry the reparse bit; they are
  classified `LinkKind::Reparse` (precise tag identification is a Phase-2+
  refinement; behavior is identical).
- Consequence: filesystem cycles cannot cause infinite traversal — there is
  no recursion through links at all. Tested with a→b→a cycles, self-links,
  and broken links (fake platform + real-fs fixtures).
- Follow-modes (with dev/ino cycle guards) are deliberately NOT implemented
  in Phase 1; the policy enum is where they will land.

## Error handling

- Per-entry failures are recovered, categorized, tallied, and the scan
  continues. Categories: `PermissionDenied`, `NotFound`, `InUse`,
  `BrokenLink`, `MetadataUnavailable`, `Unsupported`, `Transient`, `Other` —
  each with a stable IPC code (e.g. `PERMISSION_DENIED`, `ENTRY_IN_USE`).
- Raw OS error codes are preserved on the `ScanError` for diagnosis.
- Category mapping is platform behavior and lives behind
  `PlatformFs::categorize_error` (Windows: sharing violation 32/33 → `InUse`;
  Unix: std `io::ErrorKind` + the three universal errnos).
- `ScanErrorReport` keeps exact per-category counts but caps stored examples
  (256) — a hostile tree cannot balloon memory.
- Root-level failures: missing/unreadable root → `ScanStatus::Failed` (typed),
  everything else → the scan completes with errors tallied.

## Concurrent modification

Files/dirs may vanish, appear, or lock mid-scan. Listing→stat gaps produce a
typed per-entry error state (`entry.error`) instead of a panic or a failed
scan. A directory listing is never assumed to remain valid.

## Cancellation

`CancelHandle` (atomic flag, clone-shared). Checked at every queue pop, every
child entry, and between workers' tasks. Cancellation produces
`ScanStatus::Cancelled` + a `Cancelled` terminal event; it never panics
workers, never corrupts anything (the engine holds no DB state). Cancel
before start short-circuits deterministically. Tested: before start, mid-scan
(deterministic trigger from the event stream).

## Progress semantics

Typed `ScanEvent::{Started, Entry, Progress, Completed, Cancelled, Failed}`.
The stream always begins with `Started` and ends with exactly one terminal
event. `Progress` snapshots are throttled to `progress_interval` (default
250 ms ⇒ ≤4/sec, the docs/PERFORMANCE.md cap) with one always-final snapshot.
Counters: files/dirs/bytes/errors seen + elapsed ms. No human-readable
strings cross this boundary.

## Platform boundaries

- Shared scanner code contains zero OS branches (enforced by review; tests
  are generic over `PlatformFs`).
- Windows: std + `windows-sys` (FFI declarations only) for volume listing
  (drives, capacity, fs type, volume serial), reparse/hidden attributes via
  std `MetadataExt`. Long paths: verified working via std's verbatim handling
  (test creates a >260-char fixture path).
- Linux: `/proc/mounts` parsing (octal escapes decoded), pseudo-filesystem
  filter list. Capacity: `None` (statvfs needs `libc` — owned follow-up).
- macOS: no std-reachable mount table without `libc`; reports only the root
  volume honestly. Engine compiles + tests on macOS CI.
- `Trash` trait intentionally absent (belongs to cleanup phases).

## CI

`.github/workflows/ci.yml`: matrix Windows/Linux/macOS — `cargo fmt --check`,
`cargo test -j 2 --workspace`, and the 10k-file perf smoke (correctness
asserted; timing printed, never a pass gate). Frontend job: `npm ci` +
`npm run build`. Native Tauri compilation is NOT attempted yet (bundle icons
do not exist until packaging; shell remains a config contract).

## Security / privacy

The engine performs no network I/O (only dependencies: `serde`, and
`windows-sys` on Windows). No telemetry, no logging of scanned paths. Paths
are data — never executed, never shell-interpolated; the scanner shells out
to nothing.

## Measured (fixture, dev machine, GNU Rust 1.98.1)

- 10k-file fixture (20 dirs × 500 files): ~250 ms (~40k files/sec) on the
  Phase-0.5 dev box (Windows 11, NVMe-class SSD). Measured via the committed
  perf-smoke test; every perf claim cites fixture + machine + commit.
- No budget is enforced in CI yet (CI runners' variance); the smoke asserts
  correctness at scale and reports timing.

## Known limitations / follow-ups

1. Unix volume capacity (statvfs) requires `libc` — deferred.
2. macOS mount table needs `libc` (getmntinfo) — deferred; root-only listing.
3. Precise Windows reparse-tag classification (junction vs symlink vs mount)
   — Phase 2+ (app attribution needs it).
4. Allocated size on Windows requires `GetFileInformationByHandle` FFI —
   deferred; Unix reports blocks×512 already.
5. Symlink follow-modes with cycle guards — future, opt-in only.
6. DB persistence of scan records — deferred until the Tauri service layer
   exists (keeps the engine DB-free per the Phase-1 boundary).


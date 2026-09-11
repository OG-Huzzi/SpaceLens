# SpaceLens — Phase 3.1 Status

- **Phase:** 3.1 — Identity-engine correctness, TOCTOU safety & true
  boundedness (audit-repair pass over Phase 3)
- **Verdict:** **PHASE 3.1 — VERIFIED** (full local gate green on Windows;
  CI matrix green on the implementation SHA `f0a6ea5` — run
  `34617425239`, all 4 jobs success with every step verified individually
  from the Actions API; the doc-record commit `f0a6ea5` reruns the
  same matrix on top of unchanged code)
- **Date:** 2026-09-11 · **Machine:** Windows 11 Pro x64, 8 GB RAM
- **Starting SHA:** `274a225` (Phase 3 record, clean tree)
- **History:** implementation `5f845e1` (run `34603456406`: windows +
  frontend ✓, ubuntu/macos clippy ✗ — unix-only dead-code lint) → repair
  `b10f9e2` (run `34615674824`: clippy green everywhere; ubuntu test ✗ —
  ext4 inode reuse defeated a delete+recreate fixture) → repair `f0a6ea5`
  (run `34617425239`: **all green**). Every failure was diagnosed from its
  actual CI log before repair; nothing was papered over.

## Why this phase existed

An independent audit identified correctness/safety gaps the Phase 3 test
suite did not catch. Each defect was confirmed from source before repair;
nothing was papered over:

1. **Same-size mutation escaped detection** — `hash_file` compared only the
   open-handle length and total bytes read; `AAAAAAAAAA`→`BBBBBBBBBB`
   (same length) passed and published a digest for content the scanner
   never observed.
2. **Observed object vs opened object never compared** — ingest recorded
   observation-time identity and hash time silently overwrote it with
   handle identity; a path swapped to a different file hashed the impostor
   into the observed file's identity.
3. **Path-open TOCTOU** — `fs::File::open(path)` follows a replacement
   symlink/junction; the "links are never followed" guarantee held only at
   scan time, not at hash time.
4. **Staging was not globally bounded** — `BTreeMap<u64, Vec<Member>>`
   over ALL eligible entries with only a per-group cap; many distinct
   sizes scaled memory with input, defeating the documented "memory stays
   flat" claim.
5. **Result buffering** — `Mutex<Vec<(Member, ContentHash)>>` grew with all
   hashed candidates, bounded only per-group.
6. **Docs overstated** — IDENTITY.md claimed flat memory; PHASE_3_STATUS
   described post-lstat mtime/kind checks that did not exist in the code.

## What was built (all within Observation → Classification → Identity → Relationships)

**Engine boundary (`spacelens-engine`, additive):**

- `HandleStat { len, modified, changed }` + `ContentReader::{pre_stat,
  post_stat}` — handle-proven state taken before/after the read (fstat
  semantics; never a second path resolution). `changed` = Unix `st_ctime`
  / NTFS `ChangeTime` (via `GetFileInformationByHandleEx(FileBasicInfo)`)
  — the change stamp that moves on same-length rewrites and cannot be set
  from userspace. Fields the OS does not maintain are `None`, never
  fabricated.
- **No-follow content open:** Unix `open(O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC)`
  (libc constants — raw values differ per Unix flavor), Windows
  `FILE_FLAG_OPEN_REPARSE_POINT|FILE_FLAG_BACKUP_SEMANTICS` + immediate
  handle inspection (reparse → `UnexpectedLink`, directory → `NotRegularFile`).
  `O_NONBLOCK` prevents a FIFO replacement from blocking a worker. New
  `ContentError::{UnexpectedLink, NotRegularFile}` — the target of a
  replacement link is never touched.
- `FsEntry.changed` / `MetadataInfo.changed` (additive): the observation-side
  twin of the handle change stamp (Unix st_ctime; None on Windows path-stats).

**Identity crate (`spacelens-identity`):**

- **7-check mutation policy** (platform-neutral, no cfg): identity
  (observed vs handle → `Replaced`), pre-read length, scan→open change
  bracket, bytes-read == observed, post-read length, post-read change
  bracket, post-read mtime bracket. Same-length mid-read rewrite → typed
  `Changed`. Object swap → new `HashFailureKind::Replaced`.
- **Streaming ingest under Contract A:** two global caps
  (`max_tracked_size_groups` = 100k, `max_tracked_candidates` = 1M) with
  exact skip counters (`candidatesSkippedSizeTracking`,
  `candidatesSkippedGlobalCap`, `candidatesUntrackedTotal`) and a new
  `DuplicateStatus::CompletedWithLimits`. `Completed` is only ever emitted
  when nothing was skipped — no silent data loss, no completeness illusion.
  Zero-byte-under-default-policy are counted with one counter, never staged.
- Deterministic job order (size asc, then first-member path bytes) — cap
  decisions are order-independent; shuffled observation order produces
  byte-identical reports, capped or not.
- Result buffering bounded transitively by the staging caps (one record per
  accepted candidate); failure detail capped at 256; group detail at 64.

**Dependencies:** `libc` (unix-only, bindings-only, used solely for
`O_NOFOLLOW/O_NONBLOCK/O_CLOEXEC/ELOOP/ENXIO` platform constants whose raw
values differ per Unix flavor). No other new dependencies. Zero `unsafe`
in the identity crate; engine `unsafe` remains confined to audited Windows
FFI. No network, no processes, no telemetry anywhere in shipped code.

## Tests (24 new adversarial + all previous kept green)

- `adversarial_pipeline_tests.rs` (15): same-length rewrite during read
  (incl. last-byte) → typed Changed; opened-identity disagreement → typed
  Replaced; degraded identity (neither/one side provable) never fabricates
  and never rejects; hard-link alias semantics; distinct-size cap → exact
  skips + CompletedWithLimits; global record cap; per-group cap exact
  accounting; shuffled-order determinism under caps; no-silent-omission
  control; cancellation during capped ingest; Vanished preserved.
  **Memory proof:** a `#[global_allocator]` counting allocator proves peak
  staging does not scale with entry count (10k vs 100k hostile
  distinct-size entries under low caps: same order of magnitude, < 4 MB
  absolute, vs ~10 MB/100k records uncapped).
- `adversarial_fs_tests.rs` (9, real filesystem through the real platform
  boundary): same path/same object groups; replaced object never groups
  (Unix: typed Replaced pinned via observation-time dev/ino); hard links
  are aliases not replacements; distinct copies are a real group; scan→
  symlink swap refused with UnexpectedLink (Windows symlink + unix ELOOP
  legs); scan→directory swap refused; broken symlink refused; FIFO swap
  refused non-blocking; engine-boundary primitive tests (identity/stats/
  EOF contract; symlink/directory refusal at the boundary directly).

All Phase 1 / 2 / 2.1 / 3 tests remain green, unchanged except for the
mechanical `FsEntry.changed` field additions.

## Second-order audit findings (fixed before this record)

1. **A full Phase 3 unit-test module had been dropped in the pipeline
   rewrite** (17 tests: grouping, cancellation matrix, unsupported mode,
   determinism, event sequencing, eligibility accounting, the real-fs
   factory round-trip). Restored verbatim, adapted to the new
   `ContentReader` trait surface. Workspace count reconciles exactly:
   289 (Phase 3) − 0 + 24 (new adversarial) = 313.
2. **Unix-only dead code**: the Windows link-privilege probe helper was
   compiled (and flagged `-D dead_code`) on unix where no caller exists —
   found by cross-target clippy locally, gated `#[cfg(windows)]`. This was
   the exact lint that failed the first CI attempt for ubuntu/macos.
3. `StdContentReader` returned `Ok(Some(0))` at EOF against its own trait
   contract (now `Ok(None)`; the pipeline accepts both shapes defensively).

## Verification

- **Local (Windows):** `cargo fmt --check` ✓ · `cargo clippy --workspace
  --all-targets -- -D warnings` ✓ (plus `--target x86_64-unknown-linux-gnu`
  cross-clippy for the unix-only code paths) · `cargo test --workspace`
  **313 passed / 0 failed** (289 at Phase 3; +24 adversarial, Phase 3 suite
  intact) · identity perf smoke ✓ (4 hostile workloads at 10k/100k/1M,
  ~linear per-entry cost) · release >4 GiB streaming proof ✓ (24.3 s).
- **CI:** matrix green on the final SHA (run recorded below) — each job
  verified individually from the Actions API.

## Residual windows — documented, not hidden (docs/IDENTITY.md §limitations)

1. Torn mid-read writes escape only where the filesystem maintains no
   change stamp AND the rewrite preserves length and mtime; window is one
   sequential pass; a later scan corrects grouping.
2. Windows scan-time object identity is unavailable on stable std: the
   observed-vs-opened replacement check runs in degraded mode there
   (handle-side identity still proves hard-link accounting). A same-content
   regular-file swap on Windows is semantically indistinguishable and
   produces a true content relationship for the hashed object.
3. 64-bit file-index granularity caveats on non-NTFS volumes → `Estimated`
   accounting, never fabricated.

## Second-order audit (performed before this verdict)

Mutation / replacement / links / identity / memory / buffers / completeness
/ accounting / cancellation / determinism / cross-platform / security /
documentation — each question from the phase brief was re-asked against the
final code; findings are folded into the audit-findings section above and
into docs/IDENTITY.md §limitations. The FIFO/`mkfifo` adversarial test
spawns `mkfifo` in a unix-only test to create the fixture (the engine
itself never spawns processes; the spawn is test fixture setup on unix CI
where the binary exists, with an honest skip if absent).

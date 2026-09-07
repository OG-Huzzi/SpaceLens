# SpaceLens — Performance Strategy

Status: Phase 0. Budgets are targets to design against, measured from Phase 1.
No premature optimization; no unmeasured claims.

## Scale targets (v1)

- 100k files: interactive, first results streaming in well under a minute on
  SATA SSD; UI fully responsive throughout.
- 1M files: complete scan on NVMe in low minutes; progress smooth, cancel instant.
- HDDs / external / partially-inaccessible: correct before fast; degraded-mode
  messaging when the device is the bottleneck.
- Memory: bounded regardless of tree size (streaming walk, batched DB writes;
  no holding the whole tree in RAM). Target: steady-state scan memory flat
  against file count (exact cap set after Phase 1 profiling).

## Strategies (designed now, implemented per phase)

- **Parallel walk, bounded concurrency.** Worker pool sized to device class
  (conservative on HDD — parallelism hurts spinning disks; wider on NVMe).
  I/O depth auto-tuned; never unbounded thread-per-dir.
- **Batched persistence.** Metadata buffered and committed in batches inside
  long transactions; UI reads via WAL snapshots, never blocked by writers.
- **Incremental rescan.** mtime/size journal per directory; unchanged subtrees
  revalidated cheaply instead of re-walked. Full rescan always available.
- **Hash discipline.** Hash only duplicate *candidates* (same size, ≥2 files),
  largest-first for early wins; persistent hash cache keyed on
  (path-id, size, mtime); never re-hash unchanged content; never hash cloud
  placeholders.
- **Streaming progress + cancel.** Progress events throttled (e.g. ≤4/sec) with
  files/sec + ETA; cancellation is cooperative-checked at tight intervals and
  must settle within ~1s, leaving the DB consistent (partial scan marked partial).
- **UI responsiveness.** All engine work off the UI thread; virtualized lists;
  category aggregates precomputed in SQL, not in JS.

## What we explicitly defer

NTFS MFT fast path (WizTree-level speed) is a Phase-10 optimization, not v1
scope: it buys Windows-only speed at the cost of platform parity and risk.
v1 wins on clarity and safety; speed must be *acceptable*, then excellent.

## Measurement (from Phase 1)

- Criterion-style Rust benches for walk/classify/hash on synthetic fixtures
  (100k / 1M generated trees, committed generators so results reproduce).
- Per-platform CI perf smoke: fixture scan must complete within budget or the
  run fails with a profile attached.
- Real-drive profiling is manual, opt-in, on maintainer hardware — never in CI,
  never on user data.
- Every perf claim in docs or marketing cites the fixture, device class, and
  commit. Unmeasured numbers are banned from the codebase and the copy.

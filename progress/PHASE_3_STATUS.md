# SpaceLens — Phase 3 Status

- **Phase:** 3 — File Identity, Hashing & Duplicate Relationships (the RELATE layer)
- **Verdict:** **PHASE 3 — VERIFIED** (CI run `34454636871` for `e453dd1`:
  all 4 jobs success — rust windows/ubuntu/macos + frontend, each verified
  individually from the Actions API)
- **Date:** 2026-09-10 · **Machine:** Windows 11 Pro x64, 8 GB RAM
- **History:** implementation `46b5dd1` (run 12: macos clippy + windows
  sparse-test failures) → repair `929e400` (run 13: unix root-entry count
  failures) → repair `15e3365` (run 14: workflow YAML broke parsing — zero
  jobs) → repair `e453dd1` (run 16: **success**). Every failure was
  diagnosed from its actual CI log before repair; nothing was papered over.

## What was built

**Engine boundary (`spacelens-engine`, additive):** `PlatformFs::read_content`
— the single content-access boundary. Streaming `ContentReader` with
handle-proven `FileIdentity` (Unix fstat / Windows
`GetFileInformationByHandle`, so hard-link semantics are provable on all
three CI platforms), typed `ContentError::{OpenFailed, ReadFailed, Aborted}`,
consumer-abort vs consumer-check-failure kept distinct. The identity layer
never crawls the filesystem; it consumes observed `FsEntry` values.

**Identity crate (`spacelens-identity`, new):** contract namespace
`spacelens.v1.identity.*`.

- **Content identity = SHA-256** (`sha2 0.10`, RustCrypto; pure Rust,
  MIT/Apache, maintained). `ContentHash([u8; 32])` strong type; identity is
  meaningless without its algorithm tag. Path, name, and metadata are never
  identity.
- **Pipeline:** ingest → eligibility contract (observer-proved regular files
  only) → size grouping (same size ⇒ candidacy, never equality; singletons
  never hashed) → bounded worker pool (bounded channel, bounded candidates
  per group) → mutation-checked streaming hashing (open+fstat length +
  bytes-read checks; per-chunk cancel checks; else typed
  `Changed`/`Vanished`, never a false relationship) → deterministic grouping
  and ordering (size asc, then hash bytes asc).
  *(Phase 3.1 correction of this record: the "pre-lstat / post-lstat
  kind+size+mtime match" wording described checks that were not in the
  shipped code. The 3.1 hardening implemented the full bracket contract —
  see PHASE_3_1_STATUS.md and docs/IDENTITY.md §mutation policy.)*
- **Honest storage accounting:** `logical_duplicate_bytes` always exact;
  `recoverable_bytes = size × (distinct_objects − 1)` only with
  `StorageAccounting::Exact` (every member's handle identity known); hard-link
  aliases are members with zero recoverable bytes.
- **Zero-byte policy:** counted, not grouped, by default; explicit opt-in
  groups them under the well-defined empty identity.
- **Links are never followed** — the eligibility contract rejects everything
  the observer did not prove to be a regular file; no second traversal
  exists.
- **Purity:** zero `unsafe`, no I/O surface in the crate beyond the engine
  boundary, dependencies = serde + sha2 + spacelens-engine. No DB, no UI, no
  recommendations, no deletion — nothing beyond Observation → Classification
  → Identity → Relationships.

**Tests (53 new):** SHA-256 known-answer vectors; empty/same-content/
same-name-different-content/same-size-different-content; real-fs hard links
(one object, zero recoverable); symlinks never followed/grouped; mutation &
vanish typed; permission-denied typed; zero-byte policy both modes; hostile
storms (many groups, same-size storm, many zero-byte); determinism;
cancellation; >4 GiB streaming proof (release-mode, virtual 10 GiB content,
O(1) memory); u64 chunk-boundary grouping (always-on); perf smoke 10k/100k/1M
across 4 hostile workloads with linear-scaling guard (CI-enforced).

## Defects found and fixed during verification (the honest record)

1. **Consumer-abort mislabeling (pre-commit):** consistency-check errors from
   the read callback were converted to `ContentError::Aborted` by the engine
   boundary, indistinguishable from deliberate cancellation. Split into
   distinct error kinds; covered by `file_changed_between_scan_and_hash…`.
2. **Unnecessary `unsafe` (second-order audit):** a trait-object cast for the
   worker pool when the trait already declared `Send + Sync` supertraits.
   Removed; the workspace now contains zero `unsafe` in Phase 3 code.
3. **Unix root-entry test failures (CI runs 12, 13):** the scanner
   intentionally emits the scanned root as an entry; Unix hosts saw one more
   entry than Windows hosts (where the sparse test silently skipped because
   `set_len` fails on this host's volume). Test helper now filters the root —
   assertions test engine semantics, not the host.
4. **Sparse-test hygiene (run 12):** the >4 GiB real-fs test physically
   copied files (non-sparse) and hashed 10 GiB in debug (~6 min on macOS).
   Real-fs case shrunk to 256 MiB; the true >4 GiB proof is a release-mode
   virtual test (10 GiB of patterned content, no disk, ~25 s ≈ SHA-256
   floor), CI runs it as its own step; an always-on u64 chunk-boundary test
   covers size plumbing cheaply everywhere.
5. **Workflow YAML (run 14):** a colon+space in a step name made GitHub
   reject the workflow (zero jobs). Fixed; parse verified locally before
   push (lesson recorded: validate workflow YAML in the pre-push gate).

## Verification

- **Local (Windows):** `cargo fmt --check` ✓ · `clippy --workspace
  --all-targets -D warnings` ✓ · `cargo test --workspace` **289 passed /
  0 failed / 2 ignored** (all Phase 1/2/2.1 suites still green — nothing
  removed or weakened) · Phase 1/2/3 perf smokes ✓ · release >4 GiB proof ✓
  (25.8 s ≈ 800 MB/s, linear scaling 10k→1M on all workloads).
- **CI:** run `34454636871` for `e453dd1` — **success**, all 4 jobs verified
  individually: `rust (windows-latest)` (11/11 steps incl. Phase 3 smoke and
  release streaming proof), `rust (ubuntu-latest)`, `rust (macos-latest)`,
  `frontend`.
- **Documentation:** `docs/IDENTITY.md` (full semantics), `docs/ARCHITECTURE.md`
  (module boundaries now mark hasher/duplicates implemented), and
  `docs/API_CONTRACTS.md` (`spacelens.v1.identity.*`) all describe the
  implementation as it exists.

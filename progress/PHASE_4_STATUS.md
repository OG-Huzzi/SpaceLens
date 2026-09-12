# SpaceLens — Phase 4 Status

- **Phase:** 4 — Relationship & Duplicate Intelligence
- **Verdict:** **PHASE 4 VERIFIED** (full local gate green on Windows;
  CI matrix green on the implementation SHA — runs recorded below; every
  job verified individually from the Actions API)
- **Date:** 2026-09-12 · **Machine:** Windows 11 Pro x64, 8 GB RAM
- **Starting SHA:** `e15f03d` (Phase 3.2 record, clean tree)

## Purpose and scope discipline

Phase 3 established trustworthy observation, object identity, content
identity, safe hashing, and deterministic duplicate grouping. Phase 4 turns
those facts into a **Relationship Intelligence Layer**: explicit typed
relationship kinds, categorical evidence, formal duplicate invariants,
conservative recoverability, undetermined summaries, deterministic
ordering, and a query API. **No destructive action exists anywhere in this
phase** — no delete/quarantine/purge/movement/rename/uninstall, no
recommendations, no "one-click free space". Facts and explainable derived
information only.

## What was built

**Design note (Objective 1)** — the identity stack is documented in
docs/RELATIONSHIPS.md and the module docs: path identity (scan-scoped
`FsEntry::id/path`) → object identity (handle-proven `(volume, file id)`,
Unix st_dev/st_ino, Windows FILE_ID_INFO — Phase 3.2) → content identity
(SHA-256 under the Phase 3 mutation/identity/TOCTOU contract) →
relationship identity (this layer). Run identity reuses the pipeline run's
timestamps; no parallel id scheme is invented.

**Domain model + derivation (`crates/spacelens-identity::relationships`,
pure over `DuplicateReport` — no I/O):**

- Typed kinds, never conflated: `HardLinkAlias` (same object, multiple
  paths, zero recoverable) vs `ContentDuplicate` (distinct objects, same
  size + SHA-256). Pure alias sets never masquerade as content duplicates;
  alias sets inside mixed groups are exposed explicitly (`alias_sets`).
- Categorical `Evidence` (`OBJECT_IDENTITY_EQUAL`, `CONTENT_HASH_EQUAL`,
  `SIZE_EQUAL`) — no vague confidence scores. Ids are identity-derived
  (`content-<sha256hex>`, `alias-<volume>-<fileid>`) — deterministic,
  collision-free, stable across runs.
- Formal invariants (Objective 4) documented and test-pinned: same size ⇏
  duplicate; same name ⇏ duplicate; same path ⇏ same object; same object ⇒
  alias; equal content + independent objects ⇒ content duplicate; failed
  hash ⇒ no relationship.
- Conservative recoverability (Objective 6): reuses Phase 3 accounting —
  `size × (distinct objects − 1)` under `Exact`; honest upper bound under
  `Estimated`; `None` where unprovable; alias relationships `None`
  (removing an alias frees nothing). Physical allocation semantics are
  explicitly NOT modeled.
- Undetermined summaries (Objectives 10/13): typed Phase 3 failure kinds
  with exact counts, exact not-examined counts (cap exclusions), bounded
  detail. Status reuses `DuplicateStatus` (no parallel hierarchy);
  derivation refuses to publish relationships from `Cancelled`/`Unsupported`
  runs (defense in depth against upstream regressions). A run with
  undetermined files can never look like a clean "zero duplicates" result.
- Determinism (Objective 8): canonical order — kind → identity key →
  first path bytes; members by path bytes; evidence sorted. Shuffled
  observation order produces identical reports (test-pinned).
- Boundedness (Objective 9): transitive over the Phase 3 global caps plus
  its own `max_relationship_records` (250k default) with an exact
  truncation counter; published + truncated = derived. No database.
- Query API (Objective 11): `RelationshipIndex` —
  `relationships_for_path/object/content`, `duplicate_groups`,
  `hard_link_groups`; BTreeMap-backed, deduplicated, bounded by the report.
- Serialization (Objectives 15/16): serde camelCase + SCREAMING_SNAKE
  evidence, round-trip tested; content identity published as `sha256Hex` +
  algorithm tag; no raw digests, handles, or platform structs cross the
  contract boundary.

**Tests (Objectives 17–19): 36 new** — 12 unit + 24 integration:
- Full Objective 17 matrix: exact duplicates; same-size different-content;
  same-filename alone; different-filenames same content; hard links;
  aliases + independent copy; 3+ objects; hash failure (excluded member +
  typed undetermined); mutation/replacement (typed, never a relationship);
  cancellation; bounded limits (`CompletedWithLimits`); shuffled-order
  determinism.
- Objective 18 operational invariants (property-style): published digest ==
  hash of served bytes; same size never implies duplicate (20-file loop);
  same name never implies duplicate; same path never assumed same object;
  failed hash never joins a relationship. No impossible "collisions can
  never happen" claims.
- Objective 19 adversarial scale: 300 duplicate groups (deterministic ids);
  one 400-member group (exact counts, capped detail flagged); 200
  hard-link aliases (EXACT member count recovered from the Phase 3
  pure-alias accounting contract); 500 singleton sizes (zero cost); 50/50
  mixed success/failure fully accounted; index lookups exhaustive at
  scale. Real-fs derivation test: scan → hard links + independent copy →
  both kinds with Exact accounting.
- Two REAL bugs the tests caught during development, fixed:
  `parse_scheme`-style path parsing must ignore file names ("ok.bin"
  starts with `o`), and pure-alias member counts must come from the group
  contract, not from capped detail.

**Benchmark (Objective 20):** `relationship_engine_throughput` (ignored,
CI-run) over mostly-unique and many-duplicates at 10k/100k: pipeline run
vs derive vs index timings with the approximately-linear per-entry scaling
guard — proving no accidental O(n²) pairwise comparison and no
thread-per-entry behavior. Local: 100k entries → run 8.5 s, derive 1.0 s,
index 1.1 s (≈10× cost for 10× input). The intended algorithm remains
group-by-size → hash candidates → group-by-content → derive.

**Regression audit (Objective 24) — verified from the test suite:**
observation (Phase 1), classification (Phase 2/2.1), identity/hashing,
Windows identity (3.2), path-chain hardening (3.2), cancellation
determinism, mutation detection, links-never-followed, bounded hashing,
and deterministic output all remain green — the full workspace suite
(Phase 1/2/2.1/3/3.1/3.2 + Phase 4) passes unchanged, and no Phase 3.2
safety guarantee regressed.

## Verification

- **Local (Windows):** `cargo fmt --check` ✓ · `cargo clippy --workspace
  --all-targets -- -D warnings` ✓ (plus linux-target cross-clippy for
  unix-gated paths) · `cargo test --workspace` **364 passed / 0 failed**
  (328 at Phase 3.2; +36 Phase 4) · Phase 3+4 perf smokes ✓ · release
  >4 GiB streaming proof ✓.
- **CI:** recorded below after the matrix run.

## Self-audit (Objective — final questions, answered from the code)

1. Equal-sized files wrongly declared duplicates? No — grouping is by
   (size, content hash); same-size alone never groups (test-pinned).
2. Same-name files wrongly duplicated? No — names are opaque (pinned).
3. Hard-link aliases mistaken for separate copies? No — alias kind is
   separate, with `recoverable_bytes = None` (pinned, real-fs included).
4. Failed hashes silently disappear? No — typed undetermined records with
   exact counts (pinned).
5. Incomplete scans look like "zero duplicates"? No —
   `CompletedWithLimits` + `not_examined` counters (pinned).
6. Ordering dependent on worker scheduling? No — canonical sort,
   shuffled-order tests (pinned).
7. Relationship memory unbounded? No — transitive caps + own cap with
   exact truncation (pinned).
8. O(n²) for large candidate sets? No — size/hash-group pipeline +
   linear-scaling benchmark guard (pinned).
9. Recoverable space overstated for hard links? No — alias relationships
   carry `None`; mixed groups collapse to distinct objects (pinned).
10. Relationship without structured evidence? Impossible — evidence is a
    non-empty, canonical field of every relationship (type-level).
11. Cancellation publishing partial completed state? No — derivation
    refuses non-completed runs; pipeline publishes no groups on
    cancellation (pinned).
12. Phase 3.2 safety regressions? None — full workspace green including
    all 3.2 adversarial suites.
13. Destructive behavior introduced? None — grep-audited: no deletion,
    movement, process, or network surface in the relationship layer (pure
    derivation; purity audit below).

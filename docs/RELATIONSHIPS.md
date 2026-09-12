# SpaceLens — Relationship & Duplicate Intelligence (Phase 4)

Status: implemented and verified. This document describes the
implementation as it exists — every claim is backed by a test, every
limitation is stated. **This phase reports facts and explainable derived
information only: no deletion, cleanup, file movement, or recommendations
exist in Phase 4** (those belong to later phases, per the master plan).

## The identity stack

```text
Path identity            FsEntry::id / FsEntry::path (scan-scoped)
       ↓
Object identity          FileIdentity — (volume, file id) proven from
                         handles: Unix st_dev/st_ino via fstat; Windows
                         volume serial + FILE_ID_INFO file id via
                         query-only handles (Phase 3.2). Hard links
                         share it. Never fabricated where the OS cannot
                         prove it.
       ↓
Content identity         ContentHash — SHA-256 over bytes, published only
                         after the full Phase 3 mutation/identity/TOCTOU
                         check sequence.
       ↓
Relationship identity    THIS LAYER — typed, evidenced, deterministic
                         statements derived PURELY from the verified
                         pipeline output (crates/spacelens-identity ::
                         relationships).
```

**Run identity:** a relationship report derives from exactly one pipeline
run; the run's `started_at`/`finished_at` travel in the report. No
parallel run-id scheme is invented (the scan-scoped `FsEntry::id` and the
report provenance are the existing canonical references; a persistent
scan id belongs with persistence, a later phase).

## Relationship kinds (exhaustive, never conflated)

| Kind | Meaning | Evidence |
|---|---|---|
| `HardLinkAlias` | Different paths referring to the **same filesystem object**. Not a duplicate: no second physical copy exists. | `OBJECT_IDENTITY_EQUAL` |
| `ContentDuplicate` | **Distinct filesystem objects** carrying byte-identical content. | `CONTENT_HASH_EQUAL` + `SIZE_EQUAL` |

Explicitly **not** relationships (and never inferred from):

- **same size alone** — same-size different-content files are hashed and
  then not grouped; they appear in no relationship,
- **same filename alone** — names are opaque data, never identity,
- **same pathname alone** — a path naming the same object is not assumed;
  relationships carry the proven object identities (a same-path pair
  observed as distinct objects still derives by identity, not path),
- **anything unproven** — failed/skipped/inaccessible files are
  *undetermined* (below), never "no duplicates" and never guessed
  relationships.

A content duplicate whose members include hard-link aliases exposes them
explicitly (`alias_sets`): "4 paths, 3 objects — two paths are aliases of
one object" is directly representable, and a pure alias set never
masquerades as a content duplicate (there is no second copy).

## Evidence and explainability

Evidence is **categorical**, never a vague confidence score. Every
relationship's evidence list is non-empty, sorted canonically, and
answers "why does SpaceLens believe these entries are related?":

- *Exact duplicate:* `CONTENT_HASH_EQUAL` (+ `SIZE_EQUAL` as supporting
  fact) — the published content hash proves byte equality under the
  Phase 3 contract (same size + same SHA-256 ⇒ same content).
- *Alias set:* `OBJECT_IDENTITY_EQUAL` — proven from open/query handles.
- Explanations are rendered from the structured evidence and counts
  (`member_count`, `distinct_objects`, `alias_sets`), not from
  hard-coded prose. Example renderings live in
  `relationships.rs` docs and the test assertions.

## Duplicate invariants (formal)

1. For two **independent objects**: same size + same SHA-256 content ⇒
   equal content (the Phase 3 hashing contract; a cryptographic collision
   is the only theoretical exception and is treated as out of scope for
   facts about *published identities*).
2. Same size alone ⇒ **nothing**.
3. Same filename alone ⇒ **nothing**.
4. Same pathname alone ⇒ **nothing** without object identity.
5. Same object ⇒ alias relationship (independent of content — though
   aliases trivially share content).
6. A failed/skipped hash ⇒ **no relationship** for that file and an
   undetermined record; never a false duplicate, never a silent skip.

## Recoverable-space semantics (calculation only — no deletion)

- `logical_duplicate_bytes = size × (paths − 1)` — a fact about *bytes
  described*, always exact.
- `recoverable_bytes` for content duplicates =
  `size × (distinct_objects − 1)` under `Exact` accounting (every
  member's object identity proven — hard links collapsed exactly); the
  honest upper bound under `Estimated` (some identity unprovable);
  **`None` where it cannot be proven** — never a fabricated number.
- Alias relationships: `recoverable_bytes = None` — removing one alias of
  a hard-linked set frees nothing. `None` is an explicit unknown/zero,
  never presented as a savings estimate.
- Physical allocation (sparse, compression, copy-on-write) is NOT modeled
  — those need filesystem-provided evidence and belong to later phases.

## Undetermined files ("relationship undetermined" ≠ "no duplicates")

A run with failures or cap exclusions is surfaced as such:

- `undetermined.failed` — exact count of files whose hashing failed, with
  `failed_by_reason` (typed Phase 3 kinds: `Hash{category}`, `Changed`,
  `Replaced`, `Vanished`, `Cancelled`, `Unsupported`) and a bounded
  detail list (≤256, upstream cap; overflow counted).
- `undetermined.not_examined` — exact count of eligible files never
  hashed because a global cap bit (distinct-size tracking cap or the
  global candidate-record cap).
- The status (`DuplicateStatus` reused) marks capped runs
  `CompletedWithLimits` — a capped or partially-undetermined result can
  never masquerade as a clean complete "zero duplicates" answer.

## Deterministic ordering

- Relationships: **kind** (`HardLinkAlias` before `ContentDuplicate`) →
  **identity key** (object id, resp. content digest) → first member path
  bytes.
- Members within a relationship: path bytes ascending.
- Alias sets within a content duplicate: by object identity; paths
  path-byte ordered.
- Ids are derived from identity (`content-<sha256-hex>`,
  `alias-<volume>-<fileid>`), stable across runs; no counters.
- Repeated runs over the same input produce byte-identical reports,
  regardless of scan traversal order, worker scheduling, hash completion
  order, or directory enumeration order (asserted by shuffled-order
  tests).

## Boundedness

- The derivation is **pure** over the pipeline report, which is bounded
  by the Phase 3 global caps (distinct-size tracking, candidate records,
  per-group detail, failure detail). The derivation adds at most one
  record per content group plus one per alias set — transitively bounded.
- Its own hard cap `max_relationship_records` (default 250,000) truncates
  deterministically (canonical-order head kept) with an exact
  `relationships_truncated` counter; published + truncated = derived, and
  the stats describe published records only.
- The query index is BTreeMap-based and bounded by the published
  relationships. **No database is introduced** — persistence belongs to
  a later phase.

## Query API (`RelationshipIndex`)

`relationships_for_path`, `relationships_for_object`,
`relationships_for_content`, `duplicate_groups`, `hard_link_groups` —
clean lookups designed so storage search, history, intelligent uninstall,
cleanup recommendations, and diagnostics (later phases) can be built
without redesigning the relationship model.

## API / IPC contract (`spacelens.v1.relationship.*`)

`RelationshipReport`, `Relationship`, `RelationshipKind`, `Evidence`,
`MemberRef`, `ObjectRef`, `AliasSet`, `ContentRef`, `Undetermined`,
`RelationshipStats`, `RelationshipOptions` — all serde camelCase (evidence
`SCREAMING_SNAKE_CASE`), deterministic, additive-only within v1
(docs/API_CONTRACTS.md). Internal engine types (raw digests, handles,
platform structs) do not cross the boundary: content identity is
published as hex + algorithm tag.

## Known limitations (honest)

1. **Relationships derive from hashed candidates only.** Files outside
   the candidate set (size singletons) and files whose hashing failed are
   not part of any relationship — alias sets among *unhashed* files are
   therefore not reported (deriving them would require re-partitioning
   raw scan data, which Phase 4 deliberately does not do). A hash-failed
   alias is visible as an undetermined file.
2. **Alias member counts in mixed groups with truncated detail** are
   scoped to the reported member detail (≤64 per group upstream) and
   flagged via `detail_truncated`. Pure-alias groups (Exact + no
   recoverable bytes) carry the EXACT member count — the Phase 3
   accounting contract proves the whole group is one object. Counts are
   never fabricated either way.
3. **Estimated accounting** means distinctness is unknown (some member's
   object identity unprovable on the platform/volume): recoverable bytes
   are an upper bound assuming all members are distinct objects.
4. **No dedup across runs / no history:** each report is run-scoped;
   cross-run relationship memory is a persistence-phase concern.

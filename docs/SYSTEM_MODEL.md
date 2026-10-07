# CoreSight System Model (Phase 6.3)

Status: implemented as an **in-memory, read-only** correlation layer over
the Phase 6.1/6.2 subsystems. No executor, no network, no persistence, no
subprocess. This document records the contracts the code actually enforces.

```text
Observation
    ↓
Identity / Classification
    ↓
Application Intelligence
    ↓
Correlation            ← crates/coresight-system-model
    ↓
System Model
    ↓
Queries / Insights
```

## 1. SystemModel purpose

Join the facts the other subsystems already established into **one coherent,
immutable graph** instead of several unrelated maps maintained by callers:

```text
filesystem observations  (paths, access, object identity, content digests)
    + classification     (category, subcategory, confidence — copied, never re-derived)
    + identity relationships (hard-link aliases, content duplicates)
    + application inventory + ownership evidence (Phase 6.2)
    + capability state    (honest statuses, verbatim)
    + history context     (projected from stored history only)
        ↓
SystemModel { artifacts, applications, edges, historical_context,
              insights, candidates, observations, truncation }
```

## 2. Node types

**`ArtifactNode`** — one observed path, with three independent facts:

| Field | Meaning |
| --- | --- |
| `path` | The exact observed path (lossless). |
| `identity` | Canonical `ObjectIdentity { volume, file_id, file_id_hi }`, or `None`. |
| `content_sha256` | A digest the identity engine proved, or `None`. |

Plus the classifier's copied verdict (`category`, `subcategory`,
`classification_confidence`), `access`, `provenance`, and the derived
`application_status`. Path, object identity, and content identity answer
different questions and stay separately representable: two nodes may share an
object (hard-link aliases), two nodes may share content (duplicates), and a
node may have a path with no proven identity (recorded as `None`, never
fabricated).

**`ApplicationNode`** — one logical application (`ApplicationId`, name,
publisher, bundle identifier, install location, executable, unioned
provenance, resolution `state` with structured `state_reasons`).

## 3. Edge types

| Kind | Meaning | Domain | Provenance |
| --- | --- | --- | --- |
| `Contains` / `LocatedUnder` | Directory structure, both directions | Filesystem | Observed |
| `ApplicationInstallRoot` | The artifact is a recorded install root | ApplicationIntelligence | Observed |
| `ApplicationExecutable` | The artifact is a recorded executable | ApplicationIntelligence | Observed |
| `ApplicationData/Cache/Log/Config` | Descriptive roles from the classifier | Classification | Inferred |
| `OwnedBy` | Credible ownership only | ApplicationIntelligence | Inferred |
| `AssociatedWith` | Non-credible link | ApplicationIntelligence | Inferred |
| `SharedBy` | Two applications both relate to one artifact | ApplicationIntelligence | Inferred |
| `DuplicateOf` | Distinct objects, byte-identical content | Identity | Observed |
| `HardLinkAliasOf` | One object, several paths | Identity | Observed |
| `HistoricalAliasOf` / `HistoricalMoveOf` | Stored past fact | History | Observed |

Only relationships supportable by existing evidence are emitted. Every edge
carries kind, domain, endpoints, assessment, provenance, and structured
evidence.

## 4. Identity semantics

All artifact nodes carry the single canonical `ObjectIdentity`. No narrow
pair, no packed `u128`, no hash-as-identity, no path-as-identity. Where the
platform proved nothing, `identity` is `None` and candidates record
`UnprovenObjectIdentity` as a blocker — never a fabricated value. Mixed
provability stays conservative (wide ≠ narrow with the same low pair).

Nodes are keyed by a lossless encoding of the path bytes (`ArtifactKey`:
hex of the platform encoding, order-preserving), explicitly documented as a
node key and **never** as an identity.

## 5. Application correlation

Observed facts (install roots, recorded executables) become `Observed` edges.
Association evidence feeds one bounded correlation-aware accumulator per
(app, artifact), so the same signal forwarded as several items still counts
once. Exactly one ownership edge per pair: `OwnedBy` when the accumulated
assessment is credible, `AssociatedWith` otherwise. Structural-only evidence
stays below the credibility line by construction.

A single artifact may relate to zero, one, several, or conflicting
applications — reported as `Unassociated` / `Associated` / `Shared` /
`Conflicting` (`Uncertain` when claims exist but none is credible).

## 6. Ownership semantics

`RelationKind` semantics at the model level: **containment is not
ownership**. `Contains` is pure structure; a containing root can relate an
application to an artifact but can never make it `OwnedBy`. The query
`owning_applications` counts each application once, so descriptive role edges
never inflate claimant counts.

## 7. Classification semantics

Classification is copied into nodes and chosen as role edges (`ApplicationData`,
`ApplicationCache`, `ApplicationLog`) when the evidence already links the
application. Classification never overwrites association and association
never overwrites classification — they are separate fields and separate
edges.

## 8. History integration boundary

History arrives only as caller-projected `HistoryFact` records (run id, path,
identity, category). The model emits `HistoricalAliasOf`/`HistoricalMoveOf`
edges and keeps the context records, but **never derives history from current
state**: a context record for a path the model does not hold publishes data
but anchors no edge. No schema was changed and no persistence was added.

## 9. Evidence propagation

App-originated evidence arrives with its Phase 6.2 correlation groups
intact. Aggregation is per (app, artifact) through the same ceilings, so:

- install root + executable path from one registry record appraise once;
- name-derived heuristics cannot corroborate themselves into strength;
- registry evidence stays in its `SourceRecord` group from app scan through
  model build.

## 10. Conflict handling

Contradictory claims coexist: two `Strong`/`Direct` claims become
`Conflicting` status, the `ConflictingOwnership` insight, and blocked
candidates. A later input never overwrites an earlier one — duplicates merge
by content rank, and the permutation/idempotence tests prove it.

## 11. Unknown / unavailable semantics

`denied != empty != missing != unsupported != unavailable != failed`.
`ProvenanceState::from_access` maps every access state honestly.
`ArtifactApplicationStatus` distinguishes `Unassociated` (sources usable, no
claim — the only state that may inform later orphan reasoning) from
`AssociationUnsupported` / `AssociationUnavailable` / `AssociationFailed`
(source gaps, never "no application"). The sole supported source-status
claim about "orphan" is deliberately withheld: the model only reports
`UnassociatedArtifact` insights and `Orphan` candidates with
`InsufficientEvidence` blockers.

## 12. Bounds

`SystemModelLimits` (defaults): `max_artifacts` 200,000,
`max_applications` 20,000, `max_edges` 500,000, `max_evidence_per_edge` 16,
`max_edges_per_node` 256, `max_historical_context` 4,096, `max_insights`
4,096, `max_candidates` 4,096.

Admission is incremental through `BoundedTopK` at every collection
(artifact nodes, application nodes, edges, history, insights, candidates);
per-node edge capping is a deterministic reduction of the already-bounded edge
set, and every dropped item is counted exactly in `ModelTruncation`.

## 13. Determinism

The model is a pure function of the input fact multiset: canonical keys,
content-ranked duplicate resolution, canonical re-sorting of every published
collection. Permutation tests cover reversed artifacts/applications/
relationships/history, full input rotation, a 200-permutation sweep of an
8-artifact fixture, duplicate inputs, and split-and-swap fragment builds.

## 14. Query contracts

Queries in `insight.rs` are **pure, typed, bounded, and canonically ordered**:

`applications_for_artifact` (one row per app, all edge kinds),
`owning_applications` (distinct credible apps), `artifacts_for_application`,
`artifacts_of_classification`, `shared_artifacts`, `conflicting_ownership`,
`unresolved_associations`, `strongly_associated_artifacts`,
`artifacts_without_application` (usable sources, no claim), `aliases_of_object`,
`association_unknown_artifacts` (sources unusable), `insights_of_kind`.
Every list query takes a `limit` and reports exact `truncated` overflow.

## 15. Safety boundary

Candidates are inert data (`SystemCandidate`): `action_kind`, target,
confidence, assessment, `ActionClass::Destructive` recorded descriptively,
blockers always including `NoExecutorInThisPhase`. `can_authorize_execution`
and `candidate_is_authorized` return `false` for every input, asserted in
tests. No executor, no destructive primitive, no subprocess, no network
anywhere in the crate.

## 16. Persistence boundary

```text
Application persistence NOT STARTED
System-model persistence NOT STARTED
Database schema NOT changed
```

The model is `Serialize`/`Deserialize`-capable for testing/transport only
(the round-trip test asserts exact equality); the object-identity index is
serialized through a string-keyed form because serde maps require string
keys. Nothing is persisted.

## 17. Platform support

Shared platform-neutral code; a source-scan guard test forbids platform
conditionals, subprocess/network tokens, mutating-primitive tokens, lossy
path tokens, and the history/SQLite dependency. Real filesystem reads happen
in the observation layers, never in this crate. Synthetic fixtures run on
all three CI platforms; a portable non-UTF-8 regression runs everywhere.

## 18. Verification record (Phase 6.3)

Verified commit `0676223` — CI run
[37649180299](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37649180299),
conclusion **success**:

```text
rust (ubuntu-latest)   success
rust (windows-latest)  success
rust (macos-latest)    success
frontend               success
```

Local results on the same HEAD: `cargo fmt --all --check` clean,
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean, `cargo test --workspace` and `--all-features` green, `npm ci` +
`npm run build` success, `git diff --check` clean, Linux and macOS
cross-target checks for the touched crates green, and all ignored
performance smoke suites (engine, classifier, identity, history) passing.

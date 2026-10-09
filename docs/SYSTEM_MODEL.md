# CoreSight System Model (Phase 6.3 hardened; Phase 6.4 persistent)

Status: implemented as a **pure, deterministic, bounded, read-only**
correlation layer over the Phase 6.1/6.2 subsystems, with Phase 6.4
**durable snapshot persistence** layered OUTSIDE the crate. Phase 6.3
hardening is VERIFIED (source commit `df2e24c`, exact-SHA CI run
37797444920, all four jobs green). The verified hardening includes
private canonical storage, canonical-only deserialization, bidirectional
index invariants, bounded correlation and source summaries,
truncation-aware association status, validated relationship proofs,
deterministic duplicate resolution, node-attached history, source-specific
install-root/executable provenance, evidence ceilings at aggregation and
deserialization, strict lossless path handling, and candidate-level
executable confidence.

Phase 6.4 adds **no line of persistence code to this crate**: it stays
pure, platform-neutral, and database-independent (§16, §17). Storage of
canonical application-intelligence facts and system-model snapshot
*inputs* lives in the persistence boundary
(`coresight-history`, schema v5 — see docs/DATABASE.md and
docs/HISTORY.md), and reload feeds the SAME `build_system_model` /
`finalize` / `check_invariants` path as a fresh build. No executor,
network, subprocess, or filesystem mutation exists anywhere in this
crate.

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
              historical_assertions, insights, candidates,
              observations, truncation }
```

Canonical storage is PRIVATE: the only construction route
(`build_system_model` → `SystemModel::finalize`) builds every index from
the canonical sets, and external code gets read-only accessors
(`artifacts()`, `applications()`, `edges()`, `historical_context()`,
`historical_assertions()`, `insights()`, `candidates()`, `observations()`,
`truncation()` — plus the existing lookup accessors). There is no mutating
API, so the indexes cannot go stale.

## 2. Node types

**`ArtifactNode`** — ONE observed path occurrence, with three independent
facts. The contract is precise:

```text
ArtifactNode     = one observed path occurrence
ArtifactKey      = lossless path-occurrence key (hex of the platform
                   encoding, order-preserving) — explicitly a NODE KEY,
                   never an identity
ObjectIdentity   = the filesystem-object identity, carried as a separate
                   fact and its own index
```

`artifact key != object identity`: two nodes may share an object
(hard-link aliases), two nodes may share content (duplicates), and a node
may have a path with no proven identity (recorded as `None`, never
fabricated). Hard-link paths are never collapsed: one object stays visible
through as many path nodes as were observed.

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
| `ApplicationInstallRoot` | A detector root candidate; `Observed` only for the exact recorded install location, otherwise inferred and weak | ApplicationIntelligence | Observed / Inferred |
| `ApplicationExecutable` | An executable path; `Observed` only for the exact metadata path, otherwise candidate-level and weak | ApplicationIntelligence | Observed / Candidate |
| `ApplicationData/Cache/Log/Config` | Descriptive roles from the classifier | Classification | Inferred |
| `OwnedBy` | Credible ownership only | ApplicationIntelligence | Inferred |
| `AssociatedWith` | Non-credible link | ApplicationIntelligence | Inferred |
| `SharedBy` | Two applications both relate to one artifact | ApplicationIntelligence | Inferred |
| `DuplicateOf` | Proven distinct objects, byte-identical content | Identity | Observed |
| `HardLinkAliasOf` | One object, several paths | Identity | Observed |

History is NOT an edge kind: there are no `HistoricalAliasOf` /
`HistoricalMoveOf` edges (a self-loop `HistoricalMoveOf(current, current)`
would mislead by suggesting a relationship between two nodes). Stored past
facts stay typed context (`HistoricalContext`) plus a node-attached
`HistoricalAssertion` (`SameObjectObserved` / `ObjectReplaced` /
`IdentityUnproven`) quoting the caller's record against its current node —
see §8.

Only relationships supportable by existing evidence are emitted. Every edge
carries kind, domain, endpoints, assessment, provenance, and structured
evidence.

## 4. Identity semantics

All artifact nodes carry the single canonical `ObjectIdentity`. No narrow
pair, no packed `u128`, no hash-as-identity, no path-as-identity. Where the
platform proved nothing, `identity` is `None` and candidates record
`UnprovenObjectIdentity` as a blocker — never a fabricated value. Mixed
provability stays conservative (wide ≠ narrow with the same low pair), and
full comparison (volume, file id, AND high bits — never narrowed) governs
every identity check, including relationship validation.

Nodes are keyed by a lossless encoding of the path bytes (`ArtifactKey`:
hex of the platform encoding, order-preserving), explicitly documented as a
node key and **never** as an identity.

`DuplicateOf` edges require proof-validated relationships (see §5): the
endpoints must share one proven digest (matching the fact's digest when
supplied), every endpoint identity must be known, and those identities must
be pairwise distinct. A `DuplicateContent` insight requires a known identity
on the node and at least one other proven, distinct identity in the digest
group; an unknown-only pair cannot claim distinct objects. Equal identities
produce hard-link alias evidence, never a distinct-object duplicate. Group
summaries keep this `O(A log A)`, even for a very large shared-digest group.

## 5. Application correlation

Only exact source-recorded install locations and executable paths become
`Observed` edges. Detector-derived roots (including a desktop-entry Exec
parent) are inferred, weak structural scope; an unrecorded executable path
remains a weak `Candidate`. A desktop-entry `install_location` is derived
from the `Exec` parent, so it is not treated as an expected install root and
its broad directory contents cannot manufacture a partial app footprint. A
module boundary never promotes either to an observed fact. Association
evidence feeds one
bounded correlation-aware accumulator per (app, artifact) — itself held in a
`BoundedTopK` capped at `max_edges` pairs — so the same signal forwarded as
several items still counts once,
and working memory is `O(max_edges × max_evidence_per_edge)`, never
`O(input)`. Dropped pairs count exactly in `claims_truncated`. Exactly one
ownership edge per surviving pair: `OwnedBy` when the accumulated
assessment is credible, `AssociatedWith` otherwise. Structural-only evidence
stays below the credibility line by construction. A claim edge publishes
only when BOTH endpoints exist as nodes: facts from applications that did
not survive the application bound seed truncation memory, never phantom
claims.

Relationship facts are caller projections, not graph truth: every endpoint
must resolve to a retained node (unknown endpoints reject the whole fact),
`HardLinkAlias` requires all endpoints to prove one equal full identity
(agreeing with the fact's `object` proof when supplied), and
`ContentDuplicate` requires all endpoints to share one proven digest (matching
the fact's `content_sha256` when supplied) AND every endpoint identity to be
known and pairwise distinct. Oversized relationship sets are atomically
rejected before pair generation; all rejected proofs are counted in
`relationships_rejected`.

Duplicate application records under one id merge commutatively: a
total-order precedence over every field (resolution state, reason set,
name, publisher, bundle identifier, install/executable paths, provenance)
picks the winner — `choose(a,b) == choose(b,a)` — while provenance unions
across all duplicates. Identical history payloads collapse; contradictory
payloads under one (run, path) are BOTH preserved (conflict preservation —
arrival order never decides historical truth).

A single artifact may relate to zero, one, several, or conflicting
applications — reported as `Unassociated` / `Associated` / `Shared` /
`Conflicting` (`Uncertain` when claims exist but none is credible,
`AssociationTruncated` when claims existed but bounds discarded them —
see §11).

## 6. Ownership semantics

`RelationKind` semantics at the model level: **containment is not
ownership**. `Contains` is pure structure; a containing root can relate an
application to an artifact but can never make it `OwnedBy`. The query
`owning_applications` counts each application once, so descriptive role edges
never inflate claimant counts.

## 7. Classification semantics

Classification is copied into nodes and chosen as role edges (`ApplicationData`,
`ApplicationCache`, `ApplicationLog`) when the evidence already links the
application. Those descriptive role edges carry the `Classification` domain
(chosen from the classifier's verdict — classification is never rewritten by
ownership, and ownership never rewrites classification). Classification never
overwrites association and association never overwrites classification — they
are separate fields and separate edges.

## 8. History integration boundary

History arrives only as caller-projected `HistoryFact` records (run id, path,
identity, category). The model keeps the context records and attaches one
`HistoricalAssertion` per joined record to its current node — quoting the
stored identity against the node's identity:

- both proven and equal → `SameObjectObserved`;
- both proven and different → `ObjectReplaced` ("history proves this path
  previously referred to a different object");
- either side unproven → `IdentityUnproven` (never asserted as sameness
  without proof).

The model **never derives history from current state**: a context record for
a path the model does not hold publishes data but anchors no assertion. No
schema was changed and no persistence was added.

## 9. Evidence propagation

App-originated evidence arrives with its Phase 6.2 correlation groups
intact. Aggregation is per (app, artifact) through the same ceilings, so:

- install root + executable path from one registry record appraise once;
- name-derived heuristics cannot corroborate themselves into strength;
- registry evidence stays in its `SourceRecord` group from app scan through
  model build.

Application resolution only treats exact source-recorded install locations
and executable paths as observed expectations. Detector-derived roots remain
inferred structural scope; an unrecorded executable path remains a weak
candidate. An observed executable candidate, or readable descendants beneath
a detector root for a source with an authoritative install location, can
contribute to `PartiallyResolved`, never `Resolved`. Desktop-entry Exec
parents are excluded from this root-based state evidence. An inaccessible
expected root or executable with no readable descendants yields `Unknown`,
not `Unresolved`; readable descendants yield `PartiallyResolved`.

## 10. Conflict handling

Contradictory claims coexist: two `Strong`/`Direct` claims become
`Conflicting` status, the `ConflictingOwnership` insight, and blocked
candidates. A later input never overwrites an earlier one — duplicates merge
by content rank, and the permutation/idempotence tests prove it.

## 11. Unknown / unavailable / truncated semantics

`denied != empty != missing != unsupported != unavailable != failed !=
truncated`. `ProvenanceState::from_access` maps every access state honestly.
`ArtifactApplicationStatus` distinguishes `Unassociated` (sources usable, no
claim, no bound dropped a claim — the only state that may inform later
orphan reasoning) from `AssociationUnsupported` / `AssociationUnavailable` /
`AssociationFailed` (source gaps, never "no application") and from
`AssociationTruncated` (claims were observed but none survived the model's
bounds — incomplete knowledge, never absence). The sole supported
source-status claim about "orphan" is deliberately withheld: the model only
reports `UnassociatedArtifact` insights and `Orphan` candidates with
`InsufficientEvidence` blockers, and only for genuinely unassociated
artifacts. Truncated artifacts surface in `unresolved_associations` (with
cause) and never in `artifacts_without_application`.

System-wide principle: **truncation represents incomplete knowledge, never
absence** — claim dropped by bound ≠ no claim; evidence dropped ≠ no
evidence; history row dropped ≠ no history; artifact dropped ≠ artifact did
not exist. Every drop is counted exactly in `ModelTruncation`
(`claims_truncated`, `roots_truncated`, `relationships_rejected`,
`source_states_truncated` alongside the original counters).

## 12. Bounds

`SystemModelLimits` (defaults): `max_artifacts` 200,000,
`max_applications` 20,000, `max_edges` 500,000, `max_evidence_per_edge` 16,
`max_edges_per_node` 256, `max_historical_context` 4,096, `max_insights`
4,096, `max_candidates` 4,096, `max_source_states` 64.

Admission is incremental through `BoundedTopK` at every collection
(artifact nodes, application nodes, edges, claim pairs, install-root groups,
source-coverage summaries, history, insights, candidates); per-node edge
capping is a deterministic reduction of the already-bounded edge set, and
every drop is counted in `ModelTruncation`. The invariant is **working memory
≤ function(SystemModelLimits), not final output ≤ limits**: the claim store
(`max_edges` pairs), root grouping (`max_edges` root keys ×
`max_applications` owners per key), `SharedBy` pair fan-out (`max_edges`
pairs), source-state summary (`max_source_states`), per-pair and per-insight/
candidate evidence accumulators (`max_evidence_per_edge`), and the
precomputed artifact structure (`O(A · depth)`, with compact per-root access
summaries) are bounded DURING the build — never collect-then-truncate.
Overflowed root groups conservatively make association knowledge incomplete
for retained artifacts when a dropped root cannot be identified without an
unbounded side set; this may overstate uncertainty but cannot create false
absence. Duplicate-content and hard-link-alias insight detection uses group
summaries (`O(A log A)`), never pairwise `O(A²)` scans; per-application state
resolution uses one exact-path lookup, constant-time root summaries, and
precomputed executable-child counts per declared root — no repeated full
artifact scans. This stage costs `O(A · depth + R log A)` for `A` retained
artifacts and `R` declared roots, plus the bounded per-root grouping pass.

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
`artifacts_without_application` (usable sources, no claim, no truncation),
`aliases_of_object`,
`association_unknown_artifacts` (sources unusable), `insights_of_kind`.
Every list query takes a `limit` and reports exact `truncated` overflow.

Honesty contract: each query documents its actual complexity. Node scans
run over the bounded artifact/insight sets (`O(A)` / `O(I)` time,
`O(limit)` retained memory); neighborhood queries run over the per-node
edge degree (`O(deg log deg)`, degree ≤ `max_edges_per_node` by
construction). No query claims `O(limit)` working memory it does not
provide.

## 15. Safety boundary

Candidates are inert data (`SystemCandidate`): `action_kind`, target,
confidence, assessment, `ActionClass::Destructive` recorded descriptively,
blockers always including `NoExecutorInThisPhase`. `can_authorize_execution`
and `candidate_is_authorized` return `false` for every input, asserted in
tests. No executor, no destructive primitive, no subprocess, no network
anywhere in the crate.

## 16. Persistence boundary

```text
Application/system snapshot persistence  IMPLEMENTED (Phase 6.4, schema v5)
Owned by                                 coresight-history (NOT this crate)
This crate's database dependency         NONE (guarded by source-scan test)
```

The model is `Serialize`/`Deserialize`-capable. The wire form carries
canonical data ONLY: indexes are `#[serde(skip)]` (never serialized) and
never read back — deserialization validates canonical ordering and
structural invariants (ordering, key uniqueness, edge-endpoint existence,
assertion targets) and then REBUILDS every index through the same
`finalize` route, running the full bidirectional `check_invariants`
before returning. A legacy `indexes` section in an older payload is
ignored: two payloads differing only in derived indexes reconstruct the
identical canonical index set, and malformed indexes can never poison the
model.

### What Phase 6.4 persists (canonical facts only)

```text
artifact facts      path (lossless), kind, object identity (full width),
                    verified digest, size, access state, classification
application facts   the full ApplicationRecord (id, name, publisher,
                    version, install location, install date, size,
                    uninstall/modify strings, source, kind,
                    system-component flag, bundle identifier,
                    executable path), unioned provenance, raw views
install roots       per application, canonically ordered
ownership evidence  kind, source, strength, correlation group, scope,
                    observed path, matched attribute/value/path
footprint           candidate path/kind/confidence + footprint evidence
relationship facts  kind, proven object identity, proven digest, members
history facts       (run, path, identity, category) as QUOTED CONTEXT
source coverage     per-source status + note (honest, never an empty
                    success)
truncation          inventory/footprint counters, preserved exactly
```

Every row carries its `run_id`, so snapshot knowledge is per-run history —
never globally mutable "current" state.

### What is NOT persisted (rebuilt or never stored)

```text
derived           node/edge collections, every index (artifact→key,
                  key→object/content/category, application→artifacts,
                  edge adjacency), claim assessments, application
                  resolution states, insights, candidates, query results
ephemeral         UI state, process handles, in-flight jobs, scan
                  progress, executor state (none exists)
inert             candidate authorization, "safe to delete" readings,
                  safety-gate verdicts — none of these exist in the crate,
                  and persisting a snapshot can never create one
```

### Reconstruction path (one canonical route)

```text
app_snapshot_* rows
   ↓  strict decode: lossless paths, full-width identity,
      ceiling-clamped evidence, id re-verified against (name, publisher)
validated SystemModelInput
   ↓  the SAME pure builder
build_system_model() → finalize() → check_invariants()
```

There is deliberately **no `load_from_db()` inside this crate**, and no
second construction route anywhere: the persistence layer returns
*inputs*, and both a fresh build and a database reload end at the same
builder. Round-trip equality is asserted over observable canonical
semantics (artifacts, applications, edges, evidence, provenance,
relationships, historical context/assertions, truncation, insights,
candidate facts, ordering) — never over private index internals.

A snapshot read is bounded by the caller's `QueryLimits`, and a bound that
cuts the fact set short is **reported, never silent**: the load names its
capped sections and the rebuild fails closed rather than building a model
from a prefix of the stored facts. A model is therefore only ever rebuilt
from the complete stored fact set, so "fewer artifacts than the run
observed" can never be mistaken for "the machine has fewer artifacts".

### Evidence and identity cannot be inflated by the database

A stored strength is re-clamped through `OwnershipEvidence::new` on
reload, so a tampered `DIRECT` claim on a weak-kind/weak-group item
returns as `Weak`; an application id that no longer matches normalized
`(name, publisher)` is a typed corruption error, not a second identity.
Truncation, source-unsupported/unavailable/failed, and access-state
distinctions survive verbatim — persistence can never turn incomplete
knowledge into `Unassociated`, `Orphan`, `Complete`, `Resolved`, or
`Safe`. See docs/DATABASE.md §"Phase 6.4" for the full storage contract.

## 17. Platform support

Shared platform-neutral code; a source-scan guard test forbids platform
conditionals, `std::fs`/environment/time access, platform path/registry
probers and application providers, subprocess/network tokens, mutating
primitives, lossy path tokens (including `.to_str()` in semantic code), and
history/SQLite dependencies. It scans production code (stripping test-only
modules) and strips comment lines so the guard itself and explanatory prose
do not trip it. Capability blockers are platform-neutral by construction
(shared code carries no platform conditionals and invents no platform-specific
requirements). Real filesystem reads happen in the observation layers, never
in this crate. Synthetic fixtures run on all three CI platforms; a portable
non-UTF-8 regression runs everywhere.

## 18. Verification record (Phase 6.3 + hardening gate)

Implementation commit `0676223` — CI run
[37649180299](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37649180299),
conclusion **success**:

```text
rust (ubuntu-latest)   success
rust (windows-latest)  success
rust (macos-latest)    success
frontend               success
```

Hardening source commit `df2e24c55807032f1bb5f61c088c660fb6587ebd` is pushed
to `main` and verified by CI run
[37797444920](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37797444920),
attempt 2, conclusion **success**:

```text
rust (ubuntu-latest)   success
rust (windows-latest)  success
rust (macos-latest)    success
frontend               success
```

The first macOS attempt was canceled by hosted-runner capacity before
acquiring a runner; the targeted retry completed successfully. No source
changes were made between attempts. The final verification-record change is
documentation-only: implementation, hardening, and verification remain
distinguished in `progress/CURRENT_PHASE.md`.

### Phase 6.4 (persistence) verification record

Phase 6.4 changed **no code in this crate** — it added the snapshot
persistence boundary in `coresight-history` (schema v5) and left
`coresight-system-model` pure and database-independent, guarded by the
existing source-scan test (which still scans every source file and still
forbids persistence, subprocess, network, platform, and lossy-path
tokens).

Implementation commit `aaff0a3bb16701f9c0b19327911fa358ade08a77` — run
[37934408329](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37934408329),
attempt 1, conclusion **success**; the follow-up commit-contract
hardening `5b18ef32b59d426bb18914e2ae081c4944b4db60` — run
[37936917961](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37936917961),
attempt 1, conclusion **success**:

```text
rust (ubuntu-latest)   success
rust (windows-latest)  success
rust (macos-latest)    success
frontend               success
```

Local gate for the same commits: `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets --all-features -- -D warnings`
clean; `cargo test --workspace` and `--all-features` **802 passed, 0
failed** (661 at the pre-6.4 baseline); all `--ignored` performance
suites green; `npm ci` + `npm run build` green; `git diff --check` clean.

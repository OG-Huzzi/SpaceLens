# Application Intelligence (Phase 6.2)

Status: implemented as an **in-memory**, read-only intelligence layer over the
Phase 6.1 application foundation. No persistence, no executor, no network, no
subprocess. This document records the contracts the code actually enforces;
where a source is only compile-verified on a platform, it says so.

```text
Application
    ↓
Identity
    ↓
Provenance
    ↓
Installation Root
    ↓
Footprint
    ↓
Artifact Relationship
    ↓
Evidence
    ↓
Ownership Assessment
    ↓
Confidence / Conflict
```

## 1. Application model

`ApplicationRecord` (`crates/coresight-apps/src/domain.rs`) is the canonical
per-application fact set. Phase 6.2 added three fields:

| Field | Meaning |
| --- | --- |
| `bundle_identifier` | Bundle/package identifier declared by the application's own metadata (macOS `CFBundleIdentifier`, MSIX family name). `None` = none declared. |
| `executable_path` | The executable path **exactly as the source recorded it**. Lossless; not verified to exist by the record itself. |
| `provenance` | Every `ApplicationSource` this logical application was observed through — canonically ordered and deduplicated. |

`observed_in_views` (Phase 6.1) remains the raw per-view provenance.

## 2. Application identity

A logical application is identified by its normalized `(name, publisher)`
pair alone, hashed into `ApplicationId`. The discovery **source is
provenance, never identity**: the same application seen through the registry,
an MSIX package, a macOS bundle, and a `.desktop` entry is ONE logical
application with unioned provenance.

Changing the source of a record does not change its id; changing its name or
publisher does.

`merge_inventory` chooses a winning record by a **total content order**
(completeness first, then every surviving field, then the display spelling of
name/publisher). Provider call order is never a tie-breaker, so
`merge(a, b) == merge(b, a)` and the winner is a maximum over the record set.
Provenance is unioned independently of which record wins.

## 3. Provider provenance

| Source | Platform | Status |
| --- | --- | --- |
| Win32 uninstall registry (HKLM-64, HKLM-32, HKCU) | Windows | Implemented (`Win32UninstallEnumerator`) |
| MSIX/AppX | Windows | **Unsupported** — reported as `SourceStatus::Unsupported`, never an empty inventory |
| macOS application bundles (`*.app/Contents/Info.plist`) | any host, real reads on macOS | Implemented (`BundlePlistProvider`); runtime-validated only on macOS |
| Freedesktop `.desktop` entries | any host, real reads on Linux/BSD | Implemented (`DesktopEntryProvider`); runtime-validated only on Linux |
| Distro package databases (dpkg/rpm/pacman) | Linux | **Unsupported** — not read at all; never approximated by invoking a package manager |
| Login items / launchd agents, TCC-protected user data, APFS volume facts | macOS | **Deferred** (Phase 6.1 catalog semantics: `Mechanism` / `RequiresFullDiskAccess`) |

Honesty rules enforced by tests: `Complete`, `Partial`, `Unsupported`,
`Failed`, and `Unavailable` are never interchangeable, and an unreadable or
denied root is never a successful empty result.

## 4. Footprint model

`FootprintKind` is an **observation taxonomy**, not a deletion plan. Phase 6.2
extended it (appending, so the existing canonical order is unchanged) with
`Executable`, `SharedLibrary`, `ApplicationData`, `CrashData`,
`UninstallMetadata`, `InstallFile`, and `Other`.

The implementation can distinguish install files, executables, shared
libraries, configuration, application data, cache, logs, crash data, user
data, shortcuts/launchers, uninstall metadata, and other artifacts. **None of
these implies removability.**

Discovery is incremental and bounded: candidates are classified and admitted
as they are observed, never collected-then-truncated. `DiscoveryLimits` bounds
records, apps probed, children per root, evidence per candidate, directories,
entries, depth, and metadata bytes.

## 5. Install-root detection

`detect_install_roots` derives candidate roots from independent signals, each
recorded on the root itself (`RootSignal`):

| Signal | Strength |
| --- | --- |
| `InstallerRecorded` (exact install location) | Direct |
| `DesktopEntryExec` (absolute `Exec`/`TryExec`) | Direct |
| `ExecutableParent` (parent of a recorded executable) | Strong |
| `BundleRoot` (`X.app` inferred from `X.app/Contents/MacOS/…`) | Strong |
| `PublisherThenName` (`<root>/<publisher>/<name>`) | Weak |
| `NameUnderProgramRoot` (`<root>/<name>`) | Weak |

Rules: the application name is **never** assumed to equal its directory name;
original paths are preserved byte-for-byte and compared through
`PathKey` (platform-encoded bytes); normalization happens only in the
comparison layer. Semantic matching never depends on replacement
characters from lossy decoding: name keys decode path components with
STRICT UTF-8 (`crates/coresight-apps/src/pathmatch.rs`) — a non-UTF-8
component is "cannot interpret" and never matches — and ASCII-defined
constants (`MacOS`, `Contents`, `exe`, `app`, `lnk`, `desktop`, …) compare
at the byte level without decoding the arbitrary file name at all. Roots
are bounded by `max_roots_per_app`, deduplicated across signals by path,
and ordered canonically — arrival order is never a tie-breaker. A root is
a *scope*, not a claim.

## 6. Executable association

`ExecutableStatus` distinguishes `ObservedExact` (the source recorded this
exact path and it was observed), `Inferred` (bundle layout), `Candidate`
(name-matched inside a detected root — explicitly **weak**), and `Unknown`.
Ownership is never inferred from `foo.exe` / `Foo/` / `Publisher/` alone
without the evidence model recording that as weak.

## 7. Evidence model and the correlation ceiling

This is the core anti-inflation contract. Each `OwnershipEvidence` names:

`kind`, `source`, `strength`, `correlation_group`, `scope`,
`observed_path`, `matched_attribute`, `matched_value`.

Three mechanisms prevent correlated signals from inflating confidence:

1. **Clamp at construction.** The stored strength is
   `min(requested, kind ceiling, group ceiling)`. An over-claimed item cannot
   be built — `EvidenceKind::max_strength` and `CorrelationGroup::ceiling`
   bound it.
2. **One vote per correlation group.** Aggregation keeps the strongest item
   *per group*; repeating a signal never adds weight.
3. **Corroboration is capped.** Two or more *independent* groups lift a `Weak`
   best signal to `Moderate` — and no further. `Strong`/`Direct` exist only
   when a single authoritative group supplies them.

Groups: `SourceRecord(source)` and `ObjectIdentity` (ceiling Direct),
`BundleIdentifier` (Strong), `InstallRootStructure` (Moderate), `NameDerived`
(**Weak** — a publisher-directory match plus an application-directory match
plus a filename match that all derive from the same normalized name are ONE
signal).

Consequence: name-derived heuristics alone can never exceed `Weak`.

### Strength vocabulary

`Direct` → `Confidence::Confirmed`; `Strong` → `Strong`; `Moderate` →
`Probable`; `Weak` → `Possible`. `Confidence::Unknown` is not a strength.

### Ownership assessment

`OwnershipAssessment`: `Unknown`, `Weak`, `Moderate`, `Strong`, `Direct`,
`Conflicting`. `Unknown` means no usable evidence — it is not a weak claim.
`Conflicting` is assigned by cross-application conflict detection, never by a
single application's own evidence.

## 8. Relationships: contains ≠ owns

`RelationKind` keeps the semantics distinct:

| Kind | Meaning |
| --- | --- |
| `Contains` | The artifact lies inside the application's install root. **Structural only.** |
| `Owns` | Evidence reaches `Strong`/`Direct`. |
| `AssociatedWith` | Weaker evidence links them. |
| `Executable` | Reserved for the recorded executable relation. |
| `DerivedFrom` | Cache/logs/data produced by the application. |
| `Conflicting` | Credible evidence points at several applications. |

An artifact whose only evidence is `InstallRootContainment` is reported as
`Contains` and can never reach ownership strength.

## 9. Shared artifacts and conflict semantics

`SharedStatus`: `Exclusive`, `Shared`, `Unknown`, `Conflicting`.

* Two or more applications with **credible** (≥ Moderate) evidence → `Shared`.
* Two or more with **strong-or-better** evidence → `Conflicting`.
* Only weak claims → `Unknown`.

Conflict is never silently resolved: all claimants are preserved in
`ArtifactOwnership::claimants`, and the relationship is `Conflicting`. A later
provider can never overwrite an earlier owner. **"Shared" never means "safe to
delete"** — this phase is analysis only.

## 10. Structured explanations

Explanations are machine-readable first. `OwnershipEvidence` carries the
structured facts; `render()` produces the human sentence at the presentation
boundary. A presentation layer may phrase things freely, but the core retains
`evidence_kind`, `source`, `strength`, `correlation_group`, `observed_path`,
and `matched_attribute` for every claim.

## 11. Bounds and complexity

Every collection fed by an unbounded source admits through `BoundedTopK`,
which keeps the canonically-smallest `capacity` keys:

* memory **O(capacity)** — never O(offered);
* `O(log capacity)` per offer;
* the retained set is always the canonically-first `capacity` distinct keys, so
  any permutation of the same offers yields the same set and the same exact
  overflow count.

Bounds in force: `max_records`, `max_evidence_per_candidate`,
`max_children_per_root`, `max_apps_probed`, `max_roots_per_app`,
`max_directories`, `max_entries`, `max_metadata_bytes`, and
`MAX_EXECUTABLE_CANDIDATES`. `AnalysisTruncation` and `FootprintReport` report
every capped item exactly.

## 12. Determinism

Same system state → identical application ids, ordering, sources, footprints,
relationships, evidence, conflicts, and explanations, regardless of provider
order, directory enumeration order, thread scheduling, hash-map iteration
order, or filesystem enumeration order.

Enforced by permutation tests (app-order rotations and reversals, reversed
artifact arrival, reversed provider inputs) plus canonical ordering of every
published collection.

## 13. No hidden I/O

`analyze(apps, artifacts, limits)` is a **pure function** over already-observed
facts (`ObservedArtifact`). It cannot perform I/O because it has no way to.
Every real filesystem read flows through `PathProber`, whose production
implementation (`PlatformPathProber`) sits on the engine's existing
`PlatformFs` boundary and therefore inherits the no-follow content contract.

## 14. Object identity

Artifacts carry the canonical `coresight_identity::ObjectIdentity`
`{ volume, file_id, file_id_hi }` when the platform proves one, and `None`
when it does not — never a narrowed copy, never a second representation, never
a fabricated value. Narrow and wide identities with the same low pair are
distinct and stay distinct. An unproven identity is recorded as a
`CandidateBlocker::UnprovenObjectIdentity`.

## 15. Access-state semantics

`denied != empty != missing != unsupported != failed`. Typed observations
(`DirectoryObservation`, `PathObservation`, `FileObservation`) carry a payload
**only** in a completed-read state; constructors enforce it and
`is_well_formed()` is asserted in tests. A denied directory never produces an
empty footprint without preserving the denial.

## 16. Read-only recommendation primitives

`OwnershipCandidate { kind, target, app, confidence, assessment, blockers,
evidence }` is **inert data**. `CandidateKind` is `UninstallArtifact`,
`Orphan`, `SharedArtifact`, or `UncertainAssociation`. Blockers always include
`NoExecutorInThisPhase`; shared/conflicting artifacts also record
`SharedArtifact`/`ConflictingOwnership`, and unproven identity records
`UnprovenObjectIdentity`.

The presence of a candidate means **none** of: authorized, safe, approved,
executed. `can_authorize_execution()` returns `false` for every analysis, and
a test asserts it.

## 17. No-executor boundary

Phase 6.2 added no delete, uninstall, cleanup, kill, registry/plist write,
config rewrite, package removal, file edit, or command execution — and no
"helper" API for any of them. There is no `CommandRunner`, `ProcessExecutor`,
`DeleteExecutor`, or `UninstallExecutor` under any name. No source invokes
`winget`, `powershell`, `cmd`, `wmic`, `npm`, `pip`, `brew`, `apt`, `dnf`,
`pacman`, `mdfind`, `system_profiler`, `osascript`, or `launchctl`.

Sources that would need such mechanisms are reported as **Unsupported** or
**Deferred** — never approximated.

## 18. Rules for the parser layer

The `Info.plist` reader is a deliberately small flat-key scanner. It performs
**no** DTD processing, **no** entity expansion beyond the five predefined XML
entities, and no recursion, so a hostile plist cannot expand entities, read
outside its buffer, or allocate unboundedly. The `.desktop` reader considers
only the `[Desktop Entry]` group and ignores localized keys so identity stays
stable; a relative or bare `Exec` command is **not** resolved (CoreSight does
not guess `PATH`).

Both parsers are STRICT about text encoding: a value that is not valid UTF-8
becomes absent (flagged via `encoding_invalid`, counted as truncated
knowledge) — never a replacement-character string that could pass as a
plausible application name, version, or identifier. Bundle and desktop-entry
projections reject metadata marked encoding-invalid rather than publishing a
partial identity. Likewise, a non-UTF-8 bundle/`.desktop` file-stem fallback
yields no identity rather than a lossy name.

## 19. Explicit non-scope

* Application persistence, database tables, snapshots, and migrations:
  **NOT STARTED**. The model is in-memory only. No schema was changed.
* GUI work: none added.
* Phase 6.3: the in-memory system model is implemented; independent hardening
  and re-verification are in progress. This does not start Phase 6.4.

## 20. Cross-target verification performed

`cargo check -p coresight-apps -p coresight-capabilities -p coresight-macos
--target x86_64-unknown-linux-gnu` and `--target x86_64-apple-darwin` both
pass from the Windows host, so the shared intelligence layer compiles for all
three platforms. The full-workspace Linux cross-check additionally requires a
cross C toolchain for `libsqlite3-sys` (the history crate's bundled SQLite),
which is not installed on this host; that is a pre-existing environment
limitation and not a property of this layer. Every Rust job in CI builds and
tests the whole workspace natively on Ubuntu, Windows, and macOS.

## 21. Known limitations

* Windows registry reads are runtime-validated only on Windows; macOS bundle
  and Linux `.desktop` reads are compile-verified on every CI platform and
  runtime-validated only on their own OS.
* No distro package database is read; there is no MSIX/AppX enumeration.
* macOS TCC-protected locations remain `RequiresFullDiskAccess`/`Deferred`.
* No real-world hostile-filesystem validation beyond synthetic fixtures.
* `BundleMetadata`/`DesktopEntryMetadata` cover the common flat shapes; an
  input the parser cannot represent yields absent fields rather than guessed
  ones. Malformed text encoding yields absent values flagged by
  `encoding_invalid` — never replacement-character identities.
* Phase 6.3 hardening (shared intelligence layer, no behavior change to
  discovery itself): byte-level ASCII matching for bundle/extension/shape
  checks; strict name-component decoding for root/executable/footprint
  matching; a strengthened shared-crate source-scan guard covering
  `footprint.rs`/`pathmatch.rs`, lossy-decode tokens, and mutating-primitive
  tokens with comment-line stripping.

## 22. Verification record

Local gate on the verified HEAD (Windows host):

```text
cargo fmt --all --check                                     clean
cargo clippy --workspace --all-targets --all-features -- -D warnings   clean
cargo test --workspace                       661 passed, 0 failed (6 ignored)
cargo test --workspace --all-features        661 passed, 0 failed (6 ignored)
npm ci && npm run build                      success (tsc --noEmit + vite)
git diff --check                             clean
cross-target check (apps/capabilities/macos)  linux + darwin compile clean
```

CI for the exact verified commit **23a3f27** — run
[37622477933](https://github.com/OG-Huzzi/SpaceLens/actions/runs/37622477933),
conclusion **success**:

```text
rust (ubuntu-latest)   success
rust (windows-latest)  success
rust (macos-latest)    success
frontend               success
```

An earlier commit in this phase (`afad3bd`) went red on the two Unix jobs
because one new Unix-gated assertion claimed a non-UTF-8 name never
normalizes to the empty matching key; on Unix it does (U+FFFD is not
alphanumeric). That was a test-authoring error, not a product defect — the
behavior was already conservative. Fixed in `23a3f27`, with the invariant
re-stated portably so it is exercised on every platform.

## 23. Performance smoke suites

The repository's existing `--ignored` performance guards were run locally and
pass unchanged: engine, classifier, identity (with the >4 GiB streaming case
skipped locally as CI does in the non-release job), history, and apps. No new
benchmark was added, and no new performance suite is required by this layer —
its bounded operations are covered by the hostile-fixture bounds tests above.

The 4 GiB streaming proof (`huge_file_streams_beyond_4gib`) runs in release
mode in CI only.


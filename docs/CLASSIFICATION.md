# SpaceLens Classification Engine — Phase 2

Status: **IMPLEMENTED + VERIFIED LOCALLY (Windows)** — see
`progress/PHASE_2_STATUS.md` for the authoritative verification record.

## Purpose

Phase 2 is the **UNDERSTAND** layer: it turns raw Phase 1 filesystem
observations into *explainable semantic classifications*. It answers
**"What is this, and why do we believe that?"** — never "what should the user
do with it?" (recommendations belong to later phases).

## Crate layout

```
crates/spacelens-classifier/
  src/
    lib.rs          crate root, contracts, re-exports
    category.rs     Category (17) + Subcategory taxonomy, stable IPC codes
    confidence.rs   Confidence bands + hard caps (test-enforced)
    evidence.rs     EvidenceKind, RuleId, bounded EvidenceList (≤8)
    rules.rs        rule table, tiers, matchers, winner selection
    context.rs      ParentContext + bounded ParentContextTracker
    classify.rs     classify() / classify_streaming() entry points
    aggregate.rs    streaming CategoryAggregator (u64 saturating)
    platform.rs     Platform data enum (single cfg! site)
  tests/
    classifier_tests.rs  integration: categories/conflicts/context/edges
    perf_tests.rs        ignored benchmark: 10k / 100k / 1M entries
```

Dependency chain (docs/ARCHITECTURE.md): `spacelens-engine →
spacelens-classifier`. The classifier is **pure**: no I/O of any kind, no
file-content reads, no network, no database, no UI. It classifies from
path/name/extension/metadata only.

## Semantic categories

17 primary categories, in canonical (aggregation/report) order:
`Applications, Games, Documents, Images, Video, Audio, Downloads, Archives,
Development, TemporaryData, Cache, Logs, Backups, SystemData, UserData,
Other, Unknown`.

These are **engine-internal semantics, not UI labels**. The UI may group them
later (e.g. "Videos" for `Video`); the `code()` strings (`"VIDEO"`,
`"TEMPORARY_DATA"`, …) are the stable v1 IPC surface.

**Unknown vs Other (critical distinction):**
- `Unknown` = insufficient evidence to classify. Reserved; a normal entry
  never lands there silently.
- `Other` = understood enough (e.g. "regular file with no content signals")
  but no more useful primary category applies. Never a dumping ground.

Subcategories (evidence-backed only): `Installer, Archive, DiskImage,
DependencyTree, BuildOutput, VcsInternals, LogFile`.

## Evidence model

Every classification carries typed evidence: `Evidence { kind, rule }` where
`EvidenceKind` (11 variants: `KnownPathPattern`, `KnownCacheLocation`,
`Extension`, `ParentContext`, …) and `RuleId` (34 stable rule identifiers)
are typed enums — never free-form strings. Evidence **never contains path
text** (privacy). The list is bounded at `MAX_EVIDENCE = 8` and ordered
deterministically (winner first, then table order).

## Confidence model

Four deterministic bands: `Unknown < Low < Medium < High`. Two hard caps are
enforced by tests, not convention:

1. **Extension-only evidence can never exceed `Medium`**
   (`Confidence::EXTENSION_ONLY_CAP`). A filename extension alone is weak
   evidence.
2. **Pure heuristics can never exceed `Low`** (`Confidence::HEURISTIC_CAP`).

No fake numeric precision (no "97.38%").

## Rule engine

The table in `rules.rs` is the single source of truth (`RULES: &[Rule]`, 30
rules). Table order is a versioned contract: adding a rule appends; reordering
is a contract change.

### Precedence tiers

| Tier | Meaning                        | Example                       |
|------|--------------------------------|-------------------------------|
| 0    | Authoritative location/system  | Program Files, /usr, Library  |
| 1    | Strong path pattern            | node_modules, .git, Caches    |
| 2    | Canonical user directory       | Downloads, Documents, Desktop |
| 3    | Strong filename pattern        | setup/installer-style names   |
| 4    | Extension table                | .pdf, .png, .zip              |
| 5    | Weak heuristics / fallback     | bare names, plain files       |

Winner selection: **lowest tier → longest matched needle → table order**.
All matching rules are retained as evidence (in table order), so conflicts
remain explainable. Context can raise confidence but **never changes which
rule wins or what category results** — a weak extension can never override
authoritative location evidence.

### Conflict resolution (deterministic, tested)

- `setup.zip`: `InstallerName` (tier 3) beats `ArchiveExtension` (tier 4);
  the losing rule is retained as evidence.
- `__pycache__`: matches both `VirtualenvDir` and `CacheDir` (tier 1);
  longest needle (11 > 5) picks `VirtualenvDir` deterministically.
- Filename-pattern matching is conservative: needle must equal the stem or
  appear as a word-ish prefix (`setup.exe`, `Setup Wizard.exe`,
  `setup_2024.zip`), never a bare substring (`container` ≠ `install`).

## Parent/child context

Two mechanisms, both memory-bounded:

- `ParentContext` — caller-supplied parent knowledge (parent category,
  user-profile flag). Pure; nothing retained.
- `ParentContextTracker` — LRU-capped (4096 entries default) streaming
  helper: feed it classified directories; children can look up their parent's
  category. O(capacity) memory, never O(tree).

Context effects (strictly limited, tested): confidence may rise **one band**
(`Low→Medium`, `Medium→High`); `Other`/`Unknown` buckets are never rescued;
category never changes. Example: `Downloads/setup.exe` with parent context →
`Downloads`/`Installer`/`High` with `ParentContext` evidence.

## Platform handling

Platform knowledge is **data**: `Platform` (Windows/Mac/Linux) is a field on
the classification input. The core engine has exactly **one** `cfg!` site
(`Platform::current()`). Windows/macOS match directory names
case-insensitively; Linux case-sensitively (tested both ways).

Platform rule sets (isolated, tier 0–1):
- **Windows**: `WindowsSystemLocation` (Windows, Program Files,
  ProgramData), `WindowsAppData` (AppData).
- **macOS**: `MacApplicationSupport` (Application Support, Library).
- **Linux**: `XdgLocation` (.local, .config, .share),
  `LinuxPackageLocation` (usr, opt, etc, var, flatpak, snap).

Capitalized variants for user dirs (`Downloads`, `Documents`, `Desktop`)
exist because XDG user dirs are conventionally capitalized on Linux while
Linux name matching is case-sensitive.

Platform rules never leak across platforms (tested: `Program Files` does not
classify on Linux; `.config` does not classify on Windows).

## Aggregation

`CategoryAggregator`: streaming, O(17) memory, `u64` saturating arithmetic
(overflow-impossible, tested with `u64::MAX` inputs). Feed it every
classification; it retains only counters. `report()` emits all 17 categories
in canonical order plus `classified_entries` / `unclassified_entries` for
honest coverage visibility.

## Privacy & security

- No file-content reads (classification is metadata/path-only).
- No network, telemetry, credentials, destructive operations (grep-audited;
  the crate contains no I/O at all).
- Evidence never stores path text.
- Secrets are not detected or inspected; a file named `.env.secret` is
  classified `Other` like any unknown file.

## Performance

Benchmark (`tests/perf_tests.rs`, ignored; run with
`cargo test -p spacelens-classifier -- --ignored --nocapture`):
synthetic mixed workload (hits and misses), classification + streaming
aggregation + context tracking, no disk I/O.

Measured locally (Windows, debug build): ~110k–120k entries/sec, linear
scaling 10k → 1M entries, deterministic aggregation verified across reruns.
No pass/fail throughput gate — the requirements are completion, determinism,
bounded memory, and correctness.

## Current limitations (honest)

- **macOS `.app` bundles**: the `MacAppBundle` rule id is reserved but no
  table rule matches directory *suffixes* yet (`.app` requires suffix
  matching, not exact-name matching). Such directories currently fall through
  to name/extension logic. Documented, tested not to panic.
- **Games category** currently only triggers via `steamapps`/`steamlibrary`
  directory names; no platform game-store library coverage beyond Steam
  conventions.
- **Ambiguous extensions** resolve deterministically rather than perfectly:
  `.ts` claims TypeScript (Development) and is excluded from the video table.
- Rule tables are deliberately small (~30 rules, ~9 extension tables) —
  quality over quantity (master prompt §41). Extension is a small, curated
  set; exotic formats fall to `Other` honestly.
- Timestamps/size are not yet used as evidence (reserved; would need careful
  deterministic design).
- Phase 2 was verified locally on **Windows** only so far; Linux/macOS
  verification happens in GitHub CI (matrix) — see status doc for CI state.

## Future extension points (not implemented)

- Suffix-match rules for app bundles and archive-part patterns.
- Ancestor-chain context (beyond immediate parent).
- Application identity model (`ApplicationIdentity`: vendor/product/id) —
  infrastructure intent documented in the master prompt, not yet built.
- Relationship types (`Contains`, `LocatedUnder`, …) — model space reserved
  in the architecture, no graph implemented.

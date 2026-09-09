# SpaceLens Classification Engine — Phase 2

Status: **REPAIRED + REVERIFIED** (independent audit repair pass) — see
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
    category.rs     Category (18) + Subcategory taxonomy, stable IPC codes
    confidence.rs   Confidence bands + RuleKind + the single cap policy
    evidence.rs     EvidenceKind, RuleId, bounded EvidenceList (≤8)
    pathctx.rs      host-independent path analysis + rooted LOCATION_RULES
    rules.rs        name/extension rule table, tiers, gates, winner selection
    context.rs      ParentContext + bounded genuine-LRU ParentContextTracker
    classify.rs     classify() / classify_streaming() entry points
    aggregate.rs    streaming CategoryAggregator (u64 saturating)
    platform.rs     Platform data enum (single cfg! site)
  tests/
    classifier_tests.rs  integration: categories/conflicts/context/edges
    semantics_tests.rs   audit-repair regression suite (one group per finding)
    perf_tests.rs        ignored benchmark: 10k / 100k / 1M entries
```

Dependency chain (docs/ARCHITECTURE.md): `spacelens-engine →
spacelens-classifier`. The classifier is **pure**: no I/O of any kind, no
file-content reads, no network, no database, no UI. It classifies from
path/name/extension/metadata only.

## Semantic categories

18 primary categories, in canonical (aggregation/report) order:
`Applications, ApplicationData, Games, Documents, Images, Video, Audio,
Downloads, Archives, Development, TemporaryData, Cache, Logs, Backups,
SystemData, UserData, Other, Unknown`.

These are **engine-internal semantics, not UI labels**. The UI may group them
later; the `code()` strings (`"VIDEO"`, `"APPLICATION_DATA"`, …) are the stable
v1 IPC surface.

**`Applications` vs `ApplicationData` (deliberate distinction):** application
*code* (`Program Files`, `/Applications`, `/opt`) and application-*owned data*
(`AppData`, `ProgramData`, `~/Library/Application Support`, `~/.config`,
`~/.local/share`) are different categories. SpaceLens must be able to answer
"how much space does this application use?" without conflating the program
with the data it produced.

**Unknown vs Other (critical distinction):**
- `Other` = the entry is *understood* at a basic level (a regular file, or a
  directory, with a usable name) but no more useful primary category applies.
  This is the normal fallback and is expected to be well-populated.
- `Unknown` = SpaceLens genuinely lacks trustworthy information. Reachable by
  exactly three detectable conditions, all tested:
  1. the observation carries an error (metadata incomplete),
  2. no usable name can be derived from the path,
  3. the entry kind is uninterpretable (socket/FIFO/device) **and** no rule
     matched.
  `Unknown` is never manufactured merely to make the enum reachable.

Subcategories (evidence-backed only): `Installer, Archive, DiskImage,
DependencyTree, BuildOutput, VcsInternals, LogFile`.

## Two strengths of knowledge

The central semantic decision of the audit repair. Classifying an entry by its
**basename alone** is deterministic but not accurate: `/home/user/project/cache`
and `~/Library/Caches` are not the same claim. The classifier therefore
separates:

- **Authoritative location knowledge** — *rooted* path prefixes the platform
  defines (`C:/Program Files`, `C:/Users/<u>/AppData`, `/usr`, `/var/log`,
  `/Applications`, `/home/<u>/.cache`). Lives in `pathctx::LOCATION_RULES`.
- **Weak basename heuristics** — bare names that merely *look* like something
  (`build`, `out`, `cache`, `backup`, `tmp`, `logs`, `setup`). Live in
  `rules::RULES` with `RuleKind::Heuristic`.

Only the first may reach `High` confidence. A weak heuristic never masquerades
as authoritative knowledge.

`LocationClass::UserHome` is a **pure container**: it tells us *whose* tree we
are in, never *what* an entry is. It neither corroborates heuristics nor passes
its category to unremarkable contents — so `/home/user/randomdir` stays
`Other`/`Low` rather than silently becoming `UserData`.

## Evidence model

Every classification carries typed evidence: `Evidence { kind, rule }` where
`EvidenceKind` (11 variants) and `RuleId` are typed enums — never free-form
strings. Evidence **never contains path text** (privacy). The list is bounded
at `MAX_EVIDENCE = 8` and ordered deterministically (winner first, then table
order).

**Evidence fidelity (audit finding 4):** the evidence kind is captured **at
match time** inside `rules::evaluate` as a typed `RuleMatch` — never
reconstructed afterwards. Every emitted evidence item truthfully describes the
signal that caused its associated rule to match: `setup.zip` produces
`Extension → ArchiveExtension` *and* `FilenamePattern → InstallerName`, not a
`KnownPathPattern` placeholder for the losing rule. A rule id reachable through
two mechanisms (a rooted location and a bare name, e.g. `CacheDir`) declares
its kind per mechanism, and the emitted kind says which one fired.

## Confidence model

Four deterministic bands: `Unknown < Low < Medium < High`. The policy is
**data, not convention**: every rule carries a `RuleKind`, and
`Confidence::cap_for(kind)` is the single source of truth for its ceiling:

| `RuleKind`   | Hard ceiling                                             |
|--------------|----------------------------------------------------------|
| `Authoritative` | `High`                                                |
| `Extension`     | `Medium` (absolute: corroboration cannot lift it)     |
| `Heuristic`     | `Low`, or `Medium` when corroborated                   |

The clamp is applied in exactly one place (`classify()`): base confidence is
min'd with the cap, then context may raise **at most one band**, clamped back
to the same cap. A table test asserts every ungated rule declares a base
confidence within its kind's ceiling, so the table cannot lie.

`ParentContext` may raise confidence one band **only if** the parent itself was
classified into a semantic (non-bucket) category. Buckets are never rescued,
categories never change, and `under_user_profile` alone is not corroboration.

### Installer extensions are gated to Downloads (Phase 2.1)

`InstallerExtension` (`.msi`, `.dmg`, `.pkg`, `.deb`, `.rpm`, `.apk`, `.msp`,
`.msu`) is gated `Under(Downloads)`, exactly like `InstallerName`. The
rationale: the extension says what the *bytes* are, but `Downloads` is a claim
about where a file **came from** — an extension alone cannot make that claim.
Consequently:

- `C:/Users/u/Downloads/blob.msi` → `Downloads`/`Installer`/`High` (the
  download location corroborates the gated winner).
- `C:/Program Files/App/setup.msi` → `Applications` (install-tree location
  wins; the installer signal survives as truthful evidence).
- `/data/blob.msi` → `Other`/`Low` with the extension on the evidence record
  (honest "understood but unclassified", never a guess).

**`.appimage` moved to the executable table (Phase 2.1):** an AppImage *is*
the application — it executes directly, with no installer step — so it now
classifies as `Applications` via `ExecutableExtension` wherever it is found,
and carries no `Installer` subcategory.

### macOS Library hierarchy (Phase 2.1)

Both Library tiers are recognised with their own specific patterns:

- System-wide: `/Library/Caches` → `Cache`, `/Library/Logs` → `Logs` — the
  broad `/library` (ApplicationData) rule says only "application-owned data"
  and must not swallow the more specific trees. Most-specific-pattern-wins by
  depth does the work; no macOS-specific logic exists outside the rule table.
- Per-user: `~/Library/Caches`, `~/Library/Logs` unchanged.
- `~/Applications` is now a rooted install location (`Applications`), matching
  `/Applications` semantics for the per-user tree.

### Path component handling (Phase 2.1)

Empty components (leading/trailing/repeated separators) and `.` components are
transparent for rooted location matching (`C:/Users/./u/Downloads` ≡
`C:/Users/u/Downloads`). `..` is deliberately **not** normalised — resolving
it would manufacture location knowledge the raw path does not assert, so an
anchored match simply fails for such paths.

## Rule engine

Two tables:

- `rules::RULES` — name/extension rules matched against the entry's basename.
- `pathctx::LOCATION_RULES` — rooted patterns matched against the entry's full
  path, per platform, with `*` single-component wildcards. The most specific
  (longest) matching pattern wins.

### Precedence tiers (`rules::RULES`)

| Tier | Meaning                     | Example                            |
|------|-----------------------------|------------------------------------|
| 1    | Authoritative path pattern  | node_modules, .git, steamapps      |
| 2    | Canonical user directory    | Downloads, Documents, Desktop      |
| 4    | Content-typed extension     | .pdf, .png, .zip, .log, .rs, .msi (gated) |
| 5    | Generic / weak file signal  | `setup`-style names (gated), `.exe` |
| 6    | Authoritative container location | entry sits in `~/Library/Caches` |

Tiers 0 and 3 are intentionally unused (numbering left stable; see `rules.rs`
module docs). Winner selection among **eligible** rules: **lowest tier →
longest matched needle → table order**.

**Why installer names sit *below* content extensions (audit finding 1):** a
name that looks like an installer is weaker evidence than an extension that
says what the bytes are. `setup.zip` stays `Archives` **everywhere** —
including inside Downloads — and `update.log` stays `Logs`.
`setup.exe` in Downloads is still a downloaded installer: both `setup` and
`.exe` are generic tier-5 signals, and the longer installer needle wins the
tier tie.

### Gating

A rule may declare a `RuleGate`. A gated rule **matches** (and is retained as
truthful evidence) but is **not eligible to win** unless its gate is satisfied.
`InstallerName` and `InstallerExtension` are both gated `Under(Downloads)`: an
installer signal only decides the category when an authoritative download
location vouches for it. A gated rule that wins is corroborated by
construction — the location supplies the confidence, so the ceiling is `High`
and one band is earned at win time.

### Conflict resolution (deterministic, tested)

- `setup.zip` (anywhere): `ArchiveExtension` (tier 4) beats `InstallerName`
  (tier 5, gated); the name is retained as `FilenamePattern` evidence.
- `setup.exe` in Downloads: `InstallerName` wins the tier-5 tie by needle
  length → `Downloads`/`Installer`/`High`.
- `update.exe`/`uninstall.exe` in `Program Files/App`: `ExecutableExtension`
  → `Applications`. Application update/uninstall artifacts are never
  "Downloads".
- `__pycache__`: matches both `VirtualenvDir` and `CacheDir` (tier 1); longest
  needle picks `VirtualenvDir`.
- Filename-pattern matching is conservative: needle must equal the stem or
  appear as a word-ish prefix (`setup.exe`, `Setup Wizard.exe`,
  `setup_2024.zip`), never a bare substring (`container` ≠ `install`).

### Deliberate needle removals (false-positive control)

- **`bin`** was removed from `BuildDir`: it is a system directory name on Unix
  (`/bin`, `/usr/bin`) as often as a build-output name. `/usr/bin` is now
  correctly `SystemData`/`High` via its rooted location; a bare `bin`
  elsewhere is honestly `Other` rather than a confident guess.
- **`env`** was removed from `VirtualenvDir`: a directory merely named `env`
  is too ambiguous to claim authoritative `High` confidence. The unambiguous
  names (`.venv`, `__pycache__`, `site-packages`) carry the claim alone.

## Parent/child context

Two mechanisms, both memory-bounded:

- `ParentContext` — caller-supplied parent knowledge (parent category,
  user-profile flag). Pure; nothing retained. The user-profile flag is derived
  from the rooted location analysis, not a substring scan of the raw path.
- `ParentContextTracker` — a **genuine LRU** (audit finding 3): an intrusive
  doubly-linked recency list over a fixed slot pool (`HashMap` index). A
  successful lookup *refreshes* recency, so the eviction victim is always the
  least recently used key — never merely the oldest inserted. O(1) operations,
  O(capacity) memory (default 4096), never O(tree). The requested capacity is
  clamped to the hard bound `MAX_ENTRIES` (Phase 2.1): a caller cannot defeat
  the "hostile tree cannot grow this structure" guarantee by passing a huge
  capacity. The canonical proof test: capacity 3, insert A B C, look up A,
  insert D → B is evicted, A survives.

Context effects (strictly limited, tested): confidence may rise one band
inside the cap; buckets are never rescued; category never changes.

## Platform handling

Platform knowledge is **data**: `Platform` (Windows/Mac/Linux) is a field on
the classification input. The core engine has exactly **one** `cfg!` site
(`Platform::current()`). Windows/macOS match directory names
case-insensitively; Linux case-sensitively (tested both ways).

**Host independence (audit finding 9):** paths are split on **both** `/` and
`\` with `C:`-style drive tokens discarded — no `std::path` separator
semantics are involved anywhere. A synthetic Windows path (`C:/Users/u/...`
or `C:\Users\u\...`) yields identical classifications on a Linux or macOS
host. Platform rules never leak across platforms (tested).

Rooted location knowledge per platform (selection):
- **Windows**: `/windows` (System), `/program files`, `/program files (x86)`
  (ApplicationInstall), `/programdata`, `/users/*/appdata` (ApplicationData),
  `/users/*/appdata/local/temp`, `/windows/temp` (Temporary), `/users/*/downloads`,
  `/users/*/documents`, `/users/*/desktop`, `/users/*` (UserHome).
- **macOS**: `/system`, `/private` (System), `/applications`,
  `/users/*/applications` (ApplicationInstall), `/library`,
  `/users/*/library`, `/users/*/library/application support` (ApplicationData),
  `/library/caches`, `/users/*/library/caches` (Cache), `/library/logs`,
  `/users/*/library/logs` (Logs), `/tmp`, `/var/folders` (Temporary),
  `/users/*` (UserHome).
- **Linux**: `/usr`, `/etc`, `/var`, `/bin`, `/sbin`, `/lib`, `/lib64`
  (System — deliberately *not* collapsed with `/opt`), `/opt`, `/snap`
  (ApplicationInstall), `/var/log` (Logs), `/var/cache` (Cache), `/tmp`,
  `/var/tmp` (Temporary), `/home/*`, `/root` (UserHome), `/home/*/.cache`
  (Cache), `/home/*/.config`, `/home/*/.local/share` (ApplicationData), plus
  `downloads`/`Downloads`, `documents`/`Documents`, `desktop`/`Desktop`
  (Linux name matching is case-sensitive, so both spellings are listed).

## Aggregation

`CategoryAggregator`: streaming, O(`Category::COUNT` = 18) memory, `u64`
saturating arithmetic (overflow-impossible, tested with `u64::MAX` inputs).
Feed it every classification; it retains only counters. `report()` emits all
18 categories in canonical order plus `classified_entries` /
`unclassified_entries` for honest coverage visibility.

## Privacy & security

- No file-content reads (classification is metadata/path-only).
- No network, telemetry, credentials, destructive operations; the crate
  contains no I/O at all (grep-audited).
- Evidence never stores path text (tested by serializing classifications of
  sensitive-looking paths and grepping the JSON).
- Secrets are not detected or inspected; a file named `.env.secret` is
  classified `Other` like any unknown file.

## Performance

Benchmark (`tests/perf_tests.rs`, ignored; run with
`cargo test -p spacelens-classifier -- --ignored --nocapture`): a deliberately
varied synthetic workload — rooted locations on all three platforms, installer
name/extension conflicts, parent/child chains, both path separators —
classification + streaming aggregation + context tracking, no disk I/O.

Measured locally (Windows, debug build): ~56k entries/sec, **linear** scaling
10k → 100k → 1M (per-entry cost stable to within noise), deterministic
aggregation verified across reruns, tracker memory bounded under a 500k-entry
hostile stream. A non-ignored companion test pins determinism and evidence
boundedness at 20k entries. No pass/fail throughput gate — the requirements
are completion, near-linear scaling, bounded memory, and correctness.

## Current limitations (honest)

- **macOS `.app` bundles**: no table rule matches directory *suffixes* — a
  bundle is an installed application because of **where it lives**
  (`/Applications`, `~/Applications`), never because of its name. A `.app`
  directory outside an install location is honestly `Other` (deliberate:
  suffix-only classification would over-claim). Phase 2.1 pins the exact
  semantics with tests, including `~/Applications` as a per-user install
  location.
- **Games** currently only triggers via `steamapps`/`steamlibrary` directory
  names; no platform game-store library coverage beyond Steam conventions.
- **Ambiguous extensions** resolve deterministically rather than perfectly:
  `.ts` claims TypeScript (Development) and is excluded from the video table.
- A directory named `cache` inside an application data tree
  (`AppData/Local/App/cache`) classifies as `Cache` (tier 1 name) rather than
  `ApplicationData` (tier 6 location) — the name is the more specific claim
  and wins by tier order. Low confidence unless corroborated.
- Timestamps/size are not yet used as evidence (reserved; would need careful
  deterministic design).
- Local verification ran on Windows; Linux/macOS verification happens in
  GitHub CI (matrix) — see the status doc for actual CI results.

## Future extension points (not implemented)

- Archive-part patterns (`.part`, `.r00`-style multi-volume names). App-bundle
  suffix matching is deliberately rejected (Phase 2.1): a bundle is an
  installed application because of its location, not its name.
- Ancestor-chain context (beyond immediate parent).
- Application identity model (`ApplicationIdentity`: vendor/product/id) —
  infrastructure intent documented in the master prompt, not yet built.
- Relationship types (`Contains`, `LocatedUnder`, …) — model space reserved
  in the architecture, no graph implemented.

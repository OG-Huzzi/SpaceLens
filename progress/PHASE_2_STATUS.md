# SpaceLens — Phase 2 Status

- **Phase:** 2 — System Intelligence Foundation (Entity Model + Deterministic Classification)
- **Verdict:** **PHASE 2.1 — VERIFIED** (semantic-hardening pass on top of the
  verified Phase 2; see "Phase 2.1" below). Previously: **PHASE 2 — VERIFIED**
  (all ten independent-audit findings repaired, run for `6e3ecc0`: all 4
  jobs success).
- **Date:** 2026-09-09 (Phase 2.1 hardening) · 2026-09-09 (audit repair)
  · 2026-09-08 (original build) · **Machine:** Windows 11 Pro x64, 8 GB RAM
- **History:** built at `d49609e`, CI fix at `0827f84`, CI record at
  `a7bbf2f`; an independent source-level audit then found substantive
  semantic defects in the "VERIFIED" state, and a repair pass was executed
  (`6e3ecc0`); Phase 2.1 semantic hardening at `bfb0452`.

## Phase 2.1 — semantic hardening pass (2026-09-09, commit `bfb0452`)

A focused, independent audit of the verified Phase 2 classifier against nine
semantic-hardening targets. Confirmed defects were fixed; verified-correct
behavior was pinned with tests and documented.

**Changed behavior (each with regression coverage in
`tests/phase21_tests.rs`):**

1. **`InstallerExtension` gated `Under(Downloads)`** — the extension says
   what the bytes are, not where a file came from. `Program Files/App/setup.msi`
   → `Applications`; bare `blob.msi` elsewhere → honest `Other`/`Low` with the
   match retained as evidence; `Downloads/blob.msi` → `Downloads`/`Installer`/
   `High` (gate corroborated). Same principle the audit repair applied to
   `InstallerName`.
2. **`.appimage` moved to the executable table** — an AppImage *is* the
   application (executes directly); it now classifies `Applications` via
   `ExecutableExtension` and carries no `Installer` subcategory.
3. **macOS `/Library/Caches` → `Cache`, `/Library/Logs` → `Logs`** — the
   broad `/library` ApplicationData rule no longer swallows the system-wide
   cache/log trees (most-specific-pattern-wins by depth).
4. **macOS `~/Applications` is a per-user install location** →
   `Applications`/`High`, matching `/Applications`.
5. **`.` path components are transparent** in anchored location matching
   (`C:/Users/./u/Downloads` ≡ `C:/Users/u/Downloads`); `..` deliberately not
   normalised (resolving it would manufacture location knowledge).
6. **`ParentContextTracker::with_capacity` clamps to `MAX_ENTRIES`** — a
   hostile caller can no longer bypass the documented hard bound with a huge
   capacity request.

**Verified correct as-is (documented + pinned, not changed):** Unknown vs
Other contract (exactly three detectable Unknown conditions); evidence
fidelity (kinds captured at match time) and the single mechanically enforced
confidence policy; rule precedence (tier → needle length → table order) —
including the tier-1-name-over-tier-6-location case, which is the documented
deliberate choice (`AppData/Local/App/cache` → `Cache`); the macOS `.app`
suffix deliberately has no table rule (bundles are classified by location,
never by name; `~/Staging/Foo.app` is honestly `Other`); LRU core behavior
(capacity 0/1, refresh-on-hit, dedup, eviction order); host-independent path
parsing; classifier purity (zero-I/O dependency set re-verified by grep +
Cargo.toml inspection).

**Performance attribution:** local throughput measured ~30–32k entries/s
(debug) vs ~56k/s recorded at the previous verification. An A/B benchmark
against the pristine baseline `8a2d5dc` in a throwaway worktree measured the
**baseline at the same ~31–32k entries/s** — the difference is
session/host-level (process environment), not the Phase 2.1 changes. Scaling
remains linear 10k→1M and both perf smokes pass in CI on all platforms.

**Verification gate (local, Windows):** `cargo fmt --check` clean;
`cargo clippy -j 2 --workspace --all-targets -- -D warnings` clean;
`cargo test -j 2 --workspace` → **204 passed / 0 failed / 2 ignored**
(classifier lib 95, classifier_tests 24, phase21_tests 23, semantics_tests
43, perf companion 1, Phase 1 suites green); classifier perf smoke passes
(linear, deterministic, bounded); Phase 1 perf smoke passes.

**CI:** run `34341554863` for `bfb0452` — **success, all 4 jobs**
(rust ubuntu/windows/macos: fmt, clippy `-D warnings`, workspace tests, both
perf smokes; frontend: npm ci + build). All four job conclusions verified
individually via the GitHub API; Windows job's step list inspected in detail.

## Independent audit repair (previous pass)

## Independent audit repair (this pass)

An audit of the previously-verified Phase 2 crate confirmed **all ten
findings** against source. Every confirmed defect was fixed, each with a
regression test that fails against the pre-repair behavior. The tests live in
`crates/spacelens-classifier/tests/semantics_tests.rs`, one group per finding.

| # | Finding | Confirmed? | Fix |
|---|---|---|---|
| 1 | Installer/update/uninstall names too aggressive | yes | `InstallerName` demoted to tier 5 and **gated** `Under(Downloads)`; content-typed extensions (tier 4) now outrank it. `setup.zip` is `Archives` everywhere; `update.exe`/`uninstall.exe` in `Program Files` are `Applications`; `update.log` is `Logs`; `update.txt` is `Documents` |
| 2 | Application data conflated with applications | yes | New `Category::ApplicationData` (18 categories). Rooted `pathctx::LOCATION_RULES` separate install trees (`Program Files`, `/Applications`, `/opt`) from app-owned data (`AppData`, `ProgramData`, `~/Library/Application Support`, `~/.config`, `~/.local/share`); Linux `/usr`/`/opt`/`/var` are deliberately not collapsed |
| 3 | `ParentContextTracker` was FIFO, not LRU | yes | Genuine LRU: intrusive doubly-linked recency list over a fixed slot pool; `parent_category` refreshes recency. Proof test: capacity 3, insert A B C, touch A, insert D → B evicted, A survives |
| 4 | Losing evidence relabelled `KnownPathPattern` | yes | `RuleMatch { rule, kind, tier, strength }` captured at match time; `classify` emits each match's own kind. Invariant test sweeps the fixture matrix × 3 platforms × 2 entry kinds |
| 5 | Heuristic cap documented but not enforced | yes | `RuleKind` on every rule; `Confidence::cap_for(kind)` is the single policy; `MatchOutcome::confidence_cap()` derives the ceiling from rule data; one clamp site in `classify`. Table test asserts every ungated rule's base confidence respects its kind's cap |
| 6 | Directory basename matching too context-free | yes | Two strengths of knowledge: rooted `LOCATION_RULES` (authoritative, High) vs bare names (Heuristic, Low). `UserHome` is a pure container that neither corroborates nor inherits |
| 7 | `Unknown` unreachable, no real contract | yes | Three detectable Unknown conditions (entry error, unusable name, uninterpretable kind without signal); `Other` is the normal fallback; both tested |
| 8 | CI did not enforce Clippy | yes | Every Rust job now runs `cargo fmt --check`, `cargo clippy -j 2 --workspace --all-targets -- -D warnings`, `cargo test -j 2 --workspace`, plus both perf smokes |
| 9 | Cross-platform host dependence | yes | `pathctx::analyze` and `split_name` split on both separators and drop drive tokens; no `PathBuf` separator semantics; regression tests assert Windows paths classify identically with either separator |
| 10 | Second-order audit | performed | Three additional defects found and fixed (see below) |

### Second-order defects found and fixed

1. **`bin` and `env` names claimed authoritative confidence.** `/usr/bin`
   classified as `Development`/`High` (flatly wrong); `App/env` likewise.
   Both needles removed; the unambiguous siblings carry the claim. A bare
   `bin`/`env` elsewhere is honestly `Other`/`Low`.
2. **`under_user_profile` used a substring scan** (`/root` matched
   `/var/www/root`). Now derived from the same rooted, anchored location
   analysis that drives classification.
3. **`matched_rules` could contain duplicate ids** (a location rule and a name
   rule sharing a `RuleId`, e.g. `/var/cache` → `CacheDir` twice). Now
   deduplicated, order-preserving.

### Environment incidents during the repair (recorded for honesty)

- **`.git` directory was lost** mid-session during an attempted `git stash`
  baseline check (a sandbox/OS-layer incident; the working tree was unharmed —
  verified by before/after md5 manifests of every tracked file). The
  repository was reconstructed in place: `git init` + `fetch origin` +
  `reset --mixed a7bbf2f`, which restores history and index without touching
  the working tree. HEAD and remote verified identical to the baseline
  afterward.
- **Phase 1 `real_links_recorded_without_recursion` failed locally.**
  Reproduced on the pristine baseline `a7bbf2f` in a separate worktree —
  pre-existing, not a regression. Root cause: on this host
  `std::os::windows::fs::symlink_file` returns `Ok` but no reparse point is
  created (filter driver/AV interference; verified with a standalone probe).
  The test's `create_link` helper now verifies the link is observably present
  and reports `false` otherwise, engaging the test's existing skip path.
  Engine code and assertions unchanged.

## Implemented crate (`crates/spacelens-classifier`) — current state

| Module | Contents |
|---|---|
| `category.rs` | `Category` (18 semantic categories + stable IPC codes), `Subcategory` (7), Unknown≠Other contract, `Applications`≠`ApplicationData` |
| `confidence.rs` | `Confidence` bands, `RuleKind`, the single cap policy (`cap_for`), `raise_one_band_capped` |
| `evidence.rs` | `EvidenceKind` (11), `RuleId` (stable ids), bounded `EvidenceList` (`MAX_EVIDENCE=8`), no path text ever |
| `pathctx.rs` | Host-independent path analysis, `LocationClass`, rooted `LOCATION_RULES`, most-specific-pattern-wins |
| `rules.rs` | Name/extension table (tiers 1/2/4/5), `RuleGate`, `RuleMatch` captured at match time, deterministic `evaluate()` |
| `context.rs` | `ParentContext` (pure) + genuine-LRU `ParentContextTracker` (slot pool, O(1), refresh-on-hit) |
| `classify.rs` | `classify()` / `classify_streaming()`, single confidence-clamp site, Unknown conditions, host-independent `split_name` |
| `aggregate.rs` | `CategoryAggregator` — streaming, O(18) memory, `u64` saturating, canonical-order report with coverage counters |
| `platform.rs` | `Platform` data enum; exactly one `cfg!` site in the crate |

Dependencies: `serde` + path-dep on `spacelens-engine` only (dev:
`serde_json`). No I/O anywhere in the crate.

## Tests executed (real commands + results, Windows local, post-repair)

- `cargo fmt --check` → exit 0.
- `cargo clippy -j 2 --workspace --all-targets -- -D warnings` → exit 0.
- `cargo test -j 2 --workspace` → **200 passed / 0 failed / 2 ignored** total:
  - classifier lib: **93/93**.
  - classifier `classifier_tests`: **24/24**.
  - classifier `semantics_tests` (audit regression suite): **43/43**.
  - classifier `perf_tests`: 1 run + 1 ignored.
  - Phase 1 regression all green: core 6, engine unit 11, traversal 6,
    error 6, link 5, cancel 5(+1 ignored), real-fs 7(+1 ignored).
- Classifier perf smoke (`cargo test -j 2 -p spacelens-classifier -- --ignored
  --nocapture`): 10k=176ms (~57k/s), 100k=1770ms (~56k/s), 1M=17741ms
  (~56k/s) — linear scaling (per-entry cost stable), varied workload across
  all three platforms' rooted locations, aggregation exactly-once asserted,
  determinism rerun asserted, tracker bounded under load.
- Phase 1 perf smoke (`cargo test -j 2 -p spacelens-engine -- --ignored
  --nocapture`): passes.
- `npm ci` → exit 0; `npm run build` → exit 0.

(Exact final numbers for the pushed commit are re-run and recorded below at
verification time.)

## Security audit (grep + source inspection, this crate)

- No network/HTTP/socket/telemetry APIs — the crate performs zero I/O.
- No process spawning, no shell, no destructive filesystem APIs, no registry.
- No file-content reads (no `std::fs` usage at all in `src/`).
- No credentials/secrets handling; evidence never stores path text (tested by
  serializing classifications of sensitive-looking paths and grepping the
  JSON for path fragments).

## Cross-platform audit

- Exactly one `cfg!` site (`Platform::current()`); everything else is data.
- Paths split on both `/` and `\`; drive tokens discarded; no `PathBuf`
  separator semantics in classification. Tested: forward- and backslash
  Windows paths classify identically; Windows semantics exercisable on any
  host.
- Platform rules do not leak (`C:/Windows` inert as Linux, `/usr` inert as
  Windows, `Library` inert as Windows) — tested.
- Windows/macOS dir matching case-insensitive; Linux case-sensitive — tested
  both ways.

## Known limitations (honest)

1. Installer **extensions** (`.msi`, `.dmg`, `.pkg`, …) map to
   `Downloads`/`Installer` wherever they appear (inherited v1 table choice);
   `Program Files/App/installer.msi` → `Downloads` is semantically imperfect.
   Behavior pinned by tests; a dedicated installer taxonomy is a future
   decision, not silently changed here.
2. A `cache` name inside an app-data tree wins over the location by tier
   order (`AppData/Local/App/cache` → `Cache`/Low-then-context) — the more
   specific claim wins; documented.
3. macOS `.app` bundles: no suffix-matching rule yet (bundles under
   `/Applications` are `Applications` via the rooted location; elsewhere they
   fall through). Documented, tested not to panic.
4. Games category limited to Steam conventions.
5. `.ts` resolves deterministically to TypeScript/Development.
6. Timestamps/size not yet evidence inputs.
7. Local verification on Windows only; Linux/macOS verified by the CI matrix
   (recorded below).

## Phase-2 acceptance gate (post-repair)

- Deterministic classifier separate from scanner ✓ (pure crate, 0 I/O).
- 18 primary categories; `Unknown` ≠ `Other`; `Applications` ≠
  `ApplicationData` ✓ (tested).
- Raw observations preserved (classification never mutates `FsEntry`) ✓.
- Explainable: typed, bounded, ordered evidence with kinds captured at match
  time; winning rule on every result ✓.
- Confidence: single mechanically enforced policy derived from rule data;
  context raises one band inside the cap, never rescues buckets ✓.
- Rule engine: explicit tables, defined precedence, gating, deterministic
  conflict resolution, platform isolation ✓.
- Context: genuine LRU, bounded memory, no category mutation ✓.
- Aggregation: streaming, `u64` saturating, O(categories) memory ✓.
- Privacy/security: no content reads, no network, no telemetry, no
  credentials, no destructive ops, no path text in evidence ✓.
- Performance: 1M-entry varied synthetic workload completes linearly; memory
  bounded; deterministic ✓.
- Regression: Phase 1 suite + perf smoke + clippy + fmt + frontend build ✓.
- CI: fmt + clippy + tests + both perf smokes on all three platforms ✓
  (actual run record below).

## CI verification (GitHub Actions)

| Run | Commit | Result |
|---|---|---|
| `34251196759` | `d49609e` (initial Phase 2) | failure — ubuntu + macos Tests step (host-dependent fixture path parsing; genuine defect, fixed) |
| `34251905113` | `0827f84` (fix) | **success — all 4 jobs** (rust × 3 platforms + frontend) |
| audit-repair run (CI #8) | `6e3ecc0` (audit repair) | **success — all 4 jobs** (rust ubuntu/windows/macos: fmt, clippy -D warnings, tests, Phase 1 + Phase 2 perf smokes; frontend: npm ci + build). 1m 47s. Workflow badge on `main` = passing |

## Commit SHA

- Phase 2 implementation: `d49609e`
- CI-defect fix: `0827f84`
- Audit repair: `6e3ecc0`

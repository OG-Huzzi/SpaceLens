# SpaceLens — Phase 3.2 Status

- **Phase:** 3.2 — Windows object identity & path-chain TOCTOU hardening
  (closes the Windows identity gap Phase 3.1 documented as limitation)
- **Verdict:** **PHASE 3.2 VERIFIED** (full local gate green on Windows;
  CI matrix green — runs recorded below; every job verified individually)
- **Date:** 2026-09-11 · **Machine:** Windows 11 Pro x64, 8 GB RAM
- **Starting SHA:** `e290c0b` (Phase 3.1 record, clean tree)

## Why this phase existed

Phase 3.1 hardened the identity engine but documented an honest gap: on
Windows, scan-time object identity was unavailable (std's path-stat surface
does not expose the volume serial / file index on stable), so the
observed-vs-opened replacement check ran degraded there, and intermediate
path components resolved normally at hash time — a parent directory swapped
for a junction could redirect the open before any identity comparison could
help. This phase closes both gaps without weakening anything Phase 3.1
established.

## What was built (all within Observation → Classification → Identity → Relationships)

**Objective 1 — Windows scan-time object identity (`spacelens-engine`):**

- `windows::identity_via_query_handle`: `CreateFileW` with
  `FILE_READ_ATTRIBUTES` (no data access, share-all — never blocks
  writers), `OPEN_EXISTING`, `FILE_FLAG_OPEN_REPARSE_POINT |
  FILE_FLAG_BACKUP_SEMANTICS`, reading `FILE_ID_INFO` →
  `(VolumeSerialNumber, 128-bit FileId)`. `OPEN_REPARSE_POINT` observes a
  reparse point itself, matching the record-only link policy; failure
  degrades to `None` — never fabricated.
- Identity derivation is identical for scan time and hash time
  (`handle_identity` uses the same FileIdInfo-first path), so comparisons
  are always like-for-like.
- On NTFS the file id embeds the MFT record reference **including its
  sequence number**, which increments on record reuse — the same-content
  delete+recreate impostor is detectable on Windows (and on Unix the mtime
  bracket backstops the same attack).
- Model extensions (additive, v1 contract): `FsEntry.file_id_hi`,
  `MetadataInfo.file_id_hi`, `FileIdentity.file_id_hi`.
- `windows-sys` gains the `Win32_Security` feature (SECURITY_ATTRIBUTES
  parameter type). unsafe remains confined to the audited Win32 FFI in
  `platform/windows.rs`; the identity crate stays 100% safe.

**Objective 2 — hash-time identity validation:**

- The pipeline compares `(volume, file-id)` plus the high bits where both
  sides proved them; mismatch → typed `HashFailureKind::Replaced`. All
  Phase 3.1 checks remain (size, bytes-read, pre/post brackets, final
  reparse protection, regular-file validation).
- New **scan→open mtime bracket**: the handle's modification stamp must
  equal the scanner's observation where both are provable — catches
  same-content replacements whose identity evidence is weaker (FAT-class
  filesystems).

**Objective 3 — intermediate-path / reparse safety:**

- **Ancestor-chain guard on both platforms:** before the final open, every
  ancestor prefix is opened no-follow (`O_NOFOLLOW|O_DIRECTORY` on Unix;
  `OPEN_REPARSE_POINT` + attribute inspection on Windows). A directory
  anywhere in the chain that became a link is refused `UnexpectedLink` →
  typed failure — the junction target is never touched.
- No false positives: `SymlinkPolicy::RecordOnly` never descends into
  links, so engine-observed paths can never contain link ancestors.
- Design rationale (documented in docs/IDENTITY.md): the authoritative
  proof that the hashed object is the observed object is the final-object
  identity comparison — an intermediate junction that resolves the path
  elsewhere lands on a *different object* and fails the comparison. The
  ancestor guard adds the deterministic structural refusal and covers
  degraded mode. Handle-relative traversal (`NtOpenFile` root-directory
  chains) is the airtight structural form and is deliberately not used —
  the identity comparison already answers the question, with less
  platform surface (residual window between independent component checks
  is documented, not claimed away).

**Objectives 4–7 — adversarial tests (11, all deterministic):**
observe → prepare → swap → hash → assert; no thread races. Every target
file gets a same-size twin so it becomes a hash candidate (size singletons
are never hashed — Phase 3 semantics). Junctions via `cmd /c mklink /J`
(no admin privilege); paths joined only via `join()` (mixed separators
make mklink parse a path component as a switch — a lesson recorded in the
test file).

- scan captures identity for files AND directories (win + unix)
- same-content replacement via concurrent-create + rename → `Replaced`
- same-content delete+recreate → rejected (NTFS sequence number / mtime
  bracket; kind may be `Replaced` or `Changed` by platform)
- Case A (parent → other normal dir, same-name/size/content impostor) →
  `Replaced`
- Case B (parent → junction) → refused, target untouched
- Case C (multi-level swap, mirror-structured impostor tree) → `Replaced`
- intermediate junction redirect → refused
- Unix twin: symlinked ancestor refused
- degraded mode (identity stripped): junction parent still refused
- hard links: same scan identity, one object, zero recoverable, Exact —
  never typed `Replaced`
- distinct same-content copies still group with Exact accounting (no
  over-rejection); redirected same-content file never attached to the
  original observation

**Objective 9 — error semantics:** unchanged typed model — `Replaced`,
`Changed`, `Vanished`, `UnexpectedLink`/`NotRegularFile` refusals,
`Hash { category }`, `Cancelled`. No generic "hash failed" collapse.

**Objective 10 — boundedness:** no new unbounded structures. Staged
records grew by fixed-size fields (mtime + high id bits ≈ 24 bytes within
the existing global caps); the ancestor guard allocates one transient
`PathBuf` per hash call. All Phase 3.1 caps, counters, statuses,
cancellation, and deterministic ordering intact (perf smokes green).

## Verification

- **Local (Windows):** `cargo fmt --check` ✓ · `cargo clippy --workspace
  --all-targets -- -D warnings` ✓ · `cargo clippy --target
  x86_64-unknown-linux-gnu` for the unix-gated code paths ✓ · `cargo test
  --workspace` **328 passed / 0 failed** (313 at Phase 3.1; +11
  adversarial +4 identity unit tests) · identity perf smoke ✓ · release
  >4 GiB streaming proof ✓.
- **CI:** recorded below after the matrix run — each job verified
  individually from the Actions API.

## Self-audit (brief §final self-audit, answered from the code)

1. **Can a parent become a junction between observation and hashing?** Yes —
   and hashing refuses it (`windows_identity_tests::parent_replaced_by_junction…`,
   unix twin). 2. **Can hashing reach a different tree?** Only the final
   open resolves the chain; whatever object it reaches must match the
   scan-time identity or the file is typed `Replaced`/refused — a
   different tree cannot produce a published hash for the observed entry.
3. **Can the implementation prove opened == observed?** Yes on both
   platforms where the OS proves identity (Unix always; Windows via
   `FILE_ID_INFO`), by comparing scan-time vs handle-proven identities.
4. **Same-content replacement detected?** Yes when identity differs
   (rename-over: always — coexisting objects; delete+recreate on NTFS: MFT
   sequence number; otherwise: mtime bracket). 5. **Same-size replacement?**
   Same mechanisms + length bracket. 6. **Intermediate reparse redirect?**
   Refused by the ancestor guard; residual swap-between-checks window
   documented, cannot yield a false relationship. 7. **Hard link mistaken
   for replacement?** No — aliases share identity (test-pinned on both
   platforms). 8. **Path race → false duplicate?** A group is published
   only from digests that passed the full check sequence; a raced impostor
   is a typed failure, never a member. 9. **Any safety claim resting on
   pathname equality?** No — identity and handle-proven state only.
10. **Unbounded memory introduced?** No — fixed-size staging fields within
    existing caps; transient per-hash PathBuf only. 11. **Limitations
    documented?** Yes — docs/IDENTITY.md §known limitations (torn writes
    without change stamps, identity reuse without sequence numbers +
    forged mtime, FAT-class fallback, swap window, scan-time handle cost).

# CoreSight — Database Architecture (SQLite)

Status: Phase 0 conceptual schema. Implemented from Phase 1. Owner: Rust
(`rusqlite`, WAL mode). The frontend NEVER opens the DB.

## Entities

- `drives` — id, stable volume identity (per-OS id), label, fs type, kind
  (internal/external/network), capacity, first/seen-last timestamps, offline flag.
- `scans` — id, drive_id, started/finished, status (complete/partial/cancelled/
  failed), root, file/dir counts, bytes, exclusions summary (what was NOT scanned).
- `entries` — one row per file/dir per scan: scan_id, parent path-id, name, kind,
  size (logical + physical where OS reports both), mtimes, attributes, symlink/
  junction target, volume boundary flags. Current history path storage is tagged
  and lossless: `u:` UTF-8, `e:` lowercase hex of platform-encoded bytes for
  non-UTF-8 paths, and `l:` for irreversibly lossy legacy rows. It performs no
  normalization, case-folding, or separator conversion; lookups use the exact
  stored path semantics. `e:` decoding validates Windows WTF-8 before forming
  UTF-16 and returns a typed decode error for malformed hex/encoding, never an
  unchecked OS-string conversion. No Phase 6.3 schema change.
- `hashes` — entry identity → SHA-256, algorithm version, hashed-at, bytes hashed.
  Cache key includes size+mtime so stale hashes invalidate without re-read.
- `snapshots` — immutable per-scan rollups: per-category bytes, totals, created-at.
  Snapshots are never mutated; deltas compute between pairs.
- `changes` — derived per-category deltas between snapshots (cached for History).
- `classifications` — entry → category path + rule id + rule version + confidence.
  Re-runnable when rules update without rescanning.
- `operations` — append-only log of cleanup actions: plan id, items, destination
  (trash/quarantine), verdicts, freed bytes, timestamps. Basis for undo + support.
- `settings` — key/value (exclusions, retention, safety policy version).

## Migrations & versioning

- Forward-only, numbered SQL migrations embedded in the binary; `schema_version`
  table; app refuses to open a newer DB (tells user to upgrade) and auto-migrates
  older ones with a pre-migration backup copy.
- Rule/policy tables carry their own versions (`rule_version`, `policy_version`)
  so classification and safety verdicts are reproducible and auditable per scan.

## Indexes (initial set; extend by measured query, not guess)

- `entries(scan_id, parent, name)`, `entries(scan_id, size)` (duplicate candidates),
  `hashes(digest)`, `snapshots(drive_id, created_at)`, `operations(created_at)`.
- Foreign keys ON; synchronous=NORMAL + WAL for scan-write/UI-read concurrency.

## Retention & privacy

- Default retention: last N snapshots per drive (N set in Phase 5; principle:
  enough for meaningful history, bounded disk cost). Old scan `entries` pruned
  with their snapshot; `operations` log kept longer (user's safety record).
- DB lives in the OS-appropriate app-data dir, restricted permissions, never
  synced, never uploaded. Delete-account == delete file (documented path).

## Corruption & recovery

- `PRAGMA integrity_check` runs on EVERY open, before anything reads or
  migrates the file (IMPLEMENTED, Phase 6.1 audit: `HistoryStore::open`
  refuses a corrupt store with the typed `StoreError::Corrupt` — never
  guess, never half-read).
- PLANNED (not yet implemented — the check refuses instead of hiding):
  quarantining the corrupt file aside, starting fresh, and offering a
  rescan flow. Also PLANNED: an automatic pre-migration backup copy of
  the store file (migrations themselves are already atomic per step,
  version bump inside the same transaction — a failed migration rolls
  back rather than leaving a half-migrated database).
- Scans are idempotent and resumable-by-restart: a crashed scan leaves a
  `failed` record and zero partial visibility (snapshot publishes atomically).

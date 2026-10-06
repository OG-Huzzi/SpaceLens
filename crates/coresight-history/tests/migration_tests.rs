//! Phase 5.1 migration tests — forward-only schema v2 → v3.
//!
//! These build a *genuine* pre-repair database (the exact Phase 5 v2 DDL
//! as it existed in `coresight-history` before the identity/path repairs)
//! and assert that opening it with the current store:
//!
//! - migrates it forward without losing any historical row,
//! - adds `file_id_hi` and leaves every pre-existing row's high bits
//!   `NULL` (never fabricated),
//! - re-tags legacy lossy path spellings as `l:` so the decoder reports
//!   them honestly instead of reinterpreting them,
//! - keeps the old `(device, inode)` rows queryable and equal-under-the
//!   narrower identity they actually proved.
//!
//! The v2 DDL is duplicated here deliberately: it is a *historical
//! fixture*. If someone edits the live `MIGRATION_V2` constant, this
//! test must NOT follow — it must keep asserting what the old database
//! on disk actually looked like.

use std::path::Path;

use coresight_history::{
    store::HISTORY_SCHEMA_VERSION, HistoryStore, ObjectId, ObservedKind, QueryLimits, RunId,
};
use rusqlite::Connection;

/// The Phase 5 v2 schema, verbatim. A historical fixture — do not "fix"
/// this to match the live migration constants.
const V2_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS scan_runs (
    run_id        TEXT PRIMARY KEY,
    started_at    INTEGER NOT NULL,
    completed_at  INTEGER,
    roots         TEXT NOT NULL,
    platform      TEXT NOT NULL,
    config        TEXT NOT NULL,
    status        TEXT NOT NULL,
    entries_examined INTEGER NOT NULL DEFAULT 0,
    files         INTEGER NOT NULL DEFAULT 0,
    dirs          INTEGER NOT NULL DEFAULT 0,
    links         INTEGER NOT NULL DEFAULT 0,
    other_entries INTEGER NOT NULL DEFAULT 0,
    bytes         INTEGER NOT NULL DEFAULT 0,
    observation_errors INTEGER NOT NULL DEFAULT 0,
    candidates_untracked INTEGER NOT NULL DEFAULT 0,
    hash_failures INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS observations (
    run_id    TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    path      TEXT NOT NULL,
    kind      TEXT NOT NULL,
    size      INTEGER,
    device    INTEGER,
    inode     INTEGER,
    modified  INTEGER,
    category  TEXT,
    subcategory TEXT,
    content_sha256 TEXT,
    obs_error TEXT,
    PRIMARY KEY (run_id, path)
);
CREATE INDEX IF NOT EXISTS idx_obs_path ON observations(path);
CREATE INDEX IF NOT EXISTS idx_obs_object ON observations(device, inode);
CREATE INDEX IF NOT EXISTS idx_obs_content ON observations(content_sha256);
CREATE TABLE IF NOT EXISTS relationship_obs (
    run_id    TEXT NOT NULL REFERENCES scan_runs(run_id) ON DELETE CASCADE,
    rel_id    TEXT NOT NULL,
    kind      TEXT NOT NULL,
    size      INTEGER NOT NULL,
    member_count INTEGER NOT NULL,
    recoverable INTEGER,
    accounting TEXT NOT NULL,
    PRIMARY KEY (run_id, rel_id)
);
CREATE TABLE IF NOT EXISTS relationship_members (
    run_id TEXT NOT NULL,
    rel_id TEXT NOT NULL,
    path   TEXT NOT NULL,
    device INTEGER,
    inode  INTEGER,
    PRIMARY KEY (run_id, rel_id, path)
);
CREATE INDEX IF NOT EXISTS idx_rel_members_path ON relationship_members(path);
";

/// Minimal core v1 bootstrap, matching `coresight_core::db`'s schema.
const V1_BOOTSTRAP: &str = "
CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version (version) VALUES (1);
";

/// A v2 database carrying one committed run, one clean path, and one
/// path whose v2 write went through `to_string_lossy()` (hence contains
/// U+FFFD).
fn build_legacy_v2(db_path: &Path) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(V1_BOOTSTRAP).unwrap();
    conn.execute_batch(V2_SCHEMA).unwrap();
    conn.execute("UPDATE schema_version SET version = 2", [])
        .unwrap();

    // Faithful to what the v2 store actually wrote: the run's full
    // ConfigFingerprint JSON (camelCase), not an empty object. The
    // current store decodes persisted configs strictly, and a real v2
    // database always carried this shape.
    let v2_config = r#"{"observationModel":1,"classifierSchema":"spacelens.v1.classification","classifierRules":1,"hashAlgorithm":"sha256","relationshipSchema":1,"historySchema":1}"#;
    conn.execute(
        "INSERT INTO scan_runs
         (run_id, started_at, completed_at, roots, platform, config, status)
         VALUES ('legacy-run', 1000, 2000, ?1, 'test', ?2, 'COMPLETED')",
        rusqlite::params![r#"["/scope-a"]"#, v2_config],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO observations
         (run_id, path, kind, size, device, inode, content_sha256)
         VALUES ('legacy-run', '/scope-a/clean.bin', 'FILE', 10, 7, 100, NULL)",
        [],
    )
    .unwrap();
    // A path the old store could not represent: the lossy conversion
    // replaced the unpaired-surrogate byte with U+FFFD.
    conn.execute(
        "INSERT INTO observations
         (run_id, path, kind, size, device, inode, content_sha256)
         VALUES ('legacy-run', '/scope-a/bad\u{FFFD}.bin', 'FILE', 20, 7, 101, NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relationship_members (run_id, rel_id, path, device, inode)
         VALUES ('legacy-run', 'rel-1', '/scope-a/clean.bin', 7, 100)",
        [],
    )
    .unwrap();
    conn.close().unwrap();
}

#[test]
fn legacy_v2_database_migrates_to_current_schema_without_losing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    build_legacy_v2(&db);

    let store = HistoryStore::open(&db).unwrap();

    // The version table advanced.
    let conn = Connection::open(&db).unwrap();
    let version: u32 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        version, HISTORY_SCHEMA_VERSION,
        "opening a legacy store must apply every pending migration"
    );

    // The v3 columns exist.
    for table in ["observations", "relationship_members"] {
        let cols = table_columns(&conn, table);
        assert!(
            cols.iter().any(|c| c == "file_id_hi"),
            "{table} must gain file_id_hi (got {cols:?})"
        );
    }

    // The historical run and BOTH its observations survived the migration.
    let run = store
        .get_run(&RunId("legacy-run".to_string()))
        .unwrap()
        .expect("legacy run must survive migration");
    assert_eq!(run.roots, vec![Path::new("/scope-a")]);

    let snapshot = store
        .load_run_snapshot(&RunId("legacy-run".to_string()))
        .unwrap()
        .expect("legacy snapshot must survive migration");
    assert_eq!(
        snapshot.snapshot.entries.len(),
        2,
        "no historical row may be dropped"
    );
    drop(store);
    conn.close().unwrap();
}

#[test]
fn migrated_legacy_rows_never_fabricate_wide_identity_bits() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    build_legacy_v2(&db);

    let store = HistoryStore::open(&db).unwrap();
    let snapshot = store
        .load_run_snapshot(&RunId("legacy-run".to_string()))
        .unwrap()
        .unwrap();

    // The v2 store recorded only (device, inode). The high bits were
    // never stored, so they must read back as an honest unknown — the
    // migration must not invent a 0 (which would assert a wide identity
    // the old run never proved).
    for entry in &snapshot.snapshot.entries {
        if entry.kind == ObservedKind::File {
            let object = entry.object.unwrap_or_else(|| {
                panic!("legacy device/inode must survive: {}", entry.path.display())
            });
            assert_eq!(object.device, 7);
            assert!(
                object.file_id_hi.is_none(),
                "a v2 row proved no high bits; migration must not fabricate them"
            );
        }
    }
}

#[test]
fn legacy_lossy_paths_are_preserved_not_reinterpreted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    build_legacy_v2(&db);

    let store = HistoryStore::open(&db).unwrap();
    let snapshot = store
        .load_run_snapshot(&RunId("legacy-run".to_string()))
        .unwrap()
        .unwrap();

    // The lossy row keeps the exact characters v2 wrote. The discarded
    // byte cannot be recovered and is not invented — what survives is
    // the replacement character v2 actually stored.
    let bad = snapshot
        .snapshot
        .entries
        .iter()
        .find(|e| e.path.to_string_lossy().contains('\u{FFFD}'))
        .expect("the legacy lossy row must still be present");
    assert_eq!(bad.path, Path::new("/scope-a/bad\u{FFFD}.bin"));

    // The clean row is the verbatim path.
    let clean = snapshot
        .snapshot
        .entries
        .iter()
        .find(|e| e.path == Path::new("/scope-a/clean.bin"))
        .expect("the clean row must still be present");
    assert_eq!(clean.path, Path::new("/scope-a/clean.bin"));
}

#[test]
fn migrated_legacy_object_identity_still_queries_by_proven_pair() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    build_legacy_v2(&db);

    let store = HistoryStore::open(&db).unwrap();
    // The query API takes the wide id; the legacy row proves only the
    // low pair, so querying with `None` high bits must find it.
    let points = store
        .history_for_object(7, 100, None, &QueryLimits::default())
        .unwrap();
    assert_eq!(
        points.len(),
        1,
        "legacy row remains queryable by its proven identity"
    );
    assert_eq!(points[0].object, Some(ObjectId::from_proven(7, 100, None)));
}

#[test]
fn migration_is_idempotent_across_repeated_opens() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    build_legacy_v2(&db);

    // Opening repeatedly must not re-apply the ALTER TABLE (which would
    // fail with "duplicate column name") and must not double-tag paths.
    for _ in 0..3 {
        let store = HistoryStore::open(&db).unwrap();
        let snapshot = store
            .load_run_snapshot(&RunId("legacy-run".to_string()))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.snapshot.entries.len(), 2);
        let clean = snapshot
            .snapshot
            .entries
            .iter()
            .find(|e| e.path == Path::new("/scope-a/clean.bin"))
            .expect("clean path must not accumulate repeated tags");
        assert_eq!(clean.path, Path::new("/scope-a/clean.bin"));
    }
}

fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .unwrap();
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

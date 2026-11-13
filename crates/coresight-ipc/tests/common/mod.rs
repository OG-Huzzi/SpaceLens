//! Shared synthetic fixtures for the `coresight-ipc` integration tests.
//!
//! Everything here is in-memory: no file is created, read, moved, or deleted.
//!
//! Each integration-test binary includes this module separately, so helpers
//! used by one test file appear unused in another. The allow is scoped to
//! this test-support module.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use coresight_classifier::{Category, Subcategory};
use coresight_engine::{EntryKind, ErrorCategoryRef, FsEntry};
use coresight_history::{ClassificationRef, RunId, Snapshot, SnapshotBuilder};
use coresight_ipc::{Drive, DriveKind, ScanRecord, ScanStatus, Session};

/// The registered sample drive id.
pub const DRIVE_ID: &str = "drive-c";
/// The first sample scan id.
pub const SCAN_ID: &str = "scan-1";
/// The second sample scan id.
pub const SCAN_ID_2: &str = "scan-2";

const T0: u64 = 1_700_000_000;
const T1: u64 = T0 + 3_600;

/// A regular-file observation.
pub fn file(path: &str, size: u64) -> FsEntry {
    FsEntry {
        id: 0,
        parent_id: None,
        path: PathBuf::from(path),
        kind: EntryKind::File,
        size,
        allocated_size: Some(size),
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device: Some(1),
        inode: Some(1),
        file_id_hi: None,
        hidden: false,
        error: None,
    }
}

/// A directory observation (size is unknown by construction).
pub fn dir(path: &str) -> FsEntry {
    FsEntry {
        kind: EntryKind::Dir,
        allocated_size: None,
        size: 0,
        ..file(path, 0)
    }
}

/// A regular file the scan could see but not read.
pub fn permission_denied(path: &str) -> FsEntry {
    let mut entry = file(path, 0);
    entry.error = Some(ErrorCategoryRef::PermissionDenied);
    entry
}

/// A stored category classification.
pub fn classified(category: Category) -> Option<ClassificationRef> {
    Some(ClassificationRef {
        category: category.code().to_string(),
        subcategory: None,
    })
}

/// A stored category + subcategory classification.
pub fn classified_sub(category: Category, subcategory: Subcategory) -> Option<ClassificationRef> {
    Some(ClassificationRef {
        category: category.code().to_string(),
        subcategory: Some(subcategory.code().to_string()),
    })
}

/// Build a snapshot from synthetic entries.
pub fn snapshot(entries: Vec<(FsEntry, Option<ClassificationRef>)>) -> Snapshot {
    let mut builder = SnapshotBuilder::new();
    for (entry, classification) in entries {
        builder.push_entry(&entry, classification);
    }
    builder
        .build(RunId("run".to_string()))
        .unwrap_or_else(|_| Snapshot {
            run_id: RunId("run".to_string()),
            entries: Vec::new(),
        })
}

/// The sample scan: 236_000 measured bytes across six files.
///
/// Categories: Cache 30k, TemporaryData 20k, Logs 5k, Documents 100k,
/// Downloads 80k, Unknown 1k. Only Cache/TemporaryData/Logs/Downloads are
/// actionable; Documents and Unknown are off-limits. Downloads carries the
/// `ARCHIVE` subcategory and Logs carries `LOG_FILE`.
pub fn sample_snapshot() -> Snapshot {
    snapshot(vec![
        (
            file("/scan/cache/a.bin", 30_000),
            classified(Category::Cache),
        ),
        (
            file("/scan/temp/b.tmp", 20_000),
            classified(Category::TemporaryData),
        ),
        (
            file("/scan/logs/app.log", 5_000),
            classified_sub(Category::Logs, Subcategory::LogFile),
        ),
        (
            file("/scan/docs/report.pdf", 100_000),
            classified(Category::Documents),
        ),
        (
            file("/scan/downloads/pack.zip", 80_000),
            classified_sub(Category::Downloads, Subcategory::Archive),
        ),
        (file("/scan/misc/note.dat", 1_000), None),
    ])
}

/// The sample snapshot with a different Cache size, for delta tests.
pub fn sample_snapshot_with_cache(cache_bytes: u64) -> Snapshot {
    snapshot(vec![
        (
            file("/scan/cache/a.bin", cache_bytes),
            classified(Category::Cache),
        ),
        (
            file("/scan/temp/b.tmp", 20_000),
            classified(Category::TemporaryData),
        ),
        (
            file("/scan/logs/app.log", 5_000),
            classified_sub(Category::Logs, Subcategory::LogFile),
        ),
        (
            file("/scan/docs/report.pdf", 100_000),
            classified(Category::Documents),
        ),
        (
            file("/scan/downloads/pack.zip", 80_000),
            classified_sub(Category::Downloads, Subcategory::Archive),
        ),
        (file("/scan/misc/note.dat", 1_000), None),
    ])
}

/// A complete instant from the fixture timeline.
pub fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The connected sample drive.
pub fn drive() -> Drive {
    Drive {
        id: DRIVE_ID.to_string(),
        root: "C:\\".to_string(),
        label: Some("System".to_string()),
        kind: DriveKind::Internal,
        capacity_bytes: Some(1_000_000_000),
        available_bytes: Some(500_000_000),
        connected: true,
    }
}

/// A remembered but currently offline drive.
pub fn offline_drive() -> Drive {
    Drive {
        id: "drive-usb".to_string(),
        root: "E:\\".to_string(),
        label: Some("Backup".to_string()),
        kind: DriveKind::Removable,
        capacity_bytes: Some(64_000_000_000),
        available_bytes: None,
        connected: false,
    }
}

/// A session with the sample drive and one completed scan.
pub fn sample_session() -> Session {
    let mut session = Session::new();
    session.register_drive(drive());
    session
        .register_scan(ScanRecord {
            scan_id: SCAN_ID.to_string(),
            drive_id: DRIVE_ID.to_string(),
            snapshot: sample_snapshot(),
            completed_at: Some(at(T0)),
            status: ScanStatus::Completed,
        })
        .expect("fixture registration");
    session
}

/// A session with one scan of the given status.
pub fn session_with_snapshot(snapshot: Snapshot, status: ScanStatus) -> Session {
    let mut session = Session::new();
    session.register_drive(drive());
    session
        .register_scan(ScanRecord {
            scan_id: SCAN_ID.to_string(),
            drive_id: DRIVE_ID.to_string(),
            snapshot,
            completed_at: Some(at(T0)),
            status,
        })
        .expect("fixture registration");
    session
}

/// A session with two completed scans (Cache 30k then 40k).
pub fn session_with_two_scans() -> Session {
    let mut session = Session::new();
    session.register_drive(drive());
    session
        .register_scan(ScanRecord {
            scan_id: SCAN_ID.to_string(),
            drive_id: DRIVE_ID.to_string(),
            snapshot: sample_snapshot_with_cache(30_000),
            completed_at: Some(at(T0)),
            status: ScanStatus::Completed,
        })
        .expect("first fixture registration");
    session
        .register_scan(ScanRecord {
            scan_id: SCAN_ID_2.to_string(),
            drive_id: DRIVE_ID.to_string(),
            snapshot: sample_snapshot_with_cache(40_000),
            completed_at: Some(at(T1)),
            status: ScanStatus::Completed,
        })
        .expect("second fixture registration");
    session
}

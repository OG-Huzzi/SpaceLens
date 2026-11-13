//! Real-filesystem integration tests for the scan backend.
//!
//! Every test creates its tree inside its **own** `tempfile::tempdir()` and
//! asserts that the scanned root is inside that directory before scanning.
//! No test reads, lists, or scans a real user path.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use coresight_engine::{ScanOptions, SymlinkPolicy};
use coresight_history::ObservedKind;
use coresight_ipc::{
    codes, dispatch, ApiError, Command, Drive, DriveKind, EngineScanBackend, Envelope,
    GetBigFileReportRequest, GetCleanupRequest, GetDuplicateReportRequest, GetGlobalStatusRequest,
    GetHomeSummaryRequest, GetStorageRequest, Response, ScanBackend, ScanProgressUpdate,
    ScanRecord, ScanRequest, ScanStateView, ScanStatus, Session, StartScanRequest,
};
use tempfile::TempDir;

/// Assert the scan root really lives inside the test's own tempdir.
fn assert_inside(root: &Path, temp: &TempDir) {
    assert!(
        root.starts_with(temp.path()),
        "test root {root:?} escaped its own tempdir {:?}",
        temp.path()
    );
}

fn test_options() -> ScanOptions {
    ScanOptions {
        threads: 2,
        symlink_policy: SymlinkPolicy::RecordOnly,
        max_depth: None,
        emit_entries: true,
        progress_interval: Duration::from_millis(10),
    }
}

fn write_file(root: &Path, relative: &str, bytes: usize) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture directory");
    }
    fs::write(&path, vec![b'x'; bytes]).expect("write fixture file");
    path
}

/// Start a scan through the real backend and drain it to the terminal event.
fn run_backend_scan(
    root: &Path,
    scan_id: &str,
) -> (coresight_ipc::ScanFinished, Vec<ScanProgressUpdate>) {
    let backend = EngineScanBackend::new();
    let handle = backend
        .start(ScanRequest {
            scan_id: scan_id.to_string(),
            root: root.to_path_buf(),
            options: test_options(),
        })
        .expect("the real backend starts");
    let mut updates = Vec::new();
    let mut finished = None;
    for update in handle.progress {
        if let ScanProgressUpdate::Finished(ref terminal) = update {
            finished = Some((**terminal).clone());
        }
        updates.push(update);
    }
    (finished.expect("exactly one terminal update"), updates)
}

fn register_finished(
    session: &mut Session,
    root: &Path,
    drive_id: &str,
    scan_id: &str,
    finished: &coresight_ipc::ScanFinished,
) {
    session.register_drive(Drive {
        id: drive_id.to_string(),
        root: root.to_string_lossy().into_owned(),
        label: None,
        kind: DriveKind::Unknown,
        capacity_bytes: None,
        available_bytes: None,
        connected: true,
    });
    session
        .register_scan(ScanRecord {
            scan_id: scan_id.to_string(),
            drive_id: drive_id.to_string(),
            snapshot: finished.snapshot.clone(),
            completed_at: Some(finished.summary.finished_at),
            status: finished.status,
        })
        .expect("register the real scan");
}

fn error_of(envelope: Envelope<Response>) -> ApiError {
    envelope
        .into_result()
        .expect_err("expected a typed error, got a success payload")
}

fn ok_response(envelope: Envelope<Response>) -> Response {
    envelope
        .into_result()
        .expect("expected a success payload, got a typed error")
}

#[test]
fn real_scan_snapshot_matches_files_created() {
    let temp = tempdir_or_panic();
    let root = temp.path().join("tree");
    fs::create_dir_all(&root).expect("create root");
    assert_inside(&root, &temp);

    write_file(&root, "a.txt", 11);
    write_file(&root, "sub/b.bin", 22);
    write_file(&root, "sub/deep/c.dat", 33);

    let (finished, _) = run_backend_scan(&root, "scan-exact");

    assert_eq!(finished.status, ScanStatus::Completed);
    let files: Vec<_> = finished
        .snapshot
        .entries
        .iter()
        .filter(|entry| entry.kind == ObservedKind::File)
        .collect();
    assert_eq!(files.len(), 3, "three real files were created");
    assert_eq!(
        files
            .iter()
            .map(|entry| entry.size.unwrap_or(0))
            .sum::<u64>(),
        66
    );
    assert_eq!(finished.report.total_logical_size(), 66);
    assert_eq!(
        finished.report.scanned_entries,
        finished.snapshot.entries.len() as u64
    );
    assert_eq!(finished.report.errored_entries, 0);
    assert_eq!(
        finished.report.entries_without_size, 0,
        "a clean walk measured or accounted for every entry"
    );
    assert!(
        finished
            .snapshot
            .entries
            .iter()
            .all(|entry| entry.classification.is_some()),
        "every real entry is classified"
    );
    // The walk really observed the two directories and the root too.
    assert_eq!(
        finished
            .snapshot
            .entries
            .iter()
            .filter(|entry| entry.kind == ObservedKind::Dir)
            .count(),
        3
    );
}

#[test]
fn duplicate_groups_require_a_real_verified_hash() {
    let temp = tempdir_or_panic();
    let root = temp.path().join("dupes");
    fs::create_dir_all(&root).expect("create root");
    assert_inside(&root, &temp);

    let payload = vec![b'd'; 4096];
    let first = root.join("one/data.bin");
    let second = root.join("two/data.bin");
    fs::create_dir_all(first.parent().expect("parent")).expect("dir one");
    fs::create_dir_all(second.parent().expect("parent")).expect("dir two");
    fs::write(&first, &payload).expect("write first");
    fs::write(&second, &payload).expect("write second");

    let (finished, _) = run_backend_scan(&root, "scan-dupes");
    assert_eq!(finished.status, ScanStatus::Completed);

    let hashes: Vec<String> = finished
        .snapshot
        .entries
        .iter()
        .filter_map(|entry| entry.content_sha256.clone())
        .collect();
    assert_eq!(hashes.len(), 2, "both verified members carry a real hash");
    assert_eq!(hashes[0], hashes[1], "identical bytes produced one hash");

    let mut verified_session = Session::new();
    register_finished(
        &mut verified_session,
        &root,
        "drive-dupes",
        "scan-dupes",
        &finished,
    );
    let Response::DuplicateReport(report) = ok_response(dispatch(
        &mut verified_session,
        Command::GetDuplicateReport(GetDuplicateReportRequest {}),
    )) else {
        panic!("expected a duplicate report");
    };
    assert_eq!(report.groups.len(), 1, "one verified duplicate group");
    assert_eq!(report.groups[0].members.len(), 2);
    assert_eq!(report.unverified_entries, Some(0));

    // The same real observations with the hashes removed must not be
    // presented as duplicates: absent identity stays unknown.
    let mut unhashed = finished.snapshot.clone();
    for entry in &mut unhashed.entries {
        entry.content_sha256 = None;
    }
    let mut unhashed_session = Session::new();
    unhashed_session.register_drive(Drive {
        id: "drive-dupes".to_string(),
        root: root.to_string_lossy().into_owned(),
        label: None,
        kind: DriveKind::Unknown,
        capacity_bytes: None,
        available_bytes: None,
        connected: true,
    });
    unhashed_session
        .register_scan(ScanRecord {
            scan_id: "scan-unhashed".to_string(),
            drive_id: "drive-dupes".to_string(),
            snapshot: unhashed,
            completed_at: Some(finished.summary.finished_at),
            status: ScanStatus::Completed,
        })
        .expect("register unhashed scan");
    let Response::DuplicateReport(report) = ok_response(dispatch(
        &mut unhashed_session,
        Command::GetDuplicateReport(GetDuplicateReportRequest {}),
    )) else {
        panic!("expected a duplicate report");
    };
    assert!(
        report.groups.is_empty(),
        "no hash means no duplicate claim, even for identical bytes"
    );
    assert_eq!(report.unverified_entries, Some(2));
}

#[test]
fn cancelled_scan_is_not_completed_and_serves_no_totals() {
    let temp = tempdir_or_panic();
    let root = temp.path().join("big");
    fs::create_dir_all(&root).expect("create root");
    assert_inside(&root, &temp);

    // Enough files that a cancellation lands mid-walk even on a fast disk.
    for directory in 0..40 {
        for file in 0..50 {
            write_file(&root, &format!("d{directory}/f{file}.bin"), 8);
        }
    }

    let backend = EngineScanBackend::new();
    let handle = backend
        .start(ScanRequest {
            scan_id: "scan-cancel".to_string(),
            root: root.clone(),
            options: test_options(),
        })
        .expect("scan starts");
    handle.cancel.cancel();

    let mut finished = None;
    for update in handle.progress {
        if let ScanProgressUpdate::Finished(terminal) = update {
            finished = Some(*terminal);
        }
    }
    let finished = finished.expect("a cancelled scan still emits one terminal update");
    assert_ne!(
        finished.status,
        ScanStatus::Completed,
        "a cancelled walk must never look complete"
    );
    assert_eq!(finished.status, ScanStatus::Cancelled);

    let mut session = Session::new();
    register_finished(
        &mut session,
        &root,
        "drive-cancel",
        "scan-cancel",
        &finished,
    );

    let Response::GlobalStatus(status) = ok_response(dispatch(
        &mut session,
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
    )) else {
        panic!("expected a global status");
    };
    assert_eq!(status.scan.state, ScanStateView::Partial);
    assert_eq!(
        status.scan.bytes_seen, None,
        "a cancelled walk's bytes are not a completed total"
    );
    assert!(status.scan.completed_at.is_some());

    let storage = error_of(dispatch(
        &mut session,
        Command::GetStorage(GetStorageRequest {}),
    ));
    assert_eq!(storage.code, codes::SESSION_UNAVAILABLE);
}

#[test]
fn missing_root_is_a_typed_error_and_registers_nothing() {
    let temp = tempdir_or_panic();
    let missing = temp.path().join("does-not-exist");
    assert_inside(&missing, &temp);
    assert!(!missing.exists());

    let backend = EngineScanBackend::new();
    let error = match backend.start(ScanRequest {
        scan_id: "scan-missing".to_string(),
        root: missing.clone(),
        options: test_options(),
    }) {
        Ok(_) => panic!("a missing root must not start a scan"),
        Err(error) => error,
    };
    assert_eq!(error.code(), codes::SCAN_ROOT_UNAVAILABLE);

    let mut session = Session::new();
    session.attach_scan_backend(Box::new(EngineScanBackend::new()));
    let error = error_of(dispatch(
        &mut session,
        Command::StartScan(StartScanRequest {
            drive_id: String::new(),
            full_rescan: Some(true),
            root: Some(missing),
        }),
    ));
    assert_eq!(error.code, codes::SCAN_ROOT_UNAVAILABLE);
    assert!(error.message.to_lowercase().contains("scan root"));

    let Response::Drives(drives) = ok_response(dispatch(
        &mut session,
        Command::ListDrives(coresight_ipc::ListDrivesRequest {}),
    )) else {
        panic!("expected a drive list");
    };
    assert!(
        drives.is_empty(),
        "a refused scan must not leave a fabricated drive behind"
    );
}

#[test]
fn observation_errors_are_recorded_and_the_walk_continues() {
    let temp = tempdir_or_panic();
    let root = temp.path().join("unreadable");
    fs::create_dir_all(&root).expect("create root");
    assert_inside(&root, &temp);

    write_file(&root, "visible.bin", 4);
    let denied = root.join("denied");
    fs::create_dir_all(&denied).expect("create denied dir");
    write_file(&denied, "hidden.bin", 4);

    let mut skipped_reason: Option<&str> = None;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o000)).expect("chmod denied");
        // A privileged test runner can still read the directory; in that case
        // the platform cannot express the case and the assertion is skipped.
        if fs::read_dir(&denied).is_ok() {
            skipped_reason = Some("running with privileges that bypass directory permissions");
        }
    }

    #[cfg(windows)]
    {
        // Windows has no unprivileged in-process way to deny directory
        // listing through std. A dangling symlink is the one real observation
        // error this platform can create without elevation; if symlink
        // creation is refused (no Developer Mode/privilege), skip with the
        // reason instead of pretending the case was exercised.
        let link = root.join("broken-link");
        match std::os::windows::fs::symlink_file(denied.join("missing-target"), &link) {
            Ok(()) => {}
            Err(_) => {
                skipped_reason = Some(
                    "creating a symlink needs Developer Mode or SeCreateSymbolicLinkPrivilege",
                );
            }
        }
    }

    let (finished, _) = run_backend_scan(&root, "scan-unreadable");
    assert_eq!(
        finished.status,
        ScanStatus::Completed,
        "one unreadable entry must never abort the whole walk"
    );
    assert!(
        finished
            .snapshot
            .entries
            .iter()
            .any(|entry| entry.path.ends_with("visible.bin")),
        "the readable file was still observed"
    );

    if let Some(reason) = skipped_reason {
        eprintln!("SKIP unreadable-entry assertion on this platform: {reason}");
    } else {
        assert!(
            finished.report.errored_entries >= 1,
            "the refused area is recorded as an observation error, not dropped"
        );
        assert!(
            finished.report.errored_entries == finished.summary.errors.total_errors()
                || finished.report.errored_entries >= finished.summary.entries_with_errors,
            "engine errors are represented in the usage report"
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&denied, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn progress_updates_are_emitted_and_a_terminal_update_ends_the_stream() {
    let temp = tempdir_or_panic();
    let root = temp.path().join("progress");
    fs::create_dir_all(&root).expect("create root");
    assert_inside(&root, &temp);
    write_file(&root, "one.bin", 100);
    write_file(&root, "two.bin", 200);

    let (finished, updates) = run_backend_scan(&root, "scan-progress");
    assert_eq!(finished.status, ScanStatus::Completed);
    assert!(
        matches!(updates.first(), Some(ScanProgressUpdate::Started { .. })),
        "the stream starts with Started"
    );
    let progress_count = updates
        .iter()
        .filter(|update| matches!(update, ScanProgressUpdate::Progress { .. }))
        .count();
    assert!(
        progress_count >= 1,
        "at least one real progress update is emitted during the walk"
    );
    let finished_count = updates
        .iter()
        .filter(|update| matches!(update, ScanProgressUpdate::Finished(_)))
        .count();
    assert_eq!(finished_count, 1, "exactly one terminal update");
    assert!(
        matches!(updates.last(), Some(ScanProgressUpdate::Finished(_))),
        "the terminal update is the last one"
    );

    let progress_event = updates
        .iter()
        .find_map(ScanProgressUpdate::progress_event)
        .expect("a progress event payload exists");
    assert_eq!(progress_event.scan_id, "scan-progress");
    assert!(progress_event.eta_sec.is_none());

    let complete_event = updates
        .last()
        .and_then(ScanProgressUpdate::complete_event)
        .expect("a complete event payload exists");
    assert_eq!(complete_event.status, ScanStatus::Completed);
    assert_eq!(complete_event.summary.bytes_seen, 300);
    assert_eq!(complete_event.summary.entries_with_errors, 0);
}

#[test]
fn completed_scan_serves_real_aggregate_views() {
    let temp = tempdir_or_panic();
    let root = temp.path().join("aggregates");
    fs::create_dir_all(&root).expect("create root");
    assert_inside(&root, &temp);

    write_file(&root, "cache/temp.bin", 10_000);
    write_file(&root, "app.log", 5_000);
    write_file(&root, "pictures/photo.jpg", 7_000);
    write_file(&root, "docs/notes.txt", 3_000);

    let (finished, _) = run_backend_scan(&root, "scan-aggregates");
    assert_eq!(finished.status, ScanStatus::Completed);

    let mut session = Session::new();
    register_finished(
        &mut session,
        &root,
        "drive-aggregates",
        "scan-aggregates",
        &finished,
    );

    let Response::GlobalStatus(status) = ok_response(dispatch(
        &mut session,
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
    )) else {
        panic!("expected a global status");
    };
    assert_eq!(status.scan.state, ScanStateView::Complete);
    assert_eq!(status.scan.bytes_seen, Some(25_000));
    assert_eq!(
        status.scan.files_seen,
        Some(finished.snapshot.entries.len() as u64)
    );
    assert!(status.scan.completed_at.is_some());
    assert!(
        status.scan.started_at.is_none(),
        "a caller-registered record carries no start instant unless the run supplied one"
    );

    let Response::Storage(storage) = ok_response(dispatch(
        &mut session,
        Command::GetStorage(GetStorageRequest {}),
    )) else {
        panic!("expected storage data");
    };
    assert_eq!(storage.total_observed_bytes, Some(25_000));
    let logs = storage
        .categories
        .iter()
        .find(|row| row.id == "LOGS")
        .expect("a logs row");
    assert_eq!(logs.bytes, Some(5_000));
    let images = storage
        .categories
        .iter()
        .find(|row| row.id == "IMAGES")
        .expect("an images row");
    assert_eq!(images.bytes, Some(7_000));

    let Response::Cleanup(cleanup) = ok_response(dispatch(
        &mut session,
        Command::GetCleanup(GetCleanupRequest {}),
    )) else {
        panic!("expected cleanup data");
    };
    assert!(
        !cleanup.items.is_empty(),
        "the real scan produced actionable category opportunities"
    );
    assert!(cleanup
        .off_limits
        .iter()
        .any(|item| item.category_id == "IMAGES"));

    let Response::HomeSummary(home) = ok_response(dispatch(
        &mut session,
        Command::GetHomeSummary(GetHomeSummaryRequest {}),
    )) else {
        panic!("expected a home summary");
    };
    assert!(home.last_scan_at.is_some());
    assert!(!home.top_categories.is_empty());
    assert!(home.safe_reclaimable_bytes.is_some());

    let Response::DuplicateReport(duplicates) = ok_response(dispatch(
        &mut session,
        Command::GetDuplicateReport(GetDuplicateReportRequest {}),
    )) else {
        panic!("expected a duplicate report");
    };
    assert!(duplicates.groups.is_empty(), "no two files are identical");
    assert_eq!(duplicates.unverified_entries, Some(4));

    let Response::BigFileReport(big) = ok_response(dispatch(
        &mut session,
        Command::GetBigFileReport(GetBigFileReportRequest {}),
    )) else {
        panic!("expected a big-file report");
    };
    assert!(
        big.files.is_empty(),
        "nothing reaches the 100 MiB threshold"
    );
    assert!(big.threshold_bytes.is_some());
}

/// `tempfile::tempdir()` with a clear failure message.
fn tempdir_or_panic() -> TempDir {
    tempfile::tempdir().expect("create an isolated tempdir")
}

#[test]
fn session_start_scan_tracks_real_progress_and_completion() {
    let temp = tempdir_or_panic();
    let root = temp.path().join("session-scan");
    fs::create_dir_all(&root).expect("create root");
    assert_inside(&root, &temp);
    write_file(&root, "a.bin", 40);
    write_file(&root, "b.bin", 60);

    let mut session = Session::new();
    session.attach_scan_backend(Box::new(EngineScanBackend::new()));

    let Response::StartScan(started) = ok_response(dispatch(
        &mut session,
        Command::StartScan(StartScanRequest {
            drive_id: String::new(),
            full_rescan: Some(true),
            root: Some(root.clone()),
        }),
    )) else {
        panic!("expected a startScan payload");
    };
    assert!(!started.scan_id.is_empty());

    let handle = session
        .take_scan_handle(&started.scan_id)
        .expect("the shell takes the scan's handle");

    // The first update carries the engine's recorded start instant; apply it
    // exactly as the shell's drain thread does.
    let first = handle.progress.recv().expect("a Started update arrives");
    assert!(matches!(first, ScanProgressUpdate::Started { .. }));
    session
        .apply_scan_update(first)
        .expect("the Started update applies");

    // The scan is registered as Running immediately, so a status poll never
    // mistakes "not drained yet" for "no scan".
    let Response::GlobalStatus(running) = ok_response(dispatch(
        &mut session,
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
    )) else {
        panic!("expected a global status");
    };
    assert_eq!(running.scan.state, ScanStateView::Scanning);
    assert!(running.scan.started_at.is_some());
    assert!(running.scan.completed_at.is_none());
    assert_eq!(running.scan.bytes_seen, None);

    let mut terminal = None;
    for update in handle.progress {
        if let ScanProgressUpdate::Finished(ref finished) = update {
            terminal = Some((**finished).clone());
        }
        session
            .apply_scan_update(update)
            .expect("every real update applies");
    }
    let terminal = terminal.expect("a terminal update arrived");
    assert_eq!(terminal.status, ScanStatus::Completed);

    let Response::GlobalStatus(done) = ok_response(dispatch(
        &mut session,
        Command::GetGlobalStatus(GetGlobalStatusRequest {}),
    )) else {
        panic!("expected a global status");
    };
    assert_eq!(done.scan.state, ScanStateView::Complete);
    assert_eq!(done.scan.bytes_seen, Some(100));
    assert!(done.scan.started_at.is_some());
    assert!(done.scan.completed_at.is_some());
    assert!(done.scan.current_area.is_none());

    // The scan auto-registered a drive from the real root; its summary is
    // served from the real snapshot, with unknown capacity left unknown.
    let Response::Drives(drives) = ok_response(dispatch(
        &mut session,
        Command::ListDrives(coresight_ipc::ListDrivesRequest {}),
    )) else {
        panic!("expected a drive list");
    };
    assert_eq!(drives.len(), 1);
    assert_eq!(drives[0].root, root.to_string_lossy().into_owned());
    assert_eq!(drives[0].capacity_bytes, None);
    let Response::DriveSummary(summary) = ok_response(dispatch(
        &mut session,
        Command::GetDriveSummary(coresight_ipc::GetDriveSummaryRequest {
            drive_id: drives[0].id.clone(),
        }),
    )) else {
        panic!("expected a drive summary");
    };
    assert_eq!(
        summary.totals.as_ref().map(|totals| totals.bytes),
        Some(100)
    );
    assert_eq!(summary.snapshot_count, 1);

    let cancelled = error_of(dispatch(
        &mut session,
        Command::CancelScan(coresight_ipc::CancelScanRequest {
            scan_id: started.scan_id.clone(),
        }),
    ));
    assert_eq!(
        cancelled.code,
        codes::SCAN_NOT_RUNNING,
        "cancelling an already-finished scan is a typed refusal"
    );

    let unknown = error_of(dispatch(
        &mut session,
        Command::CancelScan(coresight_ipc::CancelScanRequest {
            scan_id: "scan-does-not-exist".to_string(),
        }),
    ));
    assert_eq!(unknown.code, codes::SCAN_NOT_FOUND);
}

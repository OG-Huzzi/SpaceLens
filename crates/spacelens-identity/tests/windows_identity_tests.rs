//! Phase 3.2 adversarial tests — Windows object identity & path-chain
//! TOCTOU hardening, plus the Unix twins of every structural guard.
//!
//! Every test follows the deterministic sequence (no thread races):
//!
//! ```text
//! observe (real scanner → real identities)
//!      ↓ prepare replacement tree / object
//!      ↓ perform replacement (rename / junction / recreate)
//!      ↓ hash
//!      ↓ assert typed outcome
//! ```
//!
//! Fixture pattern: every observed target file gets a same-size companion
//! (`twin.bin`), so the target is a hash *candidate* (size pairs get
//! hashed; size singletons never cost a read byte — Phase 3 semantics).
//! The assertions are about the SWAPPED path's typed outcome.
//!
//! Covered (brief acceptance matrix):
//! - scan-time identity captured for Windows files AND directories,
//! - same-content replacement (concurrent+rename, and delete+recreate)
//!   rejected via object identity / mtime brackets — content equality is
//!   never proof of object identity,
//! - parent replaced by another normal directory (Case A),
//! - parent replaced by a junction (Case B — refused, target untouched),
//! - multi-level parent swap (Case C),
//! - intermediate junction redirect (refused by the ancestor guard),
//! - hard links remain one object with zero recoverable bytes,
//! - Unix twins: symlinked-ancestor refusal.
//!
//! Junction creation uses `cmd /c mklink /J` (no admin privilege needed);
//! symlink creation is privilege-gated and skipped honestly when the host
//! refuses it (the unix CI legs prove the symlink variants).

use std::path::{Path, PathBuf};

use spacelens_engine::platform::std_fs;
use spacelens_engine::{CancelHandle, FsEntry};
use spacelens_identity::{
    run_duplicates, DefaultReaderFactory, DuplicateOptions, DuplicateStatus, HashFailureKind,
    StorageAccounting,
};

/// Scan a real tree and return entries with scan-time identities as the
/// pipeline would receive them (root dropped).
fn scan_entries(root: &Path) -> Vec<FsEntry> {
    let mut entries = Vec::new();
    let options = spacelens_engine::ScanOptions {
        threads: 2,
        ..spacelens_engine::ScanOptions::default()
    };
    spacelens_engine::scan(root, options, &CancelHandle::new(), &mut |e| {
        if let spacelens_engine::ScanEvent::Entry(entry) = e {
            if entry.path == root {
                return;
            }
            entries.push(*entry);
        }
    });
    entries
}

fn run(entries: Vec<FsEntry>) -> spacelens_identity::DuplicateReport {
    let factory = DefaultReaderFactory::new(std_fs());
    run_duplicates(
        entries.into_iter(),
        &DuplicateOptions::default(),
        &CancelHandle::new(),
        Some(&factory),
        &mut |_| {},
    )
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

fn has_failure_for(
    report: &spacelens_identity::DuplicateReport,
    path: &Path,
    kind: &HashFailureKind,
) -> bool {
    report
        .failures
        .iter()
        .any(|f| f.path == path && &f.kind == kind)
}

fn paths_in_groups(report: &spacelens_identity::DuplicateReport) -> Vec<PathBuf> {
    report
        .groups
        .iter()
        .flat_map(|g| g.members.iter().map(|m| m.path.clone()))
        .collect()
}

/// Create a directory junction via `cmd /c mklink /J` (needs no admin
/// privilege). All path components must be joined with `join()` — never
/// with embedded separators (a mixed-separator path makes mklink parse a
/// `/X` component as a switch). Returns false when the host refuses.
#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) -> bool {
    let out = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            &link.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() && link.exists() => true,
        other => {
            eprintln!("junction creation refused on this host: {other:?}");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Observation: Windows scan-time identity
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn windows_scan_captures_object_identity_for_files_and_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("dirA").join("f.bin"), b"identity-evidence");

    let entries = scan_entries(root);
    // The scanner saw dirA (dir) and f.bin (file); both must carry
    // (volume, file-id) provenance now.
    let with_identity = entries
        .iter()
        .filter(|e| e.device.is_some() && e.inode.is_some())
        .count();
    assert_eq!(
        with_identity,
        entries.len(),
        "every scanned file AND directory must carry object identity: {entries:?}"
    );
    // Distinct objects have distinct identities.
    let mut ids: Vec<(u64, u64)> = entries
        .iter()
        .map(|e| (e.device.unwrap(), e.inode.unwrap()))
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        entries.len(),
        "identities must be distinct: {ids:?}"
    );
}

#[cfg(not(windows))]
#[test]
fn unix_scan_captures_object_identity_for_files_and_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("dirA").join("f.bin"), b"identity-evidence");
    let entries = scan_entries(root);
    let with_identity = entries
        .iter()
        .filter(|e| e.device.is_some() && e.inode.is_some())
        .count();
    assert_eq!(
        with_identity,
        entries.len(),
        "st_dev/st_ino must be recorded for every entry: {entries:?}"
    );
}

// ---------------------------------------------------------------------------
// Final object: same-content replacement (Objective 5 / Case D)
// ---------------------------------------------------------------------------

/// Replacement created while the original still exists, renamed over it.
/// The two objects coexisted, so their identities are distinct by
/// construction on every platform — the identity comparison MUST reject.
#[test]
fn same_content_replacement_via_rename_is_rejected_as_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let original = root.join("f.bin");
    let twin = root.join("twin.bin");
    write(&original, b"identical-payload-!!");
    write(&twin, b"identical-payload-!!");
    let size = std::fs::metadata(&original).unwrap().len();

    // Observe via a real scan.
    let entries = scan_entries(root);
    assert_eq!(entries.len(), 2, "{entries:?}");
    assert!(
        entries
            .iter()
            .all(|e| e.device.is_some() && e.inode.is_some()),
        "scan must carry identity for this test to be meaningful"
    );

    // Prepare the replacement WHILE the original exists.
    let impostor = root.join("impostor.bin");
    write(&impostor, b"identical-payload-!!");
    assert_eq!(std::fs::metadata(&impostor).unwrap().len(), size);
    // Swap atomically: the path now names the impostor object.
    std::fs::rename(&impostor, &original).unwrap();

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&original),
        "a same-content replacement must never group as the original: {report:?}"
    );
    assert!(
        has_failure_for(&report, &original, &HashFailureKind::Replaced),
        "object identity must distinguish the impostor: {report:?}"
    );
    assert_eq!(
        report.stats.files_hashed, 1,
        "the twin still hashed: {report:?}"
    );
}

/// Delete + recreate with identical bytes: the FS *may* reuse the identity
/// (ext4 inode reuse is real; NTFS increments the MFT record sequence
/// number on reuse, so its identity still differs). Where identity cannot
/// distinguish, the fresh mtime bracket is the backstop. Either way the
/// recreated object must not pass as the observed one.
#[test]
fn same_content_delete_recreate_replacement_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let original = root.join("f.bin");
    let twin = root.join("twin.bin");
    write(&original, b"recreate-payload-!!");
    write(&twin, b"recreate-payload-!!");
    let entries = scan_entries(root);
    assert_eq!(entries.len(), 2);

    std::fs::remove_file(&original).unwrap();
    // Spacing so a fresh mtime is distinguishable on any realistic FS.
    std::thread::sleep(std::time::Duration::from_millis(50));
    write(&original, b"recreate-payload-!!");

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&original),
        "recreated object must not silently pass as the observed one: {report:?}"
    );
    assert_eq!(report.stats.failures, 1, "{report:?}");
    assert!(report.failures.iter().any(|f| f.path == original
        && matches!(f.kind, HashFailureKind::Replaced | HashFailureKind::Changed)));
}

// ---------------------------------------------------------------------------
// Parent-directory replacement (Objective 4)
// ---------------------------------------------------------------------------

/// Case A: parent replaced by a DIFFERENT normal directory holding a
/// same-name, same-size, same-content file. The final object differs →
/// identity mismatch → Replaced (never grouped with the original).
#[test]
fn parent_replaced_by_normal_directory_same_content_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("A").join("f.bin"), b"parent-swap-payload");
    let twin = root.join("twin.bin");
    write(&twin, b"parent-swap-payload");

    let entries = scan_entries(root);
    let observed_file = root.join("A").join("f.bin");

    // Build the replacement tree: B/f.bin with identical bytes.
    write(&root.join("B").join("f.bin"), b"parent-swap-payload");
    // Swap A out, B in.
    std::fs::rename(root.join("A"), root.join("A_old")).unwrap();
    std::fs::rename(root.join("B"), root.join("A")).unwrap();
    std::fs::remove_dir_all(root.join("A_old")).unwrap();

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&observed_file),
        "the impostor file behind the swapped parent must not group: {report:?}"
    );
    assert!(
        has_failure_for(&report, &observed_file, &HashFailureKind::Replaced),
        "the impostor object must be typed Replaced: {report:?}"
    );
    assert_eq!(
        report.stats.files_hashed, 1,
        "only the twin hashed: {report:?}"
    );
}

/// Case B: parent replaced by a JUNCTION. The ancestor guard must refuse
/// before any content is read — the junction target is never touched.
#[cfg(windows)]
#[test]
fn parent_replaced_by_junction_is_refused_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("A").join("f.bin"), b"junction-parent-payload");
    let twin = root.join("twin.bin");
    write(&twin, b"junction-parent-payload");

    let entries = scan_entries(root);
    let observed_file = root.join("A").join("f.bin");

    // Swap A (a real dir) for a junction pointing at a same-content tree.
    write(
        &root.join("Other").join("f.bin"),
        b"junction-parent-payload",
    );
    std::fs::rename(root.join("A"), root.join("A_old")).unwrap();
    assert!(
        create_junction(&root.join("A"), &root.join("Other")),
        "junction creation is required for this test"
    );
    std::fs::remove_dir_all(root.join("A_old")).unwrap();

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&observed_file),
        "a junction parent must not lead to hashing the redirected file: {report:?}"
    );
    assert!(
        has_failure_for(&report, &observed_file, &HashFailureKind::Changed),
        "the refused path must be a typed failure (path became a link): {report:?}"
    );
    assert_eq!(
        report.stats.files_hashed, 1,
        "only the twin hashed: {report:?}"
    );
}

/// Case C: multi-level parent swap (root\A\B\f.bin, A swapped for another
/// tree with identical bytes at the same relative position).
#[test]
fn multi_level_parent_swap_same_content_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("A").join("B").join("f.bin"),
        b"deep-swap-payload",
    );
    let twin = root.join("twin.bin");
    write(&twin, b"deep-swap-payload");

    let entries = scan_entries(root);
    let observed_file = root.join("A").join("B").join("f.bin");

    // Replacement tree X/B/f.bin — mirrors the observed INNER structure so
    // that after the swap the same-name, same-content impostor exists at
    // the observed path (only its object identity differs).
    write(
        &root.join("X").join("B").join("f.bin"),
        b"deep-swap-payload",
    );
    std::fs::rename(root.join("A"), root.join("A_old")).unwrap();
    std::fs::rename(root.join("X"), root.join("A")).unwrap();
    std::fs::remove_dir_all(root.join("A_old")).unwrap();

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&observed_file),
        "the deep impostor must not group: {report:?}"
    );
    assert!(
        has_failure_for(&report, &observed_file, &HashFailureKind::Replaced),
        "the deep impostor must be typed Replaced: {report:?}"
    );
    assert_eq!(
        report.stats.files_hashed, 1,
        "only the twin hashed: {report:?}"
    );
}

/// Intermediate-component junction (deeper than the parent):
/// root\A\B\f.bin with B swapped for a junction to a same-content tree.
#[cfg(windows)]
#[test]
fn intermediate_junction_redirect_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("A").join("B").join("f.bin"),
        b"mid-junction-payload",
    );
    let twin = root.join("twin.bin");
    write(&twin, b"mid-junction-payload");

    let entries = scan_entries(root);
    let observed_file = root.join("A").join("B").join("f.bin");

    write(
        &root.join("A").join("Other").join("f.bin"),
        b"mid-junction-payload",
    );
    std::fs::rename(root.join("A").join("B"), root.join("A").join("B_old")).unwrap();
    assert!(
        create_junction(&root.join("A").join("B"), &root.join("A").join("Other")),
        "junction creation is required for this test"
    );
    std::fs::remove_dir_all(root.join("A").join("B_old")).unwrap();

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&observed_file),
        "an intermediate junction must not redirect hashing: {report:?}"
    );
    assert!(
        has_failure_for(&report, &observed_file, &HashFailureKind::Changed),
        "typed refusal expected: {report:?}"
    );
    assert_eq!(
        report.stats.files_hashed, 1,
        "only the twin hashed: {report:?}"
    );
}

/// Unix twin of the junction tests: a symlinked ANCESTOR is refused.
#[cfg(unix)]
#[test]
fn unix_symlinked_ancestor_is_refused_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("A").join("f.bin"), b"unix-ancestor-payload");
    let twin = root.join("twin.bin");
    write(&twin, b"unix-ancestor-payload");

    let entries = scan_entries(root);
    let observed_file = root.join("A").join("f.bin");

    write(&root.join("Other").join("f.bin"), b"unix-ancestor-payload");
    std::fs::rename(root.join("A"), root.join("A_old")).unwrap();
    std::os::unix::fs::symlink(root.join("Other"), root.join("A")).unwrap();
    std::fs::remove_dir_all(root.join("A_old")).unwrap();

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&observed_file),
        "a symlinked ancestor must not redirect hashing: {report:?}"
    );
    assert!(
        has_failure_for(&report, &observed_file, &HashFailureKind::Changed),
        "typed refusal expected: {report:?}"
    );
    assert_eq!(
        report.stats.files_hashed, 1,
        "only the twin hashed: {report:?}"
    );
}

/// Degraded mode (scan identity unavailable — synthesized entry without
/// identity): the structural guards still refuse a junction parent.
#[cfg(windows)]
#[test]
fn degraded_identity_still_refuses_junction_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("A").join("f.bin"), b"degraded-mode-payload");
    let twin = root.join("twin.bin");
    write(&twin, b"degraded-mode-payload");

    let mut entries = scan_entries(root);
    // Strip identity: simulate a filesystem where the scan could not prove it.
    for e in entries.iter_mut() {
        e.device = None;
        e.inode = None;
        e.file_id_hi = None;
    }
    let observed_file = root.join("A").join("f.bin");

    write(&root.join("Other").join("f.bin"), b"degraded-mode-payload");
    std::fs::rename(root.join("A"), root.join("A_old")).unwrap();
    assert!(create_junction(&root.join("A"), &root.join("Other")));
    std::fs::remove_dir_all(root.join("A_old")).unwrap();

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&observed_file),
        "degraded mode must still refuse a junction parent: {report:?}"
    );
}

// ---------------------------------------------------------------------------
// Hard links (Objective 6) — same object, never "replacement"
// ---------------------------------------------------------------------------

#[test]
fn hard_links_share_identity_and_are_not_replacements() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("a.bin"), b"hardlink-payload-3.2");
    std::fs::hard_link(root.join("a.bin"), root.join("alias.bin")).unwrap();

    let entries = scan_entries(root);
    // On identity-proving platforms both entries must carry the SAME identity.
    let ids: Vec<(u64, u64)> = entries
        .iter()
        .map(|e| (e.device.unwrap_or(0), e.inode.unwrap_or(0)))
        .collect();
    assert_eq!(
        ids[0], ids[1],
        "scan-time identity of a hard link must match"
    );

    let report = run(entries);
    assert_eq!(report.status, DuplicateStatus::Completed, "{report:?}");
    assert_eq!(report.groups.len(), 1, "{report:?}");
    let g = &report.groups[0];
    assert_eq!(g.member_count, 2);
    assert_eq!(g.recoverable_bytes, None, "one object: nothing recoverable");
    assert_eq!(g.accounting, StorageAccounting::Exact);
    // No failure may be typed Replaced for either path.
    assert!(report
        .failures
        .iter()
        .all(|f| f.kind != HashFailureKind::Replaced));
}

/// Same content, different objects: a REAL duplicate pair (identity differs,
/// content identical). Must group with Exact accounting — proving the
/// identity check does not over-reject legitimate duplicates.
#[test]
fn distinct_copies_with_identical_content_still_group() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("one.bin"), b"copy-payload-3.2-x");
    write(&root.join("two.bin"), b"copy-payload-3.2-x");

    let entries = scan_entries(root);
    let report = run(entries);
    assert_eq!(report.status, DuplicateStatus::Completed, "{report:?}");
    assert_eq!(report.groups.len(), 1, "{report:?}");
    assert_eq!(report.groups[0].member_count, 2);
    assert_eq!(report.groups[0].accounting, StorageAccounting::Exact);
    assert!(report.failures.is_empty(), "{report:?}");
}

// ---------------------------------------------------------------------------
// Unrelated same-content tree reached via a swapped parent (Case A+D):
// the redirect target's file must NOT be attached to the original
// observation even when every byte matches.
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn redirected_same_content_file_is_never_attached_to_the_observation() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("A").join("f.bin"), b"redirect-payload-x");
    let twin = root.join("twin.bin");
    write(&twin, b"redirect-payload-x");

    let entries = scan_entries(root);
    let observed_file = root.join("A").join("f.bin");

    // Build an unrelated tree whose file is byte-identical, then point a
    // junction at it.
    write(&root.join("FarAway").join("f.bin"), b"redirect-payload-x");
    std::fs::rename(root.join("A"), root.join("A_old")).unwrap();
    assert!(create_junction(&root.join("A"), &root.join("FarAway")));
    std::fs::remove_dir_all(root.join("A_old")).unwrap();

    let report = run(entries);
    let grouped = paths_in_groups(&report);
    assert!(
        !grouped.contains(&observed_file),
        "content equality must never attach a redirected object to the \
         original observation: {report:?}"
    );
}

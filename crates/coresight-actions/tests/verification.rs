//! Post-execution verification: honest byte accounting.
//!
//! The execution report's `freed_bytes` is a claim measured at the sources;
//! `verify` is what turns it into evidence about the destinations. These tests
//! prove that a missing or resized destination is reported and excluded from
//! the verified total — never smoothed into a success.

mod common;

use std::fs;

use common::assert_inside;
use coresight_actions::{verify, ExecutionReport, MovedFile};

fn report_of(entries: Vec<MovedFile>) -> ExecutionReport {
    let freed_bytes = entries
        .iter()
        .fold(0u64, |sum, entry| sum.saturating_add(entry.bytes));
    ExecutionReport {
        moved: entries,
        freed_bytes,
    }
}

#[test]
fn verify_confirms_every_destination_and_its_size() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let first = root.join("a.bin");
    let second = root.join("b.bin");
    fs::write(&first, b"hello").expect("writable");
    fs::write(&second, b"world!").expect("writable");
    assert_inside(root, &first);
    assert_inside(root, &second);

    let report = report_of(vec![
        MovedFile {
            source: root.join("origin-a.bin"),
            destination: first.clone(),
            bytes: 5,
        },
        MovedFile {
            source: root.join("origin-b.bin"),
            destination: second.clone(),
            bytes: 6,
        },
    ]);

    let verification = verify(&report);
    assert!(verification.is_complete());
    assert_eq!(verification.verified, 2);
    assert_eq!(verification.verified_bytes, 11);
    assert!(verification.missing.is_empty());
    assert!(verification.size_mismatches.is_empty());
}

#[test]
fn verify_reports_a_missing_destination() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let absent = root.join("gone.bin");
    assert_inside(root, &absent);

    let report = report_of(vec![MovedFile {
        source: root.join("origin.bin"),
        destination: absent.clone(),
        bytes: 5,
    }]);

    let verification = verify(&report);
    assert!(!verification.is_complete());
    assert_eq!(verification.verified, 0);
    assert_eq!(verification.verified_bytes, 0, "nothing was confirmed");
    assert_eq!(verification.missing, vec![absent]);
}

#[test]
fn verify_reports_a_size_mismatch_with_both_numbers() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let destination = root.join("changed.bin");
    fs::write(&destination, b"0123456789").expect("writable");
    assert_inside(root, &destination);

    let report = report_of(vec![MovedFile {
        source: root.join("origin.bin"),
        destination: destination.clone(),
        bytes: 4,
    }]);

    let verification = verify(&report);
    assert!(!verification.is_complete());
    assert_eq!(verification.verified, 0);
    assert_eq!(verification.verified_bytes, 0);
    assert_eq!(
        verification.size_mismatches,
        vec![(destination, 4, 10)],
        "(destination, recorded, observed)"
    );
}

#[test]
fn verify_never_counts_bytes_it_could_not_confirm() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let root = workspace.path();
    let present = root.join("present.bin");
    fs::write(&present, b"1234567890").expect("writable");
    assert_inside(root, &present);

    let report = report_of(vec![
        MovedFile {
            source: root.join("origin-present.bin"),
            destination: present.clone(),
            bytes: 10,
        },
        MovedFile {
            source: root.join("origin-missing.bin"),
            destination: root.join("missing.bin"),
            bytes: 90,
        },
    ]);

    let verification = verify(&report);
    assert_eq!(verification.verified, 1);
    assert_eq!(
        verification.verified_bytes, 10,
        "the 90 unverified bytes must not be counted"
    );
    assert_eq!(verification.missing.len(), 1);
}

#[test]
fn the_execution_report_serializes_as_camel_case() {
    let _workspace = tempfile::tempdir().expect("a private tempdir");

    let report = report_of(vec![MovedFile {
        source: std::path::PathBuf::from("a.bin"),
        destination: std::path::PathBuf::from("trash/a.bin"),
        bytes: 5,
    }]);

    let value = serde_json::to_value(&report).expect("the report is serializable");
    let object = value.as_object().expect("the report is a JSON object");
    assert!(object.contains_key("moved"));
    assert!(object.contains_key("freedBytes"), "got {value}");
    assert!(!object.contains_key("freed_bytes"));

    let entry = object
        .get("moved")
        .and_then(|moved| moved.get(0))
        .expect("one moved entry");
    let entry = entry.as_object().expect("an entry is an object");
    for key in ["source", "destination", "bytes"] {
        assert!(entry.contains_key(key), "missing {key} in {value}");
    }
}

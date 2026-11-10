//! Architecture and output-contract guard tests.
//!
//! The source scan mirrors the `coresight-cleanup` guard: the production
//! sources must contain no filesystem, process, network, or persistence API,
//! and no panicking call. The serde test pins the public JSON field names.

use coresight_filetools::{DuplicateOptions, RenameRule};
use std::path::PathBuf;

fn production_sources() -> Vec<(PathBuf, String)> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(root).expect("src directory") {
        let path = entry.expect("directory entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("source file");
        sources.push((path, text));
    }
    assert!(!sources.is_empty(), "the source scan must see files");
    sources
}

#[test]
fn crate_performs_no_io_execution_network_or_persistence() {
    for (path, text) in production_sources() {
        for forbidden in [
            "std::fs::",
            "std::process",
            "std::net",
            "reqwest",
            "tokio",
            "rusqlite",
            "std::os::windows",
            "std::os::unix",
            "Command::new",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains forbidden API {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn production_sources_have_no_panicking_calls() {
    for (path, text) in production_sources() {
        // `unwrap_or` / `unwrap_or_default` are fine and do not match these
        // exact call strings.
        for forbidden in [
            ".unwrap()",
            ".expect(",
            "panic!(",
            "unreachable!(",
            "todo!(",
            "unimplemented!(",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains panicking call {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn every_sum_uses_saturating_arithmetic() {
    for (path, text) in production_sources() {
        assert!(
            !text.contains(" += "),
            "{} contains an unchecked add-assign; use saturating_add",
            path.display()
        );
    }
}

#[test]
fn public_options_serialize_with_camel_case_fields() {
    let json = serde_json::to_value(DuplicateOptions::default()).expect("serialize options");

    assert_eq!(json["maxGroups"], 100);
    assert_eq!(json["minSizeBytes"], 1);
    assert_eq!(json["maxMembersPerGroup"], 256);
    assert!(json.get("max_groups").is_none());
}

#[test]
fn rename_rule_struct_fields_serialize_with_camel_case() {
    let json = serde_json::to_value(RenameRule::Replace {
        find: "a".to_string(),
        replace_with: "b".to_string(),
    })
    .expect("serialize rule");

    assert_eq!(json["replace"]["find"], "a");
    assert_eq!(json["replace"]["replaceWith"], "b");
    assert!(json["replace"].get("replace_with").is_none());
}

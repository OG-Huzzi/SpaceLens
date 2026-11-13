//! Architecture guard tests for `coresight-capture`.
//!
//! The scan mirrors the `coresight-cleanup` / `coresight-filetools` guards:
//! production sources must contain no filesystem, process, network, or
//! persistence API and no panicking call. The crate is a policy layer, so
//! these are not style preferences — they are the proof that the "local-only,
//! plan-only" claims in the crate docs are enforced by the code that exists.

use std::path::{Path, PathBuf};

fn collect_sources(directory: &Path, sources: &mut Vec<(PathBuf, String)>) {
    let entries = std::fs::read_dir(directory).expect("source directory");
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect_sources(&path, sources);
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("source file");
        sources.push((path, text));
    }
}

fn production_sources() -> Vec<(PathBuf, String)> {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut sources = Vec::new();
    collect_sources(&root, &mut sources);
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
            "unimplemented!",
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
fn production_sources_have_no_upload_or_network_surface() {
    for (path, text) in production_sources() {
        for forbidden in [
            "http://",
            "https://",
            "TcpStream",
            "UdpSocket",
            "hyper",
            "ureq",
            "multipart",
            "WebSocket",
            "fn upload(",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} contains upload/network surface {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn byte_sums_use_saturating_arithmetic() {
    for (path, text) in production_sources() {
        assert!(
            !text.contains(" += "),
            "{} contains an unchecked add-assign; use saturating_add",
            path.display()
        );
    }
}

#[test]
fn public_types_serialize_with_the_documented_field_names() {
    use coresight_capture::{CaptureNaming, CapturePolicy, RetentionPolicy};

    let json = serde_json::to_value(CaptureNaming::default()).expect("serialize naming");
    assert!(json.get("includeTimestamp").is_some());
    assert!(json.get("include_timestamp").is_none());

    let json =
        serde_json::to_value(RetentionPolicy::default()).expect("serialize retention policy");
    assert!(json.get("keepLast").is_some());
    assert!(json.get("keep_last").is_none());

    let json = serde_json::to_value(CapturePolicy::local_only()).expect("serialize policy");
    assert_eq!(json["uploadEnabled"], false);
    assert_eq!(json["requireDestinationConfirmation"], true);
}

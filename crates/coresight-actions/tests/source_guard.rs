//! The source guards: what this crate must never contain.
//!
//! Contract 7 of the brief is a *source scan*, not a behavioural test: an
//! executor can only be trusted with a user's files if it has no deletion,
//! process, or network primitive to reach for. Contract 9 (no
//! `unwrap`/`expect`/`panic!` on production paths) is enforced the same way.
//!
//! The scanner is deliberately proved non-vacuous: each test first feeds it a
//! fixture file containing the banned token and asserts it is found, then
//! scans the real `src/` tree and requires silence. A scanner that silently
//! found nothing because it looked nowhere would fail the fixture half.
//!
//! The `src/` tree is read-only here. Every test still owns its own tempdir;
//! the fixture lives inside it and is asserted to.

use std::fs;
use std::path::{Path, PathBuf};

use common::assert_inside;

mod common;

/// Primitives the executor must never contain. `remove_dir_all` is banned
/// outright; `std::fs::remove_file(` likewise (a trash move renames, it never
/// unlinks).
const FORBIDDEN: [&str; 4] = [
    "std::fs::remove_file(",
    "remove_dir_all",
    "std::process",
    "std::net",
];

/// Panicking conveniences banned from production paths.
const PANICS: [&str; 3] = ["unwrap(", "expect(", "panic!"];

/// Every `.rs` file under `root`, recursively, in deterministic order.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("the source directory is readable") {
            let entry = entry.expect("a readable directory entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The production prefix of a source file.
///
/// Everything from the first `#[cfg(test)]` line onwards is test code and is
/// excluded (this crate's unit-test modules live at the end of their files),
/// and comment lines are excluded so documentation may *name* a forbidden
/// primitive while explaining why it is absent.
fn production_code(source: &str) -> String {
    let mut code = String::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("#[cfg(test)]") {
            break;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        code.push_str(line);
        code.push('\n');
    }
    code
}

/// Every occurrence of a banned token in the production code under `root`.
fn violations(root: &Path, needles: &[&str]) -> Vec<String> {
    let mut findings = Vec::new();
    for file in rust_sources(root) {
        let source = fs::read_to_string(&file).expect("a UTF-8 source file");
        let code = production_code(&source);
        for needle in needles {
            if let Some(offset) = code.find(needle) {
                let line = code[..offset].matches('\n').count() + 1;
                findings.push(format!("{}:{line}: {needle}", file.display()));
            }
        }
    }
    findings
}

/// The crate's own `src/` directory.
fn crate_source_tree() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn production_sources_contain_no_deletion_process_or_network_primitive() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let fixture = workspace.path().join("fixture.rs");
    fs::write(
        &fixture,
        "fn probe() { std::net::TcpStream::connect(\"127.0.0.1:9\"); }\n",
    )
    .expect("the fixture is writable");
    assert_inside(workspace.path(), &fixture);

    let fixture_findings = violations(workspace.path(), &FORBIDDEN);
    assert_eq!(
        fixture_findings.len(),
        1,
        "the scanner must detect a banned token; it found {fixture_findings:?}"
    );

    let source_tree = crate_source_tree();
    let files = rust_sources(&source_tree);
    assert!(
        files.iter().any(|file| file.ends_with("lib.rs")),
        "the scan must cover lib.rs, found {files:?}"
    );
    assert!(
        files.len() >= 5,
        "the scan must cover every module, found {files:?}"
    );

    let findings = violations(&source_tree, &FORBIDDEN);
    assert!(
        findings.is_empty(),
        "coresight-actions must contain no deletion, process, or network primitive: {findings:#?}"
    );
}

#[test]
fn production_sources_contain_no_panicking_calls() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let fixture = workspace.path().join("fixture.rs");
    fs::write(&fixture, "fn probe(v: Option<u8>) -> u8 { v.unwrap() }\n")
        .expect("the fixture is writable");
    assert_inside(workspace.path(), &fixture);

    let fixture_findings = violations(workspace.path(), &PANICS);
    assert_eq!(
        fixture_findings.len(),
        1,
        "the scanner must detect a panicking call; it found {fixture_findings:?}"
    );

    let findings = violations(&crate_source_tree(), &PANICS);
    assert!(
        findings.is_empty(),
        "production paths must not panic: {findings:#?}"
    );
}

#[test]
fn the_test_modules_are_excluded_so_test_code_may_panic() {
    let workspace = tempfile::tempdir().expect("a private tempdir");
    let source = "\
fn production() -> u8 { 1 }
#[cfg(test)]
mod tests {
    #[test]
    fn t() { assert!(true, \"{}\", 1u8); let _ = Some(1u8).unwrap(); }
}
";
    let fixture = workspace.path().join("fixture.rs");
    fs::write(&fixture, source).expect("the fixture is writable");
    assert_inside(workspace.path(), &fixture);

    assert!(
        violations(workspace.path(), &PANICS).is_empty(),
        "the #[cfg(test)] module must be excluded from the production scan"
    );
}

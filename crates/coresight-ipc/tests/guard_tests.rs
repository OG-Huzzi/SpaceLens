//! Architecture guard: the IPC crate must stay a pure, platform-neutral
//! dispatcher.
//!
//! Mirrors `crates/coresight-cleanup/tests/cleanup_tests.rs`: production
//! source may not reach the filesystem, spawn processes, open sockets, touch
//! a database, or link a desktop shell. It also may not contain panic paths
//! (`unwrap`, `expect`, `panic!`, `unreachable!`) — malformed requests must
//! become typed errors instead of unwinding in the host process.

use std::path::Path;

fn source_files() -> Vec<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(root)
        .expect("src directory is readable")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("rs"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "the guard must inspect real source files"
    );
    files
}

#[test]
fn production_sources_contain_no_io_process_network_or_tauri() {
    for path in source_files() {
        let text = std::fs::read_to_string(&path).expect("source is UTF-8");
        for forbidden in [
            "std::fs::",
            "std::process",
            "std::net",
            "std::io::",
            "reqwest",
            "tokio",
            "rusqlite",
            "std::os::windows",
            "std::os::unix",
            "Command::new",
            "tauri::",
            "SystemTime::now",
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
fn production_sources_have_no_panic_paths() {
    for path in source_files() {
        let text = std::fs::read_to_string(&path).expect("source is UTF-8");
        for (line_number, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                // Documentation may name the forbidden patterns when
                // explaining the rule; code may not use them.
                continue;
            }
            for forbidden in [
                ".unwrap()",
                ".expect(",
                "panic!",
                "unreachable!",
                "todo!",
                "unimplemented!",
            ] {
                assert!(
                    !line.contains(forbidden),
                    "{}:{} uses {forbidden} in production code: {}",
                    path.display(),
                    line_number + 1,
                    line.trim()
                );
            }
        }
    }
}

#[test]
fn manifest_declares_no_tauri_dependency() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("Cargo.toml is readable");
    for line in manifest.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.starts_with('[') {
            continue;
        }
        let normalized = trimmed.to_ascii_lowercase().replace(' ', "");
        assert!(
            !normalized.starts_with("tauri=") && !normalized.starts_with("tauri."),
            "coresight-ipc must stay Tauri-free: {trimmed}"
        );
    }
}

//! Real-filesystem probe fixtures, including links. Lives in `tests/` (not
//! `src/`) because creating a real symlink is OS-specific fixture work; the
//! helper skips gracefully on hosts where symlink creation needs privileges
//! (the repo's real-fs convention, see coresight-engine `real_fs_tests`).

use std::fs;
use std::io;

use coresight_macos::{MacFileProbe, ProbeKind, StdProbe};

/// Creates a real symlink when the OS allows it; returns false when the
/// host refuses (e.g. Windows without developer mode) so the test degrades
/// to the platform-supported assertions.
fn create_symlink(target: &std::path::Path, link: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
            && fs::symlink_metadata(link)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let ok = std::os::windows::fs::symlink_file(target, link).is_ok()
            || std::os::windows::fs::symlink_dir(target, link).is_ok();
        ok && fs::symlink_metadata(link)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        false
    }
}

#[test]
fn metadata_is_link_aware() {
    let dir = tempfile::tempdir().expect("temp fixture");
    fs::write(dir.path().join("file.txt"), b"x").expect("fixture file");
    let probe = StdProbe;
    assert_eq!(
        probe.metadata(&dir.path().join("file.txt")).unwrap(),
        ProbeKind::File
    );
    assert_eq!(probe.metadata(dir.path()).unwrap(), ProbeKind::Dir);
    assert_eq!(
        probe
            .metadata(&dir.path().join("missing"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );

    if create_symlink(&dir.path().join("file.txt"), &dir.path().join("link")) {
        // lstat semantics: a link is a link, never its target's kind.
        assert_eq!(
            probe.metadata(&dir.path().join("link")).unwrap(),
            ProbeKind::Symlink
        );
    }
}

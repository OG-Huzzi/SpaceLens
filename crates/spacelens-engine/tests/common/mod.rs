//! Shared test infrastructure: a deterministic in-memory `PlatformFs` fake.
//!
//! The fake lets tests exercise scanner behavior (cycles, permission walls,
//! vanishing files, >4 GiB sizes, cancellation) without privileges, real
//! disks, or platform dependence — per docs/TESTING_STRATEGY.md (synthetic
//! fixtures only, never real user data).

#![allow(dead_code)]

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use spacelens_engine::platform::{ChildInfo, FsKind, MetadataInfo, PlatformFs};
use spacelens_engine::ErrorCategory;

pub const E_PERM: i32 = 5; // EACCES / ERROR_ACCESS_DENIED
pub const E_NOT_FOUND: i32 = 2; // ENOENT / ERROR_FILE_NOT_FOUND

#[derive(Debug, Clone)]
pub enum FakeKind {
    File { size: u64, allocated: Option<u64> },
    Dir,
    Symlink { target: PathBuf },
    Other,
}

#[derive(Debug)]
pub struct FakeNode {
    pub kind: FakeKind,
    pub hidden: bool,
    /// When set, `metadata` fails with this raw OS error code.
    pub fail_metadata: Option<i32>,
    /// When set, listing this directory fails with this raw OS error code.
    pub fail_list: Option<i32>,
}

impl FakeNode {
    fn new(kind: FakeKind) -> Self {
        FakeNode {
            kind,
            hidden: false,
            fail_metadata: None,
            fail_list: None,
        }
    }
}

#[derive(Default)]
pub struct FakeFsInner {
    nodes: HashMap<PathBuf, FakeNode>,
    children: HashMap<PathBuf, Vec<OsString>>,
}

/// In-memory filesystem implementing `PlatformFs`.
pub struct FakeFs {
    inner: Mutex<FakeFsInner>,
}

fn path_split(path: &Path) -> Option<(PathBuf, OsString)> {
    let name = path.file_name()?.to_os_string();
    let parent = path.parent()?.to_path_buf();
    Some((parent, name))
}

fn err(raw: i32) -> io::Error {
    io::Error::from_raw_os_error(raw)
}

// --- scan-recording helpers ------------------------------------------------

use std::sync::Arc;

use spacelens_engine::progress::ScanEvent;
use spacelens_engine::{CancelHandle, ScanOptions, ScanSummary};

pub(crate) struct Recording {
    pub events: Vec<ScanEvent>,
}

impl Recording {
    pub fn entries(&self) -> impl Iterator<Item = &spacelens_engine::FsEntry> {
        self.events.iter().filter_map(|e| match e {
            ScanEvent::Entry(entry) => Some(entry.as_ref()),
            _ => None,
        })
    }

    pub fn summary(&self) -> &ScanSummary {
        self.events
            .iter()
            .find_map(|e| match e {
                ScanEvent::Completed(s) | ScanEvent::Cancelled(s) | ScanEvent::Failed(s) => {
                    Some(s.as_ref())
                }
                _ => None,
            })
            .expect("scan must emit exactly one terminal event")
    }

    pub fn terminal(&self) -> Option<&ScanEvent> {
        self.events.iter().find(|e| {
            matches!(
                e,
                ScanEvent::Completed(_) | ScanEvent::Cancelled(_) | ScanEvent::Failed(_)
            )
        })
    }
}

pub(crate) fn run_fake(
    fs: &Arc<FakeFs>,
    root: &Path,
    options: ScanOptions,
    cancel: &CancelHandle,
) -> Recording {
    let mut rec = Recording { events: Vec::new() };
    let summary = spacelens_engine::scan_with(fs.as_ref(), root, options, cancel, &mut |e| {
        rec.events.push(e);
    });
    assert_eq!(rec.summary().status, summary.status);
    rec
}

pub(crate) fn opts() -> ScanOptions {
    ScanOptions {
        threads: 2,
        ..ScanOptions::default()
    }
}

impl FakeFs {
    pub fn new() -> Self {
        FakeFs {
            inner: Mutex::new(FakeFsInner::default()),
        }
    }

    pub fn add_dir(&self, path: &Path) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .nodes
            .entry(path.to_path_buf())
            .or_insert_with(|| FakeNode::new(FakeKind::Dir));
        if let Some((parent, name)) = path_split(path) {
            inner.children.entry(parent).or_default().push(name);
        }
    }

    pub fn add_file(&self, path: &Path, size: u64) {
        self.add_file_ex(path, size, None);
    }

    pub fn add_file_ex(&self, path: &Path, size: u64, allocated: Option<u64>) {
        let mut inner = self.inner.lock().unwrap();
        inner.nodes.insert(
            path.to_path_buf(),
            FakeNode::new(FakeKind::File { size, allocated }),
        );
        if let Some((parent, name)) = path_split(path) {
            inner.children.entry(parent).or_default().push(name);
        }
    }

    pub fn add_other(&self, path: &Path) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .nodes
            .insert(path.to_path_buf(), FakeNode::new(FakeKind::Other));
        if let Some((parent, name)) = path_split(path) {
            inner.children.entry(parent).or_default().push(name);
        }
    }

    pub fn add_symlink(&self, path: &Path, target: &Path) {
        let mut inner = self.inner.lock().unwrap();
        inner.nodes.insert(
            path.to_path_buf(),
            FakeNode::new(FakeKind::Symlink {
                target: target.to_path_buf(),
            }),
        );
        if let Some((parent, name)) = path_split(path) {
            inner.children.entry(parent).or_default().push(name);
        }
    }

    pub fn set_hidden(&self, path: &Path, hidden: bool) {
        self.inner
            .lock()
            .unwrap()
            .nodes
            .get_mut(path)
            .unwrap()
            .hidden = hidden;
    }

    pub fn fail_list(&self, path: &Path, code: i32) {
        self.inner
            .lock()
            .unwrap()
            .nodes
            .get_mut(path)
            .unwrap()
            .fail_list = Some(code);
    }

    pub fn fail_metadata(&self, path: &Path, code: i32) {
        self.inner
            .lock()
            .unwrap()
            .nodes
            .get_mut(path)
            .unwrap()
            .fail_metadata = Some(code);
    }
}

impl PlatformFs for FakeFs {
    fn read_dir_entries(
        &self,
        dir: &Path,
        visit: &mut dyn FnMut(ChildInfo) -> bool,
    ) -> io::Result<()> {
        let (fail_list, children) = {
            let inner = self.inner.lock().unwrap();
            let node = inner.nodes.get(dir).ok_or_else(|| err(E_NOT_FOUND))?;
            let fail_list = node.fail_list;
            let children = inner.children.get(dir).cloned().unwrap_or_default();
            (fail_list, children)
        };
        if let Some(code) = fail_list {
            return Err(err(code));
        }
        for name in children {
            let child_path = dir.join(&name);
            let inner = self.inner.lock().unwrap();
            let kind = inner.nodes.get(&child_path).map(|n| &n.kind).cloned();
            drop(inner);
            let is_symlink = matches!(kind, Some(FakeKind::Symlink { .. }));
            let is_dir = matches!(kind, Some(FakeKind::Dir));
            if !visit(ChildInfo {
                name,
                is_dir,
                is_symlink,
            }) {
                break;
            }
        }
        Ok(())
    }

    fn metadata(&self, path: &Path) -> io::Result<MetadataInfo> {
        let inner = self.inner.lock().unwrap();
        let node = inner.nodes.get(path).ok_or_else(|| err(E_NOT_FOUND))?;
        if let Some(code) = node.fail_metadata {
            return Err(err(code));
        }
        let (kind, size, allocated) = match &node.kind {
            FakeKind::File { size, allocated } => (FsKind::File, *size, *allocated),
            FakeKind::Dir => (FsKind::Dir, 0, None),
            FakeKind::Symlink { .. } => (FsKind::Symlink, 0, None),
            FakeKind::Other => (FsKind::Other, 0, None),
        };
        Ok(MetadataInfo {
            kind,
            size,
            allocated,
            modified: Some(SystemTime::UNIX_EPOCH),
            created: None,
            accessed: None,
            changed: None,
            device: Some(1),
            inode: Some(0),
            file_id_hi: None,
            reparse: false,
            hidden: node.hidden,
        })
    }

    fn read_link_target(&self, path: &Path) -> io::Result<PathBuf> {
        let inner = self.inner.lock().unwrap();
        match inner.nodes.get(path).map(|n| &n.kind) {
            Some(FakeKind::Symlink { target }) => Ok(target.clone()),
            _ => Err(err(E_NOT_FOUND)),
        }
    }

    fn categorize_error(&self, e: &io::Error) -> ErrorCategory {
        match e.raw_os_error() {
            Some(E_PERM) => ErrorCategory::PermissionDenied,
            Some(E_NOT_FOUND) => ErrorCategory::NotFound,
            _ => ErrorCategory::Other,
        }
    }

    fn is_hidden(&self, _name: &OsStr, md: &MetadataInfo) -> bool {
        md.hidden
    }
}

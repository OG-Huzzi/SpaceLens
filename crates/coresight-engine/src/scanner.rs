//! The shared streaming scanner.
//!
//! One implementation drives every OS through [`PlatformFs`]: a bounded pool
//! of worker threads pulls directory tasks from a shared queue, streams
//! entries to the caller through a bounded channel (backpressure included),
//! and honors cancellation at every entry boundary. File entries are never
//! buffered engine-side — only the directory queue holds state, and it holds
//! only directory paths + identifiers, never file data.
//!
//! Termination is structural: links are recorded, never followed (default
//! policy), so the traversal graph is the filesystem's finite directory DAG.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime};

use crate::cancel::CancelHandle;
use crate::error::{ErrorCategory, ScanError, ScanErrorReport};
use crate::model::{EntryKind, FsEntry, LinkInfo, LinkKind};
use crate::options::ScanOptions;
use crate::platform::{ChildInfo, FsKind, PlatformFs};
use crate::progress::ScanEvent;
use crate::summary::{ScanStatus, ScanSummary};

pub use crate::options::default_threads;

/// Entry-channel bound. Workers block (natural backpressure) when the
/// consumer falls this far behind; keeps memory flat for any tree size.
const ENTRY_CHANNEL_BOUND: usize = 1024;

/// Entry point using the platform's default [`PlatformFs`].
pub fn scan(
    root: &Path,
    options: ScanOptions,
    cancel: &CancelHandle,
    sink: &mut dyn FnMut(ScanEvent),
) -> ScanSummary {
    scan_with(crate::platform::std_fs(), root, options, cancel, sink)
}

/// Entry point with an explicit platform implementation (tests inject an
/// in-memory fake here; production passes `platform::std_fs()`).
pub fn scan_with(
    platform: &dyn PlatformFs,
    root: &Path,
    options: ScanOptions,
    cancel: &CancelHandle,
    sink: &mut dyn FnMut(ScanEvent),
) -> ScanSummary {
    let started = Instant::now();
    let started_at = SystemTime::now();
    sink(ScanEvent::Started);

    let shared = Arc::new(Shared::new(cancel.clone()));

    // Fast path: cancelled before the scan began.
    if cancel.is_cancelled() {
        return finish(
            ScanStatus::Cancelled,
            root,
            &shared,
            started_at,
            started,
            sink,
        );
    }

    // Phase: preparing — resolve the root.
    let root_meta = match platform.metadata(root) {
        Ok(md) => md,
        Err(err) => {
            // The scan cannot proceed at all: typed failure, one root error.
            shared.record_error(root, &err, platform);
            sink(ScanEvent::Progress(shared.snapshot(started)));
            return finish(ScanStatus::Failed, root, &shared, started_at, started, sink);
        }
    };

    // A non-directory root is a one-entry scan (file / link / special node).
    if root_meta.kind != crate::platform::FsKind::Dir {
        let entry = shared.build_entry(None, root.to_path_buf(), &root_meta, platform);
        shared.apply_entry(&entry, &root_meta);
        if options.emit_entries {
            sink(ScanEvent::Entry(Box::new(entry)));
        }
        return finish(
            ScanStatus::Completed,
            root,
            &shared,
            started_at,
            started,
            sink,
        );
    }

    // Directory root: walk it.
    let (tx, rx) = sync_channel::<FsEntry>(ENTRY_CHANNEL_BOUND);
    let root_entry = shared.build_entry(None, root.to_path_buf(), &root_meta, platform);
    shared.apply_entry(&root_entry, &root_meta);
    if options.emit_entries {
        let _ = tx.send(root_entry.clone());
    }
    shared.push_task(DirTask {
        dir_id: root_entry.id,
        path: root.to_path_buf(),
        depth: 0,
    });

    let workers = options.threads.max(1);
    let progress_interval = options.progress_interval;
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let shared = Arc::clone(&shared);
            let tx = tx.clone();
            let options = options.clone();
            scope.spawn(move || {
                worker_loop(&shared, platform, &options, &tx);
            });
        }
        drop(tx); // coordinator holds no sender: disconnect == all workers done

        // Drain: forward entries to the caller, emit throttled progress.
        let mut last_progress = Instant::now();
        loop {
            match rx.recv_timeout(progress_interval) {
                Ok(entry) => sink(ScanEvent::Entry(Box::new(entry))),
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    if last_progress.elapsed() >= progress_interval {
                        sink(ScanEvent::Progress(shared.snapshot(started)));
                        last_progress = Instant::now();
                    }
                }
            }
        }
    });

    // One final, always-emitted progress event before the terminal event.
    sink(ScanEvent::Progress(shared.snapshot(started)));

    let status = if cancel.is_cancelled() {
        ScanStatus::Cancelled
    } else if shared.root_failed.load(Ordering::Relaxed) {
        ScanStatus::Failed
    } else {
        ScanStatus::Completed
    };
    finish(status, root, &shared, started_at, started, sink)
}

/// Builds the summary and emits the single terminal event.
fn finish(
    status: ScanStatus,
    root: &Path,
    shared: &Shared,
    started_at: SystemTime,
    started: Instant,
    sink: &mut dyn FnMut(ScanEvent),
) -> ScanSummary {
    let _ = started; // elapsed is observable via Progress snapshots
    let summary = shared.summary(status, root, started_at);
    let event = match status {
        ScanStatus::Completed => ScanEvent::Completed(Box::new(summary.clone())),
        ScanStatus::Cancelled => ScanEvent::Cancelled(Box::new(summary.clone())),
        ScanStatus::Failed => ScanEvent::Failed(Box::new(summary.clone())),
    };
    sink(event);
    summary
}

struct DirTask {
    /// Id of the already-emitted directory entry; children reference it as
    /// their parent.
    dir_id: u64,
    path: PathBuf,
    depth: u32,
}

struct QueueState {
    deque: VecDeque<DirTask>,
    /// Queued + in-flight directory tasks. Reaching 0 means traversal ended.
    pending: usize,
}

struct Shared {
    queue: Mutex<QueueState>,
    available: Condvar,
    cancel: CancelHandle,
    files: AtomicU64,
    dirs: AtomicU64,
    links: AtomicU64,
    other: AtomicU64,
    bytes: AtomicU64,
    allocated: AtomicU64,
    has_allocated: AtomicBool,
    errors: AtomicU64,
    entries_with_errors: AtomicU64,
    depth_capped: AtomicBool,
    /// Root directory could not be listed: the scan yields a Failed state.
    root_failed: AtomicBool,
    report: Mutex<ScanErrorReport>,
    next_id: AtomicU64,
}

impl Shared {
    fn new(cancel: CancelHandle) -> Self {
        Shared {
            queue: Mutex::new(QueueState {
                deque: VecDeque::new(),
                pending: 0,
            }),
            available: Condvar::new(),
            cancel,
            files: AtomicU64::new(0),
            dirs: AtomicU64::new(0),
            links: AtomicU64::new(0),
            other: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            allocated: AtomicU64::new(0),
            has_allocated: AtomicBool::new(false),
            errors: AtomicU64::new(0),
            entries_with_errors: AtomicU64::new(0),
            depth_capped: AtomicBool::new(false),
            root_failed: AtomicBool::new(false),
            report: Mutex::new(ScanErrorReport::new()),
            next_id: AtomicU64::new(0),
        }
    }

    fn push_task(&self, task: DirTask) {
        let mut q = self.queue.lock().unwrap();
        q.deque.push_back(task);
        q.pending += 1;
        drop(q);
        self.available.notify_one();
    }

    /// Pop the next task, or `None` when traversal is finished or cancelled.
    /// Blocks while the queue is empty but work is still in flight.
    fn pop_task(&self) -> Option<DirTask> {
        let mut q = self.queue.lock().unwrap();
        loop {
            if let Some(task) = q.deque.pop_front() {
                return Some(task);
            }
            if q.pending == 0 || self.cancel.is_cancelled() {
                return None;
            }
            q = self.available.wait(q).unwrap();
        }
    }

    /// Mark one in-flight task complete; wakes everyone when nothing remains.
    fn complete_task(&self) {
        let mut q = self.queue.lock().unwrap();
        q.pending = q.pending.saturating_sub(1);
        if q.pending == 0 {
            self.available.notify_all();
        }
    }

    fn record_error(&self, path: &Path, err: &std::io::Error, platform: &dyn PlatformFs) {
        let scan_err = ScanError::new(platform.categorize_error(err), path.to_path_buf(), err);
        self.report.lock().unwrap().record(scan_err);
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    fn build_entry(
        &self,
        parent_id: Option<u64>,
        path: PathBuf,
        md: &crate::platform::MetadataInfo,
        platform: &dyn PlatformFs,
    ) -> FsEntry {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        FsEntry {
            id,
            parent_id,
            path,
            kind: match md.kind {
                FsKind::File => EntryKind::File,
                FsKind::Dir => EntryKind::Dir,
                FsKind::Symlink => EntryKind::Link(LinkInfo {
                    kind: if md.reparse {
                        LinkKind::Reparse
                    } else {
                        LinkKind::Symlink
                    },
                    target: None,
                    broken: false,
                }),
                FsKind::Other => EntryKind::Other,
            },
            size: if md.kind == FsKind::File { md.size } else { 0 },
            allocated_size: md.allocated,
            modified: md.modified,
            created: md.created,
            accessed: md.accessed,
            changed: md.changed,
            device: md.device,
            inode: md.inode,
            file_id_hi: md.file_id_hi,
            hidden: platform.is_hidden(&name, md),
            error: None,
        }
    }

    /// Apply one emitted entry to the counters.
    fn apply_entry(&self, entry: &FsEntry, md: &crate::platform::MetadataInfo) {
        match &entry.kind {
            EntryKind::Dir => {
                self.dirs.fetch_add(1, Ordering::Relaxed);
            }
            EntryKind::File => {
                self.files.fetch_add(1, Ordering::Relaxed);
                self.bytes.fetch_add(entry.size, Ordering::Relaxed);
                if let Some(alloc) = md.allocated {
                    self.has_allocated.store(true, Ordering::Relaxed);
                    self.allocated.fetch_add(alloc, Ordering::Relaxed);
                }
            }
            EntryKind::Link(_) => {
                self.links.fetch_add(1, Ordering::Relaxed);
            }
            EntryKind::Other => {
                self.other.fetch_add(1, Ordering::Relaxed);
            }
        }
        if entry.error.is_some() {
            self.entries_with_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn snapshot(&self, started: Instant) -> crate::progress::ProgressSnapshot {
        crate::progress::ProgressSnapshot {
            files_seen: self.files.load(Ordering::Relaxed),
            dirs_seen: self.dirs.load(Ordering::Relaxed),
            bytes_seen: self.bytes.load(Ordering::Relaxed),
            errors_seen: self.errors.load(Ordering::Relaxed),
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    }

    fn summary(&self, status: ScanStatus, root: &Path, started_at: SystemTime) -> ScanSummary {
        ScanSummary {
            status,
            root: root.to_path_buf(),
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            links: self.links.load(Ordering::Relaxed),
            other_entries: self.other.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            allocated_bytes: self
                .has_allocated
                .load(Ordering::Relaxed)
                .then(|| self.allocated.load(Ordering::Relaxed)),
            entries_with_errors: self.entries_with_errors.load(Ordering::Relaxed),
            errors: self.report.lock().unwrap().clone(),
            started_at,
            finished_at: SystemTime::now(),
            depth_capped: self.depth_capped.load(Ordering::Relaxed),
        }
    }
}

/// Worker: pull directory tasks, stream their entries, honor cancellation.
fn worker_loop(
    shared: &Shared,
    platform: &dyn PlatformFs,
    options: &ScanOptions,
    tx: &SyncSender<FsEntry>,
) {
    while let Some(task) = shared.pop_task() {
        process_dir(shared, platform, options, task, tx);
        shared.complete_task();
        if shared.cancel.is_cancelled() {
            return;
        }
    }
}

/// Process one directory: list children, record metadata, enqueue
/// subdirectories. The directory's own entry was already emitted exactly
/// once at discovery time. Every failure path is recoverable.
fn process_dir(
    shared: &Shared,
    platform: &dyn PlatformFs,
    options: &ScanOptions,
    task: DirTask,
    tx: &SyncSender<FsEntry>,
) {
    if shared.cancel.is_cancelled() {
        return;
    }

    let mut visit = |child: ChildInfo| -> bool {
        if shared.cancel.is_cancelled() {
            return false; // stop listing early
        }
        let child_path = task.path.join(&child.name);
        if child.is_symlink {
            handle_link(shared, platform, options, task.dir_id, child_path, tx);
            return true;
        }
        let md = match platform.metadata(&child_path) {
            Ok(md) => md,
            Err(err) => {
                shared.record_error(&child_path, &err, platform);
                // Emit the entry with its listing-time hint and the typed
                // error state; the scan continues.
                let kind = if child.is_dir {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                };
                let entry = FsEntry {
                    id: shared.next_id.fetch_add(1, Ordering::Relaxed),
                    parent_id: Some(task.dir_id),
                    path: child_path.clone(),
                    kind,
                    size: 0,
                    allocated_size: None,
                    modified: None,
                    created: None,
                    accessed: None,
                    changed: None,
                    device: None,
                    inode: None,
                    file_id_hi: None,
                    hidden: false,
                    error: Some(crate::model::ErrorCategoryRef::from(
                        platform.categorize_error(&err),
                    )),
                };
                if child.is_dir {
                    shared.dirs.fetch_add(1, Ordering::Relaxed);
                } else {
                    shared.files.fetch_add(1, Ordering::Relaxed);
                }
                shared.entries_with_errors.fetch_add(1, Ordering::Relaxed);
                if options.emit_entries {
                    let _ = tx.send(entry);
                }
                return true;
            }
        };

        match md.kind {
            FsKind::Dir => {
                let entry =
                    shared.build_entry(Some(task.dir_id), child_path.clone(), &md, platform);
                // Emit exactly once here; the queued task carries only the id.
                shared.apply_entry(&entry, &md);
                // Depth cap: emit but do not descend.
                if let Some(max) = options.max_depth {
                    if task.depth + 1 > max {
                        shared.depth_capped.store(true, Ordering::Relaxed);
                        if options.emit_entries {
                            let _ = tx.send(entry);
                        }
                        return true;
                    }
                }
                if options.emit_entries {
                    let _ = tx.send(entry.clone());
                }
                shared.push_task(DirTask {
                    dir_id: entry.id,
                    path: child_path,
                    depth: task.depth + 1,
                });
            }
            FsKind::Symlink => {
                // Listed as non-link but gained a link bit between listing and
                // stat — same record-only policy as explicit links.
                handle_link(shared, platform, options, task.dir_id, child_path, tx);
            }
            other_kind => {
                let mut entry = shared.build_entry(Some(task.dir_id), child_path, &md, platform);
                if other_kind == FsKind::Other {
                    entry.kind = EntryKind::Other;
                }
                shared.apply_entry(&entry, &md);
                if options.emit_entries {
                    let _ = tx.send(entry);
                }
            }
        }
        true
    };

    if let Err(err) = platform.read_dir_entries(&task.path, &mut visit) {
        // The directory itself is unreadable — recorded, never fatal.
        shared.record_error(&task.path, &err, platform);
        if task.depth == 0 {
            // Root unreadable: nothing can be scanned; typed failure.
            shared.root_failed.store(true, Ordering::Relaxed);
        }
    }
}

/// Record a link without recursing into it. Broken links are an explicit
/// recorded state; a link whose target errors while resolving records a
/// typed error and continues the scan.
fn handle_link(
    shared: &Shared,
    platform: &dyn PlatformFs,
    options: &ScanOptions,
    parent_id: u64,
    link_path: PathBuf,
    tx: &SyncSender<FsEntry>,
) {
    let (md, md_err) = match platform.metadata(&link_path) {
        Ok(md) => (Some(md), None),
        Err(err) => (None, Some(err)),
    };

    let mut entry = match &md {
        Some(md) => shared.build_entry(Some(parent_id), link_path.clone(), md, platform),
        None => FsEntry {
            id: shared.next_id.fetch_add(1, Ordering::Relaxed),
            parent_id: Some(parent_id),
            path: link_path.clone(),
            kind: EntryKind::Link(LinkInfo {
                kind: LinkKind::Unknown,
                target: None,
                broken: true,
            }),
            size: 0,
            allocated_size: None,
            modified: None,
            created: None,
            accessed: None,
            changed: None,
            device: None,
            inode: None,
            file_id_hi: None,
            hidden: false,
            error: Some(crate::model::ErrorCategoryRef::from(
                platform.categorize_error(md_err.as_ref().unwrap()),
            )),
        },
    };

    // Read the target path (this does not resolve its existence by itself).
    let raw_target = platform.read_link_target(&link_path).ok();
    let mut broken = false;
    if let Some(target) = &raw_target {
        // read_link may return a target relative to the link's directory.
        let resolved = if target.is_absolute() {
            target.clone()
        } else {
            link_path
                .parent()
                .map(|p| p.join(target))
                .unwrap_or_else(|| target.clone())
        };
        match platform.metadata(&resolved) {
            Ok(_) => {}
            Err(err) => {
                if platform.categorize_error(&err) == ErrorCategory::NotFound {
                    broken = true;
                } else {
                    shared.record_error(&resolved, &err, platform);
                }
            }
        }
    }

    if let EntryKind::Link(info) = &mut entry.kind {
        info.target = raw_target;
        info.broken = broken;
    }

    shared.links.fetch_add(1, Ordering::Relaxed);
    if broken || md.is_none() {
        shared.entries_with_errors.fetch_add(1, Ordering::Relaxed);
        entry.error = Some(crate::model::ErrorCategoryRef::BrokenLink);
    }
    if options.emit_entries {
        let _ = tx.send(entry);
    }
}

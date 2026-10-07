//! Read-only local-metadata sources (Phase 6.2).
//!
//! Two filesystem-native application sources, each reading only local file
//! metadata through the established [`PathProber`] boundary:
//!
//! * **macOS application bundles** — `*.app/Contents/Info.plist`
//!   ([`BundlePlistProvider`]);
//! * **Freedesktop desktop entries** — `*.desktop` ([`DesktopEntryProvider`]);
//!
//! Every read is bounded by [`DiscoveryLimits::max_metadata_bytes`] and
//! reports its [`AccessState`] honestly: a denied or unreadable file is never
//! an empty application.
//!
//! ## What this module deliberately does NOT do
//!
//! No `mdfind`, `system_profiler`, `osascript`, `launchctl`, `winget`,
//! `dpkg`, `apt`, `dnf`, `pacman`, `brew`, `npm`, or `pip` — no subprocess at
//! all, no package-manager database, no network. Nothing here can execute
//! anything: the parsers are pure functions over bytes.
//!
//! ## The XML plist parser
//!
//! A deliberately small scanner for the flat `<key>`/`<string>` shape that
//! `Info.plist` uses in practice. It performs **no** entity expansion, no
//! external-entity resolution, and no recursion, so a hostile plist cannot
//! make it allocate unboundedly or read outside the buffer it was given.
//! Values the parser cannot represent are simply absent — never guessed.

use std::path::{Path, PathBuf};

use crate::discovery::{ApplicationProvider, ProviderError, ProviderOutcome};
use crate::domain::{
    ApplicationId, ApplicationRecord, ApplicationSource, DiscoveryLimits, PackageKind,
    SourceCoverage, SourceStatus,
};
use crate::footprint::PathProber;
use crate::observe::ProbedKind;

/// Metadata read from an application bundle's `Info.plist`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundleMetadata {
    pub bundle_identifier: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub short_version: Option<String>,
    pub executable: Option<String>,
    pub publisher: Option<String>,
}

impl BundleMetadata {
    /// `true` when the plist carried nothing usable as an application.
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.bundle_identifier.is_none() && self.executable.is_none()
    }
}

/// Which key a plist entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BundleKey {
    Identifier,
    Name,
    Version,
    ShortVersion,
    Executable,
    Publisher,
    Other,
}

fn classify_key(key: &str) -> BundleKey {
    match key {
        "CFBundleIdentifier" => BundleKey::Identifier,
        "CFBundleName" | "CFBundleDisplayName" => BundleKey::Name,
        "CFBundleVersion" => BundleKey::Version,
        "CFBundleShortVersionString" => BundleKey::ShortVersion,
        "CFBundleExecutable" => BundleKey::Executable,
        "NSHumanReadableCopyright" => BundleKey::Publisher,
        _ => BundleKey::Other,
    }
}

/// Read the tag name at `tag_start` (just past a `<`).
fn tag_name(bytes: &[u8], tag_start: usize) -> &[u8] {
    let rest = &bytes[tag_start..];
    let end = rest
        .iter()
        .position(|b| {
            *b == b'>' || *b == b'/' || *b == b' ' || *b == b'\t' || *b == b'\r' || *b == b'\n'
        })
        .unwrap_or(rest.len());
    &rest[..end]
}

/// The index just past the `>` that ends the tag starting at `tag_start`.
fn tag_end(bytes: &[u8], tag_start: usize) -> Option<usize> {
    bytes[tag_start..]
        .iter()
        .position(|b| *b == b'>')
        .map(|p| tag_start + p + 1)
}

/// Extract the text of the element starting at `from` (just past a `<`).
/// Returns the inner text and the index just past the closing tag.
fn element_at(bytes: &[u8], from: usize) -> Option<(String, usize)> {
    let rest = &bytes[from..];
    let gt = rest.iter().position(|b| *b == b'>')?;
    let tag_bytes = &rest[..gt];
    if tag_bytes.ends_with(b"/") {
        // Self-closing element: no text.
        return Some((String::new(), from + gt + 1));
    }
    let name_end = tag_bytes
        .iter()
        .position(|b| *b == b' ' || *b == b'\t' || *b == b'\r' || *b == b'\n')
        .unwrap_or(tag_bytes.len());
    let name = &tag_bytes[..name_end];
    let close: Vec<u8> = [b"</", name, b">"].concat();
    let content_start = from + gt + 1;
    let close_at = find(&bytes[content_start..], &close)? + content_start;
    let text = decode_entities(&bytes[content_start..close_at]);
    Some((text, close_at + close.len()))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Decode the five predefined XML entities ONLY. No DTD, no external
/// entities, no numeric character references beyond a bounded decimal/hex
/// form. Unknown entities are left literal (never silently dropped).
fn decode_entities(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    if !raw.contains('&') {
        return raw.into_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.as_ref();
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail.find(';').filter(|s| *s <= 12) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => None,
        };
        match replacement {
            Some(ch) => {
                out.push(ch);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse the flat `key`/value pairs of an XML `Info.plist`.
///
/// Deterministic: a later duplicate key overrides an earlier one (plists do
/// not forbid duplicates; the last definition is the effective one, matching
/// CoreFoundation). Bounded by the size of the input slice.
pub fn parse_info_plist(bytes: &[u8]) -> BundleMetadata {
    let mut meta = BundleMetadata::default();
    let mut i = 0usize;
    let mut pending: Option<BundleKey> = None;
    while i < bytes.len() {
        let Some(lt) = bytes[i..].iter().position(|b| *b == b'<') else {
            break;
        };
        let tag_start = i + lt + 1;
        // Markup that is not an element: a processing instruction
        // (`<?xml … ?>`), a comment (`<!-- … -->`), or the doctype
        // declaration. None of these carry application metadata, and
        // skipping them without interpreting them is what keeps the parser
        // immune to DTD/entity tricks.
        match bytes.get(tag_start) {
            Some(b'/') => {
                i = tag_start;
                continue;
            }
            Some(b'?') => {
                i = find(&bytes[tag_start..], b"?>")
                    .map(|p| tag_start + p + 2)
                    .unwrap_or(bytes.len());
                continue;
            }
            Some(b'!') => {
                i = if bytes[tag_start..].starts_with(b"!--") {
                    find(&bytes[tag_start..], b"-->")
                        .map(|p| tag_start + p + 3)
                        .unwrap_or(bytes.len())
                } else {
                    find(&bytes[tag_start..], b">")
                        .map(|p| tag_start + p + 1)
                        .unwrap_or(bytes.len())
                };
                continue;
            }
            _ => {}
        }
        // Container elements carry no scalar value: skip just their START
        // tag and keep scanning their children. (Using the generic
        // element-text path here would swallow the whole subtree.)
        if is_container_tag(bytes, tag_start) {
            i = tag_end(bytes, tag_start).unwrap_or(bytes.len());
            continue;
        }
        let Some((text, next)) = element_at(bytes, tag_start) else {
            break;
        };
        let is_key = tag_name(bytes, tag_start) == b"key";
        if is_key {
            pending = Some(classify_key(text.trim()));
        } else if let Some(key) = pending.take() {
            let value = text.trim().to_string();
            if !value.is_empty() {
                match key {
                    BundleKey::Identifier => meta.bundle_identifier = Some(value),
                    BundleKey::Name => meta.name = Some(value),
                    BundleKey::Version => meta.version = Some(value),
                    BundleKey::ShortVersion => meta.short_version = Some(value),
                    BundleKey::Executable => meta.executable = Some(value),
                    BundleKey::Publisher => meta.publisher = Some(value),
                    BundleKey::Other => {}
                }
            }
        }
        i = next.max(i + 1);
    }
    meta
}

/// Tag names whose content is a nested structure rather than a scalar. Their
/// START tag is stepped over and their children are scanned normally.
fn is_container_tag(bytes: &[u8], tag_start: usize) -> bool {
    matches!(
        tag_name(bytes, tag_start),
        b"dict" | b"array" | b"plist" | b"data"
    )
}

/// Build an [`ApplicationRecord`] from bundle metadata and a bundle path.
/// `None` when the metadata carries no usable application identity.
pub fn record_from_bundle(bundle: &Path, meta: &BundleMetadata) -> Option<ApplicationRecord> {
    let name = meta
        .name
        .clone()
        .or_else(|| meta.bundle_identifier.clone())
        .or_else(|| {
            bundle
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
        })?;
    let publisher = meta.publisher.clone();
    let executable_path = meta
        .executable
        .as_ref()
        .map(|exe| bundle.join("Contents").join("MacOS").join(exe));
    Some(ApplicationRecord {
        id: ApplicationId::derive(&name, publisher.as_deref()),
        name,
        version: meta.short_version.clone().or_else(|| meta.version.clone()),
        publisher,
        install_location: Some(bundle.to_path_buf()),
        install_date: None,
        estimated_size_bytes: None,
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: ApplicationSource::BundleInfoPlist,
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: Vec::new(),
        bundle_identifier: meta.bundle_identifier.clone(),
        executable_path,
        provenance: vec![ApplicationSource::BundleInfoPlist],
    })
}

/// Result of one bundle-root enumeration: records plus the honest facts the
/// coverage note is built from.
#[derive(Debug)]
struct ScanOutcome {
    records: Vec<ApplicationRecord>,
    directories: u64,
    entries_truncated: u64,
    metadata_truncated: u64,
    unreadable_paths: Vec<String>,
    root: SourceStatus,
    root_note: Option<String>,
}

impl Default for ScanOutcome {
    fn default() -> Self {
        ScanOutcome {
            records: Vec::new(),
            directories: 0,
            entries_truncated: 0,
            metadata_truncated: 0,
            unreadable_paths: Vec::new(),
            root: SourceStatus::Unavailable,
            root_note: None,
        }
    }
}

/// Reads `*.app/Contents/Info.plist` under one or more bundle roots using a
/// bounded prober. No subprocess, no TCC bypass: a denied directory is
/// reported as such and never as an empty inventory.
pub struct BundlePlistProvider<'a> {
    pub prober: &'a dyn PathProber,
    pub roots: Vec<PathBuf>,
    pub limits: DiscoveryLimits,
}

impl BundlePlistProvider<'_> {
    fn scan(&self) -> ScanOutcome {
        let mut out = ScanOutcome::default();
        let mut missing_roots: Vec<String> = Vec::new();
        let mut read_roots = 0usize;
        for root in &self.roots {
            let listing = self
                .prober
                .list_dir(root, self.limits.max_children_per_root);
            if !listing.access.is_read() {
                missing_roots.push(format!("{} ({:?})", root.display(), listing.access));
                continue;
            }
            read_roots += 1;
            out.entries_truncated += listing.overflow;
            let mut entries = listing.entries;
            entries.sort_by(|a, b| {
                a.path
                    .as_os_str()
                    .as_encoded_bytes()
                    .cmp(b.path.as_os_str().as_encoded_bytes())
            });
            for entry in entries {
                if out.directories >= self.limits.max_directories as u64 {
                    out.entries_truncated += 1;
                    continue;
                }
                if entry.kind != ProbedKind::Dir || !is_bundle(&entry.path) {
                    continue;
                }
                out.directories += 1;
                let plist = entry.path.join("Contents").join("Info.plist");
                let observed = self
                    .prober
                    .read_file_bounded(&plist, self.limits.max_metadata_bytes);
                if !observed.access.is_read() {
                    out.unreadable_paths.push(format!(
                        "{} ({:?})",
                        plist.display(),
                        observed.access
                    ));
                    continue;
                }
                if observed.truncated {
                    out.metadata_truncated += 1;
                }
                let meta = parse_info_plist(&observed.bytes);
                if let Some(record) = record_from_bundle(&entry.path, &meta) {
                    out.records.push(record);
                }
            }
        }
        out.root = if read_roots == 0 {
            SourceStatus::Unavailable
        } else if read_roots == self.roots.len()
            && out.unreadable_paths.is_empty()
            && out.entries_truncated == 0
            && out.metadata_truncated == 0
        {
            SourceStatus::Complete
        } else {
            SourceStatus::Partial
        };
        if !missing_roots.is_empty() {
            out.root_note = Some(format!(
                "unreadable bundle roots: {}",
                missing_roots.join(", ")
            ));
        }
        out
    }
}

fn is_bundle(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("app"))
        .unwrap_or(false)
}

impl ApplicationProvider for BundlePlistProvider<'_> {
    fn source_tag(&self) -> &'static str {
        "macos-bundle-plist"
    }

    fn enumerate(&self) -> Result<Vec<ApplicationRecord>, ProviderError> {
        Ok(self.scan().records)
    }

    fn enumerate_outcome(&self) -> ProviderOutcome {
        let out = self.scan();
        let coverage = match out.root {
            SourceStatus::Unavailable => SourceCoverage::with_status(
                "macos-bundle-plist",
                SourceStatus::Unavailable,
                Some(format!(
                    "no bundle root could be read: {}",
                    out.root_note.unwrap_or_else(|| "none".to_string())
                )),
            ),
            SourceStatus::Complete => SourceCoverage::complete("macos-bundle-plist"),
            _ => {
                let mut notes = Vec::new();
                if let Some(n) = out.root_note {
                    notes.push(n);
                }
                if !out.unreadable_paths.is_empty() {
                    notes.push(format!(
                        "{} bundle(s) unreadable: {}",
                        out.unreadable_paths.len(),
                        out.unreadable_paths.join(", ")
                    ));
                }
                if out.entries_truncated > 0 {
                    notes.push(format!(
                        "{} entries not examined (bound)",
                        out.entries_truncated
                    ));
                }
                if out.metadata_truncated > 0 {
                    notes.push(format!(
                        "{} plist(s) truncated at the metadata byte bound",
                        out.metadata_truncated
                    ));
                }
                SourceCoverage::with_status(
                    "macos-bundle-plist",
                    SourceStatus::Partial,
                    Some(notes.join("; ")),
                )
            }
        };
        ProviderOutcome {
            records: out.records,
            coverage,
        }
    }
}

// ---------------------------------------------------------------------------
// Freedesktop desktop entries
// ---------------------------------------------------------------------------

/// Metadata read from a `.desktop` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopEntryMetadata {
    pub name: Option<String>,
    /// Absolute `Exec` path when the entry names one; a bare command name is
    /// NOT resolved (that would require `PATH` search = environment
    /// guessing) and stays `None`.
    pub exec_path: Option<PathBuf>,
    pub try_exec_path: Option<PathBuf>,
    pub entry_type: Option<String>,
}

/// Parse a desktop entry. Only the `[Desktop Entry]` group is considered;
/// localized keys (`Name[de]`) are ignored so identity stays stable.
///
/// Bounded by the input slice; no escaping beyond the `\`-escapes the spec
/// defines for keys.
pub fn parse_desktop_entry(bytes: &[u8]) -> DesktopEntryMetadata {
    let text = String::from_utf8_lossy(bytes);
    let mut meta = DesktopEntryMetadata::default();
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        if value.is_empty() || key.contains('[') {
            continue;
        }
        match key {
            "Name" => {
                if meta.name.is_none() {
                    meta.name = Some(value.to_string());
                }
            }
            "Exec" => {
                meta.exec_path = absolute_exec(value);
            }
            "TryExec" => {
                meta.try_exec_path = absolute_exec(value);
            }
            "Type" => meta.entry_type = Some(value.to_string()),
            _ => {}
        }
    }
    meta
}

/// The absolute path of an `Exec`/`TryExec` value: the first field when it
/// is absolute and contains no field-code/quoting surprises. A relative or
/// bare command name is not resolved — CoreSight does not guess `PATH`.
///
/// "Absolute" is tested as a leading `/` (the POSIX form defined by the
/// desktop-entry spec) rather than via `Path::is_absolute`, so the parse is
/// identical on every host and does not silently depend on the OS the parser
/// happens to be compiled for.
fn absolute_exec(value: &str) -> Option<PathBuf> {
    let first = value.split_whitespace().next()?;
    if first.starts_with('"') || first.contains('%') {
        return None;
    }
    if first.starts_with('/') {
        Some(PathBuf::from(first))
    } else {
        None
    }
}

/// Build an [`ApplicationRecord`] from a desktop entry.
pub fn record_from_desktop_entry(
    entry_path: &Path,
    meta: &DesktopEntryMetadata,
) -> Option<ApplicationRecord> {
    let name = meta.name.clone().or_else(|| {
        entry_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
    })?;
    let executable_path = meta
        .exec_path
        .clone()
        .or_else(|| meta.try_exec_path.clone());
    let install_location = executable_path
        .as_ref()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .or_else(|| entry_path.parent().map(|p| p.to_path_buf()));
    Some(ApplicationRecord {
        id: ApplicationId::derive(&name, None),
        name,
        version: None,
        publisher: None,
        install_location,
        install_date: None,
        estimated_size_bytes: None,
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: ApplicationSource::DesktopEntry,
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: Vec::new(),
        bundle_identifier: None,
        executable_path,
        provenance: vec![ApplicationSource::DesktopEntry],
    })
}

/// Reads `*.desktop` entries from bounded application directories.
pub struct DesktopEntryProvider<'a> {
    pub prober: &'a dyn PathProber,
    pub roots: Vec<PathBuf>,
    pub limits: DiscoveryLimits,
}

impl DesktopEntryProvider<'_> {
    fn scan(&self) -> ScanOutcome {
        let mut out = ScanOutcome::default();
        let mut missing_roots: Vec<String> = Vec::new();
        let mut read_roots = 0usize;
        for root in &self.roots {
            let listing = self
                .prober
                .list_dir(root, self.limits.max_children_per_root);
            if !listing.access.is_read() {
                missing_roots.push(format!("{} ({:?})", root.display(), listing.access));
                continue;
            }
            read_roots += 1;
            out.entries_truncated += listing.overflow;
            let mut entries = listing.entries;
            entries.sort_by(|a, b| {
                a.path
                    .as_os_str()
                    .as_encoded_bytes()
                    .cmp(b.path.as_os_str().as_encoded_bytes())
            });
            for entry in entries {
                if !is_desktop_entry(&entry.path) {
                    continue;
                }
                if out.directories >= self.limits.max_directories as u64 {
                    out.entries_truncated += 1;
                    continue;
                }
                out.directories += 1;
                let observed = self
                    .prober
                    .read_file_bounded(&entry.path, self.limits.max_metadata_bytes);
                if !observed.access.is_read() {
                    out.unreadable_paths.push(format!(
                        "{} ({:?})",
                        entry.path.display(),
                        observed.access
                    ));
                    continue;
                }
                if observed.truncated {
                    out.metadata_truncated += 1;
                }
                let meta = parse_desktop_entry(&observed.bytes);
                // Only real application entries: a `Type=Link` entry is not
                // an installed application.
                if let Some(t) = &meta.entry_type {
                    if !t.eq_ignore_ascii_case("Application") {
                        continue;
                    }
                }
                if let Some(record) = record_from_desktop_entry(&entry.path, &meta) {
                    out.records.push(record);
                }
            }
        }
        out.root = if read_roots == 0 {
            SourceStatus::Unavailable
        } else if read_roots == self.roots.len()
            && out.unreadable_paths.is_empty()
            && out.entries_truncated == 0
            && out.metadata_truncated == 0
        {
            SourceStatus::Complete
        } else {
            SourceStatus::Partial
        };
        if !missing_roots.is_empty() {
            out.root_note = Some(format!(
                "unreadable application directories: {}",
                missing_roots.join(", ")
            ));
        }
        out
    }
}

fn is_desktop_entry(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("desktop"))
        .unwrap_or(false)
}

impl ApplicationProvider for DesktopEntryProvider<'_> {
    fn source_tag(&self) -> &'static str {
        "linux-desktop-entry"
    }

    fn enumerate(&self) -> Result<Vec<ApplicationRecord>, ProviderError> {
        Ok(self.scan().records)
    }

    fn enumerate_outcome(&self) -> ProviderOutcome {
        let out = self.scan();
        let coverage = match out.root {
            SourceStatus::Unavailable => SourceCoverage::with_status(
                "linux-desktop-entry",
                SourceStatus::Unavailable,
                Some(format!(
                    "no application directory could be read: {}",
                    out.root_note.unwrap_or_else(|| "none".to_string())
                )),
            ),
            SourceStatus::Complete => SourceCoverage::complete("linux-desktop-entry"),
            _ => {
                let mut notes = Vec::new();
                if let Some(n) = out.root_note {
                    notes.push(n);
                }
                if !out.unreadable_paths.is_empty() {
                    notes.push(format!(
                        "{} entry file(s) unreadable: {}",
                        out.unreadable_paths.len(),
                        out.unreadable_paths.join(", ")
                    ));
                }
                if out.entries_truncated > 0 {
                    notes.push(format!(
                        "{} entries not examined (bound)",
                        out.entries_truncated
                    ));
                }
                SourceCoverage::with_status(
                    "linux-desktop-entry",
                    SourceStatus::Partial,
                    Some(notes.join("; ")),
                )
            }
        };
        ProviderOutcome {
            records: out.records,
            coverage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::footprint::{offer_path, BoundedListing};
    use crate::observe::{DirectoryObservation, FileObservation, ListedEntry};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct FakeProber {
        dirs: BTreeMap<PathBuf, DirectoryObservation>,
        files: BTreeMap<PathBuf, FileObservation>,
    }

    impl PathProber for FakeProber {
        fn children_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
            let mut set = std::collections::BTreeSet::new();
            let mut overflow = 0u64;
            for entry in self
                .dirs
                .get(dir)
                .map(|d| d.entries.clone())
                .unwrap_or_default()
            {
                if entry.kind == ProbedKind::Dir {
                    offer_path(&mut set, max, entry.path, &mut overflow);
                }
            }
            BoundedListing {
                names: set.into_iter().collect(),
                overflow,
            }
        }
        fn entries_bounded(&self, dir: &Path, max: usize) -> BoundedListing {
            let mut set = std::collections::BTreeSet::new();
            let mut overflow = 0u64;
            for entry in self
                .dirs
                .get(dir)
                .map(|d| d.entries.clone())
                .unwrap_or_default()
            {
                offer_path(&mut set, max, entry.path, &mut overflow);
            }
            BoundedListing {
                names: set.into_iter().collect(),
                overflow,
            }
        }
        fn list_dir(&self, dir: &Path, _max: usize) -> DirectoryObservation {
            self.dirs
                .get(dir)
                .cloned()
                .unwrap_or_else(DirectoryObservation::does_not_exist)
        }
        fn read_file_bounded(&self, path: &Path, _max_bytes: u64) -> FileObservation {
            self.files
                .get(path)
                .cloned()
                .unwrap_or_else(FileObservation::does_not_exist)
        }
    }

    fn dir(entries: &[(&str, ProbedKind)]) -> DirectoryObservation {
        DirectoryObservation::read(
            entries
                .iter()
                .map(|(p, k)| ListedEntry {
                    path: PathBuf::from(p),
                    kind: *k,
                })
                .collect(),
            0,
        )
    }

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Example &amp; Co</string>
    <key>CFBundleIdentifier</key>
    <string>com.example.app</string>
    <key>CFBundleShortVersionString</key>
    <string>2.1</string>
    <key>CFBundleExecutable</key>
    <string>Example</string>
</dict>
</plist>"#;

    #[test]
    fn plist_parses_the_flat_keys() {
        let meta = parse_info_plist(PLIST.as_bytes());
        assert_eq!(meta.name.as_deref(), Some("Example & Co"));
        assert_eq!(meta.bundle_identifier.as_deref(), Some("com.example.app"));
        assert_eq!(meta.short_version.as_deref(), Some("2.1"));
        assert_eq!(meta.executable.as_deref(), Some("Example"));
    }

    #[test]
    fn plist_does_not_resolve_entities_it_does_not_define() {
        let bytes = b"<dict><key>CFBundleName</key><string>&xxe; &amp; ok</string></dict>";
        let meta = parse_info_plist(bytes);
        assert_eq!(meta.name.as_deref(), Some("&xxe; & ok"));
    }

    #[test]
    fn plist_of_garbage_is_empty_not_a_panic() {
        assert!(parse_info_plist(b"not a plist at all").is_empty());
        assert!(parse_info_plist(b"<dict><key>CFBundleName").is_empty());
        assert!(parse_info_plist(b"").is_empty());
    }

    #[test]
    fn bundle_record_uses_identity_from_name_and_publisher() {
        let meta = BundleMetadata {
            name: Some("Example".into()),
            bundle_identifier: Some("com.example.app".into()),
            executable: Some("Example".into()),
            ..BundleMetadata::default()
        };
        let rec = record_from_bundle(Path::new("/Applications/Example.app"), &meta).unwrap();
        assert_eq!(rec.id, ApplicationId::derive("Example", None));
        assert_eq!(rec.source, ApplicationSource::BundleInfoPlist);
        assert_eq!(
            rec.executable_path,
            Some(PathBuf::from(
                "/Applications/Example.app/Contents/MacOS/Example"
            ))
        );
        assert_eq!(rec.bundle_identifier.as_deref(), Some("com.example.app"));
    }

    #[test]
    fn desktop_entry_parses_only_the_main_group() {
        let text = "\
[Desktop Entry]
Type=Application
Name=Example
Name[de]=Beispiel
Exec=/usr/bin/example --flag
Icon=example
";
        let meta = parse_desktop_entry(text.as_bytes());
        assert_eq!(meta.name.as_deref(), Some("Example"));
        assert_eq!(meta.exec_path, Some(PathBuf::from("/usr/bin/example")));
        assert_eq!(meta.entry_type.as_deref(), Some("Application"));
    }

    #[test]
    fn relative_exec_is_not_guessed() {
        let meta = parse_desktop_entry(b"[Desktop Entry]\nName=X\nExec=example --x\n");
        assert!(meta.exec_path.is_none(), "PATH resolution is never guessed");
    }

    #[test]
    fn non_application_desktop_entries_are_skipped() {
        let prober = FakeProber {
            dirs: BTreeMap::from([(
                PathBuf::from("/usr/share/applications"),
                dir(&[
                    ("/usr/share/applications/link.desktop", ProbedKind::File),
                    ("/usr/share/applications/app.desktop", ProbedKind::File),
                ]),
            )]),
            files: BTreeMap::from([
                (
                    PathBuf::from("/usr/share/applications/link.desktop"),
                    FileObservation::read(
                        b"[Desktop Entry]\nType=Link\nName=Link\n".to_vec(),
                        false,
                    ),
                ),
                (
                    PathBuf::from("/usr/share/applications/app.desktop"),
                    FileObservation::read(
                        b"[Desktop Entry]\nType=Application\nName=App\nExec=/usr/bin/app\n"
                            .to_vec(),
                        false,
                    ),
                ),
            ]),
        };
        let provider = DesktopEntryProvider {
            prober: &prober,
            roots: vec![PathBuf::from("/usr/share/applications")],
            limits: DiscoveryLimits::default(),
        };
        let out = provider.enumerate_outcome();
        assert_eq!(out.records.len(), 1);
        assert_eq!(out.records[0].name, "App");
        assert_eq!(out.coverage.status, SourceStatus::Complete);
    }

    #[test]
    fn an_unreadable_root_is_unavailable_not_empty() {
        let prober = FakeProber::default();
        let provider = DesktopEntryProvider {
            prober: &prober,
            roots: vec![PathBuf::from("/usr/share/applications")],
            limits: DiscoveryLimits::default(),
        };
        let out = provider.enumerate_outcome();
        assert!(out.records.is_empty());
        assert_eq!(out.coverage.status, SourceStatus::Unavailable);
        assert!(out.coverage.note.is_some());
    }

    #[test]
    fn a_denied_root_is_partial_and_never_empty_success() {
        let prober = FakeProber {
            dirs: BTreeMap::from([(
                PathBuf::from("/Applications"),
                DirectoryObservation::inaccessible("denied"),
            )]),
            files: BTreeMap::new(),
        };
        let provider = BundlePlistProvider {
            prober: &prober,
            roots: vec![PathBuf::from("/Applications")],
            limits: DiscoveryLimits::default(),
        };
        let out = provider.enumerate_outcome();
        assert_eq!(out.coverage.status, SourceStatus::Unavailable);
        assert_ne!(out.coverage.status, SourceStatus::Complete);
    }

    #[test]
    fn a_missing_plist_makes_the_source_partial() {
        let prober = FakeProber {
            dirs: BTreeMap::from([(
                PathBuf::from("/Applications"),
                dir(&[("/Applications/Example.app", ProbedKind::Dir)]),
            )]),
            files: BTreeMap::new(),
        };
        let provider = BundlePlistProvider {
            prober: &prober,
            roots: vec![PathBuf::from("/Applications")],
            limits: DiscoveryLimits::default(),
        };
        let out = provider.enumerate_outcome();
        assert!(out.records.is_empty());
        assert_eq!(out.coverage.status, SourceStatus::Partial);
        assert!(out.coverage.note.unwrap().contains("unreadable"));
    }

    #[test]
    fn bundle_scanning_is_deterministic() {
        let prober = FakeProber {
            dirs: BTreeMap::from([(
                PathBuf::from("/Applications"),
                dir(&[
                    ("/Applications/B.app", ProbedKind::Dir),
                    ("/Applications/A.app", ProbedKind::Dir),
                ]),
            )]),
            files: BTreeMap::from([
                (
                    PathBuf::from("/Applications/A.app/Contents/Info.plist"),
                    FileObservation::read(
                        b"<dict><key>CFBundleName</key><string>A</string></dict>".to_vec(),
                        false,
                    ),
                ),
                (
                    PathBuf::from("/Applications/B.app/Contents/Info.plist"),
                    FileObservation::read(
                        b"<dict><key>CFBundleName</key><string>B</string></dict>".to_vec(),
                        false,
                    ),
                ),
            ]),
        };
        let provider = BundlePlistProvider {
            prober: &prober,
            roots: vec![PathBuf::from("/Applications")],
            limits: DiscoveryLimits::default(),
        };
        let first = provider.enumerate_outcome();
        let second = provider.enumerate_outcome();
        assert_eq!(first, second);
        assert_eq!(first.records.len(), 2);
    }
}

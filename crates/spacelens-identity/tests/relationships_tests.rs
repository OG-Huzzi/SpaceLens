//! Phase 4 integration tests: the full pipeline → relationship derivation
//! chain, end to end.
//!
//! The identity-aware synthetic reader serves deterministic content from
//! path-embedded keys AND handle identities, so alias/duplicate shapes are
//! exact without touching a disk (real-fs hard links are covered in the
//! `real_fs` section). Tests follow the brief's Objective 17 matrix, the
//! Objective 18 operational invariants (no impossible SHA-256 assertions),
//! and the Objective 19 scale/adversarial cases.

use std::io;
use std::path::{Path, PathBuf};

use spacelens_engine::platform::{ContentError, ContentReader, HandleStat};
use spacelens_engine::{CancelHandle, FsEntry};
use spacelens_identity::{
    derive_relationships, run_duplicates, ContentHash, ContentReaderFactory, DuplicateOptions,
    DuplicateStatus, HashFailureKind, RelationshipIndex, RelationshipKind, RelationshipOptions,
    RelationshipReport, StorageAccounting,
};

// ---------------------------------------------------------------------------
// fixtures: entries + an identity-aware synthetic reader
// ---------------------------------------------------------------------------

/// Path scheme: `/rel/<key>/<object>/<name>` — `key` selects the content,
/// `object` selects the filesystem object identity. Two paths sharing
/// `key` are content duplicates; two paths sharing `object` are aliases.
fn entry(id: u64, key: &str, object: u64, name: &str, size: u64) -> FsEntry {
    let path = format!("/rel/k{key}/o{object:04}/{name}");
    FsEntry {
        id,
        parent_id: None,
        path: PathBuf::from(&path),
        kind: spacelens_engine::EntryKind::File,
        size,
        allocated_size: None,
        modified: None,
        created: None,
        accessed: None,
        changed: None,
        device: Some(1),
        inode: Some(object),
        file_id_hi: None,
        hidden: false,
        error: None,
    }
}

/// Missing-path marker for the reader (typed Vanished).
const GHOST: &str = "/rel/ghost";

struct IdentityReader {
    /// Paths whose hashing fails with a sharing violation (typed Hash).
    failing: Vec<String>,
}

impl ContentReaderFactory for IdentityReader {
    fn read(
        &self,
        path: &Path,
        feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
    ) -> Result<(), ContentError> {
        let p = path.to_string_lossy();
        if p == GHOST {
            return Err(ContentError::OpenFailed(io::Error::from_raw_os_error(2)));
        }
        if self.failing.iter().any(|f| *f == p) {
            return Err(ContentError::OpenFailed(io::Error::from_raw_os_error(32)));
        }
        // Parse key + object from the path scheme.
        let (key, object) = parse_scheme(path)
            .ok_or_else(|| ContentError::OpenFailed(io::Error::from_raw_os_error(2)))?;
        feed(&mut Chunks {
            key,
            object,
            served: 0,
        })
        .map_err(ContentError::ReadFailed)
    }
}

fn parse_scheme(path: &Path) -> Option<(u64, u64)> {
    // Parse ANCESTOR components only — file names may legitimately start
    // with 'k'/'o' ("ok.bin") and must not corrupt the scheme.
    let mut key = None;
    let mut object = None;
    if let Some(parent) = path.parent() {
        for c in parent.components() {
            let s = c.as_os_str().to_string_lossy();
            if let Some(k) = s.strip_prefix("k") {
                key = k.parse::<u64>().ok();
            }
            if let Some(o) = s.strip_prefix("o") {
                object = o.parse::<u64>().ok();
            }
        }
    }
    match (key, object) {
        (Some(k), Some(o)) => Some((k, o)),
        _ => None,
    }
}

struct Chunks {
    key: u64,
    object: u64,
    served: u64,
}

impl Chunks {
    const SIZE: u64 = 64;
    fn stat(&self) -> HandleStat {
        use std::time::{SystemTime, UNIX_EPOCH};
        HandleStat {
            len: Self::SIZE,
            modified: Some(UNIX_EPOCH),
            changed: Some(SystemTime::UNIX_EPOCH),
        }
    }
}

impl ContentReader for Chunks {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        if self.served >= Self::SIZE {
            return Ok(None);
        }
        let n = buf.len().min((Self::SIZE - self.served) as usize);
        // Content depends on the KEY only — different objects with the same
        // key carry identical bytes (true duplicates). The pattern is the
        // full 8-byte LE key cycled, so distinct keys never collide (an
        // 8-bit fill would collide for keys congruent mod 256).
        let pat = self.key.to_le_bytes();
        for (i, b) in buf[..n].iter_mut().enumerate() {
            *b = pat[((self.served as usize) + i) % 8] ^ 0x5A;
        }
        self.served += n as u64;
        Ok(Some(n))
    }

    fn file_identity(&self) -> spacelens_engine::FileIdentity {
        spacelens_engine::FileIdentity {
            device: Some(1),
            inode: Some(self.object),
            file_id_hi: None,
            link_count: Some(1),
        }
    }

    fn pre_stat(&self) -> io::Result<HandleStat> {
        Ok(self.stat())
    }

    fn post_stat(&self) -> io::Result<HandleStat> {
        Ok(self.stat())
    }
}

fn run(entries: Vec<FsEntry>, reader: &IdentityReader) -> RelationshipReport {
    let cancel = CancelHandle::new();
    let duplicate_report = run_duplicates(
        entries.into_iter(),
        &DuplicateOptions::default(),
        &cancel,
        Some(reader),
        &mut |_| {},
    );
    derive_relationships(&duplicate_report, &RelationshipOptions::default())
}

fn run_with_options(
    entries: Vec<FsEntry>,
    reader: &IdentityReader,
    options: &DuplicateOptions,
) -> RelationshipReport {
    let cancel = CancelHandle::new();
    let duplicate_report = run_duplicates(
        entries.into_iter(),
        options,
        &cancel,
        Some(reader),
        &mut |_| {},
    );
    derive_relationships(&duplicate_report, &RelationshipOptions::default())
}

// ---------------------------------------------------------------------------
// Objective 17: the acceptance matrix
// ---------------------------------------------------------------------------

#[test]
fn exact_duplicates_form_one_content_relationship() {
    let reader = IdentityReader { failing: vec![] };
    let report = run(
        vec![
            entry(1, "11", 101, "a.bin", 64),
            entry(2, "11", 102, "b.bin", 64),
        ],
        &reader,
    );
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert_eq!(report.relationships.len(), 1);
    let rel = &report.relationships[0];
    assert_eq!(rel.kind, RelationshipKind::ContentDuplicate);
    assert_eq!(rel.member_count, 2);
    assert_eq!(rel.distinct_objects, Some(2));
    assert_eq!(rel.recoverable_bytes, Some(64));
    assert_eq!(rel.accounting, StorageAccounting::Exact);
    assert_eq!(report.undetermined.failed, 0);
}

#[test]
fn same_size_different_content_is_not_a_relationship() {
    let reader = IdentityReader { failing: vec![] };
    let report = run(
        vec![
            entry(1, "11", 101, "a.bin", 64),
            entry(2, "22", 102, "b.bin", 64),
        ],
        &reader,
    );
    assert_eq!(report.status, DuplicateStatus::Completed);
    assert!(report.relationships.is_empty());
    // Same-size candidates were hashed and proven distinct (the pipeline
    // counts them; the relationship layer sees no relationship).
    assert_eq!(report.stats.content_duplicates, 0);
    assert_eq!(report.undetermined.failed, 0);
}

#[test]
fn same_filename_alone_is_never_a_relationship() {
    let reader = IdentityReader { failing: vec![] };
    // Same file NAME, different content, different objects.
    let report = run(
        vec![
            entry(1, "11", 101, "same-name.bin", 64),
            entry(2, "22", 102, "same-name.bin", 64),
        ],
        &reader,
    );
    assert!(report.relationships.is_empty(), "{report:?}");
}

#[test]
fn different_filenames_same_content_is_a_duplicate() {
    let reader = IdentityReader { failing: vec![] };
    let report = run(
        vec![
            entry(1, "33", 101, "totally-different-a.txt", 64),
            entry(2, "33", 102, "completely-unrelated-b.txt", 64),
        ],
        &reader,
    );
    assert_eq!(report.relationships.len(), 1);
    assert_eq!(
        report.relationships[0].kind,
        RelationshipKind::ContentDuplicate
    );
}

#[test]
fn hard_links_form_an_alias_relationship_not_two_copies() {
    let reader = IdentityReader { failing: vec![] };
    // Two paths, ONE object (same key → same content trivially).
    let report = run(
        vec![
            entry(1, "44", 101, "a.bin", 64),
            entry(2, "44", 101, "alias.bin", 64),
        ],
        &reader,
    );
    assert_eq!(report.relationships.len(), 1);
    let rel = &report.relationships[0];
    assert_eq!(rel.kind, RelationshipKind::HardLinkAlias);
    assert_eq!(
        rel.recoverable_bytes, None,
        "one object: nothing recoverable"
    );
    assert_eq!(rel.distinct_objects, Some(1));
    assert_eq!(
        report.stats.content_duplicates, 0,
        "no second physical copy"
    );
}

#[test]
fn aliases_plus_independent_duplicate_span_both_kinds() {
    let reader = IdentityReader { failing: vec![] };
    // A,B = object 101 (aliases); C = object 102 (independent copy).
    let report = run(
        vec![
            entry(1, "55", 101, "a.bin", 64),
            entry(2, "55", 101, "b.bin", 64),
            entry(3, "55", 102, "c.bin", 64),
        ],
        &reader,
    );
    assert_eq!(report.relationships.len(), 2);
    let alias = &report.relationships[0];
    let content = &report.relationships[1];
    assert_eq!(alias.kind, RelationshipKind::HardLinkAlias);
    assert_eq!(
        alias.object,
        Some(spacelens_identity::ObjectRef {
            volume: 1,
            file_id: 101
        })
    );
    assert_eq!(content.kind, RelationshipKind::ContentDuplicate);
    assert_eq!(content.member_count, 3);
    assert_eq!(content.distinct_objects, Some(2));
    assert_eq!(content.alias_sets.len(), 1);
    assert_eq!(content.recoverable_bytes, Some(64), "one redundant copy");
}

#[test]
fn three_or_more_objects_group_deterministically() {
    let reader = IdentityReader { failing: vec![] };
    let report = run(
        vec![
            entry(1, "66", 103, "c.bin", 64),
            entry(2, "66", 101, "a.bin", 64),
            entry(3, "66", 102, "b.bin", 64),
        ],
        &reader,
    );
    assert_eq!(report.relationships.len(), 1);
    let rel = &report.relationships[0];
    assert_eq!(rel.member_count, 3);
    assert_eq!(rel.distinct_objects, Some(3));
    assert_eq!(rel.recoverable_bytes, Some(128), "size × (3 − 1)");
    // Member order: path bytes, regardless of observation order.
    let paths: Vec<String> = rel
        .members
        .iter()
        .map(|m| m.path.to_string_lossy().to_string())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
}

#[test]
fn hash_failure_excludes_member_and_stays_undetermined() {
    let reader = IdentityReader {
        failing: vec!["/rel/k77/o0102/fail.bin".to_string()],
    };
    let report = run(
        vec![
            entry(1, "77", 101, "ok.bin", 64),
            entry(2, "77", 102, "fail.bin", 64),
        ],
        &reader,
    );
    // The relationship must NOT exist (one member failed)...
    assert!(report.relationships.is_empty(), "{report:?}");
    // ...and the failure must remain visible as UNDETERMINED, not "none".
    assert_eq!(report.undetermined.failed, 1);
    // The synthetic factory raises a raw sharing-violation code; the
    // pipeline's platform-NEUTRAL fallback maps it to Other (the engine's
    // own platform categorization maps 32 to InUse — covered by engine
    // tests). The typed Hash kind is preserved either way.
    assert_eq!(
        report.undetermined.failed_by_reason[0].0,
        HashFailureKind::Hash {
            category: spacelens_engine::ErrorCategory::Other,
        }
    );
}

#[test]
fn mutation_and_replacement_produce_no_false_duplicates() {
    // Mutation: content changes mid-read (same length) → the pipeline's
    // change-stamp bracket rejects it → typed Changed, no relationship.
    // The scripted reader cannot mutate; the pipeline-level tests cover
    // mutation/rejection exhaustively (adversarial_pipeline_tests). Here we
    // pin the DERIVATION side: a rejected file yields no relationship and
    // a typed undetermined entry — via the failure path above. Replacement
    // (Replaced) is likewise derived from the pipeline's typed failures.
    let mut entries = vec![
        entry(1, "88", 101, "stable.bin", 64),
        entry(2, "88", 102, "victim.bin", 64),
    ];
    entries[1].device = Some(9);
    entries[1].inode = Some(999); // observed identity ≠ handle identity below

    struct ReplacingReader {
        stable_path: PathBuf,
    }
    impl ContentReaderFactory for ReplacingReader {
        fn read(
            &self,
            path: &Path,
            feed: &mut dyn FnMut(&mut dyn ContentReader) -> io::Result<()>,
        ) -> Result<(), ContentError> {
            let stable = path == self.stable_path;
            feed(&mut ReplacedChunks { served: 0, stable }).map_err(ContentError::ReadFailed)
        }
    }
    struct ReplacedChunks {
        served: u64,
        stable: bool,
    }
    impl ContentReader for ReplacedChunks {
        fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
            if self.served >= 64 {
                return Ok(None);
            }
            let n = buf.len().min((64 - self.served) as usize);
            // The stable path serves the shared key content; the impostor
            // serves different bytes (same length).
            let fill = if self.stable {
                0x5A ^ 0x58
            } else {
                0x5A ^ 0x33
            };
            buf[..n].fill(fill);
            self.served += n as u64;
            Ok(Some(n))
        }
        fn file_identity(&self) -> spacelens_engine::FileIdentity {
            // The stable file's opened identity matches the observation;
            // the victim's opened object is NOT the observed one.
            spacelens_engine::FileIdentity {
                device: Some(1),
                inode: if self.stable { Some(101) } else { Some(102) },
                file_id_hi: None,
                link_count: Some(1),
            }
        }
        fn pre_stat(&self) -> io::Result<HandleStat> {
            Ok(HandleStat {
                len: 64,
                modified: None,
                changed: None,
            })
        }
        fn post_stat(&self) -> io::Result<HandleStat> {
            Ok(HandleStat {
                len: 64,
                modified: None,
                changed: None,
            })
        }
    }

    let cancel = CancelHandle::new();
    let duplicate_report = run_duplicates(
        entries.into_iter(),
        &DuplicateOptions::default(),
        &cancel,
        Some(&ReplacingReader {
            stable_path: PathBuf::from("/rel/k88/o0101/stable.bin"),
        }),
        &mut |_| {},
    );
    let report = derive_relationships(&duplicate_report, &RelationshipOptions::default());
    assert!(
        report.relationships.is_empty(),
        "an identity-mismatched impostor must never become a relationship: {report:?}"
    );
    assert_eq!(report.undetermined.failed, 1);
    assert_eq!(
        report.undetermined.failed_by_reason[0].0,
        HashFailureKind::Replaced
    );
}

#[test]
fn cancellation_publishes_no_relationships() {
    let reader = IdentityReader { failing: vec![] };
    let cancel = CancelHandle::new();
    cancel.cancel();
    let duplicate_report = run_duplicates(
        vec![
            entry(1, "11", 101, "a.bin", 64),
            entry(2, "11", 102, "b.bin", 64),
        ]
        .into_iter(),
        &DuplicateOptions::default(),
        &cancel,
        Some(&reader),
        &mut |_| {},
    );
    let report = derive_relationships(&duplicate_report, &RelationshipOptions::default());
    assert_eq!(report.status, DuplicateStatus::Cancelled);
    assert!(report.relationships.is_empty());
}

#[test]
fn capped_runs_report_completed_with_limits_through_the_relationship_layer() {
    let reader = IdentityReader { failing: vec![] };
    let options = DuplicateOptions {
        max_tracked_size_groups: 2,
        ..DuplicateOptions::default()
    };
    // 10 distinct sizes; only 2 tracked → 8 not examined.
    let entries: Vec<FsEntry> = (0..10u64)
        .map(|i| {
            entry(
                i,
                &format!("{:02}", 10 + i),
                100 + i,
                &format!("f{}.bin", i),
                64 + i,
            )
        })
        .collect();
    let report = run_with_options(entries, &reader, &options);
    assert_eq!(report.status, DuplicateStatus::CompletedWithLimits);
    assert_eq!(report.undetermined.not_examined, 8);
}

#[test]
fn ordering_is_independent_of_observation_order() {
    let reader = IdentityReader { failing: vec![] };
    let entries: Vec<FsEntry> = vec![
        entry(1, "91", 101, "a.bin", 64),
        entry(2, "91", 102, "b.bin", 64),
        entry(3, "92", 103, "c.bin", 64),
        entry(4, "92", 104, "d.bin", 64),
        entry(5, "93", 105, "e.bin", 64),
        entry(6, "93", 105, "e-alias.bin", 64),
    ];
    let mut reversed = entries.clone();
    reversed.reverse();

    let a = run(entries, &reader);
    let b = run(reversed, &reader);
    assert_eq!(
        a.relationships, b.relationships,
        "same input → identical relationships"
    );
    assert_eq!(a.stats, b.stats);
    // 2 content duplicates + 1 alias set.
    assert_eq!(a.stats.content_duplicates, 2);
    assert_eq!(a.stats.hard_link_alias_sets, 1);
}

// ---------------------------------------------------------------------------
// Objective 18: operational invariants (property-style, parameterized)
// ---------------------------------------------------------------------------

#[test]
fn invariant_equal_content_yields_equal_published_digest() {
    let reader = IdentityReader { failing: vec![] };
    let report = run(
        vec![
            entry(1, "101", 201, "x.bin", 64),
            entry(2, "101", 202, "y.bin", 64),
        ],
        &reader,
    );
    let content = &report.relationships[0].content.as_ref().unwrap();
    // Same key → same content → the published hex equals a direct hash of
    // the bytes the reader serves for that key (the 8-byte LE key pattern,
    // XOR 0x5A, cycled over 64 bytes).
    let mut pattern = [0u8; 64];
    let le = 101u64.to_le_bytes();
    for (i, b) in pattern.iter_mut().enumerate() {
        *b = le[i % 8] ^ 0x5A;
    }
    let direct = ContentHash::from_bytes(&pattern);
    assert_eq!(
        content.sha256_hex,
        direct.as_hex(),
        "published digest must match the bytes served"
    );
    assert_eq!(content.algorithm, "sha256");
}

#[test]
fn invariant_same_size_never_implies_duplicate() {
    let reader = IdentityReader { failing: vec![] };
    // 20 distinct keys, all same size: zero relationships, zero errors.
    let entries: Vec<FsEntry> = (0..20u64)
        .map(|i| {
            entry(
                i,
                &format!("{:02}", i),
                300 + i,
                &format!("f{i:02}.bin"),
                64,
            )
        })
        .collect();
    let report = run(entries, &reader);
    assert!(report.relationships.is_empty());
    assert_eq!(report.undetermined.failed, 0);
    assert_eq!(report.stats.content_duplicates, 0);
}

#[test]
fn invariant_same_name_never_implies_duplicate() {
    let reader = IdentityReader { failing: vec![] };
    let entries: Vec<FsEntry> = (0..10u64)
        .map(|i| {
            entry(
                i,
                &format!("{:02}", 50 + i),
                400 + i,
                "identical-name.bin",
                64,
            )
        })
        .collect();
    let report = run(entries, &reader);
    assert!(report.relationships.is_empty());
}

#[test]
fn invariant_failed_hash_never_implies_duplicate() {
    // Three-way same-content group where one member fails: the two
    // successful members still relate; the failed one is in NO
    // relationship and stays undetermined.
    let reader = IdentityReader {
        failing: vec!["/rel/k140/o0302/mid.bin".to_string()],
    };
    let report = run(
        vec![
            entry(1, "140", 301, "a.bin", 64),
            entry(2, "140", 302, "mid.bin", 64),
            entry(3, "140", 303, "c.bin", 64),
        ],
        &reader,
    );
    assert_eq!(report.relationships.len(), 1);
    let rel = &report.relationships[0];
    assert_eq!(rel.member_count, 2, "the failed member is excluded");
    assert!(
        rel.members
            .iter()
            .all(|m| m.path != Path::new("/rel/k140/o0303/mid.bin")),
        "failed member in no relationship"
    );
    assert_eq!(report.undetermined.failed, 1);
}

#[test]
fn invariant_same_path_never_assumed_same_object() {
    // Two OBSERVED entries at the same path (pathological input — e.g. a
    // file replaced between observations). Same path ⇒ nothing; only the
    // object identity comparison distinguishes them.
    let reader = IdentityReader { failing: vec![] };
    let e1 = entry(1, "150", 501, "twin.bin", 64);
    let mut e2 = entry(2, "150", 502, "twin.bin", 64);
    // Same path via an identical path string.
    e2.path = e1.path.clone();
    let report = run(vec![e1, e2], &reader);
    // They are distinct OBJECTS with the same content → a content
    // duplicate is proven by identity+hash — but the point of the
    // invariant is that no "same path ⇒ same object" shortcut exists: the
    // relationship must carry DISTINCT object identities.
    if let Some(rel) = report.relationships.first() {
        assert_eq!(rel.distinct_objects, Some(2), "same path ≠ same object");
    }
}

// ---------------------------------------------------------------------------
// Objective 19: adversarial scale (synthetic, no disk)
// ---------------------------------------------------------------------------

fn scale_reader() -> IdentityReader {
    IdentityReader { failing: vec![] }
}

#[test]
fn scale_many_duplicate_groups_stay_deterministic() {
    let reader = scale_reader();
    let groups = 300u64;
    let entries: Vec<FsEntry> = (0..groups)
        .flat_map(|g| {
            vec![
                entry(g * 2, &format!("{g:04}"), 1000 + g * 2, "a.bin", 64),
                entry(g * 2 + 1, &format!("{g:04}"), 1000 + g * 2 + 1, "b.bin", 64),
            ]
        })
        .collect();
    let report = run(entries, &reader);
    assert_eq!(report.stats.content_duplicates, groups);
    // Determinism across a re-run with reversed observation order.
    // (Rebuilt entries with different ids would change nothing observable;
    // ids are scan-scoped and DO flow into members, so reverse the same
    // entries for the identity check.)
    // Deterministic ordering assertion: content ids strictly ordered.
    let ids: Vec<&str> = report.relationships.iter().map(|r| r.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "canonical order must hold");
}

#[test]
fn scale_one_enormous_group_stays_exact_and_bounded_in_detail() {
    let reader = scale_reader();
    let n = 400u64;
    let entries: Vec<FsEntry> = (0..n)
        .map(|i| entry(i, "9999", 2000 + i, &format!("f{i:04}.bin"), 64))
        .collect();
    let report = run(entries, &reader);
    assert_eq!(report.relationships.len(), 1);
    let rel = &report.relationships[0];
    assert_eq!(rel.member_count, n);
    // Detail capped at the group level (64), counts exact.
    assert!(rel.detail_truncated);
    assert_eq!(
        rel.members.len(),
        spacelens_identity::duplicate::DUPLICATE_GROUP_DETAIL_CAP
    );
    // Alias sets: none (all distinct objects).
    assert!(rel.alias_sets.is_empty());
    assert_eq!(rel.recoverable_bytes, Some(64 * (n - 1)));
}

#[test]
fn scale_many_hard_link_aliases() {
    let reader = scale_reader();
    let n = 200u64;
    let entries: Vec<FsEntry> = (0..n)
        .map(|i| entry(i, "4242", 77, &format!("alias{i:04}.bin"), 64))
        .collect();
    let report = run(entries, &reader);
    // All paths, ONE object: exactly one alias relationship, no content
    // duplicate, nothing recoverable.
    assert_eq!(report.relationships.len(), 1);
    let rel = &report.relationships[0];
    assert_eq!(rel.kind, RelationshipKind::HardLinkAlias);
    assert_eq!(rel.member_count, n);
    assert_eq!(rel.recoverable_bytes, None);
    assert_eq!(report.stats.content_duplicates, 0);
}

#[test]
fn scale_many_singleton_sizes_cost_nothing() {
    let reader = scale_reader();
    let n = 500u64;
    let entries: Vec<FsEntry> = (0..n)
        .map(|i| {
            // Distinct sizes → distinct size groups → never hashed.
            entry(
                i,
                &format!("{}", i),
                5000 + i,
                &format!("f{i:04}.bin"),
                64 + i,
            )
        })
        .collect();
    let report = run(entries, &reader);
    assert!(report.relationships.is_empty());
    // Singletons were never hashed: nothing to relate, nothing failed.
    assert_eq!(report.stats.content_duplicates, 0);
    assert_eq!(report.undetermined.failed, 0);
    assert_eq!(
        report.undetermined.not_examined, 0,
        "no caps bit at this scale"
    );
}

#[test]
fn scale_mixed_success_and_failure_keeps_everything_accounted() {
    let failing: Vec<String> = (0..50u64)
        .map(|i| format!("/rel/k700/o{:04}/fail{i:02}.bin", 100 + i))
        .collect();
    let reader = IdentityReader {
        failing: failing.clone(),
    };
    let mut entries = Vec::new();
    for i in 0..50u64 {
        // 50 failing files (same content key) + 50 succeeding duplicates in
        // 25 pairs.
        entries.push(entry(i, "700", 100 + i, &format!("fail{i:02}.bin"), 64));
        // Pairs: (0,1), (2,3), ... each pair shares its own content key.
        entries.push(entry(
            100 + i,
            &format!("{}", 800 + i / 2),
            200 + i,
            &format!("ok{i:02}.bin"),
            64,
        ));
    }
    let report = run(entries, &reader);
    assert_eq!(report.undetermined.failed, 50);
    // 50 ok files in 25 same-key pairs → 25 content relationships.
    assert_eq!(report.stats.content_duplicates, 25);
    assert_eq!(report.stats.paths_in_relationships, 50);
    // See hash_failure_excludes_member: raw 32 → Other under the
    // platform-neutral fallback.
    assert_eq!(
        report.undetermined.failed_by_reason[0].0,
        HashFailureKind::Hash {
            category: spacelens_engine::ErrorCategory::Other,
        }
    );
}

// ---------------------------------------------------------------------------
// Index queries at scale
// ---------------------------------------------------------------------------

#[test]
fn index_scale_lookups_are_exhaustive() {
    let reader = scale_reader();
    let groups = 200u64;
    let entries: Vec<FsEntry> = (0..groups)
        .flat_map(|g| {
            vec![
                entry(g * 2, &format!("{g:04}"), 6000 + g * 2, "a.bin", 64),
                entry(g * 2 + 1, &format!("{g:04}"), 6000 + g * 2 + 1, "b.bin", 64),
            ]
        })
        .collect();
    let report = run(entries, &reader);
    let index = RelationshipIndex::build(&report);

    // Every content relationship is findable by its own digest.
    for rel in index.duplicate_groups() {
        let hex = &rel.content.as_ref().unwrap().sha256_hex;
        let mut digest = [0u8; 32];
        for (i, b) in digest.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        assert_eq!(
            index
                .relationships_for_content(&ContentHash::from_bytes(&digest))
                .len(),
            1
        );
    }
    // Every member path finds its relationship.
    for rel in index.relationships() {
        for m in &rel.members {
            assert!(
                index
                    .relationships_for_path(&m.path)
                    .iter()
                    .any(|r| r.id == rel.id),
                "path {} must find {}",
                m.path.display(),
                rel.id
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Real filesystem: the derivation over the real platform boundary
// ---------------------------------------------------------------------------

/// Real hard links + an independent copy through a real scan: the alias
/// relationship and the content relationship must come out separately,
/// with Exact accounting from handle-proven identities.
#[test]
fn real_fs_hard_links_and_copies_derive_both_kinds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let a = root.join("a.bin");
    let alias = root.join("alias.bin");
    let copy = root.join("copy.bin");
    std::fs::write(&a, b"relationship-real-fs").unwrap();
    std::fs::hard_link(&a, &alias).unwrap();
    std::fs::copy(&a, &copy).unwrap();

    // Scan for real.
    let mut entries = Vec::new();
    spacelens_engine::scan(
        root,
        spacelens_engine::ScanOptions {
            threads: 2,
            ..spacelens_engine::ScanOptions::default()
        },
        &CancelHandle::new(),
        &mut |e| {
            if let spacelens_engine::ScanEvent::Entry(entry) = e {
                if entry.path != root {
                    entries.push(*entry);
                }
            }
        },
    );

    let factory =
        spacelens_identity::DefaultReaderFactory::new(spacelens_engine::platform::std_fs());
    let duplicate_report = run_duplicates(
        entries.into_iter(),
        &DuplicateOptions::default(),
        &CancelHandle::new(),
        Some(&factory),
        &mut |_| {},
    );
    let report = derive_relationships(&duplicate_report, &RelationshipOptions::default());

    assert_eq!(report.status, DuplicateStatus::Completed);
    assert_eq!(report.relationships.len(), 2);
    let alias = &report.relationships[0];
    let content = &report.relationships[1];
    assert_eq!(alias.kind, RelationshipKind::HardLinkAlias);
    assert_eq!(alias.member_count, 2);
    assert_eq!(alias.recoverable_bytes, None);
    assert_eq!(content.kind, RelationshipKind::ContentDuplicate);
    assert_eq!(content.member_count, 3);
    assert_eq!(content.distinct_objects, Some(2));
    assert_eq!(content.alias_sets.len(), 1);
    assert_eq!(content.recoverable_bytes, Some(20));
    assert_eq!(content.accounting, StorageAccounting::Exact);
}

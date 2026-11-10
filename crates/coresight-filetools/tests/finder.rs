//! Finder contract tests.
//!
//! Synthetic snapshots only. These tests prove that the name criterion reads
//! the entry's OWN name (never a parent directory), that blank text is not a
//! criterion, that unknown size/age/category can never satisfy a bound in
//! either unknown-evidence mode, that every hit explains itself, that the
//! order is identical across input permutations, that truncation and totals
//! are exact and saturating, and that the JSON surface stays camelCase.

use coresight_classifier::Category;
use coresight_filetools::{
    search, search_at, SearchHit, SearchKind, SearchOptions, SearchQuery, SearchReport,
};
use coresight_history::{ClassificationRef, ObservedEntry, ObservedKind, RunId, Snapshot};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY: u64 = 86_400;

fn at(days: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(days * DAY)
}

fn category_ref(code: &str) -> Option<ClassificationRef> {
    Some(ClassificationRef {
        category: code.to_string(),
        subcategory: None,
    })
}

fn entry_with_path(
    path: PathBuf,
    kind: ObservedKind,
    size: Option<u64>,
    modified: Option<SystemTime>,
    category: Option<&str>,
) -> ObservedEntry {
    ObservedEntry {
        path,
        kind,
        size,
        object: None,
        modified,
        classification: category.and_then(category_ref),
        content_sha256: None,
        observation_error: None,
    }
}

fn entry(
    path: &str,
    kind: ObservedKind,
    size: Option<u64>,
    modified: Option<SystemTime>,
    category: Option<&str>,
) -> ObservedEntry {
    entry_with_path(PathBuf::from(path), kind, size, modified, category)
}

fn file(
    path: &str,
    size: Option<u64>,
    modified: Option<SystemTime>,
    category: Option<&str>,
) -> ObservedEntry {
    entry(path, ObservedKind::File, size, modified, category)
}

fn dir(path: &str) -> ObservedEntry {
    entry(path, ObservedKind::Dir, None, None, None)
}

fn snapshot(entries: Vec<ObservedEntry>) -> Snapshot {
    Snapshot {
        run_id: RunId("finder-run".to_string()),
        entries,
    }
}

fn query_with_text(text: &str) -> SearchQuery {
    SearchQuery {
        text: Some(text.to_string()),
        ..SearchQuery::default()
    }
}

fn options(max_results: usize, strict_unknowns: bool) -> SearchOptions {
    SearchOptions {
        max_results,
        strict_unknowns,
    }
}

fn paths(report: &SearchReport) -> Vec<PathBuf> {
    report.hits.iter().map(|hit| hit.path.clone()).collect()
}

fn reasons(hit: &SearchHit) -> Vec<String> {
    hit.matched_because.clone()
}

// ---------------------------------------------------------------------------
// Name criterion
// ---------------------------------------------------------------------------

#[test]
fn text_matches_the_entry_name_case_insensitively() {
    let report = search_at(
        &snapshot(vec![
            file("/d/Quarterly-REPORT.pdf", Some(10), None, None),
            file("/d/reporting", Some(10), None, None),
            file("/d/notes.txt", Some(10), None, None),
        ]),
        &query_with_text("RePoRt"),
        &options(10, true),
        at(100),
    );

    assert_eq!(
        paths(&report),
        vec![
            PathBuf::from("/d/Quarterly-REPORT.pdf"),
            PathBuf::from("/d/reporting")
        ]
    );
    assert_eq!(
        reasons(&report.hits[0]),
        vec!["name contains \"RePoRt\"".to_string()],
        "the explanation quotes the query as typed, not the folded form"
    );
}

#[test]
fn text_never_matches_a_parent_directory_name() {
    // The cleanup accident this guards against: matching a parent folder must
    // not pull its whole subtree into the result.
    let report = search_at(
        &snapshot(vec![
            file("/d/report-archive/notes.txt", Some(10), None, None),
            dir("/d/report-archive"),
            file("/d/report.txt", Some(10), None, None),
        ]),
        &query_with_text("report"),
        &options(10, true),
        at(100),
    );

    assert_eq!(
        paths(&report),
        vec![
            PathBuf::from("/d/report.txt"),
            PathBuf::from("/d/report-archive")
        ],
        "only entries whose OWN name contains the text may match"
    );
}

#[test]
fn blank_text_is_not_a_criterion() {
    let entries = vec![
        file("/d/a.txt", Some(10), None, None),
        file("/d/b.bin", None, None, None),
    ];

    let empty = search_at(
        &snapshot(entries.clone()),
        &query_with_text(""),
        &options(10, true),
        at(100),
    );
    let spaces = search_at(
        &snapshot(entries.clone()),
        &query_with_text("   "),
        &options(10, true),
        at(100),
    );
    let unconstrained = search_at(
        &snapshot(entries),
        &SearchQuery::default(),
        &options(10, true),
        at(100),
    );

    assert_eq!(empty, unconstrained);
    assert_eq!(spaces, unconstrained);
    assert_eq!(empty.matched, 2);
    assert!(empty.hits.iter().all(|hit| hit.matched_because.is_empty()));
}

#[test]
fn text_is_trimmed_before_matching() {
    let report = search_at(
        &snapshot(vec![file("/d/report.txt", Some(10), None, None)]),
        &query_with_text("  report  "),
        &options(10, true),
        at(100),
    );

    assert_eq!(report.hits.len(), 1);
    assert_eq!(reasons(&report.hits[0]), vec!["name contains \"report\""]);
}

// ---------------------------------------------------------------------------
// Unconstrained query
// ---------------------------------------------------------------------------

#[test]
fn an_unconstrained_query_matches_everything_with_empty_reasons() {
    let report = search_at(
        &snapshot(vec![
            file("/d/a.txt", Some(10), Some(at(1)), Some("CACHE")),
            dir("/d/b"),
            entry("/d/c", ObservedKind::Link, None, None, None),
        ]),
        &SearchQuery::default(),
        &options(10, true),
        at(100),
    );

    assert_eq!(report.matched, 3);
    assert_eq!(report.truncated, 0);
    assert_eq!(report.entries_examined, 3);
    assert_eq!(report.excluded_unknown, 0);
    assert!(
        report.hits.iter().all(|hit| hit.matched_because.is_empty()),
        "an unconstrained listing has no criterion to explain"
    );
    assert!(report.hits.iter().all(|hit| hit.fully_evaluated));
}

#[test]
fn an_unconstrained_query_is_still_bounded_and_counted() {
    let entries: Vec<ObservedEntry> = (0..5)
        .map(|index| file(&format!("/d/file{index}.bin"), Some(1), None, None))
        .collect();

    let report = search_at(
        &snapshot(entries),
        &SearchQuery::default(),
        &options(2, true),
        at(100),
    );

    assert_eq!(report.hits.len(), 2);
    assert_eq!(report.matched, 5);
    assert_eq!(report.truncated, 3);
}

// ---------------------------------------------------------------------------
// Unknown evidence: age
// ---------------------------------------------------------------------------

#[test]
fn unknown_age_is_excluded_and_counted_in_audit_mode() {
    let report = search_at(
        &snapshot(vec![
            file("/d/old.bin", Some(10), Some(at(60)), None),
            file("/d/young.bin", Some(10), Some(at(95)), None),
            file("/d/unknown-age.bin", Some(10), None, None),
        ]),
        &SearchQuery {
            min_age_days: Some(30),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );

    assert_eq!(paths(&report), vec![PathBuf::from("/d/old.bin")]);
    assert_eq!(report.excluded_unknown, 1);
    assert_eq!(reasons(&report.hits[0]), vec!["40 days old".to_string()]);
}

#[test]
fn unknown_age_is_never_a_hit_in_filter_mode_either() {
    let report = search_at(
        &snapshot(vec![
            file("/d/old.bin", Some(10), Some(at(60)), None),
            file("/d/unknown-age.bin", Some(10), None, None),
        ]),
        &SearchQuery {
            min_age_days: Some(30),
            ..SearchQuery::default()
        },
        &options(10, false),
        at(100),
    );

    assert_eq!(
        paths(&report),
        vec![PathBuf::from("/d/old.bin")],
        "filter mode must not publish an entry it never measured"
    );
    assert_eq!(
        report.excluded_unknown, 1,
        "with no evaluable criterion at all the entry is still dropped and counted"
    );
}

#[test]
fn filter_mode_publishes_a_partial_hit_without_claiming_the_skipped_criterion() {
    let entries = vec![file("/d/report.txt", Some(10), None, None)];

    let lenient = search_at(
        &snapshot(entries.clone()),
        &SearchQuery {
            min_age_days: Some(30),
            ..query_with_text("report")
        },
        &options(10, false),
        at(100),
    );
    assert_eq!(lenient.hits.len(), 1);
    assert!(
        !lenient.hits[0].fully_evaluated,
        "a skipped criterion must be visible on the hit"
    );
    assert_eq!(
        reasons(&lenient.hits[0]),
        vec!["name contains \"report\"".to_string()],
        "the unevaluated age criterion is never claimed as satisfied"
    );
    assert_eq!(lenient.excluded_unknown, 0);

    let strict = search_at(
        &snapshot(entries),
        &SearchQuery {
            min_age_days: Some(30),
            ..query_with_text("report")
        },
        &options(10, true),
        at(100),
    );
    assert!(strict.hits.is_empty(), "audit mode refuses the partial hit");
    assert_eq!(strict.excluded_unknown, 1);
}

#[test]
fn a_future_timestamp_is_not_evidence_of_age() {
    let report = search_at(
        &snapshot(vec![file("/d/future.bin", Some(10), Some(at(200)), None)]),
        &SearchQuery {
            min_age_days: Some(30),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );

    assert!(report.hits.is_empty());
    assert_eq!(report.excluded_unknown, 1);
}

#[test]
fn the_age_boundary_is_inclusive() {
    let report = search_at(
        &snapshot(vec![
            file("/d/at-30-days.bin", Some(10), Some(at(70)), None),
            file(
                "/d/one-second-younger.bin",
                Some(10),
                Some(at(70) + Duration::from_secs(1)),
                None,
            ),
        ]),
        &SearchQuery {
            min_age_days: Some(30),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );

    assert_eq!(paths(&report), vec![PathBuf::from("/d/at-30-days.bin")]);
    assert_eq!(reasons(&report.hits[0]), vec!["30 days old".to_string()]);
}

#[test]
fn search_uses_the_wall_clock_for_the_age_criterion() {
    let now = SystemTime::now();
    let old = now - Duration::from_secs(40 * DAY);
    let recent = now - Duration::from_secs(2 * DAY);

    let report = search(
        &snapshot(vec![
            file("/d/old.bin", Some(10), Some(old), None),
            file("/d/recent.bin", Some(10), Some(recent), None),
        ]),
        &SearchQuery {
            min_age_days: Some(30),
            ..SearchQuery::default()
        },
        &options(10, true),
    );

    assert_eq!(paths(&report), vec![PathBuf::from("/d/old.bin")]);
}

// ---------------------------------------------------------------------------
// Unknown evidence: size
// ---------------------------------------------------------------------------

#[test]
fn unknown_size_can_never_satisfy_a_size_bound() {
    let entries = vec![
        file("/d/measured.bin", Some(5_000), None, None),
        file("/d/unknown-size.bin", None, None, None),
    ];

    let lower = search_at(
        &snapshot(entries.clone()),
        &SearchQuery {
            min_size_bytes: Some(1_000),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );
    assert_eq!(paths(&lower), vec![PathBuf::from("/d/measured.bin")]);
    assert_eq!(lower.excluded_unknown, 1);

    let upper = search_at(
        &snapshot(entries),
        &SearchQuery {
            max_size_bytes: Some(10_000),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );
    assert_eq!(paths(&upper), vec![PathBuf::from("/d/measured.bin")]);
    assert_eq!(upper.excluded_unknown, 1);
}

#[test]
fn size_bounds_are_inclusive() {
    let report = search_at(
        &snapshot(vec![
            file("/d/exact.bin", Some(1_000), None, None),
            file("/d/too-small.bin", Some(999), None, None),
            file("/d/too-big.bin", Some(2_001), None, None),
        ]),
        &SearchQuery {
            min_size_bytes: Some(1_000),
            max_size_bytes: Some(2_000),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );

    assert_eq!(paths(&report), vec![PathBuf::from("/d/exact.bin")]);
}

#[test]
fn contradictory_size_bounds_match_nothing_without_panicking() {
    let report = search_at(
        &snapshot(vec![file("/d/anything.bin", Some(10), None, None)]),
        &SearchQuery {
            min_size_bytes: Some(10),
            max_size_bytes: Some(5),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );

    assert!(report.hits.is_empty());
    assert_eq!(report.excluded_unknown, 0);
}

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------

#[test]
fn unknown_category_codes_never_satisfy_a_category_filter() {
    let report = search_at(
        &snapshot(vec![
            file("/d/cache.bin", Some(10), None, Some("CACHE")),
            file("/d/unclassified.bin", Some(10), None, None),
            file("/d/future-code.bin", Some(10), None, Some("FUTURE_CODE")),
        ]),
        &SearchQuery {
            categories: vec![Category::Cache],
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );

    assert_eq!(paths(&report), vec![PathBuf::from("/d/cache.bin")]);
    assert_eq!(
        report.excluded_unknown, 2,
        "a missing or unknown code is unknown, never 'any category'"
    );
    assert_eq!(
        reasons(&report.hits[0]),
        vec!["category is Cache".to_string()]
    );
}

#[test]
fn every_stable_category_code_decodes_into_a_filterable_category() {
    let entries: Vec<ObservedEntry> = Category::ALL
        .iter()
        .map(|category| {
            file(
                &format!("/d/{}", category.code()),
                Some(10),
                None,
                Some(category.code()),
            )
        })
        .collect();

    for category in Category::ALL {
        let report = search_at(
            &snapshot(entries.clone()),
            &SearchQuery {
                categories: vec![category],
                ..SearchQuery::default()
            },
            &options(10, true),
            at(100),
        );

        assert_eq!(report.hits.len(), 1, "code {}", category.code());
        assert_eq!(report.hits[0].category, Some(category));
    }
}

// ---------------------------------------------------------------------------
// Kinds
// ---------------------------------------------------------------------------

#[test]
fn empty_kinds_means_any_and_a_kind_filter_selects() {
    let entries = vec![
        file("/d/a.bin", Some(10), None, None),
        dir("/d/b"),
        entry("/d/c", ObservedKind::Link, None, None, None),
        entry("/d/d", ObservedKind::Other, None, None, None),
    ];

    let any = search_at(
        &snapshot(entries.clone()),
        &SearchQuery::default(),
        &options(10, true),
        at(100),
    );
    assert_eq!(any.matched, 4);

    let filtered = search_at(
        &snapshot(entries),
        &SearchQuery {
            kinds: vec![SearchKind::Directory, SearchKind::Link],
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );
    assert_eq!(
        paths(&filtered),
        vec![PathBuf::from("/d/b"), PathBuf::from("/d/c")]
    );
    assert_eq!(filtered.hits[0].kind, SearchKind::Directory);
    assert_eq!(filtered.hits[1].kind, SearchKind::Link);
    assert_eq!(reasons(&filtered.hits[0]), vec!["kind is Directory"]);
}

// ---------------------------------------------------------------------------
// Explanations
// ---------------------------------------------------------------------------

#[test]
fn every_hit_explains_each_criterion_it_satisfied() {
    let report = search_at(
        &snapshot(vec![file(
            "/d/report-2026.txt",
            Some(2_048),
            Some(at(60)),
            Some("CACHE"),
        )]),
        &SearchQuery {
            text: Some("report".to_string()),
            categories: vec![Category::Cache],
            min_size_bytes: Some(100),
            max_size_bytes: Some(4_096),
            min_age_days: Some(30),
            kinds: vec![SearchKind::File],
        },
        &options(10, true),
        at(100),
    );

    assert_eq!(report.hits.len(), 1);
    let hit = &report.hits[0];
    assert!(hit.fully_evaluated);
    assert_eq!(
        reasons(hit),
        vec![
            "name contains \"report\"".to_string(),
            "category is Cache".to_string(),
            "size 2048 bytes is at least 100 bytes".to_string(),
            "size 2048 bytes is at most 4096 bytes".to_string(),
            "40 days old".to_string(),
            "kind is File".to_string(),
        ],
        "one sentence per applied criterion, in the documented order"
    );
}

#[test]
fn a_single_day_of_age_is_phrased_in_the_singular() {
    let report = search_at(
        &snapshot(vec![file("/d/one.bin", Some(1), Some(at(9)), None)]),
        &SearchQuery {
            min_age_days: Some(1),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(10),
    );

    assert_eq!(reasons(&report.hits[0]), vec!["1 day old".to_string()]);
}

// ---------------------------------------------------------------------------
// Determinism and ordering
// ---------------------------------------------------------------------------

#[test]
fn hits_sort_by_kind_then_known_size_desc_then_path() {
    let report = search_at(
        &snapshot(vec![
            entry("/d/z-dir", ObservedKind::Dir, None, None, None),
            file("/d/b.bin", Some(100), None, None),
            file("/d/a.bin", Some(100), None, None),
            file("/d/c.bin", None, None, None),
            file("/d/d.bin", Some(5_000), None, None),
            entry("/d/link", ObservedKind::Link, None, None, None),
            entry("/d/other", ObservedKind::Other, None, None, None),
        ]),
        &SearchQuery::default(),
        &options(10, true),
        at(100),
    );

    assert_eq!(
        paths(&report),
        vec![
            PathBuf::from("/d/d.bin"),
            PathBuf::from("/d/a.bin"),
            PathBuf::from("/d/b.bin"),
            PathBuf::from("/d/c.bin"),
            PathBuf::from("/d/z-dir"),
            PathBuf::from("/d/link"),
            PathBuf::from("/d/other"),
        ],
        "files first, then by size descending with unknown sizes last, then path"
    );
}

#[test]
fn the_report_is_identical_across_input_permutations() {
    let entries = vec![
        file("/d/alpha.txt", Some(100), Some(at(10)), Some("DOCUMENTS")),
        file("/d/beta.bin", Some(2_000), None, Some("CACHE")),
        dir("/d/gamma"),
        file("/d/delta.log", None, None, Some("LOGS")),
        entry("/d/epsilon", ObservedKind::Link, None, None, None),
        file(
            "/d/zeta.tmp",
            Some(100),
            Some(at(80)),
            Some("TEMPORARY_DATA"),
        ),
    ];
    let mut reversed = entries.clone();
    reversed.reverse();
    let mut rotated = entries.clone();
    rotated.rotate_left(3);

    let query = SearchQuery {
        min_size_bytes: Some(1),
        ..query_with_text("a")
    };
    let forward = search(&snapshot(entries), &query, &options(4, true));
    let backward = search(&snapshot(reversed), &query, &options(4, true));
    let rotated = search(&snapshot(rotated), &query, &options(4, true));

    assert_eq!(forward, backward);
    assert_eq!(forward, rotated);
}

#[test]
fn both_unknown_modes_agree_when_every_criterion_is_evaluable() {
    let entries = vec![
        file(
            "/d/report-2026.txt",
            Some(2_048),
            Some(at(10)),
            Some("CACHE"),
        ),
        file("/d/other.bin", Some(1), Some(at(90)), None),
    ];
    let query = SearchQuery {
        min_size_bytes: Some(1_024),
        min_age_days: Some(30),
        ..query_with_text("report")
    };

    let strict = search_at(
        &snapshot(entries.clone()),
        &query,
        &options(10, true),
        at(100),
    );
    let lenient = search_at(&snapshot(entries), &query, &options(10, false), at(100));

    assert_eq!(strict, lenient);
    assert_eq!(strict.matched, 1);
    assert!(strict.hits[0].fully_evaluated);
}

// ---------------------------------------------------------------------------
// Bounds, totals, accounting
// ---------------------------------------------------------------------------

#[test]
fn truncation_and_totals_are_exact() {
    let report = search_at(
        &snapshot(vec![
            file("/d/a.bin", Some(100), None, None),
            file("/d/b.bin", Some(200), None, None),
            file("/d/c.bin", Some(300), None, None),
            file("/d/d.bin", Some(400), None, None),
        ]),
        &SearchQuery::default(),
        &options(2, true),
        at(100),
    );

    assert_eq!(
        paths(&report),
        vec![PathBuf::from("/d/d.bin"), PathBuf::from("/d/c.bin")]
    );
    assert_eq!(report.matched, 4);
    assert_eq!(report.truncated, 2);
    assert_eq!(
        report.total_bytes, 700,
        "totals cover the RETURNED hits; matched/truncated say what is hidden"
    );
}

#[test]
fn zero_max_results_truncates_everything() {
    let report = search_at(
        &snapshot(vec![file("/d/big.bin", Some(1_000), None, None)]),
        &SearchQuery::default(),
        &options(0, true),
        at(100),
    );

    assert!(report.hits.is_empty());
    assert_eq!(report.matched, 1);
    assert_eq!(report.truncated, 1);
    assert_eq!(report.total_bytes, 0);
}

#[test]
fn total_bytes_saturates_instead_of_wrapping() {
    let report = search_at(
        &snapshot(vec![
            file("/d/one.bin", Some(u64::MAX), None, None),
            file("/d/two.bin", Some(u64::MAX), None, None),
        ]),
        &SearchQuery::default(),
        &options(10, true),
        at(100),
    );

    assert_eq!(report.matched, 2);
    assert_eq!(report.total_bytes, u64::MAX);
}

#[test]
fn entries_examined_counts_every_entry_including_excluded_ones() {
    let report = search_at(
        &snapshot(vec![
            file("/d/known.bin", Some(10), None, None),
            file("/d/unknown.bin", None, None, None),
            dir("/d/folder"),
        ]),
        &SearchQuery {
            min_size_bytes: Some(1),
            ..SearchQuery::default()
        },
        &options(10, true),
        at(100),
    );

    assert_eq!(report.entries_examined, 3);
    assert_eq!(report.matched, 1);
    assert_eq!(report.excluded_unknown, 2);
}

#[test]
fn a_non_utf8_name_cannot_satisfy_a_text_criterion() {
    let entries = vec![entry_with_path(
        non_utf8_name(),
        ObservedKind::File,
        Some(10),
        None,
        None,
    )];

    for strict_unknowns in [true, false] {
        let report = search_at(
            &snapshot(entries.clone()),
            &query_with_text("bad"),
            &options(10, strict_unknowns),
            at(100),
        );
        assert!(
            report.hits.is_empty(),
            "a name that cannot be folded is unknown, never a match"
        );
        assert_eq!(report.excluded_unknown, 1);
    }
}

#[cfg(windows)]
fn non_utf8_name() -> PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    // An unpaired surrogate is a legal Windows path unit but not valid UTF-8,
    // so `Path::to_str` is `None` for it.
    let mut units: Vec<u16> = "bad-".encode_utf16().collect();
    units.push(0xD800);
    units.extend("-name.txt".encode_utf16());
    PathBuf::from("/d").join(OsString::from_wide(&units))
}

#[cfg(unix)]
fn non_utf8_name() -> PathBuf {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    PathBuf::from("/d").join(OsStr::from_bytes(b"bad-\xFF-name.txt"))
}

// ---------------------------------------------------------------------------
// Defaults and JSON surface
// ---------------------------------------------------------------------------

#[test]
fn default_options_are_bounded_and_strict() {
    let options = SearchOptions::default();
    assert_eq!(options.max_results, 100);
    assert!(
        options.strict_unknowns,
        "audit mode is the safety-first default"
    );

    let query = SearchQuery::default();
    assert!(query.text.is_none());
    assert!(query.categories.is_empty());
    assert!(query.min_size_bytes.is_none());
    assert!(query.max_size_bytes.is_none());
    assert!(query.min_age_days.is_none());
    assert!(query.kinds.is_empty());
}

#[test]
fn query_options_hit_and_report_serialize_with_camel_case_fields() {
    let options_json = serde_json::to_value(SearchOptions::default()).expect("serialize options");
    assert_eq!(options_json["maxResults"], 100);
    assert_eq!(options_json["strictUnknowns"], true);
    assert!(options_json.get("max_results").is_none());
    assert!(options_json.get("strict_unknowns").is_none());

    let query_json = serde_json::to_value(SearchQuery {
        text: Some("report".to_string()),
        categories: vec![Category::Cache],
        min_size_bytes: Some(1),
        max_size_bytes: Some(2),
        min_age_days: Some(3),
        kinds: vec![SearchKind::Directory],
    })
    .expect("serialize query");
    assert_eq!(query_json["text"], "report");
    assert_eq!(query_json["categories"][0], "CACHE");
    assert_eq!(query_json["minSizeBytes"], 1);
    assert_eq!(query_json["maxSizeBytes"], 2);
    assert_eq!(query_json["minAgeDays"], 3);
    assert_eq!(query_json["kinds"][0], "DIRECTORY");
    assert!(query_json.get("min_size_bytes").is_none());

    let report = search_at(
        &snapshot(vec![file("/d/report.txt", Some(10), None, None)]),
        &query_with_text("report"),
        &options(10, true),
        at(100),
    );
    let report_json = serde_json::to_value(&report).expect("serialize report");
    assert_eq!(report_json["matched"], 1);
    assert_eq!(report_json["totalBytes"], 10);
    assert_eq!(report_json["truncated"], 0);
    assert_eq!(report_json["excludedUnknown"], 0);
    assert_eq!(report_json["entriesExamined"], 1);
    assert_eq!(
        report_json["hits"][0]["matchedBecause"][0],
        "name contains \"report\""
    );
    assert_eq!(report_json["hits"][0]["fullyEvaluated"], true);
    assert_eq!(report_json["hits"][0]["kind"], "FILE");
}

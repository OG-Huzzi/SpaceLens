//! Phase 6.4.1 Workstream A — collision-free application identity.
//!
//! The Phase 6.4 identity hashed `"{name}|{publisher}"`, which is
//! ambiguous at the component boundary. Phase 6.4.1 replaces it with a
//! length-prefixed encoding. These tests pin the properties the encoding
//! must have, the normalization semantics it must preserve, and the
//! compatibility rule that lets persisted legacy ids be re-keyed safely.

use coresight_apps::{
    discovery::merge_inventory, ApplicationId, ApplicationRecord, ApplicationSource,
    DiscoveryLimits, Inventory, PackageKind, ProviderOutcome, SourceCoverage,
};

fn record(name: &str, publisher: Option<&str>, source: ApplicationSource) -> ApplicationRecord {
    ApplicationRecord {
        id: ApplicationId::derive(name, publisher),
        name: name.to_string(),
        version: None,
        publisher: publisher.map(str::to_string),
        install_location: None,
        install_date: None,
        estimated_size_bytes: None,
        uninstall_string: None,
        quiet_uninstall_string: None,
        modify_path: None,
        install_source: None,
        source: source.clone(),
        kind: PackageKind::Installed,
        system_component: false,
        observed_in_views: Vec::new(),
        bundle_identifier: None,
        executable_path: None,
        provenance: vec![source],
    }
}

// ---------------------------------------------------------------------------
// injectivity of the encoding
// ---------------------------------------------------------------------------

#[test]
fn a_pipe_in_the_name_cannot_forge_a_component_boundary() {
    // The exact ambiguity the Phase 6.4 encoding had.
    let a = ApplicationId::derive("A|B", Some("C"));
    let b = ApplicationId::derive("A", Some("B|C"));
    assert_ne!(
        a, b,
        "a separator inside a component must not create a shared identity"
    );
}

#[test]
fn a_pipe_in_the_publisher_cannot_forge_a_component_boundary() {
    assert_ne!(
        ApplicationId::derive("App", Some("Pub|X")),
        ApplicationId::derive("App|Pub", Some("X")),
        "the publisher's separator must not be interchangeable with the name's"
    );
}

#[test]
fn component_boundaries_are_injective_across_many_pairs() {
    // Property-style: every distinct NORMALIZED pair yields a distinct id.
    // Covers empty components, embedded separators, unicode, and
    // whitespace-only names — the shapes a delimiter encoding conflates.
    let names = [
        "",
        "A",
        "A|B",
        "A|",
        "|A",
        "||",
        "A B",
        "A|B|C",
        "ünïcødé",
        "|",
    ];
    // `None` and `Some("")` are deliberately one value here: they
    // normalize to the same publisher, so they must yield one id.
    let publishers: [Option<&str>; 5] = [None, Some("P"), Some("|P"), Some("P|"), Some("|")];

    let mut ids = std::collections::BTreeSet::new();
    let mut pairs = std::collections::BTreeSet::new();
    for name in names {
        for publisher in publishers {
            let id = ApplicationId::derive(name, publisher);
            assert!(
                ids.insert(id.0.clone()),
                "duplicate id derived for {name:?}/{publisher:?}"
            );
            pairs.insert((
                name.trim().to_lowercase(),
                publisher.unwrap_or("").trim().to_lowercase(),
            ));
        }
    }
    assert_eq!(
        ids.len(),
        pairs.len(),
        "one id per distinct normalized pair"
    );
}

#[test]
fn an_empty_name_and_an_absent_publisher_stay_distinct_pairs() {
    // Distinct components that normalization does NOT collapse must stay
    // distinct.
    assert_ne!(
        ApplicationId::derive("", Some("p")),
        ApplicationId::derive("n", Some("p")),
        "an empty name is its own value, not a wildcard"
    );
    assert_ne!(
        ApplicationId::derive("n", None),
        ApplicationId::derive("n", Some("p")),
        "an absent publisher is its own value"
    );
}

// ---------------------------------------------------------------------------
// normalization semantics preserved
// ---------------------------------------------------------------------------

#[test]
fn case_and_surrounding_whitespace_do_not_change_identity() {
    assert_eq!(
        ApplicationId::derive("Foo", Some("Acme")),
        ApplicationId::derive("  foo  ", Some("  ACME  ")),
        "normalization trims and lowercases both components"
    );
    assert_eq!(
        ApplicationId::derive("MiXeD", None),
        ApplicationId::derive("mixed", None),
        "case folding applies to the name too"
    );
}

#[test]
fn interior_whitespace_is_significant() {
    // Trimming only the OUTER whitespace keeps "a b" and "ab" distinct.
    assert_ne!(
        ApplicationId::derive("a b", None),
        ApplicationId::derive("ab", None)
    );
}

#[test]
fn an_absent_and_an_empty_publisher_are_the_same_identity() {
    // Documented: `None` and `Some("")` normalize to the same pair, so
    // they are one logical application.
    assert_eq!(
        ApplicationId::derive("Foo", None),
        ApplicationId::derive("Foo", Some("")),
        "an absent publisher is the empty publisher"
    );
}

#[test]
fn unicode_is_hashed_as_utf8_bytes_without_unicode_normalization() {
    // No hidden NFC/NFD rule: NFC and NFD spellings of the same name are
    // distinct inputs and stay distinct identities. Documented rather
    // than silently unified.
    let nfc = "café";
    let nfd = "cafe\u{0301}";
    assert_ne!(nfc, nfd, "the two spellings are different strings");
    assert_ne!(
        ApplicationId::derive(nfc, None),
        ApplicationId::derive(nfd, None),
        "no Unicode normalization is applied; spellings stay distinct"
    );
    // ...and the SAME spelling is stable.
    assert_eq!(
        ApplicationId::derive(nfc, None),
        ApplicationId::derive(nfc, None)
    );
}

#[test]
fn identity_is_deterministic() {
    for _ in 0..8 {
        assert_eq!(
            ApplicationId::derive("App", Some("Pub")),
            ApplicationId::derive("App", Some("Pub")),
            "the id must be a pure function of the normalized pair"
        );
    }
}

// ---------------------------------------------------------------------------
// source is provenance, never identity
// ---------------------------------------------------------------------------

#[test]
fn changing_only_the_source_does_not_change_identity() {
    // The rule the encoding must not disturb: the same (name, publisher)
    // from different sources is ONE logical application.
    let derived = ApplicationId::derive("Cross", Some("Source"));
    for source in [
        ApplicationSource::RegistryUninstall,
        ApplicationSource::PackagedApp,
        ApplicationSource::BundleInfoPlist,
        ApplicationSource::DesktopEntry,
        ApplicationSource::FilesystemPresence,
    ] {
        let rec = record("Cross", Some("Source"), source.clone());
        assert_eq!(
            rec.id, derived,
            "{source:?} must not alter the identity of a pair"
        );
    }
}

#[test]
fn the_inventory_merge_key_and_the_derived_id_always_agree() {
    // `derive` and `merge_inventory` must key on exactly the same
    // normalized pair — otherwise the id and the merge key could disagree
    // and one application could appear as two (or two as one).
    let sources = [
        ApplicationSource::RegistryUninstall,
        ApplicationSource::PackagedApp,
        ApplicationSource::BundleInfoPlist,
    ];
    let limits = DiscoveryLimits::default();
    let outcomes: Vec<ProviderOutcome> = sources
        .iter()
        .map(|s| ProviderOutcome {
            records: vec![record("Merged", Some("Together"), s.clone())],
            coverage: SourceCoverage::complete("probe"),
        })
        .collect();
    let inventory: Inventory = merge_inventory(outcomes, &limits);
    assert_eq!(
        inventory.records.len(),
        1,
        "the same pair across sources is ONE logical application"
    );
    assert_eq!(
        inventory.records[0].id,
        ApplicationId::derive("Merged", Some("Together")),
        "the merged record's id must be the derived id"
    );
    assert_eq!(
        inventory.records[0].provenance.len(),
        3,
        "provenance is the union across sources"
    );
}

#[test]
fn distinct_pairs_stay_distinct_after_merging() {
    let limits = DiscoveryLimits::default();
    let outcomes = vec![ProviderOutcome {
        records: vec![
            record("Alpha", Some("Pub"), ApplicationSource::RegistryUninstall),
            record("Alpha", Some("Other"), ApplicationSource::RegistryUninstall),
            record("Beta", Some("Pub"), ApplicationSource::RegistryUninstall),
        ],
        coverage: SourceCoverage::complete("probe"),
    }];
    let inventory = merge_inventory(outcomes, &limits);
    assert_eq!(
        inventory.records.len(),
        3,
        "three distinct pairs are three logical applications"
    );
    let ids: Vec<&str> = inventory.records.iter().map(|r| r.id.0.as_str()).collect();
    assert!(ids.contains(&ApplicationId::derive("Alpha", Some("Pub")).0.as_str()));
    assert!(ids.contains(&ApplicationId::derive("Alpha", Some("Other")).0.as_str()));
    assert!(ids.contains(&ApplicationId::derive("Beta", Some("Pub")).0.as_str()));
    // Canonical ordering: (lowercased name, publisher, id) — the order
    // `merge_inventory` publishes, not an id-string sort.
    let keys: Vec<(String, String)> = inventory
        .records
        .iter()
        .map(|r| {
            (
                r.name.to_lowercase(),
                r.publisher.as_deref().unwrap_or("").to_string(),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "the merged inventory is canonically ordered");
}

// ---------------------------------------------------------------------------
// compatibility / legacy recognition
// ---------------------------------------------------------------------------

#[test]
fn the_legacy_key_is_still_reproducible_for_migration_recognition() {
    // The migration recognises a legacy row by recomputing what the OLD
    // encoding would have derived from that row's own facts. That
    // recomputation must stay available and stable.
    assert_eq!(
        ApplicationId::legacy_derivation_key("A|B", Some("C")),
        "a|b|c",
        "the legacy key is the delimiter-joined normalized pair"
    );
    assert_eq!(
        ApplicationId::legacy_derivation_key("Foo", None),
        "foo|",
        "an absent publisher stays the empty component"
    );
}

#[test]
fn the_encoding_version_is_recorded_for_migration_decisions() {
    // v2 is the collision-free encoding; v1 was the ambiguous one.
    assert_eq!(ApplicationId::ID_ENCODING_VERSION, 2);
}

#[test]
fn the_compatibility_rule_is_stated_for_callers() {
    let note = ApplicationId::COMPAT_NOTE;
    assert!(note.contains("length-prefixed"), "{note}");
    assert!(note.contains("never globally replaced"), "{note}");
}

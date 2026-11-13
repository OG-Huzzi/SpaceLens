//! Uninstall-planner safety matrix (jv16 PowerTools tool #3).
//!
//! Every test uses synthetic paths and supplied evidence only: no real home
//! directory, no real install tree, and no execution. The dangerous cases are
//! the point of the suite:
//!
//! * a install-root child is `Safe`, the root itself is not;
//! * contention is always `OffLimits` and stays visible in `off_limits_kept`;
//! * a name-only match stays `Review` and says so;
//! * one unknown size makes the total unknown while `safe_bytes` stays known;
//! * `safe_bytes` saturates;
//! * ordering is deterministic under permutation;
//! * `plan_action` is always `InspectOnly`;
//! * a missing vendor command is distinguishable from an empty one.

use std::path::{Path, PathBuf};

use coresight_apps::{
    plan_action, plan_uninstall, LeftoverCandidate, LeftoverEvidence, UninstallAction,
    UninstallItemTier, UninstallPlan, UninstallRequest, UninstallScope, VendorUninstallerStatus,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn scope(home: &str, install_root: Option<&str>) -> UninstallScope {
    UninstallScope {
        home_dir: Some(PathBuf::from(home)),
        install_root: install_root.map(PathBuf::from),
        system_roots: vec![PathBuf::from("C:/Windows")],
    }
}

fn candidate(evidence: LeftoverEvidence, bytes: Option<u64>) -> LeftoverCandidate {
    LeftoverCandidate { evidence, bytes }
}

fn request_with(
    scope: UninstallScope,
    vendor_uninstaller: Option<&str>,
    candidates: Vec<LeftoverCandidate>,
) -> UninstallRequest {
    UninstallRequest {
        app_id: "app-test".to_string(),
        display_name: "Acme App".to_string(),
        scope,
        vendor_uninstaller: vendor_uninstaller.map(str::to_string),
        candidates,
    }
}

fn plan_with(scope: UninstallScope, candidates: Vec<LeftoverCandidate>) -> UninstallPlan {
    plan_uninstall(&request_with(scope, None, candidates))
}

fn path(text: &str) -> PathBuf {
    PathBuf::from(text)
}

// ---------------------------------------------------------------------------
// Install-root boundary
// ---------------------------------------------------------------------------

#[test]
fn install_root_child_is_safe_but_the_root_itself_is_not() {
    // The install root is outside the home on purpose: on Windows that is the
    // normal shape, and rule 1 would be meaningless if the home boundary
    // overrode the recorded root.
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: path("C:/Program Files/Acme/bin/app.exe"),
                },
                Some(100),
            ),
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: path("C:/Program Files/Acme"),
                },
                Some(1_000),
            ),
        ],
    );

    assert_eq!(plan.items.len(), 2, "both boundary cases are proposed");
    let child = plan
        .items
        .iter()
        .find(|item| item.path == path("C:/Program Files/Acme/bin/app.exe"))
        .expect("child item exists");
    let root = plan
        .items
        .iter()
        .find(|item| item.path == path("C:/Program Files/Acme"))
        .expect("root item exists");

    assert_eq!(child.tier, UninstallItemTier::Safe);
    assert_eq!(
        root.tier,
        UninstallItemTier::Review,
        "the declared root itself is a boundary, never Safe"
    );
    assert_ne!(root.tier, UninstallItemTier::Safe);
    assert!(child.why.contains("Strictly inside"));
    assert!(root.why.contains("root itself"));
    assert_eq!(plan.safe_bytes, 100);
    assert_eq!(plan.total_bytes, Some(1_100));
}

#[test]
fn a_recorded_install_root_inside_a_system_root_is_never_safe() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Windows/Temp/Acme")),
        vec![candidate(
            LeftoverEvidence::InsideInstallRoot {
                path: path("C:/Windows/Temp/Acme/bin/a.dll"),
            },
            Some(10),
        )],
    );

    assert!(plan.items.is_empty(), "system-root veto wins over Safe");
    assert_eq!(plan.off_limits_kept.len(), 1);
    assert_eq!(plan.off_limits_kept[0].tier, UninstallItemTier::OffLimits);
    assert!(plan.off_limits_kept[0].why.contains("system root"));
}

#[test]
fn inconsistent_inside_install_root_evidence_fails_closed() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![candidate(
            LeftoverEvidence::InsideInstallRoot {
                path: path("C:/Users/tester/AppData/Local/Acme/app.exe"),
            },
            Some(10),
        )],
    );

    assert!(plan.items.is_empty());
    assert_eq!(plan.off_limits_kept.len(), 1);
    assert_eq!(plan.off_limits_kept[0].tier, UninstallItemTier::OffLimits);
    assert!(plan.off_limits_kept[0]
        .why
        .contains("not actually inside the declared install root"));
}

// ---------------------------------------------------------------------------
// Contention
// ---------------------------------------------------------------------------

#[test]
fn contended_path_is_always_off_limits_and_kept() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![candidate(
            LeftoverEvidence::Contended {
                path: path("C:/Program Files/Acme/shared.dll"),
                other_apps: vec!["Other App".to_string()],
            },
            Some(64),
        )],
    );

    assert!(
        plan.items.is_empty(),
        "a contended path is never part of the proposal"
    );
    assert_eq!(plan.off_limits_kept.len(), 1);
    let kept = &plan.off_limits_kept[0];
    assert_eq!(kept.tier, UninstallItemTier::OffLimits);
    assert!(kept.why.contains("always off-limits"));
    assert!(kept.why.contains("Other App"));
    assert_eq!(plan.total_bytes, Some(0), "nothing is proposed");
}

#[test]
fn contention_vetoes_a_weaker_safe_claim_on_the_same_path() {
    // The same object reported once as an install-root child and once as
    // contended must have ONE decision: OffLimits. Keeping the Safe copy would
    // let a future executor delete a path another app still uses.
    let shared = path("C:/Program Files/Acme/plugin.dll");
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: shared.clone(),
                },
                Some(10),
            ),
            candidate(
                LeftoverEvidence::Contended {
                    path: shared.clone(),
                    other_apps: vec!["Other App".to_string()],
                },
                Some(10),
            ),
        ],
    );

    assert!(plan.items.is_empty(), "no weaker copy survives contention");
    assert_eq!(plan.off_limits_kept.len(), 1);
    assert_eq!(plan.off_limits_kept[0].path, shared);
    assert!(matches!(
        plan.off_limits_kept[0].evidence,
        LeftoverEvidence::Contended { .. }
    ));
}

// ---------------------------------------------------------------------------
// Heuristic name matches
// ---------------------------------------------------------------------------

#[test]
fn heuristic_name_match_is_review_and_admits_the_weak_link() {
    let plan = plan_with(
        scope("C:/Users/tester", None),
        vec![candidate(
            LeftoverEvidence::HeuristicNameMatch {
                path: path("C:/Users/tester/AppData/Roaming/AcmeCache"),
            },
            Some(12),
        )],
    );

    assert_eq!(plan.items.len(), 1);
    let item = &plan.items[0];
    assert_eq!(item.tier, UninstallItemTier::Review);
    assert!(
        item.why.contains("Name match only"),
        "the reason must name the weakness: {}",
        item.why
    );
    assert!(
        item.why.contains("not recorded evidence"),
        "the reason must say the link is not recorded: {}",
        item.why
    );
    assert_eq!(plan.safe_bytes, 0);
}

#[test]
fn heuristic_outside_every_recorded_scope_is_off_limits_not_review() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![candidate(
            LeftoverEvidence::HeuristicNameMatch {
                path: path("D:/Elsewhere/AcmeCache"),
            },
            Some(12),
        )],
    );

    assert!(plan.items.is_empty());
    assert_eq!(plan.off_limits_kept.len(), 1);
    assert_eq!(plan.off_limits_kept[0].tier, UninstallItemTier::OffLimits);
    assert!(plan.off_limits_kept[0].why.contains("Name match only"));
    assert!(plan.off_limits_kept[0]
        .why
        .contains("not recorded evidence"));
}

// ---------------------------------------------------------------------------
// Honest byte accounting
// ---------------------------------------------------------------------------

#[test]
fn one_unknown_size_makes_total_unknown_but_safe_bytes_stays_known() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: path("C:/Program Files/Acme/bin/app.exe"),
                },
                Some(100),
            ),
            candidate(
                LeftoverEvidence::MatchesBundleId {
                    path: path("C:/Users/tester/AppData/Roaming/Acme/config.dat"),
                    bundle_id: "com.acme.app".to_string(),
                },
                None,
            ),
        ],
    );

    assert_eq!(
        plan.total_bytes, None,
        "a partial total must never be presented as complete"
    );
    assert_eq!(plan.unknown_size_items, 1);
    assert_eq!(plan.safe_bytes, 100, "the known safe size is still counted");
}

#[test]
fn safe_bytes_excludes_known_review_sizes() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: path("C:/Program Files/Acme/a.bin"),
                },
                Some(100),
            ),
            candidate(
                LeftoverEvidence::MatchesBundleId {
                    path: path("C:/Users/tester/AppData/Roaming/Acme/b.dat"),
                    bundle_id: "com.acme.app".to_string(),
                },
                Some(50),
            ),
        ],
    );

    assert_eq!(plan.safe_bytes, 100, "Review sizes are not safe bytes");
    assert_eq!(plan.total_bytes, Some(150));
}

#[test]
fn safe_bytes_saturates_instead_of_wrapping() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: path("C:/Program Files/Acme/a.bin"),
                },
                Some(u64::MAX),
            ),
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: path("C:/Program Files/Acme/b.bin"),
                },
                Some(10),
            ),
        ],
    );

    assert_eq!(plan.safe_bytes, u64::MAX);
    assert_eq!(plan.total_bytes, Some(u64::MAX));
}

#[test]
fn conflicting_sizes_on_one_path_become_unknown_instead_of_a_guess() {
    let target = path("C:/Program Files/Acme/data.bin");
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: target.clone(),
                },
                Some(10),
            ),
            candidate(
                LeftoverEvidence::InsideInstallRoot { path: target },
                Some(20),
            ),
        ],
    );

    assert_eq!(plan.items.len(), 1, "one path has one decision");
    assert_eq!(plan.items[0].bytes, None);
    assert_eq!(plan.unknown_size_items, 1);
    assert_eq!(plan.total_bytes, None);
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn ordering_is_deterministic_under_permuted_input() {
    let candidates = vec![
        candidate(
            LeftoverEvidence::MatchesBundleId {
                path: path("C:/Users/tester/AppData/Roaming/Acme/z.dat"),
                bundle_id: "com.acme.app".to_string(),
            },
            Some(1),
        ),
        candidate(
            LeftoverEvidence::InsideInstallRoot {
                path: path("C:/Program Files/Acme/b.bin"),
            },
            Some(1),
        ),
        candidate(
            LeftoverEvidence::InsideInstallRoot {
                path: path("C:/Program Files/Acme/a.bin"),
            },
            Some(1),
        ),
        candidate(
            LeftoverEvidence::HeuristicNameMatch {
                path: path("C:/Windows/System32/acme.dll"),
            },
            Some(1),
        ),
    ];

    let forward = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        candidates.clone(),
    );
    let mut reversed_candidates = candidates;
    reversed_candidates.reverse();
    let reversed = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        reversed_candidates,
    );

    assert_eq!(forward, reversed, "input order must not change the plan");

    let tiers: Vec<UninstallItemTier> = forward.items.iter().map(|item| item.tier).collect();
    assert_eq!(
        tiers,
        vec![
            UninstallItemTier::Safe,
            UninstallItemTier::Safe,
            UninstallItemTier::Review
        ]
    );
    assert_eq!(forward.items[0].path, path("C:/Program Files/Acme/a.bin"));
    assert_eq!(forward.items[1].path, path("C:/Program Files/Acme/b.bin"));
    assert_eq!(
        forward.items[2].path,
        path("C:/Users/tester/AppData/Roaming/Acme/z.dat")
    );
    assert_eq!(forward.off_limits_kept.len(), 1);
}

// ---------------------------------------------------------------------------
// Review evidence and scope
// ---------------------------------------------------------------------------

#[test]
fn bundle_appdata_and_registry_evidence_are_review() {
    let plan = plan_with(
        scope("C:/Users/tester", None),
        vec![
            candidate(
                LeftoverEvidence::MatchesBundleId {
                    path: path("C:/Users/tester/Library/Preferences/com.acme.plist"),
                    bundle_id: "com.acme.app".to_string(),
                },
                Some(1),
            ),
            candidate(
                LeftoverEvidence::InsideAppDataRoot {
                    path: path("C:/Users/tester/AppData/Roaming/Acme"),
                    root: "Roaming".to_string(),
                },
                Some(2),
            ),
            candidate(
                LeftoverEvidence::MatchesUninstallKey {
                    key_path: "HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Acme"
                        .to_string(),
                },
                Some(3),
            ),
        ],
    );

    assert_eq!(plan.items.len(), 3);
    assert!(plan
        .items
        .iter()
        .all(|item| item.tier == UninstallItemTier::Review));
    assert!(plan
        .items
        .iter()
        .any(|item| item.why.contains("com.acme.app")));
    assert!(plan
        .items
        .iter()
        .any(|item| item.why.contains("\"Roaming\"")));
    assert!(plan
        .items
        .iter()
        .any(|item| item.why.contains("uninstall registry key")));
}

#[test]
fn registry_key_evidence_does_not_need_a_filesystem_scope() {
    let plan = plan_with(
        UninstallScope::default(),
        vec![candidate(
            LeftoverEvidence::MatchesUninstallKey {
                key_path: "HKCU\\Software\\Acme".to_string(),
            },
            Some(1),
        )],
    );

    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].tier, UninstallItemTier::Review);
    assert!(plan.off_limits_kept.is_empty());
}

#[test]
fn paths_outside_home_and_the_install_root_are_off_limits() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![
            candidate(
                LeftoverEvidence::MatchesBundleId {
                    path: path("C:/Windows/System32/acme.dll"),
                    bundle_id: "com.acme.app".to_string(),
                },
                Some(1),
            ),
            candidate(
                LeftoverEvidence::InsideAppDataRoot {
                    path: path("D:/Elsewhere/Acme"),
                    root: "Roaming".to_string(),
                },
                Some(1),
            ),
        ],
    );

    assert!(plan.items.is_empty());
    assert_eq!(plan.off_limits_kept.len(), 2);
    // Off-limits items are ordered by path bytes: C:/Windows before D:/Elsewhere.
    assert!(plan.off_limits_kept[0].why.contains("system root"));
    assert!(plan.off_limits_kept[1]
        .why
        .contains("outside the user's home"));
}

#[test]
fn dot_dot_segments_are_resolved_before_scope_checks() {
    let plan = plan_with(
        scope("C:/Users/tester", None),
        vec![candidate(
            LeftoverEvidence::MatchesBundleId {
                path: path("C:/Users/tester/../../Windows/System32/acme.dll"),
                bundle_id: "com.acme.app".to_string(),
            },
            Some(1),
        )],
    );

    assert!(plan.items.is_empty());
    assert_eq!(plan.off_limits_kept.len(), 1);
    assert!(plan.off_limits_kept[0].why.contains("system root"));
}

#[test]
fn an_empty_home_is_never_a_root_that_contains_everything() {
    let plan = plan_with(
        UninstallScope {
            home_dir: Some(PathBuf::new()),
            install_root: None,
            system_roots: Vec::new(),
        },
        vec![candidate(
            LeftoverEvidence::HeuristicNameMatch {
                path: path("C:/Users/tester/AppData/Roaming/AcmeCache"),
            },
            Some(1),
        )],
    );

    assert!(
        plan.items.is_empty(),
        "an empty scope must fail closed, not claim universal containment"
    );
    assert_eq!(plan.off_limits_kept.len(), 1);
    assert_eq!(plan.off_limits_kept[0].tier, UninstallItemTier::OffLimits);
}

// ---------------------------------------------------------------------------
// Action and vendor-command honesty
// ---------------------------------------------------------------------------

#[test]
fn plan_action_is_always_inspect_only() {
    let empty = plan_with(scope("C:/Users/tester", None), vec![]);
    let populated = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![candidate(
            LeftoverEvidence::InsideInstallRoot {
                path: path("C:/Program Files/Acme/bin/app.exe"),
            },
            Some(1),
        )],
    );

    assert_eq!(plan_action(&empty), UninstallAction::InspectOnly);
    assert_eq!(plan_action(&populated), UninstallAction::InspectOnly);
    assert!(matches!(
        plan_action(&populated),
        UninstallAction::InspectOnly
    ));
}

#[test]
fn missing_vendor_uninstaller_is_distinguishable_from_an_empty_one() {
    let candidates = || {
        vec![candidate(
            LeftoverEvidence::InsideInstallRoot {
                path: path("C:/Program Files/Acme/bin/app.exe"),
            },
            Some(1),
        )]
    };

    let missing = plan_uninstall(&request_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        None,
        candidates(),
    ));
    let empty = plan_uninstall(&request_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        Some(""),
        candidates(),
    ));
    let recorded = plan_uninstall(&request_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        Some("MsiExec.exe /X{00000000-0000-0000-0000-000000000000}"),
        candidates(),
    ));

    assert_eq!(
        missing.vendor_uninstaller_status(),
        VendorUninstallerStatus::NotRecorded
    );
    assert_eq!(
        empty.vendor_uninstaller_status(),
        VendorUninstallerStatus::RecordedEmpty
    );
    assert_eq!(
        recorded.vendor_uninstaller_status(),
        VendorUninstallerStatus::Recorded
    );
    assert_ne!(
        missing.vendor_uninstaller_status(),
        empty.vendor_uninstaller_status()
    );
    assert_eq!(empty.vendor_uninstaller, Some(String::new()));
    assert!(missing.note.contains("not recorded"));
    assert!(empty.note.contains("recorded as an empty command"));
    assert_ne!(missing.note, empty.note);

    for plan in [&missing, &empty, &recorded] {
        assert!(plan.note.contains("This build removes nothing"));
        assert!(plan.note.contains("no vendor uninstaller is run"));
        assert!(plan.note.contains("typed safety pipeline"));
    }
}

#[test]
fn empty_plan_still_states_the_contract_and_has_a_zero_total() {
    let plan = plan_with(scope("C:/Users/tester", None), vec![]);

    assert!(plan.items.is_empty());
    assert!(plan.off_limits_kept.is_empty());
    assert_eq!(plan.total_bytes, Some(0));
    assert_eq!(plan.safe_bytes, 0);
    assert_eq!(plan.unknown_size_items, 0);
    assert!(plan.note.contains("removes nothing"));
}

// ---------------------------------------------------------------------------
// Shape and purity guards
// ---------------------------------------------------------------------------

#[test]
fn serialize_uses_camel_case_fields_and_screaming_snake_enums() {
    let plan = plan_with(
        scope("C:/Users/tester", Some("C:/Program Files/Acme")),
        vec![
            candidate(
                LeftoverEvidence::InsideInstallRoot {
                    path: path("C:/Program Files/Acme/bin/app.exe"),
                },
                Some(10),
            ),
            candidate(
                LeftoverEvidence::MatchesBundleId {
                    path: path("C:/Users/tester/AppData/Roaming/Acme/z.dat"),
                    bundle_id: "com.acme.app".to_string(),
                },
                Some(2),
            ),
        ],
    );

    let value = serde_json::to_value(&plan).expect("plan serializes");
    for key in [
        "appId",
        "displayName",
        "items",
        "totalBytes",
        "safeBytes",
        "unknownSizeItems",
        "offLimitsKept",
        "vendorUninstaller",
        "note",
    ] {
        assert!(value.get(key).is_some(), "missing camelCase field {key}");
    }

    let first = &value["items"][0];
    assert_eq!(first["tier"], serde_json::json!("SAFE"));
    assert!(first["evidence"].get("INSIDE_INSTALL_ROOT").is_some());
    assert!(first["evidence"]["INSIDE_INSTALL_ROOT"]
        .get("path")
        .is_some());

    let second = &value["items"][1];
    assert_eq!(second["tier"], serde_json::json!("REVIEW"));
    assert!(second["evidence"]["MATCHES_BUNDLE_ID"]
        .get("bundleId")
        .is_some());
}

#[test]
fn uninstall_module_is_inert_and_panic_free() {
    // The crate has a source guard for the shared intelligence files; this
    // module is new and needs its own. Needles are ordinary strings because
    // only the module file is scanned, never this test.
    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/uninstall.rs");
    let text = std::fs::read_to_string(&module).expect("uninstall.rs is readable");
    let code: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();

    for needle in [
        "std::fs",
        "std::process",
        "std::net",
        "std::io",
        "Command::new",
        "OpenOptions",
        "winreg",
        "unsafe",
        ".unwrap(",
        ".expect(",
        "panic!",
        "unreachable!",
        "todo!",
        "to_string_lossy",
    ] {
        assert!(
            !code.iter().any(|line| line.contains(needle)),
            "{needle:?} found in the uninstall planner; it must stay pure and panic-free"
        );
    }
}

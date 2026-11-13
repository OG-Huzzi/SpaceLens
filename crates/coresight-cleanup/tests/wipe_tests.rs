//! Integration tests for the wiper planner.
//!
//! Every fixture is synthetic and in memory: `plan_wipe` reads no filesystem
//! and writes none. The tests prove the honesty contracts — the medium decides
//! the assurance, `Unknown` is never upgraded, snapshots block even on
//! magnetic media, size uncertainty is never rounded away, containment is
//! enforced, ordering is deterministic, and the jv16 "fake files" feature is
//! refused — plus the static rule that the production module contains no panic
//! path and no I/O.

use coresight_cleanup::{
    plan_wipe, plausible_deniability_supported, StorageMedium, WipeAssurance, WipeBlocker,
    WipeOptions, WipePlan, WipeStep, WipeTarget,
};
use std::path::{Path, PathBuf};

// --- fixtures --------------------------------------------------------------

fn roots() -> Vec<PathBuf> {
    vec![PathBuf::from("/allowed")]
}

fn target(path: &str, bytes: Option<u64>) -> WipeTarget {
    WipeTarget {
        path: PathBuf::from(path),
        bytes,
        may_have_snapshots: false,
    }
}

fn snapshot_target(path: &str, bytes: Option<u64>) -> WipeTarget {
    WipeTarget {
        path: PathBuf::from(path),
        bytes,
        may_have_snapshots: true,
    }
}

fn options() -> WipeOptions {
    WipeOptions::default()
}

fn step_for<'a>(plan: &'a WipePlan, path: &str) -> &'a WipeStep {
    plan.steps
        .iter()
        .find(|step| step.target == Path::new(path))
        .expect("the plan must contain a step for the requested path")
}

// --- contract 1: solid-state and copy-on-write media ------------------------

#[test]
fn solid_state_never_yields_overwrite_plausible() {
    let plan = plan_wipe(
        &[
            target("/allowed/a", Some(10)),
            snapshot_target("/allowed/b", Some(20)),
        ],
        StorageMedium::SolidState,
        &roots(),
        &options(),
    );

    assert_eq!(
        plan.steps.len(),
        2,
        "both targets are reported, none dropped"
    );
    for step in &plan.steps {
        assert!(
            matches!(
                step.assurance,
                WipeAssurance::RequiresFullDiskSanitisation { .. }
            ),
            "solid state must recommend full-disk sanitisation, got {:?}",
            step.assurance
        );
        assert!(
            !matches!(step.assurance, WipeAssurance::OverwritePlausible { .. }),
            "solid state must never offer a fake overwrite"
        );
        assert_eq!(
            step.blocked,
            Some(WipeBlocker::MediumDefeatsOverwrite),
            "the medium is the dominant blocker, even when snapshots also apply"
        );
    }
    assert_eq!(plan.blocked_count, 2);
    assert_eq!(
        plan.total_bytes,
        Some(0),
        "blocked bytes are not capacity to act"
    );
}

#[test]
fn copy_on_write_never_yields_overwrite_plausible() {
    let plan = plan_wipe(
        &[target("/allowed/a", Some(10))],
        StorageMedium::CopyOnWrite,
        &roots(),
        &options(),
    );

    let step = step_for(&plan, "/allowed/a");
    assert!(matches!(
        step.assurance,
        WipeAssurance::RequiresFullDiskSanitisation { .. }
    ));
    assert_eq!(step.blocked, Some(WipeBlocker::MediumDefeatsOverwrite));
}

// --- contract 2: unknown medium --------------------------------------------

#[test]
fn unknown_medium_is_best_effort_and_always_blocked() {
    let plan = plan_wipe(
        &[
            target("/allowed/a", Some(10)),
            target("/allowed/b", Some(20)),
        ],
        StorageMedium::Unknown,
        &roots(),
        &options(),
    );

    assert_eq!(plan.steps.len(), 2);
    for step in &plan.steps {
        assert!(
            matches!(step.assurance, WipeAssurance::BestEffortOnly { .. }),
            "an unidentified medium can only support a best-effort claim, got {:?}",
            step.assurance
        );
        assert!(
            !matches!(step.assurance, WipeAssurance::OverwritePlausible { .. }),
            "unknown must never be silently upgraded to magnetic"
        );
        assert!(step.blocked.is_some(), "unknown media are always blocked");
    }
    assert_eq!(plan.blocked_count, 2);
}

// --- contract 3: snapshots ---------------------------------------------------

#[test]
fn snapshots_block_even_on_magnetic_media() {
    let plan = plan_wipe(
        &[
            target("/allowed/live", Some(10)),
            snapshot_target("/allowed/old", Some(20)),
        ],
        StorageMedium::Magnetic,
        &roots(),
        &options(),
    );

    let old = step_for(&plan, "/allowed/old");
    assert_eq!(old.blocked, Some(WipeBlocker::SnapshotsMayRetain));
    assert!(
        matches!(
            old.assurance,
            WipeAssurance::OverwritePlausible { passes: 1 }
        ),
        "the medium still allows a plausible overwrite; the snapshot blocks the step"
    );

    let live = step_for(&plan, "/allowed/live");
    assert_eq!(live.blocked, None);
    assert_eq!(plan.blocked_count, 1);
    assert_eq!(
        plan.total_bytes,
        Some(10),
        "only the unblocked target contributes bytes"
    );
    assert!(plan
        .caveat
        .as_deref()
        .is_some_and(|caveat| caveat.contains("SNAPSHOTS_MAY_RETAIN")));
}

// --- contract 4: unknown size -----------------------------------------------

#[test]
fn unknown_size_makes_the_total_unknown() {
    let lenient = WipeOptions {
        passes: 1,
        require_known_size: false,
    };
    let plan = plan_wipe(
        &[
            target("/allowed/known", Some(10)),
            target("/allowed/unsized", None),
        ],
        StorageMedium::Magnetic,
        &roots(),
        &lenient,
    );

    assert_eq!(
        plan.blocked_count, 0,
        "a lenient policy does not block an unsized target"
    );
    assert_eq!(
        plan.total_bytes, None,
        "a partial sum must never be presented as a total"
    );
    assert!(
        plan.caveat.is_some(),
        "an unknown size always earns a caveat"
    );

    let strict = plan_wipe(
        &[
            target("/allowed/known", Some(10)),
            target("/allowed/unsized", None),
        ],
        StorageMedium::Magnetic,
        &roots(),
        &options(),
    );
    assert_eq!(
        step_for(&strict, "/allowed/unsized").blocked,
        Some(WipeBlocker::UnknownSize)
    );
    assert_eq!(
        strict.total_bytes, None,
        "the unknown size still makes the total unknown when the step is blocked"
    );
}

// --- contract 5: allowed roots ----------------------------------------------

#[test]
fn paths_outside_allowed_roots_are_blocked_not_included() {
    let plan = plan_wipe(
        &[
            target("/allowed/a", Some(10)),
            target("/elsewhere/b", Some(20)),
            // Component-wise containment: this is NOT inside `/allowed`.
            target("/allowed-extra/c", Some(30)),
            // A parent-directory component cannot be resolved without I/O.
            target("/allowed/../etc/d", Some(40)),
        ],
        StorageMedium::Magnetic,
        &roots(),
        &options(),
    );

    assert_eq!(plan.steps.len(), 4, "every requested target is reported");
    assert_eq!(
        step_for(&plan, "/elsewhere/b").blocked,
        Some(WipeBlocker::OutsideAllowedRoots)
    );
    assert_eq!(
        step_for(&plan, "/allowed-extra/c").blocked,
        Some(WipeBlocker::OutsideAllowedRoots),
        "prefix matching must be component-wise, not byte-prefix"
    );
    assert_eq!(
        step_for(&plan, "/allowed/../etc/d").blocked,
        Some(WipeBlocker::OutsideAllowedRoots),
        "a path that escapes the root through `..` is never silently included"
    );
    assert_eq!(plan.blocked_count, 3);
    assert_eq!(
        plan.total_bytes,
        Some(10),
        "outside paths contribute no actionable bytes"
    );
    assert!(plan
        .caveat
        .as_deref()
        .is_some_and(|caveat| caveat.contains("OUTSIDE_ALLOWED_ROOTS")));
}

#[test]
fn an_empty_root_list_blocks_everything() {
    let plan = plan_wipe(
        &[target("/allowed/a", Some(10))],
        StorageMedium::Magnetic,
        &[],
        &options(),
    );

    assert_eq!(
        step_for(&plan, "/allowed/a").blocked,
        Some(WipeBlocker::OutsideAllowedRoots)
    );
    assert_eq!(plan.total_bytes, Some(0));
}

// --- contract 6: deterministic ordering ------------------------------------

#[test]
fn ordering_is_deterministic_under_permuted_input() {
    let forward = vec![
        target("/allowed/z", Some(1)),
        target("/allowed/a", Some(2)),
        target("/allowed/m", Some(3)),
    ];
    let mut reversed = forward.clone();
    reversed.reverse();

    let first = plan_wipe(&forward, StorageMedium::Magnetic, &roots(), &options());
    let second = plan_wipe(&reversed, StorageMedium::Magnetic, &roots(), &options());

    assert_eq!(
        first, second,
        "permuting the targets must not change the plan"
    );
    let paths: Vec<PathBuf> = first.steps.iter().map(|step| step.target.clone()).collect();
    assert_eq!(
        paths,
        vec![
            PathBuf::from("/allowed/a"),
            PathBuf::from("/allowed/m"),
            PathBuf::from("/allowed/z"),
        ],
        "steps are ordered by path bytes ascending"
    );
}

// --- contract 7: the jv16 feature is refused --------------------------------

#[test]
fn plausible_deniability_is_refused() {
    assert!(
        !plausible_deniability_supported(),
        "CoreSight declines to generate fake files: deceptive, no cryptographic benefit, \
         and easily mistaken for data protection"
    );
}

// --- contract 8: no panic path and no I/O in production --------------------

#[test]
fn the_wipe_module_has_no_panic_path_or_io() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/wipe.rs");
    let source = std::fs::read_to_string(path).expect("src/wipe.rs must be readable");
    for forbidden in [
        ".unwrap(",
        ".expect(",
        "panic!",
        "todo!",
        "unimplemented!",
        "std::fs::",
        "std::process",
        "std::net",
        "Command::new",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/wipe.rs must not contain {forbidden}"
        );
    }
}

// --- option and shape rules ------------------------------------------------

#[test]
fn a_zero_pass_count_is_clamped_to_one() {
    let zero = WipeOptions {
        passes: 0,
        require_known_size: true,
    };
    let plan = plan_wipe(
        &[target("/allowed/a", Some(1))],
        StorageMedium::Magnetic,
        &roots(),
        &zero,
    );

    assert!(
        matches!(
            step_for(&plan, "/allowed/a").assurance,
            WipeAssurance::OverwritePlausible { passes: 1 }
        ),
        "zero passes describes no overwrite, so it is clamped to one"
    );

    let seven = WipeOptions {
        passes: 7,
        require_known_size: true,
    };
    let recorded = plan_wipe(
        &[target("/allowed/a", Some(1))],
        StorageMedium::Magnetic,
        &roots(),
        &seven,
    );
    assert!(matches!(
        step_for(&recorded, "/allowed/a").assurance,
        WipeAssurance::OverwritePlausible { passes: 7 }
    ));
}

#[test]
fn directory_shaped_paths_are_not_regular_files() {
    let plan = plan_wipe(
        &[
            target("/allowed/file", Some(1)),
            target("/allowed/folder/", Some(2)),
            target("/allowed/sub\\", Some(3)),
        ],
        StorageMedium::Magnetic,
        &roots(),
        &options(),
    );

    assert_eq!(step_for(&plan, "/allowed/file").blocked, None);
    assert_eq!(
        step_for(&plan, "/allowed/folder/").blocked,
        Some(WipeBlocker::NotARegularFile)
    );
    assert_eq!(
        step_for(&plan, "/allowed/sub\\").blocked,
        Some(WipeBlocker::NotARegularFile),
        "a trailing backslash is also a directory spelling"
    );
    assert_eq!(plan.blocked_count, 2);
}

#[test]
fn every_plan_states_that_this_build_does_not_wipe() {
    for medium in [
        StorageMedium::SolidState,
        StorageMedium::Magnetic,
        StorageMedium::CopyOnWrite,
        StorageMedium::Unknown,
    ] {
        let plan = plan_wipe(&[], medium, &roots(), &options());
        let note = plan.note.to_lowercase();
        assert!(
            note.contains("performs no wiping"),
            "notes must say so plainly"
        );
        assert!(
            note.contains("multi-pass overwrite is not a guarantee"),
            "notes must not imply that more passes help"
        );
        assert!(note.contains("wear-levelled"));
        assert!(note.contains("copy-on-write"));
    }
}

#[test]
fn an_empty_target_list_yields_a_clean_plan() {
    let plan = plan_wipe(&[], StorageMedium::Magnetic, &roots(), &options());

    assert!(plan.steps.is_empty());
    assert_eq!(plan.blocked_count, 0);
    assert_eq!(plan.total_bytes, Some(0));
    assert_eq!(plan.caveat, None);
    assert!(!plan.note.is_empty(), "the note is never optional");
}

#[test]
fn a_clean_magnetic_plan_carries_no_caveat() {
    let plan = plan_wipe(
        &[target("/allowed/a", Some(4))],
        StorageMedium::Magnetic,
        &roots(),
        &options(),
    );

    assert_eq!(plan.blocked_count, 0);
    assert_eq!(plan.caveat, None);
    assert_eq!(plan.total_bytes, Some(4));
    assert!(matches!(
        step_for(&plan, "/allowed/a").assurance,
        WipeAssurance::OverwritePlausible { passes: 1 }
    ));
}

#[test]
fn total_bytes_saturates_instead_of_wrapping() {
    let plan = plan_wipe(
        &[
            target("/allowed/a", Some(u64::MAX)),
            target("/allowed/b", Some(10)),
        ],
        StorageMedium::Magnetic,
        &roots(),
        &options(),
    );

    assert_eq!(plan.total_bytes, Some(u64::MAX));
}

// --- stable codes and wire format -----------------------------------------

#[test]
fn blockers_expose_stable_codes_and_reasons() {
    assert_eq!(
        WipeBlocker::MediumDefeatsOverwrite.code(),
        "MEDIUM_DEFEATS_OVERWRITE"
    );
    assert_eq!(
        WipeBlocker::SnapshotsMayRetain.code(),
        "SNAPSHOTS_MAY_RETAIN"
    );
    assert_eq!(WipeBlocker::UnknownSize.code(), "UNKNOWN_SIZE");
    assert_eq!(
        WipeBlocker::OutsideAllowedRoots.code(),
        "OUTSIDE_ALLOWED_ROOTS"
    );
    assert_eq!(WipeBlocker::NotARegularFile.code(), "NOT_A_REGULAR_FILE");
    for blocker in [
        WipeBlocker::MediumDefeatsOverwrite,
        WipeBlocker::SnapshotsMayRetain,
        WipeBlocker::UnknownSize,
        WipeBlocker::OutsideAllowedRoots,
        WipeBlocker::NotARegularFile,
    ] {
        assert!(
            !blocker.reason().is_empty(),
            "{:?} must explain itself",
            blocker
        );
    }
}

#[test]
fn the_wire_format_is_camel_case_fields_and_screaming_snake_enums() {
    let step = WipeStep {
        target: PathBuf::from("/allowed/a"),
        assurance: WipeAssurance::OverwritePlausible { passes: 3 },
        blocked: Some(WipeBlocker::SnapshotsMayRetain),
    };
    let json = serde_json::to_string(&step).expect("the step serializes");
    assert!(json.contains("\"target\":\"/allowed/a\""));
    assert!(json.contains("\"passes\":3"));
    assert!(json.contains("OVERWRITE_PLAUSIBLE"));
    assert!(json.contains("SNAPSHOTS_MAY_RETAIN"));
    let decoded: WipeStep = serde_json::from_str(&json).expect("the step deserializes");
    assert_eq!(decoded, step);

    let target = snapshot_target("/allowed/a", Some(1));
    let target_json = serde_json::to_string(&target).expect("the target serializes");
    assert!(target_json.contains("\"mayHaveSnapshots\":true"));
    assert!(target_json.contains("\"bytes\":1"));
}

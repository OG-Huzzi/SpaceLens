//! Bulk-rename PLANNING contract tests.
//!
//! No file is touched. The tests prove extension preservation, deterministic
//! sequence assignment under input permutation, saturating numbering, the
//! duplicate-target / empty-name / invalid-name / duplicate-source rejects,
//! and that a truthful no-op preview is retained rather than hidden.

use coresight_filetools::{plan_rename, RenameError, RenameInput, RenamePlan, RenameRule};
use std::path::PathBuf;

fn input(path: &str) -> RenameInput {
    RenameInput {
        path: PathBuf::from(path),
    }
}

fn plan(entries: &[RenameInput], rule: &RenameRule) -> RenamePlan {
    plan_rename(entries, rule).expect("plan should be accepted")
}

fn target(plan: &RenamePlan, from: &str) -> PathBuf {
    plan.operations
        .iter()
        .find(|operation| operation.from.as_path() == std::path::Path::new(from))
        .expect("operation should exist")
        .to
        .clone()
}

#[test]
fn prefix_is_added_to_the_stem_and_extension_is_preserved() {
    let result = plan(
        &[input("/work/report.txt"), input("/work/notes")],
        &RenameRule::AddPrefix("2026-".to_string()),
    );

    assert_eq!(
        target(&result, "/work/report.txt"),
        PathBuf::from("/work/2026-report.txt")
    );
    assert_eq!(
        target(&result, "/work/notes"),
        PathBuf::from("/work/2026-notes")
    );
}

#[test]
fn suffix_is_added_to_the_stem_not_the_extension() {
    let result = plan(
        &[input("/work/archive.tar.gz")],
        &RenameRule::AddSuffix("_v2".to_string()),
    );

    assert_eq!(
        target(&result, "/work/archive.tar.gz"),
        PathBuf::from("/work/archive.tar_v2.gz")
    );
}

#[test]
fn replace_operates_on_the_stem_only() {
    let result = plan(
        &[input("/d/My Report.txt"), input("/d/a.txt")],
        &RenameRule::Replace {
            find: " ".to_string(),
            replace_with: "_".to_string(),
        },
    );
    assert_eq!(
        target(&result, "/d/My Report.txt"),
        PathBuf::from("/d/My_Report.txt")
    );

    let stem_only = plan(
        &[input("/d/a.txt")],
        &RenameRule::Replace {
            find: "txt".to_string(),
            replace_with: "md".to_string(),
        },
    );
    assert_eq!(
        target(&stem_only, "/d/a.txt"),
        PathBuf::from("/d/a.txt"),
        "the extension is never part of the replacement surface"
    );
}

#[test]
fn remove_characters_treats_the_rule_as_a_set() {
    let result = plan(
        &[input("/d/my report-final.txt")],
        &RenameRule::RemoveCharacters(" -".to_string()),
    );

    assert_eq!(
        target(&result, "/d/my report-final.txt"),
        PathBuf::from("/d/myreportfinal.txt")
    );
}

#[test]
fn sequence_numbers_follow_canonical_path_order_not_input_order() {
    let forward = plan(
        &[input("/d/a.txt"), input("/d/m.txt"), input("/d/z.txt")],
        &RenameRule::Sequence {
            start: 1,
            padding: 0,
        },
    );
    let reversed = plan(
        &[input("/d/z.txt"), input("/d/m.txt"), input("/d/a.txt")],
        &RenameRule::Sequence {
            start: 1,
            padding: 0,
        },
    );

    assert_eq!(forward, reversed);
    assert_eq!(target(&forward, "/d/a.txt"), PathBuf::from("/d/a_1.txt"));
    assert_eq!(target(&forward, "/d/m.txt"), PathBuf::from("/d/m_2.txt"));
    assert_eq!(target(&forward, "/d/z.txt"), PathBuf::from("/d/z_3.txt"));
}

#[test]
fn sequence_padding_is_applied() {
    let result = plan(
        &[input("/d/photo.jpg")],
        &RenameRule::Sequence {
            start: 7,
            padding: 4,
        },
    );

    assert_eq!(
        target(&result, "/d/photo.jpg"),
        PathBuf::from("/d/photo_0007.jpg")
    );
}

#[test]
fn sequence_numbers_saturate_at_u64_max() {
    let result = plan(
        &[input("/d/a.txt"), input("/d/b.txt")],
        &RenameRule::Sequence {
            start: u64::MAX,
            padding: 0,
        },
    );

    let expected = format!("_{}", u64::MAX);
    assert_eq!(
        target(&result, "/d/a.txt"),
        PathBuf::from(format!("/d/a{expected}.txt"))
    );
    assert_eq!(
        target(&result, "/d/b.txt"),
        PathBuf::from(format!("/d/b{expected}.txt")),
        "start + index saturates instead of wrapping"
    );
}

#[test]
fn absurd_sequence_padding_is_clamped_not_allocated() {
    let result = plan(
        &[input("/d/a.txt")],
        &RenameRule::Sequence {
            start: 1,
            padding: usize::MAX,
        },
    );

    let name = target(&result, "/d/a.txt");
    let name = name
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .expect("file name");
    assert!(name.starts_with("a_"));
    assert!(name.ends_with(".txt"));
    assert!(name.len() < 64, "padding must be clamped, got {name:?}");
}

#[test]
fn duplicate_targets_are_rejected() {
    let error = plan_rename(
        &[input("/d/a-b.txt"), input("/d/ab.txt")],
        &RenameRule::RemoveCharacters("-".to_string()),
    )
    .expect_err("both entries would become ab.txt");

    assert!(matches!(error, RenameError::DuplicateTarget { .. }));
}

#[test]
fn empty_names_are_rejected() {
    let error = plan_rename(
        &[input("/d/README")],
        &RenameRule::RemoveCharacters("README".to_string()),
    )
    .expect_err("removing every character leaves an empty name");

    assert!(matches!(error, RenameError::EmptyName { .. }));
}

#[test]
fn names_containing_path_separators_are_rejected() {
    let error = plan_rename(
        &[input("/d/a.txt")],
        &RenameRule::AddPrefix("../".to_string()),
    )
    .expect_err("a rule must never inject a path component");

    assert!(matches!(error, RenameError::InvalidName { .. }));
}

#[test]
fn duplicate_source_entries_are_rejected() {
    let error = plan_rename(
        &[input("/d/a.txt"), input("/d/a.txt")],
        &RenameRule::AddPrefix("x_".to_string()),
    )
    .expect_err("the same source cannot be planned twice");

    assert!(matches!(error, RenameError::DuplicateSource { .. }));
}

#[test]
fn a_rule_that_changes_nothing_keeps_a_truthful_no_op_preview() {
    let result = plan(
        &[input("/d/a.txt")],
        &RenameRule::Replace {
            find: "zzz".to_string(),
            replace_with: "x".to_string(),
        },
    );

    assert_eq!(result.operations.len(), 1);
    assert_eq!(result.operations[0].from, result.operations[0].to);
}

#[test]
fn empty_input_yields_an_empty_plan_without_error() {
    let result = plan(&[], &RenameRule::AddPrefix("x_".to_string()));
    assert!(result.operations.is_empty());
}

#[test]
fn dotfiles_and_trailing_dots_are_preserved() {
    let dotfile = plan(
        &[input("/d/.gitignore")],
        &RenameRule::AddPrefix("x".to_string()),
    );
    assert_eq!(
        target(&dotfile, "/d/.gitignore"),
        PathBuf::from("/d/x.gitignore")
    );

    let trailing_dot = plan(
        &[input("/d/report.")],
        &RenameRule::AddPrefix("x".to_string()),
    );
    assert_eq!(
        target(&trailing_dot, "/d/report."),
        PathBuf::from("/d/xreport.")
    );
}

#[test]
fn prefix_plan_is_identical_across_input_permutations() {
    let entries = vec![
        input("/d/zeta.txt"),
        input("/a/alpha.txt"),
        input("/m/middle.txt"),
    ];
    let mut reversed = entries.clone();
    reversed.reverse();
    let rule = RenameRule::AddSuffix("_final".to_string());

    assert_eq!(plan_rename(&entries, &rule), plan_rename(&reversed, &rule));
}

#[cfg(windows)]
#[test]
fn case_only_rename_is_a_real_operation_not_a_duplicate_source() {
    let result = plan(
        &[input("C:\\d\\File.txt")],
        &RenameRule::Replace {
            find: "File".to_string(),
            replace_with: "file".to_string(),
        },
    );

    assert_eq!(result.operations.len(), 1);
    assert_eq!(result.operations[0].to, PathBuf::from("C:\\d\\file.txt"));
}

#[cfg(windows)]
#[test]
fn case_variant_source_paths_are_one_path_on_windows() {
    let error = plan_rename(
        &[input("C:\\d\\File.txt"), input("C:\\d\\file.txt")],
        &RenameRule::AddPrefix("x_".to_string()),
    )
    .expect_err("Windows path identity is case-insensitive");

    assert!(matches!(error, RenameError::DuplicateSource { .. }));
}

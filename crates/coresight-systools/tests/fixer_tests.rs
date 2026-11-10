//! Integration tests for the `fixer` module.
//!
//! Every fixture is synthetic. The tests prove the module's honesty
//! contracts: a skipped check is never a pass, `Unknown` is never healthy and
//! never broken, no rule claims repairability, plans are inspection-only,
//! ordering is deterministic, counters saturate, and the production module
//! contains no I/O, execution, or panic path.

use std::collections::BTreeSet;

use coresight_systools::{
    default_rules, plan_repairs, run, update_findings, CheckObservation, DiagnosticRule,
    DiagnosticRun, FindingSeverity, RepairAction, SystemFacts, UpdateCheck, UpdateState,
};

fn observed(check_id: &'static str, text: &str) -> CheckObservation {
    CheckObservation {
        check_id,
        observed: Some(text.to_string()),
        source: format!("synthetic probe for {check_id}"),
        failed: false,
    }
}

fn failed(check_id: &'static str) -> CheckObservation {
    CheckObservation {
        check_id,
        observed: None,
        source: format!("synthetic probe for {check_id}"),
        failed: true,
    }
}

fn facts(checks: Vec<CheckObservation>) -> SystemFacts {
    SystemFacts { checks }
}

fn rule(code: &'static str, check_id: &'static str, severity: FindingSeverity) -> DiagnosticRule {
    DiagnosticRule {
        code,
        check_id,
        severity,
        description: "Synthetic rule used by the fixer tests.",
        recommended_interval_days: None,
    }
}

fn codes(result: &DiagnosticRun) -> Vec<&'static str> {
    result
        .findings
        .iter()
        .map(|finding| finding.rule.code)
        .collect()
}

fn update_check(component: &str, state: UpdateState) -> UpdateCheck {
    UpdateCheck {
        component: component.to_string(),
        state,
        evidence: format!("synthetic evidence for {component}"),
        source: "synthetic update probe".to_string(),
    }
}

// --- default rule set -------------------------------------------------------

#[test]
fn default_rule_set_is_small_well_formed_and_evidence_only() {
    let rules = default_rules();
    assert!(
        (6..=10).contains(&rules.len()),
        "default rule set should stay small, got {}",
        rules.len()
    );

    let mut rule_codes = BTreeSet::new();
    let mut check_ids = BTreeSet::new();
    let mut has_interval = false;
    let mut has_no_interval = false;
    let mut has_critical = false;
    let mut has_attention = false;

    for rule in &rules {
        assert!(
            rule_codes.insert(rule.code),
            "duplicate rule code {}",
            rule.code
        );
        assert!(
            check_ids.insert(rule.check_id),
            "duplicate check id {}",
            rule.check_id
        );
        assert!(
            rule.code
                .chars()
                .all(|character| character.is_ascii_uppercase()
                    || character.is_ascii_digit()
                    || character == '_'),
            "code {} is not SCREAMING_SNAKE_CASE",
            rule.code
        );
        assert!(!rule.description.trim().is_empty());
        assert!(
            rule.description.ends_with('.'),
            "description must be a sentence: {}",
            rule.description
        );
        assert!(
            !rule.description.to_lowercase().contains("registry"),
            "this crate cannot see the registry, so no rule may point at it: {}",
            rule.code
        );
        match rule.recommended_interval_days {
            Some(days) => {
                assert!(days > 0, "{} has a zero-day interval", rule.code);
                has_interval = true;
            }
            None => has_no_interval = true,
        }
        match rule.severity {
            FindingSeverity::Critical => has_critical = true,
            FindingSeverity::Attention => has_attention = true,
            FindingSeverity::Info => {}
        }
    }

    assert!(
        has_interval && has_no_interval,
        "interval hints must stay optional"
    );
    assert!(
        has_critical && has_attention,
        "the set must distinguish severity"
    );
}

// --- skipped versus evaluated ----------------------------------------------

#[test]
fn skipped_check_is_not_a_passing_check() {
    let rules = [
        rule("CORE_TEST_ALPHA", "alpha", FindingSeverity::Attention),
        rule("CORE_TEST_BETA", "beta", FindingSeverity::Attention),
    ];

    let skipped = run(&facts(vec![failed("alpha")]), &rules);
    assert!(skipped.findings.is_empty());
    assert_eq!(skipped.evaluated, 0);
    assert_eq!(skipped.skipped, 2);
    assert_eq!(skipped.accounted_rules(), 2);

    let evidenced = run(
        &facts(vec![observed("alpha", "alpha is evidenced")]),
        &rules,
    );
    assert_eq!(evidenced.findings.len(), 1);
    assert_eq!(evidenced.evaluated, 1);
    assert_eq!(evidenced.skipped, 1);

    let complete = run(
        &facts(vec![
            observed("alpha", "alpha is evidenced"),
            observed("beta", "beta is evidenced"),
        ]),
        &rules,
    );
    assert_eq!(complete.findings.len(), 2);
    assert_eq!(complete.evaluated, 2);
    assert_eq!(complete.skipped, 0);

    let skipped_note = skipped.coverage_note();
    assert!(skipped_note.contains("2 skipped"), "{skipped_note}");
    assert!(
        skipped_note.contains("a skipped check is not a passing check"),
        "{skipped_note}"
    );
    assert!(!skipped_note.contains("none skipped"), "{skipped_note}");

    let complete_note = complete.coverage_note();
    assert!(complete_note.contains("none skipped"), "{complete_note}");
    assert!(
        !complete_note.contains("not a passing check"),
        "{complete_note}"
    );

    assert_ne!(skipped.skipped, complete.skipped);
    assert_eq!(skipped.accounted_rules(), complete.accounted_rules());
}

#[test]
fn absent_observation_is_skipped_and_never_reported_as_clean() {
    let rules = default_rules();
    let result = run(&SystemFacts::empty(), &rules);

    assert!(result.findings.is_empty());
    assert_eq!(result.evaluated, 0);
    assert_eq!(result.skipped, rules.len());
    assert_eq!(result.accounted_rules(), rules.len());

    let note = result.coverage_note();
    assert!(note.contains("skipped"), "{note}");
    assert!(note.contains("no evidence"), "{note}");
    assert!(!note.to_lowercase().contains("all clear"), "{note}");
    assert!(!note.to_lowercase().contains("no problems"), "{note}");
    assert!(!note.to_lowercase().contains("passed"), "{note}");
    assert!(!note.to_lowercase().contains("healthy"), "{note}");
}

#[test]
fn blank_observation_is_not_usable_evidence() {
    let rules = [rule(
        "CORE_BLANK",
        "blank_check",
        FindingSeverity::Attention,
    )];
    let result = run(&facts(vec![observed("blank_check", "   \t ")]), &rules);

    assert!(result.findings.is_empty());
    assert_eq!(result.evaluated, 0);
    assert_eq!(result.skipped, 1);
}

#[test]
fn failed_observation_does_not_mask_usable_evidence() {
    let checks = vec![failed("alpha"), observed("alpha", "usable evidence")];
    let result = run(
        &facts(checks),
        &[rule("CORE_ALPHA", "alpha", FindingSeverity::Attention)],
    );

    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].observed, "usable evidence");
    assert_eq!(result.evaluated, 1);
    assert_eq!(result.skipped, 0);
}

#[test]
fn duplicate_observations_resolve_independently_of_input_order() {
    let rules = [rule(
        "CORE_DUPLICATE",
        "duplicate",
        FindingSeverity::Attention,
    )];
    let forward = facts(vec![
        observed("duplicate", "zebra"),
        observed("duplicate", "alpha"),
    ]);
    let mut reversed_checks = forward.checks.clone();
    reversed_checks.reverse();
    let backward = facts(reversed_checks);

    let first = run(&forward, &rules);
    let second = run(&backward, &rules);

    assert_eq!(first, second);
    assert_eq!(first.findings.len(), 1);
    assert_eq!(first.findings[0].observed, "alpha");
}

#[test]
fn run_without_rules_evaluates_nothing_and_says_so() {
    let result = run(&facts(vec![observed("alpha", "evidence")]), &[]);

    assert!(result.findings.is_empty());
    assert_eq!(result.evaluated, 0);
    assert_eq!(result.skipped, 0);
    assert_eq!(result.accounted_rules(), 0);
    let note = result.coverage_note();
    assert!(note.contains("no diagnostic rules"), "{note}");
    assert!(!note.to_lowercase().contains("healthy"), "{note}");
    assert!(!note.to_lowercase().contains("passed"), "{note}");
}

// --- deterministic ordering -------------------------------------------------

#[test]
fn findings_are_ordered_by_severity_then_code() {
    let rules = [
        rule("CORE_INFO", "info_check", FindingSeverity::Info),
        rule("CORE_Z_CRITICAL", "z_check", FindingSeverity::Critical),
        rule(
            "CORE_ATTENTION",
            "attention_check",
            FindingSeverity::Attention,
        ),
        rule("CORE_A_CRITICAL", "a_check", FindingSeverity::Critical),
    ];
    let checks = vec![
        observed("info_check", "info"),
        observed("z_check", "z"),
        observed("attention_check", "attention"),
        observed("a_check", "a"),
    ];

    let result = run(&facts(checks), &rules);
    assert_eq!(
        codes(&result),
        vec![
            "CORE_A_CRITICAL",
            "CORE_Z_CRITICAL",
            "CORE_ATTENTION",
            "CORE_INFO"
        ]
    );
    assert_eq!(result.count_by_severity(FindingSeverity::Critical), 2);
    assert_eq!(result.count_by_severity(FindingSeverity::Attention), 1);
    assert_eq!(result.count_by_severity(FindingSeverity::Info), 1);
}

#[test]
fn duplicate_rule_codes_have_a_total_order() {
    let rules = [
        rule("CORE_SAME", "zeta_check", FindingSeverity::Attention),
        rule("CORE_SAME", "alpha_check", FindingSeverity::Attention),
    ];
    let checks = vec![
        observed("zeta_check", "zeta"),
        observed("alpha_check", "alpha"),
    ];

    let result = run(&facts(checks), &rules);
    assert_eq!(result.findings.len(), 2);
    assert_eq!(result.findings[0].rule.check_id, "alpha_check");
    assert_eq!(result.findings[1].rule.check_id, "zeta_check");
}

#[test]
fn rule_and_observation_order_do_not_change_the_run() {
    let mut rules = vec![
        rule("CORE_C", "c_check", FindingSeverity::Info),
        rule("CORE_A", "a_check", FindingSeverity::Critical),
        rule("CORE_B", "b_check", FindingSeverity::Attention),
    ];
    let mut checks = vec![
        observed("a_check", "a"),
        observed("b_check", "b"),
        observed("c_check", "c"),
    ];

    let forward = run(&facts(checks.clone()), &rules);
    rules.reverse();
    checks.reverse();
    let backward = run(&facts(checks), &rules);

    assert_eq!(forward, backward);
}

#[test]
fn rule_finding_quotes_observation_evidence_and_source() {
    let mut check = observed("quote_check", "  4.2 GiB free of 500 GiB  ");
    check.source = "  statfs on /  ".to_string();

    let result = run(
        &facts(vec![check]),
        &[rule(
            "CORE_QUOTE",
            "quote_check",
            FindingSeverity::Attention,
        )],
    );

    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].observed, "4.2 GiB free of 500 GiB");
    assert_eq!(result.findings[0].source, "statfs on /");
    assert_eq!(result.findings[0].rule.code, "CORE_QUOTE");
}

// --- accounting and counters ------------------------------------------------

#[test]
fn every_supplied_rule_is_accounted_for_exactly_once() {
    let rules = default_rules();
    let checks: Vec<CheckObservation> = rules
        .iter()
        .take(4)
        .map(|rule| observed(rule.check_id, "synthetic evidence"))
        .collect();
    let expected_evaluated = checks.len();

    let result = run(&facts(checks), &rules);
    assert_eq!(result.evaluated, expected_evaluated);
    assert_eq!(
        result.skipped,
        rules.len().saturating_sub(expected_evaluated)
    );
    assert_eq!(result.accounted_rules(), rules.len());
    assert_eq!(result.findings.len(), result.evaluated);
}

#[test]
fn counter_arithmetic_saturates_at_usize_max() {
    let synthetic = DiagnosticRun {
        findings: Vec::new(),
        skipped: usize::MAX,
        evaluated: 1,
    };
    assert_eq!(synthetic.accounted_rules(), usize::MAX);

    let both_max = DiagnosticRun {
        findings: Vec::new(),
        skipped: usize::MAX,
        evaluated: usize::MAX,
    };
    assert_eq!(both_max.accounted_rules(), usize::MAX);
}

// --- repair plans -----------------------------------------------------------

#[test]
fn no_rule_in_this_build_is_repairable() {
    let rules = default_rules();
    let checks: Vec<CheckObservation> = rules
        .iter()
        .map(|rule| observed(rule.check_id, "synthetic evidence"))
        .collect();
    let result = run(&facts(checks), &rules);

    assert_eq!(result.evaluated, rules.len());
    assert_eq!(result.findings.len(), rules.len());
    assert!(
        result.findings.iter().all(|finding| !finding.repairable),
        "no default rule may claim a repair in this build"
    );

    let custom = run(
        &facts(vec![observed("custom_check", "custom evidence")]),
        &[rule("CORE_CUSTOM", "custom_check", FindingSeverity::Info)],
    );
    assert_eq!(custom.findings.len(), 1);
    assert!(
        !custom.findings[0].repairable,
        "caller-supplied rules cannot become repairable either"
    );
}

#[test]
fn plan_repairs_is_inspection_only_and_states_its_limits() {
    let rules = [
        rule("CORE_ONE", "one_check", FindingSeverity::Critical),
        rule("CORE_TWO", "two_check", FindingSeverity::Attention),
    ];
    let result = run(
        &facts(vec![
            observed("one_check", "one evidence"),
            observed("two_check", "two evidence"),
        ]),
        &rules,
    );
    let plan = plan_repairs(&result);

    assert_eq!(plan.steps.len(), result.findings.len());
    assert_eq!(plan.steps.len(), 2);
    assert!(
        plan.steps
            .iter()
            .all(|step| step.action == RepairAction::InspectOnly),
        "this build may only propose inspections"
    );
    assert!(plan.steps.iter().all(|step| !step.action.changes_system()));
    assert!(plan.steps.iter().all(|step| !step.requires_privilege));
    assert!(plan.steps.iter().all(|step| step.reversible));
    assert!(plan
        .steps
        .iter()
        .all(|step| step.description.contains("changes nothing")));
    assert_eq!(plan.steps[0].rule_code, "CORE_ONE");
    assert!(plan.note.contains("performs no repairs"), "{}", plan.note);
    assert!(plan.note.contains("typed safety pipeline"), "{}", plan.note);

    let empty_plan = plan_repairs(&run(&SystemFacts::empty(), &[]));
    assert!(empty_plan.steps.is_empty());
    assert!(empty_plan.note.contains("performs no repairs"));
}

// --- update fixer -----------------------------------------------------------

#[test]
fn unknown_update_state_is_neither_healthy_nor_broken() {
    let findings = update_findings(&[update_check("Windows Update", UpdateState::Unknown)]);
    assert_eq!(findings.len(), 1);
    let finding = &findings[0];

    assert_eq!(finding.state, UpdateState::Unknown);
    assert_eq!(finding.code, "UPDATE_UNKNOWN");
    assert_ne!(finding.code, "UPDATE_HEALTHY");
    assert_ne!(finding.state, UpdateState::Healthy);
    assert!(
        finding.summary.contains("could not determine"),
        "{}",
        finding.summary
    );
    assert!(
        finding
            .summary
            .contains("neither a healthy result nor a confirmed failure"),
        "{}",
        finding.summary
    );
    assert!(
        !finding.summary.contains("reported a healthy state"),
        "{}",
        finding.summary
    );
    assert!(
        !finding.summary.contains("reported error"),
        "{}",
        finding.summary
    );
}

#[test]
fn update_error_without_a_code_says_the_code_was_not_reported() {
    let without_code = update_findings(&[update_check(
        "Windows Update",
        UpdateState::Error { code: None },
    )]);
    assert_eq!(without_code.len(), 1);
    assert_eq!(without_code[0].code, "UPDATE_ERROR");
    assert!(
        without_code[0]
            .summary
            .contains("did not report an error code"),
        "{}",
        without_code[0].summary
    );
    assert!(
        !without_code[0].summary.contains("0x"),
        "{}",
        without_code[0].summary
    );

    let with_code = update_findings(&[update_check(
        "Windows Update",
        UpdateState::Error {
            code: Some("0x80070422".to_string()),
        },
    )]);
    assert_eq!(with_code[0].code, "UPDATE_ERROR");
    assert!(
        with_code[0].summary.contains("0x80070422"),
        "{}",
        with_code[0].summary
    );
}

#[test]
fn update_findings_report_every_component_including_healthy() {
    let checks = vec![
        update_check("Windows Update", UpdateState::Healthy),
        update_check("Microsoft Store", UpdateState::Stopped),
        update_check(
            "Defender Definitions",
            UpdateState::Error {
                code: Some("0x1".to_string()),
            },
        ),
        update_check("OneDrive", UpdateState::Error { code: None }),
        update_check("Optional Drivers", UpdateState::Unknown),
    ];

    let findings = update_findings(&checks);
    assert_eq!(findings.len(), checks.len());

    let findings_by_code: BTreeSet<&str> = findings.iter().map(|finding| finding.code).collect();
    assert!(findings_by_code.contains("UPDATE_HEALTHY"));
    assert!(findings_by_code.contains("UPDATE_STOPPED"));
    assert!(findings_by_code.contains("UPDATE_ERROR"));
    assert!(findings_by_code.contains("UPDATE_UNKNOWN"));
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding.code == "UPDATE_ERROR")
            .count(),
        2
    );

    let healthy = findings
        .iter()
        .find(|finding| finding.state == UpdateState::Healthy);
    assert!(healthy.is_some(), "healthy components must be reported");
    if let Some(healthy) = healthy {
        assert!(
            healthy.summary.contains("reported a healthy state"),
            "{}",
            healthy.summary
        );
    }

    let stopped = findings
        .iter()
        .find(|finding| finding.state == UpdateState::Stopped);
    assert!(stopped.is_some(), "stopped components must be reported");
    if let Some(stopped) = stopped {
        assert!(
            stopped.summary.contains("reported as stopped"),
            "{}",
            stopped.summary
        );
    }
}

#[test]
fn update_findings_are_deterministic_and_sorted_by_component() {
    let mut checks = vec![
        update_check("Windows Update", UpdateState::Stopped),
        update_check("Defender Definitions", UpdateState::Healthy),
        update_check("Microsoft Store", UpdateState::Unknown),
    ];
    let forward = update_findings(&checks);
    checks.reverse();
    let backward = update_findings(&checks);

    assert_eq!(forward, backward);
    let components: Vec<&str> = forward
        .iter()
        .map(|finding| finding.component.as_str())
        .collect();
    assert_eq!(
        components,
        vec!["Defender Definitions", "Microsoft Store", "Windows Update"]
    );
}

// --- serialization ----------------------------------------------------------

#[test]
fn diagnostic_run_and_plan_serialize_with_camel_case_fields() {
    let result = run(
        &facts(vec![observed("alpha", "alpha evidence")]),
        &[rule("CORE_ALPHA", "alpha", FindingSeverity::Attention)],
    );
    let run_json = serde_json::to_string(&result).expect("diagnostic run serializes");
    assert!(run_json.contains("\"skipped\":0"), "{run_json}");
    assert!(run_json.contains("\"evaluated\":1"), "{run_json}");
    assert!(run_json.contains("\"repairable\":false"), "{run_json}");
    assert!(
        run_json.contains("\"recommendedIntervalDays\":null"),
        "{run_json}"
    );
    assert!(run_json.contains("\"ATTENTION\""), "{run_json}");

    let plan = plan_repairs(&result);
    let plan_json = serde_json::to_string(&plan).expect("repair plan serializes");
    assert!(plan_json.contains("\"INSPECT_ONLY\""), "{plan_json}");
    assert!(
        plan_json.contains("\"ruleCode\":\"CORE_ALPHA\""),
        "{plan_json}"
    );
    assert!(
        plan_json.contains("\"requiresPrivilege\":false"),
        "{plan_json}"
    );
}

#[test]
fn update_findings_serialize_unknown_as_its_own_state() {
    let findings = update_findings(&[update_check("Windows Update", UpdateState::Unknown)]);
    let json = serde_json::to_string(&findings).expect("update findings serialize");
    assert!(json.contains("\"UPDATE_UNKNOWN\""), "{json}");
    assert!(json.contains("\"UNKNOWN\""), "{json}");
    assert!(!json.contains("\"HEALTHY\""), "{json}");
}

// --- architecture guard -----------------------------------------------------

#[test]
fn fixer_source_contains_no_io_execution_or_panic_paths() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/fixer.rs");
    let text = std::fs::read_to_string(path).expect("fixer source must be readable");

    for forbidden in [
        "std::fs::",
        "std::process",
        "std::net",
        "rusqlite",
        "Command::new",
        "winreg",
        "std::os::windows",
    ] {
        assert!(
            !text.contains(forbidden),
            "fixer.rs contains forbidden API {forbidden}"
        );
    }
    for forbidden in [
        ".unwrap()",
        ".expect(",
        "panic!(",
        "todo!(",
        "unimplemented!(",
    ] {
        assert!(
            !text.contains(forbidden),
            "fixer.rs contains forbidden panic path {forbidden}"
        );
    }
}

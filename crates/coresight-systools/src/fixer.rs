//! Rule-based diagnostics and **proposal-only** repair planning.
//!
//! jv16 PowerTools advertises a "System Fixer" that scans for "common Windows
//! and system errors" and fixes them. CoreSight must not repeat that claim. A
//! fix is only honest when the condition, the action, and the result can all be
//! verified, and this build can verify none of the action. So the advertised
//! surface is split in two:
//!
//! * [`run`] evaluates a caller-supplied [`DiagnosticRule`] set against
//!   caller-supplied [`CheckObservation`]s and returns one [`RuleFinding`] per
//!   rule that had usable evidence.
//! * [`plan_repairs`] converts those findings into [`RepairStep`]s. Every step
//!   is [`RepairAction::InspectOnly`]: a proposal to review evidence, never a
//!   change. This build contains no repair executor.
//!
//! ## Contract: a skipped check is not a passing check
//!
//! A rule is **evaluated** only when an observation with its `check_id` exists,
//! did not fail, and carries non-blank evidence text. Otherwise the rule is
//! **skipped** and counted in [`DiagnosticRun::skipped`]. A skipped rule
//! produced no finding because CoreSight had nothing to reason about — not
//! because the system is healthy. [`DiagnosticRun::coverage_note`] says so in
//! words, and no formatter here renders a skipped rule as "passed", "clean", or
//! "no problem".
//!
//! This module reports *triggered conditions*. A collector that checked a rule
//! and found nothing to report still cannot use this module to certify absence:
//! absence of evidence is exactly what `skipped` counts. Certifying a clean
//! result needs a separate covered-clean signal from the collector, and this
//! build does not invent one.
//!
//! ## Contract: rules describe evidence, not causes or fixes
//!
//! A [`DiagnosticRule`] says what a check looks at; it never says what caused
//! the condition or what would remove it. The evidence string is carried into
//! the finding with only surrounding whitespace trimmed — never parsed,
//! summarised, or replaced. CoreSight cannot see the registry, service
//! configuration, or file contents, so no rule in [`default_rules`] pretends
//! to repair them.
//!
//! ## Contract: no repair capability is claimed
//!
//! [`DiagnosticRule`] deliberately has no `repairable` field, so a caller cannot
//! mark a rule repairable. [`RuleFinding::repairable`] is `false` for every
//! rule — default or caller-supplied — because the build has no executor.
//! [`plan_repairs`] emits [`RepairAction::InspectOnly`] steps and a note that
//! states the limit. A UI may render the shape of a repair plan; it must not
//! render a fix.
//!
//! ## Update Fixer: one row per component, including healthy ones
//!
//! [`update_findings`] is a separate, narrower view over per-component update
//! state. It maps every supplied [`UpdateCheck`] to exactly one
//! [`UpdateFinding`] — including healthy ones, so "healthy" and "not examined"
//! can never look alike — and it gives [`UpdateState::Unknown`] its own finding
//! instead of guessing in either direction.
//!
//! ## What this module deliberately does NOT do
//!
//! * No filesystem, registry, service-control, process, network, or
//!   persistence access. Facts arrive as data.
//! * No repair execution, no privilege escalation, no installer, no service
//!   restart, no setting change.
//! * No interpretation of evidence text: the `observed` string is quoted, not
//!   parsed. A collector that wants a rule to fire must decide that the
//!   condition is present and supply the evidence.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::system::FindingSeverity;

/// The caller-supplied facts one diagnostic pass may consider.
///
/// Facts are data, not instructions: the engine never asks the machine for
/// anything and never writes back. An empty `checks` list means no check had
/// anything to report — which is *not* the same as every check passing. See
/// [`DiagnosticRun::skipped`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemFacts {
    /// One row per check that has evidence to contribute.
    ///
    /// Collectors emit a row only when they have something to show. A
    /// collector that ran a check and found the condition absent contributes
    /// no row; this crate deliberately cannot turn that silence into a pass.
    pub checks: Vec<CheckObservation>,
}

impl SystemFacts {
    /// Facts with no observations at all.
    ///
    /// Use this when no collector ran. Every rule then counts as skipped,
    /// which is the honest description of a scan that examined nothing.
    pub fn empty() -> Self {
        SystemFacts { checks: Vec::new() }
    }
}

/// One check's evidence, as reported by a collector.
///
/// `check_id` is `&'static str` on purpose: check ids are compile-time
/// constants defined by the collecting adapter, so a report cannot carry an
/// invented id at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckObservation {
    /// Stable id of the check that produced this, e.g. `disk_free`.
    pub check_id: &'static str,
    /// Observed value rendered for evidence, e.g. `4.2 GiB free of 500 GiB`.
    ///
    /// `None` means the check produced no evidence. A blank or whitespace-only
    /// string is treated the same way: it cannot be shown to a user as proof.
    pub observed: Option<String>,
    /// How this observation was obtained, e.g. `statfs on /`.
    pub source: String,
    /// True when the check could not run (permission, unsupported, error).
    ///
    /// A failed check is never allowed to look like a passing one: it
    /// contributes no evidence, so its rule is counted as skipped.
    pub failed: bool,
}

/// One diagnostic rule: which check it reads, how much attention its evidence
/// warrants, and why the rule exists.
///
/// The type intentionally has no `repairable` field. Repairability is a
/// property of the build, not of a rule, and this build has no repair
/// executor; see [`RuleFinding::repairable`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRule {
    /// Stable machine code, e.g. `CORE_DISK_FREE_LOW`.
    pub code: &'static str,
    /// Stable id of the check that supplies this rule's evidence.
    pub check_id: &'static str,
    /// How much attention evidence for this rule warrants.
    pub severity: FindingSeverity,
    /// Plain-language description of what the rule looks for.
    pub description: &'static str,
    /// Minimum days between runs, if the rule is expensive. Optional.
    ///
    /// `None` means the rule is cheap enough to run on every pass. A value is
    /// a scheduling hint only; nothing in this crate reads a clock.
    pub recommended_interval_days: Option<u32>,
}

/// The small, documented rule set CoreSight ships.
///
/// Each rule is evidence-bound: it fires only when a collector supplies a
/// usable observation for its `check_id`. The set is deliberately small
/// because every entry is a promise that CoreSight can show the user the
/// evidence behind it.
///
/// | code | check id | severity | why it exists |
/// | --- | --- | --- | --- |
/// | `CORE_DISK_FREE_LOW` | `disk_free` | attention | Free space is directly measurable, and updates, installers, and logs need room. The rule quotes the collector's figures and does not guess what fills the disk. |
/// | `CORE_BACKUP_NOT_OBSERVED` | `backup_last_success` | attention | A missing success record is evidence about records, not proof that no backup exists. Weekly, because enumerating backup history is comparatively expensive. |
/// | `CORE_OS_UPDATE_PENDING` | `os_update_pending` | attention | An available update is a state, not a vulnerability verdict. CoreSight cannot see what the update contains. |
/// | `CORE_OS_BUILD_AGED` | `os_build_age` | attention | Build age compared with the window the collector documents is a leading indicator that fixes may no longer arrive. Monthly, because builds change slowly. |
/// | `CORE_STARTUP_LOAD_HIGH` | `startup_item_count` | attention | The count is exact and actionable, but the rule names no item: this crate has no startup-manager authority and cannot say which entry is safe to remove. |
/// | `CORE_UPDATE_SERVICE_STOPPED` | `update_service_state` | critical | A stopped update service is directly observed state that stops automatic update delivery, so it warrants prompt attention. The rule does not claim why it stopped. |
/// | `CORE_UPDATE_ATTEMPT_FAILED` | `update_last_result` | attention | A failed update attempt with a code is verifiable; the rule quotes the code without interpreting it, because meanings vary by component and version. |
/// | `CORE_RESTART_PENDING` | `reboot_pending` | attention | A pending restart is a state, not a fault: updates or drivers are staged but not fully applied. The rule promises nothing about what the restart changes. |
/// | `CORE_SECURITY_PROTECTION_OFF` | `security_protection_state` | critical | A registered security product reporting real-time protection off is checkable, important evidence. The rule does not scan for malware or verify the product. |
pub fn default_rules() -> Vec<DiagnosticRule> {
    vec![
        DiagnosticRule {
            code: "CORE_DISK_FREE_LOW",
            check_id: "disk_free",
            severity: FindingSeverity::Attention,
            description: "Free space on a fixed volume was below the reserve threshold the \
                          collector applies; the rule quotes the reported figures and does not \
                          guess what is using the space.",
            recommended_interval_days: None,
        },
        DiagnosticRule {
            code: "CORE_BACKUP_NOT_OBSERVED",
            check_id: "backup_last_success",
            severity: FindingSeverity::Attention,
            description: "No successful backup of user data was found in the collector's \
                          backup-history window; the rule reports a missing success record and \
                          does not claim that no backup exists.",
            recommended_interval_days: Some(7),
        },
        DiagnosticRule {
            code: "CORE_OS_UPDATE_PENDING",
            check_id: "os_update_pending",
            severity: FindingSeverity::Attention,
            description: "The operating system reports an update that is available or pending \
                          installation; this is an update-state observation, not a claim that a \
                          specific vulnerability is present.",
            recommended_interval_days: Some(1),
        },
        DiagnosticRule {
            code: "CORE_OS_BUILD_AGED",
            check_id: "os_build_age",
            severity: FindingSeverity::Attention,
            description: "The installed OS build is older than the age window the collector \
                          documents for that version; the rule repeats the comparison and does \
                          not independently assert vendor support status.",
            recommended_interval_days: Some(30),
        },
        DiagnosticRule {
            code: "CORE_STARTUP_LOAD_HIGH",
            check_id: "startup_item_count",
            severity: FindingSeverity::Attention,
            description: "The number of enabled startup items exceeds the count threshold the \
                          collector documents; the rule reports the count and does not decide \
                          which item, if any, is safe to remove.",
            recommended_interval_days: Some(30),
        },
        DiagnosticRule {
            code: "CORE_UPDATE_SERVICE_STOPPED",
            check_id: "update_service_state",
            severity: FindingSeverity::Critical,
            description: "The operating system's update service was reported as stopped; while \
                          stopped it does not apply updates automatically, and the rule does not \
                          claim why it stopped.",
            recommended_interval_days: Some(1),
        },
        DiagnosticRule {
            code: "CORE_UPDATE_ATTEMPT_FAILED",
            check_id: "update_last_result",
            severity: FindingSeverity::Attention,
            description: "The most recent update attempt was reported as failed, with an error \
                          code when one was supplied; the rule quotes the failure and does not \
                          interpret the code.",
            recommended_interval_days: Some(1),
        },
        DiagnosticRule {
            code: "CORE_RESTART_PENDING",
            check_id: "reboot_pending",
            severity: FindingSeverity::Attention,
            description: "The operating system reports that a restart is pending to finish \
                          applying updates or drivers; the rule repeats that state and does not \
                          claim what will change after the restart.",
            recommended_interval_days: Some(1),
        },
        DiagnosticRule {
            code: "CORE_SECURITY_PROTECTION_OFF",
            check_id: "security_protection_state",
            severity: FindingSeverity::Critical,
            description: "A registered security product reports that real-time protection is off; \
                          the rule quotes that report and does not scan for malware or verify the \
                          product.",
            recommended_interval_days: None,
        },
    ]
}

/// One rule that produced a finding, with the evidence attached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleFinding {
    /// The rule that produced this finding.
    pub rule: DiagnosticRule,
    /// The evidence text supplied by the collector, with surrounding
    /// whitespace trimmed and nothing else changed.
    pub observed: String,
    /// How the evidence was obtained, copied from the observation.
    pub source: String,
    /// Whether a repair exists in this build. Always `false`: there is no
    /// repair executor, so no rule — default or caller-supplied — can be
    /// repaired here.
    pub repairable: bool,
}

/// The result of one diagnostic pass.
///
/// `evaluated` and `skipped` count rules, not observations. Every supplied
/// rule is accounted for exactly once, so a zero-finding run can always be
/// read together with how many rules actually had evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRun {
    /// Findings ordered by severity descending, then rule code ascending, with
    /// total tie-breaks on the remaining rule and evidence fields.
    pub findings: Vec<RuleFinding>,
    /// Rules that produced no finding because their check failed, was absent,
    /// or returned no usable evidence.
    ///
    /// **A skipped check is not a passing check.** This number must be shown
    /// next to the findings, not folded away; it is the difference between
    /// "nothing was wrong" and "we could not tell".
    pub skipped: usize,
    /// Rules that ran and produced a finding.
    pub evaluated: usize,
}

impl DiagnosticRun {
    /// Rules accounted for, saturating at `usize::MAX`.
    ///
    /// For a run produced by [`run`] this equals the number of supplied rules.
    pub fn accounted_rules(&self) -> usize {
        self.evaluated.saturating_add(self.skipped)
    }

    /// How many findings carry one severity, saturating at `usize::MAX`.
    pub fn count_by_severity(&self, severity: FindingSeverity) -> usize {
        self.findings
            .iter()
            .filter(|finding| finding.rule.severity == severity)
            .fold(0usize, |total, _| total.saturating_add(1))
    }

    /// A one-line coverage sentence for the UI.
    ///
    /// It never describes a skipped rule as passed. When a run has zero
    /// findings and zero skipped rules (only possible when no rules were
    /// supplied or a caller built the run by hand), it still does not claim
    /// the system is healthy — the engine has no way to know that.
    pub fn coverage_note(&self) -> String {
        let accounted = self.accounted_rules();
        if accounted == 0 {
            return "no diagnostic rules were supplied, so nothing was evaluated".to_string();
        }
        if self.skipped == 0 {
            format!(
                "{accounted} diagnostic rules accounted for: {} evaluated, none skipped",
                self.evaluated
            )
        } else {
            format!(
                "{accounted} diagnostic rules accounted for: {} evaluated, {} skipped because \
                 the check failed, was absent, or returned no evidence — a skipped check is not \
                 a passing check",
                self.evaluated, self.skipped
            )
        }
    }
}

/// Run a rule set over caller-supplied facts.
///
/// A rule runs if and only if a [`CheckObservation`] with its `check_id` exists,
/// `failed` is `false`, and `observed` holds non-blank text. Any other case is
/// counted in [`DiagnosticRun::skipped`]; it is never silently dropped and
/// never reported as "no problem found".
///
/// The evidence string is not parsed. The collector decides that a rule's
/// condition is present; this function quotes what the collector supplied.
/// Ordering is total (severity descending, then code, then remaining fields),
/// so permuting either the rules or the observations cannot change the run.
pub fn run(facts: &SystemFacts, rules: &[DiagnosticRule]) -> DiagnosticRun {
    let evidence = usable_evidence(&facts.checks);
    let mut findings: Vec<RuleFinding> = Vec::new();
    let mut skipped = 0usize;
    let mut evaluated = 0usize;

    for rule in rules {
        match evidence.get(rule.check_id) {
            Some(observation) => {
                evaluated = evaluated.saturating_add(1);
                findings.push(RuleFinding {
                    rule: rule.clone(),
                    observed: observation.observed.to_string(),
                    source: observation.source.to_string(),
                    repairable: false,
                });
            }
            None => {
                skipped = skipped.saturating_add(1);
            }
        }
    }

    sort_rule_findings(&mut findings);

    DiagnosticRun {
        findings,
        skipped,
        evaluated,
    }
}

/// What a proposed repair step would do.
///
/// The names are explicit because a future executor must classify every step
/// before it may run. Only [`RepairAction::InspectOnly`] is proposed by this
/// build; the other variants exist so a later, separately authorized phase can
/// classify its actions through the same type instead of inventing a second
/// vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RepairAction {
    /// Read and present evidence; change nothing. The only action this build
    /// can propose.
    InspectOnly,
    /// Delete a rebuildable cache. Not used in this build.
    ClearCache,
    /// Stop and start a service. Not used in this build.
    RestartService,
    /// Change a persisted setting. Not used in this build.
    ChangeSetting,
    /// Install a supplied update package. Not used in this build.
    InstallUpdate,
    /// Copy data to a backup location, then delete the original. Not used in
    /// this build.
    BackUpThenClear,
}

impl RepairAction {
    /// Stable machine code used in reports.
    pub fn code(self) -> &'static str {
        match self {
            RepairAction::InspectOnly => "INSPECT_ONLY",
            RepairAction::ClearCache => "CLEAR_CACHE",
            RepairAction::RestartService => "RESTART_SERVICE",
            RepairAction::ChangeSetting => "CHANGE_SETTING",
            RepairAction::InstallUpdate => "INSTALL_UPDATE",
            RepairAction::BackUpThenClear => "BACK_UP_THEN_CLEAR",
        }
    }

    /// Whether the action would modify the system if it were ever executed.
    ///
    /// Only [`RepairAction::InspectOnly`] returns `false`. This is a property
    /// of the action kind, not evidence that an executor exists: none does.
    pub fn changes_system(self) -> bool {
        !matches!(self, RepairAction::InspectOnly)
    }
}

/// One proposed step in a repair plan.
///
/// In this build every step is an inspection proposal. The metadata fields
/// describe what a future executor would need to know, not what happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairStep {
    /// Code of the rule whose finding prompted this step.
    pub rule_code: &'static str,
    /// What the step would do. Always [`RepairAction::InspectOnly`] here.
    pub action: RepairAction,
    /// Plain-language description of the step, including that it changes
    /// nothing.
    pub description: String,
    /// Whether the proposed action could be undone. An inspection changes
    /// nothing, so it is trivially reversible.
    pub reversible: bool,
    /// Whether the proposed action would need elevated rights. An inspection
    /// of already-collected evidence needs none.
    pub requires_privilege: bool,
}

/// A repair plan: a proposal the UI can render, not a capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairPlan {
    /// One inspection step per finding, in the run's deterministic order.
    pub steps: Vec<RepairStep>,
    /// A plain statement of what this plan can and cannot do.
    pub note: String,
}

/// The fixed note attached to every plan. It names the real pipeline stages
/// CoreSight already defines in `coresight-capabilities::safety`, so a future
/// implementation has a contract to satisfy rather than a slogan to repeat.
const REPAIR_PLAN_NOTE: &str = concat!(
    "This build performs no repairs. Every step is an inspection proposal only: CoreSight has ",
    "no repair executor, no registry writer, no service controller, no installer, and no ",
    "process runner, so it cannot change the system. Any future repair must pass CoreSight's ",
    "typed safety pipeline — OBSERVE, ANALYZE, RECOMMEND, PREVIEW, VALIDATE, EXECUTE, VERIFY, ",
    "ROLLBACK — with a typed action kind, an explicit user decision, and post-action ",
    "verification. None of that exists in this build, and no step may be presented as a fix.",
);

/// Turn a diagnostic run into a repair plan.
///
/// This is a shape for the UI, not a capability. Every step is
/// [`RepairAction::InspectOnly`] and the returned note says plainly that this
/// build performs no repairs. A future repair must pass the typed safety
/// pipeline before it may run; this function cannot construct such a step.
///
/// Steps mirror findings one-to-one, in the run's deterministic order, so a
/// plan can never propose more than the run evidenced.
pub fn plan_repairs(run: &DiagnosticRun) -> RepairPlan {
    let steps: Vec<RepairStep> = run
        .findings
        .iter()
        .map(|finding| RepairStep {
            rule_code: finding.rule.code,
            action: RepairAction::InspectOnly,
            description: format!(
                "Review the recorded evidence for {} (check '{}') with the user. This step only \
                 reads evidence that was already collected and changes nothing.",
                finding.rule.code, finding.rule.check_id
            ),
            reversible: true,
            requires_privilege: false,
        })
        .collect();

    RepairPlan {
        steps,
        note: REPAIR_PLAN_NOTE.to_string(),
    }
}

/// One update component's state as reported by a collector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// Component name, e.g. `Windows Update`.
    pub component: String,
    /// The state the collector observed.
    pub state: UpdateState,
    /// The evidence text behind the state, e.g. `service status: stopped`.
    pub evidence: String,
    /// How the state was obtained, e.g. `ServiceController query`.
    pub source: String,
}

/// The state of one update component.
///
/// `Unknown` is a first-class state, not a synonym for either healthy or
/// broken: the collector could not determine the state, and CoreSight will not
/// turn that into a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum UpdateState {
    /// The component reported a healthy state.
    Healthy,
    /// The component was reported as stopped.
    Stopped,
    /// The component reported an error, with a code when one was supplied.
    Error {
        /// The error code exactly as reported. `None` means the collector did
        /// not report one; it is never replaced with a guessed code.
        code: Option<String>,
    },
    /// The collector could not determine the state.
    Unknown,
}

impl UpdateState {
    /// Stable machine code used in reports.
    pub fn code(&self) -> &'static str {
        match self {
            UpdateState::Healthy => "HEALTHY",
            UpdateState::Stopped => "STOPPED",
            UpdateState::Error { .. } => "ERROR",
            UpdateState::Unknown => "UNKNOWN",
        }
    }

    /// Short human label for a UI row.
    pub fn label(&self) -> &'static str {
        match self {
            UpdateState::Healthy => "Healthy",
            UpdateState::Stopped => "Stopped",
            UpdateState::Error { .. } => "Error",
            UpdateState::Unknown => "Unknown",
        }
    }
}

/// One update component's finding.
///
/// `code` is derived from the state, so `UPDATE_UNKNOWN` can never be rendered
/// as `UPDATE_HEALTHY` by a downstream formatter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFinding {
    /// Stable machine code of the finding kind: `UPDATE_HEALTHY`,
    /// `UPDATE_STOPPED`, `UPDATE_ERROR`, or `UPDATE_UNKNOWN`.
    pub code: &'static str,
    /// The component as reported, trimmed. Empty input stays empty and is
    /// described as an unnamed component in the summary.
    pub component: String,
    /// The exact state, copied without interpretation.
    pub state: UpdateState,
    /// One-sentence summary that never upgrades `Unknown` to healthy or
    /// broken, and never invents an error code.
    pub summary: String,
    /// The evidence text supplied by the collector, trimmed.
    pub evidence: String,
    /// How the state was obtained, trimmed.
    pub source: String,
}

/// Report every supplied update component exactly once.
///
/// Unlike [`run`], which reports only triggered conditions, this function
/// reports healthy components too, so "healthy" and "not examined" can never
/// look alike. `Unknown` gets its own `UPDATE_UNKNOWN` finding whose summary
/// says the state could not be determined; `Error { code: None }` says the code
/// was not reported rather than inventing one.
///
/// Ordering is deterministic: component name (case-insensitive, then exact),
/// then error/stopped/unknown/healthy rank for duplicate components, then the
/// error code, source, and evidence as total tie-breaks.
pub fn update_findings(checks: &[UpdateCheck]) -> Vec<UpdateFinding> {
    let mut findings: Vec<UpdateFinding> = checks
        .iter()
        .map(|check| {
            let component = check.component.trim().to_string();
            let label = if component.is_empty() {
                "Unnamed update component".to_string()
            } else {
                component.clone()
            };
            let (code, summary) = update_summary(&label, &check.state);
            UpdateFinding {
                code,
                component,
                state: check.state.clone(),
                summary,
                evidence: check.evidence.trim().to_string(),
                source: check.source.trim().to_string(),
            }
        })
        .collect();

    sort_update_findings(&mut findings);
    findings
}

/// The stable code and honest sentence for one update state.
fn update_summary(label: &str, state: &UpdateState) -> (&'static str, String) {
    match state {
        UpdateState::Healthy => (
            "UPDATE_HEALTHY",
            format!("{label}: reported a healthy state."),
        ),
        UpdateState::Stopped => (
            "UPDATE_STOPPED",
            format!(
                "{label}: reported as stopped, so it is not applying updates automatically. \
                 CoreSight observed the state and did not determine why it stopped."
            ),
        ),
        UpdateState::Error { code: Some(code) } => (
            "UPDATE_ERROR",
            format!(
                "{label}: reported error code {code}. The code is quoted as supplied and was \
                 not interpreted by CoreSight."
            ),
        ),
        UpdateState::Error { code: None } => (
            "UPDATE_ERROR",
            format!(
                "{label}: reported an error but did not report an error code, so CoreSight \
                 cannot name the failure."
            ),
        ),
        UpdateState::Unknown => (
            "UPDATE_UNKNOWN",
            format!(
                "{label}: CoreSight could not determine the state because the check returned no \
                 usable state. This is neither a healthy result nor a confirmed failure."
            ),
        ),
    }
}

/// Canonical update-finding order. Component is primary so the UI can show one
/// stable row per component; the remaining keys make duplicate components
/// deterministic.
fn sort_update_findings(findings: &mut [UpdateFinding]) {
    findings.sort_by(|left, right| {
        left.component
            .to_lowercase()
            .cmp(&right.component.to_lowercase())
            .then_with(|| left.component.cmp(&right.component))
            .then_with(|| update_state_rank(&left.state).cmp(&update_state_rank(&right.state)))
            .then_with(|| update_state_detail(&left.state).cmp(update_state_detail(&right.state)))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.evidence.cmp(&right.evidence))
    });
}

/// Most attention first; used only as a tie-break between duplicate components.
fn update_state_rank(state: &UpdateState) -> u8 {
    match state {
        UpdateState::Error { .. } => 0,
        UpdateState::Stopped => 1,
        UpdateState::Unknown => 2,
        UpdateState::Healthy => 3,
    }
}

/// The error code, when present, as a sort tie-break. Empty for other states.
fn update_state_detail(state: &UpdateState) -> &str {
    match state {
        UpdateState::Error { code } => code.as_deref().unwrap_or(""),
        _ => "",
    }
}

/// Canonical rule-finding order: severity descending, then code ascending,
/// then the remaining rule and evidence fields as total tie-breaks.
fn sort_rule_findings(findings: &mut [RuleFinding]) {
    findings.sort_by(|left, right| {
        right
            .rule
            .severity
            .cmp(&left.rule.severity)
            .then_with(|| left.rule.code.cmp(right.rule.code))
            .then_with(|| left.rule.check_id.cmp(right.rule.check_id))
            .then_with(|| left.observed.cmp(&right.observed))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.rule.description.cmp(right.rule.description))
            .then_with(|| {
                left.rule
                    .recommended_interval_days
                    .cmp(&right.rule.recommended_interval_days)
            })
    });
}

/// The usable evidence for each check id, selected deterministically.
///
/// A check contributes evidence only when it did not fail and its observed
/// text is non-blank after trimming. When the caller supplies several
/// observations for one id, the lexicographically smallest `(observed,
/// source)` pair wins, so the outcome cannot depend on input order. A failed
/// observation never masks a usable one.
fn usable_evidence(checks: &[CheckObservation]) -> BTreeMap<&'static str, Evidence<'_>> {
    let mut evidence: BTreeMap<&'static str, Evidence<'_>> = BTreeMap::new();

    for check in checks {
        if check.failed {
            continue;
        }
        let observed = match check.observed.as_deref().map(str::trim) {
            Some(text) if !text.is_empty() => text,
            _ => continue,
        };
        let candidate = Evidence {
            observed,
            source: check.source.trim(),
        };
        match evidence.entry(check.check_id) {
            Entry::Occupied(mut occupied) => {
                let existing = occupied.get_mut();
                if (candidate.observed, candidate.source) < (existing.observed, existing.source) {
                    *existing = candidate;
                }
            }
            Entry::Vacant(vacant) => {
                vacant.insert(candidate);
            }
        }
    }

    evidence
}

/// One check's borrowed, trimmed evidence.
struct Evidence<'a> {
    /// Trimmed observation text.
    observed: &'a str,
    /// Trimmed source description.
    source: &'a str,
}

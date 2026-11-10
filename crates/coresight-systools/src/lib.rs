//! CoreSight **system tools** — pure, read-only inventory and explanation for
//! three jv16 PowerTools-style surfaces, at CoreSight's honesty standard.
//!
//! jv16 PowerTools ships a startup manager with a boot timer, a "System
//! Examiner" hardware/software dump, and a process-based task manager. This
//! crate models the *facts and the reasoning* behind those surfaces. It knows
//! nothing about the machine it runs on and cannot change it.
//!
//! ## What is here
//!
//! * [`startup`] — inventories startup items and explains each one. Every
//!   finding names the evidence that produced its tier. A launch daemon and a
//!   per-user login item stay distinct types because they have different
//!   privileges.
//! * [`system`] — a structured examination report in which every fact that
//!   was not collected is `None` (unknown), coverage is counted, and
//!   [`summarize`] states incomplete coverage out loud.
//! * [`tasks`] — application-level task inventory: processes are attributed
//!   to an application identity, and processes that map to no application are
//!   counted in `unattributed_processes` instead of disappearing.
//!
//! ## Contract: inventory, never action
//!
//! **This crate cannot disable, enable, delete, stop, restart, or modify
//! anything.** There is no executor, no command runner, no registry writer,
//! no persistence, and no network client in it. [`StartupTier::SafeToDisable`]
//! is a *classification for a later, separately authorized UI action*; it is
//! not an action performed here, and this crate contains no code path that
//! could perform it.
//!
//! ## Contract: unknown is not empty, and unknown is not safe
//!
//! * An unrecognised [`StartupSource`] is [`StartupTier::OffLimits`] — never
//!   offered as safe — because disabling what cannot be classified is a
//!   guess.
//! * Every optional system fact is `Option<T>`; `None` means "not collected".
//!   It never means zero, and no formatter in this crate renders it as zero.
//! * A [`SystemReport`] is complete only when it had sources and every one
//!   succeeded. Partial reports say "incomplete" in [`summarize`].
//!
//! ## Contract: deterministic and saturating
//!
//! Reports are sorted by a total key, so permuting the input cannot change
//! the output. Every counter uses saturating arithmetic; a counter can never
//! wrap to a smaller number.
//!
//! ## What this crate deliberately does NOT do
//!
//! * No filesystem access, no registry access, no subprocess execution, no
//!   network, no persistence, no clock, no environment inspection.
//! * No startup mutation of any kind, no "fix" action, and no diagnostic
//!   claim beyond the evidence supplied by the caller.
//! * No guessing: source kinds are explicit, and the only "safe to disable"
//!   names come from [`KNOWN_SAFE_VENDOR_UPDATER_PATTERNS`], a deliberately
//!   tiny, documented list.
//!
//! Adapters in later phases collect the facts and pass them in; the UI renders
//! these types. Neither reimplements the rules here.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod startup {
    //! Startup-item inventory and explanation.
    //!
    //! This module never collapses source kinds. A launch daemon runs as root
    //! outside any user session; a per-user launch agent runs inside the
    //! user's session; a registry Run key may be machine-wide or per-user.
    //! Those are different privileges, so they are different variants.
    //!
    //! Tier rules, in precedence order (also documented on [`StartupTier`]):
    //!
    //! 1. [`StartupSource::LaunchDaemon`] and any privileged registry location
    //!    (machine hive, SYSTEM/LOCAL SERVICE/NETWORK SERVICE hive, service or
    //!    policy key path) are [`StartupTier::OffLimits`].
    //! 2. A registry Run/RunOnce or Startup-folder entry whose *name* matches
    //!    [`KNOWN_SAFE_VENDOR_UPDATER_PATTERNS`] is
    //!    [`StartupTier::SafeToDisable`].
    //! 3. Per-user Run/RunOnce and Startup-folder entries are
    //!    [`StartupTier::ReviewFirst`].
    //! 4. Launch agents, systemd units, and cron jobs are
    //!    [`StartupTier::ReviewFirst`] — they may be essential.
    //! 5. Unrecognised sources are [`StartupTier::OffLimits`].
    //!
    //! The analysis reads only the supplied slice. It performs no I/O and
    //! cannot enable or disable anything.

    use serde::{Deserialize, Serialize};

    /// The vendor-updater labels CoreSight is willing to classify as safe to
    /// disable.
    ///
    /// This list is deliberately tiny, and adding to it requires evidence:
    /// vendor documentation showing that the component re-registers itself
    /// and that the user's application keeps working without it. CoreSight
    /// does **not** guess at vendor names, and a match is evidence about the
    /// *label only* — the analysis never verifies the publisher or binary.
    ///
    /// Matching is case-insensitive, ignores one trailing `.lnk`, and accepts
    /// a label only when the normalized item name is exactly the pattern or
    /// the pattern followed by one of ` `, `-`, `_`, `:`, `(`, `[`, or `.`.
    /// That prevents `EvilGoogleUpdate` from matching while allowing
    /// `GoogleUpdate (32-bit)`.
    pub const KNOWN_SAFE_VENDOR_UPDATER_PATTERNS: &[&str] =
        &["googleupdate", "googleupdater", "microsoftedgeupdate"];

    /// Which family a startup source belongs to, independent of its payload.
    ///
    /// The distinction is a privilege boundary, not a presentation nicety:
    /// `LaunchDaemon` runs as root, `LaunchAgent` runs as the user, and a
    /// registry Run key can be either depending on its hive.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
    pub enum SourceKind {
        /// A registry `Run` value (persistent).
        RegistryRun,
        /// A registry `RunOnce` value (removed by Windows after it runs).
        RegistryRunOnce,
        /// A file in a Startup folder.
        StartupFolder,
        /// A per-user launch agent (launchd, macOS).
        LaunchAgent,
        /// A system launch daemon (launchd, macOS; runs as root).
        LaunchDaemon,
        /// A systemd unit.
        SystemdUnit,
        /// A cron job.
        CronJob,
        /// A source the adapter could not classify.
        Unknown,
    }

    impl SourceKind {
        /// Every kind, in canonical report order.
        pub const ALL: [SourceKind; 8] = [
            SourceKind::RegistryRun,
            SourceKind::RegistryRunOnce,
            SourceKind::StartupFolder,
            SourceKind::LaunchAgent,
            SourceKind::LaunchDaemon,
            SourceKind::SystemdUnit,
            SourceKind::CronJob,
            SourceKind::Unknown,
        ];

        /// Stable machine code used in reports and tests.
        pub fn code(self) -> &'static str {
            match self {
                SourceKind::RegistryRun => "REGISTRY_RUN",
                SourceKind::RegistryRunOnce => "REGISTRY_RUN_ONCE",
                SourceKind::StartupFolder => "STARTUP_FOLDER",
                SourceKind::LaunchAgent => "LAUNCH_AGENT",
                SourceKind::LaunchDaemon => "LAUNCH_DAEMON",
                SourceKind::SystemdUnit => "SYSTEMD_UNIT",
                SourceKind::CronJob => "CRON_JOB",
                SourceKind::Unknown => "UNKNOWN",
            }
        }

        /// Short human label for a UI heading.
        pub fn label(self) -> &'static str {
            match self {
                SourceKind::RegistryRun => "Registry Run key",
                SourceKind::RegistryRunOnce => "Registry RunOnce key",
                SourceKind::StartupFolder => "Startup folder",
                SourceKind::LaunchAgent => "Launch agent",
                SourceKind::LaunchDaemon => "Launch daemon",
                SourceKind::SystemdUnit => "systemd unit",
                SourceKind::CronJob => "Cron job",
                SourceKind::Unknown => "Unrecognised source",
            }
        }

        /// Canonical order index, used to keep reports deterministic.
        pub fn rank(self) -> usize {
            match self {
                SourceKind::RegistryRun => 0,
                SourceKind::RegistryRunOnce => 1,
                SourceKind::StartupFolder => 2,
                SourceKind::LaunchAgent => 3,
                SourceKind::LaunchDaemon => 4,
                SourceKind::SystemdUnit => 5,
                SourceKind::CronJob => 6,
                SourceKind::Unknown => 7,
            }
        }
    }

    /// Which Startup folder holds a file.
    ///
    /// The per-user folder and the all-users folder are kept distinct because
    /// the latter needs administrator rights and affects every account.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
    pub enum StartupFolderScope {
        /// The current user's Startup folder.
        User,
        /// The machine-wide Startup folder shared by every account.
        AllUsers,
    }

    /// Where a startup item comes from, with enough detail to verify it.
    ///
    /// Variants are never merged: a launch daemon and a user login item have
    /// different privileges, so the model must not present them as the same
    /// thing.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
    pub enum StartupSource {
        /// A value under a registry `Run` key.
        RegistryRun {
            /// Registry hive as reported by the adapter, e.g. `HKCU` or `HKLM`.
            hive: String,
            /// Full key path below the hive.
            key_path: String,
        },
        /// A value under a registry `RunOnce` key.
        RegistryRunOnce {
            /// Registry hive as reported by the adapter, e.g. `HKCU`.
            hive: String,
            /// Full key path below the hive.
            key_path: String,
        },
        /// A shortcut or executable in a Startup folder.
        StartupFolder {
            /// Which Startup folder holds the entry.
            scope: StartupFolderScope,
        },
        /// A per-user launch agent (launchd).
        LaunchAgent {
            /// The launchd label, e.g. `com.example.helper`.
            label: String,
        },
        /// A system launch daemon (launchd); runs as root outside a session.
        LaunchDaemon {
            /// The launchd label, e.g. `com.example.daemon`.
            label: String,
        },
        /// A systemd unit.
        SystemdUnit {
            /// The unit name, e.g. `cups.service`.
            unit: String,
        },
        /// A cron job.
        CronJob {
            /// The schedule as reported, e.g. `0 3 * * *`.
            schedule: String,
        },
        /// A source the adapter could not classify.
        Unknown {
            /// Raw source description retained so the user can still see it.
            raw: String,
        },
    }

    impl StartupSource {
        /// The source family, used for grouping and canonical ordering.
        pub fn kind(&self) -> SourceKind {
            match self {
                StartupSource::RegistryRun { .. } => SourceKind::RegistryRun,
                StartupSource::RegistryRunOnce { .. } => SourceKind::RegistryRunOnce,
                StartupSource::StartupFolder { .. } => SourceKind::StartupFolder,
                StartupSource::LaunchAgent { .. } => SourceKind::LaunchAgent,
                StartupSource::LaunchDaemon { .. } => SourceKind::LaunchDaemon,
                StartupSource::SystemdUnit { .. } => SourceKind::SystemdUnit,
                StartupSource::CronJob { .. } => SourceKind::CronJob,
                StartupSource::Unknown { .. } => SourceKind::Unknown,
            }
        }

        /// Whether this source sits in a privileged location.
        ///
        /// Evidence used here, and nothing else: launch daemons (root), the
        /// machine hive, the SYSTEM/LOCAL SERVICE/NETWORK SERVICE user hives,
        /// and service or policy key paths. `Unknown` is not privileged; it is
        /// unclassifiable, which is handled separately.
        pub fn is_privileged(&self) -> bool {
            match self {
                StartupSource::LaunchDaemon { .. } => true,
                StartupSource::RegistryRun { hive, key_path }
                | StartupSource::RegistryRunOnce { hive, key_path } => {
                    hive_is_privileged(hive) || key_path_is_privileged(key_path)
                }
                _ => false,
            }
        }

        /// A human-readable description of the location, quoted in the
        /// finding's `why` sentence.
        pub fn location(&self) -> String {
            match self {
                StartupSource::RegistryRun { hive, key_path } => {
                    format!(
                        "the registry run key {}",
                        join_registry_path(hive, key_path)
                    )
                }
                StartupSource::RegistryRunOnce { hive, key_path } => format!(
                    "the registry run-once key {}",
                    join_registry_path(hive, key_path)
                ),
                StartupSource::StartupFolder {
                    scope: StartupFolderScope::User,
                } => "the per-user Startup folder".to_string(),
                StartupSource::StartupFolder {
                    scope: StartupFolderScope::AllUsers,
                } => "the all-users Startup folder (machine-wide, requires administrator rights)"
                    .to_string(),
                StartupSource::LaunchAgent { label } => {
                    format!("the per-user launch agent '{label}'")
                }
                StartupSource::LaunchDaemon { label } => {
                    format!("the system launch daemon '{label}'")
                }
                StartupSource::SystemdUnit { unit } => format!("the systemd unit '{unit}'"),
                StartupSource::CronJob { schedule } => {
                    format!("the cron job scheduled '{schedule}'")
                }
                StartupSource::Unknown { raw } => format!("an unrecognised source ('{raw}')"),
            }
        }
    }

    /// The classification of one startup item for a later, authorized review.
    ///
    /// The tier is derived from the source and the item name only. It is never
    /// derived from a boot-time estimate, a vendor guess, or a popularity
    /// list.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
    pub enum StartupTier {
        /// The source is a per-user, reversible entry whose name matches the
        /// documented vendor-updater allow-list.
        SafeToDisable,
        /// The user must read the evidence before deciding; the item may be
        /// essential.
        ReviewFirst,
        /// Never offer to disable: privileged, or impossible to classify.
        OffLimits,
    }

    impl StartupTier {
        /// Stable machine code used in reports.
        pub fn code(self) -> &'static str {
            match self {
                StartupTier::SafeToDisable => "SAFE_TO_DISABLE",
                StartupTier::ReviewFirst => "REVIEW_FIRST",
                StartupTier::OffLimits => "OFF_LIMITS",
            }
        }

        /// Short human label for the UI.
        pub fn label(self) -> &'static str {
            match self {
                StartupTier::SafeToDisable => "Safe to disable",
                StartupTier::ReviewFirst => "Review first",
                StartupTier::OffLimits => "Off limits",
            }
        }

        /// Severity rank; a larger number is more restrictive. Used so
        /// off-limits items sort first in a report.
        pub fn rank(self) -> usize {
            match self {
                StartupTier::SafeToDisable => 0,
                StartupTier::ReviewFirst => 1,
                StartupTier::OffLimits => 2,
            }
        }
    }

    /// One startup item as reported by a collector.
    ///
    /// Every field is exactly what the collector observed. `command: None`
    /// means the command line was not recorded — not that there is no command.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StartupItem {
        /// Stable identifier within the collecting source.
        pub id: String,
        /// The name shown by the source (a registry value name, a shortcut
        /// file name, a launchd label, ...).
        pub name: String,
        /// The command line, when the source records one.
        pub command: Option<String>,
        /// The exact source, including its privilege-relevant details.
        pub source: StartupSource,
        /// Whether the source currently starts the item.
        pub enabled: bool,
        /// The account that owns the entry, when known.
        pub owner: Option<String>,
    }

    /// One explained item.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StartupFinding {
        /// The item as collected.
        pub item: StartupItem,
        /// The tier derived from the source and name.
        pub tier: StartupTier,
        /// A real sentence naming the evidence behind the tier.
        pub why: String,
    }

    /// How many items of one source family the inventory contains.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SourceCount {
        /// The source family.
        pub source: SourceKind,
        /// How many items of this family were supplied.
        pub total: usize,
        /// How many of them are enabled.
        pub enabled: usize,
    }

    /// Caller options for [`analyze_startup`].
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StartupOptions {
        /// Include disabled items in the finding list. Disabled items are
        /// still counted in `by_source`, because the inventory exists either
        /// way.
        pub include_disabled: bool,
        /// Optional cap on the number of findings returned. `None` means no
        /// cap. Everything omitted by the cap is counted in
        /// [`StartupReport::truncated`], never silently dropped.
        pub max_items: Option<usize>,
    }

    impl Default for StartupOptions {
        fn default() -> Self {
            StartupOptions {
                include_disabled: true,
                max_items: None,
            }
        }
    }

    impl StartupOptions {
        /// Every item, no cap.
        pub fn all() -> Self {
            StartupOptions::default()
        }

        /// Only enabled items, no cap.
        pub fn enabled_only() -> Self {
            StartupOptions {
                include_disabled: false,
                max_items: None,
            }
        }
    }

    /// The result of analysing one startup inventory.
    ///
    /// `total_enabled` and `by_source` describe the whole supplied inventory,
    /// including items omitted by `max_items`; `items` is the (possibly
    /// truncated) finding list. That way truncation cannot understate how much
    /// starts at login.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StartupReport {
        /// The explained items, in canonical order.
        pub items: Vec<StartupFinding>,
        /// Enabled items in the whole supplied inventory.
        pub total_enabled: usize,
        /// Per-source totals for the whole supplied inventory, in
        /// [`SourceKind::ALL`] order, omitting families with no items.
        pub by_source: Vec<SourceCount>,
        /// Findings omitted by `max_items`. Zero when nothing was omitted.
        pub truncated: usize,
    }

    impl StartupReport {
        /// How many findings of one tier are in this (possibly truncated)
        /// report.
        pub fn count(&self, tier: StartupTier) -> usize {
            self.items
                .iter()
                .filter(|finding| finding.tier == tier)
                .fold(0usize, |total, _| total.saturating_add(1))
        }

        /// Whether the finding list was capped.
        pub fn is_truncated(&self) -> bool {
            self.truncated > 0
        }

        /// Items in the whole supplied inventory, recovered from the
        /// per-source totals.
        pub fn supplied_items(&self) -> usize {
            self.by_source
                .iter()
                .fold(0usize, |total, count| total.saturating_add(count.total))
        }
    }

    /// Analyse a startup inventory and explain every item.
    ///
    /// This is a pure function over the supplied slice: it reads nothing from
    /// the machine and writes nothing back. It cannot enable or disable an
    /// item — no such operation exists in this crate.
    ///
    /// Ordering is canonical (tier severity descending, then source family,
    /// then case-folded name, then id and remaining fields), so permuting the
    /// input cannot change the report.
    pub fn analyze_startup(items: &[StartupItem], options: &StartupOptions) -> StartupReport {
        let mut total_enabled = 0usize;
        let mut counts: Vec<(SourceKind, usize, usize)> = SourceKind::ALL
            .iter()
            .map(|kind| (*kind, 0usize, 0usize))
            .collect();

        for item in items {
            if item.enabled {
                total_enabled = total_enabled.saturating_add(1);
            }
            let rank = item.source.kind().rank();
            if let Some(slot) = counts.get_mut(rank) {
                slot.1 = slot.1.saturating_add(1);
                if item.enabled {
                    slot.2 = slot.2.saturating_add(1);
                }
            }
        }

        let by_source: Vec<SourceCount> = counts
            .into_iter()
            .filter(|(_, total, _)| *total > 0)
            .map(|(source, total, enabled)| SourceCount {
                source,
                total,
                enabled,
            })
            .collect();

        let mut findings: Vec<StartupFinding> = items
            .iter()
            .filter(|item| options.include_disabled || item.enabled)
            .map(|item| {
                let (tier, why) = explain(item);
                StartupFinding {
                    item: item.clone(),
                    tier,
                    why,
                }
            })
            .collect();

        sort_findings(&mut findings);

        let limit = match options.max_items {
            Some(limit) => limit,
            None => usize::MAX,
        };
        let truncated = findings.len().saturating_sub(limit);
        findings.truncate(limit);

        StartupReport {
            items: findings,
            total_enabled,
            by_source,
            truncated,
        }
    }

    /// Whether `name` matches one of the documented vendor-updater labels,
    /// returning the pattern that matched.
    ///
    /// Matching is intentionally stricter than a substring search: a label
    /// must be followed by nothing or by a separator. See
    /// [`KNOWN_SAFE_VENDOR_UPDATER_PATTERNS`].
    pub fn known_vendor_updater_pattern(name: &str) -> Option<&'static str> {
        let normalized = normalize_updater_label(name);
        for pattern in KNOWN_SAFE_VENDOR_UPDATER_PATTERNS {
            if normalized == *pattern {
                return Some(*pattern);
            }
            if let Some(rest) = normalized.strip_prefix(pattern) {
                if matches!(
                    rest.chars().next(),
                    Some(' ' | '-' | '_' | ':' | '(' | '[' | '.')
                ) {
                    return Some(*pattern);
                }
            }
        }
        None
    }

    /// Explain one item. The first matching rule wins; the order is the
    /// documented precedence in the module docs.
    fn explain(item: &StartupItem) -> (StartupTier, String) {
        let location = item.source.location();
        match &item.source {
            StartupSource::LaunchDaemon { label } => (
                StartupTier::OffLimits,
                format!(
                    "Registered as the launch daemon '{label}'; launch daemons run as root outside \
                     any user session, so disabling one can stop security, networking, backup, or \
                     update services. CoreSight will not call a root-owned daemon safe to disable."
                ),
            ),
            StartupSource::RegistryRun { .. } | StartupSource::RegistryRunOnce { .. } => {
                if item.source.is_privileged() {
                    (
                        StartupTier::OffLimits,
                        format!(
                            "Registered under {location}, a privileged registry location. A \
                             machine-wide startup entry needs administrator rights to change and \
                             may be required by a driver or security product, so CoreSight treats \
                             it as off limits rather than safe to disable."
                        ),
                    )
                } else if let Some(pattern) = known_vendor_updater_pattern(&item.name) {
                    (
                        StartupTier::SafeToDisable,
                        format!(
                            "Registered under {location}; its name matches the documented \
                             vendor-updater allow-list entry '{pattern}', so disabling it is \
                             reversible and the vendor component re-registers itself. CoreSight \
                             matched the label only and did not verify the publisher or binary; \
                             turning it off may delay the next automatic update.{}",
                            command_note(item)
                        ),
                    )
                } else {
                    (
                        StartupTier::ReviewFirst,
                        format!(
                            "Registered under {location}; the name does not match CoreSight's \
                             documented vendor-updater allow-list, so it may launch an app, a \
                             sync client, or a helper you rely on. Disabling it is reversible but \
                             may stop background sync.{}",
                            command_note(item)
                        ),
                    )
                }
            }
            StartupSource::StartupFolder { scope } => {
                let location_sentence = match scope {
                    StartupFolderScope::User => "Runs at login from the per-user Startup folder.",
                    StartupFolderScope::AllUsers => {
                        "Runs at login for every account from the all-users Startup folder, \
                         which requires administrator rights to change."
                    }
                };
                if let Some(pattern) = known_vendor_updater_pattern(&item.name) {
                    (
                        StartupTier::SafeToDisable,
                        format!(
                            "{location_sentence} Its name matches the documented vendor-updater \
                             allow-list entry '{pattern}', so disabling it is reversible and the \
                             vendor component re-registers itself; CoreSight matched the label \
                             only and did not verify the publisher or binary.{}",
                            command_note(item)
                        ),
                    )
                } else {
                    let warning = match scope {
                        StartupFolderScope::User => {
                            " Disabling it is reversible but may stop an app's background sync."
                        }
                        StartupFolderScope::AllUsers => {
                            " It affects every account, so review it first; disabling it is \
                             reversible but may stop a background task for all users."
                        }
                    };
                    (
                        StartupTier::ReviewFirst,
                        format!("{location_sentence}{warning}{}", command_note(item)),
                    )
                }
            }
            StartupSource::LaunchAgent { label } => (
                StartupTier::ReviewFirst,
                format!(
                    "Registered as the per-user launch agent '{label}'; launch agents start at \
                     login and may be a sync client, an input method, or a helper the user \
                     depends on. Disabling it is reversible but can stop that background work."
                ),
            ),
            StartupSource::SystemdUnit { unit } => (
                StartupTier::ReviewFirst,
                format!(
                    "Enabled as the systemd unit '{unit}'; CoreSight cannot tell from the unit \
                     name whether it is a user service or a system service, and it may be \
                     required for networking, storage, or login."
                ),
            ),
            StartupSource::CronJob { schedule } => (
                StartupTier::ReviewFirst,
                format!(
                    "Scheduled by cron with the schedule '{schedule}'; cron jobs may be backups, \
                     certificate renewals, or monitoring whose absence is noticed only later, so \
                     review the job before disabling it."
                ),
            ),
            StartupSource::Unknown { raw } => (
                StartupTier::OffLimits,
                format!(
                    "CoreSight could not classify this source (raw value: '{raw}'), so it cannot \
                     say what the entry controls. Disabling something it cannot classify would \
                     be a guess; it stays off limits until an adapter reports a recognised \
                     source type."
                ),
            ),
        }
    }

    /// The command-line half of an explanation. A missing command is stated as
    /// unknown, never as "no command".
    fn command_note(item: &StartupItem) -> String {
        match item.command.as_deref().map(str::trim) {
            Some(command) if !command.is_empty() => {
                format!(" It records the command '{command}'.")
            }
            _ => " No command line was recorded, so what it launches is unknown.".to_string(),
        }
    }

    /// Canonical finding order: most restrictive tier first, then source
    /// family, then case-folded name, then the remaining fields as total
    /// tie-breaks.
    fn sort_findings(findings: &mut [StartupFinding]) {
        findings.sort_by(|left, right| {
            right
                .tier
                .rank()
                .cmp(&left.tier.rank())
                .then_with(|| {
                    left.item
                        .source
                        .kind()
                        .rank()
                        .cmp(&right.item.source.kind().rank())
                })
                .then_with(|| {
                    left.item
                        .name
                        .to_lowercase()
                        .cmp(&right.item.name.to_lowercase())
                })
                .then_with(|| left.item.id.cmp(&right.item.id))
                .then_with(|| left.item.command.cmp(&right.item.command))
                .then_with(|| left.item.owner.cmp(&right.item.owner))
                .then_with(|| left.item.enabled.cmp(&right.item.enabled))
        });
    }

    /// Lower-case a label and drop one trailing `.lnk`, matching the
    /// documented allow-list rule.
    fn normalize_updater_label(name: &str) -> String {
        let trimmed = name.trim();
        let without_lnk = match trimmed
            .len()
            .checked_sub(4)
            .and_then(|index| trimmed.get(index..))
        {
            Some(tail) if tail.eq_ignore_ascii_case(".lnk") => &trimmed[..trimmed.len() - 4],
            _ => trimmed,
        };
        without_lnk.trim().to_lowercase()
    }

    /// Whether a hive string is the machine hive or a system-owned user hive.
    ///
    /// A path-suffixed hive (`HKLM\SOFTWARE`) is accepted defensively so an
    /// adapter that includes the subkey cannot downgrade a machine entry to a
    /// per-user one.
    fn hive_is_privileged(hive: &str) -> bool {
        let compact = hive.trim().to_lowercase().replace(' ', "");
        if matches!(compact.as_str(), "hklm" | "hkey_local_machine")
            || compact.starts_with("hklm\\")
            || compact.starts_with("hkey_local_machine\\")
        {
            return true;
        }
        let sid = if let Some(rest) = compact.strip_prefix("hku\\") {
            rest
        } else if let Some(rest) = compact.strip_prefix("hkey_users\\") {
            rest
        } else {
            return false;
        };
        ["s-1-5-18", "s-1-5-19", "s-1-5-20"]
            .iter()
            .any(|system_sid| sid == *system_sid || sid.starts_with(&format!("{system_sid}\\")))
    }

    /// Whether a key path names service or policy configuration. Both are
    /// machine-managed areas, so an entry under them is not a user preference.
    fn key_path_is_privileged(key_path: &str) -> bool {
        let normalized = key_path.trim().to_lowercase();
        let bounded = format!(
            "\\{}",
            normalized.trim_start_matches('\\').trim_end_matches('\\')
        );
        bounded.contains("\\services\\") || bounded.contains("\\policies\\")
    }

    /// Join a hive and key path without doubling the separator.
    fn join_registry_path(hive: &str, key_path: &str) -> String {
        let hive = hive.trim().trim_end_matches('\\');
        let key = key_path.trim().trim_start_matches('\\');
        if key.is_empty() {
            hive.to_string()
        } else {
            format!("{hive}\\{key}")
        }
    }
}

pub mod system {
    //! Structured system-examination report.
    //!
    //! Every fact is optional because a collector may not be able to observe
    //! it. `None` means **unknown**, never zero: a memory total of `None` is
    //! "we did not learn the total", not "the machine has no memory". Coverage
    //! is counted so a partial report cannot be presented as complete.

    use coresight_classifier::Platform;
    use serde::{Deserialize, Serialize};

    /// Facts about the operating system and machine identity.
    ///
    /// Only `platform` is always known (it is a compile-time property of the
    /// running build). Everything else is optional and stays `None` when it
    /// was not collected.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PlatformInfo {
        /// The platform family this build targets.
        pub platform: Platform,
        /// OS product name, e.g. `Windows 11 Pro`.
        pub os_name: Option<String>,
        /// OS version string as reported, e.g. `24H2`.
        pub os_version: Option<String>,
        /// OS build string as reported.
        pub os_build: Option<String>,
        /// CPU architecture as reported, e.g. `x86_64`.
        pub architecture: Option<String>,
        /// Host name as reported.
        pub hostname: Option<String>,
    }

    impl PlatformInfo {
        /// A platform record with every optional fact unknown.
        pub fn new(platform: Platform) -> Self {
            PlatformInfo {
                platform,
                os_name: None,
                os_version: None,
                os_build: None,
                architecture: None,
                hostname: None,
            }
        }

        /// A short description that never invents facts: unknown OS details
        /// are stated as unknown rather than omitted silently.
        pub fn describe(&self) -> String {
            let mut parts: Vec<String> = Vec::new();
            if let Some(name) = non_empty(&self.os_name) {
                parts.push(name.to_string());
            }
            if let Some(version) = non_empty(&self.os_version) {
                parts.push(version.to_string());
            }
            if let Some(build) = non_empty(&self.os_build) {
                parts.push(format!("build {build}"));
            }
            if let Some(architecture) = non_empty(&self.architecture) {
                parts.push(architecture.to_string());
            }
            if parts.is_empty() {
                format!("{} (OS details unknown)", platform_label(self.platform))
            } else {
                format!("{} ({})", platform_label(self.platform), parts.join(", "))
            }
        }
    }

    /// Memory facts. Each field is independently optional.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct MemoryInfo {
        /// Installed physical memory, when known.
        pub total_bytes: Option<u64>,
        /// Currently available physical memory, when known.
        pub available_bytes: Option<u64>,
        /// Currently used physical memory, when the collector reports it
        /// directly.
        pub used_bytes: Option<u64>,
    }

    impl MemoryInfo {
        /// A memory record with nothing known.
        pub fn unknown() -> Self {
            MemoryInfo::default()
        }

        /// Used memory: the reported value, or total minus available when
        /// both are known. `None` when it cannot be derived — never zero.
        pub fn used_bytes_resolved(&self) -> Option<u64> {
            match (self.used_bytes, self.total_bytes, self.available_bytes) {
                (Some(used), _, _) => Some(used),
                (None, Some(total), Some(available)) => Some(total.saturating_sub(available)),
                _ => None,
            }
        }

        /// Used share of physical memory, 0–100, when both sides are known.
        /// A zero or unknown total yields `None` (unknown), not `0 %`.
        pub fn used_ratio_percent(&self) -> Option<u8> {
            let total = self.total_bytes?;
            if total == 0 {
                return None;
            }
            let used = self.used_bytes_resolved()?;
            Some(percentage(used, total))
        }
    }

    /// One storage volume.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StorageVolume {
        /// Stable identifier of the volume (mount point or volume id).
        pub id: String,
        /// Volume label, when known.
        pub label: Option<String>,
        /// Mount point, when known.
        pub mount_point: Option<String>,
        /// File-system name, when known.
        pub file_system: Option<String>,
        /// Capacity, when known. `None` is unknown, not zero.
        pub total_bytes: Option<u64>,
        /// Free space, when known. `None` is unknown, not zero.
        pub available_bytes: Option<u64>,
        /// Whether the volume is removable, when known. `None` means the
        /// collector did not report it.
        pub removable: Option<bool>,
    }

    impl StorageVolume {
        /// Used bytes, only when both capacity and free space are known.
        pub fn used_bytes(&self) -> Option<u64> {
            match (self.total_bytes, self.available_bytes) {
                (Some(total), Some(available)) => Some(total.saturating_sub(available)),
                _ => None,
            }
        }

        /// Used share of capacity, only when capacity is known and non-zero.
        pub fn used_ratio_percent(&self) -> Option<u8> {
            match (self.used_bytes(), self.total_bytes) {
                (Some(used), Some(total)) if total > 0 => Some(percentage(used, total)),
                _ => None,
            }
        }
    }

    /// Severity of a diagnostic finding.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
    #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
    pub enum FindingSeverity {
        /// Context only; nothing appears wrong.
        Info,
        /// Worth a look, but the evidence does not establish harm.
        Attention,
        /// Evidence of a condition that needs prompt attention.
        Critical,
    }

    impl FindingSeverity {
        /// Stable machine code used in reports.
        pub fn code(self) -> &'static str {
            match self {
                FindingSeverity::Info => "INFO",
                FindingSeverity::Attention => "ATTENTION",
                FindingSeverity::Critical => "CRITICAL",
            }
        }
    }

    /// One descriptive observation from the examination.
    ///
    /// Findings state what was observed and the evidence for it. They must not
    /// claim a fix, a cause, or a consequence the crate cannot back up; the
    /// `code` is a compile-time constant so a report cannot carry an invented
    /// code string.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DiagnosticFinding {
        /// Stable machine code of the observation.
        pub code: &'static str,
        /// How much attention the evidence warrants.
        pub severity: FindingSeverity,
        /// One-sentence description of what was observed.
        pub summary: String,
        /// The evidence behind the summary, in plain language.
        pub evidence: String,
    }

    /// Which sources were attempted, and how they ended.
    ///
    /// A report is complete only when it attempted at least one source and
    /// every attempted source succeeded. An empty report is **not** complete:
    /// nothing was examined.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ReportCoverage {
        /// Sources the collector attempted.
        pub sources_total: usize,
        /// Sources that returned usable facts.
        pub sources_succeeded: usize,
        /// Sources that errored.
        pub sources_failed: usize,
        /// Sources the platform does not support.
        pub sources_unsupported: usize,
    }

    impl ReportCoverage {
        /// Build a coverage record from explicit counts.
        pub fn new(
            sources_total: usize,
            sources_succeeded: usize,
            sources_failed: usize,
            sources_unsupported: usize,
        ) -> Self {
            ReportCoverage {
                sources_total,
                sources_succeeded,
                sources_failed,
                sources_unsupported,
            }
        }

        /// Coverage of a report whose `sources` sources all succeeded.
        /// `sources == 0` still yields `is_complete() == false`.
        pub fn complete(sources: usize) -> Self {
            ReportCoverage {
                sources_total: sources,
                sources_succeeded: sources,
                sources_failed: 0,
                sources_unsupported: 0,
            }
        }

        /// Whether every attempted source succeeded and at least one source
        /// was attempted.
        pub fn is_complete(&self) -> bool {
            self.sources_total > 0
                && self.sources_succeeded == self.sources_total
                && self.sources_failed == 0
                && self.sources_unsupported == 0
        }

        /// Saturated sum of the succeeded, failed, and unsupported counts.
        /// Used for display; it may saturate at `usize::MAX`.
        pub fn accounted_sources(&self) -> usize {
            self.sources_succeeded
                .saturating_add(self.sources_failed)
                .saturating_add(self.sources_unsupported)
        }

        /// Whether the four counters add up exactly. Uses checked arithmetic,
        /// so an overflowing (impossible) tally is reported as inconsistent
        /// rather than silently saturated into agreement.
        pub fn is_consistent(&self) -> bool {
            self.sources_succeeded
                .checked_add(self.sources_failed)
                .and_then(|sum| sum.checked_add(self.sources_unsupported))
                == Some(self.sources_total)
        }

        /// A one-line coverage note for the UI. States "complete" only when
        /// [`ReportCoverage::is_complete`] is true.
        pub fn note(&self) -> String {
            if self.is_complete() {
                return format!(
                    "coverage {} of {} sources: complete",
                    self.sources_succeeded, self.sources_total
                );
            }
            let mut reasons: Vec<String> = Vec::new();
            if self.sources_total == 0 {
                reasons.push("no sources were examined".to_string());
            }
            if self.sources_failed > 0 {
                reasons.push(format!("{} failed", self.sources_failed));
            }
            if self.sources_unsupported > 0 {
                reasons.push(format!("{} unsupported", self.sources_unsupported));
            }
            let unaccounted = self.sources_total.saturating_sub(self.accounted_sources());
            if unaccounted > 0 {
                reasons.push(format!("{unaccounted} not reported"));
            }
            if !self.is_consistent() {
                reasons.push("source accounting inconsistent".to_string());
            }
            if reasons.is_empty() {
                reasons.push("not every source succeeded".to_string());
            }
            format!(
                "coverage {} of {} sources: incomplete ({})",
                self.sources_succeeded,
                self.sources_total,
                reasons.join(", ")
            )
        }
    }

    /// A structured system-examination report.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SystemReport {
        /// Platform identity and OS facts.
        pub platform: PlatformInfo,
        /// Memory facts, when the memory source was available.
        pub memory: Option<MemoryInfo>,
        /// Storage volumes that were observed.
        pub storage: Vec<StorageVolume>,
        /// Descriptive findings.
        pub findings: Vec<DiagnosticFinding>,
        /// Which sources were attempted and how they ended.
        pub coverage: ReportCoverage,
    }

    impl SystemReport {
        /// Whether the report covers every source it attempted.
        pub fn is_complete(&self) -> bool {
            self.coverage.is_complete()
        }

        /// How many findings have one severity (saturating).
        pub fn count_by_severity(&self, severity: FindingSeverity) -> usize {
            self.findings
                .iter()
                .filter(|finding| finding.severity == severity)
                .fold(0usize, |total, _| total.saturating_add(1))
        }

        /// Findings ordered most severe first, then by code. Deterministic for
        /// any input order.
        pub fn findings_sorted(&self) -> Vec<&DiagnosticFinding> {
            let mut findings: Vec<&DiagnosticFinding> = self.findings.iter().collect();
            findings.sort_by(|left, right| {
                right
                    .severity
                    .cmp(&left.severity)
                    .then_with(|| left.code.cmp(right.code))
            });
            findings
        }

        /// Total capacity across volumes, only when every volume reports a
        /// capacity. `None` when storage is empty or any capacity is unknown —
        /// a partial total would be a lie.
        pub fn known_storage_total_bytes(&self) -> Option<u64> {
            known_total(&self.storage)
        }
    }

    /// Produce a short human-readable summary line.
    ///
    /// Unknown facts are printed as unknown (never as zero), and incomplete
    /// coverage is stated explicitly so the line cannot be mistaken for a
    /// complete examination.
    pub fn summarize(report: &SystemReport) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(report.platform.describe());

        let memory = match &report.memory {
            None => "RAM unknown (not collected)".to_string(),
            Some(memory) => match memory.total_bytes {
                Some(total) => format!("RAM {}", human_bytes(total)),
                None => "RAM unknown".to_string(),
            },
        };
        parts.push(memory);

        parts.push(describe_storage(&report.storage));

        if report.findings.is_empty() {
            parts.push(if report.coverage.is_complete() {
                "no diagnostic findings".to_string()
            } else {
                "no diagnostic findings reported".to_string()
            });
        } else {
            parts.push(format!(
                "{} findings ({} critical, {} attention)",
                report.findings.len(),
                report.count_by_severity(FindingSeverity::Critical),
                report.count_by_severity(FindingSeverity::Attention)
            ));
        }

        parts.push(report.coverage.note());
        parts.join(" · ")
    }

    /// Human-readable storage sentence. A partial capacity is never summed
    /// into a total.
    fn describe_storage(storage: &[StorageVolume]) -> String {
        if storage.is_empty() {
            return "no storage volumes reported".to_string();
        }
        let noun = if storage.len() == 1 {
            "volume"
        } else {
            "volumes"
        };
        match known_total(storage) {
            Some(total) => format!(
                "{} storage {} ({} total)",
                storage.len(),
                noun,
                human_bytes(total)
            ),
            None => {
                let unknown = storage
                    .iter()
                    .filter(|volume| volume.total_bytes.is_none())
                    .fold(0usize, |total, _| total.saturating_add(1));
                format!(
                    "{} storage {} ({} with unknown capacity)",
                    storage.len(),
                    noun,
                    unknown
                )
            }
        }
    }

    /// Total capacity when every volume reports it; otherwise `None`.
    fn known_total(storage: &[StorageVolume]) -> Option<u64> {
        if storage.is_empty() {
            return None;
        }
        storage.iter().try_fold(0u64, |total, volume| {
            Some(total.saturating_add(volume.total_bytes?))
        })
    }

    /// A trimmed non-empty string, if there is one.
    fn non_empty(value: &Option<String>) -> Option<&str> {
        value
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }

    /// Human label for a platform. The compile-time family is always known.
    fn platform_label(platform: Platform) -> &'static str {
        match platform {
            Platform::Windows => "Windows",
            Platform::Mac => "macOS",
            Platform::Linux => "Linux",
        }
    }

    /// `part` as an integer percentage of `whole`, clamped to 0–100.
    fn percentage(part: u64, whole: u64) -> u8 {
        if whole == 0 {
            return 0;
        }
        let scaled = (u128::from(part)).saturating_mul(100) / u128::from(whole);
        if scaled > 100 {
            100
        } else {
            scaled as u8
        }
    }

    /// Deterministic human-readable byte count with one decimal place in
    /// binary units. Integer math only, so the same input always produces the
    /// same string.
    fn human_bytes(bytes: u64) -> String {
        const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
        if bytes < 1024 {
            return format!("{bytes} B");
        }
        let mut tenths = u128::from(bytes).saturating_mul(10);
        let mut unit = 0usize;
        while tenths >= 1024 * 10 && unit + 1 < UNITS.len() {
            tenths /= 1024;
            unit += 1;
        }
        format!("{}.{} {}", tenths / 10, tenths % 10, UNITS[unit])
    }
}

pub mod tasks {
    //! Application-level task inventory.
    //!
    //! jv16's task view is application-centric; an operating system reports
    //! processes. This module keeps both facts visible: processes are grouped
    //! by the application identity they were attributed to, and processes that
    //! were attributed to nothing are counted in
    //! [`TaskInventory::unattributed_processes`]. The two identities are never
    //! merged: grouping is by exact `app_id`, so two applications that share a
    //! display name remain two entries.

    use std::collections::btree_map::Entry;
    use std::collections::BTreeMap;

    use coresight_apps::ApplicationId;
    use serde::{Deserialize, Serialize};

    /// One application in the task inventory.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct TaskEntry {
        /// CoreSight application identity. Grouping never uses the display
        /// name.
        pub app_id: String,
        /// Name shown to the user. Two different ids may share one name.
        pub display_name: String,
        /// Whether at least one attributed process is currently running.
        pub running: bool,
        /// Exact number of attributed processes.
        pub process_count: usize,
        /// True only when every attributed process belongs to the current
        /// user; a partially-owned application is not presented as "yours".
        pub owned_by_user: bool,
    }

    /// One process row from the collector, before grouping.
    ///
    /// `app_id: None` is the honest "could not attribute this" signal. Such a
    /// row is counted, never dropped.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ProcessAttribution {
        /// The application this process was attributed to, when known.
        pub app_id: Option<ApplicationId>,
        /// The application name the collector knows, when known.
        pub display_name: Option<String>,
        /// Whether the process runs as the current user.
        pub owned_by_user: bool,
    }

    /// The application task inventory.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct TaskInventory {
        /// Applications, sorted by `app_id` for determinism. Each `app_id`
        /// appears at most once.
        pub entries: Vec<TaskEntry>,
        /// Processes that mapped to no known application. This number is
        /// always reported; it is never folded into an application count.
        pub unattributed_processes: usize,
    }

    impl TaskInventory {
        /// Build an inventory, merging entries that share an `app_id`.
        ///
        /// Merging keeps `process_count` exact (saturating sum), ORs
        /// `running`, and ANDs `owned_by_user`, so a partially-owned
        /// application is not labelled as the user's. Display names are
        /// resolved deterministically to the lexicographically smallest
        /// non-empty name. An entry with a blank `app_id` cannot be attributed
        /// to an application, so its `process_count` is added to
        /// `unattributed_processes`.
        pub fn new(entries: Vec<TaskEntry>, unattributed_processes: usize) -> Self {
            let mut unattributed = unattributed_processes;
            let mut merged: BTreeMap<String, TaskEntry> = BTreeMap::new();

            for entry in entries {
                if entry.app_id.trim().is_empty() {
                    unattributed = unattributed.saturating_add(entry.process_count);
                    continue;
                }
                let display_name = normalize_display_name(&entry.display_name, &entry.app_id);
                match merged.entry(entry.app_id.clone()) {
                    Entry::Occupied(mut occupied) => {
                        let existing = occupied.get_mut();
                        existing.process_count =
                            existing.process_count.saturating_add(entry.process_count);
                        existing.running = existing.running || entry.running;
                        existing.owned_by_user = existing.owned_by_user && entry.owned_by_user;
                        if display_name < existing.display_name {
                            existing.display_name = display_name;
                        }
                    }
                    Entry::Vacant(vacant) => {
                        vacant.insert(TaskEntry {
                            app_id: entry.app_id,
                            display_name,
                            running: entry.running,
                            process_count: entry.process_count,
                            owned_by_user: entry.owned_by_user,
                        });
                    }
                }
            }

            TaskInventory {
                entries: merged.into_values().collect(),
                unattributed_processes: unattributed,
            }
        }

        /// Merge process→application rows into an inventory.
        ///
        /// Rows with no `app_id` (or a blank one) count toward
        /// `unattributed_processes`; rows with an id contribute exactly one
        /// process to that application. Two distinct ids always produce two
        /// entries, even when their display names are identical.
        pub fn from_attributions(rows: &[ProcessAttribution]) -> Self {
            let mut entries: Vec<TaskEntry> = Vec::new();
            let mut unattributed = 0usize;

            for row in rows {
                match &row.app_id {
                    Some(app_id) if !app_id.0.trim().is_empty() => {
                        let display_name = match row.display_name.as_deref().map(str::trim) {
                            Some(name) if !name.is_empty() => name.to_string(),
                            _ => app_id.0.clone(),
                        };
                        entries.push(TaskEntry {
                            app_id: app_id.0.clone(),
                            display_name,
                            running: true,
                            process_count: 1,
                            owned_by_user: row.owned_by_user,
                        });
                    }
                    _ => {
                        unattributed = unattributed.saturating_add(1);
                    }
                }
            }

            TaskInventory::new(entries, unattributed)
        }

        /// Total processes represented: attributed plus unattributed.
        pub fn total_processes(&self) -> usize {
            self.entries
                .iter()
                .fold(self.unattributed_processes, |total, entry| {
                    total.saturating_add(entry.process_count)
                })
        }

        /// Processes attributed to a known application.
        pub fn attributed_processes(&self) -> usize {
            self.entries.iter().fold(0usize, |total, entry| {
                total.saturating_add(entry.process_count)
            })
        }

        /// How many applications have at least one running process.
        pub fn running_apps(&self) -> usize {
            self.entries
                .iter()
                .filter(|entry| entry.running)
                .fold(0usize, |total, _| total.saturating_add(1))
        }

        /// Whether every process was attributed. `false` is the honest state
        /// to show in the UI.
        pub fn is_fully_attributed(&self) -> bool {
            self.unattributed_processes == 0
        }

        /// Look up one application by exact identity.
        ///
        /// A linear scan is used deliberately: the entry list is public and
        /// may have been deserialized in an arbitrary order, so this lookup
        /// does not depend on the constructors' sorting invariant.
        pub fn entry(&self, app_id: &str) -> Option<&TaskEntry> {
            self.entries.iter().find(|entry| entry.app_id == app_id)
        }
    }

    /// Trim a display name, falling back to the identity when it is blank.
    fn normalize_display_name(display_name: &str, app_id: &str) -> String {
        let trimmed = display_name.trim();
        if trimmed.is_empty() {
            app_id.to_string()
        } else {
            trimmed.to_string()
        }
    }
}

pub use coresight_apps::ApplicationId;
pub use startup::{
    analyze_startup, known_vendor_updater_pattern, SourceCount, SourceKind, StartupFinding,
    StartupFolderScope, StartupItem, StartupOptions, StartupReport, StartupSource, StartupTier,
    KNOWN_SAFE_VENDOR_UPDATER_PATTERNS,
};
pub use system::{
    summarize, DiagnosticFinding, FindingSeverity, MemoryInfo, PlatformInfo, ReportCoverage,
    StorageVolume, SystemReport,
};
pub use tasks::{ProcessAttribution, TaskEntry, TaskInventory};

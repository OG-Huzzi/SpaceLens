//! Startup inventory parsing: raw platform facts in, typed
//! [`StartupItem`]s out.
//!
//! ## A field extractor, not a parser
//!
//! The launchd reader below is deliberately **not** a plist parser. It is a
//! tolerant field scan: it finds `<key>Label</key>` followed by a
//! `<string>`, and `<key>ProgramArguments</key>` followed by an `<array>` of
//! `<string>` elements, and it reads `Program` as a fallback. It does not
//! build a document tree, validate XML, process a DOCTYPE, expand custom
//! entities, read CDATA, understand every plist type, or read binary
//! (`bplist00`) plists. When it cannot find a field it rejects the entry with
//! a reason instead of guessing. Calling this a plist parser would be an
//! over-claim; calling it a field extractor is accurate.
//!
//! The systemd reader is the same kind of scan: it looks for the first
//! `ExecStart=` assignment, outside comments, and joins backslash
//! continuations. It does not interpret systemd's specifier or escape
//! language.
//!
//! ## What is carried through verbatim
//!
//! A Windows Run/RunOnce value's `hive` and `key_path` are copied into
//! [`StartupSource`] exactly as supplied — no case folding, separator
//! rewriting, or trimming — because the tier decision and the user's ability
//! to find the entry both depend on the real location. The Run-versus-RunOnce
//! classification uses a trimmed, case-insensitive view of the last key
//! component, but that view only chooses the variant; it never replaces the
//! stored path.
//!
//! ## Privilege boundaries are preserved
//!
//! [`LaunchdScope::SystemDaemon`] maps to
//! [`StartupSource::LaunchDaemon`],
//! which [`analyze_startup`](coresight_systools::startup::analyze_startup)
//! classifies as `OffLimits`. A system daemon runs as root outside any user
//! session; collapsing it into a user agent would understate that.
//!
//! ## Shell-profile lines are conservatively off limits
//!
//! A line in `.profile`, `.zshrc`, or a comparable file can set `PATH`,
//! unlock an SSH agent, start a credential helper, or do anything else the
//! shell can do. This crate does not interpret shell syntax, so it cannot
//! know what disabling the line would break. There is no shell-profile
//! variant in [`StartupSource`];
//! the honest choice is
//! [`StartupSource::Unknown`]
//! with a raw description that names the profile and line. `analyze_startup`
//! classifies an unknown source as `OffLimits`, so a profile line can never be
//! presented as safe to disable. Pretending the line were a launch agent or a
//! cron job would be a false classification with a different (and weaker)
//! tier.
//!
//! ## What is rejected
//!
//! * A launchd plist with no `Label`, an empty `Label`, no
//!   `ProgramArguments[0]`, no `Program`, NUL bytes, or a binary plist.
//! * A registry value whose key path does not end in `Run` or `RunOnce`, or
//!   whose hive, value name, or command is empty.
//! * A systemd unit with no `ExecStart=` (a `.target`, for example), an empty
//!   `ExecStart=`, or an empty unit name.
//! * A shell-profile line that is empty or a comment.
//! * Any field longer than the documented limits below.
//!
//! Every rejection is returned in [`StartupParseResult::rejected`] with a
//! non-empty reason. [`StartupParseResult::unit_parse_failures`] additionally
//! counts systemd entries that produced no item.
//!
//! ## Limitations stated plainly
//!
//! * `RawStartupEntry` carries no enabled/disabled signal, so every parsed
//!   item is marked `enabled: true`: the shell supplies entries it observed
//!   as registered startup entries. A disabled entry needs an explicit input
//!   field that this schema does not have.
//! * Launchd `ProgramArguments` are joined with single spaces for display.
//!   Argument quoting is not reconstructed, and values are trimmed. The first
//!   argument is still the program.
//! * A shell-profile item's name is the first whitespace-delimited token of
//!   the line; the full line is stored as the command.
//! * Duplicate raw entries are preserved; this parser is not a de-duplicator.
//! * No I/O, no registry access, no plist reading, no process execution: all
//!   text and rows arrive from the caller.

use coresight_systools::startup::{StartupItem, StartupSource};
use serde::{Deserialize, Serialize};

use crate::text::{excerpt, fits, text_lines, truncate_chars};

/// Longest startup name (a label, registry value name, or unit name) this
/// parser will carry.
pub const MAX_STARTUP_NAME_CHARS: usize = 512;

/// Longest registry key path this parser will carry.
pub const MAX_REGISTRY_KEY_PATH_CHARS: usize = 2048;

/// Longest command line (plist arguments, registry command, shell line, or
/// systemd `ExecStart`) this parser will carry.
pub const MAX_STARTUP_COMMAND_CHARS: usize = 4096;

/// Longest source label quoted in a [`ParseReject`].
pub const MAX_REJECT_SOURCE_CHARS: usize = 200;

/// Which launchd domain a plist came from.
///
/// The distinction is a privilege boundary, not a presentation detail:
/// a system daemon runs as root outside any user session, while a user agent
/// runs inside the user's session. The parser maps the former to
/// `LaunchDaemon` so the analyzer can keep it off limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LaunchdScope {
    /// A per-user launch agent (`~/Library/LaunchAgents`).
    UserAgent,
    /// A system launch daemon (`/Library/LaunchDaemons`); runs as root.
    SystemDaemon,
}

impl LaunchdScope {
    /// Stable machine code used in ids and reject labels.
    fn code(self) -> &'static str {
        match self {
            LaunchdScope::UserAgent => "user-agent",
            LaunchdScope::SystemDaemon => "system-daemon",
        }
    }
}

/// One raw startup fact supplied by the desktop shell.
///
/// The shell does the platform work — reading a plist, exporting a registry
/// key, reading a unit file, or listing a profile line — and hands the text
/// or row here. This enum is therefore an input contract, not a capability:
/// constructing a variant does not read anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum RawStartupEntry {
    /// A macOS launchd plist as text, plus the source label the shell used
    /// (usually the plist path) and the domain it was found in.
    LaunchdPlist {
        /// Source label for diagnostics, e.g. the plist path.
        label: String,
        /// The plist text. Binary plists are rejected, not decoded.
        plist_text: String,
        /// Which launchd domain the plist came from.
        scope: LaunchdScope,
    },
    /// A Windows Run/RunOnce registry value.
    RegistryRunValue {
        /// Registry hive exactly as reported, e.g. `HKCU` or `HKLM`.
        hive: String,
        /// Full key path below the hive, exactly as reported.
        key_path: String,
        /// The value name, which is the startup item's name.
        value_name: String,
        /// The command stored in the value.
        command: String,
    },
    /// A shell-profile line that starts a program at login.
    ShellProfileLine {
        /// The profile file that contains the line, e.g. `~/.zshrc`.
        profile: String,
        /// The line itself.
        line: String,
    },
    /// A systemd unit file as text.
    SystemdUnit {
        /// The unit name, e.g. `cups.service` or `backup.target`.
        unit_name: String,
        /// The unit file text.
        unit_text: String,
    },
}

/// One raw entry that could not be turned into a [`StartupItem`].
///
/// `source` names the input entry (a plist label, registry location, profile,
/// or unit) and `reason` says exactly what was missing or malformed. A reject
/// is evidence for the UI; it is never a silent drop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseReject {
    /// A bounded, human-readable label for the rejected source.
    pub source: String,
    /// Why the entry was refused.
    pub reason: String,
}

/// The result of parsing a startup inventory.
///
/// `items` is canonically ordered (source family, case-folded name, id, and
/// remaining fields), so permuting the input cannot change it. `rejected` is
/// ordered by source then reason. `unit_parse_failures` counts systemd
/// entries specifically, so a UI can say "three unit files could not be read"
/// without counting unrelated plist failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupParseResult {
    /// Successfully parsed items, in canonical order.
    pub items: Vec<StartupItem>,
    /// Entries that produced no item, with a reason.
    pub rejected: Vec<ParseReject>,
    /// How many [`RawStartupEntry::SystemdUnit`] entries were rejected.
    pub unit_parse_failures: usize,
}

/// Parse caller-supplied startup facts into typed inventory items.
///
/// This function is total: every input, including empty text, NUL bytes, a
/// megabyte of junk, and deeply nested XML-like structure, returns a result.
/// It performs no I/O and cannot enable, disable, or execute anything.
pub fn parse_startup(entries: &[RawStartupEntry]) -> StartupParseResult {
    let mut items: Vec<StartupItem> = Vec::new();
    let mut rejected: Vec<ParseReject> = Vec::new();
    let mut unit_parse_failures = 0usize;

    for entry in entries {
        match entry {
            RawStartupEntry::LaunchdPlist {
                label,
                plist_text,
                scope,
            } => match parse_launchd(plist_text, *scope) {
                Ok(item) => items.push(item),
                Err(reason) => rejected.push(ParseReject {
                    source: launchd_reject_source(label, *scope),
                    reason,
                }),
            },
            RawStartupEntry::RegistryRunValue {
                hive,
                key_path,
                value_name,
                command,
            } => match parse_registry_run(hive, key_path, value_name, command) {
                Ok(item) => items.push(item),
                Err(reason) => rejected.push(ParseReject {
                    source: registry_reject_source(hive, key_path, value_name),
                    reason,
                }),
            },
            RawStartupEntry::ShellProfileLine { profile, line } => {
                match parse_shell_profile_line(profile, line) {
                    Ok(item) => items.push(item),
                    Err(reason) => rejected.push(ParseReject {
                        source: shell_reject_source(profile),
                        reason,
                    }),
                }
            }
            RawStartupEntry::SystemdUnit {
                unit_name,
                unit_text,
            } => match parse_systemd_unit(unit_name, unit_text) {
                Ok(item) => items.push(item),
                Err(reason) => {
                    unit_parse_failures = unit_parse_failures.saturating_add(1);
                    rejected.push(ParseReject {
                        source: unit_reject_source(unit_name),
                        reason,
                    });
                }
            },
        }
    }

    sort_items(&mut items);
    rejected.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then_with(|| left.reason.cmp(&right.reason))
    });

    StartupParseResult {
        items,
        rejected,
        unit_parse_failures,
    }
}

/// Parse one launchd plist field by field.
fn parse_launchd(plist_text: &str, scope: LaunchdScope) -> Result<StartupItem, String> {
    if plist_text.trim().is_empty() {
        return Err("plist text is empty".to_string());
    }
    if plist_text.contains('\0') {
        return Err(
            "plist text contains a NUL byte, so it is not XML text; binary plists are not supported"
                .to_string(),
        );
    }
    if plist_text.trim_start().starts_with("bplist") {
        return Err("binary plists are not supported; supply the XML plist text".to_string());
    }

    let cleaned = strip_xml_comments(plist_text);

    let label = match string_value_after_key(&cleaned, "Label") {
        Some(value) => value.trim().to_string(),
        None => {
            return Err(
                "no Label key followed by a <string> value was found in the plist".to_string(),
            )
        }
    };
    if label.is_empty() {
        return Err("the plist Label value is empty".to_string());
    }
    if !fits(&label, MAX_STARTUP_NAME_CHARS) {
        return Err(format!(
            "the plist Label is longer than the {MAX_STARTUP_NAME_CHARS}-character limit"
        ));
    }

    let command =
        match launchd_command(&cleaned) {
            Some(command) => command,
            None => return Err(
                "no program was found: ProgramArguments[0] and Program are both missing or empty"
                    .to_string(),
            ),
        };
    if !fits(&command, MAX_STARTUP_COMMAND_CHARS) {
        return Err(format!(
            "the launchd command is longer than the {MAX_STARTUP_COMMAND_CHARS}-character limit"
        ));
    }

    let source = match scope {
        LaunchdScope::UserAgent => StartupSource::LaunchAgent {
            label: label.clone(),
        },
        LaunchdScope::SystemDaemon => StartupSource::LaunchDaemon {
            label: label.clone(),
        },
    };

    Ok(StartupItem {
        id: format!("launchd:{}:{}", scope.code(), label),
        name: label,
        command: Some(command),
        source,
        enabled: true,
        owner: None,
    })
}

/// The launchd command: `ProgramArguments` joined with spaces, or `Program`.
fn launchd_command(cleaned: &str) -> Option<String> {
    if let Some(arguments) = array_after_key(cleaned, "ProgramArguments") {
        let values = array_string_values(arguments);
        if let Some(first) = values.first() {
            if !first.trim().is_empty() {
                let joined = values
                    .iter()
                    .map(|argument| argument.trim())
                    .collect::<Vec<&str>>()
                    .join(" ");
                return Some(joined);
            }
        }
    }

    let program = string_value_after_key(cleaned, "Program")?;
    let program = program.trim();
    if program.is_empty() {
        None
    } else {
        Some(program.to_string())
    }
}

/// Parse one Windows Run/RunOnce value.
fn parse_registry_run(
    hive: &str,
    key_path: &str,
    value_name: &str,
    command: &str,
) -> Result<StartupItem, String> {
    if hive.trim().is_empty() {
        return Err("the registry entry has an empty hive".to_string());
    }
    if key_path.trim().is_empty() {
        return Err("the registry entry has an empty key path".to_string());
    }
    if value_name.trim().is_empty() {
        return Err("the registry Run value has an empty value name".to_string());
    }
    if command.trim().is_empty() {
        return Err("the registry Run value has an empty command".to_string());
    }
    if !fits(hive, MAX_STARTUP_NAME_CHARS) {
        return Err(format!(
            "the registry hive is longer than the {MAX_STARTUP_NAME_CHARS}-character limit"
        ));
    }
    if !fits(key_path, MAX_REGISTRY_KEY_PATH_CHARS) {
        return Err(format!(
            "the registry key path is longer than the {MAX_REGISTRY_KEY_PATH_CHARS}-character limit"
        ));
    }
    if !fits(value_name, MAX_STARTUP_NAME_CHARS) {
        return Err(format!(
            "the registry value name is longer than the {MAX_STARTUP_NAME_CHARS}-character limit"
        ));
    }
    if !fits(command, MAX_STARTUP_COMMAND_CHARS) {
        return Err(format!(
            "the registry command is longer than the {MAX_STARTUP_COMMAND_CHARS}-character limit"
        ));
    }

    let run_once = match run_key_kind(key_path) {
        Some(run_once) => run_once,
        None => {
            return Err(format!(
                "key path {} does not end in a Run or RunOnce key",
                quote_excerpt(key_path)
            ))
        }
    };

    // The variant is chosen from a normalized view, but the payload is the
    // caller's exact hive and key path: the hard contract is that they survive
    // unchanged.
    let source = if run_once {
        StartupSource::RegistryRunOnce {
            hive: hive.to_string(),
            key_path: key_path.to_string(),
        }
    } else {
        StartupSource::RegistryRun {
            hive: hive.to_string(),
            key_path: key_path.to_string(),
        }
    };

    let name = value_name.trim().to_string();
    Ok(StartupItem {
        id: format!(
            "registry:{}:{}:{}",
            if run_once { "run-once" } else { "run" },
            hive.trim(),
            key_path.trim()
        ),
        name,
        command: Some(command.trim().to_string()),
        source,
        enabled: true,
        owner: None,
    })
}

/// `Some(true)` for a RunOnce key, `Some(false)` for a Run key, `None` for
/// anything else.
fn run_key_kind(key_path: &str) -> Option<bool> {
    let trimmed = key_path.trim().trim_end_matches('\\');
    let last = trimmed.rsplit('\\').next().unwrap_or(trimmed);
    match last.trim().to_ascii_lowercase().as_str() {
        "run" => Some(false),
        "runonce" => Some(true),
        _ => None,
    }
}

/// Parse one shell-profile line.
///
/// The payload is not interpreted. See the module docs for why the source is
/// [`StartupSource::Unknown`] and therefore off limits.
fn parse_shell_profile_line(profile: &str, line: &str) -> Result<StartupItem, String> {
    if profile.trim().is_empty() {
        return Err("the shell profile entry has an empty profile name".to_string());
    }
    if line.trim().is_empty() {
        return Err("the shell profile line is empty".to_string());
    }
    if line.trim_start().starts_with('#') {
        return Err("the shell profile line is a comment, not a program".to_string());
    }
    if !fits(profile, MAX_STARTUP_NAME_CHARS) {
        return Err(format!(
            "the shell profile name is longer than the {MAX_STARTUP_NAME_CHARS}-character limit"
        ));
    }
    if !fits(line, MAX_STARTUP_COMMAND_CHARS) {
        return Err(format!(
            "the shell profile line is longer than the {MAX_STARTUP_COMMAND_CHARS}-character limit"
        ));
    }

    let trimmed = line.trim();
    let name = trimmed.split_whitespace().next().unwrap_or(trimmed);
    let source = StartupSource::Unknown {
        raw: format!("shell profile {}: {}", profile.trim(), trimmed),
    };

    Ok(StartupItem {
        id: format!("shell-profile:{}:{}", profile.trim(), trimmed),
        name: name.to_string(),
        command: Some(trimmed.to_string()),
        source,
        enabled: true,
        owner: None,
    })
}

/// Parse one systemd unit file.
fn parse_systemd_unit(unit_name: &str, unit_text: &str) -> Result<StartupItem, String> {
    let name = unit_name.trim();
    if name.is_empty() {
        return Err("the systemd entry has an empty unit name".to_string());
    }
    if !fits(name, MAX_STARTUP_NAME_CHARS) {
        return Err(format!(
            "the systemd unit name is longer than the {MAX_STARTUP_NAME_CHARS}-character limit"
        ));
    }
    if unit_text.contains('\0') {
        return Err("the unit text contains a NUL byte".to_string());
    }

    let command = match find_exec_start(unit_text) {
        Some(command) if !command.is_empty() => command,
        Some(_) => {
            return Err("the unit has an ExecStart= directive with an empty value".to_string())
        }
        None => {
            return Err(
                "the unit has no ExecStart= directive (a .target unit, for example, has none)"
                    .to_string(),
            )
        }
    };
    if !fits(&command, MAX_STARTUP_COMMAND_CHARS) {
        return Err(format!(
            "the ExecStart command is longer than the {MAX_STARTUP_COMMAND_CHARS}-character limit"
        ));
    }

    Ok(StartupItem {
        id: format!("systemd:{name}"),
        name: name.to_string(),
        command: Some(command),
        source: StartupSource::SystemdUnit {
            unit: name.to_string(),
        },
        enabled: true,
        owner: None,
    })
}

/// Find the first `ExecStart=` directive outside comments.
///
/// An odd number of trailing backslashes continues the value on the next
/// line, which is the common systemd spelling. Escapes and specifiers are not
/// otherwise interpreted; only the first `ExecStart=` in the file is used.
fn find_exec_start(unit_text: &str) -> Option<String> {
    let lines: Vec<&str> = text_lines(unit_text).map(|(_, line)| line).collect();
    let mut index = 0usize;
    while let Some(raw) = lines.get(index) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with(';') {
            if let Some(rest) = trimmed.strip_prefix("ExecStart") {
                if let Some(value) = rest.trim_start().strip_prefix('=') {
                    return Some(join_continuations(&lines, index, value));
                }
            }
        }
        index = index.saturating_add(1);
    }
    None
}

/// Join a systemd directive value across backslash continuations.
fn join_continuations(lines: &[&str], start: usize, first: &str) -> String {
    let mut value = first.trim().to_string();
    let mut index = start;
    loop {
        let trimmed = value.trim_end();
        let trailing = trimmed
            .chars()
            .rev()
            .take_while(|character| *character == '\\')
            .count();
        if trailing % 2 == 0 {
            break;
        }
        let without_continuation = match trimmed.strip_suffix('\\') {
            Some(prefix) => prefix.trim_end(),
            None => break,
        };
        let next = match lines.get(index.saturating_add(1)) {
            Some(next) => next.trim(),
            None => {
                value = without_continuation.to_string();
                break;
            }
        };
        value = format!("{without_continuation} {next}");
        index = index.saturating_add(1);
    }
    value.trim().to_string()
}

/// Strip XML comments so a commented-out key cannot be mistaken for a live
/// one. An unterminated comment is treated as running to end of input.
fn strip_xml_comments(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        if let Some(prefix) = rest.get(..start) {
            output.push_str(prefix);
        }
        let tail = match rest.get(start..) {
            Some(tail) => tail,
            None => return output,
        };
        match tail.find("-->") {
            Some(end) => {
                let next = start
                    .checked_add(end)
                    .and_then(|value| value.checked_add(3));
                rest = match next.and_then(|next| rest.get(next..)) {
                    Some(next) => next,
                    None => return output,
                };
            }
            None => return output,
        }
    }
    output.push_str(rest);
    output
}

/// The text after the closing `</key>` of the first `<key>key</key>` whose
/// name matches (after trimming).
fn after_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = text;
    loop {
        let start = rest.find("<key>")?;
        let after_open = rest.get(start.checked_add(5)?..)?;
        let close = after_open.find("</key>")?;
        let name = after_open.get(..close)?;
        let consumed = start.checked_add(5)?.checked_add(close)?.checked_add(6)?;
        let after = rest.get(consumed..)?;
        if name.trim() == key {
            return Some(after);
        }
        rest = after;
    }
}

/// The immediate `<string>` element after `key`, decoded and returned.
fn string_value_after_key(text: &str, key: &str) -> Option<String> {
    let after = after_key(text, key)?;
    let trimmed = after.trim_start();
    let content = trimmed.strip_prefix("<string>")?;
    let close = content.find("</string>")?;
    let value = content.get(..close)?;
    Some(decode_xml_entities(value))
}

/// The contents of the immediate `<array>` element after `key`.
fn array_after_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let after = after_key(text, key)?;
    let trimmed = after.trim_start();
    let content = trimmed.strip_prefix("<array>")?;
    let close = content.find("</array>")?;
    content.get(..close)
}

/// Every `<string>` value inside an array body, in document order.
fn array_string_values(inner: &str) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    let mut rest = inner;
    while let Some(start) = rest.find("<string>") {
        let after = match rest.get(start.saturating_add(8)..) {
            Some(after) => after,
            None => break,
        };
        let close = match after.find("</string>") {
            Some(close) => close,
            None => break,
        };
        let value = match after.get(..close) {
            Some(value) => value,
            None => break,
        };
        values.push(decode_xml_entities(value));
        let consumed = start
            .checked_add(8)
            .and_then(|value| value.checked_add(close))
            .and_then(|value| value.checked_add(9));
        rest = match consumed.and_then(|consumed| rest.get(consumed..)) {
            Some(rest) => rest,
            None => break,
        };
    }
    values
}

/// Decode the five predefined XML entities and numeric character references.
/// Anything else is left literal rather than guessed.
fn decode_xml_entities(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(ampersand) = rest.find('&') {
        match rest.get(..ampersand) {
            Some(prefix) => output.push_str(prefix),
            None => break,
        }
        let tail = match rest.get(ampersand..) {
            Some(tail) => tail,
            None => break,
        };
        let decoded = tail
            .find(';')
            .filter(|semicolon| *semicolon <= 12)
            .and_then(|semicolon| {
                let entity = tail.get(1..semicolon)?;
                let character = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => numeric_xml_entity(entity),
                }?;
                Some((character, semicolon.saturating_add(1)))
            });
        match decoded {
            Some((character, consumed)) => {
                output.push(character);
                rest = match tail.get(consumed..) {
                    Some(next) => next,
                    None => return output,
                };
            }
            None => {
                output.push('&');
                rest = match tail.get(1..) {
                    Some(next) => next,
                    None => return output,
                };
            }
        }
    }
    output.push_str(rest);
    output
}

/// Decode `&#123;` and `&#x1F;` references.
fn numeric_xml_entity(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let value = if let Some(hex) = digits
        .strip_prefix('x')
        .or_else(|| digits.strip_prefix('X'))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        digits.parse::<u32>().ok()?
    };
    char::from_u32(value)
}

/// Sort items by a total key so permuted input produces identical output.
fn sort_items(items: &mut [StartupItem]) {
    items.sort_by(|left, right| {
        left.source
            .kind()
            .rank()
            .cmp(&right.source.kind().rank())
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
            .then_with(|| left.command.cmp(&right.command))
            .then_with(|| left.owner.cmp(&right.owner))
            .then_with(|| left.enabled.cmp(&right.enabled))
            .then_with(|| left.source.cmp(&right.source))
    });
}

/// A bounded source label for a rejected launchd plist.
fn launchd_reject_source(label: &str, scope: LaunchdScope) -> String {
    let trimmed = label.trim();
    let base = if trimmed.is_empty() {
        format!("launchd plist ({})", scope.code())
    } else {
        format!(
            "launchd plist {} ({})",
            quote_excerpt(trimmed),
            scope.code()
        )
    };
    truncate_chars(&base, MAX_REJECT_SOURCE_CHARS)
}

/// A bounded source label for a rejected registry value.
fn registry_reject_source(hive: &str, key_path: &str, value_name: &str) -> String {
    let base = format!(
        "registry value {}\\{} :: {}",
        hive.trim(),
        key_path.trim(),
        value_name.trim()
    );
    truncate_chars(&base, MAX_REJECT_SOURCE_CHARS)
}

/// A bounded source label for a rejected shell-profile line.
fn shell_reject_source(profile: &str) -> String {
    let trimmed = profile.trim();
    let base = if trimmed.is_empty() {
        "shell profile (unnamed)".to_string()
    } else {
        format!("shell profile {}", quote_excerpt(trimmed))
    };
    truncate_chars(&base, MAX_REJECT_SOURCE_CHARS)
}

/// A bounded source label for a rejected systemd unit.
fn unit_reject_source(unit_name: &str) -> String {
    let trimmed = unit_name.trim();
    let base = if trimmed.is_empty() {
        "systemd unit (unnamed)".to_string()
    } else {
        format!("systemd unit {}", quote_excerpt(trimmed))
    };
    truncate_chars(&base, MAX_REJECT_SOURCE_CHARS)
}

/// Quote a short excerpt for a reason string, bounded.
fn quote_excerpt(text: &str) -> String {
    format!("\"{}\"", excerpt(text))
}

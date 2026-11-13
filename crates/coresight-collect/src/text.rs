//! Shared, total text helpers for the line-oriented inventories.
//!
//! Every helper is a pure function over caller-supplied text. Slices are
//! taken with `get(...)` or produced by `find`, so a multi-byte character can
//! never be split, and no helper can panic on malformed input.
//!
//! The constants here are part of the loader contracts: a field that exceeds
//! its documented limit is refused with a reason instead of being copied,
//! unbounded, into a report.

use serde::{Deserialize, Serialize};

/// Maximum number of characters of an offending line quoted in a
/// [`LineReject`].
///
/// A one-megabyte junk line must not become a one-megabyte error message, so
/// the quoted text is bounded. The line number and reason stay exact.
pub const REJECT_EXCERPT_CHARS: usize = 160;

/// One line a text loader refused, with its 1-based line number and the
/// reason.
///
/// This is the line-oriented counterpart of
/// [`ParseReject`](crate::ParseReject): the text loaders can point at the exact
/// line, while a startup entry is identified by its source label. A rejected
/// line is always reported; nothing is silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineReject {
    /// 1-based line number in the supplied text.
    pub line: usize,
    /// The offending line, trimmed and truncated to the documented excerpt
    /// limit.
    pub text: String,
    /// Why the line was refused, in plain language.
    pub reason: String,
}

impl LineReject {
    /// Build a reject record, bounding the quoted line.
    pub(crate) fn new(line: usize, text: &str, reason: impl Into<String>) -> Self {
        LineReject {
            line,
            text: excerpt(text),
            reason: reason.into(),
        }
    }
}

/// Iterate over `(1-based line number, line)` pairs.
///
/// A leading UTF-8 byte-order mark is skipped; both `\n` and `\r\n` endings
/// are accepted. An empty input yields no lines, which the loaders treat as
/// an empty inventory, never an error.
pub(crate) fn text_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let body = match text.strip_prefix('\u{feff}') {
        Some(body) => body,
        None => text,
    };
    body.lines()
        .enumerate()
        .map(|(index, line)| (index.saturating_add(1), line))
}

/// Whether a line is blank or a whole-line `#` comment.
pub(crate) fn is_blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

/// Whether `text` fits within `max_chars` characters.
///
/// Counting stops at the first excess character, so a huge field is refused
/// without scanning all of it.
pub(crate) fn fits(text: &str, max_chars: usize) -> bool {
    text.chars().take(max_chars.saturating_add(1)).count() <= max_chars
}

/// Truncate `text` to at most `max_chars` characters.
///
/// When anything is removed the last three characters are `...`, so the
/// returned string never exceeds the limit.
pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return text.chars().take(max_chars).collect();
    }
    let mut output: String = text.chars().take(max_chars.saturating_sub(3)).collect();
    output.push_str("...");
    output
}

/// A bounded, human-readable excerpt of a line.
pub(crate) fn excerpt(text: &str) -> String {
    truncate_chars(text.trim(), REJECT_EXCERPT_CHARS)
}

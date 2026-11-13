//! CoreSight **inventory parsing** — the pure layer that turns raw,
//! caller-supplied system facts into the typed inventories the IPC surface
//! serves.
//!
//! The four IPC reports that need this data — startup inventory, DNS ranking
//! candidates, a block-list plan, and a privacy summary — each depend on
//! facts only the desktop shell can gather: a launchd plist, a registry
//! export, a hosts-style list, a browser profile listing. This crate is the
//! layer between those facts and the analyzers:
//!
//! * [`startup`] parses plists, registry rows, systemd units, and shell
//!   profile lines into [`coresight_systools::startup::StartupItem`]s.
//! * [`dns`] parses a documented text listing into
//!   [`coresight_webtools::DnsCandidate`]s.
//! * [`blocklist`] parses `domain[,category]` rules and delegates domain
//!   validation to [`coresight_webtools::plan_block_list`].
//! * [`privacy`] parses `kind,count,bytes_or_unknown` rows into
//!   [`coresight_webtools::PrivacyFinding`]s.
//!
//! The analysis itself — tiering, ranking, validation, summarization — stays
//! in `coresight-systools` and `coresight-webtools`. This crate does not
//! reimplement those rules; it produces their inputs and reports what it
//! could not read.
//!
//! ## Contract: a parser, not a collector
//!
//! **This crate performs no I/O.** There is no filesystem access, no registry
//! access, no process execution, no network client, no database, no clock,
//! and no environment inspection. Text and typed rows arrive as arguments;
//! typed inventories leave as return values. The platform shim that reads a
//! plist or exports a registry key is a thin shell outside this crate, which
//! is what makes the parsing testable without a machine and keeps the
//! untestable platform work small.
//!
//! ## Contract: total, never panicking
//!
//! Every parser accepts any `&str`: empty input, whitespace, NUL bytes, a
//! megabyte of junk, and deeply nested XML-like structure all return a
//! result. No production path uses a panicking construct. Empty input is an
//! empty inventory, not an error.
//!
//! ## Contract: rejected, never silently dropped
//!
//! Unparseable input is returned in a `rejected` list with a source (or
//! 1-based line number) and a reason. Nothing is skipped quietly, and nothing
//! malformed is defaulted into a permissive value. The one documented default
//! — an omitted block-list category becoming `Custom` — is stated in the
//! rule's reason and counted in
//! [`BlockParseResult::defaulted_categories`].
//!
//! ## Contract: unknown is not zero
//!
//! A privacy byte count of `unknown` or an empty field becomes
//! `estimated_bytes: None`, never `Some(0)`. Only a literal `0` becomes
//! `Some(0)`. Likewise, a DNS flag that is present but unreadable is a
//! rejected line, not `false`.
//!
//! ## Contract: deterministic
//!
//! Every returned inventory is sorted by a total key, so permuting the input
//! entries or lines cannot change it. Rejected lines keep their input line
//! numbers, which necessarily follow the text as supplied.
//!
//! ## What this crate deliberately does NOT do
//!
//! * It is **not** a plist parser or an XML validator. The launchd reader is
//!   a documented field extractor; it rejects binary plists and anything whose
//!   fields it cannot find. See [`startup`] for the exact limits.
//! * It is not a domain validator. [`blocklist`] passes rules to the
//!   webtools planner, which is the single authority.
//! * It does not probe DNS, rank candidates, summarize privacy, write a hosts
//!   file, read a browser profile, or change any startup entry. It has no
//!   execution primitive at all.
//! * It does not invent an enabled/disabled state. `RawStartupEntry` has no
//!   such signal; every parsed item is marked enabled and the limitation is
//!   documented in [`startup`].
//!
//! The desktop shell and the IPC adapters consume these types; they do not
//! reimplement the field extraction rules here.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod blocklist;
pub mod dns;
pub mod privacy;
pub mod startup;

mod text;

pub use blocklist::{parse_block_rules, BlockParseResult};
pub use dns::{parse_dns_candidates, DnsParseResult};
pub use privacy::{parse_privacy_listing, PrivacyParseResult};
pub use startup::{parse_startup, LaunchdScope, ParseReject, RawStartupEntry, StartupParseResult};
pub use text::LineReject;

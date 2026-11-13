//! The typed error and envelope contract (`docs/API_CONTRACTS.md` §Errors).
//!
//! Every command returns [`Envelope`]. Exactly one side is set: a success
//! payload ([`Envelope::ok`]) or an [`ApiError`] ([`Envelope::err`]). The
//! fields are private and there is no other constructor, so "both set" and
//! "neither set" are unrepresentable.
//!
//! Errors never carry a stack trace, an internal debug dump, or a raw OS
//! message. `message` is the user-facing sentence the UI may render
//! verbatim for a known code; `detail` is optional machine-readable
//! context. `PLAN_REJECTED_UNSAFE` publishes its contract `reasons[]` both
//! as a typed wire field and as the JSON array stored in `detail`. Unknown
//! codes must be shown by the UI as a generic safe fallback, so no code here
//! ever says "internal error" or exposes paths.

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// The stable error codes of the `coresight.v1.*` contract.
///
/// The first six are named by the contract itself. The rest are the typed
/// outcomes this dispatcher needs to be honest about what it cannot serve;
/// they are additive within `v1` and must stay stable once shipped.
pub mod codes {
    /// A scan is still running; results are only served after it finishes.
    pub const SCAN_IN_PROGRESS: &str = "SCAN_IN_PROGRESS";
    /// A remembered drive is offline and no retained snapshot can answer.
    pub const DRIVE_OFFLINE: &str = "DRIVE_OFFLINE";
    /// The safety validator vetoed a plan; `detail` carries the reasons.
    pub const PLAN_REJECTED_UNSAFE: &str = "PLAN_REJECTED_UNSAFE";
    /// The request selected no work to perform.
    pub const NOTHING_TO_DO: &str = "NOTHING_TO_DO";
    /// Reserved by the contract for a future scanner: some paths were not
    /// observable because the process lacks permission.
    pub const PERMISSION_GAP: &str = "PERMISSION_GAP";
    /// Reserved by the contract for a future history store: the database
    /// was corrupt and had to be rebuilt from a fresh scan.
    pub const DB_CORRUPT_RESCANNED: &str = "DB_CORRUPT_RESCANNED";
    /// The request named a command outside the `coresight.v1.*` surface.
    pub const UNKNOWN_COMMAND: &str = "UNKNOWN_COMMAND";
    /// The scan id is not known in this session.
    pub const SCAN_NOT_FOUND: &str = "SCAN_NOT_FOUND";
    /// The scan is known but is no longer running (already completed,
    /// cancelled, or failed), so it cannot be cancelled again.
    pub const SCAN_NOT_RUNNING: &str = "SCAN_NOT_RUNNING";
    /// The supplied scan root does not exist or cannot be observed.
    pub const SCAN_ROOT_UNAVAILABLE: &str = "SCAN_ROOT_UNAVAILABLE";
    /// The scan exists but its results cannot be served (cancelled, failed,
    /// or no scan backend is attached).
    pub const SCAN_UNAVAILABLE: &str = "SCAN_UNAVAILABLE";
    /// No history store is attached to this build.
    pub const HISTORY_UNAVAILABLE: &str = "HISTORY_UNAVAILABLE";
    /// The command exists in the contract but has no implementation here.
    pub const NOT_IMPLEMENTED_IN_THIS_BUILD: &str = "NOT_IMPLEMENTED_IN_THIS_BUILD";
    /// The request payload is malformed or violates the command's contract.
    pub const INVALID_REQUEST: &str = "INVALID_REQUEST";
    /// An opportunity id does not exist in this session.
    pub const UNKNOWN_OPPORTUNITY: &str = "UNKNOWN_OPPORTUNITY";
    /// A plan id does not exist in this session.
    pub const PLAN_NOT_FOUND: &str = "PLAN_NOT_FOUND";
    /// The plan was never accepted by `confirmPlan` in this session.
    pub const PLAN_NOT_CONFIRMED: &str = "PLAN_NOT_CONFIRMED";
    /// A drive id is not known in this session.
    pub const DRIVE_NOT_FOUND: &str = "DRIVE_NOT_FOUND";
    /// Selected opportunities cannot be pinned to concrete targets.
    pub const PLAN_UNRESOLVABLE: &str = "PLAN_UNRESOLVABLE";
    /// The session holds no completed scan or no registered report for a
    /// command that requires one. This is the honest "not available here"
    /// outcome; the dispatcher never substitutes sample data or zeros.
    pub const SESSION_UNAVAILABLE: &str = "SESSION_UNAVAILABLE";

    /// Every code this crate can emit (or reserves from the contract).
    ///
    /// The list is asserted unique by the contract tests so a typo cannot
    /// silently create a second spelling of the same outcome.
    pub const ALL: [&str; 20] = [
        SCAN_IN_PROGRESS,
        DRIVE_OFFLINE,
        PLAN_REJECTED_UNSAFE,
        NOTHING_TO_DO,
        PERMISSION_GAP,
        DB_CORRUPT_RESCANNED,
        UNKNOWN_COMMAND,
        SCAN_NOT_FOUND,
        SCAN_NOT_RUNNING,
        SCAN_ROOT_UNAVAILABLE,
        SCAN_UNAVAILABLE,
        HISTORY_UNAVAILABLE,
        NOT_IMPLEMENTED_IN_THIS_BUILD,
        INVALID_REQUEST,
        UNKNOWN_OPPORTUNITY,
        PLAN_NOT_FOUND,
        PLAN_NOT_CONFIRMED,
        DRIVE_NOT_FOUND,
        PLAN_UNRESOLVABLE,
        SESSION_UNAVAILABLE,
    ];
}

/// A typed command error.
///
/// `code` is a `&'static str` deliberately: a code is part of the compiled
/// contract, not caller data, and the UI switches on it. `message` is the
/// reviewed, user-safe sentence. `detail` is optional diagnostic context
/// and is never a stack trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    /// Stable contract code (see [`codes`]).
    pub code: &'static str,
    /// User-facing sentence the UI may render verbatim.
    pub message: String,
    /// Optional machine-readable context. For `PLAN_REJECTED_UNSAFE` this
    /// is a JSON array of human-readable reason strings (`reasons[]`).
    pub detail: Option<String>,
}

/// Serialize the contract's error shape:
/// `{ code, message, detail?, reasons? }`.
///
/// `reasons` is emitted for `PLAN_REJECTED_UNSAFE` from the same list that
/// `detail` carries, so a wire consumer can read the contract's `reasons[]`
/// without parsing a string. It is never emitted for other codes.
impl Serialize for ApiError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let reasons = self.rejection_reasons();
        let field_count = 2 + usize::from(self.detail.is_some()) + usize::from(!reasons.is_empty());
        let mut state = serializer.serialize_struct("ApiError", field_count)?;
        state.serialize_field("code", self.code)?;
        state.serialize_field("message", &self.message)?;
        if let Some(detail) = &self.detail {
            state.serialize_field("detail", detail)?;
        }
        if !reasons.is_empty() {
            state.serialize_field("reasons", &reasons)?;
        }
        state.end()
    }
}

impl ApiError {
    /// Build an error with no detail.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            code,
            message: message.into(),
            detail: None,
        }
    }

    /// Attach optional diagnostic context. Never attach a stack trace or a
    /// raw OS error dump here.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Build the `PLAN_REJECTED_UNSAFE` error for a safety veto.
    ///
    /// The contract's placement for `reasons[]` is a typed list. Because
    /// [`ApiError`] has exactly the contract's three fields, the reasons
    /// travel in `detail` as a JSON array; [`Self::rejection_reasons`]
    /// decodes them without guessing.
    pub fn plan_rejected(reasons: &[String]) -> Self {
        let detail = serde_json::to_string(reasons).unwrap_or_else(|_| "[]".to_string());
        ApiError {
            code: codes::PLAN_REJECTED_UNSAFE,
            message: "The plan was rejected by CoreSight's safety policy. \
                      The current build authorizes read-only actions only, so \
                      nothing was moved, deleted, or changed."
                .to_string(),
            detail: Some(detail),
        }
    }

    /// Decode the `reasons[]` of a `PLAN_REJECTED_UNSAFE` error.
    ///
    /// Returns an empty list for every other code and for a malformed
    /// detail; it never invents reasons.
    pub fn rejection_reasons(&self) -> Vec<String> {
        if self.code != codes::PLAN_REJECTED_UNSAFE {
            return Vec::new();
        }
        let Some(detail) = self.detail.as_deref() else {
            return Vec::new();
        };
        serde_json::from_str::<Vec<String>>(detail).unwrap_or_default()
    }
}

/// The command result frame: exactly one of `ok` / `error`.
///
/// Construction is private, so a value that carries both a payload and an
/// error (or neither) cannot be built. Serialization omits the unset side,
/// which gives the UI an unambiguous shape to switch on.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[must_use = "an Envelope is the command result; inspect ok/error before dropping it"]
pub struct Envelope<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ApiError>,
}

impl<T> Envelope<T> {
    /// A successful result. This is the only way to set `ok`.
    pub fn ok(value: T) -> Self {
        Envelope {
            ok: Some(value),
            error: None,
        }
    }

    /// A typed failure. This is the only way to set `error`.
    pub fn err(error: ApiError) -> Self {
        Envelope {
            ok: None,
            error: Some(error),
        }
    }

    /// Whether the envelope carries a success payload.
    pub fn is_ok(&self) -> bool {
        self.ok.is_some()
    }

    /// Whether the envelope carries a typed error.
    pub fn is_err(&self) -> bool {
        self.error.is_some()
    }

    /// Borrow the success payload, if any.
    pub fn ok_value(&self) -> Option<&T> {
        self.ok.as_ref()
    }

    /// Borrow the typed error, if any.
    pub fn error(&self) -> Option<&ApiError> {
        self.error.as_ref()
    }

    /// Consume the envelope as a `Result`.
    ///
    /// The final arm is unreachable through the public constructors; it
    /// still returns a typed error instead of panicking so no code path in
    /// this crate can abort the host process.
    pub fn into_result(self) -> Result<T, ApiError> {
        match (self.ok, self.error) {
            (Some(value), _) => Ok(value),
            (None, Some(error)) => Err(error),
            (None, None) => Err(ApiError::new(
                codes::INVALID_REQUEST,
                "The command returned no result. Nothing was changed.",
            )),
        }
    }
}

/// Sanitize a caller-supplied identifier before putting it in `detail`.
///
/// Ids are echoed only for diagnosis. Control characters, quotes, and
/// path separators are replaced so a hostile id cannot forge log lines or
/// smuggle a second message. The value is capped so an enormous id cannot
/// bloat the response.
pub(crate) fn detail_token(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(80));
    let mut truncated = false;
    for (index, ch) in value.chars().enumerate() {
        if index >= 80 {
            truncated = true;
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/' | '\\') {
            out.push(ch);
        } else {
            out.push('?');
        }
    }
    if truncated {
        out.push('~');
    }
    out
}

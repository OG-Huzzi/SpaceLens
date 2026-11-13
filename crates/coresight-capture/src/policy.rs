//! Privacy guarantees: the capture policy is local-only by construction.
//!
//! This module exists because the unsafe path must be *unrepresentable in
//! practice*, not merely discouraged. There is no upload function in this
//! crate and no network dependency in its manifest; [`upload_supported`]
//! returns `false` as a compile-time fact about the build, and
//! [`default_policy`] sets `upload_enabled: false` so the UI can display the
//! guarantee instead of asking the user to trust a promise.
//!
//! ## Any future sharing feature is a separate, explicitly authorized change
//!
//! `CapturePolicy::upload_enabled` is a *report*, not a switch. Flipping it in
//! a config file, a settings migration, or a feature flag would not add an
//! upload path — there is nothing for it to call. Adding one requires a new
//! module, a new dependency review, a user consent flow, and a capability
//! gate; that is a deliberate multi-step change, not a flag flip. Tests pin
//! the default and [`policy_violations`] lets the shell refuse a policy that
//! claims otherwise.
//!
//! ## Sensitive regions are obligations, not geometry
//!
//! [`SensitiveRegion`] names a *class* of area the shell must omit or mask
//! before the image is saved. It deliberately carries no coordinates: this
//! crate has no access to the screen and must not pretend to compute pixel
//! geometry it cannot see. The shell translates each obligation into window
//! and rectangle decisions at capture time, and a capture that cannot satisfy
//! an `Omit` obligation must not be taken.

use serde::{Deserialize, Serialize};

/// A region the shell must treat specially before saving a capture.
///
/// `Omit` and `Mask` are different promises: omitting hides the area
/// entirely (no pixels at all), masking replaces it with an opaque block.
/// A credential dialog cannot be "masked a bit"; it must be left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SensitiveRegion {
    /// A region the shell must not include at all.
    Omit {
        /// Why this region must be absent, in language the user can read.
        reason: String,
    },
    /// A region the shell must mask.
    Mask {
        /// Why this region must be masked, in language the user can read.
        reason: String,
    },
}

impl SensitiveRegion {
    /// Why this region is sensitive. Always safe to show the user.
    pub fn reason(&self) -> &str {
        match self {
            SensitiveRegion::Omit { reason } | SensitiveRegion::Mask { reason } => reason,
        }
    }

    /// Whether the region must be left out entirely rather than masked.
    pub fn must_omit(&self) -> bool {
        matches!(self, SensitiveRegion::Omit { .. })
    }
}

/// The policy the capture flow must obey.
///
/// The fields are public so the UI can render the guarantee and the shell can
/// audit it; they are not configuration knobs for a capture to loosen at
/// runtime. [`default_policy`] is the only policy the application builds, and
/// [`policy_violations`] reports anything that departs from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePolicy {
    /// Always false in this build. There is no upload path in the codebase.
    pub upload_enabled: bool,
    /// True: the user must confirm the destination before the first capture.
    pub require_destination_confirmation: bool,
    /// Areas that must be redacted/omitted by the shell before saving.
    pub sensitive_regions: Vec<SensitiveRegion>,
}

impl CapturePolicy {
    /// Build the local-only policy.
    ///
    /// It is the same policy as [`default_policy`]; the named constructor
    /// exists so a call site states the guarantee it relies on instead of
    /// depending on an unstated default.
    pub fn local_only() -> Self {
        default_policy()
    }

    /// Whether this policy keeps every capture on the machine.
    ///
    /// The shell checks this before opening the capture UI; a policy that
    /// fails it is a bug to surface, not a mode to run in.
    pub fn is_local_only(&self) -> bool {
        !self.upload_enabled
    }
}

/// Whether this build can upload a capture anywhere.
///
/// Always `false`. The function exists so callers (and the UI) ask the build
/// instead of hard-coding an assumption that could drift; there is no code
/// path behind it that could ever return `true` without a new module, a new
/// dependency, and a new consent flow.
#[must_use]
pub fn upload_supported() -> bool {
    false
}

/// The default, safe capture policy.
///
/// * `upload_enabled: false` — a screenshot never leaves the machine. A
///   sharing feature would be a separate, explicitly authorized change, not
///   a flag flip of this value.
/// * `require_destination_confirmation: true` — the first capture of a
///   session asks the user where it goes, so "it saved somewhere" is never a
///   surprise.
/// * Three default obligations: credential surfaces are omitted, password
///   and one-time-code fields are masked, and notification toasts are masked
///   (they routinely carry message previews and 2FA codes).
#[must_use]
pub fn default_policy() -> CapturePolicy {
    CapturePolicy {
        upload_enabled: false,
        require_destination_confirmation: true,
        sensitive_regions: vec![
            SensitiveRegion::Omit {
                reason: "Credential and password-manager surfaces (OS sign-in, UAC prompts, \
                         vault windows) must not appear in a screenshot at all."
                    .to_string(),
            },
            SensitiveRegion::Mask {
                reason: "Password fields and one-time-code inputs must be masked before the \
                         image is written."
                    .to_string(),
            },
            SensitiveRegion::Mask {
                reason: "Notification toasts must be masked: they routinely show private \
                         message previews, calendar details, and two-factor codes."
                    .to_string(),
            },
        ],
    }
}

/// Audit a policy before the shell acts on it.
///
/// Returns one message per guarantee the policy does not satisfy; an empty
/// vector means the policy is safe to act on. This is how a future settings
/// surface, a migration, or a deserialized policy is checked: the shell can
/// refuse to capture instead of discovering the problem after a file exists.
#[must_use]
pub fn policy_violations(policy: &CapturePolicy) -> Vec<&'static str> {
    let mut violations: Vec<&'static str> = Vec::new();

    if policy.upload_enabled {
        violations.push(
            "uploadEnabled is true, but this build has no upload path: a capture must never \
             leave the machine.",
        );
    }
    if !policy.require_destination_confirmation {
        violations.push(
            "requireDestinationConfirmation is false: the user must confirm where a capture is \
             written before the first capture.",
        );
    }
    if policy.sensitive_regions.is_empty() {
        violations.push(
            "sensitiveRegions is empty: the shell would save credential and notification pixels \
             unredacted.",
        );
    }

    violations
}

//! Typed contract for startup / login-item and launchd discovery
//! (Phase 6.1, contracts C and D). Model only — no discovery provider
//! exists yet, and nothing in this build ever disables a startup item.
//!
//! Future macOS providers (LaunchAgent/Daemon plist enumeration, login-item
//! APIs) report through [`crate::Observation`] / [`crate::CapabilityReport`]
//! and must keep every unknown an explicit `Option` — `enabled: None` means
//! unknown, never guessed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Which startup mechanism a discovered item uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StartupMechanism {
    /// A launchd user agent (`~/Library/LaunchAgents`).
    LaunchAgent,
    /// A launchd system daemon/agent (`/Library/Launch{Agents,Daemons}`).
    LaunchDaemon,
    /// A login item (modern BTM records or the legacy shared-file list).
    LoginItem,
    Unknown,
}

impl StartupMechanism {
    pub fn tag(self) -> &'static str {
        match self {
            StartupMechanism::LaunchAgent => "launch-agent",
            StartupMechanism::LaunchDaemon => "launch-daemon",
            StartupMechanism::LoginItem => "login-item",
            StartupMechanism::Unknown => "unknown",
        }
    }
}

/// Whose startup the item belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StartupScope {
    CurrentUser,
    AllUsers,
    Unknown,
}

impl StartupScope {
    pub fn tag(self) -> &'static str {
        match self {
            StartupScope::CurrentUser => "current-user",
            StartupScope::AllUsers => "all-users",
            StartupScope::Unknown => "unknown",
        }
    }
}

/// Stable, content-derived identifier (same inputs → same id), mirroring
/// the `coresight_apps::ApplicationId` convention. Hashed over the OS-level
/// bytes of the owner path, so non-UTF-8 paths cannot collide.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StartupItemId(pub String);

impl StartupItemId {
    pub fn derive(
        mechanism: StartupMechanism,
        scope: StartupScope,
        label: Option<&str>,
        owner_path: Option<&Path>,
    ) -> Self {
        let mut key = Vec::new();
        key.extend_from_slice(mechanism.tag().as_bytes());
        key.push(b'|');
        key.extend_from_slice(scope.tag().as_bytes());
        key.push(b'|');
        key.extend_from_slice(label.unwrap_or("").as_bytes());
        key.push(b'|');
        if let Some(p) = owner_path {
            key.extend_from_slice(p.as_os_str().as_encoded_bytes());
        }
        let digest = Sha256::digest(&key);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        StartupItemId(format!("startup-{hex}"))
    }
}

/// One discovered startup item: a typed contract, not an executed action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupItem {
    pub id: StartupItemId,
    pub mechanism: StartupMechanism,
    pub scope: StartupScope,
    pub label: Option<String>,
    /// The plist/bundle/record path when the mechanism has one.
    pub owner_path: Option<PathBuf>,
    /// `None` = unknown; never guessed from the name.
    pub enabled: Option<bool>,
    /// Provenance strings naming the source of every claim.
    pub evidence: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_deterministic_and_input_sensitive() {
        let path = Path::new("/Library/LaunchDaemons/com.example.svc.plist");
        let a = StartupItemId::derive(
            StartupMechanism::LaunchDaemon,
            StartupScope::AllUsers,
            Some("com.example.svc"),
            Some(path),
        );
        let b = StartupItemId::derive(
            StartupMechanism::LaunchDaemon,
            StartupScope::AllUsers,
            Some("com.example.svc"),
            Some(path),
        );
        assert_eq!(a, b, "same inputs derive the same id");
        assert!(a.0.starts_with("startup-"));

        let other_label = StartupItemId::derive(
            StartupMechanism::LaunchDaemon,
            StartupScope::AllUsers,
            Some("com.example.other"),
            Some(path),
        );
        let other_scope = StartupItemId::derive(
            StartupMechanism::LaunchDaemon,
            StartupScope::CurrentUser,
            Some("com.example.svc"),
            Some(path),
        );
        assert_ne!(a, other_label);
        assert_ne!(a, other_scope);
    }

    #[test]
    fn serde_round_trip() {
        let item = StartupItem {
            id: StartupItemId::derive(
                StartupMechanism::LaunchAgent,
                StartupScope::CurrentUser,
                Some("com.example.agent"),
                None,
            ),
            mechanism: StartupMechanism::LaunchAgent,
            scope: StartupScope::CurrentUser,
            label: Some("com.example.agent".into()),
            owner_path: Some(PathBuf::from("~/Library/LaunchAgents/a.plist")),
            enabled: None,
            evidence: vec!["catalog:user-launch-agents".into()],
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("LAUNCH_AGENT"));
        let back: StartupItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back, item);
    }
}

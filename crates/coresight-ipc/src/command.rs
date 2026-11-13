//! The ten typed commands of `coresight.v1.*` (`docs/API_CONTRACTS.md`
//! §Commands).
//!
//! On the wire every command is an adjacent-tagged pair:
//!
//! ```json
//! { "command": "coresight.v1.startScan",
//!   "payload": { "driveId": "drive-c", "fullRescan": true } }
//! ```
//!
//! The discriminant is the full namespaced contract name — not a short
//! Rust variant name — so a log line, a proxy, or a future language binding
//! can route on it without a translation table. [`Command::name`] returns
//! the same string for shells that build the frame themselves.
//!
//! The request structs are deliberately narrow: only the arguments the
//! contract documents. Unknown extra fields are ignored so `v1` can grow
//! additively; missing or wrongly typed required fields become a typed
//! `INVALID_REQUEST` when the frame is parsed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{codes, ApiError};

/// The `coresight.v1.startScan` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartScanRequest {
    /// The drive to scan.
    pub drive_id: String,
    /// Force a full rescan instead of an incremental one. `None` means the
    /// shell did not specify a preference; the backend decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_rescan: Option<bool>,
}

/// The `coresight.v1.cancelScan` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelScanRequest {
    /// The scan to cancel.
    pub scan_id: String,
}

/// The `coresight.v1.getDriveSummary` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetDriveSummaryRequest {
    /// The drive whose summary is requested.
    pub drive_id: String,
}

/// The `coresight.v1.getCategories` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCategoriesRequest {
    /// The completed scan to read.
    pub scan_id: String,
    /// When set, return that category's subcategory breakdown instead of
    /// the top-level category list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_category: Option<String>,
}

/// The `coresight.v1.getOpportunities` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetOpportunitiesRequest {
    /// The completed scan to read.
    pub scan_id: String,
}

/// The `coresight.v1.previewPlan` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPlanRequest {
    /// Opportunity ids selected by the user, in the order the preview
    /// should present them. Duplicates and unknown ids are refused.
    pub opportunity_ids: Vec<String>,
}

/// The `coresight.v1.confirmPlan` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmPlanRequest {
    /// The plan returned by `previewPlan` in this session.
    pub plan_id: String,
    /// The exact warning strings the user acknowledged. Warnings are not
    /// optional: an unacknowledged warning blocks acceptance.
    #[serde(default)]
    pub acked_warnings: Vec<String>,
}

/// The `coresight.v1.executePlan` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutePlanRequest {
    /// The plan previously accepted by `confirmPlan` in this session.
    pub plan_id: String,
}

/// The `coresight.v1.getHistory` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetHistoryRequest {
    /// The drive whose snapshot deltas are requested.
    pub drive_id: String,
}

/// The `coresight.v1.listDrives` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListDrivesRequest {}

/// One command of the `coresight.v1.*` surface.
///
/// The serde representation is the transport frame documented on the
/// module: a namespaced `command` discriminant plus a typed `payload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", content = "payload")]
pub enum Command {
    /// Start a new scan (requires a scan backend; none is attached here).
    #[serde(rename = "coresight.v1.startScan")]
    StartScan(StartScanRequest),
    /// Cancel a running scan (requires a scan backend; none is attached here).
    #[serde(rename = "coresight.v1.cancelScan")]
    CancelScan(CancelScanRequest),
    /// Totals, exclusions note, last scan, and retained snapshot count.
    #[serde(rename = "coresight.v1.getDriveSummary")]
    GetDriveSummary(GetDriveSummaryRequest),
    /// Category (or subcategory) byte totals for one completed scan.
    #[serde(rename = "coresight.v1.getCategories")]
    GetCategories(GetCategoriesRequest),
    /// Cleanup opportunities for one completed scan.
    #[serde(rename = "coresight.v1.getOpportunities")]
    GetOpportunities(GetOpportunitiesRequest),
    /// Resolve selected opportunities into a reviewable preview.
    #[serde(rename = "coresight.v1.previewPlan")]
    PreviewPlan(PreviewPlanRequest),
    /// Run safety validation on a preview synchronously.
    #[serde(rename = "coresight.v1.confirmPlan")]
    ConfirmPlan(ConfirmPlanRequest),
    /// Execute an accepted plan (requires a policy and an executor).
    #[serde(rename = "coresight.v1.executePlan")]
    ExecutePlan(ExecutePlanRequest),
    /// Snapshot deltas (requires a history store; none is attached here).
    #[serde(rename = "coresight.v1.getHistory")]
    GetHistory(GetHistoryRequest),
    /// Connected and remembered-offline drives known to this session.
    #[serde(rename = "coresight.v1.listDrives")]
    ListDrives(ListDrivesRequest),
}

impl Command {
    /// The full namespaced wire names, in contract order.
    pub const ALL_NAMES: [&'static str; 10] = [
        "coresight.v1.startScan",
        "coresight.v1.cancelScan",
        "coresight.v1.getDriveSummary",
        "coresight.v1.getCategories",
        "coresight.v1.getOpportunities",
        "coresight.v1.previewPlan",
        "coresight.v1.confirmPlan",
        "coresight.v1.executePlan",
        "coresight.v1.getHistory",
        "coresight.v1.listDrives",
    ];

    /// The namespaced wire name of this command.
    pub fn name(&self) -> &'static str {
        match self {
            Command::StartScan(_) => Command::ALL_NAMES[0],
            Command::CancelScan(_) => Command::ALL_NAMES[1],
            Command::GetDriveSummary(_) => Command::ALL_NAMES[2],
            Command::GetCategories(_) => Command::ALL_NAMES[3],
            Command::GetOpportunities(_) => Command::ALL_NAMES[4],
            Command::PreviewPlan(_) => Command::ALL_NAMES[5],
            Command::ConfirmPlan(_) => Command::ALL_NAMES[6],
            Command::ExecutePlan(_) => Command::ALL_NAMES[7],
            Command::GetHistory(_) => Command::ALL_NAMES[8],
            Command::ListDrives(_) => Command::ALL_NAMES[9],
        }
    }

    /// Parse a transport frame (`{ "command": ..., "payload": ... }`).
    ///
    /// Unknown names are a typed [`codes::UNKNOWN_COMMAND`] error — never
    /// a panic and never a guessed command. A malformed frame or payload is
    /// a typed [`codes::INVALID_REQUEST`] error with the serde field path
    /// (never a value dump or a backtrace).
    pub fn from_wire(value: &Value) -> Result<Command, ApiError> {
        let object = value.as_object().ok_or_else(|| {
            ApiError::new(
                codes::INVALID_REQUEST,
                "The request is not a JSON object. Nothing was changed.",
            )
        })?;
        let name = object
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ApiError::new(
                    codes::INVALID_REQUEST,
                    "The request is missing a string `command` field. Nothing was changed.",
                )
            })?;
        let payload = object
            .get("payload")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        Command::from_name(name, payload)
    }

    /// Parse one command by its namespaced name and payload.
    ///
    /// This is the mapping function a Tauri shell uses when it already has
    /// the command name and the argument object separately.
    pub fn from_name(name: &str, payload: Value) -> Result<Command, ApiError> {
        match name {
            "coresight.v1.startScan" => deserialize_payload(name, payload).map(Command::StartScan),
            "coresight.v1.cancelScan" => {
                deserialize_payload(name, payload).map(Command::CancelScan)
            }
            "coresight.v1.getDriveSummary" => {
                deserialize_payload(name, payload).map(Command::GetDriveSummary)
            }
            "coresight.v1.getCategories" => {
                deserialize_payload(name, payload).map(Command::GetCategories)
            }
            "coresight.v1.getOpportunities" => {
                deserialize_payload(name, payload).map(Command::GetOpportunities)
            }
            "coresight.v1.previewPlan" => {
                deserialize_payload(name, payload).map(Command::PreviewPlan)
            }
            "coresight.v1.confirmPlan" => {
                deserialize_payload(name, payload).map(Command::ConfirmPlan)
            }
            "coresight.v1.executePlan" => {
                deserialize_payload(name, payload).map(Command::ExecutePlan)
            }
            "coresight.v1.getHistory" => {
                deserialize_payload(name, payload).map(Command::GetHistory)
            }
            "coresight.v1.listDrives" => {
                let request: ListDrivesRequest = deserialize_payload(name, payload)?;
                Ok(Command::ListDrives(request))
            }
            _ => Err(ApiError::new(
                codes::UNKNOWN_COMMAND,
                "The request named a command that is not part of the coresight.v1 contract. \
                 Nothing was changed.",
            )
            .with_detail(format!("unknown command name ({} bytes)", name.len()))),
        }
    }
}

fn deserialize_payload<T: serde::de::DeserializeOwned>(
    name: &str,
    payload: Value,
) -> Result<T, ApiError> {
    serde_json::from_value(payload).map_err(|error| {
        ApiError::new(
            codes::INVALID_REQUEST,
            "The request payload does not match the command's contract. Nothing was changed.",
        )
        .with_detail(format!(
            "{} payload: {}",
            name,
            truncate(&error.to_string(), 240)
        ))
    })
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out = String::with_capacity(max + 1);
    for ch in value.chars().take(max) {
        out.push(ch);
    }
    out.push('~');
    out
}

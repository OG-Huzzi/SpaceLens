//! The typed commands of `coresight.v1.*` (`docs/API_CONTRACTS.md`
//! §Commands): the original ten engine commands plus the ten aggregate view
//! commands consumed by `src/api.ts`.
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
///
/// `root` is the real directory (or single file) to walk. When it is absent
/// the backend scans the platform home directory reported by the engine's
/// `SysDirs` boundary. `driveId` is retained from the original v1 frame for
/// compatibility; it names the drive the scan is registered against and may
/// be omitted, in which case the session derives one from the real root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartScanRequest {
    /// The drive to scan. Optional: a scan started from a bare `root` gets a
    /// drive record derived from that root.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub drive_id: String,
    /// Force a full rescan instead of an incremental one. `None` means the
    /// shell did not specify a preference; the backend decides. This build
    /// keeps no incremental index, so every scan is a full walk either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_rescan: Option<bool>,
    /// Absolute path to walk. `None` means the platform home directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<std::path::PathBuf>,
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

/// The `coresight.v1.getGlobalStatus` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetGlobalStatusRequest {}

/// The `coresight.v1.getHomeSummary` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetHomeSummaryRequest {}

/// The `coresight.v1.getStorage` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetStorageRequest {}

/// The `coresight.v1.getCleanup` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetCleanupRequest {}

/// The `coresight.v1.getDuplicateReport` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetDuplicateReportRequest {}

/// The `coresight.v1.getBigFileReport` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetBigFileReportRequest {}

/// The `coresight.v1.getStartupReport` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetStartupReportRequest {}

/// The `coresight.v1.getDnsRanking` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetDnsRankingRequest {}

/// The `coresight.v1.getBlockListPlan` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetBlockListPlanRequest {}

/// The `coresight.v1.getPrivacySummary` request (no arguments).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetPrivacySummaryRequest {}

/// One command of the `coresight.v1.*` surface.
///
/// The serde representation is the transport frame documented on the
/// module: a namespaced `command` discriminant plus a typed `payload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", content = "payload")]
pub enum Command {
    /// Start a new scan (requires a scan backend; the shell attaches the real
    /// engine backend).
    #[serde(rename = "coresight.v1.startScan")]
    StartScan(StartScanRequest),
    /// Cancel a running scan (requires a scan backend; the shell attaches the
    /// real engine backend).
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
    /// Whole-app status: contract version, latest scan state, and whether the
    /// payload is simulated (never true for this dispatcher).
    #[serde(rename = "coresight.v1.getGlobalStatus")]
    GetGlobalStatus(GetGlobalStatusRequest),
    /// Home screen aggregate: reclaimable bytes, drives, top categories, and
    /// the change since the previous full-scope scan.
    #[serde(rename = "coresight.v1.getHomeSummary")]
    GetHomeSummary(GetHomeSummaryRequest),
    /// Storage screen: category totals plus the duplicate and big-file
    /// reports projected from the latest completed scan.
    #[serde(rename = "coresight.v1.getStorage")]
    GetStorage(GetStorageRequest),
    /// Cleanup screen: the planner's actionable items and the off-limits
    /// categories it exposed but never proposes.
    #[serde(rename = "coresight.v1.getCleanup")]
    GetCleanup(GetCleanupRequest),
    /// Duplicate report for the latest completed scan.
    #[serde(rename = "coresight.v1.getDuplicateReport")]
    GetDuplicateReport(GetDuplicateReportRequest),
    /// Big-file report for the latest completed scan.
    #[serde(rename = "coresight.v1.getBigFileReport")]
    GetBigFileReport(GetBigFileReportRequest),
    /// Startup inventory analysis registered into this session.
    #[serde(rename = "coresight.v1.getStartupReport")]
    GetStartupReport(GetStartupReportRequest),
    /// DNS benchmark ranking registered into this session.
    #[serde(rename = "coresight.v1.getDnsRanking")]
    GetDnsRanking(GetDnsRankingRequest),
    /// Planned hosts-file block list registered into this session.
    #[serde(rename = "coresight.v1.getBlockListPlan")]
    GetBlockListPlan(GetBlockListPlanRequest),
    /// Privacy findings summary registered into this session.
    #[serde(rename = "coresight.v1.getPrivacySummary")]
    GetPrivacySummary(GetPrivacySummaryRequest),
}

impl Command {
    /// The original ten namespaced wire names, in contract order.
    ///
    /// Kept as the stable prefix of the `v1` table. [`Command::ALL_NAMES_V1`]
    /// is the complete table this build serves.
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

    /// Every namespaced wire name this build serves, in contract order: the
    /// original ten followed by the ten aggregate view commands.
    pub const ALL_NAMES_V1: [&'static str; 20] = [
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
        "coresight.v1.getGlobalStatus",
        "coresight.v1.getHomeSummary",
        "coresight.v1.getStorage",
        "coresight.v1.getCleanup",
        "coresight.v1.getDuplicateReport",
        "coresight.v1.getBigFileReport",
        "coresight.v1.getStartupReport",
        "coresight.v1.getDnsRanking",
        "coresight.v1.getBlockListPlan",
        "coresight.v1.getPrivacySummary",
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
            Command::GetGlobalStatus(_) => Command::ALL_NAMES_V1[10],
            Command::GetHomeSummary(_) => Command::ALL_NAMES_V1[11],
            Command::GetStorage(_) => Command::ALL_NAMES_V1[12],
            Command::GetCleanup(_) => Command::ALL_NAMES_V1[13],
            Command::GetDuplicateReport(_) => Command::ALL_NAMES_V1[14],
            Command::GetBigFileReport(_) => Command::ALL_NAMES_V1[15],
            Command::GetStartupReport(_) => Command::ALL_NAMES_V1[16],
            Command::GetDnsRanking(_) => Command::ALL_NAMES_V1[17],
            Command::GetBlockListPlan(_) => Command::ALL_NAMES_V1[18],
            Command::GetPrivacySummary(_) => Command::ALL_NAMES_V1[19],
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
            "coresight.v1.getGlobalStatus" => {
                deserialize_payload(name, payload).map(Command::GetGlobalStatus)
            }
            "coresight.v1.getHomeSummary" => {
                deserialize_payload(name, payload).map(Command::GetHomeSummary)
            }
            "coresight.v1.getStorage" => {
                deserialize_payload(name, payload).map(Command::GetStorage)
            }
            "coresight.v1.getCleanup" => {
                deserialize_payload(name, payload).map(Command::GetCleanup)
            }
            "coresight.v1.getDuplicateReport" => {
                deserialize_payload(name, payload).map(Command::GetDuplicateReport)
            }
            "coresight.v1.getBigFileReport" => {
                deserialize_payload(name, payload).map(Command::GetBigFileReport)
            }
            "coresight.v1.getStartupReport" => {
                deserialize_payload(name, payload).map(Command::GetStartupReport)
            }
            "coresight.v1.getDnsRanking" => {
                deserialize_payload(name, payload).map(Command::GetDnsRanking)
            }
            "coresight.v1.getBlockListPlan" => {
                deserialize_payload(name, payload).map(Command::GetBlockListPlan)
            }
            "coresight.v1.getPrivacySummary" => {
                deserialize_payload(name, payload).map(Command::GetPrivacySummary)
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

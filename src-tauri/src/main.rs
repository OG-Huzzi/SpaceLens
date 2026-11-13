//! CoreSight Tauri 2 shell — transport only.
//!
//! This crate owns exactly three things: the desktop window (declared in
//! `tauri.conf.json`), the `coresight.v1.*` command table, and the bridge
//! that drains scan progress out of `coresight-ipc` and emits it as Tauri
//! events. Every command is a thin adapter: build the typed
//! `coresight_ipc::Command` from the invoke arguments, hand it to
//! `coresight_ipc::dispatch` under the single managed `Session`, and return
//! the resulting `Envelope<Response>` unchanged.
//!
//! # Result framing
//!
//! Every command returns the crate's `Envelope<Response>` verbatim, so the UI
//! sees one shape for all twenty commands:
//!
//! ```json
//! { "ok":    { "kind": "coresight.v1.listDrives", "payload": [ /* Drive[] */ ] } }
//! { "error": { "code": "...", "message": "...", "detail": "..." } }
//! ```
//!
//! Nothing in the shell rewrites, unwraps or inspects a payload.
//!
//! # Scanning
//!
//! The shell attaches the real `coresight_ipc::EngineScanBackend` and stores
//! each started scan's `ScanHandle` in `ShellState`. A single background
//! thread drains those handles, applies every update to the session (so
//! `getGlobalStatus` sees live counters and the final snapshot), and emits
//! the two contract events:
//!
//! * `coresight.v1.scanProgress` — `{ scanId, filesSeen, bytesSeen,
//!   currentCategory?, etaSec? }`
//! * `coresight.v1.scanComplete` — `{ scanId, status, summary }`
//!
//! All scan business logic (root resolution, classification, snapshot,
//! hashing, cancellation, typed errors) lives in `coresight-ipc`; this file
//! only translates.
//!
//! # Honest unavailability
//!
//! History and plan execution remain unavailable (`HISTORY_UNAVAILABLE` and
//! the typed `PLAN_REJECTED_UNSAFE` rejection under the read-only execution
//! policy). That is the safety contract working as designed — the shell must
//! not paper over it with sample data.
//!
//! # Invoke names
//!
//! Tauri derives the JavaScript command name from the Rust function name, and a
//! Rust identifier cannot contain dots, so each contract name maps one-to-one
//! onto the handler below:
//!
//! | Contract command (`docs/API_CONTRACTS.md`) | `invoke` name        |
//! |--------------------------------------------|----------------------|
//! | `coresight.v1.startScan`                   | `start_scan`         |
//! | `coresight.v1.cancelScan`                  | `cancel_scan`        |
//! | `coresight.v1.getDriveSummary`             | `get_drive_summary`  |
//! | `coresight.v1.getCategories`               | `get_categories`     |
//! | `coresight.v1.getOpportunities`            | `get_opportunities`  |
//! | `coresight.v1.previewPlan`                 | `preview_plan`       |
//! | `coresight.v1.confirmPlan`                 | `confirm_plan`       |
//! | `coresight.v1.executePlan`                 | `execute_plan`       |
//! | `coresight.v1.getHistory`                  | `get_history`        |
//! | `coresight.v1.listDrives`                  | `list_drives`        |
//! | `coresight.v1.getGlobalStatus`             | `get_global_status`  |
//! | `coresight.v1.getHomeSummary`              | `get_home_summary`   |
//! | `coresight.v1.getStorage`                  | `get_storage`        |
//! | `coresight.v1.getCleanup`                  | `get_cleanup`        |
//! | `coresight.v1.getDuplicateReport`          | `get_duplicate_report` |
//! | `coresight.v1.getBigFileReport`            | `get_big_file_report` |
//! | `coresight.v1.getStartupReport`            | `get_startup_report` |
//! | `coresight.v1.getDnsRanking`               | `get_dns_ranking`    |
//! | `coresight.v1.getBlockListPlan`            | `get_block_list_plan` |
//! | `coresight.v1.getPrivacySummary`           | `get_privacy_summary` |
//!
//! Arguments use the documented camelCase shape, for example
//! `invoke("start_scan", { root, driveId?, fullRescan? })`. The ten aggregate
//! view commands take no arguments.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use coresight_ipc::{
    ApiError, CancelScanRequest, Command, ConfirmPlanRequest, EngineScanBackend, Envelope,
    ExecutePlanRequest, GetBigFileReportRequest, GetBlockListPlanRequest, GetCategoriesRequest,
    GetCleanupRequest, GetDnsRankingRequest, GetDriveSummaryRequest, GetDuplicateReportRequest,
    GetGlobalStatusRequest, GetHistoryRequest, GetHomeSummaryRequest, GetOpportunitiesRequest,
    GetPrivacySummaryRequest, GetStartupReportRequest, GetStorageRequest, ListDrivesRequest,
    PreviewPlanRequest, Response, ScanHandle, ScanProgressUpdate, Session, StartScanRequest,
};
use tauri::{Emitter, State};

/// Transport-level code used only when an earlier command panicked while
/// holding the session lock.
///
/// This is deliberately not one of the engine's `coresight.v1` codes: the UI's
/// documented behaviour for an unknown code is its generic safe fallback, which
/// is the honest presentation for a shell-level fault.
const SESSION_UNAVAILABLE: &str = "SESSION_UNAVAILABLE";

/// The single process-wide IPC session plus the registry of live scan
/// handles.
///
/// All command handlers are synchronous on purpose: the `MutexGuard` is
/// taken, used and dropped inside one call frame, so it can never be held
/// across an `.await` and the UI cannot deadlock itself by awaiting a command
/// that waits for it.
struct ShellState {
    session: Mutex<Session>,
    /// The handles of scans that are still producing updates. The shell owns
    /// the receivers; the session owns the cancellation tokens.
    scans: Mutex<BTreeMap<String, ScanHandle>>,
}

/// Dispatch under the session lock and return the envelope untouched.
fn forward(state: State<'_, Arc<ShellState>>, command: Command) -> Envelope<Response> {
    dispatch_session(&state.session, command)
}

/// The lock-and-dispatch helper shared by every handler.
fn dispatch_session(session: &Mutex<Session>, command: Command) -> Envelope<Response> {
    match session.lock() {
        Ok(mut session) => coresight_ipc::dispatch(&mut session, command),
        Err(_) => Envelope::err(ApiError::new(
            SESSION_UNAVAILABLE,
            "The CoreSight session is unavailable because an earlier command \
             failed unexpectedly. Restart CoreSight; nothing was changed.",
        )),
    }
}

// ---------------------------------------------------------------------------
// Command table — one thin handler per `coresight.v1.*` contract command.
// Argument names are snake_case in Rust and camelCase on the wire (Tauri's
// default conversion), matching docs/API_CONTRACTS.md.
// ---------------------------------------------------------------------------

#[tauri::command]
fn start_scan(
    state: State<'_, Arc<ShellState>>,
    drive_id: Option<String>,
    full_rescan: Option<bool>,
    root: Option<String>,
) -> Envelope<Response> {
    let envelope = dispatch_session(
        &state.session,
        Command::StartScan(StartScanRequest {
            drive_id: drive_id.unwrap_or_default(),
            full_rescan,
            root: root.map(PathBuf::from),
        }),
    );

    // A successful start is always followed by taking its handle so the
    // background drain thread can forward progress. This is transport
    // bookkeeping, not scan logic.
    if let Some(Response::StartScan(started)) = envelope.ok_value() {
        let scan_id = started.scan_id.clone();
        let handle = match state.session.lock() {
            Ok(mut session) => session.take_scan_handle(&scan_id),
            Err(_) => None,
        };
        if let Some(handle) = handle {
            if let Ok(mut scans) = state.scans.lock() {
                scans.insert(scan_id, handle);
            }
        }
    }
    envelope
}

#[tauri::command]
fn cancel_scan(state: State<'_, Arc<ShellState>>, scan_id: String) -> Envelope<Response> {
    forward(state, Command::CancelScan(CancelScanRequest { scan_id }))
}

#[tauri::command]
fn get_drive_summary(state: State<'_, Arc<ShellState>>, drive_id: String) -> Envelope<Response> {
    forward(
        state,
        Command::GetDriveSummary(GetDriveSummaryRequest { drive_id }),
    )
}

#[tauri::command]
fn get_categories(
    state: State<'_, Arc<ShellState>>,
    scan_id: String,
    parent_category: Option<String>,
) -> Envelope<Response> {
    forward(
        state,
        Command::GetCategories(GetCategoriesRequest {
            scan_id,
            parent_category,
        }),
    )
}

#[tauri::command]
fn get_opportunities(state: State<'_, Arc<ShellState>>, scan_id: String) -> Envelope<Response> {
    forward(
        state,
        Command::GetOpportunities(GetOpportunitiesRequest { scan_id }),
    )
}

#[tauri::command]
fn preview_plan(
    state: State<'_, Arc<ShellState>>,
    opportunity_ids: Vec<String>,
) -> Envelope<Response> {
    forward(
        state,
        Command::PreviewPlan(PreviewPlanRequest { opportunity_ids }),
    )
}

#[tauri::command]
fn confirm_plan(
    state: State<'_, Arc<ShellState>>,
    plan_id: String,
    acked_warnings: Vec<String>,
) -> Envelope<Response> {
    forward(
        state,
        Command::ConfirmPlan(ConfirmPlanRequest {
            plan_id,
            acked_warnings,
        }),
    )
}

#[tauri::command]
fn execute_plan(state: State<'_, Arc<ShellState>>, plan_id: String) -> Envelope<Response> {
    forward(state, Command::ExecutePlan(ExecutePlanRequest { plan_id }))
}

#[tauri::command]
fn get_history(state: State<'_, Arc<ShellState>>, drive_id: String) -> Envelope<Response> {
    forward(state, Command::GetHistory(GetHistoryRequest { drive_id }))
}

#[tauri::command]
fn list_drives(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::ListDrives(ListDrivesRequest {}))
}

// ---------------------------------------------------------------------------
// Aggregate view commands (`src/api.ts` shapes).
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_global_status(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::GetGlobalStatus(GetGlobalStatusRequest {}))
}

#[tauri::command]
fn get_home_summary(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::GetHomeSummary(GetHomeSummaryRequest {}))
}

#[tauri::command]
fn get_storage(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::GetStorage(GetStorageRequest {}))
}

#[tauri::command]
fn get_cleanup(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::GetCleanup(GetCleanupRequest {}))
}

#[tauri::command]
fn get_duplicate_report(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(
        state,
        Command::GetDuplicateReport(GetDuplicateReportRequest {}),
    )
}

#[tauri::command]
fn get_big_file_report(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::GetBigFileReport(GetBigFileReportRequest {}))
}

#[tauri::command]
fn get_startup_report(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::GetStartupReport(GetStartupReportRequest {}))
}

#[tauri::command]
fn get_dns_ranking(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::GetDnsRanking(GetDnsRankingRequest {}))
}

#[tauri::command]
fn get_block_list_plan(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(state, Command::GetBlockListPlan(GetBlockListPlanRequest {}))
}

#[tauri::command]
fn get_privacy_summary(state: State<'_, Arc<ShellState>>) -> Envelope<Response> {
    forward(
        state,
        Command::GetPrivacySummary(GetPrivacySummaryRequest {}),
    )
}

// ---------------------------------------------------------------------------
// Scan event bridge
// ---------------------------------------------------------------------------

/// The contract event name for throttled scan progress.
const EVENT_SCAN_PROGRESS: &str = "coresight.v1.scanProgress";
/// The contract event name for the terminal scan update.
const EVENT_SCAN_COMPLETE: &str = "coresight.v1.scanComplete";

/// Drain every live handle, apply its updates to the session, and forward the
/// typed payloads to the UI.
///
/// Runs on one dedicated thread for the life of the process. Nothing here
/// decides what a scan means; it only moves already-typed updates from
/// `coresight-ipc` to Tauri.
fn drain_scan_events(app: tauri::AppHandle, state: Arc<ShellState>) {
    loop {
        let mut updates: Vec<ScanProgressUpdate> = Vec::new();
        let mut finished: Vec<String> = Vec::new();

        match state.scans.lock() {
            Ok(mut scans) => {
                for (scan_id, handle) in scans.iter() {
                    loop {
                        match handle.progress.try_recv() {
                            Ok(update) => updates.push(update),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                finished.push(scan_id.clone());
                                break;
                            }
                        }
                    }
                }
                for scan_id in finished {
                    scans.remove(&scan_id);
                }
            }
            // A poisoned registry means the shell is already degraded; keep
            // the thread alive so a later successful lock can still drain.
            Err(_) => {}
        }

        for update in updates {
            let progress = update.progress_event();
            let complete = update.complete_event();
            match state.session.lock() {
                Ok(mut session) => {
                    let _ = session.apply_scan_update(update);
                }
                Err(_) => continue,
            }
            if let Some(payload) = progress {
                let _ = app.emit(EVENT_SCAN_PROGRESS, payload);
            }
            if let Some(payload) = complete {
                let _ = app.emit(EVENT_SCAN_COMPLETE, payload);
            }
        }

        std::thread::sleep(Duration::from_millis(40));
    }
}

fn main() {
    let mut session = Session::new();
    session.attach_scan_backend(Box::new(EngineScanBackend::new()));

    let state = Arc::new(ShellState {
        session: Mutex::new(session),
        scans: Mutex::new(BTreeMap::new()),
    });
    let drain_state = Arc::clone(&state);

    tauri::Builder::default()
        .manage(state)
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("coresight-scan-events".to_string())
                .spawn(move || drain_scan_events(handle, drain_state))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_scan,
            cancel_scan,
            get_drive_summary,
            get_categories,
            get_opportunities,
            preview_plan,
            confirm_plan,
            execute_plan,
            get_history,
            list_drives,
            get_global_status,
            get_home_summary,
            get_storage,
            get_cleanup,
            get_duplicate_report,
            get_big_file_report,
            get_startup_report,
            get_dns_ranking,
            get_block_list_plan,
            get_privacy_summary
        ])
        .run(tauri::generate_context!())
        .expect("CoreSight could not start: the Tauri runtime failed to initialise");
}

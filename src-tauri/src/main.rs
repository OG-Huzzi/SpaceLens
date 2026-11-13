//! CoreSight Tauri 2 shell — transport only.
//!
//! This crate owns exactly two things: the desktop window (declared in
//! `tauri.conf.json`) and the `coresight.v1.*` command table. Every command is a
//! thin adapter: build the typed `coresight_ipc::Command` from the invoke
//! arguments, hand it to `coresight_ipc::dispatch` under the single managed
//! `Session`, and return the resulting `Envelope<Response>` unchanged. There is
//! no filesystem access, no scanning, no plan logic and no fabricated data in
//! this file.
//!
//! # Result framing
//!
//! Every command returns the crate's `Envelope<Response>` verbatim, so the UI
//! sees one shape for all ten commands:
//!
//! ```json
//! { "ok":    { "kind": "coresight.v1.listDrives", "payload": [ /* Drive[] */ ] } }
//! { "error": { "code": "...", "message": "...", "detail": "..." } }
//! ```
//!
//! Nothing in the shell rewrites, unwraps or inspects a payload.
//!
//! # Honest unavailability
//!
//! The shell attaches no scan backend and no history store, because it has none
//! and must not invent one. `startScan`, `cancelScan` and `getHistory` therefore
//! answer with the IPC layer's typed `SCAN_UNAVAILABLE` / `HISTORY_UNAVAILABLE`
//! errors, and `confirmPlan`/`executePlan` refuse under the read-only execution
//! policy. That is the safety contract working as designed — the shell must not
//! paper over it with sample data.
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
//!
//! Arguments use the documented camelCase shape, e.g.
//! `invoke("start_scan", { driveId, fullRescan })`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use coresight_ipc::{
    ApiError, CancelScanRequest, Command, ConfirmPlanRequest, Envelope, ExecutePlanRequest,
    GetCategoriesRequest, GetDriveSummaryRequest, GetHistoryRequest, GetOpportunitiesRequest,
    ListDrivesRequest, PreviewPlanRequest, Response, Session, StartScanRequest,
};
use tauri::State;

/// Transport-level code used only when an earlier command panicked while
/// holding the session lock.
///
/// This is deliberately not one of the engine's `coresight.v1` codes: the UI's
/// documented behaviour for an unknown code is its generic safe fallback, which
/// is the honest presentation for a shell-level fault.
const SESSION_UNAVAILABLE: &str = "SESSION_UNAVAILABLE";

/// The single process-wide IPC session.
///
/// All commands are synchronous on purpose: the `MutexGuard` is taken, used and
/// dropped inside one call frame, so it can never be held across an `.await`
/// and the UI cannot deadlock itself by awaiting a command that waits for it.
struct ShellState {
    session: Mutex<Session>,
}

/// The whole translation layer: dispatch under the session lock and return the
/// envelope untouched. Every `#[tauri::command]` below is a one-line call to
/// this function.
fn forward(state: State<'_, ShellState>, command: Command) -> Envelope<Response> {
    match state.session.lock() {
        // Synchronous by design (see `ShellState`): no `.await` exists here, so
        // the guard is released before this function returns.
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
    state: State<'_, ShellState>,
    drive_id: String,
    full_rescan: Option<bool>,
) -> Envelope<Response> {
    forward(
        state,
        Command::StartScan(StartScanRequest {
            drive_id,
            full_rescan,
        }),
    )
}

#[tauri::command]
fn cancel_scan(state: State<'_, ShellState>, scan_id: String) -> Envelope<Response> {
    forward(state, Command::CancelScan(CancelScanRequest { scan_id }))
}

#[tauri::command]
fn get_drive_summary(state: State<'_, ShellState>, drive_id: String) -> Envelope<Response> {
    forward(
        state,
        Command::GetDriveSummary(GetDriveSummaryRequest { drive_id }),
    )
}

#[tauri::command]
fn get_categories(
    state: State<'_, ShellState>,
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
fn get_opportunities(state: State<'_, ShellState>, scan_id: String) -> Envelope<Response> {
    forward(
        state,
        Command::GetOpportunities(GetOpportunitiesRequest { scan_id }),
    )
}

#[tauri::command]
fn preview_plan(state: State<'_, ShellState>, opportunity_ids: Vec<String>) -> Envelope<Response> {
    forward(
        state,
        Command::PreviewPlan(PreviewPlanRequest { opportunity_ids }),
    )
}

#[tauri::command]
fn confirm_plan(
    state: State<'_, ShellState>,
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
fn execute_plan(state: State<'_, ShellState>, plan_id: String) -> Envelope<Response> {
    forward(state, Command::ExecutePlan(ExecutePlanRequest { plan_id }))
}

#[tauri::command]
fn get_history(state: State<'_, ShellState>, drive_id: String) -> Envelope<Response> {
    forward(state, Command::GetHistory(GetHistoryRequest { drive_id }))
}

#[tauri::command]
fn list_drives(state: State<'_, ShellState>) -> Envelope<Response> {
    forward(state, Command::ListDrives(ListDrivesRequest {}))
}

fn main() {
    let state = ShellState {
        session: Mutex::new(Session::new()),
    };

    tauri::Builder::default()
        .manage(state)
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
            list_drives
        ])
        .run(tauri::generate_context!())
        .expect("CoreSight could not start: the Tauri runtime failed to initialise");
}

//! SpaceLens Tauri shell — Phase 0 placeholder.
//!
//! NOT compiled in Phase 0: linking Tauri on Windows requires the MSVC
//! toolchain, which this machine cannot install (C: has ~258 MB free).
//! First compile happens in MSVC CI in Phase 1. The shell will own window
//! setup and typed command registration only — all filesystem truth stays in
//! `spacelens-core` behind the `spacelens.v1.*` contract.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Phase 1: register `spacelens.v1.*` commands from docs/API_CONTRACTS.md.
    // tauri::Builder::default().invoke_handler(…).run(…).expect("…");
    println!("SpaceLens shell placeholder — see docs/ARCHITECTURE.md");
}

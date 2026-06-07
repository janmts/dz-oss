use std::sync::Arc;
use tauri::State;

use crate::{api, db, parser, settings, AppState};

#[tauri::command]
pub fn get_sessions(state: State<'_, Arc<AppState>>) -> Result<Vec<db::SessionRow>, String> {
    api::list_sessions(&state)
}

#[tauri::command]
pub fn get_session_packets(
    state: State<'_, Arc<AppState>>,
    session_id: i64,
) -> Result<Vec<parser::TelemetryPacket>, String> {
    api::session_packets(&state, session_id)
}

#[tauri::command]
pub fn get_session_laps(
    state: State<'_, Arc<AppState>>,
    session_id: i64,
) -> Result<Vec<db::LapRow>, String> {
    api::session_laps(&state, session_id)
}

#[tauri::command]
pub fn delete_session(state: State<'_, Arc<AppState>>, session_id: i64) -> Result<(), String> {
    api::delete_session(&state, session_id)
}

#[tauri::command]
pub fn clear_all_sessions(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    api::clear_all_sessions(&state)
}

#[tauri::command]
pub fn rename_session(
    state: State<'_, Arc<AppState>>,
    session_id: i64,
    name: Option<String>,
) -> Result<(), String> {
    api::rename_session(&state, session_id, name)
}

#[tauri::command]
pub fn set_session_bookmark(
    state: State<'_, Arc<AppState>>,
    session_id: i64,
    bookmarked: bool,
) -> Result<(), String> {
    api::set_session_bookmark(&state, session_id, bookmarked)
}

#[tauri::command]
pub fn get_drift_runs(state: State<'_, Arc<AppState>>) -> Result<Vec<db::DriftRunRow>, String> {
    api::list_drift_runs(&state)
}

#[tauri::command]
pub fn set_drift_run_manual_score(
    state: State<'_, Arc<AppState>>,
    run_id: i64,
    manual_score: i64,
    notes: Option<String>,
) -> Result<(), String> {
    api::set_drift_run_manual_score(&state, run_id, manual_score, notes)
}

#[tauri::command]
pub fn get_drift_zones(state: State<'_, Arc<AppState>>) -> Result<Vec<db::DriftZoneRow>, String> {
    api::list_drift_zones(&state)
}

#[tauri::command]
pub fn save_drift_zone(
    state: State<'_, Arc<AppState>>,
    zone: db::DriftZoneInput,
) -> Result<db::DriftZoneRow, String> {
    api::save_drift_zone(&state, zone)
}

#[tauri::command]
pub fn delete_drift_zone(state: State<'_, Arc<AppState>>, zone_id: i64) -> Result<(), String> {
    api::delete_drift_zone(&state, zone_id)
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> settings::Settings {
    api::get_settings(&state)
}

#[tauri::command]
pub fn save_settings(
    state: State<'_, Arc<AppState>>,
    new_settings: settings::Settings,
) -> Result<(), String> {
    api::save_settings(&state, new_settings)
}

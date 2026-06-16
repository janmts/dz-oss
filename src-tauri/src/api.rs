use crate::{db, settings, AppState};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn list_sessions(state: &AppState) -> Result<Vec<db::SessionRow>, String> {
    let conn = state.db.lock().unwrap();
    db::list_sessions(&conn).map_err(|e| e.to_string())
}

pub fn session_packets(
    state: &AppState,
    session_id: i64,
) -> Result<Vec<crate::parser::TelemetryPacket>, String> {
    let conn = state.db.lock().unwrap();
    let blobs = db::get_session_packets(&conn, session_id).map_err(|e| e.to_string())?;
    Ok(blobs
        .iter()
        .filter_map(|b| crate::parser::parse(b).ok())
        .collect())
}

pub fn session_laps(state: &AppState, session_id: i64) -> Result<Vec<db::LapRow>, String> {
    let conn = state.db.lock().unwrap();
    db::get_session_laps(&conn, session_id).map_err(|e| e.to_string())
}

pub fn delete_session(state: &AppState, session_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::delete_session(&conn, session_id).map_err(|e| e.to_string())
}

pub fn clear_all_sessions(state: &AppState) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::clear_all_sessions(&conn).map_err(|e| e.to_string())
}

pub fn rename_session(
    state: &AppState,
    session_id: i64,
    name: Option<String>,
) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::rename_session(&conn, session_id, name.as_deref()).map_err(|e| e.to_string())
}

pub fn set_session_bookmark(
    state: &AppState,
    session_id: i64,
    bookmarked: bool,
) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::set_session_bookmark(&conn, session_id, bookmarked).map_err(|e| e.to_string())
}

pub fn list_drift_runs(state: &AppState) -> Result<Vec<db::DriftRunRow>, String> {
    let conn = state.db.lock().unwrap();
    db::list_drift_runs(&conn).map_err(|e| e.to_string())
}

/// Re-score every stored run from its saved packets using the current scoring
/// params (per-zone config over defaults), season-adjusted per run: a run is
/// bound to the in-game season its `started_at` falls in, and outside winter
/// the tarmac gate is dropped (off-tarmac surfaces pay). Returns the number of
/// runs updated. Used to refresh computed scores after the formula is tuned.
pub fn recompute_drift_scores(state: &AppState) -> Result<usize, String> {
    let conn = state.db.lock().unwrap();
    let refs = db::list_drift_run_refs(&conn).map_err(|e| e.to_string())?;
    let mut count = 0;
    for (run_id, zone_id, started_at) in refs {
        let season = crate::season::season_at_utc_ms(started_at);
        let params = match zone_id {
            Some(z) => db::get_drift_zone(&conn, z)
                .map(|row| crate::scoring::ScoringParams::from_config(&row.scoring_config))
                .unwrap_or_default(),
            None => crate::scoring::ScoringParams::default(),
        }
        .for_season(season);
        let blobs = db::get_drift_run_packets(&conn, run_id).map_err(|e| e.to_string())?;
        let pkts: Vec<_> = blobs
            .iter()
            .filter_map(|b| crate::parser::parse(b).ok())
            .collect();
        let result = crate::scoring::score_run(&pkts, &params);
        let breakdown = serde_json::to_string(&result).ok();
        db::update_drift_run_score(&conn, run_id, Some(result.score as f32), breakdown.as_deref())
            .map_err(|e| e.to_string())?;
        count += 1;
    }
    Ok(count)
}

/// One drift run's full per-packet telemetry plus authoritative per-tick scoring
/// diagnostics, for the run viewer. `packets` and `ticks` are index-aligned.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftRunPackets {
    pub run_id: i64,
    pub zone_id: Option<i64>,
    /// Season the run is scored under (drives the off-tarmac gate), derived from
    /// its `started_at` — the same binding [`recompute_drift_scores`] uses.
    pub season: &'static str,
    pub packets: Vec<crate::parser::TelemetryPacket>,
    pub ticks: Vec<crate::scoring::TickScore>,
    pub score: crate::scoring::RunScore,
}

/// Load one run's stored packets and score them with per-tick emission, so the
/// run viewer can plot the trace + timeline and colour each tick by what the
/// game actually banked. Resolves the run's season-adjusted per-zone params the
/// same way [`recompute_drift_scores`] does, so the diagnostics match the stored
/// `computed_score`.
pub fn drift_run_packets(state: &AppState, run_id: i64) -> Result<DriftRunPackets, String> {
    let conn = state.db.lock().unwrap();
    let (zone_id, started_at) = db::get_drift_run_ref(&conn, run_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("drift run {run_id} not found"))?;
    let season = crate::season::season_at_utc_ms(started_at);
    // Load the zone once: it supplies both the season-adjusted scoring params and
    // the split geometry the per-sector rollup buckets against.
    let zone_row = zone_id.and_then(|z| db::get_drift_zone(&conn, z).ok());
    let params = zone_row
        .as_ref()
        .map(|row| crate::scoring::ScoringParams::from_config(&row.scoring_config))
        .unwrap_or_default()
        .for_season(season);
    let packets: Vec<crate::parser::TelemetryPacket> = db::get_drift_run_packets(&conn, run_id)
        .map_err(|e| e.to_string())?
        .iter()
        .filter_map(|b| crate::parser::parse(b).ok())
        .collect();
    let (mut score, mut ticks) = crate::scoring::score_run_with_ticks(&packets, &params);
    // Bucket the per-tick points into sectors (split-gate crossings from entry) so
    // the run viewer can show "where did I score". No-op for zones with no splits.
    if let Some(row) = zone_row.as_ref() {
        crate::drift::rescore_sectors(row, &packets, &mut ticks, &mut score);
    }
    Ok(DriftRunPackets {
        run_id,
        zone_id,
        season: season.as_str(),
        packets,
        ticks,
        score,
    })
}

/// Delete one run and its packets/splits/score. Returns Ok even if the id was
/// already gone (idempotent from the caller's perspective).
pub fn delete_drift_run(state: &AppState, run_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::delete_drift_run(&conn, run_id)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Purge all invalid runs (out-of-bounds, never finished). Returns how many
/// were removed.
pub fn delete_invalid_drift_runs(state: &AppState) -> Result<usize, String> {
    let conn = state.db.lock().unwrap();
    db::delete_invalid_drift_runs(&conn).map_err(|e| e.to_string())
}

pub fn set_drift_run_manual_score(
    state: &AppState,
    run_id: i64,
    manual_score: i64,
    notes: Option<String>,
) -> Result<(), String> {
    let entered_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis() as i64;
    let conn = state.db.lock().unwrap();
    db::set_drift_run_manual_score(&conn, run_id, manual_score, notes.as_deref(), entered_at)
        .map_err(|e| e.to_string())
}

pub fn drift_run_status(state: &AppState) -> crate::drift::DriftRunStatus {
    state.drift_manager.lock().unwrap().status()
}

pub fn list_drift_zones(state: &AppState) -> Result<Vec<db::DriftZoneRow>, String> {
    let conn = state.db.lock().unwrap();
    db::list_drift_zones(&conn).map_err(|e| e.to_string())
}

pub fn save_drift_zone(
    state: &AppState,
    zone: db::DriftZoneInput,
) -> Result<db::DriftZoneRow, String> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis() as i64;
    let conn = state.db.lock().unwrap();
    let id = db::save_drift_zone(&conn, &zone, now_ms).map_err(|e| e.to_string())?;
    db::get_drift_zone(&conn, id).map_err(|e| e.to_string())
}

pub fn delete_drift_zone(state: &AppState, zone_id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    db::delete_drift_zone(&conn, zone_id).map_err(|e| e.to_string())
}

pub fn get_settings(state: &AppState) -> settings::Settings {
    state.settings.lock().unwrap().clone()
}

pub fn save_settings(state: &AppState, new_settings: settings::Settings) -> Result<(), String> {
    settings::save(&new_settings).map_err(|e| e.to_string())?;
    let auto_record = new_settings.auto_record;
    *state.settings.lock().unwrap() = new_settings;
    state
        .session_manager
        .lock()
        .unwrap()
        .set_auto_record(auto_record);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionManager;
    use std::sync::Mutex;

    // save_settings is covered by the server integration test (Task 7) — it writes to disk.

    fn test_state() -> AppState {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        AppState {
            db: Mutex::new(conn),
            session_manager: Mutex::new(SessionManager::new(true)),
            drift_manager: Mutex::new(crate::drift::DriftRunManager::new()),
            settings: Mutex::new(settings::Settings::default()),
        }
    }

    #[test]
    fn list_is_empty_then_reflects_inserts() {
        let state = test_state();
        assert_eq!(list_sessions(&state).unwrap().len(), 0);
        {
            let conn = state.db.lock().unwrap();
            db::open_session(&conn, 1000, 42, 5, 800).unwrap();
        }
        assert_eq!(list_sessions(&state).unwrap().len(), 1);
    }

    #[test]
    fn rename_and_bookmark_and_delete_roundtrip() {
        let state = test_state();
        let id = {
            let conn = state.db.lock().unwrap();
            db::open_session(&conn, 1000, 42, 5, 800).unwrap()
        };
        rename_session(&state, id, Some("Practice".into())).unwrap();
        set_session_bookmark(&state, id, true).unwrap();
        let rows = list_sessions(&state).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name.as_deref(), Some("Practice"));
        assert!(rows[0].bookmarked);
        delete_session(&state, id).unwrap();
        assert_eq!(list_sessions(&state).unwrap().len(), 0);
    }

    #[test]
    fn clear_all_removes_everything() {
        let state = test_state();
        {
            let conn = state.db.lock().unwrap();
            db::open_session(&conn, 1, 1, 1, 1).unwrap();
            db::open_session(&conn, 2, 2, 2, 2).unwrap();
        }
        clear_all_sessions(&state).unwrap();
        assert_eq!(list_sessions(&state).unwrap().len(), 0);
    }

    #[test]
    fn drift_runs_and_manual_score_roundtrip() {
        let state = test_state();
        let id = {
            let conn = state.db.lock().unwrap();
            db::open_drift_run(&conn, None, 1000, 3249, 5, 900, 1, 77).unwrap()
        };
        set_drift_run_manual_score(&state, id, 99_999, Some("entered after run".into())).unwrap();
        let rows = list_drift_runs(&state).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].car_ordinal, 3249);
        assert_eq!(rows[0].manual_score, Some(99_999));
    }

    #[test]
    fn drift_zones_roundtrip() {
        let state = test_state();
        let zone = db::DriftZoneInput {
            id: None,
            name: "Mountain Pass".into(),
            description: None,
            active: true,
            left_boundary: vec![db::ZonePoint { x: 1.0, z: 2.0 }],
            right_boundary: vec![db::ZonePoint { x: 3.0, z: 4.0 }],
            start_gate: Vec::new(),
            finish_gate: Vec::new(),
            split_gates: Vec::new(),
            scoring_config: serde_json::json!({}),
        };
        let saved = save_drift_zone(&state, zone).unwrap();
        assert_eq!(saved.name, "Mountain Pass");
        assert_eq!(list_drift_zones(&state).unwrap().len(), 1);
        delete_drift_zone(&state, saved.id).unwrap();
        assert_eq!(list_drift_zones(&state).unwrap().len(), 0);
    }

    #[test]
    fn get_settings_returns_defaults() {
        let state = test_state();
        assert_eq!(get_settings(&state).port, 20440);
    }
}

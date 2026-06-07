use rusqlite::{Connection, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn db_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fh6-tel")
        .join("sessions.db")
}

pub fn open() -> Result<Connection> {
    let path = db_path();
    std::fs::create_dir_all(path.parent().unwrap()).ok();
    let conn = Connection::open(&path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    init(&conn)?;
    Ok(conn)
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            id INTEGER PRIMARY KEY,
            started_at INTEGER NOT NULL,
            ended_at INTEGER,
            car_ordinal INTEGER NOT NULL DEFAULT 0,
            car_class INTEGER NOT NULL DEFAULT 0,
            car_pi INTEGER NOT NULL DEFAULT 0,
            best_lap REAL,
            packet_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS session_packets (
            id INTEGER PRIMARY KEY,
            session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            timestamp_ms INTEGER NOT NULL,
            data BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_packets_session ON session_packets(session_id);",
    )?;
    migrate(conn);
    Ok(())
}

/// Additive, backwards-compatible migrations. Each ALTER is idempotent: on an
/// already-migrated DB SQLite returns a "duplicate column" error which we
/// intentionally ignore. Never drops or rewrites existing data.
fn migrate(conn: &Connection) {
    let _ = conn.execute("ALTER TABLE sessions ADD COLUMN name TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN bookmarked INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // Per-lap times. UNIQUE(session_id, lap_number) so a re-driven lap (rewind)
    // overwrites rather than duplicating.
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS session_laps (
            id INTEGER PRIMARY KEY,
            session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            lap_number INTEGER NOT NULL,
            lap_time REAL NOT NULL,
            UNIQUE(session_id, lap_number)
        );
        CREATE INDEX IF NOT EXISTS idx_laps_session ON session_laps(session_id);",
    );
    let _ = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS drift_zones (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            active INTEGER NOT NULL DEFAULT 1,
            left_boundary_json TEXT NOT NULL,
            right_boundary_json TEXT NOT NULL,
            start_gate_json TEXT NOT NULL DEFAULT '[]',
            finish_gate_json TEXT NOT NULL DEFAULT '[]',
            split_gates_json TEXT NOT NULL DEFAULT '[]',
            scoring_config_json TEXT NOT NULL DEFAULT '{}'
        );
        CREATE TABLE IF NOT EXISTS drift_runs (
            id INTEGER PRIMARY KEY,
            zone_id INTEGER REFERENCES drift_zones(id) ON DELETE SET NULL,
            started_at INTEGER NOT NULL,
            ended_at INTEGER,
            car_ordinal INTEGER NOT NULL DEFAULT 0,
            car_class INTEGER NOT NULL DEFAULT 0,
            car_pi INTEGER NOT NULL DEFAULT 0,
            drivetrain_type INTEGER NOT NULL DEFAULT -1,
            car_group INTEGER NOT NULL DEFAULT 0,
            valid INTEGER NOT NULL DEFAULT 1,
            invalid_reason TEXT,
            computed_score REAL,
            packet_count INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS drift_run_packets (
            id INTEGER PRIMARY KEY,
            run_id INTEGER NOT NULL REFERENCES drift_runs(id) ON DELETE CASCADE,
            timestamp_ms INTEGER NOT NULL,
            data BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS drift_run_splits (
            id INTEGER PRIMARY KEY,
            run_id INTEGER NOT NULL REFERENCES drift_runs(id) ON DELETE CASCADE,
            split_index INTEGER NOT NULL,
            start_timestamp_ms INTEGER,
            end_timestamp_ms INTEGER,
            duration_ms INTEGER,
            computed_score REAL,
            telemetry_summary_json TEXT NOT NULL DEFAULT '{}',
            UNIQUE(run_id, split_index)
        );
        CREATE TABLE IF NOT EXISTS drift_run_scores (
            id INTEGER PRIMARY KEY,
            run_id INTEGER NOT NULL REFERENCES drift_runs(id) ON DELETE CASCADE,
            manual_score INTEGER NOT NULL,
            notes TEXT,
            entered_at INTEGER NOT NULL,
            UNIQUE(run_id)
        );
        CREATE INDEX IF NOT EXISTS idx_drift_runs_zone_started ON drift_runs(zone_id, started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_drift_runs_car_started ON drift_runs(car_ordinal, started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_drift_packets_run ON drift_run_packets(run_id);
        CREATE INDEX IF NOT EXISTS idx_drift_splits_run ON drift_run_splits(run_id);",
    );
    // Per-run scoring breakdown (avg/max angle, drift time, multiplier, …) as
    // JSON — drives the dashboard readout and the finetuning workflow.
    let _ = conn.execute("ALTER TABLE drift_runs ADD COLUMN score_breakdown_json TEXT", []);
}

pub fn insert_lap(
    conn: &Connection,
    session_id: i64,
    lap_number: i64,
    lap_time: f32,
) -> Result<()> {
    conn.execute(
        "INSERT INTO session_laps (session_id, lap_number, lap_time) VALUES (?1, ?2, ?3)
         ON CONFLICT(session_id, lap_number) DO UPDATE SET lap_time=excluded.lap_time",
        rusqlite::params![session_id, lap_number, lap_time],
    )?;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LapRow {
    pub lap_number: i64,
    pub lap_time: f32,
}

/// Fastest recorded lap for a session, or None if it has no laps. Derived from
/// the lap table so it reflects rewind upserts/overwrites correctly.
pub fn min_lap_time(conn: &Connection, session_id: i64) -> Result<Option<f32>> {
    conn.query_row(
        "SELECT MIN(lap_time) FROM session_laps WHERE session_id=?1",
        [session_id],
        |r| r.get::<_, Option<f32>>(0),
    )
}

pub fn get_session_laps(conn: &Connection, session_id: i64) -> Result<Vec<LapRow>> {
    let mut stmt = conn.prepare(
        "SELECT lap_number, lap_time FROM session_laps
         WHERE session_id=?1 ORDER BY lap_number ASC",
    )?;
    let rows = stmt.query_map([session_id], |r| {
        Ok(LapRow {
            lap_number: r.get(0)?,
            lap_time: r.get(1)?,
        })
    })?;
    rows.collect()
}

pub fn open_session(
    conn: &Connection,
    started_at: i64,
    car_ordinal: i32,
    car_class: i32,
    car_pi: i32,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO sessions (started_at, car_ordinal, car_class, car_pi) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![started_at, car_ordinal, car_class, car_pi],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Sets car metadata on a session that still has car_ordinal=0.
/// Called every packet so the first non-zero ordinal wins, handling the case
/// where the opening packet arrives before the game has populated car data.
pub fn update_session_car_if_unknown(
    conn: &Connection,
    id: i64,
    car_ordinal: i32,
    car_class: i32,
    car_pi: i32,
) -> Result<()> {
    conn.execute(
        "UPDATE sessions SET car_ordinal=?1, car_class=?2, car_pi=?3 WHERE id=?4 AND car_ordinal=0",
        rusqlite::params![car_ordinal, car_class, car_pi, id],
    )?;
    Ok(())
}

pub fn reopen_session(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE sessions SET ended_at = NULL WHERE id=?1", [id])?;
    Ok(())
}

pub fn close_session(conn: &Connection, id: i64, ended_at: i64, best_lap: f32) -> Result<()> {
    // best_lap is the authoritative MIN of the session's lap table (computed by
    // the caller), so the latest close is correct — including after a rewind
    // overwrote a provisional partial with the true lap. <=0 means "no laps":
    // keep whatever was there.
    conn.execute(
        "UPDATE sessions SET ended_at=?1,
         best_lap = CASE WHEN ?2 > 0.0 THEN ?2 ELSE best_lap END
         WHERE id=?3",
        rusqlite::params![ended_at, best_lap, id],
    )?;
    Ok(())
}

pub fn insert_packet(
    conn: &Connection,
    session_id: i64,
    timestamp_ms: u32,
    data: &[u8],
) -> Result<()> {
    conn.execute(
        "INSERT INTO session_packets (session_id, timestamp_ms, data) VALUES (?1, ?2, ?3)",
        rusqlite::params![session_id, timestamp_ms, data],
    )?;
    conn.execute(
        "UPDATE sessions SET packet_count = packet_count + 1 WHERE id=?1",
        [session_id],
    )?;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub car_ordinal: i32,
    pub car_class: i32,
    pub car_pi: i32,
    pub best_lap: Option<f32>,
    pub packet_count: i64,
    pub name: Option<String>,
    pub bookmarked: bool,
}

pub fn list_sessions(conn: &Connection) -> Result<Vec<SessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, started_at, ended_at, car_ordinal, car_class, car_pi, best_lap, packet_count,
                name, bookmarked
         FROM sessions ORDER BY bookmarked DESC, started_at DESC LIMIT 100",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(SessionRow {
            id: r.get(0)?,
            started_at: r.get(1)?,
            ended_at: r.get(2)?,
            car_ordinal: r.get(3)?,
            car_class: r.get(4)?,
            car_pi: r.get(5)?,
            best_lap: r.get(6)?,
            packet_count: r.get(7)?,
            name: r.get(8)?,
            bookmarked: r.get::<_, i64>(9)? != 0,
        })
    })?;
    rows.collect()
}

pub fn rename_session(conn: &Connection, id: i64, name: Option<&str>) -> Result<()> {
    // Empty/whitespace name clears back to the default label.
    let trimmed = name.map(str::trim).filter(|s| !s.is_empty());
    conn.execute(
        "UPDATE sessions SET name=?1 WHERE id=?2",
        rusqlite::params![trimmed, id],
    )?;
    Ok(())
}

pub fn set_session_bookmark(conn: &Connection, id: i64, bookmarked: bool) -> Result<()> {
    conn.execute(
        "UPDATE sessions SET bookmarked=?1 WHERE id=?2",
        rusqlite::params![bookmarked as i64, id],
    )?;
    Ok(())
}

pub fn get_session_packets(conn: &Connection, session_id: i64) -> Result<Vec<Vec<u8>>> {
    let mut stmt = conn.prepare(
        "SELECT data FROM session_packets WHERE session_id=?1 ORDER BY timestamp_ms ASC",
    )?;
    let rows = stmt.query_map([session_id], |r| r.get::<_, Vec<u8>>(0))?;
    rows.collect()
}

pub fn delete_session(conn: &Connection, id: i64) -> Result<()> {
    // FK cascade isn't enabled on this connection, so clean children explicitly.
    conn.execute("DELETE FROM session_laps WHERE session_id=?1", [id])?;
    conn.execute("DELETE FROM session_packets WHERE session_id=?1", [id])?;
    conn.execute("DELETE FROM sessions WHERE id=?1", [id])?;
    Ok(())
}

pub fn clear_all_sessions(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DELETE FROM session_laps; DELETE FROM session_packets; DELETE FROM sessions;",
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZonePoint {
    pub x: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftZoneInput {
    pub id: Option<i64>,
    pub name: String,
    pub description: Option<String>,
    pub active: bool,
    pub left_boundary: Vec<ZonePoint>,
    pub right_boundary: Vec<ZonePoint>,
    #[serde(default)]
    pub start_gate: Vec<ZonePoint>,
    #[serde(default)]
    pub finish_gate: Vec<ZonePoint>,
    #[serde(default)]
    pub split_gates: Vec<Vec<ZonePoint>>,
    #[serde(default)]
    pub scoring_config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftZoneRow {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub active: bool,
    pub left_boundary: Vec<ZonePoint>,
    pub right_boundary: Vec<ZonePoint>,
    pub start_gate: Vec<ZonePoint>,
    pub finish_gate: Vec<ZonePoint>,
    pub split_gates: Vec<Vec<ZonePoint>>,
    pub scoring_config: serde_json::Value,
}

fn zone_points_json(points: &[ZonePoint]) -> Result<String> {
    serde_json::to_string(points).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn split_gates_json(gates: &[Vec<ZonePoint>]) -> Result<String> {
    serde_json::to_string(gates).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn value_json(value: &serde_json::Value) -> Result<String> {
    serde_json::to_string(value).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn parse_points_json(raw: String) -> Vec<ZonePoint> {
    serde_json::from_str(&raw).unwrap_or_default()
}

fn parse_split_gates_json(raw: String) -> Vec<Vec<ZonePoint>> {
    serde_json::from_str(&raw).unwrap_or_default()
}

fn parse_value_json(raw: String) -> serde_json::Value {
    serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
}

fn normalize_zone_input(input: &DriftZoneInput) -> DriftZoneInput {
    let start_gate = if input.start_gate.len() == 2 {
        input.start_gate.clone()
    } else if let (Some(left), Some(right)) =
        (input.left_boundary.first(), input.right_boundary.first())
    {
        vec![left.clone(), right.clone()]
    } else {
        Vec::new()
    };
    let finish_gate = if input.finish_gate.len() == 2 {
        input.finish_gate.clone()
    } else if let (Some(left), Some(right)) =
        (input.left_boundary.last(), input.right_boundary.last())
    {
        vec![left.clone(), right.clone()]
    } else {
        Vec::new()
    };
    DriftZoneInput {
        id: input.id,
        name: input.name.trim().to_string(),
        description: input
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        active: input.active,
        left_boundary: input.left_boundary.clone(),
        right_boundary: input.right_boundary.clone(),
        start_gate,
        finish_gate,
        split_gates: input.split_gates.clone(),
        scoring_config: input.scoring_config.clone(),
    }
}

pub fn save_drift_zone(conn: &Connection, input: &DriftZoneInput, now_ms: i64) -> Result<i64> {
    let zone = normalize_zone_input(input);
    let name = if zone.name.is_empty() {
        "Untitled drift zone".to_string()
    } else {
        zone.name
    };
    let left = zone_points_json(&zone.left_boundary)?;
    let right = zone_points_json(&zone.right_boundary)?;
    let start = zone_points_json(&zone.start_gate)?;
    let finish = zone_points_json(&zone.finish_gate)?;
    let splits = split_gates_json(&zone.split_gates)?;
    let scoring = value_json(&zone.scoring_config)?;

    if let Some(id) = zone.id {
        conn.execute(
            "UPDATE drift_zones
             SET name=?1, description=?2, updated_at=?3, active=?4,
                 left_boundary_json=?5, right_boundary_json=?6,
                 start_gate_json=?7, finish_gate_json=?8,
                 split_gates_json=?9, scoring_config_json=?10
             WHERE id=?11",
            rusqlite::params![
                name,
                zone.description,
                now_ms,
                zone.active as i64,
                left,
                right,
                start,
                finish,
                splits,
                scoring,
                id,
            ],
        )?;
        Ok(id)
    } else {
        conn.execute(
            "INSERT INTO drift_zones (
                name, description, created_at, updated_at, active,
                left_boundary_json, right_boundary_json, start_gate_json,
                finish_gate_json, split_gates_json, scoring_config_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                name,
                zone.description,
                now_ms,
                now_ms,
                zone.active as i64,
                left,
                right,
                start,
                finish,
                splits,
                scoring,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }
}

pub fn get_drift_zone(conn: &Connection, id: i64) -> Result<DriftZoneRow> {
    conn.query_row(
        "SELECT id, name, description, created_at, updated_at, active,
                left_boundary_json, right_boundary_json, start_gate_json,
                finish_gate_json, split_gates_json, scoring_config_json
         FROM drift_zones WHERE id=?1",
        [id],
        |r| {
            Ok(DriftZoneRow {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
                active: r.get::<_, i64>(5)? != 0,
                left_boundary: parse_points_json(r.get(6)?),
                right_boundary: parse_points_json(r.get(7)?),
                start_gate: parse_points_json(r.get(8)?),
                finish_gate: parse_points_json(r.get(9)?),
                split_gates: parse_split_gates_json(r.get(10)?),
                scoring_config: parse_value_json(r.get(11)?),
            })
        },
    )
}

pub fn list_drift_zones(conn: &Connection) -> Result<Vec<DriftZoneRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, created_at, updated_at, active,
                left_boundary_json, right_boundary_json, start_gate_json,
                finish_gate_json, split_gates_json, scoring_config_json
         FROM drift_zones ORDER BY active DESC, updated_at DESC, name ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(DriftZoneRow {
            id: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            created_at: r.get(3)?,
            updated_at: r.get(4)?,
            active: r.get::<_, i64>(5)? != 0,
            left_boundary: parse_points_json(r.get(6)?),
            right_boundary: parse_points_json(r.get(7)?),
            start_gate: parse_points_json(r.get(8)?),
            finish_gate: parse_points_json(r.get(9)?),
            split_gates: parse_split_gates_json(r.get(10)?),
            scoring_config: parse_value_json(r.get(11)?),
        })
    })?;
    rows.collect()
}

pub fn delete_drift_zone(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE drift_runs SET zone_id=NULL WHERE zone_id=?1", [id])?;
    conn.execute("DELETE FROM drift_zones WHERE id=?1", [id])?;
    Ok(())
}

pub fn open_drift_run(
    conn: &Connection,
    zone_id: Option<i64>,
    started_at: i64,
    car_ordinal: i32,
    car_class: i32,
    car_pi: i32,
    drivetrain_type: i32,
    car_group: u32,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO drift_runs (
            zone_id, started_at, car_ordinal, car_class, car_pi, drivetrain_type, car_group
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            zone_id,
            started_at,
            car_ordinal,
            car_class,
            car_pi,
            drivetrain_type,
            car_group as i64,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn close_drift_run(
    conn: &Connection,
    run_id: i64,
    ended_at: i64,
    valid: bool,
    invalid_reason: Option<&str>,
    computed_score: Option<f32>,
) -> Result<()> {
    conn.execute(
        "UPDATE drift_runs
         SET ended_at=?1, valid=?2, invalid_reason=?3, computed_score=?4
         WHERE id=?5",
        rusqlite::params![
            ended_at,
            valid as i64,
            invalid_reason,
            computed_score,
            run_id,
        ],
    )?;
    Ok(())
}

pub fn insert_drift_run_packet(
    conn: &Connection,
    run_id: i64,
    timestamp_ms: u32,
    data: &[u8],
) -> Result<()> {
    conn.execute(
        "INSERT INTO drift_run_packets (run_id, timestamp_ms, data) VALUES (?1, ?2, ?3)",
        rusqlite::params![run_id, timestamp_ms, data],
    )?;
    conn.execute(
        "UPDATE drift_runs SET packet_count = packet_count + 1 WHERE id=?1",
        [run_id],
    )?;
    Ok(())
}

/// Raw packet blobs for a run, in time order — used to (re)score from history.
pub fn get_drift_run_packets(conn: &Connection, run_id: i64) -> Result<Vec<Vec<u8>>> {
    let mut stmt = conn.prepare(
        "SELECT data FROM drift_run_packets WHERE run_id=?1 ORDER BY timestamp_ms ASC",
    )?;
    let rows = stmt.query_map([run_id], |r| r.get::<_, Vec<u8>>(0))?;
    rows.collect()
}

/// (run_id, zone_id) for every run — lets the recompute pass pick the right
/// per-zone scoring config without loading full rows.
pub fn list_drift_run_refs(conn: &Connection) -> Result<Vec<(i64, Option<i64>)>> {
    let mut stmt = conn.prepare("SELECT id, zone_id FROM drift_runs ORDER BY id ASC")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// Store a recomputed score + breakdown on an already-closed run. Leaves
/// ended_at / valid / invalid_reason untouched.
pub fn update_drift_run_score(
    conn: &Connection,
    run_id: i64,
    computed_score: Option<f32>,
    breakdown_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE drift_runs SET computed_score=?1, score_breakdown_json=?2 WHERE id=?3",
        rusqlite::params![computed_score, breakdown_json, run_id],
    )?;
    Ok(())
}

pub fn set_drift_run_manual_score(
    conn: &Connection,
    run_id: i64,
    manual_score: i64,
    notes: Option<&str>,
    entered_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO drift_run_scores (run_id, manual_score, notes, entered_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(run_id) DO UPDATE SET
            manual_score=excluded.manual_score,
            notes=excluded.notes,
            entered_at=excluded.entered_at",
        rusqlite::params![run_id, manual_score, notes, entered_at],
    )?;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftRunRow {
    pub id: i64,
    pub zone_id: Option<i64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub car_ordinal: i32,
    pub car_class: i32,
    pub car_pi: i32,
    pub drivetrain_type: i32,
    pub car_group: i64,
    pub valid: bool,
    pub invalid_reason: Option<String>,
    pub computed_score: Option<f32>,
    pub manual_score: Option<i64>,
    pub packet_count: i64,
    pub score_breakdown: Option<serde_json::Value>,
}

pub fn list_drift_runs(conn: &Connection) -> Result<Vec<DriftRunRow>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.zone_id, r.started_at, r.ended_at, r.car_ordinal,
                r.car_class, r.car_pi, r.drivetrain_type, r.car_group,
                r.valid, r.invalid_reason, r.computed_score, s.manual_score,
                r.packet_count, r.score_breakdown_json
         FROM drift_runs r
         LEFT JOIN drift_run_scores s ON s.run_id = r.id
         ORDER BY r.started_at DESC
         LIMIT 200",
    )?;
    let rows = stmt.query_map([], |r| {
        let breakdown: Option<String> = r.get(14)?;
        Ok(DriftRunRow {
            id: r.get(0)?,
            zone_id: r.get(1)?,
            started_at: r.get(2)?,
            ended_at: r.get(3)?,
            car_ordinal: r.get(4)?,
            car_class: r.get(5)?,
            car_pi: r.get(6)?,
            drivetrain_type: r.get(7)?,
            car_group: r.get(8)?,
            valid: r.get::<_, i64>(9)? != 0,
            invalid_reason: r.get(10)?,
            computed_score: r.get(11)?,
            manual_score: r.get(12)?,
            packet_count: r.get(13)?,
            score_breakdown: breakdown.and_then(|s| serde_json::from_str(&s).ok()),
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        conn
    }

    #[test]
    fn init_creates_tables() {
        let conn = in_memory();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sessions','session_packets')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn open_and_close_session() {
        let conn = in_memory();
        let id = open_session(&conn, 12345, 3, 5, 900).unwrap();
        assert!(id > 0);
        close_session(&conn, id, 1000, 78.5).unwrap();
        let ended: Option<i64> = conn
            .query_row("SELECT ended_at FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(ended.is_some());
    }

    #[test]
    fn reopen_clears_ended_at() {
        let conn = in_memory();
        let id = open_session(&conn, 12345, 3, 5, 900).unwrap();
        close_session(&conn, id, 1000, 78.5).unwrap();
        reopen_session(&conn, id).unwrap();
        let ended: Option<i64> = conn
            .query_row("SELECT ended_at FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(ended.is_none());
    }

    #[test]
    fn close_records_caller_supplied_best() {
        // best_lap is now the authoritative MIN of the lap table supplied by
        // the caller, so the latest close with a real value wins (this is what
        // corrects a provisional partial after a rewind).
        let conn = in_memory();
        let id = open_session(&conn, 0, 0, 0, 0).unwrap();
        close_session(&conn, id, 100, 37.0).unwrap(); // provisional partial
        reopen_session(&conn, id).unwrap();
        close_session(&conn, id, 200, 54.0).unwrap(); // true min after upsert
        let best: Option<f32> = conn
            .query_row("SELECT best_lap FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(best, Some(54.0));
    }

    #[test]
    fn close_updates_to_better_best_lap() {
        let conn = in_memory();
        let id = open_session(&conn, 0, 0, 0, 0).unwrap();
        close_session(&conn, id, 100, 65.5).unwrap();
        reopen_session(&conn, id).unwrap();
        // Better lap after rewind — should update
        close_session(&conn, id, 200, 60.0).unwrap();
        let best: Option<f32> = conn
            .query_row("SELECT best_lap FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(best, Some(60.0));
    }

    #[test]
    fn close_with_no_lap_keeps_existing_best() {
        let conn = in_memory();
        let id = open_session(&conn, 0, 0, 0, 0).unwrap();
        close_session(&conn, id, 100, 65.5).unwrap();
        reopen_session(&conn, id).unwrap();
        // -1.0 means no lap was recorded post-rewind
        close_session(&conn, id, 200, -1.0).unwrap();
        let best: Option<f32> = conn
            .query_row("SELECT best_lap FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(best, Some(65.5));
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = in_memory();
        // init() already ran migrate once; running again must not error.
        migrate(&conn);
        migrate(&conn);
        let id = open_session(&conn, 1, 0, 0, 0).unwrap();
        let rows = list_sessions(&conn).unwrap();
        let row = rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.name, None);
        assert!(!row.bookmarked);
    }

    #[test]
    fn rename_and_clear_session() {
        let conn = in_memory();
        let id = open_session(&conn, 1, 0, 0, 0).unwrap();
        rename_session(&conn, id, Some("  Nürburgring hotlap  ")).unwrap();
        let name: Option<String> = conn
            .query_row("SELECT name FROM sessions WHERE id=?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(name.as_deref(), Some("Nürburgring hotlap"));
        // Whitespace-only clears back to default (NULL).
        rename_session(&conn, id, Some("   ")).unwrap();
        let cleared: Option<String> = conn
            .query_row("SELECT name FROM sessions WHERE id=?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(cleared, None);
    }

    #[test]
    fn bookmark_toggle_and_ordering() {
        let conn = in_memory();
        let a = open_session(&conn, 100, 0, 0, 0).unwrap();
        let b = open_session(&conn, 200, 0, 0, 0).unwrap();
        set_session_bookmark(&conn, a, true).unwrap();
        let rows = list_sessions(&conn).unwrap();
        // Bookmarked session sorts first despite older started_at.
        assert_eq!(rows[0].id, a);
        assert!(rows[0].bookmarked);
        assert_eq!(rows[1].id, b);
        set_session_bookmark(&conn, a, false).unwrap();
        let rows = list_sessions(&conn).unwrap();
        assert_eq!(rows[0].id, b);
    }

    #[test]
    fn laps_insert_get_and_replace() {
        let conn = in_memory();
        let id = open_session(&conn, 1, 0, 0, 0).unwrap();
        insert_lap(&conn, id, 1, 92.5).unwrap();
        insert_lap(&conn, id, 2, 90.1).unwrap();
        // Re-driven lap 2 (rewind) overwrites, no duplicate row.
        insert_lap(&conn, id, 2, 88.0).unwrap();
        let laps = get_session_laps(&conn, id).unwrap();
        assert_eq!(laps.len(), 2);
        assert_eq!(laps[0].lap_number, 1);
        assert!((laps[1].lap_time - 88.0).abs() < 0.001);
    }

    #[test]
    fn delete_session_removes_laps_and_packets() {
        let conn = in_memory();
        let id = open_session(&conn, 1, 0, 0, 0).unwrap();
        insert_lap(&conn, id, 1, 90.0).unwrap();
        insert_packet(&conn, id, 1, &vec![0u8; 311]).unwrap();
        delete_session(&conn, id).unwrap();
        assert_eq!(get_session_laps(&conn, id).unwrap().len(), 0);
        let pkts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_packets WHERE session_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pkts, 0);
    }

    #[test]
    fn clear_all_sessions_empties_everything() {
        let conn = in_memory();
        for s in 0..3 {
            let id = open_session(&conn, s, 0, 0, 0).unwrap();
            insert_lap(&conn, id, 1, 80.0).unwrap();
            insert_packet(&conn, id, 1, &vec![0u8; 311]).unwrap();
        }
        clear_all_sessions(&conn).unwrap();
        assert_eq!(list_sessions(&conn).unwrap().len(), 0);
        let counts: (i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM session_packets), (SELECT COUNT(*) FROM session_laps)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0));
    }

    #[test]
    fn insert_and_count_packets() {
        let conn = in_memory();
        let id = open_session(&conn, 0, 0, 0, 0).unwrap();
        let blob = vec![0u8; 311];
        insert_packet(&conn, id, 1000, &blob).unwrap();
        insert_packet(&conn, id, 2000, &blob).unwrap();
        let count: i64 = conn
            .query_row("SELECT packet_count FROM sessions WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn drift_tables_are_created() {
        let conn = in_memory();
        for table in [
            "drift_zones",
            "drift_runs",
            "drift_run_packets",
            "drift_run_splits",
            "drift_run_scores",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing table {table}");
        }
    }

    fn zone_input(name: &str) -> DriftZoneInput {
        DriftZoneInput {
            id: None,
            name: name.into(),
            description: Some("test zone".into()),
            active: true,
            left_boundary: vec![ZonePoint { x: 0.0, z: 0.0 }, ZonePoint { x: 0.0, z: 10.0 }],
            right_boundary: vec![ZonePoint { x: 5.0, z: 0.0 }, ZonePoint { x: 5.0, z: 10.0 }],
            start_gate: Vec::new(),
            finish_gate: Vec::new(),
            split_gates: vec![vec![
                ZonePoint { x: 0.0, z: 5.0 },
                ZonePoint { x: 5.0, z: 5.0 },
            ]],
            scoring_config: serde_json::json!({ "version": 1 }),
        }
    }

    #[test]
    fn drift_zone_save_lists_and_derives_gates() {
        let conn = in_memory();
        let id = save_drift_zone(&conn, &zone_input("Switchbacks"), 1000).unwrap();
        let zones = list_drift_zones(&conn).unwrap();
        assert_eq!(zones.len(), 1);
        let zone = &zones[0];
        assert_eq!(zone.id, id);
        assert_eq!(zone.name, "Switchbacks");
        assert_eq!(zone.left_boundary.len(), 2);
        assert_eq!(zone.right_boundary.len(), 2);
        assert_eq!(zone.start_gate.len(), 2);
        assert_eq!(zone.finish_gate.len(), 2);
        assert_eq!(zone.split_gates.len(), 1);
        assert_eq!(zone.scoring_config["version"], 1);
    }

    #[test]
    fn drift_zone_update_replaces_geometry() {
        let conn = in_memory();
        let id = save_drift_zone(&conn, &zone_input("Original"), 1000).unwrap();
        let mut update = zone_input("Updated");
        update.id = Some(id);
        update.active = false;
        update.left_boundary.push(ZonePoint { x: 1.0, z: 20.0 });
        save_drift_zone(&conn, &update, 2000).unwrap();

        let zone = get_drift_zone(&conn, id).unwrap();
        assert_eq!(zone.name, "Updated");
        assert!(!zone.active);
        assert_eq!(zone.updated_at, 2000);
        assert_eq!(zone.left_boundary.len(), 3);
        assert_eq!(list_drift_zones(&conn).unwrap().len(), 1);
    }

    #[test]
    fn drift_zone_delete_keeps_runs_with_null_zone() {
        let conn = in_memory();
        let zone_id = save_drift_zone(&conn, &zone_input("Zone"), 1000).unwrap();
        let run_id = open_drift_run(&conn, Some(zone_id), 1100, 1, 2, 300, 2, 3).unwrap();
        delete_drift_zone(&conn, zone_id).unwrap();
        assert_eq!(list_drift_zones(&conn).unwrap().len(), 0);
        let zone: Option<i64> = conn
            .query_row(
                "SELECT zone_id FROM drift_runs WHERE id=?1",
                [run_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(zone, None);
    }

    #[test]
    fn drift_run_keeps_car_metadata_and_packets() {
        let conn = in_memory();
        let id = open_drift_run(&conn, None, 123, 3249, 5, 900, 1, 77).unwrap();
        insert_drift_run_packet(&conn, id, 1000, &vec![1u8; 324]).unwrap();
        insert_drift_run_packet(&conn, id, 1016, &vec![2u8; 324]).unwrap();
        close_drift_run(&conn, id, 5000, true, None, Some(12345.0)).unwrap();

        let rows = list_drift_runs(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.id, id);
        assert_eq!(row.car_ordinal, 3249);
        assert_eq!(row.car_class, 5);
        assert_eq!(row.car_pi, 900);
        assert_eq!(row.drivetrain_type, 1);
        assert_eq!(row.car_group, 77);
        assert_eq!(row.packet_count, 2);
        assert!(row.valid);
        assert!((row.computed_score.unwrap() - 12345.0).abs() < 0.001);
    }

    #[test]
    fn drift_manual_score_upserts() {
        let conn = in_memory();
        let id = open_drift_run(&conn, None, 123, 1, 2, 300, 2, 3).unwrap();
        set_drift_run_manual_score(&conn, id, 42_000, Some("first read"), 1000).unwrap();
        set_drift_run_manual_score(&conn, id, 43_500, Some("corrected"), 2000).unwrap();

        let rows = list_drift_runs(&conn).unwrap();
        assert_eq!(rows[0].manual_score, Some(43_500));
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM drift_run_scores WHERE run_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}

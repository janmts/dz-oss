use std::collections::VecDeque;

use rusqlite::Connection;
use serde::Serialize;

use crate::{db, parser, scoring};

/// Inter-arrival gap (wall-clock ms) above which a run treats the elapsed time
/// as a telemetry stall — a pause, alt-tab, menu, or UDP stall — rather than
/// genuine in-game non-scoring time. Telemetry is a fixed ~64 Hz tick
/// (~15.6 ms between packets), and even a long burst of UDP loss stays well
/// under this, so anything larger is a delivery stop, not the car failing to
/// score. Such time is excluded from the starvation timer (see `note_packet`).
const STALL_GAP_MS: i64 = 500;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub z: f64,
}

impl From<&db::ZonePoint> for Point {
    fn from(value: &db::ZonePoint) -> Self {
        Self {
            x: value.x,
            z: value.z,
        }
    }
}

#[derive(Debug, Clone)]
struct RunnableZone {
    id: i64,
    name: String,
    polygon: Vec<Point>,
    // The two end gates. A run may enter through either and must exit the other,
    // so the zone is bidirectional — neither is intrinsically start nor finish.
    gate_a: [Point; 2],
    gate_b: [Point; 2],
    /// Metres a point may stray outside the polygon and still count as inside.
    /// Retained (and still parsed from the zone config) but no longer gates run
    /// validity: FH6's real fail condition is score-starvation, not a spatial
    /// boundary, so [`DriftRunManager::note_packet`] aborts on the starvation
    /// timer instead. Kept for the `within_slack` helper / possible future use.
    #[allow(dead_code)]
    slack_m: f64,
    params: scoring::ScoringParams,
}

#[derive(Debug, Clone)]
struct ActiveRun {
    id: i64,
    zone: RunnableZone,
    /// The gate this run must cross to complete — the one it did NOT enter through.
    finish_gate: [Point; 2],
    started_at: i64,
    packet_count: i64,
    /// Wall-clock ms of the last packet that earned points. The run aborts if
    /// this falls more than the configured starvation timeout behind `now`.
    last_score_ms: i64,
    /// Wall-clock ms of the previous packet seen during this run (scoring or
    /// not). Drives stall detection: telemetry is a fixed ~64 Hz tick, so a gap
    /// far larger than that means delivery stopped — a pause, alt-tab, menu, or
    /// UDP stall — frozen time that must not count toward starvation.
    last_packet_ms: i64,
    /// Drift-direction sign (+1/−1) latched at the last scoring packet; 0 until
    /// the run first scores. Drives the live flip counter.
    score_sign: i8,
    /// Signed-direction reversals between consecutive scoring packets so far —
    /// the live "flip count" instrument (same definition as the stored
    /// `directionFlips` breakdown field).
    direction_flips: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftRunStatus {
    pub state: String,
    pub run_id: Option<i64>,
    pub zone_id: Option<i64>,
    pub zone_name: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub packet_count: i64,
    pub invalid_reason: Option<String>,
    /// Whether the latest packet is currently earning points (drifting with a
    /// tyre on tarmac). False while the "death timer" is counting down.
    pub scoring: bool,
    /// Seconds left before the run aborts from score starvation, while running
    /// and not currently scoring. `None` when scoring, idle, closed, or when the
    /// starvation timer is disabled.
    pub starve_remaining_s: Option<f64>,
    /// Live signed drift angle (°) of the latest packet — positive one way,
    /// negative the other. `None` when idle or closed.
    pub angle_deg: Option<f64>,
    /// Live speed (m/s) of the latest packet. `None` when idle or closed.
    pub speed_ms: Option<f64>,
    /// Direction flips (signed-angle reversals between scoring packets) so far
    /// in this run. `None` when idle.
    pub direction_flips: Option<u32>,
}

impl DriftRunStatus {
    pub fn idle() -> Self {
        Self {
            state: "idle".into(),
            run_id: None,
            zone_id: None,
            zone_name: None,
            started_at: None,
            ended_at: None,
            packet_count: 0,
            invalid_reason: None,
            scoring: false,
            starve_remaining_s: None,
            angle_deg: None,
            speed_ms: None,
            direction_flips: None,
        }
    }

    fn running(
        run: &ActiveRun,
        scoring: bool,
        starve_remaining_s: Option<f64>,
        pkt: &parser::TelemetryPacket,
    ) -> Self {
        Self {
            state: "running".into(),
            run_id: Some(run.id),
            zone_id: Some(run.zone.id),
            zone_name: Some(run.zone.name.clone()),
            started_at: Some(run.started_at),
            ended_at: None,
            packet_count: run.packet_count,
            invalid_reason: None,
            scoring,
            starve_remaining_s: if scoring { None } else { starve_remaining_s },
            angle_deg: Some(scoring::drift_angle_signed_deg(pkt)),
            speed_ms: Some(pkt.speed_ms as f64),
            direction_flips: Some(run.direction_flips),
        }
    }

    fn closed(run: &ActiveRun, ended_at: i64, valid: bool, invalid_reason: Option<String>) -> Self {
        Self {
            state: if valid { "completed" } else { "invalid" }.into(),
            run_id: Some(run.id),
            zone_id: Some(run.zone.id),
            zone_name: Some(run.zone.name.clone()),
            started_at: Some(run.started_at),
            ended_at: Some(ended_at),
            packet_count: run.packet_count,
            invalid_reason,
            scoring: false,
            starve_remaining_s: None,
            angle_deg: None,
            speed_ms: None,
            direction_flips: Some(run.direction_flips),
        }
    }
}

pub struct DriftRunManager {
    active: Option<ActiveRun>,
    last_point: Option<Point>,
    last_status: DriftRunStatus,
    /// Rolling PRE-ROLL trail: raw packets seen while NO run is active, as
    /// (packet timestamp_ms, wall-clock ms, raw bytes), oldest first. When a
    /// run starts the buffer is stored alongside it (drift_run_preroll_packets)
    /// so analysis can see how the car approached the gate — e.g. how long its
    /// drift had been established, which measurably changes when the game
    /// starts crediting. Disjoint from drift_run_packets by construction:
    /// in-run packets are never buffered, and the buffer is cleared once
    /// flushed (a back-to-back re-entry gets a trail reaching back only to the
    /// previous run's end). ~64 Hz × 10 s × 324 B ≈ 200 KB of RAM at default.
    preroll: VecDeque<(u32, i64, Vec<u8>)>,
}

impl DriftRunManager {
    pub fn new() -> Self {
        Self {
            active: None,
            last_point: None,
            last_status: DriftRunStatus::idle(),
            preroll: VecDeque::new(),
        }
    }

    pub fn status(&self) -> DriftRunStatus {
        self.last_status.clone()
    }

    pub fn note_packet(
        &mut self,
        conn: &Connection,
        pkt: &parser::TelemetryPacket,
        raw: &[u8],
        now_ms: i64,
        starve_timeout_s: f64,
        preroll_s: f64,
    ) -> Option<DriftRunStatus> {
        let current = packet_point(pkt)?;
        let previous = self.last_point;
        self.last_point = Some(current);

        if let Some(run) = self.active.as_mut() {
            if segment_crosses_gate(previous, current, run.finish_gate) {
                if let Err(e) = db::insert_drift_run_packet(conn, run.id, pkt.timestamp_ms, raw) {
                    eprintln!("[drift] packet insert error: {e}");
                } else {
                    run.packet_count += 1;
                }
                let (score, breakdown) = score_from_packets(conn, run.id, &run.zone.params);
                let status = DriftRunStatus::closed(run, now_ms, true, None);
                if let Err(e) = db::close_drift_run(conn, run.id, now_ms, true, None, score) {
                    eprintln!("[drift] close error: {e}");
                }
                if let Err(e) =
                    db::update_drift_run_score(conn, run.id, score, breakdown.as_deref())
                {
                    eprintln!("[drift] score store error: {e}");
                }
                self.active = None;
                self.last_status = status.clone();
                return Some(status);
            }

            // Every packet while the run is live belongs to it — record it
            // regardless of whether it's scoring (off-track packets are kept so
            // re-scoring sees the full run).
            if let Err(e) = db::insert_drift_run_packet(conn, run.id, pkt.timestamp_ms, raw) {
                eprintln!("[drift] packet insert error: {e}");
            } else {
                run.packet_count += 1;
            }

            // Exclude paused/stalled wall-clock time from the starvation timer.
            // While the game is paused (or alt-tabbed, in a menu, or briefly
            // UDP-stalled) NO packets arrive, yet the wall clock keeps running.
            // The first packet after the stall would otherwise see the entire
            // gap as non-scoring time and abort instantly on resume. A gap far
            // larger than the ~64 Hz tick can't be in-game time, so roll the
            // last-score stamp forward by the gap (capped at now) — only
            // continuous, in-game non-scoring time counts toward the timeout.
            let gap_ms = now_ms - run.last_packet_ms;
            if gap_ms > STALL_GAP_MS {
                run.last_score_ms = (run.last_score_ms + gap_ms).min(now_ms);
            }
            run.last_packet_ms = now_ms;

            // Score-starvation abort. FH6 ends a run when no drift points have
            // accrued for a while — NOT when the car leaves a spatial boundary
            // (the polygon is only used to detect the entry gate-crossing). A
            // tyre on tarmac off in a side road keeps scoring and stays alive;
            // wandering into the scenery stops scoring and times out.
            let scoring = scoring::is_scoring_packet(pkt, &run.zone.params);
            if scoring {
                run.last_score_ms = now_ms;
                let sign = if scoring::drift_angle_signed_deg(pkt) >= 0.0 {
                    1i8
                } else {
                    -1i8
                };
                if run.score_sign != 0 && sign != run.score_sign {
                    run.direction_flips += 1;
                }
                run.score_sign = sign;
            }
            let starve_s = (now_ms - run.last_score_ms).max(0) as f64 / 1000.0;
            if starve_timeout_s > 0.0 && starve_s > starve_timeout_s {
                let reason = format!("no drift score for {starve_timeout_s:.0}s");
                let (score, breakdown) = score_from_packets(conn, run.id, &run.zone.params);
                let status = DriftRunStatus::closed(run, now_ms, false, Some(reason.clone()));
                if let Err(e) =
                    db::close_drift_run(conn, run.id, now_ms, false, Some(&reason), score)
                {
                    eprintln!("[drift] starvation close error: {e}");
                }
                if let Err(e) =
                    db::update_drift_run_score(conn, run.id, score, breakdown.as_deref())
                {
                    eprintln!("[drift] score store error: {e}");
                }
                self.active = None;
                self.last_status = status.clone();
                return Some(status);
            }

            let remaining = (starve_timeout_s > 0.0).then(|| (starve_timeout_s - starve_s).max(0.0));
            let status = DriftRunStatus::running(run, scoring, remaining, pkt);
            self.last_status = status.clone();
            return Some(status);
        }

        // Idle: keep the pre-roll trail current. The packet is appended AFTER
        // the gate check below, so a run-opening packet (already stored as the
        // run's first packet) never lands in its own trail.
        self.trim_preroll(now_ms, preroll_s);

        let Some(previous) = previous else {
            self.push_preroll(pkt.timestamp_ms, now_ms, raw, preroll_s);
            return None;
        };
        let zones = match db::list_drift_zones(conn) {
            Ok(zones) => zones,
            Err(e) => {
                eprintln!("[drift] zone list error: {e}");
                self.push_preroll(pkt.timestamp_ms, now_ms, raw, preroll_s);
                return None;
            }
        };
        // A run starts only when the car crosses *between* an end gate's two
        // points while entering the polygon — matching the game's "between the
        // flags" gates. Bidirectional: whichever gate was crossed is the entry,
        // the other is the finish. (This precise crossing test relies on the end
        // gates being correctly placed — see `from_row`, which always derives
        // them from the current boundary so they can't go stale.)
        let started = zones
            .iter()
            .filter_map(RunnableZone::from_row)
            .filter_map(|zone| {
                if point_in_polygon(previous, &zone.polygon)
                    || !point_in_polygon(current, &zone.polygon)
                {
                    return None;
                }
                let crossed_a =
                    segment_intersects(previous, current, zone.gate_a[0], zone.gate_a[1]);
                let crossed_b =
                    segment_intersects(previous, current, zone.gate_b[0], zone.gate_b[1]);
                let (entry, finish) = if crossed_a {
                    (zone.gate_a, zone.gate_b)
                } else if crossed_b {
                    (zone.gate_b, zone.gate_a)
                } else {
                    return None;
                };
                Some((zone, entry, finish))
            })
            .min_by(|a, b| {
                gate_distance_sq(current, a.1)
                    .partial_cmp(&gate_distance_sq(current, b.1))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        let Some((zone, _entry, finish_gate)) = started else {
            self.push_preroll(pkt.timestamp_ms, now_ms, raw, preroll_s);
            return None;
        };
        // Bind the run to the in-game season at its start: outside winter the
        // tarmac gate is dropped (grass pays — and therefore also keeps the
        // starvation timer fed). The seasoned params drive live scoring,
        // starvation, and the close-time score for this run's whole life.
        let mut zone = zone;
        zone.params = zone.params.for_season(crate::season::season_at_utc_ms(now_ms));

        match db::open_drift_run(
            conn,
            Some(zone.id),
            now_ms,
            pkt.car_ordinal,
            pkt.car_class,
            pkt.car_pi,
            pkt.drivetrain_type,
            pkt.car_group,
        ) {
            Ok(id) => {
                let mut run = ActiveRun {
                    id,
                    zone,
                    finish_gate,
                    started_at: now_ms,
                    packet_count: 0,
                    last_score_ms: now_ms,
                    last_packet_ms: now_ms,
                    score_sign: 0,
                    direction_flips: 0,
                };
                // Attach the buffered approach trail to the run, then drop it —
                // a flushed trail belongs to exactly one run.
                let trail: Vec<(u32, Vec<u8>)> = self
                    .preroll
                    .drain(..)
                    .map(|(ts, _, data)| (ts, data))
                    .collect();
                if let Err(e) = db::insert_drift_run_preroll(conn, id, &trail) {
                    eprintln!("[drift] preroll insert error: {e}");
                }
                if let Err(e) = db::insert_drift_run_packet(conn, id, pkt.timestamp_ms, raw) {
                    eprintln!("[drift] opening packet insert error: {e}");
                } else {
                    run.packet_count = 1;
                }
                let scoring = scoring::is_scoring_packet(pkt, &run.zone.params);
                if scoring {
                    run.last_score_ms = now_ms;
                    run.score_sign = if scoring::drift_angle_signed_deg(pkt) >= 0.0 {
                        1
                    } else {
                        -1
                    };
                }
                let remaining = (starve_timeout_s > 0.0).then_some(starve_timeout_s);
                let status = DriftRunStatus::running(&run, scoring, remaining, pkt);
                self.active = Some(run);
                self.last_status = status.clone();
                Some(status)
            }
            Err(e) => {
                eprintln!("[drift] open error: {e}");
                None
            }
        }
    }

    /// Append a packet to the pre-roll trail (no-op when the trail is disabled).
    fn push_preroll(&mut self, timestamp_ms: u32, now_ms: i64, raw: &[u8], preroll_s: f64) {
        if preroll_s <= 0.0 {
            return;
        }
        self.preroll.push_back((timestamp_ms, now_ms, raw.to_vec()));
    }

    /// Drop trail entries older than the window (all of them if disabled —
    /// covers the setting being turned down/off while idle).
    fn trim_preroll(&mut self, now_ms: i64, preroll_s: f64) {
        if preroll_s <= 0.0 {
            self.preroll.clear();
            return;
        }
        let cutoff = now_ms - (preroll_s * 1000.0) as i64;
        while matches!(self.preroll.front(), Some((_, t, _)) if *t < cutoff) {
            self.preroll.pop_front();
        }
    }
}

impl Default for DriftRunManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RunnableZone {
    fn from_row(row: &db::DriftZoneRow) -> Option<Self> {
        if !row.active || row.left_boundary.len() < 2 || row.right_boundary.len() < 2 {
            return None;
        }
        // The end gates ARE the first/last boundary point pairs — always derived
        // from the current boundary, never the stored gate (which can go stale
        // when the boundary is edited after the first save).
        let gate_a = [
            Point::from(row.left_boundary.first()?),
            Point::from(row.right_boundary.first()?),
        ];
        let gate_b = [
            Point::from(row.left_boundary.last()?),
            Point::from(row.right_boundary.last()?),
        ];
        let mut polygon: Vec<Point> = row.left_boundary.iter().map(Point::from).collect();
        polygon.extend(row.right_boundary.iter().rev().map(Point::from));
        if polygon.len() < 3 {
            return None;
        }
        Some(Self {
            id: row.id,
            name: row.name.clone(),
            polygon,
            gate_a,
            gate_b,
            slack_m: slack_from_config(&row.scoring_config),
            params: scoring::ScoringParams::from_config(&row.scoring_config),
        })
    }
}

/// Per-zone boundary slack (metres) from the zone's `scoring_config` bag,
/// defaulting to 3 m. Stored there (not a dedicated column) since it's just
/// another per-zone knob.
fn slack_from_config(config: &serde_json::Value) -> f64 {
    config
        .get("boundarySlackM")
        .and_then(|v| v.as_f64())
        .unwrap_or(3.0)
        .max(0.0)
}

/// True if `point` is inside the polygon, or outside it by no more than
/// `slack_m` metres (distance to the nearest edge). World coords are in metres.
pub fn within_slack(point: Point, polygon: &[Point], slack_m: f64) -> bool {
    if point_in_polygon(point, polygon) {
        return true;
    }
    if slack_m <= 0.0 || polygon.len() < 2 {
        return false;
    }
    let mut min_dist = f64::INFINITY;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        min_dist = min_dist.min(point_segment_dist(point, polygon[j], polygon[i]));
        if min_dist <= slack_m {
            return true;
        }
        j = i;
    }
    min_dist <= slack_m
}

/// Euclidean distance from `p` to segment `a`–`b`.
fn point_segment_dist(p: Point, a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    let len2 = dx * dx + dz * dz;
    let t = if len2 <= 1e-12 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.z - a.z) * dz) / len2).clamp(0.0, 1.0)
    };
    let projx = a.x + t * dx;
    let projz = a.z + t * dz;
    ((p.x - projx).powi(2) + (p.z - projz).powi(2)).sqrt()
}

/// Re-read a run's stored packets and score them. Runs once on close, off the
/// per-packet path; loading ~a few thousand blobs and parsing is well under a
/// frame. Returns the (computed_score, breakdown_json) to persist.
fn score_from_packets(
    conn: &Connection,
    run_id: i64,
    params: &scoring::ScoringParams,
) -> (Option<f32>, Option<String>) {
    let blobs = match db::get_drift_run_packets(conn, run_id) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[drift] score load error: {e}");
            return (None, None);
        }
    };
    let pkts: Vec<parser::TelemetryPacket> =
        blobs.iter().filter_map(|b| parser::parse(b).ok()).collect();
    let result = scoring::score_run(&pkts, params);
    (Some(result.score as f32), serde_json::to_string(&result).ok())
}


fn packet_point(pkt: &parser::TelemetryPacket) -> Option<Point> {
    if pkt.position_x == 0.0 && pkt.position_z == 0.0 {
        return None;
    }
    Some(Point {
        x: pkt.position_x as f64,
        z: pkt.position_z as f64,
    })
}

pub fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let pi = polygon[i];
        let pj = polygon[j];
        if point_on_segment(point, pi, pj) {
            return true;
        }
        let crosses = (pi.z > point.z) != (pj.z > point.z);
        if crosses {
            let x_at_z = (pj.x - pi.x) * (point.z - pi.z) / (pj.z - pi.z) + pi.x;
            if point.x < x_at_z {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn segment_crosses_gate(previous: Option<Point>, current: Point, gate: [Point; 2]) -> bool {
    previous
        .map(|prev| segment_intersects(prev, current, gate[0], gate[1]))
        .unwrap_or(false)
}

pub fn segment_intersects(a: Point, b: Point, c: Point, d: Point) -> bool {
    let o1 = orientation(a, b, c);
    let o2 = orientation(a, b, d);
    let o3 = orientation(c, d, a);
    let o4 = orientation(c, d, b);

    if o1 == 0.0 && point_on_segment(c, a, b) {
        return true;
    }
    if o2 == 0.0 && point_on_segment(d, a, b) {
        return true;
    }
    if o3 == 0.0 && point_on_segment(a, c, d) {
        return true;
    }
    if o4 == 0.0 && point_on_segment(b, c, d) {
        return true;
    }
    (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0)
}

fn orientation(a: Point, b: Point, c: Point) -> f64 {
    let v = (b.z - a.z) * (c.x - b.x) - (b.x - a.x) * (c.z - b.z);
    if v.abs() < 1e-9 {
        0.0
    } else {
        v
    }
}

fn point_on_segment(p: Point, a: Point, b: Point) -> bool {
    orientation(a, b, p) == 0.0
        && p.x >= a.x.min(b.x) - 1e-9
        && p.x <= a.x.max(b.x) + 1e-9
        && p.z >= a.z.min(b.z) - 1e-9
        && p.z <= a.z.max(b.z) + 1e-9
}

fn gate_distance_sq(point: Point, gate: [Point; 2]) -> f64 {
    let mid = Point {
        x: (gate[0].x + gate[1].x) / 2.0,
        z: (gate[0].z + gate[1].z) / 2.0,
    };
    (point.x - mid.x).powi(2) + (point.z - mid.z).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_zone() -> db::DriftZoneRow {
        db::DriftZoneRow {
            id: 1,
            name: "Test Zone".into(),
            description: None,
            created_at: 0,
            updated_at: 0,
            active: true,
            left_boundary: vec![
                db::ZonePoint { x: 0.0, z: 0.0 },
                db::ZonePoint { x: 0.0, z: 10.0 },
            ],
            right_boundary: vec![
                db::ZonePoint { x: 5.0, z: 0.0 },
                db::ZonePoint { x: 5.0, z: 10.0 },
            ],
            start_gate: Vec::new(),
            finish_gate: Vec::new(),
            split_gates: Vec::new(),
            scoring_config: serde_json::json!({}),
        }
    }

    fn packet(x: f32, z: f32) -> parser::TelemetryPacket {
        parser::TelemetryPacket {
            is_race_on: true,
            timestamp_ms: 100,
            engine_max_rpm: 0.0,
            engine_idle_rpm: 0.0,
            current_engine_rpm: 0.0,
            accel_x: 0.0,
            accel_y: 0.0,
            accel_z: 0.0,
            vel_x: 0.0,
            vel_y: 0.0,
            vel_z: 0.0,
            angular_velocity_x: 0.0,
            angular_velocity_y: 0.0,
            angular_velocity_z: 0.0,
            position_x: x,
            position_y: 0.0,
            position_z: z,
            tire_slip_ratio_fl: 0.0,
            tire_slip_ratio_fr: 0.0,
            tire_slip_ratio_rl: 0.0,
            tire_slip_ratio_rr: 0.0,
            tire_slip_angle_fl: 0.0,
            tire_slip_angle_fr: 0.0,
            tire_slip_angle_rl: 0.0,
            tire_slip_angle_rr: 0.0,
            tire_combined_slip_fl: 0.0,
            tire_combined_slip_fr: 0.0,
            tire_combined_slip_rl: 0.0,
            tire_combined_slip_rr: 0.0,
            surface_rumble_fl: 0.0,
            surface_rumble_fr: 0.0,
            surface_rumble_rl: 0.0,
            surface_rumble_rr: 0.0,
            car_ordinal: 3249,
            car_class: 5,
            car_pi: 900,
            drivetrain_type: 1,
            num_cylinders: 8,
            car_group: 77,
            smashable_vel_diff: 0.0,
            smashable_mass: 0.0,
            speed_ms: 0.0,
            power: 0.0,
            torque: 0.0,
            tire_temp_fl: 0.0,
            tire_temp_fr: 0.0,
            tire_temp_rl: 0.0,
            tire_temp_rr: 0.0,
            boost: 0.0,
            fuel: 0.0,
            distance_traveled: 0.0,
            best_lap: 0.0,
            last_lap: 0.0,
            current_lap: 0.0,
            current_race_time: 0.0,
            lap_number: 0,
            race_position: 0,
            throttle: 0,
            brake: 0,
            clutch: 0,
            handbrake: 0,
            gear: 1,
            steer: 0,
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            suspension_fl: 0.0,
            suspension_fr: 0.0,
            suspension_rl: 0.0,
            suspension_rr: 0.0,
            tire_wear_fl: None,
            tire_wear_fr: None,
            tire_wear_rl: None,
            tire_wear_rr: None,
        }
    }

    /// A scoring packet at world (x, z): ~30° sideslip at 20 m/s, rears sliding,
    /// all wheels on tarmac (surface_rumble 0). `is_scoring_packet` is true.
    fn drifting_packet(x: f32, z: f32, ms: u32) -> parser::TelemetryPacket {
        let mut p = packet(x, z);
        p.timestamp_ms = ms;
        let b = 30f64.to_radians();
        p.speed_ms = 20.0;
        p.vel_x = (20.0 * b.sin()) as f32;
        p.vel_z = (20.0 * b.cos()) as f32;
        p.tire_combined_slip_rl = 3.0;
        p.tire_combined_slip_rr = 3.0;
        p
    }

    fn in_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    #[test]
    fn point_in_zone_polygon() {
        let zone = RunnableZone::from_row(&square_zone()).unwrap();
        assert!(point_in_polygon(Point { x: 2.5, z: 5.0 }, &zone.polygon));
        assert!(!point_in_polygon(Point { x: 8.0, z: 5.0 }, &zone.polygon));
    }

    #[test]
    fn segment_crosses_gate() {
        assert!(segment_intersects(
            Point { x: 2.0, z: -1.0 },
            Point { x: 2.0, z: 1.0 },
            Point { x: 0.0, z: 0.0 },
            Point { x: 5.0, z: 0.0 },
        ));
        assert!(!segment_intersects(
            Point { x: 6.0, z: -1.0 },
            Point { x: 6.0, z: 1.0 },
            Point { x: 0.0, z: 0.0 },
            Point { x: 5.0, z: 0.0 },
        ));
    }

    #[test]
    fn run_starts_and_finishes_on_gates() {
        let conn = in_memory();
        db::save_drift_zone(
            &conn,
            &db::DriftZoneInput {
                id: None,
                name: "Run".into(),
                description: None,
                active: true,
                left_boundary: square_zone().left_boundary,
                right_boundary: square_zone().right_boundary,
                start_gate: Vec::new(),
                finish_gate: Vec::new(),
                split_gates: Vec::new(),
                scoring_config: serde_json::json!({}),
            },
            1,
        )
        .unwrap();
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        assert!(mgr
            .note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, 0.0, 10.0)
            .is_none());
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, 0.0, 10.0)
            .unwrap();
        assert_eq!(started.state, "running");
        let finished = mgr
            .note_packet(&conn, &packet(2.0, 11.0), &raw, 2000, 0.0, 10.0)
            .unwrap();
        assert_eq!(finished.state, "completed");
        let rows = db::list_drift_runs(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].valid);
        assert_eq!(rows[0].packet_count, 2);
    }

    #[test]
    fn leaving_zone_no_longer_voids_the_run() {
        // The polygon no longer gates validity — straying outside it keeps the
        // run alive (starvation disabled here with timeout 0 to isolate this).
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, 0.0, 10.0);
        mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, 0.0, 10.0);
        // x=9 is far outside the x=5 edge — previously invalid, now still running.
        let still = mgr
            .note_packet(&conn, &packet(9.0, 5.0), &raw, 1300, 0.0, 10.0)
            .unwrap();
        assert_eq!(still.state, "running");
        assert!(db::list_drift_runs(&conn).unwrap()[0].valid);
    }

    fn save_square_zone(conn: &Connection, slack: f64) -> i64 {
        db::save_drift_zone(
            conn,
            &db::DriftZoneInput {
                id: None,
                name: "Run".into(),
                description: None,
                active: true,
                left_boundary: square_zone().left_boundary,
                right_boundary: square_zone().right_boundary,
                start_gate: Vec::new(),
                finish_gate: Vec::new(),
                split_gates: Vec::new(),
                scoring_config: serde_json::json!({ "boundarySlackM": slack }),
            },
            1,
        )
        .unwrap()
    }

    #[test]
    fn run_aborts_on_score_starvation() {
        // No points for longer than the timeout (1 s here) → the run aborts.
        // Packets arrive at a realistic sub-stall cadence (100 ms) so this is
        // genuine in-game starvation, not a telemetry stall the gap-skip would
        // absorb (see `pause_does_not_abort_run_on_resume`).
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, 1.0, 10.0);
        let started = mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, 1.0, 10.0).unwrap();
        assert_eq!(started.state, "running");
        // Non-scoring packets every 100 ms; once >1 s has elapsed with no score
        // the run starves. Bounded loop so a logic regression can't hang.
        let mut now = 1200i64;
        let mut last = started;
        while last.state == "running" && now <= 4000 {
            last = mgr
                .note_packet(&conn, &packet(2.0, 5.0), &raw, now, 1.0, 10.0)
                .unwrap();
            now += 100;
        }
        assert_eq!(last.state, "invalid");
        assert!(!last.scoring);
        let rows = db::list_drift_runs(&conn).unwrap();
        assert!(!rows[0].valid);
        assert!(rows[0]
            .invalid_reason
            .as_deref()
            .unwrap()
            .contains("no drift score"));
    }

    /// Put all four wheels on a rough surface (fully off the tarmac).
    fn grass(mut p: parser::TelemetryPacket) -> parser::TelemetryPacket {
        p.surface_rumble_fl = 0.6;
        p.surface_rumble_fr = 0.6;
        p.surface_rumble_rl = 0.6;
        p.surface_rumble_rr = 0.6;
        p
    }

    #[test]
    fn grass_dwell_feeds_starvation_outside_winter_but_starves_in_winter() {
        // The tarmac gate is seasonal: outside winter, off-tarmac drifting
        // banks points (grass pays full rate), so a long grass excursion must
        // keep feeding the starvation timer. In winter the same excursion
        // banks nothing and the run starves out. Runs are bound to the season
        // of their wall-clock start time.
        for (t0, expect_alive) in [
            (crate::season::SPRING_ANCHOR_MS + 3_600_000, true), // spring
            (crate::season::SPRING_ANCHOR_MS - 3_600_000, false), // winter
        ] {
            let conn = in_memory();
            save_square_zone(&conn, 0.0);
            let mut mgr = DriftRunManager::new();
            let raw = vec![1u8; 324];
            mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, t0, 1.0, 10.0);
            let started = mgr
                .note_packet(&conn, &packet(2.0, 1.0), &raw, t0 + 100, 1.0, 10.0)
                .unwrap();
            assert_eq!(started.state, "running");
            // 2+ s of all-four-wheels-in-the-grass drifting at 100 ms cadence
            // (starve timeout is 1 s here).
            let mut last = started;
            let mut now = t0 + 200;
            let mut ms = 200u32;
            while last.state == "running" && now <= t0 + 2400 {
                last = mgr
                    .note_packet(&conn, &grass(drifting_packet(2.0, 5.0, ms)), &raw, now, 1.0, 10.0)
                    .unwrap();
                now += 100;
                ms += 100;
            }
            if expect_alive {
                assert_eq!(last.state, "running", "grass keeps a spring run alive");
                assert!(last.scoring, "spring grass packets are scoring packets");
            } else {
                assert_eq!(last.state, "invalid", "winter grass must starve the run");
                assert!(!last.scoring);
            }
        }
    }

    #[test]
    fn pause_does_not_abort_run_on_resume() {
        // A pause freezes telemetry: no packets arrive for the pause duration
        // while the wall clock keeps advancing. The first packet after a pause
        // longer than the timeout must NOT trip the starvation abort — under
        // the old wall-clock logic it saw the whole pause as non-scoring time
        // and killed the run instantly (issue #19).
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, 5.0, 10.0);
        // Enter scoring, then a couple of packets at normal ~64 Hz cadence.
        let started = mgr
            .note_packet(&conn, &drifting_packet(2.0, 1.0, 100), &raw, 1100, 5.0, 10.0)
            .unwrap();
        assert_eq!(started.state, "running");
        assert!(started.scoring);
        mgr.note_packet(&conn, &drifting_packet(2.0, 2.0, 116), &raw, 1116, 5.0, 10.0);
        // Pause for 30 s (>> the 5 s timeout), then resume with a NON-scoring
        // packet — the worst case, since there's no fresh score to reset the
        // timer. The stall window is excluded, so the run survives.
        let resumed = mgr
            .note_packet(&conn, &packet(2.0, 3.0), &raw, 31_116, 5.0, 10.0)
            .unwrap();
        assert_eq!(resumed.state, "running");
        // Genuine starvation still bites: ~64 Hz non-scoring packets after the
        // resume accumulate past the timeout and abort the run.
        let mut now = 31_216i64;
        let mut last = resumed;
        while last.state == "running" && now <= 40_000 {
            last = mgr
                .note_packet(&conn, &packet(2.0, 5.0), &raw, now, 5.0, 10.0)
                .unwrap();
            now += 100;
        }
        assert_eq!(last.state, "invalid");
    }

    #[test]
    fn scoring_packets_prevent_starvation() {
        // A run that keeps earning points stays alive well past the timeout.
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, 1.0, 10.0);
        mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, 1.0, 10.0);
        // Scoring packets every 0.5 s for 3 s — each resets the starve timer.
        let (mut t, mut now) = (1600u32, 1600i64);
        for _ in 0..6 {
            let s = mgr
                .note_packet(&conn, &drifting_packet(2.0, 5.0, t), &raw, now, 1.0, 10.0)
                .unwrap();
            assert_eq!(s.state, "running");
            assert!(s.scoring);
            t += 500;
            now += 500;
        }
        assert_eq!(mgr.status().state, "running");
    }

    #[test]
    fn starvation_disabled_keeps_run_alive_indefinitely() {
        // timeout 0 = disabled: a long non-scoring stretch never aborts.
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, 0.0, 10.0);
        mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, 0.0, 10.0);
        let still = mgr.note_packet(&conn, &packet(2.0, 5.0), &raw, 99000, 0.0, 10.0).unwrap();
        assert_eq!(still.state, "running");
    }

    #[test]
    fn run_enters_from_either_gate() {
        // Drive the zone the "reverse" way: enter through the far (z=10) gate and
        // exit through the near (z=0) gate. Bidirectional detection completes it.
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        assert!(mgr.note_packet(&conn, &packet(2.0, 11.0), &raw, 1000, 0.0, 10.0).is_none());
        let started = mgr.note_packet(&conn, &packet(2.0, 9.0), &raw, 1100, 0.0, 10.0).unwrap();
        assert_eq!(started.state, "running");
        let finished = mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 2000, 0.0, 10.0).unwrap();
        assert_eq!(finished.state, "completed");
        let rows = db::list_drift_runs(&conn).unwrap();
        assert!(rows[0].valid);
    }

    #[test]
    fn side_entry_without_gate_crossing_does_not_start() {
        // Entering through a long side (a wall, mid-zone) must NOT start a run —
        // a run requires crossing between an end gate's two points, like the
        // game's flag gates.
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        // (-1,5) outside; (1,5) inside, but the segment crosses the left boundary
        // (x=0) mid-zone — not an end gate (z=0 or z=10).
        assert!(mgr.note_packet(&conn, &packet(-1.0, 5.0), &raw, 1000, 0.0, 10.0).is_none());
        assert!(mgr.note_packet(&conn, &packet(1.0, 5.0), &raw, 1100, 0.0, 10.0).is_none());
        assert!(db::list_drift_runs(&conn).unwrap().is_empty());
    }

    #[test]
    fn inactive_or_incomplete_zones_are_ignored() {
        let mut inactive = square_zone();
        inactive.active = false;
        assert!(RunnableZone::from_row(&inactive).is_none());
        let mut incomplete = square_zone();
        incomplete.right_boundary.clear();
        assert!(RunnableZone::from_row(&incomplete).is_none());
    }

    #[test]
    fn preroll_trail_is_stored_with_the_run() {
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw_old = vec![7u8; 324];
        let raw_new = vec![8u8; 324];
        let raw_run = vec![9u8; 324];
        // One idle packet that will fall out of the 10s window, one inside it.
        let mut p = packet(2.0, -3.0);
        p.timestamp_ms = 100;
        mgr.note_packet(&conn, &p, &raw_old, 1_000, 0.0, 10.0);
        let mut p = packet(2.0, -1.0);
        p.timestamp_ms = 200;
        mgr.note_packet(&conn, &p, &raw_new, 50_000, 0.0, 10.0);
        // Gate crossing opens the run; this packet belongs to the run itself.
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw_run, 50_100, 0.0, 10.0)
            .unwrap();
        assert_eq!(started.state, "running");
        let run_id = started.run_id.unwrap();
        let trail = db::get_drift_run_preroll(&conn, run_id).unwrap();
        assert_eq!(trail.len(), 1, "only the in-window idle packet");
        assert_eq!(trail[0], raw_new);
        // The opening packet went to drift_run_packets, not the trail.
        assert_eq!(db::get_drift_run_packets(&conn, run_id).unwrap().len(), 1);
        assert_eq!(db::get_drift_run_packets(&conn, run_id).unwrap()[0], raw_run);
    }

    #[test]
    fn preroll_trail_is_consumed_by_the_run_it_opens() {
        // In-run packets are never buffered and a flushed trail is dropped, so
        // a back-to-back second run only gets the packets between the runs.
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        let between = vec![2u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1_000, 0.0, 10.0);
        let first = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1_100, 0.0, 10.0)
            .unwrap();
        assert_eq!(first.state, "running");
        let done = mgr
            .note_packet(&conn, &packet(2.0, 11.0), &raw, 2_000, 0.0, 10.0)
            .unwrap();
        assert_eq!(done.state, "completed");
        // One idle packet between the runs, then re-enter through the far gate.
        mgr.note_packet(&conn, &packet(2.0, 10.5), &between, 2_100, 0.0, 10.0);
        let second = mgr
            .note_packet(&conn, &packet(2.0, 9.0), &raw, 2_200, 0.0, 10.0)
            .unwrap();
        assert_eq!(second.state, "running");
        let trail = db::get_drift_run_preroll(&conn, second.run_id.unwrap()).unwrap();
        assert_eq!(trail.len(), 1, "trail reaches back only to the previous run's end");
        assert_eq!(trail[0], between);
    }

    #[test]
    fn preroll_disabled_keeps_no_trail() {
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1_000, 0.0, 0.0);
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1_100, 0.0, 0.0)
            .unwrap();
        assert_eq!(started.state, "running");
        assert!(db::get_drift_run_preroll(&conn, started.run_id.unwrap())
            .unwrap()
            .is_empty());
    }
}

use rusqlite::Connection;
use serde::Serialize;

use crate::{db, parser, scoring};

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
    /// Metres a point may stray outside the polygon and still count as inside,
    /// to tolerate sparse/corner-cutting boundary geometry. Per-zone tunable.
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
        }
    }

    fn running(run: &ActiveRun) -> Self {
        Self {
            state: "running".into(),
            run_id: Some(run.id),
            zone_id: Some(run.zone.id),
            zone_name: Some(run.zone.name.clone()),
            started_at: Some(run.started_at),
            ended_at: None,
            packet_count: run.packet_count,
            invalid_reason: None,
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
        }
    }
}

pub struct DriftRunManager {
    active: Option<ActiveRun>,
    last_point: Option<Point>,
    last_status: DriftRunStatus,
}

impl DriftRunManager {
    pub fn new() -> Self {
        Self {
            active: None,
            last_point: None,
            last_status: DriftRunStatus::idle(),
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

            if !within_slack(current, &run.zone.polygon, run.zone.slack_m) {
                let reason = "left drift zone before finish".to_string();
                let (score, breakdown) = score_from_packets(conn, run.id, &run.zone.params);
                let status = DriftRunStatus::closed(run, now_ms, false, Some(reason.clone()));
                if let Err(e) =
                    db::close_drift_run(conn, run.id, now_ms, false, Some(&reason), score)
                {
                    eprintln!("[drift] invalid close error: {e}");
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

            if let Err(e) = db::insert_drift_run_packet(conn, run.id, pkt.timestamp_ms, raw) {
                eprintln!("[drift] packet insert error: {e}");
            } else {
                run.packet_count += 1;
                let status = DriftRunStatus::running(run);
                self.last_status = status.clone();
                return Some(status);
            }
            return None;
        }

        let Some(previous) = previous else {
            return None;
        };
        let zones = match db::list_drift_zones(conn) {
            Ok(zones) => zones,
            Err(e) => {
                eprintln!("[drift] zone list error: {e}");
                return None;
            }
        };
        // A run starts when the car crosses into the polygon through *either* end
        // gate; whichever gate it crossed is the entry, the other is the finish.
        let started = zones
            .iter()
            .filter_map(RunnableZone::from_row)
            .filter_map(|zone| {
                if point_in_polygon(previous, &zone.polygon)
                    || !point_in_polygon(current, &zone.polygon)
                {
                    return None;
                }
                let crossed_a = segment_intersects(previous, current, zone.gate_a[0], zone.gate_a[1]);
                let crossed_b = segment_intersects(previous, current, zone.gate_b[0], zone.gate_b[1]);
                // Entry gate (for ranking) and the opposite gate (to finish on).
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
            return None;
        };

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
                };
                if let Err(e) = db::insert_drift_run_packet(conn, id, pkt.timestamp_ms, raw) {
                    eprintln!("[drift] opening packet insert error: {e}");
                } else {
                    run.packet_count = 1;
                }
                let status = DriftRunStatus::running(&run);
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
        let gate_a = gate_or_derived(
            &row.start_gate,
            row.left_boundary.first()?,
            row.right_boundary.first()?,
        );
        let gate_b = gate_or_derived(
            &row.finish_gate,
            row.left_boundary.last()?,
            row.right_boundary.last()?,
        );
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

fn gate_or_derived(
    gate: &[db::ZonePoint],
    left: &db::ZonePoint,
    right: &db::ZonePoint,
) -> [Point; 2] {
    if gate.len() == 2 {
        [Point::from(&gate[0]), Point::from(&gate[1])]
    } else {
        [Point::from(left), Point::from(right)]
    }
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
            .note_packet(&conn, &packet(2.0, -1.0), &raw, 1000)
            .is_none());
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1100)
            .unwrap();
        assert_eq!(started.state, "running");
        let finished = mgr
            .note_packet(&conn, &packet(2.0, 11.0), &raw, 2000)
            .unwrap();
        assert_eq!(finished.state, "completed");
        let rows = db::list_drift_runs(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].valid);
        assert_eq!(rows[0].packet_count, 2);
    }

    #[test]
    fn run_invalidates_when_leaving_geofence() {
        let conn = in_memory();
        let zone = square_zone();
        db::save_drift_zone(
            &conn,
            &db::DriftZoneInput {
                id: None,
                name: "Run".into(),
                description: None,
                active: true,
                left_boundary: zone.left_boundary,
                right_boundary: zone.right_boundary,
                start_gate: Vec::new(),
                finish_gate: Vec::new(),
                split_gates: Vec::new(),
                // Zero slack → strict geofence for this test.
                scoring_config: serde_json::json!({ "boundarySlackM": 0.0 }),
            },
            1,
        )
        .unwrap();
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000);
        mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100);
        let invalid = mgr
            .note_packet(&conn, &packet(8.0, 4.0), &raw, 1300)
            .unwrap();
        assert_eq!(invalid.state, "invalid");
        let rows = db::list_drift_runs(&conn).unwrap();
        assert!(!rows[0].valid);
        assert_eq!(
            rows[0].invalid_reason.as_deref(),
            Some("left drift zone before finish")
        );
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
    fn run_tolerates_boundary_slack() {
        // With 3 m slack, a 1 m excursion past the edge keeps the run alive; a
        // 4 m excursion still voids it.
        let conn = in_memory();
        save_square_zone(&conn, 3.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000);
        mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100);
        // x=6 is 1 m outside the x=5 edge → within slack → still running.
        let still = mgr.note_packet(&conn, &packet(6.0, 5.0), &raw, 1200).unwrap();
        assert_eq!(still.state, "running");
        // x=9 is 4 m outside → beyond slack → invalid.
        let gone = mgr.note_packet(&conn, &packet(9.0, 5.0), &raw, 1300).unwrap();
        assert_eq!(gone.state, "invalid");
    }

    #[test]
    fn run_enters_from_either_gate() {
        // Drive the zone the "reverse" way: enter through the far (z=10) gate and
        // exit through the near (z=0) gate. Bidirectional detection completes it.
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        assert!(mgr.note_packet(&conn, &packet(2.0, 11.0), &raw, 1000).is_none());
        let started = mgr.note_packet(&conn, &packet(2.0, 9.0), &raw, 1100).unwrap();
        assert_eq!(started.state, "running");
        let finished = mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 2000).unwrap();
        assert_eq!(finished.state, "completed");
        let rows = db::list_drift_runs(&conn).unwrap();
        assert!(rows[0].valid);
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
}

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
                let (score, breakdown) = score_from_packets(conn, run.id, &run.zone);
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
                let (score, breakdown) = score_from_packets(conn, run.id, &run.zone);
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
        // when the boundary is edited after the first save). Tessellation
        // preserves the endpoints, so the gates are identical curved or straight.
        let gate_a = [
            Point::from(row.left_boundary.first()?),
            Point::from(row.right_boundary.first()?),
        ];
        let gate_b = [
            Point::from(row.left_boundary.last()?),
            Point::from(row.right_boundary.last()?),
        ];
        // The entry-detection polygon follows the SAME centripetal curve the editor
        // and maps draw when the zone is smoothed (`curve == "catmull"`), so a run
        // is tested against the boundary that's on screen. Linear zones keep the
        // raw straight chords. `left ++ reversed(right)` closes the corridor ring.
        let left: Vec<Point> = row.left_boundary.iter().map(Point::from).collect();
        let right: Vec<Point> = row.right_boundary.iter().map(Point::from).collect();
        let (left, right) = if curve_is_catmull(&row.scoring_config) {
            (
                tessellate(&left, false, CURVE_DEFAULT_SEGMENTS),
                tessellate(&right, false, CURVE_DEFAULT_SEGMENTS),
            )
        } else {
            (left, right)
        };
        let mut polygon = left;
        polygon.extend(right.iter().rev().copied());
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

/// Whether a zone's boundary is a centripetal Catmull-Rom curve
/// (`scoring_config.curve == "catmull"`) rather than straight chords. Mirrors
/// `zoneCurveMode` in `src/lib/curve.ts` so display geometry equals scored.
fn curve_is_catmull(config: &serde_json::Value) -> bool {
    config.get("curve").and_then(|v| v.as_str()) == Some("catmull")
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

/// Interpolated points emitted per anchor span when tessellating a curved zone
/// shape (mirrors `DEFAULT_SEGMENTS` in `src/lib/curve.ts`).
pub const CURVE_DEFAULT_SEGMENTS: usize = 10;

/// Centripetal Catmull-Rom (alpha = 0.5) densification of a zone shape, in world
/// metres. This is the SHARED geometry contract with the frontend: it MUST stay
/// byte-for-byte identical to `src/lib/curve.ts::tessellate` so the boundary the
/// editor/maps draw is exactly the boundary the scorer tests — display == scored.
/// The `tessellate_matches_golden_and_invariants` test below pins the same golden
/// numbers asserted by `scripts/check-curve.mjs` on the JS side. Knot deltas use
/// `sqrt().sqrt()` (dist^0.5), NOT `powf(0.25)`: IEEE sqrt is correctly-rounded
/// and matches JS `Math.sqrt` to the bit, whereas `pow` is not guaranteed to.
///
/// `closed` selects a closed ring (a scoring region) vs an open polyline (a
/// road-edge boundary). Fewer than 3 anchors can't form a curve (a 2-point gate,
/// a single point), so they pass through unchanged.
pub fn tessellate(anchors: &[Point], closed: bool, segments: usize) -> Vec<Point> {
    let seg = segments.max(1);
    let n = anchors.len();
    if n < 3 {
        return anchors.to_vec();
    }
    let mut out: Vec<Point> = Vec::new();
    if closed {
        for i in 0..n {
            emit_span(
                &mut out,
                anchors[(i + n - 1) % n],
                anchors[i],
                anchors[(i + 1) % n],
                anchors[(i + 2) % n],
                seg,
            );
        }
    } else {
        // Reflect the endpoints so the curve reaches the first/last anchor with a
        // natural tangent and no zero-length end span (which would divide by zero
        // in the knot deltas).
        let start = Point {
            x: 2.0 * anchors[0].x - anchors[1].x,
            z: 2.0 * anchors[0].z - anchors[1].z,
        };
        let end = Point {
            x: 2.0 * anchors[n - 1].x - anchors[n - 2].x,
            z: 2.0 * anchors[n - 1].z - anchors[n - 2].z,
        };
        for i in 0..n - 1 {
            let p0 = if i == 0 { start } else { anchors[i - 1] };
            let p3 = if i + 2 <= n - 1 { anchors[i + 2] } else { end };
            emit_span(&mut out, p0, anchors[i], anchors[i + 1], p3, seg);
        }
        out.push(anchors[n - 1]);
    }
    out
}

/// Centripetal knot delta between two control points: dist^0.5, written as a
/// double sqrt for bit-exact parity with the JS reference.
fn knot_delta(a: Point, b: Point) -> f64 {
    let dx = b.x - a.x;
    let dz = b.z - a.z;
    (dx * dx + dz * dz).sqrt().sqrt()
}

/// Emit `seg` points along the Catmull-Rom span p1->p2 (neighbours p0, p3) via
/// the Barry-Goldman pyramid. k = 0..seg-1 (t = t1 included, t = t2 excluded);
/// t = t1 evaluates exactly to p1, so each anchor lands in the output once and the
/// curve interpolates its anchors. Mirrors `emitSpan` in curve.ts exactly.
fn emit_span(out: &mut Vec<Point>, p0: Point, p1: Point, p2: Point, p3: Point, seg: usize) {
    let t0 = 0.0;
    let t1 = t0 + knot_delta(p0, p1);
    let t2 = t1 + knot_delta(p1, p2);
    let t3 = t2 + knot_delta(p2, p3);

    // Coincident control points collapse a knot interval; fall back to a straight
    // p1->p2 chord for this span rather than dividing by zero.
    if !(t1 > t0) || !(t2 > t1) || !(t3 > t2) {
        for k in 0..seg {
            let u = k as f64 / seg as f64;
            out.push(Point {
                x: p1.x + (p2.x - p1.x) * u,
                z: p1.z + (p2.z - p1.z) * u,
            });
        }
        return;
    }

    for k in 0..seg {
        let t = t1 + (t2 - t1) * (k as f64 / seg as f64);
        let a1x = ((t1 - t) / (t1 - t0)) * p0.x + ((t - t0) / (t1 - t0)) * p1.x;
        let a1z = ((t1 - t) / (t1 - t0)) * p0.z + ((t - t0) / (t1 - t0)) * p1.z;
        let a2x = ((t2 - t) / (t2 - t1)) * p1.x + ((t - t1) / (t2 - t1)) * p2.x;
        let a2z = ((t2 - t) / (t2 - t1)) * p1.z + ((t - t1) / (t2 - t1)) * p2.z;
        let a3x = ((t3 - t) / (t3 - t2)) * p2.x + ((t - t2) / (t3 - t2)) * p3.x;
        let a3z = ((t3 - t) / (t3 - t2)) * p2.z + ((t - t2) / (t3 - t2)) * p3.z;
        let b1x = ((t2 - t) / (t2 - t0)) * a1x + ((t - t0) / (t2 - t0)) * a2x;
        let b1z = ((t2 - t) / (t2 - t0)) * a1z + ((t - t0) / (t2 - t0)) * a2z;
        let b2x = ((t3 - t) / (t3 - t1)) * a2x + ((t - t1) / (t3 - t1)) * a3x;
        let b2z = ((t3 - t) / (t3 - t1)) * a2z + ((t - t1) / (t3 - t1)) * a3z;
        out.push(Point {
            x: ((t2 - t) / (t2 - t1)) * b1x + ((t - t1) / (t2 - t1)) * b2x,
            z: ((t2 - t) / (t2 - t1)) * b1z + ((t - t1) / (t2 - t1)) * b2z,
        });
    }
}

/// Re-read a run's stored packets and score them. Runs once on close, off the
/// per-packet path; loading ~a few thousand blobs and parsing is well under a
/// frame. Returns the (computed_score, breakdown_json) to persist.
fn score_from_packets(
    conn: &Connection,
    run_id: i64,
    zone: &RunnableZone,
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
    // Detect rewinds and score the corrected (replayed stretch removed) run, so the
    // stored score matches the in-game score. The run stays flagged via `rewinds`
    // and is excluded from the fit regardless.
    let rewinds = detect_rewinds(&pkts);
    let mut result =
        score_run_corrected(&pkts, &rewinds, Some((zone.gate_a, zone.gate_b)), &zone.params);
    result.rewinds = rewinds;
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

/// The geometry a recorded run is bucketed into sectors against: the mid-run
/// split lines plus the two end gates, derived from a zone row the same way the
/// live scorer derives its gates (first/last boundary point pairs). Splits are
/// straight 2-point rungs (not tessellated) in the stored order, which the editor
/// keeps in A→B driving order, so they line up with `scoringConfig.sectorNames`.
pub struct SectorGeometry {
    pub splits: Vec<[Point; 2]>,
    pub gate_a: [Point; 2],
    pub gate_b: [Point; 2],
}

/// Build [`SectorGeometry`] from a zone row, or `None` if it can't form end gates
/// (fewer than two boundary points per side). Malformed splits (fewer than two
/// points) are dropped.
pub fn sector_geometry(row: &db::DriftZoneRow) -> Option<SectorGeometry> {
    if row.left_boundary.len() < 2 || row.right_boundary.len() < 2 {
        return None;
    }
    let gate_a = [
        Point::from(row.left_boundary.first()?),
        Point::from(row.right_boundary.first()?),
    ];
    let gate_b = [
        Point::from(row.left_boundary.last()?),
        Point::from(row.right_boundary.last()?),
    ];
    let splits = row
        .split_gates
        .iter()
        .filter(|g| g.len() >= 2)
        .map(|g| [Point::from(&g[0]), Point::from(&g[1])])
        .collect();
    Some(SectorGeometry {
        splits,
        gate_a,
        gate_b,
    })
}

/// Assign each packet a sector index by counting split-gate crossings from the
/// entry gate, **monotonic / furthest-reached**: the run only ever advances to
/// the next split in travel order and never falls back, so weaving across a
/// boundary can't bounce the count (the roadmap's preferred rule over a net
/// crossing count). Returns one index per packet (index-aligned), in
/// **A→B-canonical** order — sector `i` is the gap named by `sectorNames[i]`
/// regardless of which gate the run entered. Empty `splits` ⇒ all zeros.
///
/// Entry direction is read from the first positioned packet: runs are recorded
/// from the entry crossing, so the opening position sits at one end gate; the
/// nearer gate is the entry, which fixes whether the canonical index counts up
/// from gate A or down from gate B.
pub fn assign_sectors(
    packets: &[parser::TelemetryPacket],
    splits: &[[Point; 2]],
    gate_a: [Point; 2],
    gate_b: [Point; 2],
) -> Vec<u32> {
    let n = splits.len();
    if n == 0 || packets.is_empty() {
        return vec![0; packets.len()];
    }
    let entry_is_a = match packets.iter().find_map(packet_point) {
        Some(p) => gate_distance_sq(p, gate_a) <= gate_distance_sq(p, gate_b),
        None => true,
    };
    let mut out = Vec::with_capacity(packets.len());
    let mut prev: Option<Point> = None;
    if entry_is_a {
        // Forward (entry = gate A): cross splits 0,1,…; sector counts up from 0.
        let mut sector = 0u32;
        let mut next = 0usize;
        for pkt in packets {
            let cur = packet_point(pkt);
            if let (Some(pp), Some(c)) = (prev, cur) {
                while next < n && segment_intersects(pp, c, splits[next][0], splits[next][1]) {
                    next += 1;
                    sector += 1;
                }
            }
            out.push(sector);
            if cur.is_some() {
                prev = cur;
            }
        }
    } else {
        // Reverse (entry = gate B): cross splits N-1,…,0; sector counts down from N.
        let mut sector = n as u32;
        let mut next = n as isize - 1;
        for pkt in packets {
            let cur = packet_point(pkt);
            if let (Some(pp), Some(c)) = (prev, cur) {
                while next >= 0
                    && segment_intersects(pp, c, splits[next as usize][0], splits[next as usize][1])
                {
                    next -= 1;
                    sector -= 1;
                }
            }
            out.push(sector);
            if cur.is_some() {
                prev = cur;
            }
        }
    }
    out
}

/// Fill in the per-sector breakdown for a re-scored run: tag each [`TickScore`]
/// with its sector and roll the per-tick points / drift time up into
/// `score.sectors` (A→B order, length = splits + 1). No-op when the zone has no
/// split geometry, leaving every tick in sector 0 and `score.sectors` empty.
/// `packets`, `ticks` and the derived sectors are all index-aligned.
pub fn rescore_sectors(
    row: &db::DriftZoneRow,
    packets: &[parser::TelemetryPacket],
    ticks: &mut [scoring::TickScore],
    score: &mut scoring::RunScore,
) {
    let Some(geom) = sector_geometry(row) else {
        return;
    };
    if geom.splits.is_empty() {
        return;
    }
    let sectors = assign_sectors(packets, &geom.splits, geom.gate_a, geom.gate_b);
    let n = geom.splits.len() + 1;
    let mut roll = vec![scoring::SectorScore::default(); n];
    let mut prev_ms: Option<u32> = None;
    for ((pkt, tick), &s) in packets.iter().zip(ticks.iter()).zip(sectors.iter()) {
        let dt = match prev_ms {
            Some(prev) => scoring::frame_dt(prev, pkt.timestamp_ms),
            None => 1.0 / 60.0,
        };
        prev_ms = Some(pkt.timestamp_ms);
        let bucket = &mut roll[(s as usize).min(n - 1)];
        bucket.points += tick.points;
        bucket.sample_count += 1;
        if tick.is_drifting {
            bucket.drift_time_s += dt;
        }
    }
    for (tick, &s) in ticks.iter_mut().zip(sectors.iter()) {
        tick.sector = s;
    }
    score.sectors = roll;
}

/// A rewind stops UDP transmission, so it lands in the stored packets as a gap (ms)
/// far larger than the ~16 ms 64 Hz tick. Pauses gap too — the JUMP tells them apart.
const REWIND_GAP_MS: i64 = 400;
/// Planar jump (m) across that gap above which it's a rewind, not a pause. Pauses
/// resume in place (largest non-rewind jump DB-wide is 11.9 m); real rewinds jump
/// back 40+ m (staged runs #668/#670/#671 measured 42.8 / 109.5 / 128.6 m). 25 m
/// sits in the wide empty margin between, so detection has no false positives.
const REWIND_JUMP_M: f64 = 25.0;
/// Resume-to-nearest-recorded-point distance (3D, m) within which the rewind landed
/// back ON the path (in-zone — the game continued). Beyond it the rewind left the
/// zone (e.g. out the start gate), which the game treats as a fail/re-trigger.
/// Staged in-zone rewinds resumed at 0.0 m; the out-of-zone one at 61.1 m.
const REWIND_ON_PATH_M: f64 = 15.0;

/// World position `(x, y, z)` of a packet, or `None` for the blanked `(0,0)`
/// sentinel the live recorder drops. Mirrors [`packet_point`] but keeps height for
/// the rewind target search — full 3D so a resume on one level of an overlapping
/// (loop/spiral) zone can't match a same-`(x,z)` point on another level.
fn packet_point3(pkt: &parser::TelemetryPacket) -> Option<(f64, f64, f64)> {
    if pkt.position_x == 0.0 && pkt.position_z == 0.0 {
        return None;
    }
    Some((
        pkt.position_x as f64,
        pkt.position_y as f64,
        pkt.position_z as f64,
    ))
}

/// Detect in-game rewinds in a recorded run (see [`scoring::RewindEvent`]). A rewind
/// shows up as a telemetry gap (transmission stops) with a large BACKWARD position
/// jump on resume; the resume's nearest earlier recorded point is the rewind target.
/// Geometry-free (position track only — shares no state with the zone gates), so it
/// runs identically at close, on recompute, and on the run-viewer path. Cheap: the
/// O(n) nearest-point search runs only for the (typically ≤2) gaps that clear the
/// jump threshold, never for ordinary pauses.
pub fn detect_rewinds(packets: &[parser::TelemetryPacket]) -> Vec<scoring::RewindEvent> {
    let mut out = Vec::new();
    if packets.len() < 2 {
        return out;
    }
    let pts: Vec<Option<(f64, f64, f64)>> = packets.iter().map(packet_point3).collect();
    for i in 0..packets.len() - 1 {
        let dt = packets[i + 1].timestamp_ms as i64 - packets[i].timestamp_ms as i64;
        if dt < REWIND_GAP_MS {
            continue;
        }
        let (Some(from), Some(resume)) = (pts[i], pts[i + 1]) else {
            continue;
        };
        let jump = ((resume.0 - from.0).powi(2) + (resume.2 - from.2).powi(2)).sqrt();
        if jump < REWIND_JUMP_M {
            continue;
        }
        // Nearest earlier recorded point (full 3D) to the resume — the rewind target.
        let mut best = f64::INFINITY;
        let mut target = i;
        for (j, p) in pts.iter().enumerate().take(i + 1) {
            if let Some(p) = p {
                let d = ((p.0 - resume.0).powi(2)
                    + (p.1 - resume.1).powi(2)
                    + (p.2 - resume.2).powi(2))
                .sqrt();
                if d < best {
                    best = d;
                    target = j;
                }
            }
        }
        out.push(scoring::RewindEvent {
            gap_index: i,
            resume_index: i + 1,
            gap_ms: dt as u32,
            jump_m: jump,
            target_index: target,
            resume_path_dist_m: best,
            on_path: best <= REWIND_ON_PATH_M,
        });
    }
    out
}

/// Mark `[start, end]` (inclusive) of `mask` as abandoned. A no-op if `start > end`.
fn mark_range(mask: &mut [bool], start: usize, end_inclusive: usize) {
    for m in mask.iter_mut().take(end_inclusive + 1).skip(start) {
        *m = true;
    }
}

/// First packet index after `resume` whose arrival crosses an end gate — the zone
/// re-trigger after a rewind left the zone. `None` if the run never re-enters.
fn reentry_index(
    packets: &[parser::TelemetryPacket],
    resume: usize,
    gate_a: [Point; 2],
    gate_b: [Point; 2],
) -> Option<usize> {
    let mut prev = packet_point(packets.get(resume)?);
    for j in (resume + 1)..packets.len() {
        let cur = packet_point(&packets[j]);
        if let (Some(p), Some(c)) = (prev, cur) {
            if segment_intersects(p, c, gate_a[0], gate_a[1])
                || segment_intersects(p, c, gate_b[0], gate_b[1])
            {
                return Some(j);
            }
        }
        if cur.is_some() {
            prev = cur;
        }
    }
    None
}

/// Per-packet "abandoned" mask for a rewound run — the replayed stretch the game
/// discarded but the raw integral double-counts (`true` = drop from the corrected
/// score). An IN-ZONE rewind drops the replayed forward attempt `(target, gap]`; a
/// LEFT-ZONE rewind (rewound out of the zone → the game failed + re-triggered)
/// drops everything up to the re-entry gate crossing after the resume. The
/// left-zone case needs the end gates; without them (or if no re-entry is found) it
/// falls back to the in-zone rule. Multiple rewinds union.
pub fn rewind_abandoned_mask(
    packets: &[parser::TelemetryPacket],
    rewinds: &[scoring::RewindEvent],
    gates: Option<([Point; 2], [Point; 2])>,
) -> Vec<bool> {
    let mut mask = vec![false; packets.len()];
    if packets.is_empty() {
        return mask;
    }
    let last = packets.len() - 1;
    for rw in rewinds {
        let forward = (rw.target_index + 1, rw.gap_index.min(last));
        let (start, end) = if rw.on_path {
            forward
        } else if let Some((ga, gb)) = gates {
            match reentry_index(packets, rw.resume_index, ga, gb) {
                Some(j) => (0, j.saturating_sub(1)),
                None => forward,
            }
        } else {
            forward
        };
        mark_range(&mut mask, start, end);
    }
    mask
}

/// Score a run with any rewinds corrected: integrate only over the packets the game
/// kept (abandoned stretches removed). Identical to [`scoring::score_run`] when
/// `rewinds` is empty. The score paths detect rewinds and pass them here so the
/// stored/displayed score matches the in-game score; the run stays flagged and is
/// excluded from the fit regardless (correction is display-only).
pub fn score_run_corrected(
    packets: &[parser::TelemetryPacket],
    rewinds: &[scoring::RewindEvent],
    gates: Option<([Point; 2], [Point; 2])>,
    params: &scoring::ScoringParams,
) -> scoring::RunScore {
    if rewinds.is_empty() {
        return scoring::score_run(packets, params);
    }
    let mask = rewind_abandoned_mask(packets, rewinds, gates);
    let kept: Vec<parser::TelemetryPacket> = packets
        .iter()
        .zip(&mask)
        .filter(|(_, abandoned)| !**abandoned)
        .map(|(p, _)| p.clone())
        .collect();
    scoring::score_run(&kept, params)
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

    /// A positioned packet at world (x, y, z) with an explicit game timestamp (ms),
    /// for driving the rewind detector (which reads only position + timestamp).
    fn pos_packet(x: f32, y: f32, z: f32, ms: u32) -> parser::TelemetryPacket {
        let mut p = packet(x, z);
        p.position_y = y;
        p.timestamp_ms = ms;
        p
    }

    // Forward drive: 11 packets 0→100 m along z at the 64 Hz tick (~16 ms apart).
    fn forward_drive() -> Vec<parser::TelemetryPacket> {
        (0..=10)
            .map(|i| pos_packet(0.0, 0.0, i as f32 * 10.0, i as u32 * 16))
            .collect()
    }

    #[test]
    fn detect_rewinds_flags_backward_jump_across_gap() {
        // After the drive, a 4 s gap (transmission stops) resumes back on the z=30 m
        // point (index 3) — the in-zone rewind signature.
        let mut pkts = forward_drive();
        pkts.push(pos_packet(0.0, 0.0, 30.0, 10 * 16 + 4000));
        let r = detect_rewinds(&pkts);
        assert_eq!(r.len(), 1);
        let e = &r[0];
        assert_eq!(e.gap_index, 10);
        assert_eq!(e.resume_index, 11);
        assert_eq!(e.target_index, 3, "resume matches the z=30 m point");
        assert_eq!(e.gap_ms, 4000);
        assert!((e.jump_m - 70.0).abs() < 1e-6, "100→30 m backward jump");
        assert!(e.on_path, "resume on an earlier point => in-zone rewind");
        assert!(e.resume_path_dist_m < 1e-6);
    }

    #[test]
    fn detect_rewinds_ignores_pause_in_place() {
        // A gap of the same length, but the car resumes where it stopped (a pause /
        // alt-tab): no backward jump, so it's not a rewind.
        let mut pkts = forward_drive();
        pkts.push(pos_packet(0.0, 0.0, 100.0, 10 * 16 + 4000));
        assert!(detect_rewinds(&pkts).is_empty());
    }

    #[test]
    fn detect_rewinds_ignores_clean_run() {
        // Continuous 64 Hz streaming at ~32 m/s — no gap, so nothing to flag.
        let pkts: Vec<_> = (0..200)
            .map(|i| pos_packet(0.0, 0.0, i as f32 * 0.5, i as u32 * 16))
            .collect();
        assert!(detect_rewinds(&pkts).is_empty());
    }

    #[test]
    fn detect_rewinds_classifies_offpath_resume() {
        // Rewind out of the recorded path (like #671 out the start gate): the resume
        // is far from every earlier point, so it's flagged but marked off-path.
        let mut pkts = forward_drive();
        pkts.push(pos_packet(500.0, 0.0, 500.0, 10 * 16 + 4000));
        let r = detect_rewinds(&pkts);
        assert_eq!(r.len(), 1);
        assert!(!r[0].on_path, "resume far from the path => left the zone");
        assert!(r[0].resume_path_dist_m > REWIND_ON_PATH_M);
    }

    #[test]
    fn rewind_mask_inzone_marks_target_to_gap() {
        let pkts = vec![packet(1.0, 1.0); 12];
        let rw = scoring::RewindEvent {
            gap_index: 8,
            resume_index: 9,
            gap_ms: 4000,
            jump_m: 50.0,
            target_index: 3,
            resume_path_dist_m: 0.0,
            on_path: true,
        };
        let mask = rewind_abandoned_mask(&pkts, &[rw], None);
        for (i, &m) in mask.iter().enumerate() {
            assert_eq!(m, (4..=8).contains(&i), "idx {i}");
        }
    }

    #[test]
    fn rewind_correction_drops_inzone_replayed_stretch() {
        let params = scoring::ScoringParams::default();
        // Forward drift: z = 5,15,…,95 (10 scoring pkts, idx 0..9), ~64 Hz.
        let mut pkts: Vec<_> = (0..10)
            .map(|i| drifting_packet(2.5, 5.0 + i as f32 * 10.0, i as u32 * 16))
            .collect();
        // Rewind: 4 s gap, resume back on the z=35 m point (idx 3); then re-drive.
        pkts.push(drifting_packet(2.5, 35.0, 9 * 16 + 4000));
        for k in 1..=4 {
            pkts.push(drifting_packet(2.5, 35.0 + k as f32 * 10.0, 9 * 16 + 4000 + k as u32 * 16));
        }
        let rewinds = detect_rewinds(&pkts);
        assert_eq!(rewinds.len(), 1);
        assert!(rewinds[0].on_path && rewinds[0].target_index == 3 && rewinds[0].gap_index == 9);

        let raw = scoring::score_run(&pkts, &params).score;
        let corrected = score_run_corrected(&pkts, &rewinds, Some((GATE_A, GATE_B)), &params).score;
        assert!(corrected < raw, "corrected {corrected} should be < raw {raw}");
        // Equals scoring only the kept packets (drop the replayed (target,gap] = 4..=9).
        let kept: Vec<_> = pkts
            .iter()
            .enumerate()
            .filter(|(i, _)| !(4..=9).contains(i))
            .map(|(_, p)| p.clone())
            .collect();
        let expect = scoring::score_run(&kept, &params).score;
        assert!((corrected - expect).abs() < 1e-6, "corrected {corrected} vs kept {expect}");
    }

    #[test]
    fn rewind_correction_leftzone_drops_failed_attempt() {
        let params = scoring::ScoringParams::default();
        // First attempt inside the zone (scoring): z = 5,15,…,95 at x=2.5 (idx 0..9).
        let mut pkts: Vec<_> = (0..10)
            .map(|i| drifting_packet(2.5, 5.0 + i as f32 * 10.0, i as u32 * 16))
            .collect();
        // Rewind OUT the start gate (z=0 line) to z=-50 (idx 10), 5 s gap.
        pkts.push(drifting_packet(2.5, -50.0, 9 * 16 + 5000));
        // Drive back, re-crossing gate A between z=-10 and z=+10 → re-trigger, then drift.
        let base = 9 * 16 + 5000;
        for (k, z) in [-30.0_f32, -10.0, 10.0, 20.0, 30.0].iter().enumerate() {
            pkts.push(drifting_packet(2.5, *z, base + (k as u32 + 1) * 16));
        }
        let rewinds = detect_rewinds(&pkts);
        assert_eq!(rewinds.len(), 1);
        assert!(!rewinds[0].on_path, "rewound out of the zone");

        // Re-entry crosses gate A at idx 13; everything before it is the failed attempt.
        let mask = rewind_abandoned_mask(&pkts, &rewinds, Some((GATE_A, GATE_B)));
        assert!(mask[..13].iter().all(|&m| m), "first attempt + drive-back abandoned");
        assert!(mask[13..].iter().all(|&m| !m), "the re-drive is kept");

        let raw = scoring::score_run(&pkts, &params).score;
        let corrected = score_run_corrected(&pkts, &rewinds, Some((GATE_A, GATE_B)), &params).score;
        assert!(corrected < raw && corrected > 0.0, "corrected {corrected} raw {raw}");
    }

    fn in_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn
    }

    // A split line spanning the square corridor (x 0→5) at height `z`.
    fn split_z(z: f64) -> [Point; 2] {
        [Point { x: 0.0, z }, Point { x: 5.0, z }]
    }
    // square_zone()'s derived end gates: A at z=0, B at z=10.
    const GATE_A: [Point; 2] = [Point { x: 0.0, z: 0.0 }, Point { x: 5.0, z: 0.0 }];
    const GATE_B: [Point; 2] = [Point { x: 0.0, z: 10.0 }, Point { x: 5.0, z: 10.0 }];

    #[test]
    fn sectors_count_up_from_gate_a() {
        let splits = vec![split_z(3.33), split_z(6.66)];
        let pkts: Vec<_> = [0.5, 2.0, 3.0, 4.0, 5.0, 7.0, 9.0]
            .iter()
            .map(|&z| packet(2.5, z))
            .collect();
        // Enters by gate A (near z=0): sector counts up as each split is crossed.
        assert_eq!(
            assign_sectors(&pkts, &splits, GATE_A, GATE_B),
            vec![0, 0, 0, 1, 1, 2, 2]
        );
    }

    #[test]
    fn sectors_count_down_from_gate_b() {
        let splits = vec![split_z(3.33), split_z(6.66)];
        let pkts: Vec<_> = [9.5, 7.0, 6.0, 4.0, 3.0, 1.0]
            .iter()
            .map(|&z| packet(2.5, z))
            .collect();
        // Reverse entry (near z=10): canonical index counts down from N, so the
        // same physical sector still maps to the same sectorNames slot.
        assert_eq!(
            assign_sectors(&pkts, &splits, GATE_A, GATE_B),
            vec![2, 2, 1, 1, 0, 0]
        );
    }

    #[test]
    fn sectors_are_monotonic_through_wobble() {
        let splits = vec![split_z(3.33)];
        let pkts: Vec<_> = [3.0, 4.0, 3.0, 4.0]
            .iter()
            .map(|&z| packet(2.5, z))
            .collect();
        // Cross once → sector 1; weaving back across the line never falls back.
        assert_eq!(assign_sectors(&pkts, &splits, GATE_A, GATE_B), vec![0, 1, 1, 1]);
    }

    #[test]
    fn no_splits_leaves_everything_in_sector_zero() {
        let pkts: Vec<_> = [1.0, 2.0, 3.0].iter().map(|&z| packet(2.5, z)).collect();
        assert_eq!(assign_sectors(&pkts, &[], GATE_A, GATE_B), vec![0, 0, 0]);
    }

    #[test]
    fn rescore_sectors_buckets_points_and_sums_to_run_score() {
        let mut row = square_zone();
        row.split_gates = vec![
            vec![
                db::ZonePoint { x: 0.0, z: 3.33 },
                db::ZonePoint { x: 5.0, z: 3.33 },
            ],
            vec![
                db::ZonePoint { x: 0.0, z: 6.66 },
                db::ZonePoint { x: 5.0, z: 6.66 },
            ],
        ];
        // A scoring run driving A→B through both splits.
        let pkts: Vec<_> = (0..12)
            .map(|i| drifting_packet(2.5, 0.8 * i as f32 + 0.5, 100 + i as u32 * 16))
            .collect();
        let params = scoring::ScoringParams::default();
        let (mut score, mut ticks) = scoring::score_run_with_ticks(&pkts, &params);
        rescore_sectors(&row, &pkts, &mut ticks, &mut score);

        assert_eq!(score.sectors.len(), 3, "N splits ⇒ N+1 sectors");
        assert_eq!(ticks.len(), pkts.len());
        // Forward entry ⇒ per-tick sector is non-decreasing.
        assert!(ticks.windows(2).all(|w| w[0].sector <= w[1].sector));
        // Per-sector points reconstruct the run total (no points lost or double-counted).
        let summed: f64 = score.sectors.iter().map(|s| s.points).sum();
        assert!(
            (summed - score.score).abs() < 1e-3,
            "sector sum {summed} vs run score {}",
            score.score
        );
        assert!(score.score > 0.0, "the run should bank points");
    }

    #[test]
    fn point_in_zone_polygon() {
        let zone = RunnableZone::from_row(&square_zone()).unwrap();
        assert!(point_in_polygon(Point { x: 2.5, z: 5.0 }, &zone.polygon));
        assert!(!point_in_polygon(Point { x: 8.0, z: 5.0 }, &zone.polygon));
    }

    #[test]
    fn tessellate_matches_golden_and_invariants() {
        // Goldens are duplicated verbatim in scripts/check-curve.mjs; identical
        // numbers on both sides + identical arithmetic == display equals scored.
        let near = |a: f64, b: f64| (a - b).abs() <= 1e-9;
        let pt = |x: f64, z: f64| Point { x, z };

        // open L-corner, seg = 4 → (n-1)*seg + 1 = 9 points, interpolates anchors
        let l = vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0)];
        let a = tessellate(&l, false, 4);
        assert_eq!(a.len(), 9);
        assert!(near(a[0].x, 0.0) && near(a[0].z, 0.0));
        assert!(near(a[4].x, 10.0) && near(a[4].z, 0.0), "anchor 1 at index seg");
        assert!(near(a[8].x, 10.0) && near(a[8].z, 10.0), "last anchor");
        assert!(near(a[2].x, 5.625) && near(a[2].z, -0.625), "a[2] = {:?}", a[2]);
        assert!(a.iter().all(|p| p.x.is_finite() && p.z.is_finite()));

        // closed square, seg = 4 → n*seg = 16 points, anchors at every i*seg
        let sq = vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0)];
        let b = tessellate(&sq, true, 4);
        assert_eq!(b.len(), 16);
        assert!(near(b[0].x, 0.0) && near(b[0].z, 0.0));
        assert!(near(b[4].x, 10.0) && near(b[4].z, 0.0));
        assert!(near(b[8].x, 10.0) && near(b[8].z, 10.0));
        assert!(near(b[12].x, 0.0) && near(b[12].z, 10.0));
        assert!(near(b[2].x, 5.0) && near(b[2].z, -1.25), "b[2] = {:?}", b[2]);

        // unevenly-spaced open: the irrational sqrt-path parity lock
        let un = vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(12.0, 8.0)];
        let u = tessellate(&un, false, 4);
        assert_eq!(u.len(), 9);
        assert!(
            near(u[2].x, 5.510823710739302) && near(u[2].z, -0.5771314068283177),
            "u[2] = {:?}",
            u[2]
        );
        assert!(
            near(u[6].x, 11.421236023089621) && near(u[6].z, 3.524085249957107),
            "u[6] = {:?}",
            u[6]
        );

        // collinear anchors stay on the line (affine blends of collinear points)
        let col = vec![pt(0.0, 0.0), pt(5.0, 0.0), pt(10.0, 0.0), pt(20.0, 0.0)];
        for p in tessellate(&col, false, 3) {
            assert!(near(p.z, 0.0), "collinear drifted off-axis: {:?}", p);
        }

        // fewer than 3 anchors → passthrough copy (gates / single points)
        let two = vec![pt(1.0, 2.0), pt(3.0, 4.0)];
        assert_eq!(tessellate(&two, false, 4), two);
    }

    #[test]
    fn curved_zone_tessellates_entry_polygon_yet_preserves_gates() {
        // 3-point boundaries per side so tessellation actually engages (<3 = no-op).
        let mut row = square_zone();
        row.left_boundary = vec![
            db::ZonePoint { x: 0.0, z: 0.0 },
            db::ZonePoint { x: 0.0, z: 5.0 },
            db::ZonePoint { x: 0.0, z: 10.0 },
        ];
        row.right_boundary = vec![
            db::ZonePoint { x: 6.0, z: 0.0 },
            db::ZonePoint { x: 5.0, z: 5.0 },
            db::ZonePoint { x: 6.0, z: 10.0 },
        ];

        // Linear (no curve flag): the polygon is just the 6 raw points.
        let linear = RunnableZone::from_row(&row).unwrap();
        assert_eq!(linear.polygon.len(), 6);

        // Curved: each side densifies to (3-1)*seg + 1 points, both sides joined.
        row.scoring_config = serde_json::json!({ "curve": "catmull" });
        let curved = RunnableZone::from_row(&row).unwrap();
        assert_eq!(curved.polygon.len(), 2 * (2 * CURVE_DEFAULT_SEGMENTS + 1));
        assert!(curved.polygon.len() > linear.polygon.len());

        // Gates are the raw endpoints in BOTH cases — tessellation preserves them,
        // so smoothing never moves where a run starts/finishes.
        assert_eq!(curved.gate_a, linear.gate_a);
        assert_eq!(curved.gate_b, linear.gate_b);
        let near = |a: Point, x: f64, z: f64| (a.x - x).abs() < 1e-9 && (a.z - z).abs() < 1e-9;
        assert!(near(curved.gate_a[0], 0.0, 0.0) && near(curved.gate_a[1], 6.0, 0.0));
        assert!(near(curved.gate_b[0], 0.0, 10.0) && near(curved.gate_b[1], 6.0, 10.0));
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

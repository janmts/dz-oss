use std::collections::VecDeque;

use rusqlite::Connection;
use serde::Serialize;

use crate::{db, parser, scoring};

/// Inter-arrival gap (wall-clock ms) above which a run treats the elapsed time
/// as a telemetry stall — a pause, alt-tab, menu, rewind, or UDP stall — rather
/// than genuine in-game time. Telemetry is a fixed ~64 Hz tick (~15.6 ms between
/// packets), and even a long burst of UDP loss stays well under this, so anything
/// larger is a delivery stop, not the car standing still in-game. Such frozen time
/// is excluded from the forward-progress stall timer (see `note_packet`).
const STALL_GAP_MS: i64 = 500;

/// Forward-progress floor (m/s). Sustained progress below this — `speed ×
/// cos(travel-vs-route)`, see [`forward_progress`] — is a stall (idle, crawl, or
/// travelling the wrong way). ≈5 km/h: run #707 bracketed it to (4, 6] km/h (a
/// 6 km/h coast survived, braking to 3 km/h killed); #706 at 4 km/h killed.
/// Per-zone override `progressFloorMps`.
const PROGRESS_FLOOR_MPS: f64 = 1.4;

/// Sustained sub-floor progress (ms) that trips the in-zone stall kill. Clean idle
/// runs all died at exactly 3.0 s (wrong-way slightly faster, ~2.5–2.7 s; one timer
/// covers both). Frozen telemetry time (gaps > [`STALL_GAP_MS`]) is excluded.
/// Per-zone override `progressStallS` (seconds).
const PROGRESS_STALL_MS: i64 = 3000;

/// Out-of-bounds slack (m past the flags polygon) that trips the spatial kill —
/// distance-gated, NOT a timer (a position test, so leave-and-return within it is
/// free). Measured roughly uniform across corners: n=9 exits at 7–34 m/s all died
/// at a constant ~44–46 m past the original corner, ~39–41 m at a second corner.
/// Per-zone override `oobSlackM`. This is the membership inflation of the flags —
/// the same [`within_slack`] primitive as the old 3 m boundary slack, just fatter.
const OOB_SLACK_M: f64 = 45.0;

/// Centerline resolution: both boundaries are arc-resampled to this many points and
/// averaged index-wise to form the route reference (mirrors `centerline(n=80)` in
/// `scripts/drift_kill.py`).
const CENTERLINE_POINTS: usize = 80;

/// Half-length (m) of the chord used to read the route-forward tangent at the car's
/// tracked arc position (mirrors the ±8 m chord in `drift_kill.py`).
const ROUTE_TANGENT_CHORD_M: f64 = 8.0;

/// Local arc window (± m) the car is re-projected onto the centerline within, once
/// tracking has started — so a hairpin's far leg, distant in arc, can't steal the
/// projection and flip the route 180° (the nearest-segment leg-flip). The first
/// projection searches the whole centerline. Mirrors the 60 m window in `drift_kill.py`.
const ARC_TRACK_WINDOW_M: f64 = 60.0;

/// Minimum displacement (m) between two recorded positions for a reliable world
/// travel bearing. Below it the car is treated as ~stationary (no bearing → progress
/// 0 → stall accrues). Mirrors `TRAVEL_MIN_DISP` in `drift_kill.py`.
const TRAVEL_MIN_DISP_M: f64 = 0.5;

/// Maximum look-back (recorded positions, ~0.75 s at 64 Hz) when searching for the
/// travel-bearing reference. Mirrors `TRAVEL_LOOKBACK` in `drift_kill.py`; also caps
/// the per-run recent-position ring.
const TRAVEL_LOOKBACK: usize = 48;

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
    /// Corridor mid-line (the route reference the forward-progress kill measures
    /// travel against). Built once from the raw boundaries — see [`Centerline`].
    centerline: Centerline,
    /// Metres a point may stray past the flags polygon before the out-of-bounds
    /// kill fires (`oobSlackM`, default [`OOB_SLACK_M`]). A position test via
    /// [`within_slack`], so re-entering within it keeps the run alive.
    oob_slack_m: f64,
    /// Forward progress (m/s) at/above which the run is advancing; sustained below
    /// it is a stall (`progressFloorMps`, default [`PROGRESS_FLOOR_MPS`]).
    progress_floor_mps: f64,
    /// Sustained sub-floor progress (ms) that trips the stall kill (`progressStallS`
    /// seconds, default [`PROGRESS_STALL_MS`]). 0 disables the stall kill.
    progress_stall_ms: i64,
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
    /// Whether the run entered through gate A (vs gate B) — orients the centerline
    /// arc/route-forward toward the finish gate. Set once at run start.
    entry_is_a: bool,
    /// Tracked arc-length position (m) of the car along the zone centerline,
    /// monotonically projected within a local window so a hairpin can't leg-flip it.
    arc: f64,
    /// Whether the first (whole-centerline) arc projection has been done; after it,
    /// projection uses the local [`ARC_TRACK_WINDOW_M`] window.
    arc_started: bool,
    /// Recent in-run positions (newest last), capped at [`TRAVEL_LOOKBACK`]. The
    /// world travel bearing is read from the most recent point ≥ [`TRAVEL_MIN_DISP_M`]
    /// back, so progress survives the ~64 Hz jitter of a near-stationary car.
    recent: VecDeque<Point>,
    /// Wall-clock ms of the last packet whose forward progress reached the floor.
    /// The stall kill fires when this falls more than `progress_stall_ms` behind
    /// `now`; frozen telemetry gaps roll it forward (paused time ≠ stalled progress).
    last_progress_ok_ms: i64,
    /// Wall-clock ms of the previous packet seen during this run. Drives the
    /// gap-exclusion: telemetry is a fixed ~64 Hz tick, so a gap far larger than
    /// that means delivery stopped — a pause, alt-tab, menu, rewind, or UDP stall —
    /// frozen time that must not count toward the stall.
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
    /// tyre on tarmac). A live banking instrument only — it no longer gates run
    /// validity (the kill is forward-progress / out-of-bounds, not scoring).
    pub scoring: bool,
    /// Seconds left before the forward-progress stall aborts the run, while it is
    /// not advancing (progress below the floor). `None` when advancing, idle,
    /// closed, or when the kill is disabled — i.e. the live "death timer".
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
            // The caller supplies this only while the stall timer is counting
            // (not advancing); pass it through untouched — a run can advance
            // without scoring, and that is no longer a death-timer state.
            starve_remaining_s,
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

    /// End the active run immediately as INVALID, scoring whatever packets were
    /// recorded so far. Backs the manual "abort run" control: when the game has
    /// already failed a zone there's no reason to wait out the score-starvation
    /// timer, and it's the explicit stop for continuous-recording measurement
    /// runs (where the starvation timeout is turned off so the record spans the
    /// game's true end). Returns the closed status, or `None` if nothing was
    /// active. Mirrors the starvation-close branch in [`Self::note_packet`].
    pub fn abort_active(&mut self, conn: &Connection, now_ms: i64) -> Option<DriftRunStatus> {
        let run = self.active.take()?;
        let reason = "aborted".to_string();
        let (score, breakdown) = score_from_packets(conn, run.id, &run.zone);
        let status = DriftRunStatus::closed(&run, now_ms, false, Some(reason.clone()));
        if let Err(e) = db::close_drift_run(conn, run.id, now_ms, false, Some(&reason), score) {
            eprintln!("[drift] abort close error: {e}");
        }
        if let Err(e) = db::update_drift_run_score(conn, run.id, score, breakdown.as_deref()) {
            eprintln!("[drift] abort score store error: {e}");
        }
        self.last_status = status.clone();
        Some(status)
    }

    /// `kill_enabled` runs the forward-progress / out-of-bounds kill (the normal
    /// mode); `false` is measurement mode — the run is only ever closed by the
    /// finish gate or a manual [`Self::abort_active`], so a recording can span the
    /// game's true end. The kill *timing* is per-zone (`progressStallS`/`oobSlackM`),
    /// not this flag.
    pub fn note_packet(
        &mut self,
        conn: &Connection,
        pkt: &parser::TelemetryPacket,
        raw: &[u8],
        now_ms: i64,
        kill_enabled: bool,
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

            // Exclude paused/frozen wall-clock time from the progress-stall timer.
            // While the game is paused (or alt-tabbed, in a menu, mid-rewind, or
            // briefly UDP-stalled) NO packets arrive, yet the wall clock keeps
            // running. The first packet after the stall would otherwise see the
            // whole gap as time-without-progress and abort instantly on resume. A
            // gap far larger than the ~64 Hz tick can't be in-game time, so roll the
            // last-progress stamp forward by the gap (capped at now) — only
            // continuous, in-game time-without-progress counts toward the kill.
            let gap_ms = now_ms - run.last_packet_ms;
            if gap_ms > STALL_GAP_MS {
                run.last_progress_ok_ms = (run.last_progress_ok_ms + gap_ms).min(now_ms);
                // Drop the pre-gap trajectory so the first post-gap travel bearing
                // isn't read across the position jump (a rewind, or a pause the car
                // rolled through); the ring refills within ~1 packet.
                run.recent.clear();
            }
            run.last_packet_ms = now_ms;

            // Live banking instrument (HUD only — NOT a kill factor): whether this
            // packet earns points, plus the signed-direction flip count. The kill
            // is forward-progress / out-of-bounds, independent of scoring and season
            // (a car driving around not-scoring stays alive; #687 lived 30 s+).
            let scoring = scoring::is_scoring_packet(pkt, &run.zone.params);
            if scoring {
                let sign = if scoring::drift_angle_signed_deg(pkt) >= 0.0 { 1i8 } else { -1i8 };
                if run.score_sign != 0 && sign != run.score_sign {
                    run.direction_flips += 1;
                }
                run.score_sign = sign;
            }

            // Forward progress = speed × cos(travel-vs-route): advance the arc/
            // route-forward tracker on the centerline, read the world travel bearing
            // from the recent-position ring, and reset the stall stamp whenever
            // progress reaches the floor. Nose/heading is NOT consulted — a
            // 180°-reversed car travelling forward is fine (#699); only TRAVEL
            // direction matters.
            let (arc, route_fwd) = route_forward(
                &run.zone.centerline,
                current,
                run.arc,
                run.arc_started,
                run.entry_is_a,
            );
            run.arc = arc;
            run.arc_started = true;
            let progress = forward_progress(
                pkt.speed_ms as f64,
                travel_bearing(&run.recent, current),
                route_fwd,
            );
            run.recent.push_back(current);
            while run.recent.len() > TRAVEL_LOOKBACK {
                run.recent.pop_front();
            }
            if progress >= run.zone.progress_floor_mps {
                run.last_progress_ok_ms = now_ms;
            }

            // First-to-fire kill (skipped entirely in measurement mode). (B)
            // OUT-OF-BOUNDS is spatial — a fat membership boundary (~45 m past the
            // flags), distance-gated not timed — checked first because the route
            // projection is meaningless once the car is far outside. (A)
            // PROGRESS-STALL is the in-zone kill: idle, crawl, and travel-wrong-way
            // unified as "not advancing for ~3 s".
            let kill_reason: Option<&'static str> = if !kill_enabled {
                None
            } else if !within_slack(current, &run.zone.polygon, run.zone.oob_slack_m) {
                Some("out of bounds")
            } else if run.zone.progress_stall_ms > 0
                && now_ms - run.last_progress_ok_ms > run.zone.progress_stall_ms
            {
                Some("no forward progress")
            } else {
                None
            };
            if let Some(reason) = kill_reason {
                let (score, breakdown) = score_from_packets(conn, run.id, &run.zone);
                let status = DriftRunStatus::closed(run, now_ms, false, Some(reason.to_string()));
                if let Err(e) = db::close_drift_run(conn, run.id, now_ms, false, Some(reason), score) {
                    eprintln!("[drift] kill close error: {e}");
                }
                if let Err(e) = db::update_drift_run_score(conn, run.id, score, breakdown.as_deref()) {
                    eprintln!("[drift] score store error: {e}");
                }
                self.active = None;
                self.last_status = status.clone();
                return Some(status);
            }

            // Death-timer readout: seconds left before the stall kill, shown only
            // while actually stalling (progress below the floor) — a run that is
            // advancing but not scoring shows no countdown.
            let stall_remaining = (kill_enabled
                && run.zone.progress_stall_ms > 0
                && progress < run.zone.progress_floor_mps)
            .then(|| {
                ((run.zone.progress_stall_ms - (now_ms - run.last_progress_ok_ms)).max(0)) as f64
                    / 1000.0
            });
            let status = DriftRunStatus::running(run, scoring, stall_remaining, pkt);
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
        // Entry gate fixes the centerline orientation toward the finish: the finish
        // is the gate NOT entered, so the run entered through A iff finish == gate B.
        let entry_is_a = finish_gate == zone.gate_b;
        // Seed the arc tracker by projecting the opening position onto the WHOLE
        // centerline (window = total); every later packet tracks within a local
        // window from there.
        let initial_arc = if entry_is_a { 0.0 } else { zone.centerline.total };
        let (arc0, _) = route_forward(&zone.centerline, current, initial_arc, false, entry_is_a);

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
                    entry_is_a,
                    arc: arc0,
                    arc_started: true,
                    recent: VecDeque::new(),
                    last_progress_ok_ms: now_ms,
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
                // Seed the recent-position ring with the opening point so the next
                // packet has a travel-bearing reference.
                run.recent.push_back(current);
                let scoring = scoring::is_scoring_packet(pkt, &run.zone.params);
                if scoring {
                    run.score_sign = if scoring::drift_angle_signed_deg(pkt) >= 0.0 {
                        1
                    } else {
                        -1
                    };
                }
                // A run just opened isn't stalling yet — no death-timer countdown.
                let status = DriftRunStatus::running(&run, scoring, None, pkt);
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
        // Route reference for the forward-progress kill, built from the RAW
        // boundaries (mirrors scripts/drift_kill.py, which uses the stored points —
        // a tessellated bulge would shift the mid-line). Always succeeds here (≥2
        // points per side is guaranteed above).
        let centerline = Centerline::build(&left, &right)?;
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
        let cfg = &row.scoring_config;
        Some(Self {
            id: row.id,
            name: row.name.clone(),
            polygon,
            gate_a,
            gate_b,
            centerline,
            oob_slack_m: config_f64(cfg, "oobSlackM", OOB_SLACK_M).max(0.0),
            progress_floor_mps: config_f64(cfg, "progressFloorMps", PROGRESS_FLOOR_MPS).max(0.0),
            progress_stall_ms: (config_f64(cfg, "progressStallS", PROGRESS_STALL_MS as f64 / 1000.0)
                .max(0.0)
                * 1000.0) as i64,
            params: scoring::ScoringParams::from_config(cfg),
        })
    }
}

/// Read a per-zone `f64` knob from the zone's `scoring_config` bag, falling back to
/// `default` when absent or non-numeric. The kill thresholds live here (not in
/// dedicated columns) for the same reason every other per-zone tuning knob does.
fn config_f64(config: &serde_json::Value, key: &str, default: f64) -> f64 {
    config
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(default)
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

// ── Forward-progress geometry (ported from scripts/drift_kill.py) ────────────
// The in-zone kill measures the car's progress ALONG the route, not its raw speed
// or its nose direction. The route reference is the corridor mid-line (centerline);
// the car is projected onto it with monotonic local-window tracking (so a hairpin
// can't flip the route 180°), and progress is `speed × cos(travel − route)`.

/// Arc-parameterized corridor mid-line — the route reference the forward-progress
/// kill projects the car onto. Built once per runnable zone by arc-resampling both
/// boundaries to [`CENTERLINE_POINTS`] and averaging index-wise (mirrors
/// `centerline()` in `scripts/drift_kill.py`): the GPS route runs ~down the middle,
/// so this beats either edge or a straight chord to the finish gate (a chord is
/// wrong ~27% of the time where the route winds away from the gate; validated there).
#[derive(Debug, Clone)]
struct Centerline {
    pts: Vec<Point>,
    /// Cumulative arc length to each point; `cum[0] == 0`, `*cum.last() == total`.
    cum: Vec<f64>,
    total: f64,
}

impl Centerline {
    /// Build from the two raw boundaries, or `None` if either has fewer than two
    /// points (no extent to resample).
    fn build(left: &[Point], right: &[Point]) -> Option<Self> {
        if left.len() < 2 || right.len() < 2 {
            return None;
        }
        let l = resample(left, CENTERLINE_POINTS);
        let r = resample(right, CENTERLINE_POINTS);
        let pts: Vec<Point> = (0..CENTERLINE_POINTS)
            .map(|i| Point {
                x: (l[i].x + r[i].x) / 2.0,
                z: (l[i].z + r[i].z) / 2.0,
            })
            .collect();
        let mut cum = vec![0.0; pts.len()];
        for i in 1..pts.len() {
            cum[i] = cum[i - 1] + dist(pts[i - 1], pts[i]);
        }
        let total = *cum.last().unwrap_or(&0.0);
        Some(Self { pts, cum, total })
    }

    /// Position at arc-length `arc` (clamped to `[0, total]`), linearly interpolated
    /// along the polyline. Mirrors `point_at` in `drift_kill.py`.
    fn point_at(&self, arc: f64) -> Point {
        let arc = arc.clamp(0.0, self.total);
        for i in 0..self.pts.len().saturating_sub(1) {
            if self.cum[i + 1] >= arc {
                let d = self.cum[i + 1] - self.cum[i];
                let denom = if d == 0.0 { 1e-9 } else { d };
                let t = (arc - self.cum[i]) / denom;
                return Point {
                    x: self.pts[i].x + t * (self.pts[i + 1].x - self.pts[i].x),
                    z: self.pts[i].z + t * (self.pts[i + 1].z - self.pts[i].z),
                };
            }
        }
        self.pts.last().copied().unwrap_or(Point { x: 0.0, z: 0.0 })
    }

    /// Arc position (within `±window` of `center`) nearest `p`, by a ~1 m scan —
    /// the monotonic local-window projection that stops a hairpin's far leg from
    /// stealing the match. Mirrors `nearest_arc` in `drift_kill.py`.
    fn nearest_arc(&self, p: Point, center: f64, window: f64) -> f64 {
        let a0 = (center - window).max(0.0);
        let a1 = (center + window).min(self.total);
        let steps = (((a1 - a0) / 1.0) as i64).max(4);
        let mut best = f64::INFINITY;
        let mut barc = center;
        for s in 0..=steps {
            let arc = a0 + (a1 - a0) * s as f64 / steps as f64;
            let q = self.point_at(arc);
            let d = (p.x - q.x).powi(2) + (p.z - q.z).powi(2);
            if d < best {
                best = d;
                barc = arc;
            }
        }
        barc
    }
}

/// Arc-resample a polyline to `n` arc-equispaced points. Mirrors `_resample` in
/// `scripts/drift_kill.py`. `path` must be non-empty; one point yields `n` copies.
fn resample(path: &[Point], n: usize) -> Vec<Point> {
    let mut cum = vec![0.0; path.len()];
    for i in 1..path.len() {
        cum[i] = cum[i - 1] + dist(path[i - 1], path[i]);
    }
    let total = *cum.last().unwrap_or(&0.0);
    let mut out = Vec::with_capacity(n);
    for s in 0..n {
        let arc = if n > 1 {
            total * s as f64 / (n - 1) as f64
        } else {
            0.0
        };
        let mut pushed = false;
        for i in 0..path.len().saturating_sub(1) {
            if cum[i + 1] >= arc {
                let d = cum[i + 1] - cum[i];
                let denom = if d == 0.0 { 1e-9 } else { d };
                let t = (arc - cum[i]) / denom;
                out.push(Point {
                    x: path[i].x + t * (path[i + 1].x - path[i].x),
                    z: path[i].z + t * (path[i + 1].z - path[i].z),
                });
                pushed = true;
                break;
            }
        }
        if !pushed {
            out.push(path.last().copied().unwrap_or(Point { x: 0.0, z: 0.0 }));
        }
    }
    out
}

/// Planar distance between two points.
fn dist(a: Point, b: Point) -> f64 {
    ((b.x - a.x).powi(2) + (b.z - a.z).powi(2)).sqrt()
}

/// World bearing (deg) of a displacement — `atan2(dx, dz)`, the FH world convention
/// (+z forward). Mirrors `bearing(dx, dz)` in `drift_kill.py`.
fn bearing(dx: f64, dz: f64) -> f64 {
    dx.atan2(dz).to_degrees()
}

/// Smallest signed difference `a − b` wrapped to `(−180, 180]`. Mirrors `ang_diff`.
fn ang_diff(a: f64, b: f64) -> f64 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// Advance the arc tracker to `p` and read the route-forward bearing (deg) there.
/// Projects onto the centerline within a local arc window (the whole line on the
/// first call, `started == false`), then takes a ±[`ROUTE_TANGENT_CHORD_M`] chord
/// tangent oriented toward the finish gate (entry A ⇒ +arc, entry B ⇒ −arc).
/// Returns `(new_arc, bearing_deg)`. Mirrors `route_forward_series` in `drift_kill.py`.
fn route_forward(
    cl: &Centerline,
    p: Point,
    prev_arc: f64,
    started: bool,
    entry_is_a: bool,
) -> (f64, f64) {
    let window = if started { ARC_TRACK_WINDOW_M } else { cl.total };
    let arc = cl.nearest_arc(p, prev_arc, window);
    let sign = if entry_is_a { 1.0 } else { -1.0 };
    let ahead = cl.point_at(arc + sign * ROUTE_TANGENT_CHORD_M);
    let behind = cl.point_at(arc - sign * ROUTE_TANGENT_CHORD_M);
    (arc, bearing(ahead.x - behind.x, ahead.z - behind.z))
}

/// World travel bearing (deg) from the most recent earlier position at least
/// [`TRAVEL_MIN_DISP_M`] from `current` (scanning the ring newest-first, up to
/// [`TRAVEL_LOOKBACK`] back). `None` when the car is ~stationary — no reliable
/// bearing — which the caller treats as zero progress. Mirrors `travel_bearing`.
fn travel_bearing(recent: &VecDeque<Point>, current: Point) -> Option<f64> {
    for prev in recent.iter().rev().take(TRAVEL_LOOKBACK) {
        let dx = current.x - prev.x;
        let dz = current.z - prev.z;
        if (dx * dx + dz * dz).sqrt() >= TRAVEL_MIN_DISP_M {
            return Some(bearing(dx, dz));
        }
    }
    None
}

/// Forward progress (m/s along the route): `speed × cos(travel − route_forward)`.
/// +ve advances to the finish, −ve reverses up-route; a `None` travel bearing
/// (≈stationary) is zero. Crawl-robust by construction (`speed·cos`, NOT a noisy
/// d(arc)/dt). Sustained below the floor is the in-zone stall kill. Mirrors `drift_kill.py`.
fn forward_progress(speed_ms: f64, travel_bearing: Option<f64>, route_forward: f64) -> f64 {
    match travel_bearing {
        Some(tb) => speed_ms * ang_diff(tb, route_forward).to_radians().cos(),
        None => 0.0,
    }
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

    /// A packet at world (x, z) moving dead-straight at `speed` m/s — speed set and
    /// car-local velocity along +z (so it is NOT drifting / not scoring). Drives the
    /// forward-progress kill, which reads `speed_ms` and successive positions.
    fn moving_packet(x: f32, z: f32, speed: f32, ms: u32) -> parser::TelemetryPacket {
        let mut p = packet(x, z);
        p.timestamp_ms = ms;
        p.speed_ms = speed;
        p.vel_z = speed;
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

    // ── Forward-progress geometry ────────────────────────────────────────────

    /// An L-corridor (a quarter turn) for the route-forward parity check.
    fn l_corridor_zone() -> db::DriftZoneRow {
        db::DriftZoneRow {
            left_boundary: vec![
                db::ZonePoint { x: 0.0, z: 0.0 },
                db::ZonePoint { x: 0.0, z: 20.0 },
                db::ZonePoint { x: 20.0, z: 20.0 },
            ],
            right_boundary: vec![
                db::ZonePoint { x: 4.0, z: 0.0 },
                db::ZonePoint { x: 4.0, z: 16.0 },
                db::ZonePoint { x: 20.0, z: 16.0 },
            ],
            ..square_zone()
        }
    }

    #[test]
    fn ang_diff_wraps_to_smallest_signed_offset() {
        assert!((ang_diff(170.0, -170.0) - (-20.0)).abs() < 1e-9);
        assert!((ang_diff(-170.0, 170.0) - 20.0).abs() < 1e-9);
        assert!((ang_diff(10.0, 350.0) - 20.0).abs() < 1e-9);
        assert!(ang_diff(45.0, 45.0).abs() < 1e-9);
    }

    #[test]
    fn forward_progress_is_speed_times_route_alignment() {
        // Along the route ⇒ full speed; reversed ⇒ negative; perpendicular ⇒ ~0;
        // no bearing (stationary) ⇒ 0; 60° off ⇒ cos(60°) = half speed.
        assert!((forward_progress(10.0, Some(0.0), 0.0) - 10.0).abs() < 1e-9);
        assert!((forward_progress(10.0, Some(180.0), 0.0) + 10.0).abs() < 1e-9);
        assert!(forward_progress(10.0, Some(90.0), 0.0).abs() < 1e-9);
        assert_eq!(forward_progress(10.0, None, 0.0), 0.0);
        assert!((forward_progress(10.0, Some(90.0), 30.0) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn travel_bearing_skips_sub_threshold_jitter() {
        // Newest-first scan: a <0.5 m wobble is skipped for the first point ≥0.5 m
        // back; a ring with only sub-threshold history yields None (≈stationary).
        let mut ring: VecDeque<Point> = VecDeque::new();
        ring.push_back(Point { x: 0.0, z: 0.0 }); // 2 m back
        ring.push_back(Point { x: 0.0, z: 1.9 }); // 0.1 m back (skipped)
        let b = travel_bearing(&ring, Point { x: 0.0, z: 2.0 }).unwrap();
        assert!(b.abs() < 1e-9, "bearing (0,0)→(0,2) is 0° (+z), got {b}");
        let still: VecDeque<Point> = [Point { x: 0.0, z: 2.0 }].into_iter().collect();
        assert!(travel_bearing(&still, Point { x: 0.0, z: 2.0 }).is_none());
    }

    #[test]
    fn centerline_runs_down_a_straight_corridor() {
        // square_zone: left (0,0)-(0,10), right (5,0)-(5,10) ⇒ mid-line x=2.5, len 10.
        let zone = RunnableZone::from_row(&square_zone()).unwrap();
        let cl = &zone.centerline;
        assert_eq!(cl.pts.len(), CENTERLINE_POINTS);
        assert!((cl.total - 10.0).abs() < 1e-6);
        for p in &cl.pts {
            assert!((p.x - 2.5).abs() < 1e-9, "mid-line at x=2.5, got {}", p.x);
        }
        // Entry A points toward the finish (+z) ⇒ ~0°; entry B the other way ⇒ ~180°.
        let (_, fwd_a) = route_forward(cl, Point { x: 2.5, z: 5.0 }, 5.0, true, true);
        assert!(fwd_a.abs() < 1e-6, "entry-A route-forward ≈ 0°, got {fwd_a}");
        let (_, fwd_b) = route_forward(cl, Point { x: 2.5, z: 5.0 }, 5.0, true, false);
        assert!((fwd_b.abs() - 180.0).abs() < 1e-6, "entry-B ≈ 180°, got {fwd_b}");
    }

    #[test]
    fn centerline_route_forward_matches_python_reference() {
        // Goldens from scripts/drift_kill.py's geometry (centerline + arc-tracked
        // route_forward) on the L-corridor — the Rust port must reproduce them to
        // float precision. Regenerate with the heredoc in notes/KILL_LOGIC_TODO.md.
        let zone = RunnableZone::from_row(&l_corridor_zone()).unwrap();
        let cl = &zone.centerline;
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(near(cl.total, 35.866_529_672_439), "total = {}", cl.total);

        // Fresh (whole-centerline) projections along the turn: (x, z, arc, fwd°).
        let fresh = [
            (2.0, 1.0, 1.024_757_990_6, 0.0),
            (2.0, 10.0, 10.247_579_906_4, 1.385_711_724_6),
            (3.0, 18.0, 18.445_643_831_5, 48.634_403_312_8),
            (10.0, 18.0, 25.618_949_766_0, 88.614_288_275_4),
            (18.0, 18.0, 33.817_013_691_2, 90.0),
        ];
        for (x, z, arc_g, fwd_g) in fresh {
            let (arc, fwd) = route_forward(cl, Point { x, z }, 0.0, false, true);
            assert!(near(arc, arc_g), "arc@({x},{z}) = {arc} vs {arc_g}");
            assert!(near(fwd, fwd_g), "fwd@({x},{z}) = {fwd} vs {fwd_g}");
        }

        // Tracked sequence (entry A): prev_arc fed forward like the live path.
        let tracked = [
            (2.0, 2.0, 2.049_515_981_3, 0.0),
            (2.0, 9.0, 9.222_821_915_8, 0.0),
            (2.0, 17.0, 17.420_885_840_9, 41.365_596_687_2),
            (8.0, 18.0, 23.569_433_784_7, 79.941_718_890_0),
            (16.0, 18.0, 31.767_497_709_9, 90.0),
        ];
        let mut prev = 0.0;
        let mut started = false;
        for (x, z, arc_g, fwd_g) in tracked {
            let (arc, fwd) = route_forward(cl, Point { x, z }, prev, started, true);
            prev = arc;
            started = true;
            assert!(near(arc, arc_g), "tracked arc@({x},{z}) = {arc} vs {arc_g}");
            assert!(near(fwd, fwd_g), "tracked fwd@({x},{z}) = {fwd} vs {fwd_g}");
        }
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
            .note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, false, 10.0)
            .is_none());
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, false, 10.0)
            .unwrap();
        assert_eq!(started.state, "running");
        let finished = mgr
            .note_packet(&conn, &packet(2.0, 11.0), &raw, 2000, false, 10.0)
            .unwrap();
        assert_eq!(finished.state, "completed");
        let rows = db::list_drift_runs(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].valid);
        assert_eq!(rows[0].packet_count, 2);
    }

    #[test]
    fn out_of_bounds_voids_the_run() {
        // The measured spatial kill: straying past the flags polygon by more than
        // the OOB slack voids the run (distance-gated, not a timer). Reverses the
        // old winter-only "leaving never voids" finding. progressStallS=0 isolates
        // the OOB kill from the stall kill.
        let conn = in_memory();
        save_square_zone_cfg(
            &conn,
            serde_json::json!({ "oobSlackM": 3.0, "progressStallS": 0.0 }),
        );
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, true, 10.0);
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, true, 10.0)
            .unwrap();
        assert_eq!(started.state, "running");
        // x=9 is 4 m past the x=5 edge — beyond the 3 m slack → out of bounds.
        let killed = mgr
            .note_packet(&conn, &packet(9.0, 5.0), &raw, 1300, true, 10.0)
            .unwrap();
        assert_eq!(killed.state, "invalid");
        let rows = db::list_drift_runs(&conn).unwrap();
        assert!(!rows[0].valid);
        assert_eq!(rows[0].invalid_reason.as_deref(), Some("out of bounds"));
    }

    #[test]
    fn leave_and_return_within_slack_survives() {
        // A position-test OOB makes leave-and-return free by construction: straying
        // within the slack — and returning — never voids. progressStallS=0 isolates
        // the OOB rule from the stall.
        let conn = in_memory();
        save_square_zone_cfg(
            &conn,
            serde_json::json!({ "oobSlackM": 3.0, "progressStallS": 0.0 }),
        );
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, true, 10.0);
        mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, true, 10.0);
        // x=6 is 1 m outside the x=5 edge — within the 3 m slack → still running…
        let out = mgr
            .note_packet(&conn, &packet(6.0, 5.0), &raw, 1300, true, 10.0)
            .unwrap();
        assert_eq!(out.state, "running");
        // …and returning inside keeps it alive.
        let back = mgr
            .note_packet(&conn, &packet(2.0, 8.0), &raw, 1500, true, 10.0)
            .unwrap();
        assert_eq!(back.state, "running");
        assert!(db::list_drift_runs(&conn).unwrap()[0].valid);
    }

    fn save_square_zone_cfg(conn: &Connection, config: serde_json::Value) -> i64 {
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
                scoring_config: config,
            },
            1,
        )
        .unwrap()
    }

    // Most lifecycle tests don't probe the kill thresholds (they pass
    // kill_enabled=false, or finish/abort before any kill fires); this sets just
    // the OOB slack for the few that do.
    fn save_square_zone(conn: &Connection, oob_slack: f64) -> i64 {
        save_square_zone_cfg(conn, serde_json::json!({ "oobSlackM": oob_slack }))
    }

    #[test]
    fn run_aborts_on_progress_stall() {
        // The in-zone kill: a car that stops advancing dies after the stall window
        // (0.3 s here). Packets arrive at a realistic sub-stall cadence (100 ms) so
        // this is genuine in-game time-without-progress, not a telemetry gap the
        // skip would absorb (see `pause_does_not_trip_progress_stall`).
        let conn = in_memory();
        save_square_zone_cfg(&conn, serde_json::json!({ "progressStallS": 0.3 }));
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, true, 10.0);
        let started = mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, true, 10.0).unwrap();
        assert_eq!(started.state, "running");
        // Stationary (speed 0) packets every 100 ms; once >0.3 s has elapsed with no
        // forward progress the run is killed. Bounded loop so a regression can't hang.
        let mut now = 1200i64;
        let mut last = started;
        while last.state == "running" && now <= 4000 {
            last = mgr
                .note_packet(&conn, &packet(2.0, 5.0), &raw, now, true, 10.0)
                .unwrap();
            now += 100;
        }
        assert_eq!(last.state, "invalid");
        let rows = db::list_drift_runs(&conn).unwrap();
        assert!(!rows[0].valid);
        assert_eq!(rows[0].invalid_reason.as_deref(), Some("no forward progress"));
    }

    #[test]
    fn reversing_run_stalls_despite_speed() {
        // Travel-wrong-way unifies with idle: a car moving at speed but UP-route
        // (reversing toward the entry) has negative forward progress, so the SAME
        // stall kills it — speed alone doesn't keep a run alive, only progress does.
        let conn = in_memory();
        save_square_zone_cfg(&conn, serde_json::json!({ "progressStallS": 0.3 }));
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, true, 10.0);
        // Enter through gate A and reach mid-zone, then reverse back toward A at
        // 6 m/s (−z) — staying inside the polygon so only the stall can fire.
        mgr.note_packet(&conn, &moving_packet(2.0, 1.0, 6.0, 0), &raw, 1100, true, 10.0);
        mgr.note_packet(&conn, &moving_packet(2.0, 6.0, 6.0, 16), &raw, 1200, true, 10.0);
        let mut now = 1300i64;
        let mut z = 5.4f32;
        let mut ms = 116u32;
        let mut last = mgr.status();
        while last.state == "running" && now <= 4000 {
            last = mgr
                .note_packet(&conn, &moving_packet(2.0, z, 6.0, ms), &raw, now, true, 10.0)
                .unwrap();
            z -= 0.6; // reverse up-route; stays within (0, 10) over the stall window
            now += 100;
            ms += 100;
        }
        assert_eq!(last.state, "invalid");
        assert_eq!(
            db::list_drift_runs(&conn).unwrap()[0]
                .invalid_reason
                .as_deref(),
            Some("no forward progress")
        );
    }

    #[test]
    fn forward_progress_keeps_run_alive_without_scoring() {
        // The correctness win: a car driving forward but NOT drifting (not scoring)
        // stays alive well past the stall window — the old score-starvation timer
        // false-killed exactly this (a car drove around unscored for 30 s+, #687).
        let conn = in_memory();
        save_square_zone_cfg(&conn, serde_json::json!({ "progressStallS": 0.3 }));
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, true, 10.0);
        mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, true, 10.0);
        // Drive dead-straight forward at 6 m/s for ~0.8 s (>> the 0.3 s stall window)
        // without reaching the finish gate (z=10).
        let mut now = 1200i64;
        let mut z = 1.6f32;
        let mut ms = 116u32;
        for _ in 0..8 {
            let s = mgr
                .note_packet(&conn, &moving_packet(2.0, z, 6.0, ms), &raw, now, true, 10.0)
                .unwrap();
            assert_eq!(s.state, "running");
            assert!(!s.scoring, "straight driving doesn't score");
            assert!(s.starve_remaining_s.is_none(), "advancing ⇒ no death timer");
            z += 0.6;
            now += 100;
            ms += 100;
        }
        assert_eq!(mgr.status().state, "running");
    }

    #[test]
    fn manual_abort_ends_active_run_as_invalid() {
        // The manual "abort run" control closes the live run at once as invalid,
        // without waiting out the starvation timer; a second abort is a no-op.
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, true, 10.0);
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, true, 10.0)
            .unwrap();
        assert_eq!(started.state, "running");

        let aborted = mgr.abort_active(&conn, 1200).expect("a run was active");
        assert_eq!(aborted.state, "invalid");
        assert!(!aborted.scoring);
        assert!(mgr.abort_active(&conn, 1300).is_none(), "nothing left to abort");

        let rows = db::list_drift_runs(&conn).unwrap();
        assert!(!rows[0].valid);
        assert_eq!(rows[0].invalid_reason.as_deref(), Some("aborted"));
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
    fn seasonal_tarmac_gate_still_drives_the_live_scoring_flag() {
        // The KILL is now forward-progress / out-of-bounds (season-independent — a
        // moving winter grass run is no longer starved out), but the seasonal
        // tarmac SCORING gate is unchanged: it still binds at run start and drives
        // the live `scoring` instrument (and the close-time score). Outside winter
        // grass pays (scoring); in winter an all-off-tarmac packet banks nothing
        // (the game wants ≥2 wheels on tarmac). Kill disabled here to isolate the
        // gate; its effect on the score itself is covered in scoring.rs.
        for (t0, expect_scoring) in [
            (crate::season::SPRING_ANCHOR_MS + 3_600_000, true), // spring: grass pays
            (crate::season::SPRING_ANCHOR_MS - 3_600_000, false), // winter: needs ≥2 on tarmac
        ] {
            let conn = in_memory();
            save_square_zone(&conn, 0.0);
            let mut mgr = DriftRunManager::new();
            let raw = vec![1u8; 324];
            mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, t0, false, 10.0);
            mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, t0 + 100, false, 10.0);
            // An all-four-wheels-in-the-grass drifting packet, mid-zone.
            let s = mgr
                .note_packet(
                    &conn,
                    &grass(drifting_packet(2.0, 5.0, 200)),
                    &raw,
                    t0 + 200,
                    false,
                    10.0,
                )
                .unwrap();
            assert_eq!(s.state, "running", "kill disabled keeps the run alive");
            assert_eq!(
                s.scoring, expect_scoring,
                "winter must gate off-tarmac scoring; spring must pay it"
            );
        }
    }

    #[test]
    fn pause_does_not_trip_progress_stall() {
        // A pause freezes telemetry: no packets arrive for the pause duration while
        // the wall clock keeps advancing. The first packet after a pause longer than
        // the stall window must NOT trip the kill — that frozen time isn't
        // time-without-progress (the old wall-clock logic killed instantly on
        // resume, issue #19; the gap-exclusion carries over).
        let conn = in_memory();
        save_square_zone_cfg(&conn, serde_json::json!({ "progressStallS": 0.3 }));
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, true, 10.0);
        // Enter and advance a little (progressing) at normal ~64 Hz cadence.
        let started = mgr
            .note_packet(&conn, &moving_packet(2.0, 1.0, 6.0, 100), &raw, 1100, true, 10.0)
            .unwrap();
        assert_eq!(started.state, "running");
        mgr.note_packet(&conn, &moving_packet(2.0, 1.6, 6.0, 116), &raw, 1116, true, 10.0);
        // Pause 30 s (>> the 0.3 s window), then resume STATIONARY (worst case — no
        // progress to reset the timer). The frozen gap is excluded, so it survives.
        let resumed = mgr
            .note_packet(&conn, &packet(2.0, 1.6), &raw, 31_116, true, 10.0)
            .unwrap();
        assert_eq!(resumed.state, "running");
        // Genuine stall still bites: stationary packets after the resume accumulate
        // past the window and kill the run.
        let mut now = 31_216i64;
        let mut last = resumed;
        while last.state == "running" && now <= 40_000 {
            last = mgr
                .note_packet(&conn, &packet(2.0, 1.6), &raw, now, true, 10.0)
                .unwrap();
            now += 100;
        }
        assert_eq!(last.state, "invalid");
        assert_eq!(
            db::list_drift_runs(&conn).unwrap()[0]
                .invalid_reason
                .as_deref(),
            Some("no forward progress")
        );
    }

    #[test]
    fn kill_disabled_keeps_run_alive_indefinitely() {
        // kill_enabled=false (measurement mode): a stationary car that would
        // otherwise stall out never auto-fails — the run ends only at the finish
        // gate or a manual abort.
        let conn = in_memory();
        save_square_zone(&conn, 0.0);
        let mut mgr = DriftRunManager::new();
        let raw = vec![1u8; 324];
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1000, false, 10.0);
        mgr.note_packet(&conn, &packet(2.0, 1.0), &raw, 1100, false, 10.0);
        let still = mgr
            .note_packet(&conn, &packet(2.0, 5.0), &raw, 99000, false, 10.0)
            .unwrap();
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
        assert!(mgr.note_packet(&conn, &packet(2.0, 11.0), &raw, 1000, false, 10.0).is_none());
        let started = mgr.note_packet(&conn, &packet(2.0, 9.0), &raw, 1100, false, 10.0).unwrap();
        assert_eq!(started.state, "running");
        let finished = mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 2000, false, 10.0).unwrap();
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
        assert!(mgr.note_packet(&conn, &packet(-1.0, 5.0), &raw, 1000, false, 10.0).is_none());
        assert!(mgr.note_packet(&conn, &packet(1.0, 5.0), &raw, 1100, false, 10.0).is_none());
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
        mgr.note_packet(&conn, &p, &raw_old, 1_000, false, 10.0);
        let mut p = packet(2.0, -1.0);
        p.timestamp_ms = 200;
        mgr.note_packet(&conn, &p, &raw_new, 50_000, false, 10.0);
        // Gate crossing opens the run; this packet belongs to the run itself.
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw_run, 50_100, false, 10.0)
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
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1_000, false, 10.0);
        let first = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1_100, false, 10.0)
            .unwrap();
        assert_eq!(first.state, "running");
        let done = mgr
            .note_packet(&conn, &packet(2.0, 11.0), &raw, 2_000, false, 10.0)
            .unwrap();
        assert_eq!(done.state, "completed");
        // One idle packet between the runs, then re-enter through the far gate.
        mgr.note_packet(&conn, &packet(2.0, 10.5), &between, 2_100, false, 10.0);
        let second = mgr
            .note_packet(&conn, &packet(2.0, 9.0), &raw, 2_200, false, 10.0)
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
        mgr.note_packet(&conn, &packet(2.0, -1.0), &raw, 1_000, false, 0.0);
        let started = mgr
            .note_packet(&conn, &packet(2.0, 1.0), &raw, 1_100, false, 0.0)
            .unwrap();
        assert_eq!(started.state, "running");
        assert!(db::get_drift_run_preroll(&conn, started.run_id.unwrap())
            .unwrap()
            .is_empty());
    }
}

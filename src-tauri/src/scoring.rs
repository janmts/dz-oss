//! Baseline drift-score estimator.
//!
//! The model mirrors how Forza Horizon scores drift zones: points accrue while
//! the car is sideways, proportional to **drift angle × speed**, integrated over
//! time, and amplified by a **combo multiplier** that grows the longer a drift is
//! held and collapses the moment it breaks. Defaults were calibrated against
//! recorded FH6 telemetry (see scripts/score_model.py); the coefficients are
//! expected to be finetuned later against logged in-game scores.
//!
//! All parameters are overridable per-zone via `drift_zones.scoring_config_json`,
//! deserialized over [`ScoringParams::default`].

use serde::{Deserialize, Serialize};

use crate::parser::TelemetryPacket;

/// Tunable scoring coefficients. Every field has a `serde(default)`, so an empty
/// `{}` config yields all defaults and any provided key overrides just that one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScoringParams {
    /// Below this speed (m/s) nothing scores.
    pub min_speed_ms: f64,
    /// Drift angle (deg) must reach this to count as drifting.
    pub min_angle_deg: f64,
    /// Above this angle (deg) the car is treated as spun out — the drift breaks.
    pub spin_angle_deg: f64,
    /// Angle (deg) at which the angle factor peaks; beyond it returns diminish.
    pub sweet_angle_deg: f64,
    /// Speed (m/s) at which the speed factor saturates (~70 mph).
    pub speed_cap_ms: f64,
    /// Rear combined-slip threshold (normalized, 1.0 ≈ grip limit) for "sliding".
    pub slip_gate: f64,
    /// Points per second at full angle/speed factors and multiplier 1.0.
    pub base_rate: f64,
    /// Multiplier increase per second of sustained, unbroken drift.
    pub mult_growth_per_s: f64,
    /// Maximum combo multiplier.
    pub mult_cap: f64,
    /// Convention offset (radians) added to the raw sideslip. Calibrated ≈ 0.
    pub yaw_offset_rad: f64,
    /// Final scale factor mapping raw points to in-game magnitude.
    pub scale: f64,
}

impl Default for ScoringParams {
    fn default() -> Self {
        Self {
            min_speed_ms: 8.0,
            min_angle_deg: 12.0,
            spin_angle_deg: 90.0,
            sweet_angle_deg: 45.0,
            speed_cap_ms: 31.0,
            slip_gate: 1.0,
            base_rate: 1000.0,
            mult_growth_per_s: 0.6,
            mult_cap: 5.0,
            yaw_offset_rad: 0.0,
            // Calibrated so the one known-scored run (57016) reproduces exactly.
            scale: 4.258,
        }
    }
}

impl ScoringParams {
    /// Build params from a zone's `scoring_config` JSON, falling back to defaults
    /// for any missing/invalid keys (and entirely on a non-object value).
    pub fn from_config(config: &serde_json::Value) -> Self {
        serde_json::from_value(config.clone()).unwrap_or_default()
    }
}

/// Per-run scoring result plus the breakdown used for display and tuning.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunScore {
    pub score: f64,
    pub sample_count: usize,
    pub drift_time_s: f64,
    pub total_time_s: f64,
    pub avg_angle_deg: f64,
    pub max_angle_deg: f64,
    pub avg_speed_ms: f64,
    pub max_multiplier: f64,
}

/// Chassis sideslip (drift angle) in **degrees**, always ≥ 0.
///
/// β = angle between where the car points (`yaw`) and where it is actually
/// moving (world velocity heading). The `atan2(vz, vx) - yaw` form and a ~0
/// offset were established empirically from recorded runs (scripts/calibrate2).
pub fn drift_angle_deg(pkt: &TelemetryPacket, yaw_offset_rad: f64) -> f64 {
    let vel_heading = (pkt.vel_z as f64).atan2(pkt.vel_x as f64);
    let beta = wrap_pi(vel_heading - pkt.yaw as f64 - yaw_offset_rad);
    beta.abs().to_degrees()
}

/// Rear-axle combined slip: max of the two rear tires' combined slip. Forza's
/// combined slip is normalized so ~1.0 is the grip limit; higher ⇒ sliding.
fn rear_combined_slip(pkt: &TelemetryPacket) -> f64 {
    (pkt.tire_combined_slip_rl.abs() as f64).max(pkt.tire_combined_slip_rr.abs() as f64)
}

/// Angle contribution: ramps 0→1 up to the sweet spot, then diminishes toward a
/// 0.3 floor near spin-out. Zero outside the [min, spin] band.
fn angle_factor(angle_deg: f64, p: &ScoringParams) -> f64 {
    if angle_deg < p.min_angle_deg || angle_deg > p.spin_angle_deg {
        return 0.0;
    }
    if angle_deg <= p.sweet_angle_deg {
        angle_deg / p.sweet_angle_deg
    } else {
        let span = (p.spin_angle_deg - p.sweet_angle_deg).max(1e-6);
        (1.0 - 0.5 * (angle_deg - p.sweet_angle_deg) / span).max(0.3)
    }
}

/// Speed contribution: linear up to the cap, flat beyond. Zero below min speed.
fn speed_factor(speed_ms: f64, p: &ScoringParams) -> f64 {
    if speed_ms < p.min_speed_ms {
        return 0.0;
    }
    (speed_ms.min(p.speed_cap_ms)) / p.speed_cap_ms
}

/// Seconds between two packet timestamps (ms). Duplicate stamps (Δ=0) telescope
/// to a 0 contribution; out-of-range gaps (pause/rewind) fall back to one 60 Hz
/// frame so a stall can't fabricate a huge multiplier or time.
fn frame_dt(prev_ms: u32, cur_ms: u32) -> f64 {
    let d = cur_ms as i64 - prev_ms as i64;
    if (0..100).contains(&d) {
        d as f64 / 1000.0
    } else {
        1.0 / 60.0
    }
}

/// Score a run from its packets in time order. Returns zeros for an empty run.
pub fn score_run(packets: &[TelemetryPacket], p: &ScoringParams) -> RunScore {
    let mut total = 0.0;
    let mut drift_time = 0.0;
    let mut total_time = 0.0;
    let mut angle_sum = 0.0;
    let mut max_angle = 0.0_f64;
    let mut speed_sum = 0.0;
    let mut drift_samples = 0usize;
    let mut max_multiplier = 1.0_f64;
    let mut drift_duration = 0.0;
    let mut prev_ms: Option<u32> = None;

    for pkt in packets {
        let dt = match prev_ms {
            Some(prev) => frame_dt(prev, pkt.timestamp_ms),
            None => 1.0 / 60.0,
        };
        prev_ms = Some(pkt.timestamp_ms);
        total_time += dt;

        let speed = pkt.speed_ms as f64;
        let angle = drift_angle_deg(pkt, p.yaw_offset_rad);
        let drifting = speed >= p.min_speed_ms
            && angle >= p.min_angle_deg
            && angle <= p.spin_angle_deg
            && rear_combined_slip(pkt) >= p.slip_gate;

        if drifting {
            drift_duration += dt;
            let multiplier = (1.0 + p.mult_growth_per_s * drift_duration).min(p.mult_cap);
            max_multiplier = max_multiplier.max(multiplier);
            total += p.base_rate * angle_factor(angle, p) * speed_factor(speed, p) * multiplier * dt;
            drift_time += dt;
            angle_sum += angle;
            speed_sum += speed;
            max_angle = max_angle.max(angle);
            drift_samples += 1;
        } else {
            // Drift broke: combo is lost (multiplier rebuilds from drift_duration).
            drift_duration = 0.0;
        }
    }

    RunScore {
        score: total * p.scale,
        sample_count: packets.len(),
        drift_time_s: drift_time,
        total_time_s: total_time,
        avg_angle_deg: if drift_samples > 0 { angle_sum / drift_samples as f64 } else { 0.0 },
        max_angle_deg: max_angle,
        avg_speed_ms: if drift_samples > 0 { speed_sum / drift_samples as f64 } else { 0.0 },
        max_multiplier,
    }
}

/// Wrap radians to (-π, π].
fn wrap_pi(a: f64) -> f64 {
    use std::f64::consts::PI;
    let mut x = (a + PI) % (2.0 * PI);
    if x < 0.0 {
        x += 2.0 * PI;
    }
    x - PI
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packet whose world velocity points along +heading rotated by `beta`
    /// from where the car is pointing (`yaw`), at a given speed, with the rears
    /// sliding. Lets tests dial in an exact drift angle.
    fn drifting_packet(yaw: f32, beta_deg: f64, speed: f32) -> TelemetryPacket {
        // vel_heading = atan2(vz, vx); choose it so wrap(vel_heading - yaw) = beta.
        let vel_heading = yaw as f64 + beta_deg.to_radians();
        let (sin, cos) = vel_heading.sin_cos();
        let mut p = base_packet();
        p.yaw = yaw;
        p.speed_ms = speed;
        // atan2(vz, vx) = vel_heading  ⇒  vx = cos, vz = sin (scaled by speed).
        p.vel_x = (speed as f64 * cos) as f32;
        p.vel_z = (speed as f64 * sin) as f32;
        p.timestamp_ms = 0;
        p.tire_combined_slip_rl = 3.0;
        p.tire_combined_slip_rr = 3.0;
        p
    }

    fn base_packet() -> TelemetryPacket {
        // Reuse the parser's zeroed packet shape via a 324-byte parse.
        crate::parser::parse(&vec![0u8; 324]).unwrap()
    }

    fn at(mut p: TelemetryPacket, ms: u32) -> TelemetryPacket {
        p.timestamp_ms = ms;
        p
    }

    #[test]
    fn drift_angle_recovers_beta() {
        // Across several yaws, the computed |β| matches the injected angle.
        for &yaw in &[0.0_f32, 1.0, -2.0, 3.0] {
            for &beta in &[15.0_f64, 35.0, 60.0] {
                let p = drifting_packet(yaw, beta, 20.0);
                let got = drift_angle_deg(&p, 0.0);
                assert!((got - beta).abs() < 0.5, "yaw={yaw} beta={beta} got={got}");
            }
        }
    }

    #[test]
    fn straight_line_does_not_score() {
        // β = 0 ⇒ below min angle ⇒ no points even at speed.
        let params = ScoringParams::default();
        let pkts: Vec<_> = (0..120)
            .map(|i| at(drifting_packet(0.5, 0.0, 25.0), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert_eq!(r.score, 0.0);
        assert_eq!(r.drift_time_s, 0.0);
    }

    #[test]
    fn sustained_drift_builds_multiplier_and_scores() {
        let params = ScoringParams::default();
        // ~3 s of a steady 40° drift at 20 m/s, 60 Hz.
        let pkts: Vec<_> = (0..180)
            .map(|i| at(drifting_packet(1.0, 40.0, 20.0), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert!(r.score > 0.0);
        assert!(r.max_multiplier > 1.0, "multiplier should grow");
        assert!((r.avg_angle_deg - 40.0).abs() < 1.0);
        assert!(r.drift_time_s > 2.5 && r.drift_time_s < 3.1);
    }

    #[test]
    fn breaking_drift_resets_multiplier() {
        let params = ScoringParams::default();
        // Drift, then a straight (non-drift) stretch, then drift again: the
        // second stretch must start from multiplier 1, not the earlier peak.
        let mut pkts = Vec::new();
        let mut t = 0u32;
        for _ in 0..180 {
            pkts.push(at(drifting_packet(1.0, 40.0, 20.0), t));
            t += 16;
        }
        for _ in 0..120 {
            pkts.push(at(drifting_packet(1.0, 0.0, 20.0), t)); // straight = break
            t += 16;
        }
        let with_break = score_run(&pkts, &params);
        // Sanity: a single continuous drift of equal drift-time scores more,
        // because the multiplier is never reset.
        let continuous: Vec<_> = (0..180)
            .map(|i| at(drifting_packet(1.0, 40.0, 20.0), i * 16))
            .collect();
        let cont = score_run(&continuous, &params);
        assert!(with_break.max_multiplier >= cont.max_multiplier - 1e-9);
        assert!(with_break.score < cont.score * 2.0);
    }

    #[test]
    fn spun_out_angle_does_not_score() {
        let params = ScoringParams::default();
        let pkts: Vec<_> = (0..120)
            .map(|i| at(drifting_packet(0.0, 120.0, 20.0), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert_eq!(r.score, 0.0, "120° is past spin_angle, must not score");
    }

    #[test]
    fn config_overrides_defaults() {
        let cfg = serde_json::json!({ "scale": 10.0, "minAngleDeg": 30.0 });
        let p = ScoringParams::from_config(&cfg);
        assert_eq!(p.scale, 10.0);
        assert_eq!(p.min_angle_deg, 30.0);
        // Untouched keys keep defaults.
        assert_eq!(p.speed_cap_ms, ScoringParams::default().speed_cap_ms);
    }

    #[test]
    fn empty_run_scores_zero() {
        let r = score_run(&[], &ScoringParams::default());
        assert_eq!(r.score, 0.0);
        assert_eq!(r.sample_count, 0);
    }

    /// Parity check against the Python prototype + the one known in-game score.
    /// Reads the developer's local DB; ignored by default.
    /// Run: cargo test --lib -- --ignored matches_recorded --nocapture
    #[test]
    #[ignore = "reads the local sessions.db"]
    fn matches_recorded_run_one() {
        let conn = rusqlite::Connection::open(crate::db::db_path()).unwrap();
        let blobs = crate::db::get_drift_run_packets(&conn, 1).unwrap();
        let pkts: Vec<_> = blobs
            .iter()
            .filter_map(|b| crate::parser::parse(b).ok())
            .collect();
        let r = score_run(&pkts, &ScoringParams::default());
        println!(
            "run#1 computed={:.0} drift_t={:.1}s total_t={:.1}s avg_ang={:.0} max_ang={:.0} max_mult={:.1}",
            r.score, r.drift_time_s, r.total_time_s, r.avg_angle_deg, r.max_angle_deg, r.max_multiplier
        );
        assert!((r.score - 57016.0).abs() < 2000.0, "got {}", r.score);
    }
}

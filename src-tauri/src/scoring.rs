//! Baseline drift-score estimator.
//!
//! The model mirrors the strongest signal seen in recorded FH6 drift-zone data:
//! points accrue while the car is sideways, proportional to **drift angle ×
//! speed**, integrated over time. Earlier builds also applied a growing combo
//! multiplier, but the logged in-game scores fit better without it. The combo
//! knobs remain per-zone overrides for future experiments.
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
    /// Exponent for the low-angle ramp up to the sweet spot. 1.0 is linear;
    /// values <1 give shallow high-speed drifts more credit. The default 0.10
    /// fits the logged FH6 samples best. The optimum keeps sliding down as more
    /// low-angle cars are logged: on a 77-run sample the residual still
    /// correlated +0.71 with avg drift angle at 0.4, and after two shallow-angle
    /// cars landed (110 runs) it had crept back to +0.39 at 0.20. 0.10 nulls
    /// that correlation (+0.04) and gives the lowest error. In FH6, once the car
    /// is past the drift gate, angle barely scales the score — speed × time
    /// dominates.
    pub angle_power: f64,
    /// Speed (m/s) at which the speed factor saturates (~134 mph).
    pub speed_cap_ms: f64,
    /// Rear combined-slip threshold (normalized, 1.0 ≈ grip limit) for "sliding".
    pub slip_gate: f64,
    /// Points per second at full angle/speed factors and multiplier 1.0.
    pub base_rate: f64,
    /// Multiplier increase per second of sustained, unbroken drift. The default
    /// is 0.0 because the current FH6 drift-zone samples fit better without a
    /// combo-style multiplier.
    pub mult_growth_per_s: f64,
    /// Maximum combo multiplier. Values <=1.0 disable combo scaling.
    pub mult_cap: f64,
    /// Max time (s) a drift may dip out of band and still be considered a
    /// linked flick (drifting resumes the opposite direction). This only affects
    /// scoring when combo growth is enabled.
    pub transition_grace_s: f64,
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
            angle_power: 0.10,
            speed_cap_ms: 60.0,
            slip_gate: 1.0,
            base_rate: 1000.0,
            mult_growth_per_s: 0.0,
            mult_cap: 1.0,
            transition_grace_s: 0.5,
            // Least-squares fit across valid logged in-game scores (110 runs
            // across three zones) with the default no-combo, angle_power=0.10
            // model; re-derive as more scores are logged. Marker rows (the
            // all-9s placeholder) and invalid runs are excluded from the fit.
            scale: 10.986,
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
    /// Maximum score multiplier reached. Defaults to 1.0 unless combo growth is
    /// enabled by a per-zone config.
    pub max_multiplier: f64,
    /// Number of flicks (direction changes that bridged a short out-of-band dip).
    pub transitions: usize,
}

/// Chassis sideslip (drift angle) in **degrees**, always ≥ 0.
///
/// Forza reports velocity in the **car's local frame**, so sideslip is simply
/// the angle between lateral (`velX`) and longitudinal (`velZ`) velocity — no
/// yaw needed. Verified ≈0 (mean 0.03°, std 0.45°) on straight grip driving
/// across all headings (scripts/frame_test.py).
pub fn drift_angle_deg(pkt: &TelemetryPacket) -> f64 {
    (pkt.vel_x as f64)
        .atan2(pkt.vel_z as f64)
        .abs()
        .to_degrees()
}

/// Rear-axle combined slip: max of the two rear tires' combined slip. Forza's
/// combined slip is normalized so ~1.0 is the grip limit; higher ⇒ sliding.
fn rear_combined_slip(pkt: &TelemetryPacket) -> f64 {
    (pkt.tire_combined_slip_rl.abs() as f64).max(pkt.tire_combined_slip_rr.abs() as f64)
}

/// Angle contribution: ramps 0->1 up to the sweet spot using `angle_power`,
/// then declines linearly to 0.5 at the spin angle (the drop to 0 past spin is
/// the spin-out break). Zero outside the [min, spin] band.
fn angle_factor(angle_deg: f64, p: &ScoringParams) -> f64 {
    if angle_deg < p.min_angle_deg || angle_deg > p.spin_angle_deg {
        return 0.0;
    }
    if angle_deg <= p.sweet_angle_deg {
        let ramp = (angle_deg / p.sweet_angle_deg).clamp(0.0, 1.0);
        let power = p.angle_power.max(1e-6);
        ramp.powf(power)
    } else {
        // Linear decline from 1.0 at the sweet spot to 0.5 at the spin angle;
        // anything past spin is already 0 (the guard above). No lower floor is
        // needed — within (sweet, spin] the expression stays in [0.5, 1.0].
        let span = (p.spin_angle_deg - p.sweet_angle_deg).max(1e-6);
        1.0 - 0.5 * (angle_deg - p.sweet_angle_deg) / span
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
    let mut out_time = 0.0; // time since the drift last dipped out of band
    let mut last_sign = 0i8; // direction of the last in-band drift (+1 / −1)
    let mut transitions = 0usize;
    let mut prev_ms: Option<u32> = None;

    for pkt in packets {
        let dt = match prev_ms {
            Some(prev) => frame_dt(prev, pkt.timestamp_ms),
            None => 1.0 / 60.0,
        };
        prev_ms = Some(pkt.timestamp_ms);
        total_time += dt;

        let speed = pkt.speed_ms as f64;
        let signed = (pkt.vel_x as f64).atan2(pkt.vel_z as f64).to_degrees();
        let angle = signed.abs();
        let sign = if signed >= 0.0 { 1i8 } else { -1i8 };
        let drifting = speed >= p.min_speed_ms
            && angle >= p.min_angle_deg
            && angle <= p.spin_angle_deg
            && rear_combined_slip(pkt) >= p.slip_gate;

        if drifting {
            // Resuming after a dip: count a linked flick only if drifting picked
            // back up the opposite way within the grace window. If combo growth
            // is enabled, only those linked flicks preserve drift duration.
            if out_time > 0.0 {
                let flick = out_time <= p.transition_grace_s && last_sign != 0 && sign != last_sign;
                if flick {
                    transitions += 1;
                } else {
                    drift_duration = 0.0;
                }
                out_time = 0.0;
            }
            drift_duration += dt;
            let multiplier = if p.mult_growth_per_s > 0.0 && p.mult_cap > 1.0 {
                (1.0 + p.mult_growth_per_s * drift_duration).min(p.mult_cap)
            } else {
                1.0
            };
            max_multiplier = max_multiplier.max(multiplier);
            total +=
                p.base_rate * angle_factor(angle, p) * speed_factor(speed, p) * multiplier * dt;
            drift_time += dt;
            angle_sum += angle;
            speed_sum += speed;
            max_angle = max_angle.max(angle);
            drift_samples += 1;
            last_sign = sign;
        } else {
            out_time += dt;
            if out_time > p.transition_grace_s {
                drift_duration = 0.0; // sustained break: combo is lost
            }
        }
    }

    RunScore {
        score: total * p.scale,
        sample_count: packets.len(),
        drift_time_s: drift_time,
        total_time_s: total_time,
        avg_angle_deg: if drift_samples > 0 {
            angle_sum / drift_samples as f64
        } else {
            0.0
        },
        max_angle_deg: max_angle,
        avg_speed_ms: if drift_samples > 0 {
            speed_sum / drift_samples as f64
        } else {
            0.0
        },
        max_multiplier,
        transitions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packet with local-frame velocity at sideslip `beta_deg` and `speed`,
    /// rears sliding. Lets tests dial in an exact drift angle.
    fn drifting_packet(beta_deg: f64, speed: f32) -> TelemetryPacket {
        let b = beta_deg.to_radians();
        let mut p = base_packet();
        p.speed_ms = speed;
        // angle = atan2(vx, vz)  ⇒  vx = speed·sin β (lateral), vz = speed·cos β.
        p.vel_x = (speed as f64 * b.sin()) as f32;
        p.vel_z = (speed as f64 * b.cos()) as f32;
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

    fn combo_params() -> ScoringParams {
        ScoringParams {
            mult_growth_per_s: 0.6,
            mult_cap: 5.0,
            ..ScoringParams::default()
        }
    }

    #[test]
    fn drift_angle_recovers_beta() {
        for &beta in &[15.0_f64, 35.0, 60.0] {
            let p = drifting_packet(beta, 20.0);
            let got = drift_angle_deg(&p);
            assert!((got - beta).abs() < 0.5, "beta={beta} got={got}");
        }
    }

    #[test]
    fn straight_line_does_not_score() {
        // β = 0 ⇒ below min angle ⇒ no points even at speed.
        let params = ScoringParams::default();
        let pkts: Vec<_> = (0..120)
            .map(|i| at(drifting_packet(0.0, 25.0), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert_eq!(r.score, 0.0);
        assert_eq!(r.drift_time_s, 0.0);
    }

    #[test]
    fn default_angle_curve_boosts_shallow_drift() {
        let params = ScoringParams::default();
        let linear = ScoringParams {
            angle_power: 1.0,
            ..ScoringParams::default()
        };

        assert!(angle_factor(23.0, &params) > angle_factor(23.0, &linear));
        assert!((angle_factor(params.sweet_angle_deg, &params) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn sustained_drift_scores_without_default_combo() {
        let params = ScoringParams::default();
        // ~3 s of a steady 40° drift at 20 m/s, 60 Hz.
        let pkts: Vec<_> = (0..180)
            .map(|i| at(drifting_packet(40.0, 20.0), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert!(r.score > 0.0);
        assert_eq!(r.max_multiplier, 1.0);
        assert!((r.avg_angle_deg - 40.0).abs() < 1.0);
        assert!(r.drift_time_s > 2.5 && r.drift_time_s < 3.1);
    }

    #[test]
    fn combo_config_builds_multiplier() {
        let params = combo_params();
        let pkts: Vec<_> = (0..180)
            .map(|i| at(drifting_packet(40.0, 20.0), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert!(r.max_multiplier > 1.0, "configured multiplier should grow");
    }

    #[test]
    fn breaking_drift_resets_configured_multiplier() {
        let params = combo_params();
        // Drift, then a straight (non-drift) stretch, then drift again: the
        // second stretch must start from multiplier 1, not the earlier peak.
        let mut pkts = Vec::new();
        let mut t = 0u32;
        for _ in 0..180 {
            pkts.push(at(drifting_packet(40.0, 20.0), t));
            t += 16;
        }
        for _ in 0..120 {
            pkts.push(at(drifting_packet(0.0, 20.0), t)); // straight = break
            t += 16;
        }
        for _ in 0..180 {
            pkts.push(at(drifting_packet(40.0, 20.0), t));
            t += 16;
        }
        let with_break = score_run(&pkts, &params);
        // Sanity: a single continuous drift of equal drift-time scores more,
        // because the multiplier is never reset.
        let continuous: Vec<_> = (0..360)
            .map(|i| at(drifting_packet(40.0, 20.0), i * 16))
            .collect();
        let cont = score_run(&continuous, &params);
        assert!(cont.max_multiplier > with_break.max_multiplier);
        assert!(with_break.score < cont.score);
    }

    #[test]
    fn flick_keeps_configured_combo_but_straighten_breaks_it() {
        let p = combo_params();
        // Build a left drift, a brief through-zero, then a right drift.
        fn run(gap_ticks: usize) -> Vec<TelemetryPacket> {
            let mut pkts = Vec::new();
            let mut t = 0u32;
            for _ in 0..120 {
                pkts.push(at(drifting_packet(40.0, 20.0), t));
                t += 16;
            }
            for _ in 0..gap_ticks {
                pkts.push(at(drifting_packet(0.0, 20.0), t)); // straight (out of band)
                t += 16;
            }
            for _ in 0..120 {
                pkts.push(at(drifting_packet(-40.0, 20.0), t)); // opposite direction
                t += 16;
            }
            pkts
        }
        // ~0.13 s gap < grace → a flick: combo survives the direction change.
        let flick = score_run(&run(8), &p);
        // ~1.1 s gap > grace → a straighten: combo resets even though direction flips.
        let straighten = score_run(&run(70), &p);

        assert_eq!(flick.transitions, 1);
        assert_eq!(straighten.transitions, 0);
        // With combo growth explicitly enabled, the flick keeps building one
        // long combo while the straighten has to rebuild from 1×.
        assert!(flick.max_multiplier > straighten.max_multiplier + 0.2);
        assert!(flick.score > straighten.score);
    }

    #[test]
    fn spun_out_angle_does_not_score() {
        let params = ScoringParams::default();
        let pkts: Vec<_> = (0..120)
            .map(|i| at(drifting_packet(120.0, 20.0), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert_eq!(r.score, 0.0, "120° is past spin_angle, must not score");
    }

    #[test]
    fn config_overrides_defaults() {
        let cfg = serde_json::json!({ "scale": 10.0, "minAngleDeg": 30.0, "anglePower": 1.0 });
        let p = ScoringParams::from_config(&cfg);
        assert_eq!(p.scale, 10.0);
        assert_eq!(p.min_angle_deg, 30.0);
        assert_eq!(p.angle_power, 1.0);
        // Untouched keys keep defaults.
        assert_eq!(p.speed_cap_ms, ScoringParams::default().speed_cap_ms);
    }

    #[test]
    fn empty_run_scores_zero() {
        let r = score_run(&[], &ScoringParams::default());
        assert_eq!(r.score, 0.0);
        assert_eq!(r.sample_count, 0);
    }

    /// Parity check against the Python prototype + the logged in-game scores.
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
            "run#1 computed={:.0} drift_t={:.1}s total_t={:.1}s avg_ang={:.0} max_ang={:.0} max_mult={:.1} flicks={}",
            r.score, r.drift_time_s, r.total_time_s, r.avg_angle_deg, r.max_angle_deg, r.max_multiplier, r.transitions
        );
        // Scale is a least-squares fit across all logged scores, not a
        // single-point calibration, so run #1 is allowed to sit within ±15%.
        assert!(
            (r.score - 57016.0).abs() / 57016.0 < 0.15,
            "got {}",
            r.score
        );
    }
}

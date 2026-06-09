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
    /// Below this speed (m/s) nothing scores. Lowered 8→1.5 once burnout "cheese"
    /// runs (slow, lightly-angled, rear lit) showed FH6 scores moving drifts far
    /// below the old ~18 mph floor — the game's only speed requirement is that the
    /// car is actually moving. 1.5 m/s (~3.4 mph) calibrated on 5 such runs across
    /// 2 cars (reproduces them within ~5%); kept above ~0.5 m/s so the
    /// `atan2(velX,velZ)` drift angle stays meaningful (it's noise at true v≈0,
    /// where stationary/dead-straight burnouts correctly score nothing and starve
    /// out). Barely affects normal runs (almost none drift below 8 m/s).
    pub min_speed_ms: f64,
    /// Drift angle (deg) must reach this to count as drifting. Lowered 12→10
    /// once the tarmac gate removed the off-track over-scorers: those 10–12°
    /// packets are genuinely scoring slides the game credits, and counting them
    /// (with the flatter ramp below) erased the shallow under-scoring bias.
    pub min_angle_deg: f64,
    /// Spin-out cutoff: above this drift angle (deg) the car is treated as spun
    /// out and scores nothing. Raised 90→120 once wilder runs showed the game
    /// keeps scoring slides well past 90° (observed up to ~115°); capping at 90
    /// was zeroing real slide time and under-scoring the 12 runs that reach
    /// there (+0.10pp MAE to extend it). The decline SLOPE is anchored to 90°
    /// (see `above_sweet_decline` / `angle_factor`), so raising this cutoff
    /// extends scoring past 90° without altering the 45–90° band.
    pub spin_angle_deg: f64,
    /// Angle (deg) at which the angle factor peaks; beyond it returns diminish.
    pub sweet_angle_deg: f64,
    /// Exponent for the low-angle ramp up to the sweet spot. 1.0 is linear;
    /// values <1 give shallow high-speed drifts more credit. History tracked the
    /// optimum sliding down as low-angle cars landed (0.5→0.4→0.20→0.10). Now at
    /// **0.15, paired with `min_angle_deg`=10 and the tarmac gate** — that combo
    /// flattened the persistent shallow under-scoring (the <20° band went from
    /// −8% bias to flat) and roughly halved overall MAE (3.40%→1.68% on 236
    /// runs). The earlier 0.10 fit could not chase this while off-track runs
    /// were over-scoring; the tarmac gate unblocked it. In FH6, once past the
    /// drift gate, angle barely scales the score — speed × time dominates.
    pub angle_power: f64,
    /// Decline of the angle factor above the sweet spot, expressed as the drop
    /// by **90°**: the factor reaches `1.0 - this` at 90° and continues at that
    /// same per-degree slope until `spin_angle_deg`. (The slope is anchored to
    /// 90°, NOT to the cutoff, so the cutoff can move without changing the
    /// curve.) 0.0 = flat. The old hardcoded value was 0.5; lowered to 0.25 once
    /// steep (>45°) runs from an S2 AWD landed and showed 0.5 over-penalized the
    /// 45–57° band. Refit `scale` after changing. ⚠️ STILL THE THINNEST-
    /// SUPPORTED LEVER: the steep tail (≥40° avg) is only ~8 runs, ALL from one
    /// car (S2 AWD ord 3865), and it's the cohort the shallow retune slightly
    /// worsened (−4.5%→−6.3%). Steep angle is fully confounded with that car —
    /// gather steep runs across MULTIPLE cars before retuning this; do not chase
    /// it on the S2 alone. (Instantaneous angles now observed up to ~115°.)
    pub above_sweet_decline: f64,
    /// Speed (m/s) at which the speed factor saturates (~134 mph).
    pub speed_cap_ms: f64,
    /// Rear combined-slip threshold (normalized, 1.0 ≈ grip limit) for "sliding".
    pub slip_gate: f64,
    /// Points per second at full angle/speed factors and multiplier 1.0.
    pub base_rate: f64,
    /// When true, a packet scores only while **at least one tyre is on tarmac**;
    /// a fully off-track packet (all four wheels on grass/dirt) scores nothing.
    /// This mirrors FH6: drifting through the scenery earns no points. Validated
    /// on 236 logged runs — gating all-four-off packets to zero dropped MAE
    /// 3.66%→3.40% and fixed the deep-grass over-scorers without touching runs
    /// that merely clip the verge with one wheel. Refit `scale` if toggled.
    pub require_tarmac_contact: bool,
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
            min_speed_ms: 1.5,
            min_angle_deg: 10.0,
            spin_angle_deg: 120.0,
            sweet_angle_deg: 45.0,
            angle_power: 0.15,
            above_sweet_decline: 0.25,
            speed_cap_ms: 60.0,
            slip_gate: 1.0,
            base_rate: 1000.0,
            require_tarmac_contact: true,
            mult_growth_per_s: 0.0,
            mult_cap: 1.0,
            transition_grace_s: 0.5,
            // Least-squares fit across valid logged in-game scores (242 runs
            // across three zones / 7 cars) with the no-combo, min_angle=10,
            // angle_power=0.15, above_sweet_decline=0.25, spin_angle=120,
            // min_speed=1.5 model AND the tarmac gate on; re-derive as more
            // scores are logged. Marker rows (the all-9s placeholder) and invalid
            // runs are excluded from the fit. Lineage: 11.024 (no gate) → 11.038
            // (gate, min12/ap0.10) → 10.814 (gate, min10/ap0.15) → 10.813 (spin
            // 90→120) → 10.803 (min_speed 8→1.5, now scoring low-speed burnout
            // drifts). Normal-run MAE ~1.61%; ~1.68% incl. the 5 burnout runs.
            scale: 10.803,
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

/// SurfaceRumble below this counts the wheel as "on tarmac". A smooth surface
/// reports exactly 0; grass/dirt/gravel reads well above 0.05, so any cutoff in
/// (0, 0.05] gives identical results (the signal is effectively binary).
const TARMAC_RUMBLE_EPS: f32 = 0.05;

/// True if **at least one tyre** is on tarmac. Only a fully off-track packet —
/// all four wheels on a rough surface — returns false.
fn on_tarmac(pkt: &TelemetryPacket) -> bool {
    pkt.surface_rumble_fl <= TARMAC_RUMBLE_EPS
        || pkt.surface_rumble_fr <= TARMAC_RUMBLE_EPS
        || pkt.surface_rumble_rl <= TARMAC_RUMBLE_EPS
        || pkt.surface_rumble_rr <= TARMAC_RUMBLE_EPS
}

/// Whether the car is sliding hard enough to count as drifting: above the min
/// speed, drift angle within the [min, spin] band, and the rear axle past the
/// slip gate. Independent of surface — see [`is_scoring_packet`] for the
/// points-accruing predicate.
pub fn is_drifting(pkt: &TelemetryPacket, p: &ScoringParams) -> bool {
    let speed = pkt.speed_ms as f64;
    let angle = drift_angle_deg(pkt);
    speed >= p.min_speed_ms
        && angle >= p.min_angle_deg
        && angle <= p.spin_angle_deg
        && rear_combined_slip(pkt) >= p.slip_gate
}

/// Whether this packet earns points: drifting **and**, when the tarmac gate is
/// on, at least one tyre on tarmac. This is the signal the run-abort starvation
/// timer watches — a run with no scoring packet for too long is dead.
pub fn is_scoring_packet(pkt: &TelemetryPacket, p: &ScoringParams) -> bool {
    is_drifting(pkt, p) && (!p.require_tarmac_contact || on_tarmac(pkt))
}

/// Angle contribution: ramps 0->1 up to the sweet spot using `angle_power`, then
/// declines linearly toward `1.0 - above_sweet_decline` at the spin angle (the
/// drop to 0 past spin is the spin-out break). Zero outside the [min, spin] band.
fn angle_factor(angle_deg: f64, p: &ScoringParams) -> f64 {
    if angle_deg < p.min_angle_deg || angle_deg > p.spin_angle_deg {
        return 0.0;
    }
    if angle_deg <= p.sweet_angle_deg {
        let ramp = (angle_deg / p.sweet_angle_deg).clamp(0.0, 1.0);
        let power = p.angle_power.max(1e-6);
        ramp.powf(power)
    } else {
        // Linear decline from 1.0 at the sweet spot at a FIXED rate: it reaches
        // (1 - above_sweet_decline) at 90° and continues at that same slope up to
        // spin_angle_deg (past which the guard above returns 0). Anchoring the
        // slope to 90° rather than to spin_angle_deg decouples "how fast credit
        // falls off" from "where a drift counts as spun out", so the cutoff can
        // be raised past 90° (the game scores slides to ~115°) without changing
        // the well-fit 45–90° band. Clamp at 0 for safety on extreme configs.
        const DECLINE_REF_DEG: f64 = 90.0;
        let span = (DECLINE_REF_DEG - p.sweet_angle_deg).max(1e-6);
        (1.0 - p.above_sweet_decline * (angle_deg - p.sweet_angle_deg) / span).max(0.0)
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
        let drifting = is_drifting(pkt, p);

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
            // Points accrue only with a tyre on tarmac (when the gate is on);
            // off-track sliding still counts as drift time / continuity below.
            if !p.require_tarmac_contact || on_tarmac(pkt) {
                total +=
                    p.base_rate * angle_factor(angle, p) * speed_factor(speed, p) * multiplier * dt;
            }
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

    /// Put all four wheels on a rough surface (fully off-track).
    fn all_wheels_off_tarmac(mut p: TelemetryPacket) -> TelemetryPacket {
        p.surface_rumble_fl = 0.6;
        p.surface_rumble_fr = 0.6;
        p.surface_rumble_rl = 0.6;
        p.surface_rumble_rr = 0.6;
        p
    }

    #[test]
    fn fully_off_track_drift_scores_zero() {
        let params = ScoringParams::default();
        // 3 s of a steady 40° drift at 20 m/s, but all four wheels in the grass.
        let pkts: Vec<_> = (0..180)
            .map(|i| at(all_wheels_off_tarmac(drifting_packet(40.0, 20.0)), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert_eq!(r.score, 0.0, "all-four-off-tarmac must not score");
        // It still counts as drift time / sliding for the breakdown.
        assert!(r.drift_time_s > 2.5);
    }

    #[test]
    fn one_tyre_on_tarmac_still_scores_fully() {
        let params = ScoringParams::default();
        let on_one = |i: u32| {
            let mut p = all_wheels_off_tarmac(drifting_packet(40.0, 20.0));
            p.surface_rumble_rl = 0.0; // one rear wheel back on tarmac
            at(p, i * 16)
        };
        let mixed: Vec<_> = (0..180).map(on_one).collect();
        let all_on: Vec<_> = (0..180).map(|i| at(drifting_packet(40.0, 20.0), i * 16)).collect();
        let r_mixed = score_run(&mixed, &params);
        let r_on = score_run(&all_on, &params);
        assert!(r_mixed.score > 0.0);
        // One tyre on tarmac earns the same as all four — the gate is all-or-nothing.
        assert!((r_mixed.score - r_on.score).abs() < 1e-6);
    }

    #[test]
    fn tarmac_gate_can_be_disabled() {
        let params = ScoringParams {
            require_tarmac_contact: false,
            ..ScoringParams::default()
        };
        let pkts: Vec<_> = (0..180)
            .map(|i| at(all_wheels_off_tarmac(drifting_packet(40.0, 20.0)), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert!(r.score > 0.0, "with the gate off, off-track drift scores again");
    }

    #[test]
    fn default_gate_counts_shallow_eleven_degree_drift() {
        // The retune lowered min_angle 12→10, so an 11° slide now scores.
        let params = ScoringParams::default();
        assert_eq!(params.min_angle_deg, 10.0);
        assert!(angle_factor(11.0, &params) > 0.0);
        // ...and a 9° slide is still below the gate.
        assert_eq!(angle_factor(9.0, &params), 0.0);
    }

    #[test]
    fn low_speed_angled_drift_scores_with_lowered_floor() {
        // min_speed is 1.5: a 3 m/s angled slide (burnout-cheese regime) now
        // scores; a 1 m/s crawl is still below the floor and scores nothing.
        let params = ScoringParams::default();
        assert_eq!(params.min_speed_ms, 1.5);
        let slow: Vec<_> = (0..120).map(|i| at(drifting_packet(20.0, 3.0), i * 16)).collect();
        assert!(score_run(&slow, &params).score > 0.0, "3 m/s angled drift should score");
        let crawl: Vec<_> = (0..120).map(|i| at(drifting_packet(20.0, 1.0), i * 16)).collect();
        assert_eq!(score_run(&crawl, &params).score, 0.0, "1 m/s is below the floor");
    }

    #[test]
    fn spun_out_angle_does_not_score() {
        let params = ScoringParams::default();
        let pkts: Vec<_> = (0..120)
            .map(|i| at(drifting_packet(150.0, 20.0), i * 16))
            .collect();
        let r = score_run(&pkts, &params);
        assert_eq!(r.score, 0.0, "150° is past spin_angle (120), must not score");
    }

    #[test]
    fn drift_past_ninety_still_scores_but_decline_anchor_holds() {
        // spin_angle is 120: a 100° slide is past the old 90° cap but still
        // scores (the game credits slides to ~115°); 130° is spun out → 0.
        let params = ScoringParams::default();
        assert!(angle_factor(100.0, &params) > 0.0);
        assert_eq!(angle_factor(130.0, &params), 0.0);
        // Raising the cutoff must NOT change the 45–90° band: the decline is
        // anchored at 90°, so factor(90°) is exactly 1 − above_sweet_decline.
        assert!((angle_factor(90.0, &params) - (1.0 - params.above_sweet_decline)).abs() < 1e-9);
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

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
    /// by **90°**: the factor falls by `this` at 90° and continues at that
    /// same per-degree slope until `spin_angle_deg`. (The slope is anchored to
    /// 90°, NOT to the cutoff, so the cutoff can move without changing the
    /// curve.) 0.0 = no decline. Lineage: hardcoded 0.5 → 0.25 (S2 AWD steep
    /// runs) → **0.0** (per-frame vision showed the game's credit does not fall
    /// off above 45°). The steep-angle RISE the vision data favoured lives in
    /// `above_sweet_rise`/`rise_saturation_deg`, not in a negative value here;
    /// this stays as a per-zone override lever. Refit `scale` after changing.
    pub above_sweet_decline: f64,
    /// Saturating RISE of the angle factor above the sweet spot: the factor
    /// climbs from 1.0 at `sweet_angle_deg` to `1.0 + this` at
    /// `rise_saturation_deg` and then holds that plateau out to
    /// `spin_angle_deg`. Default **0.085** — per-frame vision ground truth
    /// (OCR of the on-screen tally) across 9 instrumented runs / 4 cars /
    /// 4 zones / both seasons shows the game's steep-angle credit is already
    /// ~+5% by 50° and plateaus at ~+8–9% by ~58°, holding flat past 100°
    /// (pooled in-band ratios at v≥12 m/s, normalized per run by the
    /// below-sweet band; saturating-step rms 1.35% vs 2.7% for a linear ramp
    /// vs 6.2% flat). The DB-wide integral agrees: steep-exposure-quartile
    /// bias −1.6%→−0.5% with no movement (≤0.4pp) in the grass cohorts.
    /// Refit `scale` after changing.
    pub above_sweet_rise: f64,
    /// Angle (deg) at which `above_sweet_rise` is fully reached (the plateau
    /// start). The rise ramps linearly from `sweet_angle_deg` to here.
    pub rise_saturation_deg: f64,
    /// Speed (m/s) at which the speed factor saturates (~134 mph).
    pub speed_cap_ms: f64,
    /// Rear combined-slip threshold (normalized, 1.0 ≈ grip limit) for "sliding".
    pub slip_gate: f64,
    /// Points per second at full angle/speed factors and multiplier 1.0.
    pub base_rate: f64,
    /// When true, a packet scores only while at least `min_tarmac_wheels` tyres
    /// are on tarmac; an off-track packet (too many wheels on grass/dirt) scores
    /// nothing. This mirrors FH6: drifting through the scenery earns no points.
    /// Validated on 236 logged runs — gating fully-off packets to zero dropped
    /// MAE 3.66%→3.40% and fixed the deep-grass over-scorers. Refit `scale` if
    /// toggled.
    pub require_tarmac_contact: bool,
    /// How many wheels must be on tarmac for a packet to score (when
    /// `require_tarmac_contact` is on). Raised 1→**2**: a max-error sweep showed
    /// every big over-scorer banked 6–22% of its points with only 1–2 wheels on
    /// tarmac (verge-clipping runs the game wasn't crediting), and play-testing
    /// confirmed the mechanic directly — two wheels on tarmac bank, one does
    /// not, with no front-vs-rear distinction. Requiring 2 cut DB MAE
    /// 1.65%→1.41% and the worst run error 15.5%→6.3% (253 runs); 3+ or
    /// contact-weighted variants over-correct. The original "≥1 beat ≥2"
    /// comparison predates the shallow retune, whose miscalibration masked the
    /// partial-contact over-credit. Refit `scale` after changing.
    pub min_tarmac_wheels: u32,
    /// LOW-SPEED COMPOSITE term 1 — extra below-sweet steepening at crawl
    /// speeds. Below `sweet_angle_deg` the ramp exponent becomes
    /// `angle_power + this × w(v)`, where w fades linearly from 1 at
    /// `lowspeed_full_ms` to 0 at `lowspeed_zero_ms`. Anchored at the sweet
    /// spot: 45°+ pays full rate at any speed (run #521 measured 45–60° at
    /// 1.00 at 5–9 m/s while 10–45° was depressed, deepening toward low angle
    /// and low speed). Default **0.22**, fit jointly with the pause/transit
    /// terms on the crawl-specimen integrals (8 z3 low-speed runs, +2.8..+10.9%
    /// → ±1.5% MAE) against per-frame vision core cells across 9 runs; guards:
    /// burnouts, z5 held out (gate-B offset family). Refit `scale` if changed.
    pub lowspeed_power_add: f64,
    /// Speed (m/s) at and below which the low-speed steepening is at full
    /// depth.
    pub lowspeed_full_ms: f64,
    /// Speed (m/s) at and above which the low-speed steepening is gone. Vision
    /// core cells read ~0.92–0.96 at 5–9 m/s, ~0.97 at 12–14, clean by ~14–16.
    pub lowspeed_zero_ms: f64,
    /// LOW-SPEED COMPOSITE term 2 — per-flip pause: after each drift-direction
    /// flip (signed-angle reversal between consecutive scoring packets, the
    /// `direction_flips` definition) in-band credit is suppressed for this many
    /// seconds, weighted by a speed fade that reaches 0 at
    /// `flip_pause_zero_ms`. Vision shows every flip sits in a 0.5–1.5 s
    /// banking pause; net of the transit credit below this costs ~−65 pts per
    /// flip at 7 m/s (the #509/#513 35-vs-86-flip controlled pair, which the
    /// fitted pair reproduces to +0.05pp). 0 disables. Refit `scale` if
    /// changed.
    pub flip_pause_s: f64,
    /// Speed (m/s) at which the flip-pause weight reaches zero (ramps down
    /// over the 6 m/s below it). The pause is measurably alive at 12.8 m/s
    /// (#459) and gone for normal-speed runs.
    pub flip_pause_zero_ms: f64,
    /// LOW-SPEED COMPOSITE term 3 — transit credit: when scoring resumes after
    /// a sub-gate dip (angle dropped below `min_angle_deg`, a flip's
    /// through-zero, a brief speed/slip dip), the dip is re-paid at the last
    /// in-band rate × this gain, for up to 0.5 s of dip, weighted by a speed
    /// fade reaching 0 at `transit_zero_ms`. This is the game's through-dip /
    /// re-establishment credit in the crawl regime (#457/#458 measured the
    /// equivalent of 0.45–0.49 × a 0.5 s-cap unit — i.e. this gain — and
    /// burnout timelines bank through every weave dip). 0 disables. Refit
    /// `scale` if changed.
    pub transit_gain: f64,
    /// Speed (m/s) at which the transit-credit weight reaches zero (ramps down
    /// over the 6 m/s below it). At normal speeds gripped straights pay
    /// nothing (#493/#496/#505: zero trickle between drifts), so the credit
    /// must die well before the normal regime.
    pub transit_zero_ms: f64,
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
            above_sweet_decline: 0.0,
            above_sweet_rise: 0.085,
            rise_saturation_deg: 58.0,
            speed_cap_ms: 60.0,
            slip_gate: 1.0,
            base_rate: 1000.0,
            require_tarmac_contact: true,
            min_tarmac_wheels: 2,
            lowspeed_power_add: 0.22,
            lowspeed_full_ms: 3.0,
            lowspeed_zero_ms: 16.0,
            flip_pause_s: 0.04,
            flip_pause_zero_ms: 16.0,
            transit_gain: 0.40,
            transit_zero_ms: 11.0,
            mult_growth_per_s: 0.0,
            mult_cap: 1.0,
            transition_grace_s: 0.5,
            // Least-squares fit across valid logged in-game scores (358 runs
            // across five zones / 8+ cars, season-aware: winter runs gated,
            // spring runs ungated; crawl specimens now INCLUDED since the
            // low-speed composite models them) with the no-combo, min_angle=10,
            // angle_power=0.15, above_sweet_rise=0.085@58°, spin_angle=120,
            // min_speed=1.5 model, the 2-wheel tarmac gate AND the low-speed
            // composite (steepening 0.22→16, pause 0.04s, transit 0.40→11) on;
            // re-derive as more scores are logged. Marker rows (the all-9s
            // placeholder), invalid runs and the #387/#388 state-machine
            // specimens are excluded from the fit. Lineage: 11.024 (no gate) →
            // 11.038 (gate, min12/ap0.10) → 10.814 (gate, min10/ap0.15) →
            // 10.813 (spin 90→120) → 10.803 (min_speed 8→1.5) → 10.745
            // (decline 0.25→0.0 from per-frame vision) → 10.768 (tarmac gate
            // 1→2 wheels) → 10.668 (saturating steep-angle rise 0.085@58°) →
            // 10.716 (low-speed composite).
            scale: 10.716,
        }
    }
}

impl ScoringParams {
    /// Build params from a zone's `scoring_config` JSON, falling back to defaults
    /// for any missing/invalid keys (and entirely on a non-object value).
    pub fn from_config(config: &serde_json::Value) -> Self {
        serde_json::from_value(config.clone()).unwrap_or_default()
    }

    /// Season-adjusted copy: in seasons where off-tarmac surfaces pay (every
    /// season but winter — see [`crate::season`]), the tarmac-contact gate is
    /// moot and is switched off, so grass banks at full rate. Because
    /// [`is_scoring_packet`] follows the same params, grass dwell then no
    /// longer trips the score-starvation abort either (a real spring failure
    /// mode: ~6 s grass excursions the game happily pays through). Winter
    /// keeps the gate exactly as configured.
    pub fn for_season(&self, season: crate::season::Season) -> Self {
        let mut p = self.clone();
        if season.off_tarmac_pays() {
            p.require_tarmac_contact = false;
        }
        p
    }
}

/// One sector's slice of a run — the points banked between two split gates (or
/// between an end gate and the nearest split). Index-aligned to the zone's
/// `sectorNames` in A→B order (N splits ⇒ N+1 sectors), so it reads the same
/// whichever gate the run entered through. Drives the run viewer's "where did I
/// score" per-sector readout. Summing `points` across sectors reproduces the run
/// score. Only the run-viewer re-scoring path fills these (the live scorer has
/// no zone geometry); a run with no split geometry leaves the vector empty.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SectorScore {
    /// Scaled points banked while in this sector.
    pub points: f64,
    /// Packets recorded in this sector (drifting or not).
    pub sample_count: usize,
    /// Seconds spent drifting in this sector.
    pub drift_time_s: f64,
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
    /// Drift-direction sign changes between consecutive *scoring* packets — the
    /// "flip count" the tuning analyses use (weave jitter shows up here). Unlike
    /// `transitions` it has no grace window: every signed-angle reversal while
    /// banking counts.
    pub direction_flips: usize,
    /// Scaled points re-paid through brief sub-gate dips by the low-speed
    /// transit term (0 for normal-speed runs — the credit fades out by
    /// `transit_zero_ms`).
    pub transit_pts: f64,
    /// Scaled points withheld by the low-speed flip-pause term (0 for
    /// normal-speed runs).
    pub flip_pause_pts: f64,
    /// Per-sector point rollup in A→B order (see [`SectorScore`]). Empty unless a
    /// run was scored against zone split geometry (the run-viewer path); the live
    /// scorer and geometry-less re-scores leave it empty.
    pub sectors: Vec<SectorScore>,
}

/// Per-packet scoring diagnostics emitted alongside the run score, one entry per
/// input packet in the same order. Lets the run viewer colour each tick by what
/// the scorer actually did — banking (green), drifting-but-unpaid (red), or idle
/// (grey) — without re-deriving the season/tarmac gate on the frontend.
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TickScore {
    /// Sliding hard enough to count as drifting (speed/angle/slip gates), surface aside.
    pub is_drifting: bool,
    /// Banking points this tick: drifting AND (gate off OR enough wheels on tarmac).
    pub is_scoring: bool,
    /// Wheels on tarmac (SurfaceRumble ≈ 0) this tick, 0–4.
    pub tarmac_wheels: u32,
    /// Scaled points banked at this tick (main credit less the flip-pause, plus
    /// any transit credit re-paid here). Summing these reproduces the run score.
    pub points: f64,
    /// Sector this tick falls in: splits crossed since the entry gate, monotonic
    /// (furthest-reached, so weaving across a boundary can't bounce it). A→B
    /// canonical, so `sectorNames[sector]` names it regardless of entry
    /// direction. 0 when the zone has no splits or no geometry was available
    /// (the live scorer leaves it 0; the run-viewer path fills it).
    pub sector: u32,
}

/// Signed chassis sideslip (drift angle) in **degrees** — positive when sliding
/// one way, negative the other; the live instrument and flip counting need the
/// sign.
///
/// Forza reports velocity in the **car's local frame**, so sideslip is simply
/// the angle between lateral (`velX`) and longitudinal (`velZ`) velocity — no
/// yaw needed. Verified ≈0 (mean 0.03°, std 0.45°) on straight grip driving
/// across all headings (scripts/frame_test.py).
pub fn drift_angle_signed_deg(pkt: &TelemetryPacket) -> f64 {
    (pkt.vel_x as f64).atan2(pkt.vel_z as f64).to_degrees()
}

/// Chassis sideslip (drift angle) in **degrees**, always ≥ 0.
pub fn drift_angle_deg(pkt: &TelemetryPacket) -> f64 {
    drift_angle_signed_deg(pkt).abs()
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

/// Number of tyres on tarmac (smooth surface, SurfaceRumble ≈ 0).
fn tarmac_wheel_count(pkt: &TelemetryPacket) -> u32 {
    [
        pkt.surface_rumble_fl,
        pkt.surface_rumble_fr,
        pkt.surface_rumble_rl,
        pkt.surface_rumble_rr,
    ]
    .iter()
    .filter(|&&r| r <= TARMAC_RUMBLE_EPS)
    .count() as u32
}

/// True if enough tyres are on tarmac for the packet to bank points. The game
/// needs **two** wheels down (play-tested: two bank, one doesn't, front vs rear
/// makes no difference) — see `min_tarmac_wheels`.
fn on_tarmac(pkt: &TelemetryPacket, p: &ScoringParams) -> bool {
    tarmac_wheel_count(pkt) >= p.min_tarmac_wheels
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
/// on, enough tyres on tarmac. This is the signal the run-abort starvation
/// timer watches — a run with no scoring packet for too long is dead.
pub fn is_scoring_packet(pkt: &TelemetryPacket, p: &ScoringParams) -> bool {
    is_drifting(pkt, p) && (!p.require_tarmac_contact || on_tarmac(pkt, p))
}

/// Ramp span (m/s) over which the flip-pause / transit weights fade from 1 to
/// 0 below their respective `*_zero_ms` speeds.
const REGIME_FADE_SPAN_MS: f64 = 6.0;

/// Longest stretch of a sub-gate dip (s) the transit credit re-pays.
const TRANSIT_CAP_S: f64 = 0.5;

/// Low-speed steepening weight: 1 at/below `lowspeed_full_ms`, fading linearly
/// to 0 at `lowspeed_zero_ms`.
fn lowspeed_w(speed_ms: f64, p: &ScoringParams) -> f64 {
    let span = (p.lowspeed_zero_ms - p.lowspeed_full_ms).max(1e-6);
    ((p.lowspeed_zero_ms - speed_ms) / span).clamp(0.0, 1.0)
}

/// Flip-pause / transit speed weight: 1 up to `zero_ms - REGIME_FADE_SPAN_MS`,
/// fading linearly to 0 at `zero_ms`.
fn regime_w(speed_ms: f64, zero_ms: f64) -> f64 {
    ((zero_ms - speed_ms) / REGIME_FADE_SPAN_MS).clamp(0.0, 1.0)
}

/// Angle contribution: ramps 0->1 up to the sweet spot using `angle_power`
/// (steepened by `lowspeed_power_add` at crawl speeds — the below-sweet
/// low-speed depression, anchored so the sweet spot always pays 1.0), then
/// RISES to a `1.0 + above_sweet_rise` plateau at `rise_saturation_deg` (minus
/// any `above_sweet_decline` slope, kept as an override lever). Zero outside the
/// [min, spin] band — the drop to 0 past spin is the spin-out break.
fn angle_factor(angle_deg: f64, speed_ms: f64, p: &ScoringParams) -> f64 {
    if angle_deg < p.min_angle_deg || angle_deg > p.spin_angle_deg {
        return 0.0;
    }
    if angle_deg <= p.sweet_angle_deg {
        let ramp = (angle_deg / p.sweet_angle_deg).clamp(0.0, 1.0);
        let power = (p.angle_power + p.lowspeed_power_add * lowspeed_w(speed_ms, p)).max(1e-6);
        ramp.powf(power)
    } else {
        // Saturating rise: per-frame vision shows the game's credit climbs fast
        // just past the sweet spot and plateaus (~+8.5% by ~58°), holding flat
        // out past 100°.
        let rise_span = (p.rise_saturation_deg - p.sweet_angle_deg).max(1e-6);
        let rise_ramp = ((angle_deg - p.sweet_angle_deg) / rise_span).clamp(0.0, 1.0);
        // Linear decline from 1.0 at the sweet spot at a FIXED rate: it falls by
        // above_sweet_decline at 90° and continues at that same slope up to
        // spin_angle_deg (past which the guard above returns 0). Anchoring the
        // slope to 90° rather than to spin_angle_deg decouples "how fast credit
        // falls off" from "where a drift counts as spun out", so the cutoff can
        // be raised past 90° (the game scores slides to ~115°) without changing
        // the 45–90° band. Clamp at 0 for safety on extreme configs.
        const DECLINE_REF_DEG: f64 = 90.0;
        let span = (DECLINE_REF_DEG - p.sweet_angle_deg).max(1e-6);
        (1.0 + p.above_sweet_rise * rise_ramp
            - p.above_sweet_decline * (angle_deg - p.sweet_angle_deg) / span)
            .max(0.0)
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
pub(crate) fn frame_dt(prev_ms: u32, cur_ms: u32) -> f64 {
    let d = cur_ms as i64 - prev_ms as i64;
    if (0..100).contains(&d) {
        d as f64 / 1000.0
    } else {
        1.0 / 60.0
    }
}

/// Score a run from its packets in time order. Returns zeros for an empty run.
pub fn score_run(packets: &[TelemetryPacket], p: &ScoringParams) -> RunScore {
    score_run_inner(packets, p, None)
}

/// Like [`score_run`] but also returns one [`TickScore`] per input packet (same
/// order) — the authoritative per-tick diagnostics the run viewer overlays on
/// the map trace and timeline. The aggregate [`RunScore`] is byte-for-byte
/// identical to [`score_run`]'s (same arithmetic; the buffer is write-only).
pub fn score_run_with_ticks(
    packets: &[TelemetryPacket],
    p: &ScoringParams,
) -> (RunScore, Vec<TickScore>) {
    let mut ticks = Vec::with_capacity(packets.len());
    let result = score_run_inner(packets, p, Some(&mut ticks));
    (result, ticks)
}

fn score_run_inner(
    packets: &[TelemetryPacket],
    p: &ScoringParams,
    mut ticks: Option<&mut Vec<TickScore>>,
) -> RunScore {
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
    let mut score_sign = 0i8; // direction latched at the last *scoring* packet
    let mut direction_flips = 0usize;
    let mut prev_ms: Option<u32> = None;
    // Low-speed composite state. `total_time` doubles as the run clock the
    // pause window and dip lengths are measured on.
    let mut pause_until = -1.0_f64; // flip-pause window end (run clock)
    let mut pause_w = 0.0_f64; // suppression weight inside the window
    let mut transit_raw = 0.0_f64; // raw pts re-paid through dips
    let mut pause_raw = 0.0_f64; // raw pts withheld around flips
    // Last crediting packet: (index, run clock, raw rate pts/s, speed).
    let mut last_credit: Option<(usize, f64, f64, f64)> = None;

    for (idx, pkt) in packets.iter().enumerate() {
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
        // Per-tick diagnostics (write-only; never feed back into the score).
        let tarmac_wheels = tarmac_wheel_count(pkt);
        let on_tarmac_now = tarmac_wheels >= p.min_tarmac_wheels;
        let scoring_tick = drifting && (!p.require_tarmac_contact || on_tarmac_now);
        let mut tick_pts_raw = 0.0_f64;

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
            // Points accrue only with enough tyres on tarmac (when the gate is
            // on); off-track sliding still counts as drift time / continuity.
            if !p.require_tarmac_contact || on_tarmac_now {
                let rate =
                    p.base_rate * angle_factor(angle, speed, p) * speed_factor(speed, p) * multiplier;
                // Transit credit: resuming after >=1 non-crediting packet
                // re-pays the dip at the last in-band rate, speed-faded and
                // capped — the game banks through brief sub-gate dips at crawl.
                if let Some((li, lt, lrate, lspeed)) = last_credit {
                    if p.transit_gain > 0.0 && idx > li + 1 {
                        let w = regime_w(lspeed, p.transit_zero_ms);
                        if w > 0.0 {
                            let credit =
                                p.transit_gain * w * lrate * (total_time - lt).min(TRANSIT_CAP_S);
                            total += credit;
                            transit_raw += credit;
                            tick_pts_raw += credit;
                        }
                    }
                }
                if score_sign != 0 && sign != score_sign {
                    direction_flips += 1;
                    // Flip pause: suppress in-band credit (including this
                    // packet's) for flip_pause_s, speed-faded; back-to-back
                    // flips extend the window, the strongest weight wins.
                    if p.flip_pause_s > 0.0 {
                        let wp = regime_w(speed, p.flip_pause_zero_ms);
                        if wp > 0.0 {
                            pause_w = if total_time < pause_until { pause_w.max(wp) } else { wp };
                            pause_until = total_time + p.flip_pause_s;
                        }
                    }
                }
                score_sign = sign;
                let supp = if total_time < pause_until { pause_w } else { 0.0 };
                total += rate * (1.0 - supp) * dt;
                tick_pts_raw += rate * (1.0 - supp) * dt;
                pause_raw += rate * supp * dt;
                last_credit = Some((idx, total_time, rate, speed));
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

        if let Some(ts) = ticks.as_mut() {
            ts.push(TickScore {
                is_drifting: drifting,
                is_scoring: scoring_tick,
                tarmac_wheels,
                points: tick_pts_raw * p.scale,
                sector: 0,
            });
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
        direction_flips,
        transit_pts: transit_raw * p.scale,
        flip_pause_pts: pause_raw * p.scale,
        sectors: Vec::new(),
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

        assert!(angle_factor(23.0, 25.0, &params) > angle_factor(23.0, 25.0, &linear));
        assert!((angle_factor(params.sweet_angle_deg, 25.0, &params) - 1.0).abs() < 1e-9);
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
    fn direction_flips_count_signed_reversals_while_scoring() {
        let params = ScoringParams::default();
        // Weave: 1 s at +25°, 1 s at −25°, repeated — 3 reversals. A steady run
        // of the same length counts zero. (drifting_packet keeps all wheels on
        // tarmac, so every in-band packet scores.)
        let mut weave = Vec::new();
        for seg in 0..4 {
            let beta = if seg % 2 == 0 { 25.0 } else { -25.0 };
            for i in 0..60u32 {
                weave.push(at(drifting_packet(beta, 15.0), (seg * 60 + i) * 16));
            }
        }
        assert_eq!(score_run(&weave, &params).direction_flips, 3);

        let steady: Vec<_> = (0..240)
            .map(|i| at(drifting_packet(25.0, 15.0), i * 16))
            .collect();
        assert_eq!(score_run(&steady, &params).direction_flips, 0);

        // A sub-gate dip (β=0 for a stretch) between two same-direction segments
        // is not a flip; the same dip into the OPPOSITE direction is one.
        let mut dip_same = Vec::new();
        let mut dip_flip = Vec::new();
        for i in 0..60u32 {
            dip_same.push(at(drifting_packet(25.0, 15.0), i * 16));
            dip_flip.push(at(drifting_packet(25.0, 15.0), i * 16));
        }
        for i in 60..90u32 {
            dip_same.push(at(drifting_packet(0.0, 15.0), i * 16));
            dip_flip.push(at(drifting_packet(0.0, 15.0), i * 16));
        }
        for i in 90..150u32 {
            dip_same.push(at(drifting_packet(25.0, 15.0), i * 16));
            dip_flip.push(at(drifting_packet(-25.0, 15.0), i * 16));
        }
        assert_eq!(score_run(&dip_same, &params).direction_flips, 0);
        assert_eq!(score_run(&dip_flip, &params).direction_flips, 1);
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
    fn off_tarmac_pays_full_rate_outside_winter() {
        // The off-tarmac rule is seasonal: winter zeroes packets with too few
        // wheels on tarmac, every other season pays grass at FULL rate
        // (measured in spring: a heavy-grass run re-scored ungated matched the
        // game to −0.6% vs −15.1% under the winter gate, and per-moment vision
        // showed grass moments banking at the normal rate).
        let grass: Vec<_> = (0..180)
            .map(|i| at(all_wheels_off_tarmac(drifting_packet(40.0, 20.0)), i * 16))
            .collect();
        let tarmac: Vec<_> = (0..180)
            .map(|i| at(drifting_packet(40.0, 20.0), i * 16))
            .collect();
        let winter = ScoringParams::default().for_season(crate::season::Season::Winter);
        assert_eq!(score_run(&grass, &winter).score, 0.0, "winter keeps the gate");
        let spring = ScoringParams::default().for_season(crate::season::Season::Spring);
        let grass_score = score_run(&grass, &spring).score;
        let tarmac_score = score_run(&tarmac, &spring).score;
        assert!(grass_score > 0.0, "spring grass must bank");
        assert!(
            (grass_score - tarmac_score).abs() < 1e-6,
            "spring grass pays the same as tarmac ({grass_score} vs {tarmac_score})"
        );
    }

    #[test]
    fn one_wheel_on_tarmac_does_not_score() {
        // The game needs TWO wheels down to bank (play-tested); a single wheel
        // clipping the tarmac earns nothing, whichever wheel it is.
        let params = ScoringParams::default();
        for wheel in 0..4 {
            let on_one = |i: u32| {
                let mut p = all_wheels_off_tarmac(drifting_packet(40.0, 20.0));
                match wheel {
                    0 => p.surface_rumble_fl = 0.0,
                    1 => p.surface_rumble_fr = 0.0,
                    2 => p.surface_rumble_rl = 0.0,
                    _ => p.surface_rumble_rr = 0.0,
                }
                at(p, i * 16)
            };
            let pkts: Vec<_> = (0..180).map(on_one).collect();
            let r = score_run(&pkts, &params);
            assert_eq!(r.score, 0.0, "one wheel (idx {wheel}) on tarmac must not score");
            // It still counts as drift time / sliding for the breakdown.
            assert!(r.drift_time_s > 2.5);
        }
    }

    #[test]
    fn two_wheels_on_tarmac_score_fully() {
        // Two wheels down earn the same as all four — the gate is a threshold,
        // not proportional — and front vs rear pairs make no difference.
        let params = ScoringParams::default();
        let pairs: [(fn(&mut TelemetryPacket), &str); 2] = [
            (
                |p| {
                    p.surface_rumble_fl = 0.0;
                    p.surface_rumble_fr = 0.0;
                },
                "front pair",
            ),
            (
                |p| {
                    p.surface_rumble_rl = 0.0;
                    p.surface_rumble_rr = 0.0;
                },
                "rear pair",
            ),
        ];
        let all_on: Vec<_> = (0..180).map(|i| at(drifting_packet(40.0, 20.0), i * 16)).collect();
        let r_on = score_run(&all_on, &params);
        for (put_down, label) in pairs {
            let pkts: Vec<_> = (0..180)
                .map(|i| {
                    let mut p = all_wheels_off_tarmac(drifting_packet(40.0, 20.0));
                    put_down(&mut p);
                    at(p, i * 16)
                })
                .collect();
            let r = score_run(&pkts, &params);
            assert!(r.score > 0.0, "{label} on tarmac must score");
            assert!(
                (r.score - r_on.score).abs() < 1e-6,
                "{label} must earn the same as all four wheels down"
            );
        }
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
        assert!(angle_factor(11.0, 25.0, &params) > 0.0);
        // ...and a 9° slide is still below the gate.
        assert_eq!(angle_factor(9.0, 25.0, &params), 0.0);
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
        assert!(angle_factor(100.0, 25.0, &params) > 0.0);
        assert_eq!(angle_factor(130.0, 25.0, &params), 0.0);
        // Raising the cutoff must NOT change the 45–90° band: the decline is
        // anchored at 90° and 90° is past the rise plateau, so factor(90°) is
        // exactly 1 + above_sweet_rise − above_sweet_decline.
        let expect = 1.0 + params.above_sweet_rise - params.above_sweet_decline;
        assert!((angle_factor(90.0, 25.0, &params) - expect).abs() < 1e-9);
    }

    #[test]
    fn default_angle_factor_rises_to_plateau_above_sweet() {
        // Per-frame vision ground truth (9 runs / 4 cars / 4 zones): the game's
        // steep-angle credit climbs fast past the 45° sweet spot and saturates
        // at ~+8.5% by ~58°, holding that plateau out past 100°. The factor is
        // exactly 1.0 at the sweet spot, halfway up mid-ramp, and flat at
        // 1 + above_sweet_rise from the saturation angle to the spin cutoff.
        let params = ScoringParams::default();
        assert_eq!(params.above_sweet_decline, 0.0);
        assert_eq!(params.above_sweet_rise, 0.085);
        assert_eq!(params.rise_saturation_deg, 58.0);
        assert!((angle_factor(45.0, 25.0, &params) - 1.0).abs() < 1e-9);
        let mid = 1.0 + params.above_sweet_rise * 0.5;
        assert!((angle_factor(51.5, 25.0, &params) - mid).abs() < 1e-9);
        let plateau = 1.0 + params.above_sweet_rise;
        for a in [58.0, 75.0, 90.0, 115.0] {
            assert!((angle_factor(a, 25.0, &params) - plateau).abs() < 1e-9,
                    "expected plateau {plateau} at {a}°, got {}", angle_factor(a, 25.0, &params));
        }
    }

    #[test]
    fn lowspeed_steepening_depresses_shallow_but_anchors_sweet() {
        // Below-sweet credit steepens at crawl speed by lowspeed_power_add
        // ((a/45)^0.22 extra at full depth); the sweet spot and everything
        // above it pay full rate at ANY speed — the 45° anchor (#521: 45–60°
        // reads 1.00 at 5–9 m/s while 10–45° is depressed).
        let p = ScoringParams::default();
        let lo = angle_factor(20.0, 3.0, &p);
        let hi = angle_factor(20.0, 25.0, &p);
        let expect = (20.0_f64 / 45.0).powf(p.lowspeed_power_add);
        assert!((lo / hi - expect).abs() < 1e-9, "ratio {} vs {}", lo / hi, expect);
        // Fade: gone at/above lowspeed_zero_ms, partial in between.
        assert!((angle_factor(20.0, p.lowspeed_zero_ms, &p) - hi).abs() < 1e-9);
        let mid = angle_factor(20.0, 9.5, &p);
        assert!(mid > lo && mid < hi);
        // The anchor: 45° and the rise plateau are speed-independent.
        for a in [45.0, 50.0, 58.0, 90.0] {
            assert!(
                (angle_factor(a, 3.0, &p) - angle_factor(a, 25.0, &p)).abs() < 1e-9,
                "above-sweet must not depend on speed (a={a})"
            );
        }
    }

    #[test]
    fn transit_credit_repays_brief_dips_at_crawl_only() {
        // drift → 0.3 s sub-gate dip → drift, at 6 m/s: the dip is re-paid at
        // last rate × transit_gain × wt. The same shape at 25 m/s gets nothing
        // (the credit fades out by transit_zero_ms).
        let p = ScoringParams::default();
        fn run_at(speed: f32) -> Vec<TelemetryPacket> {
            let mut pkts = Vec::new();
            let mut t = 0u32;
            for _ in 0..128 {
                pkts.push(at(drifting_packet(25.0, speed), t));
                t += 16;
            }
            for _ in 0..19 {
                pkts.push(at(drifting_packet(0.0, speed), t)); // sub-gate dip
                t += 16;
            }
            for _ in 0..128 {
                pkts.push(at(drifting_packet(25.0, speed), t));
                t += 16;
            }
            pkts
        }
        let slow = score_run(&run_at(6.0), &p);
        assert!(slow.transit_pts > 0.0, "crawl dip must be re-paid");
        // gain × wt × last_rate × gap: wt = (11−6)/6, gap = 20 frames ≈ 0.32 s.
        let af = angle_factor(25.0, 6.0, &p);
        let lrate = p.base_rate * af * speed_factor(6.0, &p) * p.scale;
        let expect = p.transit_gain * (5.0 / 6.0) * lrate * 0.320;
        assert!(
            (slow.transit_pts - expect).abs() / expect < 0.05,
            "transit {} vs expected {}",
            slow.transit_pts,
            expect
        );
        let fast = score_run(&run_at(25.0), &p);
        assert_eq!(fast.transit_pts, 0.0, "no transit credit at normal speed");
        // No resumption ⇒ no credit: a run that ends mid-dip banks nothing.
        let mut tail = run_at(6.0);
        tail.truncate(128 + 10);
        assert_eq!(score_run(&tail, &p).transit_pts, 0.0);
    }

    #[test]
    fn flip_pause_withholds_credit_at_crawl_only() {
        // A weave flip at 6 m/s suppresses the next flip_pause_s of in-band
        // credit (wp = (16−6)/6 → clamped 1.0 here); the same flip at 25 m/s
        // costs nothing.
        let p = ScoringParams::default();
        fn weave_at(speed: f32) -> Vec<TelemetryPacket> {
            let mut pkts = Vec::new();
            let mut t = 0u32;
            for _ in 0..128 {
                pkts.push(at(drifting_packet(25.0, speed), t));
                t += 16;
            }
            for _ in 0..128 {
                pkts.push(at(drifting_packet(-25.0, speed), t));
                t += 16;
            }
            pkts
        }
        let slow = score_run(&weave_at(6.0), &p);
        assert_eq!(slow.direction_flips, 1);
        assert!(slow.flip_pause_pts > 0.0, "crawl flip must pause banking");
        // ~flip_pause_s of full-rate credit withheld (weight 1 at 6 m/s; the
        // 64 Hz grid quantizes the window to ±1 frame).
        let af = angle_factor(25.0, 6.0, &p);
        let rate = p.base_rate * af * speed_factor(6.0, &p) * p.scale;
        let expect = rate * p.flip_pause_s;
        assert!(
            (slow.flip_pause_pts - expect).abs() / expect < 0.45,
            "pause {} vs ~{}",
            slow.flip_pause_pts,
            expect
        );
        let fast = score_run(&weave_at(25.0), &p);
        assert_eq!(fast.direction_flips, 1);
        assert_eq!(fast.flip_pause_pts, 0.0, "no pause at normal speed");
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

    #[test]
    fn ticks_align_with_predicates_and_reconstruct_score() {
        // score_run_with_ticks emits one TickScore per packet, in order; the
        // per-tick flags match the scoring predicates exactly; and the per-tick
        // points sum back to the run score (the buffer never alters the math).
        let p = ScoringParams::default();
        let mut pkts = Vec::new();
        let mut ms = 0u32;
        for _ in 0..60 {
            pkts.push(at(drifting_packet(40.0, 20.0), ms)); // banking (on tarmac)
            ms += 16;
        }
        for _ in 0..30 {
            pkts.push(at(all_wheels_off_tarmac(drifting_packet(40.0, 20.0)), ms)); // drifting, gated off
            ms += 16;
        }
        for _ in 0..30 {
            pkts.push(at(drifting_packet(0.0, 20.0), ms)); // straight: idle
            ms += 16;
        }
        for _ in 0..60 {
            pkts.push(at(drifting_packet(40.0, 20.0), ms)); // banking again
            ms += 16;
        }

        let (agg, ticks) = score_run_with_ticks(&pkts, &p);
        assert_eq!(ticks.len(), pkts.len());
        // Aggregate is identical to the plain scorer.
        assert_eq!(agg.score, score_run(&pkts, &p).score);
        // Flags line up packet-for-packet with the predicates.
        for (pkt, t) in pkts.iter().zip(&ticks) {
            assert_eq!(t.is_drifting, is_drifting(pkt, &p));
            assert_eq!(t.is_scoring, is_scoring_packet(pkt, &p));
            assert_eq!(t.tarmac_wheels, tarmac_wheel_count(pkt));
        }
        // The grass stretch is drifting but unpaid; the straight is neither.
        assert!(ticks[70].is_drifting && !ticks[70].is_scoring);
        assert_eq!(ticks[70].tarmac_wheels, 0);
        assert!(!ticks[100].is_drifting);
        // Per-tick points reconstruct the run total.
        let cum: f64 = ticks.iter().map(|t| t.points).sum();
        assert!(agg.score > 0.0);
        assert!(
            (cum - agg.score).abs() <= agg.score.abs() * 1e-9 + 1e-6,
            "cumulative tick points {cum} vs score {}",
            agg.score
        );
    }

    /// Low-speed composite parity vs the Python mirror on real packet data:
    /// prints raw (pre-scale) totals for the key crawl/burnout specimens so a
    /// param change can be cross-checked against score_audio_timeline.py
    /// (which must reproduce these to float precision). Reads the developer's
    /// local DB read-only; ignored by default.
    /// Run: cargo test --lib -- --ignored lowspeed_composite_parity --nocapture
    #[test]
    #[ignore = "reads the local sessions.db"]
    fn lowspeed_composite_parity_specimens() {
        let conn = rusqlite::Connection::open(crate::db::db_path()).unwrap();
        let winter = ScoringParams::default().for_season(crate::season::Season::Winter);
        let spring = ScoringParams::default().for_season(crate::season::Season::Spring);
        // (#457/#322/#385 are winter-era runs; #513/#521 are spring.)
        for (run_id, p) in [
            (457i64, &winter),
            (513, &spring),
            (521, &spring),
            (322, &winter),
            (385, &winter),
        ] {
            let blobs = crate::db::get_drift_run_packets(&conn, run_id).unwrap();
            let pkts: Vec<_> = blobs
                .iter()
                .filter_map(|b| crate::parser::parse(b).ok())
                .collect();
            let r = score_run(&pkts, p);
            println!(
                "run#{run_id} raw={:.1} score={:.0} transit={:.0} pause={:.0} flips={}",
                r.score / p.scale,
                r.score,
                r.transit_pts,
                r.flip_pause_pts,
                r.direction_flips
            );
        }
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

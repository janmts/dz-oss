//! FH6 festival season schedule + the season-dependent scoring rule.
//!
//! Seasons rotate weekly every **Thursday 14:30 UTC**. The anchor was measured
//! in-game: the winter→spring flip landed at 2026-06-11 14:30 UTC exactly on
//! the scheduled boundary (the last winter run started 14:28 UTC, the first
//! spring run 14:33 UTC).
//!
//! Why scoring cares: the off-tarmac credit rule is SEASON-DEPENDENT. In
//! winter a drift packet with too few tyres on tarmac banks NOTHING (the
//! tarmac gate, `ScoringParams::require_tarmac_contact`). In spring the game
//! pays grass at FULL RATE — a heavy-grass spring run scored −15.1% under the
//! winter gate but −0.6% ungated, and per-moment vision confirmed grass
//! moments banking at the normal rate. Summer and autumn are UNMEASURED until
//! they arrive; they are assumed spring-like (paying) on the reading that
//! winter's zeroing is snow burying the off-road surface — flip
//! [`Season::off_tarmac_pays`] if play-testing the first summer week
//! (2026-06-18) says otherwise.
//!
//! Season is provably OUT-OF-BAND: a full float sweep over same-car runs
//! straddling the boundary found no in-packet separator (the per-wheel
//! surface enum is byte-identical across seasons), so the binding is a run's
//! wall-clock start time against this schedule — deterministic, derived on
//! demand, never stored. If the schedule ever shifts, fixing it here and
//! hitting Recompute rebinds every stored run consistently.

/// First instant of the measured SPRING anchor week (2026-06-11 14:30:00 UTC).
pub(crate) const SPRING_ANCHOR_MS: i64 = 1_781_188_200_000;
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    pub fn as_str(self) -> &'static str {
        match self {
            Season::Spring => "spring",
            Season::Summer => "summer",
            Season::Autumn => "autumn",
            Season::Winter => "winter",
        }
    }

    /// Whether off-tarmac surfaces bank drift points this season. Winter
    /// zeroes them (measured); spring pays full rate (measured); summer and
    /// autumn are assumed paying until their first week arrives.
    pub fn off_tarmac_pays(self) -> bool {
        !matches!(self, Season::Winter)
    }
}

/// The in-game season at a wall-clock instant (Unix epoch ms). Extrapolates
/// the weekly Thursday-14:30-UTC rotation in both directions from the
/// measured spring anchor.
pub fn season_at_utc_ms(utc_ms: i64) -> Season {
    let weeks = (utc_ms - SPRING_ANCHOR_MS).div_euclid(WEEK_MS);
    match weeks.rem_euclid(4) {
        0 => Season::Spring,
        1 => Season::Summer,
        2 => Season::Autumn,
        _ => Season::Winter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_week_is_spring_and_boundary_is_exact() {
        assert_eq!(season_at_utc_ms(SPRING_ANCHOR_MS), Season::Spring);
        // One ms before the boundary is still the previous (winter) week.
        assert_eq!(season_at_utc_ms(SPRING_ANCHOR_MS - 1), Season::Winter);
        // End of the spring week / start of the next.
        assert_eq!(season_at_utc_ms(SPRING_ANCHOR_MS + WEEK_MS - 1), Season::Spring);
        assert_eq!(season_at_utc_ms(SPRING_ANCHOR_MS + WEEK_MS), Season::Summer);
    }

    #[test]
    fn rotation_cycles_through_four_seasons() {
        for (weeks, expect) in [
            (0, Season::Spring),
            (1, Season::Summer),
            (2, Season::Autumn),
            (3, Season::Winter),
            (4, Season::Spring),
            (-1, Season::Winter),
            (-2, Season::Autumn),
        ] {
            assert_eq!(
                season_at_utc_ms(SPRING_ANCHOR_MS + weeks * WEEK_MS),
                expect,
                "{weeks} weeks from anchor"
            );
        }
    }

    #[test]
    fn only_winter_withholds_off_tarmac_credit() {
        assert!(!Season::Winter.off_tarmac_pays());
        assert!(Season::Spring.off_tarmac_pays());
        assert!(Season::Summer.off_tarmac_pays());
        assert!(Season::Autumn.off_tarmac_pays());
    }

    #[test]
    fn known_runs_bind_to_their_measured_seasons() {
        // Run #479 (last winter run) started 2026-06-11 14:28 UTC; run #480
        // (first spring run) 14:33 UTC — two minutes either side of the flip.
        assert_eq!(season_at_utc_ms(1_781_188_080_000), Season::Winter);
        assert_eq!(season_at_utc_ms(1_781_188_380_000), Season::Spring);
    }
}

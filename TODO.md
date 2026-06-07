# Drift Zone Tracker — Roadmap / TODO

Running list of features, fixes, and ideas so nothing gets lost between sessions.
Checked items are done; the rest are roughly in priority order.

---

## ✅ Done (this work)

- **Computed drift score engine** (`src-tauri/src/scoring.rs`): drift angle from
  car-local velocity `atan2(velX, velZ)`, scored as `angle × speed × combo`
  integrated over time. Combo is **transition-aware** — a flick (dip that
  resumes the opposite direction within `transition_grace_s`) keeps the chain; a
  straighten breaks it.
- **Score storage + display**: `computed_score` + `score_breakdown_json` per run;
  dashboard shows computed vs actual, breakdown (drift time, angle, multiplier,
  flicks), and run timestamps. **Recompute** button re-scores all runs from
  stored packets.
- **Calibration**: scale fit by least-squares to logged in-game scores
  (~9.8% mean error, ~18% worst, over 8 runs).
- **Zone detection**:
  - Per-zone **boundary slack** (default 3 m, tunable) — tolerates straying past
    the edge so good runs aren't killed by a minor clip.
  - **Bidirectional, precise gate-crossing entry** — enter through either end
    gate (between its two points, like the in-game flags), finish on the other.
  - **End gates always derived** from the current boundary (no stale stored
    state).

---

## 🔜 A. Zone authoring tooling — *highest priority*
*(Need to map all the drift zones before recorded-run viewing is worthwhile.)*

- [ ] **A1. Click-to-add boundary points.** Click the map in the editor to drop a
  point on the active side, instead of only capturing the car's live position.
  The `latLng↔world` transforms already exist in `DriftZoneEditor.svelte`; this
  is mostly a `map.on('click')` handler appending to the active boundary.
- [ ] **A2. Auto-capture / "record mode".** Drive the edge and auto-drop a point
  every ~N metres (or seconds), instead of one `Ctrl+Alt+Z` per point. Builds on
  A1's plumbing.

---

## 📊 B. Scoring finetuning — *ongoing, data-driven*

- [ ] **Log more runs + enter actual in-game scores**, then re-run the tuning
  harness (`scripts/score_model.py`, local/gitignored) and adjust params in
  `ScoringParams::default`.
- [ ] **Speed weighting** — currently a linear factor capped at 31 m/s (~70 mph),
  co-equal with angle. Can't be calibrated yet: all logged runs sit at
  16–21 m/s avg (never near the cap) and speed↔angle are confounded. **Record
  AWD / higher-speed runs** (ideally some fast-but-low-angle) to break the
  confound, then revisit the cap/curve.
- [ ] **Angle curve** — data currently hints the 30–36° band is slightly
  over-weighted (residual vs angle correlation ≈ −0.78). Revisit once more runs
  exist; don't overfit the current 8 points.
- [ ] **Multiplier curve** (`mult_growth_per_s`, `mult_cap`, `transition_grace_s`)
  — tune as more flick-heavy and varied runs come in.
- [ ] Reminder: **click Recompute** in the drift dashboard after any param change
  to refresh stored scores.

---

## 🎬 C. Recorded-run viewing — *after zones are mapped*

Drift runs store the **same** raw packets as time-attack sessions, so the
existing analysis/replay machinery is reusable.

- [ ] **Expose drift-run packets to the frontend** — one mirror of the session
  path: `api::drift_run_packets() -> Vec<TelemetryPacket>`, a `get_drift_run_packets`
  command, an HTTP route, and `ipc.getDriftRunPackets`. (Backend
  `db::get_drift_run_packets` already exists.)
- [ ] **Reuse `AnalysisTab` / `MapPanel` / `startReplay`** for a drift run (replay
  is packet-driven, not session-bound — should "just work").
- [ ] *Bonus, drift-specific:*
  - [ ] Multi-run **overlay comparison** (several runs of the same zone vs your best).
  - [ ] Add **drift angle β, per-tick score-rate, multiplier** as graph metrics —
    a "why did this run score X" visualizer.
  - [ ] **Score-rate heatmap** on the map trace (where you're banking points).

---

## 🧩 Open questions / smaller ideas

- [ ] **Live ticking score** during a run (like the in-game counter) — deferred
  from v1; revisit if wanted.
- [ ] Should **invalid / early-exit runs** count anywhere, or stay reference-only?
- [ ] **Per-zone scoring-param UI** (beyond the slack field) — params are already
  overridable via `scoring_config_json`, just no UI yet.

---

## 🛠️ Dev notes

- **Tuning harness** (local, gitignored, in `scripts/`): `score_model.py`
  (fit scale + per-run computed-vs-actual error), `inspect_drift.py`
  (per-run telemetry stats). These read the local `sessions.db` directly.
- **Scoring params**: `ScoringParams::default` in `src-tauri/src/scoring.rs`;
  per-zone overrides live in `drift_zones.scoring_config_json`
  (e.g. `boundarySlackM`).
- **FH6 telemetry gotchas** (baked into code, noted here so they're not
  re-discovered): velocity is **car-local** (not world); `tireSlip*` fields are
  **normalized** (~1.0 = grip limit), not radians; packet timestamps contain
  duplicate stamps (Δ=0).

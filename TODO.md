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

- [x] **A1. Click-to-add boundary points.** Click the map in the editor to drop a
  point on the active side, instead of only capturing the car's live position.
  The `latLng↔world` transforms already exist in `DriftZoneEditor.svelte`; this
  is mostly a `map.on('click')` handler appending to the active boundary.
- [ ] **A2. Auto-capture / "record mode".** Drive the edge and auto-drop a point
  every ~N metres (or seconds), instead of one `Ctrl+Alt+Z` per point. Builds on
  A1's plumbing.

---

## 📊 B. Scoring finetuning — *ongoing, data-driven*

> **Zone id ↔ name** (the deleted "failed" zone was id **1**; ids 2/3/4 never
> renumbered, so the in-app list of 3 is ids 2–4):
> | zone_id | name | valid runs | notes |
> |---|---|---|---|
> | **2** | TOkyo | 46 | mid-speed |
> | **3** | RED MOUNTAIN | 73 | best-covered; high-speed |
> | **4** | Box touge | 13 | **thinnest** — long, technical, low-speed |

**Current status (2026-06-08):** 110 valid scored runs across **5 cars & 3 zones**
(the calibration notes further down started at *8 runs / 1 zone* — kept as a
record of the journey). Shipped model is no-combo, `angle_power=0.10`,
`scale=10.986` → **MAE 2.79%, max 12.8%, bias −0.9%**. Speed term is settled; the
**angle curve is the live lever** (its optimum has slid 0.5→0.4→0.20→0.10 as
shallower cars landed). Active to-dos are the **B0** checklist; the bullets below
are the original breadcrumbs, annotated with how each resolved.

### B0. Data-gathering checklist (2026-06-08, at 110 valid scored / 5 cars)

*Roughly highest-information first. Goal is to break confounds & pin curve ends, not just add volume.*

- [ ] **Isolate uphill effect — matched shallow Transit (ord 1477) up vs down on RED MOUNTAIN (z3).** ~4–5 each direction. Every sub-18° run today is uphill, so we can't tell if the −10–13% on #135/#137 is *grade* or just *shallow angle*. This answers it.
- [ ] **Get the new S2 AWD (ord 3865) out of zone 3.** It's 17/19 in RED MOUNTAIN (z3) — drive it in **TOkyo (z2, has none)** and **Box touge (z4)**, ~8–10 runs. Decouples its −3.4% car bias from zone 3's −2%.
- [ ] **More Transit (1477) in TOkyo (z2) & RED MOUNTAIN (z3)**, both modes (long shallow sweeps *and* thrown-around). Currently only 4 / 6 there.
- [ ] **Fill the shallow tail (<22° avg) across *multiple* cars**, not just the Transit — tests whether `angle_power=0.10` has stabilized or keeps sliding toward a gate.
- [ ] **Fill the steep tail** — lean on the S2 AWD's high-angle runs; there's ~zero run-avg support above ~40°, so the at/above-sweet curve is unconstrained.
- [ ] **Box touge (z4) volume** — thinnest zone at 13 valid (≤4/car). A dozen more across cars de-risks the "fits one global scale" claim.
- [ ] **Let OCR keep capturing** — validated indistinguishable from hand-entered; free volume while you target the above.
- [ ] After ~20–30 new runs: re-run `score_deepdive.py` / `score_probe.py`, check if the angle-curve optimum moved again, refit scale, **Recompute**.

- [ ] **Log more runs + enter actual in-game scores**, then re-run the tuning
  harness (`scripts/score_model.py`, local/gitignored) and adjust params in
  `ScoringParams::default`.
- [x] **Speed weighting** — currently a linear factor capped at 31 m/s (~70 mph),
  co-equal with angle. Can't be calibrated yet: all logged runs sit at
  16–21 m/s avg (never near the cap) and speed↔angle are confounded. **Record
  AWD / higher-speed runs** (ideally some fast-but-low-angle) to break the
  confound, then revisit the cap/curve.
  → **Resolved 2026-06-08:** did exactly this. Cap is now `speed_cap_ms=60`;
  runs span ~13–41 m/s avg across zones. Speed term re-confirmed optimal (linear,
  `speed_power=1.0`, cap doesn't bind). The apparent speed↔err signal was the
  angle curve in disguise — it shrank as `angle_power` dropped.
- [x] **Angle curve** — data currently hints the 30–36° band is slightly
  over-weighted (residual vs angle correlation ≈ −0.78). Revisit once more runs
  exist; don't overfit the current 8 points.
  → **Superseded 2026-06-08:** that −0.78 was 8 Tokyo-only runs under the *old*
  (buggy, yaw-based) angle formula — not comparable to today. Since the
  car-local-velocity fix the angle curve has been the main lever, retuned
  0.5→0.4→0.20→0.10; on 110 runs the residual-vs-angle correlation is ~+0.04
  (nulled) at `angle_power=0.10`. Still live — see B0.
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

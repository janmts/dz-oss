# Drift-zone editor — targeted rebuild

A living backlog for rebuilding `DriftZoneEditor.svelte` and laying the **curved
zone geometry** foundation under it. Decisions locked 2026-06-14; jot rough notes
as you use the new editor.

**Why now.** Under the old "only gates matter" assumption, mapping a zone was one
rough click while driving — the editor was fine for that. The hidden **scoreable-
zone position gate** is now validated (see the Haruna finding), so zone geometry
will gate scoring per-tick in a future scorer, the boundaries need to be real
curves, and the editor becomes a heavy-use sculpting tool. The current editor is
clunky enough that mapping accurate scoring zones in it would be miserable, so it
gets a targeted rebuild first.

Key files: `src/lib/components/DriftZoneEditor.svelte` (the rebuild), `src/lib/
mapView.ts` (`createGameMap`/`addLine` — reused), `src/lib/curve.ts` (NEW shared
tessellator), `src/lib/types.ts` (`ZonePoint`/`DriftZoneInput`), backend
`src-tauri/src/drift.rs` (`RunnableZone::from_row`, the geometry helpers + the
Rust `tessellate` mirror).

---

## Geometry model (the foundation — "anchors are truth, points are a view")

- Store ONLY the sparse hand-placed anchors + a `curve: 'linear' | 'catmull'`
  flag. NEVER persist baked dense points — that's what makes re-editing messy.
- Display AND the Rust scorer call ONE shared **centripetal Catmull-Rom (α=0.5)**
  tessellator on the same anchors ⇒ display == scored by construction. Mirror the
  routine JS↔Rust and pin it with a shared golden-fixture parity test.
- Tessellate once per run (cache it); per-tick is just `point_in_polygon` — trivial
  at the 64 Hz tick.
- All config rides in `scoringConfig` (the JSON bag that already holds
  `boundarySlackM`) ⇒ **no DB migration**.
- Rejected: bake-at-save (lossy/messy re-edit); draggable bézier handles (overkill
  — needs ZonePoint + IPC + DB + Rust changes for nothing a chord-testing backend
  consumes).

## Two shapes, separate jobs

- **left/right boundary** (two OPEN polylines) — unchanged role: detects run
  start/finish via the derived end gates (`drift.rs` builds `left ++ reversed(right)`
  and derives gates from the endpoints).
- **scoring ring** (NEW, one CLOSED polygon) — the per-tick position gate. Tighter
  than the painted flags; optionally DIRECTED per entry gate (`byGate.a/b`).
  Optional per-zone ⇒ absent = today's non-positional scoring, so it rolls out one
  zone at a time as each is mapped against the run viewer. Seed it from the
  boundary (`left ++ reversed(right)`), then inset/drag/curve. Parametric
  `mode:'inset'` { lateralM, entry/exit per gate } is the quick "pull it inward"
  on-ramp before hand-mapping.

---

## Pain points being fixed (current editor)

- [x] **Insert dumps the live point on top of the selection.** ✅ `insertOnCurve`
      now places a point ON the drawn curve at the midpoint of the segment next to
      the selection (chord midpoint for linear/<3 anchors), pre-selected & draggable.
- [x] **Every edit yanks the camera.** ✅ `fitMapToGeometry` runs ONLY on zone-select
      (first load) + the explicit Fit button; the stray capture/insert fit calls are
      gone, so the camera stays where the user put it while sculpting.
- [x] **"Remove last L/R" nukes a gate anchor.** ✅ Replaced by **Delete selected**
      (toolbar + Delete key → `deletePoint(selectedPoint)`); `removeLastPoint` deleted.
- [x] **No undo/redo.** ✅ Snapshot stack of draft geometry + structural state
      (anchors/ring/splits/curve/selection), pushed before each mutating op + marker
      `dragstart`; Ctrl+Z / Ctrl+Y (Ctrl+Shift+Z), guarded against text-field undo;
      cleared on zone-select/new/save. 100-entry cap.
- [x] **Split gates can't be authored.** ✅ Step 5 below — `'split'` target + gate
      sub-selector.

## Layout & UX

- [x] **Map is the hero.** ✅ Metadata (name/description/slack/active) moved into the
      sidebar under the zone list; the map fills the main panel (flex:1).
- [x] **Grouped toolbar** ✅ hairline-divider groups: *Target* segmented
      (`Left · Right · Ring · Split`) · *Point ops* (`Insert · Delete · Undo · Redo`)
      · *View* (`Fit · Smooth · Seed ring`) · *Capture* (`Capture live` + readout,
      de-emphasised, right-aligned). (Smooth sits in View per the spec; it's the
      persisted curve toggle = display==scored, not a transient view-only toggle.)
- [x] **Replace the long data boxes** ✅ compact inline status chips
      (`L 6 · R 5 · ring 12 · split 0` + selected-point label).
- [x] **Drop the SVG fallback** ✅ removed entirely (svgEl/dragging/toSvg/fromClient/
      path/transform + all SVG markup/CSS); replaced with a calibration guard that
      links to Settings → Calibrate map via a new `onOpenSettings` prop wired in
      `+page.svelte`. (`DriftZoneMap` still has its inherited fallback — clean
      separately; see Deferred.)

## Backend touch (Stage 1 only)

- [ ] `RunnableZone::from_row` tessellates the boundary (when `curve` is set)
      BEFORE building the entry polygon/gates, so run-start detection matches the
      drawn curve. Cache per zone-list change, not per packet.

---

## Build order

1. [x] **`tessellate()` — JS (`src/lib/curve.ts`) + Rust (`drift.rs`) + shared
       golden-value parity test** (`scripts/check-curve.mjs` ↔
       `drift.rs::tests::tessellate_matches_golden_and_invariants`). ✅
2. [x] Editor shell rebuild: layout (sidebar metadata, grouped toolbar, status
       chips), drop SVG fallback → calibration guard, stable camera, undo/redo,
       insert-on-curve, delete-selected. ✅ See Shipped.
3. [x] Curve flag wiring (display + save + `from_row`). ✅ A "Smooth" toggle in the
       editor flips `scoringConfig.curve`; boundaries render the centripetal curve
       on all 3 surfaces (editor / DriftZoneMap / RunMap) with markers staying on
       the raw anchors, and `RunnableZone::from_row` tessellates the entry polygon
       to match (gates derive from endpoints, which tessellation preserves).
4. [x] Scoring-ring target + seed-from-boundary + violet render. ✅ Closed ring
       authored via a "Ring" target (reuses the point-edit machinery) + a "Seed
       ring" button (`left ++ reversed(right)`); renders on editor / DriftZoneMap /
       RunMap; persists in `scoringConfig.scoringRegion.anchors` (no migration);
       the Smooth toggle curves it (closed). Adversarially reviewed (no bugs;
       5 fixes applied). Backend does NOT score it yet (Stage 2).
5. [x] Split target. ✅ `'split'` joins the segmented target; splits are a COLLECTION
       (`draft.splitGates: ZonePoint[][]`, each gate exactly 2 points) so it gets a
       gate sub-selector (`gate 1 · gate 2 · + Add gate · Delete gate`) — multi-gate
       per the user's call. `selectedSplit` indexes the active gate; map-click /
       Capture place its 2 points (capped, auto-create on first), dashed-violet line
       + `1a/1b` markers, dblclick/Delete removes the whole gate, incomplete gates
       pruned on save. Backend already stores `split_gates_json` (no change needed).

## Deferred

- [ ] **P3 — editor keybinds.** Rebindable shortcuts for the editor ops, and
      replacing the awkward hardcoded `ctrl+alt` global-capture combos.
- [ ] **Stage 2 — the position gate.** Add `&& point_in_scoring_region(pos)` to the
      scoring predicate; reuses the Stage-1 tessellation, per-zone rollout.
      **Ring scorer contract (from the step-4 review):** the per-tick test MUST
      branch on the zone's `curve` mode exactly like `ringCurve` / `from_row` —
      `tessellate({closed:true})` only when `'catmull'`, raw anchors when
      `'linear'` — and build `point_in_polygon` over that WITHOUT a closing dup
      (the ray-cast wraps last→first). Unconditionally tessellating a linear ring
      would score an area that diverges from the violet outline the user drew. Add
      a ring-anchor-driven closed-tessellation golden parity case then.
      **Entry-gate selection:** the backend already determines the entry gate at run
      start (`crossed_a`/`crossed_b`, drift.rs:328-338) but keeps only
      `finish_gate` on `ActiveRun`; record the entry gate (A/B) there (~1 line) so
      the per-tick test can pick `byGate?.[entry] ?? anchors`.
- [ ] **Directed (per-entry-gate) scoring rings — CONFIRMED requirement.** A zone
      needs up to 2 rings, each bound to an entry gate. Storage is already
      free-form (opaque `scoringConfig` JSON, no migration ever). Locked shape:
      `scoringRegion: { anchors?: ZonePoint[]; byGate?: { a?: ZonePoint[]; b?: ZonePoint[] } }`
      — reader picks `byGate?.[entryGate] ?? anchors`, so today's `{ anchors }`
      zones stay valid as the shared/both-gates ring (nothing to re-author). Gate
      A/B are defined by boundary order (A = first points, B = last), so "Reverse
      direction" must swap the directed rings too (or bind rings to gate coords,
      not order). Frontend then needs an `a | b | both` sub-target picker; the
      backend reader/scorer is Stage 2.
- [x] **Ring colour token.** ✅ The ring now has its own `--scoring-ring` (#cf72e0
      orchid), distinct from the muted `--gate-split` violet the splits keep; applied
      on all 3 surfaces (editor / DriftZoneMap / RunMap). Also (user ask, 2026-06-14)
      brightened the boundary tokens for at-a-glance legibility on terrain:
      `--map-left` #84b577→#57c95e, `--map-right` #82a7c8→#4fb0ec (shared everywhere
      the boundary semantic renders — DriftRunDashboard legend, MapCalibrator too).
- [x] **SVG fallback ring.** ✅ Moot — the editor's SVG fallback was dropped in the
      step-2 rebuild (replaced by the calibration guard). NOTE `DriftZoneMap` still
      carries its own inherited SVG fallback (renders boundaries/gates/splits but not
      the ring) — clean that one separately if/when it matters.
- [ ] **Cross-surface visual polish** (line casing, shared legend, gate
      iconography) from the earlier rendering review — though a shared zone-style
      preset is worth introducing during the rebuild so the new ring/split shapes
      are styled consistently from day one.

## Shipped (for reference)

- **Shared tessellator (build-order step 1).** `src/lib/curve.ts` `tessellate()`
  (centripetal Catmull-Rom α=0.5, open polyline + closed ring) + a byte-identical
  Rust mirror `drift.rs::tessellate`, pinned to the SAME golden values on both
  sides (`scripts/check-curve.mjs` — run `node --experimental-strip-types
  scripts/check-curve.mjs` — and `tessellate_matches_golden_and_invariants`).
  Invariants tested: interpolates its anchors, correct point counts, collinear
  stays straight, <3 anchors pass through, no NaN.
- **Curve flag wired end-to-end (build-order step 3).** `scoringConfig.curve`
  (`'linear'`|`'catmull'`, no migration) drives a "Smooth" toggle in the editor
  form-row; `boundaryCurve()` curves the rendered line on the editor,
  `DriftZoneMap`, and `RunMap` (markers stay on raw anchors); `from_row`
  tessellates the entry-detection polygon to the same curve so display == scored.
  Backend test `curved_zone_tessellates_entry_polygon_yet_preserves_gates`. Still
  TODO: the closed scoring-ring target + seed-from-boundary (step 4).
- **Scoring ring authoring (build-order step 4).** A closed scoring ring is now a
  first-class edit target alongside Left/Right (`curve.ts` `zoneScoringRing` +
  `ringCurve`; `boundary('ring')`/`setBoundary('ring')` in the editor). "Seed ring"
  copies `left ++ reversed(right)`; the ring renders violet on editor /
  DriftZoneMap / RunMap, the Smooth toggle curves it (closed), and it persists in
  `scoringConfig.scoringRegion.anchors` (no migration, round-trips as opaque JSON).
  Backend unchanged (not scored yet — Stage 2). 3-reviewer adversarial pass found
  no bugs; applied fixes: ring-aware `reverseBoundaries` selection, Ring/Seed
  disabled in the uncalibrated fallback, `ringAnchors` in fit/transform, seed guard
  ≥2/side, and the Stage-2 ring-scorer contract documented on `ringCurve`.
- **Editor shell rebuild + split target (build-order steps 2 & 5).** Reshaped
  `DriftZoneEditor.svelte` (kept the proven map/curve/render core; rewrote the
  shell). Layout: zone metadata moved to the sidebar under the zone list, map is the
  hero (flex:1), grouped hairline toolbar (Target `Left·Right·Ring·Split` / Point ops
  `Insert·Delete·Undo·Redo` / View `Fit·Smooth·Seed ring` / Capture, de-emphasised),
  compact status chips replacing the data boxes. Behaviour: insert-on-curve (point on
  the drawn curve, not a live-telemetry dump), stable camera (fit only on zone-select
  + Fit button), delete-selected (retired `removeLastPoint`), full undo/redo (geometry
  + structural snapshot stack, Ctrl+Z/Y, text-field-safe, cleared on select/new/save).
  SVG fallback dropped → calibration guard linking to Settings → Calibrate map (new
  `onOpenSettings` prop wired in `+page.svelte`). Split authoring (step 5): `'split'`
  target + multi-gate sub-selector (`draft.splitGates`, each gate 2 points, dashed
  violet, `1a/1b` markers); backend already stores splits (no change). Colour: new
  `--scoring-ring` (#cf72e0) token decouples ring from split; `--map-left`/`--map-right`
  brightened for legibility — applied across editor / DriftZoneMap / RunMap. Frontend-
  only, NO DB migration (scoringConfig + split_gates_json round-trip raw; verified the
  save path stores `scoring_config` as an opaque `serde_json::Value`). svelte-check 0 /
  prod build green; verified in-browser vs the real DB (zone load, insert/undo/redo,
  seed ring, 2-gate split authoring, palette, 0 console errors). 5-reviewer adversarial
  pass: most findings were false positives (undo order, gate-curve kink, splitLines
  staleness — all disproven); applied 4 real/hardening fixes (save() clears undo
  stacks; defensive selection clamp in `applySnapshot`; IPC cast order; split-dragstart
  comment). **Feedback round ([[user]], 2026-06-14):** (1) Delete-zone moved out of the
  footer (too easy to hit next to Save) to a quiet underlined link at the bottom of the
  sidebar zone-meta; footer is now Save-only. (2) [SUPERSEDED same day by the Split SECTOR model — see the final Shipped bullet; this first cut is kept for history.] Split gates were NAMEABLE —
  `splitGateNames` (index-aligned `string[]` in `scoringConfig.splitGateNames`, opaque
  round-trip, undo-tracked, pruned with gates on save); the sub-selector chip + status
  use the name (else "gate N"), the on-map markers stay `1a/1b` (creation order). Both
  re-verified in-browser. NOT yet committed/branched (working tree on `main`). DEFERRED still:
  P3 keybinds, Stage-2 position gate, directed per-gate ring sub-target, DriftZoneMap's
  own SVG fallback.
- **Split SECTOR model (supersedes the same-day gate-naming first cut).** Decided with
  the user: name the GAPS between dividers, not the split lines. A split is a *divider*;
  N splits make N+1 *sectors* (`A → split1 → … → splitN → B`). Names live on the sectors
  (`sectorNames`, length N+1, in `scoringConfig.sectorNames` — opaque round-trip,
  undo-tracked, pruned in lockstep with incomplete splits on save). Why: it dodges the
  fencepost (N split lines but N+1 names — anchoring the name to a line orphans one end
  sector, and *which* end flips with run direction) and is direction-stable (names attach
  to physical sectors; the entry gate only sets traversal order; A/B never need names —
  they're the caps of the first/last sectors). New splits **auto-insert in driving order**
  (`splitPathPos` = arc-length of the gate midpoint projected onto the boundary), so
  "split 1" is the first reached from A, killing the "split 4 is first" confusion. UI =
  an interleaved strip `A · ⟦sector⟧ · |split| · ⟦sector⟧ · B` (sector text inputs in the
  gaps, numbered split-node chips as dividers); the on-map markers stay numeric `1a/1b`
  (now path-ordered). Verified in-browser: reverse-order placement auto-sorts to A→B,
  naming, delete→sector-merge (keeps the left name), and undo all correct; svelte-check 0,
  build green. KNOWN LIMITATION: dragging a split clean *past* another doesn't re-order
  (rare — delete + re-place). **NEXT, backend = the crossing-count per-sector scorer:**
  bucket each tick's points by how many splits the run has crossed from its entry gate,
  emit per-sector point totals (for run-viewer "where did I score" readouts). User signed
  off on crossing-count first; arc-length banding (project car position onto the path,
  bucket by band) is the fallback if back-and-forth wobble miscounts. Entry gate is
  already known to the backend (`crossed_a`/`crossed_b`, drift.rs).

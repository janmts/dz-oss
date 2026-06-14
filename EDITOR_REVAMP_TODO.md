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

- [ ] **Insert dumps the live point on top of the selection.** `insertAtSelected`
      inserts the live telemetry point, not a point between neighbours. → Insert at
      the **midpoint of the curve segment** next to the selection, pre-selected and
      draggable.
- [ ] **Every edit yanks the camera.** `fitMapToGeometry` is called on
      capture/insert/select. → Fit ONLY on the explicit button + first zone-select;
      otherwise leave the camera where the user put it.
- [ ] **"Remove last L/R" nukes a gate anchor.** It slices the literal last point
      in the chain. → **Delete *selected* point** (logic already exists as
      `deletePoint`).
- [ ] **No undo/redo.** Fatal for sculpting. → Snapshot stack of the draft
      geometry, pushed on each mutating op (Ctrl+Z / Ctrl+Y).
- [ ] **Split gates can't be authored.** They render but there's no UI. → Falls out
      of the target selector below.

## Layout & UX

- [ ] **Map is the hero.** Move zone metadata (name/description/slack/active) into
      the sidebar under the zone list — stop it eating the strip above the map.
- [ ] **Grouped toolbar** (hairline dividers, data-logger style), not one flat row:
      *Target* segmented (`Left edge · Right edge · Scoring ring · Split`) ·
      *Point ops* (`Insert · Delete selected · Undo · Redo`) ·
      *View* (`Fit · Show tessellated` toggle · `Seed from boundary` for the ring) ·
      *Capture* (`Capture live point` + readout, de-emphasised).
- [ ] **Replace the long data boxes** with compact inline status chips
      (`L 6 · R 5 · ring 12 · split 0`) to reclaim vertical space for the map.
- [ ] **Drop the SVG fallback** (`makeCalib` never nulls with the bundled map; an
      uncalibrated, map-less editor is useless for zone mapping). Replace with a
      "calibrate the map first →" guard pointing at the calibrator. (`DriftZoneMap`
      has the same inherited fallback — clean it the same way, separately.)

## Backend touch (Stage 1 only)

- [ ] `RunnableZone::from_row` tessellates the boundary (when `curve` is set)
      BEFORE building the entry polygon/gates, so run-start detection matches the
      drawn curve. Cache per zone-list change, not per packet.

---

## Build order

1. [x] **`tessellate()` — JS (`src/lib/curve.ts`) + Rust (`drift.rs`) + shared
       golden-value parity test** (`scripts/check-curve.mjs` ↔
       `drift.rs::tests::tessellate_matches_golden_and_invariants`). ✅
2. [ ] Editor shell rebuild: layout (sidebar metadata, grouped toolbar, status
       chips), drop SVG fallback → calibration guard, stable camera, undo/redo,
       insert-on-curve, delete-selected.
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
5. [ ] Split target (nearly free once the target model exists).

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
- [ ] **Ring colour token.** The ring currently reuses `--violet` (== `--gate-split`
      `#a995cf`); on the maps a ring (solid) and a split gate (dashed) are the same
      hue. Decide: keep the solid-vs-dashed convention, or give the ring its own
      token. (Editor has no split rendering, so no collision there.)
- [ ] **SVG fallback ring.** The uncalibrated SVG-fallback editor doesn't render
      the ring; the Ring/Seed controls are disabled there for now. Moot once the
      fallback is dropped in the editor rebuild (step 2).
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

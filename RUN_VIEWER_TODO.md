# Run viewer — improvements & follow-ups

A living backlog for the **Runs** tab (the recorded-run map overlay + datalogger
graph, added in [PR #34](https://github.com/janmts/dz-oss/pull/34)). Jot down
anything you notice while using it — rough notes are fine, we can refine later.

Key files: `src/lib/components/RunsTab.svelte` (shell), `RunMap.svelte` (map
overlay), `RunGraph.svelte` (lanes), `RunSelector.svelte` (filters + list),
`CheckboxDropdown.svelte` (reused dropdown), `src/lib/runViewer.ts` (data layer:
channels, decimation, scoring-state), `src/lib/stores/runViewer.ts` (selection /
lanes / hover state). Backend: `get_drift_run_packets` in `src-tauri/src/api.rs`
+ `TickScore` in `scoring.rs`.

---

## Planned follow-ups

- [ ] **Real-time playback (P2).** Play a run back through the gauge cluster + a
      moving map arrow. Extend the existing `replay` store + `ReplayBar.svelte`
      to accept a drift run; reuse `displayPacket` so the gauges/tire panel
      animate for free. Use a **timestamp-driven** clock (respect the true ~64 Hz
      tick + stall gaps — see the FH6 telemetry cadence) rather than the current
      index×60 fps stepping, so playback is real-time and pauses read correctly.
      A scrubber should move the same shared `hover` cursor (map + graph).

- [ ] **Comparison time-alignment.** Today multiple runs align by sample index
      (tick number ≈ t=0 at the entry gate). Add alternative alignments:
      distance-into-run, or gate-relative, so two runs of different pace line up
      meaningfully on the timeline. (See `RunGraph` x-axis build + `runViewer.ts`
      `timeAxis`.)

- [ ] **Wall-clock time option.** The graph x-axis + hover card use the *sample
      clock* (`index / 64 Hz`) for consistency. Offer a true wall-clock
      (`timestampMs`) mode — FH6 emits duplicate timestamps so the two diverge;
      sample-clock is uniform, wall-clock is "real" elapsed.

- [ ] **Map → graph cursor exactness.** Graph → map highlight is exact; map →
      graph is approximate (`valToPos` in `RunGraph`'s hover `$effect`). Make the
      map hover drive the graph cursor to the precise sample.

---

## Known limitations / simplifications

- [ ] **No preroll approach trail.** The backend stores a pre-gate approach
      trail (`drift_run_preroll_packets`) but the viewer doesn't show it. Could
      draw it as a faded lead-in to the trace (shows how the drift was set up
      before the gate). Endpoint would need to return it.

- [ ] **No calibration-epoch metadata.** Re-plotting assumes the *current* map
      calibration. Fine unless the map is re-calibrated between runs; if that
      ever happens, old runs would plot slightly off.

- [ ] **Trace visibility on green terrain.** The scoring-green trace can be a
      touch low-contrast over satellite-green tiles. Consider a dark casing
      (outline) under the trace, or a slightly heavier weight.

---

## Ideas / nice-to-haves

- [ ] Per-tick **comparison delta** in the hover card when 2+ runs are selected
      (e.g. Δ speed / Δ angle / Δ cumulative points at the same time).
- [ ] **Keyboard scrubbing** along the trace (←/→ to step ticks, with the map +
      graph cursor following).
- [ ] **Export** the selected run(s) — CSV of the per-tick channels, or a PNG of
      the map + graph.
- [ ] Persist the **lane layout / channel selection** (and Map+graph vs
      Graph-only) across sessions so a preferred analysis setup sticks.
- [ ] **Scoreable-zone overlay** — once the hidden scoreable polygon is mapped,
      draw it so off-zone (unpaid) stretches are obvious spatially.
- [ ] Quick **"jump to" markers** on the trace for notable events (first scoring
      tick, each direction flip, longest off-tarmac stretch).

---

## Shipped (for reference)

- Map overlay on the real game tiles: zone border + gates, scoring-state-coloured
  trace, invisible hover targets (16 Hz ∪ every rumble tick) → amber highlight +
  per-tick card.
- Datalogger graph: up to 4 cursor-synced lanes, per-lane searchable channel
  pickers, drag-resize, Map+graph / Graph-only toggle.
- Run selector: car / zone / season multi-select filters + multi-select
  comparison (N traces on the map, N series per lane).
- Backend `get_drift_run_packets` returning packets + authoritative per-tick
  `TickScore` (scoring/drifting/tarmac/points) that reconstruct the run score.

<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte';
  import type { LatLng, LeafletMouseEvent, Marker, Polyline } from 'leaflet';
  import { packet, displayPacket } from '$lib/stores/telemetry';
  import {
    deleteDriftZone,
    driftZones,
    loadDriftZones,
    saveDriftZone,
    settings,
  } from '$lib/stores/sessions';
  import { effectiveMapConfig } from '$lib/mapDefaults';
  import { createGameMap, makeCalib, type GameMap } from '$lib/mapView';
  import { themeColor } from '$lib/theme';
  import { ipc } from '$lib/ipc';
  import type { DriftZoneInput, DriftZoneRow, ZonePoint } from '$lib/types';
  import {
    boundaryCurve,
    parseScoringRegion,
    ringCurve,
    sharedScoringRing,
    tessellate,
    zoneCurveMode,
    DEFAULT_SEGMENTS,
    type ScoringRegion,
    type ZoneCurveMode,
  } from '$lib/curve';

  // The editor needs the calibrated game map; when it isn't calibrated we show a
  // guard that links back to Settings → Calibrate map (wired from +page.svelte).
  let { onOpenSettings }: { onOpenSettings?: () => void } = $props();

  // Edit targets. 'ring' is the closed scoring region; 'split' is the collection
  // of mid-run split gates (each a 2-point line) — both reuse the point-edit
  // machinery of the two open road-edge boundaries.
  type BoundarySide = 'left' | 'right' | 'ring' | 'split';

  const editorExtraZoom = 4;

  function blankZone(): DriftZoneInput {
    return {
      id: null,
      name: 'New drift zone',
      description: null,
      active: true,
      leftBoundary: [],
      rightBoundary: [],
      startGate: [],
      finishGate: [],
      splitGates: [],
      scoringConfig: { version: 1 },
    };
  }

  function toInput(zone: DriftZoneRow): DriftZoneInput {
    return {
      id: zone.id,
      name: zone.name,
      description: zone.description,
      active: zone.active,
      leftBoundary: zone.leftBoundary.map((p) => ({ ...p })),
      rightBoundary: zone.rightBoundary.map((p) => ({ ...p })),
      startGate: zone.startGate.map((p) => ({ ...p })),
      finishGate: zone.finishGate.map((p) => ({ ...p })),
      splitGates: zone.splitGates.map((gate) => gate.map((p) => ({ ...p }))),
      scoringConfig: { ...zone.scoringConfig },
    };
  }

  let draft = $state<DriftZoneInput>(blankZone());
  let selectedId = $state<number | null>(null);
  let activeSide = $state<BoundarySide>('left');
  // Which split gate (index into draft.splitGates) the 'split' target edits.
  let selectedSplit = $state(-1);
  let status = $state('');
  let saving = $state(false);
  // Per-zone boundary slack (m). Lives in scoringConfig.boundarySlackM; mirrored
  // here for a typed number input and written back on save.
  let slackM = $state(3);
  // Boundary interpolation. Lives in scoringConfig.curve; 'catmull' draws AND
  // scores the boundary as a centripetal curve through the anchors.
  let curveMode = $state<ZoneCurveMode>('linear');
  // Closed scoring ring (the per-tick scoreable region — separate from the
  // road-edge boundary). Lives in scoringConfig.scoringRegion.anchors.
  let ringAnchors = $state<ZonePoint[]>([]);
  // SECTOR names — names for the GAPS between dividers, not the split lines. With N
  // splits there are N+1 sectors (A→split₁ … split_N→B); `sectorNames` is length
  // N+1, index-aligned to driving order. The split lines stay pure geometry (markers
  // 1a/1b). Lives in scoringConfig.sectorNames (opaque JSON, no migration). A split
  // is a divider — it ends one sector and starts the next — so the name belongs on
  // the gap, which also dodges the "N lines but N+1 names" fencepost.
  let sectorNames = $state<string[]>([]);

  function syncFromConfig() {
    const v = draft.scoringConfig?.boundarySlackM;
    slackM = typeof v === 'number' ? v : 3;
    curveMode = zoneCurveMode(draft.scoringConfig);
    ringAnchors = sharedScoringRing(draft.scoringConfig).map((p) => ({ ...p }));
    const names = draft.scoringConfig?.sectorNames;
    sectorNames = Array.from({ length: draft.splitGates.length + 1 }, (_, i) =>
      Array.isArray(names) && typeof names[i] === 'string' ? (names[i] as string) : ''
    );
  }

  let selectedPoint = $state<{ side: BoundarySide; index: number } | null>(null);
  let mapHost = $state<HTMLDivElement | null>(null);
  let gm: GameMap | null = null;
  let liveMarker: Marker | null = null;
  // Imperative refs to the boundary/gate polylines so we can update their geometry
  // live during a marker drag without recreating the marker being dragged.
  let leftLine: Polyline | null = null;
  let rightLine: Polyline | null = null;
  let startGateLine: Polyline | null = null;
  let finishGateLine: Polyline | null = null;
  let ringLine: Polyline | null = null;
  let splitLines: (Polyline | null)[] = [];
  let unsubscribeShortcut: (() => void) | null = null;
  let lastKnownPoint = $state<ZonePoint | null>(null);
  let lastKnownAt = $state(0);
  let mapReady = $state(false);

  // ── Undo / redo ──────────────────────────────────────────────────────────
  // Snapshot the draft GEOMETRY + structural state (anchors, ring, splits, curve
  // mode, selection) before each mutating op. Text metadata (name/desc/slack/
  // active) is intentionally NOT tracked — that's standard form behaviour.
  type GeomSnapshot = {
    left: ZonePoint[];
    right: ZonePoint[];
    ring: ZonePoint[];
    splits: ZonePoint[][];
    sectorNames: string[];
    curve: ZoneCurveMode;
    selectedPoint: { side: BoundarySide; index: number } | null;
    selectedSplit: number;
    activeSide: BoundarySide;
  };
  let undoStack = $state<GeomSnapshot[]>([]);
  let redoStack = $state<GeomSnapshot[]>([]);

  function snapshot(): GeomSnapshot {
    return {
      left: draft.leftBoundary.map((p) => ({ ...p })),
      right: draft.rightBoundary.map((p) => ({ ...p })),
      ring: ringAnchors.map((p) => ({ ...p })),
      splits: draft.splitGates.map((g) => g.map((p) => ({ ...p }))),
      sectorNames: sectorNames.slice(),
      curve: curveMode,
      selectedPoint: selectedPoint ? { ...selectedPoint } : null,
      selectedSplit,
      activeSide,
    };
  }

  // Push the CURRENT state so a later undo can return to it. Call BEFORE mutating.
  function pushUndo() {
    undoStack = [...undoStack, snapshot()];
    if (undoStack.length > 100) undoStack = undoStack.slice(-100);
    redoStack = [];
  }

  function applySnapshot(s: GeomSnapshot) {
    draft.leftBoundary = s.left.map((p) => ({ ...p }));
    draft.rightBoundary = s.right.map((p) => ({ ...p }));
    ringAnchors = s.ring.map((p) => ({ ...p }));
    draft.splitGates = s.splits.map((g) => g.map((p) => ({ ...p })));
    sectorNames = s.sectorNames.slice();
    curveMode = s.curve;
    selectedSplit = s.selectedSplit;
    activeSide = s.activeSide;
    // Restore selection, defensively clamped to the restored geometry. Snapshots
    // are internally consistent, but a clamp keeps a stray index from ever
    // pointing past the array it refers to.
    let sp = s.selectedPoint ? { ...s.selectedPoint } : null;
    if (sp) {
      const len = boundary(sp.side).length;
      if (len === 0) sp = null;
      else if (sp.index >= len) sp = { side: sp.side, index: len - 1 };
    }
    selectedPoint = sp;
  }

  function undo() {
    if (!undoStack.length) return;
    redoStack = [...redoStack, snapshot()];
    const s = undoStack[undoStack.length - 1];
    undoStack = undoStack.slice(0, -1);
    applySnapshot(s);
    status = 'Undo.';
  }

  function redo() {
    if (!redoStack.length) return;
    undoStack = [...undoStack, snapshot()];
    const s = redoStack[redoStack.length - 1];
    redoStack = redoStack.slice(0, -1);
    applySnapshot(s);
    status = 'Redo.';
  }

  onMount(async () => {
    void loadDriftZones();
    unsubscribeShortcut = await ipc.subscribeDriftZoneCapture(({ side }) => {
      capturePoint((side ?? activeSide) as BoundarySide, true);
    });
    window.addEventListener('keydown', onLocalKeydown);
  });

  onDestroy(() => {
    unsubscribeShortcut?.();
    window.removeEventListener('keydown', onLocalKeydown);
    gm?.destroy();
    gm = null;
  });

  $effect(() => {
    const p = $packet ?? $displayPacket;
    if (!p || (p.positionX === 0 && p.positionZ === 0)) return;
    lastKnownPoint = { x: p.positionX, z: p.positionZ };
    lastKnownAt = Date.now();
  });

  let livePoint = $derived(lastKnownPoint);
  let liveAgeSecs = $derived(lastKnownAt ? Math.max(0, (Date.now() - lastKnownAt) / 1000) : 0);
  let cfg = $derived($settings ? effectiveMapConfig($settings) : null);
  let calib = $derived(cfg ? makeCalib(cfg) : null);
  let mapUsable = $derived(!!cfg && !!calib);

  let allPoints = $derived([
    ...draft.leftBoundary,
    ...draft.rightBoundary,
    ...draft.startGate,
    ...draft.finishGate,
    ...draft.splitGates.flat(),
    ...ringAnchors,
    ...(livePoint ? [livePoint] : []),
  ]);

  function worldToLatLng(point: ZonePoint): LatLng {
    return gm!.worldToLatLng(point);
  }

  function latLngToWorld(latlng: LatLng): ZonePoint {
    return gm!.latLngToWorld(latlng);
  }

  // Latlngs for a boundary's rendered LINE: the centripetal curve through the
  // side's anchors when smoothed (straight chords otherwise), with `dragIndex`
  // optionally overridden by an in-progress drag. Markers stay on the raw
  // anchors — only the line follows the curve. Matches the scorer's tessellation
  // (drift.rs from_row) so the drawn boundary equals the scored one.
  function boundaryLatLngs(side: BoundarySide, dragIndex = -1, dragLL: LatLng | null = null): LatLng[] {
    const pts = boundary(side).map((p, i) => (i === dragIndex && dragLL ? latLngToWorld(dragLL) : p));
    return boundaryCurve(pts, curveMode).map(worldToLatLng);
  }

  // Latlngs for the closed scoring-ring LINE: the curve through the ring anchors
  // closed back to the start, with an optional in-progress drag override.
  function ringLatLngs(dragIndex = -1, dragLL: LatLng | null = null): LatLng[] {
    const pts = ringAnchors.map((p, i) => (i === dragIndex && dragLL ? latLngToWorld(dragLL) : p));
    return ringCurve(pts, curveMode).map(worldToLatLng);
  }

  function markerIcon(side: BoundarySide, label: string, selected = false) {
    const cls =
      side === 'left'
        ? 'zone-marker-left'
        : side === 'right'
          ? 'zone-marker-right'
          : side === 'ring'
            ? 'zone-marker-ring'
            : 'zone-marker-split';
    return gm!.L.divIcon({
      className: `zone-marker ${cls}${selected ? ' zone-marker-selected' : ''}`,
      html: `<span>${label}</span>`,
      iconSize: [24, 24],
      iconAnchor: [12, 12],
    });
  }

  function liveIcon() {
    return gm!.L.divIcon({
      className: 'zone-marker zone-marker-live',
      html: '<span>●</span>',
      iconSize: [22, 22],
      iconAnchor: [11, 11],
    });
  }

  async function initMap() {
    if (!mapHost || gm || !mapUsable || !cfg) return;
    gm = await createGameMap(mapHost, cfg, { extraZoom: editorExtraZoom });
    gm.map.on('click', onLeafletMapClick);
    mapReady = true;
    redrawLeaflet();
  }

  $effect(() => {
    if (mapHost && cfg && mapUsable && !gm) void initMap();
  });

  function redrawLeaflet() {
    if (!gm || !mapUsable) return;
    gm.clearLines();
    gm.markers.clearLayers();
    leftLine = null;
    rightLine = null;
    startGateLine = null;
    finishGateLine = null;
    ringLine = null;
    splitLines = [];

    if (draft.leftBoundary.length > 1) {
      leftLine = gm.addLine(boundaryLatLngs('left'), 5, {
        color: themeColor('--map-left', '#57c95e'),
        opacity: 0.95,
      });
    }
    if (draft.rightBoundary.length > 1) {
      rightLine = gm.addLine(boundaryLatLngs('right'), 5, {
        color: themeColor('--map-right', '#4fb0ec'),
        opacity: 0.95,
      });
    }
    if (draft.leftBoundary.length && draft.rightBoundary.length) {
      startGateLine = gm.addLine(
        [draft.leftBoundary[0], draft.rightBoundary[0]].map(worldToLatLng),
        3,
        { color: themeColor('--gate-a', '#d2a24c'), dashArray: '10 7' }
      );
      finishGateLine = gm.addLine(
        [
          draft.leftBoundary[draft.leftBoundary.length - 1],
          draft.rightBoundary[draft.rightBoundary.length - 1],
        ].map(worldToLatLng),
        3,
        { color: themeColor('--gate-b', '#d56c62'), dashArray: '10 7' }
      );
    }

    for (const side of ['left', 'right'] as BoundarySide[]) {
      boundary(side).forEach((point, index) => {
        const selected = selectedPoint?.side === side && selectedPoint.index === index;
        const marker = gm!.L.marker(worldToLatLng(point), {
          draggable: true,
          icon: markerIcon(side, `${side[0].toUpperCase()}${index + 1}`, selected),
        }).addTo(gm!.markers);
        marker.on('click', () => selectPoint(side, index));
        marker.on('dragstart', () => pushUndo());
        marker.on('drag', () => updateLinesDuringDrag(side, index, marker.getLatLng()));
        marker.on('dragend', () => {
          const points = [...boundary(side)];
          points[index] = latLngToWorld(marker.getLatLng());
          setBoundary(side, points);
          selectPoint(side, index);
        });
        marker.on('dblclick', () => deletePoint(side, index));
      });
    }

    // Scoring ring (closed): the per-tick scoreable region, distinct from the
    // road-edge boundary. Drawn over the corridor; markers stay on raw anchors.
    if (ringAnchors.length > 1) {
      ringLine = gm.addLine(ringLatLngs(), 4, {
        color: themeColor('--scoring-ring', '#cf72e0'),
        opacity: 0.92,
      });
    }
    ringAnchors.forEach((point, index) => {
      const selected = selectedPoint?.side === 'ring' && selectedPoint.index === index;
      const marker = gm!.L.marker(worldToLatLng(point), {
        draggable: true,
        icon: markerIcon('ring', `S${index + 1}`, selected),
      }).addTo(gm!.markers);
      marker.on('click', () => selectPoint('ring', index));
      marker.on('dragstart', () => pushUndo());
      marker.on('drag', () => updateLinesDuringDrag('ring', index, marker.getLatLng()));
      marker.on('dragend', () => {
        const points = [...boundary('ring')];
        points[index] = latLngToWorld(marker.getLatLng());
        setBoundary('ring', points);
        selectPoint('ring', index);
      });
      marker.on('dblclick', () => deletePoint('ring', index));
    });

    // Split gates: a collection of mid-run 2-point lines (sector splits). Each is
    // dashed violet; the two endpoints are draggable. Selecting/dragging a gate's
    // point makes that gate the active split target.
    draft.splitGates.forEach((gate, gi) => {
      if (gate.length === 2) {
        splitLines[gi] = gm!.addLine(gate.map(worldToLatLng), 3, {
          color: themeColor('--gate-split', '#a995cf'),
          dashArray: '6 6',
          opacity: 0.9,
        });
      } else {
        splitLines[gi] = null;
      }
      gate.forEach((point, pi) => {
        const selected =
          selectedPoint?.side === 'split' && selectedSplit === gi && selectedPoint.index === pi;
        const marker = gm!.L.marker(worldToLatLng(point), {
          draggable: true,
          icon: markerIcon('split', `${gi + 1}${pi === 0 ? 'a' : 'b'}`, selected),
        }).addTo(gm!.markers);
        marker.on('click', () => {
          selectedSplit = gi;
          selectPoint('split', pi);
        });
        marker.on('dragstart', () => {
          // selectedSplit before pushUndo so the snapshot records the gate being
          // edited — undo then restores both the points AND the active gate.
          selectedSplit = gi;
          pushUndo();
        });
        marker.on('drag', () => updateSplitLineDuringDrag(gi, pi, marker.getLatLng()));
        marker.on('dragend', () => {
          const g = [...draft.splitGates[gi]];
          g[pi] = latLngToWorld(marker.getLatLng());
          draft.splitGates = draft.splitGates.map((x, i) => (i === gi ? g : x));
          selectedSplit = gi;
          selectPoint('split', pi);
        });
        marker.on('dblclick', () => deleteSplitGate(gi));
      });
    });

    // Re-add the live marker (cleared above); untracked so rebuilding the
    // boundaries never subscribes this path to the 64 Hz live position.
    liveMarker = null;
    untrack(() => updateLiveMarker());
  }

  // Move just the live marker — runs every telemetry tick without touching the
  // boundary lines or point markers.
  function updateLiveMarker() {
    if (!gm || !mapUsable) return;
    if (livePoint) {
      const ll = worldToLatLng(livePoint);
      if (liveMarker) {
        liveMarker.setLatLng(ll);
      } else {
        liveMarker = gm.L.marker(ll, { icon: liveIcon(), interactive: false }).addTo(gm.markers);
      }
    } else if (liveMarker) {
      liveMarker.remove();
      liveMarker = null;
    }
  }

  // Live-update polyline geometry while a marker is being dragged. We can't commit to
  // `draft` here because that would trigger a full redraw and destroy the marker mid-drag,
  // so we move the affected line vertices imperatively and commit the state on `dragend`.
  function updateLinesDuringDrag(side: BoundarySide, index: number, latlng: LatLng) {
    if (!mapUsable) return;
    if (side === 'ring') {
      ringLine?.setLatLngs(ringLatLngs(index, latlng));
      return;
    }
    // Position of vertex (s, i): the live drag latlng for the moving point, else stored world pos.
    const at = (s: BoundarySide, i: number): LatLng =>
      s === side && i === index ? latlng : worldToLatLng(boundary(s)[i]);

    const sideLine = side === 'left' ? leftLine : rightLine;
    sideLine?.setLatLngs(boundaryLatLngs(side, index, latlng));

    if (draft.leftBoundary.length && draft.rightBoundary.length) {
      startGateLine?.setLatLngs([at('left', 0), at('right', 0)]);
      finishGateLine?.setLatLngs([
        at('left', draft.leftBoundary.length - 1),
        at('right', draft.rightBoundary.length - 1),
      ]);
    }
  }

  function updateSplitLineDuringDrag(gi: number, pi: number, latlng: LatLng) {
    if (!mapUsable) return;
    const gate = draft.splitGates[gi];
    if (!gate || gate.length !== 2) return;
    splitLines[gi]?.setLatLngs(gate.map((p, k) => (k === pi ? latlng : worldToLatLng(p))));
  }

  function fitMapToGeometry() {
    if (!gm || !mapUsable || !cfg || allPoints.length === 0) return;
    const fitZoom = gm.fitWorld(allPoints, 0.2, cfg.viewMaxZoom + editorExtraZoom);
    // Lines show their tuned base weight at this framing and thin out from it.
    gm.setWeightRefZoom(fitZoom);
  }

  // Boundary lines + point markers rebuild on geometry/selection changes only —
  // never on the live telemetry tick.
  $effect(() => {
    void draft.leftBoundary;
    void draft.rightBoundary;
    void draft.splitGates;
    void selectedSplit;
    void selectedPoint;
    void curveMode;
    void ringAnchors;
    if (mapReady) redrawLeaflet();
  });

  // The live marker moves in place every telemetry tick.
  $effect(() => {
    void livePoint;
    if (mapReady) updateLiveMarker();
  });

  function selectZone(zone: DriftZoneRow) {
    draft = toInput(zone);
    selectedId = zone.id;
    selectedPoint = null;
    selectedSplit = zone.splitGates.length ? 0 : -1;
    activeSide = 'left';
    undoStack = [];
    redoStack = [];
    status = '';
    syncFromConfig();
    // Fit once on zone load; thereafter the camera stays where the user put it.
    setTimeout(fitMapToGeometry, 0);
  }

  function newZone() {
    draft = blankZone();
    selectedId = null;
    selectedPoint = null;
    selectedSplit = -1;
    activeSide = 'left';
    undoStack = [];
    redoStack = [];
    status = '';
    syncFromConfig();
  }

  function boundary(side: BoundarySide): ZonePoint[] {
    if (side === 'ring') return ringAnchors;
    if (side === 'split') return draft.splitGates[selectedSplit] ?? [];
    return side === 'left' ? draft.leftBoundary : draft.rightBoundary;
  }

  function setBoundary(side: BoundarySide, points: ZonePoint[]) {
    if (side === 'ring') ringAnchors = points;
    else if (side === 'split') {
      if (selectedSplit < 0) return;
      draft.splitGates = draft.splitGates.map((g, i) => (i === selectedSplit ? points : g));
    } else if (side === 'left') draft.leftBoundary = points;
    else draft.rightBoundary = points;
  }

  function setTarget(side: BoundarySide) {
    activeSide = side;
    if (side === 'split' && selectedSplit < 0 && draft.splitGates.length) selectedSplit = 0;
  }

  // N splits → N+1 sectors (the gaps, including the A-side and B-side ends).
  let sectorSlots = $derived(Array.from({ length: draft.splitGates.length + 1 }, (_, i) => i));

  // Arc-length of the point on `path` nearest to p — orders splits ALONG the
  // corridor so "split 1" is the first one reached from gate A, not the first authored.
  function arcLengthOf(p: ZonePoint, path: ZonePoint[]): number {
    let bestDist = Infinity;
    let bestArc = 0;
    let arc = 0;
    for (let i = 0; i < path.length - 1; i++) {
      const a = path[i];
      const b = path[i + 1];
      const dx = b.x - a.x;
      const dz = b.z - a.z;
      const len2 = dx * dx + dz * dz || 1e-9;
      const t = Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.z - a.z) * dz) / len2));
      const d = (p.x - (a.x + t * dx)) ** 2 + (p.z - (a.z + t * dz)) ** 2;
      const segLen = Math.sqrt(len2);
      if (d < bestDist) {
        bestDist = d;
        bestArc = arc + t * segLen;
      }
      arc += segLen;
    }
    return bestArc;
  }

  function splitPathPos(gate: ZonePoint[]): number {
    if (gate.length < 2) return Infinity; // incomplete sorts last
    const ref = draft.leftBoundary.length >= 2 ? draft.leftBoundary : draft.rightBoundary;
    if (ref.length < 2) return 0;
    const mid = { x: (gate[0].x + gate[1].x) / 2, z: (gate[0].z + gate[1].z) / 2 };
    return arcLengthOf(mid, ref);
  }

  // Slot a freshly-completed split into driving order (A→…→B), carrying its (blank)
  // trailing sector name. Only fresh gates are re-slotted, so there's no named sector
  // to lose; dragging an existing split past another does NOT re-order (rare — delete
  // and re-place it if you need it moved).
  function sortNewSplitIntoPlace(from: number) {
    if (draft.leftBoundary.length < 2 && draft.rightBoundary.length < 2) return;
    const gate = draft.splitGates[from];
    if (!gate || gate.length < 2) return;
    const pos = splitPathPos(gate);
    let target = 0;
    draft.splitGates.forEach((g, i) => {
      if (i !== from && g.length === 2 && splitPathPos(g) < pos) target++;
    });
    if (target === from) return;
    const gates = draft.splitGates.slice();
    gates.splice(from, 1);
    const names = sectorNames.slice();
    const movedName = names.splice(from + 1, 1)[0] ?? '';
    const insertAt = from < target ? target - 1 : target;
    gates.splice(insertAt, 0, gate);
    names.splice(insertAt + 1, 0, movedName);
    draft.splitGates = gates;
    sectorNames = names;
    selectedSplit = insertAt;
    selectedPoint = null;
  }

  function renameSector(i: number, value: string) {
    const next = sectorNames.slice();
    while (next.length <= i) next.push('');
    next[i] = value;
    sectorNames = next;
  }

  function appendBoundaryPoint(side: BoundarySide, point: ZonePoint, source: 'map' | 'telemetry') {
    if (side === 'split') {
      // No split selected yet → start a fresh divider with this point (and a new
      // trailing sector slot).
      if (selectedSplit < 0 || selectedSplit >= draft.splitGates.length) {
        pushUndo();
        draft.splitGates = [...draft.splitGates, [{ ...point }]];
        sectorNames = [...sectorNames, ''];
        selectedSplit = draft.splitGates.length - 1;
        activeSide = 'split';
        selectedPoint = { side: 'split', index: 0 };
        status = 'Started split — place its second point across the road.';
        return;
      }
      const gate = draft.splitGates[selectedSplit];
      if (gate.length >= 2) {
        status = 'Split complete. Add another split, or drag its points to refine.';
        return;
      }
      pushUndo();
      const next = [...gate, { ...point }];
      setBoundary('split', next);
      activeSide = 'split';
      selectedPoint = { side: 'split', index: next.length - 1 };
      if (next.length === 2) {
        sortNewSplitIntoPlace(selectedSplit);
        status = 'Split complete — slotted into driving order.';
      } else {
        status = 'Split — place its second point across the road.';
      }
      return;
    }
    pushUndo();
    const nextIndex = boundary(side).length;
    setBoundary(side, [...boundary(side), { ...point }]);
    activeSide = side;
    selectedPoint = { side, index: nextIndex };
    status =
      source === 'map'
        ? `Added ${side} point ${nextIndex + 1} from map click.`
        : `Captured ${side} point ${nextIndex + 1} from last known telemetry.`;
  }

  function onLeafletMapClick(e: LeafletMouseEvent) {
    if (!mapUsable) return;
    appendBoundaryPoint(activeSide, latLngToWorld(e.latlng), 'map');
  }

  function pointLabel(side: BoundarySide, index: number): string {
    if (side === 'left') return `L${index + 1}`;
    if (side === 'right') return `R${index + 1}`;
    if (side === 'ring') return `S${index + 1}`;
    return `split ${selectedSplit + 1}${index === 0 ? 'a' : 'b'}`;
  }

  function selectedLabel(): string {
    if (activeSide === 'split') {
      if (selectedSplit < 0) return 'No split';
      return selectedPoint?.side === 'split'
        ? pointLabel('split', selectedPoint.index)
        : `split ${selectedSplit + 1}`;
    }
    return selectedPoint ? pointLabel(selectedPoint.side, selectedPoint.index) : 'No point selected';
  }

  function capturePoint(side: BoundarySide = activeSide, fromShortcut = false) {
    if (!livePoint) {
      status = 'Waiting for the first telemetry world position. Drive briefly, then capture.';
      return;
    }
    appendBoundaryPoint(side, livePoint, 'telemetry');
    if (fromShortcut && side !== activeSide) activeSide = side;
    if (fromShortcut) status = `${status.replace(/\.$/, '')} via shortcut.`;
  }

  // The curve point at the midpoint of the segment between anchors i and j — ON
  // the drawn curve (so inserting an anchor there barely changes the shape, just
  // adds a draggable handle). Linear / <3 anchors fall back to the chord midpoint.
  function curvePointBetween(pts: ZonePoint[], i: number, j: number, closed: boolean): ZonePoint {
    const a = pts[i];
    const b = pts[j];
    if (curveMode === 'linear' || pts.length < 3) {
      return { x: (a.x + b.x) / 2, z: (a.z + b.z) / 2 };
    }
    const curve = tessellate(pts, { closed });
    const idx = i * DEFAULT_SEGMENTS + Math.floor(DEFAULT_SEGMENTS / 2);
    const k = closed ? idx % curve.length : Math.min(idx, curve.length - 1);
    return { x: curve[k].x, z: curve[k].z };
  }

  // Insert a new anchor on the curve next to the selection (no live telemetry):
  // between the selected anchor and its successor (or predecessor if it's last),
  // pre-selected and ready to drag.
  function insertOnCurve() {
    const side = activeSide;
    if (side === 'split') {
      status = 'Insert doesn’t apply to split gates (they are 2-point lines).';
      return;
    }
    const pts = boundary(side);
    if (pts.length < 2) {
      status = 'Add at least 2 points to this target before inserting.';
      return;
    }
    const closed = side === 'ring';
    let i = selectedPoint?.side === side ? selectedPoint.index : pts.length - 1;
    let j: number;
    let insertAt: number;
    if (closed) {
      j = (i + 1) % pts.length;
      insertAt = i + 1;
    } else if (i < pts.length - 1) {
      j = i + 1;
      insertAt = i + 1;
    } else {
      // Last anchor: insert into the segment before it.
      j = i;
      i = i - 1;
      insertAt = j;
    }
    const mid = curvePointBetween(pts, i, j, closed);
    pushUndo();
    const next = [...pts];
    next.splice(insertAt, 0, mid);
    setBoundary(side, next);
    activeSide = side;
    selectedPoint = { side, index: insertAt };
    status = `Inserted ${pointLabel(side, insertAt)} on the curve.`;
  }

  function onLocalKeydown(e: KeyboardEvent) {
    const tag = (e.target as HTMLElement | null)?.tagName;
    const inField = tag === 'INPUT' || tag === 'TEXTAREA';
    const key = e.key.toLowerCase();
    // Global capture (works while FH6 has focus too) — alt distinguishes from undo.
    if (e.ctrlKey && e.altKey) {
      if (key === 'z') {
        e.preventDefault();
        capturePoint(activeSide, true);
      } else if (key === 'l') {
        e.preventDefault();
        capturePoint('left', true);
      } else if (key === 'r') {
        e.preventDefault();
        capturePoint('right', true);
      }
      return;
    }
    // Undo / redo — but never steal a text field's own undo.
    if (e.ctrlKey && !inField) {
      if (key === 'z' && !e.shiftKey) {
        e.preventDefault();
        undo();
      } else if (key === 'y' || (key === 'z' && e.shiftKey)) {
        e.preventDefault();
        redo();
      }
    }
  }

  function reverseBoundaries() {
    pushUndo();
    draft.leftBoundary = [...draft.leftBoundary].reverse();
    draft.rightBoundary = [...draft.rightBoundary].reverse();
    // Only the left/right arrays were reversed — don't remap a ring/split selection.
    if (selectedPoint && (selectedPoint.side === 'left' || selectedPoint.side === 'right')) {
      const len = boundary(selectedPoint.side).length;
      selectedPoint = { side: selectedPoint.side, index: len - selectedPoint.index - 1 };
    }
  }

  // Seed the closed scoring ring from the road-edge boundary: left ++ reversed
  // right — the same corridor ring the scorer builds — then tighten by dragging
  // anchors inward. A one-time copy; editing the ring never moves the boundary.
  function seedRingFromBoundary() {
    if (draft.leftBoundary.length < 2 || draft.rightBoundary.length < 2) {
      status = 'Map the left and right boundary (at least 2 points each) before seeding the scoring ring.';
      return;
    }
    pushUndo();
    ringAnchors = [
      ...draft.leftBoundary.map((p) => ({ ...p })),
      ...draft.rightBoundary.map((p) => ({ ...p })).reverse(),
    ];
    activeSide = 'ring';
    selectedPoint = null;
    status = `Seeded scoring ring from boundary (${ringAnchors.length} points). Drag inward to tighten.`;
  }

  function toggleSmooth() {
    pushUndo();
    curveMode = curveMode === 'catmull' ? 'linear' : 'catmull';
  }

  function deletePoint(side: BoundarySide, index: number) {
    pushUndo();
    setBoundary(
      side,
      boundary(side).filter((_, i) => i !== index)
    );
    if (selectedPoint?.side === side) {
      const nextLen = boundary(side).length;
      selectedPoint = nextLen === 0 ? null : { side, index: Math.min(index, nextLen - 1) };
    }
  }

  function addSplitGate() {
    pushUndo();
    draft.splitGates = [...draft.splitGates, []];
    // A new divider at the end opens a new trailing sector.
    sectorNames = [...sectorNames, ''];
    selectedSplit = draft.splitGates.length - 1;
    activeSide = 'split';
    selectedPoint = null;
    status = 'New split — click two points across the road (or capture) to place it.';
  }

  function deleteSplitGate(idx: number) {
    if (idx < 0 || idx >= draft.splitGates.length) return;
    pushUndo();
    draft.splitGates = draft.splitGates.filter((_, i) => i !== idx);
    // Removing divider idx merges sector idx and idx+1 — keep the left one's name.
    sectorNames = sectorNames.filter((_, i) => i !== idx + 1);
    selectedSplit = Math.min(selectedSplit, draft.splitGates.length - 1);
    if (selectedPoint?.side === 'split') selectedPoint = null;
    status = 'Removed split.';
  }

  // Toolbar "Delete" / Delete key: a split gate is atomic (its point can't stand
  // alone), so deleting on the split target removes the whole gate.
  function deleteSelected() {
    if (activeSide === 'split') {
      if (selectedSplit >= 0) deleteSplitGate(selectedSplit);
      else status = 'Select a split gate to delete.';
      return;
    }
    if (!selectedPoint) {
      status = 'Select a point to delete.';
      return;
    }
    deletePoint(selectedPoint.side, selectedPoint.index);
  }

  function selectPoint(side: BoundarySide, index: number) {
    selectedPoint = { side, index };
    activeSide = side;
  }

  async function save() {
    saving = true;
    // Drop incomplete splits (a divider needs both points). Removing divider i
    // merges its two sectors, so drop the name on its right; iterate high→low so
    // indices stay valid.
    for (let i = draft.splitGates.length - 1; i >= 0; i--) {
      if (draft.splitGates[i].length !== 2) {
        draft.splitGates = draft.splitGates.filter((_, k) => k !== i);
        sectorNames = sectorNames.filter((_, k) => k !== i + 1);
      }
    }
    selectedSplit = draft.splitGates.length ? Math.min(Math.max(selectedSplit, 0), draft.splitGates.length - 1) : -1;
    // Persist slack + curve mode + scoring ring + sector names into the bag.
    const cfg: Record<string, unknown> = {
      ...draft.scoringConfig,
      boundarySlackM: Math.max(0, slackM),
      curve: curveMode,
    };
    // Preserve any directed per-gate rings — the editor only authors the shared ring.
    const region: ScoringRegion = { ...parseScoringRegion(draft.scoringConfig) };
    if (ringAnchors.length) region.anchors = ringAnchors.map((p) => ({ ...p }));
    else delete region.anchors;
    if (region.anchors?.length || region.byGate) cfg.scoringRegion = region;
    else delete cfg.scoringRegion;
    if (sectorNames.some((n) => n && n.trim())) cfg.sectorNames = sectorNames.map((n) => n ?? '');
    else delete cfg.sectorNames;
    delete cfg.splitGateNames; // retired: names live on sectors now, not gates
    draft.scoringConfig = cfg;
    try {
      const saved = await saveDriftZone(draft);
      draft = toInput(saved);
      selectedId = saved.id;
      syncFromConfig();
      selectedSplit = draft.splitGates.length ? Math.min(Math.max(selectedSplit, 0), draft.splitGates.length - 1) : -1;
      // Save is a commit point; the reloaded draft makes prior snapshots stale.
      undoStack = [];
      redoStack = [];
      status = `Saved ${saved.name}.`;
    } finally {
      saving = false;
    }
  }

  async function removeZone() {
    if (!selectedId) return;
    if (!confirm(`Delete drift zone "${draft.name}"? Runs keep their data but lose the zone link.`)) return;
    await deleteDriftZone(selectedId);
    newZone();
  }
</script>

<div class="editor">
  <aside class="sidebar">
    <div class="side-head">
      <span class="cap">Drift Zones</span>
      <button class="ghost" onclick={newZone}>+ New</button>
    </div>

    <div class="zone-list">
      {#each $driftZones as zone}
        <button
          class="zone-row"
          class:active={zone.id === selectedId}
          onclick={() => selectZone(zone)}
        >
          <span>{zone.name}</span>
          <small class="mono">{zone.leftBoundary.length}L / {zone.rightBoundary.length}R</small>
        </button>
      {:else}
        <p class="empty">No zones saved yet.</p>
      {/each}
    </div>

    <div class="zone-meta">
      <div class="meta-head">{selectedId ? 'Edit zone' : 'New zone'}</div>
      <label>
        <span class="cap">Name</span>
        <input bind:value={draft.name} />
      </label>
      <label>
        <span class="cap">Description</span>
        <input bind:value={draft.description} placeholder="Optional notes for this zone" />
      </label>
      <div class="meta-row">
        <label class="num" title="Metres a run may stray past the boundary before it voids. ~3 m suits most zones; raise it for sparsely-mapped or wide zones.">
          <span class="cap">Slack (m)</span>
          <input class="mono" type="number" min="0" step="0.5" bind:value={slackM} />
        </label>
        <label class="checkbox">
          <input type="checkbox" bind:checked={draft.active} />
          Active
        </label>
      </div>
      {#if selectedId}
        <button class="danger-link" onclick={removeZone}>Delete this zone…</button>
      {/if}
    </div>
  </aside>

  <section class="main-panel">
    <div class="toolbar">
      <div class="group">
        <div class="segmented">
          <button class:active={activeSide === 'left'} onclick={() => setTarget('left')}>Left</button>
          <button class:active={activeSide === 'right'} onclick={() => setTarget('right')}>Right</button>
          <button class:active={activeSide === 'ring'} onclick={() => setTarget('ring')}>Ring</button>
          <button class:active={activeSide === 'split'} onclick={() => setTarget('split')}>Split</button>
        </div>
      </div>

      <span class="divider"></span>

      <div class="group">
        <button onclick={insertOnCurve} title="Insert a point on the curve next to the selected one, ready to drag">Insert</button>
        <button onclick={deleteSelected} title="Delete the selected point (or split gate)">Delete</button>
        <button onclick={undo} disabled={!undoStack.length} title="Undo (Ctrl+Z)">Undo</button>
        <button onclick={redo} disabled={!redoStack.length} title="Redo (Ctrl+Y)">Redo</button>
      </div>

      <span class="divider"></span>

      <div class="group">
        <button onclick={fitMapToGeometry} title="Frame the whole zone (the only thing that moves the camera)">Fit</button>
        <button
          class:active={curveMode === 'catmull'}
          onclick={toggleSmooth}
          title="Curve the display AND scoring through the anchors (centripetal Catmull-Rom). Off = straight segments."
        >Smooth</button>
        <button onclick={seedRingFromBoundary} title="Copy left ++ reversed-right into the scoring ring, then drag anchors inward to tighten it.">Seed ring</button>
      </div>

      <div class="group capture">
        <button class="subtle" onclick={() => capturePoint()} title="Append the last known telemetry position to the active target">Capture live</button>
        <span class="live mono">
          {livePoint
            ? `X ${livePoint.x.toFixed(0)} · Z ${livePoint.z.toFixed(0)}${liveAgeSecs > 2 ? ` · ${liveAgeSecs.toFixed(0)}s` : ''}`
            : 'no telemetry'}
        </span>
      </div>
    </div>

    {#if activeSide === 'split'}
      <div class="splitbar">
        <span class="cap">Sectors</span>
        <span class="endcap a" title="Entry/finish gate A (boundary start)">A</span>
        {#each sectorSlots as i}
          <input
            class="sector-name"
            placeholder={`sector ${i + 1}`}
            value={sectorNames[i] ?? ''}
            oninput={(e) => renameSector(i, e.currentTarget.value)}
          />
          {#if i < draft.splitGates.length}
            <button
              class="btn-sm splitnode"
              class:active={selectedSplit === i}
              class:incomplete={draft.splitGates[i].length < 2}
              title={draft.splitGates[i].length < 2
                ? 'Incomplete — place its 2 points across the road'
                : `split ${i + 1} — click to select its line`}
              onclick={() => {
                selectedSplit = i;
                selectedPoint = null;
              }}
            >{i + 1}</button>
          {/if}
        {/each}
        <span class="endcap b" title="Entry/finish gate B (boundary end)">B</span>
        <button class="btn-sm gatechip add" onclick={addSplitGate}>+ Split</button>
        {#if selectedSplit >= 0}
          <button class="btn-sm gatechip del" onclick={() => deleteSplitGate(selectedSplit)}>Delete split {selectedSplit + 1}</button>
        {/if}
      </div>
    {/if}

    <div class="chips">
      <span class="chip">L <b>{draft.leftBoundary.length}</b></span>
      <span class="chip">R <b>{draft.rightBoundary.length}</b></span>
      <span class="chip">ring <b>{ringAnchors.length}</b></span>
      <span class="chip">split <b>{draft.splitGates.length}</b></span>
      <span class="chip sel">{selectedLabel()}</span>
    </div>

    <div class="map-wrap">
      {#if mapUsable}
        <div class="leaflet-host" bind:this={mapHost}></div>
      {:else}
        <div class="cal-guard">
          <p class="cal-title">Map isn’t calibrated yet</p>
          <p class="cal-body">
            Mapping a drift zone needs the calibrated game map. Calibrate it once in
            Settings, then this editor can place points against the real terrain.
          </p>
          <button class="primary" onclick={() => onOpenSettings?.()}>Open calibration settings →</button>
        </div>
      {/if}
    </div>

    <footer>
      <span class="status">{status}</span>
      <button class="primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save zone'}</button>
    </footer>
  </section>
</div>

<style>
  .editor {
    min-height: 0;
    display: grid;
    grid-template-columns: 280px 1fr;
    background: var(--bg-body);
    overflow: hidden;
  }
  .sidebar {
    border-right: 1px solid var(--bd-subtle);
    background: var(--bg-panel);
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .side-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.75rem 0.9rem;
    border-bottom: 1px solid var(--bd-dim);
    flex: none;
  }
  .zone-list {
    padding: 0.55rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    overflow-y: auto;
    flex: 1;
    min-height: 4rem;
  }
  .zone-row {
    text-align: left;
    border: 1px solid var(--bd-dim);
    background: var(--bg-card);
    color: var(--tx-mid);
    border-radius: var(--r-md);
    padding: 0.5rem 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    cursor: pointer;
    font-family: inherit;
    font-size: 0.8rem;
    font-weight: 550;
  }
  .zone-row:hover {
    border-color: var(--bd-muted);
  }
  .zone-row.active {
    border-color: color-mix(in srgb, var(--ac) 55%, var(--bg-panel));
    box-shadow: inset 2px 0 0 var(--ac);
  }
  .zone-row small {
    color: var(--tx-dim);
    font-size: 0.64rem;
  }
  .zone-meta {
    border-top: 1px solid var(--bd-subtle);
    padding: 0.6rem 0.7rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    flex: none;
    background: var(--bg-panel);
  }
  .meta-head {
    color: var(--tx-mid);
    font-size: 0.7rem;
    font-weight: 600;
  }
  .meta-row {
    display: flex;
    gap: 0.7rem;
    align-items: end;
  }
  .main-panel {
    min-height: 0;
    overflow: hidden;
    padding: 0.7rem 1rem 0.85rem;
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    color: var(--tx-lo);
    font-size: 0.74rem;
  }
  .num input {
    width: 5rem;
  }
  .checkbox {
    flex-direction: row;
    align-items: center;
    gap: 0.4rem;
    padding-bottom: 0.42rem;
    accent-color: var(--ac);
  }
  input {
    background: var(--bg-card);
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-sm);
    color: var(--tx-hi);
    padding: 0.4rem 0.55rem;
    font-family: inherit;
    font-size: 0.8rem;
  }
  input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--ac) 60%, var(--bg-panel));
  }

  /* ── Grouped, data-logger toolbar ── */
  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
    flex: none;
  }
  .group {
    display: flex;
    align-items: center;
    gap: 0.3rem;
  }
  .group.capture {
    margin-left: auto;
    gap: 0.45rem;
  }
  .divider {
    width: 1px;
    align-self: stretch;
    min-height: 1.5rem;
    background: var(--bd-subtle);
    margin: 0 0.1rem;
  }
  .segmented {
    display: flex;
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .segmented button {
    border: 0;
    border-radius: 0;
  }
  .segmented button.active {
    background: var(--ac);
    color: var(--bg-body);
    font-weight: 650;
  }
  button {
    background: var(--bg-elevated);
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-sm);
    color: var(--tx-mid);
    cursor: pointer;
    padding: 0.4rem 0.7rem;
    font-family: inherit;
    font-size: 0.74rem;
  }
  button:hover:not(:disabled) {
    border-color: var(--bd-strong);
    color: var(--tx-hi);
  }
  button:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .group button.active {
    border-color: color-mix(in srgb, var(--ac) 55%, var(--bg-panel));
    color: var(--ac);
    background: var(--ac-wash);
  }
  button.subtle {
    background: none;
    color: var(--tx-dim);
    border-color: var(--bd-muted);
    font-size: 0.68rem;
  }
  button.subtle:hover:not(:disabled) {
    color: var(--tx-mid);
  }
  button.ghost {
    background: none;
    border: 1px solid var(--bd-muted);
    color: var(--tx-dim);
    font-size: 0.62rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    padding: 0.22rem 0.5rem;
  }
  button.ghost:hover:not(:disabled) {
    color: var(--tx-hi);
    border-color: var(--bd-strong);
  }
  .btn-sm {
    padding: 0.22rem 0.5rem;
    font-size: 0.66rem;
  }
  .live {
    color: var(--tx-dim);
    font-size: 0.66rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  /* ── Split-gate sub-selector ── */
  .splitbar {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    flex-wrap: wrap;
    padding: 0.32rem 0.45rem;
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-sm);
    flex: none;
  }
  .splitbar .cap {
    margin-right: 0.2rem;
  }
  .endcap {
    min-width: 1.5rem;
    text-align: center;
    font-family: var(--font-mono);
    font-size: 0.66rem;
    font-weight: 700;
    padding: 0.22rem 0.5rem;
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-sm);
    background: var(--bg-elevated);
  }
  .endcap.a {
    color: var(--gate-a);
    border-color: color-mix(in srgb, var(--gate-a) 45%, var(--bd-muted));
  }
  .endcap.b {
    color: var(--gate-b);
    border-color: color-mix(in srgb, var(--gate-b) 45%, var(--bd-muted));
  }
  .sector-name {
    width: 7rem;
    padding: 0.2rem 0.45rem;
    font-size: 0.68rem;
  }
  .splitnode {
    min-width: 1.5rem;
    text-align: center;
    color: var(--gate-split);
    border-color: color-mix(in srgb, var(--gate-split) 40%, var(--bd-muted));
    background: var(--bg-elevated);
  }
  .splitnode.active {
    color: var(--tx-hi);
    border-color: color-mix(in srgb, var(--gate-split) 70%, var(--bg-panel));
    background: color-mix(in srgb, var(--gate-split) 18%, transparent);
  }
  .splitnode.incomplete {
    border-style: dashed;
    opacity: 0.7;
  }
  .gatechip.del {
    color: var(--bad-tx);
    border-color: color-mix(in srgb, var(--bad) 40%, var(--bg-panel));
  }
  .gatechip.del:hover:not(:disabled) {
    border-color: var(--bad);
  }

  /* ── Status chips ── */
  .chips {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    flex-wrap: wrap;
    flex: none;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-xs);
    padding: 0.18rem 0.45rem;
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--tx-dim);
  }
  .chip b {
    font-family: var(--font-mono);
    font-size: 0.72rem;
    color: var(--tx-mid);
    font-weight: 600;
  }
  .chip.sel {
    margin-left: auto;
    text-transform: none;
    letter-spacing: 0;
    color: var(--tx-lo);
  }

  /* ── Map (the hero) ── */
  .map-wrap {
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--bg-body);
    flex: 1;
    min-height: 380px;
    /* Keep leaflet's pane z-indexes from escaping above app overlays. */
    isolation: isolate;
  }
  .leaflet-host {
    width: 100%;
    height: 100%;
    min-height: 380px;
  }
  .cal-guard {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.7rem;
    height: 100%;
    min-height: 380px;
    text-align: center;
    padding: 2rem;
  }
  .cal-title {
    color: var(--tx-hi);
    font-size: 0.95rem;
    font-weight: 650;
  }
  .cal-body {
    color: var(--tx-dim);
    font-size: 0.78rem;
    line-height: 1.5;
    max-width: 42ch;
  }

  /* ── Markers ── */
  :global(.zone-marker) {
    background: none;
    border: none;
  }
  :global(.zone-marker span) {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 999px;
    border: 2px solid var(--bg-body);
    color: var(--bg-body);
    font-size: 0.55rem;
    font-weight: 800;
    box-shadow: 0 1px 5px rgba(0, 0, 0, 0.45);
  }
  :global(.zone-marker-left span) {
    background: var(--map-left);
  }
  :global(.zone-marker-right span) {
    background: var(--map-right);
  }
  :global(.zone-marker-ring span) {
    background: var(--scoring-ring);
  }
  :global(.zone-marker-split span) {
    background: var(--gate-split);
  }
  :global(.zone-marker-live span) {
    background: var(--live-dot);
    color: var(--bg-body);
  }
  :global(.zone-marker-selected span) {
    outline: 3px solid var(--ac-bright);
    outline-offset: 2px;
  }
  :global(.leaflet-container) {
    background: var(--bg-card);
    font: inherit;
  }

  /* ── Footer ── */
  footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.5rem;
    flex: none;
  }
  .status {
    margin-right: auto;
    color: var(--tx-dim);
    font-size: 0.74rem;
  }
  .empty {
    color: var(--tx-dim);
    font-size: 0.7rem;
    padding: 0.4rem;
  }
  .danger-link {
    align-self: flex-start;
    margin-top: 0.1rem;
    background: none;
    border: none;
    color: var(--tx-dim);
    font-size: 0.66rem;
    padding: 0.15rem 0;
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  .danger-link:hover:not(:disabled) {
    color: var(--bad-tx);
    border-color: transparent;
  }
  .primary {
    background: var(--ac);
    border-color: var(--ac);
    color: var(--bg-body);
    font-weight: 650;
  }
  .primary:hover:not(:disabled) {
    background: var(--ac-bright);
    border-color: var(--ac-bright);
    color: var(--bg-body);
  }
</style>

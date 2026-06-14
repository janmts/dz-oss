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
    zoneCurveMode,
    type ScoringRegion,
    type ZoneCurveMode,
  } from '$lib/curve';

  // 'ring' is the closed scoring region; it reuses the same point-edit machinery
  // as the two open road-edge boundaries (the name is kept for minimal churn).
  type BoundarySide = 'left' | 'right' | 'ring';

  const width = 1000;
  const height = 640;
  const pad = 48;
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

  function syncFromConfig() {
    const v = draft.scoringConfig?.boundarySlackM;
    slackM = typeof v === 'number' ? v : 3;
    curveMode = zoneCurveMode(draft.scoringConfig);
    ringAnchors = sharedScoringRing(draft.scoringConfig).map((p) => ({ ...p }));
  }
  let dragging = $state<{ side: BoundarySide; index: number } | null>(null);
  let selectedPoint = $state<{ side: BoundarySide; index: number } | null>(null);
  let svgEl = $state<SVGSVGElement | null>(null);
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
  let unsubscribeShortcut: (() => void) | null = null;
  let lastKnownPoint = $state<ZonePoint | null>(null);
  let lastKnownAt = $state(0);
  let mapReady = $state(false);

  onMount(async () => {
    void loadDriftZones();
    unsubscribeShortcut = await ipc.subscribeDriftZoneCapture(({ side }) => {
      capturePoint(side ?? activeSide, true);
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

  let transform = $derived.by(() => {
    if (allPoints.length === 0) {
      return {
        minX: -50,
        maxX: 50,
        minZ: -50,
        maxZ: 50,
        scale: 1,
      };
    }
    let minX = Infinity, maxX = -Infinity, minZ = Infinity, maxZ = -Infinity;
    for (const p of allPoints) {
      minX = Math.min(minX, p.x);
      maxX = Math.max(maxX, p.x);
      minZ = Math.min(minZ, p.z);
      maxZ = Math.max(maxZ, p.z);
    }
    const spanX = Math.max(1, maxX - minX);
    const spanZ = Math.max(1, maxZ - minZ);
    const scale = Math.min((width - pad * 2) / spanX, (height - pad * 2) / spanZ);
    const extraX = ((width - pad * 2) / scale - spanX) / 2;
    const extraZ = ((height - pad * 2) / scale - spanZ) / 2;
    return {
      minX: minX - extraX,
      maxX: maxX + extraX,
      minZ: minZ - extraZ,
      maxZ: maxZ + extraZ,
      scale,
    };
  });

  function toSvg(p: ZonePoint): [number, number] {
    const x = pad + (p.x - transform.minX) * transform.scale;
    const y = pad + (transform.maxZ - p.z) * transform.scale;
    return [x, y];
  }

  function fromClient(e: MouseEvent | PointerEvent): ZonePoint {
    const rect = svgEl!.getBoundingClientRect();
    const sx = ((e.clientX - rect.left) / rect.width) * width;
    const sy = ((e.clientY - rect.top) / rect.height) * height;
    return {
      x: transform.minX + (sx - pad) / transform.scale,
      z: transform.maxZ - (sy - pad) / transform.scale,
    };
  }

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
      side === 'left' ? 'zone-marker-left' : side === 'right' ? 'zone-marker-right' : 'zone-marker-ring';
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

    if (draft.leftBoundary.length > 1) {
      leftLine = gm.addLine(boundaryLatLngs('left'), 5, {
        color: themeColor('--map-left', '#84b577'),
        opacity: 0.95,
      });
    }
    if (draft.rightBoundary.length > 1) {
      rightLine = gm.addLine(boundaryLatLngs('right'), 5, {
        color: themeColor('--map-right', '#82a7c8'),
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
        color: themeColor('--violet', '#a995cf'),
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
      marker.on('drag', () => updateLinesDuringDrag('ring', index, marker.getLatLng()));
      marker.on('dragend', () => {
        const points = [...boundary('ring')];
        points[index] = latLngToWorld(marker.getLatLng());
        setBoundary('ring', points);
        selectPoint('ring', index);
      });
      marker.on('dblclick', () => deletePoint('ring', index));
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

  function path(points: ZonePoint[]): string {
    return points
      .map((p) => {
        const [x, y] = toSvg(p);
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(' ');
  }

  function selectZone(zone: DriftZoneRow) {
    draft = toInput(zone);
    selectedId = zone.id;
    selectedPoint = null;
    status = '';
    syncFromConfig();
    setTimeout(fitMapToGeometry, 0);
  }

  function newZone() {
    draft = blankZone();
    selectedId = null;
    selectedPoint = null;
    status = '';
    syncFromConfig();
  }

  function boundary(side: BoundarySide): ZonePoint[] {
    if (side === 'ring') return ringAnchors;
    return side === 'left' ? draft.leftBoundary : draft.rightBoundary;
  }

  function setBoundary(side: BoundarySide, points: ZonePoint[]) {
    if (side === 'ring') ringAnchors = points;
    else if (side === 'left') draft.leftBoundary = points;
    else draft.rightBoundary = points;
  }

  function appendBoundaryPoint(side: BoundarySide, point: ZonePoint, source: 'map' | 'telemetry') {
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

  function onSvgMapPointerUp(e: PointerEvent) {
    if (!svgEl) return;
    appendBoundaryPoint(activeSide, fromClient(e), 'map');
  }

  function selectedBoundaryPointLabel(): string {
    if (!selectedPoint) return 'No point selected';
    const prefix = selectedPoint.side === 'left' ? 'L' : selectedPoint.side === 'ring' ? 'S' : 'R';
    return `${prefix}${selectedPoint.index + 1}`;
  }

  function capturePoint(side: BoundarySide = activeSide, fromShortcut = false) {
    if (!livePoint) {
      status = 'Waiting for the first telemetry world position. Drive briefly, then reopen/click capture.';
      return;
    }
    appendBoundaryPoint(side, livePoint, 'telemetry');
    if (fromShortcut && side !== activeSide) activeSide = side;
    const shortcutLabel = fromShortcut ? ' via shortcut' : '';
    if (shortcutLabel) status = `${status.replace(/\.$/, '')}${shortcutLabel}.`;
    setTimeout(fitMapToGeometry, 0);
  }

  function insertAtSelected(offset: 0 | 1) {
    if (!selectedPoint) {
      status = 'Select a boundary point first.';
      return;
    }
    if (!livePoint) {
      status = 'Waiting for the first telemetry world position before inserting.';
      return;
    }
    const points = [...boundary(selectedPoint.side)];
    const insertIndex = selectedPoint.index + offset;
    points.splice(insertIndex, 0, { ...livePoint });
    setBoundary(selectedPoint.side, points);
    selectedPoint = { side: selectedPoint.side, index: insertIndex };
    activeSide = selectedPoint.side;
    status = `Inserted ${selectedBoundaryPointLabel()} from last known telemetry.`;
    setTimeout(fitMapToGeometry, 0);
  }

  function onLocalKeydown(e: KeyboardEvent) {
    if (!e.ctrlKey || !e.altKey) return;
    const key = e.key.toLowerCase();
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
  }

  function removeLastPoint() {
    const points = boundary(activeSide);
    if (points.length === 0) return;
    setBoundary(activeSide, points.slice(0, -1));
    if (selectedPoint?.side === activeSide && selectedPoint.index >= points.length - 1) {
      selectedPoint = points.length > 1 ? { side: activeSide, index: points.length - 2 } : null;
    }
  }

  function reverseBoundaries() {
    draft.leftBoundary = [...draft.leftBoundary].reverse();
    draft.rightBoundary = [...draft.rightBoundary].reverse();
    // Only the left/right arrays were reversed — don't remap a ring selection.
    if (selectedPoint && selectedPoint.side !== 'ring') {
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
    ringAnchors = [
      ...draft.leftBoundary.map((p) => ({ ...p })),
      ...draft.rightBoundary.map((p) => ({ ...p })).reverse(),
    ];
    activeSide = 'ring';
    selectedPoint = null;
    status = `Seeded scoring ring from boundary (${ringAnchors.length} points). Drag inward to tighten.`;
  }

  function deletePoint(side: BoundarySide, index: number) {
    setBoundary(side, boundary(side).filter((_, i) => i !== index));
    if (selectedPoint?.side === side) {
      const nextLen = boundary(side).length;
      selectedPoint = nextLen === 0 ? null : { side, index: Math.min(index, nextLen - 1) };
    }
  }

  function selectPoint(side: BoundarySide, index: number) {
    selectedPoint = { side, index };
    activeSide = side;
  }

  function onPointKeydown(e: KeyboardEvent, side: BoundarySide, index: number) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      selectPoint(side, index);
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      deletePoint(side, index);
    }
  }

  function startDrag(e: PointerEvent, side: BoundarySide, index: number) {
    e.preventDefault();
    e.stopPropagation();
    selectPoint(side, index);
    dragging = { side, index };
    (e.currentTarget as SVGElement).setPointerCapture(e.pointerId);
  }

  function moveDrag(e: PointerEvent) {
    if (!dragging || !svgEl) return;
    const points = [...boundary(dragging.side)];
    points[dragging.index] = fromClient(e);
    setBoundary(dragging.side, points);
  }

  function stopDrag() {
    dragging = null;
  }

  function sideLabel(side: BoundarySide) {
    return side === 'left' ? 'Left boundary' : 'Right boundary';
  }

  function gateLine(points: ZonePoint[]): string {
    return points.length === 2 ? path(points) : '';
  }

  async function save() {
    saving = true;
    // Persist slack + curve mode + the scoring ring into the per-zone config bag.
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
    draft.scoringConfig = cfg;
    try {
      const saved = await saveDriftZone(draft);
      draft = toInput(saved);
      selectedId = saved.id;
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
  </aside>

  <section class="main-panel">
    <header class="head">
      <div>
        <h2>{selectedId ? 'Edit Drift Zone' : 'New Drift Zone'}</h2>
        <p>Click the map or capture live telemetry to add road-edge points, then drag to refine.</p>
      </div>
    </header>

    <div class="form-row">
      <label>
        <span class="cap">Name</span>
        <input bind:value={draft.name} />
      </label>
      <label class="num" title="Metres a run may stray past the boundary before it voids. ~3 m suits most zones; raise it for sparsely-mapped or wide zones.">
        <span class="cap">Slack (m)</span>
        <input class="mono" type="number" min="0" step="0.5" bind:value={slackM} />
      </label>
      <label class="checkbox">
        <input type="checkbox" bind:checked={draft.active} />
        Active
      </label>
      <label class="checkbox" title="Draw and score this zone's boundary as a smooth centripetal Catmull-Rom curve through the anchors — the display matches scoring. Off = straight segments between points.">
        <input
          type="checkbox"
          checked={curveMode === 'catmull'}
          onchange={(e) => (curveMode = e.currentTarget.checked ? 'catmull' : 'linear')}
        />
        Smooth
      </label>
    </div>

    <label>
      <span class="cap">Description</span>
      <input bind:value={draft.description} placeholder="Optional notes for this zone" />
    </label>

    <div class="toolbar">
      <div class="segmented">
        <button class:active={activeSide === 'left'} onclick={() => (activeSide = 'left')}>Left</button>
        <button class:active={activeSide === 'right'} onclick={() => (activeSide = 'right')}>Right</button>
        <button class:active={activeSide === 'ring'} disabled={!mapUsable} onclick={() => (activeSide = 'ring')}>Ring</button>
      </div>
      <button onclick={() => capturePoint()}>Capture live point</button>
      <button onclick={() => insertAtSelected(0)}>Insert before</button>
      <button onclick={() => insertAtSelected(1)}>Insert after</button>
      <button onclick={removeLastPoint}>Remove last {activeSide}</button>
      <button onclick={reverseBoundaries}>Reverse direction</button>
      <button onclick={seedRingFromBoundary} disabled={!mapUsable} title="Copy left ++ reversed-right into the scoring ring, then drag anchors inward to tighten it.">Seed ring</button>
      <button onclick={fitMapToGeometry}>Fit map</button>
      <span class="live mono">
        {livePoint
          ? `Last X ${livePoint.x.toFixed(1)} / Z ${livePoint.z.toFixed(1)}${liveAgeSecs > 2 ? ` (${liveAgeSecs.toFixed(0)}s old)` : ''}`
          : 'Waiting for telemetry'}
      </span>
    </div>

    <p class="shortcut-hint">
      Global shortcuts while FH6 has focus: <strong>Ctrl+Alt+Z</strong> captures selected side,
      <strong>Ctrl+Alt+L</strong> captures left, <strong>Ctrl+Alt+R</strong> captures right.
      Selected point: <strong>{selectedBoundaryPointLabel()}</strong>.
    </p>

    <div class="map-wrap">
      {#if mapUsable}
        <div class="leaflet-host" bind:this={mapHost}></div>
      {:else}
        <div class="fallback-note">Map calibration unavailable; showing uncalibrated world-coordinate editor.</div>
        <svg
          bind:this={svgEl}
          viewBox={`0 0 ${width} ${height}`}
          onpointermove={moveDrag}
          onpointerup={stopDrag}
          onpointercancel={stopDrag}
          role="img"
          aria-label="Drift zone boundary editor"
        >
          <rect
            class="map-bg"
            x="0"
            y="0"
            width={width}
            height={height}
            role="presentation"
            onpointerup={onSvgMapPointerUp}
          />

          {#if draft.leftBoundary.length > 1}
            <polyline class="boundary left" points={path(draft.leftBoundary)} />
          {/if}
          {#if draft.rightBoundary.length > 1}
            <polyline class="boundary right" points={path(draft.rightBoundary)} />
          {/if}

          {#if draft.leftBoundary.length > 0 && draft.rightBoundary.length > 0}
            <polyline
              class="gate start"
              points={gateLine([draft.leftBoundary[0], draft.rightBoundary[0]])}
            />
            <polyline
              class="gate finish"
              points={gateLine([
                draft.leftBoundary[draft.leftBoundary.length - 1],
                draft.rightBoundary[draft.rightBoundary.length - 1],
              ])}
            />
          {/if}

          {#if livePoint}
            {@const live = toSvg(livePoint)}
            <circle class="live-dot" cx={live[0]} cy={live[1]} r="7" />
          {/if}

          {#each draft.leftBoundary as point, index}
            {@const p = toSvg(point)}
            <g class="point-group">
              <circle
                class="point left"
                cx={p[0]}
                cy={p[1]}
                r="8"
                role="button"
                tabindex="0"
                aria-label={`Select left point ${index + 1}`}
                class:selected={selectedPoint?.side === 'left' && selectedPoint.index === index}
                onpointerdown={(e) => startDrag(e, 'left', index)}
                onkeydown={(e) => onPointKeydown(e, 'left', index)}
                ondblclick={() => deletePoint('left', index)}
              />
              <text x={p[0] + 12} y={p[1] - 10}>L{index + 1}</text>
            </g>
          {/each}

          {#each draft.rightBoundary as point, index}
            {@const p = toSvg(point)}
            <g class="point-group">
              <circle
                class="point right"
                cx={p[0]}
                cy={p[1]}
                r="8"
                role="button"
                tabindex="0"
                aria-label={`Select right point ${index + 1}`}
                class:selected={selectedPoint?.side === 'right' && selectedPoint.index === index}
                onpointerdown={(e) => startDrag(e, 'right', index)}
                onkeydown={(e) => onPointKeydown(e, 'right', index)}
                ondblclick={() => deletePoint('right', index)}
              />
              <text x={p[0] + 12} y={p[1] - 10}>R{index + 1}</text>
            </g>
          {/each}
        </svg>
      {/if}
    </div>

    <div class="details">
      <div><strong>{sideLabel('left')}</strong><span class="mono">{draft.leftBoundary.length} points</span></div>
      <div><strong>{sideLabel('right')}</strong><span class="mono">{draft.rightBoundary.length} points</span></div>
      <div><strong>End gates</strong><span>First &amp; last point pairs — enter either, exit the other</span></div>
      <div><strong>Editing</strong><span>Click map to add; drag points to refine</span></div>
    </div>

    <footer>
      <span class="status">{status}</span>
      <button class="danger" disabled={!selectedId} onclick={removeZone}>Delete</button>
      <button class="primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save zone'}</button>
    </footer>
  </section>
</div>

<style>
  .editor {
    min-height: 0;
    display: grid;
    grid-template-columns: 240px 1fr;
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
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.85rem 0 0.25rem;
  }
  h2 {
    margin: 0;
    color: var(--tx-hi);
    font-size: 0.95rem;
    font-weight: 650;
  }
  .head p {
    margin-top: 0.2rem;
    color: var(--tx-dim);
    font-size: 0.74rem;
  }
  .zone-list {
    padding: 0.55rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    overflow-y: auto;
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
  .main-panel {
    min-height: 0;
    overflow-y: auto;
    padding: 0 1rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    color: var(--tx-lo);
    font-size: 0.75rem;
  }
  .form-row {
    display: grid;
    grid-template-columns: 1fr auto auto auto;
    gap: 0.75rem;
    align-items: end;
  }
  .num input {
    width: 5.5rem;
  }
  .checkbox {
    flex-direction: row;
    align-items: center;
    padding-bottom: 0.42rem;
    accent-color: var(--ac);
  }
  input {
    background: var(--bg-card);
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-sm);
    color: var(--tx-hi);
    padding: 0.42rem 0.55rem;
    font-family: inherit;
    font-size: 0.8rem;
  }
  input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--ac) 60%, var(--bg-panel));
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    flex-wrap: wrap;
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
    opacity: 0.45;
    cursor: default;
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
  .live {
    margin-left: auto;
    color: var(--tx-dim);
    font-size: 0.66rem;
  }
  .shortcut-hint {
    color: var(--tx-dim);
    font-size: 0.7rem;
    line-height: 1.45;
    margin-top: -0.2rem;
  }
  .shortcut-hint strong {
    color: var(--tx-lo);
    font-weight: 600;
    font-family: var(--font-mono);
    font-size: 0.66rem;
  }
  .map-wrap {
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--bg-body);
    /* Keep leaflet's pane z-indexes from escaping above app overlays. */
    isolation: isolate;
  }
  svg {
    display: block;
    width: 100%;
    height: min(56vh, 640px);
    touch-action: none;
  }
  .leaflet-host {
    width: 100%;
    height: min(56vh, 640px);
    min-height: 380px;
  }
  .fallback-note {
    padding: 0.45rem 0.6rem;
    color: var(--tx-dim);
    font-size: 0.7rem;
    border-bottom: 1px solid var(--bd-dim);
    background: var(--bg-card);
  }
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
    background: var(--violet);
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
  .map-bg {
    fill: var(--bg-body);
    cursor: crosshair;
  }
  .boundary {
    fill: none;
    stroke-width: 5;
    stroke-linecap: round;
    stroke-linejoin: round;
    pointer-events: none;
  }
  .boundary.left {
    stroke: var(--map-left);
  }
  .boundary.right {
    stroke: var(--map-right);
  }
  .gate {
    fill: none;
    stroke-width: 3;
    stroke-dasharray: 10 7;
    pointer-events: none;
  }
  .gate.start {
    stroke: var(--gate-a);
  }
  .gate.finish {
    stroke: var(--gate-b);
  }
  .point {
    stroke: var(--bg-body);
    stroke-width: 2;
    cursor: grab;
  }
  .point.left {
    fill: var(--map-left);
  }
  .point.right {
    fill: var(--map-right);
  }
  .point:active {
    cursor: grabbing;
  }
  .point.selected {
    stroke: var(--ac-bright);
    stroke-width: 4;
  }
  .point-group text {
    fill: var(--tx-lo);
    font-size: 18px;
    user-select: none;
    pointer-events: none;
  }
  .live-dot {
    fill: var(--live-dot);
    stroke: var(--bg-body);
    stroke-width: 2;
    pointer-events: none;
  }
  .details {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.5rem;
  }
  .details div {
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-md);
    padding: 0.55rem;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .details strong {
    color: var(--tx-mid);
    font-size: 0.7rem;
    font-weight: 600;
  }
  .details span, .empty {
    color: var(--tx-dim);
    font-size: 0.66rem;
  }
  footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.5rem;
    padding-top: 0.25rem;
  }
  .status {
    margin-right: auto;
    color: var(--tx-dim);
    font-size: 0.74rem;
  }
  .danger {
    color: var(--bad-tx);
    border-color: color-mix(in srgb, var(--bad) 45%, var(--bg-panel));
  }
  .danger:hover:not(:disabled) {
    color: var(--bad-tx);
    border-color: var(--bad);
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

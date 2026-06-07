<script lang="ts">
  import { onMount } from 'svelte';
  import { packet } from '$lib/stores/telemetry';
  import {
    deleteDriftZone,
    driftZones,
    loadDriftZones,
    saveDriftZone,
  } from '$lib/stores/sessions';
  import type { DriftZoneInput, DriftZoneRow, ZonePoint } from '$lib/types';

  let { onClose }: { onClose: () => void } = $props();

  type BoundarySide = 'left' | 'right';

  const width = 1000;
  const height = 640;
  const pad = 48;

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
  let dragging = $state<{ side: BoundarySide; index: number } | null>(null);
  let svgEl = $state<SVGSVGElement | null>(null);

  onMount(() => {
    void loadDriftZones();
  });

  let livePoint = $derived.by<ZonePoint | null>(() => {
    const p = $packet;
    if (!p || (p.positionX === 0 && p.positionZ === 0)) return null;
    return { x: p.positionX, z: p.positionZ };
  });

  let allPoints = $derived([
    ...draft.leftBoundary,
    ...draft.rightBoundary,
    ...draft.startGate,
    ...draft.finishGate,
    ...draft.splitGates.flat(),
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

  function fromClient(e: PointerEvent): ZonePoint {
    const rect = svgEl!.getBoundingClientRect();
    const sx = ((e.clientX - rect.left) / rect.width) * width;
    const sy = ((e.clientY - rect.top) / rect.height) * height;
    return {
      x: transform.minX + (sx - pad) / transform.scale,
      z: transform.maxZ - (sy - pad) / transform.scale,
    };
  }

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
    status = '';
  }

  function newZone() {
    draft = blankZone();
    selectedId = null;
    status = '';
  }

  function boundary(side: BoundarySide): ZonePoint[] {
    return side === 'left' ? draft.leftBoundary : draft.rightBoundary;
  }

  function setBoundary(side: BoundarySide, points: ZonePoint[]) {
    if (side === 'left') draft.leftBoundary = points;
    else draft.rightBoundary = points;
  }

  function capturePoint() {
    if (!livePoint) {
      status = 'No live world position available.';
      return;
    }
    setBoundary(activeSide, [...boundary(activeSide), { ...livePoint }]);
    status = `Captured ${activeSide} point ${boundary(activeSide).length}.`;
  }

  function removeLastPoint() {
    const points = boundary(activeSide);
    if (points.length === 0) return;
    setBoundary(activeSide, points.slice(0, -1));
  }

  function reverseBoundaries() {
    draft.leftBoundary = [...draft.leftBoundary].reverse();
    draft.rightBoundary = [...draft.rightBoundary].reverse();
  }

  function deletePoint(side: BoundarySide, index: number) {
    setBoundary(side, boundary(side).filter((_, i) => i !== index));
  }

  function startDrag(e: PointerEvent, side: BoundarySide, index: number) {
    e.preventDefault();
    e.stopPropagation();
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

<div class="overlay" role="dialog" aria-modal="true">
  <div class="editor">
    <aside class="sidebar">
      <div class="side-head">
        <h2>Drift Zones</h2>
        <button onclick={newZone}>New</button>
      </div>

      <div class="zone-list">
        {#each $driftZones as zone}
          <button
            class="zone-row"
            class:active={zone.id === selectedId}
            onclick={() => selectZone(zone)}
          >
            <span>{zone.name}</span>
            <small>{zone.leftBoundary.length}L / {zone.rightBoundary.length}R</small>
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
          <p>Capture both road edges from live telemetry, then refine points by dragging them.</p>
        </div>
        <button class="close" onclick={onClose}>✕</button>
      </header>

      <div class="form-row">
        <label>
          Name
          <input bind:value={draft.name} />
        </label>
        <label class="checkbox">
          <input type="checkbox" bind:checked={draft.active} />
          Active
        </label>
      </div>

      <label>
        Description
        <input bind:value={draft.description} placeholder="Optional notes for this zone" />
      </label>

      <div class="toolbar">
        <div class="segmented">
          <button class:active={activeSide === 'left'} onclick={() => (activeSide = 'left')}>Left</button>
          <button class:active={activeSide === 'right'} onclick={() => (activeSide = 'right')}>Right</button>
        </div>
        <button onclick={capturePoint}>Capture live point</button>
        <button onclick={removeLastPoint}>Remove last {activeSide}</button>
        <button onclick={reverseBoundaries}>Reverse direction</button>
        <span class="live">
          {livePoint ? `Live X ${livePoint.x.toFixed(1)} / Z ${livePoint.z.toFixed(1)}` : 'No live position'}
        </span>
      </div>

      <div class="map-wrap">
        <svg
          bind:this={svgEl}
          viewBox={`0 0 ${width} ${height}`}
          onpointermove={moveDrag}
          onpointerup={stopDrag}
          onpointercancel={stopDrag}
          role="img"
          aria-label="Drift zone boundary editor"
        >
          <rect x="0" y="0" width={width} height={height} />

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
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <circle
                class="point left"
                cx={p[0]}
                cy={p[1]}
                r="8"
                onpointerdown={(e) => startDrag(e, 'left', index)}
                ondblclick={() => deletePoint('left', index)}
              />
              <text x={p[0] + 12} y={p[1] - 10}>L{index + 1}</text>
            </g>
          {/each}

          {#each draft.rightBoundary as point, index}
            {@const p = toSvg(point)}
            <g class="point-group">
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <circle
                class="point right"
                cx={p[0]}
                cy={p[1]}
                r="8"
                onpointerdown={(e) => startDrag(e, 'right', index)}
                ondblclick={() => deletePoint('right', index)}
              />
              <text x={p[0] + 12} y={p[1] - 10}>R{index + 1}</text>
            </g>
          {/each}
        </svg>
      </div>

      <div class="details">
        <div><strong>{sideLabel('left')}</strong><span>{draft.leftBoundary.length} points</span></div>
        <div><strong>{sideLabel('right')}</strong><span>{draft.rightBoundary.length} points</span></div>
        <div><strong>Start/finish</strong><span>Derived from first/last left+right points</span></div>
        <div><strong>Editing</strong><span>Drag points to refine; double-click to delete</span></div>
      </div>

      <footer>
        <span class="status">{status}</span>
        <button class="danger" disabled={!selectedId} onclick={removeZone}>Delete</button>
        <button class="primary" disabled={saving} onclick={save}>{saving ? 'Saving…' : 'Save zone'}</button>
      </footer>
    </section>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.76);
    display: flex;
    align-items: stretch;
    justify-content: center;
    z-index: 120;
    padding: 2rem;
  }
  .editor {
    width: min(1180px, 100%);
    min-height: 0;
    display: grid;
    grid-template-columns: 260px 1fr;
    background: var(--bg-panel);
    border: 1px solid var(--bd-muted);
    border-radius: 12px;
    overflow: hidden;
    box-shadow: 0 20px 80px rgba(0, 0, 0, 0.55);
  }
  .sidebar {
    border-right: 1px solid var(--bd-dim);
    background: var(--bg-card);
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .side-head, .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 1rem;
    border-bottom: 1px solid var(--bd-dim);
  }
  h2 {
    margin: 0;
    color: var(--tx-hi);
    font-size: 1rem;
  }
  .head p {
    margin-top: 0.2rem;
    color: var(--tx-dim);
    font-size: 0.75rem;
  }
  .zone-list {
    padding: 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    overflow-y: auto;
  }
  .zone-row {
    text-align: left;
    border: 1px solid var(--bd-dim);
    background: var(--bg-panel);
    color: var(--tx-mid);
    border-radius: 6px;
    padding: 0.55rem;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  .zone-row.active {
    border-color: var(--ac);
  }
  .zone-row small {
    color: var(--tx-dim);
    font-size: 0.68rem;
  }
  .main-panel {
    min-height: 0;
    overflow-y: auto;
    padding: 0 1rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
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
    grid-template-columns: 1fr auto;
    gap: 0.75rem;
    align-items: end;
  }
  .checkbox {
    flex-direction: row;
    align-items: center;
    padding-bottom: 0.42rem;
  }
  input {
    background: var(--bg-body);
    border: 1px solid var(--bd-muted);
    border-radius: 5px;
    color: var(--tx-hi);
    padding: 0.45rem 0.55rem;
    font: inherit;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .segmented {
    display: flex;
    border: 1px solid var(--bd-muted);
    border-radius: 6px;
    overflow: hidden;
  }
  .segmented button {
    border: 0;
    border-radius: 0;
  }
  .segmented button.active {
    background: var(--ac);
    color: var(--bg-body);
  }
  button {
    background: var(--bg-elevated);
    border: 1px solid var(--bd-muted);
    border-radius: 5px;
    color: var(--tx-mid);
    cursor: pointer;
    padding: 0.42rem 0.75rem;
    font-size: 0.78rem;
  }
  button:hover:not(:disabled) {
    filter: brightness(1.16);
  }
  button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .close {
    padding: 0.2rem 0.45rem;
  }
  .live {
    margin-left: auto;
    color: var(--tx-dim);
    font-size: 0.72rem;
  }
  .map-wrap {
    border: 1px solid var(--bd-dim);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-body);
  }
  svg {
    display: block;
    width: 100%;
    height: min(56vh, 640px);
    touch-action: none;
  }
  rect {
    fill: var(--bg-body);
  }
  .boundary {
    fill: none;
    stroke-width: 5;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .boundary.left {
    stroke: #22c55e;
  }
  .boundary.right {
    stroke: #3b82f6;
  }
  .gate {
    fill: none;
    stroke-width: 3;
    stroke-dasharray: 10 7;
  }
  .gate.start {
    stroke: #f59e0b;
  }
  .gate.finish {
    stroke: #ef4444;
  }
  .point {
    stroke: #020617;
    stroke-width: 2;
    cursor: grab;
  }
  .point.left {
    fill: #22c55e;
  }
  .point.right {
    fill: #3b82f6;
  }
  .point:active {
    cursor: grabbing;
  }
  .point-group text {
    fill: var(--tx-lo);
    font-size: 18px;
    user-select: none;
    pointer-events: none;
  }
  .live-dot {
    fill: #fbbf24;
    stroke: #020617;
    stroke-width: 2;
  }
  .details {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.5rem;
  }
  .details div {
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: 6px;
    padding: 0.55rem;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .details strong {
    color: var(--tx-mid);
    font-size: 0.72rem;
  }
  .details span, .empty {
    color: var(--tx-dim);
    font-size: 0.68rem;
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
    font-size: 0.75rem;
  }
  .danger {
    color: #fca5a5;
    border-color: #7f1d1d;
  }
  .primary {
    background: var(--ac);
    border-color: var(--ac);
    color: var(--bg-body);
  }
</style>

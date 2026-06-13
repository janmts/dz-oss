<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import type { CircleMarker } from 'leaflet';
  import { effectiveMapConfig } from '$lib/mapDefaults';
  import { createGameMap, makeCalib, type GameMap } from '$lib/mapView';
  import { loadDriftRunPackets } from '$lib/stores/sessions';
  import { themeColor } from '$lib/theme';
  import type { AppSettings, DriftRunRow, DriftZoneRow, TelemetryPacket, ZonePoint } from '$lib/types';

  let {
    run,
    zone,
    settings,
    livePacket,
  }: {
    run: DriftRunRow | null;
    zone: DriftZoneRow | null;
    settings: AppSettings | null;
    livePacket: TelemetryPacket | null;
  } = $props();

  const width = 1000;
  const height = 640;
  const pad = 42;
  const tarmacEps = 0.05;
  const displayHz = 16;
  const displayIntervalMs = 1000 / displayHz;
  const hoverWorldRadiusM = 4;
  const wheels = [
    ['FL', 'surfaceRumbleFl'],
    ['FR', 'surfaceRumbleFr'],
    ['RL', 'surfaceRumbleRl'],
    ['RR', 'surfaceRumbleRr'],
  ] as const;

  type WheelKey = (typeof wheels)[number][1];
  type Tick = {
    packet: TelemetryPacket;
    index: number;
    elapsedS: number;
    roughWheels: number;
  };
  type PlotTick = Tick & { x: number; y: number };

  let packets = $state<TelemetryPacket[]>([]);
  let loading = $state(false);
  let error = $state('');
  let hoverTick = $state<Tick | null>(null);
  let svgEl = $state<SVGSVGElement | null>(null);
  let mapHost = $state<HTMLDivElement | null>(null);
  let gm: GameMap | null = null;
  let mapReady = $state(false);
  let hoverMarker: CircleMarker | null = null;
  let lastFitKey: string | null = null;
  let loadSeq = 0;

  let cfg = $derived(settings ? effectiveMapConfig(settings) : null);
  let mapUsable = $derived(!!cfg && !!makeCalib(cfg));

  $effect(() => {
    const id = run?.id;
    const seq = ++loadSeq;
    packets = [];
    hoverTick = null;
    error = '';
    if (!id) return;

    loading = true;
    loadDriftRunPackets(id)
      .then((rows) => {
        if (seq !== loadSeq) return;
        packets = rows;
      })
      .catch((e) => {
        if (seq !== loadSeq) return;
        error = String(e);
      })
      .finally(() => {
        if (seq === loadSeq) loading = false;
      });
  });

  let validPackets = $derived(
    packets
      .map((packet, index) => ({ packet, index }))
      .filter(({ packet }) => packet.positionX !== 0 || packet.positionZ !== 0)
  );

  let firstTimestamp = $derived(packets[0]?.timestampMs ?? 0);

  let livePoint = $derived(
    livePacket && (livePacket.positionX !== 0 || livePacket.positionZ !== 0)
      ? { x: livePacket.positionX, z: livePacket.positionZ }
      : null
  );

  let staticWorldPoints = $derived.by(() => {
    const pts: ZonePoint[] = [
      ...(zone?.leftBoundary ?? []),
      ...(zone?.rightBoundary ?? []),
      ...(zone?.startGate ?? []),
      ...(zone?.finishGate ?? []),
      ...(zone?.splitGates.flat() ?? []),
      ...validPackets.map(({ packet }) => ({ x: packet.positionX, z: packet.positionZ })),
    ];
    return pts;
  });

  let zonePolygon = $derived.by<ZonePoint[]>(() => {
    if (!zone || zone.leftBoundary.length < 2 || zone.rightBoundary.length < 2) return [];
    return [...zone.leftBoundary, ...[...zone.rightBoundary].reverse()];
  });

  let transform = $derived.by(() => {
    if (staticWorldPoints.length === 0) {
      return { minX: -50, maxX: 50, minZ: -50, maxZ: 50, scale: 1 };
    }
    let minX = Infinity;
    let maxX = -Infinity;
    let minZ = Infinity;
    let maxZ = -Infinity;
    for (const p of staticWorldPoints) {
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

  function toSvgPoint(p: ZonePoint): [number, number] {
    return [
      pad + (p.x - transform.minX) * transform.scale,
      pad + (transform.maxZ - p.z) * transform.scale,
    ];
  }

  function path(points: ZonePoint[]) {
    return points
      .map((p) => {
        const [x, y] = toSvgPoint(p);
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(' ');
  }

  function derivedStartGate(z: DriftZoneRow): ZonePoint[] {
    return z.leftBoundary.length && z.rightBoundary.length
      ? [z.leftBoundary[0], z.rightBoundary[0]]
      : [];
  }

  function derivedFinishGate(z: DriftZoneRow): ZonePoint[] {
    return z.leftBoundary.length && z.rightBoundary.length
      ? [z.leftBoundary[z.leftBoundary.length - 1], z.rightBoundary[z.rightBoundary.length - 1]]
      : [];
  }

  function roughWheelCount(packet: TelemetryPacket) {
    return wheels.reduce((n, [, key]) => n + (packet[key] > tarmacEps ? 1 : 0), 0);
  }

  let rawTicks = $derived.by<Tick[]>(() =>
    validPackets.map(({ packet, index }) => {
      return {
        packet,
        index,
        elapsedS: (packet.timestampMs - firstTimestamp) / 1000,
        roughWheels: roughWheelCount(packet),
      };
    })
  );

  let displayTicks = $derived.by<Tick[]>(() => {
    if (rawTicks.length <= 2) return rawTicks;
    const sampled: Tick[] = [];
    let nextVisualMs = -Infinity;
    let lastIndex = -1;

    function push(tick: Tick) {
      if (tick.index === lastIndex) return;
      sampled.push(tick);
      lastIndex = tick.index;
    }

    for (let i = 0; i < rawTicks.length; i++) {
      const tick = rawTicks[i];
      const forceKeep = i === 0 || i === rawTicks.length - 1 || tick.roughWheels > 0;
      if (forceKeep || tick.packet.timestampMs >= nextVisualMs) {
        push(tick);
        if (!forceKeep || tick.packet.timestampMs >= nextVisualMs) {
          nextVisualMs = tick.packet.timestampMs + displayIntervalMs;
        }
      }
    }

    return sampled;
  });

  let plotTicks = $derived.by<PlotTick[]>(() =>
    displayTicks.map((tick) => {
      const [x, y] = toSvgPoint({ x: tick.packet.positionX, z: tick.packet.positionZ });
      return { ...tick, x, y };
    })
  );

  let tracePoints = $derived(plotTicks.map((t) => `${t.x.toFixed(1)},${t.y.toFixed(1)}`).join(' '));
  let hoverPlotPoint = $derived.by(() => {
    if (!hoverTick) return null;
    const [x, y] = toSvgPoint({ x: hoverTick.packet.positionX, z: hoverTick.packet.positionZ });
    return { x, y };
  });
  let hoverOob = $derived(hoverTick ? packetOob(hoverTick.packet) : null);

  function wheelRows(packet: TelemetryPacket) {
    return wheels.map(([label, key]) => {
      const value = packet[key];
      return {
        label,
        value,
        state: value <= tarmacEps ? 'smooth' : 'rough',
      };
    });
  }

  function signedAngle(packet: TelemetryPacket) {
    return (Math.atan2(packet.velX, packet.velZ) * 180) / Math.PI;
  }

  function formatNum(n: number, digits = 2) {
    return Number.isFinite(n) ? n.toFixed(digits) : '-';
  }

  function pointInPolygon(point: ZonePoint, polygon: ZonePoint[]) {
    if (polygon.length < 3) return false;
    let inside = false;
    let j = polygon.length - 1;
    for (let i = 0; i < polygon.length; i++) {
      const pi = polygon[i];
      const pj = polygon[j];

      const dx = pj.x - pi.x;
      const dz = pj.z - pi.z;
      const segLen2 = dx * dx + dz * dz;
      if (segLen2 > 0) {
        const t = Math.max(0, Math.min(1, ((point.x - pi.x) * dx + (point.z - pi.z) * dz) / segLen2));
        const px = pi.x + t * dx;
        const pz = pi.z + t * dz;
        if (Math.hypot(point.x - px, point.z - pz) < 1e-6) return true;
      }

      if ((pi.z > point.z) !== (pj.z > point.z)) {
        const xCross = ((pj.x - pi.x) * (point.z - pi.z)) / (pj.z - pi.z) + pi.x;
        if (point.x < xCross) inside = !inside;
      }
      j = i;
    }
    return inside;
  }

  function packetOob(packet: TelemetryPacket) {
    if (zonePolygon.length < 3 || (packet.positionX === 0 && packet.positionZ === 0)) return null;
    return !pointInPolygon({ x: packet.positionX, z: packet.positionZ }, zonePolygon);
  }

  function handlePointerMove(e: PointerEvent) {
    if (!svgEl || plotTicks.length === 0) return;
    const rect = svgEl.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / rect.width) * width;
    const y = ((e.clientY - rect.top) / rect.height) * height;
    let best: PlotTick | null = null;
    let bestD = Infinity;
    for (const tick of plotTicks) {
      const dx = tick.x - x;
      const dy = tick.y - y;
      const d = dx * dx + dy * dy;
      if (d < bestD) {
        bestD = d;
        best = tick;
      }
    }
    hoverTick = bestD <= 24 * 24 ? best : null;
  }

  async function initMap() {
    if (!mapHost || gm || !cfg || !mapUsable) return;
    gm = await createGameMap(mapHost, cfg, { extraZoom: 3 });
    gm.map.on('mousemove', handleMapMouseMove);
    gm.map.on('mouseout', clearHoverMarker);
    mapReady = true;
    drawLeaflet();
    untrack(() => updateLiveMarker());
  }

  function destroyMap() {
    clearHoverMarker();
    gm?.destroy();
    gm = null;
    mapReady = false;
    lastFitKey = null;
  }

  onDestroy(destroyMap);

  $effect(() => {
    if (!mapUsable && gm) destroyMap();
    if (mapHost && mapUsable && !gm) void initMap();
  });

  $effect(() => {
    void run;
    void zone;
    void displayTicks;
    if (mapReady) drawLeaflet();
  });

  $effect(() => {
    void livePacket;
    if (mapReady) updateLiveMarker();
  });

  function traceColor() {
    return '#bd05fa';
  }

  function tickColor(tick: Tick) {
    if (tick.roughWheels >= 2) return themeColor('--bad-tx', '#f28c7a');
    if (tick.roughWheels > 0) return themeColor('--warn', '#e5b65a');
    return traceColor();
  }

  function drawLeaflet() {
    if (!gm) return;
    gm.clearLines();
    hoverMarker = null;
    hoverTick = null;

    if (zone) {
      if (zone.leftBoundary.length > 1) {
        gm.addLine(zone.leftBoundary.map(gm.worldToLatLng), 5, {
          color: themeColor('--map-left', '#84b577'),
          opacity: 0.9,
        });
      }
      if (zone.rightBoundary.length > 1) {
        gm.addLine(zone.rightBoundary.map(gm.worldToLatLng), 5, {
          color: themeColor('--map-right', '#82a7c8'),
          opacity: 0.9,
        });
      }
      const start = derivedStartGate(zone);
      const finish = derivedFinishGate(zone);
      if (start.length === 2) {
        gm.addLine(start.map(gm.worldToLatLng), 4, {
          color: themeColor('--gate-a', '#d2a24c'),
          dashArray: '10 7',
        });
      }
      if (finish.length === 2) {
        gm.addLine(finish.map(gm.worldToLatLng), 4, {
          color: themeColor('--gate-b', '#d56c62'),
          dashArray: '10 7',
        });
      }
      zone.splitGates.forEach((gate) => {
        if (gate.length !== 2) return;
        gm!.addLine(gate.map(gm!.worldToLatLng), 3, {
          color: themeColor('--gate-split', '#a995cf'),
          dashArray: '6 6',
          opacity: 0.85,
        });
      });
    }

    if (displayTicks.length > 1) {
      gm.addLine(
        displayTicks.map((tick) => gm!.worldToLatLng({
          x: tick.packet.positionX,
          z: tick.packet.positionZ,
        })),
        2.8,
        { color: traceColor(), opacity: 0.82 }
      );
    }

    for (const tick of displayTicks) {
      gm.L.circleMarker(
        gm.worldToLatLng({ x: tick.packet.positionX, z: tick.packet.positionZ }),
        {
          radius: tick.roughWheels > 0 ? 3.25 : 1.8,
          stroke: false,
          fill: true,
          fillColor: tickColor(tick),
          fillOpacity: tick.roughWheels > 0 ? 0.95 : 0.55,
          interactive: false,
        }
      ).addTo(gm.overlay);
    }

    untrack(() => fitMapToGeometry(`${run?.id ?? 'none'}:${zone?.id ?? 'none'}:${displayTicks.length}`));
  }

  function fitMapToGeometry(fitKey: string) {
    if (!gm || lastFitKey === fitKey || staticWorldPoints.length === 0) return;
    const fitZoom = gm.fitWorld(staticWorldPoints, 0.14, (cfg?.viewMaxZoom ?? 0) + 3);
    gm.setWeightRefZoom(fitZoom);
    lastFitKey = fitKey;
  }

  function updateLiveMarker() {
    if (!gm) return;
    if (livePoint && livePacket) {
      const headingDeg = ((livePacket.yaw * 180) / Math.PI) % 360;
      gm.setLiveArrow(livePoint, headingDeg, 30);
    } else {
      gm.removeLiveArrow();
    }
  }

  function clearHoverMarker() {
    hoverMarker?.remove();
    hoverMarker = null;
    hoverTick = null;
  }

  function updateHoverMarker(tick: Tick | null) {
    if (!gm || !tick) {
      clearHoverMarker();
      return;
    }
    const ll = gm.worldToLatLng({ x: tick.packet.positionX, z: tick.packet.positionZ });
    if (!hoverMarker) {
      hoverMarker = gm.L.circleMarker(ll, {
        radius: 8,
        stroke: true,
        color: themeColor('--ac-bright', '#f1c56b'),
        weight: 2.2,
        fill: false,
        interactive: false,
      }).addTo(gm.overlay);
    } else {
      hoverMarker.setLatLng(ll);
    }
  }

  function handleMapMouseMove(e: import('leaflet').LeafletMouseEvent) {
    if (!gm || displayTicks.length === 0) return;
    const world = gm.latLngToWorld(e.latlng);
    let best: Tick | null = null;
    let bestD = Infinity;
    for (const tick of displayTicks) {
      const dx = tick.packet.positionX - world.x;
      const dy = tick.packet.positionZ - world.z;
      const d = dx * dx + dy * dy;
      if (d < bestD) {
        bestD = d;
        best = tick;
      }
    }
    hoverTick = bestD <= hoverWorldRadiusM * hoverWorldRadiusM ? best : null;
    updateHoverMarker(hoverTick);
  }
</script>

<div class="visualizer">
  <div class="viz-head">
    <div>
      <span class="cap">Run Visualizer</span>
      <strong>{run ? `#${run.zoneRunNumber} · global #${run.id}` : 'No run selected'}</strong>
    </div>
    <div class="mono">
      {packets.length.toLocaleString()} packets · {displayTicks.length.toLocaleString()} drawn
    </div>
  </div>

  <div class="viz-body">
    <div class="plot">
      {#if mapUsable}
        <div class="leaflet-host" bind:this={mapHost}></div>
        {#if !run}
          <div class="empty overlay">Select a run</div>
        {:else if loading}
          <div class="empty overlay">Loading packets…</div>
        {:else if error}
          <div class="empty overlay">Failed: {error}</div>
        {:else if displayTicks.length === 0}
          <div class="empty overlay">No position data</div>
        {/if}
      {:else if !run}
        <div class="empty">Select a run</div>
      {:else if loading}
        <div class="empty">Loading packets…</div>
      {:else if error}
        <div class="empty">Failed: {error}</div>
      {:else if displayTicks.length === 0}
        <div class="empty">No position data</div>
      {:else}
        <svg
          bind:this={svgEl}
          viewBox={`0 0 ${width} ${height}`}
          role="img"
          aria-label="Drift run telemetry trace"
          onpointermove={handlePointerMove}
          onpointerleave={() => (hoverTick = null)}
        >
          <rect class="bg" x="0" y="0" width={width} height={height} />
          {#if zone?.leftBoundary.length && zone.leftBoundary.length > 1}
            <polyline class="boundary left" points={path(zone.leftBoundary)} />
          {/if}
          {#if zone?.rightBoundary.length && zone.rightBoundary.length > 1}
            <polyline class="boundary right" points={path(zone.rightBoundary)} />
          {/if}
          {#if zone}
            <polyline class="gate start" points={path(derivedStartGate(zone))} />
            <polyline class="gate finish" points={path(derivedFinishGate(zone))} />
            {#each zone.splitGates as gate}
              <polyline class="gate split" points={path(gate)} />
            {/each}
          {/if}
          <polyline class="trace" points={tracePoints} />
          {#each plotTicks as tick (tick.index)}
            <circle
              class="tick"
              class:edge={tick.roughWheels > 0}
              class:heavy={tick.roughWheels >= 2}
              cx={tick.x}
              cy={tick.y}
              r={tick.roughWheels > 0 ? 3 : 1.65}
            />
          {/each}
          {#if hoverPlotPoint}
            <circle class="hover-ring" cx={hoverPlotPoint.x} cy={hoverPlotPoint.y} r="8" />
          {/if}
        </svg>
      {/if}
    </div>

    <aside class="readout">
      {#if hoverTick}
        {@const p = hoverTick.packet}
        <div class="readout-title">
          <span class="cap">Tick</span>
          <strong class="mono">{hoverTick.index + 1} / {packets.length}</strong>
        </div>
        <div class="readout-grid">
          <div>
            <span class="cap">Elapsed</span>
            <strong class="mono">{formatNum(hoverTick.elapsedS, 3)}s</strong>
          </div>
          <div>
            <span class="cap">Speed</span>
            <strong class="mono">{formatNum(p.speedMs, 2)} m/s</strong>
          </div>
          <div>
            <span class="cap">Angle</span>
            <strong class="mono">{formatNum(signedAngle(p), 1)}°</strong>
          </div>
          <div>
            <span class="cap">Rough wheels</span>
            <strong class="mono">{hoverTick.roughWheels}</strong>
          </div>
          <div>
            <span class="cap">Zone</span>
            <strong class="mono" class:oob={hoverOob === true}>
              {hoverOob === null ? '-' : hoverOob ? 'OOB' : 'in bounds'}
            </strong>
          </div>
        </div>
        <div class="coords">
          <span class="cap">World</span>
          <strong class="mono">
            x {formatNum(p.positionX, 3)}<br />
            y {formatNum(p.positionY, 3)}<br />
            z {formatNum(p.positionZ, 3)}
          </strong>
        </div>
        <div class="wheel-table">
          {#each wheelRows(p) as wheel}
            <div class:rough={wheel.state === 'rough'}>
              <span>{wheel.label}</span>
              <strong>{wheel.state}</strong>
              <code>{formatNum(wheel.value, 3)}</code>
            </div>
          {/each}
        </div>
      {:else}
        <div class="readout-idle">
          <span class="cap">Hover</span>
          <strong>Telemetry tick</strong>
        </div>
      {/if}
    </aside>
  </div>
</div>

<style>
  .visualizer {
    width: 100%;
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-md);
    background: var(--bg-body);
    overflow: hidden;
  }
  .viz-head {
    flex: 0 0 auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.7rem 0.85rem;
    border-bottom: 1px solid var(--bd-subtle);
  }
  .viz-head div:first-child {
    min-width: 0;
    display: grid;
    gap: 0.15rem;
  }
  .viz-head strong {
    color: var(--tx-hi);
    font-size: 0.9rem;
    font-weight: 620;
  }
  .viz-head .mono {
    color: var(--tx-dim);
    font-size: 0.72rem;
  }
  .viz-body {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 220px;
  }
  .plot {
    position: relative;
    min-width: 0;
    min-height: 0;
  }
  .leaflet-host {
    width: 100%;
    height: 100%;
    min-height: 360px;
    background: var(--bg-card);
    isolation: isolate;
  }
  :global(.leaflet-container) {
    background: var(--bg-card);
    font: inherit;
  }
  :global(.player-arrow) {
    background: none;
    border: none;
  }
  svg {
    width: 100%;
    height: 100%;
    display: block;
  }
  .bg {
    fill: var(--bg-card);
  }
  .trace {
    fill: none;
    stroke: #bd05fa;
    stroke-linecap: round;
    stroke-linejoin: round;
    stroke-width: 3;
    opacity: 0.82;
  }
  .tick {
    fill: #bd05fa;
    pointer-events: none;
  }
  .tick.edge {
    fill: var(--warn);
  }
  .tick.heavy {
    fill: var(--bad-tx);
  }
  .hover-ring {
    fill: none;
    stroke: var(--ac-bright);
    stroke-width: 2.4;
    pointer-events: none;
  }
  .boundary {
    fill: none;
    stroke-width: 4.2;
    stroke-linecap: round;
    stroke-linejoin: round;
    opacity: 0.82;
  }
  .boundary.left {
    stroke: var(--map-left);
  }
  .boundary.right {
    stroke: var(--map-right);
  }
  .gate {
    fill: none;
    stroke-width: 3.2;
    stroke-dasharray: 11 8;
    stroke-linecap: round;
    opacity: 0.82;
  }
  .gate.start {
    stroke: var(--gate-a);
  }
  .gate.finish {
    stroke: var(--gate-b);
  }
  .gate.split {
    stroke: var(--gate-split);
    stroke-width: 2.4;
    stroke-dasharray: 6 7;
  }
  .empty {
    height: 100%;
    min-height: 360px;
    display: grid;
    place-items: center;
    color: var(--tx-xdim);
    font-size: 0.82rem;
  }
  .empty.overlay {
    position: absolute;
    inset: 0;
    z-index: 500;
    pointer-events: none;
    background: color-mix(in srgb, var(--bg-body) 20%, transparent);
  }
  .readout {
    min-width: 0;
    border-left: 1px solid var(--bd-subtle);
    background: var(--bg-panel);
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .readout-title,
  .readout-idle {
    display: grid;
    gap: 0.16rem;
  }
  .readout-title strong,
  .readout-idle strong {
    color: var(--tx-hi);
    font-size: 0.9rem;
    font-weight: 620;
  }
  .readout-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.55rem 0.7rem;
  }
  .readout-grid div,
  .coords {
    min-width: 0;
    display: grid;
    gap: 0.16rem;
  }
  .readout-grid strong,
  .coords strong {
    color: var(--tx-mid);
    font-size: 0.76rem;
    font-weight: 560;
  }
  .readout-grid strong.oob {
    color: var(--warn);
  }
  .wheel-table {
    display: grid;
    gap: 0.35rem;
  }
  .wheel-table div {
    display: grid;
    grid-template-columns: 2.2rem 1fr 4.2rem;
    align-items: center;
    gap: 0.45rem;
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-sm);
    padding: 0.38rem 0.45rem;
    color: var(--tx-dim);
    font-size: 0.72rem;
  }
  .wheel-table div.rough {
    border-color: color-mix(in srgb, var(--warn) 48%, var(--bg-panel));
    background: color-mix(in srgb, var(--warn) 7%, transparent);
    color: var(--tx-mid);
  }
  .wheel-table strong {
    color: inherit;
    font-weight: 600;
  }
  .wheel-table code {
    color: var(--tx-hi);
    font-family: var(--font-mono);
    text-align: right;
  }
  @media (max-width: 980px) {
    .viz-body {
      grid-template-columns: 1fr;
    }
    .readout {
      border-left: 0;
      border-top: 1px solid var(--bd-subtle);
    }
  }
</style>

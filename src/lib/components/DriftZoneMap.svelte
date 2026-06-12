<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { effectiveMapConfig } from '$lib/mapDefaults';
  import { createGameMap, makeCalib, type GameMap } from '$lib/mapView';
  import { themeColor } from '$lib/theme';
  import type { AppSettings, DriftZoneRow, TelemetryPacket, ZonePoint } from '$lib/types';

  let {
    zone,
    settings,
    livePacket,
  }: {
    zone: DriftZoneRow | null;
    settings: AppSettings;
    livePacket: TelemetryPacket | null;
  } = $props();

  const width = 1000;
  const height = 640;
  const pad = 48;
  const zoneExtraZoom = 3;

  let mapHost = $state<HTMLDivElement | null>(null);
  let gm: GameMap | null = null;
  let mapReady = $state(false);
  let lastFitKey: string | null = null;

  let cfg = $derived(effectiveMapConfig(settings));
  let mapUsable = $derived(!!makeCalib(cfg));

  let livePoint = $derived(
    livePacket && (livePacket.positionX !== 0 || livePacket.positionZ !== 0)
      ? { x: livePacket.positionX, z: livePacket.positionZ }
      : null
  );

  let allPoints = $derived([
    ...(zone?.leftBoundary ?? []),
    ...(zone?.rightBoundary ?? []),
    ...(zone?.startGate ?? []),
    ...(zone?.finishGate ?? []),
    ...(zone?.splitGates.flat() ?? []),
    ...(livePoint ? [livePoint] : []),
  ]);

  let transform = $derived.by(() => {
    if (allPoints.length === 0) {
      return { minX: -50, maxX: 50, minZ: -50, maxZ: 50, scale: 1 };
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

  function path(points: ZonePoint[]): string {
    return points
      .map((p) => {
        const [x, y] = toSvg(p);
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(' ');
  }

  function gateLine(points: ZonePoint[]): string {
    return points.length === 2 ? path(points) : '';
  }

  // The end gates are always the first/last boundary point pairs — derive from
  // the current boundary (never the stored gate, which can go stale) so this
  // matches the editor and the backend detector exactly.
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

  async function initMap() {
    if (!mapHost || gm || !mapUsable) return;
    gm = await createGameMap(mapHost, cfg, { extraZoom: zoneExtraZoom });
    mapReady = true;
    drawZone();
    untrack(() => updateLive());
  }

  $effect(() => {
    if (mapHost && mapUsable && !gm) void initMap();
  });

  // Static geometry rebuilds only when the zone changes — never on the live
  // telemetry tick (the fit's read of the live point is untracked below), so
  // the lines stay put and in sync with the tiles during zoom.
  $effect(() => {
    void zone;
    if (mapReady) drawZone();
  });

  // The live marker moves in place every telemetry tick.
  $effect(() => {
    void livePacket;
    if (mapReady) updateLive();
  });

  onDestroy(() => {
    gm?.destroy();
    gm = null;
  });

  function fitMapToGeometry(fitKey: string) {
    if (!gm || allPoints.length === 0 || lastFitKey === fitKey) return;
    const fitZoom = gm.fitWorld(allPoints, 0.18, cfg.viewMaxZoom + zoneExtraZoom);
    // Lines show their tuned base weight at this framing and thin out from it.
    gm.setWeightRefZoom(fitZoom);
    lastFitKey = fitKey;
  }

  function drawZone() {
    if (!gm) return;
    gm.clearLines();

    if (zone) {
      if (zone.leftBoundary.length > 1) {
        gm.addLine(zone.leftBoundary.map(gm.worldToLatLng), 5, {
          color: themeColor('--map-left', '#84b577'),
          opacity: 0.95,
        });
      }
      if (zone.rightBoundary.length > 1) {
        gm.addLine(zone.rightBoundary.map(gm.worldToLatLng), 5, {
          color: themeColor('--map-right', '#82a7c8'),
          opacity: 0.95,
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
          opacity: 0.9,
        });
      });
    }

    // The fit depends on the live point's presence; untrack it so rebuilding
    // the zone never subscribes this code path to the per-tick live position.
    untrack(() => fitMapToGeometry(`${zone?.id ?? 'none'}:${!!livePoint}`));
  }

  function updateLive() {
    if (!gm) return;
    if (livePoint && livePacket) {
      const headingDeg = ((livePacket.yaw * 180) / Math.PI) % 360;
      gm.setLiveArrow(livePoint, headingDeg, 30);
      // Frame the live position the first time it appears.
      untrack(() => fitMapToGeometry(`${zone?.id ?? 'none'}:true`));
    } else {
      gm.removeLiveArrow();
    }
  }
</script>

<div class="zone-map">
  {#if !zone}
    <div class="empty">Select a drift zone</div>
  {:else if mapUsable}
    <div class="leaflet-host" bind:this={mapHost}></div>
  {:else}
    <div class="fallback-note">Map calibration unavailable; showing world-coordinate geometry.</div>
    <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label="Drift zone map">
      <rect x="0" y="0" width={width} height={height} />
      {#if zone.leftBoundary.length > 1}
        <polyline class="boundary left" points={path(zone.leftBoundary)} />
      {/if}
      {#if zone.rightBoundary.length > 1}
        <polyline class="boundary right" points={path(zone.rightBoundary)} />
      {/if}
      <polyline class="gate start" points={gateLine(derivedStartGate(zone))} />
      <polyline class="gate finish" points={gateLine(derivedFinishGate(zone))} />
      {#each zone.splitGates as gate}
        <polyline class="gate split" points={gateLine(gate)} />
      {/each}
      {#if livePoint}
        {@const live = toSvg(livePoint)}
        <circle class="live-dot" cx={live[0]} cy={live[1]} r="8" />
      {/if}
    </svg>
  {/if}
</div>

<style>
  .zone-map {
    width: 100%;
    height: 100%;
    min-height: 0;
    background: var(--bg-body);
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-md);
    overflow: hidden;
    display: flex;
    flex-direction: column;
    /* Keep leaflet's pane z-indexes from escaping above app overlays. */
    isolation: isolate;
  }
  .leaflet-host {
    width: 100%;
    height: 100%;
    min-height: 360px;
  }
  .empty {
    flex: 1;
    min-height: 360px;
    display: grid;
    place-items: center;
    color: var(--tx-xdim);
    font-size: 0.85rem;
  }
  .fallback-note {
    padding: 0.45rem 0.6rem;
    color: var(--tx-dim);
    font-size: 0.72rem;
    border-bottom: 1px solid var(--bd-dim);
    background: var(--bg-card);
  }
  svg {
    display: block;
    width: 100%;
    height: 100%;
    min-height: 360px;
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
    stroke: var(--map-left);
  }
  .boundary.right {
    stroke: var(--map-right);
  }
  .gate {
    fill: none;
    stroke-width: 3;
    stroke-dasharray: 10 7;
  }
  .gate.start {
    stroke: var(--gate-a);
  }
  .gate.finish {
    stroke: var(--gate-b);
  }
  .gate.split {
    stroke: var(--gate-split);
    stroke-dasharray: 6 6;
  }
  .live-dot {
    fill: var(--live-dot);
    stroke: var(--bg-body);
    stroke-width: 2;
  }
  :global(.leaflet-container) {
    background: var(--bg-card);
    font: inherit;
  }
  /* Strip Leaflet's default divIcon box so only the arrow shows. */
  :global(.player-arrow) {
    background: none;
    border: none;
  }
</style>

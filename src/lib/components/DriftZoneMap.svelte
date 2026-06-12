<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import type { LatLng, LatLngBounds, LayerGroup, Map as LMap, Marker, PolylineOptions, TileLayer } from 'leaflet';
  import { effectiveMapConfig, type EffectiveMapConfig } from '$lib/mapDefaults';
  import { xyzSimpleCRS } from '$lib/mapCrs';
  import { applyBoundsFloor, applyLineWeights, scaledWeight, tileExtentBounds, type WeightedLine } from '$lib/mapView';
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
  let L = $state<typeof import('leaflet') | null>(null);
  let map = $state<LMap | null>(null);
  let tiles = $state<TileLayer | null>(null);
  let zoneLayer = $state<LayerGroup | null>(null);
  let markerLayer = $state<LayerGroup | null>(null);
  let liveMarker: Marker | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let mapReady = $state(false);
  let lastFitKey: string | null = null;
  // Bundled tile extent (drives maxBounds + the dynamic zoom-out floor).
  let extentBounds: LatLngBounds | null = null;
  // Static boundary/gate lines (rebuilt only when the zone changes) + the zoom at
  // which they show full weight (anchored to the auto-fit zoom).
  let staticLines: WeightedLine[] = [];
  let weightRefZoom = 0;

  let cfg = $derived(effectiveMapConfig(settings));

  let calib = $derived.by(() => {
    const aw = cfg.calAWorld, bw = cfg.calBWorld;
    const ap = cfg.calAPix, bp = cfg.calBPix;
    const dWX = bw[0] - aw[0];
    const dWZ = bw[1] - aw[1];
    if (Math.abs(dWX) < 1e-6 || Math.abs(dWZ) < 1e-6) return null;
    const mX = (bp[0] - ap[0]) / dWX;
    const mZ = (bp[1] - ap[1]) / dWZ;
    return {
      mX,
      mZ,
      bX: ap[0] - mX * aw[0],
      bY: ap[1] - mZ * aw[1],
    };
  });

  let mapUsable = $derived(!!calib);

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

  function worldToPix(point: ZonePoint): [number, number] {
    const c = calib!;
    return [c.mX * point.x + c.bX, c.mZ * point.z + c.bY];
  }

  function worldToLatLng(point: ZonePoint) {
    const [x, y] = worldToPix(point);
    return map!.unproject(L!.point(x, y), cfg.maxZoom);
  }

  function liveIcon(pkt: TelemetryPacket) {
    const headingDeg = ((pkt.yaw * 180) / Math.PI) % 360;
    return L!.divIcon({
      className: 'drift-player-arrow',
      html:
        '<svg width="30" height="30" viewBox="0 0 24 24">' +
        `<path transform="rotate(${headingDeg} 12 12)" ` +
        `d="M12 2 L19 21 L12 15 L5 21 Z" fill="${themeColor('--live-dot', '#ecc274')}" ` +
        `stroke="${themeColor('--bg-body', '#0f1012')}" stroke-width="1.5" stroke-linejoin="round"/></svg>`,
      iconSize: [30, 30],
      iconAnchor: [15, 15],
    });
  }

  async function initMap(config: EffectiveMapConfig) {
    if (!mapHost || map || !mapUsable) return;
    L = await import('leaflet');
    await import('leaflet/dist/leaflet.css');
    map = L.map(mapHost, {
      crs: xyzSimpleCRS(L),
      attributionControl: false,
      zoomControl: true,
      minZoom: config.minZoom,
      maxZoom: config.viewMaxZoom + zoneExtraZoom,
      maxBoundsViscosity: 1.0,
    });
    // Constrain camera + tile requests to the bundled tile extent: no scrolling
    // off into empty space, and no 404s for tiles that were never downloaded.
    extentBounds = tileExtentBounds(L, map, config);
    tiles = L.tileLayer(config.tileUrl, {
      minZoom: config.minZoom,
      maxZoom: config.viewMaxZoom + zoneExtraZoom,
      maxNativeZoom: config.maxZoom,
      tileSize: config.tileSize,
      noWrap: true,
      bounds: extentBounds ?? undefined,
      // Retain more off-screen tiles and don't refetch mid-zoom-animation, so
      // panning/zooming doesn't flash white reloading tiles it just had.
      keepBuffer: 4,
      updateWhenZooming: false,
    }).addTo(map);
    if (extentBounds) map.setMaxBounds(extentBounds.pad(0.05));
    zoneLayer = L.layerGroup().addTo(map);
    markerLayer = L.layerGroup().addTo(map);
    weightRefZoom = config.defaultZoom;
    map.setView(
      map.unproject(L.point(config.defaultCenter[0], config.defaultCenter[1]), config.maxZoom),
      config.defaultZoom
    );
    // Keep line thickness proportional to the map as the user zooms.
    map.on('zoomend', () => {
      if (map) applyLineWeights(staticLines, map.getZoom(), weightRefZoom);
    });
    // Floor the zoom so the map can't be scrolled out into empty space; recompute
    // on resize since the floor depends on the viewport size.
    if (extentBounds) applyBoundsFloor(map, extentBounds);
    resizeObserver = new ResizeObserver(() => {
      map?.invalidateSize();
      if (map && extentBounds) applyBoundsFloor(map, extentBounds);
    });
    resizeObserver.observe(mapHost);
    mapReady = true;
    drawZone();
    updateLiveMarker();
  }

  $effect(() => {
    if (mapHost && mapUsable && !map) void initMap(cfg);
  });

  // Static geometry rebuilds only when the zone changes (the fit's read of the
  // live point is untracked, below, so this never re-fires on a telemetry tick).
  $effect(() => {
    void zone;
    if (mapReady) drawZone();
  });

  // The live marker moves every telemetry tick without disturbing the lines.
  $effect(() => {
    void livePacket;
    if (mapReady) updateLiveMarker();
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    liveMarker?.remove();
    map?.remove();
    map = null;
  });

  function fitMapToGeometry(fitKey: string) {
    if (!map || !L || !mapUsable || allPoints.length === 0 || lastFitKey === fitKey) return;
    const bounds = L.latLngBounds(allPoints.map(worldToLatLng));
    map.fitBounds(bounds.pad(0.18), { maxZoom: cfg.viewMaxZoom + zoneExtraZoom });
    lastFitKey = fitKey;
    // Anchor full line weight to this framing, then apply it.
    weightRefZoom = map.getZoom();
    applyLineWeights(staticLines, map.getZoom(), weightRefZoom);
  }

  // Rebuild the static boundary/gate lines. Infrequent — runs only when the
  // selected zone changes, never on the live telemetry tick.
  function drawZone() {
    if (!map || !L || !zoneLayer || !mapUsable) return;
    zoneLayer.clearLayers();
    staticLines = [];
    const add = (latlngs: LatLng[], base: number, style: PolylineOptions) => {
      const line = L!.polyline(latlngs, {
        ...style,
        weight: scaledWeight(base, map!.getZoom(), weightRefZoom),
      }).addTo(zoneLayer!);
      staticLines.push({ line, base });
    };

    if (zone) {
      if (zone.leftBoundary.length > 1) {
        add(zone.leftBoundary.map(worldToLatLng), 5, {
          color: themeColor('--map-left', '#84b577'),
          opacity: 0.95,
        });
      }
      if (zone.rightBoundary.length > 1) {
        add(zone.rightBoundary.map(worldToLatLng), 5, {
          color: themeColor('--map-right', '#82a7c8'),
          opacity: 0.95,
        });
      }
      const start = derivedStartGate(zone);
      const finish = derivedFinishGate(zone);
      if (start.length === 2) {
        add(start.map(worldToLatLng), 4, {
          color: themeColor('--gate-a', '#d2a24c'),
          dashArray: '10 7',
        });
      }
      if (finish.length === 2) {
        add(finish.map(worldToLatLng), 4, {
          color: themeColor('--gate-b', '#d56c62'),
          dashArray: '10 7',
        });
      }
      zone.splitGates.forEach((gate) => {
        if (gate.length !== 2) return;
        add(gate.map(worldToLatLng), 3, {
          color: themeColor('--gate-split', '#a995cf'),
          dashArray: '6 6',
          opacity: 0.9,
        });
      });
    }

    // The fit depends on the live point's presence; untrack it so rebuilding the
    // zone never subscribes this code path to the per-tick live position.
    untrack(() => fitMapToGeometry(`${zone?.id ?? 'none'}:${!!livePoint}`));
  }

  // Move/refresh just the live player marker — runs every telemetry tick but
  // never touches the static zone layer, so the lines stay in sync during zoom.
  function updateLiveMarker() {
    if (!map || !L || !markerLayer || !mapUsable) return;
    if (livePoint && livePacket) {
      const ll = worldToLatLng(livePoint);
      if (liveMarker) {
        liveMarker.setLatLng(ll);
        liveMarker.setIcon(liveIcon(livePacket));
      } else {
        liveMarker = L.marker(ll, { icon: liveIcon(livePacket), interactive: false }).addTo(markerLayer);
      }
      // Frame the live position the first time it appears.
      untrack(() => fitMapToGeometry(`${zone?.id ?? 'none'}:true`));
    } else if (liveMarker) {
      liveMarker.remove();
      liveMarker = null;
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
  :global(.drift-player-arrow) {
    background: none;
    border: none;
  }
</style>

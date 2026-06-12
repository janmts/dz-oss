<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { LatLng, Polyline } from 'leaflet';
  import TrackMap from './TrackMap.svelte';
  import { effectiveMapConfig } from '$lib/mapDefaults';
  import { createGameMap, makeCalib, type GameMap } from '$lib/mapView';
  import { LAP_PALETTE, themeColor } from '$lib/theme';
  import type { TelemetryPacket, AppSettings } from '$lib/types';

  let {
    points,
    currentIndex = -1,
    drawLine = true,
    colorByLap = true,
    fixedTrace = false,
    settings,
    compact = false,
  }: {
    points: TelemetryPacket[];
    currentIndex?: number;
    drawLine?: boolean;
    colorByLap?: boolean;
    /** Complete known track (replay / recorded view): fit once, then let the
     *  user zoom in & pan but lock them inside the track's bounds/zoom. */
    fixedTrace?: boolean;
    settings: AppSettings;
    compact?: boolean;
  } = $props();

  const LAP_COLORS = LAP_PALETTE;
  // Trace points closer than this (world units ≈ metres) to the last drawn
  // vertex are skipped — at 64 Hz consecutive packets are centimetres apart,
  // and sub-pixel vertices only bloat the polylines.
  const TRACE_MIN_STEP = 2.0;

  // Resolves to the FH6 Japan preset unless the user opted into overriding.
  let cfg = $derived(effectiveMapConfig(settings));
  let usable = $derived(!!makeCalib(cfg));

  let host = $state<HTMLDivElement | null>(null);
  let gm: GameMap | null = null;

  // Incremental trace state. The points array identity is the dataset key:
  // the same array growing means append-only work; a new array (new replay /
  // new session selection) rebuilds from scratch.
  let lastPoints: TelemetryPacket[] | null = null;
  let valid: TelemetryPacket[] = [];
  let scanned = 0;
  let drawnIdx = 0;
  let curSeg: Polyline | null = null;
  let curLap = -1;
  let lastSegLL: LatLng | null = null;
  let lastWorld: { x: number; z: number } | null = null;
  let lastDrawLine: boolean | null = null;
  let lastColorByLap: boolean | null = null;
  let lastFixedTrace: boolean | null = null;
  let camLocked = false;
  let lastTraceFitAt = 0;

  onMount(async () => {
    if (!usable || !host) return;
    gm = await createGameMap(host, cfg, { zoomControl: !compact });
    update();
  });

  onDestroy(() => {
    gm?.destroy();
    gm = null;
  });

  // Re-render on data changes; all work below is incremental, so the 64 Hz
  // live tick costs a marker move, not a layer teardown.
  $effect(() => {
    void points;
    void currentIndex;
    void drawLine;
    void colorByLap;
    void fixedTrace;
    if (gm) update();
  });

  function resetTrace() {
    gm?.clearLines();
    curSeg = null;
    curLap = -1;
    lastSegLL = null;
    lastWorld = null;
    drawnIdx = 0;
  }

  function extendTrace() {
    if (!gm) return;
    const traceWeight = compact ? 2 : 3;
    for (let i = drawnIdx; i < valid.length; i++) {
      const p = valid[i];
      const lap = colorByLap ? p.lapNumber : 0;
      const lapChanged = curSeg !== null && lap !== curLap;
      if (!lapChanged && lastWorld) {
        const dx = p.positionX - lastWorld.x;
        const dz = p.positionZ - lastWorld.z;
        if (dx * dx + dz * dz < TRACE_MIN_STEP * TRACE_MIN_STEP) continue;
      }
      const ll = gm.worldToLatLng({ x: p.positionX, z: p.positionZ });
      if (!curSeg || lapChanged) {
        const color = colorByLap
          ? LAP_COLORS[p.lapNumber % LAP_COLORS.length]
          : themeColor('--ac', '#d2a24c');
        // Start the new lap's segment from the previous vertex so the trace
        // stays visually continuous across the colour change.
        curSeg = gm.addLine(lastSegLL ? [lastSegLL, ll] : [ll], traceWeight, {
          color,
          opacity: 0.9,
        });
        curLap = lap;
      } else {
        curSeg.addLatLng(ll);
      }
      lastSegLL = ll;
      lastWorld = { x: p.positionX, z: p.positionZ };
    }
    drawnIdx = valid.length;
  }

  function update() {
    if (!gm) return;

    // Dataset / mode switch → full rebuild.
    if (
      points !== lastPoints ||
      points.length < scanned ||
      drawLine !== lastDrawLine ||
      colorByLap !== lastColorByLap ||
      fixedTrace !== lastFixedTrace
    ) {
      lastPoints = points;
      lastDrawLine = drawLine;
      lastColorByLap = colorByLap;
      lastFixedTrace = fixedTrace;
      scanned = 0;
      valid = [];
      resetTrace();
      if (camLocked) {
        gm.unlockCamera();
        camLocked = false;
      }
    }
    // Scan only the unseen tail for valid (non-zero) positions.
    for (; scanned < points.length; scanned++) {
      const p = points[scanned];
      if (p.positionX !== 0 || p.positionZ !== 0) valid.push(p);
    }

    if (valid.length === 0) {
      resetTrace();
      gm.removeLiveArrow();
      return;
    }

    if (drawLine && valid.length > 1) extendTrace();

    const mi = currentIndex >= 0 && currentIndex < points.length ? currentIndex : valid.length - 1;
    const mp = points[mi] ?? valid[valid.length - 1];
    if (!mp || (mp.positionX === 0 && mp.positionZ === 0)) return;

    const world = { x: mp.positionX, z: mp.positionZ };
    const headingDeg = ((mp.yaw * 180) / Math.PI) % 360;
    const created = gm.setLiveArrow(world, headingDeg, compact ? 22 : 28);

    if (fixedTrace && valid.length > 1) {
      // Replay / recorded view: fit the whole track once, then lock the
      // camera to that extent — the user may zoom in & pan, but can't zoom
      // out past the full-track view or pan off the track.
      if (!camLocked) {
        const fitZoom = gm.lockCamera(valid.map((p) => ({ x: p.positionX, z: p.positionZ })));
        gm.setWeightRefZoom(fitZoom);
        camLocked = true;
      }
    } else if (drawLine && valid.length > 1) {
      // Live recording: track grows — keep it framed, but gently (at most
      // once a second, never against the user's own pan/zoom).
      const now = Date.now();
      if (now - lastTraceFitAt > 1000 && gm.userIdle()) {
        lastTraceFitAt = now;
        const fitZoom = gm.fitWorld(
          valid.map((p) => ({ x: p.positionX, z: p.positionZ })),
          0.05,
          cfg.defaultZoom
        );
        gm.setWeightRefZoom(fitZoom);
      }
    } else if (created) {
      // Free-roam / live marker: frame the player once on first contact…
      gm.centerOn(world);
    } else {
      // …then follow with a dead-zone pan that never fights the user.
      gm.followLiveArrow();
    }
  }
</script>

{#if usable}
  <div class="map-host" class:compact bind:this={host}></div>
{:else}
  <!-- No tile URL or no calibration → plain vector trace. -->
  <TrackMap {points} {currentIndex} {colorByLap} {drawLine} {compact} />
{/if}

<style>
  .map-host {
    width: 100%;
    aspect-ratio: 1;
    border-radius: var(--r-sm);
    overflow: hidden;
    background: var(--bg-card);
    /* Keep leaflet's pane z-indexes from escaping above app overlays. */
    isolation: isolate;
  }
  .map-host.compact {
    aspect-ratio: 1;
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

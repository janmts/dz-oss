<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { createGameMap, type GameMap } from '$lib/mapView';
  import { effectiveMapConfig } from '$lib/mapDefaults';
  import { settings, loadSettings, driftZones, loadDriftZones } from '$lib/stores/sessions';
  import { runViews, hover, traceColorMode } from '$lib/stores/runViewer';
  import { themeColor } from '$lib/theme';
  import { boundaryCurve, ringCurve, scoringRings, zoneCurveMode } from '$lib/curve';
  import {
    scoreState,
    visibleTickIndices,
    driftAngleDeg,
    TELEMETRY_HZ,
    SCORE_STATE_LABEL,
    type ScoreState,
    type RunView,
  } from '$lib/runViewer';
  import type { DriftZoneRow, ZonePoint } from '$lib/types';

  let host = $state<HTMLDivElement | null>(null);
  let gm: GameMap | null = null;
  let ready = $state(false);
  let cardPos = $state<{ x: number; y: number } | null>(null);
  let highlight: ReturnType<GameMap['L']['circleMarker']> | null = null;
  let fitZoom = 0;
  let lastFitKey = '';
  let mapCfg: ReturnType<typeof effectiveMapConfig> | null = null;

  function stateColors(): Record<ScoreState, string> {
    return {
      scoring: themeColor('--ok', '#84b577'),
      unpaid: themeColor('--bad', '#d56c62'),
      idle: themeColor('--tx-dim', '#8a8b85'),
    };
  }

  const nonZero = (p: { positionX: number; positionZ: number }) => p.positionX !== 0 || p.positionZ !== 0;

  onMount(async () => {
    if (!host) return;
    const s = get(settings) ?? (await loadSettings());
    void loadDriftZones();
    mapCfg = effectiveMapConfig(s);
    try {
      // extraZoom matches the analysis-oriented zone maps so you can zoom in
      // close to inspect individual ticks (the live track map caps at viewMaxZoom).
      gm = await createGameMap(host, mapCfg, { zoomControl: true, extraZoom: 3 });
      ready = true;
      redraw();
    } catch (e) {
      console.error('[runs] map init failed', e);
    }
  });

  onDestroy(() => {
    gm?.destroy();
    gm = null;
  });

  // Redraw geometry whenever the visible runs, colour mode, or zones change.
  $effect(() => {
    void $runViews;
    void $driftZones;
    void $traceColorMode;
    if (ready) redraw();
  });

  // Move the shared-cursor highlight ring when hover changes (from map OR graph).
  $effect(() => {
    const h = $hover;
    if (!gm || !gm.calib) return;
    if (!h) {
      highlight?.remove();
      highlight = null;
      return;
    }
    const view = $runViews.find((v) => v.runId === h.runId);
    const pkt = view?.data.packets[h.index];
    if (!view || !pkt || !nonZero(pkt)) return;
    const ll = gm.worldToLatLng({ x: pkt.positionX, z: pkt.positionZ });
    if (highlight) {
      highlight.setLatLng(ll);
    } else {
      highlight = gm.L.circleMarker(ll, {
        radius: 4.5,
        weight: 1.5,
        color: themeColor('--bg-card', '#18191d'),
        fillColor: themeColor('--ac-bright', '#ecc274'),
        fillOpacity: 1,
        interactive: false,
      }).addTo(gm.markers);
    }
  });

  function zoneFor(zoneId: number | null): DriftZoneRow | undefined {
    return zoneId == null ? undefined : $driftZones.find((z) => z.id === zoneId);
  }

  function drawZone(zone: DriftZoneRow) {
    if (!gm) return;
    const toLL = (p: ZonePoint) => gm!.worldToLatLng(p);
    const mode = zoneCurveMode(zone.scoringConfig);
    if (zone.leftBoundary.length > 1)
      gm.addLine(boundaryCurve(zone.leftBoundary, mode).map(toLL), 2.5, { color: themeColor('--map-left', '#84b577'), opacity: 0.85 });
    if (zone.rightBoundary.length > 1)
      gm.addLine(boundaryCurve(zone.rightBoundary, mode).map(toLL), 2.5, { color: themeColor('--map-right', '#82a7c8'), opacity: 0.85 });
    for (const ring of scoringRings(zone.scoringConfig)) {
      if (ring.length > 1)
        gm.addLine(ringCurve(ring, mode).map(toLL), 2.5, { color: themeColor('--scoring-ring', '#cf72e0'), opacity: 0.85 });
    }
    const a0 = zone.leftBoundary[0], b0 = zone.rightBoundary[0];
    const aN = zone.leftBoundary.at(-1), bN = zone.rightBoundary.at(-1);
    if (a0 && b0) gm.addLine([toLL(a0), toLL(b0)], 3, { color: themeColor('--gate-a', '#d2a24c'), dashArray: '8 6' });
    if (aN && bN) gm.addLine([toLL(aN), toLL(bN)], 3, { color: themeColor('--gate-b', '#d56c62'), dashArray: '8 6' });
    for (const g of zone.splitGates)
      if (g.length === 2) gm.addLine(g.map(toLL), 2, { color: themeColor('--gate-split', '#a995cf'), dashArray: '5 5', opacity: 0.8 });
  }

  function drawTrace(view: RunView, mode: 'scoring' | 'byRun', col: Record<ScoreState, string>) {
    if (!gm) return;
    const { packets, ticks } = view.data;
    if (mode === 'byRun') {
      const lls = packets.filter(nonZero).map((p) => gm!.worldToLatLng({ x: p.positionX, z: p.positionZ }));
      if (lls.length > 1) gm.addLine(lls, 2.5, { color: view.color, opacity: 0.9 });
      return;
    }
    // Scoring mode: break the line into colour segments by per-tick state.
    let seg: ReturnType<GameMap['addLine']> | null = null;
    let segState: ScoreState | null = null;
    let lastLL: ReturnType<GameMap['worldToLatLng']> | null = null;
    for (let i = 0; i < packets.length; i++) {
      const p = packets[i];
      if (!nonZero(p)) continue;
      const ll = gm.worldToLatLng({ x: p.positionX, z: p.positionZ });
      const st = scoreState(ticks[i]);
      if (!seg || st !== segState) {
        seg = gm.addLine(lastLL ? [lastLL, ll] : [ll], 2.6, { color: col[st], opacity: 0.92 });
        segState = st;
      } else {
        seg.addLatLng(ll);
      }
      lastLL = ll;
    }
  }

  // Invisible hover targets: the trace LINE is the visual; each tick gets a
  // generous transparent hit circle so hovering needs no pixel-perfect aim
  // (the amber highlight dot + per-tick card are the feedback).
  function addMarkers(view: RunView) {
    if (!gm) return;
    const { packets } = view.data;
    for (const idx of visibleTickIndices(view.data)) {
      const p = packets[idx];
      if (!nonZero(p)) continue;
      const ll = gm.worldToLatLng({ x: p.positionX, z: p.positionZ });
      const m = gm.L.circleMarker(ll, {
        radius: 9,
        stroke: false,
        fill: true,
        fillOpacity: 0,
        interactive: true,
        bubblingMouseEvents: false,
      }).addTo(gm.markers);
      const runId = view.runId;
      m.on('mouseover', (e: { containerPoint: { x: number; y: number } }) => {
        hover.set({ runId, index: idx });
        cardPos = { x: e.containerPoint.x, y: e.containerPoint.y };
      });
      m.on('mouseout', () => {
        hover.set(null);
        cardPos = null;
      });
    }
  }

  function redraw() {
    if (!gm || !gm.calib) return;
    gm.clearLines();
    gm.markers.clearLayers();
    highlight = null;
    const views = $runViews;
    if (views.length === 0) {
      lastFitKey = '';
      return;
    }

    const col = stateColors();
    const mode: 'scoring' | 'byRun' = views.length > 1 ? 'byRun' : $traceColorMode;

    const zone = zoneFor(views[0].row.zoneId);
    if (zone) drawZone(zone);

    const pts: ZonePoint[] = [];
    for (const v of views) {
      drawTrace(v, mode, col);
      for (const p of v.data.packets) if (nonZero(p)) pts.push({ x: p.positionX, z: p.positionZ });
    }

    // Frame the run only when the SET of runs changes (not on a colour-mode
    // toggle), so the camera isn't yanked. fitWorld leaves the camera free —
    // content-clamped zoom + pan like the rest of the app — so you can zoom
    // right in to inspect individual ticks.
    const key = views.map((v) => v.runId).join(',');
    if (pts.length && key !== lastFitKey) {
      fitZoom = gm.fitWorld(pts, 0.12, mapCfg?.viewMaxZoom);
      lastFitKey = key;
    }
    gm.setWeightRefZoom(fitZoom || gm.map.getZoom());

    for (const v of views) addMarkers(v);
  }

  // Per-tick hover card data.
  let card = $derived.by(() => {
    const h = $hover;
    if (!h || !cardPos) return null;
    const view = $runViews.find((v) => v.runId === h.runId);
    if (!view) return null;
    const p = view.data.packets[h.index];
    const t = view.data.ticks[h.index];
    if (!p || !t) return null;
    const on = (r: number) => r <= 0.05;
    return {
      color: view.color,
      runId: view.runId,
      // Sample clock (index / 64 Hz), matching the graph x-axis + cursor so the
      // card time and the timeline cursor agree (wall-clock timestampMs diverges
      // because FH6 emits duplicate timestamps).
      tSec: h.index / TELEMETRY_HZ,
      idx: h.index,
      angle: driftAngleDeg(p),
      speed: p.speedMs,
      rearSlip: Math.max(Math.abs(p.tireCombinedSlipRl), Math.abs(p.tireCombinedSlipRr)),
      throttle: Math.round((p.throttle / 255) * 100),
      brake: Math.round((p.brake / 255) * 100),
      state: scoreState(t) as ScoreState,
      wheels: { fl: on(p.surfaceRumbleFl), fr: on(p.surfaceRumbleFr), rl: on(p.surfaceRumbleRl), rr: on(p.surfaceRumbleRr) },
      tarmacOff: 4 - t.tarmacWheels,
    };
  });
</script>

<div class="run-map">
  <div class="map-host" bind:this={host}></div>

  {#if card && cardPos}
    <div
      class="tick-card"
      class:flip-x={cardPos.x > (host?.clientWidth ?? 0) - 230}
      class:flip-y={cardPos.y > (host?.clientHeight ?? 0) - 130}
      style:left="{cardPos.x}px"
      style:top="{cardPos.y}px"
    >
      <div class="tc-head">
        <span class="tc-dot" style:background={card.color}></span>
        <span class="mono">#{card.runId} · t {card.tSec.toFixed(2)}s</span>
        <span class="tc-state" data-state={card.state}>{SCORE_STATE_LABEL[card.state]}</span>
      </div>
      <div class="tc-grid">
        <span class="k">angle</span><span class="v mono">{card.angle.toFixed(1)}°</span>
        <span class="k">speed</span><span class="v mono">{card.speed.toFixed(1)} m/s</span>
        <span class="k">r.slip</span><span class="v mono">{card.rearSlip.toFixed(1)}</span>
        <span class="k">thr/brk</span><span class="v mono">{card.throttle}/{card.brake}%</span>
      </div>
      <div class="tc-wheels">
        <span class="k">tarmac</span>
        <span class="wh" class:off={!card.wheels.fl}>FL</span>
        <span class="wh" class:off={!card.wheels.fr}>FR</span>
        <span class="wh" class:off={!card.wheels.rl}>RL</span>
        <span class="wh" class:off={!card.wheels.rr}>RR</span>
        {#if card.tarmacOff > 0}<span class="tc-off">{card.tarmacOff} off</span>{/if}
      </div>
    </div>
  {/if}

  {#if $runViews.length === 0}
    <div class="map-empty">Select a run to plot it on the map.</div>
  {/if}
</div>

<style>
  .run-map { position: relative; width: 100%; height: 100%; min-height: 0; }
  .map-host {
    width: 100%;
    height: 100%;
    min-height: 0;
    border-radius: var(--r-md);
    overflow: hidden;
    background: var(--bg-card);
    isolation: isolate;
  }
  :global(.run-map .leaflet-container) { background: var(--bg-card); font: inherit; }

  .map-empty {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--tx-dim);
    font-size: 0.78rem;
    pointer-events: none;
  }

  .tick-card {
    position: absolute;
    z-index: 20;
    transform: translate(14px, 14px);
    width: 214px;
    background: var(--bg-panel);
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-md);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.5);
    padding: 7px 9px;
    font-size: 0.7rem;
    pointer-events: none;
  }
  .tick-card.flip-x { transform: translate(calc(-100% - 14px), 14px); }
  .tick-card.flip-y { transform: translate(14px, calc(-100% - 14px)); }
  .tick-card.flip-x.flip-y { transform: translate(calc(-100% - 14px), calc(-100% - 14px)); }
  .tc-head {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--tx-dim);
    border-bottom: 1px solid var(--bd-dim);
    padding-bottom: 4px;
    margin-bottom: 5px;
  }
  .tc-dot { width: 8px; height: 8px; border-radius: 2px; flex: none; }
  .tc-state { margin-left: auto; }
  .tc-state[data-state='scoring'] { color: var(--ok); }
  .tc-state[data-state='unpaid'] { color: var(--bad); }
  .tc-state[data-state='idle'] { color: var(--tx-dim); }
  .tc-grid { display: grid; grid-template-columns: auto 1fr auto 1fr; gap: 3px 8px; }
  .tc-grid .k, .tc-wheels .k { color: var(--tx-dim); }
  .tc-grid .v { color: var(--tx-hi); text-align: right; }
  .tc-wheels {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
    border-top: 1px solid var(--bd-dim);
    padding-top: 5px;
  }
  .wh { font-family: var(--font-mono); color: var(--ok); font-size: 0.66rem; }
  .wh.off { color: var(--bad); }
  .tc-off { margin-left: auto; color: var(--tx-dim); font-size: 0.62rem; }
</style>

<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { Map as LMap } from 'leaflet';
  import { packet } from '$lib/stores/telemetry';
  import { settings, saveSettings } from '$lib/stores/sessions';
  import { effectiveMapConfig, FH6_JAPAN } from '$lib/mapDefaults';
  import { xyzSimpleCRS } from '$lib/mapCrs';
  import { tileExtentBounds } from '$lib/mapView';
  import { themeColor } from '$lib/theme';
  import type { AppSettings } from '$lib/types';

  let { onClose }: { onClose: () => void } = $props();

  // Hold the last known in-game position so pausing (telemetry stops / sends
  // zeros) doesn't blank the readout mid-calibration.
  let lastX = $state(0);
  let lastZ = $state(0);
  let gotPos = $state(false);
  $effect(() => {
    const p = $packet;
    if (p && (p.positionX !== 0 || p.positionZ !== 0)) {
      lastX = p.positionX;
      lastZ = p.positionZ;
      gotPos = true;
    }
  });
  let posX = $derived(lastX);
  let posZ = $derived(lastZ);
  let hasLive = $derived(gotPos);

  type Pt = { world: [number, number] | null; pix: [number, number] | null };
  let A = $state<Pt>({ world: null, pix: null });
  let B = $state<Pt>({ world: null, pix: null });
  let target = $state<'A' | 'B'>('A');

  let host = $state<HTMLDivElement | null>(null);
  let L: typeof import('leaflet') | null = null;
  let map: LMap | null = null;
  let markers: import('leaflet').LayerGroup | null = null;

  const cfg = effectiveMapConfig($settings!);

  onMount(async () => {
    if (!host) return;
    L = await import('leaflet');
    await import('leaflet/dist/leaflet.css');

    map = L.map(host, {
      crs: xyzSimpleCRS(L),
      attributionControl: false,
      minZoom: cfg.minZoom,
      maxZoom: cfg.viewMaxZoom,
      maxBoundsViscosity: 1.0,
    });
    // Constrain camera + tile requests to the bundled tile extent (no overshoot
    // into empty space, no 404s for tiles that were never downloaded).
    const extent = tileExtentBounds(L, map, cfg);
    L.tileLayer(cfg.tileUrl, {
      minZoom: cfg.minZoom,
      maxZoom: cfg.viewMaxZoom,
      maxNativeZoom: cfg.maxZoom,
      tileSize: cfg.tileSize,
      noWrap: true,
      bounds: extent ?? undefined,
    }).addTo(map);
    if (extent) map.setMaxBounds(extent.pad(0.1));
    markers = L.layerGroup().addTo(map);

    // Centre on the bundled map's pixel extent.
    const c1 = map.unproject(L.point(FH6_JAPAN.pixelMin[0], FH6_JAPAN.pixelMin[1]), cfg.maxZoom);
    const c2 = map.unproject(L.point(FH6_JAPAN.pixelMax[0], FH6_JAPAN.pixelMax[1]), cfg.maxZoom);
    map.fitBounds(extent ?? L.latLngBounds(c1, c2));

    map.on('click', (e: import('leaflet').LeafletMouseEvent) => {
      const p = map!.project(e.latlng, cfg.maxZoom);
      const pt = [Math.round(p.x), Math.round(p.y)] as [number, number];
      if (target === 'A') A = { ...A, pix: pt };
      else B = { ...B, pix: pt };
      redraw();
    });
  });

  onDestroy(() => {
    map?.remove();
    map = null;
  });

  function redraw() {
    if (!map || !L || !markers) return;
    markers.clearLayers();
    for (const [pt, label, color] of [
      [A, 'A', themeColor('--map-left', '#84b577')],
      [B, 'B', themeColor('--map-right', '#82a7c8')],
    ] as const) {
      if (pt.pix) {
        const ll = map.unproject(L.point(pt.pix[0], pt.pix[1]), cfg.maxZoom);
        L.circleMarker(ll, {
          radius: 7,
          color: themeColor('--bg-body', '#0f1012'),
          weight: 1,
          fillColor: color,
          fillOpacity: 1,
        })
          .bindTooltip(label, { permanent: true, direction: 'top' })
          .addTo(markers);
      }
    }
  }

  function captureWorld(which: 'A' | 'B') {
    const w: [number, number] = [posX, posZ];
    if (which === 'A') A = { ...A, world: w };
    else B = { ...B, world: w };
  }

  let rows = $derived([
    { name: 'A' as const, pt: A },
    { name: 'B' as const, pt: B },
  ]);

  let complete = $derived(
    !!A.world && !!A.pix && !!B.world && !!B.pix &&
      (A.world[0] !== B.world[0] || A.world[1] !== B.world[1])
  );

  let viewMsg = $state('');
  async function saveDefaultView() {
    if (!map || !L || !$settings) return;
    const c = map.project(map.getCenter(), cfg.maxZoom);
    await saveSettings({
      ...$settings,
      mapDefaultCenter: [Math.round(c.x), Math.round(c.y)],
      mapDefaultZoom: Math.round(map.getZoom()),
    });
    viewMsg = 'Saved as default view';
    setTimeout(() => (viewMsg = ''), 2500);
  }

  async function apply() {
    if (!complete || !$settings) return;
    await saveSettings({
      ...$settings,
      mapCalAWorld: A.world!,
      mapCalAPix: A.pix!,
      mapCalBWorld: B.world!,
      mapCalBPix: B.pix!,
    });
    onClose();
  }

  let snippet = $derived(
    complete
      ? `calAWorld: [${A.world![0]}, ${A.world![1]}],\n` +
        `calAPix: [${A.pix![0]}, ${A.pix![1]}],\n` +
        `calBWorld: [${B.world![0]}, ${B.world![1]}],\n` +
        `calBPix: [${B.pix![0]}, ${B.pix![1]}],`
      : ''
  );
</script>

<div class="overlay" role="dialog" aria-modal="true">
  <div class="cal">
    <header>
      <h3>Map Calibration</h3>
      <button class="close" onclick={onClose}>✕</button>
    </header>

    <div class="body">
      <div class="map" bind:this={host}></div>

      <aside class="side">
        <p class="help">
          For each point: drive in-game to a recognisable spot, press
          <em>Capture world</em>, then click that exact spot on the map.
        </p>

        <div class="live" class:on={hasLive}>
          <span>Live position</span>
          <strong>{hasLive ? `X ${posX.toFixed(1)}  Z ${posZ.toFixed(1)}` : 'no telemetry'}</strong>
        </div>

        <div class="target">
          <button class:sel={target === 'A'} onclick={() => (target = 'A')}>Point A</button>
          <button class:sel={target === 'B'} onclick={() => (target = 'B')}>Point B</button>
          <span class="hint">Map clicks set the selected point.</span>
        </div>

        {#each rows as row (row.name)}
          <div class="pt">
            <div class="pt-head">{row.name}</div>
            <div class="pt-row">
              <span>World</span>
              <code>{row.pt.world ? `${row.pt.world[0].toFixed(1)}, ${row.pt.world[1].toFixed(1)}` : '—'}</code>
              <button disabled={!hasLive} onclick={() => captureWorld(row.name)}>
                Capture world
              </button>
            </div>
            <div class="pt-row">
              <span>Pixel</span>
              <code>{row.pt.pix ? `${row.pt.pix[0]}, ${row.pt.pix[1]}` : '— click map'}</code>
            </div>
          </div>
        {/each}

        {#if complete}
          <div class="snippet">
            <span class="hint">Saved to your settings on Apply. To bake in as
              the default for everyone, paste into <code>src/lib/mapDefaults.ts</code>:</span>
            <pre>{snippet}</pre>
          </div>
        {/if}

        <div class="view-row">
          <button onclick={saveDefaultView}>Save current view as default</button>
          {#if viewMsg}<span class="hint">{viewMsg}</span>{/if}
        </div>

        <div class="actions">
          <button onclick={onClose}>Cancel</button>
          <button class="primary" disabled={!complete} onclick={apply}>
            Apply calibration
          </button>
        </div>
      </aside>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed; inset: 0; background: rgba(0, 0, 0, 0.75);
    display: flex; align-items: center; justify-content: center;
    /* Above the settings modal (1200) that opens it. */
    z-index: 1300;
  }
  .cal {
    width: min(1100px, 96vw); height: min(760px, 94vh);
    background: var(--bg-panel); border: 1px solid var(--bd-muted);
    border-radius: var(--r-lg); display: flex; flex-direction: column;
    box-shadow: 0 16px 60px rgba(0, 0, 0, 0.55);
  }
  header {
    display: flex; justify-content: space-between; align-items: center;
    padding: 0.8rem 1.1rem; border-bottom: 1px solid var(--bd-dim);
  }
  h3 { margin: 0; color: var(--tx-hi); font-size: 1rem; }
  .close { background: none; border: none; color: var(--tx-dim); font-size: 1.1rem; cursor: pointer; }
  .close:hover { color: var(--tx-hi); }
  .body { flex: 1; display: grid; grid-template-columns: 1fr 320px; min-height: 0; }
  .map { background: var(--bg-card); min-width: 0; }
  .side {
    border-left: 1px solid var(--bd-dim); padding: 0.9rem;
    display: flex; flex-direction: column; gap: 0.8rem; overflow-y: auto;
  }
  .help { color: var(--tx-lo); font-size: 0.78rem; }
  .live {
    display: flex; flex-direction: column; gap: 0.15rem;
    background: var(--bg-card); border: 1px solid var(--bd-dim);
    border-radius: 6px; padding: 0.5rem 0.7rem;
  }
  .live span { color: var(--tx-dim); font-size: 0.62rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.12em; }
  .live strong { color: var(--tx-dim); font-size: 0.85rem; font-family: var(--font-mono); font-variant-numeric: tabular-nums; }
  .live.on strong { color: var(--tx-hi); }
  .target { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; }
  .target button {
    background: var(--bg-elevated); border: 1px solid var(--bd-muted);
    color: var(--tx-dim); padding: 0.3rem 0.7rem; border-radius: var(--r-sm);
    font-size: 0.78rem; cursor: pointer; font-family: inherit;
  }
  .target button.sel { border-color: color-mix(in srgb, var(--ac) 55%, var(--bg-panel)); color: var(--ac); background: var(--ac-wash); }
  .pt {
    background: var(--bg-card); border: 1px solid var(--bd-dim);
    border-radius: 6px; padding: 0.5rem 0.6rem;
    display: flex; flex-direction: column; gap: 0.3rem;
  }
  .pt-head { color: var(--tx-mid); font-weight: 700; font-size: 0.8rem; }
  .pt-row { display: flex; align-items: center; gap: 0.5rem; font-size: 0.75rem; }
  .pt-row span { color: var(--tx-dim); width: 3rem; }
  .pt-row code { color: var(--tx-hi); flex: 1; font-family: var(--font-mono); }
  .pt-row button {
    background: var(--ac); color: var(--bg-body); border: none; border-radius: var(--r-sm);
    padding: 0.2rem 0.5rem; font-size: 0.72rem; font-weight: 600; cursor: pointer; font-family: inherit;
  }
  .pt-row button:hover:not(:disabled) { background: var(--ac-bright); }
  .pt-row button:disabled { opacity: 0.4; cursor: default; }
  .snippet pre {
    background: var(--bg-body); border: 1px solid var(--bd-dim);
    border-radius: var(--r-sm); padding: 0.5rem; color: var(--tx-mid);
    font-size: 0.68rem; font-family: var(--font-mono); white-space: pre-wrap; margin-top: 0.3rem;
  }
  .hint { font-size: 0.68rem; color: var(--tx-dim); }
  .view-row {
    display: flex; align-items: center; gap: 0.5rem; margin-top: auto;
    flex-wrap: wrap;
  }
  .view-row button {
    background: var(--bg-elevated); border: 1px solid var(--bd-muted);
    color: var(--tx-mid); padding: 0.35rem 0.7rem; border-radius: var(--r-sm);
    font-size: 0.78rem; cursor: pointer; font-family: inherit;
  }
  .view-row button:hover { border-color: var(--bd-strong); color: var(--tx-hi); }
  .actions { display: flex; justify-content: flex-end; gap: 0.5rem; }
  .actions button {
    padding: 0.4rem 0.9rem; border-radius: var(--r-sm); border: 1px solid var(--bd-muted);
    background: var(--bg-elevated); color: var(--tx-mid); cursor: pointer; font-size: 0.82rem;
    font-family: inherit;
  }
  .actions button.primary { background: var(--ac); border-color: var(--ac); color: var(--bg-body); font-weight: 650; }
  .actions button.primary:hover:not(:disabled) { background: var(--ac-bright); border-color: var(--ac-bright); }
  .actions button:disabled { opacity: 0.45; cursor: default; }
  :global(.leaflet-container) { background: var(--bg-card); font: inherit; }
</style>

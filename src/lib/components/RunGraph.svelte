<script lang="ts">
  import { onDestroy } from 'svelte';
  import uPlot from 'uplot';
  import 'uplot/dist/uPlot.min.css';
  import { themeColor } from '$lib/theme';
  import {
    lanes,
    runViews,
    hover,
    addLane,
    removeLane,
    toggleLaneChannel,
    setLaneHeight,
    MAX_LANES,
    type LaneConfig,
  } from '$lib/stores/runViewer';
  import { CHANNELS, CHANNELS_BY_KEY, channelSeries, TELEMETRY_HZ } from '$lib/runViewer';
  import CheckboxDropdown from './CheckboxDropdown.svelte';

  const SYNC_KEY = 'runs-viewer';
  const DASHES: number[][] = [[], [6, 3], [2, 3], [8, 3, 2, 3]];

  let host = $state<HTMLDivElement | null>(null);
  const plots = new Map<number, uPlot>();

  // Suppress the hover→store write while we move the cursor programmatically,
  // so map→graph sync doesn't loop back into graph→map.
  let syncing = false;

  function destroyAll() {
    for (const p of plots.values()) p.destroy();
    plots.clear();
  }
  onDestroy(destroyAll);

  function fmtTime(s: number) {
    const m = Math.floor(s / 60);
    return `${m}:${(s % 60).toFixed(2).padStart(5, '0')}`;
  }

  // Compact axis labels so large totals (cumulative points) fit the gutter.
  function fmtCompact(n: number): string {
    const a = Math.abs(n);
    if (a >= 1e6) return `${(n / 1e6).toFixed(a >= 1e7 ? 0 : 1)}M`;
    if (a >= 1e3) return `${(n / 1e3).toFixed(a >= 1e4 ? 0 : 1)}k`;
    return `${Math.round(n)}`;
  }

  // Tooltip: rows = channels, columns = runs. Series order is run-major,
  // channel-inner → u.data[runIdx * nCh + chIdx + 1].
  function tooltip(channels: string[], runs: Array<{ label: string; color: string }>): uPlot.Plugin {
    let el: HTMLDivElement;
    const nCh = channels.length;
    const cols = `max-content ${runs.map(() => 'minmax(40px,auto)').join(' ')}`;
    return {
      hooks: {
        init(u) {
          el = document.createElement('div');
          el.className = 'u-tt';
          u.over.appendChild(el);
          u.over.addEventListener('mouseleave', () => (el.style.display = 'none'));
        },
        setCursor(u) {
          const idx = u.cursor.idx;
          if (idx == null) { el.style.display = 'none'; return; }
          const x = (u.data[0] as number[])[idx];
          if (x == null) { el.style.display = 'none'; return; }
          let grid = `<div></div>`;
          for (const r of runs) grid += `<div class="u-tt-hdr"><span class="u-tt-sw" style="background:${r.color}"></span>${r.label}</div>`;
          for (let ci = 0; ci < nCh; ci++) {
            grid += `<div class="u-tt-metric">${channels[ci]}</div>`;
            for (let ri = 0; ri < runs.length; ri++) {
              const v = (u.data[ri * nCh + ci + 1] as (number | null)[] | undefined)?.[idx];
              grid += `<div class="u-tt-val">${v != null ? v.toFixed(1) : '—'}</div>`;
            }
          }
          el.innerHTML = `<div class="u-tt-time">${fmtTime(x)}</div><div class="u-tt-grid" style="grid-template-columns:${cols}">${grid}</div>`;
          el.style.display = 'block';
          const cx = u.cursor.left ?? 0;
          const tw = el.offsetWidth;
          const flip = cx + 16 + tw > u.over.clientWidth;
          el.style.left = flip ? `${cx - tw - 4}px` : `${cx + 12}px`;
          el.style.top = `${Math.max(0, (u.cursor.top ?? 0) - el.offsetHeight / 2)}px`;
        },
      },
    };
  }

  // Drive the shared hover store from the graph cursor (graph → map highlight).
  function cursorBridge(): uPlot.Plugin {
    return {
      hooks: {
        setCursor(u) {
          if (syncing) return;
          const idx = u.cursor.idx;
          const focused = $runViews[0];
          if (idx == null || !focused) hover.set(null);
          else hover.set({ runId: focused.runId, index: idx });
        },
      },
    };
  }

  function buildLane(lane: LaneConfig, mount: HTMLDivElement) {
    const views = $runViews;
    const chDefs = lane.channelKeys.map((k) => CHANNELS_BY_KEY.get(k)).filter(Boolean) as typeof CHANNELS;
    if (views.length === 0 || chDefs.length === 0) return;

    const maxN = Math.max(...views.map((v) => v.data.packets.length));
    const x = Array.from({ length: maxN }, (_, i) => i / TELEMETRY_HZ);

    const series: uPlot.Series[] = [{ label: 't' }];
    const data: (number | null)[][] = [x];
    for (const v of views) {
      chDefs.forEach((def, ci) => {
        const arr = channelSeries(v.data, def);
        const padded: (number | null)[] = arr.length < maxN ? [...arr, ...Array(maxN - arr.length).fill(null)] : arr;
        data.push(padded);
        series.push({
          label: `#${v.runId} ${def.label}`,
          stroke: v.color,
          dash: DASHES[ci % DASHES.length],
          width: 1.3,
        });
      });
    }

    const axisColor = themeColor('--tx-dim', '#7c7d78');
    const gridColor = themeColor('--bd-dim', '#222428');
    const opts: uPlot.Options = {
      width: mount.clientWidth || 600,
      height: lane.height,
      legend: { show: false },
      cursor: { sync: { key: SYNC_KEY }, points: { size: 6 } },
      scales: { x: { time: false } },
      plugins: [
        tooltip(chDefs.map((d) => d.label), views.map((v) => ({ label: `#${v.runId}`, color: v.color }))),
        cursorBridge(),
      ],
      series,
      axes: [
        { stroke: axisColor, grid: { stroke: gridColor }, ticks: { stroke: gridColor } },
        {
          stroke: axisColor,
          grid: { stroke: gridColor },
          ticks: { stroke: gridColor },
          size: 50,
          values: (_u, splits) => splits.map((v) => fmtCompact(v as number)),
        },
      ],
    };
    plots.set(lane.id, new uPlot(opts, data as uPlot.AlignedData, mount));
  }

  // Structural rebuild key: rebuild plots when lanes' channels or the run set
  // change — NOT on height (handled separately via setSize).
  let structureKey = $derived(
    JSON.stringify($lanes.map((l) => ({ id: l.id, ch: l.channelKeys }))) +
      '|' +
      $runViews.map((v) => v.runId).join(','),
  );

  $effect(() => {
    void structureKey;
    const h = host;
    if (!h) return;
    destroyAll();
    const mounts = h.querySelectorAll<HTMLDivElement>('.lane-chart');
    for (const m of mounts) {
      const id = Number(m.dataset.lane);
      const lane = $lanes.find((l) => l.id === id);
      if (lane) buildLane(lane, m);
    }
  });

  // Apply height changes without a full rebuild.
  $effect(() => {
    for (const lane of $lanes) {
      const p = plots.get(lane.id);
      const m = host?.querySelector<HTMLDivElement>(`.lane-chart[data-lane="${lane.id}"]`);
      if (p && m) p.setSize({ width: m.clientWidth || 600, height: lane.height });
    }
  });

  // Map → graph cursor follow.
  $effect(() => {
    const hv = $hover;
    if (!hv) return;
    syncing = true;
    for (const [, p] of plots) {
      const xs = p.data[0] as number[];
      const tval = hv.index / TELEMETRY_HZ;
      const left = p.valToPos(tval, 'x');
      const top = p.valToPos((p.scales.y?.max ?? 1), 'y');
      if (Number.isFinite(left)) p.setCursor({ left, top: Number.isFinite(top) ? top : 0 });
      void xs;
    }
    syncing = false;
  });

  // Keep plots sized to the host on resize.
  $effect(() => {
    const h = host;
    if (!h || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(() => {
      for (const lane of $lanes) {
        const p = plots.get(lane.id);
        const m = h.querySelector<HTMLDivElement>(`.lane-chart[data-lane="${lane.id}"]`);
        if (p && m) p.setSize({ width: m.clientWidth || 600, height: lane.height });
      }
    });
    ro.observe(h);
    return () => ro.disconnect();
  });

  // Lane resize drag.
  function startResize(e: PointerEvent, lane: LaneConfig) {
    e.preventDefault();
    const startY = e.clientY;
    const startH = lane.height;
    const move = (ev: PointerEvent) => setLaneHeight(lane.id, startH + (ev.clientY - startY));
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  const channelOpts = CHANNELS.map((c) => ({ value: c.key, label: c.label, hint: c.unit }));
</script>

<div class="graph" bind:this={host}>
  {#each $lanes as lane (lane.id)}
    <div class="lane">
      <div class="lane-head">
        <span class="lane-tag">Lane {lane.id}</span>
        <CheckboxDropdown
          label={lane.channelKeys.length ? `${lane.channelKeys.length} ch` : 'channels'}
          options={channelOpts}
          selected={lane.channelKeys}
          onToggle={(k) => toggleLaneChannel(lane.id, k)}
        />
        <span class="lane-h mono">{lane.height}px</span>
        {#if $lanes.length > 1}
          <button class="lane-x" title="Remove lane" onclick={() => removeLane(lane.id)} aria-label="Remove lane">✕</button>
        {/if}
      </div>
      <div class="lane-chart" data-lane={lane.id} style:height="{lane.height}px"></div>
      <div
        class="resize"
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize lane"
        title="Drag to resize"
        onpointerdown={(e) => startResize(e, lane)}
      >
        <span></span><span></span>
      </div>
    </div>
  {/each}

  <div class="graph-foot">
    {#if $lanes.length < MAX_LANES}
      <button class="add-lane" onclick={addLane}>+ Add lane</button>
    {/if}
    <span class="lane-count mono">{$lanes.length} / {MAX_LANES}</span>
    {#if $runViews.length === 0}
      <span class="foot-hint">select a run to populate the graph</span>
    {/if}
  </div>
</div>

<style>
  .graph { display: flex; flex-direction: column; gap: 6px; min-height: 0; overflow-y: auto; padding: 2px; }
  .lane { border: 1px solid var(--bd-dim); border-radius: var(--r-sm); background: var(--bg-card); overflow: hidden; }
  .lane-head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 8px;
    border-bottom: 1px solid var(--bd-dim);
    font-size: 0.66rem;
    color: var(--tx-lo);
  }
  .lane-tag { color: var(--tx-mid); }
  .lane-h { margin-left: auto; color: var(--tx-dim); font-size: 0.62rem; }
  .lane-x { background: none; border: none; color: var(--tx-dim); cursor: pointer; font-size: 0.7rem; padding: 0 2px; }
  .lane-x:hover { color: var(--bad); }
  .lane-chart { width: 100%; }
  .resize {
    height: 7px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 3px;
    background: var(--bg-panel);
    border-top: 1px solid var(--bd-dim);
    cursor: row-resize;
  }
  .resize span { width: 14px; height: 2px; border-radius: 1px; background: var(--bd-strong); }

  .graph-foot { display: flex; align-items: center; gap: 10px; padding: 2px 4px; }
  .add-lane {
    background: var(--bg-card);
    border: 1px solid var(--bd-muted);
    color: var(--tx-lo);
    font-family: inherit;
    font-size: 0.68rem;
    border-radius: var(--r-sm);
    padding: 0.2rem 0.5rem;
    cursor: pointer;
  }
  .add-lane:hover { border-color: var(--bd-strong); color: var(--tx-mid); }
  .lane-count { font-size: 0.62rem; color: var(--tx-dim); }
  .foot-hint { font-size: 0.66rem; color: var(--tx-dim); margin-left: auto; }

  :global(.graph .u-tt) {
    position: absolute;
    z-index: 10;
    background: var(--bg-panel);
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-md);
    padding: 0.35rem 0.5rem;
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.45);
    font-size: 0.7rem;
    pointer-events: none;
  }
  :global(.graph .u-tt-time) { color: var(--tx-dim); font-family: var(--font-mono); margin-bottom: 3px; }
  :global(.graph .u-tt-grid) { display: grid; column-gap: 0.55rem; row-gap: 0.12rem; align-items: center; }
  :global(.graph .u-tt-hdr) { display: flex; align-items: center; gap: 4px; color: var(--tx-mid); }
  :global(.graph .u-tt-sw) { width: 7px; height: 7px; border-radius: 50%; display: inline-block; }
  :global(.graph .u-tt-metric) { color: var(--tx-mid); }
  :global(.graph .u-tt-val) { font-family: var(--font-mono); font-variant-numeric: tabular-nums; color: var(--tx-hi); text-align: right; }
</style>

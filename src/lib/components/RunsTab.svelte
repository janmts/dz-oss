<script lang="ts">
  import RunSelector from './RunSelector.svelte';
  import RunMap from './RunMap.svelte';
  import RunGraph from './RunGraph.svelte';
  import RunSectors from './RunSectors.svelte';
  import { graphLayout, traceColorMode, runViews, loadError, loadingRunIds } from '$lib/stores/runViewer';

  let multi = $derived($runViews.length > 1);
  let effectiveMode = $derived(multi ? 'byRun' : $traceColorMode);
</script>

<div class="runs">
  <div class="toolbar">
      <div class="seg">
        <button class:on={$graphLayout === 'map+graph'} onclick={() => graphLayout.set('map+graph')}>Map + graph</button>
        <button class:on={$graphLayout === 'graph'} onclick={() => graphLayout.set('graph')}>Graph only</button>
      </div>

      {#if $graphLayout !== 'graph'}
        {#if effectiveMode === 'scoring'}
          <div class="legend">
            <span class="lg"><i style="background:var(--ok-bright)"></i>scoring</span>
            <span class="lg"><i style="background:var(--bad)"></i>off-tarmac / unpaid</span>
            <span class="lg"><i style="background:var(--tx-dim)"></i>idle</span>
          </div>
        {:else}
          <div class="legend">
            {#each $runViews as v (v.runId)}
              <span class="lg"><i style="background:{v.color}"></i>#{v.runId}</span>
            {/each}
          </div>
        {/if}

        <!-- Zone overlay vocabulary (the map draws these regardless of trace mode). -->
        <div class="legend vocab">
          <span class="lg"><i style="background:var(--zone-edge)"></i>boundary</span>
          <span class="lg"><i class="sq" style="background:var(--gate-a)"></i>A</span>
          <span class="lg"><i class="sq" style="background:var(--gate-b)"></i>B</span>
          <span class="lg"><i style="background:var(--gate-split)"></i>split</span>
        </div>

        <div class="colour-mode">
          <button
            class:on={$traceColorMode === 'scoring' && !multi}
            disabled={multi}
            title={multi ? 'Colour by run while comparing' : 'Colour the trace by scoring state'}
            onclick={() => traceColorMode.set('scoring')}
          >Scoring</button>
          <button
            class:on={effectiveMode === 'byRun'}
            onclick={() => traceColorMode.set('byRun')}
          >By run</button>
        </div>
      {/if}

      {#if $loadingRunIds.size > 0}<span class="status">loading…</span>{/if}
      {#if $loadError}<span class="status err">{$loadError}</span>{/if}
    </div>

  <div class="body">
    <aside class="left">
      <div class="rail"><RunSelector /></div>
      <div class="sector-section"><RunSectors /></div>
    </aside>

    <section class="stage" class:graph-only={$graphLayout === 'graph'}>
      {#if $graphLayout !== 'graph'}
        <div class="map-region"><RunMap /></div>
      {/if}
      <div class="graph-region"><RunGraph /></div>
    </section>
  </div>
</div>

<style>
  /* Full-width toolbar on top; below it a left column (run list + per-sector
     section) beside the stage. The left column mirrors the stage's map:graph
     flex ratio so the run list ends roughly at the map's bottom edge, leaving
     the section to sit beside the graph. */
  .runs { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .body { flex: 1; min-height: 0; display: grid; grid-template-columns: 240px minmax(0, 1fr); }
  .left { display: flex; flex-direction: column; min-height: 0; border-right: 1px solid var(--bd-dim); background: var(--bg-panel); }
  .rail { flex: 1.5 1 0; min-height: 0; overflow: hidden; }
  .sector-section { flex: 1 1 0; min-height: 0; overflow: hidden; border-top: 1px solid var(--bd-dim); }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--bd-dim);
    background: var(--bg-panel);
    flex-wrap: wrap;
  }
  .seg, .colour-mode { display: inline-flex; border: 1px solid var(--bd-muted); border-radius: var(--r-sm); overflow: hidden; }
  .seg button, .colour-mode button {
    background: none;
    border: none;
    font-family: inherit;
    font-size: 0.66rem;
    color: var(--tx-lo);
    padding: 0.26rem 0.6rem;
    cursor: pointer;
  }
  .seg button.on, .colour-mode button.on { background: var(--bg-elevated); color: var(--tx-hi); }
  .colour-mode button:disabled { color: var(--tx-ghost); cursor: default; }
  .colour-mode { margin-left: auto; }

  .legend { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
  .legend.vocab { margin-top: 4px; }
  .lg { display: inline-flex; align-items: center; gap: 5px; font-size: 0.66rem; color: var(--tx-lo); }
  .lg i { width: 13px; height: 3px; border-radius: 2px; display: inline-block; }
  /* Square swatch echoes the A/B gate-post glyph (vs the bar = a line). */
  .lg i.sq { width: 9px; height: 9px; border-radius: var(--r-xs); }

  .status { font-size: 0.66rem; color: var(--tx-dim); }
  .status.err { color: var(--bad); }

  .stage { min-width: 0; min-height: 0; display: flex; flex-direction: column; padding: 8px; gap: 8px; }
  .map-region { flex: 1.5; min-height: 0; }
  .graph-region { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .stage.graph-only .graph-region { flex: 1; }
</style>

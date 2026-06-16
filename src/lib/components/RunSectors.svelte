<script lang="ts">
  // The "Per sector" section: a per-sector points readout for the selected run(s),
  // plus the leading-run diff when comparing two runs. Sits in the bottom-left
  // cell (under the run list), beside the graph.
  import { runViews } from '$lib/stores/runViewer';
  import { driftZones } from '$lib/stores/sessions';
  import { sectorRows, hasSectors, type RunView, type SectorRow } from '$lib/runViewer';

  function zoneForRun(v: RunView) {
    return $driftZones.find((z) => z.id === v.row.zoneId) ?? null;
  }

  // Per-run sector rows, only for runs whose zone actually has splits.
  let perRun = $derived(
    $runViews
      .filter((v) => hasSectors(v.data))
      .map((v) => ({ view: v, rows: sectorRows(v.data, zoneForRun(v)) })),
  );

  let sectorCount = $derived(Math.max(0, ...perRun.map((r) => r.rows.length)));
  // Bar scale: the biggest single-sector total across every run, so bars compare.
  let maxPoints = $derived(Math.max(1, ...perRun.flatMap((r) => r.rows.map((s) => s.points))));
  let comparing = $derived(perRun.length === 2);

  function labelFor(i: number): string {
    for (const r of perRun) if (r.rows[i]) return r.rows[i].name;
    return `Sector ${i + 1}`;
  }
  function cell(runIdx: number, i: number): SectorRow | undefined {
    return perRun[runIdx]?.rows[i];
  }
  const fmt = (n: number) => Math.round(n).toLocaleString();
</script>

<div class="sectors">
  <div class="head">
    <span class="title">Per sector</span>
  </div>

  {#if $runViews.length === 0}
    <p class="hint">Select a run to see its per-sector score.</p>
  {:else if perRun.length === 0}
    <p class="hint">This zone has no split sectors. Add splits in the zone editor to break a run down by section.</p>
  {:else}
    <div class="legend">
      {#each perRun as r (r.view.runId)}
        <span class="lg"><i style:background={r.view.color}></i>#{r.view.runId}</span>
      {/each}
    </div>

    <div class="list">
      {#each Array(sectorCount) as _, i (i)}
        {@const a = cell(0, i)}
        {@const b = cell(1, i)}
        <div class="sec">
          <div class="sec-top">
            <span class="sec-name" title={labelFor(i)}>{labelFor(i)}</span>
            {#if comparing && a && b}
              {@const lead = b.points >= a.points ? perRun[1] : perRun[0]}
              {@const margin = Math.abs(b.points - a.points)}
              {#if margin < 1}
                <span class="sec-lead even">even</span>
              {:else}
                <span class="sec-lead" style:color={lead.view.color} title="#{lead.view.runId} scored the most here, by {fmt(margin)} pts">#{lead.view.runId} +{fmt(margin)}</span>
              {/if}
            {/if}
          </div>
          {#each perRun as r (r.view.runId)}
            {@const s = r.rows[i]}
            <div class="row" title={s ? `${fmt(s.points)} pts · ${s.pct.toFixed(0)}% · ${s.driftTimeS.toFixed(1)}s drifting` : 'no data'}>
              <span class="bar-track">
                <span class="bar" style:width="{s ? (s.points / maxPoints) * 100 : 0}%" style:background={r.view.color}></span>
              </span>
              <span class="pts mono">{s ? fmt(s.points) : '—'}</span>
              <span class="pct mono">{s ? `${s.pct.toFixed(0)}%` : ''}</span>
            </div>
          {/each}
        </div>
      {/each}
    </div>

    <p class="foot">Split lines on the graph follow the primary run (#{perRun[0].view.runId}).</p>
  {/if}
</div>

<style>
  .sectors { display: flex; flex-direction: column; min-height: 0; height: 100%; padding: 8px; gap: 8px; }
  .head { display: flex; align-items: center; gap: 8px; }
  .title { font-size: 0.7rem; color: var(--tx-mid); text-transform: uppercase; letter-spacing: 0.05em; }

  .hint { font-size: 0.7rem; color: var(--tx-dim); line-height: 1.5; margin: 0; }

  .legend { display: flex; gap: 12px; flex-wrap: wrap; }
  .lg { display: inline-flex; align-items: center; gap: 5px; font-size: 0.64rem; color: var(--tx-lo); }
  .lg i { width: 10px; height: 10px; border-radius: 2px; display: inline-block; }

  .list { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: 6px; padding-right: 2px; }
  .sec {
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-sm);
    background: var(--bg-card);
    padding: 5px 7px;
  }
  .sec-top { display: flex; align-items: baseline; gap: 6px; margin-bottom: 3px; }
  .sec-name {
    font-size: 0.68rem;
    color: var(--tx-mid);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Leading run + margin in that run's colour, so the chip itself names who's
     ahead in the sector — no baseline guesswork. */
  .sec-lead { margin-left: auto; font-size: 0.62rem; font-family: var(--font-mono); }
  .sec-lead.even { color: var(--tx-dim); }

  .row { display: flex; align-items: center; gap: 8px; padding: 1px 0; }
  .bar-track { flex: 1; min-width: 0; height: 7px; background: var(--bg-panel); border-radius: 2px; overflow: hidden; }
  .bar { display: block; height: 100%; border-radius: 2px; min-width: 1px; }
  .pts { font-size: 0.66rem; color: var(--tx-hi); min-width: 7ch; text-align: right; flex: none; }
  .pct { font-size: 0.6rem; color: var(--tx-dim); min-width: 4ch; text-align: right; flex: none; }

  .foot { font-size: 0.58rem; color: var(--tx-dim); margin: 0; line-height: 1.4; }
</style>

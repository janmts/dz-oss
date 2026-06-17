<script lang="ts">
  import { onMount } from 'svelte';
  import { driftRuns, driftZones, loadDriftRuns, loadDriftZones } from '$lib/stores/sessions';
  import { selectedRunIds, runViews, focusRun, toggleRun } from '$lib/stores/runViewer';
  import { carName } from '$lib/car-name';
  import CheckboxDropdown from './CheckboxDropdown.svelte';
  import type { DriftRunRow } from '$lib/types';

  let search = $state('');
  let fCars = $state<string[]>([]);
  let fZones = $state<string[]>([]);
  let fSeasons = $state<string[]>([]);

  onMount(() => {
    void loadDriftZones();
    void loadDriftRuns();
  });

  const SEASONS = ['spring', 'summer', 'autumn', 'winter'];

  let zoneName = $derived(new Map($driftZones.map((z) => [z.id, z.name])));
  let colorById = $derived(new Map($runViews.map((v) => [v.runId, v.color])));

  // Filter option lists, derived from the runs actually present.
  let carOpts = $derived(
    [...new Set($driftRuns.map((r) => r.carOrdinal))]
      .map((ord) => ({ value: String(ord), label: carName(ord) }))
      .sort((a, b) => a.label.localeCompare(b.label)),
  );
  let zoneOpts = $derived(
    $driftZones.map((z) => ({ value: String(z.id), label: z.name })),
  );
  let seasonOpts = SEASONS.map((s) => ({ value: s, label: s }));

  function toggleIn(arr: string[], v: string): string[] {
    return arr.includes(v) ? arr.filter((x) => x !== v) : [...arr, v];
  }

  let filtered = $derived(
    $driftRuns.filter((r) => {
      if (fCars.length && !fCars.includes(String(r.carOrdinal))) return false;
      if (fZones.length && !(r.zoneId != null && fZones.includes(String(r.zoneId)))) return false;
      if (fSeasons.length && !fSeasons.includes(r.season)) return false;
      if (search.trim()) {
        const q = search.trim().toLowerCase();
        const hay = `#${r.id} ${carName(r.carOrdinal)} ${r.zoneId != null ? zoneName.get(r.zoneId) ?? '' : ''}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    }),
  );

  function score(r: DriftRunRow): string {
    const v = r.computedScore ?? r.manualScore;
    return v != null ? Math.round(v).toLocaleString() : '—';
  }
</script>

<div class="selector">
  <div class="filters">
    <input class="search" placeholder="Search runs…" bind:value={search} />
    <div class="filter-row">
      <CheckboxDropdown label="Cars" options={carOpts} selected={fCars} onToggle={(v) => (fCars = toggleIn(fCars, v))} />
      <CheckboxDropdown label="Zones" options={zoneOpts} selected={fZones} onToggle={(v) => (fZones = toggleIn(fZones, v))} />
      <CheckboxDropdown label="Seasons" options={seasonOpts} selected={fSeasons} onToggle={(v) => (fSeasons = toggleIn(fSeasons, v))} searchable={false} />
    </div>
  </div>

  <div class="count">{filtered.length} run{filtered.length === 1 ? '' : 's'} · {$selectedRunIds.length} selected</div>

  <div class="run-list">
    {#each filtered as r (r.id)}
      {@const sel = $selectedRunIds.includes(r.id)}
      <div class="run-row" class:selected={sel} class:invalid={!r.valid}>
        <input
          type="checkbox"
          class="cmp"
          checked={sel}
          title="Add to comparison"
          onchange={() => toggleRun(r)}
        />
        <button type="button" class="run-main" onclick={() => focusRun(r)}>
          {#if sel}
            <span class="swatch" style:background={colorById.get(r.id) ?? 'var(--ac)'}></span>
          {:else}
            <span class="swatch ghost"></span>
          {/if}
          <span class="rid mono">#{r.id}</span>
          <span class="rzone">{r.zoneId != null ? zoneName.get(r.zoneId) ?? '—' : '—'}</span>
          <span class="rseason" data-season={r.season}>{r.season[0].toUpperCase()}</span>
          {#if r.scoreBreakdown?.rewinds?.length}
            <span
              class="rrewind"
              title="{r.scoreBreakdown.rewinds.length} in-game rewind{r.scoreBreakdown.rewinds
                .length === 1
                ? ''
                : 's'} detected — the score over-counts the replayed stretch; excluded from the fit"
            >⟲</span>
          {/if}
          {#if r.valid}
            <span class="rscore mono">{score(r)}</span>
          {:else}
            <span class="rinvalid">invalid</span>
          {/if}
        </button>
      </div>
    {/each}
    {#if filtered.length === 0}
      <div class="empty">No runs match.</div>
    {/if}
  </div>
</div>

<style>
  .selector { display: flex; flex-direction: column; min-height: 0; height: 100%; }
  .filters { padding: 8px; border-bottom: 1px solid var(--bd-dim); display: flex; flex-direction: column; gap: 7px; }
  .search {
    width: 100%;
    font-family: inherit;
    font-size: 0.72rem;
    color: var(--tx-mid);
    background: var(--bg-card);
    border: 1px solid var(--bd-subtle);
    border-radius: var(--r-sm);
    padding: 0.32rem 0.55rem;
  }
  .filter-row { display: flex; gap: 5px; flex-wrap: wrap; }
  .count {
    font-size: 0.62rem;
    color: var(--tx-dim);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 6px 9px 4px;
  }
  .run-list { flex: 1; min-height: 0; overflow-y: auto; padding: 0 6px 8px; }

  .run-row {
    display: flex;
    align-items: center;
    gap: 4px;
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-sm);
    margin-bottom: 4px;
    padding-left: 5px;
  }
  .run-row.selected {
    border-color: color-mix(in srgb, var(--ac) 45%, var(--bg-panel));
    box-shadow: inset 2px 0 0 var(--ac);
    background: color-mix(in srgb, var(--ac) 6%, var(--bg-panel));
  }
  .run-row.invalid {
    border-color: color-mix(in srgb, var(--bad) 42%, var(--bg-panel));
    background: color-mix(in srgb, var(--bad) 4%, var(--bg-panel));
  }
  .cmp { accent-color: var(--ac); cursor: pointer; flex: none; }
  .run-main {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    font-family: inherit;
    color: var(--tx-mid);
    padding: 0.34rem 0.4rem;
    cursor: pointer;
    text-align: left;
  }
  .swatch { width: 8px; height: 8px; border-radius: 2px; flex: none; }
  .swatch.ghost { border: 1px solid var(--bd-muted); background: transparent; }
  .rid { font-size: 0.72rem; color: var(--tx-hi); flex: none; }
  .rzone {
    flex: 1;
    min-width: 0;
    font-size: 0.7rem;
    color: var(--tx-lo);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rseason {
    font-size: 0.58rem;
    font-weight: 700;
    color: var(--tx-dim);
    flex: none;
    width: 14px;
    text-align: center;
  }
  .rseason[data-season='winter'] { color: var(--info); }
  .rseason[data-season='spring'] { color: var(--ok); }
  .rseason[data-season='summer'] { color: var(--warn); }
  .rseason[data-season='autumn'] { color: var(--bad); }
  .rrewind { font-size: 0.72rem; color: var(--warn); flex: none; line-height: 1; cursor: help; }
  .rscore { font-size: 0.7rem; color: var(--tx-mid); flex: none; }
  .rinvalid { font-size: 0.62rem; color: var(--bad); flex: none; }
  .empty { font-size: 0.72rem; color: var(--tx-dim); padding: 1rem 0.5rem; text-align: center; }
</style>

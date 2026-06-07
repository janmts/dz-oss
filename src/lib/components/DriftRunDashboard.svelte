<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { displayPacket } from '$lib/stores/telemetry';
  import {
    driftRuns,
    driftRunStatus,
    driftZones,
    loadDriftRuns,
    loadDriftZones,
    setDriftRunManualScore,
    settings,
    startDriftRunStatusListener,
  } from '$lib/stores/sessions';
  import { carName } from '$lib/car-name';
  import { CAR_CLASS_LABELS, DRIVETRAIN_LABELS, type DriftRunRow, type DriftZoneRow } from '$lib/types';
  import DriftZoneMap from './DriftZoneMap.svelte';

  let { onClose }: { onClose: () => void } = $props();

  let selectedId = $state<number | null>(null);
  let scoreDraft = $state('');
  let savingScore = $state(false);
  let scoreStatus = $state('');
  let lastScoreRunId = $state<number | null>(null);
  let selectedRunId = $state<number | null>(null);
  let unsubscribeStatus: (() => void) | null = null;

  onMount(async () => {
    await Promise.all([loadDriftZones(), loadDriftRuns()]);
    unsubscribeStatus = await startDriftRunStatusListener();
  });

  onDestroy(() => {
    unsubscribeStatus?.();
  });

  let selectedZone = $derived.by(() => {
    if (selectedId !== null) return $driftZones.find((zone) => zone.id === selectedId) ?? null;
    return $driftZones.find((zone) => zone.active) ?? $driftZones[0] ?? null;
  });

  $effect(() => {
    if (selectedId === null && selectedZone) selectedId = selectedZone.id;
  });

  let selectedRuns = $derived.by(() => {
    if (!selectedZone) return [] as DriftRunRow[];
    return $driftRuns.filter((run) => run.zoneId === selectedZone.id);
  });

  let latestRun = $derived(selectedRuns[0] ?? null);
  let activeForSelected = $derived(
    $driftRunStatus.state === 'running' && selectedZone?.id === $driftRunStatus.zoneId
  );
  let scoringRun = $derived.by(() => {
    if (selectedRunId !== null) {
      const explicit = selectedRuns.find((run) => run.id === selectedRunId);
      if (explicit) return explicit;
    }
    if (
      ($driftRunStatus.state === 'completed' || $driftRunStatus.state === 'invalid') &&
      $driftRunStatus.runId &&
      $driftRunStatus.zoneId === selectedZone?.id
    ) {
      return $driftRuns.find((run) => run.id === $driftRunStatus.runId) ?? latestRun;
    }
    return latestRun;
  });

  $effect(() => {
    if (scoringRun?.id !== lastScoreRunId) {
      lastScoreRunId = scoringRun?.id ?? null;
      scoreDraft = scoringRun?.manualScore !== null && scoringRun?.manualScore !== undefined
        ? String(scoringRun.manualScore)
        : '';
      scoreStatus = '';
    }
  });

  function selectZone(zone: DriftZoneRow) {
    selectedId = zone.id;
    selectedRunId = null;
    scoreStatus = '';
  }

  function formatDate(ms: number | null) {
    return ms ? new Date(ms).toLocaleString() : '-';
  }

  function formatDuration(run: DriftRunRow | null) {
    if (!run?.endedAt) return '-';
    return `${Math.max(0, (run.endedAt - run.startedAt) / 1000).toFixed(1)}s`;
  }

  function formatStatus(run: DriftRunRow) {
    if (!run.endedAt) return 'Running';
    if (!run.valid) return 'Invalid';
    return 'Completed';
  }

  function statusLabel() {
    if ($driftRunStatus.state === 'running') return 'RUNNING';
    if ($driftRunStatus.state === 'completed') return 'COMPLETE';
    if ($driftRunStatus.state === 'invalid') return 'INVALID';
    return 'ARMED';
  }

  function zoneCompleteness(zone: DriftZoneRow) {
    return zone.leftBoundary.length >= 2 && zone.rightBoundary.length >= 2 ? 'Ready' : 'Incomplete';
  }

  async function saveScore() {
    if (!scoringRun) return;
    const parsed = Number.parseInt(scoreDraft.trim(), 10);
    if (!Number.isFinite(parsed)) {
      scoreStatus = 'Enter a whole-number score.';
      return;
    }
    savingScore = true;
    try {
      await setDriftRunManualScore(scoringRun.id, parsed, null);
      scoreStatus = 'Saved';
    } finally {
      savingScore = false;
    }
  }
</script>

<div class="overlay" role="dialog" aria-modal="true">
  <div class="dashboard">
    <aside class="zone-rail">
      <header class="rail-head">
        <div>
          <h2>Drift</h2>
          <span>{statusLabel()}</span>
        </div>
        <button class="close narrow" onclick={onClose}>x</button>
      </header>

      <div class="zone-list">
        {#each $driftZones as zone}
          <button
            class="zone-row"
            class:active={zone.id === selectedZone?.id}
            onclick={() => selectZone(zone)}
          >
            <span class="zone-name">{zone.name}</span>
            <span class="zone-meta">
              {zoneCompleteness(zone)} / {zone.active ? 'Active' : 'Inactive'}
            </span>
            <span class="zone-points">{zone.leftBoundary.length}L / {zone.rightBoundary.length}R</span>
          </button>
        {:else}
          <p class="empty">No zones saved yet.</p>
        {/each}
      </div>
    </aside>

    <main class="map-stage">
      <header class="stage-head">
        <div class="title-block">
          <h2>{selectedZone?.name ?? 'Drift Zone'}</h2>
          <div class="legend">
            <span><i class="left"></i>Left</span>
            <span><i class="right"></i>Right</span>
            <span><i class="start"></i>Start</span>
            <span><i class="finish"></i>Finish</span>
            <span><i class="split"></i>Split</span>
          </div>
        </div>
        <button class="close wide" onclick={onClose}>x</button>
      </header>

      <div class="map-shell">
        {#if $settings}
          <DriftZoneMap zone={selectedZone} settings={$settings} livePacket={$displayPacket} />
        {/if}
      </div>
    </main>

    <aside class="run-rail">
      <section class="run-card" class:live={activeForSelected} class:bad={$driftRunStatus.state === 'invalid'}>
        <div class="card-title">
          <span>Current Run</span>
          <strong>{activeForSelected ? 'LIVE' : statusLabel()}</strong>
        </div>
        <div class="metric-grid">
          <div>
            <span>Zone</span>
            <strong>{$driftRunStatus.zoneName ?? selectedZone?.name ?? '-'}</strong>
          </div>
          <div>
            <span>Packets</span>
            <strong>{$driftRunStatus.packetCount}</strong>
          </div>
          <div>
            <span>Started</span>
            <strong>{formatDate($driftRunStatus.startedAt)}</strong>
          </div>
          <div>
            <span>Ended</span>
            <strong>{formatDate($driftRunStatus.endedAt)}</strong>
          </div>
        </div>
        {#if $driftRunStatus.invalidReason}
          <p class="invalid">{$driftRunStatus.invalidReason}</p>
        {/if}
      </section>

      <section class="score-card">
        <div class="card-title">
          <span>Actual Score</span>
          <strong>{scoringRun ? `#${scoringRun.id}` : '-'}</strong>
        </div>
        <div class="score-input">
          <input
            inputmode="numeric"
            placeholder="Score"
            bind:value={scoreDraft}
            disabled={!scoringRun || savingScore}
            onkeydown={(e) => e.key === 'Enter' && saveScore()}
          />
          <button class="primary" disabled={!scoringRun || savingScore} onclick={saveScore}>
            {savingScore ? 'Saving' : 'Save'}
          </button>
        </div>
        <p class="score-status">{scoreStatus || (scoringRun ? formatDate(scoringRun.startedAt) : 'No completed runs')}</p>
      </section>

      <section class="history">
        <div class="card-title">
          <span>Recent Scores</span>
          <strong>{selectedRuns.length}</strong>
        </div>
        <div class="run-list">
          {#each selectedRuns as run}
            <button
              class="run-row"
              class:active={run.id === scoringRun?.id}
              onclick={() => {
                selectedRunId = run.id;
              }}
            >
              <div>
                <strong>{run.manualScore ?? run.computedScore ?? '-'}</strong>
                <span>{formatStatus(run)} / {formatDuration(run)}</span>
              </div>
              <div>
                <span>{carName(run.carOrdinal)}</span>
                <small>{CAR_CLASS_LABELS[run.carClass] ?? '?'} {run.carPi} / {DRIVETRAIN_LABELS[run.drivetrainType] ?? '?'}</small>
              </div>
            </button>
          {:else}
            <p class="empty">No runs for this zone.</p>
          {/each}
        </div>
      </section>
    </aside>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.76);
    z-index: 120;
    padding: 1.5rem;
  }
  .dashboard {
    width: min(1360px, 100%);
    height: min(900px, 100%);
    margin: 0 auto;
    display: grid;
    grid-template-columns: 260px minmax(420px, 1fr) 320px;
    background: var(--bg-panel);
    border: 1px solid var(--bd-muted);
    border-radius: 10px;
    overflow: hidden;
    box-shadow: 0 20px 80px rgba(0, 0, 0, 0.55);
  }
  .zone-rail,
  .run-rail {
    min-height: 0;
    background: var(--bg-card);
    display: flex;
    flex-direction: column;
  }
  .zone-rail {
    border-right: 1px solid var(--bd-dim);
  }
  .run-rail {
    border-left: 1px solid var(--bd-dim);
    padding: 0.75rem;
    gap: 0.75rem;
  }
  .rail-head,
  .stage-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.9rem 1rem;
    border-bottom: 1px solid var(--bd-dim);
  }
  .rail-head h2,
  .stage-head h2 {
    margin: 0;
    color: var(--tx-hi);
    font-size: 1rem;
  }
  .rail-head span {
    color: var(--ac);
    font-size: 0.62rem;
    font-weight: 800;
    letter-spacing: 0.12em;
  }
  .close {
    background: none;
    border: 1px solid var(--bd-muted);
    border-radius: 4px;
    color: var(--tx-dim);
    cursor: pointer;
    line-height: 1;
    padding: 0.25rem 0.45rem;
  }
  .close:hover {
    color: var(--tx-hi);
    border-color: var(--bd-strong);
  }
  .close.narrow {
    display: none;
  }
  .zone-list {
    min-height: 0;
    overflow-y: auto;
    padding: 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  .zone-row,
  .run-row {
    text-align: left;
    border: 1px solid var(--bd-dim);
    background: var(--bg-panel);
    color: var(--tx-mid);
    border-radius: 6px;
    cursor: pointer;
  }
  .zone-row {
    padding: 0.6rem;
    display: grid;
    gap: 0.18rem;
  }
  .zone-row.active,
  .run-row.active {
    border-color: var(--ac);
  }
  .zone-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--tx-mid);
    font-size: 0.84rem;
    font-weight: 700;
  }
  .zone-meta,
  .zone-points,
  .empty,
  .score-status,
  .run-row span,
  .run-row small {
    color: var(--tx-dim);
    font-size: 0.68rem;
  }
  .map-stage {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
  }
  .stage-head {
    padding: 0.75rem 1rem;
  }
  .title-block {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 0.55rem;
    color: var(--tx-dim);
    font-size: 0.68rem;
  }
  .legend span {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
  }
  .legend i {
    width: 16px;
    height: 3px;
    border-radius: 2px;
    display: inline-block;
  }
  .legend .left { background: #22c55e; }
  .legend .right { background: #3b82f6; }
  .legend .start { background: #f59e0b; }
  .legend .finish { background: #ef4444; }
  .legend .split { background: #a855f7; }
  .map-shell {
    flex: 1;
    min-height: 0;
    padding: 0.8rem;
  }
  .run-card,
  .score-card,
  .history {
    background: var(--bg-panel);
    border: 1px solid var(--bd-dim);
    border-radius: 8px;
  }
  .run-card.live {
    border-color: #22c55e;
  }
  .run-card.bad {
    border-color: #ef4444;
  }
  .card-title {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.65rem 0.75rem;
    border-bottom: 1px solid var(--bd-dim);
  }
  .card-title span {
    color: var(--tx-dim);
    font-size: 0.62rem;
    font-weight: 800;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .card-title strong {
    color: var(--tx-hi);
    font-size: 0.72rem;
    font-variant-numeric: tabular-nums;
  }
  .metric-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem;
    padding: 0.75rem;
  }
  .metric-grid div {
    min-width: 0;
    display: grid;
    gap: 0.2rem;
  }
  .metric-grid span {
    color: var(--tx-dim);
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.07em;
  }
  .metric-grid strong {
    color: var(--tx-mid);
    font-size: 0.78rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .invalid {
    color: #fca5a5;
    font-size: 0.75rem;
    padding: 0 0.75rem 0.75rem;
  }
  .score-card {
    padding-bottom: 0.65rem;
  }
  .score-input {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0.5rem;
    padding: 0.75rem 0.75rem 0.45rem;
  }
  input {
    min-width: 0;
    background: var(--bg-body);
    border: 1px solid var(--bd-muted);
    border-radius: 5px;
    color: var(--tx-hi);
    padding: 0.45rem 0.55rem;
    font: inherit;
  }
  button.primary {
    background: var(--ac);
    border: 1px solid var(--ac);
    border-radius: 5px;
    color: var(--bg-body);
    cursor: pointer;
    font-size: 0.78rem;
    font-weight: 700;
    padding: 0.45rem 0.7rem;
  }
  button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .score-status {
    padding: 0 0.75rem;
  }
  .history {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .run-list {
    min-height: 0;
    overflow-y: auto;
    padding: 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }
  .run-row {
    padding: 0.55rem;
    display: grid;
    gap: 0.35rem;
  }
  .run-row div {
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  .run-row strong {
    color: var(--tx-hi);
    font-size: 0.95rem;
    font-variant-numeric: tabular-nums;
  }
  .empty {
    padding: 1rem;
    text-align: center;
  }

  @media (max-width: 980px) {
    .overlay {
      padding: 0.75rem;
    }
    .dashboard {
      grid-template-columns: 1fr;
      grid-template-rows: auto minmax(360px, 1fr) minmax(260px, 0.8fr);
      overflow-y: auto;
    }
    .zone-rail {
      border-right: 0;
      border-bottom: 1px solid var(--bd-dim);
      max-height: 190px;
    }
    .run-rail {
      border-left: 0;
      border-top: 1px solid var(--bd-dim);
      min-height: 320px;
    }
    .close.narrow {
      display: inline-block;
    }
    .close.wide {
      display: none;
    }
    .zone-list {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
    }
  }
</style>

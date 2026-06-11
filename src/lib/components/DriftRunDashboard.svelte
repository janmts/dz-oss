<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { displayPacket } from '$lib/stores/telemetry';
  import {
    driftRuns,
    driftRunStatus,
    driftZones,
    deleteDriftRun,
    deleteInvalidDriftRuns,
    loadDriftRuns,
    loadDriftZones,
    recomputeDriftScores,
    setDriftRunManualScore,
    settings,
    startDriftRunStatusListener,
  } from '$lib/stores/sessions';
  import { carName } from '$lib/car-name';
  import { CAR_CLASS_LABELS, DRIVETRAIN_LABELS, type DriftRunRow, type DriftZoneRow } from '$lib/types';
  import DriftZoneMap from './DriftZoneMap.svelte';

  let selectedId = $state<number | null>(null);
  let scoreDraft = $state('');
  let noteDraft = $state('');
  let savingScore = $state(false);
  let scoreStatus = $state('');
  let lastScoreRunId = $state<number | null>(null);
  let selectedRunId = $state<number | null>(null);
  let autoSelectedRunId = $state<number | null>(null);
  let recomputing = $state(false);
  let recomputeStatus = $state('');
  let purging = $state(false);

  // Invalid runs across every zone (the purge command is global, matching this).
  let invalidCount = $derived($driftRuns.filter((run) => !run.valid).length);
  let scoreInput: HTMLInputElement | null = null;
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
  // Death-timer counting: live run that isn't currently earning points.
  let liveStarving = $derived(activeForSelected && !$driftRunStatus.scoring);
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
    const runId = $driftRunStatus.runId;
    if ($driftRunStatus.state !== 'completed' || runId === null || runId === autoSelectedRunId) return;

    autoSelectedRunId = runId;
    selectedRunId = runId;
    if ($driftRunStatus.zoneId !== null) selectedId = $driftRunStatus.zoneId;
  });

  $effect(() => {
    if (scoringRun?.id !== lastScoreRunId) {
      lastScoreRunId = scoringRun?.id ?? null;
      scoreDraft = scoringRun?.manualScore !== null && scoringRun?.manualScore !== undefined
        ? String(scoringRun.manualScore)
        : '';
      noteDraft = scoringRun?.manualNotes ?? '';
      scoreStatus = '';
      if (scoringRun?.id === autoSelectedRunId) queueMicrotask(() => scoreInput?.focus());
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

  function formatShort(ms: number | null) {
    return ms
      ? new Date(ms).toLocaleString(undefined, {
          month: 'short',
          day: 'numeric',
          hour: '2-digit',
          minute: '2-digit',
        })
      : '-';
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

  function formatScore(n: number | null | undefined) {
    return n === null || n === undefined ? '-' : Math.round(n).toLocaleString();
  }

  // Computed-vs-actual gap for the selected run — the signal that drives tuning.
  let scoreDelta = $derived.by(() => {
    const c = scoringRun?.computedScore;
    const m = scoringRun?.manualScore;
    if (c === null || c === undefined || m === null || m === undefined) return null;
    const diff = Math.round(c) - m;
    const pct = m !== 0 ? (diff / m) * 100 : 0;
    return { diff, pct };
  });

  async function saveScore() {
    if (!scoringRun) return;
    const parsed = Number.parseInt(scoreDraft.trim(), 10);
    if (!Number.isFinite(parsed)) {
      scoreStatus = 'Enter a whole-number score.';
      return;
    }
    savingScore = true;
    try {
      await setDriftRunManualScore(scoringRun.id, parsed, noteDraft.trim() || null);
      scoreStatus = 'Saved';
    } finally {
      savingScore = false;
    }
  }

  async function recompute() {
    recomputing = true;
    recomputeStatus = '';
    try {
      const n = await recomputeDriftScores();
      recomputeStatus = `Rescored ${n} run${n === 1 ? '' : 's'}`;
    } catch (e) {
      recomputeStatus = `Failed: ${e}`;
    } finally {
      recomputing = false;
    }
  }

  // Permanently drop a single run (and its telemetry) — for OOB noise or a run
  // where the wrong actual score was typed in. Irreversible, so confirm first.
  async function removeRun(run: DriftRunRow) {
    const tag = run.valid ? '' : ' (invalid)';
    if (
      !confirm(
        `Delete run #${run.zoneRunNumber}${tag} (global #${run.id}) — ` +
          `computed ${formatScore(run.computedScore)}? ` +
          'This permanently removes its telemetry and cannot be undone.'
      )
    )
      return;
    if (selectedRunId === run.id) selectedRunId = null;
    if (autoSelectedRunId === run.id) autoSelectedRunId = null;
    await deleteDriftRun(run.id);
  }

  // Bulk-purge every invalid run across all zones in one go.
  async function purgeInvalid() {
    const n = invalidCount;
    if (n === 0) return;
    if (
      !confirm(
        `Delete ALL ${n} invalid run${n === 1 ? '' : 's'} across every zone? ` +
          'This permanently removes their telemetry and cannot be undone.'
      )
    )
      return;
    purging = true;
    try {
      const removed = await deleteInvalidDriftRuns();
      recomputeStatus = `Purged ${removed} invalid run${removed === 1 ? '' : 's'}`;
    } catch (e) {
      recomputeStatus = `Failed: ${e}`;
    } finally {
      purging = false;
    }
  }
</script>

<div class="dashboard">
  <aside class="zone-rail">
    <header class="rail-head">
      <span class="cap">Zones</span>
      <span class="state mono" data-state={$driftRunStatus.state}>{statusLabel()}</span>
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
            {zoneCompleteness(zone)} · {zone.active ? 'Active' : 'Inactive'}
          </span>
          <span class="zone-points mono">{zone.leftBoundary.length}L / {zone.rightBoundary.length}R</span>
        </button>
      {:else}
        <p class="empty">No zones saved yet.</p>
      {/each}
    </div>
  </aside>

  <main class="map-stage">
    <header class="stage-head">
      <h2>{selectedZone?.name ?? 'Drift Zone'}</h2>
      <div class="legend">
        <span><i class="left"></i>Left</span>
        <span><i class="right"></i>Right</span>
        <span><i class="start"></i>Gate A</span>
        <span><i class="finish"></i>Gate B</span>
        <span><i class="split"></i>Split</span>
      </div>
    </header>

    <div class="map-shell">
      {#if $settings}
        <DriftZoneMap zone={selectedZone} settings={$settings} livePacket={$displayPacket} />
      {/if}
    </div>
  </main>

  <aside class="run-rail">
    <section
      class="run-card"
      class:live={activeForSelected}
      class:starving={liveStarving}
      class:bad={$driftRunStatus.state === 'invalid'}
    >
      <div class="card-title">
        <span class="cap">Current Run</span>
        <strong class="mono" class:warn={liveStarving}>
          {#if liveStarving}
            NO SCORE{#if $driftRunStatus.starveRemainingS != null} · {$driftRunStatus.starveRemainingS.toFixed(1)}s{/if}
          {:else if activeForSelected}
            LIVE
          {:else}
            {statusLabel()}
          {/if}
        </strong>
      </div>
      {#if activeForSelected}
        <!-- Live play-test instruments: signed drift angle, speed in the
             model's native unit, and the run's flip count (same definition as
             the stored directionFlips breakdown / the tuning scripts). -->
        <div class="live-instruments">
          <div class="inst">
            <span class="cap">Angle</span>
            <strong class="big mono"
              >{$driftRunStatus.angleDeg != null
                ? `${$driftRunStatus.angleDeg >= 0 ? '+' : ''}${$driftRunStatus.angleDeg.toFixed(1)}°`
                : '–'}</strong
            >
          </div>
          <div class="inst">
            <span class="cap">Speed</span>
            <strong class="big mono"
              >{$driftRunStatus.speedMs != null ? `${$driftRunStatus.speedMs.toFixed(1)} m/s` : '–'}</strong
            >
          </div>
          <div class="inst">
            <span class="cap">Flips</span>
            <strong class="big mono">{$driftRunStatus.directionFlips ?? 0}</strong>
          </div>
        </div>
      {/if}
      <div class="metric-grid">
        <div>
          <span class="cap">Zone</span>
          <strong>{$driftRunStatus.zoneName ?? selectedZone?.name ?? '-'}</strong>
        </div>
        <div>
          <span class="cap">Packets</span>
          <strong class="mono">{$driftRunStatus.packetCount}</strong>
        </div>
        <div>
          <span class="cap">Started</span>
          <strong class="mono">{formatDate($driftRunStatus.startedAt)}</strong>
        </div>
        <div>
          <span class="cap">Ended</span>
          <strong class="mono">{formatDate($driftRunStatus.endedAt)}</strong>
        </div>
      </div>
      {#if liveStarving}
        <p class="starving-msg">
          ⚠ Not scoring — no tyre on tarmac / not drifting{#if $driftRunStatus.starveRemainingS != null} · run ends in {$driftRunStatus.starveRemainingS.toFixed(1)}s{/if}
        </p>
      {/if}
      {#if $driftRunStatus.invalidReason}
        <p class="invalid">{$driftRunStatus.invalidReason}</p>
      {/if}
    </section>

    <section class="score-card">
      <div class="card-title">
        <span class="cap">Score {scoringRun ? `#${scoringRun.zoneRunNumber}` : ''}</span>
        <button class="ghost" disabled={recomputing} onclick={recompute} title="Re-score all runs from saved telemetry">
          {recomputing ? 'Rescoring…' : '↻ Recompute'}
        </button>
      </div>
      <div class="score-readout">
        <div class="computed">
          <span class="cap">Computed</span>
          <strong class="mono">{formatScore(scoringRun?.computedScore)}</strong>
        </div>
        {#if scoreDelta}
          <div class="delta" class:over={scoreDelta.diff > 0} class:under={scoreDelta.diff < 0}>
            <span class="cap">vs actual</span>
            <strong class="mono">{scoreDelta.diff > 0 ? '+' : ''}{scoreDelta.diff.toLocaleString()} ({scoreDelta.pct.toFixed(0)}%)</strong>
          </div>
        {/if}
      </div>
      {#if scoringRun?.scoreBreakdown}
        {@const b = scoringRun.scoreBreakdown}
        <div class="breakdown mono">
          <span>drift {b.driftTimeS.toFixed(1)}s / {b.totalTimeS.toFixed(1)}s</span>
          <span>angle {b.avgAngleDeg.toFixed(0)}° avg · {b.maxAngleDeg.toFixed(0)}° max</span>
          <span>speed {b.avgSpeedMs.toFixed(1)} m/s</span>
          {#if b.directionFlips !== undefined}
            <span>flips {b.directionFlips}</span>
          {/if}
          {#if b.maxMultiplier > 1.01}
            <span>mult ×{b.maxMultiplier.toFixed(1)}</span>
          {/if}
        </div>
      {/if}
      {#if scoringRun}
        <p class="recorded mono">Recorded {formatDate(scoringRun.startedAt)} · global #{scoringRun.id}</p>
      {/if}
      <label class="actual-label cap" for="actual-score">Actual (in-game)</label>
      <div class="score-input">
        <input
          id="actual-score"
          class="mono"
          bind:this={scoreInput}
          inputmode="numeric"
          placeholder="Enter in-game score"
          bind:value={scoreDraft}
          disabled={!scoringRun || savingScore}
          onkeydown={(e) => e.key === 'Enter' && saveScore()}
        />
        <button class="primary" disabled={!scoringRun || savingScore} onclick={saveScore}>
          {savingScore ? 'Saving' : 'Save'}
        </button>
      </div>
      <div class="note-row">
        <input
          class="note-input"
          placeholder="Note — car / style / tag (saved with score)"
          bind:value={noteDraft}
          disabled={!scoringRun || savingScore}
          onkeydown={(e) => e.key === 'Enter' && saveScore()}
        />
      </div>
      <p class="score-status">{scoreStatus || recomputeStatus || (scoringRun ? '' : 'No completed runs')}</p>
    </section>

    <section class="history">
      <div class="card-title">
        <span class="cap">Recent Runs · newest first</span>
        <div class="title-actions">
          {#if invalidCount > 0}
            <button
              class="ghost danger"
              disabled={purging}
              onclick={purgeInvalid}
              title="Permanently delete every invalid run across all zones"
            >
              {purging ? 'Purging…' : `Purge ${invalidCount} invalid`}
            </button>
          {/if}
          <strong class="mono">{selectedRuns.length}</strong>
        </div>
      </div>
      <div class="run-list">
        {#each selectedRuns as run}
          <div class="run-row-wrap" class:invalid={!run.valid}>
            <button
              class="run-row"
              class:active={run.id === scoringRun?.id}
              onclick={() => {
                selectedRunId = run.id;
              }}
            >
              <div>
                <strong class="mono">{formatScore(run.computedScore)}</strong>
                <span class="when mono">#{run.zoneRunNumber} · {formatShort(run.startedAt)}</span>
              </div>
              <div>
                <span class="mono">act {run.manualScore?.toLocaleString() ?? '—'} · {formatStatus(run)} / {formatDuration(run)}</span>
              </div>
              <div>
                <span>{carName(run.carOrdinal)}</span>
                <small>{CAR_CLASS_LABELS[run.carClass] ?? '?'} {run.carPi} / {DRIVETRAIN_LABELS[run.drivetrainType] ?? '?'}</small>
              </div>
              {#if run.manualNotes}
                <div class="note-line" title={run.manualNotes}>{run.manualNotes}</div>
              {/if}
            </button>
            <button
              class="run-del"
              aria-label="Delete run #{run.zoneRunNumber}"
              title="Delete this run permanently"
              onclick={() => removeRun(run)}
            >×</button>
          </div>
        {:else}
          <p class="empty">No runs for this zone.</p>
        {/each}
      </div>
    </section>
  </aside>
</div>

<style>
  .dashboard {
    min-height: 0;
    display: grid;
    grid-template-columns: 240px minmax(420px, 1fr) 330px;
    background: var(--bg-body);
    overflow: hidden;
  }
  .zone-rail,
  .run-rail {
    min-height: 0;
    background: var(--bg-panel);
    display: flex;
    flex-direction: column;
  }
  .zone-rail {
    border-right: 1px solid var(--bd-subtle);
  }
  .run-rail {
    border-left: 1px solid var(--bd-subtle);
    padding: 0.7rem;
    gap: 0.7rem;
  }
  .rail-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.75rem 0.9rem;
    border-bottom: 1px solid var(--bd-dim);
  }
  .state {
    font-size: 0.62rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    color: var(--tx-dim);
  }
  .state[data-state="running"]   { color: var(--ok); }
  .state[data-state="completed"] { color: var(--ac); }
  .state[data-state="invalid"]   { color: var(--bad); }

  .zone-list {
    min-height: 0;
    overflow-y: auto;
    padding: 0.55rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .zone-row,
  .run-row {
    text-align: left;
    border: 1px solid var(--bd-dim);
    background: var(--bg-card);
    color: var(--tx-mid);
    border-radius: var(--r-md);
    cursor: pointer;
    font-family: inherit;
  }
  .zone-row {
    padding: 0.55rem 0.6rem;
    display: grid;
    gap: 0.16rem;
  }
  .zone-row:hover,
  .run-row:hover {
    border-color: var(--bd-muted);
  }
  .zone-row.active,
  .run-row.active {
    border-color: color-mix(in srgb, var(--ac) 55%, var(--bg-panel));
    box-shadow: inset 2px 0 0 var(--ac);
  }
  .zone-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--tx-mid);
    font-size: 0.8rem;
    font-weight: 600;
  }
  .zone-meta,
  .zone-points,
  .empty,
  .score-status {
    color: var(--tx-dim);
    font-size: 0.66rem;
  }
  .run-row span,
  .run-row small {
    color: var(--tx-lo);
    font-size: 0.7rem;
  }
  .map-stage {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-body);
  }
  .stage-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--bd-dim);
  }
  .stage-head h2 {
    margin: 0;
    color: var(--tx-hi);
    font-size: 0.92rem;
    font-weight: 650;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 0.6rem;
    color: var(--tx-dim);
    font-size: 0.64rem;
    flex-shrink: 0;
  }
  .legend span {
    display: inline-flex;
    align-items: center;
    gap: 0.28rem;
  }
  .legend i {
    width: 14px;
    height: 2px;
    border-radius: 1px;
    display: inline-block;
  }
  .legend .left   { background: var(--map-left); }
  .legend .right  { background: var(--map-right); }
  .legend .start  { background: var(--gate-a); }
  .legend .finish { background: var(--gate-b); }
  .legend .split  { background: var(--gate-split); }
  .map-shell {
    flex: 1;
    min-height: 0;
    padding: 0.7rem;
    /* Contain leaflet's internal z-indexes (panes go up to ~1000) so they
       can't stack above app-level overlays like the settings modal. */
    isolation: isolate;
  }
  .run-card,
  .score-card,
  .history {
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-md);
  }
  .run-card.live {
    border-color: color-mix(in srgb, var(--ok) 60%, var(--bg-panel));
  }
  /* Death-timer counting (live but not scoring) — overrides the green .live. */
  .run-card.starving {
    border-color: var(--warn);
    box-shadow: 0 0 0 1px var(--warn) inset;
  }
  .run-card.bad {
    border-color: color-mix(in srgb, var(--bad) 60%, var(--bg-panel));
  }
  .card-title strong.warn {
    color: var(--warn);
  }
  .starving-msg {
    color: var(--warn);
    font-size: 0.7rem;
    padding: 0 0.7rem 0.7rem;
  }
  .card-title {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.55rem 0.7rem;
    border-bottom: 1px solid var(--bd-dim);
  }
  .card-title strong {
    color: var(--tx-hi);
    font-size: 0.68rem;
    font-weight: 600;
    letter-spacing: 0.06em;
  }
  .live-instruments {
    display: grid;
    grid-template-columns: 1.2fr 1.2fr 0.8fr;
    gap: 0.5rem;
    padding: 0.6rem 0.7rem 0;
  }
  .live-instruments .inst {
    min-width: 0;
    display: grid;
    gap: 0.12rem;
  }
  .live-instruments strong.big {
    color: var(--tx-hi);
    font-size: 1.3rem;
    font-weight: 600;
    white-space: nowrap;
  }
  .metric-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem;
    padding: 0.7rem;
  }
  .metric-grid div {
    min-width: 0;
    display: grid;
    gap: 0.18rem;
  }
  .metric-grid strong {
    color: var(--tx-mid);
    font-size: 0.74rem;
    font-weight: 550;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .invalid {
    color: var(--bad-tx);
    font-size: 0.72rem;
    padding: 0 0.7rem 0.7rem;
  }
  .score-card {
    padding-bottom: 0.6rem;
  }
  button.ghost {
    background: none;
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-sm);
    color: var(--tx-dim);
    cursor: pointer;
    font-family: inherit;
    font-size: 0.58rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    padding: 0.2rem 0.4rem;
    text-transform: uppercase;
  }
  button.ghost:hover:not(:disabled) {
    color: var(--tx-hi);
    border-color: var(--bd-strong);
  }
  button.ghost.danger:hover:not(:disabled) {
    color: var(--bad-tx);
    border-color: var(--bad);
  }
  .title-actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .score-readout {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.6rem 0.7rem 0.35rem;
  }
  .score-readout .computed {
    display: grid;
    gap: 0.12rem;
  }
  .score-readout .computed strong {
    color: var(--tx-hi);
    font-size: 1.55rem;
    font-weight: 650;
    line-height: 1;
  }
  .score-readout .delta {
    display: grid;
    gap: 0.12rem;
    text-align: right;
  }
  .score-readout .delta strong {
    font-size: 0.82rem;
    color: var(--tx-mid);
  }
  .score-readout .delta.over strong {
    color: var(--warn);
  }
  .score-readout .delta.under strong {
    color: var(--info);
  }
  .breakdown {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem 0.7rem;
    padding: 0 0.7rem 0.5rem;
    color: var(--tx-lo);
    font-size: 0.71rem;
  }
  .recorded {
    padding: 0.15rem 0.7rem 0;
    color: var(--tx-lo);
    font-size: 0.67rem;
  }
  .actual-label {
    display: block;
    padding: 0.4rem 0.7rem 0;
  }
  .score-input {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0.5rem;
    padding: 0.4rem 0.7rem 0.45rem;
  }
  input {
    min-width: 0;
    background: var(--bg-body);
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-sm);
    color: var(--tx-hi);
    padding: 0.42rem 0.55rem;
    font-family: inherit;
    font-size: 0.78rem;
  }
  input:focus {
    outline: none;
    border-color: color-mix(in srgb, var(--ac) 60%, var(--bg-panel));
  }
  button.primary {
    background: var(--ac);
    border: 1px solid var(--ac);
    border-radius: var(--r-sm);
    color: var(--bg-body);
    cursor: pointer;
    font-family: inherit;
    font-size: 0.74rem;
    font-weight: 650;
    padding: 0.42rem 0.7rem;
  }
  button.primary:hover:not(:disabled) {
    background: var(--ac-bright);
    border-color: var(--ac-bright);
  }
  button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .note-row {
    padding: 0 0.7rem 0.3rem;
  }
  .note-input {
    width: 100%;
    font-size: 0.74rem;
  }
  .score-status {
    padding: 0 0.7rem;
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
    gap: 0.3rem;
  }
  .run-row {
    padding: 0.5rem 0.55rem;
    display: grid;
    gap: 0.3rem;
  }
  .run-row-wrap {
    display: flex;
    align-items: stretch;
    gap: 0.3rem;
  }
  .run-row-wrap .run-row {
    flex: 1;
    min-width: 0;
  }
  .run-row-wrap.invalid .run-row strong {
    color: var(--tx-dim);
  }
  .run-del {
    flex: 0 0 auto;
    width: 1.8rem;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--bd-dim);
    background: var(--bg-card);
    border-radius: var(--r-md);
    color: var(--tx-dim);
    cursor: pointer;
    font-size: 0.95rem;
    line-height: 1;
  }
  .run-del:hover {
    color: var(--bad-tx);
    border-color: var(--bad);
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
    font-size: 0.9rem;
    font-weight: 600;
  }
  .run-row .note-line {
    display: block;
    color: var(--ac);
    font-size: 0.64rem;
    font-style: italic;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    padding: 1rem;
    text-align: center;
  }

  @media (max-width: 980px) {
    .dashboard {
      grid-template-columns: 1fr;
      grid-template-rows: auto minmax(360px, 1fr) minmax(260px, 0.8fr);
      overflow-y: auto;
    }
    .zone-rail {
      border-right: 0;
      border-bottom: 1px solid var(--bd-subtle);
      max-height: 190px;
    }
    .run-rail {
      border-left: 0;
      border-top: 1px solid var(--bd-subtle);
      min-height: 320px;
    }
    .zone-list {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
    }
  }
</style>

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
  let zoneQuery = $state('');

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

  let visibleZones = $derived.by(() => {
    const q = zoneQuery.trim().toLowerCase();
    if (!q) return $driftZones;
    return $driftZones.filter((zone) => zone.name.toLowerCase().includes(q));
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

  // Current-run cells stack time over date so neither truncates in the rail.
  function formatTimePart(ms: number | null) {
    return ms ? new Date(ms).toLocaleTimeString() : '-';
  }

  function formatDatePart(ms: number | null) {
    return ms ? new Date(ms).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) : '';
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

  function zoneReady(zone: DriftZoneRow) {
    return zone.leftBoundary.length >= 2 && zone.rightBoundary.length >= 2;
  }

  function formatScore(n: number | null | undefined) {
    return n === null || n === undefined ? '-' : Math.round(n).toLocaleString();
  }

  // ── Drift-angle gauge geometry ────────────────────────────────────────────
  // Semicircle spanning −90°…+90° of signed drift angle. The dead band below
  // ±10° mirrors the scoring model's min_angle; the accent ticks at ±45° mark
  // the sweet-spot anchor — useful for holding target angles in play-tests.
  const G = { cx: 110, cy: 104, r: 84 };
  const GAUGE_TICKS = Array.from({ length: 13 }, (_, i) => -90 + i * 15);
  const GAUGE_LABELS = [-90, -45, 0, 45, 90];

  function gaugeXY(deg: number, radius: number): [number, number] {
    const t = ((90 - deg) * Math.PI) / 180;
    return [G.cx + radius * Math.cos(t), G.cy - radius * Math.sin(t)];
  }

  function arcPath(from: number, to: number, radius: number): string {
    const [x1, y1] = gaugeXY(from, radius);
    const [x2, y2] = gaugeXY(to, radius);
    return `M ${x1.toFixed(1)} ${y1.toFixed(1)} A ${radius} ${radius} 0 0 1 ${x2.toFixed(1)} ${y2.toFixed(1)}`;
  }

  // Below this speed (m/s) the car is effectively stationary and the velocity
  // vector is numerically meaningless: the backend's signed angle is
  // `atan2(velX, velZ)`, which degenerates into noise as v→0 and jitters across
  // the whole range — flailing the needle. Matches the ~0.5 m/s atan2 floor the
  // scoring model keeps `min_speed_ms` above (see scoring.rs). Below it we treat
  // the live angle as indeterminate: park the needle at centre and show "—"
  // rather than tracking noise. (Clamping alone can't help — the noise already
  // spans ±90°.)
  const ANGLE_NOISE_FLOOR_MS = 0.5;

  let angleIndeterminate = $derived(
    activeForSelected && ($driftRunStatus.speedMs ?? 0) < ANGLE_NOISE_FLOOR_MS
  );
  let liveAngle = $derived(
    activeForSelected && !angleIndeterminate ? $driftRunStatus.angleDeg : null
  );
  let needleDeg = $derived(Math.max(-90, Math.min(90, liveAngle ?? 0)));

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
  <aside class="left-rail">
    <section class="zone-box">
      <div class="card-title">
        <span class="cap">Zones</span>
        <strong class="mono">{zoneQuery.trim() ? `${visibleZones.length} / ${$driftZones.length}` : $driftZones.length}</strong>
      </div>
      <div class="zone-search-row">
        <input
          class="zone-search"
          placeholder="Filter zones…"
          bind:value={zoneQuery}
        />
      </div>
      <div class="zone-list">
        {#each visibleZones as zone (zone.id)}
          <button
            class="zone-row"
            class:active={zone.id === selectedZone?.id}
            onclick={() => selectZone(zone)}
          >
            <span class="zone-name">{zone.name}</span>
            {#if zone.active}
              <span class="zone-armed" title="Active — armed for auto-detection"></span>
            {/if}
            {#if zoneReady(zone)}
              <svg
                class="zone-ready"
                viewBox="0 0 24 24"
                width="12"
                height="12"
                fill="none"
                stroke="currentColor"
                stroke-width="3"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-label="Ready"
              ><path d="M20 6 9 17l-5-5" /></svg>
            {:else}
              <span class="zone-incomplete" title="Incomplete — needs ≥2 points per side">!</span>
            {/if}
          </button>
        {:else}
          <p class="empty">{$driftZones.length ? 'No zones match.' : 'No zones saved yet.'}</p>
        {/each}
      </div>
    </section>

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

      <!-- Live play-test instruments: signed drift-angle needle, speed in the
           model's native unit, and the run's flip count (same definition as
           the stored directionFlips breakdown / the tuning scripts). -->
      <div class="gauge-wrap" class:idle={!activeForSelected} class:stalled={angleIndeterminate}>
        <svg class="gauge" viewBox="0 0 220 150" aria-label="Signed drift angle">
          <path class="g-track" d={arcPath(-90, 90, G.r)} />
          <path class="g-dead" d={arcPath(-10, 10, G.r)} />
          {#each GAUGE_TICKS as t}
            {@const major = t % 45 === 0}
            {@const [x1, y1] = gaugeXY(t, G.r)}
            {@const [x2, y2] = gaugeXY(t, major ? G.r - 10 : G.r - 6)}
            <line
              class="g-tick"
              class:major
              class:sweet={Math.abs(t) === 45}
              x1={x1} y1={y1} x2={x2} y2={y2}
            />
          {/each}
          {#each GAUGE_LABELS as t}
            {@const [x, y] = gaugeXY(t, G.r - 20)}
            <text class="g-label mono" {x} y={y + 3} text-anchor="middle">{Math.abs(t)}</text>
          {/each}
          <line
            class="needle"
            x1={G.cx} y1={G.cy}
            x2={G.cx} y2={G.cy - (G.r - 14)}
            transform="rotate({needleDeg} {G.cx} {G.cy})"
            style="transition: transform 60ms linear;"
          />
          <circle class="hub" cx={G.cx} cy={G.cy} r="4.5" />
          <text class="g-val mono" x={G.cx} y="134" text-anchor="middle">
            {liveAngle != null ? `${liveAngle >= 0 ? '+' : ''}${liveAngle.toFixed(1)}°` : '—'}
          </text>
          <text class="g-cap" x={G.cx} y="147" text-anchor="middle">DRIFT ANGLE</text>
        </svg>
      </div>

      <div class="live-instruments">
        <div class="inst">
          <span class="cap">Speed</span>
          <strong class="big mono"
            >{activeForSelected && $driftRunStatus.speedMs != null
              ? `${$driftRunStatus.speedMs.toFixed(1)} m/s`
              : '–'}</strong
          >
        </div>
        <div class="inst">
          <span class="cap">Flips</span>
          <strong class="big mono">{activeForSelected ? ($driftRunStatus.directionFlips ?? 0) : '–'}</strong>
        </div>
      </div>

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
          <strong class="mono">{formatTimePart($driftRunStatus.startedAt)}</strong>
          {#if $driftRunStatus.startedAt}
            <small class="date-sub mono">{formatDatePart($driftRunStatus.startedAt)}</small>
          {/if}
        </div>
        <div>
          <span class="cap">Ended</span>
          <strong class="mono">{formatTimePart($driftRunStatus.endedAt)}</strong>
          {#if $driftRunStatus.endedAt}
            <small class="date-sub mono">{formatDatePart($driftRunStatus.endedAt)}</small>
          {/if}
        </div>
      </div>
      {#if liveStarving || $driftRunStatus.invalidReason}
        <div class="run-foot">
          {#if liveStarving}
            <p class="starving-msg">
              ⚠ Not scoring — no tyre on tarmac / not drifting{#if $driftRunStatus.starveRemainingS != null} · run ends in {$driftRunStatus.starveRemainingS.toFixed(1)}s{/if}
            </p>
          {/if}
          {#if $driftRunStatus.invalidReason}
            <p class="invalid-msg">{$driftRunStatus.invalidReason}</p>
          {/if}
        </div>
      {/if}
    </section>
  </aside>

  <main class="map-stage">
    <header class="stage-head">
      <h2>{selectedZone?.name ?? 'Drift Zone'}</h2>
      <div class="legend">
        <span><i class="edge"></i>Boundary</span>
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
        <div class="breakdown">
          <div>
            <span class="cap">Drift time</span>
            <strong class="mono">{b.driftTimeS.toFixed(1)}s / {b.totalTimeS.toFixed(1)}s</strong>
          </div>
          <div>
            <span class="cap">Angle</span>
            <strong class="mono">{b.avgAngleDeg.toFixed(0)}° avg · {b.maxAngleDeg.toFixed(0)}° max</strong>
          </div>
          <div>
            <span class="cap">Speed</span>
            <strong class="mono">{b.avgSpeedMs.toFixed(1)} m/s</strong>
          </div>
          {#if b.directionFlips !== undefined}
            <div>
              <span class="cap">Flips</span>
              <strong class="mono">{b.directionFlips}</strong>
            </div>
          {/if}
          {#if b.maxMultiplier > 1.01}
            <div>
              <span class="cap">Mult</span>
              <strong class="mono">×{b.maxMultiplier.toFixed(1)}</strong>
            </div>
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
                <span class="when mono">#{run.zoneRunNumber} · {formatShort(run.startedAt)} · {run.season}</span>
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
    grid-template-columns: 330px minmax(420px, 1fr) 330px;
    background: var(--bg-body);
    overflow: hidden;
  }
  .left-rail,
  .run-rail {
    min-height: 0;
    background: var(--bg-panel);
    display: flex;
    flex-direction: column;
    padding: 0.7rem;
    gap: 0.7rem;
  }
  .left-rail {
    border-right: 1px solid var(--bd-subtle);
  }
  .run-rail {
    border-left: 1px solid var(--bd-subtle);
  }

  /* ── Zone selector: compact, fixed-height, searchable ── */
  .zone-box {
    flex-shrink: 0;
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-md);
    display: flex;
    flex-direction: column;
  }
  .zone-search-row {
    padding: 0.45rem 0.55rem 0.1rem;
  }
  .zone-search {
    width: 100%;
    font-size: 0.72rem;
    padding: 0.28rem 0.5rem;
  }
  .zone-list {
    height: clamp(120px, 18vh, 180px);
    overflow-y: auto;
    padding: 0.4rem 0.45rem 0.45rem;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .zone-row {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 0.45rem;
    text-align: left;
    background: none;
    border: none;
    border-radius: var(--r-sm);
    color: var(--tx-mid);
    padding: 0.32rem 0.5rem;
    cursor: pointer;
    font-family: inherit;
    min-width: 0;
  }
  .zone-row:hover {
    background: var(--bg-elevated);
  }
  .zone-row.active {
    background: var(--ac-wash);
    box-shadow: inset 2px 0 0 var(--ac);
  }
  .zone-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 0.78rem;
    font-weight: 550;
  }
  .zone-armed {
    flex-shrink: 0;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--ac);
  }
  .zone-ready {
    flex-shrink: 0;
    color: var(--ok);
  }
  .zone-incomplete {
    flex-shrink: 0;
    color: var(--warn);
    font-size: 0.72rem;
    font-weight: 750;
    line-height: 1;
  }

  /* ── Current run card ── */
  .run-card {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-md);
  }
  /* Corner-of-the-eye glanceable: bright 2px ring + soft glow while live. */
  .run-card.live {
    border-color: var(--ok-bright);
    box-shadow:
      0 0 0 1px var(--ok-bright) inset,
      0 0 16px color-mix(in srgb, var(--ok-bright) 22%, transparent);
  }
  .run-card.live .card-title strong:not(.warn) {
    color: var(--ok-bright);
  }
  /* Death-timer counting (live but not scoring) — overrides the green .live. */
  .run-card.starving {
    border-color: var(--warn);
    box-shadow:
      0 0 0 1px var(--warn) inset,
      0 0 16px color-mix(in srgb, var(--warn) 22%, transparent);
  }
  .run-card.bad {
    border-color: color-mix(in srgb, var(--bad) 60%, var(--bg-panel));
  }
  .card-title strong.warn {
    color: var(--warn);
  }
  /* Status strips pinned to the card's bottom edge. */
  .run-foot {
    margin-top: auto;
  }
  .starving-msg {
    color: var(--warn);
    font-size: 0.72rem;
    padding: 0.55rem 0.8rem;
    background: color-mix(in srgb, var(--warn) 9%, transparent);
    border-top: 1px solid color-mix(in srgb, var(--warn) 40%, var(--bg-panel));
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

  /* Angle gauge */
  .gauge-wrap {
    padding: 0.9rem 1rem 0.3rem;
  }
  .gauge {
    display: block;
    width: 100%;
  }
  .g-track {
    fill: none;
    stroke: var(--bd-subtle);
    stroke-width: 2.5;
  }
  .g-dead {
    fill: none;
    stroke: var(--bg-track);
    stroke-width: 6;
  }
  .g-tick {
    stroke: var(--bd-strong);
    stroke-width: 1.2;
    stroke-linecap: round;
  }
  .g-tick.major {
    stroke: var(--tx-dim);
    stroke-width: 1.8;
  }
  .g-tick.sweet {
    stroke: var(--ac);
  }
  .g-label {
    fill: var(--tx-xdim);
    font-size: 9px;
  }
  .needle {
    stroke: var(--ac);
    stroke-width: 2.5;
    stroke-linecap: round;
  }
  .hub {
    fill: var(--bg-elevated);
    stroke: var(--bd-strong);
    stroke-width: 1.5;
  }
  .g-val {
    fill: var(--tx-hi);
    font-size: 23px;
    font-weight: 600;
  }
  .g-cap {
    fill: var(--tx-xdim);
    font-size: 7.5px;
    font-weight: 600;
    letter-spacing: 0.16em;
    font-family: var(--font-ui);
  }
  /* Idle (no live run) and stalled (live run, car stopped → angle is noise)
     both ghost the needle and dim the value: a parked "no reading" gauge. */
  .gauge-wrap.idle .needle,
  .gauge-wrap.stalled .needle { stroke: var(--tx-ghost); }
  .gauge-wrap.idle .g-val,
  .gauge-wrap.stalled .g-val { fill: var(--tx-xdim); }
  .run-card.starving .needle { stroke: var(--warn); }
  /* A stalled car is always starving, but a centred "no reading" needle is
     truer than the starving warn tint — keep it ghosted (higher specificity). */
  .run-card.starving .gauge-wrap.stalled .needle { stroke: var(--tx-ghost); }

  .live-instruments {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.6rem;
    padding: 0.3rem 0.8rem 0.4rem;
  }
  .live-instruments .inst {
    min-width: 0;
    display: grid;
    gap: 0.14rem;
  }
  .live-instruments strong.big {
    color: var(--tx-hi);
    font-size: 1.35rem;
    font-weight: 600;
    white-space: nowrap;
  }
  .metric-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.55rem;
    padding: 0.5rem 0.8rem 0.8rem;
    border-top: 1px solid var(--bd-dim);
    margin-top: 0.3rem;
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
  .metric-grid .date-sub {
    color: var(--tx-dim);
    font-size: 0.62rem;
  }
  /* Named -msg so it can never collide with the history's .run-row-wrap.invalid
     state class — that collision was the long-standing "shrunken invalid row"
     bug (the message's padding/font-size leaked onto the row wrapper). */
  .invalid-msg {
    color: var(--bad-tx);
    font-size: 0.72rem;
    padding: 0.55rem 0.8rem;
    background: color-mix(in srgb, var(--bad) 9%, transparent);
    border-top: 1px solid color-mix(in srgb, var(--bad) 40%, var(--bg-panel));
  }

  /* ── Map stage ── */
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
  .legend .edge   { background: var(--zone-edge); }
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

  /* ── Right rail: score + history ── */
  .score-card,
  .history {
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-md);
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
    padding: 0.7rem 0.8rem 0.5rem;
  }
  .score-readout .computed {
    display: grid;
    gap: 0.14rem;
  }
  .score-readout .computed strong {
    color: var(--tx-hi);
    font-size: 1.6rem;
    font-weight: 650;
    line-height: 1;
  }
  .score-readout .delta {
    display: grid;
    gap: 0.14rem;
    text-align: right;
  }
  .score-readout .delta strong {
    font-size: 0.84rem;
    color: var(--tx-mid);
  }
  .score-readout .delta.over strong {
    color: var(--warn);
  }
  .score-readout .delta.under strong {
    color: var(--info);
  }
  .breakdown {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem 0.8rem;
    padding: 0.3rem 0.8rem 0.55rem;
  }
  .breakdown div {
    min-width: 0;
    display: grid;
    gap: 0.16rem;
  }
  .breakdown strong {
    color: var(--tx-mid);
    font-size: 0.74rem;
    font-weight: 550;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .recorded {
    padding: 0.15rem 0.8rem 0;
    color: var(--tx-lo);
    font-size: 0.67rem;
  }
  .actual-label {
    display: block;
    padding: 0.45rem 0.8rem 0;
  }
  .score-input {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0.5rem;
    padding: 0.4rem 0.8rem 0.45rem;
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
    padding: 0 0.8rem 0.3rem;
  }
  .note-input {
    width: 100%;
    font-size: 0.74rem;
  }
  .score-status {
    padding: 0 0.8rem;
  }
  .empty,
  .score-status {
    color: var(--tx-dim);
    font-size: 0.66rem;
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
    text-align: left;
    border: 1px solid var(--bd-dim);
    background: var(--bg-panel);
    color: var(--tx-mid);
    border-radius: var(--r-md);
    cursor: pointer;
    font-family: inherit;
    padding: 0.5rem 0.55rem;
    display: grid;
    gap: 0.3rem;
  }
  .run-row:hover {
    border-color: var(--bd-muted);
  }
  .run-row.active {
    border-color: color-mix(in srgb, var(--ac) 55%, var(--bg-panel));
    box-shadow: inset 2px 0 0 var(--ac);
  }
  .run-row span,
  .run-row small {
    color: var(--tx-lo);
    font-size: 0.7rem;
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
  /* Invalid runs: identical geometry to every other row — the dull red
     outline alone is the at-a-glance failure marker. */
  .run-row-wrap.invalid .run-row {
    border-color: color-mix(in srgb, var(--bad) 42%, var(--bg-panel));
    background: color-mix(in srgb, var(--bad) 4%, var(--bg-panel));
  }
  .run-row-wrap.invalid .run-row:hover {
    border-color: color-mix(in srgb, var(--bad) 62%, var(--bg-panel));
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
    background: var(--bg-panel);
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
    .left-rail {
      border-right: 0;
      border-bottom: 1px solid var(--bd-subtle);
    }
    .run-rail {
      border-left: 0;
      border-top: 1px solid var(--bd-subtle);
      min-height: 320px;
    }
    .run-card {
      flex: none;
      overflow-y: visible;
    }
  }
</style>

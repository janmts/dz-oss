<script lang="ts">
  import { onMount } from 'svelte';
  import {
    sessions,
    loadSessions,
    deleteSession,
    clearAllSessions,
    setSessionBookmark,
    renameSession,
    loadSessionPackets,
    loadSessionLaps,
    settings,
  } from '$lib/stores/sessions';
  import { startReplay } from '$lib/stores/telemetry';
  import { carName } from '$lib/car-name';
  import type { TelemetryPacket, SessionRow, SessionLap } from '$lib/types';
  import MapPanel from './MapPanel.svelte';
  import AnalysisTab from './AnalysisTab.svelte';

  let {
    useMph = true,
    onReplayStarted,
  }: {
    useMph: boolean;
    onReplayStarted: () => void;
  } = $props();

  onMount(loadSessions);

  let selected = $state<SessionRow | null>(null);

  type DetailTab = 'analysis' | 'map' | 'replay';
  let detailTab = $state<DetailTab>('analysis');

  let packets = $state<TelemetryPacket[]>([]);
  let laps = $state<SessionLap[]>([]);
  let loading = $state(false);
  let loadedSessionId = $state<number | null>(null);

  $effect(() => {
    const sel = selected;
    if (!sel) return;
    if (sel.id === loadedSessionId) return;
    loadedSessionId = sel.id;
    loading = true;
    packets = [];
    laps = [];
    loadSessionPackets(sel.id).then((p) => {
      // Stale-response guard: only apply if this is still the selected session.
      if (loadedSessionId === sel.id) {
        packets = p;
        loading = false;
      }
    });
    loadSessionLaps(sel.id).then((l) => {
      if (loadedSessionId === sel.id) laps = l;
    });
  });

  let bestLapNumber = $derived(
    laps.length
      ? laps.reduce((b, l) => (l.lapTime < b.lapTime ? l : b)).lapNumber
      : -1
  );

  // Header edit state
  let editing = $state(false);
  let draftName = $state('');

  let defaultLabel = $derived(
    selected
      ? `${carName(selected.carOrdinal)} — ${new Date(selected.startedAt).toLocaleString()}`
      : ''
  );
  let displayName = $derived(selected ? (selected.name ?? defaultLabel) : '');

  function formatTime(seconds: number) {
    if (!seconds || seconds <= 0) return '—';
    const m = Math.floor(seconds / 60);
    const s = (seconds % 60).toFixed(3).padStart(6, '0');
    return `${m}:${s}`;
  }

  function formatClock(seconds: number) {
    if (!isFinite(seconds) || seconds < 0) seconds = 0;
    const m = Math.floor(seconds / 60);
    const s = (seconds % 60).toFixed(3).padStart(6, '0');
    return `${m}:${s}`;
  }

  function formatDate(ms: number) {
    return new Date(ms).toLocaleString();
  }

  function select(session: SessionRow) {
    selected = session;
    detailTab = 'analysis';
    editing = false;
  }

  async function handleDelete(session: SessionRow, e: MouseEvent) {
    e.stopPropagation();
    const label = session.name ?? formatDate(session.startedAt);
    if (!confirm(`Delete session "${label}"?`)) return;
    if (selected?.id === session.id) {
      selected = null;
      loadedSessionId = null;
    }
    await deleteSession(session.id);
  }

  async function toggleBookmark(session: SessionRow, e?: MouseEvent) {
    e?.stopPropagation();
    const next = !session.bookmarked;
    session.bookmarked = next;
    await setSessionBookmark(session.id, next);
  }

  async function handleClearAll() {
    const n = $sessions.length;
    if (n === 0) return;
    if (!confirm(`Delete ALL ${n} session${n === 1 ? '' : 's'}? This cannot be undone.`))
      return;
    selected = null;
    loadedSessionId = null;
    await clearAllSessions();
  }

  function startEdit() {
    if (!selected) return;
    draftName = selected.name ?? '';
    editing = true;
  }

  async function commitName() {
    editing = false;
    if (!selected) return;
    const v = draftName.trim();
    await renameSession(selected.id, v.length ? v : null);
    selected.name = v.length ? v : null;
  }

  function beginReplay() {
    if (!selected) return;
    startReplay(selected.id, displayName, packets);
    onReplayStarted();
  }
</script>

<div class="sessions">
  <aside class="list-rail">
    <header class="rail-head">
      <span class="cap">Sessions</span>
      <div class="head-actions">
        <span class="count mono">{$sessions.length}</span>
        <button
          class="ghost danger"
          disabled={$sessions.length === 0}
          onclick={handleClearAll}
        >Clear all</button>
      </div>
    </header>

    <div class="session-list">
      {#each $sessions as session (session.id)}
        <div
          class="session-row"
          class:active={selected?.id === session.id}
          role="button"
          tabindex="0"
          onclick={() => select(session)}
          onkeydown={(e) => e.key === 'Enter' && select(session)}
        >
          <button
            class="star"
            class:on={session.bookmarked}
            title={session.bookmarked ? 'Remove bookmark' : 'Bookmark'}
            onclick={(e) => toggleBookmark(session, e)}
          >
            {session.bookmarked ? '★' : '☆'}
          </button>
          <div class="session-info">
            <span class="session-name">
              {session.name ?? carName(session.carOrdinal)}
            </span>
            <span class="session-date mono">{formatDate(session.startedAt)}</span>
            <span class="session-best mono">Best {formatTime(session.bestLap ?? 0)}</span>
          </div>
          <button
            class="row-del"
            title="Delete session"
            aria-label="Delete session"
            onclick={(e) => handleDelete(session, e)}
          >×</button>
        </div>
      {:else}
        <p class="empty">No sessions recorded yet.</p>
      {/each}
    </div>
  </aside>

  <section class="detail">
    {#if !selected}
      <div class="placeholder">
        <span class="cap">No session selected</span>
        <p>Pick a recorded session from the list to analyse, map or replay it.</p>
      </div>
    {:else}
      <header class="detail-head">
        <div class="title-area">
          {#if editing}
            <input
              class="name-input"
              bind:value={draftName}
              placeholder={defaultLabel}
              onkeydown={(e) => {
                if (e.key === 'Enter') commitName();
                if (e.key === 'Escape') (editing = false);
              }}
              onblur={commitName}
            />
          {:else}
            <button class="name-display" onclick={startEdit} title="Click to rename">
              {displayName}
              <span class="edit-hint">✎</span>
            </button>
          {/if}
          <button
            class="star big"
            class:on={selected.bookmarked}
            onclick={() => selected && toggleBookmark(selected)}
            title={selected.bookmarked ? 'Remove bookmark' : 'Bookmark'}
          >
            {selected.bookmarked ? '★' : '☆'}
          </button>
        </div>

        <div class="detail-tabs">
          <button class:active={detailTab === 'analysis'} onclick={() => (detailTab = 'analysis')}>
            Analysis
          </button>
          <button class:active={detailTab === 'map'} onclick={() => (detailTab = 'map')}>
            Map
          </button>
          <button class:active={detailTab === 'replay'} onclick={() => (detailTab = 'replay')}>
            Replay
          </button>
        </div>
      </header>

      <div class="content">
        {#if loading}
          <p class="status">Loading {selected.packetCount} packets…</p>
        {:else if packets.length === 0}
          <p class="status">No telemetry recorded for this session.</p>
        {:else if detailTab === 'analysis'}
          <AnalysisTab {packets} {laps} {useMph} />
        {:else if detailTab === 'map'}
          <div class="map-tab">
            {#if $settings}
              <MapPanel points={packets} colorByLap drawLine fixedTrace settings={$settings} />
            {/if}
            <p class="help">
              Driven line from recorded world position. Each colour is a separate lap.
              Configure tiles &amp; calibration in Settings → Track Map; without them
              this shows a plain vector trace.
            </p>
          </div>
        {:else}
          <div class="replay-panel">
            <div class="meta">
              <div><span class="cap">Car</span><strong>{carName(selected.carOrdinal)}</strong></div>
              <div><span class="cap">Duration</span><strong class="mono">{formatClock(packets.length / 60)}</strong></div>
              <div><span class="cap">Samples</span><strong class="mono">{packets.length}</strong></div>
              <div>
                <span class="cap">Best lap</span>
                <strong class="mono">{selected.bestLap ? formatClock(selected.bestLap) : '—'}</strong>
              </div>
            </div>

            {#if laps.length}
              <div class="laps">
                <div class="laps-title cap">Lap times</div>
                {#each laps as lap}
                  <div class="lap-row" class:best={lap.lapNumber === bestLapNumber}>
                    <span>Lap {lap.lapNumber + 1}</span>
                    <span class="lap-time mono">{formatClock(lap.lapTime)}</span>
                  </div>
                {/each}
              </div>
            {/if}

            <p class="help">
              Replays this session through the gauge cluster with a timeline you can
              scrub, play and speed up.
            </p>
            <button class="replay-go" onclick={beginReplay}>▶ Replay on gauges</button>
          </div>
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .sessions {
    min-height: 0;
    display: grid;
    grid-template-columns: 320px 1fr;
    background: var(--bg-body);
    overflow: hidden;
  }
  .list-rail {
    min-height: 0;
    background: var(--bg-panel);
    border-right: 1px solid var(--bd-subtle);
    display: flex;
    flex-direction: column;
  }
  .rail-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.75rem 0.9rem;
    border-bottom: 1px solid var(--bd-dim);
  }
  .head-actions { display: flex; align-items: center; gap: 0.55rem; }
  .count { color: var(--tx-dim); font-size: 0.66rem; }
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
    text-transform: uppercase;
    padding: 0.2rem 0.4rem;
  }
  button.ghost:hover:not(:disabled) { color: var(--tx-hi); border-color: var(--bd-strong); }
  button.ghost.danger:hover:not(:disabled) { color: var(--bad-tx); border-color: var(--bad); }
  button.ghost:disabled { opacity: 0.45; cursor: default; }

  .session-list {
    min-height: 0;
    overflow-y: auto;
    padding: 0.55rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .session-row {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.5rem 0.55rem;
    border-radius: var(--r-md);
    cursor: pointer;
    border: 1px solid var(--bd-dim);
    background: var(--bg-card);
  }
  .session-row:hover { border-color: var(--bd-muted); }
  .session-row.active {
    border-color: color-mix(in srgb, var(--ac) 55%, var(--bg-panel));
    box-shadow: inset 2px 0 0 var(--ac);
  }
  .star {
    background: none; border: none; cursor: pointer;
    font-size: 1rem; color: var(--tx-xdim); line-height: 1; flex-shrink: 0;
    padding: 0;
  }
  .star:hover { color: var(--tx-lo); }
  .star.on { color: var(--ac); }
  .star.big { font-size: 1.15rem; }
  .session-info { display: flex; flex-direction: column; gap: 0.12rem; flex: 1; min-width: 0; }
  .session-name {
    font-size: 0.8rem; font-weight: 600; color: var(--tx-mid);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .session-date { font-size: 0.62rem; color: var(--tx-dim); }
  .session-best { font-size: 0.66rem; color: var(--violet); font-weight: 600; }
  .row-del {
    background: none; border: none; cursor: pointer;
    font-size: 0.95rem; color: var(--tx-xdim); flex-shrink: 0;
    line-height: 1; padding: 0.1rem 0.2rem;
  }
  .row-del:hover { color: var(--bad-tx); }
  .empty { color: var(--tx-xdim); font-size: 0.8rem; text-align: center; padding: 2rem 1rem; }

  .detail {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .placeholder {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
  }
  .placeholder p {
    color: var(--tx-dim);
    font-size: 0.8rem;
    max-width: 320px;
    text-align: center;
  }

  .detail-head {
    border-bottom: 1px solid var(--bd-dim);
    padding: 0.6rem 1rem 0;
    background: var(--bg-panel);
  }
  .title-area {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    min-width: 0;
  }
  .name-display {
    background: none;
    border: none;
    color: var(--tx-hi);
    font-family: inherit;
    font-size: 0.92rem;
    font-weight: 600;
    cursor: pointer;
    text-align: left;
    padding: 0.2rem 0.3rem;
    border-radius: var(--r-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .name-display:hover { background: var(--bg-elevated); }
  .edit-hint { color: var(--tx-dim); font-size: 0.75rem; margin-left: 0.4rem; }
  .name-input {
    flex: 1;
    background: var(--bg-elevated);
    border: 1px solid color-mix(in srgb, var(--ac) 60%, var(--bg-panel));
    color: var(--tx-hi);
    font-family: inherit;
    font-size: 0.92rem;
    padding: 0.3rem 0.5rem;
    border-radius: var(--r-sm);
  }
  .name-input:focus { outline: none; }

  .detail-tabs {
    display: flex;
    gap: 0.2rem;
    margin-top: 0.35rem;
  }
  .detail-tabs button {
    position: relative;
    background: none;
    border: none;
    color: var(--tx-dim);
    font-family: inherit;
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    padding: 0.45rem 0.8rem;
    cursor: pointer;
  }
  .detail-tabs button:hover { color: var(--tx-mid); }
  .detail-tabs button.active { color: var(--tx-hi); }
  .detail-tabs button.active::after {
    content: '';
    position: absolute;
    left: 0.5rem;
    right: 0.5rem;
    bottom: -1px;
    height: 2px;
    background: var(--ac);
  }

  .content {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 1rem;
  }
  .status {
    color: var(--tx-dim);
    text-align: center;
    padding: 3rem;
  }
  .map-tab {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
    height: 100%;
  }
  .map-tab :global(.track) {
    max-width: min(560px, 100%);
    flex: 1;
  }
  .replay-panel {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1.5rem;
    padding: 2rem 1rem;
  }
  .meta {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.75rem;
    width: 100%;
    max-width: 560px;
  }
  .meta > div {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.25rem;
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    border-radius: var(--r-md);
    padding: 0.75rem 0.5rem;
  }
  .meta strong {
    color: var(--tx-hi);
    font-size: 0.92rem;
    font-weight: 600;
  }
  .laps {
    width: 100%;
    max-width: 360px;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }
  .laps-title { margin-bottom: 0.25rem; }
  .lap-row {
    display: flex;
    justify-content: space-between;
    padding: 0.35rem 0.6rem;
    border-radius: var(--r-sm);
    background: var(--bg-card);
    border: 1px solid var(--bd-dim);
    color: var(--tx-mid);
    font-size: 0.78rem;
  }
  .lap-row.best {
    border-color: color-mix(in srgb, var(--violet) 55%, var(--bg-panel));
    color: color-mix(in srgb, var(--violet) 70%, white);
    font-weight: 600;
  }
  .help {
    color: var(--tx-lo);
    font-size: 0.8rem;
    text-align: center;
    max-width: 420px;
  }
  .replay-go {
    background: var(--ac);
    color: var(--bg-body);
    border: none;
    border-radius: var(--r-md);
    padding: 0.65rem 1.5rem;
    font-family: inherit;
    font-size: 0.9rem;
    font-weight: 650;
    cursor: pointer;
  }
  .replay-go:hover { background: var(--ac-bright); }

  @media (max-width: 880px) {
    .sessions {
      grid-template-columns: 1fr;
      grid-template-rows: minmax(160px, auto) 1fr;
      overflow-y: auto;
    }
    .list-rail {
      border-right: 0;
      border-bottom: 1px solid var(--bd-subtle);
      max-height: 240px;
    }
  }
</style>

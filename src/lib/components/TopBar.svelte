<script module lang="ts">
  export type AppTab = 'drift' | 'gauges' | 'zones' | 'sessions';
</script>

<script lang="ts">
  import { ipc } from '$lib/ipc';
  import { isConnected, displayPacket } from '$lib/stores/telemetry';
  import { carName } from '$lib/car-name';
  import { CAR_CLASS_LABELS, DRIVETRAIN_LABELS } from '$lib/types';

  let {
    activeTab,
    onTab,
    onSettings,
    tiresVisible = true,
    mapEnabled = false,
    mapPoppedOut = false,
    onToggleTires,
    onToggleMap,
  }: {
    activeTab: AppTab;
    onTab: (tab: AppTab) => void;
    onSettings: () => void;
    tiresVisible?: boolean;
    mapEnabled?: boolean;
    mapPoppedOut?: boolean;
    onToggleTires?: () => void;
    onToggleMap?: () => void;
  } = $props();

  const TABS: { id: AppTab; label: string }[] = [
    { id: 'drift', label: 'Drift' },
    { id: 'gauges', label: 'Gauges' },
    { id: 'zones', label: 'Zones' },
    { id: 'sessions', label: 'Sessions' },
  ];

  let pkt = $derived($displayPacket);
  let connected = $derived($isConnected);
  let carLabel = $derived(pkt ? carName(pkt.carOrdinal) : '—');
  let isUnknown = $derived(pkt ? carLabel.startsWith('Car #') : false);
  let classLabel = $derived(pkt ? (CAR_CLASS_LABELS[pkt.carClass] ?? '?') : '—');
  let piLabel = $derived(pkt ? String(pkt.carPi) : '—');
  let driveLabel = $derived(pkt ? (DRIVETRAIN_LABELS[pkt.drivetrainType] ?? '?') : '—');

  let copied = $state(false);
  let version = $state('');
  ipc.getAppVersion().then(v => { version = v; });

  async function copyOrdinal() {
    if (!pkt || !isUnknown) return;
    await navigator.clipboard.writeText(String(pkt.carOrdinal));
    copied = true;
    setTimeout(() => { copied = false; }, 1800);
  }
</script>

<header class="topbar">
  <div class="brand">
    <span class="wordmark">FH6<i>/</i>TEL</span>
    <span class="status" title={connected ? 'Receiving telemetry' : 'Waiting for telemetry'}>
      <span class="dot" class:live={connected}></span>
      <span class="status-label">{connected ? 'LIVE' : 'WAIT'}</span>
    </span>
  </div>

  <nav class="tabs" aria-label="Views">
    {#each TABS as t (t.id)}
      <button
        class="tab"
        class:active={activeTab === t.id}
        aria-current={activeTab === t.id ? 'page' : undefined}
        onclick={() => onTab(t.id)}
      >{t.label}</button>
    {/each}
  </nav>

  <div class="right">
    <div class="car-info">
      <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
      <span
        class="car-name"
        class:unknown={isUnknown}
        onclick={copyOrdinal}
        title={isUnknown ? `Ordinal: ${pkt?.carOrdinal} — click to copy` : undefined}
      >
        {copied ? 'Copied!' : carLabel}
      </span>
      <span class="badge class-badge" data-class={classLabel}>{classLabel}</span>
      <span class="badge mono">{piLabel}</span>
      <span class="badge">{driveLabel}</span>
    </div>

    {#if activeTab === 'gauges'}
      <div class="panel-toggles">
        <button
          class="panel-chip"
          class:active={tiresVisible}
          onclick={onToggleTires}
          title={tiresVisible ? 'Hide tires' : 'Show tires'}
        >Tires</button>
        <button
          class="panel-chip"
          class:active={mapEnabled}
          class:popped={mapPoppedOut}
          onclick={onToggleMap}
          title={mapPoppedOut ? 'Close pop-out map' : mapEnabled ? 'Hide map' : 'Show map'}
        >Map{#if mapPoppedOut} ⤢{/if}</button>
      </div>
    {/if}

    <button class="icon-btn" onclick={onSettings} title="Settings" aria-label="Settings">
      <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
        <line x1="21" y1="5" x2="14" y2="5" /><line x1="10" y1="5" x2="3" y2="5" />
        <line x1="21" y1="12" x2="12" y2="12" /><line x1="8" y1="12" x2="3" y2="12" />
        <line x1="21" y1="19" x2="16" y2="19" /><line x1="12" y1="19" x2="3" y2="19" />
        <line x1="14" y1="3" x2="14" y2="7" /><line x1="8" y1="10" x2="8" y2="14" /><line x1="16" y1="17" x2="16" y2="21" />
      </svg>
    </button>
    {#if version}<span class="version mono">v{version}</span>{/if}
  </div>
</header>

<style>
  .topbar {
    display: flex;
    align-items: stretch;
    gap: 1rem;
    padding: 0 0.9rem 0 1rem;
    height: 2.6rem;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--bd-subtle);
    flex-shrink: 0;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    flex-shrink: 0;
  }
  .wordmark {
    font-size: 0.78rem;
    font-weight: 750;
    letter-spacing: 0.1em;
    color: var(--tx-hi);
  }
  .wordmark i {
    font-style: normal;
    color: var(--ac);
    padding: 0 0.08em;
  }
  .status { display: flex; align-items: center; gap: 0.32rem; }
  .dot {
    width: 7px; height: 7px; border-radius: 50%;
    background: var(--bad);
    transition: background 0.3s, box-shadow 0.3s;
  }
  .dot.live { background: var(--ok); box-shadow: 0 0 6px color-mix(in srgb, var(--ok) 70%, transparent); }
  .status-label {
    font-size: 0.58rem; font-weight: 650; letter-spacing: 0.14em;
    color: var(--tx-dim);
    font-family: var(--font-mono);
  }

  .tabs {
    display: flex;
    align-items: stretch;
    gap: 0.15rem;
  }
  .tab {
    position: relative;
    background: none;
    border: none;
    padding: 0 0.85rem;
    font-family: inherit;
    font-size: 0.7rem;
    font-weight: 600;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--tx-dim);
    cursor: pointer;
  }
  .tab:hover { color: var(--tx-mid); }
  .tab.active { color: var(--tx-hi); }
  .tab.active::after {
    content: '';
    position: absolute;
    left: 0.55rem;
    right: 0.55rem;
    bottom: -1px;
    height: 2px;
    background: var(--ac);
  }

  .right {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 0.7rem;
    min-width: 0;
  }

  .car-info { display: flex; align-items: center; gap: 0.4rem; min-width: 0; overflow: hidden; }
  .car-name {
    font-size: clamp(0.68rem, 1.5vw, 0.78rem);
    font-weight: 550;
    color: var(--tx-mid);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: clamp(80px, 20vw, 240px);
  }
  .car-name.unknown { color: var(--tx-dim); cursor: copy; }
  .car-name.unknown:hover { color: var(--tx-lo); }
  .badge {
    font-size: clamp(0.55rem, 1.1vw, 0.62rem);
    font-weight: 650;
    padding: 0.12rem 0.34rem;
    border: 1px solid var(--bd-muted);
    border-radius: var(--r-xs);
    color: var(--tx-lo);
    flex-shrink: 0;
    white-space: nowrap;
  }
  /* Class badge colours are semantic — desaturated to match the system */
  .class-badge[data-class="X"]  { color: var(--bad);    border-color: color-mix(in srgb, var(--bad) 45%, var(--bg-panel)); }
  .class-badge[data-class="S2"] { color: var(--warn);   border-color: color-mix(in srgb, var(--warn) 45%, var(--bg-panel)); }
  .class-badge[data-class="S1"] { color: var(--ac);     border-color: color-mix(in srgb, var(--ac) 45%, var(--bg-panel)); }
  .class-badge[data-class="A"]  { color: var(--ok);     border-color: color-mix(in srgb, var(--ok) 45%, var(--bg-panel)); }
  .class-badge[data-class="B"]  { color: var(--info);   border-color: color-mix(in srgb, var(--info) 45%, var(--bg-panel)); }
  .class-badge[data-class="C"]  { color: var(--violet); border-color: color-mix(in srgb, var(--violet) 45%, var(--bg-panel)); }
  .class-badge[data-class="D"]  { color: var(--tx-lo);  border-color: var(--bd-subtle); }

  .panel-toggles { display: flex; align-items: center; gap: 0.25rem; }
  .panel-chip {
    background: none;
    border: 1px solid var(--bd-muted);
    color: var(--tx-dim);
    font-family: inherit;
    font-size: 0.58rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    padding: 0.18rem 0.45rem;
    border-radius: var(--r-xs);
    cursor: pointer;
  }
  .panel-chip:hover { color: var(--tx-mid); border-color: var(--bd-strong); }
  .panel-chip.active {
    border-color: color-mix(in srgb, var(--ac) 55%, var(--bg-panel));
    color: var(--ac);
    background: var(--ac-wash);
  }
  .panel-chip.popped { border-color: var(--warn); color: var(--warn); }

  .icon-btn {
    background: none; border: none; cursor: pointer;
    color: var(--tx-dim); padding: 0.3rem 0.35rem;
    border-radius: var(--r-sm);
    line-height: 0;
  }
  .icon-btn:hover { background: var(--bg-elevated); color: var(--tx-mid); }

  .version { font-size: 0.58rem; color: var(--tx-xdim); }
</style>

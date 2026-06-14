<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { isDesktop } from '$lib/ipc';
  import { startTelemetryListener, replay } from '$lib/stores/telemetry';
  import { loadSettings, settings, saveSettings } from '$lib/stores/sessions';
  import TopBar, { type AppTab } from '$lib/components/TopBar.svelte';
  import CompassBar from '$lib/components/CompassBar.svelte';
  import CenterPanel from '$lib/components/CenterPanel.svelte';
  import TireWidget from '$lib/components/TireWidget.svelte';
  import LiveTrackMap from '$lib/components/LiveTrackMap.svelte';
  import FloatingPanel from '$lib/components/FloatingPanel.svelte';
  import LapBar from '$lib/components/LapBar.svelte';
  import SessionsTab from '$lib/components/SessionsTab.svelte';
  import ReplayBar from '$lib/components/ReplayBar.svelte';
  import SettingsModal from '$lib/components/SettingsModal.svelte';
  import DriftZoneEditor from '$lib/components/DriftZoneEditor.svelte';
  import DriftRunDashboard from '$lib/components/DriftRunDashboard.svelte';
  import RunsTab from '$lib/components/RunsTab.svelte';

  // Drift-first: the run dashboard is the primary working screen.
  let tab = $state<AppTab>('drift');
  let showSettings = $state(false);
  let toasts = $state<{ id: number; message: string }[]>([]);
  let nextToastId = 0;
  let pendingUpdate = $state<{ version: string; install: () => Promise<void> } | null>(null);
  let updateInstalling = $state(false);
  let popoutRef: Window | null = null;
  let mapPoppedOut = $state(false);
  let controlBc: BroadcastChannel | null = null;
  let heartbeatTimer: ReturnType<typeof setTimeout> | null = null;

  function onPopoutClosed() {
    mapPoppedOut = false;
    if (heartbeatTimer) { clearTimeout(heartbeatTimer); heartbeatTimer = null; }
  }

  function resetHeartbeat() {
    if (heartbeatTimer) clearTimeout(heartbeatTimer);
    // 4 s without a heartbeat → pop-out must have died without sending popout-closed.
    heartbeatTimer = setTimeout(onPopoutClosed, 4000);
  }

  async function closePopout() {
    onPopoutClosed();
    if (isDesktop) {
      try {
        const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
        const win = await WebviewWindow.getByLabel('map').catch(console.error);
        await win?.close();
      } catch { /* ignore */ }
    } else {
      popoutRef?.close();
    }
  }

  function addToast(message: string) {
    const id = nextToastId++;
    toasts = [...toasts, { id, message }];
    setTimeout(() => { toasts = toasts.filter(t => t.id !== id); }, 4000);
  }

  async function popOutMap() {
    if (isDesktop) {
      // Tauri: create a new WebviewWindow; focus if already open
      try {
        const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
        const existing = await WebviewWindow.getByLabel('map').catch(() => null);
        if (existing) {
          await existing.setFocus();
          return;
        }
        await new WebviewWindow('map', {
          url: '/map',
          title: 'Track Map — DZ-OSS',
          width: 500,
          height: 520,
          resizable: true,
          decorations: true,
        });
      } catch (e) {
        console.error('Failed to open map window', e);
      }
    } else {
      // Browser: reuse existing pop-out if still open
      if (popoutRef && !popoutRef.closed) {
        popoutRef.focus();
        return;
      }
      popoutRef = window.open('/map', 'fh6-map', 'width=500,height=520,resizable=yes');
    }
  }

  onDestroy(() => controlBc?.close());

  onMount(async () => {
    await loadSettings();
    await startTelemetryListener({ onError: (m) => addToast(m), onBindFailed: (m) => addToast(m) });
    controlBc = new BroadcastChannel('fh6-tel-map');
    controlBc.onmessage = (e: MessageEvent<{ type: string }>) => {
      if (e.data?.type === 'popout-opened') { mapPoppedOut = true; resetHeartbeat(); }
      if (e.data?.type === 'popout-heartbeat') { resetHeartbeat(); }
      if (e.data?.type === 'popout-closed') { onPopoutClosed(); }
    };
    if (isDesktop) {
      try {
        const { invoke } = await import('@tauri-apps/api/core');
        const info = await invoke<{ version: string; is_deb: boolean } | null>('check_for_update');
        if (info) {
          pendingUpdate = {
            version: info.version,
            install: async () => {
              updateInstalling = true;
              await invoke('install_update', { isDeb: info.is_deb });
            },
          };
        }
      } catch {
        // Offline or update endpoint unreachable — ignore
      }
    }
  });

  let s = $derived($settings);

  // Replay plays back through the gauge cluster — jump there when one starts.
  $effect(() => {
    if ($replay.active) tab = 'gauges';
  });
</script>

{#if pendingUpdate}
  <div class="update-bar">
    <span>Update v{pendingUpdate.version} available</span>
    <button class="update-install" disabled={updateInstalling} onclick={() => pendingUpdate?.install()}>
      {updateInstalling ? 'Installing…' : 'Install & restart'}
    </button>
    <button class="update-dismiss" onclick={() => (pendingUpdate = null)}>✕</button>
  </div>
{/if}

<div class="app">
  <TopBar
    activeTab={tab}
    onTab={(t) => (tab = t)}
    onSettings={() => (showSettings = true)}
    tiresVisible={s?.tiresVisible ?? true}
    mapEnabled={s?.mapEnabled ?? false}
    {mapPoppedOut}
    onToggleTires={async () => { if (s) await saveSettings({ ...s, tiresVisible: !(s.tiresVisible ?? true) }); }}
    onToggleMap={async () => {
      if (mapPoppedOut) { await closePopout(); return; }
      if (s) await saveSettings({ ...s, mapEnabled: !s.mapEnabled });
    }}
  />

  <main class="view">
    {#if tab === 'drift'}
      <DriftRunDashboard />
    {:else if tab === 'gauges'}
      <div class="gauges">
        <CompassBar />
        <div class="gauge-center">
          <CenterPanel useMph={s?.useMph ?? true} />
        </div>
        <div class="lap-bar">
          <LapBar />
        </div>
      </div>
    {:else if tab === 'zones'}
      <DriftZoneEditor />
    {:else if tab === 'sessions'}
      <SessionsTab useMph={s?.useMph ?? true} onReplayStarted={() => (tab = 'gauges')} />
    {:else if tab === 'runs'}
      <RunsTab />
    {/if}
  </main>

  {#if tab === 'gauges' && (s?.tiresVisible ?? true)}
    <FloatingPanel
      id="fh6-tires"
      title="TIRES"
      defaultWidth={200}
      defaultTop={64}
      onClose={async () => { if (s) await saveSettings({ ...s, tiresVisible: false }); }}
    >
      <TireWidget
        tireTempCold={s?.tireTempCold ?? 60}
        tireTempOptimal={s?.tireTempOptimal ?? 85}
        tireTempHot={s?.tireTempHot ?? 110}
      />
    </FloatingPanel>
  {/if}

  {#if tab === 'gauges' && s?.mapEnabled}
    <FloatingPanel
      id="fh6-map"
      title="TRACK MAP"
      defaultWidth={200}
      defaultBottom={56}
      resizable
      hidden={mapPoppedOut}
      onClose={async () => { if (s) await saveSettings({ ...s, mapEnabled: false }); }}
    >
      {#snippet actions()}
        <button
          class="popout-btn"
          onclick={popOutMap}
          title="Pop out map"
          aria-label="Pop out map"
        >
          <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M15 3h6v6" /><path d="M10 14 21 3" />
            <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
          </svg>
        </button>
      {/snippet}
      <LiveTrackMap />
    </FloatingPanel>
  {/if}
</div>

<ReplayBar />

{#if toasts.length > 0}
  <div class="toast-stack">
    {#each toasts as toast (toast.id)}
      <div class="toast">{toast.message}</div>
    {/each}
  </div>
{/if}

{#if showSettings}
  <SettingsModal onClose={() => (showSettings = false)} />
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
    width: 100vw;
  }

  .view {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .view > :global(*) {
    flex: 1;
    min-height: 0;
  }

  .gauges {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .gauge-center {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    background: var(--bg-body);
  }
  .lap-bar { height: clamp(2.5rem, 5.5vh, 4rem); flex-shrink: 0; }

  .update-bar {
    position: fixed; top: 0; left: 0; right: 0; z-index: 1500;
    display: flex; align-items: center; gap: 0.75rem;
    padding: 0.35rem 1rem;
    background: var(--ac-dim); border-bottom: 1px solid var(--ac);
    font-size: 0.78rem; color: var(--tx-mid);
  }
  .update-bar span { flex: 1; }
  .update-install {
    background: var(--ac); color: var(--bg-body); border: none; border-radius: var(--r-sm);
    padding: 0.2rem 0.65rem; font-size: 0.75rem; font-weight: 600; cursor: pointer;
    font-family: inherit;
  }
  .update-install:disabled { opacity: 0.6; cursor: default; }
  .update-dismiss {
    background: none; border: none; color: var(--tx-dim);
    font-size: 0.85rem; cursor: pointer; padding: 0 0.25rem;
  }
  .update-dismiss:hover { color: var(--tx-hi); }

  .toast-stack {
    position: fixed; bottom: 4rem; left: 50%; transform: translateX(-50%);
    display: flex; flex-direction: column; gap: 0.5rem; z-index: 1400;
    pointer-events: none;
  }
  .toast {
    background: var(--bg-elevated); border: 1px solid var(--bad); border-radius: var(--r-md);
    color: var(--bad-tx); font-size: 0.8rem; padding: 0.5rem 1rem;
    max-width: 420px; text-align: center;
  }

  .popout-btn {
    background: none;
    border: none;
    color: var(--tx-xdim);
    cursor: pointer;
    padding: 0;
    line-height: 0;
  }
  .popout-btn:hover { color: var(--tx-hi); }
</style>

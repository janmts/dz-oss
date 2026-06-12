<script lang="ts">
  import { packet, replay } from '$lib/stores/telemetry';
  import { settings } from '$lib/stores/sessions';
  import MapPanel from './MapPanel.svelte';
  import type { TelemetryPacket } from '$lib/types';

  let livePosition = $state<TelemetryPacket | null>(null);
  let bc = $state<BroadcastChannel | null>(null);

  $effect(() => {
    bc = new BroadcastChannel('fh6-tel-map');
    // A (re)opened popout needs the full dataset on its next broadcast.
    bc.onmessage = (e: MessageEvent) => {
      if (e.data?.type === 'popout-opened') lastSentPts = null;
    };
    return () => bc?.close();
  });

  $effect(() => {
    const p = $packet;
    if ($replay.active) return;
    if (!p || !p.isRaceOn) { livePosition = null; return; }
    if (p.positionX !== 0 || p.positionZ !== 0) livePosition = p;
  });

  let pts = $derived($replay.active ? $replay.packets : (livePosition ? [livePosition] : []));
  let idx = $derived($replay.active ? $replay.index : 0);
  let drawLine = $derived($replay.active);

  let lastBroadcast = 0;
  let lastSentPts: TelemetryPacket[] | null = null;
  $effect(() => {
    void pts; void idx; void drawLine;
    if (!bc) return;
    const now = Date.now();
    if (now - lastBroadcast < 50) return;
    lastBroadcast = now;
    // The replay packet array is loaded once and only its index moves: clone +
    // send the (possibly huge) array only when its identity changes, and light
    // index-only updates (pts: null) otherwise.
    const sendPts = pts !== lastSentPts;
    bc.postMessage({
      type: 'map-state',
      pts: sendPts ? $state.snapshot(pts) : null,
      idx,
      drawLine,
      colorByLap: $replay.active,
      fixedTrace: $replay.active,
    });
    if (sendPts) lastSentPts = pts;
  });
</script>

{#if $settings}
  <MapPanel
    points={pts}
    currentIndex={idx}
    {drawLine}
    colorByLap={$replay.active}
    fixedTrace={$replay.active}
    settings={$settings}
  />
{/if}

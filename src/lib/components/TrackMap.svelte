<script lang="ts">
  import { LAP_PALETTE } from '$lib/theme';
  import type { TelemetryPacket } from '$lib/types';

  // Presentational only. Draws the driven line from world position (X = east,
  // Z = north). Z is flipped so north points up in SVG space.
  let {
    points,
    currentIndex = -1,
    colorByLap = true,
    showStart = true,
    drawLine = true,
    compact = false,
  }: {
    points: TelemetryPacket[];
    currentIndex?: number;
    colorByLap?: boolean;
    showStart?: boolean;
    drawLine?: boolean;
    compact?: boolean;
  } = $props();

  const LAP_COLORS = LAP_PALETTE;

  const VB = 100;
  const PAD = 6;

  // Filter out (0,0) placeholder samples that appear before the game populates
  // position (e.g. opening packet) so they don't distort the bounds.
  let valid = $derived(
    points.filter((p) => p.positionX !== 0 || p.positionZ !== 0)
  );

  let bounds = $derived.by(() => {
    if (valid.length === 0) return null;
    let minX = Infinity,
      maxX = -Infinity,
      minZ = Infinity,
      maxZ = -Infinity;
    for (const p of valid) {
      if (p.positionX < minX) minX = p.positionX;
      if (p.positionX > maxX) maxX = p.positionX;
      if (p.positionZ < minZ) minZ = p.positionZ;
      if (p.positionZ > maxZ) maxZ = p.positionZ;
    }
    const spanX = maxX - minX || 1;
    const spanZ = maxZ - minZ || 1;
    const span = Math.max(spanX, spanZ);
    return { minX, minZ, spanX, spanZ, span };
  });

  function project(p: TelemetryPacket): [number, number] {
    const b = bounds!;
    const usable = VB - PAD * 2;
    // Center the (possibly non-square) track within the square viewBox.
    const ox = PAD + (b.span - b.spanX) / 2 / b.span * usable;
    const oz = PAD + (b.span - b.spanZ) / 2 / b.span * usable;
    const x = ox + ((p.positionX - b.minX) / b.span) * usable;
    const y = oz + ((p.positionZ - b.minZ) / b.span) * usable;
    return [x, VB - y]; // flip Z so north is up
  }

  // One polyline per lap so each lap can carry its own colour.
  let segments = $derived.by(() => {
    if (!drawLine || !bounds || valid.length < 2) return [];
    const segs: { lap: number; d: string }[] = [];
    let cur: string[] = [];
    let lap = valid[0].lapNumber;
    for (const p of valid) {
      if (colorByLap && p.lapNumber !== lap && cur.length) {
        segs.push({ lap, d: cur.join(' ') });
        cur = [cur[cur.length - 1].replace('L', 'M')];
        lap = p.lapNumber;
      }
      const [x, y] = project(p);
      cur.push(`${cur.length === 0 ? 'M' : 'L'}${x.toFixed(2)},${y.toFixed(2)}`);
    }
    if (cur.length) segs.push({ lap, d: cur.join(' ') });
    return segs;
  });

  let marker = $derived.by(() => {
    if (!bounds || currentIndex < 0 || currentIndex >= points.length) return null;
    const p = points[currentIndex];
    if (p.positionX === 0 && p.positionZ === 0) return null;
    return project(p);
  });

  let startPt = $derived(
    drawLine && bounds && valid.length ? project(valid[0]) : null
  );
</script>

<div class="track" class:compact>
  {#if !bounds}
    <span class="empty">No position data</span>
  {:else}
    <svg viewBox="0 0 {VB} {VB}" preserveAspectRatio="xMidYMid meet">
      {#each segments as seg, i}
        <path
          d={seg.d}
          fill="none"
          stroke={colorByLap ? LAP_COLORS[seg.lap % LAP_COLORS.length] : 'var(--ac)'}
          stroke-width={compact ? 1.4 : 1}
          stroke-linejoin="round"
          stroke-linecap="round"
        />
      {/each}
      {#if showStart && startPt}
        <circle cx={startPt[0]} cy={startPt[1]} r={compact ? 2 : 1.6}
          fill="none" stroke="var(--tx-dim)" stroke-width="0.8" />
      {/if}
      {#if marker}
        <circle cx={marker[0]} cy={marker[1]} r={compact ? 2.6 : 2}
          fill="var(--live-dot)" stroke="var(--bg-body)" stroke-width="0.5" />
      {/if}
    </svg>
  {/if}
</div>

<style>
  .track {
    width: 100%;
    aspect-ratio: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  svg {
    width: 100%;
    height: 100%;
    display: block;
  }
  .empty {
    color: var(--tx-xdim);
    font-size: 0.7rem;
  }
  .compact .empty {
    font-size: 0.6rem;
  }
</style>

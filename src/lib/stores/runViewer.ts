// Run-viewer state. Multi-run from the ground up: the viewer holds an ORDERED
// set of selected run ids, each loaded into a cache and surfaced as a RunView
// with a stable palette colour. One run today, N tomorrow — the map (N traces)
// and graph (N series per lane) both read `runViews`.

import { writable, derived, get } from 'svelte/store';
import { ipc } from '$lib/ipc';
import { LAP_PALETTE } from '$lib/theme';
import type { DriftRunRow, DriftRunPackets } from '$lib/types';
import type { RunView } from '$lib/runViewer';

/** Selected run ids — the comparison set, in selection order (colour order). */
export const selectedRunIds = writable<number[]>([]);

/** Loaded payloads, keyed by run id. Survives deselection so re-selecting is
 *  instant; the map/graph read it through `runViews`. */
const cache = new Map<number, { row: DriftRunRow; data: DriftRunPackets }>();
const cacheVersion = writable(0);

export const loadingRunIds = writable<Set<number>>(new Set());
export const loadError = writable<string | null>(null);

/** The visible runs with presentation state. Colour = palette slot by selection
 *  order, so a run keeps its colour across the map and every graph lane. */
export const runViews = derived([selectedRunIds, cacheVersion], ([$ids]) => {
  const views: RunView[] = [];
  $ids.forEach((id, i) => {
    const e = cache.get(id);
    if (e) views.push({ runId: id, row: e.row, data: e.data, color: LAP_PALETTE[i % LAP_PALETTE.length], visible: true });
  });
  return views;
});

/** First selected run — drives single-run scoring colour + the default hover. */
export const focusedRunId = derived(selectedRunIds, ($ids) => $ids[0] ?? null);

/** Map trace colour mode. `scoring` (green/red/grey) is the single-run default;
 *  `byRun` (palette) is forced when more than one run is shown. */
export const traceColorMode = writable<'scoring' | 'byRun'>('scoring');

/** Shared hover cursor across the map and the timeline: which run + tick index. */
export const hover = writable<{ runId: number; index: number } | null>(null);

// ── Graph layout (datalogger lanes) ─────────────────────────────────────────

export interface LaneConfig {
  id: number;
  channelKeys: string[];
  height: number;
}

export const MAX_LANES = 4;
export const graphLayout = writable<'map+graph' | 'graph'>('map+graph');

let nextLaneId = 3;
export const lanes = writable<LaneConfig[]>([
  { id: 1, channelKeys: ['points', 'angle'], height: 150 },
  { id: 2, channelKeys: ['speed'], height: 110 },
]);

export function addLane() {
  lanes.update((ls) => (ls.length >= MAX_LANES ? ls : [...ls, { id: nextLaneId++, channelKeys: ['rearSlip'], height: 100 }]));
}
export function removeLane(id: number) {
  lanes.update((ls) => (ls.length <= 1 ? ls : ls.filter((l) => l.id !== id)));
}
export function setLaneChannels(id: number, channelKeys: string[]) {
  lanes.update((ls) => ls.map((l) => (l.id === id ? { ...l, channelKeys } : l)));
}
export function toggleLaneChannel(id: number, key: string) {
  lanes.update((ls) =>
    ls.map((l) =>
      l.id === id
        ? { ...l, channelKeys: l.channelKeys.includes(key) ? l.channelKeys.filter((k) => k !== key) : [...l.channelKeys, key] }
        : l,
    ),
  );
}
export function setLaneHeight(id: number, height: number) {
  lanes.update((ls) => ls.map((l) => (l.id === id ? { ...l, height: Math.max(48, Math.round(height)) } : l)));
}

// ── Selection / loading ─────────────────────────────────────────────────────

async function ensureLoaded(id: number, row: DriftRunRow) {
  if (cache.has(id)) return;
  loadingRunIds.update((s) => new Set(s).add(id));
  loadError.set(null);
  try {
    const data = await ipc.getDriftRunPackets(id);
    cache.set(id, { row, data });
    cacheVersion.update((v) => v + 1);
  } catch (e) {
    loadError.set(e instanceof Error ? e.message : String(e));
  } finally {
    loadingRunIds.update((s) => {
      const n = new Set(s);
      n.delete(id);
      return n;
    });
  }
}

/** Select exactly this run (row click) — single-run view. */
export function focusRun(row: DriftRunRow) {
  selectedRunIds.set([row.id]);
  traceColorMode.set('scoring');
  void ensureLoaded(row.id, row);
}

/** Toggle a run in the comparison set (checkbox). */
export function toggleRun(row: DriftRunRow) {
  const ids = get(selectedRunIds);
  if (ids.includes(row.id)) {
    selectedRunIds.set(ids.filter((x) => x !== row.id));
  } else {
    selectedRunIds.set([...ids, row.id]);
    if (ids.length >= 1) traceColorMode.set('byRun'); // 2nd+ run → colour by run
    void ensureLoaded(row.id, row);
  }
}

export function clearSelection() {
  selectedRunIds.set([]);
}

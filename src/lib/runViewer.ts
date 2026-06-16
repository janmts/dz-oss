// Run-viewer data layer: pure, UI-agnostic helpers shared by the Runs tab's
// map overlay and graph engine. Multi-run comparison is the default shape — the
// store holds RunView[], and everything here works on one run's DriftRunPackets
// so N runs compose for free.

import type { TelemetryPacket, TickScore, DriftRunPackets, DriftRunRow, DriftZoneRow } from '$lib/types';

/** A loaded run plus its presentation state. The viewer holds RunView[] so
 *  multi-run comparison is the default shape, never a bolt-on; colour is the
 *  run's palette slot, stable across the map and every graph lane. */
export interface RunView {
  runId: number;
  row: DriftRunRow;
  data: DriftRunPackets;
  color: string;
  visible: boolean;
}

/** Telemetry tick rate (Hz). FH6 streams a fixed ~64 Hz (15.625 ms). */
export const TELEMETRY_HZ = 64;

/** Per-tick scoring state, for trace + timeline colouring. */
export type ScoreState = 'scoring' | 'unpaid' | 'idle';

/** Authoritative per-tick state from the backend scorer:
 *  scoring (banking points) / unpaid (drifting but not banking) / idle. */
export function scoreState(tick: TickScore): ScoreState {
  if (tick.isScoring) return 'scoring';
  if (tick.isDrifting) return 'unpaid';
  return 'idle';
}

/** CSS-var token per scoring state. Resolve with themeColor() at draw time so
 *  Leaflet/uPlot get a concrete hex that tracks the theme. */
export const SCORE_STATE_TOKEN: Record<ScoreState, string> = {
  scoring: '--ok',
  unpaid: '--bad',
  idle: '--tx-dim',
};

export const SCORE_STATE_LABEL: Record<ScoreState, string> = {
  scoring: 'scoring',
  unpaid: 'off-tarmac / unpaid',
  idle: 'idle',
};

/** A "rumble tick" — at least one wheel off tarmac this tick. These are always
 *  kept as hoverable dots regardless of decimation (the user's "every rumble
 *  tick for accurate analysis"). */
export function isRumbleTick(tick: TickScore): boolean {
  return tick.tarmacWheels < 4;
}

/** Signed drift angle (deg) from a packet — atan2(velX, velZ) in the car-local
 *  frame; mirrors scoring::drift_angle_signed_deg (no yaw needed). */
export function driftAngleDeg(p: TelemetryPacket): number {
  return Math.atan2(p.velX, p.velZ) * (180 / Math.PI);
}

/** Indices of the ticks to render as hoverable dots on the map: decimated to
 *  `hz` (default 16) EXCEPT every rumble tick is always kept, plus the first
 *  and last. The connecting LINE uses the full-resolution path — only the dots
 *  are decimated, so the trace stays smooth while the dot count stays light. */
export function visibleTickIndices(
  data: DriftRunPackets,
  hz = 16,
  baseHz = TELEMETRY_HZ,
): number[] {
  const n = data.ticks.length;
  if (n === 0) return [];
  const step = Math.max(1, Math.round(baseHz / hz));
  const keep = new Set<number>([0, n - 1]);
  for (let i = 0; i < n; i += step) keep.add(i);
  for (let i = 0; i < n; i++) if (isRumbleTick(data.ticks[i])) keep.add(i);
  return [...keep].sort((a, b) => a - b);
}

// ── Channel registry (timeline graph) ───────────────────────────────────────

export type ChannelKind = 'line' | 'step';

export interface ChannelDef {
  key: string;
  label: string;
  unit?: string;
  kind?: ChannelKind;
  /** Default-on in a fresh lane's channel picker. */
  defaultOn?: boolean;
  /** Running sum of get() rather than the instantaneous value (the points curve). */
  cumulative?: boolean;
  /** Instantaneous value at tick i. */
  get: (p: TelemetryPacket, t: TickScore, i: number, all: DriftRunPackets) => number;
}

/** Every channel the timeline can plot. The lane channel-pickers (searchable
 *  checkbox dropdowns) list these; each lane chooses its own subset. */
export const CHANNELS: ChannelDef[] = [
  { key: 'angle', label: 'Drift angle', unit: '°', defaultOn: true, get: (p) => driftAngleDeg(p) },
  { key: 'speed', label: 'Speed', unit: 'm/s', defaultOn: true, get: (p) => p.speedMs },
  // World height. Monotonic on a spiral zone (e.g. z10 Kawazu Loop Bridge), so it
  // reads as the descending laps separated by ~15 m steps — the at-a-glance
  // disambiguator for a route that overlaps itself in the top-down map.
  { key: 'elevation', label: 'Elevation', unit: 'm', get: (p) => p.positionY },
  { key: 'points', label: 'Cumulative points', cumulative: true, defaultOn: true, get: (_p, t) => t.points },
  { key: 'pointsRate', label: 'Points / s', get: (_p, t) => t.points * TELEMETRY_HZ },
  {
    key: 'rearSlip',
    label: 'Rear combined slip',
    get: (p) => Math.max(Math.abs(p.tireCombinedSlipRl), Math.abs(p.tireCombinedSlipRr)),
  },
  { key: 'throttle', label: 'Throttle', unit: '%', get: (p) => (p.throttle / 255) * 100 },
  { key: 'brake', label: 'Brake', unit: '%', get: (p) => (p.brake / 255) * 100 },
  { key: 'steer', label: 'Steer', unit: '%', get: (p) => (p.steer / 127) * 100 },
  { key: 'yawRate', label: 'Yaw rate', unit: '°/s', get: (p) => p.angularVelocityY * (180 / Math.PI) },
  { key: 'tarmacWheels', label: 'Tarmac wheels', kind: 'step', get: (_p, t) => t.tarmacWheels },
  {
    key: 'surfaceRumble',
    label: 'Surface rumble (max)',
    get: (p) => Math.max(p.surfaceRumbleFl, p.surfaceRumbleFr, p.surfaceRumbleRl, p.surfaceRumbleRr),
  },
];

export const CHANNELS_BY_KEY = new Map(CHANNELS.map((c) => [c.key, c]));

/** Channel keys on by default in lane 1 / lane 2 of a fresh viewer. */
export const DEFAULT_CHANNELS = CHANNELS.filter((c) => c.defaultOn).map((c) => c.key);

/** Full-resolution series array for a channel over a run's ticks (one value per
 *  packet). Cumulative channels return the running sum. */
export function channelSeries(data: DriftRunPackets, def: ChannelDef): number[] {
  const n = data.ticks.length;
  const out = new Array<number>(n);
  if (def.cumulative) {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      acc += def.get(data.packets[i], data.ticks[i], i, data);
      out[i] = acc;
    }
  } else {
    for (let i = 0; i < n; i++) out[i] = def.get(data.packets[i], data.ticks[i], i, data);
  }
  return out;
}

/** Time axis (seconds from the run's first packet) — t=0 at the entry gate,
 *  the default alignment for overlaying multiple runs. */
export function timeAxis(data: DriftRunPackets): number[] {
  const n = data.packets.length;
  if (n === 0) return [];
  const t0 = data.packets[0].timestampMs;
  const out = new Array<number>(n);
  for (let i = 0; i < n; i++) out[i] = (data.packets[i].timestampMs - t0) / 1000;
  return out;
}

// ── Sectors (split-gate crossings) ───────────────────────────────────────────

/** The authored sector names for a zone (the gaps between splits, A→B order).
 *  Lives in `scoringConfig.sectorNames`; empty when unauthored. */
export function zoneSectorNames(zone: DriftZoneRow | null | undefined): string[] {
  const raw = (zone?.scoringConfig as Record<string, unknown> | undefined)?.sectorNames;
  return Array.isArray(raw) ? raw.map((s) => String(s)) : [];
}

/** Display name for sector `i`, falling back to "Sector N" when unauthored. */
export function sectorName(zone: DriftZoneRow | null | undefined, i: number): string {
  return zoneSectorNames(zone)[i]?.trim() || `Sector ${i + 1}`;
}

/** A contiguous run of ticks sharing one sector index. `start`/`end` are tick
 *  indices (end inclusive). With the monotonic scorer these come out in travel
 *  order, so the boundary between two spans is exactly one split crossing — the
 *  x-position for a vertical divider, and the bounds to centre a sector title. */
export interface SectorSpan {
  sector: number;
  start: number;
  end: number;
}

/** Split a run's ticks into contiguous sector spans (see [`SectorSpan`]). One
 *  span when the zone has no splits. */
export function sectorSpans(data: DriftRunPackets): SectorSpan[] {
  const ticks = data.ticks;
  if (ticks.length === 0) return [];
  const spans: SectorSpan[] = [];
  let cur = ticks[0].sector ?? 0;
  let start = 0;
  for (let i = 1; i < ticks.length; i++) {
    const s = ticks[i].sector ?? 0;
    if (s !== cur) {
      spans.push({ sector: cur, start, end: i - 1 });
      cur = s;
      start = i;
    }
  }
  spans.push({ sector: cur, start, end: ticks.length - 1 });
  return spans;
}

/** True when the run carries per-sector scoring (zone had splits and was scored
 *  on the run-viewer path). Below this, the sector UI hides itself. */
export function hasSectors(data: DriftRunPackets): boolean {
  return (data.score.sectors?.length ?? 0) > 1;
}

/** One row of the per-sector "where did I score" readout: the sector's points,
 *  its share of the run total, and time-on-sector, paired with its name. */
export interface SectorRow {
  index: number;
  name: string;
  points: number;
  sampleCount: number;
  driftTimeS: number;
  /** Share of the run's banked points (0–100). */
  pct: number;
}

/** Per-sector rows for one run, named from the zone (A→B order). */
export function sectorRows(data: DriftRunPackets, zone: DriftZoneRow | null | undefined): SectorRow[] {
  const sectors = data.score.sectors ?? [];
  const total = sectors.reduce((a, s) => a + s.points, 0);
  return sectors.map((s, i) => ({
    index: i,
    name: sectorName(zone, i),
    points: s.points,
    sampleCount: s.sampleCount,
    driftTimeS: s.driftTimeS,
    pct: total > 0 ? (s.points / total) * 100 : 0,
  }));
}

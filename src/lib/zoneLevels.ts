import type { ZonePoint } from '$lib/types';

export interface ZoneLevelBand {
  name: string;
  yMin: number;
  yMax: number;
}

export interface ZoneLevelHit {
  index: number;
  level: ZoneLevelBand;
}

export const LEVEL_SWATCHES = ['#f0ede4', '#9ba3aa', '#555d66', '#c7bda6', '#747b84', '#323841'];

function isFiniteNumber(v: unknown): v is number {
  return typeof v === 'number' && Number.isFinite(v);
}

export function cleanZoneLevel(level: ZoneLevelBand, fallbackName: string): ZoneLevelBand | null {
  if (!isFiniteNumber(level.yMin) || !isFiniteNumber(level.yMax)) return null;
  const yMin = Math.min(level.yMin, level.yMax);
  const yMax = Math.max(level.yMin, level.yMax);
  return {
    name: (level.name || fallbackName).trim() || fallbackName,
    yMin,
    yMax,
  };
}

export function normalizeZoneLevels(levels: ZoneLevelBand[]): ZoneLevelBand[] {
  return levels
    .map((level, i) => cleanZoneLevel(level, `Level ${i + 1}`))
    .filter((level): level is ZoneLevelBand => !!level);
}

export function parseZoneLevels(config: Record<string, unknown> | null | undefined): ZoneLevelBand[] {
  const raw = config?.levels;
  if (!Array.isArray(raw)) return [];
  return normalizeZoneLevels(
    raw.map((v, i) => {
      const r = v && typeof v === 'object' ? (v as Record<string, unknown>) : {};
      return {
        name: typeof r.name === 'string' ? r.name : `Level ${i + 1}`,
        yMin: Number(r.yMin),
        yMax: Number(r.yMax),
      };
    })
  );
}

export function zoneLevelForY(levels: ZoneLevelBand[], y: number | null | undefined): ZoneLevelHit | null {
  if (!isFiniteNumber(y)) return null;
  const clean = normalizeZoneLevels(levels);
  for (let i = 0; i < clean.length; i++) {
    const level = clean[i];
    if (y >= level.yMin && y <= level.yMax) return { index: i, level };
  }
  return null;
}

export function zoneLevelBadge(levels: ZoneLevelBand[], y: number | null | undefined): string {
  const hit = zoneLevelForY(levels, y);
  return hit ? `Y${hit.index + 1}` : '';
}

export function zoneLevelLabel(levels: ZoneLevelBand[], y: number | null | undefined): string {
  const hit = zoneLevelForY(levels, y);
  return hit ? hit.level.name : '';
}

export function pointY(point: ZonePoint | null | undefined): number | null {
  return isFiniteNumber(point?.y) ? point.y : null;
}

export function suggestZoneLevelsFromElevations(ys: number[], count: number): ZoneLevelBand[] {
  const finite = ys.filter(isFiniteNumber);
  if (!finite.length) return [];
  const n = Math.max(1, Math.min(8, Math.round(count)));
  const yMin = Math.min(...finite);
  const yMax = Math.max(...finite);
  const span = Math.max(0.1, yMax - yMin);
  const pad = Math.max(0.25, span * 0.006);
  const names =
    n === 1
      ? ['Full height']
      : n === 2
        ? ['Upper', 'Lower']
        : n === 3
          ? ['Upper', 'Middle', 'Lower']
          : Array.from({ length: n }, (_, i) => `Level ${i + 1}`);

  return Array.from({ length: n }, (_, i) => {
    const top = yMax - (span * i) / n;
    const bottom = yMax - (span * (i + 1)) / n;
    return {
      name: names[i],
      yMin: roundY(bottom - (i === n - 1 ? pad : 0)),
      yMax: roundY(top + (i === 0 ? pad : 0)),
    };
  });
}

export function formatLevelRange(level: ZoneLevelBand): string {
  return `${level.yMin.toFixed(1)}-${level.yMax.toFixed(1)} m`;
}

function roundY(v: number): number {
  return Math.round(v * 10) / 10;
}

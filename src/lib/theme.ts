// Resolve design-system CSS custom properties for JS consumers (leaflet
// polylines, uPlot axes, …) that can't use var() directly. Tokens are static
// for the app's lifetime, so resolved values are cached.
const cache = new Map<string, string>();

export function themeColor(varName: string, fallback: string): string {
  if (typeof window === 'undefined') return fallback;
  let v = cache.get(varName);
  if (!v) {
    v = getComputedStyle(document.documentElement).getPropertyValue(varName).trim() || fallback;
    cache.set(varName, v);
  }
  return v;
}

/** Categorical palette for per-lap traces (charts + maps) — desaturated to
 *  match the graphite system while staying mutually distinguishable. */
export const LAP_PALETTE = [
  '#82a7c8', // steel blue
  '#d2a24c', // amber
  '#84b577', // sage
  '#a995cf', // violet
  '#d56c62', // brick
  '#6fb3ad', // teal
  '#c98ba7', // rose
];

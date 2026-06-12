import type * as Leaflet from 'leaflet';
import type { LatLngBounds, Map as LMap, Polyline } from 'leaflet';
import type { EffectiveMapConfig } from '$lib/mapDefaults';

// Shared Leaflet view helpers used by every map surface (live track, drift-zone
// display/editor, calibrator). Two concerns live here:
//   1. Constraining the camera + tile requests to the bundled tile extent, so
//      the user can't scroll off into empty space that 404s.
//   2. Scaling polyline stroke weight with zoom so lines keep a roughly constant
//      on-map thickness instead of ballooning when zoomed out.

/**
 * LatLngBounds of the bundled tile pyramid's pixel extent, in the map's CRS,
 * or null when the active config has no known extent (e.g. a user-supplied tile
 * source whose coverage we can't infer).
 *
 * The extent pixels are full-resolution coordinates at `cfg.maxZoom` — the same
 * reference zoom the calibration pixels use — so we unproject at `cfg.maxZoom`,
 * matching `worldToLatLng` everywhere else.
 *
 * Feed the result to BOTH `map.setMaxBounds(...)` (so the camera can't leave the
 * covered region) and the tile layer's `bounds` option (so Leaflet never even
 * requests tiles outside the downloaded region — this is what kills the 404s).
 */
export function tileExtentBounds(
  L: typeof Leaflet,
  map: LMap,
  cfg: EffectiveMapConfig
): LatLngBounds | null {
  if (!cfg.pixelMin || !cfg.pixelMax) return null;
  const a = map.unproject(L.point(cfg.pixelMin[0], cfg.pixelMin[1]), cfg.maxZoom);
  const b = map.unproject(L.point(cfg.pixelMax[0], cfg.pixelMax[1]), cfg.maxZoom);
  return L.latLngBounds(a, b);
}

/**
 * Screen-pixel stroke weight that scales with zoom so a line keeps a roughly
 * constant on-map thickness. `refZoom` is the zoom at which the line shows its
 * full `base` weight (we anchor it to the auto-fit zoom, so the default framing
 * looks exactly as before). Each level zoomed out halves the weight, clamped to
 * `min`; zooming in never exceeds `base`, so lines don't fatten past their
 * tuned look.
 */
export function scaledWeight(
  base: number,
  zoom: number,
  refZoom: number,
  min = 1,
  max = base
): number {
  const w = base * 2 ** (zoom - refZoom);
  return Math.min(max, Math.max(min, w));
}

/** A polyline paired with the base weight it should show at `refZoom`. */
export interface WeightedLine {
  line: Polyline;
  base: number;
}

/** Re-apply zoom-scaled weights to a set of lines at the current zoom. */
export function applyLineWeights(
  lines: WeightedLine[],
  zoom: number,
  refZoom: number
): void {
  for (const { line, base } of lines) {
    line.setStyle({ weight: scaledWeight(base, zoom, refZoom) });
  }
}

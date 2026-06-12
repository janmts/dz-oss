import type { AppSettings } from '$lib/types';

// Built-in map preset. The Forza Horizon 6: Japan tiles are fetched by
// scripts/download-map-tiles.mjs and bundled into the build, served same-origin
// at /maptiles/{z}/{y}/{x}.jpg (the tile source uses a z/y/x path order, which
// the download script mirrors on disk).
//
// Calibration maps game world (X, Z) → full-resolution map pixels (X, Y) via
// two reference points. Fill these in once known; until A ≠ B the map shows
// the auto-fitted vector trace instead of tile-aligned positions.
export const FH6_JAPAN = {
  tileUrl: '/maptiles/{z}/{y}/{x}.jpg',
  minZoom: 9,
  maxZoom: 14,
  tileSize: 256,
  // Full-resolution (maxZoom) pixel extent of the bundled tiles: tile x/y
  // 8128..8191 at 256 px → drives the tile layer's `bounds` (no requests —
  // and therefore no 404s — outside the downloaded pyramid).
  pixelMin: [8128 * 256, 8128 * 256] as [number, number],
  pixelMax: [8192 * 256, 8192 * 256] as [number, number],
  // Extent of the actual map artwork (tiles 8144..8175): the source pads the
  // island to a power-of-two square with solid-gray filler tiles, and this
  // crops that ring out of the camera range. Measured from tile data by
  // scripts/measure-map-content.mjs — re-run it if the tiles are ever
  // re-downloaded (e.g. a map expansion) and paste the printed values.
  contentPixelMin: [2084864, 2084864] as [number, number],
  contentPixelMax: [2093056, 2093056] as [number, number],
  // You can zoom in past the deepest tiles (they upscale) up to viewMaxZoom.
  viewMaxZoom: 15,
  // Initial camera: a full-resolution pixel (centre of the extent) + zoom.
  defaultZoom: 14,
  defaultCenter: [
    (8128 * 256 + 8192 * 256) / 2,
    (8128 * 256 + 8192 * 256) / 2,
  ] as [number, number],
  calAWorld: [-119.49154, 3888.595],
  calAPix: [2089486, 2087415],
  calBWorld: [-7104.7695, -1863.08],
  calBPix: [2086885, 2089556],
};

// Calibration is "set" once the two world points differ.
function hasCalibration(s: AppSettings): boolean {
  const a = s.mapCalAWorld, b = s.mapCalBWorld;
  return a[0] !== b[0] || a[1] !== b[1];
}

export interface EffectiveMapConfig {
  tileUrl: string;
  minZoom: number;
  maxZoom: number;
  tileSize: number;
  viewMaxZoom: number;
  defaultZoom: number;
  defaultCenter: [number, number];
  calAWorld: [number, number];
  calAPix: [number, number];
  calBWorld: [number, number];
  calBPix: [number, number];
  // Pixel extents at maxZoom, or null when an overridden tile source is in use
  // (its coverage is unknown, so nothing is clamped). `pixelMin/Max` is the
  // downloaded tile pyramid (tile-request bounds); `contentPixelMin/Max` is the
  // real artwork inside it (camera bounds + zoom-out floor).
  pixelMin: [number, number] | null;
  pixelMax: [number, number] | null;
  contentPixelMin: [number, number] | null;
  contentPixelMax: [number, number] | null;
}

// When the user hasn't opted into overriding, the FH6 Japan preset is used
// verbatim; otherwise their stored settings drive the map.
export function effectiveMapConfig(s: AppSettings): EffectiveMapConfig {
  // Tile source / zoom: FH6 Japan preset unless the user opted into overriding.
  const base = s.mapOverride
    ? {
        tileUrl:
          s.mapTileUrl && s.mapTileUrl.includes('{z}')
            ? s.mapTileUrl
            : FH6_JAPAN.tileUrl,
        minZoom: s.mapMinZoom,
        maxZoom: s.mapMaxZoom,
        tileSize: s.mapTileSize,
      }
    : {
        tileUrl: FH6_JAPAN.tileUrl,
        minZoom: FH6_JAPAN.minZoom,
        maxZoom: FH6_JAPAN.maxZoom,
        tileSize: FH6_JAPAN.tileSize,
      };

  // Calibration always comes from settings once set (the in-app tool writes
  // it there), regardless of the override flag; otherwise the baked-in
  // FH6_JAPAN values (zeros until measured).
  const cal = hasCalibration(s)
    ? {
        calAWorld: s.mapCalAWorld,
        calAPix: s.mapCalAPix,
        calBWorld: s.mapCalBWorld,
        calBPix: s.mapCalBPix,
      }
    : {
        calAWorld: FH6_JAPAN.calAWorld as [number, number],
        calAPix: FH6_JAPAN.calAPix as [number, number],
        calBWorld: FH6_JAPAN.calBWorld as [number, number],
        calBPix: FH6_JAPAN.calBPix as [number, number],
      };

  // View settings (zoom cap + opening camera) come from settings when set,
  // independent of the override flag, so the calibrator's "save current view"
  // works on the default map too. 0 / unset → preset.
  const tileMax = base.maxZoom;
  const view = {
    viewMaxZoom:
      s.mapViewMaxZoom > 0
        ? Math.max(s.mapViewMaxZoom, tileMax)
        : Math.max(FH6_JAPAN.viewMaxZoom, tileMax),
    defaultZoom: s.mapDefaultZoom > 0 ? s.mapDefaultZoom : FH6_JAPAN.defaultZoom,
    defaultCenter:
      s.mapDefaultCenter[0] !== 0 || s.mapDefaultCenter[1] !== 0
        ? s.mapDefaultCenter
        : FH6_JAPAN.defaultCenter,
  };

  // Extents only apply when the bundled FH6 tiles are actually in use (preset,
  // or an override that didn't supply a valid custom URL).
  const extent =
    base.tileUrl === FH6_JAPAN.tileUrl
      ? {
          pixelMin: FH6_JAPAN.pixelMin as [number, number],
          pixelMax: FH6_JAPAN.pixelMax as [number, number],
          contentPixelMin: FH6_JAPAN.contentPixelMin as [number, number],
          contentPixelMax: FH6_JAPAN.contentPixelMax as [number, number],
        }
      : { pixelMin: null, pixelMax: null, contentPixelMin: null, contentPixelMax: null };

  return { ...base, ...view, ...cal, ...extent };
}

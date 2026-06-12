import type * as Leaflet from 'leaflet';
import type {
  LatLng,
  LatLngBounds,
  LayerGroup,
  Map as LMap,
  Marker,
  PathOptions,
  Polyline,
  PolylineOptions,
} from 'leaflet';
import { xyzSimpleCRS } from '$lib/mapCrs';
import { themeColor } from '$lib/theme';
import type { EffectiveMapConfig } from '$lib/mapDefaults';
import type { ZonePoint } from '$lib/types';

// The one shared Leaflet setup for every map surface (live track, drift-zone
// display, zone editor, calibrator). Each surface used to hand-roll its own
// init/calibration/redraw and the copies drifted — every map bug so far lived
// in that divergence. createGameMap() owns:
//
//   - camera discipline: tile requests bounded to the downloaded pyramid (no
//     404s), camera bounded to the real artwork (not the gray filler ring),
//     and a dynamic zoom-out floor so the view stops once the map fills it;
//   - calibration: the single world(X,Z) → map-pixel fit and its inverses;
//   - zoom-proportional line weights (base·2^Δz, reapplied on zoomend — this
//     matches Leaflet's CSS zoom animation exactly, so lines never pop);
//   - the live player marker, moved/rotated in place (no per-tick DOM churn);
//   - dead-zone camera follow that never fights user pan/zoom input.

/** Per-axis linear fit from two reference points: world X → pixel X, world Z →
 *  pixel Y. A rotation/similarity can't represent the axis reflection an
 *  overhead game map needs (world Z grows north, pixel Y grows south);
 *  per-axis slopes carry their own sign, so this stays correct. */
export interface Calib {
  mX: number;
  mZ: number;
  bX: number;
  bY: number;
}

export function makeCalib(cfg: EffectiveMapConfig): Calib | null {
  const aw = cfg.calAWorld, bw = cfg.calBWorld;
  const ap = cfg.calAPix, bp = cfg.calBPix;
  const dWX = bw[0] - aw[0];
  const dWZ = bw[1] - aw[1];
  // The two points must differ on both world axes.
  if (Math.abs(dWX) < 1e-6 || Math.abs(dWZ) < 1e-6) return null;
  const mX = (bp[0] - ap[0]) / dWX;
  const mZ = (bp[1] - ap[1]) / dWZ;
  return { mX, mZ, bX: ap[0] - mX * aw[0], bY: ap[1] - mZ * aw[1] };
}

export interface GameMapOptions {
  /** Show Leaflet's +/- control (default true). */
  zoomControl?: boolean;
  /** Extra zoom-in headroom past cfg.viewMaxZoom (zone map 3, editor 4). */
  extraZoom?: number;
  /** Clamp the camera to the artwork extent + apply the zoom-out floor
   *  (default true). The calibrator opts out — it needs the whole pyramid,
   *  gray ring included, to pick reference pixels. */
  clampToContent?: boolean;
}

export interface GameMap {
  L: typeof Leaflet;
  map: LMap;
  calib: Calib | null;
  /** Static geometry (zone lines, traces). Cleared by clearLines(). */
  overlay: LayerGroup;
  /** Marker layer, above the lines. */
  markers: LayerGroup;
  tileBounds: LatLngBounds | null;
  contentBounds: LatLngBounds | null;
  worldToPix(p: ZonePoint): [number, number];
  pixToWorld(px: { x: number; y: number }): ZonePoint;
  worldToLatLng(p: ZonePoint): LatLng;
  latLngToWorld(ll: LatLng): ZonePoint;
  /** Add a zoom-weighted polyline to the overlay. `base` is the stroke weight
   *  shown at the current weight-reference zoom; it thins by 2× per level
   *  zoomed out below it (dashes scale along). */
  addLine(latlngs: LatLng[], base: number, style: PolylineOptions): Polyline;
  /** Remove all addLine() polylines and their weight registrations. */
  clearLines(): void;
  /** Anchor zoom at which lines show their full base weight, e.g. a fit zoom. */
  setWeightRefZoom(zoom: number): void;
  /** Create/move the live player arrow; rotation happens in place on the
   *  existing DOM node. Returns true when the marker was first created. */
  setLiveArrow(p: ZonePoint, headingDeg: number, sizePx?: number): boolean;
  removeLiveArrow(): void;
  /** Centre the camera on a world point at the current zoom (no animation). */
  centerOn(p: ZonePoint): void;
  /** Dead-zone follow: pans only when the live arrow nears the viewport edge,
   *  and never while the user is interacting or a zoom animation runs. */
  followLiveArrow(): void;
  /** False while the user is interacting (recent wheel/pointer input) or a
   *  zoom animation is in flight — callers skip auto-camera moves then. */
  userIdle(): boolean;
  /** Fit the camera to world points (no animation); returns the landed zoom. */
  fitWorld(points: ZonePoint[], padFrac: number, fitMaxZoom?: number): number;
  /** Replay-style lock: fit once, then forbid zooming/panning beyond it.
   *  Returns the fit zoom. */
  lockCamera(points: ZonePoint[]): number;
  /** Undo lockCamera: restore the content clamp and the dynamic zoom floor. */
  unlockCamera(): void;
  destroy(): void;
}

const WEIGHT_MIN_PX = 0.75;
/** How long after wheel/pointer input the camera refuses to auto-follow. */
const USER_BUSY_MS = 1100;

interface WeightedLine {
  line: Polyline;
  base: number;
  dash: string | null;
}

function scaleDash(dash: string, f: number): string {
  return dash
    .split(/[\s,]+/)
    .map((n) => +(Number(n) * f).toFixed(2))
    .join(' ');
}

export async function createGameMap(
  host: HTMLElement,
  cfg: EffectiveMapConfig,
  opts: GameMapOptions = {}
): Promise<GameMap> {
  const L = await import('leaflet');
  await import('leaflet/dist/leaflet.css');

  const extra = opts.extraZoom ?? 0;
  const maxZoom = cfg.viewMaxZoom + extra;
  const calib = makeCalib(cfg);

  const map = L.map(host, {
    crs: xyzSimpleCRS(L),
    // Canvas renderer: replay traces run to tens of thousands of points, and
    // re-setting megabyte-long SVG path strings on every zoom is a jank spike.
    preferCanvas: true,
    attributionControl: false,
    zoomControl: opts.zoomControl ?? true,
    minZoom: cfg.minZoom,
    maxZoom,
    maxBoundsViscosity: 1.0,
    // Local tiles appear in milliseconds; the 200 ms white-through fade only
    // makes them look slow.
    fadeAnimation: false,
    // Half-level wheel steps: snappier feel than full-level jumps.
    zoomSnap: 0.5,
    zoomDelta: 0.5,
    wheelPxPerZoomLevel: 120,
    wheelDebounceTime: 25,
  });

  const unprojPx = (px: [number, number]) => map.unproject(L.point(px[0], px[1]), cfg.maxZoom);
  const boundsOf = (a: [number, number] | null, b: [number, number] | null) =>
    a && b ? L.latLngBounds(unprojPx(a), unprojPx(b)) : null;

  // Two distinct extents: the downloaded pyramid (tile-request bounds — this is
  // what kills the 404s) and the artwork inside it (camera bounds — this is
  // what hides the gray filler ring).
  const tileBounds = boundsOf(cfg.pixelMin, cfg.pixelMax);
  const contentBounds = boundsOf(cfg.contentPixelMin, cfg.contentPixelMax);
  const clamp = opts.clampToContent ?? true;
  const cameraBounds = clamp ? contentBounds : null;

  L.tileLayer(cfg.tileUrl, {
    minZoom: cfg.minZoom,
    maxZoom,
    // Tiles only exist up to the native zoom; beyond it Leaflet upscales.
    maxNativeZoom: cfg.maxZoom,
    tileSize: cfg.tileSize,
    noWrap: true,
    // Clamped surfaces never legitimately show the filler ring, so don't even
    // create those tiles — transient zoom overscan shows the dark container
    // background instead of flashing near-white gray.
    bounds: cameraBounds ?? tileBounds ?? undefined,
    // Tiles are bundled/local: keep a generous ring of DOM tiles alive.
    keepBuffer: 6,
  }).addTo(map);

  const overlay = L.layerGroup().addTo(map);
  const markers = L.layerGroup().addTo(map);

  // --- camera clamps -------------------------------------------------------
  let cameraLocked = false;

  if (cameraBounds) map.setMaxBounds(cameraBounds);

  // maxBounds stops panning but not zooming out (it only constrains position),
  // so the actual zoom-out stop is a dynamic minZoom: the lowest zoom at which
  // the artwork still *covers* the viewport (inside=true). Together with the
  // exact maxBounds above, the view can never show anything but map — no gray
  // filler, no void. Viewport-dependent → recomputed on resize.
  function applyZoomFloor() {
    if (!cameraBounds || cameraLocked) return;
    const coverZoom = map.getBoundsZoom(cameraBounds, true);
    const floor = Math.max(cfg.minZoom, Math.min(coverZoom, cfg.defaultZoom));
    map.setMinZoom(floor);
    if (map.getZoom() < floor) map.setZoom(floor);
  }

  map.setView(unprojPx(cfg.defaultCenter), cfg.defaultZoom);
  applyZoomFloor();

  const resizeObserver = new ResizeObserver(() => {
    map.invalidateSize();
    applyZoomFloor();
  });
  resizeObserver.observe(host);

  // --- neighbour-tile prewarm ------------------------------------------------
  // Leaflet only keeps DOM tiles for the current level (+keepBuffer ring), so a
  // zoom step or a pan into fresh territory starts from a cold fetch+decode —
  // visible as late pop-in even with bundled tiles, and the desktop webview's
  // asset protocol doesn't HTTP-cache between requests. After each settle,
  // fetch the tiles covering (a padded copy of) the view at the current and
  // neighbouring integer zooms; holding the Image objects keeps the decoded
  // resources alive in the renderer's memory cache, so when Leaflet creates a
  // tile with the same URL it paints immediately. Bundled source only — a
  // custom source has unknown coverage and would 404.
  const warmPixMin = cameraBounds ? cfg.contentPixelMin : cfg.pixelMin;
  const warmPixMax = cameraBounds ? cfg.contentPixelMax : cfg.pixelMax;
  const WARM_KEEP = 600;
  const warmed = new Map<string, HTMLImageElement>();

  function warmTiles() {
    if (!warmPixMin || !warmPixMax) return;
    const cur = Math.round(map.getZoom());
    let budget = 120;
    for (const z of [cur, cur - 1, cur + 1]) {
      if (z < cfg.minZoom || z > cfg.maxZoom || budget <= 0) continue;
      const down = 2 ** (cfg.maxZoom - z);
      // Pyramid/content extent in tile coords at this level.
      const ex0 = Math.floor(warmPixMin[0] / down / cfg.tileSize);
      const ey0 = Math.floor(warmPixMin[1] / down / cfg.tileSize);
      const ex1 = Math.ceil(warmPixMax[0] / down / cfg.tileSize) - 1;
      const ey1 = Math.ceil(warmPixMax[1] / down / cfg.tileSize) - 1;
      // Viewport (padded wider at the current level to stay ahead of pans).
      const llb = map.getBounds().pad(z === cur ? 0.5 : 0.15);
      const pA = map.project(llb.getNorthWest(), z);
      const pB = map.project(llb.getSouthEast(), z);
      const x0 = Math.max(ex0, Math.floor(Math.min(pA.x, pB.x) / cfg.tileSize));
      const x1 = Math.min(ex1, Math.floor(Math.max(pA.x, pB.x) / cfg.tileSize));
      const y0 = Math.max(ey0, Math.floor(Math.min(pA.y, pB.y) / cfg.tileSize));
      const y1 = Math.min(ey1, Math.floor(Math.max(pA.y, pB.y) / cfg.tileSize));
      for (let y = y0; y <= y1 && budget > 0; y++) {
        for (let x = x0; x <= x1 && budget > 0; x++) {
          const url = cfg.tileUrl
            .replace('{z}', String(z))
            .replace('{x}', String(x))
            .replace('{y}', String(y));
          if (warmed.has(url)) continue;
          const img = new Image();
          img.decoding = 'async';
          img.src = url;
          warmed.set(url, img);
          budget--;
          if (warmed.size > WARM_KEEP) {
            const oldest = warmed.keys().next().value;
            if (oldest !== undefined) warmed.delete(oldest);
          }
        }
      }
    }
  }
  // Debounced: a wheel burst fires moveend per half-level step, and decode
  // bursts mid-animation compete with the zoom rasterisation for frame time
  // (visible as compositor misses). Warm only once things go quiet.
  let warmTimer: ReturnType<typeof setTimeout> | null = null;
  function scheduleWarm() {
    if (warmTimer) clearTimeout(warmTimer);
    warmTimer = setTimeout(() => {
      warmTimer = null;
      if (zoomAnimating) scheduleWarm();
      else warmTiles();
    }, 200);
  }
  map.on('moveend', scheduleWarm);
  scheduleWarm();

  // --- user-interaction bookkeeping (for follow etiquette) -----------------
  let userBusyUntil = 0;
  let zoomAnimating = false;
  const markBusy = () => {
    userBusyUntil = Date.now() + USER_BUSY_MS;
  };
  host.addEventListener('wheel', markBusy, { passive: true });
  host.addEventListener('pointerdown', markBusy);
  map.on('zoomstart', () => {
    zoomAnimating = true;
  });
  map.on('zoomend', () => {
    zoomAnimating = false;
  });

  // --- calibration-backed coordinate helpers -------------------------------
  function worldToPix(p: ZonePoint): [number, number] {
    const c = calib!;
    return [c.mX * p.x + c.bX, c.mZ * p.z + c.bY];
  }
  function pixToWorld(px: { x: number; y: number }): ZonePoint {
    const c = calib!;
    return { x: (px.x - c.bX) / c.mX, z: (px.y - c.bY) / c.mZ };
  }
  function worldToLatLng(p: ZonePoint): LatLng {
    const [x, y] = worldToPix(p);
    return map.unproject(L.point(x, y), cfg.maxZoom);
  }
  function latLngToWorld(ll: LatLng): ZonePoint {
    return pixToWorld(map.project(ll, cfg.maxZoom));
  }

  // --- zoom-proportional line weights ---------------------------------------
  const lines: WeightedLine[] = [];
  let weightRefZoom = cfg.defaultZoom;

  function weightAt(base: number, zoom: number): number {
    return Math.min(base, Math.max(WEIGHT_MIN_PX, base * 2 ** (zoom - weightRefZoom)));
  }

  function applyWeights() {
    const z = map.getZoom();
    for (const wl of lines) {
      const w = weightAt(wl.base, z);
      const style: PathOptions = { weight: w };
      if (wl.dash) style.dashArray = scaleDash(wl.dash, w / wl.base);
      wl.line.setStyle(style);
    }
  }
  map.on('zoomend', applyWeights);

  function addLine(latlngs: LatLng[], base: number, style: PolylineOptions): Polyline {
    const dash = typeof style.dashArray === 'string' ? style.dashArray : null;
    const w = weightAt(base, map.getZoom());
    const line = L.polyline(latlngs, {
      ...style,
      weight: w,
      dashArray: dash ? scaleDash(dash, w / base) : style.dashArray,
    }).addTo(overlay);
    lines.push({ line, base, dash });
    return line;
  }

  function clearLines() {
    overlay.clearLayers();
    lines.length = 0;
  }

  function setWeightRefZoom(zoom: number) {
    weightRefZoom = zoom;
    applyWeights();
  }

  // --- live player arrow -----------------------------------------------------
  let liveMarker: Marker | null = null;
  let liveSize = 0;
  let liveHeading = NaN;

  function arrowIcon(sizePx: number) {
    return L.divIcon({
      className: 'player-arrow',
      html:
        `<svg width="${sizePx}" height="${sizePx}" viewBox="0 0 24 24" style="transform-origin:50% 50%">` +
        `<path d="M12 2 L19 21 L12 15 L5 21 Z" fill="${themeColor('--live-dot', '#ecc274')}" ` +
        `stroke="${themeColor('--bg-body', '#0f1012')}" stroke-width="1.5" stroke-linejoin="round"/></svg>`,
      iconSize: [sizePx, sizePx],
      iconAnchor: [sizePx / 2, sizePx / 2],
    });
  }

  function setLiveArrow(p: ZonePoint, headingDeg: number, sizePx = 28): boolean {
    const ll = worldToLatLng(p);
    let created = false;
    if (!liveMarker || liveSize !== sizePx) {
      removeLiveArrow();
      liveMarker = L.marker(ll, { icon: arrowIcon(sizePx), interactive: false }).addTo(markers);
      liveSize = sizePx;
      liveHeading = NaN;
      created = true;
    } else {
      liveMarker.setLatLng(ll);
    }
    if (!Number.isFinite(liveHeading) || Math.abs(headingDeg - liveHeading) > 0.25) {
      const svg = liveMarker.getElement()?.firstElementChild as HTMLElement | null;
      if (svg) svg.style.transform = `rotate(${headingDeg}deg)`;
      liveHeading = headingDeg;
    }
    return created;
  }

  function removeLiveArrow() {
    liveMarker?.remove();
    liveMarker = null;
    liveSize = 0;
    liveHeading = NaN;
  }

  function centerOn(p: ZonePoint) {
    map.setView(worldToLatLng(p), map.getZoom(), { animate: false });
  }

  function userIdle(): boolean {
    return !zoomAnimating && Date.now() >= userBusyUntil;
  }

  function followLiveArrow() {
    if (!liveMarker || cameraLocked || !userIdle()) return;
    const pt = map.latLngToContainerPoint(liveMarker.getLatLng());
    const size = map.getSize();
    const mx = Math.max(40, size.x * 0.18);
    const my = Math.max(40, size.y * 0.18);
    const tx = Math.min(Math.max(pt.x, mx), size.x - mx);
    const ty = Math.min(Math.max(pt.y, my), size.y - my);
    // Pan only by the overshoot, so the camera scrolls gently with the car
    // instead of snapping it back to dead centre on every tick.
    if (tx !== pt.x || ty !== pt.y) map.panBy([pt.x - tx, pt.y - ty], { animate: false });
  }

  // --- fits & camera locks ----------------------------------------------------
  function fitWorld(points: ZonePoint[], padFrac: number, fitMaxZoom?: number): number {
    const b = L.latLngBounds(points.map(worldToLatLng)).pad(padFrac);
    map.fitBounds(b, { maxZoom: fitMaxZoom ?? maxZoom, animate: false });
    return map.getZoom();
  }

  function lockCamera(points: ZonePoint[]): number {
    const b = L.latLngBounds(points.map(worldToLatLng));
    map.fitBounds(b, { padding: [20, 20], maxZoom: cfg.defaultZoom, animate: false });
    cameraLocked = true;
    map.setMinZoom(map.getZoom());
    map.setMaxBounds(b.pad(0.05));
    return map.getZoom();
  }

  function unlockCamera() {
    if (!cameraLocked) return;
    cameraLocked = false;
    map.setMinZoom(cfg.minZoom);
    if (cameraBounds) map.setMaxBounds(cameraBounds);
    else map.setMaxBounds(undefined);
    applyZoomFloor();
  }

  function destroy() {
    resizeObserver.disconnect();
    host.removeEventListener('wheel', markBusy);
    host.removeEventListener('pointerdown', markBusy);
    removeLiveArrow();
    if (warmTimer) clearTimeout(warmTimer);
    warmed.clear();
    map.remove();
  }

  return {
    L,
    map,
    calib,
    overlay,
    markers,
    tileBounds,
    contentBounds,
    worldToPix,
    pixToWorld,
    worldToLatLng,
    latLngToWorld,
    addLine,
    clearLines,
    setWeightRefZoom,
    setLiveArrow,
    removeLiveArrow,
    centerOn,
    followLiveArrow,
    userIdle,
    fitWorld,
    lockCamera,
    unlockCamera,
    destroy,
  };
}

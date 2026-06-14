import type { ZonePoint } from '$lib/types';

// Centripetal Catmull-Rom densification of a drift-zone shape, in WORLD coords.
//
// This is the SHARED geometry contract: the editor/maps render the curve by
// tessellating the sparse anchors here, and the Rust scorer tessellates the SAME
// anchors with a byte-for-byte mirror of this routine (see `drift.rs::tessellate`
// + the `curve.fixtures.json` parity test). The arithmetic structure below must
// stay identical to the Rust side — display == scored geometry only holds if the
// two produce the same points. If you change the math here, change it there and
// regenerate the fixture (`node scripts/check-curve.mjs --write`).
//
// Why centripetal (alpha = 0.5): hand-placed / live-captured anchors are unevenly
// spaced, and uniform Catmull-Rom (alpha = 0) forms cusps and self-intersecting
// loops on uneven spacing — worst exactly at corners, the part that matters most.
// Centripetal is the standard fix. We use alpha = 0.5 exclusively and express the
// knot delta as sqrt(sqrt(d2)) rather than pow(d2, 0.25): IEEE sqrt is
// correctly-rounded (so JS and Rust agree to the bit), pow is not.

export interface TessellateOptions {
  /** Closed ring (scoring region) vs open polyline (a road-edge boundary).
   *  Closed wraps the last anchor back to the first; open passes through the
   *  first/last anchor with a reflected end tangent. */
  closed?: boolean;
  /** Interpolated points emitted per anchor span. 10 reads smooth at editor
   *  zoom; higher = denser (and heavier). */
  segments?: number;
}

export const DEFAULT_SEGMENTS = 10;

/** Densify `anchors` into the curve points the display draws and the scorer
 *  tests. Fewer than 3 anchors can't form a curve (a 2-point gate, a single
 *  point) so they pass through unchanged — a defensive copy either way. */
export function tessellate(anchors: ZonePoint[], opts: TessellateOptions = {}): ZonePoint[] {
  const seg = Math.max(1, Math.floor(opts.segments ?? DEFAULT_SEGMENTS));
  const closed = opts.closed ?? false;
  const n = anchors.length;
  if (n < 3) return anchors.map((p) => ({ x: p.x, z: p.z }));

  const out: ZonePoint[] = [];
  if (closed) {
    for (let i = 0; i < n; i++) {
      emitSpan(
        out,
        anchors[(i - 1 + n) % n],
        anchors[i],
        anchors[(i + 1) % n],
        anchors[(i + 2) % n],
        seg
      );
    }
  } else {
    // Reflect the endpoints (p_{-1} = 2*p0 - p1, p_n = 2*p_{n-1} - p_{n-2}) so the
    // curve reaches the first/last anchor with a natural tangent and no
    // zero-length end span (which would divide by zero in the knot deltas).
    const start: ZonePoint = { x: 2 * anchors[0].x - anchors[1].x, z: 2 * anchors[0].z - anchors[1].z };
    const end: ZonePoint = {
      x: 2 * anchors[n - 1].x - anchors[n - 2].x,
      z: 2 * anchors[n - 1].z - anchors[n - 2].z,
    };
    for (let i = 0; i < n - 1; i++) {
      const p0 = i === 0 ? start : anchors[i - 1];
      const p3 = i + 2 <= n - 1 ? anchors[i + 2] : end;
      emitSpan(out, p0, anchors[i], anchors[i + 1], p3, seg);
    }
    out.push({ x: anchors[n - 1].x, z: anchors[n - 1].z });
  }
  return out;
}

/** A zone's boundary interpolation mode. Lives in `scoringConfig.curve`. */
export type ZoneCurveMode = 'linear' | 'catmull';

/** Read the interpolation mode from a zone's `scoringConfig` bag (the same JSON
 *  bag `boundarySlackM` lives in). Mirrors `curve_is_catmull` in drift.rs. */
export function zoneCurveMode(config: Record<string, unknown> | null | undefined): ZoneCurveMode {
  return config && config.curve === 'catmull' ? 'catmull' : 'linear';
}

/** Curve a road-edge boundary for display when the zone is smoothed, else return
 *  the straight chords. The default segment count matches the Rust scorer
 *  (`drift.rs` `CURVE_DEFAULT_SEGMENTS`), so the drawn boundary equals the scored
 *  one — keep the two in lockstep. */
export function boundaryCurve(points: ZonePoint[], mode: ZoneCurveMode): ZonePoint[] {
  return mode === 'catmull' ? tessellate(points, { closed: false }) : points;
}

/** The persisted scoring-region shape (in `scoringConfig.scoringRegion`). A zone
 *  may have a shared ring (`anchors`, applied for either entry gate) and/or
 *  directed rings (`byGate`, each applied only when the run entered via that
 *  gate). Gate A = the first-boundary-points gate, B = the last (see drift.rs).
 *  The editor authors `anchors` today; the directed `byGate` form is stored
 *  forward-compat so no migration / re-author is needed when per-gate authoring
 *  lands. The backend Stage-2 scorer mirrors `scoringRingForEntry` below. */
export interface ScoringRegion {
  anchors?: ZonePoint[];
  byGate?: { a?: ZonePoint[]; b?: ZonePoint[] };
}

const ringPoints = (v: unknown): ZonePoint[] => (Array.isArray(v) ? (v as ZonePoint[]) : []);

/** Parse the scoring region out of a zone's `scoringConfig` bag (never throws). */
export function parseScoringRegion(config: Record<string, unknown> | null | undefined): ScoringRegion {
  const r = config?.scoringRegion;
  return r && typeof r === 'object' ? (r as ScoringRegion) : {};
}

/** The shared / "both gates" ring — the single ring the editor authors today. */
export function sharedScoringRing(config: Record<string, unknown> | null | undefined): ZonePoint[] {
  return ringPoints(parseScoringRegion(config).anchors);
}

/** Every ring to DISPLAY for a zone: the shared ring plus any directed per-gate
 *  rings, each a raw anchor list (close/curve at draw time via ringCurve). */
export function scoringRings(config: Record<string, unknown> | null | undefined): ZonePoint[][] {
  const r = parseScoringRegion(config);
  const rings: ZonePoint[][] = [];
  for (const ring of [r.anchors, r.byGate?.a, r.byGate?.b]) {
    const pts = ringPoints(ring);
    if (pts.length) rings.push(pts);
  }
  return rings;
}

/** The ring that SCORES for a run entering via `gate` (the Stage-2 selection
 *  rule, to be mirrored in the Rust scorer): the directed ring for that gate,
 *  else the shared ring. */
export function scoringRingForEntry(
  config: Record<string, unknown> | null | undefined,
  gate: 'a' | 'b'
): ZonePoint[] {
  const r = parseScoringRegion(config);
  const directed = ringPoints(gate === 'a' ? r.byGate?.a : r.byGate?.b);
  return directed.length ? directed : ringPoints(r.anchors);
}

/** Display points for a CLOSED scoring ring: the centripetal curve through the
 *  anchors when smoothed (raw otherwise), with the first point appended so a
 *  polyline renders the closed loop. Empty for fewer than 2 anchors.
 *
 *  STAGE-2 SCORER CONTRACT: a future per-tick position gate that tests
 *  point-in-(scoring ring) MUST branch on the zone's curve mode exactly as this
 *  does — tessellate({closed:true}) only when 'catmull', raw anchors when
 *  'linear' — and build its polygon WITHOUT the appended closing dup (the
 *  ray-cast wraps last->first implicitly). Unconditionally tessellating a linear
 *  ring would score an area that diverges from the violet outline drawn here. */
export function ringCurve(points: ZonePoint[], mode: ZoneCurveMode): ZonePoint[] {
  if (points.length < 2) return [];
  const curve = mode === 'catmull' ? tessellate(points, { closed: true }) : points;
  return [...curve, curve[0]];
}

/** Centripetal knot delta between two control points: dist^0.5, written as a
 *  double sqrt for bit-exact parity with Rust. */
function knotDelta(a: ZonePoint, b: ZonePoint): number {
  const dx = b.x - a.x;
  const dz = b.z - a.z;
  return Math.sqrt(Math.sqrt(dx * dx + dz * dz));
}

/** Emit `seg` points along the Catmull-Rom span p1->p2 (neighbours p0, p3), via
 *  the Barry-Goldman pyramid. Emits k = 0..seg-1 (so t = t1 is included, t = t2
 *  excluded); t = t1 evaluates exactly to p1, so each anchor lands in the output
 *  exactly once and the curve interpolates its anchors. */
function emitSpan(
  out: ZonePoint[],
  p0: ZonePoint,
  p1: ZonePoint,
  p2: ZonePoint,
  p3: ZonePoint,
  seg: number
): void {
  const t0 = 0;
  const t1 = t0 + knotDelta(p0, p1);
  const t2 = t1 + knotDelta(p1, p2);
  const t3 = t2 + knotDelta(p2, p3);

  // Coincident control points collapse a knot interval; fall back to a straight
  // p1->p2 chord for this span rather than dividing by zero.
  if (!(t1 > t0) || !(t2 > t1) || !(t3 > t2)) {
    for (let k = 0; k < seg; k++) {
      const u = k / seg;
      out.push({ x: p1.x + (p2.x - p1.x) * u, z: p1.z + (p2.z - p1.z) * u });
    }
    return;
  }

  for (let k = 0; k < seg; k++) {
    const t = t1 + (t2 - t1) * (k / seg);
    const a1x = ((t1 - t) / (t1 - t0)) * p0.x + ((t - t0) / (t1 - t0)) * p1.x;
    const a1z = ((t1 - t) / (t1 - t0)) * p0.z + ((t - t0) / (t1 - t0)) * p1.z;
    const a2x = ((t2 - t) / (t2 - t1)) * p1.x + ((t - t1) / (t2 - t1)) * p2.x;
    const a2z = ((t2 - t) / (t2 - t1)) * p1.z + ((t - t1) / (t2 - t1)) * p2.z;
    const a3x = ((t3 - t) / (t3 - t2)) * p2.x + ((t - t2) / (t3 - t2)) * p3.x;
    const a3z = ((t3 - t) / (t3 - t2)) * p2.z + ((t - t2) / (t3 - t2)) * p3.z;
    const b1x = ((t2 - t) / (t2 - t0)) * a1x + ((t - t0) / (t2 - t0)) * a2x;
    const b1z = ((t2 - t) / (t2 - t0)) * a1z + ((t - t0) / (t2 - t0)) * a2z;
    const b2x = ((t3 - t) / (t3 - t1)) * a2x + ((t - t1) / (t3 - t1)) * a3x;
    const b2z = ((t3 - t) / (t3 - t1)) * a2z + ((t - t1) / (t3 - t1)) * a3z;
    out.push({
      x: ((t2 - t) / (t2 - t1)) * b1x + ((t - t1) / (t2 - t1)) * b2x,
      z: ((t2 - t) / (t2 - t1)) * b1z + ((t - t1) / (t2 - t1)) * b2z,
    });
  }
}

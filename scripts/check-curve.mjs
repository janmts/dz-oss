// JS-side test + parity harness for the shared zone tessellator (src/lib/curve.ts).
//
// The repo has no JS test runner, so this is a runnable node check (matching the
// scripts/*.mjs convention). It asserts the tessellator's invariants AND a set of
// GOLDEN coordinates that are duplicated verbatim in the Rust test
// (`drift.rs::tests::tessellate_matches_golden`). Same goldens + identical
// arithmetic on both sides == display geometry equals scored geometry.
//
// Run:  node --experimental-strip-types scripts/check-curve.mjs
// (Node >= 22.6 strips the TS types from curve.ts at load.)

import { tessellate, ringCurve, scoringRings, scoringRingForEntry } from '../src/lib/curve.ts';

let failures = 0;
function check(name, cond) {
  if (cond) {
    console.log(`  ok   ${name}`);
  } else {
    console.error(`  FAIL ${name}`);
    failures++;
  }
}
const EPS = 1e-9;
const near = (a, b) => Math.abs(a - b) <= EPS;
const ptNear = (p, x, z) => near(p.x, x) && near(p.z, z);
const finite = (pts) => pts.every((p) => Number.isFinite(p.x) && Number.isFinite(p.z));

// --- canonical inputs (shared with the Rust test) -------------------------
const openL = [
  { x: 0, z: 0 },
  { x: 10, z: 0 },
  { x: 10, z: 10 },
];
const closedSquare = [
  { x: 0, z: 0 },
  { x: 10, z: 0 },
  { x: 10, z: 10 },
  { x: 0, z: 10 },
];
const collinear = [
  { x: 0, z: 0 },
  { x: 5, z: 0 },
  { x: 10, z: 0 },
  { x: 20, z: 0 },
];
// Unevenly-spaced spans → centripetal ≠ uniform → irrational knot deltas, so this
// case exercises the sqrt path that the symmetric cases above collapse out.
const openUneven = [
  { x: 0, z: 0 },
  { x: 10, z: 0 },
  { x: 12, z: 8 },
];

console.log('open L-corner, seg=4');
const a = tessellate(openL, { closed: false, segments: 4 });
check('count == (n-1)*seg + 1 == 9', a.length === 9);
check('passes through anchor 0', ptNear(a[0], 0, 0));
check('passes through anchor 1 at index seg', ptNear(a[4], 10, 0));
check('passes through anchor 2 (last)', ptNear(a[8], 10, 10));
check('no NaN/Inf', finite(a));

console.log('closed square, seg=4');
const b = tessellate(closedSquare, { closed: true, segments: 4 });
check('count == n*seg == 16', b.length === 16);
check('anchor 0 at index 0', ptNear(b[0], 0, 0));
check('anchor 1 at index seg', ptNear(b[4], 10, 0));
check('anchor 2 at index 2*seg', ptNear(b[8], 10, 10));
check('anchor 3 at index 3*seg', ptNear(b[12], 0, 10));
check('no NaN/Inf', finite(b));

console.log('collinear open, seg=3 → stays on the line');
const c = tessellate(collinear, { closed: false, segments: 3 });
check('all z ~ 0', c.every((p) => near(p.z, 0)));
check('x monotonic non-decreasing', c.every((p, i) => i === 0 || p.x >= c[i - 1].x - EPS));

console.log('fewer than 3 anchors → passthrough copy');
const d = tessellate([{ x: 1, z: 2 }, { x: 3, z: 4 }], { closed: false, segments: 4 });
check('returns the 2 input points unchanged', d.length === 2 && ptNear(d[0], 1, 2) && ptNear(d[1], 3, 4));

console.log('uneven open, seg=4');
const u = tessellate(openUneven, { closed: false, segments: 4 });
check('count == 9', u.length === 9);
check('passes through anchor 0', ptNear(u[0], 0, 0));
check('passes through anchor 1 at index seg', ptNear(u[4], 10, 0));
check('passes through anchor 2 (last)', ptNear(u[8], 12, 8));
check('no NaN/Inf', finite(u));

// --- GOLDEN interior points (must match drift.rs verbatim) ----------------
// Asserted here so a change to the JS math is caught; duplicated in the Rust test
// so both languages are pinned to the same numbers. The symmetric cases are exact
// dyadics (equal knot spacing); the uneven case is the irrational sqrt-path lock.
console.log('golden interior points');
check('open L-corner a[2]', ptNear(a[2], 5.625, -0.625));
check('closed square b[2]', ptNear(b[2], 5, -1.25));
check('uneven open u[2]', ptNear(u[2], 5.510823710739302, -0.5771314068283177));
check('uneven open u[6]', ptNear(u[6], 11.421236023089621, 3.524085249957107));

console.log('ringCurve closed loop');
const ring = ringCurve(closedSquare, 'catmull');
check('non-empty closed loop', ring.length > 0);
check('first point repeated to close', ptNear(ring[0], ring[ring.length - 1].x, ring[ring.length - 1].z));
check('single anchor → empty', ringCurve([{ x: 0, z: 0 }], 'catmull').length === 0);

console.log('scoring-region readers (forward-compat per-gate shape)');
const tri = (s) => [{ x: 0, z: 0 }, { x: s, z: 0 }, { x: s, z: s }];
const shared = { scoringRegion: { anchors: tri(1) } };
const directed = { scoringRegion: { byGate: { a: tri(2), b: tri(3) } } };
const both = { scoringRegion: { anchors: tri(1), byGate: { a: tri(2) } } };
check('shared → 1 display ring', scoringRings(shared).length === 1);
check('directed → 2 display rings', scoringRings(directed).length === 2);
check('shared + one directed → 2 display rings', scoringRings(both).length === 2);
check('entry a picks byGate.a', scoringRingForEntry(directed, 'a')[1].x === 2);
check('entry b picks byGate.b', scoringRingForEntry(directed, 'b')[1].x === 3);
check('entry falls back to shared when no directed', scoringRingForEntry(shared, 'a').length === 3);
check('entry b on shared+a falls back to shared', scoringRingForEntry(both, 'b')[1].x === 1);
check('no region → empty', scoringRingForEntry({}, 'a').length === 0);

if (process.argv.includes('--print')) {
  console.log('a =', JSON.stringify(a));
  console.log('b =', JSON.stringify(b));
  console.log('u =', JSON.stringify(u));
}

if (failures) {
  console.error(`\n${failures} check(s) FAILED`);
  process.exit(1);
}
console.log('\nall curve checks passed');

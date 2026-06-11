#!/usr/bin/env node
/**
 * Validates the car-ordinal lookup tables consumed by src/lib/car-name.ts.
 *
 * Guards against the class of bug in issue #18, where the FH6 table had
 * copy-paste duplicate name strings that pointed several different ordinals
 * at the same car (e.g. the Unimog U5023 rendered as a Mercedes-AMG GT R).
 *
 * Checks:
 *   1. Both JSON tables parse and every value is a non-empty string.
 *   2. The FH6 table has no duplicate display-name strings, except the small
 *      allowlist of genuine same-vehicle variants (AI "traffic" clones and
 *      DLC/livery copies that legitimately share a name).
 *   3. A few known-good ordinals still resolve correctly (issue #18 regressions).
 *
 * Run: `npm run check:cars`  (also wired ahead of `npm run build`).
 */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = new URL('../', import.meta.url);
const load = (rel) => JSON.parse(readFileSync(new URL(rel, root), 'utf-8'));

const fh6 = load('src/lib/fh6-car-ordinals.json');
const legacy = load('src/lib/car-ordinals.json');

const errors = [];

// 1. shape ---------------------------------------------------------------
for (const [table, name] of [[fh6, 'fh6-car-ordinals.json'], [legacy, 'car-ordinals.json']]) {
  for (const [ord, value] of Object.entries(table)) {
    if (!/^\d+$/.test(ord)) errors.push(`${name}: non-numeric ordinal key "${ord}"`);
    if (typeof value !== 'string' || value.trim() === '') {
      errors.push(`${name}: ordinal ${ord} has empty/invalid name ${JSON.stringify(value)}`);
    }
  }
}

// 2. duplicate display strings in the FH6 table --------------------------
// Allowlisted names map one vehicle to several ordinals on purpose: the game
// ships AI-traffic clones (asset suffix _Traffic / _Jobs) and DLC/preorder or
// livery copies (ID / WP / Trolli) that share an identical real car. Telemetry
// only ever reports the base ordinal, so an identical display name is correct.
// Confirmed against the ONYX game-asset DB (asset codes share the same model).
const INTENTIONAL_DUPLICATES = new Set([
  '1962 Peel P50',
  '1992 Mitsubishi Galant VR-4',
  '1994 Honda Acty',
  '1995 Mitsubishi Montero Exceed 2800 TD',
  '1995 Porsche 911 GT2',
  '1997 Nissan Stagea RS FOUR V',
  '2005 SUBARU LEGACY B4 2.0 GT',
  '2005 Toyota Crown Super Deluxe Taxi',
  '2010 Mazda Mazdaspeed 3',
  '2012 Jeep Wrangler Rubicon',
  '2013 Mercedes-Benz G 65 AMG',
  '2017 Toyota JPN Taxi',
  '2021 Mercedes-AMG GT Black Series',
  '2022 Honda e',
  '2022 SUBARU WRX',
  '2024 Nissan GT-R NISMO',
  '2026 Toyota Scallop',
]);

const byName = new Map();
for (const [ord, value] of Object.entries(fh6)) {
  (byName.get(value) ?? byName.set(value, []).get(value)).push(ord);
}
for (const [value, ords] of byName) {
  if (ords.length > 1 && !INTENTIONAL_DUPLICATES.has(value)) {
    errors.push(
      `fh6-car-ordinals.json: duplicate name ${JSON.stringify(value)} on ordinals ` +
      `${ords.join(', ')} — a copy-paste mistake (see issue #18) or a new ` +
      `same-vehicle variant to add to INTENTIONAL_DUPLICATES.`,
    );
  }
}

// 3. regression spot-checks ----------------------------------------------
// The merged map applied by src/lib/car-name.ts (FH6 wins over legacy).
const merged = { ...legacy, ...fh6 };
const expect = {
  2986: 'Unimog',                 // issue #18 headline case
  2654: 'GT R',                   // the car 2986 used to be mislabelled as
  4114: 'Skyline GT-R',           // README-confirmed test entry
  2574: 'Warthog',                // README-confirmed test entry
};
for (const [ord, needle] of Object.entries(expect)) {
  const name = merged[ord];
  if (!name || !name.includes(needle)) {
    errors.push(`carName(${ord}) = ${JSON.stringify(name)} — expected to contain ${JSON.stringify(needle)}`);
  }
}

// report -----------------------------------------------------------------
if (errors.length) {
  console.error(`✗ car-ordinal check failed (${errors.length}):`);
  for (const e of errors) console.error(`  - ${e}`);
  process.exit(1);
}
console.log(
  `✓ car-ordinal check passed — ${Object.keys(fh6).length} FH6 + ` +
  `${Object.keys(legacy).length} legacy entries, ` +
  `${INTENTIONAL_DUPLICATES.size} allowlisted same-vehicle names.`,
);

#!/usr/bin/env node
// Measures the real (non-filler) content extent of the bundled map tiles.
//
// The tile source pads the island artwork to a power-of-two square with
// byte-identical solid-gray filler tiles. This script finds that filler at the
// deepest zoom (most frequent byte-identical file), takes the bounding box of
// every other tile, and prints the pixel-extent constants for
// `FH6_JAPAN.contentPixelMin/Max` in src/lib/mapDefaults.ts.
//
// Re-run after re-downloading tiles (e.g. a map expansion) and paste the
// printed constants if they changed.
//
// Usage: node scripts/measure-map-content.mjs [--tiles <dir>] [--tile-size <px>]

import { createHash } from 'node:crypto';
import { readdir, readFile, stat } from 'node:fs/promises';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

const args = process.argv.slice(2);
let tilesDir = join(ROOT, 'static', 'maptiles');
let tileSize = 256;
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--tiles') tilesDir = args[++i];
  else if (args[i] === '--tile-size') tileSize = Number(args[++i]);
  else throw new Error(`Unknown arg: ${args[i]}`);
}

const zooms = (await readdir(tilesDir)).map(Number).filter(Number.isFinite);
if (zooms.length === 0) throw new Error(`No zoom directories in ${tilesDir}`);
const z = Math.max(...zooms);
const zDir = join(tilesDir, String(z));

// Disk layout is {z}/{y}/{x}.jpg (mapgenie path order, see download script).
const tiles = [];
for (const yName of await readdir(zDir)) {
  const y = Number(yName);
  if (!Number.isFinite(y)) continue;
  for (const xFile of await readdir(join(zDir, yName))) {
    const x = Number(xFile.split('.')[0]);
    if (!Number.isFinite(x)) continue;
    const path = join(zDir, yName, xFile);
    tiles.push({ x, y, path, size: (await stat(path)).size });
  }
}
console.log(`z${z}: ${tiles.length} tiles on disk`);

// Most frequent file size = filler candidate (the artwork tiles all differ).
const bySize = new Map();
for (const t of tiles) bySize.set(t.size, (bySize.get(t.size) ?? 0) + 1);
const [fillerSize, fillerCount] = [...bySize.entries()].sort((a, b) => b[1] - a[1])[0];
if (fillerCount < tiles.length * 0.05) {
  console.log('No dominant identical-size tile group — map may have no filler border.');
  console.log('Content extent = full tile extent; nothing to crop.');
  process.exit(0);
}

// Confirm the candidates are byte-identical, not a size coincidence.
const hash = async (p) => createHash('sha1').update(await readFile(p)).digest('hex');
const candidates = tiles.filter((t) => t.size === fillerSize);
const fillerHash = await hash(candidates[0].path);
const fillerKeys = new Set();
for (const t of candidates) {
  if ((await hash(t.path)) === fillerHash) fillerKeys.add(`${t.x},${t.y}`);
}
console.log(
  `filler: ${fillerKeys.size} byte-identical tiles of ${fillerSize} B (sha1 ${fillerHash.slice(0, 12)}…)`
);

const content = tiles.filter((t) => !fillerKeys.has(`${t.x},${t.y}`));
if (content.length === 0) throw new Error('Every tile classified as filler — wrong directory?');
const min = { x: Infinity, y: Infinity };
const max = { x: -Infinity, y: -Infinity };
for (const t of content) {
  min.x = Math.min(min.x, t.x); min.y = Math.min(min.y, t.y);
  max.x = Math.max(max.x, t.x); max.y = Math.max(max.y, t.y);
}
const w = max.x - min.x + 1;
const h = max.y - min.y + 1;
if (content.length !== w * h) {
  console.log(
    `note: content region is not a solid rectangle (${content.length} tiles in a ${w}×${h} box) — bbox is still correct.`
  );
}

console.log(`content tiles: x ${min.x}..${max.x}, y ${min.y}..${max.y} (${w}×${h} of ${Math.sqrt(tiles.length)}² total)`);
console.log('');
console.log(`For src/lib/mapDefaults.ts (pixel coords at maxZoom = ${z}):`);
console.log(`  contentPixelMin: [${min.x * tileSize}, ${min.y * tileSize}] as [number, number],`);
console.log(`  contentPixelMax: [${(max.x + 1) * tileSize}, ${(max.y + 1) * tileSize}] as [number, number],`);

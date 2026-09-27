// Prints the size of each section of a .wasm file: where the bytes go.
import { readFileSync } from 'node:fs';

const bytes = readFileSync(process.argv[2]);
const names = ['custom', 'type', 'import', 'function', 'table', 'memory', 'global',
  'export', 'start', 'element', 'code', 'data', 'datacount', 'tag'];
let off = 8;
const leb = () => { let r = 0, s = 0, b; do { b = bytes[off++]; r |= (b & 0x7f) << s; s += 7; } while (b & 0x80); return r >>> 0; };
const rows = [];
while (off < bytes.length) {
  const id = bytes[off++];
  const size = leb();
  let name = names[id] ?? `id${id}`;
  if (id === 0) { const start = off; const n = leb(); name = `custom:${bytes.subarray(off, off + n).toString()}`; off = start; }
  rows.push([name, size]);
  off += size;
}
console.log(`total ${bytes.length}`);
for (const [n, s] of rows) console.log(`${n.padEnd(28)} ${String(s).padStart(9)}`);

// Node smoke, ESM: `node scripts/node-smoke.mjs [loader]`.
//
// Imports an ESM loader from pkg/ and runs the shared checks. `loader` is
//   - concerto-engine.node.mjs (the default), the one Node imports: the
//     `web` glue, instantiated with initSync from the raw .wasm, read with
//     readFileSync;
//   - or concerto-engine.mjs, the browser loader (initSync from the inlined
//     bytes), which Node runs as well.
// One loader per process: both instantiate the same `web` glue module.
import { runChecks } from './checks.mjs';

const loader = process.argv[2] ?? 'concerto-engine.node.mjs';
const t0 = performance.now();
const engine = await import(`../pkg/${loader}`);
const importMs = performance.now() - t0;
// Synchronous, straight after the import.
new engine.ModelManagerHandle().free();

await engine.init();
const rows = runChecks(engine);
console.log(JSON.stringify({
  runtime: `node ${process.version} (ESM)`,
  module: `pkg/${loader}`,
  importMs: +importMs.toFixed(1),
  rows,
}, null, 2));
process.exit(rows.every((r) => r.ok) ? 0 : 1);

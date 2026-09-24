// Node smoke, ESM: `node scripts/node-smoke.mjs`.
//
// Imports the ESM loader (pkg/concerto-engine.mjs: the `web` glue,
// instantiated with initSync from the inlined bytes while the module is
// evaluated) and runs the shared checks.
import { runChecks } from './checks.mjs';

const t0 = performance.now();
const engine = await import('../pkg/concerto-engine.mjs');
const importMs = performance.now() - t0;
// Synchronous, straight after the import.
new engine.ModelManagerHandle().free();

await engine.init();
const rows = runChecks(engine);
console.log(JSON.stringify({
  runtime: `node ${process.version} (ESM)`,
  importMs: +importMs.toFixed(1),
  rows,
}, null, 2));
process.exit(rows.every((r) => r.ok) ? 0 : 1);

// Node ESM smoke: import the inlined-bytes loader, which compiles and
// instantiates synchronously during import, then run the shared checks.
import { readFileSync } from 'node:fs';

const t0 = performance.now();
const engine = await import('../dist/concerto-engine.mjs');
const importMs = performance.now() - t0;
const { runChecks } = await import('./checks.mjs');

const metamodelAst = JSON.parse(readFileSync(new URL('../fixtures/metamodel.json', import.meta.url), 'utf8'));
const results = runChecks(engine, { metamodelAst });
console.log(JSON.stringify({
  runtime: `node ${process.version} (ESM)`,
  importMs: +importMs.toFixed(1),
  loadTimings: engine.loadTimings,
  results,
}, null, 2));

// Node CommonJS smoke (Node 18+): `require` the inlined-bytes loader. The
// require returns a ready module, so `new Engine()` is synchronous for CJS
// callers too. The shared checks are ESM, so they are loaded with import().
const { readFileSync } = require('node:fs');
const path = require('node:path');

const t0 = performance.now();
const engine = require('../dist/concerto-engine.cjs');
const requireMs = performance.now() - t0;
// Synchronous, straight after require: the property that matters.
const probe = new engine.Engine();
probe.free();

import('./checks.mjs').then(({ runChecks }) => {
  const metamodelAst = JSON.parse(readFileSync(path.join(__dirname, '../fixtures/metamodel.json'), 'utf8'));
  const results = runChecks(engine, { metamodelAst });
  console.log(JSON.stringify({
    runtime: `node ${process.version} (CommonJS)`,
    requireMs: +requireMs.toFixed(1),
    results,
  }, null, 2));
}).catch((e) => { console.error(e); process.exit(1); });

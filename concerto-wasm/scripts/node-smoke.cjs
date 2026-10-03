// Node smoke, CommonJS: `node scripts/node-smoke.cjs [module]`.
//
// Requires the CommonJS loader (pkg/concerto-engine.cjs, or `module`, e.g.
// `@accordproject/concerto-engine` resolved from the concerto checkout's
// workspace link) and uses it on the very next line: `require` returns a
// ready module, so instantiation is synchronous. Then runs the shared checks.
const path = require('node:path');

const target = process.argv[2] ?? path.join(__dirname, '../pkg/concerto-engine.cjs');
const t0 = performance.now();
const engine = require(require.resolve(target, { paths: [process.cwd()] }));
const requireMs = performance.now() - t0;
// Synchronous, straight after require.
new engine.ModelManagerHandle().free();

import('./checks.mjs').then(async ({ runChecks }) => {
  await engine.init();
  const rows = runChecks(engine);
  // P5-110 (#477): a second instantiation of the module (a fresh require of
  // the self-contained loader) hashes instance keys under other keys.
  const resolved = require.resolve(target, { paths: [process.cwd()] });
  delete require.cache[resolved];
  const again = require(resolved);
  const probes = [engine.hashSeed().probe, again.hashSeed().probe];
  rows.push({
    name: 'two instantiations seed the untrusted-key hasher differently',
    ok: again !== engine && probes[0] !== probes[1],
    detail: { probes },
  });
  console.log(JSON.stringify({
    runtime: `node ${process.version} (CommonJS)`,
    module: target,
    requireMs: +requireMs.toFixed(1),
    rows,
  }, null, 2));
  process.exit(rows.every((r) => r.ok) ? 0 : 1);
}).catch((e) => { console.error(e); process.exit(1); });

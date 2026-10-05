// Node smoke, ESM: `node scripts/node-smoke.mjs [loader]`.
//
// Imports an ESM loader from pkg/ and runs the shared checks. `loader` is
//   - concerto-engine.node.mjs (the default), the one Node imports: the
//     `web` glue, instantiated with initSync from the raw .wasm, read with
//     readFileSync, so it is ready once imported;
//   - or concerto-engine.mjs, the browser loader, which is not instantiated
//     until `await init()`. Node's fetch cannot read a file: URL, so this
//     smoke passes the bytes as `init({ module_or_path })`; the Chromium
//     smoke runs its default fetch.
// One loader per process: both instantiate the same `web` glue module.
import { readFileSync } from 'node:fs';
import { runChecks } from './checks.mjs';

const loader = process.argv[2] ?? 'concerto-engine.node.mjs';
const browserLoader = loader === 'concerto-engine.mjs';
const t0 = performance.now();
const engine = await import(`../pkg/${loader}`);
const importMs = performance.now() - t0;
const rows = [];

if (browserLoader) {
  rows.push(...await browserInit(engine));
} else {
  // Synchronous, straight after the import.
  new engine.ModelManagerHandle().free();
  await engine.init();
}
rows.push(...runChecks(engine));
console.log(JSON.stringify({
  runtime: `node ${process.version} (ESM)`,
  module: `pkg/${loader}`,
  importMs: +importMs.toFixed(1),
  rows,
}, null, 2));
process.exit(rows.every((r) => r.ok) ? 0 : 1);

// The browser loader's explicit init (BC-32): nothing is instantiated at
// import, setHost before init is kept, and init is idempotent.
async function browserInit(engine) {
  const out = [];
  const row = (name, ok, detail) => out.push({ name, ok: !!ok, ...(detail === undefined ? {} : { detail }) });

  let before;
  try {
    new engine.ModelManagerHandle().free();
  } catch (e) {
    before = e;
  }
  row('an engine call before init() throws', before instanceof Error, before && String(before));

  class HostError extends Error {}
  engine.setHost((payload) => new HostError(payload.message));

  const bytes = readFileSync(new URL('../pkg/concerto_wasm.wasm', import.meta.url));
  const first = engine.init({ module_or_path: bytes });
  const second = engine.init();
  row('init() is idempotent: every call returns the same promise', first === second && first instanceof Promise);
  await first;
  row('init() after it resolved returns the same resolved promise', engine.init() === first);

  const mm = new engine.ModelManagerHandle();
  let thrown;
  try {
    mm.addModel('{}');
  } catch (e) {
    thrown = e;
  }
  mm.free();
  row('the error factory set before init() is registered by init()', thrown instanceof HostError, thrown && String(thrown));
  return out;
}

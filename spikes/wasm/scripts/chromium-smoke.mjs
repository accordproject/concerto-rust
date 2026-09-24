// Headless-Chromium smoke (Playwright): `node scripts/chromium-smoke.mjs [--bench]`.
//
// Serves this directory over HTTP and, in a page:
//   1. probes Chromium's limit on synchronous WebAssembly.Module compilation
//      on the main thread, with synthetic modules of growing size;
//   2. imports the inlined-bytes ESM loader on the main thread (synchronous
//      compile + instantiate) and runs the shared checks;
//   3. does the same inside a module Worker, where no limit applies;
//   4. tries the async fallback: WebAssembly.compile, then a synchronous
//      instantiate of the compiled module;
//   5. with --bench, runs the boundary benchmark in the page.
// It runs in both Playwright browsers: the headless shell (the default) and
// full Chromium in new-headless mode (channel "chromium").
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const root = fileURLToPath(new URL('..', import.meta.url));
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.json': 'application/json', '.wasm': 'application/wasm' };
const PAGE = '<!doctype html><meta charset="utf-8"><title>concerto wasm spike</title><body>spike</body>';

const server = createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  if (path === '/') { res.writeHead(200, { 'content-type': 'text/html' }); res.end(PAGE); return; }
  const file = normalize(join(root, path));
  if (!file.startsWith(root)) { res.writeHead(403); res.end(); return; }
  try {
    const body = await readFile(file);
    res.writeHead(200, { 'content-type': TYPES[extname(file)] ?? 'application/octet-stream' });
    res.end(body);
  } catch {
    res.writeHead(404); res.end();
  }
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const origin = `http://127.0.0.1:${server.address().port}`;
const bench = process.argv.includes('--bench');

// --- code evaluated in the page ----------------------------------------------

async function probeSyncLimit() {
  // A valid, empty module padded with a custom section to `size` bytes.
  const make = (size) => {
    const header = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    const payloadLen = size - header.length - 1 - 5 - 2; // id, 5-byte LEB, name
    const leb = [];
    let n = payloadLen + 2;
    for (let i = 0; i < 5; i++) { leb.push((n & 0x7f) | (i < 4 ? 0x80 : 0)); n >>>= 7; }
    const b = new Uint8Array(size);
    b.set(header, 0); b[8] = 0; b.set(leb, 9); b[14] = 1; b[15] = 0x78; // name "x"
    return b;
  };
  const KB = 1024; const MB = KB * KB;
  const sizes = [4 * KB, 4 * KB + 1, 1 * MB, 4 * MB, 8 * MB, 8 * MB + 1, 16 * MB];
  const out = [];
  for (const size of sizes) {
    try { new WebAssembly.Module(make(size)); out.push({ size, ok: true }); }
    catch (e) { out.push({ size, ok: false, error: `${e.name}: ${e.message}` }); }
  }
  return out;
}

async function mainThread(bench) {
  const t0 = performance.now();
  let engine;
  try {
    engine = await import('/dist/concerto-engine.mjs');
  } catch (e) {
    return { ok: false, error: `${e.name}: ${e.message}` };
  }
  const importMs = performance.now() - t0;
  const metamodelAst = await (await fetch('/fixtures/metamodel.json')).json();
  const { runChecks } = await import('/scripts/checks.mjs');
  const results = runChecks(engine, { metamodelAst });
  let benchRows;
  if (bench) {
    const { runBench, bestOf } = await import('/scripts/bench-core.mjs');
    const runs = [];
    for (let i = 0; i < 3; i++) runs.push(runBench(engine, { metamodelAst }));
    benchRows = bestOf(runs);
  }
  return { ok: true, importMs, loadTimings: engine.loadTimings, results, benchRows };
}

async function inWorker() {
  const src = `
    self.onmessage = async () => {
      try {
        const t0 = performance.now();
        const engine = await import('${location.origin}/dist/concerto-engine.mjs');
        const importMs = performance.now() - t0;
        const metamodelAst = await (await fetch('${location.origin}/fixtures/metamodel.json')).json();
        const { runChecks } = await import('${location.origin}/scripts/checks.mjs');
        self.postMessage({ ok: true, importMs, loadTimings: engine.loadTimings, results: runChecks(engine, { metamodelAst }) });
      } catch (e) { self.postMessage({ ok: false, error: e.name + ': ' + e.message }); }
    };`;
  const url = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }));
  const w = new Worker(url, { type: 'module' });
  const res = await new Promise((r) => { w.onmessage = (m) => r(m.data); w.onerror = (e) => r({ ok: false, error: e.message }); w.postMessage(0); });
  w.terminate();
  return res;
}

async function asyncCompileThenSyncInstantiate() {
  // What P4-01's browser loader would do when sync compile is refused:
  // compile off the main thread's sync path, then hand the Module to initSync.
  const bytes = new Uint8Array(await (await fetch('/dist/concerto_wasm_spike_opt.wasm')).arrayBuffer());
  const t0 = performance.now();
  const module = await WebAssembly.compile(bytes);
  const t1 = performance.now();
  const glue = await import('/dist/web/concerto_wasm_spike.js?fresh');
  try {
    glue.initSync({ module });
  } catch (e) {
    return { ok: false, compileMs: t1 - t0, error: `${e.name}: ${e.message}` };
  }
  const t2 = performance.now();
  const e = new glue.Engine();
  const ok = e.namespaceCount() === 0;
  e.free();
  return { ok, asyncCompileMs: t1 - t0, syncInstantiateMs: t2 - t1 };
}

// --- drive the browsers --------------------------------------------------------

const report = { origin, browsers: [] };
let failed = false;
for (const [label, opts] of [['headless shell (default)', {}], ['chromium, new headless', { channel: 'chromium' }]]) {
  const browser = await chromium.launch(opts);
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(origin);
  const r = { browser: label, version: browser.version() };
  r.syncLimit = await page.evaluate(probeSyncLimit);
  r.mainThread = await page.evaluate(mainThread, bench && label.startsWith('headless'));
  r.worker = await page.evaluate(inWorker);
  r.asyncCompile = await page.evaluate(asyncCompileThenSyncInstantiate);
  r.pageErrors = errors;
  if (!r.mainThread.ok || !r.worker.ok || !r.asyncCompile.ok) failed = true;
  report.browsers.push(r);
  await browser.close();
}
server.close();

for (const r of report.browsers) {
  const benchRows = r.mainThread.benchRows;
  delete r.mainThread.benchRows;
  console.log(JSON.stringify(r, null, 2));
  if (benchRows) {
    const { formatRows } = await import('./bench-core.mjs');
    console.log(`\nbenchmark in ${r.browser} ${r.version}, main thread, best of 3\n${formatRows(benchRows)}`);
  }
}
process.exit(failed ? 1 : 0);

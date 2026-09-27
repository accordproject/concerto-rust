// Headless-Chromium smoke (Playwright): `node scripts/chromium-smoke.mjs`.
//
// Serves this directory over HTTP and, in a page, in both Playwright
// browsers (the headless shell and full Chromium in new-headless mode):
//   1. probes the main thread's limit on a synchronous WebAssembly.Module
//      compile, with padded valid modules around 8 MiB, and compares it with
//      the size of pkg/concerto_wasm.wasm (spike REPORT §2);
//   2. imports the ESM loader on the main thread (synchronous compile and
//      instantiate from the inlined bytes) and runs the shared checks;
//   3. does the same in a module Worker;
//   4. runs the async fallback, WebAssembly.compile then initSync, on a fresh
//      copy of the `web` glue;
//   5. times the handle API's calls on the main thread (spike REPORT §3).
// Exits non-zero if any step or check fails.
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const root = fileURLToPath(new URL('..', import.meta.url));
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.json': 'application/json', '.wasm': 'application/wasm' };
const PAGE = '<!doctype html><meta charset="utf-8"><title>concerto-wasm smoke</title><body>smoke</body>';

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
const wasmBytes = (await stat(join(root, 'pkg/concerto_wasm.wasm'))).size;

// --- code evaluated in the page ----------------------------------------------

function probeSyncLimit() {
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
  const MB = 1024 * 1024;
  const out = [];
  for (const size of [4 * MB, 8 * MB, 8 * MB + 1]) {
    try { new WebAssembly.Module(make(size)); out.push({ size, ok: true }); }
    catch (e) { out.push({ size, ok: false, error: `${e.name}: ${e.message}` }); }
  }
  return out;
}

async function mainThread() {
  const t0 = performance.now();
  let engine;
  try {
    engine = await import('/pkg/concerto-engine.mjs');
  } catch (e) {
    return { ok: false, error: `${e.name}: ${e.message}` };
  }
  const importMs = performance.now() - t0;
  // Synchronous, straight after the import.
  new engine.ModelManagerHandle().free();
  await engine.init();
  const { runChecks } = await import('/scripts/checks.mjs');
  const rows = runChecks(engine);
  return { ok: rows.every((r) => r.ok), importMs, rows };
}

async function inWorker() {
  const src = `
    self.onmessage = async () => {
      try {
        const engine = await import('${location.origin}/pkg/concerto-engine.mjs');
        const { runChecks } = await import('${location.origin}/scripts/checks.mjs');
        const rows = runChecks(engine);
        self.postMessage({ ok: rows.every((r) => r.ok), rows: rows.filter((r) => !r.ok) });
      } catch (e) { self.postMessage({ ok: false, error: e.name + ': ' + e.message }); }
    };`;
  const url = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }));
  const w = new Worker(url, { type: 'module' });
  const res = await new Promise((r) => { w.onmessage = (m) => r(m.data); w.onerror = (e) => r({ ok: false, error: e.message }); w.postMessage(0); });
  w.terminate();
  return res;
}

async function asyncFallback() {
  // What init() does when the main thread refuses the synchronous compile.
  const bytes = new Uint8Array(await (await fetch('/pkg/concerto_wasm.wasm')).arrayBuffer());
  const t0 = performance.now();
  const module = await WebAssembly.compile(bytes);
  const t1 = performance.now();
  const glue = await import('/pkg/web/concerto_wasm.js?fresh');
  try {
    glue.initSync({ module });
  } catch (e) {
    return { ok: false, error: `${e.name}: ${e.message}` };
  }
  const t2 = performance.now();
  const mm = new glue.ModelManagerHandle();
  const ok = mm.modelFileId('concerto@1.0.0') === 0;
  mm.free();
  return { ok, compileMs: t1 - t0, instantiateMs: t2 - t1 };
}

async function timeCalls() {
  const engine = await import('/pkg/concerto-engine.mjs');
  const mm = new engine.ModelManagerHandle();
  const concept = mm.declarationId('concerto@1.0.0.Concept');
  const N = 20000;
  const time = (fn) => {
    let best = Infinity;
    for (let run = 0; run < 3; run++) {
      const t0 = performance.now();
      for (let i = 0; i < N; i++) fn();
      best = Math.min(best, performance.now() - t0);
    }
    return +((best * 1e6) / N).toFixed(0); // ns per call, best of 3
  };
  const rows = {
    'generation()': time(() => mm.generation()),
    'declarationId(fqn)': time(() => mm.declarationId('concerto@1.0.0.Concept')),
    'propertyIds(decl)': time(() => mm.propertyIds(concept)),
    'declarationSnapshot(decl)': time(() => mm.declarationSnapshot(concept)),
    'JSON.parse(declarationSnapshot(decl))': time(() => JSON.parse(mm.declarationSnapshot(concept))),
  };
  mm.free();
  return rows;
}

// --- drive the browsers --------------------------------------------------------

const report = { origin, wasmBytes, browsers: [] };
let failed = false;
for (const [label, opts] of [['headless shell (default)', {}], ['chromium, new headless', { channel: 'chromium' }]]) {
  const browser = await chromium.launch(opts);
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(String(e)));
  await page.goto(origin);
  const r = { browser: label, version: browser.version() };
  r.syncLimit = await page.evaluate(probeSyncLimit);
  const limit = r.syncLimit.filter((p) => p.ok).reduce((max, p) => Math.max(max, p.size), 0);
  r.sizeCheck = { wasmBytes, largestSyncCompileAccepted: limit, ok: wasmBytes <= limit };
  r.mainThread = await page.evaluate(mainThread);
  r.worker = await page.evaluate(inWorker);
  r.asyncFallback = await page.evaluate(asyncFallback);
  r.nsPerCall = await page.evaluate(timeCalls);
  r.pageErrors = errors;
  if (!r.sizeCheck.ok || !r.mainThread.ok || !r.worker.ok || !r.asyncFallback.ok || errors.length) failed = true;
  report.browsers.push(r);
  await browser.close();
}
server.close();

console.log(JSON.stringify(report, null, 2));
process.exit(failed ? 1 : 0);

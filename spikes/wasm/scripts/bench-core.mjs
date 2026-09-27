// Boundary-cost benchmark shared by Node (scripts/bench.mjs) and headless
// Chromium (scripts/chromium-smoke.mjs). Returns rows of
// { case, unit, value, note }.
import { syntheticModels } from './fixtures.mjs';
import { errorFactory } from './checks.mjs';

/** Runs `fn` repeatedly for at least `minMs` (after a warm-up); per-call time. */
function timeit(fn, { minMs = 300, batch = 1 } = {}) {
  const warm = performance.now();
  while (performance.now() - warm < minMs / 3) for (let i = 0; i < batch; i++) fn();
  let n = 0;
  const t0 = performance.now();
  let t;
  do {
    for (let i = 0; i < batch; i++) fn();
    n += batch;
    t = performance.now();
  } while (t - t0 < minMs);
  return (t - t0) / n; // ms per call
}

export function runBench(engine, { metamodelAst, shape = { namespaces: 10, decls: 50, props: 10 } } = {}) {
  const { Engine, noop, add, strLen, makeStr, setErrorFactory, throwSample } = engine;
  const rows = [];
  const perCall = (name, ms, note = '') => rows.push({
    case: name, unit: 'calls/s', value: Math.round(1000 / ms), nsPerCall: Math.round(ms * 1e6), note,
  });
  const once = (name, ms, note = '') => rows.push({ case: name, unit: 'ms', value: +ms.toFixed(3), note });

  // --- raw boundary ----------------------------------------------------------
  const s32 = 'x'.repeat(32);
  const s1k = 'x'.repeat(1024);
  perCall('noop()', timeit(noop, { batch: 1000 }));
  perCall('add(u32, u32) -> u32', timeit(() => add(1, 2), { batch: 1000 }));
  perCall('strLen(32-char string)', timeit(() => strLen(s32), { batch: 1000 }));
  perCall('strLen(1 KiB string)', timeit(() => strLen(s1k), { batch: 1000 }));
  perCall('makeStr(32) -> string', timeit(() => makeStr(32), { batch: 1000 }));
  perCall('makeStr(1024) -> string', timeit(() => makeStr(1024), { batch: 1000 }));
  perCall('JS baseline: read s.length', timeit(() => s32.length, { batch: 1000 }), 'for scale');

  // --- the model set -------------------------------------------------------
  const models = syntheticModels(shape);
  if (metamodelAst) models.push(metamodelAst);
  const strings = models.map((m) => JSON.stringify(m));
  const bytes = strings.reduce((a, s) => a + s.length, 0);
  const declTotal = models.reduce((a, m) => a + m.declarations.length, 0);
  const propTotal = models.reduce((a, m) => a + m.declarations.reduce((b, d) => b + (d.properties?.length ?? 0), 0), 0);
  const setNote = `${models.length} files, ${declTotal} decls, ${propTotal} props, ${(bytes / 1024).toFixed(0)} KiB JSON`;

  // --- coarse: loading a whole model set -----------------------------------
  once('load set: JSON.stringify + addModel(string)', timeit(() => {
    const e = new Engine();
    for (const m of models) e.addModel(JSON.stringify(m));
    e.free();
  }, { minMs: 1000 }), setNote);
  once('load set: addModel(pre-stringified)', timeit(() => {
    const e = new Engine();
    for (const s of strings) e.addModel(s);
    e.free();
  }, { minMs: 1000 }), 'string marshalling + serde_json parse in WASM');
  once('load set: addModelObject(object) [serde-wasm-bindgen]', timeit(() => {
    const e = new Engine();
    for (const m of models) e.addModelObject(m);
    e.free();
  }, { minMs: 1000 }));
  once('JS baseline: JSON.stringify(set)', timeit(() => { for (const m of models) JSON.stringify(m); }, { minMs: 500 }));
  once('new Engine() + free()', timeit(() => new Engine().free(), { minMs: 500 }), 'parses the system model');

  const e = new Engine();
  for (const s of strings) e.addModel(s);
  once('validateModels() on the set', timeit(() => e.validateModels(), { minMs: 1000 }));

  // --- coarse: reading the whole set back ----------------------------------
  const json = e.snapshotJson();
  once('snapshotJson() -> string', timeit(() => e.snapshotJson(), { minMs: 1000 }), `${(json.length / 1024).toFixed(0)} KiB`);
  once('snapshotJson() + JSON.parse', timeit(() => JSON.parse(e.snapshotJson()), { minMs: 1000 }));
  once('snapshotObject() [serde-wasm-bindgen]', timeit(() => e.snapshotObject(), { minMs: 1000 }));

  // --- fine-grained: walking the set one getter at a time ------------------
  const namespaces = JSON.parse(json).map((f) => f.namespace);
  const handles = namespaces.map((ns) => e.namespaceHandle(ns));
  let calls = 0;
  const walkStr = () => {
    let c = 0;
    for (const ns of namespaces) {
      const n = e.declCount(ns); c++;
      for (let i = 0; i < n; i++) {
        e.declName(ns, i); e.declKind(ns, i); c += 2;
        const pn = e.propCount(ns, i); c++;
        for (let j = 0; j < pn; j++) { e.propName(ns, i, j); e.propType(ns, i, j); e.propIsOptional(ns, i, j); c += 3; }
      }
    }
    calls = c;
  };
  const walkHandle = () => {
    for (const h of handles) {
      const n = e.hDeclCount(h);
      for (let i = 0; i < n; i++) {
        e.hDeclName(h, i); e.hDeclKind(h, i);
        const pn = e.hPropCount(h, i);
        for (let j = 0; j < pn; j++) { e.hPropName(h, i, j); e.hPropType(h, i, j); e.hPropIsOptional(h, i, j); }
      }
    }
  };
  const tStr = timeit(walkStr, { minMs: 1000 });
  once('fine walk, namespace-string keys', tStr, `${calls} calls, ${Math.round((tStr * 1e6) / calls)} ns/call`);
  const tH = timeit(walkHandle, { minMs: 1000 });
  once('fine walk, integer handles', tH, `${calls} calls, ${Math.round((tH * 1e6) / calls)} ns/call`);
  once('coarse: snapshotObject() then the same walk in JS', timeit(() => {
    let x = 0;
    for (const f of e.snapshotObject()) for (const d of f.declarations) {
      x += d.name.length + d.kind.length;
      for (const p of d.properties) x += p.name.length + (p.typeName?.length ?? 0) + (p.isOptional ? 1 : 0);
    }
    return x;
  }, { minMs: 1000 }));
  const snap = e.snapshotObject();
  once('JS baseline: same walk over a cached snapshot', timeit(() => {
    let x = 0;
    for (const f of snap) for (const d of f.declarations) {
      x += d.name.length + d.kind.length;
      for (const p of d.properties) x += p.name.length + (p.typeName?.length ?? 0) + (p.isOptional ? 1 : 0);
    }
    return x;
  }, { minMs: 500 }));

  // --- single getters ------------------------------------------------------
  const ns0 = namespaces[0];
  perCall('declName(nsString, i) -> string', timeit(() => e.declName(ns0, 1), { batch: 1000 }));
  perCall('hDeclName(handle, i) -> string', timeit(() => e.hDeclName(handles[0], 1), { batch: 1000 }));
  perCall('hPropIsOptional(handle, i, j) -> bool', timeit(() => e.hPropIsOptional(handles[0], 1, 1), { batch: 1000 }));

  // --- errors ----------------------------------------------------------------
  const throwIt = () => { try { throwSample('IllegalModel'); } catch { /* expected */ } };
  perCall('throw mapped error, strategy A (plain Error)', timeit(throwIt, { batch: 100 }));
  setErrorFactory(errorFactory);
  perCall('throw mapped error, strategy B (JS factory)', timeit(throwIt, { batch: 100 }));
  setErrorFactory(undefined);
  perCall('JS baseline: throw/catch new Error', timeit(() => { try { throw new Error('x'); } catch { /* */ } }, { batch: 100 }));

  e.free();
  return rows;
}

export function formatRows(rows) {
  return rows.map((r) => {
    const v = r.unit === 'calls/s'
      ? `${r.value.toLocaleString('en-US').padStart(13)} calls/s  ${String(r.nsPerCall).padStart(7)} ns`
      : `${r.value.toFixed(3).padStart(13)} ms`;
    return `${r.case.padEnd(54)} ${v}${r.note ? `   ${r.note}` : ''}`;
  }).join('\n');
}

/** Merges several runs of runBench, keeping the best value for each case. */
export function bestOf(runs) {
  return runs[0].map((row, k) => {
    const vals = runs.map((r) => r[k]);
    const best = row.unit === 'calls/s'
      ? vals.reduce((a, b) => (b.value > a.value ? b : a))
      : vals.reduce((a, b) => (b.value < a.value ? b : a));
    return { ...best };
  });
}

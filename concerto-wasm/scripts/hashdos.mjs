// WASM HashDoS check: `node --expose-gc scripts/hashdos.mjs` (`npm run
// smoke:hashdos`, part of `npm run smoke:node`).
//
// On wasm32-unknown-unknown the standard library's `RandomState` has no
// entropy: its keys are derived from memory addresses, the same in every
// instantiation of a build, and the first is bumped by one for each new
// map. JSON parsed into `serde_json::Value` lands in maps hashed that way,
// so anyone with the build can craft object keys that all collide in the
// next map the engine builds. The engine's entry points parse untrusted JSON
// into `concerto_core::json::Value`, whose maps are seeded from
// `crypto.getRandomValues` at instantiation.
//
// The module is the engine built here with the `hashdos-probe` feature
// (src/hashdos_probe.rs) and wasm-bindgen's Node glue: its bindings plus
// `stdKeys`, which crafts keys for the hasher of the next std map (or the
// one `delta` maps later). Each row crafts keys for the map the call under test would build
// for the crafted object if it parsed into `serde_json::Value`, times that
// one call, then times the same call with ordinary keys of the same shape.
// The control row is a std `HashMap`: quadratic, at least CONTROL_MIN
// times the ordinary keys. Every engine row must stay within RATIO times
// the ordinary keys, plus SLACK_MS. With the JSON entry points switched
// back to parsing into `serde_json::Value` (with `preserve_order`), the
// four parsing rows cost 3.4 to 5.2 times the ordinary keys and fail;
// `serializerFromJsonCompact` reads straight into seeded `JsObject`s
// either way. Needs cargo, the wasm32 target and wasm-bindgen-cli, as
// build.sh does; it does not use pkg/.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const crate = path.join(here, '..');
const require = createRequire(import.meta.url);

/**
 * How many keys the crafted object has: as many as a hashbrown table of
 * 2^BITS buckets holds (7/8 of them), so every table the keys grow through
 * is one where they all collide.
 */
const KEYS = 7000;
const BITS = 13;
/**
 * An engine row may cost this many times the ordinary keys, plus SLACK_MS.
 * Parsed into std-hashed maps, the crafted keys cost about four times as
 * much; parsed into seeded ones, about the same.
 */
const RATIO = 2;
const SLACK_MS = 5;
/** The control must cost at least this many times the ordinary keys. */
const CONTROL_MIN = 4;

function targetDir() {
  if (process.env.CARGO_TARGET_DIR) return process.env.CARGO_TARGET_DIR;
  const meta = JSON.parse(execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps'], { cwd: crate }));
  return meta.target_directory;
}

/**
 * Builds the engine for wasm32 with the `hashdos-probe` feature, in its own
 * target directory (build.sh's artifact is left alone), and loads its Node
 * glue.
 */
function loadModule() {
  const target = path.join(targetDir(), 'hashdos-probe');
  execFileSync('cargo', ['build', '--release', '--target', 'wasm32-unknown-unknown', '--features', 'hashdos-probe'],
    { cwd: crate, stdio: ['ignore', 'ignore', 'inherit'], env: { ...process.env, CARGO_TARGET_DIR: target } });
  const wasm = path.join(target, 'wasm32-unknown-unknown', 'release', 'concerto_wasm.wasm');
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'concerto-hashdos-'));
  execFileSync('wasm-bindgen', ['--target', 'nodejs', '--out-dir', out, wasm], { stdio: 'inherit' });
  return require(path.join(out, 'concerto_wasm.js'));
}

/** `f()`'s duration in milliseconds. */
function time(f) {
  const t0 = performance.now();
  f();
  return performance.now() - t0;
}

const MM = 'concerto.metamodel@1.0.0';
/** A concept with one `Map<String, String>` field. */
const MODEL = {
  $class: `${MM}.Model`,
  namespace: 'org.hashdos@1.0.0',
  imports: [],
  declarations: [
    {
      $class: `${MM}.ConceptDeclaration`, name: 'Bag', isAbstract: false,
      properties: [{
        $class: `${MM}.ObjectProperty`, name: 'entries', isArray: false, isOptional: false,
        type: { $class: `${MM}.TypeIdentifier`, name: 'Entries' },
      }],
    },
    {
      $class: `${MM}.MapDeclaration`, name: 'Entries',
      key: { $class: `${MM}.StringMapKeyType` },
      value: { $class: `${MM}.StringMapValueType` },
    },
  ],
};

const engine = loadModule();
engine.setHost((payload) => Object.assign(new Error(payload.message), { payload }));
const mm = new engine.ModelManagerHandle();
mm.addModel(JSON.stringify(MODEL), 'hashdos.cto');
const env = { newId: () => 'id', nowMs: () => 0 };

const object = (keys) => Object.fromEntries(keys.map((k) => [k, 'v']));
/** `f`, with any error it throws swallowed: only its duration matters. */
const attempt = (f) => { try { f(); } catch { /* not what is timed */ } };

// Each subject: `delta`, the maps `serde_json` would build in the call
// before the crafted object's (an object's map is built once its first key
// is read, so an enclosing object's comes first), and `prepare`, which
// builds the call's argument from the keys and returns the call alone.
const instanceText = (keys) => JSON.stringify({ $class: 'org.hashdos@1.0.0.Bag', entries: object(keys) });
const astText = (keys) => JSON.stringify({ $class: `${MM}.Model`, ...object(keys) });
const subjects = {
  'checkAstShape (model AST)': {
    delta: 0, prepare: (keys) => { const text = astText(keys); return () => attempt(() => engine.checkAstShape(text)); },
  },
  'validateAstValue (model AST)': {
    delta: 0, prepare: (keys) => { const text = astText(keys); return () => attempt(() => mm.validateAstValue(text)); },
  },
  'dcsValidate (decorator command set)': {
    delta: 0,
    prepare: (keys) => {
      const commandSet = { $class: 'org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet', ...object(keys) };
      return () => attempt(() => mm.dcsValidate(commandSet));
    },
  },
  'validateInstance (Map entries, all diagnostics)': {
    delta: 1,
    prepare: (keys) => {
      const text = instanceText(keys);
      return () => {
        const out = JSON.parse(mm.validateInstance(text, 'null', undefined, 2));
        if (out.diagnostics.length) throw new Error(`diagnostics ${JSON.stringify(out.diagnostics[0])}`);
      };
    },
  },
  'serializerFromJsonCompact (fromJSON wire, Map entries)': {
    delta: 1,
    prepare: (keys) => { const text = instanceText(keys); return () => mm.serializerFromJsonCompact(text, 'null', env); },
  },
};

/** `f()`'s duration in milliseconds, after a collection when `gc` is exposed. */
function timed(f) {
  globalThis.gc?.();
  return time(f);
}
/** The fastest of three runs. */
const best = (f) => Math.min(timed(f), timed(f), timed(f));

const ordinary = engine.ordinaryKeys(KEYS);
const rows = [];
{
  const crafted = engine.stdKeys(KEYS, BITS, 0);
  globalThis.gc?.();
  const ms = { crafted: time(() => engine.stdInsert(crafted)), ordinary: best(() => engine.stdInsert(ordinary)) };
  rows.push({
    name: `control: ${KEYS} keys crafted for the next std map are quadratic in a std HashMap`,
    ok: ms.crafted >= CONTROL_MIN * ms.ordinary,
    detail: { ms, ratio: +(ms.crafted / ms.ordinary).toFixed(1) },
  });
}
for (const [name, { delta, prepare }] of Object.entries(subjects)) {
  const row = { name: `${name}: keys crafted for the std hasher cost about what ordinary keys do` };
  try {
    const plain = prepare(ordinary);
    plain(); // warm up, and surface a broken subject before timing it
    const crafted = prepare(engine.stdKeys(KEYS, BITS, delta));
    // Run at once after crafting: no std map may be built in between.
    const ms = { crafted: timed(crafted), ordinary: best(plain) };
    row.ok = ms.crafted <= RATIO * ms.ordinary + SLACK_MS;
    row.detail = { ms, ratio: +(ms.crafted / ms.ordinary).toFixed(2) };
  } catch (e) {
    row.ok = false;
    row.detail = String(e?.stack ?? e);
  }
  rows.push(row);
}

console.log(JSON.stringify({ runtime: `node ${process.version}`, keys: KEYS, rows }, null, 2));
process.exit(rows.every((r) => r.ok) ? 0 : 1);

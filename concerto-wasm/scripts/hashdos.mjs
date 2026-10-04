// WASM HashDoS check: `node scripts/hashdos.mjs [module]`, after build.sh.
//
// On wasm32-unknown-unknown the standard library's `RandomState` has no
// entropy: its keys are derived from memory addresses, the same in every
// instantiation of a build. JSON parsed into `serde_json::Value` lands in
// maps hashed that way, so anyone with the build can craft object keys that
// all collide. The engine's entry points parse untrusted JSON into
// `concerto_core::json::Value`, whose maps are seeded from
// `crypto.getRandomValues` at instantiation.
//
// The example module `hashdos_keys` (examples/hashdos_keys.rs, built here
// for wasm32 with wasm-bindgen's Node glue) crafts keys whose hashes under
// one of its own fixed-key `RandomState`s share their low bits. The control
// row shows those keys are quadratic in a std map with that hasher. The
// other rows hand the same keys, and ordinary keys of the same shape, to
// the engine's entry points (pkg/concerto-engine.cjs, or `module`), and
// require the crafted ones to cost about the same: within RATIO times the
// ordinary ones, plus SLACK.
//
// The keys target the example's own hasher. Each std map in a build gets
// keys one apart (the counter is bumped per map), which someone with the
// build can compute offline but this check cannot recover from outside the
// standard library; so the rows show the entry points stay linear on keys
// that defeat the fixed-key hasher, and the type (`concerto_core::json`)
// is what keeps every parsed object off that hasher.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const crate = path.join(here, '..');
const require = createRequire(import.meta.url);

/** How many object keys each document has. */
const KEYS = 6000;
/** hashbrown's bucket count for KEYS entries is 2^BITS (KEYS * 8 / 7, rounded up). */
const BITS = 13;
/** The crafted keys may cost this many times the ordinary ones, plus SLACK. */
const RATIO = 4;
const SLACK_MS = 30;
/** The control must show at least this ratio, or the keys no longer collide. */
const CONTROL_MIN = 4;

function targetDir() {
  if (process.env.CARGO_TARGET_DIR) return process.env.CARGO_TARGET_DIR;
  const meta = JSON.parse(execFileSync('cargo', ['metadata', '--format-version', '1', '--no-deps'], { cwd: crate }));
  return meta.target_directory;
}

/** Builds the example for wasm32 and loads its Node glue. */
function loadKeySource() {
  execFileSync('cargo', ['build', '--release', '--example', 'hashdos_keys', '--target', 'wasm32-unknown-unknown'],
    { cwd: crate, stdio: ['ignore', 'ignore', 'inherit'] });
  const wasm = path.join(targetDir(), 'wasm32-unknown-unknown', 'release', 'examples', 'hashdos_keys.wasm');
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'concerto-hashdos-'));
  execFileSync('wasm-bindgen', ['--target', 'nodejs', '--out-dir', out, wasm], { stdio: 'inherit' });
  return require(path.join(out, 'hashdos_keys.js'));
}

/** The fastest of three runs of `f`, in milliseconds. */
function time(f) {
  let best = Infinity;
  for (let i = 0; i < 3; i++) {
    const t0 = performance.now();
    f();
    best = Math.min(best, performance.now() - t0);
  }
  return best;
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

const keySource = loadKeySource();
const target = process.argv[2] ?? path.join(crate, 'pkg/concerto-engine.cjs');
const engine = require(require.resolve(target, { paths: [process.cwd()] }));
await engine.init();
engine.setHost((payload) => Object.assign(new Error(payload.message), { payload }));

const colliding = keySource.stdKeys(KEYS, BITS, true);
const ordinary = keySource.stdKeys(KEYS, BITS, false);
const entries = (keys) => Object.fromEntries(keys.map((k) => [k, 'v']));
const docs = (keys) => {
  const object = entries(keys);
  return {
    instance: JSON.stringify({ $class: 'org.hashdos@1.0.0.Bag', entries: object }),
    ast: JSON.stringify({ ...MODEL, declarations: [{ ...MODEL.declarations[0], ...object }] }),
    commandSet: {
      $class: 'org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet',
      name: 'hashdos', version: '1.0.0', commands: [], ...object,
    },
  };
};
const crafted = docs(colliding);
const plain = docs(ordinary);

const mm = new engine.ModelManagerHandle();
mm.addModel(JSON.stringify(MODEL), 'hashdos.cto');
const env = { newId: () => 'id', nowMs: () => 0 };
const attempt = (f) => () => { try { f(); } catch { /* the error is not what is timed */ } };

const subjects = {
  'validateInstance (all diagnostics)': (d) => () => {
    const out = JSON.parse(mm.validateInstance(d.instance, 'null', undefined, 2));
    if (out.diagnostics.length) throw new Error(`diagnostics ${JSON.stringify(out.diagnostics[0])}`);
  },
  'serializerFromJsonCompact (fromJSON wire)': (d) => () => { mm.serializerFromJsonCompact(d.instance, 'null', env); },
  'checkAstShape (model AST)': (d) => attempt(() => engine.checkAstShape(d.ast)),
  'validateAstValue (model AST)': (d) => attempt(() => mm.validateAstValue(d.ast)),
  'dcsValidate (decorator command set)': (d) => attempt(() => mm.dcsValidate(d.commandSet)),
};

const rows = [];
const control = { colliding: time(() => keySource.stdInsert(colliding)), ordinary: time(() => keySource.stdInsert(ordinary)) };
rows.push({
  name: `control: ${KEYS} crafted keys are quadratic in a std HashMap with the fixed-key hasher they target`,
  ok: control.colliding >= CONTROL_MIN * control.ordinary,
  detail: { ms: control, ratio: +(control.colliding / control.ordinary).toFixed(1) },
});
for (const [name, run] of Object.entries(subjects)) {
  const row = { name: `${name}: crafted keys cost about what ordinary keys do` };
  try {
    run(plain)(); // warm up, and surface a broken subject before timing it
    const ms = { colliding: time(run(crafted)), ordinary: time(run(plain)) };
    row.ok = ms.colliding <= RATIO * ms.ordinary + SLACK_MS;
    row.detail = { ms, ratio: +(ms.colliding / ms.ordinary).toFixed(2) };
  } catch (e) {
    row.ok = false;
    row.detail = String(e?.stack ?? e);
  }
  rows.push(row);
}

console.log(JSON.stringify({ runtime: `node ${process.version}`, module: target, keys: KEYS, rows }, null, 2));
process.exit(rows.every((r) => r.ok) ? 0 : 1);

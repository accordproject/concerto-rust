// Cold-load cost of the inlined-bytes loaders: each run is a fresh Node
// process, so nothing is cached between runs. Reports medians.
//
//   node scripts/load-bench.mjs [path/to/node ...]   (default: this node)
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const RUNS = 9;
const dist = fileURLToPath(new URL('../dist/', import.meta.url));
const nodes = process.argv.slice(2).length ? process.argv.slice(2) : [process.execPath];

const esm = `
const t0 = performance.now();
const m = await import(${JSON.stringify(`${dist}concerto-engine.mjs`)});
const t1 = performance.now();
new m.Engine().free();
const t2 = performance.now();
console.log(JSON.stringify({ load: t1 - t0, firstEngine: t2 - t1, ...m.loadTimings }));`;
const cjs = `
const t0 = performance.now();
const m = require(${JSON.stringify(`${dist}concerto-engine.cjs`)});
const t1 = performance.now();
new m.Engine().free();
const t2 = performance.now();
console.log(JSON.stringify({ load: t1 - t0, firstEngine: t2 - t1 }));`;

const median = (xs) => { const s = [...xs].sort((a, b) => a - b); return s[s.length >> 1]; };
const fmt = (x) => (x === undefined ? '-' : x.toFixed(1));

console.log('node      loader  load ms  decode  compile  instantiate  first Engine  decoder');
for (const node of nodes) {
  const version = execFileSync(node, ['--version']).toString().trim();
  for (const [kind, code, flags] of [['ESM', esm, ['--input-type=module']], ['CJS', cjs, []]]) {
    const runs = [];
    for (let i = 0; i < RUNS; i++) {
      runs.push(JSON.parse(execFileSync(node, [...flags, '-e', code]).toString()));
    }
    const m = (k) => (runs[0][k] === undefined ? undefined : median(runs.map((r) => r[k])));
    console.log(`${version.padEnd(9)} ${kind.padEnd(6)} ${fmt(m('load')).padStart(8)} ${fmt(m('decodeMs')).padStart(7)} ${fmt(m('compileMs')).padStart(8)} ${fmt(m('instantiateMs')).padStart(12)} ${fmt(m('firstEngine')).padStart(13)}  ${runs[0].decoder ?? 'Buffer'}`);
  }
}

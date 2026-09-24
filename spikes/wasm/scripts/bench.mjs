// Node boundary-cost benchmark: `node scripts/bench.mjs [--runs N] [--json]`.
//
// Runs the whole suite N times (default 5) and keeps the best result for each
// case. Best-of is the most stable statistic on a shared, loaded machine.
import { readFileSync } from 'node:fs';
import * as engine from '../dist/concerto-engine.mjs';
import { runBench, formatRows, bestOf } from './bench-core.mjs';

const i = process.argv.indexOf('--runs');
const runs = i > 0 ? Number(process.argv[i + 1]) : 5;
const metamodelAst = JSON.parse(readFileSync(new URL('../fixtures/metamodel.json', import.meta.url), 'utf8'));
const all = [];
for (let r = 0; r < runs; r++) all.push(runBench(engine, { metamodelAst }));
const rows = bestOf(all);
if (process.argv.includes('--json')) console.log(JSON.stringify(rows, null, 2));
else console.log(`node ${process.version}, best of ${runs}\n${formatRows(rows)}`);

#!/usr/bin/env node
// P5-81 (accordproject/concerto-rust#425): the size tables of the spike
// report, from a sizes/ directory that build-wasm.sh filled:
// <variant>-<o3|z>/shipped.wasm, and <variant>-<o3|z>-v0/named.wasm for the
// variants with a breakdown.
//
//   node compare.mjs <sizes dir> [--json]
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { sizes, breakdown, AREAS } from './wasmsize.mjs';

const [dir, ...rest] = process.argv.slice(2);
const VARIANTS = ['base', 'derived', 'table', 'altvalue', 'altdeny'];
const out = {};
for (const opt of ['o3', 'z']) {
    for (const v of VARIANTS) {
        const shipped = join(dir, `${v}-${opt}`, 'shipped.wasm');
        if (!existsSync(shipped)) continue;
        const named = join(dir, `${v}-${opt}-v0`, 'named.wasm');
        out[`${v}-${opt}`] = {
            shipped: sizes(readFileSync(shipped)),
            named: existsSync(named) ? breakdown(readFileSync(named)) : null,
        };
    }
}
if (rest.includes('--json')) {
    console.log(JSON.stringify(out, null, 1));
    process.exit(0);
}
const kb = (b) => (b / 1000).toFixed(1);
const pc = (a, b) => `${a >= b ? '+' : ''}${(100 * (a - b) / b).toFixed(1)}%`;
for (const opt of ['o3', 'z']) {
    const ref = out[`derived-${opt}`];
    console.log(`\n### ${opt}: shipped .wasm (KB = 1000 B)\n`);
    console.log('| variant | raw | gzip | brotli | vs derived raw / gz / br |');
    console.log('|---|---:|---:|---:|---|');
    for (const v of VARIANTS) {
        const x = out[`${v}-${opt}`];
        if (!x) continue;
        const s = x.shipped;
        console.log(`| ${v} | ${kb(s.raw)} | ${kb(s.gzip)} | ${kb(s.brotli)} | ${ref ? `${pc(s.raw, ref.shipped.raw)} / ${pc(s.gzip, ref.shipped.gzip)} / ${pc(s.brotli, ref.shipped.brotli)}` : '-'} |`);
    }
    const d = out[`derived-${opt}`]?.named;
    const t = out[`table-${opt}`]?.named;
    const b = out[`base-${opt}`]?.named;
    if (d && t) {
        console.log(`\n### ${opt}: code bytes by area (named v0 build, KB)\n`);
        console.log('| area | base | derived | table | table - derived |');
        console.log('|---|---:|---:|---:|---:|');
        for (const [a] of AREAS) {
            console.log(`| ${a} | ${b ? kb(b.areas[a]) : '-'} | ${kb(d.areas[a])} | ${kb(t.areas[a])} | ${kb(t.areas[a] - d.areas[a])} |`);
        }
        console.log(`| code section | ${b ? kb(b.codeBytes) : '-'} | ${kb(d.codeBytes)} | ${kb(t.codeBytes)} | ${kb(t.codeBytes - d.codeBytes)} |`);
        console.log(`| data section | ${b ? kb(b.dataBytes) : '-'} | ${kb(d.dataBytes)} | ${kb(t.dataBytes)} | ${kb(t.dataBytes - d.dataBytes)} |`);
        console.log(`| functions | ${b ? b.functions : '-'} | ${d.functions} | ${t.functions} | ${t.functions - d.functions} |`);
    }
}

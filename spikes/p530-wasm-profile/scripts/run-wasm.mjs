#!/usr/bin/env node
// P5-30 (accordproject/concerto-rust#335): the WASM half. Times each stage
// of the extractDecorators binding body inside WASM (see src/lib.rs), over
// the same input text the native half (`p530-native`) reads.
//
//   node scripts/run-wasm.mjs --pkg <dir with p530_wasm_profile.js> --input <models.json>
//       [--engine <concerto-engine.cjs>] [--iters N] [--warmup N]
//       [--loop all|extract|rebuild --seconds S]   (for node --cpu-prof)
//
// --pkg     a `wasm-bindgen --target nodejs` output directory of this crate.
// --engine  also time the shipped binding, decoratorManagerExtractDecorators,
//           end to end, on the same input (JS object in, JS object out).
//
// Prints one JSON object with the median and min per stage in microseconds.

import fs from 'fs';
import path from 'path';
import { createRequire } from 'module';

const require = createRequire(import.meta.url);
const a = { iters: 30, warmup: 5, loop: null, seconds: 10 };
for (let i = 2; i < process.argv.length; i++) {
    const k = process.argv[i];
    const v = () => process.argv[++i];
    if (k === '--pkg') { a.pkg = path.resolve(v()); }
    else if (k === '--input') { a.input = v(); }
    else if (k === '--engine') { a.engine = path.resolve(v()); }
    else if (k === '--iters') { a.iters = Number(v()); }
    else if (k === '--warmup') { a.warmup = Number(v()); }
    else if (k === '--loop') { a.loop = v(); }
    else if (k === '--seconds') { a.seconds = Number(v()); }
    else { throw new Error(`unknown argument ${k}`); }
}

const text = fs.readFileSync(a.input, 'utf8');
const m = a.pkg ? require(path.join(a.pkg, 'p530_wasm_profile.js')) : null;
const opts = { removeDecoratorsFromModel: true, locale: 'en' };
const now = () => process.hrtime.bigint();
const us = (t0) => Number(process.hrtime.bigint() - t0) / 1000;

if (a.loop) {
    const end = Date.now() + a.seconds * 1000;
    const engine = a.engine ? require(a.engine) : null;
    const models = JSON.parse(text);
    // Named so the profile summary can keep only the samples under it.
    const p530MeasuredLoop = () => {
        let calls = 0;
        while (Date.now() < end) {
            if (a.loop === 'engine') {
                engine.decoratorManagerExtractDecorators(models, opts);
            } else if (a.loop === 'all') {
                JSON.parse(m.p530_all(text));
            } else {
                m.p530_set_input(text);
                m.p530_parse();
                m.p530_rebuild();
                if (a.loop !== 'rebuild') {
                    m.p530_extract();
                    m.p530_encode();
                }
                m.p530_drop();
            }
            calls++;
        }
        return calls;
    };
    console.error(`loop ${a.loop}: ${p530MeasuredLoop()} calls`);
    process.exit(0);
}

const median = (v) => [...v].sort((x, y) => x - y)[Math.floor(v.length / 2)];
const out = { node: process.version, execArgv: process.execArgv };
const series = {};
const push = (k, v) => (series[k] = series[k] || []).push(v);

if (m) {
    for (let i = 0; i < a.warmup + a.iters; i++) {
        const rec = i >= a.warmup;
        let t = now(); m.p530_set_input(text); const copyIn = us(t);
        t = now(); m.p530_parse(); const parse = us(t);
        t = now(); m.p530_rebuild(); const rebuild = us(t);
        t = now(); m.p530_extract(); const extract = us(t);
        t = now(); m.p530_encode(); const encode = us(t);
        t = now(); const s = m.p530_take_output(); const copyOut = us(t);
        t = now(); JSON.parse(s); const jsParse = us(t);
        t = now(); m.p530_drop(); const drop = us(t);
        if (rec) {
            push('copyIn', copyIn); push('parse', parse); push('rebuild', rebuild);
            push('extract', extract); push('encode', encode); push('copyOut', copyOut);
            push('jsParse', jsParse); push('drop', drop);
            push('wasmStagesTotal', parse + rebuild + extract + encode + drop);
        }
    }
    for (let i = 0; i < a.warmup + a.iters; i++) {
        const t = now(); JSON.parse(m.p530_all(text)); const all = us(t);
        if (i >= a.warmup) { push('allOneCall', all); }
    }
    out.memoryBytes = m.p530_memory_bytes();
    const micro = (f, units) => {
        const v = [];
        for (let i = 0; i < 15; i++) { const t = now(); f(); v.push((us(t) * 1000) / units); }
        return median(v);
    };
    out.microAllocNsPerRound = micro(() => m.p530_micro_alloc(100000), 100000);
    out.microSipHashNsPerKey = micro(() => m.p530_micro_siphash(1000000), 1000000);
    out.microMemcpyNsPer4KiB = micro(() => m.p530_micro_memcpy(100000), 100000);
}

if (a.engine) {
    const engine = require(a.engine);
    const models = JSON.parse(text);
    for (let i = 0; i < a.warmup + a.iters; i++) {
        let t = now(); JSON.stringify(models); const stringify = us(t);
        t = now(); engine.decoratorManagerExtractDecorators(models, opts); const e2e = us(t);
        if (i >= a.warmup) { push('engineJsonStringify', stringify); push('engineBinding', e2e); }
    }
}

for (const [k, v] of Object.entries(series)) {
    out[k] = { medianUs: Number(median(v).toFixed(1)), minUs: Number(Math.min(...v).toFixed(1)) };
}
console.log(JSON.stringify(out));

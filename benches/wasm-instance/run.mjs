// Times the in-WASM workload 3 bench (see README.md) under Node and writes a
// JSON summary: per route, 5 warm-up passes and 30 timed passes over the 500
// instances, reported per instance, as migration/bench/run-ts.mjs does.
//
// Usage: node run.mjs [--out <file>] [--label <name>] [--samples N] [--warmup N]
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const bench = require(path.join(here, 'pkg', 'concerto_bench_wasm_instance.js'));

const args = process.argv.slice(2);
const opt = (name, dflt) => {
    const i = args.indexOf(`--${name}`);
    return i >= 0 ? args[i + 1] : dflt;
};
const warmup = Number(opt('warmup', 5));
const samples = Number(opt('samples', 30));

function median(v) {
    const s = [...v].sort((a, b) => a - b);
    const m = Math.floor(s.length / 2);
    return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}
function summarise(ms, n) {
    const us = ms.map((t) => (t * 1000) / n);
    const mean = us.reduce((a, b) => a + b, 0) / us.length;
    const sd = Math.sqrt(us.reduce((a, b) => a + (b - mean) ** 2, 0) / (us.length - 1));
    return { samples: us.length, n, median_us: median(us), mean_us: mean, stddev_us: sd, min_us: Math.min(...us), max_us: Math.max(...us), cv: sd / mean };
}
function time(fn, n) {
    for (let i = 0; i < warmup; i++) fn();
    const ms = [];
    for (let i = 0; i < samples; i++) {
        const t0 = process.hrtime.bigint();
        fn();
        ms.push(Number(process.hrtime.bigint() - t0) / 1e6);
    }
    return summarise(ms, n);
}

let commit = null;
try {
    commit = execFileSync('git', ['-C', here, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
} catch { /* not a checkout */ }

const loadStart = os.loadavg();
const n = bench.setup();
const results = {
    label: opt('label', null),
    concerto_rust_commit: commit,
    machine: { platform: os.platform(), arch: os.arch(), cpus: os.cpus()?.[0]?.model, cpu_count: os.cpus()?.length, node: process.version },
    timestamp: new Date().toISOString(),
    warmup,
    loadavg_start: loadStart,
    workloads: {
        validate_only: time(() => bench.runValidateOnly(), n),
        from_json: time(() => bench.runFromJson(), n),
        validate_instance_native: time(() => bench.runValidateInstanceNative(), n),
    },
    loadavg_end: os.loadavg(),
};
const out = opt('out', null);
if (out) writeFileSync(out, JSON.stringify(results, null, 2) + '\n');
for (const [k, v] of Object.entries(results.workloads)) {
    console.log(`${k.padEnd(26)} median ${v.median_us.toFixed(2)} µs/instance (cv ${(v.cv * 100).toFixed(1)}%)`);
}

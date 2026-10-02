#!/usr/bin/env node
// P5-81 (accordproject/concerto-rust#425): collects the model ASTs the
// table-decoder equivalence test (concerto-core `introspect::table_equiv`)
// reads, as JSON lines `{"source": ..., "text": <AST as JSON text>}`, one
// per distinct AST text:
//   - oracle: every `concerto.metamodel@1.0.0.Model` object found at any
//     depth in the oracle corpus fixtures (pinned corpus plus supplement),
//     and every AST in its cto-cache;
//   - conformance: every .cto in the concerto-conformance checkout, parsed
//     by the reference concerto-cto 5.0.0 (with and without locations);
//   - core-test: every .cto under concerto-core's test/ (parsed the same
//     way) and every JSON file there that holds a model AST.
// A .cto the 5.0.0 parser rejects is skipped (it has no AST to decode).
//
//   node collect-models.mjs <concerto checkout> <conformance checkout> <out.jsonl>
import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createRequire } from 'node:module';

const [concerto, conformance, out] = process.argv.slice(2);
const require = createRequire(join(concerto, 'migration/oracle/reference/package.json'));
const { Parser } = require('@accordproject/concerto-cto');

const MODEL = 'concerto.metamodel@1.0.0.Model';
const seen = new Set();
const lines = [];
const counts = {};
function add(source, ast) {
    const text = JSON.stringify(ast);
    if (seen.has(text)) return;
    seen.add(text);
    counts[source] = (counts[source] || 0) + 1;
    lines.push(JSON.stringify({ source, text }));
}

function* walk(dir, pred) {
    for (const name of readdirSync(dir)) {
        if (name === 'node_modules' || name === '.git') continue;
        const p = join(dir, name);
        const st = statSync(p);
        if (st.isDirectory()) yield* walk(p, pred);
        else if (pred(p)) yield p;
    }
}

function findModels(value, source) {
    if (Array.isArray(value)) { for (const v of value) findModels(v, source); return; }
    if (value && typeof value === 'object') {
        if (value.$class === MODEL) add(source, value);
        for (const v of Object.values(value)) findModels(v, source);
    }
}

function parseCto(path, source) {
    const text = readFileSync(path, 'utf8');
    for (const skipLocationNodes of [false, true]) {
        try {
            add(source, Parser.parse(text, path, { skipLocationNodes }));
        } catch { /* not a 5.0.0 model */ }
    }
}

const fixtures = join(concerto, 'migration/oracle/fixtures');
for (const p of walk(fixtures, p => p.endsWith('.json'))) {
    findModels(JSON.parse(readFileSync(p, 'utf8')), 'oracle');
}
for (const p of walk(join(concerto, 'migration/oracle/cto-cache'), p => p.endsWith('.json'))) {
    findModels(JSON.parse(readFileSync(p, 'utf8')), 'oracle');
}
for (const p of walk(conformance, p => p.endsWith('.cto'))) parseCto(p, 'conformance');
const coreTest = join(concerto, 'packages/concerto-core/test');
for (const p of walk(coreTest, p => p.endsWith('.cto'))) parseCto(p, 'core-test');
for (const p of walk(coreTest, p => p.endsWith('.json'))) {
    try { findModels(JSON.parse(readFileSync(p, 'utf8')), 'core-test'); } catch { /* not JSON */ }
}
writeFileSync(out, lines.join('\n') + '\n');
console.error(JSON.stringify({ distinct: lines.length, bySource: counts }));

#!/usr/bin/env node
// P5-81 (accordproject/concerto-rust#425): WASM size and the per-area
// breakdown the spike reports, P5-39/P5-48 method.
//
//   node wasmsize.mjs <shipped.wasm> <named.wasm> [<label>]
//
// <shipped.wasm>: names stripped, as build.sh ships it: raw, gzip -9 and
// brotli q11. <named.wasm>: the same engine built with v0 symbol mangling
// (RUSTFLAGS="-C symbol-mangling-version=v0") and wasm-opt -g, so every
// function's name carries its generic arguments and a monomorphisation can
// be charged to the type it was instantiated for. A function goes to the
// first area that matches:
//   typed-AST deserialisation: the typed metamodel read (typed_ast, kept,
//     the table decoder) and every deserialisation instance for a generated
//     metamodel type (concerto.metamodel@1.0.0);
//   other serde deserialisation: every other Deserialize/Visitor/Access
//     instance and serde_json's parser (instances, DCS, vocabulary, the
//     wire format, the binding's own view structs);
//   serde serialisation;
//   regress;
//   everything else (split: concerto_core, concerto_wasm, concerto_core_js,
//     Debug/Display, drop glue, other).
// Prints JSON.
import { readFileSync } from 'node:fs';
import zlib from 'node:zlib';
import { functions } from './funcs.mjs';

export function sizes(buf) {
    return {
        raw: buf.length,
        gzip: zlib.gzipSync(buf, { level: 9 }).length,
        brotli: zlib.brotliCompressSync(buf, {
            params: {
                [zlib.constants.BROTLI_PARAM_QUALITY]: 11,
                [zlib.constants.BROTLI_PARAM_SIZE_HINT]: buf.length,
            },
        }).length,
    };
}

const DE = /(serde_core\S*::de::|Deserializ|Visitor|MapAccess|SeqAccess|EnumAccess|deserialize|serde_json\S*::de::|serde_json\S*::read::)/;
const TYPED = /(introspect::typed_ast|introspect::kept|concerto_metamodel\S*::table)/;
const MM = /concerto_metamodel\S*::concerto_metamodel_1_0_0/;

export const AREAS = [
    ['typed-AST deserialisation', n => TYPED.test(n) || (MM.test(n) && DE.test(n))],
    ['other serde deserialisation', n => DE.test(n)],
    ['serde serialisation', n => /(serde_core\S*::ser::|Serialize|serialize|serde_json\S*::ser::)/.test(n)],
    ['regress', n => /regress/.test(n)],
    ['else: concerto_core', n => /concerto_core\[/.test(n) || /concerto_core::/.test(n)],
    ['else: concerto_wasm', n => /concerto_wasm/.test(n) || / shim$/.test(n)],
    ['else: concerto_core_js', n => /concerto_core_js/.test(n)],
    ['else: Debug/Display', n => /core\S*::fmt::(Debug|Display)/.test(n)],
    ['else: drop glue', n => /drop_in_place/.test(n)],
    ['else: other', () => true],
];

export function area(name) {
    return AREAS.find(([, test]) => test(name))[0];
}

export function breakdown(buf) {
    const { fns, codeBytes, dataBytes } = functions(buf);
    const areas = Object.fromEntries(AREAS.map(([k]) => [k, 0]));
    for (const f of fns) areas[area(f.name)] += f.bytes;
    return { codeBytes, dataBytes, functions: fns.length, areas };
}

if (process.argv[1] && process.argv[1].endsWith('wasmsize.mjs')) {
    const [shipped, named, label] = process.argv.slice(2);
    const out = { label: label || shipped, shipped: sizes(readFileSync(shipped)) };
    if (named) out.named = breakdown(readFileSync(named));
    console.log(JSON.stringify(out, null, 2));
}

#!/usr/bin/env node
// P5-30: self time (or self instructions) by cause, for a symbolised WASM
// V8 CPU profile or a native callgrind profile of the same work, so the two
// sides can be put next to each other.
//
//   node scripts/summarize.mjs cpuprof <file.cpuprofile> [--top N]
//   node scripts/summarize.mjs callgrind <callgrind_annotate output> [--top N]
//
// For a cpuprofile only samples under `p530MeasuredLoop` (run-wasm.mjs
// --loop) count, and every sample goes to its leaf frame. For callgrind,
// the input is `callgrind_annotate --inclusive=no --threshold=100` text.
// Prints JSON: the share of each cause, and the top frames.

import fs from 'fs';

const [kind, file] = process.argv.slice(2, 4);
const topN = Number(process.argv[process.argv.indexOf('--top') + 1]) || 40;

// Order matters: the first matching rule wins.
const CAUSES = [
    ['alloc', /dlmalloc|__rust_alloc|__rust_dealloc|__rust_realloc|__rdl_|__rg_|alloc::alloc::|RawVec|raw_vec|finish_grow|reserve|do_reserve|_int_malloc|_int_free|malloc|cfree|\bfree\b|realloc|unlink_chunk|tcache|sysmalloc|malloc_consolidate|talc|TalckWasm|GlobalDlmalloc/],
    ['memcpy/memset/memcmp', /memcpy|memmove|memset|memcmp|bcmp|compiler_builtins::mem|__memcpy|__memmove|__memset|__memcmp|strlen/],
    ['hash + map probe (SipHash, Fx, indexmap, hashbrown)', /indexmap::map::IndexMap.*(insert_full|get_index_of|get|entry|swap_remove|shift_remove)|indexmap::map::core|sip|SipHasher|Hasher|core::hash|hash_one|BuildHasher|RandomState|FxHash|rustc_hash|indexmap::map::core::.*(hash|find|get_index_of|insert_full)|hashbrown/],
    ['json parse (serde_json::de)', /serde_json::de|Deserializer|parse_str|parse_whitespace|parse_ident|parse_integer|parse_number|parse_decimal|parse_any|deserialize_any|visit_map|MapAccess|SeqAccess|next_key_seed|next_value_seed|serde_json::read/],
    ['json encode (serde_json::ser)', /serde_json::ser|format_escaped_str|Serialize for serde_json|serialize_map|serialize_entry|to_writer|to_string|itoa|ryu|zmij/],
    ['Value clone', /as core::clone::Clone>::clone|Clone.*Value|Value.*clone|clone::Clone|to_owned|to_vec/],
    ['drop', /drop_in_place|core::ptr::drop|as core::ops::drop::Drop>::drop/],
    ['formatting (core::fmt)', /core::fmt|alloc::fmt|fmt::write|Formatter|format_inner|pad_integral|write_str|write_fmt/],
    ['regex (regress)', /regress/],
    ['concerto-core', /concerto_core|concerto_metamodel|concerto_core_js/],
    ['serde_json::Value other', /serde_json/],
    ['std other', /core::|alloc::|std::/],
];

function cause(name) {
    for (const [c, re] of CAUSES) {
        if (re.test(name)) {
            return c;
        }
    }
    return 'other';
}

const causes = {};
const frames = {};
let total = 0;
function add(name, w) {
    const c = cause(name);
    causes[c] = (causes[c] || 0) + w;
    frames[name] = (frames[name] || 0) + w;
    total += w;
}

if (kind === 'cpuprof') {
    const prof = JSON.parse(fs.readFileSync(file, 'utf8'));
    const byId = new Map(prof.nodes.map((n) => [n.id, n]));
    const parent = new Map();
    for (const n of prof.nodes) {
        for (const c of n.children || []) {
            parent.set(c, n.id);
        }
    }
    const measured = new Map();
    const isMeasured = (id) => {
        if (measured.has(id)) {
            return measured.get(id);
        }
        const n = byId.get(id);
        const r = n.callFrame.functionName === 'p530MeasuredLoop' ? true : parent.has(id) ? isMeasured(parent.get(id)) : false;
        measured.set(id, r);
        return r;
    };
    prof.samples.forEach((id, i) => {
        const dt = prof.timeDeltas[i] || 0;
        const f = byId.get(id).callFrame;
        if (f.functionName === '(garbage collector)') {
            add('(js gc)', dt);
            return;
        }
        if (!isMeasured(id)) {
            return;
        }
        const where = f.url.startsWith('wasm://') ? '' : ` [js ${f.url.split('/').pop()}]`;
        add(`${f.functionName || '(anon)'}${where}`, dt);
    });
} else if (kind === 'callgrind') {
    // Lines like: "12,345,678 (12.34%)  file:function [binary]"
    for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
        const m = line.match(/^\s*([\d,]+)\s+(?:\([\d.]+%\)\s+)?(\S.*?)\s*(?:\[[^\]]*\])?\s*$/);
        if (!m || /PROGRAM TOTALS|^Ir/.test(m[2])) {
            continue;
        }
        const name = m[2].replace(/^[^:]*:/, '');
        add(name, Number(m[1].replace(/,/g, '')));
    }
} else {
    throw new Error('kind is cpuprof or callgrind');
}

const pct = (v) => Number(((100 * v) / total).toFixed(1));
console.log(JSON.stringify({
    file,
    kind,
    total,
    causes: Object.fromEntries(Object.entries(causes).sort((a, b) => b[1] - a[1]).map(([k, v]) => [k, pct(v)])),
    top: Object.entries(frames).sort((a, b) => b[1] - a[1]).slice(0, topN).map(([k, v]) => [pct(v), cause(k), k.slice(0, 160)]),
}, null, 1));

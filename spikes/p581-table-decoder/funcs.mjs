#!/usr/bin/env node
// P5-81: lists the code size of every function in a named .wasm (name
// section), as TSV: bytes, name. Used by wasmsize.mjs and for inspection.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

function leb(buf, pos) {
    let result = 0, shift = 0, byte;
    do {
        byte = buf[pos++];
        result += (byte & 0x7f) * 2 ** shift;
        shift += 7;
    } while (byte & 0x80);
    return [result, pos];
}

function sections(buf) {
    const out = [];
    let pos = 8;
    while (pos < buf.length) {
        const id = buf[pos++];
        let size;
        [size, pos] = leb(buf, pos);
        out.push({ id, start: pos, end: pos + size });
        pos += size;
    }
    return out;
}

/** [{ name, bytes }] for every defined function, plus the section sizes. */
export function functions(buf) {
    const secs = sections(buf);
    let imported = 0;
    const imp = secs.find(s => s.id === 2);
    if (imp) {
        let pos = imp.start, n;
        [n, pos] = leb(buf, pos);
        for (let i = 0; i < n; i++) {
            let len;
            [len, pos] = leb(buf, pos); pos += len;
            [len, pos] = leb(buf, pos); pos += len;
            const kind = buf[pos++];
            if (kind === 0) { [, pos] = leb(buf, pos); imported++; }
            else if (kind === 1) { pos++; let f; [f, pos] = leb(buf, pos); [, pos] = leb(buf, pos); if (f & 1) [, pos] = leb(buf, pos); }
            else if (kind === 2) { let f; [f, pos] = leb(buf, pos); [, pos] = leb(buf, pos); if (f & 1) [, pos] = leb(buf, pos); }
            else if (kind === 3) { pos += 2; }
            else { throw new Error(`import kind ${kind}`); }
        }
    }
    const names = new Map();
    for (const s of secs.filter(s => s.id === 0)) {
        let pos = s.start, len;
        [len, pos] = leb(buf, pos);
        if (buf.toString('utf8', pos, pos + len) !== 'name') continue;
        pos += len;
        while (pos < s.end) {
            const sub = buf[pos++];
            let size;
            [size, pos] = leb(buf, pos);
            if (sub === 1) {
                let p = pos, n;
                [n, p] = leb(buf, p);
                for (let i = 0; i < n; i++) {
                    let idx, l;
                    [idx, p] = leb(buf, p);
                    [l, p] = leb(buf, p);
                    names.set(idx, buf.toString('utf8', p, p + l));
                    p += l;
                }
            }
            pos += size;
        }
    }
    const code = secs.find(s => s.id === 10);
    const data = secs.find(s => s.id === 11);
    const fns = [];
    let pos = code.start, n;
    [n, pos] = leb(buf, pos);
    for (let i = 0; i < n; i++) {
        const before = pos;
        let size;
        [size, pos] = leb(buf, pos);
        pos += size;
        fns.push({ name: names.get(imported + i) || '', bytes: pos - before });
    }
    return { fns, codeBytes: code.end - code.start, dataBytes: data ? data.end - data.start : 0 };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    const { fns } = functions(readFileSync(process.argv[2]));
    for (const f of fns.sort((a, b) => b.bytes - a.bytes)) console.log(`${f.bytes}\t${f.name}`);
}

// Records what node-semver's `parse(version)` returns for a fixed, generated
// set of inputs, for the differential test `semver_parse_matches_node_semver`
// in concerto-core/src/model_util.rs (task P5-20, F4; since P5-25 the
// `semver` crate plus a node-compat wrapper).
//
// The recording must come from node-semver 7.6.3, the version concerto-core
// 5.0.0 depends on. Pass the path to that package:
//
//   node concerto-core/tests/semver/record.mjs \
//     ../concerto/packages/concerto-core/node_modules/semver \
//     > concerto-core/tests/semver/node-semver-7.6.3.json
//
// The inputs are deterministic (a fixed seed), so re-running the script
// reproduces the committed file byte for byte. Inputs never contain a lone
// UTF-16 surrogate, since a Rust `&str` cannot hold one.

import { createRequire } from 'node:module';
import path from 'node:path';

const dir = process.argv[2];
if (!dir) {
    console.error('usage: node record.mjs <path to node-semver 7.6.3>');
    process.exit(2);
}
const require = createRequire(path.resolve(dir, 'package.json'));
const semverVersion = require('./package.json').version;
if (semverVersion !== '7.6.3') {
    console.error(`expected node-semver 7.6.3, found ${semverVersion}`);
    process.exit(1);
}
const parse = require('./functions/parse.js');
const { safeRe, t } = require('./internal/re.js');

const inputs = new Set();
const add = (s) => inputs.add(s);

// Hand-picked cases: prerelease, build metadata, leading zeros, whitespace,
// `v` prefixes, numeric limits.
[
    '', '1', '1.0', '1.0.0', '0.0.0', 'v1.0.0', 'V1.0.0', 'vv1.0.0', '=1.0.0',
    'v 1.0.0', ' v1.0.0', 'v1.0.0 ', '1.0.0.0', '1..0', '.1.0.0', '1.0.0.',
    '01.0.0', '1.01.0', '1.0.01', '00.0.0', '1.0.00', '1.2.3',
    '1.2.3-0', '1.2.3-00', '1.2.3-01', '1.2.3-0a', '1.2.3-01a', '1.2.3-a',
    '1.2.3-alpha.1', '1.2.3-alpha..1', '1.2.3-alpha.', '1.2.3-.alpha',
    '1.2.3-', '1.2.3--', '1.2.3---', '1.2.3-a-b', '1.2.3-a.b.c.d',
    '1.2.3-rc.10', '1.2.3-0.0.0', '1.2.3-0.00', '1.2.3-x.7.z.92',
    '1.2.3+', '1.2.3+a', '1.2.3+01', '1.2.3+a.b', '1.2.3+a..b', '1.2.3+.a',
    '1.2.3+a.', '1.2.3+-', '1.2.3++', '1.2.3+a+b', '1.2.3-a+b', '1.2.3-a+b.c',
    '1.2.3+b-a', '1.2.3-+', '1.2.3-a+', '1.2.3-a_b', '1.2.3+a_b', '1.1.2+.123',
    '1.2.3-αβ', '1.2.3+ß', '1.2.3-a b', '1 .2.3', '1. 2.3',
    '9007199254740991.0.0', '9007199254740992.0.0', '0.9007199254740991.0',
    '0.0.9007199254740992', '99999999999999999999.0.0',
    '1.2.3-9007199254740990', '1.2.3-9007199254740991', '1.2.3-9007199254740992',
    '1.2.3-99999999999999999999', '1.2.3-0.9007199254740991',
    '1.2.3-\u0663', '\u0661.0.0', '1.0.\uFF11', '\uFF11.0.0',
    '1.0.0\u0000', '\u00001.0.0', '1.0.0\n', '\n1.0.0', '\t1.0.0\t',
    '\u000B1.0.0', '\f1.0.0', '\r\n1.0.0\r\n', '\u00A01.0.0', '1.0.0\u00A0',
    '\u16801.0.0', '\u20001.0.0', '\u200A1.0.0', '\u20281.0.0', '\u20291.0.0',
    '\u202F1.0.0', '\u205F1.0.0', '\u30001.0.0', '\uFEFF1.0.0', '1.0.0\uFEFF',
    '\u00851.0.0', '\u180E1.0.0', '\u200B1.0.0', '1.0.0\u200B',
].forEach(add);

// Very long input: MAX_LENGTH (256) is checked on the untrimmed string in
// UTF-16 units, and the regex's {0,256}, {0,250} and {1,250} limits.
const pad = (n, c) => c.repeat(Math.max(0, n));
for (const total of [249, 250, 251, 255, 256, 257, 300]) {
    add('1.0.0-' + pad(total - 6, 'a'));
    add('1.0.0-' + pad(total - 7, 'a') + '1');
    add('1.0.0-1' + pad(total - 7, 'a'));
    add('1.0.0-' + pad(total - 7, '1') + 'a');
    add('1.0.0-' + pad(total - 6, '1'));
    add('1.0.0+' + pad(total - 6, 'b'));
    add('1.0.0-a+' + pad(total - 8, 'b'));
    add('v1.0.0-' + pad(total - 7, 'a'));
    add(pad(total - 5, '1') + '.0.0');
    add('1.0.0' + pad(total - 5, ' '));
    add(pad(total - 5, ' ') + '1.0.0');
    add(pad(total - 5, '\u3000') + '1.0.0');
    add('1.0.0-' + Array.from({ length: Math.floor((total - 6) / 2) }, () => 'a').join('.'));
    add('1.0.0+' + Array.from({ length: Math.floor((total - 6) / 2) }, () => 'b').join('.'));
    // Astral characters count as two UTF-16 units each.
    add('1.0.0' + pad(Math.floor((total - 5) / 2), '\u{1F600}'));
    add(pad(Math.floor((total - 5) / 2), '\u{1F600}') + '1.0.0');
}

// Random versions near the grammar, from a fixed seed (mulberry32).
let seed = 0x5020f4;
const random = () => {
    seed = (seed + 0x6D2B79F5) | 0;
    let r = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    r = (r + Math.imul(r ^ (r >>> 7), 61 | r)) ^ r;
    return ((r ^ (r >>> 14)) >>> 0) / 4294967296;
};
const pick = (xs) => xs[Math.floor(random() * xs.length)];
// Mostly valid parts, so that about half the versions parse.
const numbers = ['0', '1', '9', '10', '123', '9007199254740991'];
const badNumbers = ['00', '01', '007', '9007199254740992', ''];
const ids = ['0', '1', '10', '0a', '01a', 'a', 'alpha', 'rc', '-', 'a-b', '1-', '-1', 'Z9', 'x.y', '9007199254740991'];
const badIds = ['00', '01', '', 'a_b', 'é'];
const number = () => (random() < 0.05 ? pick(badNumbers) : pick(numbers));
const id = () => (random() < 0.05 ? pick(badIds) : pick(ids));
const junk = [' ', '\t', '.', '-', '+', 'v', '=', '_', '\u00A0', '\uFEFF', '\u0085', 'é', '\u{1F600}'];
const generate = () => {
    let s = random() < 0.2 ? 'v' : '';
    s += `${number()}.${number()}.${number()}`;
    if (random() < 0.5) {
        s += '-' + Array.from({ length: 1 + Math.floor(random() * 3) }, () => id()).join('.');
    }
    if (random() < 0.4) {
        s += '+' + Array.from({ length: 1 + Math.floor(random() * 3) }, () => id()).join('.');
    }
    // Mutate: insert, delete or replace one character, or pad with space.
    const r = random();
    const chars = Array.from(s);
    const at = Math.floor(random() * (chars.length + 1));
    if (r < 0.15) {
        chars.splice(at, 0, pick(junk));
    } else if (r < 0.3 && chars.length > 0) {
        chars.splice(Math.min(at, chars.length - 1), 1);
    } else if (r < 0.4 && chars.length > 0) {
        chars.splice(Math.min(at, chars.length - 1), 1, pick(junk));
    } else if (r < 0.5) {
        return pick([' ', '\n', '\u3000']) + chars.join('') + pick(['', ' ', '\t']);
    }
    return chars.join('');
};
for (let i = 0; i < 4000; i++) {
    add(generate());
}

const cases = [...inputs].map((input) => {
    const v = parse(input);
    return {
        input,
        parsed: v === null ? null : {
            raw: v.raw,
            major: v.major,
            minor: v.minor,
            patch: v.patch,
            prerelease: v.prerelease,
            build: v.build,
            version: v.version,
        },
    };
});

process.stdout.write(JSON.stringify({
    semver: semverVersion,
    full: safeRe[t.FULL].source,
    cases,
}, null, 0).replace(/\},\{"input"/g, '},\n{"input"') + '\n');

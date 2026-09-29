// P5-30: a CONCERTO_ENGINE_MODULE shim that loads the real engine
// (P530_REAL_ENGINE) and writes the first `models` argument
// decoratorManagerExtractDecorators receives to P530_DUMP, as the JSON text
// the binding's own `to_json` produces (JSON.stringify). Every call also
// records its wall time and the JSON.stringify / JSON.parse time around it,
// on globalThis.__p530.
'use strict';
const fs = require('fs');
const real = require(process.env.P530_REAL_ENGINE);
const out = Object.create(real);
let dumped = false;
const stats = (globalThis.__p530 = { calls: 0, ns: 0 });
out.decoratorManagerExtractDecorators = function (models, options) {
    if (!dumped && process.env.P530_DUMP) {
        fs.writeFileSync(process.env.P530_DUMP, JSON.stringify(models));
        dumped = true;
    }
    const t0 = process.hrtime.bigint();
    try {
        return real.decoratorManagerExtractDecorators(models, options);
    } finally {
        stats.calls++;
        stats.ns += Number(process.hrtime.bigint() - t0);
    }
};
module.exports = out;

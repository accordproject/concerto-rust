const e = require('/home/user/wt/P5-42/concerto-rust/concerto-wasm/pkg/concerto-engine.cjs');
module.exports = new Proxy(e, { get(t, k) { return k === 'DcsManagerHandle' ? undefined : t[k]; } });

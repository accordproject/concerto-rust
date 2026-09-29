// Split of the cold extract call's extra cost, Rust engine through the TS API.
const [dist, file] = [process.argv[2], process.argv[3]];
const { ModelManager, DecoratorManager } = require(dist);
const rust = require(process.env.CONCERTO_ENGINE_MODULE || '@accordproject/concerto-engine');
const models = JSON.parse(require('fs').readFileSync(file, 'utf8'));
const ast = { $class: 'concerto.metamodel@1.0.0.Models', models };
const med = a => a.slice().sort((x, y) => x - y)[a.length >> 1];
const T = {}; const add = (k, v) => (T[k] = T[k] || []).push(v);
const time = (k, f) => { if (global.gc) global.gc(); const t = process.hrtime.bigint(); const r = f(); add(k, Number(process.hrtime.bigint() - t) / 1e6); return r; };
for (let i = 0; i < 18; i++) {
  const mm = new ModelManager(); mm.fromAst(ast);
  const a = time('getAst_resolve_1st', () => mm.getAst(true, false));
  time('getAst_resolve_2nd', () => mm.getAst(true, false));
  time('getAst_noresolve', () => mm.getAst(false, false));
  const h = time('DcsManagerHandle_new', () => new rust.DcsManagerHandle(a.models)); h.free();
  time('extract_cold', () => DecoratorManager.extractDecorators(mm, {}));
  time('extract_warm', () => DecoratorManager.extractDecorators(mm, {}));
}
const out = {}; for (const k in T) out[k] = +med(T[k].slice(3)).toFixed(2);
console.log(JSON.stringify(out));

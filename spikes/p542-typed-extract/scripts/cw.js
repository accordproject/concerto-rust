// Cold (first call on a fresh source manager) vs warm (repeat call) extractDecorators.
const [dist, file, n] = [process.argv[2], process.argv[3], +(process.argv[4] || 15)];
const { ModelManager, DecoratorManager } = require(dist);
const models = JSON.parse(require('fs').readFileSync(file, 'utf8'));
const ast = { $class: 'concerto.metamodel@1.0.0.Models', models };
const med = a => a.slice().sort((x, y) => x - y)[a.length >> 1];
const cold = [], warm = [];
for (let i = 0; i < n + 3; i++) {
  const mm = new ModelManager(); mm.fromAst(ast);
  let t = process.hrtime.bigint(); DecoratorManager.extractDecorators(mm, {});
  const c = Number(process.hrtime.bigint() - t) / 1e6;
  t = process.hrtime.bigint(); DecoratorManager.extractDecorators(mm, {});
  const w = Number(process.hrtime.bigint() - t) / 1e6;
  if (i >= 3) { cold.push(c); warm.push(w); }
}
console.log(JSON.stringify({ cold_ms: +med(cold).toFixed(2), warm_ms: +med(warm).toFixed(2) }));

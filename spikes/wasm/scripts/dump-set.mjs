// Writes the benchmark's model set to dist/bench-set.json for examples/native.rs.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { syntheticModels } from './fixtures.mjs';

const models = syntheticModels({ namespaces: 10, decls: 50, props: 10 });
models.push(JSON.parse(readFileSync(new URL('../fixtures/metamodel.json', import.meta.url), 'utf8')));
mkdirSync(new URL('../dist/', import.meta.url), { recursive: true });
writeFileSync(new URL('../dist/bench-set.json', import.meta.url), JSON.stringify(models));

// Reports the .wasm sizes at each stage, raw and compressed.
import { readFileSync } from 'node:fs';
import { gzipSync, brotliCompressSync, constants } from 'node:zlib';

const NAME = 'concerto_wasm_spike';
const dist = new URL('../dist/', import.meta.url);
const files = [
  ['cargo (before wasm-bindgen)', process.argv[2]],
  ['wasm-bindgen output', new URL(`${NAME}_bindgen.wasm`, dist)],
  ['after wasm-opt -Oz', new URL(`${NAME}_opt.wasm`, dist)],
  ['ESM loader + inlined base64', new URL('wasm-bytes.mjs', dist)],
];
for (const [label, f] of files) {
  const b = readFileSync(f);
  const gz = gzipSync(b, { level: 9 }).length;
  const br = brotliCompressSync(b, { params: { [constants.BROTLI_PARAM_QUALITY]: 11 } }).length;
  console.log(`${label.padEnd(30)} ${String(b.length).padStart(9)} B  gzip ${String(gz).padStart(8)}  brotli ${String(br).padStart(8)}`);
}

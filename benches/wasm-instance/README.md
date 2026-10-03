# benches/wasm-instance/

The in-WASM half of workload 3 (task P5-13, accordproject/concerto-rust#297).
It compiles `../benches/instance_validate.rs`'s workload (the same model, the
same 500 instances, the same three routes) to `wasm32-unknown-unknown` and
times it in V8 under Node. Each timed call walks all 500 instances inside
WASM, so the figure is the instance validator's own cost in WASM, without
the per-call work between TS and the engine that `resource.validate()`
through the TS API also pays.

This is bench tooling only. It adds no binding to concerto-wasm, and
nothing links it. Like concerto-wasm, it is its own Cargo workspace.

## Running

```sh
# wasm-bindgen-cli 0.2.128 and binaryen's wasm-opt on PATH, e.g. after
# `npm ci` in concerto-wasm:
PATH=$PWD/../../concerto-wasm/node_modules/.bin:$PATH sh build.sh
node run.mjs --out result.json
```

`run.mjs` reports the median, mean and spread in µs per instance for:

- `validate_only`: `instance::validate::validate_instance`, the
  `ResourceValidator` walk, the in-WASM validator floor that P5-12b
  measured (accordproject/concerto-rust#292);
- `from_json`: concerto-core-js's `Serializer::from_json`, what the
  engine's `serializerFromJsonCompact` runs;
- `validate_instance_native`: `ModelManager::validate_instance`.

To time another tree (for example the integration head as a "before"),
copy this directory, without `target/` and `pkg/`, into that checkout's
`benches/` and build it there.

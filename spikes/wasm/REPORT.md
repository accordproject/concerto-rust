# WASM feasibility spike: report

Tracks concerto-rust#86 (P4-01a), for plan concerto-rust#29 (§3, §4 P4-01, §7; decision D2).

This directory holds a throwaway wasm-bindgen crate, `concerto-wasm-spike`. It depends on `../../concerto-core` by path and exposes:
- an `Engine` that wraps `ModelManager`, with `addModel(json)`, `addModelObject(ast)` and `validateModels()`;
- snapshots of every loaded namespace: `snapshotJson()` returns a JSON string and `snapshotObject()` returns an object built by serde-wasm-bindgen;
- fine-grained getters, keyed either by a namespace string (`declName` and so on) or by an integer handle (`hDeclName` and so on);
- boundary probes: `noop`, `add`, `strLen`, `makeStr`;
- error probes: `throwSample`, `setErrorFactory`, `panicSample`, `Engine.panicInMethod`.

The crate is not a workspace member (it has its own empty `[workspace]`). Nothing outside `spikes/wasm/` changed.

## How to run

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked   # must equal the crate version
npm install                          # binaryen (wasm-opt) + Playwright 1.59.1
sh build.sh                          # cargo, wasm-bindgen (web + nodejs), wasm-opt -Oz, inlined loaders
node scripts/node-smoke.mjs          # Node ESM smoke
node scripts/node-smoke.cjs          # Node CommonJS smoke (Node 18+)
node scripts/chromium-smoke.mjs      # headless shell + full Chromium
node scripts/bench.mjs               # boundary benchmark
node scripts/load-bench.mjs [node...]  # cold load per Node binary
sh scripts/measure-all.sh            # everything, both profiles, into results/
```

`results/` holds the raw output behind every number:
- `release-*`: `opt-level = "z"`, LTO, `panic = "abort"`;
- `release-speed-*`: the same with `opt-level = 3`;
- `smoke-*`: the final code.

Measurement conditions: i7-7820HQ (4 cores, 8 threads), macOS 13. The machine was **heavily loaded by other jobs** (load average 40–115), so absolute timings are noisy by 2–3×. Benchmarks report best-of-N (5 in Node, 3 in Chromium). The conclusions rest only on differences of an order of magnitude, or on differences that repeat across both runtimes and both profiles.

The model set is 10 synthetic namespaces plus the real `concerto.metamodel@1.0.0` AST: 11 files, 562 declarations, 5,006 properties, 720 KiB of JSON.

## 1. Synchronous instantiation from inlined bytes in Node 18+ — YES

There are two loaders, both carrying the `.wasm` as base64:
- `dist/concerto-engine.mjs` uses the `web` glue: `new WebAssembly.Module(bytes)`, then `initSync({module})`, at module evaluation.
- `dist/concerto-engine.cjs` uses the `nodejs` glue, with `readFileSync` replaced by `Buffer.from(<base64>)`.

Both smokes pass on Node 18.20.8 and 24.21.0; Node 20 and 22 were exercised by the load benchmark. The CJS smoke calls `new Engine()` on the line straight after `require()`.

Cold load: a fresh process each run, median of 9, in ms:

| Node | ESM load | decode | compile | instantiate | CJS require | first new Engine() |
|---|---|---|---|---|---|---|
| 18 | 224 | 41 | 32 | 2.4 | 188 | 0.3 |
| 20 | 57 | 1.1 | 7.9 | 2.7 | 36 | 8.5 |
| 22 | 57 | 0.8 | 5.2 | 1.8 | 100* | 8.3 |
| 24 | 61 | 0.7 | 4.2 | 2.0 | 89* | 10–75* |

\* Noisy; the opt-3 run measured 36, 36 and 10–14 ms.

- Node 20+ compiles lazily, so part of the cost moves to the first call (about 8–14 ms). Node 18 compiles eagerly, which takes about 200 ms but is still synchronous.
- An `atob` decode loop took 786 ms on Node 18. The loader now tries decoders in this order: `Uint8Array.fromBase64` (Chromium 147; not available in Node 24.21), then `Buffer`, then `atob`.
- Inlining adds 33%: 1,115,534 bytes become 1,487,380 base64 characters (gzip 596 KB, brotli 415 KB).

**Recommendation:** ship both loaders. concerto-core's `main` is CommonJS and Node 18 has no `require(esm)`, so the CJS loader is required.

## 2. Browsers

**Size after `wasm-opt -Oz`**

| Build | Raw | gzip | brotli |
|---|---|---|---|
| opt-level `z` | 1,115,534 | 424 KB | 298 KB |
| opt-level `3` | 1,551,104 | 561 KB | 384 KB |

The `z` build is 725 KB of code and 382 KB of data. Most of the data is `regex-syntax` Unicode tables, pulled in via `fancy-regex`. Measure again after the move to `regress`.

**Sync-compile limit, measured with padded valid modules.** In both the Playwright headless shell and full Chromium 147:
- the main thread accepts up to **8,388,608 bytes inclusive**;
- one byte more gives `RangeError: WebAssembly.Compile is disallowed on the main thread, if the buffer size is larger than 8MB`;
- 4,097 bytes is fine; the old 4 KB limit is gone.

**The real module compiles synchronously on the main thread:** 5.7 ms compile, 2.1 ms instantiate, 3.8 ms decode, and all checks pass. Headroom under the limit is 7.5× for the `z` build and 5.4× for the opt-3 build.

**Fallbacks**
- Module worker importing the same loader: works (import 37–47 ms).
- `await WebAssembly.compile`, then synchronous `initSync({module})`: works (compile 5–17 ms, instantiate 5–12 ms). Instantiating an already compiled module synchronously is not subject to the limit.
- Chunking: not needed.

**Recommendation:** keep synchronous instantiation as the default. Add a CI size budget (for example, fail above 4 MiB). Also export an async `init()` built on compile-then-`initSync`. Don't build worker or chunking machinery.

Not measured: Firefox, WebKit, and CSP (`'wasm-unsafe-eval'`).

## 3. Boundary cost

**Per call, Node 24 / Chromium 147**

| Call | Node 24 | Chromium 147 |
|---|---|---|
| `noop` | 15 ns | 6 ns |
| `add(u32,u32)` | 30 ns | 25 ns |
| string in, 32 chars | 507 ns | 209 ns |
| string out, 32 chars | 1,119 ns | 913 ns |
| bool getter by handle | 567 ns | 272 ns |
| string getter by handle | 1,521 ns | 1,555 ns |
| string getter by namespace-string key | 2,177 ns | 3,047 ns |

**Walking the set** (16,715 getter calls):

| Strategy | Node 24 | Chromium 147 |
|---|---|---|
| fine-grained, string keys | 28.1 ms | 27.2 ms |
| fine-grained, integer handles | 25.8 ms (1.54 µs/call) | 29.2 ms |
| `snapshotJson` + `JSON.parse` (514 KiB) | 21.9 ms (13.4 on opt-3) | 7.9 ms |
| `snapshotObject` (serde-wasm-bindgen) | 19.0 ms | 19.3 ms |
| walk over a cached JS snapshot | 0.30 ms | 0.075 ms |

**Loading the set, Node / Chromium**

| Path | Node 24 | Chromium 147 |
|---|---|---|
| `addModel(string)` | 84 ms | 59 ms |
| `JSON.stringify` + `addModel` | 94 ms | 56 ms |
| `addModelObject` (serde-wasm-bindgen) | 177 ms | 86 ms |
| native Rust, same work | 97–170 ms | – |
| `validateModels` | 8.3 ms (native 4.5–6.9) | 7.1 ms |

What the numbers show:
- Fine-grained walks cost 100–350× more than a cached snapshot.
- Handles beat string keys by only 0–30%; returned strings dominate the cost.
- JSON strings beat serde-wasm-bindgen for large trees in both directions: 1.5–2× faster for input, and 2.4× faster for output in Chromium.
- The load cost is concerto-core's own (`ModelFile::from_json`), not WASM's: native is no faster.
- opt-3 gave no repeatable speed gain apart from JSON serialisation, and costs 39% more bytes. Keep `z`.

**Recommendation:** per-getter thin views are not viable. Views should cache a JSON snapshot of each `ModelFile` or declaration, fetched in one call and invalidated by a generation counter that every mutation bumps. Work-heavy operations must be single coarse calls (validation, `Serializer.fromJSON`/`toJSON`, instance validation).

## 4. Error mapping — YES

- **Strategy A.** Rust builds a `js_sys::Error` with `name = kind` and copies the payload onto it (`kind`, `message`, `fileName`, `location`, `typeName`, `namespace`). The result is `instanceof Error` and keeps its fields and stack.
- **Strategy B.** A JS factory is registered at load (`setErrorFactory`); Rust calls `factory(kind, message, props)` and throws the result.
  - The result is a real `IllegalModelException`: it is `instanceof` that class, `BaseException` and `Error`, with `modelFile` and `fileLocation` intact and WASM frames in the stack.
  - Real errors map correctly: duplicate namespace, malformed JSON, and `validateModels` ("ValidationException: … Could not find super type Missing for Orphan").
  - The engine stays usable afterwards.
  - Checked in Node ESM and CJS, and in the Chromium main thread and worker.
- **Cost:** a mapped throw costs 44–81 µs, against 17–19 µs for a bare JS throw.
- **Panics.** With `panic = "abort"`, a panic surfaces as `WebAssembly.RuntimeError: unreachable`. A fresh object created afterwards works. The object that panicked inside a `&mut self` method is poisoned for good: every later call throws "recursive use of an object detected which would lead to unsafe aliasing in rust".
- Use after `free()` throws "null pointer passed to rust". wasm-bindgen registers a `FinalizationRegistry`, so views don't need an explicit `free()`.

**Recommendation:** use strategy B, with the payload extended to `{kind, code, params, location}`. Treat any `RuntimeError` as fatal for that manager. Never panic on the boundary path; add clippy `unwrap_used`/`indexing_slicing` lints to the binding crate.

## 5. Build tooling

**cargo + wasm-bindgen-cli 0.2.128 + npm binaryen@132 (used here)**
- One cargo build produces both the `web` and the `nodejs` glue.
- `wasm-opt -Oz` takes about 28 s.
- `build.sh` takes 2 min 27 s warm on this loaded machine.

**wasm-pack 0.15.0**
- Builds one target per invocation.
- Its downloaded binaryen 117 **fails** with "Bulk memory operations require bulk memory" until `[package.metadata.wasm-pack.profile.release] wasm-opt` sets the feature flags (now in `Cargo.toml`). Output is then 1,117,221 bytes, in 1 min 19 s.

A cold cargo wasm32 build took 5 min 37 s.

**Recommendation:** use cargo + wasm-bindgen-cli + wasm-opt.

CI needs:
- `rustup target add wasm32-unknown-unknown` (installs cleanly);
- a prebuilt `wasm-bindgen-cli` pinned to exactly the crate version: `cargo install` took 6 min 48 s here (use taiki-e/install-action or cargo-binstall; not tried);
- `wasm-opt` with these flags: `--enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-reference-types --enable-multivalue --enable-mutable-globals`;
- `npx playwright install chromium`;
- a Node 18 job for the CJS loader;
- a size-budget check.

Keep the binding crate out of the root workspace's host build.

## Input to P1-04 (handle design)

1. Make `DeclId`/`PropId` dense `u32` arena indices, passed to JS as plain numbers. Use one exported object per `ModelManager`, not one wasm-bindgen object per declaration.
2. Keep ids stable across mutations (append-only arena with tombstones, or generation-tagged ids). Add a `generation()` counter that every mutation bumps.
3. Make snapshots first-class: `declaration_snapshot(DeclId)` and `model_file_snapshot(ns)` return `Serialize` structs, sent across as JSON strings. Per-field getters are for rare paths only.
4. Id lookup must not hash strings; the string key costs 0–30% per call.
5. Never panic on the boundary path: a panic inside a `&mut self` method poisons the whole manager.

## Input to P4-01 (loader)

1. Build two loaders from one build, both inlining base64 and instantiating synchronously:
   - ESM: `web` glue plus `initSync`;
   - CJS: `nodejs` glue with `readFileSync` swapped out. `inline.mjs` asserts the exact line it replaces, so pin wasm-bindgen.
2. Decoder order: `Uint8Array.fromBase64`, then `Buffer`, then `atob`.
3. Add an async `init()` (compile, then `initSync`) as a browser fallback; sync stays the default.
4. Register the error factory once at load.
5. Build with `opt-level = "z"`, LTO, `panic = "abort"`, and `wasm-opt -Oz` with explicit feature flags. Add a CI size budget.
6. Expect a cold start of about 30–60 ms on Node 20 and later and about 200 ms on Node 18, plus 8–14 ms for the first `new ModelManager()` on Node 20 and later.

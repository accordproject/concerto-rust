# concerto-wasm

The WASM binding of `concerto-core`, built with wasm-bindgen (decision D2 of
accordproject/concerto-rust#29). Its output, `pkg/`, is the npm package
`@accordproject/concerto-engine`, which the concerto-core TS views load when
`CONCERTO_ENGINE=rust` (PORTING.md 1.5 and 4). Task P4-01 (#60) built it, and
it closes #28's WASM half.

The crate is not a member of the root workspace (it has its own empty
`[workspace]`), so the host build of concerto-core never compiles
wasm-bindgen (spike REPORT §5).

## Build

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked   # must equal the crate's wasm-bindgen
npm install          # binaryen 132 (wasm-opt) and Playwright 1.59.1
npm run build        # sh build.sh
```

`build.sh` runs cargo, then wasm-bindgen for both the `web` and the `nodejs`
targets, then `wasm-opt -O3`, then `scripts/inline.mjs`. It fails when the
optimised module is over the 4 MiB budget. The release profile and
`wasm-opt` both optimise for speed rather than size since task P5-06
(accordproject/concerto-rust#220): the module grows by about 1 MB (to about
2.6 MB, still inside the budget) and the P5-04 workloads run 10-35% faster
through the TS views. It writes:

| File | For | Loads by |
|---|---|---|
| `pkg/concerto-engine.cjs` | Node `require` | the `nodejs` glue, with the `.wasm` inlined as base64 in place of `readFileSync` |
| `pkg/concerto-engine.mjs` | `import`, browsers and Node | the `web` glue, `initSync` from the inlined bytes while the module is evaluated |
| `pkg/package.json` | `@accordproject/concerto-engine` | `exports`: `require` → `.cjs`, `import` → `.mjs` |

Both loaders instantiate **synchronously** when loaded; there is no fetch
and no async compile. Both also export an async `init()`. On a browser main
thread that refuses a synchronous compile (Chromium's limit is 8 MiB),
`init()` compiles asynchronously and then instantiates with `initSync`.
Otherwise it resolves at once.

Lint with `cargo fmt -- --check` and
`cargo clippy --target wasm32-unknown-unknown --all-targets -- -D warnings`.
`Cargo.toml` denies `unwrap_used`, `expect_used`, `indexing_slicing` and
`panic`.

## Linking from concerto (decision D9: not published)

The concerto checkout has a workspace package, `packages/concerto-engine`,
named `@accordproject/concerto-engine`. It re-exports
`../concerto-rust/concerto-wasm/pkg/concerto-engine.{cjs,mjs}` from a
concerto-rust checkout **next to** the concerto checkout. `npm install` in
concerto links it into `node_modules`, where the shim finds it.
`CONCERTO_ENGINE_MODULE=<path to concerto-engine.cjs>` still overrides it.

## The handle API

There is one exported object per `ModelManager`, `ModelManagerHandle`
(spike "Input to P1-04"). Model files, declarations and properties are
named by the P1-04 arena's dense `u32` handles (`ModelFileId`, `DeclId`,
`PropId`), which are plain JS numbers. A handle keeps naming the same element
for the life of the manager.

| Member | Returns |
|---|---|
| `new ModelManagerHandle()` | a manager with `concerto@1.0.0` loaded |
| `addModel(astJson, fileName?)` | the new model file's handle; `astJson` is `JSON.stringify(ast)` |
| `validateModels()` | nothing; throws the first problem |
| `epoch()` | the mutation counter; a cached snapshot is current while it is unchanged. It moves iff the manager may have changed; staging and the extract memo never move it (P5-101, D-7) |
| `generation()` | the model files' state version (`ModelManager::state_version`); kept exported, not used by the views |
| `modelFileId(namespace)`, `declarationId(fqn)` | a handle, or `undefined` |
| `modelFileIds()`, `declarationIds(file)`, `propertyIds(decl)` | `Uint32Array` of handles, in order |
| `modelFileOf(decl)`, `parentOf(prop)` | a handle, or `undefined` |
| `modelFileSnapshot(file)` | JSON text `{namespace, version, fileName, ast}` |
| `declarationSnapshot(decl)` | JSON text `{name, fullyQualifiedName, modelFile, ast}` |
| `propertySnapshot(prop)` | JSON text `{name, declaration, ast}` |
| `free()` | releases the manager; a `FinalizationRegistry` does it anyway |

- The `ast` in a snapshot is the node as it was loaded (OD-3).
- Snapshots are JSON text, which the view parses once and caches, because
  per-field getters cost 100–350× more (spike REPORT §3).
- Enum values have no `PropId` until P2-04.

**Errors.** Every core error is thrown through the error factory that the
shim registers with `setHost(factory, semverParse)`, as the payload
`{kind, code, params, message, location, …}` (PORTING.md 2).
- Loader errors that no unit has ported yet (a duplicate namespace, a
  handle that names nothing) have `code: "pre-port"` and their message
  verbatim.
- Malformed JSON text is a JS `SyntaxError`.

**The resident DCS manager** (P5-27, F6). `new DcsManagerHandle(models)`
loads the source models of a `DecoratorManager` call once. The shim keeps it
for as long as the source `ModelManager`'s epoch and model files are
unchanged. Its `decorateModels(target, commandSets, options)`,
`extractDecorators(target, options)`, `extractVocabularies(target, options)`
and `extractNonVocabDecorators(target, options)` return what the matching
`decoratorManager*` binding returns. Each also stages the result's model
files into `target`, the new manager's `ModelManagerHandle`, and adds
`staged` (a `[stageId, header]` entry, or `null`, for each result model)
and `validated`. `ModelManagerHandle.dcsValidate(commandSet)` is
`DecoratorManager.validate`'s structural check against the handle's own
manager: the shim calls it on the validation manager it has just built,
instead of `decoratorManagerValidate` rebuilding one from the model files.
The per-call `decoratorManager*` bindings are unchanged.

**DCS operations on the source handle** (P5-55, F-A1).
`ModelManagerHandle.dcsDecorateModels(target, commandSets, options)`,
`dcsExtractDecorators(target, options)`, `dcsExtractVocabularies(target,
options)` and `dcsExtractNonVocabDecorators(target, options)` are the
`DcsManagerHandle` operations, run on the source `ModelManager`'s own
handle, which already mirrors its models. The shim calls them whenever the
source manager's `getAst`, `getModelFiles` and `resolveMetaModel` are its
own, so it no longer copies the models into a `DcsManagerHandle`, and keeps
`DcsManagerHandle` and the per-call bindings as fallbacks. `target` must be
another handle. `dcsDecorateModels` validates its result with `target`'s
`decoratorValidation`; none of them changes the handle or its epoch.

**Extract result memo** (P5-56, F-A2). With `removeDecoratorsFromModel`
false, the three `dcsExtract*` methods keep a per-epoch memo on the handle:
the second call on unchanged models keeps the result manager, its encoded
AST and the resolved source models, and every later call rebuilds only the
command sets and vocabularies from them. Any change that moves the epoch
drops the memo, and so does `ModelManagerHandle.dropDcsMemo()` (additive).
Errors are never memoised, every call returns new JS objects and new staged
clones, and the memo never moves the epoch.

The P0-04b trial bindings (`modelUtil*`, `numberValidator*`,
`scalarDeclaration*`) are unchanged. Their views still hand their JS objects
back, until the graph they meet is Rust-backed (P4-06 … P4-08).

## Smokes

```sh
npm run smoke:node       # node scripts/node-smoke.cjs && node scripts/node-smoke.mjs
npm run smoke:chromium   # node scripts/chromium-smoke.mjs (Playwright's chromium)
```

- Both run the checks in `scripts/checks.mjs` against the loaded module:
  handles, snapshots, `generation()`, stable handles across a load,
  `validateModels`, errors through the factory, `free()`, a trial binding.
- `node-smoke.cjs [module]` also takes a module to `require`. For example,
  from the concerto checkout:
  `node ../concerto-rust/concerto-wasm/scripts/node-smoke.cjs @accordproject/concerto-engine`
  runs the checks through the workspace link.
- The Chromium smoke runs in the Playwright headless shell and in full
  Chromium. In each, it:
  - probes the main thread's synchronous-compile limit and checks the module
    is under it;
  - runs the checks on the main thread and in a module Worker;
  - runs the async fallback;
  - times the handle API's calls.

`results/` holds the output of the runs below.

## The spike, on the final crate

The spike (P4-01a, #86) is written up in
[`spikes/wasm/REPORT.md`](../spikes/wasm/REPORT.md). The smokes repeat its
browser and boundary measurements on this crate. Conditions: macOS 13 on an
i7-7820HQ, shared with other jobs, so timings are noisy.

**Size.** 1,789,836 bytes after `wasm-opt -Oz` (the size-optimised build this
section measured; the speed-optimised build P5-06 switched to is about 2.6 MB), against the spike's 1,115,534;
concerto-core has grown since the spike, which bound only `add_model` and
`validate_models` (the size was not broken down further). That is 43% of the
4 MiB budget and 21% of Chromium's 8 MiB sync-compile limit.

**Synchronous compile, Chromium 147.0.7727.15** (headless shell and full
Chromium): the main thread accepts 8,388,608 bytes and refuses 8,388,609
with a `RangeError`, as in the spike. The real module compiles and
instantiates synchronously on the main thread, and in a Worker. The async
fallback (`WebAssembly.compile`, then `initSync`) works: compile 6–7 ms,
instantiate 6–13 ms.

**Loading.** Both loaders pass on Node 18.20.8, 20.20.2, 22.23.2 and
24.21.0 (`results/smoke-node-*`). `new ModelManagerHandle()` runs on the line
after `require`/`import`. In Chromium, importing the ESM loader took
59–105 ms.

**Boundary cost of the handle API**, in ns per call (best of 3 × 20,000,
Chromium main thread, headless shell / full Chromium):

| Call | ns |
|---|---|
| `generation()` | 60 / 45 |
| `declarationId(fqn)` | 395 / 310 |
| `propertyIds(decl)` | 635 / 315 |
| `declarationSnapshot(decl)` (`concerto@1.0.0.Concept`) | 6,610 / 7,460 |
| the same, plus `JSON.parse` | 9,905 / 12,185 |

These agree with the spike:
- Handle calls cost a few hundred ns.
- A snapshot costs about as much as 5–8 string getters, but it carries the
  element's whole state. A view that caches it and checks `epoch()` (the
  views' key since P5-06; `generation()` was measured, at the same cost)
  pays that cost once per element per mutation.

For P4-12, nothing here changes the spike's finding that the load cost is
concerto-core's own rather than the boundary's; these numbers give no
reason for a NAPI addon.

Firefox, WebKit and CSP (`'wasm-unsafe-eval'`) were not measured, as in the
spike.

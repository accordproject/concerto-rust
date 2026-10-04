# concerto-wasm

The WASM binding of `concerto-core`, built with wasm-bindgen. Its output,
`pkg/`, is the npm package `@accordproject/concerto-engine`, which
concerto-core's engine (`src/engine/`) loads (PORTING.md 1.5 and 4).

The crate is not a member of the root workspace (it has its own empty
`[workspace]`), so the host build of concerto-core never compiles
wasm-bindgen.

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
`wasm-opt` both optimise for speed rather than size (about 1 MB larger,
inside the budget, and 10-35% faster through the TS views). It writes:

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

## Linking from concerto (not published)

The concerto checkout has a workspace package, `packages/concerto-engine`,
named `@accordproject/concerto-engine`. It re-exports
`../concerto-rust/concerto-wasm/pkg/concerto-engine.{cjs,mjs}` from a
concerto-rust checkout **next to** the concerto checkout. `npm install` in
concerto links it into `node_modules`, where the shim finds it.
`CONCERTO_ENGINE_MODULE=<path to concerto-engine.cjs>` still overrides it.

## The exported surface

The exports are the ones concerto-core's engine calls; their TS signatures
are in concerto's `packages/concerto-core/src/engine/bindings.d.ts`.

**`ModelManagerHandle`**, one per `ModelManager`. Model files, declarations
and properties are named by the arena's dense `u32` handles, plain JS
numbers, which keep naming the same element until a model file is updated
or deleted. `epoch()` is the handle's mutation counter: it moves iff the
manager may have changed (staging and the extract memo never move it), and
it stamps the handle's own caches (the `validatePropertyById` slots and the
extract memo). The TS views key their caches on their own
`EngineState.version`, not on `epoch()`. `fork()` gives an independent
copy; `free()` releases the handle (a `FinalizationRegistry` does it
anyway).

| Group | Members |
|---|---|
| Loading | `addModel`, `addModelWithDefinitions`, `updateModelFile`, `deleteModelFile`, `updateExternalModels`, `validateModelFiles`, `validateAstValue`, `throwAlreadyExists`, `setDecoratorValidation`, `setDangerouslyAllowReservedSystemTypeNamesInUserModels` |
| Staging | `stageModelFileBytes` (a model file loaded once, from UTF-8 JSON text or the compact binary layout, with the AST shape check folded in), `commitStagedModelFile(s)`, `validateAndCommitStagedModelFile`, `updateStagedModelFile`, `updateExternalModelsStaged`, `validateAstStaged`, `modelFileValidateStaged`, `dropStagedModelFile`, `stagedModelFileViewSnapshot`, `modelFileViewSnapshotOf` |
| Lookups | `modelFileId`, `declarationId`, `modelFileSnapshot`, `getNamespaces`, `getTypeName`, `resolveType`, `derivesFrom`, `isAssignableTo`, `modelManagerGetModelFileByFileName`, and the `modelFile*` members by file handle (`GetImports`, `IsLocalType`, `GetTypeName`, `GetFullyQualifiedTypeName`, `ResolveType`, `Validate`, `ValidateDetached`, `Filter`, `FilterStaged`) |
| Arena answers (BC-52) | `modelUtilIsAssignableTo`, `modelUtilIsEnum`, `modelUtilIsMap`, `modelUtilIsScalar`, `modelUtilIsValidMapKeyScalar`, `scalarDeclarationValidate`, `decoratorValidate`, `classDeclarationGetAssignableClassDeclarations`, `classDeclarationGetDirectSubclasses` |
| Serializer | `serializerFromJsonCompact(Bytes)`, `serializerToJson(Bytes)`: `Serializer.fromJSON`/`toJSON` in one call, over the wire encoding as JSON text or the compact layout |
| Instances | `validateInstance` (the collect-all diagnostics), `validateResourceBinary`, `validatePropertyBinary`, `validationPropertySlot`, `validatePropertyById` (`ValidatedResource` validation in one call) |
| DecoratorManager | `dcsValidate`, `dcsDecorateModels`, `dcsExtract` |

**`DcsManagerHandle`**: the input manager of a `DecoratorManager` call whose
source manager's handle cannot stand for it (its `getAst`, `getModelFiles`
or `resolveMetaModel` is not its own). Its `decorateModels` and `extract`
give what the source handle's `dcsDecorateModels` and `dcsExtract` give:
the result's model files staged into `target`, the new manager's handle,
with `staged` (a flat `[stageId, ...header]` entry, or `null`, per result
model) and `validated`. The engine builds one per call and frees it.

**Extract memo.** `dcsExtract` keeps a per-epoch memo on the handle, keyed
by the epoch, whether the system models are walked, and the stripping
action when `removeDecoratorsFromModel` is set: the second call at the same
key keeps the result manager, its encoded AST and the source models, and
every later call rebuilds only the command sets and vocabularies. Errors
are never memoised, every call returns new JS objects, the staged files are
shared with the kept result manager, and the memo never moves the epoch.

**Free functions**: the per-element construction views (`modelFileViewSnapshot`,
`scalarDeclarationProcess`, `classDeclarationProcess`, `propertyProcess`,
`fieldProcess`, `decoratorProcess`, the `map*Process`/`Validate` members,
the validator constructors and checks), `ModelUtil`'s string members,
`resourceId*`, `checkAstShape`, `systemModelFileHeader`,
`validateMetaModelInstance`, the `decoratorManager*` helpers, and `setHost`.

**Errors.** Every core error is thrown through the error factory the engine
registers with `setHost(factory)`, as the payload
`{kind, code, params, message, location, …}` (PORTING.md 2). A message with
no catalogue entry has `code: "pre-port"` and its message verbatim.
Malformed JSON text is a JS `SyntaxError`.

## Smokes

```sh
npm run smoke:node       # node scripts/node-smoke.cjs && node scripts/node-smoke.mjs && npm run smoke:hashdos
npm run smoke:hashdos    # node scripts/hashdos.mjs
npm run smoke:chromium   # node scripts/chromium-smoke.mjs (Playwright's chromium)
```

- Both run the checks in `scripts/checks.mjs` against the loaded module:
  handles, snapshots, `epoch()`, stable handles across a load,
  `validateModelFiles`, errors through the factory, `free()`, and the
  staging, serializer and DCS bindings.
- `node-smoke.cjs [module]` also takes a module to `require`. For example,
  from the concerto checkout:
  `node ../concerto-rust/concerto-wasm/scripts/node-smoke.cjs @accordproject/concerto-engine`
  runs the checks through the workspace link.
- `hashdos.mjs [module]` is the WASM HashDoS check. It builds the
  `hashdos_keys` example (examples/hashdos_keys.rs) for wasm32 with
  wasm-bindgen's Node glue, which crafts object keys that collide under the
  standard library's fixed-key hasher on wasm32, and shows them quadratic in
  a std map with that hasher. It then hands the same keys, and ordinary
  ones, to the engine's JSON entry points (`validateInstance`,
  `serializerFromJsonCompact`, `checkAstShape`, `validateAstValue`,
  `dcsValidate`) and requires the crafted ones to cost about the same: the
  engine parses untrusted JSON into `concerto_core::json::Value`, whose maps
  are seeded at instantiation. Needs cargo, the wasm32 target and
  wasm-bindgen-cli, as `build.sh` does.
- The Chromium smoke runs in the Playwright headless shell and in full
  Chromium. In each, it:
  - probes the main thread's synchronous-compile limit and checks the module
    is under it;
  - runs the checks on the main thread and in a module Worker;
  - runs the async fallback;
  - times the handle API's calls.

`results/` holds the output of the runs below.

## The spike, on the final crate

The WASM spike is written up in
[`spikes/wasm/REPORT.md`](../spikes/wasm/REPORT.md). The smokes repeat its
browser and boundary measurements on this crate. Conditions: macOS 13 on an
i7-7820HQ, shared with other jobs, so timings are noisy.

**Size.** 1,789,836 bytes after `wasm-opt -Oz` (the size-optimised build this
section measured; the current speed-optimised build is about 2.6 MB), against the spike's 1,115,534;
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
Chromium main thread, headless shell / full Chromium), measured on a
handle API that also had `generation()` and declaration and property
handles (`chromium-smoke.mjs` times `epoch()`, `modelFileId`, `getTypeName` and
`modelFileSnapshot` instead):

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
  element's whole state. A view that caches it and checks a mutation
  counter (`generation()` was measured, at the same cost as `epoch()`; the
  views now key on their own `EngineState.version`) pays that cost once per
  element per mutation.

Nothing here changes the spike's finding that the load cost is
concerto-core's own rather than the boundary's; these numbers give no
reason for a NAPI addon.

Firefox, WebKit and CSP (`'wasm-unsafe-eval'`) were not measured, as in the
spike.

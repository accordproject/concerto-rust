# P5-42 spike: where the resident DCS extract call spends its time, and two prototype walks

Measure-only spike for the F-A design task (accordproject/concerto-rust#352).
Nothing here is shipped, and nothing depends on it. The crate declares its own
`[workspace]`, so it is not a member of the root workspace.

## What it runs

`src/main.rs` takes one of the P5-30 dumped inputs. Each input is the exact
`models` array the TS API hands the engine for `DecoratorManager.extractDecorators`
(the files are `spikes/p530-wasm-profile` data, `concerto-core-test-data.json`,
`conformance.json` and `synthetic-large.json`). The spike builds the input
manager once, untimed, the way the resident path keeps it (P5-27
`DcsManagerHandle`). Then, on every iteration, it times these steps natively
with the shipped release profile (opt 3, fat LTO, 1 CGU, panic=abort):

| stage | what it is |
|---|---|
| `extract_total` | `dcs::extract_decorators(&manager, remove=false)`, the whole call |
| `resolve` | `ModelManager::ast(resolve, system)`: the resolved `Value` tree the extractor clones and walks (inside `extract_total`) |
| `result_build` | `ModelManager::new()`, loading the resolved non-system models, and `validate_models()`: how `DecoratorExtractor::extract` builds the result manager (inside `extract_total`) |
| `new_mm` | `ModelManager::new()` alone (inside `result_build`) |
| `validate_only` | `validate_models()` on the already-loaded input manager: the validation share of `result_build` |
| `encode` | the P5-41 direct encode shape: the result's model ASTs (borrowed), command sets and vocabularies to JSON text |
| `stage_clone` | `stage_result`'s clone of each non-system result `ModelFile` |
| `drop` | dropping the `ExtractResult` |
| `walk_borrowed` | prototype: walks the input manager's own (unresolved) ASTs, borrowing each node's `decorators`, and builds the command sets as `Value`s |
| `walk_typed` | prototype: the same, reading the typed model (`ModelFile`, `Declaration`, `Property`, `MapDeclaration` key/value, `Decorator`, `DecoratorArgument`) |
| `walk_typed_direct` | prototype: the typed walk collecting borrowed hits, with the command sets serialised straight to JSON text through a serde view (no `Value`) |
| `cold_source_build_us` | one-off: building the input manager from the models, which is what a cold `DcsManagerHandle::new` does after the JSON parse |

All three prototypes produce the same number of commands and vocabulary entries
as the real extractor on all three inputs. The binary checks this and prints
the counts. The prototypes are not parity-exact ports:

- they resolve type-reference arguments with `resolve_type_name` instead of
  copying the resolved AST node;
- they write simplified vocabulary entries instead of the YAML;
- they do not strip decorators (`removeDecoratorsFromModel`);
- they do not fall back to the raw AST for a decorator that the typed model
  cannot represent losslessly.

They only measure what the walk and the output building cost.

## Run

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=<per-task dir> cargo build --release
for r in 1 2 3; do for s in synthetic-large conformance concerto-core-test-data; do
  <target>/release/p542-native <p530 data>/$s.json --iters 60 --warmup 10 > results/r$r-$s.json
done; done
```

## Results

The results are in `results/`. `medians.md` holds the median of the three
round medians, and `loads.txt` holds the load average before each part (the
1-minute load was below 0.6 throughout). The machine was the cloud container:
4 vCPU Xeon @ 2.10GHz, rustc 1.94.1, concerto-rust at `369e6ea` (the
integration head, with P5-27 and P5-41 merged).

## Through the TS API (`results/ts-api/`)

The `scripts/` directory holds scratch drivers. They were run against the
concerto integration head `6561368bb` (the dist built in this worktree) and
against the engine that `concerto-wasm/build.sh` builds from this head.
`wasm-opt` is not installed on this machine, so the engine is not
wasm-opt'd, which P5-30 found about 10% slower. The Rust figures are
therefore pessimistic.

- `cw.js`: cold versus warm `DecoratorManager.extractDecorators`. The
  cold call is the first call on a freshly loaded source manager. The warm
  call is a repeat on the same manager, which is the resident P5-27 path.
  TS 5.0.0 comes from `migration/oracle/reference`. The script ran three
  interleaved rounds of 15 samples each. See `medians.md` and `loads.txt`.
- `cold.js`: the parts of the cold call's extra cost. It times
  `getAst(true, false)` and `new DcsManagerHandle(models)` separately.
- `dv.js`, `dv2.js` and `hide.cjs`: an error-parity divergence found while
  designing, recorded in `decorator-validation.txt`. On the Rust engine,
  `fromAst` into a manager with `decoratorValidation` set to `error` does
  not throw for an undeclared decorator, while TS 5.0.0 throws
  `IllegalModelException`. `decorateModels` inherits this behaviour on both
  the resident path and the per-call path (`hide.cjs` hides
  `DcsManagerHandle`).

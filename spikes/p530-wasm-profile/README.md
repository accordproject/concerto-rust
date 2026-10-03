# P5-30: why is the DCS path slower in WASM than native? (measure only)

Task P5-30 (N2, accordproject/concerto-rust#335), from P5-22's finding
(#326, concerto `migration/bench/RESULTS.md`):
`decoratorManagerExtractDecorators` took 45 ms inside WASM on
synthetic-large, against 18 ms for `extract_decorators` in the native crate.

Nothing here is shipped, and no engine code changed. The crate runs the
exact body of the `decoratorManagerExtractDecorators` binding
(`concerto-wasm/src/lib.rs`), split into stages, both natively and in WASM,
from the same input text, so each stage can be timed and profiled on both
sides:

| stage | what it is in the binding |
|---|---|
| `copyIn` (WASM only) | wasm-bindgen copying the input string into linear memory |
| `parse` | `serde_json::from_str` of the models text (`to_json`) |
| `rebuild` | `model_manager_from_asts` |
| `extract` | `dcs::extract_decorators` |
| `encode` | `extract_result_to_js` plus `serde_json::to_string` (`to_js`) |
| `drop` | freeing what the call built |
| `copyOut`, `jsParse` (WASM only) | the output string back to JS, then `JSON.parse` |

## Files

- `src/lib.rs`: the stages, the wasm-bindgen exports (`wasm` feature), and
  three microbenchmarks: allocator churn, SipHash and `memcpy`.
- `src/main.rs`: the native driver (`p530-native`).
- `scripts/dump-engine.cjs`: a `CONCERTO_ENGINE_MODULE` shim that writes
  the `models` argument the shipped engine receives through the TS API
  (`p515-sweep.mjs --ops extract_decorators`) to a file. Both sides read
  those files, so the input is byte-identical to the TS-API path.
- `scripts/run-wasm.mjs`: the WASM driver. It also times the shipped
  binding end to end on the same input, and has a `--loop` mode for
  `node --cpu-prof`.
- `scripts/summarize.mjs`: self time by cause, for a symbolised V8 CPU
  profile or for a callgrind profile of the native binary.
- `build-variants.sh`: the WASM variants (shipped settings, named, no
  wasm-opt, `+simd128`, talc allocator, `opt-level` 2 and "s", no LTO) and
  the native variants (glibc malloc, dlmalloc, allocation counting).
- `p530-run.sh`: the profiles phase and the timed phase (three interleaved
  rounds behind the P5-15/P5-22 quiet gate).
- `results/`: the raw outputs of the run reported on #335.

## Reproducing

```sh
# in a concerto checkout next to this concerto-rust one, with concerto-core
# built and migration/oracle/reference installed:
P530_REAL_ENGINE=$PWD/../concerto-rust/concerto-wasm/pkg/concerto-engine.cjs \
P530_DUMP=/tmp/p530/synthetic-large.json \
CONCERTO_ENGINE_MODULE=$PWD/../concerto-rust/spikes/p530-wasm-profile/scripts/dump-engine.cjs \
  node migration/bench/p515-sweep.mjs --ops extract_decorators --sets synthetic-large --samples 1 --warmup 0

# here, with wasm-bindgen 0.2.128 and binaryen's wasm-opt on PATH:
CARGO_TARGET_DIR=/tmp/p530/target sh build-variants.sh /tmp/p530/v
CONCERTO=... ENGINE=... ENGINE_NAMED=... V=/tmp/p530/v DATA=/tmp/p530 sh p530-run.sh timed /tmp/p530/out
```

## Findings (timed run, 2026-09-29)

Cloud container: 4 vCPU Xeon @ 2.10GHz, node v22.22.2, rustc 1.94.1,
wasm-bindgen 0.2.128, wasm-opt 132. concerto 7dc28bafd, concerto-rust
98e0809. Three interleaved rounds, each phase behind the quiet gate
(1-min load < 2, 5-min load < 3); every gate passed at loads 1.25 to 1.69
(`results/timed/timed-loads.txt`). The figures are medians of the three
per-round medians (`results/timed/medians.txt`, `scripts/aggregate.mjs`).

synthetic-large, ms:

| stage | native glibc | native dlmalloc | WASM (shipped settings) |
|---|---|---|---|
| parse | 1.68 | 1.72 | 1.98 |
| rebuild | 3.95 | 3.96 | 5.07 |
| extract | 13.39 | 12.61 | 14.63 |
| encode | 7.86 | 5.83 | 6.48 |
| drop | 3.00 | 1.90 | 1.87 |
| **same work, total** | **30.21** | **26.08** | **30.24** |
| copyIn + copyOut + JSON.parse (WASM only) | | | 3.05 |
| whole body in one call | | | 32.67 |
| shipped `decoratorManagerExtractDecorators` binding | | | 40.21 |
| TS-API path, Rust engine (`p515-sweep`) | | | 50.25 |
| TS-API path, TS reference 5.0.0 | | | 7.81 |

- For the same work, WASM runs at 1.00x native glibc and 1.16x native
  dlmalloc. P5-22's "45 ms vs 18 ms" compared the whole binding (JSON in,
  rebuild, extract, encode, JSON out) with the native extract stage alone.
  Measured stage for stage, extract is 14.6 ms in WASM against 13.4 ms
  native (1.09x).
- The gap to TS (6.4x on the TS-API path) is in the algorithm, not in WASM:
  native Rust doing the same work takes 26 to 30 ms, against 7.8 ms for the
  whole TS call. One call makes about 398k allocations (extract alone
  220k allocations and 21.4 MB, from `native-count-alloc`).
- Profiles (symbolised V8 `--cpu-prof`, `results/profiles/`): allocation
  29% (dlmalloc malloc/free, `__rdl_dealloc`), SipHash and indexmap probes
  14%, `Value`/`String` clones 9% plus `clone_into` 4%, JSON encode 8%,
  drop 6%. The shipped engine profile has the same shape. Native callgrind
  (instruction counts): glibc malloc/free 52%, SipHash about 5%.
- Build and runtime variants (stages total, shipped 30.24 ms): talc 29.79,
  `+simd128` 33.91, no LTO 34.38, no wasm-opt 33.34, `opt-level=2` 39.59,
  `opt-level="s"` 41.03. V8 flags on the shipped build:
  `--no-wasm-bounds-checks` 31.65, `--wasm-enforce-bounds-checks` 35.96,
  `--no-liftoff` 33.90, `--experimental-wasm-inlining` 30.97. None beats
  the shipped settings by more than the round-to-round noise, so the
  settings are not the cause. Bounds checks cost nothing on x64 (V8 uses
  guard pages), and i64/f64 conversions do not show in the profiles.

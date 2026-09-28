# Native Rust benchmarks through the public API

**Task:** P6-04 (accordproject/concerto-rust#273), plan decision D11
(accordproject/concerto-rust#29). Depends on P6-01 (#83, the public API
design, `docs/public-api.md`) and P6-02 (#84, the native acceptance
example, `docs/native-guide.md`). Requested by the maintainer on
2026-09-27, **for interest — it does not block any gate or release.**
Measurement only; no engine change.

This benchmarks `concerto-core` the way `docs/native-guide.md` uses it:
through the D11 public API alone, no TypeScript, no WASM, no `js-compat`
feature. It sits alongside the existing P5-04 criterion suite (task #75,
`benches/load_validate.rs`, `benches/validate_metamodel.rs`,
`benches/instance_validate.rs`), which times a mix of public and internal
entry points for comparison, and reports the result next to those P5-04
numbers and the TS reference so the three routes line up.

## Where the numbers are

- **The Rust source:** `benches/benches/public_api.rs` — see its module
  docs for exactly which public API item each workload calls, and why it
  differs from the P5-04 files (in short: `add_model_ast` instead of the
  deprecated `add_model`, `ModelManager::validate_instance`/
  `check_instance` instead of the `js-compat`-only `instance::validate`
  and `concerto-core-js`'s `Serializer`).
- **Raw results:** `benches/results/P6-04-native-{1,2}.json` in this repo
  (native, `extract-results.sh`'s format), and, in the `concerto` repo,
  `migration/bench/results/P6-04-{ts-reference-5.0.0,rust-via-ts}-{1,2}.json`
  (`run-ts.mjs`'s format).
- **The full three-way table, machine/toolchain record, and discussion:**
  `migration/bench/RESULTS.md` in the `concerto` repo (the "P6-04" section,
  at the top). This page is a short pointer into it, not a duplicate of it.

## What it covers

| Workload | Public API called | Comparable P5-04 workload |
|---|---|---|
| model load | `ModelManager::add_model_ast` (+ batch `add_model_asts`) | `load_validate.rs`'s `load` (uses the deprecated `add_model`) |
| model validate | `ModelManager::validate_models` | `load_validate.rs`'s `validate` (same call) |
| validateAst | `concerto_core::metamodel::validate_ast` (the crate-root free function — see the caveat below) | `validate_metamodel.rs`'s `concerto-core/validate_ast` (`ModelManager::validate_ast`, the resident-metamodel method) |
| instance populate + validate | `ModelManager::validate_instance` (first error), `ModelManager::check_instance` (collect-all, accordproject/concerto#1239) | `instance_validate.rs`'s `validate_instance_native` (same call, benchmarked there too) and `from_json` (`concerto-core-js`'s `Serializer`, not public API) |
| serialisation | *not benchmarked* — `Serializer`/`Factory`/`Resource`/`InstanceGenerator` are explicitly out of D11's scope (`docs/public-api.md` §1) and live in the unpublished `concerto-core-js` crate | `instance_validate.rs`'s `from_json` |

Same model sets as P5-04: `concerto-core-test-data` (35 files),
`conformance` (41 files), `synthetic-large` (1 file, 300 declarations), and
500 generated instances of one synthetic model — all loaded from
`migration/bench/fixtures/` in the `concerto` repo, so every route in the
table compares byte-identical models.

## The headline numbers (two runs, 2026-09-28; see RESULTS.md for the full table)

Speed relative to the TS 5.0.0 reference, median of two runs (lower is
faster):

| Workload | Native Rust / TS | Rust via TS API (WASM) / TS |
|---|---|---|
| load (conformance) | 4.4× slower | 7.7× slower |
| validate (conformance) | 3.2× slower | 4.9× slower |
| validateAst (conformance) | 9.3× slower¹ | **2.4× faster** |
| instance populate+validate (500 synthetic) | 1.1× slower | 3.8× slower |

¹ **Not a like-for-like validateAst comparison** — the native number times
the crate-root free function, which rebuilds the metamodel check on every
call; the WASM route goes through the resident-metamodel method
(`ModelManager::validate_ast`), paying that cost once per manager instead
of once per call. See RESULTS.md's "The validateAst outlier" for the full
explanation and the open question it raises for a public, resident-metamodel
validateAst entry point.

The one clear win for a native caller today is **instance
populate-and-validate**: `ModelManager::validate_instance` is close to the
TS reference directly (1.1×) and 3.4× faster than the same work done
through the TS public API, since it pays no WASM marshalling cost per
call.

## Running it yourself

```sh
cd concerto-rust
cargo bench --manifest-path benches/Cargo.toml --bench public_api
./benches/extract-results.sh benches/results/public-api.json
```

See `benches/README.md` for the fixture-generation step
(`generate-fixtures.mjs` in the `concerto` repo) this needs first, and for
the `CONCERTO_REPO` override if `concerto` is not checked out as this
repo's sibling.

## No CI gate

Per the task, this is informational: no regression threshold is enforced
in CI, and none is proposed here. `cargo bench --manifest-path
benches/Cargo.toml` (which already runs every target, including
`public_api`, with no extra flag needed) remains the one documented
command, per `benches/README.md`.

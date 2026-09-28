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
  `migration/bench/RESULTS.md` in the `concerto` repo (the "P6-04" section;
  the "P5-21" section above it has the updated validateAst figures).
  This page is a short pointer into it, not a duplicate of it.

## What it covers

| Workload | Public API called | Comparable P5-04 workload |
|---|---|---|
| model load | `ModelManager::add_model_ast` (+ batch `add_model_asts`) | `load_validate.rs`'s `load` (uses the deprecated `add_model`) |
| model validate | `ModelManager::validate_models` | `load_validate.rs`'s `validate` (same call) |
| validateAst | `concerto_core::metamodel::validate_ast` (the crate-root free function; resident metamodel since P5-21 — see footnote 1 below) | `validate_metamodel.rs`'s `concerto-core/validate_ast` (`ModelManager::validate_ast`, the resident-metamodel method) |
| instance populate + validate | `ModelManager::validate_instance` (first error), `ModelManager::check_instance` (collect-all, accordproject/concerto#1239) | `instance_validate.rs`'s `validate_instance_native` (same call, benchmarked there too) and `from_json` (`concerto-core-js`'s `Serializer`, not public API) |
| serialisation | *not benchmarked* — `Serializer`/`Factory`/`Resource`/`InstanceGenerator` are explicitly out of D11's scope (`docs/public-api.md` §1) and live in the unpublished `concerto-core-js` crate | `instance_validate.rs`'s `from_json` |

Same model sets as P5-04: `concerto-core-test-data` (35 files),
`conformance` (41 files), `synthetic-large` (1 file, 300 declarations), and
500 generated instances of one synthetic model — all loaded from
`migration/bench/fixtures/` in the `concerto` repo, so every route in the
table compares byte-identical models.

## The headline numbers (two runs, 2026-09-28; see RESULTS.md for the full table)

Speed relative to the TS 5.0.0 reference (lower is faster). `Rust via TS
API (WASM) / TS` is the median of its two runs, both run on a quiet
machine. `Native Rust / TS` uses native **run 1 only**: native run 2 ran
under load contention (another workload started on the machine partway
through that criterion run) and is excluded from the ratio — see
RESULTS.md's "Native round 2 was contended" for the recorded loads and why
this table doesn't use the run 1/2 median here:

| Workload | Native Rust / TS² | Rust via TS API (WASM) / TS |
|---|---|---|
| load (conformance) | 4.1× slower | 7.7× slower |
| validate (conformance) | 2.5× slower | 4.9× slower |
| validateAst (conformance) | **3.4× faster**¹ (was 7.4× slower before P5-21) | **2.4× faster** |
| instance populate+validate (500 synthetic) | **1.1× faster** | 3.8× slower |

¹ **Updated by P5-21 (accordproject/concerto-rust#319).** P6-04's native
figure (7.4× slower) timed the crate-root free function
`concerto_core::metamodel::validate_ast` when it still rebuilt the
metamodel check on every call, while the WASM route went through the
resident-metamodel method (`ModelManager::validate_ast`, P5-13), paying
that cost once per manager. P5-21 made the free function run on a
per-thread resident metamodel manager, with the same public signature,
results and error kinds. Re-measured on 2026-09-28 (criterion
`validate_metamodel.rs`'s `concerto-core/metamodel::validate_ast`, results
in `benches/results/P5-21/native-{1,2}.json`): conformance 60.6 / 61.7 µs
per model (P6-04 measured 2230 µs, on a different machine), against
202.6 / 215.4 / 204.5 µs for the TS 5.0.0 reference on the same machine
(median 204.5 µs, so 0.30×, about 3.4× faster); concerto-core-test-data
147.1 / 147.0 µs against a TS median of 521.2 µs (0.28×). That machine
was shared (TS start loads 3.59, 2.04 and 2.24, above P6-04's quiet gate
of 2), so the figures are indicative, but the margin is well above the
up-to-1.6× contention effect P6-04 recorded. The other columns of this
table are still P6-04's.
See RESULTS.md's "P5-21" section (in the `concerto` repo) for the runs,
machine loads and the TS raw results, and "The validateAst outlier" for
the original P6-04 analysis.

² Native run 1 only (quiet start to finish), against the TS median.
Native run 2 ran under load contention (RESULTS.md's "Native round 2 was
contended"); folding it in via the run 1/2 median, as the rest of this
table's ratios do for the other routes, would show every native/TS ratio
1.2×-1.6× worse, including a **slower**, not faster, instance
populate+validate figure — that contended figure is not reported here.

The one clear win for a native caller today is **instance
populate-and-validate**: on the quiet run, `ModelManager::validate_instance`
is slightly faster than the TS reference directly (0.88×) and 4.3× faster
than the same work done through the TS public API, since it pays no WASM
marshalling cost per call.

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

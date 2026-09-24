# benches/

Criterion benchmarks for task **P5-04a** (issue accordproject/concerto-rust#92,
under the migration plan accordproject/concerto-rust#29): a benchmark
against the TypeScript runtime on the same models, so every later phase of
the migration can show its speed-up against a committed baseline.

This is the Rust half. The TS half lives in `accordproject/concerto`'s
`migration/bench/`, which also generates the fixtures both harnesses load
(see below) - so both sides run against byte-identical models.

## Layout

- `Cargo.toml` - a standalone workspace member, `concerto-benches`,
  `publish = false`, not linked from any published crate.
- `benches/common/` - shared fixture-loading code (not a bench target
  itself; see the `autobenches = false` note in `Cargo.toml`).
- `benches/load_validate.rs` - workload 1.
- `benches/validate_metamodel.rs` - workload 2.
- `extract-results.sh` - pulls a small JSON summary out of criterion's
  `target/criterion/**/estimates.json` output (see "Baseline" below).
- `results/` - the committed output of `extract-results.sh` (one file per
  run, named by timestamp).

Workload 3 (instance validation) is TS-only for now, per the issue -
`concerto-core` has no instance layer yet (see the migration plan, §1.2:
"there is... no instance layer"); it is added here once that lands, after
task P3-01.

## Repo layout this assumes

Per the migration plan's repo layout, this repo, `concerto` and (for
workload 2) `concerto-validate-rs` are checked out as siblings, e.g. all
under `/home/user/`. Override with the `CONCERTO_REPO` environment
variable if `concerto` is not there (see `benches/common/mod.rs`); if
`concerto-validate-rs` is not at `/home/user/concerto-validate-rs`, build
with `--no-default-features` to skip the parts of workload 2 that need it
(see "Running" below).

`concerto-validate-rs`'s `build.rs` downloads and overwrites its own
tracked `metamodel.json` from the network on every `cargo build`/`bench`
that touches it (a known issue - see the migration plan's §1.3). Restore
it afterwards: `git -C /home/user/concerto-validate-rs checkout
metamodel.json` (harmless to skip if there is no network access to
overwrite it with in the first place).

## Fixtures

Run `node migration/bench/generate-fixtures.mjs` in the `concerto` repo
first (see that repo's `migration/bench/README.md`) - it converts
`concerto-core`'s test data and the `concerto-conformance` AST fixtures,
and generates the synthetic large model, all as plain AST JSON under
`migration/bench/fixtures/model-sets/` there. This crate reads that
directory directly (`benches/common/mod.rs`), so both harnesses always
load the exact same models; nothing is duplicated into this repo.

## Running

**The one documented Rust benchmark command**, from this repo's root:

```sh
cargo bench -p concerto-benches
```

(Add `--no-default-features` to skip the `concerto-validate-rs` half of
workload 2 if that repo is not checked out.)

Criterion's usual options apply, e.g. `-- --quick` for a fast sanity run
with fewer samples, or `-- --save-baseline <name>` to name a run for later
comparison with `critcmp` or criterion's own baseline diffing.

### Workloads

1. **`load_validate`** - for each of the three model sets (see the
   fixtures README), `ModelManager::add_model` for every model into a
   fresh manager (`load`), then `ModelManager::validate_models` once over
   the whole loaded set (`validate`). Mirrors the TS harness's
   `AstModelManager`/`validateModelFiles` split exactly, so the two
   `load` numbers and the two `validate` numbers are directly comparable.
2. **`validate_metamodel`** - `ModelFile::from_json` (concerto-core's
   structural check - the counterpart to the TS harness's `validateAst`,
   which also runs a structural check against the metamodel schema) and,
   when the `validate-rs` feature is enabled (the default),
   `concerto_validator_rs::validate_metamodel` from the separate
   `concerto-validate-rs` crate, over the same fixtures.

### On "where Rust has the capability"

The exit condition asks for a baseline "where Rust has the capability".
Concretely, in the current state of the trial port (task #81) and P1-05
(#42):

- `validate_models` does not yet accept every fixture that TS's
  `validateModelFiles` accepts - specifically, a relationship to a class
  whose identifier comes through inheritance from a supertype is
  currently rejected (see DIVERGENCES.md and the plan's §1.2 gap list:
  "explicit-over-explicit identity and inherited identifier lookup").
  This affects the `concerto-core-test-data` and `synthetic-large` model
  sets' `validate` benchmark (not `load`, which is purely structural).
- `concerto-validate-rs` is a separate, much less complete
  implementation (see the plan's §1.3 "confirmed bugs" list) and does
  not yet accept every fixture either.

Rather than fail the whole run over a known gap, both benches check each
case once outside the timed section and:
- skip a `validate` benchmark entirely (with a message on stderr) if the
  whole model set fails, since `validate_models` has no partial mode; or
- benchmark only the subset of models `concerto-validate-rs` currently
  accepts (also reported on stderr), since it is a per-file check.

Every skip and partial-coverage note is printed to stderr when you run
`cargo bench`, and is not silently absorbed into a number.

## Baseline

After running `cargo bench -p concerto-benches`, extract a committed
summary with:

```sh
benches/extract-results.sh
```

This writes `benches/results/<timestamp>-rust.json` (median and mean, in
nanoseconds, both per batch and per logical operation - each `n` matches
the model count in the corresponding `id`). Criterion's own much larger
`target/criterion/` HTML/SVG report tree is not committed (it is already
`.gitignore`d, via `/target`); this summary is what "raw results ...
committed" means for the Rust side.

The combined TS-vs-Rust baseline table lives in `concerto`'s
`migration/bench/RESULTS.md`, built from a `run-ts.mjs` run and a
`cargo bench` + `extract-results.sh` run recorded on the same machine.

//! Workload 1 (see the plan, task P5-04a): loading and validating a model
//! set. Benchmarks the same three model sets the TS harness runs
//! (`migration/bench/run-ts.mjs` in the `concerto` repo), loaded from the
//! same AST fixtures under `migration/bench/fixtures/model-sets/` there -
//! see `benches/common/mod.rs` and this crate's README.
//!
//! Each iteration times two phases separately, matching the TS side
//! (`AstModelManager::addModel` there is structural-load-then-semantic-
//! validate per file; here we do the structural load for every file first,
//! `ModelFile::from_json`, then one whole-manager semantic pass,
//! `validate_models`):
//!   - `load`: `ModelManager::add_model` for every model in the set, into a
//!     fresh manager.
//!   - `validate`: `ModelManager::validate_models` once, over the whole set.

use concerto_core::ModelManager;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

#[path = "common/mod.rs"]
mod common;

fn load_only(set: &[(String, serde_json::Value)]) -> ModelManager {
    let mut mgr = ModelManager::new().expect("system model loads");
    for (name, ast) in set {
        mgr.add_model(ast, Some(name.clone()))
            .unwrap_or_else(|e| panic!("loading {name}: {e}"));
    }
    mgr
}

fn bench_model_set(c: &mut Criterion, set_name: &str) {
    let set = common::load_model_set(set_name);
    assert!(!set.is_empty(), "fixture set '{set_name}' is empty - run generate-fixtures.mjs in the concerto repo first");

    let mut group = c.benchmark_group(format!("load_validate/{set_name}"));
    group.bench_with_input(
        BenchmarkId::new("load", set.len()),
        &set,
        |b, set| {
            b.iter(|| load_only(set));
        },
    );

    // Rust's `validate_models` is not yet at parity with the TS reference
    // for every model (see the plan's §1.2 gap list, and DIVERGENCES.md) -
    // some of these fixtures are chosen only for being self-contained and
    // valid *on the TS side*. Per the exit condition ("baseline table ...
    // where Rust has the capability"), we check once, outside the timed
    // section, and skip this half of the workload rather than fail the
    // whole run when a fixture set hits a known gap.
    match load_only(&set).validate_models() {
        Ok(()) => {
            group.bench_with_input(
                BenchmarkId::new("validate", set.len()),
                &set,
                |b, set| {
                    // Loading is excluded from the timed section: we build a
                    // fresh, already-loaded manager once per sample and only
                    // time `validate_models`.
                    b.iter_batched(
                        || load_only(set),
                        |mgr: ModelManager| mgr.validate_models().expect("set validates"),
                        criterion::BatchSize::LargeInput,
                    );
                },
            );
        }
        Err(e) => {
            eprintln!(
                "load_validate/{set_name}/validate: SKIPPED - validate_models() does not yet \
                 pass on this fixture set ({e}); see PORTING.md / DIVERGENCES.md"
            );
        }
    }

    group.finish();
}

fn benches(c: &mut Criterion) {
    for set_name in ["concerto-core-test-data", "conformance", "synthetic-large"] {
        bench_model_set(c, set_name);
    }
}

criterion_group!(load_validate, benches);
criterion_main!(load_validate);

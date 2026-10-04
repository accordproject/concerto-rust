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
//!
//! Two more ids time the load from JSON text, as the WASM bindings receive
//! it (`JSON.stringify(ast)`), for the typed AST spike (P5-06c,
//! accordproject/concerto-rust#234):
//!   - `load_text_value`: `serde_json::from_str` into a `Value`, then
//!     `ModelManager::add_owned_model_with_definitions` (the bindings'
//!     path before P5-06c);
//!   - `load_text_typed`: `ModelFile::from_json_text`, then
//!     `ModelManager::add_model_file` (the typed AST path).

use concerto_core::{ModelFile, ModelManager};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

#[path = "common/mod.rs"]
mod common;

fn load_only(set: &[(String, concerto_core::json::Value)]) -> ModelManager {
    let mut mgr = ModelManager::new().expect("system model loads");
    for (name, ast) in set {
        mgr.add_model(ast, Some(name.clone()))
            .unwrap_or_else(|e| panic!("loading {name}: {e}"));
    }
    mgr
}

fn load_text_value(texts: &[(String, String)]) -> ModelManager {
    let mut mgr = ModelManager::new().expect("system model loads");
    for (name, text) in texts {
        let value: concerto_core::json::Value = serde_json::from_str(text).expect("fixture is JSON");
        mgr.add_owned_model_with_definitions(value, None, Some(name.clone()))
            .unwrap_or_else(|e| panic!("loading {name}: {e}"));
    }
    mgr
}

fn load_text_typed(texts: &[(String, String)]) -> ModelManager {
    let mut mgr = ModelManager::new().expect("system model loads");
    for (name, text) in texts {
        let file = ModelFile::from_json_text(text, None, Some(name.clone()))
            .expect("fixture is JSON")
            .unwrap_or_else(|e| panic!("loading {name}: {e}"));
        mgr.add_model_file(file)
            .unwrap_or_else(|e| panic!("loading {name}: {e}"));
    }
    mgr
}

fn bench_model_set(c: &mut Criterion, set_name: &str) {
    let set = common::load_model_set(set_name);
    assert!(!set.is_empty(), "fixture set '{set_name}' is empty - run generate-fixtures.mjs in the concerto repo first");
    let texts: Vec<(String, String)> = set
        .iter()
        .map(|(name, ast)| (name.clone(), ast.to_string()))
        .collect();

    let mut group = c.benchmark_group(format!("load_validate/{set_name}"));
    group.bench_with_input(
        BenchmarkId::new("load", set.len()),
        &set,
        |b, set| {
            b.iter(|| load_only(set));
        },
    );
    group.bench_with_input(
        BenchmarkId::new("load_text_value", texts.len()),
        &texts,
        |b, texts| {
            b.iter(|| load_text_value(texts));
        },
    );
    group.bench_with_input(
        BenchmarkId::new("load_text_typed", texts.len()),
        &texts,
        |b, texts| {
            b.iter(|| load_text_typed(texts));
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

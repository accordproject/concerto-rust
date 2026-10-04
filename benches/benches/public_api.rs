//! Task P6-04 (accordproject/concerto-rust#273, plan accordproject/concerto-rust#29):
//! native Rust benchmarks through the **P6-01 public API**
//! (`docs/public-api.md`, accordproject/concerto-rust#83), as opposed to
//! `load_validate.rs`, `validate_metamodel.rs` and `instance_validate.rs`
//! (task P5-04, #75), which time internal entry points (`ModelFile::from_json`,
//! the deprecated `ModelManager::add_model`, `add_owned_model_with_definitions`,
//! the `js-compat`-only `instance::validate::validate_instance`, and the
//! `concerto-core-js` `Serializer`) alongside the public ones.
//!
//! Everything in this file compiles and runs with **no `js-compat` feature**
//! and **no `concerto-core-js` dependency** — the same default-feature
//! surface `concerto-core/examples/standalone.rs` and `docs/native-guide.md`
//! walk through. (This crate's `Cargo.toml` still enables `js-compat` on
//! `accordproject-concerto-core` for the *other* bench files' sake; this
//! file simply never calls anything that feature gates.)
//!
//! Covers the workloads P6-04 asks for:
//!   - **model load**: [`concerto_core::ModelManager::add_model_ast`] (one
//!     call per file, the stable replacement for the deprecated `add_model`
//!     `load_validate.rs` still uses) and
//!     [`concerto_core::ModelManager::add_model_asts`] (the batch form,
//!     which loads and then validates the whole set in one call, rolling
//!     back on any failure — a different entry point with different
//!     rollback cost, not just a loop over `add_model_ast`).
//!   - **model validate**: [`concerto_core::ModelManager::validate_models`]
//!     (already the public entry point; unchanged from `load_validate.rs`,
//!     repeated here so this file stands alone as the public-API baseline).
//!   - **validateAst**: [`concerto_core::metamodel::validate_ast`], the
//!     crate-root free function, which since P5-21 runs on a per-thread
//!     resident metamodel manager (`ModelManager::validate_ast(&ModelFile)`,
//!     the resident-metamodel method form P5-13 optimised, is already
//!     benchmarked in `validate_metamodel.rs`).
//!   - **instance populate and validate**:
//!     [`concerto_core::ModelManager::validate_instance`] (first error) and
//!     [`concerto_core::ModelManager::check_instance`] (collect-all,
//!     accordproject/concerto#1239). Per P5-13/P6-01 (`docs/public-api.md`
//!     section 5.7, finding F8), both now read the document the way TS
//!     `Serializer.fromJSON` does (`instance::from_json`) — populate *and*
//!     validate in one call — unlike the `js-compat`-only
//!     `instance::validate::validate_instance` in `instance_validate.rs`,
//!     which times the `ResourceValidator` walk alone over an
//!     already-populated value.
//!   - **serialisation**: *not exposed*. Typed instance objects and JSON
//!     generation (`Serializer`, `Factory`, `Resource`,
//!     `InstanceGenerator`) are explicitly out of the D11 scope
//!     (`docs/public-api.md` section 1) and live in the unpublished
//!     `concerto-core-js` crate, not in `concerto-core`'s public API. There
//!     is nothing to benchmark here through the public API; see
//!     `instance_validate.rs`'s `from_json` id for the `concerto-core-js`
//!     `Serializer` number instead.

use concerto_core::instance::ValidationOptions;
use concerto_core::{metamodel, ModelManager};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use concerto_core::json;
use concerto_core::json::Value;

#[path = "common/mod.rs"]
mod common;

const NUM_INSTANCES: usize = 500;

// ---------------------------------------------------------------------
// Workloads 1 and 2 (model sets): load and validate through the public API
// ---------------------------------------------------------------------

/// [`ModelManager::add_model_ast`] for every model in the set, into a fresh
/// manager. The stable, non-deprecated counterpart to `load_validate.rs`'s
/// `load_only` (which uses `add_model`, `#[deprecated]` since P6-01).
fn load_via_public_api(set: &[(String, Value)]) -> ModelManager {
    let mut mgr = ModelManager::new().expect("system model loads");
    for (name, ast) in set {
        mgr.add_model_ast(ast, Some(name))
            .unwrap_or_else(|e| panic!("loading {name}: {e}"));
    }
    mgr
}

/// [`ModelManager::add_model_asts`]: the batch entry point. Loads the whole
/// set and validates the manager once, rolling every file back if either
/// step fails — a different cost shape from `load_via_public_api` followed
/// by a separate `validate_models` call (this crate's `load_validate.rs`
/// workload), since a mid-batch snapshot/rollback happens on every call,
/// not only on failure.
fn load_and_validate_batch(set: &[(String, Value)]) -> Result<ModelManager, concerto_core::Error> {
    let mut mgr = ModelManager::new().expect("system model loads");
    let models = set.iter().map(|(name, ast)| (ast, Some(name.as_str())));
    mgr.add_model_asts(models)?;
    Ok(mgr)
}

fn bench_model_set(c: &mut Criterion, set_name: &str) {
    let set = common::load_model_set(set_name);
    assert!(
        !set.is_empty(),
        "fixture set '{set_name}' is empty - run generate-fixtures.mjs in the concerto repo first"
    );

    let mut group = c.benchmark_group(format!("public_api/{set_name}"));

    group.bench_with_input(BenchmarkId::new("add_model_ast", set.len()), &set, |b, set| {
        b.iter(|| load_via_public_api(set));
    });

    // Checked once outside the timed section, per this crate's convention
    // (see load_validate.rs): `validate_models` does not yet accept every
    // fixture set (DIVERGENCES.md), so a whole-set failure is skipped
    // rather than silently absorbed into the numbers.
    match load_via_public_api(&set).validate_models() {
        Ok(()) => {
            group.bench_with_input(BenchmarkId::new("validate_models", set.len()), &set, |b, set| {
                b.iter_batched(
                    || load_via_public_api(set),
                    |mgr: ModelManager| mgr.validate_models().expect("set validates"),
                    criterion::BatchSize::LargeInput,
                );
            });

            group.bench_with_input(
                BenchmarkId::new("add_model_asts_batch", set.len()),
                &set,
                |b, set| {
                    b.iter(|| load_and_validate_batch(set).expect("batch loads and validates"));
                },
            );
        }
        Err(e) => {
            eprintln!(
                "public_api/{set_name}/validate_models: SKIPPED - does not yet pass on this \
                 fixture set ({e}); see PORTING.md / DIVERGENCES.md"
            );
        }
    }

    // validateAst (metamodel::validate_ast, the crate-root free function):
    // checked once outside the timed section (see validate_metamodel.rs,
    // which benchmarks it next to `ModelManager::validate_ast`). Since
    // P5-21 (accordproject/concerto-rust#319) it runs on a per-thread
    // resident metamodel manager rather than rebuilding one per call.
    let accepted: Vec<&Value> = set
        .iter()
        .map(|(_, ast)| ast)
        .filter(|ast| metamodel::validate_ast(ast).is_ok())
        .collect();
    if accepted.is_empty() {
        eprintln!(
            "public_api/{set_name}/metamodel::validate_ast: SKIPPED - it accepts none of these \
             models"
        );
    } else {
        group.bench_with_input(
            BenchmarkId::new("metamodel::validate_ast", accepted.len()),
            &accepted,
            |b, accepted| {
                b.iter(|| {
                    for ast in accepted {
                        metamodel::validate_ast(ast).expect("accepted above");
                    }
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------
// Workload 3: instance populate + validate through the public API
// ---------------------------------------------------------------------

/// The same `org.accordproject.bench.instance@1.0.0.Item` model as
/// `instance_validate.rs`, kept byte-identical so the two files' instance
/// numbers are directly comparable.
fn model_ast() -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "decorators": [],
        "namespace": "org.accordproject.bench.instance@1.0.0",
        "imports": [],
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Item",
                "isAbstract": false,
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id", "isArray": false, "isOptional": false },
                    { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "sequence", "isArray": false, "isOptional": false },
                    { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "weight", "isArray": false, "isOptional": true },
                    { "$class": "concerto.metamodel@1.0.0.BooleanProperty", "name": "active", "isArray": false, "isOptional": false },
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "labels", "isArray": true, "isOptional": true }
                ],
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" }
            }
        ]
    })
}

fn build_instance_workload() -> (ModelManager, Vec<Value>) {
    let mut mgr = ModelManager::new().expect("system model loads");
    mgr.add_model_ast(&model_ast(), Some("bench-instance.json"))
        .expect("bench-instance model loads");
    mgr.validate_models().expect("bench-instance model validates");

    let instances: Vec<Value> = (0..NUM_INSTANCES)
        .map(|i| {
            json!({
                "$class": "org.accordproject.bench.instance@1.0.0.Item",
                "id": format!("item-{i}"),
                "sequence": i,
                "weight": (i as f64) * 1.5,
                "active": i % 2 == 0,
                "labels": [format!("label-{}", i % 7), format!("tag-{}", i % 3)]
            })
        })
        .collect();

    (mgr, instances)
}

fn bench_instance(c: &mut Criterion) {
    let (mgr, instances) = build_instance_workload();
    let options = ValidationOptions::default();

    // Check once outside the timed section (this crate's convention).
    for value in &instances {
        mgr.validate_instance(value, &options)
            .unwrap_or_else(|e| panic!("generated instance failed to validate: {e}"));
    }

    let mut group = c.benchmark_group("public_api/instance");

    // Populate + validate, first-error style (TS `Serializer.fromJSON` with
    // `validate: true`; docs/public-api.md 5.7).
    group.bench_with_input(
        BenchmarkId::new("validate_instance", instances.len()),
        &instances,
        |b, instances| {
            b.iter(|| {
                for value in instances {
                    mgr.validate_instance(value, &options).expect("instance validates");
                }
            });
        },
    );

    // Populate + validate, collect-all style (accordproject/concerto#1239).
    group.bench_with_input(
        BenchmarkId::new("check_instance", instances.len()),
        &instances,
        |b, instances| {
            b.iter(|| {
                for value in instances {
                    let report = mgr.check_instance(value, &options);
                    assert!(report.is_valid(), "instance validates");
                }
            });
        },
    );

    group.finish();
}

fn benches(c: &mut Criterion) {
    for set_name in ["concerto-core-test-data", "conformance", "synthetic-large"] {
        bench_model_set(c, set_name);
    }
    bench_instance(c);
}

criterion_group!(public_api, benches);
criterion_main!(public_api);

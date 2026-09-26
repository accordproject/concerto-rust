//! Workload 3 (see the plan, task P5-04, issue accordproject/concerto-rust#75):
//! instance validation. `benches/README.md` (task P5-04a, #92) left this
//! workload TS-only, "added here once [the instance layer] lands, after
//! task P3-01" - P3-01 (#56) has since landed `concerto_core::instance`, so
//! this file adds the Rust half.
//!
//! Mirrors the TS harness's `instance_validate` workload
//! (`migration/bench/run-ts.mjs` in the `concerto` repo,
//! `buildInstanceWorkload`/`benchInstanceValidate`) as closely as the two
//! runtimes allow: the same model (`org.accordproject.bench.instance@1.0.0.Item`,
//! identified by `id`, with a required `String id`, required `Integer
//! sequence`, optional `Double weight`, required `Boolean active` and an
//! optional `String[] labels`) and the same 500 generated instances, built
//! directly from the model's `concerto.metamodel@1.0.0` AST (there is no
//! CTO parser on the Rust side - see the plan §1.1, "CTO parsing is in
//! concerto-cto, which is out of scope" - so the AST below is the Rust
//! equivalent of the TS harness's `.cto` source, kept in lockstep with it
//! by hand; it is also exactly the shape of `BenchClass0` in the shared
//! `synthetic-large` fixture, `migration/bench/generate-fixtures.mjs`,
//! which independently confirms the two are equivalent ASTs).
//!
//! ## Where this and the TS workload diverge
//!
//! TS times two things: `Serializer#fromJSON` (populate *and* validate
//! together) and a standalone `Resource#validate()` on the already-
//! populated resource. Rust has no `JSONPopulator`/`Resource` port yet
//! (task P3-01b, accordproject/concerto-rust#124, per
//! `concerto-core/src/instance/validate.rs`'s module docs) - only the
//! validator itself, over a `Value` already shaped the way a populated
//! `Resource` would serialize. So this file benchmarks one thing,
//! `validate_instance`, which is the Rust counterpart to TS's
//! `resource.validate()` (`validate_only` below), **not** to
//! `fromJSON`'s combined populate-and-validate figure - there is no
//! populate phase to time on the Rust side yet. See this crate's
//! README for how the two are reported.

use concerto_core::ModelManager;
use concerto_core::instance::validate::{ValidateOptions, validate_instance};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use serde_json::{Value, json};

const NUM_INSTANCES: usize = 500;

/// The `org.accordproject.bench.instance@1.0.0` model, as a
/// `concerto.metamodel@1.0.0.Model` AST - the Rust-side equivalent of the
/// TS harness's `.cto` source (see this file's module docs).
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
                    {
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "id",
                        "isArray": false,
                        "isOptional": false
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.IntegerProperty",
                        "name": "sequence",
                        "isArray": false,
                        "isOptional": false
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.DoubleProperty",
                        "name": "weight",
                        "isArray": false,
                        "isOptional": true
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.BooleanProperty",
                        "name": "active",
                        "isArray": false,
                        "isOptional": false
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "labels",
                        "isArray": true,
                        "isOptional": true
                    }
                ],
                "identified": {
                    "$class": "concerto.metamodel@1.0.0.IdentifiedBy",
                    "name": "id"
                }
            }
        ]
    })
}

fn build_workload() -> (ModelManager, Vec<Value>) {
    let mut mgr = ModelManager::new().expect("system model loads");
    mgr.add_model(&model_ast(), Some("bench-instance.json".to_string()))
        .expect("bench-instance model loads");
    mgr.validate_models()
        .expect("bench-instance model validates");

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

fn benches(c: &mut Criterion) {
    let (mgr, instances) = build_workload();
    let options = ValidateOptions::default();

    // Check once outside the timed section, per this crate's convention
    // (see load_validate.rs and validate_metamodel.rs): fail loudly, not
    // silently, if a generated instance stops validating.
    for value in &instances {
        validate_instance(&mgr, value, &options)
            .unwrap_or_else(|e| panic!("generated instance failed to validate: {e}"));
    }

    let mut group = c.benchmark_group("instance_validate");
    group.bench_with_input(
        BenchmarkId::new("validate_only", instances.len()),
        &instances,
        |b, instances| {
            b.iter(|| {
                for value in instances {
                    validate_instance(&mgr, value, &options).expect("instance validates");
                }
            });
        },
    );
    group.finish();
}

criterion_group!(instance_validate, benches);
criterion_main!(instance_validate);

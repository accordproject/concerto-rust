//! Workload 2 (see the plan, task P5-04a): validating the metamodel AST.
//!
//! The TS side of this workload is `ModelManager.validateAst`, which
//! deserialises the AST against the metamodel schema via the serializer
//! (see `run-ts.mjs`'s `validateAst` workload in the `concerto` repo). The
//! two Rust-side counterparts benchmarked here, per the issue:
//!   - `concerto-core`: `ModelFile::from_json`, which performs the
//!     equivalent structural check by deserialising the AST into
//!     `concerto-metamodel`'s strongly-typed schema (concerto-core has no
//!     separate "validate the AST" step; construction *is* that check).
//!   - `concerto-validate-rs`: its own, independent `validate_metamodel`
//!     function, over the same fixtures (only when the `validate-rs`
//!     feature is enabled; see benches/README.md).

use concerto_core::ModelFile;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

#[path = "common/mod.rs"]
mod common;

fn bench_model_set(c: &mut Criterion, set_name: &str) {
    let set = common::load_model_set(set_name);
    assert!(!set.is_empty(), "fixture set '{set_name}' is empty - run generate-fixtures.mjs in the concerto repo first");

    let mut group = c.benchmark_group(format!("validate_metamodel/{set_name}"));

    group.bench_with_input(
        BenchmarkId::new("concerto-core/from_json", set.len()),
        &set,
        |b, set| {
            b.iter(|| {
                for (name, ast) in set {
                    ModelFile::from_json(ast, Some(name.clone()))
                        .unwrap_or_else(|e| panic!("structural validation of {name}: {e}"));
                }
            });
        },
    );

    #[cfg(feature = "validate-rs")]
    {
        // concerto-validator-rs takes the AST as a JSON string, not a
        // parsed Value, so we serialise once up front, outside the timed
        // section.
        //
        // It is a separate, much less complete implementation (see the
        // plan's §1.3 "confirmed bugs" list) that does not yet handle every
        // construct these fixtures use. Per the exit condition ("baseline
        // table ... where Rust has the capability"), we check each file
        // once outside the timed section and only benchmark over the
        // subset it currently accepts, reporting how many that is.
        let all_strings: Vec<(String, String)> = set
            .iter()
            .map(|(name, ast)| (name.clone(), serde_json::to_string(ast).expect("AST re-serialises")))
            .collect();
        let supported: Vec<(String, String)> = all_strings
            .into_iter()
            .filter(|(_, json)| concerto_validator_rs::validate_metamodel(json).is_ok())
            .collect();

        if supported.is_empty() {
            eprintln!(
                "validate_metamodel/{set_name}/concerto-validate-rs: SKIPPED - it accepts none \
                 of this fixture set's {} models yet",
                set.len(),
            );
        } else {
            if supported.len() < set.len() {
                eprintln!(
                    "validate_metamodel/{set_name}/concerto-validate-rs: benchmarking {}/{} \
                     models - the rest hit known gaps in that crate (see PORTING.md's \
                     concerto-validate-rs notes)",
                    supported.len(),
                    set.len(),
                );
            }
            group.bench_with_input(
                BenchmarkId::new("concerto-validate-rs/validate_metamodel", supported.len()),
                &supported,
                |b, set| {
                    b.iter(|| {
                        for (name, json) in set {
                            concerto_validator_rs::validate_metamodel(json)
                                .unwrap_or_else(|e| panic!("concerto-validate-rs on {name}: {e}"));
                        }
                    });
                },
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

criterion_group!(validate_metamodel, benches);
criterion_main!(validate_metamodel);

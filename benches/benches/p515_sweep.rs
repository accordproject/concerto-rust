//! P5-15 (accordproject/concerto-rust#309): the crate-direct half of the
//! performance profiling sweep. Measure only.
//!
//! Each benchmark is `<op>/<set>` and times one batch of `n` items over the
//! same inputs as the TS-API sweep (`migration/bench/p515-sweep.mjs` in the
//! concerto repo), read from `migration/bench/fixtures/p515/<set>.json`
//! (written by `p515-prepare.mjs`). `n` per benchmark is written to
//! `$CARGO_TARGET_DIR/criterion/p515-n.json`, so a report can divide back to
//! per-item figures.
//!
//! ```text
//! cargo bench --manifest-path benches/Cargo.toml --bench p515_sweep
//! # a hot loop of one benchmark, for `sample <pid>`:
//! cargo bench --manifest-path benches/Cargo.toml --bench p515_sweep -- --profile-time 20 'from_json/conformance$'
//! ```
//!
//! What each crate row stands for (the TS-API op it mirrors, and what the
//! WASM binding adds on top):
//! - `mm_new`: `ModelManager::new` (`new ModelManagerHandle`).
//! - `modelfile_new`: `ModelFile::from_json_text` over the AST text
//!   (`stageModelFile`).
//! - `add_model_file`: a fresh manager, then per model `from_json_text`
//!   and `validate_and_add_model_file` (stage, then validate and commit;
//!   `validate_detached_model_file` then `add_model_file` before P5-48).
//!   `addCTOModel` has no crate row of its own: its CTO
//!   parse is TS (concerto-cto) on both engines, and the rest is this.
//! - `from_json` / `to_json`: `concerto_core_js::Serializer` on a resident
//!   manager, from a prebuilt `JsValue` (the binding also parses the JSON
//!   text and decodes/encodes the wire format).
//! - `new_resource`: `concerto_core_js::factory::new_resource`. TS
//!   `Factory.newResource` is TS on the Rust engine too (it crosses only for
//!   introspection reads), so this row is what a ported factory would cost.
//! - `dcs_decorate` / `dcs_validate` / `extract_*`: `concerto_core::dcs` on a
//!   resident manager. The `*_rebuild` rows add what the binding does per
//!   call: rebuild a manager from the model ASTs first and, for decorate,
//!   hand back the whole AST (`model_manager_to_ast`).
//! - introspection: `get_type_declaration`, `resolve_type`,
//!   `Decorated::decorators`/`decorator`, `model_files` namespaces,
//!   `derives_from`, `is_type_assignable_to`.
//! - instance validation (added by P5-22, accordproject/concerto-rust#326):
//!   `validate` / `set_property_value` / `add_array_value`:
//!   `concerto_core_js::resource` over `from_json` instances (the WASM
//!   binding adds the binary encode of the live object and the crossing).

// A benchmark binary, not library code: panicking on a broken fixture is
// the right failure mode.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, missing_docs)]

mod common;

use std::collections::BTreeMap;
use std::hint::black_box;

use concerto_core::dcs::{self, DecorateOptions, ExtractOptions};
use concerto_core::instance::InstanceEnv;
use concerto_core::{Decorated, ModelFile, ModelManager};
use concerto_core_js::value::Instance;
use concerto_core_js::{JsValue, Serializer, factory, resource};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use serde_json::{Value, json};

const SETS: [&str; 3] = ["concerto-core-test-data", "conformance", "synthetic-large"];

struct FixedEnv;

impl InstanceEnv for FixedEnv {
    fn new_id(&mut self) -> String {
        "00000000-0000-4000-8000-000000000000".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

struct SetData {
    models: Vec<(String, Value, String)>,
    instances: Vec<(String, Option<String>, Value)>,
    dcs: Value,
    dcs_models: Vec<String>,
    pairs: Vec<(String, String)>,
}

/// P5-60 (accordproject/concerto-rust#392): the same input fix as the TS
/// sweep's `withoutEnumIsAbstract` (P5-56, `p515-sweep.mjs`). The generated
/// synthetic-large model gives its EnumDeclaration an `isAbstract: false`,
/// a key the metamodel does not declare for enums; the strict typed AST read
/// (P5-49, P5-61) rejects it, so no crate row could load that set. It is
/// dropped here, for every model, before the AST text is taken.
fn without_enum_is_abstract(ast: &mut Value) {
    if let Some(decls) = ast.get_mut("declarations").and_then(Value::as_array_mut) {
        for decl in decls {
            if decl["$class"] == "concerto.metamodel@1.0.0.EnumDeclaration" {
                if let Some(obj) = decl.as_object_mut() {
                    obj.remove("isAbstract");
                }
            }
        }
    }
}

fn load(set: &str) -> SetData {
    let path = common::fixtures_dir()
        .join("p515")
        .join(format!("{set}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e} (run p515-prepare.mjs)", path.display()));
    let v: Value = serde_json::from_str(&text).expect("p515 fixture JSON");
    let models = v["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let mut ast = m["ast"].clone();
            without_enum_is_abstract(&mut ast);
            let text = serde_json::to_string(&ast).unwrap();
            (m["name"].as_str().unwrap().to_string(), ast, text)
        })
        .collect();
    let instances = v["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["fqn"].as_str().unwrap().to_string(),
                i["id"].as_str().map(str::to_string),
                i["json"].clone(),
            )
        })
        .collect();
    let dcs_models = v["dcsModels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect();
    let pairs = v["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p[0].as_str().unwrap().to_string(),
                p[1].as_str().unwrap().to_string(),
            )
        })
        .collect();
    SetData {
        models,
        instances,
        dcs: v["dcs"].clone(),
        dcs_models,
        pairs,
    }
}

fn file_of(name: &str, text: &str) -> ModelFile {
    ModelFile::from_json_text(text, None, Some(name.to_string()))
        .expect("AST text parses")
        .expect("AST loads")
}

/// What `add_model_file` times, and every resident manager below.
fn manager_of<'a>(models: impl Iterator<Item = &'a (String, Value, String)>) -> ModelManager {
    let mut mm = ModelManager::new().expect("a fresh manager");
    for (name, _, text) in models {
        let mf = file_of(name, text);
        // P5-48: the binding's validate-and-commit, in one step.
        mm.validate_and_add_model_file(mf)
            .map_err(|(err, _)| err)
            .expect("model validates and adds");
    }
    mm
}

fn dcs_manager(d: &SetData) -> ModelManager {
    manager_of(d.models.iter().filter(|m| d.dcs_models.contains(&m.0)))
}

fn decorate_options() -> DecorateOptions {
    DecorateOptions {
        validate: true,
        validate_commands: true,
        ..DecorateOptions::default()
    }
}

fn extract_options() -> ExtractOptions {
    ExtractOptions {
        remove_decorators_from_model: true,
        locale: "en".to_string(),
    }
}

/// The model files the fixture itself loaded (what the WASM binding's
/// `model_manager_from_asts_with_user_ns` keeps): not the system,
/// decorator or root models `ModelManager::new` preloads.
fn user_files<'m>(mm: &'m ModelManager, d: &SetData) -> Vec<&'m ModelFile> {
    let user: std::collections::HashSet<&str> = d
        .models
        .iter()
        .filter_map(|m| m.1["namespace"].as_str())
        .collect();
    mm.model_files()
        .filter(|mf| user.contains(mf.namespace()))
        .collect()
}

fn bench(c: &mut Criterion) {
    // P5-80 (#424, analysis only): the validation-plan prototype's switch,
    // so p580-run.sh times the plan off and on with one binary.
    if let Ok(v) = std::env::var("CONCERTO_VALIDATION_PLAN") {
        concerto_core::instance::plan::set_enabled(v != "0");
    }
    let mut sizes: BTreeMap<String, usize> = BTreeMap::new();
    let mut g = c.benchmark_group("p515");
    g.sample_size(20);

    g.bench_function("mm_new/conformance", |b| {
        b.iter(|| black_box(ModelManager::new().unwrap()))
    });
    sizes.insert("mm_new/conformance".into(), 1);

    for set in SETS {
        let d = load(set);
        let mut n = |op: &str, v: usize| {
            sizes.insert(format!("{op}/{set}"), v);
        };

        // ---- Model load --------------------------------------------------
        n("modelfile_new", d.models.len());
        g.bench_function(format!("modelfile_new/{set}"), |b| {
            b.iter(|| {
                for (name, _, text) in &d.models {
                    black_box(file_of(name, text));
                }
            })
        });
        n("add_model_file", d.models.len());
        g.bench_function(format!("add_model_file/{set}"), |b| {
            b.iter(|| black_box(manager_of(d.models.iter())))
        });

        // ---- Serializer and Factory --------------------------------------
        let mm = manager_of(d.models.iter());
        let serializer = Serializer::new(true, true, None).unwrap();
        let objects: Vec<JsValue> = d
            .instances
            .iter()
            .map(|(_, _, j)| JsValue::from_json(j))
            .collect();
        n("from_json", objects.len());
        g.bench_function(format!("from_json/{set}"), |b| {
            b.iter(|| {
                for o in &objects {
                    black_box(serializer.from_json(&mm, o, None, &mut FixedEnv).unwrap());
                }
            })
        });
        let resources: Vec<JsValue> = objects
            .iter()
            .map(|o| {
                JsValue::Instance(Box::new(
                    serializer.from_json(&mm, o, None, &mut FixedEnv).unwrap(),
                ))
            })
            .collect();
        n("to_json", resources.len());
        g.bench_function(format!("to_json/{set}"), |b| {
            b.iter(|| {
                for r in &resources {
                    black_box(serializer.to_json(&mm, r, None).unwrap());
                }
            })
        });
        let targets: Vec<(String, String, JsValue)> = d
            .instances
            .iter()
            .map(|(fqn, id, _)| {
                let decl = mm
                    .declaration(mm.get_type_declaration(fqn).unwrap())
                    .unwrap();
                let ns = fqn.rsplit_once('.').unwrap().0.to_string();
                let id_value = id.clone().map_or(JsValue::Undefined, JsValue::String);
                (ns, decl.name().to_string(), id_value)
            })
            .collect();
        n("new_resource", targets.len());
        g.bench_function(format!("new_resource/{set}"), |b| {
            b.iter(|| {
                for (ns, name, id) in &targets {
                    black_box(
                        factory::new_resource(&mm, ns, name, id.clone(), false, &mut FixedEnv)
                            .unwrap(),
                    );
                }
            })
        });

        // ---- Instance validation (P5-22, #326) ----------------------------
        // `concerto_core_js::resource`'s `validate`, `set_property_value`
        // and `add_array_value` over the instances `from_json` builds, with
        // the calls chosen from each instance's JSON keys as p515-sweep.mjs
        // chooses them.
        let instances: Vec<Instance> = objects
            .iter()
            .map(|o| serializer.from_json(&mm, o, None, &mut FixedEnv).unwrap())
            .collect();
        n("validate", instances.len());
        g.bench_function(format!("validate/{set}"), |b| {
            let mut insts = instances.clone();
            b.iter(|| {
                for inst in &mut insts {
                    resource::validate(&mm, inst).unwrap();
                }
            })
        });
        let keys = |arrays: bool| -> Vec<(usize, String)> {
            d.instances
                .iter()
                .enumerate()
                .flat_map(|(i, (_, _, j))| {
                    j.as_object()
                        .unwrap()
                        .iter()
                        .filter(|(k, v)| {
                            !k.starts_with('$')
                                && if arrays {
                                    v.as_array().is_some_and(|a| !a.is_empty())
                                } else {
                                    !v.is_array()
                                }
                        })
                        .map(move |(k, _)| (i, k.clone()))
                })
                .collect()
        };
        let set_items: Vec<(usize, String, JsValue)> = keys(false)
            .into_iter()
            .map(|(i, k)| {
                let v = instances[i].get(&k).clone();
                (i, k, v)
            })
            .collect();
        n("set_property_value", set_items.len());
        g.bench_function(format!("set_property_value/{set}"), |b| {
            let mut insts = instances.clone();
            b.iter(|| {
                for (i, k, v) in &set_items {
                    resource::set_property_value(&mm, &mut insts[*i], k, v.clone()).unwrap();
                }
            })
        });
        let add_items: Vec<(usize, String, JsValue)> = keys(true)
            .into_iter()
            .map(|(i, k)| {
                let JsValue::Array(a) = instances[i].get(&k) else {
                    panic!("{k} is not an array");
                };
                let v = a[0].clone();
                (i, k, v)
            })
            .collect();
        n("add_array_value", add_items.len());
        if !add_items.is_empty() {
            g.bench_function(format!("add_array_value/{set}"), |b| {
                b.iter_batched(
                    || instances.clone(),
                    |mut insts| {
                        for (i, k, v) in &add_items {
                            resource::add_array_value(&mm, &mut insts[*i], k, v.clone()).unwrap();
                        }
                        insts
                    },
                    BatchSize::LargeInput,
                )
            });
        }

        // ---- DecoratorManager --------------------------------------------
        let dmm = dcs_manager(&d);
        n("dcs_decorate", 1);
        g.bench_function(format!("dcs_decorate/{set}"), |b| {
            b.iter_batched(
                || vec![d.dcs.clone()],
                |mut sets| {
                    black_box(
                        dcs::decorate_models(&dmm, &mut sets, &mut decorate_options()).unwrap(),
                    )
                },
                BatchSize::SmallInput,
            )
        });
        let dcs_asts: Vec<Value> = d
            .models
            .iter()
            .filter(|m| d.dcs_models.contains(&m.0))
            .map(|m| m.1.clone())
            .collect();
        let rebuild = |asts: &[Value]| {
            let mut mm = ModelManager::new().unwrap();
            for a in asts {
                mm.add_model_with_definitions(a, None, None).unwrap();
            }
            mm
        };
        n("dcs_decorate_rebuild", 1);
        g.bench_function(format!("dcs_decorate_rebuild/{set}"), |b| {
            b.iter_batched(
                || vec![d.dcs.clone()],
                |mut sets| {
                    let mm = rebuild(&dcs_asts);
                    let out =
                        dcs::decorate_models(&mm, &mut sets, &mut decorate_options()).unwrap();
                    let models: Vec<Value> = out.model_files().map(|mf| mf.ast().clone()).collect();
                    black_box(
                        json!({ "$class": "concerto.metamodel@1.0.0.Models", "models": models }),
                    )
                },
                BatchSize::SmallInput,
            )
        });
        n("dcs_validate", 1);
        let files = user_files(&dmm, &d);
        g.bench_function(format!("dcs_validate/{set}"), |b| {
            b.iter(|| black_box(dcs::validate(&d.dcs, Some(&files)).unwrap()))
        });
        n("dcs_validate_rebuild", 1);
        g.bench_function(format!("dcs_validate_rebuild/{set}"), |b| {
            b.iter(|| {
                let mm = rebuild(&dcs_asts);
                let files = user_files(&mm, &d);
                black_box(dcs::validate(&d.dcs, Some(&files)).unwrap())
            })
        });
        let decorated =
            dcs::decorate_models(&dmm, &mut [d.dcs.clone()], &mut decorate_options()).unwrap();
        let decorated_asts: Vec<Value> = user_files(&decorated, &d)
            .iter()
            .map(|mf| mf.ast().clone())
            .collect();
        for (op, vocab) in [
            ("extract_decorators", false),
            ("extract_vocabularies", true),
        ] {
            n(op, 1);
            g.bench_function(format!("{op}/{set}"), |b| {
                b.iter(|| {
                    black_box(if vocab {
                        dcs::extract_vocabularies(&decorated, &extract_options()).unwrap()
                    } else {
                        dcs::extract_decorators(&decorated, &extract_options()).unwrap()
                    })
                })
            });
            n(&format!("{op}_rebuild"), 1);
            g.bench_function(format!("{op}_rebuild/{set}"), |b| {
                b.iter(|| {
                    let mm = rebuild(&decorated_asts);
                    black_box(if vocab {
                        dcs::extract_vocabularies(&mm, &extract_options()).unwrap()
                    } else {
                        dcs::extract_decorators(&mm, &extract_options()).unwrap()
                    })
                })
            });
        }

        // ---- Introspection -----------------------------------------------
        n("get_type", d.pairs.len());
        g.bench_function(format!("get_type/{set}"), |b| {
            b.iter(|| {
                for (fqn, _) in &d.pairs {
                    black_box(mm.get_type_declaration(fqn).unwrap());
                }
            })
        });
        n("resolve_type", d.pairs.len());
        g.bench_function(format!("resolve_type/{set}"), |b| {
            b.iter(|| {
                for (fqn, _) in &d.pairs {
                    black_box(mm.resolve_type("p515", fqn).unwrap());
                }
            })
        });
        let decls: Vec<_> = user_files(&decorated, &d)
            .into_iter()
            .flat_map(|mf| mf.declarations().iter())
            .collect();
        n("get_decorators", decls.len());
        g.bench_function(format!("get_decorators/{set}"), |b| {
            b.iter(|| {
                for decl in &decls {
                    black_box(decl.decorators());
                    black_box(decl.decorator("Term"));
                }
            })
        });
        n("get_namespaces", 1);
        g.bench_function(format!("get_namespaces/{set}"), |b| {
            b.iter(|| {
                black_box(
                    mm.model_files()
                        .map(|mf| mf.namespace().to_string())
                        .collect::<Vec<_>>(),
                )
            })
        });
        n("derives_from", d.pairs.len());
        g.bench_function(format!("derives_from/{set}"), |b| {
            b.iter(|| {
                for (a, s) in &d.pairs {
                    black_box(mm.derives_from(a, s).unwrap());
                }
            })
        });
        n("is_assignable_to", d.pairs.len());
        g.bench_function(format!("is_assignable_to/{set}"), |b| {
            b.iter(|| {
                for (a, s) in &d.pairs {
                    black_box(mm.is_type_assignable_to(a, s));
                }
            })
        });
    }
    g.finish();

    let target = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let _ = std::fs::create_dir_all(target.join("criterion"));
    let _ = std::fs::write(
        target.join("criterion").join("p515-n.json"),
        serde_json::to_string_pretty(&sizes).unwrap(),
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);

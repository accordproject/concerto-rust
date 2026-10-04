//! Native timing and profiling loops for the two validator paths
//! (accordproject/concerto-rust#297, P5-13): the metamodel check
//! ([`ModelManager::validate_ast`], `run-ts.mjs` workload 2) and instance
//! validation (workload 3): the JS layer's `Serializer::from_json` and
//! `resource::validate`, which the WASM binding runs, and the native
//! [`ModelManager::validate_instance`].
//!
//! ```text
//! cargo run --release -p accordproject-concerto-core --example validator_profile -- time <model-sets dir>
//! cargo run --release -p accordproject-concerto-core --example validator_profile -- loop <model-sets dir> <set|instance|native-instance> <seconds>
//! ```
//!
//! `<model-sets dir>` is the concerto repo's
//! `migration/bench/fixtures/model-sets`. `time` prints the median µs per
//! model (or per instance); `loop` runs one workload in a hot loop for a
//! sampling profiler (macOS `sample <pid>`).

use std::time::{Duration, Instant};

use concerto_core::instance::{InstanceEnv, ValidationOptions};
use concerto_core::json;
use concerto_core::json::Value;
use concerto_core::{ModelFile, ModelManager};
use concerto_core_js::resource;
use concerto_core_js::{Instance, JsValue, Serializer};

const INSTANCES: usize = 500;

struct FixedEnv;

impl InstanceEnv for FixedEnv {
    fn new_id(&mut self) -> String {
        "00000000-0000-4000-8000-000000000000".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// The models of a set that `validate_ast` accepts, as `run-ts.mjs`'s
/// `benchValidateAst` keeps them.
fn model_set(dir: &str, set: &str) -> (ModelManager, Vec<ModelFile>) {
    let mut files: Vec<_> = std::fs::read_dir(format!("{dir}/{set}"))
        .expect("model-set directory")
        .map(|e| e.expect("directory entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let mut mm = ModelManager::new().expect("a fresh manager");
    let mut models = Vec::new();
    for p in files {
        let text = std::fs::read_to_string(&p).expect("fixture text");
        let v: Value = serde_json::from_str(&text).expect("fixture JSON");
        let name = p
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .to_string();
        if let Ok(mf) = ModelFile::from_json(&v, Some(name))
            && mm.validate_ast(&mf).is_ok()
        {
            models.push(mf);
        }
    }
    (mm, models)
}

/// `run-ts.mjs`'s `buildInstanceWorkload`: the `Item` model and its 500
/// instances.
fn instance_workload() -> (ModelManager, Vec<Value>) {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.accordproject.bench.instance@1.0.0",
        "imports": [],
        "declarations": [{
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Item",
            "isAbstract": false,
            "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id", "isArray": false, "isOptional": false },
                { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "sequence", "isArray": false, "isOptional": false },
                { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "weight", "isArray": false, "isOptional": true },
                { "$class": "concerto.metamodel@1.0.0.BooleanProperty", "name": "active", "isArray": false, "isOptional": false },
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "labels", "isArray": true, "isOptional": true }
            ]
        }]
    });
    let mut mm = ModelManager::new().expect("a fresh manager");
    mm.add_model_ast(&ast, Some("bench-instance.cto"))
        .expect("the bench model loads");
    mm.validate_models().expect("the bench model validates");
    let instances = (0..INSTANCES)
        .map(|i| {
            json!({
                "$class": "org.accordproject.bench.instance@1.0.0.Item",
                "id": format!("item-{i}"),
                "sequence": i,
                "weight": (i as f64) * 1.5,
                "active": i % 2 == 0,
                "labels": [format!("label-{}", i % 7), format!("tag-{}", i % 3)],
            })
        })
        .collect();
    (mm, instances)
}

fn from_json_all(
    mm: &ModelManager,
    serializer: &Serializer,
    instances: &[JsValue],
) -> Vec<Instance> {
    instances
        .iter()
        .map(|json| {
            serializer
                .from_json(mm, json, None, &mut FixedEnv)
                .expect("a bench instance populates")
        })
        .collect()
}

fn time(dir: &str) {
    for set in ["concerto-core-test-data", "conformance"] {
        let (mut mm, models) = model_set(dir, set);
        let n = models.len() as f64;
        let mut samples = Vec::new();
        for s in 0..60 {
            let t0 = Instant::now();
            for mf in &models {
                mm.validate_ast(mf).expect("accepted above");
            }
            if s >= 10 {
                samples.push(t0.elapsed().as_secs_f64() * 1e6 / n);
            }
        }
        println!(
            "validate_ast {set} n={}: {:.1} us/model",
            models.len(),
            median(samples)
        );
    }
    let (mm, plain) = instance_workload();
    let instances: Vec<JsValue> = plain.iter().map(JsValue::from_json).collect();
    let serializer = Serializer::new(true, true, None).expect("a serializer");
    let mut resources = from_json_all(&mm, &serializer, &instances);
    let options = ValidationOptions::default();
    let n = instances.len() as f64;
    let (mut populate, mut validate, mut native) = (Vec::new(), Vec::new(), Vec::new());
    for s in 0..60 {
        let t0 = Instant::now();
        std::hint::black_box(from_json_all(&mm, &serializer, &instances));
        let a = t0.elapsed().as_secs_f64() * 1e6 / n;
        let t0 = Instant::now();
        for r in &mut resources {
            resource::validate(&mm, r).expect("a bench instance validates");
        }
        let b = t0.elapsed().as_secs_f64() * 1e6 / n;
        let t0 = Instant::now();
        for json in &plain {
            mm.validate_instance(json, &options)
                .expect("a bench instance validates");
        }
        let c = t0.elapsed().as_secs_f64() * 1e6 / n;
        if s >= 10 {
            populate.push(a);
            validate.push(b);
            native.push(c);
        }
    }
    println!(
        "instance n={INSTANCES}: js from_json {:.2} us/op, js resource::validate {:.2} us/op, native validate_instance {:.2} us/op",
        median(populate),
        median(validate),
        median(native)
    );
}

fn hot_loop(dir: &str, workload: &str, secs: u64) {
    let end = Instant::now() + Duration::from_secs(secs);
    let mut calls = 0u64;
    eprintln!("{workload}: pid {}", std::process::id());
    if workload == "instance" {
        let (mm, plain) = instance_workload();
        let instances: Vec<JsValue> = plain.iter().map(JsValue::from_json).collect();
        let serializer = Serializer::new(true, true, None).expect("a serializer");
        let mut resources = from_json_all(&mm, &serializer, &instances);
        while Instant::now() < end {
            std::hint::black_box(from_json_all(&mm, &serializer, &instances));
            for r in &mut resources {
                resource::validate(&mm, r).expect("a bench instance validates");
            }
            calls += 1;
        }
    } else if workload == "native-instance" {
        let (mm, plain) = instance_workload();
        let options = ValidationOptions::default();
        while Instant::now() < end {
            for json in &plain {
                mm.validate_instance(json, &options)
                    .expect("a bench instance validates");
            }
            calls += 1;
        }
    } else {
        let (mut mm, models) = model_set(dir, workload);
        while Instant::now() < end {
            for mf in &models {
                mm.validate_ast(mf).expect("accepted above");
                calls += 1;
            }
        }
    }
    eprintln!("{calls} calls");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["time", dir] => time(dir),
        ["loop", dir, workload, secs] => hot_loop(dir, workload, secs.parse().expect("seconds")),
        _ => eprintln!(
            "usage: validator_profile time <dir> | loop <dir> <set|instance|native-instance> <seconds>"
        ),
    }
}

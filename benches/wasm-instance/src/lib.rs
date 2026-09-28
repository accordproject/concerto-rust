//! The in-WASM instance validator (task P5-13, accordproject/concerto-rust#297).
//!
//! Workload 3 of `benches/benches/instance_validate.rs`, compiled to wasm32
//! and driven from Node by `run.mjs`: the same model, the same 500
//! instances and the same three routes, timed in V8. Each `run*` call walks
//! all 500 instances once inside WASM, so the time per instance is the
//! validator's own cost, with one TS-to-WASM call per 500 instances instead
//! of per instance.
//!
//! - `runValidateOnly`: `instance::validate::validate_instance`, the
//!   `ResourceValidator` walk over a prepared value (the "validator floor"
//!   P5-12b measured at 3.9 µs, accordproject/concerto-rust#292).
//! - `runFromJson`: concerto-core-js's `Serializer::from_json` (populate,
//!   then validate), what the engine's `serializerFromJson` runs.
//! - `runValidateInstanceNative`: `ModelManager::validate_instance`, the
//!   native plain-JSON route.

use std::cell::RefCell;

use concerto_core::ModelManager;
use concerto_core::instance::validate::{ValidateOptions, validate_instance};
use concerto_core::instance::{InstanceEnv, ValidationOptions};
use concerto_core_js::{JsValue as CoreJsValue, Serializer};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

const NUM_INSTANCES: usize = 500;

struct Workload {
    mgr: ModelManager,
    instances: Vec<Value>,
    js_instances: Vec<CoreJsValue>,
    serializer: Serializer,
}

thread_local! {
    static WORKLOAD: RefCell<Option<Workload>> = const { RefCell::new(None) };
}

/// The `org.accordproject.bench.instance@1.0.0` model: a copy of
/// `instance_validate.rs`'s `model_ast`.
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

/// A fixed identifier and clock, as in `instance_validate.rs`.
struct FixedEnv;

impl InstanceEnv for FixedEnv {
    fn new_id(&mut self) -> String {
        "00000000-0000-4000-8000-000000000000".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

fn err(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

/// Builds the model and the 500 instances, and checks every route once
/// outside the timed section. Returns the number of instances.
#[wasm_bindgen]
pub fn setup() -> Result<u32, JsError> {
    let mut mgr = ModelManager::new().map_err(err)?;
    // `add_model`, as `instance_validate.rs` calls it, so the same file
    // builds on trees from before `add_model_ast`.
    #[allow(deprecated)]
    mgr.add_model(&model_ast(), Some("bench-instance.json".to_string()))
        .map_err(err)?;
    mgr.validate_models().map_err(err)?;
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
    let js_instances: Vec<CoreJsValue> = instances.iter().map(CoreJsValue::from_json).collect();
    let serializer = Serializer::new(true, true, None).map_err(err)?;
    let w = Workload {
        mgr,
        instances,
        js_instances,
        serializer,
    };
    let options = ValidateOptions::default();
    let native = ValidationOptions::default();
    for (value, js) in w.instances.iter().zip(&w.js_instances) {
        validate_instance(&w.mgr, value, &options).map_err(err)?;
        w.serializer
            .from_json(&w.mgr, js, None, &mut FixedEnv)
            .map_err(err)?;
        w.mgr.validate_instance(value, &native).map_err(err)?;
    }
    WORKLOAD.with(|cell| *cell.borrow_mut() = Some(w));
    Ok(NUM_INSTANCES as u32)
}

fn with_workload(body: impl FnOnce(&Workload) -> Result<(), JsError>) -> Result<(), JsError> {
    WORKLOAD.with(|cell| match cell.borrow().as_ref() {
        Some(w) => body(w),
        None => Err(JsError::new("setup() has not run")),
    })
}

/// One pass of `validate_instance` over the 500 instances.
#[wasm_bindgen(js_name = runValidateOnly)]
pub fn run_validate_only() -> Result<(), JsError> {
    with_workload(|w| {
        let options = ValidateOptions::default();
        for value in &w.instances {
            validate_instance(&w.mgr, value, &options).map_err(err)?;
        }
        Ok(())
    })
}

/// One pass of `Serializer::from_json` over the 500 instances.
#[wasm_bindgen(js_name = runFromJson)]
pub fn run_from_json() -> Result<(), JsError> {
    with_workload(|w| {
        for js in &w.js_instances {
            w.serializer
                .from_json(&w.mgr, js, None, &mut FixedEnv)
                .map_err(err)?;
        }
        Ok(())
    })
}

/// One pass of `ModelManager::validate_instance` over the 500 instances.
#[wasm_bindgen(js_name = runValidateInstanceNative)]
pub fn run_validate_instance_native() -> Result<(), JsError> {
    with_workload(|w| {
        let native = ValidationOptions::default();
        for value in &w.instances {
            w.mgr.validate_instance(value, &native).map_err(err)?;
        }
        Ok(())
    })
}

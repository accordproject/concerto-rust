//! P5-114 (accordproject/concerto-rust#484, R2C-2): `Object.keys` lists an
//! object's array-index keys first. Ordering 100,000 numeric keys stays
//! about as cheap as ordering 100,000 other keys, through the native
//! `ModelManager::validate_instance` and the JS object model's
//! `fromJSON`/`toJSON` round trip. Ordering them by filtering every key
//! against the list of numeric keys was O(n * k), about 5 * 10^9 string
//! comparisons here.

use std::time::{Duration, Instant};

use concerto_core::ModelManager;
use concerto_core::instance::{InstanceEnv, ValidationOptions};
use concerto_core::json;
use concerto_core::json::{Map, Value};
use concerto_core_js::{JsValue, Serializer};

/// How many map entries each instance has.
const KEYS: usize = 100_000;

/// The numeric-key instance may take this many times as long as the other
/// one, plus [`SLACK`]. Sorting the numeric keys costs a little more; a
/// quadratic ordering costs thousands of times as much.
const RATIO: u32 = 4;
/// Absolute slack on top of [`RATIO`], so a slow or busy machine does not
/// fail the test on timer noise.
const SLACK: Duration = Duration::from_millis(250);

struct Env;

impl InstanceEnv for Env {
    fn new_id(&mut self) -> String {
        "id".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

/// A concept with one `Map<String, String>` field.
fn model() -> ModelManager {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.keys@1.0.0",
        "imports": [],
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Bag",
                "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.ObjectProperty",
                    "name": "entries",
                    "isArray": false,
                    "isOptional": false,
                    "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Entries" }
                }]
            },
            {
                "$class": "concerto.metamodel@1.0.0.MapDeclaration",
                "name": "Entries",
                "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" }
            }
        ]
    });
    let mut mm = ModelManager::new().expect("a model manager");
    mm.add_model_with_definitions(&ast, None, Some("keys.cto".into()))
        .expect("the model loads");
    mm
}

/// A `Bag` whose map holds `keys`, in that insertion order.
fn bag(keys: &[String]) -> Value {
    let entries: Map<String, Value> = keys
        .iter()
        .map(|key| (key.clone(), Value::String("v".into())))
        .collect();
    json!({ "$class": "org.keys@1.0.0.Bag", "entries": entries })
}

/// The faster of two runs of `f`.
fn fastest(mut f: impl FnMut()) -> Duration {
    (0..2)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .min()
        .expect("two runs")
}

/// The native check: `ModelManager::validate_instance` (concerto-core's
/// `fromJSON` port over plain JSON).
fn native(mm: &ModelManager, json: &Value) -> Duration {
    fastest(|| {
        mm.validate_instance(json, &ValidationOptions::default())
            .expect("the instance validates");
    })
}

/// The JS object model: JSON -> JS object -> `fromJSON` -> `toJSON`, both
/// with validation.
fn round_trip(mm: &ModelManager, json: &Value) -> Duration {
    let serializer = Serializer::new(true, true, None).expect("a serializer");
    fastest(|| {
        let object = JsValue::from_json(json);
        let bag = serializer
            .from_json(mm, &object, None, &mut Env)
            .expect("the instance validates");
        let JsValue::Map(read) = bag.get("entries") else {
            panic!("entries is a Map");
        };
        assert_eq!(read.len(), KEYS);
        serializer
            .to_json(mm, &JsValue::Instance(Box::new(bag)), None)
            .expect("the instance serializes");
    })
}

#[test]
fn numeric_keys_stay_roughly_linear() {
    let mm = model();
    // Inserted in descending order, so every key moves when sorted.
    let numeric: Vec<String> = (0..KEYS).rev().map(|i| i.to_string()).collect();
    let named: Vec<String> = (0..KEYS).map(|i| format!("k{i}")).collect();
    let (numeric, named) = (bag(&numeric), bag(&named));
    // Warm up allocations and the model's caches.
    native(&mm, &bag(&["1".to_string(), "a".to_string()]));

    let named_time = native(&mm, &named);
    let numeric_time = native(&mm, &numeric);
    assert!(
        numeric_time <= named_time * RATIO + SLACK,
        "validate_instance: {KEYS} numeric keys took {numeric_time:?}, {KEYS} other keys {named_time:?}"
    );

    let named_time = round_trip(&mm, &named);
    let numeric_time = round_trip(&mm, &numeric);
    assert!(
        numeric_time <= named_time * RATIO + SLACK,
        "fromJSON/toJSON: {KEYS} numeric keys took {numeric_time:?}, {KEYS} other keys {named_time:?}"
    );
}

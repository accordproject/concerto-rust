//! The source models are themselves metamodel ASTs, so each one must survive
//! deserialising into [`mm::Model`] and serialising back.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::Value;

/// Compares numbers by value, so `1` and `1.0` are the same.
fn normalise(value: Value) -> Value {
    match value {
        Value::Number(n) => serde_json::json!(n.as_f64()),
        Value::Array(items) => Value::Array(items.into_iter().map(normalise).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalise(v))).collect())
        }
        other => other,
    }
}

fn round_trips(file: &str) {
    let path = format!("{}/vendor/{file}", env!("CARGO_MANIFEST_DIR"));
    let original: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let model: mm::Model = serde_json::from_value(original.clone()).unwrap();
    let back = serde_json::to_value(&model).unwrap();
    assert_eq!(
        normalise(back),
        normalise(original),
        "{file} changed on the round trip"
    );
}

#[test]
fn the_root_model_round_trips() {
    round_trips("concerto@1.0.0.json");
}

#[test]
fn the_decorator_model_round_trips() {
    round_trips("concerto.decorator@1.0.0.json");
}

#[test]
fn the_metamodel_round_trips() {
    round_trips("concerto.metamodel@1.0.0.json");
}

#[test]
fn the_decorator_command_model_round_trips() {
    round_trips("org.accordproject.decoratorcommands@0.4.0.json");
}

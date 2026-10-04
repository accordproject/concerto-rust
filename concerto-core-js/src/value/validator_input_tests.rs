use super::*;
use concerto_core::json;

/// Everything the validator reads of `value`, through `V`'s
/// [`ValidatorInput`], in the plain-JSON shape, for comparison.
fn reading<V: ValidatorInput>(value: &V) -> Value {
    let object = value.as_object().map(|o| {
        let keys: Vec<&str> = o.keys().collect();
        // The walk reads the properties of an object with a `$class`
        // only (`ValidatorObject::get`).
        let gets: Vec<Value> = keys
            .iter()
            .filter(|k| **k != "$class" && o.class().is_some())
            .map(|k| o.get(k).map_or(Value::Null, |v| v.to_value().into_owned()))
            .collect();
        json!({
            "class": o.class(),
            "relationship": o.is_relationship(),
            "keys": keys,
            "gets": gets,
        })
    });
    let entries = value.map_entries().map(|entries| {
        entries
            .map(|(k, v)| json!([k.to_value(), v.to_value()]))
            .collect::<Vec<_>>()
    });
    json!({
        "undefined": value.is_undefined(),
        "null": value.is_null(),
        "object": object,
        "array": value.as_array().map(|items| items.iter().map(|i| i.to_value().into_owned()).collect::<Vec<_>>()),
        "str": ValidatorInput::as_str(value),
        "f64": ValidatorInput::as_f64(value).map(|n| (n.to_bits(), n.is_sign_negative())),
        "boolean": ValidatorInput::is_boolean(value),
        "dayjs": value.is_dayjs(),
        "entries": entries,
    })
}

/// A JS value reads exactly as its plain-JSON shape
/// ([`JsValue::to_validator_value`]) does, the tagged one-key objects
/// a plain object may spell included.
#[test]
fn a_js_value_reads_as_its_plain_json_shape() {
    let instance = |kind: InstanceKind| {
        let mut i = Instance::new(
            kind,
            "org.acme@1.0.0.Person",
            "org.acme@1.0.0",
            "Person",
            Some("email".into()),
            JsValue::String("bob".into()),
            JsValue::Undefined,
        );
        i.set("name", JsValue::String("Bob".into()));
        JsValue::Instance(Box::new(i))
    };
    let mut overridden = instance(InstanceKind::Resource);
    if let JsValue::Instance(i) = &mut overridden {
        i.set("$class", JsValue::Number(1.0));
        i.set(DAYJS_TAG, JsValue::Bool(true));
    }
    let mut numeric_id = instance(InstanceKind::Relationship);
    if let JsValue::Instance(i) = &mut numeric_id {
        i.set_identifier(JsValue::Number(1.0));
    }
    let mut values = vec![
        JsValue::Undefined,
        JsValue::Null,
        JsValue::Bool(false),
        JsValue::Number(1.5),
        JsValue::Number(-0.0),
        JsValue::Number(1e300),
        JsValue::Number(f64::NAN),
        JsValue::Number(f64::NEG_INFINITY),
        JsValue::String("x".into()),
        JsValue::BigInt("12".into()),
        JsValue::DateTime(Dayjs::utc_parse("2021-01-01T00:00:00Z")),
        JsValue::Array(vec![JsValue::Number(1.0), JsValue::Undefined]),
        JsValue::Map(vec![
            (JsValue::String("k".into()), JsValue::Number(1.0)),
            (JsValue::Number(2.0), JsValue::Undefined),
        ]),
        instance(InstanceKind::Resource),
        instance(InstanceKind::ValidatedResource),
        instance(InstanceKind::Relationship),
        overridden,
        numeric_id,
    ];
    for plain in [
        json!({ "$class": "org.acme@1.0.0.Person", "email": "e", "n": 1 }),
        json!({ "$$undefined": true }),
        json!({ "$$undefined": true, "a": 1 }),
        json!({ "$$number": "NaN" }),
        json!({ "$$number": 1 }),
        json!({ "$$bigint": "1" }),
        json!({ "$$map": [["k", 1], ["only"], 3] }),
        json!({ "$$map": "no" }),
        json!({ "$$dayjs": "x", "$class": "a@1.0.0.B" }),
        json!({ "$$relationship": true, "$class": "a@1.0.0.B", "id": "x" }),
        json!({}),
    ] {
        values.push(JsValue::from_json(&plain));
    }
    for value in &values {
        assert_eq!(
            reading(value),
            reading(&value.to_validator_value()),
            "{value:?}"
        );
    }
}

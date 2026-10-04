//! [`Value`] reads, writes and decodes as `serde_json::Value` does.

use std::collections::BTreeMap;
use std::hash::BuildHasher;

use serde::Deserialize;

use super::{Map, Value, from_value, to_value};
use crate::hash::SeededState;
use crate::json;

/// Documents covering every kind of value, key order, duplicate keys,
/// escapes and the numbers `float_roundtrip` exists for.
const DOCUMENTS: &[&str] = &[
    "null",
    "true",
    "[]",
    "{}",
    r#"{"b":1,"a":2,"c":{"z":[1,2.5,-3,"x",null,true,{}],"y":{}}}"#,
    r#"{"k":1,"j":2,"k":3}"#,
    r#"[989.9951327998887, 0.1, 1e300, -0.0, 18446744073709551615, -9223372036854775808, 1.7976931348623157e308, 5e-324]"#,
    r#"{"é😀":"\n\t\"\\","":""}"#,
    r#"  { "spaced" : [ 1 , 2 ] }  "#,
];

#[test]
fn reads_and_writes_as_serde_json() {
    for text in DOCUMENTS {
        let ours: Value = serde_json::from_str(text).unwrap();
        let theirs: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            serde_json::to_string(&ours).unwrap(),
            serde_json::to_string(&theirs).unwrap(),
            "{text}"
        );
        assert_eq!(ours.to_string(), theirs.to_string(), "{text}");
        assert_eq!(format!("{ours:#}"), format!("{theirs:#}"), "{text}");
        assert_eq!(format!("{ours:?}"), format!("{theirs:?}"), "{text}");
        assert_eq!(text.parse::<Value>().unwrap(), ours);
    }
}

#[test]
fn rejects_what_serde_json_rejects() {
    let nested = format!("{}{}", "[".repeat(200), "]".repeat(200));
    for text in [
        "",
        "{",
        r#"{"a":}"#,
        "[1,]",
        r#"{"a":1,}"#,
        "nul",
        r#""\ud800""#,
        "1 2",
        nested.as_str(),
    ] {
        let ours = serde_json::from_str::<Value>(text).unwrap_err();
        let theirs = serde_json::from_str::<serde_json::Value>(text).unwrap_err();
        assert_eq!(ours.to_string(), theirs.to_string(), "{text}");
        assert_eq!(ours.classify(), theirs.classify(), "{text}");
    }
}

#[test]
fn remove_is_swap_remove_as_in_serde_json() {
    let text = r#"{"a":1,"b":2,"c":3,"d":4}"#;
    let mut ours: Value = serde_json::from_str(text).unwrap();
    let mut theirs: serde_json::Value = serde_json::from_str(text).unwrap();
    ours.as_object_mut().unwrap().remove("a");
    theirs.as_object_mut().unwrap().remove("a");
    ours.as_object_mut().unwrap().shift_remove("c");
    theirs.as_object_mut().unwrap().shift_remove("c");
    ours["e"] = json!(5);
    theirs["e"] = serde_json::json!(5);
    assert_eq!(ours.to_string(), theirs.to_string());
}

#[test]
fn json_macro_builds_what_serde_json_builds() {
    let n = 2.5;
    let inner = json!({"x": [1, null]});
    let ours = json!({"b": n, "a": [true, "s", inner, {"k": -1}], "c": null});
    let theirs =
        serde_json::json!({"b": n, "a": [true, "s", {"x": [1, null]}, {"k": -1}], "c": null});
    assert_eq!(ours.to_string(), theirs.to_string());
}

#[derive(Debug, PartialEq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Typed {
    name: String,
    is_array: Option<bool>,
    values: Vec<f64>,
    tags: BTreeMap<String, u8>,
    kind: Kind,
}

#[derive(Debug, PartialEq, Deserialize, serde::Serialize)]
enum Kind {
    Unit,
    Newtype(String),
    Struct { a: i32 },
}

#[test]
fn decodes_as_serde_json_decodes() {
    let good = [
        r#"{"name":"n","isArray":null,"values":[1,2.5],"tags":{"t":1},"kind":"Unit"}"#,
        r#"{"name":"n","values":[],"tags":{},"kind":{"Newtype":"x"}}"#,
        r#"{"name":"n","values":[],"tags":{},"kind":{"Struct":{"a":-1}}}"#,
    ];
    let bad = [
        r#"{"name":1,"values":[],"tags":{},"kind":"Unit"}"#,
        r#"{"name":"n","values":[],"tags":{},"kind":"Unit","extra":1}"#,
        r#"{"name":"n","values":"x","tags":{},"kind":"Unit"}"#,
        r#"{"name":"n","values":[],"tags":{"t":300},"kind":"Unit"}"#,
        r#"{"name":"n","values":[],"tags":{},"kind":{"Newtype":"x","Unit":null}}"#,
        r#"{"name":"n","values":[],"tags":{},"kind":["Unit"]}"#,
        r#"[1]"#,
    ];
    for text in good.iter().chain(&bad) {
        let ours: Value = serde_json::from_str(text).unwrap();
        let theirs: serde_json::Value = serde_json::from_str(text).unwrap();
        let by_value = from_value::<Typed>(ours.clone()).map_err(|e| e.to_string());
        let by_ref = Typed::deserialize(&ours).map_err(|e| e.to_string());
        let expected = serde_json::from_value::<Typed>(theirs.clone()).map_err(|e| e.to_string());
        assert_eq!(by_value, expected, "{text}");
        assert_eq!(by_ref, expected, "{text}");
        if let Ok(typed) = expected {
            assert_eq!(
                to_value(&typed).unwrap().to_string(),
                serde_json::to_value(&typed).unwrap().to_string()
            );
        }
    }
}

#[test]
fn numeric_keys_decode_as_in_serde_json() {
    for text in [
        r#"{"1":2,"-3":4}"#,
        r#"{"x":1}"#,
        r#"{" 1":1}"#,
        r#"{"1 ":1}"#,
    ] {
        let ours: Value = serde_json::from_str(text).unwrap();
        let theirs: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            from_value::<BTreeMap<i64, i64>>(ours).is_ok(),
            serde_json::from_value::<BTreeMap<i64, i64>>(theirs).is_ok(),
            "{text}"
        );
    }
}

/// A map is keyed by the process-wide SipHash keys, not `RandomState`.
#[test]
fn objects_hash_with_the_seeded_state() {
    let mut map = Map::new();
    map.insert("k".into(), Value::Null);
    assert_eq!(
        map.hasher().hash_one("k"),
        SeededState::default().hash_one("k")
    );
    let parsed: Value = serde_json::from_str(r#"{"a":{"b":1}}"#).unwrap();
    let inner = parsed["a"].as_object().unwrap();
    assert_eq!(
        inner.hasher().hash_one("b"),
        SeededState::default().hash_one("b")
    );
}

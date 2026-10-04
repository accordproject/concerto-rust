use super::*;

/// `object_entries_ref` gives `object_keys_ref`'s keys, in its order
/// (integer-like keys first, ascending), each with its value.
#[test]
fn object_entries_ref_matches_object_keys_ref() {
    for keys in [
        vec!["b", "a", "c"],
        vec!["b", "10", "a", "2", "01", "4294967295", "0"],
        vec![],
    ] {
        let mut map = crate::value::JsObject::default();
        for (i, key) in keys.iter().enumerate() {
            map.insert((*key).to_string(), JsValue::Number(i as f64));
        }
        let entries = object_entries_ref(&map);
        let object = JsValue::Object(map.clone());
        let expected = object_keys_ref(&object).unwrap_or_default();
        let got: Vec<&str> = entries.iter().map(|(k, _)| &**k).collect();
        let want: Vec<&str> = expected.iter().map(|k| &**k).collect();
        assert_eq!(got, want);
        for (key, value) in &entries {
            assert_eq!(Some(*value), map.get(&**key));
        }
    }
}

/// `ResourceValidator.checkItem`'s switch has no `default:` arm and
/// starts from `invalid = false`, so a type name it does not list is
/// valid.
#[test]
fn primitive_field_valid_matches_the_ts_switch() {
    assert!(primitive_field_valid(
        "String",
        &JsValue::String("x".into())
    ));
    assert!(!primitive_field_valid("String", &JsValue::Number(1.0)));
    assert!(primitive_field_valid("Double", &JsValue::Number(1.5)));
    assert!(!primitive_field_valid("Double", &JsValue::Number(f64::NAN)));
    assert!(!primitive_field_valid(
        "Integer",
        &JsValue::Number(f64::INFINITY)
    ));
    assert!(primitive_field_valid("Boolean", &JsValue::Bool(false)));
    assert!(!primitive_field_valid(
        "DateTime",
        &JsValue::String("x".into())
    ));
    assert!(primitive_field_valid("Unknown", &JsValue::Number(1.0)));
    assert!(primitive_field_valid("Unknown", &JsValue::Null));
}

/// BC-07: `strictQualifiedDateTimes: false` opens no lenient path. An
/// embedded NUL (DV-009), a date-only string or an impossible
/// date is rejected with or without the flag, with the same
/// `ValidationException` as strict mode; a strict string is accepted
/// either way, with `utcOffset` applied only when the flag is not set.
#[test]
fn datetime_strings_are_strict_either_way() {
    let non_strict = FromJsonOptions {
        accept_resources_for_relationships: false,
        utc_offset: UtcOffset::Number(60.0),
        strict_qualified_date_times: false,
        ..FromJsonOptions::default()
    };
    let strict = FromJsonOptions {
        strict_qualified_date_times: true,
        ..non_strict.clone()
    };
    for s in [
        "1970-01-01T00:00:00.000+00:00\u{0}",
        "2020-01-01",
        "2016-10-20T05:34:03.519",
        "2024-02-30T00:00:00Z",
        "2024-01-02T24:00:00Z",
    ] {
        for options in [&non_strict, &strict] {
            let result = convert_primitive("DateTime", &JsValue::String(s.into()), options, "$.t");
            let err = result.expect_err(s);
            assert_eq!(err.kind().ts_class(), "ValidationException", "{s:?}: {err}");
        }
    }
    let s = JsValue::String("2021-01-01T00:00:00Z".into());
    let Ok(JsValue::DateTime(d)) = convert_primitive("DateTime", &s, &non_strict, "$.t") else {
        panic!("non-strict should accept a strict string");
    };
    assert_eq!(d.utc_offset(), 60.0);
    let Ok(JsValue::DateTime(d)) = convert_primitive("DateTime", &s, &strict, "$.t") else {
        panic!("strict should accept a strict string");
    };
    assert!(d.is_utc());
}

/// BC-10, DV-012: `±Infinity` is not an Integer or a Long,
/// whatever the options; the same `ValidationException` as a
/// fractional number. `NaN` was already rejected.
#[test]
fn non_finite_integers_and_longs_are_rejected() {
    let options = FromJsonOptions::default();
    for type_name in ["Integer", "Long"] {
        for n in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 1.5] {
            let err = convert_primitive(type_name, &JsValue::Number(n), &options, "$.i")
                .expect_err(&format!("{type_name} {n}"));
            assert_eq!(err.kind().ts_class(), "ValidationException", "{n}: {err}");
            assert_eq!(
                err.to_string(),
                format!("Expected value at path `$.i` to be of type `{type_name}`")
            );
        }
        for n in [0.0, -3.0, 9_007_199_254_740_993.0, 1e300] {
            assert_eq!(
                convert_primitive(type_name, &JsValue::Number(n), &options, "$.i").ok(),
                Some(JsValue::Number(n)),
                "{type_name} {n}"
            );
        }
    }
    // A Double keeps them: BC-10 is about Integer and Long only.
    assert_eq!(
        convert_primitive("Double", &JsValue::Number(f64::INFINITY), &options, "$.d").ok(),
        Some(JsValue::Number(f64::INFINITY))
    );
}

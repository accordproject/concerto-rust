use crate::json::Value;
use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde::de::DeserializeSeed;

use super::{Kept, KeptSeed, Location, LocationSeed};
use crate::introspect::typed_ast::strict_from_value;

/// `location` values: the usual `Range`, and every way one can differ
/// from it (each read field by field as far as it can be, then as a
/// [`Kept`]).
pub(crate) const CASES: [&str; 24] = [
    r#"{"$class":"concerto.metamodel@1.0.0.Range","start":{"offset":78,"line":4,"column":3,"$class":"concerto.metamodel@1.0.0.Position"},"end":{"offset":105,"line":5,"column":1,"$class":"concerto.metamodel@1.0.0.Position"}}"#,
    r#"{"start":{"line":1,"column":2,"offset":3},"end":{"line":1.0,"column":-2,"offset":3e2},"source":null}"#,
    r#"{"start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3},"source":"x.cto"}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.Range","start":{"$class":"concerto.metamodel@1.0.0.Position","line":1,"column":2,"offset":3},"end":{"$class":"Position","line":1,"column":2,"offset":3}}"#,
    r#"{"$class":"x","start":{"line":1.5,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3},"source":"x"}"#,
    r#"{"start":{"line":1,"column":2,"offset":3,"line":7},"end":{"line":1,"column":2,"offset":3},"start":{"line":9,"column":2,"offset":3}}"#,
    r#"{"start":"x","end":{"line":1,"column":2,"offset":3},"start":{"line":9,"column":2,"offset":3}}"#,
    r#"{"start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3},"extra":1}"#,
    r#"{"start":{"line":1,"column":2,"offset":3,"extra":{}},"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{"start":{"line":1,"column":2},"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{"start":{"line":null,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{"start":{"line":"1","column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{"$class":null,"start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{"$class":"a","$class":"b","start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{"start":["concerto.metamodel@1.0.0.Position",1,2,3],"end":{"line":1,"column":2,"offset":3}}"#,
    r#"["a",["b",1,2,3],["c",4,5,6]]"#,
    r#"{"start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{"start":{"line":18446744073709551616,"column":2,"offset":-3},"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{"end":{"line":1,"column":2,"offset":3}}"#,
    r#"{}"#,
    r#"null"#,
    r#"7"#,
    r#""x""#,
    r#"true"#,
];

/// Text that is not JSON.
const INVALID: [&str; 5] = [
    r#"{"start":1e400}"#,
    r#"{"start":{"line":1e400}}"#,
    r#"{"start":"\ud800"}"#,
    r#"{"start":{"line":1,}}"#,
    r#"{"start" 1}"#,
];

fn kept(text: &str) -> Result<Kept, serde_json::Error> {
    let mut d = serde_json::Deserializer::from_str(text);
    let kept = KeptSeed.deserialize(&mut d)?;
    d.end()?;
    Ok(kept)
}

fn location(text: &str) -> Result<Location, serde_json::Error> {
    let mut d = serde_json::Deserializer::from_str(text);
    let location = LocationSeed.deserialize(&mut d)?;
    d.end()?;
    Ok(location)
}

/// The `Value`, decoded strictly: the reference for the kept read.
fn decoded_from_value(value: &Value) -> Result<String, ()> {
    strict_from_value::<Option<mm::Range>>(value)
        .map(|range| format!("{range:?}"))
        .map_err(|_| ())
}

/// Each case read as a [`Kept`] is its `Value` parse, and decodes into
/// `Option<Range>` exactly as that `Value` does (the same result, or an
/// error for both).
#[test]
fn kept_is_the_value_and_decodes_as_it() {
    for text in CASES {
        let value: Value = serde_json::from_str(text).unwrap();
        let kept = kept(text).unwrap();
        assert_eq!(kept.to_value(), value, "{text}");
        assert_eq!(
            serde_json::to_string(&kept.to_value()).unwrap(),
            serde_json::to_string(&value).unwrap(),
            "{text}"
        );
        let decoded = kept
            .strict_decode::<Option<mm::Range>>()
            .map(|range| format!("{range:?}"))
            .map_err(|_| ());
        assert_eq!(decoded, decoded_from_value(&value), "{text}");
    }
}

/// The same for a [`Location`], read from the text and from its
/// parsed `Value` (`typed_ast::from_value`); the usual cases are read
/// field by field.
#[test]
fn location_is_the_value_and_decodes_as_it() {
    let mut field_by_field = 0;
    for text in CASES {
        let value: Value = serde_json::from_str(text).unwrap();
        for location in [
            location(text).unwrap(),
            LocationSeed.deserialize(&value).unwrap(),
        ] {
            if matches!(location, Location::Range(_)) {
                field_by_field += 1;
            }
            assert_eq!(location.to_value(), value, "{text}");
            assert_eq!(
                serde_json::to_string(&location.to_value()).unwrap(),
                serde_json::to_string(&value).unwrap(),
                "{text}"
            );
            let decoded = location
                .decode()
                .map(|range| format!("{range:?}"))
                .map_err(|_| ());
            assert_eq!(decoded, decoded_from_value(&value), "{text}");
        }
    }
    // Ten cases from both reads (the first five; a missing field, an
    // escaped key and a number past `u64`), and the three with a
    // repeated key from the `Value`, which has only the last.
    assert_eq!(field_by_field, 23);
}

/// Text that is not JSON is the same kind of error for both reads as
/// for a `Value` parse.
#[test]
fn kept_and_location_reject_what_a_value_parse_rejects() {
    for text in INVALID {
        let value = serde_json::from_str::<Value>(text).unwrap_err();
        let kept = kept(text).unwrap_err();
        let location = location(text).unwrap_err();
        assert_eq!(value.classify(), kept.classify(), "{text}");
        assert_eq!(value.classify(), location.classify(), "{text}");
    }
}

/// `decorators` values: the usual lists, and every way one can differ
/// from them.
pub(crate) const DECORATOR_CASES: [&str; 28] = [
    r#"[]"#,
    r#"null"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x"},{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":1.5},{"$class":"concerto.metamodel@1.0.0.DecoratorBoolean","value":true},{"$class":"concerto.metamodel@1.0.0.DecoratorTypeReference","type":{"$class":"concerto.metamodel@1.0.0.TypeIdentifier","name":"T"},"isArray":true},{"$class":"concerto.metamodel@1.0.0.DecoratorTypeReference","type":{"$class":"concerto.metamodel@1.0.0.TypeIdentifier","name":"U","namespace":"org.x@1.0.0"}}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","location":{"$class":"concerto.metamodel@1.0.0.Range","start":{"offset":1,"line":1,"column":2,"$class":"concerto.metamodel@1.0.0.Position"},"end":{"offset":5,"line":1,"column":6,"$class":"concerto.metamodel@1.0.0.Position"}},"arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":-0,"location":null}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"a"},{"$class":"concerto.metamodel@1.0.0.Decorator","name":"b","arguments":[]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","extra":1}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x","extra":1}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorFoo","value":"x"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[null]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"value":"x","$class":"concerto.metamodel@1.0.0.DecoratorString"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":"x"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":1}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"a","name":"b"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","n\u0061me":"d"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator"}]"#,
    r#"[{"name":"d"}]"#,
    r#"[null]"#,
    r#""ab""#,
    r#"{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d"}"#,
    // Lists `Kept::into_decorators` reads by moving strings out, and
    // ones it leaves to the strict decode.
    r#"[{"name":"d","arguments":null,"$class":"concerto.metamodel@1.0.0.Decorator"},{"$class":"Decorator","name":"e\u0021","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":18446744073709551615},{"value":-3,"$class":"concerto.metamodel@1.0.0.DecoratorNumber"},{"$class":"concerto.metamodel@1.0.0.DecoratorBoolean","value":false}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":1e300},{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"\ud83d\ude00"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":1}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorBoolean","value":"true"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x","value":"y"}]}]"#,
    r#"[{"$class":7,"name":"d"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":null}]"#,
];

/// `identified` values.
pub(crate) const IDENTIFIED_CASES: [&str; 20] = [
    r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":"id"}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.Identified"}"#,
    r#"null"#,
    r#"{}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy"}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":"id","extra":1}"#,
    r#"{"name":"id","$class":"concerto.metamodel@1.0.0.IdentifiedBy"}"#,
    r#"{"$class":"x","name":"id"}"#,
    r#"[]"#,
    r#"0"#,
    // Values `IdentifiedSeed` reads field by field, or as far as it
    // can before reading the rest as a `Kept`.
    r#"{"name":"id"}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.Identified","name":"id"}"#,
    r#"{"name":"id","$class":"concerto.metamodel@1.0.0.Identified"}"#,
    r#"{"name":1,"$class":"concerto.metamodel@1.0.0.IdentifiedBy"}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":null}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","$class":"concerto.metamodel@1.0.0.Identified"}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":"a","name":"b"}"#,
    r#"{"n\u0061me":"id","$cl\u0061ss":"concerto.metamodel@1.0.0.IdentifiedBy"}"#,
    r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":"\u0069d"}"#,
    r#"{"extra":1,"$class":"concerto.metamodel@1.0.0.Identified"}"#,
];

/// A `decorators` value read as a [`Kept`] is its `Value`, and
/// decodes into the generated decorators, and gives the processed
/// [`Decorator`]s, exactly as that `Value` does.
///
/// [`Decorator`]: crate::introspect::Decorator
#[test]
fn kept_decorators_are_the_value_and_decode_as_it() {
    use crate::introspect::decorator::{parse_decorator_list, parse_decorators};

    for text in DECORATOR_CASES {
        let value: Value = serde_json::from_str(text).unwrap();
        let kept = kept(text).unwrap();
        assert_eq!(kept.to_value(), value, "{text}");
        let decoded = kept
            .strict_decode::<Option<Vec<mm::Decorator>>>()
            .map(|d| format!("{d:?}"))
            .map_err(|_| ());
        let from_value = strict_from_value::<Option<Vec<mm::Decorator>>>(&value)
            .map(|d| format!("{d:?}"))
            .map_err(|_| ());
        assert_eq!(decoded, from_value, "{text}");
        assert_eq!(
            parse_decorator_list(Some(&kept)),
            parse_decorators(&crate::json!({ "decorators": value })),
            "{text}"
        );
        // Decoded by moving the strings out, the same result.
        let moved = kept
            .into_decorators()
            .map(|d| format!("{d:?}"))
            .map_err(|_| ());
        assert_eq!(moved, from_value, "{text}");
    }
}

/// The same for an `identified` value.
#[test]
fn kept_identified_is_the_value_and_decodes_as_it() {
    for text in IDENTIFIED_CASES {
        let value: Value = serde_json::from_str(text).unwrap();
        let kept = kept(text).unwrap();
        assert_eq!(kept.to_value(), value, "{text}");
        let decoded = kept
            .strict_decode::<Option<mm::Identified>>()
            .map(|d| format!("{d:?}"))
            .map_err(|_| ());
        let from_value = strict_from_value::<Option<mm::Identified>>(&value)
            .map(|d| format!("{d:?}"))
            .map_err(|_| ());
        assert_eq!(decoded, from_value, "{text}");
    }
}

/// An `identified` value read by [`IdentifiedSeed`], from the text and
/// from its `Value`, decodes as its `Value` does; it is kept as a
/// [`Kept`] (its `Value`) unless it is one of the metamodel's own two
/// nodes, which BC-19's shape check accepts.
///
/// [`IdentifiedSeed`]: super::IdentifiedSeed
#[test]
fn identified_is_read_field_by_field_and_decodes_as_its_value() {
    use super::{IdentifiedRead, IdentifiedSeed};
    use crate::instance::metamodel::check_ast_shape;

    let mut field_by_field = 0;
    for text in IDENTIFIED_CASES {
        let value: Value = serde_json::from_str(text).unwrap();
        let from_value = strict_from_value::<Option<mm::Identified>>(&value)
            .map(|d| format!("{d:?}"))
            .map_err(|_| ());
        let mut d = serde_json::Deserializer::from_str(text);
        for read in [
            IdentifiedSeed.deserialize(&mut d).unwrap(),
            IdentifiedSeed.deserialize(&value).unwrap(),
        ] {
            if !matches!(read, IdentifiedRead::Kept(_)) {
                field_by_field += 1;
            }
            match read.decode() {
                Ok((identified, kept)) => {
                    assert_eq!(Ok(format!("{identified:?}")), from_value, "{text}");
                    match kept {
                        Some(kept) => assert_eq!(kept.to_value(), value, "{text}"),
                        None => {
                            let ast = crate::json!({
                                "$class": "concerto.metamodel@1.0.0.Model",
                                "namespace": "org.acme@1.0.0",
                                "declarations": [{
                                    "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                                    "name": "A", "isAbstract": false, "properties": [],
                                    "identified": value,
                                }],
                            });
                            assert!(check_ast_shape(&ast).is_ok(), "{text}");
                        }
                    }
                }
                Err(_) => assert_eq!(from_value, Err(()), "{text}"),
            }
        }
    }
    // From both reads: the two nodes, `IdentifiedBy` in the other key
    // order, with an escaped name, and with escaped keys; from the
    // `Value`, the two with a repeated key (it has only the last).
    assert_eq!(field_by_field, 12);
}

/// `decorators` values: arrays of decorator nodes as the reference
/// parser writes them, and every way one can differ from that, at
/// every point of a node (`DecoratorsSeed`).
const STRAIGHT_DECORATOR_CASES: [&str; 36] = [
    r#"[]"#,
    r#"null"#,
    r#"{}"#,
    r#"7"#,
    r#""x""#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"a \"b\""}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":1},{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":-2.5e3},{"$class":"concerto.metamodel@1.0.0.DecoratorBoolean","value":false}]},{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Other"}]"#,
    r#"[{"$class":"Decorator","name":"Term"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":null}]"#,
    r#"[{"name":"Term","$class":"concerto.metamodel@1.0.0.Decorator"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","arguments":[],"name":"Term"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator"}]"#,
    r#"[{}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","name":"Again"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[],"arguments":[]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","location":{"$class":"concerto.metamodel@1.0.0.Range","start":{"offset":1,"line":1,"column":1,"$class":"concerto.metamodel@1.0.0.Position"},"end":{"offset":2,"line":1,"column":2,"$class":"concerto.metamodel@1.0.0.Position"}}}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","extra":1}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":7}]"#,
    r#"[{"$class":7,"name":"Term"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"A"},null,{"$class":"concerto.metamodel@1.0.0.Decorator","name":"B"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"A"},{"$class":"concerto.metamodel@1.0.0.Decorator","name":"B","x":[]},{"$class":"concerto.metamodel@1.0.0.Decorator","name":"C"}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":{}}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"value":"x","$class":"concerto.metamodel@1.0.0.DecoratorString"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":1}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":"1"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x","extra":1}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorBoolean","value":true},{"$class":"concerto.metamodel@1.0.0.DecoratorTypeReference","type":{"$class":"concerto.metamodel@1.0.0.TypeIdentifier","name":"T"},"isArray":true}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":2},null]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"x","value":"y"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x","value":"y"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","$class":"concerto.metamodel@1.0.0.DecoratorString","value":"y"}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{}]}]"#,
    r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"Term","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":1}],"name":"Again"}]"#,
];

/// A `decorators` value's [`Decorators`](super::Decorators), or the
/// error decoding it, comparable across the two reads.
fn describe(read: Result<super::Decorators, serde_json::Error>) -> String {
    match read {
        Ok(decorators) => format!(
            "{} {:?} {}",
            serde_json::to_string(&decorators.node).unwrap(),
            decorators.list,
            decorators.conforms
        ),
        Err(err) => format!("error {err}"),
    }
}

/// An array of decorator nodes read straight into its decorators
/// (`DecoratorsSeed`) gives exactly what the same value read as a
/// `Kept` gives (`Decorators::from_kept`), whatever the value.
#[test]
fn decorators_read_as_they_are_read_from_a_kept() {
    use super::{Decorators, DecoratorsSeed};

    let mut fast_reads = 0;
    for text in DECORATOR_CASES.iter().chain(&STRAIGHT_DECORATOR_CASES) {
        let kept = KeptSeed
            .deserialize(&mut serde_json::Deserializer::from_str(text))
            .unwrap();
        let expected = describe(Decorators::from_kept(kept));
        let read = DecoratorsSeed
            .deserialize(&mut serde_json::Deserializer::from_str(text))
            .unwrap();
        let actual = match read {
            Ok(decorators) => {
                fast_reads += 1;
                describe(Ok(decorators))
            }
            // What is not read straight is decoded as a `Kept`: the one
            // `KeptSeed` reads, but that a number read straight first
            // is put back as the `f64` it was read as (`2.0` for `2`),
            // which decodes to the same decorators.
            Err(value) => describe(Decorators::from_kept(value)),
        };
        assert_eq!(actual, expected, "{text}");
    }
    // The arrays of nodes in the reference parser's key order, and `[]`.
    assert_eq!(fast_reads, 11);
}

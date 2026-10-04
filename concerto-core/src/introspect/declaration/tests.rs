use super::*;

fn decl(json: serde_json::Value) -> Declaration {
    Declaration::try_from(&json).expect("valid declaration")
}

#[test]
fn parses_concept_with_typed_properties() {
    let d = decl(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
        "name": "Person",
        "isAbstract": false,
        "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Thing" },
        "properties": [
            { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "firstName", "isArray": false, "isOptional": false },
            { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "age", "isArray": false, "isOptional": true }
        ]
    }));

    let c = d.as_class().expect("class");
    assert_eq!(c.kind(), ClassKind::Concept);
    assert_eq!(c.name(), "Person");
    assert!(!c.is_abstract());
    assert_eq!(c.super_type().map(|t| t.name.as_str()), Some("Thing"));
    assert_eq!(c.own_properties().len(), 2);
    assert_eq!(c.own_properties()[0].type_name(), Some("String"));
    assert!(c.own_properties()[1].is_optional());

    assert!(d.is_class_declaration());
    assert!(!d.is_enum_declaration());
}

fn identified(value: serde_json::Value) -> Result<Declaration> {
    Declaration::try_from(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
        "name": "Person",
        "isAbstract": false,
        "identified": value,
        "properties": []
    }))
}

/// An `IdentifiedBy` with an empty name: TS's `this.idField =
/// this.ast.identified.name` is read only by truthiness afterwards
/// (`if (this.idField)`), so this loads with no id field at all.
#[test]
fn an_empty_identified_by_name_loads_with_no_id_field() {
    let d = identified(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": ""
    }))
    .expect("an empty name loads");
    let c = d.as_class().expect("class");
    assert!(!c.is_identified());
    assert!(c.own_properties().is_empty());
}

/// BR-09: `identified` is read strictly. TS 5.0.0 loaded a nullish or
/// falsy non-string `IdentifiedBy` name as no identity, and a `$class`
/// of another namespace (`foo.IdentifiedBy`) or any other truthy value
/// as system identity. BC-19's shape check rejects all of these first;
/// with the check off they are the loader's error.
#[test]
fn a_malformed_identified_is_an_error() {
    for value in [
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": null }),
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": 0 }),
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": false }),
        serde_json::json!({ "$class": "foo.IdentifiedBy", "name": "email" }),
        serde_json::json!({ "name": "email" }),
        serde_json::json!({}),
        serde_json::json!(true),
        serde_json::json!(0),
        serde_json::json!(""),
        serde_json::json!([]),
    ] {
        let err = identified(value.clone()).expect_err(&value.to_string());
        assert_eq!(err.code(), "modelfile-load-unreadable", "{value}");
        assert_eq!(err.kind(), ErrorKind::IllegalModel, "{value}");
    }
    // `null` is no identity.
    let d = identified(serde_json::Value::Null).expect("null loads");
    assert!(!d.as_class().expect("class").is_identified());
}

#[test]
fn asset_kind_is_tagged() {
    let d = decl(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
        "name": "Vehicle",
        "isAbstract": false,
        "properties": []
    }));
    assert_eq!(d.declaration_kind(), "AssetDeclaration");
    assert_eq!(d.as_class().unwrap().kind(), ClassKind::Asset);
}

#[test]
fn parses_enum_and_scalar_and_map() {
    let e = decl(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.EnumDeclaration",
        "name": "Color",
        "properties": [
            { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" },
            { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "GREEN" }
        ]
    }));
    assert!(e.is_enum_declaration());
    assert!(!e.is_class_declaration());
    assert_eq!(e.name(), "Color");

    let s = decl(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringScalar",
        "name": "Email",
        "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": ".*", "flags": "" }
    }));
    assert_eq!(s.as_scalar().unwrap().scalar_type(), "String");
    assert_eq!(s.name(), "Email");
    assert!(s.is_scalar_declaration());

    let m = decl(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.MapDeclaration",
        "name": "Dictionary",
        "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
        "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" }
    }));
    assert!(m.is_map_declaration());
    assert_eq!(m.name(), "Dictionary");
}

#[test]
fn unknown_declaration_kind_errors() {
    assert!(
        Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.WidgetDeclaration",
            "name": "X"
        }))
        .is_err()
    );
}

#[test]
fn missing_class_is_rejected() {
    // TS `fromAst`'s `default` case, `thing.$class` interpolated as
    // `undefined`.
    let err = Declaration::try_from(&serde_json::json!({ "name": "X" }));
    assert_eq!(
        err.unwrap_err().to_string(),
        "Unrecognised model element \"undefined\"."
    );
}

#[test]
fn non_array_properties_is_rejected() {
    assert!(
        Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Bad",
            "properties": { "not": "an array" }
        }))
        .is_err()
    );
}

#[test]
fn a_declaration_name_must_be_an_identifier() {
    for kind in ["ConceptDeclaration", "EnumDeclaration"] {
        let mut ast = serde_json::json!({
            "$class": format!("concerto.metamodel@1.0.0.{kind}"),
            "name": "1Bad", "properties": []
        });
        if kind == "ConceptDeclaration" {
            ast["isAbstract"] = serde_json::json!(false);
        }
        let err = Declaration::try_from(&ast);
        assert_eq!(
            err.unwrap_err().to_string(),
            "Invalid class name '1Bad'",
            "{kind} with a bad name should be rejected"
        );
    }
}

/// TS `ModelFile.fromAst` matches the full metamodel `$class` strings
/// and the six scalar kinds exactly; anything else — a bare short name,
/// another namespace's, an unknown `*Scalar`, a missing `$class` — is
/// "Unrecognised model element", ahead of the name check, naming the
/// file and no location.
#[test]
fn only_the_exact_metamodel_classes_are_recognised() {
    let cases = [
        (
            serde_json::json!("ConceptDeclaration"),
            "ConceptDeclaration",
        ),
        (
            serde_json::json!("other.ns@1.0.0.ConceptDeclaration"),
            "other.ns@1.0.0.ConceptDeclaration",
        ),
        (
            serde_json::json!("concerto.metamodel@1.0.0.FooScalar"),
            "concerto.metamodel@1.0.0.FooScalar",
        ),
        (serde_json::Value::Null, "null"),
    ];
    for (class, shown) in cases {
        for name in ["Good", "1bad"] {
            let err = Declaration::from_model_json(
                    &serde_json::json!({
                        "$class": class, "name": name, "isAbstract": false, "properties": [],
                        "location": {
                            "$class": "concerto.metamodel@1.0.0.Range",
                            "start": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 0, "line": 1, "column": 1 },
                            "end": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 5, "line": 1, "column": 6 }
                        }
                    }),
                    "org.acme@1.0.0",
                    Some("x.cto"),
                )
                .unwrap_err();
            let err = err.contract().clone();
            assert_eq!(err.kind, ErrorKind::IllegalModel);
            assert_eq!(err.location, None);
            assert_eq!(
                err.final_message(),
                format!("Unrecognised model element \"{shown}\". File 'x.cto': ")
            );
        }
    }
    let err =
        Declaration::from_model_json(&serde_json::json!({ "name": "A" }), "org.acme@1.0.0", None)
            .unwrap_err();
    assert_eq!(err.to_string(), "Unrecognised model element \"undefined\".");
}

/// TS `Declaration.process` checks the name before
/// `ClassDeclaration.process` looks at the fields, so a bad name wins
/// over a system property name, and the error names the file.
#[test]
fn an_invalid_class_name_is_reported_before_a_system_field_name() {
    let err = Declaration::from_model_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "1bad", "isAbstract": false,
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "$class",
                  "isArray": false, "isOptional": false }
            ]
        }),
        "org.acme@1.0.0",
        Some("x.cto"),
    )
    .unwrap_err();
    let err = err.contract().clone();
    assert_eq!(
        err.final_message(),
        "Invalid class name '1bad' File 'x.cto': "
    );
}

#[test]
fn scalar_reports_its_concrete_kind() {
    let s = decl(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Email"
    }));
    assert_eq!(s.declaration_kind(), "StringScalar");
}

#[test]
fn scalar_with_reversed_range_is_rejected() {
    let err = Declaration::try_from(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.IntegerScalar",
        "name": "Score",
        "validator": {
            "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
            "lower": 10, "upper": 5
        }
    }));
    assert!(err.unwrap_err().to_string().contains("Lower bound"));
}

#[test]
fn scalar_with_valid_range_is_accepted() {
    assert!(
        Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.IntegerScalar",
            "name": "Score",
            "validator": {
                "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
                "lower": 0, "upper": 10
            }
        }))
        .is_ok()
    );
}

/// The code of a `modelfile-load-unreadable` error: a node the typed
/// read cannot read, an `IllegalModelException`.
fn unreadable(err: Error) -> String {
    let contract = err.contract();
    assert_eq!(contract.kind, ErrorKind::IllegalModel, "{err}");
    assert_eq!(contract.code, "modelfile-load-unreadable", "{err}");
    err.to_string()
}

/// A class declaration's `properties` must be an array of property
/// nodes whose `$class` is a full metamodel property class (BC-19's
/// shape check rejects anything else first). A malformed one is a
/// `modelfile-load-unreadable` error, not TS 5.0.0's per-site guards
/// (`classdeclaration-validate-undefined-properties`,
/// `classdeclaration-process-unrecmodelelem`).
#[test]
fn a_malformed_properties_value_is_an_unreadable_ast() {
    let class = |properties: Option<serde_json::Value>| {
        let mut node = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Bad",
            "isAbstract": false
        });
        if let Some(properties) = properties {
            node["properties"] = properties;
        }
        node
    };
    let property = |class: &str| {
        serde_json::json!([
            { "$class": class, "name": "firstName", "isArray": false, "isOptional": false }
        ])
    };
    for node in [
        class(None),
        class(Some(serde_json::json!({ "not": "an array" }))),
        class(Some(property("StringProperty"))),
        class(Some(property(
            "concerto.metamodel@1.0.0.StringPropertyconcerto.metamodel@1.0.0.StringProperty",
        ))),
        class(Some(serde_json::json!([null]))),
    ] {
        let err = Declaration::from_model_json(&node, "org.acme@1.0.0", Some("x.cto")).unwrap_err();
        unreadable(err);
    }
}

/// A map declaration is read strictly into the generated struct, so a
/// key or value of a kind the metamodel does not declare, a missing
/// key, value or name, or an object value without a well-formed `type`
/// is a `modelfile-load-unreadable` error. TS 5.0.0's per-site guards
/// for these shapes (`MapDeclaration must contain ...`, the `'in'`
/// operator `TypeError`) are gone: BC-19's shape check rejects every
/// one of them first.
#[test]
fn a_malformed_map_is_an_unreadable_ast() {
    let object_value = |ty: serde_json::Value| serde_json::json!({ "$class": "concerto.metamodel@1.0.0.ObjectMapValueType", "type": ty });
    let mut shapes = Vec::new();
    for class in [
        "StringMapKeyType",
        "foo.StringMapKeyType",
        "concerto.metamodel@1.0.0.IntegerMapKeyType",
    ] {
        shapes.push(map_to_nope(
            serde_json::json!({ "$class": class }),
            serde_json::json!({}),
        ));
    }
    for value in [
        serde_json::json!({ "$class": "StringMapValueType" }),
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.ObjectMapValueType" }),
        object_value(serde_json::Value::Null),
        object_value(serde_json::json!(true)),
        object_value(serde_json::json!([])),
        object_value(serde_json::json!({ "$class": null, "name": "Foo" })),
    ] {
        let mut node = map_to_nope(string_key(), serde_json::json!({}));
        node["value"] = value;
        shapes.push(node);
    }
    for key in ["key", "value", "name"] {
        let mut node = map_to_nope(string_key(), serde_json::json!({}));
        node.as_object_mut().unwrap().remove(key);
        shapes.push(node);
    }
    shapes.push(map_to_nope(string_key(), serde_json::json!({ "name": 5 })));
    shapes.push(map_to_nope(
        string_key(),
        serde_json::json!({ "decorators": "x" }),
    ));
    for node in shapes {
        let err = Declaration::try_from(&node).unwrap_err();
        unreadable(err);
    }
}

#[test]
fn an_enum_property_in_a_class_declaration_is_kept() {
    let d = decl(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
        "name": "C",
        "properties": [
            { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "s" }
        ]
    }));
    let c = d.as_class().unwrap();
    assert_eq!(c.own_properties().len(), 1);
    assert!(c.own_properties()[0].is_enum_value());
}

/// TS `ModelFile.fromAst` matches the fully-qualified `$class`, so a
/// short scalar `$class` is an unrecognised model element.
#[test]
fn a_scalar_class_given_as_the_short_name_is_unrecognised() {
    let err = Declaration::try_from(&serde_json::json!({
        "$class": "StringScalar",
        "name": "Email"
    }));
    assert_eq!(
        err.unwrap_err().to_string(),
        "Unrecognised model element \"StringScalar\"."
    );
}

#[test]
fn unknown_scalar_kind_errors() {
    let err = Declaration::try_from(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.MysteryScalar",
        "name": "X"
    }));
    // TS `fromAst` lists the six scalar kinds exactly.
    assert_eq!(
        err.unwrap_err().to_string(),
        "Unrecognised model element \"concerto.metamodel@1.0.0.MysteryScalar\"."
    );
}

/// A map declaration with the given key, and an object value naming `Nope`.
fn map_to_nope(key: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let mut map = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.MapDeclaration",
        "name": "M",
        "key": key,
        "value": {
            "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Nope" }
        }
    });
    for (field, value) in extra.as_object().unwrap() {
        map[field] = value.clone();
    }
    map
}

fn string_key() -> serde_json::Value {
    serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" })
}

#[test]
fn a_well_formed_map_is_typed() {
    let d = decl(map_to_nope(string_key(), serde_json::json!({})));
    let map = d.as_map().unwrap();
    assert_eq!(map.key_kind(), "StringMapKeyType");
    assert_eq!(map.value_kind(), "ObjectMapValueType");
    assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
}

/// A `MapDeclaration` with the given key and value nodes.
fn map_with(key: serde_json::Value, value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.MapDeclaration",
        "name": "MapPermutation1",
        "key": key,
        "value": value,
    })
}

fn kind(short: &str) -> serde_json::Value {
    serde_json::json!({ "$class": format!("concerto.metamodel@1.0.0.{short}") })
}

fn object_kind(short: &str, type_name: &str) -> serde_json::Value {
    serde_json::json!({
        "$class": format!("concerto.metamodel@1.0.0.{short}"),
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": type_name },
    })
}

// TS: MapDeclaration test/introspect/mapdeclaration.js `#getKey` "should
// return the correct Type when called".
#[test]
fn key_type_name_is_string_for_a_string_key() {
    let d = decl(map_with(
        kind("StringMapKeyType"),
        kind("StringMapValueType"),
    ));
    assert_eq!(d.as_map().unwrap().key_type_name(), "String");
}

#[test]
fn key_type_name_is_datetime_for_a_datetime_key() {
    let d = decl(map_with(
        kind("DateTimeMapKeyType"),
        kind("StringMapValueType"),
    ));
    assert_eq!(d.as_map().unwrap().key_type_name(), "DateTime");
}

// TS: "should return the correct Type when called - Scalar String/DateTime":
// an object key's type is the raw referenced name, unresolved.
#[test]
fn key_type_name_is_the_raw_referenced_name_for_an_object_key() {
    let d = decl(map_with(
        object_kind("ObjectMapKeyType", "GUID"),
        kind("StringMapValueType"),
    ));
    assert_eq!(d.as_map().unwrap().key_type_name(), "GUID");
}

// TS: MapDeclaration test/introspect/mapdeclaration.js `#getValue` "should
// return the correct Type when called", one case per primitive value kind.
#[test]
fn value_type_name_covers_every_primitive_kind() {
    let cases = [
        ("BooleanMapValueType", "Boolean"),
        ("DateTimeMapValueType", "DateTime"),
        ("StringMapValueType", "String"),
        ("IntegerMapValueType", "Integer"),
        ("LongMapValueType", "Long"),
        ("DoubleMapValueType", "Double"),
    ];
    for (mm_kind, expected) in cases {
        let d = decl(map_with(kind("StringMapKeyType"), kind(mm_kind)));
        assert_eq!(
            d.as_map().unwrap().value_type_name(),
            expected,
            "{mm_kind} should report {expected}"
        );
    }
}

// TS: "should return the correct values when called - Scalar
// String/DateTime", and the relationship value case: an object or
// relationship value's type is the raw referenced name, unresolved.
#[test]
fn value_type_name_is_the_raw_referenced_name_for_an_object_or_relationship_value() {
    let d = decl(map_with(
        kind("StringMapKeyType"),
        object_kind("ObjectMapValueType", "GUID"),
    ));
    assert_eq!(d.as_map().unwrap().value_type_name(), "GUID");

    let d = decl(map_with(
        kind("StringMapKeyType"),
        object_kind("RelationshipMapValueType", "Person"),
    ));
    assert_eq!(d.as_map().unwrap().value_type_name(), "Person");
}

// TS: `#toString` "should give the correct value for Map Declaration".
#[test]
fn to_string_matches_ts() {
    assert_eq!(
        MapDeclaration::to_string("com.acme@1.0.0.Dictionary"),
        "MapDeclaration {id=com.acme@1.0.0.Dictionary}"
    );
}

// TS: `#Introspect` "should return the correct value on introspection".
#[test]
fn declaration_kind_and_is_map_declaration_agree_with_ts() {
    let d = decl(map_with(
        kind("StringMapKeyType"),
        kind("StringMapValueType"),
    ));
    assert_eq!(d.declaration_kind(), "MapDeclaration");
    assert!(d.is_map_declaration());
    assert!(!d.is_class_declaration());
    assert!(!d.is_enum_declaration());
    assert!(!d.is_scalar_declaration());
}

/// Map key and value decorators are read: TS
/// `MapKeyType`/`MapValueType.process` (mapkeytype.ts, mapvaluetype.ts)
/// each run `Decorated.process()` on their own AST node, independently
/// of the map's own decorators.
#[test]
fn key_and_value_decorators_are_read_independently_of_the_map_s_own() {
    let mut key = kind("StringMapKeyType");
    key["decorators"] = serde_json::json!([
        { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "onKey", "arguments": [] }
    ]);
    let mut value = kind("StringMapValueType");
    value["decorators"] = serde_json::json!([
        { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "onValue1", "arguments": [] },
        { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "onValue2", "arguments": [] }
    ]);
    let mut map = map_with(key, value);
    map["decorators"] = serde_json::json!([
        { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "onMap", "arguments": [] }
    ]);
    let decl = decl(map);
    let d = decl.as_map().unwrap();

    assert_eq!(
        d.key_decorators()
            .iter()
            .map(Decorator::name)
            .collect::<Vec<_>>(),
        vec!["onKey"]
    );
    assert_eq!(
        d.value_decorators()
            .iter()
            .map(Decorator::name)
            .collect::<Vec<_>>(),
        vec!["onValue1", "onValue2"]
    );
    assert_eq!(
        crate::introspect::Decorated::decorators(d)
            .iter()
            .map(Decorator::name)
            .collect::<Vec<_>>(),
        vec!["onMap"]
    );
}

/// When a caller has an explicit super type value on hand (even a
/// placeholder), `process_decision` returns it verbatim and never
/// substitutes the implicit `'Concept'` default — the "has a `superType`
/// node at all" signal is `Some(_)` itself, not this string's content.
/// The real WASM binding (`classDeclarationProcess`) never reads this
/// value for that branch: it threads the AST's own raw `superType.name`
/// (which might be `undefined`, `null`, a number, …) straight through to
/// its snapshot instead, exactly because a bare `Option<&str>` cannot
/// represent every JSON shape a fuzzed AST can put there — telling
/// `undefined` apart from an explicit `null` (both "no super type to
/// resolve" in different ways: TS's `_resolveSuperType`/`getProperties`
/// treat a `null` `this.superType` as nothing to resolve, but leave
/// `undefined` to fail resolution and raise "Could not find super type
/// undefined") is that caller's job, verified at the binding/smoke-check
/// level, not this pure decision function's.
#[test]
fn an_explicit_super_type_is_returned_verbatim_not_defaulted() {
    let decision =
        ClassDeclaration::process_decision(Some("placeholder"), false, "C", None, None, "ns.C");
    assert_eq!(decision.super_type.as_deref(), Some("placeholder"));
}

/// The AST naming no `superType` node at all takes the implicit
/// `'Concept'` default.
#[test]
fn an_absent_super_type_node_takes_the_implicit_default() {
    let decision = ClassDeclaration::process_decision(None, false, "C", None, None, "ns.C");
    assert_eq!(decision.super_type.as_deref(), Some("Concept"));
}

/// The system model's own `Concept` declaration is still the one
/// exemption from the implicit default.
#[test]
fn the_system_concept_declaration_has_no_implicit_super_type() {
    let decision = ClassDeclaration::process_decision(
        None,
        true,
        "Concept",
        None,
        None,
        "concerto@1.0.0.Concept",
    );
    assert_eq!(decision.super_type, None);
}

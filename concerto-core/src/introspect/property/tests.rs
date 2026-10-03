use super::*;

#[test]
fn process_derives_type_array_and_optional() {
    let processed = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "email",
        "isArray": true,
        "isOptional": true
    }))
    .expect("valid");
    assert_eq!(processed.name, "email");
    assert_eq!(processed.property_type.as_deref(), Some("String"));
    assert!(processed.type_set);
    assert!(processed.array);
    assert!(processed.optional);
}

#[test]
fn process_object_property_type_is_the_referenced_name() {
    let processed = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ObjectProperty",
        "name": "address",
        "isArray": false,
        "isOptional": false,
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Address" }
    }))
    .expect("valid");
    assert_eq!(processed.property_type.as_deref(), Some("Address"));
}

#[test]
fn process_enum_property_leaves_type_unset() {
    let processed = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.EnumProperty",
        "name": "RED"
    }))
    .expect("valid");
    assert!(!processed.type_set);
    assert_eq!(processed.property_type, None);
    assert!(!processed.array);
    assert!(!processed.optional);
}

#[test]
fn process_rejects_an_invalid_identifier() {
    let err = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "1bad",
        "isArray": false,
        "isOptional": false
    }))
    .unwrap_err();
    assert!(err.to_string().contains("Invalid property name '1bad'"));
}

// accordproject/concerto-rust#219 (P5-05 stage-2 T2c, cluster 1): a
// fuzzer-mutated AST can put any JSON type in `name`, and TS's
// `${this.ast.name}` reports it through JS `ToString`, not as an
// absent/empty name.
#[test]
fn process_rejects_a_non_string_name_with_its_js_stringified_form() {
    let err = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": 1e308,
        "isArray": false,
        "isOptional": false
    }))
    .unwrap_err();
    assert!(err.to_string().contains("Invalid property name '1e+308'"));
}

// accordproject/concerto-rust#219 review (P5-05 stage-2 T2c, "the
// location-suffix cluster"): the `IllegalModelException` an invalid name
// raises carries the AST node's own `location` and a `model_file`
// placeholder, so `ModelManager.addModelFile`'s WASM binding
// (`propertyProcess`) can attach the real JS model file and reproduce
// TS's `File '…': line <n> column <n>, to line <n> column <n>.` suffix.
// This test reproduces that suffix text directly, with no WASM boundary
// to cross: `ContractError::final_message` is the same pure-Rust
// function the native oracle harness itself uses to decorate a message
// exactly as TS's `IllegalModelException` constructor does (OD-2) — once
// a real (not placeholder) file name is filled in, in place of the WASM
// binding, it renders the identical suffix. A prior version of this test
// only asserted `err.location.is_some()`/`err.model_file == Some(None)`
// (the placeholder itself), which checks that a location and a
// model-file slot exist but not that the slot, once filled, actually
// renders TS's suffix text — that is what this asserts.
#[test]
fn process_carries_the_ast_location_and_a_model_file_placeholder_for_an_invalid_name() {
    let mut err = process::<ContractError>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": 1e308,
        "isArray": false,
        "isOptional": false,
        "location": {
            "$class": "concerto.metamodel@1.0.0.Range",
            "start": {
                "$class": "concerto.metamodel@1.0.0.Position",
                "line": 3, "column": 5, "offset": 20
            },
            "end": {
                "$class": "concerto.metamodel@1.0.0.Position",
                "line": 3, "column": 30, "offset": 45
            }
        }
    }))
    .unwrap_err();
    assert!(
        err.location.is_some(),
        "expected the AST's own location on the error"
    );
    assert_eq!(
        err.model_file,
        Some(None),
        "expected a model-file placeholder for the WASM binding to fill in"
    );
    // Fill in the placeholder the way `propertyProcess` fills it from
    // the real JS `ModelFile`, and check the fully decorated message —
    // TS's own `ModelManager.addModelFile` suffix — matches verbatim.
    err.model_file = Some(Some("test.cto".to_string()));
    assert_eq!(
        err.final_message(),
        "Invalid property name '1e+308' File 'test.cto': line 3 column 5, to line 3 column 30. "
    );
}

// accordproject/concerto-rust#219 (P5-05 stage-2 T2c): a `name` whose
// *stringified* form still looks like a valid identifier (`false` ->
// `"false"`) passes the identifier check, but its own raw JS falsiness
// fails TS's second, separate `if (!this.name)` check, which raises a
// plain `Error`, not an `IllegalModelException`.
#[test]
fn process_rejects_a_falsy_name_that_stringifies_to_a_valid_identifier() {
    let ast = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": false,
        "isArray": false,
        "isOptional": false
    });
    let err = process::<Error>(&ast).unwrap_err();
    assert!(
        err.to_string().contains("No name for type"),
        "unexpected error: {err}"
    );
}

// accordproject/concerto-rust#217 (T2a): `ID_REGEX.test(name)` in TS
// coerces a non-string `name` with `ToString` rather than rejecting it,
// so a fuzz-mutated `name` that isn't a JSON string but stringifies to
// a valid identifier, and is itself JS-truthy, is accepted by TS and
// must be accepted here too. Minimised repro:
// `declarations[0].properties[0].name = true`
// (conformance/ModelManager.addModelFile/16267c5478a5f2840469e147.json,
// stage2/triage-clusters.json).
#[test]
fn process_accepts_a_boolean_name_like_ts_string_coercion() {
    let processed = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": true,
        "isArray": false,
        "isOptional": false
    }))
    .expect("TS: ID_REGEX.test(true) tests \"true\", which matches, and true is truthy");
    assert_eq!(processed.name, "true");
}

// Same `ID_REGEX.test` coercion theme, but an absent or `null` `name`
// stringifies to an identifier-shaped string ("undefined"/"null") that
// passes the *first* check, then fails TS's second, separate
// `if (!this.name)` raw-truthiness check (property.ts:
// `this.name = this.ast.name` is a plain, uncoerced assignment) —
// `undefined` and `null` are both JS-falsy, so TS throws `Error('No name
// for type ...')` for both, same as an explicit `false` (the
// `process_rejects_a_falsy_name_that_stringifies_to_a_valid_identifier`
// test above). A prior version of this test wrongly asserted these two
// shapes were *accepted*, conflating "passes the identifier regex" with
// "has a name" (accordproject/concerto-rust#219 review, merge of #217
// and #219's overlapping work on this function).
#[test]
fn process_rejects_a_missing_name_that_stringifies_to_a_valid_identifier() {
    let err = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "isArray": false,
        "isOptional": false
    }))
    .unwrap_err();
    assert!(
        err.to_string().contains("No name for type"),
        "unexpected error: {err}"
    );
}

#[test]
fn process_rejects_a_null_name_that_stringifies_to_a_valid_identifier() {
    let err = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": null,
        "isArray": false,
        "isOptional": false
    }))
    .unwrap_err();
    assert!(
        err.to_string().contains("No name for type"),
        "unexpected error: {err}"
    );
}

/// A `RelationshipProperty` node named `name` whose `type` is `ty`
/// (`None`: no `type` key at all).
fn relationship(name: &str, ty: Option<Value>) -> Value {
    let mut node = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
        "name": name,
        "isArray": false,
        "isOptional": false,
        "location": {
            "$class": "concerto.metamodel@1.0.0.Range",
            "start": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 107, "line": 5, "column": 3 },
            "end": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 129, "line": 6, "column": 1 }
        }
    });
    if let Some(ty) = ty {
        node["type"] = ty;
    }
    node
}

fn contract(err: Error) -> ContractError {
    match Some(err.into_contract()) {
        Some(contract) => contract,
        other => panic!("expected a contract error, got {other:?}"),
    }
}

/// DV-017 (#218): TS throws `TypeError: Cannot read properties of
/// undefined|null (reading 'name')` from `Property.process` for a
/// `RelationshipProperty` with a missing or `null` `type`; Rust raises
/// an `IllegalModelException` instead, through `propertyProcess` (the
/// rust-mode view), with the node's location and the model file to be
/// filled in by the shim.
#[test]
fn process_rejects_a_relationship_with_a_missing_or_null_type() {
    for ty in [None, Some(Value::Null)] {
        let ast = relationship("managerId", ty.clone());
        let err = contract(process::<Error>(&ast).unwrap_err());
        assert_eq!(err.kind, ErrorKind::IllegalModel, "{ty:?}");
        assert_eq!(err.code, "property-process-relationshipnotype");
        assert_eq!(err.message(), "Relationship managerId must have a type");
        assert_eq!(err.location, ast.get("location").cloned());
        assert_eq!(err.model_file, Some(None));
        assert_eq!(
            err.final_message(),
            "Relationship managerId must have a type Line 5 column 3, to line 6 column 1. "
        );
    }
}

/// Only a missing or `null` `type` crashes TS: any other value's `.name`
/// is just `undefined`, which `RelationshipDeclaration.validate` rejects
/// later ("Relationship must have a type"), so `process` still accepts
/// it with no type, as before. `ObjectProperty` has TS's own guard.
#[test]
fn process_keeps_other_typeless_relationships_and_object_properties() {
    for ty in [
        serde_json::json!({}),
        serde_json::json!("x"),
        serde_json::json!(0),
    ] {
        let processed = process::<Error>(&relationship("home", Some(ty))).unwrap();
        assert_eq!(processed.property_type, None);
        assert!(processed.type_set);
    }
    let processed = process::<Error>(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ObjectProperty",
        "name": "address",
        "type": null
    }))
    .unwrap();
    assert_eq!(processed.property_type, None);
}

/// TS checks the name before its `$class` switch, so an invalid name is
/// still what a typeless relationship reports first.
#[test]
fn process_reports_an_invalid_name_before_a_missing_relationship_type() {
    let err = contract(process::<Error>(&relationship("1bad", None)).unwrap_err());
    assert_eq!(err.code, "property-process-invalidname");
}

/// P5-61: a property node the typed read cannot read — no `$class`, an
/// unknown one, a field of the wrong type, a `RelationshipProperty`
/// with no `type` (DV-017's shape), a `null` decorator (DV-018's) — is a
/// `modelfile-load-unreadable` `IllegalModelException`. BC-19's shape
/// check rejects each of them first; with it off, only the class is
/// promised.
#[test]
fn a_malformed_property_node_is_an_unreadable_ast() {
    for ast in [
        serde_json::json!({ "name": "x" }),
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.MysteryProperty", "name": "x" }),
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "s", "isArray": "yes" }),
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringProperty", "isArray": false, "isOptional": false }),
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "s", "isArray": false,
                "isOptional": false, "decorators": [null] }),
        relationship("dept", None),
        relationship("dept", Some(Value::Null)),
    ] {
        let err = contract(Property::try_from(&ast).unwrap_err());
        assert_eq!(err.kind, ErrorKind::IllegalModel, "{ast}");
        assert_eq!(err.code, "modelfile-load-unreadable", "{ast}");
    }
}

fn prop(json: serde_json::Value) -> Property {
    Property::try_from(&json).expect("valid property")
}

#[test]
fn parses_string_property_with_validators() {
    let p = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "email",
        "isArray": false,
        "isOptional": true,
        "validator": {
            "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
            "pattern": ".*@.*",
            "flags": ""
        }
    }));
    assert_eq!(p.name(), "email");
    assert!(p.is_optional());
    assert!(!p.is_array());
    assert!(p.is_primitive());
    assert_eq!(p.type_name(), Some("String"));
    match &p {
        Property::String(s) => assert!(s.validator.is_some()),
        _ => panic!("expected String"),
    }
}

#[test]
fn parses_object_and_relationship_type_refs() {
    let o = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ObjectProperty",
        "name": "address",
        "isArray": false,
        "isOptional": false,
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Address" }
    }));
    assert!(!o.is_primitive());
    assert_eq!(o.type_name(), Some("Address"));
    assert_eq!(
        o.type_identifier().map(|t| t.name.as_str()),
        Some("Address")
    );

    let r = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
        "name": "owner",
        "isArray": true,
        "isOptional": false,
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" }
    }));
    assert!(r.is_relationship());
    assert!(r.is_array());
    assert_eq!(r.type_name(), Some("Person"));
}

#[test]
fn enum_member_has_no_type() {
    let e = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.EnumProperty",
        "name": "RED"
    }));
    assert!(e.is_enum_value());
    assert_eq!(e.type_name(), None);
    assert!(!e.is_array());
    assert!(!e.is_optional());
}

#[test]
fn unknown_property_kind_errors() {
    let err = Property::try_from(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.MysteryProperty",
        "name": "x"
    }));
    assert!(err.is_err());
}

#[test]
fn missing_class_is_rejected() {
    let err = Property::try_from(&serde_json::json!({ "name": "x" }));
    assert!(err.unwrap_err().to_string().contains("$class"));
}

/// A `Double` property carrying the given range validator.
fn ranged(lower: Option<f64>, upper: Option<f64>) -> serde_json::Value {
    let mut validator = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.DoubleDomainValidator"
    });
    if let Some(lower) = lower {
        validator["lower"] = lower.into();
    }
    if let Some(upper) = upper {
        validator["upper"] = upper.into();
    }
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.DoubleProperty",
        "name": "value", "isArray": false, "isOptional": false,
        "validator": validator
    })
}

/// A `String` property carrying the given length validator.
fn sized(min: Option<i32>, max: Option<i32>) -> serde_json::Value {
    let mut validator = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringLengthValidator"
    });
    if let Some(min) = min {
        validator["minLength"] = min.into();
    }
    if let Some(max) = max {
        validator["maxLength"] = max.into();
    }
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "text", "isArray": false, "isOptional": false,
        "lengthValidator": validator
    })
}

#[test]
fn a_property_name_must_be_an_identifier() {
    let err = Property::try_from(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "1bad", "isArray": false, "isOptional": false
    }));
    assert!(
        err.unwrap_err()
            .to_string()
            .contains("Invalid property name '1bad'")
    );
}

/// A `String` property carrying the given regex validator.
fn matching(pattern: &str) -> serde_json::Value {
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "text", "isArray": false, "isOptional": false,
        "validator": {
            "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
            "pattern": pattern, "flags": ""
        }
    })
}

#[test]
fn a_regex_validator_must_compile() {
    assert!(Property::try_from(&matching(r"^.+@.+\..+$")).is_ok());
    for pattern in ["*invalid", "[unclosed", "(unclosed"] {
        let p = Property::try_from(&matching(pattern)).expect("construction accepts it");
        let err = p.check_bound_validators("test@1.0.0.Box");
        assert!(
            err.unwrap_err().to_string().contains("regular expression"),
            "{pattern} should be rejected"
        );
    }
}

#[test]
fn range_lower_above_upper_is_rejected() {
    let p = Property::try_from(&ranged(Some(10.0), Some(5.0))).expect("construction accepts it");
    let err = p.check_bound_validators("test@1.0.0.Box");
    assert!(err.unwrap_err().to_string().contains("Lower bound"));
}

#[test]
fn range_with_one_open_end_is_accepted() {
    assert!(Property::try_from(&ranged(Some(1.0), None)).is_ok());
    assert!(Property::try_from(&ranged(None, Some(1.0))).is_ok());
    assert!(Property::try_from(&ranged(Some(1.0), Some(10.0))).is_ok());
}

#[test]
fn range_without_either_bound_is_rejected() {
    let p = Property::try_from(&ranged(None, None)).expect("construction accepts it");
    let err = p.check_bound_validators("test@1.0.0.Box");
    assert!(err.unwrap_err().to_string().contains("lower and-or upper"));
}

/// OD-3: an Integer domain bound that overflows `i32` loads and
/// validates, matching TS (which reads it as a plain JS number).
///
/// Checked against the frozen TS 5.0.0 reference (`migration/oracle/reference`
/// in the `/home/user/concerto` workspace): `ModelManager.fromAst` loading
/// the same `IntegerDomainValidator` AST returns a `NumberValidator` whose
/// `upperBound` is `2147483648`, matching `upper` here.
#[test]
fn integer_domain_bound_above_i32_max_loads() {
    let p = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.IntegerProperty",
        "name": "value", "isArray": false, "isOptional": false,
        "validator": {
            "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
            "lower": 0,
            "upper": (i32::MAX as i64) + 1
        }
    }));
    match &p {
        Property::Integer(i) => {
            assert_eq!(
                i.validator.as_ref().unwrap().upper,
                Some((i32::MAX as f64) + 1.0)
            );
        }
        _ => panic!("expected Integer"),
    }
}

/// OD-3: a Long domain bound above `i64::MAX` loads, as JS rounds it to
/// the nearest f64 and TS accepts it.
///
/// Checked against the frozen TS 5.0.0 reference (`migration/oracle/reference`
/// in the `/home/user/concerto` workspace): `ModelManager.fromAst` loading
/// the same `LongDomainValidator` AST returns a `NumberValidator` whose
/// `upperBound` is `10000000000000000000` (`1e19`), matching `upper` here.
#[test]
fn long_domain_bound_above_i64_max_loads() {
    let p = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.LongProperty",
        "name": "value", "isArray": false, "isOptional": false,
        "validator": {
            "$class": "concerto.metamodel@1.0.0.LongDomainValidator",
            "lower": 0,
            "upper": 1e19
        }
    }));
    match &p {
        Property::Long(l) => {
            assert_eq!(l.validator.as_ref().unwrap().upper, Some(1e19));
        }
        _ => panic!("expected Long"),
    }
}

#[test]
fn negative_string_length_is_rejected() {
    let p = Property::try_from(&sized(Some(-1), Some(5))).expect("construction accepts it");
    let err = p.check_bound_validators("test@1.0.0.Box");
    assert!(err.unwrap_err().to_string().contains("positive integers"));
}

#[test]
fn string_length_min_above_max_is_rejected() {
    let p = Property::try_from(&sized(Some(10), Some(5))).expect("construction accepts it");
    let err = p.check_bound_validators("test@1.0.0.Box");
    assert!(err.unwrap_err().to_string().contains("minLength"));
}

#[test]
fn string_length_within_bounds_is_accepted() {
    assert!(Property::try_from(&sized(Some(1), Some(5))).is_ok());
    assert!(Property::try_from(&sized(None, Some(5))).is_ok());
}

/// A `String[]` property with a collection size validator.
fn collection_sized(is_array: bool, min: Option<i32>, max: Option<i32>) -> serde_json::Value {
    let mut validator = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator"
    });
    if let Some(min) = min {
        validator["minSize"] = min.into();
    }
    if let Some(max) = max {
        validator["maxSize"] = max.into();
    }
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "tags", "isArray": is_array, "isOptional": false,
        "sizeValidator": validator
    })
}

#[test]
fn size_validator_on_array_is_accepted() {
    assert!(Property::try_from(&collection_sized(true, Some(1), Some(10))).is_ok());
    assert!(Property::try_from(&collection_sized(true, Some(2), None)).is_ok());
    assert!(Property::try_from(&collection_sized(true, None, Some(5))).is_ok());
}

/// TS's `Property` constructor accepts a size validator on a non-array
/// property; only `Property.validate` rejects it (property.ts), which
/// `crate::validation`'s tests cover. (P2-08 review: this test used to
/// assert that construction itself failed.)
#[test]
fn size_validator_on_non_array_is_accepted_at_construction() {
    let p = Property::try_from(&collection_sized(false, Some(1), Some(5)))
        .expect("construction accepts a size validator on a non-array property");
    assert!(p.size_validator().is_some());
}

#[test]
fn size_validator_min_above_max_is_rejected() {
    let p = Property::try_from(&collection_sized(true, Some(10), Some(2)))
        .expect("construction accepts it");
    let err = p.check_bound_validators("test@1.0.0.Box");
    assert!(
        err.unwrap_err()
            .to_string()
            .contains("minSize must be less than or equal to maxSize")
    );
}

#[test]
fn size_validator_negative_bounds_rejected() {
    let p = Property::try_from(&collection_sized(true, Some(-1), Some(5)))
        .expect("construction accepts it");
    let err = p.check_bound_validators("test@1.0.0.Box");
    assert!(err.unwrap_err().to_string().contains("positive integers"));
}

#[test]
fn size_validator_on_object_property_without_array_is_allowed() {
    let json = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ObjectProperty",
        "name": "contacts",
        "isArray": false,
        "isOptional": false,
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "PhoneBook" },
        "sizeValidator": {
            "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
            "minSize": 1,
            "maxSize": 5
        }
    });
    assert!(Property::try_from(&json).is_ok());
}

#[test]
fn size_validator_on_relationship_array_is_accepted() {
    let json = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
        "name": "advisors",
        "isArray": true,
        "isOptional": false,
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
        "sizeValidator": {
            "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
            "minSize": 1,
            "maxSize": 3
        }
    });
    let p = Property::try_from(&json).unwrap();
    assert!(p.size_validator().is_some());
    assert_eq!(p.size_validator().unwrap().min_size, Some(1.0));
    assert_eq!(p.size_validator().unwrap().max_size, Some(3.0));
}

/// Construction accepts it (TS `Property` constructor); validation
/// rejects it (`crate::validation`'s tests). P2-08 review: this test
/// used to assert construction-time rejection.
#[test]
fn size_validator_on_non_array_relationship_is_accepted_at_construction() {
    let json = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
        "name": "owner",
        "isArray": false,
        "isOptional": false,
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
        "sizeValidator": {
            "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
            "minSize": 1
        }
    });
    let p = Property::try_from(&json)
        .expect("construction accepts a size validator on a non-array relationship");
    assert!(p.size_validator().is_some());
}

#[test]
fn only_the_full_metamodel_property_classes_are_recognised() {
    // TS `ClassDeclaration.process` matches each property's `$class`
    // with `===` against the full metamodel classes
    // (accordproject/concerto-rust#285, BC-25); a short name, another
    // namespace's, or text that merely ends in a property class's short
    // name is not a property the typed read can read (P5-61: a
    // `modelfile-load-unreadable` `IllegalModelException`; BC-19's shape
    // check rejects it first).
    for class in [
        "StringProperty",
        "foo.StringProperty",
        "concerto.metamodel@1.0.0.StringPropertyconcerto.metamodel@1.0.0.StringProperty",
        "concerto.metamodel@1.0.0.EnumPropertyconcerto.metamodel@1.0.0.EnumProperty",
        "concerto.metamodel@1.0.0.RelationshipPropertyconcerto.metamodel@1.0.0.RelationshipProperty",
        "concerto.metamodel@2.0.0.StringProperty",
        "concerto.metamodel@1.0.0.Foo.StringProperty",
    ] {
        let err = Property::try_from(&serde_json::json!({
            "$class": class,
            "name": "email",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "T" }
        }))
        .unwrap_err();
        assert!(
            matches!(
                Some(err.contract()),
                Some(c) if c.kind == ErrorKind::IllegalModel && c.code == "modelfile-load-unreadable"
            ),
            "{class}: {err}"
        );
    }
    for kind in PROPERTY_KINDS {
        assert_eq!(
            property_kind(&format!("concerto.metamodel@1.0.0.{kind}")),
            Some(kind)
        );
    }
}

#[test]
fn process_matches_the_full_property_class() {
    // TS `Property.process`'s `switch (this.ast.$class)` has no arm for
    // a class that only ends in a property class's short name, so
    // `this.type` is left unassigned, and a `RelationshipProperty`
    // short name with no `type` does not reach its unguarded arm.
    for class in ["StringProperty", "foo.StringProperty"] {
        let processed = process::<ContractError>(&serde_json::json!({
            "$class": class, "name": "s"
        }))
        .unwrap();
        assert_eq!(processed.property_type, None, "{class}");
        assert!(!processed.type_set, "{class}");
    }
    assert!(
        process::<ContractError>(&serde_json::json!({
            "$class": "RelationshipProperty", "name": "r"
        }))
        .is_ok()
    );
}

/// P2-04 (plan §1.2's "enum ... reserved values" gap; issue #48): an
/// enum value may not take a reserved (system) property name either,
/// the same check every other property kind gets above.
///
/// Checked against the frozen TS 5.0.0 reference
/// (`migration/oracle/reference`): `ModelManager.addCTOModel` on
///
/// ```cto
/// namespace org.acme.enumreserved@1.0.0
/// enum Status {
///   o $identifier
/// }
/// ```
///
/// raises `IllegalModelException: Invalid field name '$identifier'`,
/// matching this test verbatim.
#[test]
fn a_reserved_name_is_rejected_on_an_enum_value() {
    let err = Property::try_from(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.EnumProperty",
        "name": "$identifier"
    }));
    assert_eq!(
        err.unwrap_err().to_string(),
        "Invalid field name '$identifier'"
    );
}

/// Ported from `test/introspect/property.js` #getSizeValidator "should
/// return null when no size validator".
#[test]
fn size_validator_is_none_when_absent() {
    let p = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "tags", "isArray": true, "isOptional": false
    }));
    assert!(p.size_validator().is_none());
}

/// Ported from `test/introspect/field.js` #constructor "should not have a
/// default value by default" and "should save the incoming default
/// value". TS builds a `Field` over a stubbed `ClassDeclaration` parent
/// for these two, but `process()` never calls it (`this.ast.defaultValue`
/// only), so the stub is inert scaffolding, not white-box coupling
/// (module doc on [`crate::model_manager::ModelManager::property_default_value`],
/// which is the same raw-AST read for a `PropId` already in the arena);
/// `Property::try_from` alone is the faithful port here, no `ModelManager`
/// or parent needed.
#[test]
fn a_default_value_is_read_from_the_ast_when_present() {
    let p = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "field", "isArray": false, "isOptional": false,
        "defaultValue": "wowSuchDefault"
    }));
    match &p {
        Property::String(s) => {
            assert_eq!(s.default_value.as_deref(), Some("wowSuchDefault"));
        }
        _ => panic!("expected String"),
    }

    let without = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "field", "isArray": false, "isOptional": false
    }));
    match &without {
        Property::String(s) => assert_eq!(s.default_value, None),
        _ => panic!("expected String"),
    }
}

/// Ported from `test/introspect/field.js` #getDefaultValue "should return
/// the default value for falsy defaults": a JSON `false` default is not
/// itself nullish, so it is kept (`Util.isNull` in TS, `!v.is_null()` in
/// [`crate::model_manager::ModelManager::property_default_value`]),
/// unlike a JSON `null`.
#[test]
fn a_falsy_boolean_default_value_is_not_treated_as_absent() {
    let p = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.BooleanProperty",
        "name": "field", "isArray": false, "isOptional": false,
        "defaultValue": false
    }));
    match &p {
        Property::Boolean(b) => assert_eq!(b.default_value, Some(false)),
        _ => panic!("expected Boolean"),
    }
}

/// Ported from `test/introspect/field.js` #constructor "should not be
/// optional by default" and "should detect if field is optional".
#[test]
fn optional_defaults_to_false_and_follows_the_ast() {
    let not_optional = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "field", "isArray": false
    }));
    assert!(!not_optional.is_optional());

    let optional = prop(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "field", "isArray": false, "isOptional": true
    }));
    assert!(optional.is_optional());
}

/// P5-61 (BR-09, accordproject/concerto-rust#393): a property's
/// `sizeValidator`, `lengthValidator` and `validator` are decoded as
/// strictly as its other fields. TS 5.0.0 read them with no type check
/// (a non-numeric bound, a non-string `$class` or pattern, a validator
/// that is not an object, #217), and so did the port until P5-61. BC-19's
/// shape check rejects each of these first; with the check off they are
/// the loader's error.
#[test]
fn a_malformed_validator_is_an_unreadable_ast() {
    let string = |key: &str, value: serde_json::Value| {
        let mut ast = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "s", "isArray": true, "isOptional": false
        });
        ast[key] = value;
        ast
    };
    for ast in [
        string(
            "sizeValidator",
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": "NaN", "maxSize": 5
            }),
        ),
        string(
            "sizeValidator",
            serde_json::json!({
                "$class": ["concerto.metamodel@1.0.0.CollectionSizeValidator"], "minSize": 1, "maxSize": 5
            }),
        ),
        string("sizeValidator", serde_json::json!({ "minSize": 1 })),
        string("sizeValidator", serde_json::json!(true)),
        string(
            "lengthValidator",
            serde_json::json!([{
                "$class": "concerto.metamodel@1.0.0.StringLengthValidator", "minLength": null, "maxLength": 10
            }]),
        ),
        string("lengthValidator", serde_json::json!({})),
        string(
            "validator",
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": 5, "flags": ""
            }),
        ),
        string(
            "validator",
            serde_json::json!({ "pattern": "a", "flags": "" }),
        ),
        string("validator", serde_json::json!(0)),
        string(
            "validator",
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "a", "flags": "", "extra": 1
            }),
        ),
    ] {
        let err = contract(Property::try_from(&ast).unwrap_err());
        assert_eq!(err.kind, ErrorKind::IllegalModel, "{ast}");
        assert_eq!(err.code, "modelfile-load-unreadable", "{ast}");
    }
    let p = prop(string("validator", serde_json::Value::Null));
    match &p {
        Property::String(s) => assert!(s.validator.is_none()),
        _ => panic!("expected String"),
    }
}

/// A well-formed `lengthValidator` whose bounds are both explicitly
/// `null` loads, and trips the "must be specified" check, in both
/// engines.
#[test]
fn string_property_with_both_length_bounds_explicitly_null_is_rejected() {
    let err = Property::try_from(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "s", "isArray": false, "isOptional": false,
        "lengthValidator": {
            "$class": "concerto.metamodel@1.0.0.StringLengthValidator",
            "minLength": null, "maxLength": null
        }
    }))
    .expect("try_from itself does not build the validator")
    .check_bound_validators("ns.C")
    .unwrap_err();
    assert!(err.to_string().contains("must be specified"));
}

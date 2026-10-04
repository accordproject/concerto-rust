    use serde_json::json;

    use super::*;

    fn manager() -> ModelManager {
        let mut mm = ModelManager::new().unwrap();
        mm.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [],
                "declarations": [
                    {
                        "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                        "name": "Car",
                        "isAbstract": false,
                        "identified": {
                            "$class": "concerto.metamodel@1.0.0.IdentifiedBy",
                            "name": "vin"
                        },
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "vin", "isArray": false, "isOptional": false },
                            { "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "built", "isArray": false, "isOptional": true },
                            { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "doors", "isArray": false, "isOptional": false, "defaultValue": 4 },
                            {
                                "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
                                "name": "owner",
                                "isArray": false,
                                "isOptional": true,
                                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Car" }
                            },
                            {
                                "$class": "concerto.metamodel@1.0.0.ObjectProperty",
                                "name": "fleet",
                                "isArray": false,
                                "isOptional": true,
                                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "CarMap" }
                            }
                        ]
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.MapDeclaration",
                        "name": "CarMap",
                        "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                        "value": {
                            "$class": "concerto.metamodel@1.0.0.RelationshipMapValueType",
                            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Car" }
                        }
                    }
                ]
            }),
            None,
        )
        .unwrap();
        mm
    }

    fn check(json: Value) -> Result<Value> {
        from_json(&manager(), &json, &FromJsonOptions::default(), &mut FixedEnv)
    }

    #[test]
    fn populates_a_datetime_a_relationship_and_a_default() {
        let value = check(json!({
            "$class": "org.acme@1.0.0.Car",
            "vin": "V1",
            "built": "2024-01-02T03:04:05Z",
            "owner": "resource:org.acme@1.0.0.Car#V2"
        }))
        .unwrap();
        assert_eq!(value["$class"], "org.acme@1.0.0.Car");
        assert_eq!(value["vin"], "V1");
        assert_eq!(value["doors"], 4);
        assert_eq!(value["built"][validate::DAYJS_TAG], "2024-01-02T03:04:05.000Z");
        assert_eq!(value["owner"][validate::RELATIONSHIP_TAG], true);
        assert_eq!(value["owner"]["vin"], "V2");
    }

    /// BC-05, DV-007: a relationship-typed map value is read as a
    /// relationship property is: a URI or bare identifier becomes a
    /// relationship; an embedded resource needs
    /// `acceptResourcesForRelationships`, and then fails validation unless
    /// the validator's own options permit it.
    #[test]
    fn a_relationship_map_value_is_read_as_a_relationship_property() {
        let value = check(json!({
            "$class": "org.acme@1.0.0.Car",
            "vin": "V1",
            "fleet": { "a": "resource:org.acme@1.0.0.Car#V2", "b": "V3" }
        }))
        .unwrap();
        let entries = value["fleet"][validate::MAP_TAG].as_array().unwrap();
        for (entry, vin) in entries.iter().zip(["V2", "V3"]) {
            assert_eq!(entry[1][validate::RELATIONSHIP_TAG], true, "{entry}");
            assert_eq!(entry[1]["$class"], "org.acme@1.0.0.Car");
            assert_eq!(entry[1]["vin"], vin);
        }

        let embedded = json!({
            "$class": "org.acme@1.0.0.Car",
            "vin": "V1",
            "fleet": { "a": { "$class": "org.acme@1.0.0.Car", "vin": "V4" } }
        });
        let err = check(embedded.clone()).unwrap_err();
        assert_eq!(err.code(), "jsonpopulator-visitrelationshipdeclaration-notastring");
        let accept = FromJsonOptions {
            accept_resources_for_relationships: true,
            ..FromJsonOptions::default()
        };
        let err = from_json(&manager(), &embedded, &accept, &mut FixedEnv).unwrap_err();
        assert_eq!(err.code(), "resourcevalidator-notrelationship");
        let permitted = FromJsonOptions {
            validator: ValidateOptions {
                permit_resources_for_relationships: true,
                ..ValidateOptions::default()
            },
            ..accept.clone()
        };
        let value = from_json(&manager(), &embedded, &permitted, &mut FixedEnv).unwrap();
        let entry = &value["fleet"][validate::MAP_TAG][0][1];
        assert_eq!(entry["$class"], "org.acme@1.0.0.Car");
        assert!(entry.get(validate::RELATIONSHIP_TAG).is_none(), "{entry}");
        let unvalidated = FromJsonOptions {
            validate: false,
            ..accept
        };
        from_json(&manager(), &embedded, &unvalidated, &mut FixedEnv).unwrap();
    }

    /// A model with a `DateTime` default on a property and on a scalar.
    fn date_time_default_manager(property: Value, scalar: Value) -> ModelManager {
        let mut mm = ModelManager::new().unwrap();
        mm.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.dates@1.0.0",
                "imports": [],
                "declarations": [
                    {
                        "$class": "concerto.metamodel@1.0.0.DateTimeScalar",
                        "name": "When",
                        "defaultValue": scalar
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                        "name": "P",
                        "isAbstract": false,
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "n", "isArray": false, "isOptional": false, "defaultValue": 1 },
                            { "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "at", "isArray": false, "isOptional": true, "defaultValue": property }
                        ]
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                        "name": "S",
                        "isAbstract": false,
                        "properties": [
                            {
                                "$class": "concerto.metamodel@1.0.0.ObjectProperty",
                                "name": "when",
                                "isArray": false,
                                "isOptional": true,
                                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "When" }
                            }
                        ]
                    }
                ]
            }),
            None,
        )
        .expect("a lenient DateTime default does not fail model load (BC-45 is lazy)");
        mm
    }

    /// BC-45: a `DateTime` default, on a property or a scalar, must be a
    /// strict `DateTime` string. The model loads whatever the default is; a
    /// bad one throws a `ValidationException` when population applies it
    /// (the document gives the field no value, or `null`), and not when the
    /// document gives the field its own value.
    #[test]
    fn a_date_time_default_is_checked_when_it_is_applied() {
        let populate = |mm: &ModelManager, class: &str| {
            from_json(
                mm,
                &json!({ "$class": format!("org.dates@1.0.0.{class}") }),
                &FromJsonOptions::default(),
                &mut FixedEnv,
            )
        };
        for ok in [json!("2022-11-18T00:00:00Z"), json!("2022-11-18T01:02:03.5+01:00")] {
            let mm = date_time_default_manager(ok.clone(), ok);
            let p = populate(&mm, "P").expect("a strict property default");
            assert!(p["at"][validate::DAYJS_TAG].is_string(), "{p}");
            populate(&mm, "S").expect("a strict scalar default");
        }
        for (bad, shown) in [
            (json!("2022-11-18"), "2022-11-18"),
            (json!("2008-09-15T15:53:00"), "2008-09-15T15:53:00"),
            (json!(""), ""),
            (json!("FOO"), "FOO"),
            (json!("2024-02-30T00:00:00Z"), "2024-02-30T00:00:00Z"),
            (json!("2024-01-02T24:00:00Z"), "2024-01-02T24:00:00Z"),
            (json!(1), "1"),
        ] {
            // A non-string scalar default does not load at all: the typed
            // `DateTimeScalar` holds a string. Keep the scalar strict then.
            let scalar = if bad.is_string() {
                bad.clone()
            } else {
                json!("2022-11-18T00:00:00Z")
            };
            let mm = date_time_default_manager(bad.clone(), scalar);
            let err = populate(&mm, "P").unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Validation, "{err}");
            assert_eq!(err.code(), "typed-assignfielddefaults-datetime");
            assert!(
                err.to_string()
                    .contains(&format!("`{shown}` for the DateTime field `org.dates@1.0.0.P.at`")),
                "{err}"
            );
            let err = from_json(
                &mm,
                &json!({ "$class": "org.dates@1.0.0.P", "at": null }),
                &FromJsonOptions::default(),
                &mut FixedEnv,
            )
            .unwrap_err();
            assert_eq!(err.code(), "typed-assignfielddefaults-datetime");
            let given = from_json(
                &mm,
                &json!({ "$class": "org.dates@1.0.0.P", "at": "2020-01-01T00:00:00Z" }),
                &FromJsonOptions::default(),
                &mut FixedEnv,
            )
            .expect("the document's own value replaces the default");
            assert_eq!(
                given.as_object().unwrap().keys().collect::<Vec<_>>(),
                ["$class", "$identifier", "$timestamp", "n", "at"],
                "the given value keeps the default's place"
            );
            assert_eq!(given["at"][validate::DAYJS_TAG], "2020-01-01T00:00:00.000Z");
            if bad.is_string() {
                let err = populate(&mm, "S").unwrap_err();
                assert_eq!(err.kind(), ErrorKind::Validation, "{err}");
                assert!(
                    err.to_string()
                        .contains(&format!("`{shown}` for the DateTime field `org.dates@1.0.0.S.when`")),
                    "{err}"
                );
            }
        }
    }

    #[test]
    fn rejects_what_the_populator_rejects() {
        let err = check(json!({"$class": "org.acme@1.0.0.Car", "vin": "V1", "doors": "4"}))
            .unwrap_err();
        assert_eq!(err.code(), "jsonpopulator-converttoobject-wrongtype");
        let err = check(json!({"$class": "org.acme@1.0.0.Car", "vin": "V1", "built": 5}))
            .unwrap_err();
        assert_eq!(err.code(), "jsonpopulator-converttoobject-wrongtype");
        let err = check(json!({"$class": "org.acme@1.0.0.Car", "vin": " "})).unwrap_err();
        assert_eq!(err.code(), "factory-newinstance-missingidentifier");
        let err = check(json!({"vin": "V1"})).unwrap_err();
        assert_eq!(err.code(), "serializer-fromjson-noclass");
    }

    #[test]
    fn the_strict_options_reject_unknown_keys_and_required_nulls() {
        let strict = FromJsonOptions {
            reject_unknown_keys: true,
            reject_required_null: true,
            ..FromJsonOptions::default()
        };
        let mm = manager();
        let err = from_json(
            &mm,
            &json!({"$class": "org.acme@1.0.0.Car", "vin": "V1", "extra": null}),
            &strict,
            &mut FixedEnv,
        )
        .unwrap_err();
        assert_eq!(err.details()[0].code, DetailCode::UnknownProperty);
        assert_eq!(err.details()[0].path, "$.extra");
        let err = from_json(
            &mm,
            &json!({"$class": "org.acme@1.0.0.Car", "vin": "V1", "doors": null}),
            &strict,
            &mut FixedEnv,
        )
        .unwrap_err();
        assert_eq!(err.details()[0].code, DetailCode::TypeViolation);
        assert_eq!(err.details()[0].path, "$.doors");
    }

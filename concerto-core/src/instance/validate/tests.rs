    //! Exercises the confirmed `concerto-validate-rs` bug fixes (module doc),
    //! plus full type support: Long, DateTime, relationships, enums, maps and
    //! scalars, matching `ResourceValidator`'s checks and messages
    //! (`resourcevalidator.ts`, verified against its golden tests in
    //! `error/mod.rs`).

    use super::*;
    use crate::instance::{Diagnostic, DiagnosticCode, ValidationReport};
    use crate::model_manager::ModelManager;
    use serde_json::json;

    /// One model exercising every kind this validator supports: multi-level
    /// inheritance (`Base` -> `Mid` -> `Leaf`), an
    /// abstract concept (`Animal`), enums, relationships, maps
    /// (`String`, `DateTime`, `Boolean`, enum and object/relationship
    /// values), a regex-validated scalar, and Integer/Long/DateTime/String
    /// fields with validators.
    fn fixture() -> ModelManager {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Animal",
                      "isAbstract": true,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Dog",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Animal" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "breed", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Base",
                      "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "a", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Mid",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "b", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Leaf",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Mid" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "c", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "Color",
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" },
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "GREEN" },
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "BLUE" }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Vehicle",
                      "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "vin" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "vin", "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.LongProperty", "name": "mileage", "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "purchasedAt", "isArray": false, "isOptional": true },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "color", "isArray": false, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Color" } },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "pet", "isArray": false, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Animal" } },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "tags", "isArray": true, "isOptional": true,
                          "sizeValidator": { "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 1, "maxSize": 3 } },
                        { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "rating", "isArray": false, "isOptional": true,
                          "validator": { "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator", "lower": 0, "upper": 5 } },
                        // Array-of-enum (`check_enum`'s own
                        // `property.is_array()` guard, at the top of the
                        // function, had no test with an *array* enum
                        // property anywhere in this fixture — every
                        // existing enum test uses `color`, which is not an
                        // array).
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "colors", "isArray": true, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Color" } },
                        // A String field with its own (non-scalar)
                        // `validator`, no `length_validator` alongside it
                        // (`check_primitive_item`'s `sp.validator
                        // .is_some() || sp.length_validator.is_some()`
                        // guard — every existing `String` field, including
                        // `vin`, carries neither, and the only other
                        // `StringValidator` exercise in this module is via
                        // a scalar, `VIN`, which runs through
                        // `check_scalar_item`, not `check_primitive_item`,
                        // so a validator on the *property* itself, alone,
                        // was never reached).
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "code", "isArray": false, "isOptional": true,
                          "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "^[A-Z]+$", "flags": "" } },
                        // No `Double` property existed anywhere in this
                        // fixture (cargo-mutants found
                        // `check_primitive_item`'s `Property::Double(dp)`
                        // match arm survived — deleting it falls through to
                        // the trailing `_ => unreachable!()`, which nothing
                        // exercised).
                        { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "weight", "isArray": false, "isOptional": true }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Owner",
                      "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "ownerId" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "ownerId", "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "vehicle", "isArray": false, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Vehicle" } },
                        { "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "vehicles", "isArray": true, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Vehicle" } }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "VIN",
                      "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "^[A-Z0-9]{5}$", "flags": "" } },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Item",
                      "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Garage",
                      "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "vinField", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "VIN" } },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "items", "isArray": true, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Item" },
                          "sizeValidator": { "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 1, "maxSize": 2 } }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "StringMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "ColorMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Color" } } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "ItemMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Item" } } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "VehicleMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.RelationshipMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Vehicle" } } },
                    // A scalar-typed map KEY (`map_key_is_scalar` had
                    // no fixture where the map key itself is an
                    // `ObjectMapKeyType` referencing a scalar — every other
                    // map here keys on a plain `StringMapKeyType`, so
                    // `map_key_is_scalar` always took its early `key_kind()
                    // != "ObjectMapKeyType"` return and its real
                    // scalar-resolution logic was never reached).
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "ScalarKeyMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.ObjectMapKeyType",
                               "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "VIN" } },
                      "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "VIN" } } },
                    // A scalar-typed map VALUE whose KEY is *not* scalar
                    // (paired with `ScalarKeyMap` above): `checkMapType`
                    // reads `ModelUtil.isScalar(mapDeclaration.getKey())`
                    // unconditionally, even while validating the value slot
                    // (module doc, `map_key_is_scalar`'s own doc) — so this
                    // map's value type is never type-checked at all, by
                    // design (a faithful port of that quirk, not a bug).
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "PlainKeyScalarValueMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "VIN" } } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "DateTimeMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.DateTimeMapValueType" } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "BooleanMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.BooleanMapValueType" } }
                ]
            }),
            None,
        )
        .unwrap();
        mgr
    }

    fn err_of(result: Result<()>) -> Error {
        result.expect_err("expected a validation failure")
    }

    // ---- Every super type's properties are merged, not just the direct one's ----

    #[test]
    fn a_three_level_inherited_field_is_recognised() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "a": "1", "b": "2", "c": "3" });
        validate_instance(&mgr, &leaf, &ValidateOptions::default())
            .expect("all three levels' fields are known");
    }

    #[test]
    fn a_missing_field_from_the_grandparent_type_is_still_caught() {
        let mgr = fixture();
        // `a` (declared on `Base`, two levels up from `Leaf`) is missing.
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3" });
        let err = err_of(validate_instance(&mgr, &leaf, &ValidateOptions::default()));
        assert!(err.to_string().contains("\"a\""), "{err}");
    }

    // ---- Abstract and nested $class values are checked ----

    #[test]
    fn an_abstract_class_at_the_root_is_rejected() {
        let mgr = fixture();
        let animal = json!({ "$class": "org.acme@1.0.0.Animal", "name": "Rex" });
        let err = err_of(validate_instance(
            &mgr,
            &animal,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            err.to_string(),
            "The class \"org.acme@1.0.0.Animal\" is abstract and should not contain an instance."
        );
    }

    #[test]
    fn an_abstract_class_nested_inside_another_resource_is_also_rejected() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 100,
            // `pet`'s declared type is `Animal`; assigning `Animal` itself
            // (not a concrete subtype like `Dog`) must be rejected exactly
            // as it would be at the root.
            "pet": { "$class": "org.acme@1.0.0.Animal", "name": "Rex" }
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            err.to_string(),
            "The class \"org.acme@1.0.0.Animal\" is abstract and should not contain an instance."
        );
    }

    #[test]
    fn a_concrete_subtype_nested_inside_another_resource_passes() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 100,
            "pet": { "$class": "org.acme@1.0.0.Dog", "name": "Rex", "breed": "Lab" }
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    // ---- Undeclared / missing / empty identifier ----

    #[test]
    fn an_undeclared_field_is_rejected() {
        let mgr = fixture();
        let leaf =
            json!({ "$class": "org.acme@1.0.0.Leaf", "a": "1", "b": "2", "c": "3", "d": "nope" });
        let err = err_of(validate_instance(&mgr, &leaf, &ValidateOptions::default()));
        assert!(err.to_string().contains("\"d\""), "{err}");
        assert!(err.to_string().contains("org.acme@1.0.0.Leaf"), "{err}");
    }

    /// [`js_id_display`] (cargo-mutants found this return value was never
    /// asserted): `Leaf` above is not identified, so
    /// [`an_undeclared_field_is_rejected`] reaches `undeclared_field`
    /// through the *other* branch (`p.current_identifier`), never through
    /// `js_id_display`. `Vehicle` is identified (by `vin`), so an undeclared
    /// field on a `Vehicle` instance whose own `vin` is absent hits
    /// `js_id_display(None)`, which is the JS `${undefined}` literal
    /// `"undefined"` — never an empty string — as the reported resource id.
    #[test]
    fn an_undeclared_field_on_an_identified_resource_with_no_identifier_value_reports_undefined() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "mileage": 5, "extra": "nope" });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string().contains("\"undefined\""),
            "expected the undefined-identifier placeholder, got: {err}"
        );
        assert!(err.to_string().contains("\"extra\""), "{err}");
    }

    /// [`visit_class_declaration`]'s undeclared-field `resource_id`
    /// computation (cargo-mutants found the `&&`->`||` and `!=`->`==` mutants
    /// at this line both survived). `key != "$identifier"` is, on its own,
    /// always true at this point (an actual `"$identifier"` key is filtered
    /// out as a system property earlier in the same loop, so this branch
    /// never reaches it) — the `!=` mutant's `==` is thus always false there.
    /// Both mutants are made observable by giving the `&&`'s *other* operand
    /// (`declared_is_identified`) a true and a false case with genuinely
    /// different `resource_id` outputs: `inner` is nested inside an
    /// already-identified `Outer` (so `p.current_identifier` is
    /// `Some("Outer#O1")` by the time it is visited) but is itself declared
    /// as the *identified* `Inner`, so the real `js_id_display`-based id
    /// (`"I1"`) differs from the `&&`/`==` mutants' `current_identifier`
    /// fallback (`"Outer#O1"`); `loose` is declared as the *unidentified*
    /// `Loose` and visited after `inner` (properties are walked in
    /// declaration order), so by then the real fallback is
    /// `p.current_identifier` as `inner` itself left it, `"Inner#I1"` (every
    /// *identified* object visited overwrites it, `inner` included — not only
    /// `Outer`) — still a value the `||` mutant's wrongly-taken
    /// `js_id_display` branch cannot produce (`"undefined"`, since an
    /// unidentified type has no identifier field to read).
    #[test]
    fn an_undeclared_field_s_reported_resource_id_depends_on_whether_the_declared_type_is_identified()
     {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.nest@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Inner",
                      "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "iid" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "iid",
                          "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Loose",
                      "isAbstract": false, "properties": [] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Outer",
                      "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "oid" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "oid",
                          "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "inner",
                          "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Inner" } },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "loose",
                          "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Loose" } }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();

        let with_bad_inner = json!({
            "$class": "org.nest@1.0.0.Outer", "oid": "O1",
            "inner": { "$class": "org.nest@1.0.0.Inner", "iid": "I1", "bogus": "nope" },
            "loose": { "$class": "org.nest@1.0.0.Loose" }
        });
        let err = err_of(validate_instance(
            &mgr,
            &with_bad_inner,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("\"I1\""), "{err}");

        // `p.current_identifier` is set (and left set) by every *identified*
        // object the walk visits, `inner` included — since properties are
        // visited in declaration order (`oid`, `inner`, `loose`) and `inner`
        // is processed first and is itself identified, it is `"Inner#I1"`,
        // not `"Outer#O1"`, that is on `p.current_identifier` by the time
        // `loose` is reached below.
        let with_bad_loose = json!({
            "$class": "org.nest@1.0.0.Outer", "oid": "O1",
            "inner": { "$class": "org.nest@1.0.0.Inner", "iid": "I1" },
            "loose": { "$class": "org.nest@1.0.0.Loose", "bogus2": "nope" }
        });
        let err = err_of(validate_instance(
            &mgr,
            &with_bad_loose,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string().contains("\"org.nest@1.0.0.Inner#I1\""),
            "{err}"
        );
    }

    #[test]
    fn a_missing_required_property_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12" });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            err.to_string(),
            "The instance \"org.acme@1.0.0.Vehicle#ABC12\" is missing the required field \"mileage\"."
        );
    }

    #[test]
    fn an_empty_identifier_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "  ", "mileage": 1 });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("empty identifier"), "{err}");
    }

    // ---- Long, DateTime, String, Boolean primitives ----

    #[test]
    fn a_valid_long_and_datetime_pass() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 9_007_199_254_740_991_i64,
            "purchasedAt": { "$$dayjs": "2020-01-01T00:00:00.000Z" }
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// A `Dayjs` instance is still a `Dayjs` instance even when the string it
    /// was built from was nonsense (module doc "Scope", [`DAYJS_TAG`]'s
    /// doc): TS's `checkItem` never re-validates it, so this passes.
    #[test]
    fn a_populated_datetime_passes_even_when_its_own_string_is_nonsense() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "purchasedAt": { "$$dayjs": "not-a-date" }
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// A raw (un-coerced) string on a `DateTime` field is always a field
    /// type violation post-population (module doc "Scope"): TS's
    /// `JSONPopulator` is what turns a wire string into a `Dayjs`, and
    /// `ResourceValidator` only ever sees the result.
    #[test]
    fn an_uncoerced_string_on_a_datetime_field_is_a_field_type_violation() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "purchasedAt": "2020-01-01T00:00:00.000Z"
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("\"purchasedAt\""), "{err}");
    }

    #[test]
    fn a_string_value_for_a_long_field_is_a_field_type_violation() {
        let mgr = fixture();
        let vehicle =
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": "far" });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("\"mileage\""), "{err}");
        assert!(
            err.to_string().contains("Expected type of value: \"Long\""),
            "{err}"
        );
    }

    #[test]
    fn a_non_object_value_on_a_datetime_field_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "purchasedAt": "not-a-date"
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("\"purchasedAt\""), "{err}");
    }

    // ---- Enums ----

    #[test]
    fn a_valid_enum_value_passes() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "color": "RED" });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// TS passes the enum declaration as `reportInvalidEnumValue`'s
    /// `field`, so the message names the enum type (`Color`), not the
    /// property (`color`); the corpus records the same wording (for example
    /// `Invalid enum value of "Purple" for the field "Color".`).
    #[test]
    fn an_invalid_enum_value_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "color": "PURPLE" });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            err.to_string(),
            "Model violation in the \"org.acme@1.0.0.Vehicle#ABC12\" instance. Invalid enum value of \"PURPLE\" for the field \"Color\"."
        );
    }

    // ---- Relationships ----

    #[test]
    fn a_populated_relationship_to_an_identified_type_passes() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicle": { "$$relationship": true, "$class": "org.acme@1.0.0.Vehicle" }
        });
        validate_instance(&mgr, &owner, &ValidateOptions::default()).unwrap();
    }

    /// A raw wire URI string on a relationship field is a field type
    /// violation post-population, the same way an un-coerced `DateTime`
    /// string is (module doc "Scope"): `JSONPopulator.visitRelationshipDeclaration`
    /// is what turns a URI string into a `Relationship`, via
    /// `Relationship.fromURI`; `ResourceValidator` only ever sees the
    /// result.
    #[test]
    fn an_uncoerced_relationship_uri_string_is_rejected() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicle": "resource:org.acme@1.0.0.Vehicle#ABC12"
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        assert!(
            err.to_string().contains("Expected a \"Relationship\""),
            "{err}"
        );
    }

    #[test]
    fn a_plain_string_that_is_not_a_relationship_is_rejected() {
        let mgr = fixture();
        let owner =
            json!({ "$class": "org.acme@1.0.0.Owner", "ownerId": "O1", "vehicle": "not a uri" });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        assert!(
            err.to_string().contains("Expected a \"Relationship\""),
            "{err}"
        );
    }

    // ---- Maps: String/DateTime/Boolean values, and the enum/relationship
    //      fix. A Map is never itself a top-level `validate_instance` entry
    //      in TS (only a `Resource` is: module doc "Scope"), so these call
    //      the internal `visit_map_declaration` directly, exactly the way a
    //      `Vehicle`-typed field pointing at a `MapDeclaration` would reach
    //      it (`Kind::MapTyped`, `check_item`). ----

    fn validate_map(mgr: &ModelManager, map_fqn: &str, value: &Value) -> Result<()> {
        validate_map_with(mgr, map_fqn, value, ValidateOptions::default())
    }

    fn validate_map_with(
        mgr: &ModelManager,
        map_fqn: &str,
        value: &Value,
        options: ValidateOptions,
    ) -> Result<()> {
        let id = mgr.declaration_id(map_fqn).unwrap();
        let map_plan = plan::map_plan(mgr, id);
        let mut params = Params::new(mgr, &options, String::new(), Sink::Stop);
        visit_map_declaration(&mut params, id, &map_plan, value)
    }

    #[test]
    fn a_string_map_with_string_values_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("1")), (json!("b"), json!("2"))]);
        validate_map(&mgr, "org.acme@1.0.0.StringMap", &map).unwrap();
    }

    #[test]
    fn a_string_map_with_a_non_string_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(1))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.StringMap", &map));
        assert!(err.to_string().contains("Expected Type of String"), "{err}");
    }

    /// A `Map` key keeps its JS type, so a number key of a
    /// `String`-keyed map is reported (`found '1234'`).
    #[test]
    fn a_string_map_with_a_number_key_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!(1234), json!("Lorem"))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.StringMap", &map));
        assert!(err.to_string().contains("but found '1234'"), "{err}");
    }

    /// A key spelt like the `undefined` marker is an ordinary
    /// key, not a JS `undefined`.
    #[test]
    fn a_map_key_spelt_like_the_undefined_marker_is_an_ordinary_key() {
        let mgr = fixture();
        let map = js_map(vec![(json!(UNDEFINED_TAG), json!("x"))]);
        validate_map(&mgr, "org.acme@1.0.0.StringMap", &map).unwrap();
    }

    /// `obj instanceof Map` — a plain object is not a `Map`.
    #[test]
    fn a_plain_object_is_not_a_map() {
        let mgr = fixture();
        let err = err_of(validate_map(
            &mgr,
            "org.acme@1.0.0.StringMap",
            &json!({ "a": "1" }),
        ));
        assert!(
            err.to_string()
                .contains("Expected a Map, but found {\"a\":\"1\"}"),
            "{err}"
        );
    }

    /// `dayjs.utc(undefined)` is the current time, so an
    /// `undefined` value of a `DateTime` map passes.
    #[test]
    fn an_undefined_datetime_map_value_passes() {
        assert!(parses_as_dayjs(&js_undefined()));
    }

    /// BC-43: a number is not a valid `DateTime` map value, as it is
    /// not a valid `DateTime` field value.
    #[test]
    fn a_number_is_not_a_valid_datetime_map_value() {
        assert!(!parses_as_dayjs(&json!(1_700_000_000_000.0)));
    }

    /// [`parses_as_dayjs`]'s `Value::String` arm (cargo-mutants found that
    /// arm's deletion survived): a strict date-time string is a valid
    /// `DateTime` map value, and (BC-43) nothing looser is.
    #[test]
    fn only_a_strict_string_is_a_valid_datetime_map_value() {
        assert!(parses_as_dayjs(&json!("2024-05-01T00:00:00Z")));
        assert!(parses_as_dayjs(&json!("2024-05-01T10:00:00.5+02:00")));
        for s in [
            "2024-05-01",
            "2024xyz",
            " 2024-05-01",
            "20240102",
            "May 1, 2020",
            "1",
            "2024-02-30T00:00:00Z",
            "2024-05-01T24:00:00Z",
        ] {
            assert!(!parses_as_dayjs(&json!(s)), "{s:?}");
        }
    }

    /// [`parses_as_dayjs`]'s wildcard arm (cargo-mutants found the whole
    /// function's body replaced with a constant `true` surviving): a value
    /// that is none of undefined, a number or a string is never a valid
    /// `DateTime` map value.
    #[test]
    fn a_boolean_is_not_a_valid_datetime_map_value() {
        assert!(!parses_as_dayjs(&json!(true)));
    }

    /// A non-finite number is printed with `toString()`.
    #[test]
    fn a_non_finite_number_is_printed_with_to_string() {
        assert_eq!(
            field_value_param(&js_special_number("-Infinity")),
            "-Infinity"
        );
        assert_eq!(js_typeof(&js_special_number("NaN")), "number");
    }

    /// Validation plan: every map in the fixture resolves to a
    /// [`MapPlan`], with each slot of the kind `checkMapType` gives
    /// it.
    #[test]
    fn every_map_in_the_fixture_has_a_map_plan() {
        let mgr = fixture();
        for (name, key, value) in [
            ("StringMap", "Primitive", "Primitive"),
            ("ColorMap", "Primitive", "Enum"),
            ("ItemMap", "Primitive", "Class"),
            ("VehicleMap", "Primitive", "Relationship"),
            ("ScalarKeyMap", "Primitive", "Primitive"),
            ("PlainKeyScalarValueMap", "Primitive", "Skip"),
            ("DateTimeMap", "Primitive", "Primitive"),
            ("BooleanMap", "Primitive", "Primitive"),
        ] {
            let id = mgr
                .declaration_id(&format!("org.acme@1.0.0.{name}"))
                .unwrap();
            let map_plan = plan::map_plan(&mgr, id).unwrap_or_else(|e| panic!("{name}: {e}"));
            let kind = |slot: &MapSlot| match slot {
                MapSlot::Primitive(_) => "Primitive",
                MapSlot::Enum(_) => "Enum",
                MapSlot::Class(_) => "Class",
                MapSlot::Relationship(Ok(_)) => "Relationship",
                MapSlot::Skip => "Skip",
                MapSlot::Relationship(Err(_)) | MapSlot::Unresolved(_) => "Unresolved",
            };
            assert_eq!((kind(&map_plan.key), kind(&map_plan.value)), (key, value), "{name}");
        }
    }

    /// A map value whose declared type resolves to an enum is accepted.
    #[test]
    fn a_map_with_a_valid_enum_value_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("RED"))]);
        validate_map(&mgr, "org.acme@1.0.0.ColorMap", &map).unwrap();
    }

    #[test]
    fn a_map_with_an_invalid_enum_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("PURPLE"))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.ColorMap", &map));
        assert!(err.to_string().contains("Invalid enum value"), "{err}");
    }

    /// BC-05, DV-007: a `RelationshipMapValueType` map value is checked as
    /// a relationship property is (`checkRelationship`): a relationship to
    /// the declared type, or a subtype, passes.
    #[test]
    fn a_map_with_a_relationship_typed_value_accepts_a_relationship() {
        let mgr = fixture();
        let map = js_map(vec![(
            json!("a"),
            json!({ "$$relationship": true, "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12" }),
        )]);
        validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &map).unwrap();
    }

    /// BC-05, DV-007: an embedded resource in a relationship map is rejected
    /// by default, as in a relationship property (TS 5.0.0 required it), and
    /// accepted exactly when `permitResourcesForRelationships` or
    /// `convertResourcesToRelationships` allows it for a property.
    #[test]
    fn a_map_with_a_relationship_typed_value_takes_a_nested_resource_only_with_the_options() {
        let mgr = fixture();
        let map = js_map(vec![(
            json!("a"),
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1 }),
        )]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &map));
        assert!(matches!(Some(err.contract()), Some(e) if e.kind == ErrorKind::Validation));
        assert!(
            err.to_string().contains("Expected a \"Relationship\""),
            "{err}"
        );
        for options in [
            ValidateOptions {
                permit_resources_for_relationships: true,
                ..ValidateOptions::default()
            },
            ValidateOptions {
                convert_resources_to_relationships: true,
                ..ValidateOptions::default()
            },
        ] {
            validate_map_with(&mgr, "org.acme@1.0.0.VehicleMap", &map, options).unwrap();
        }
    }

    /// A relationship map value of the wrong type, or a string that was
    /// never populated into a relationship, fails as a relationship
    /// property does.
    #[test]
    fn a_map_with_a_relationship_typed_value_rejects_what_a_relationship_property_rejects() {
        let mgr = fixture();
        let wrong_type = js_map(vec![(
            json!("a"),
            json!({ "$$relationship": true, "$class": "org.acme@1.0.0.Owner", "ownerId": "O1" }),
        )]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &wrong_type));
        assert!(matches!(Some(err.contract()), Some(e) if e.kind == ErrorKind::Validation));
        assert!(err.to_string().contains("org.acme@1.0.0.Owner"), "{err}");
        let uri = js_map(vec![(json!("a"), json!("resource:org.acme@1.0.0.Vehicle#V1"))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &uri));
        assert!(
            err.to_string().contains("Expected a \"Relationship\""),
            "{err}"
        );
    }

    /// A map value whose own
    /// `$class` does not resolve to a type is a `ValidationException`
    /// ("not a Resource"), not a `TypeNotFoundException` from re-resolving
    /// that `$class`. `JSONPopulator.processMapType`'s `try`/`catch` (the
    /// only place TS swallows a `getType` failure) leaves such a value
    /// exactly as parsed — never a `Resource` — so `obj instanceof
    /// Resource` is false in TS before it ever looks at the value's own
    /// `$class` again.
    #[test]
    fn a_map_value_with_an_unresolvable_class_is_rejected_as_not_a_resource() {
        let mgr = fixture();
        let map = js_map(vec![(
            json!("a"),
            json!({ "$class": "org.acme@1.0.0.Missing", "name": "x" }),
        )]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.ItemMap", &map));
        assert!(matches!(Some(err.contract()), Some(e) if e.kind == ErrorKind::Validation));
        let message = err.to_string();
        assert!(
            message.contains("Expected a \"Resource\" or a \"Concept\""),
            "{message}"
        );
        assert!(message.contains("org.acme@1.0.0.Item"), "{message}");
        assert!(!message.contains("Missing"), "{message}");
    }

    /// [`map_key_is_scalar`] (never reached beyond its own early
    /// `key_kind() != "ObjectMapKeyType"` return — every other map fixture
    /// keys on a plain `StringMapKeyType`). `ScalarKeyMap` keys *and*
    /// values on `VIN` (a `StringScalar`): a real scalar key makes
    /// `checkMapType` substitute the scalar's own underlying type
    /// (`String`) for the value check.
    #[test]
    fn a_scalar_keyed_map_with_a_string_value_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("ABC12"))]);
        validate_map(&mgr, "org.acme@1.0.0.ScalarKeyMap", &map).unwrap();
    }

    #[test]
    fn a_scalar_keyed_map_with_a_non_string_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(12345))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.ScalarKeyMap", &map));
        assert!(err.to_string().contains("Expected Type of String"), "{err}");
    }

    /// The other half of `map_key_is_scalar`'s pairing: `checkMapType`
    /// reads the *key*'s scalar-ness even while checking the *value* slot
    /// (module doc "Scope", `map_key_is_scalar`'s own doc) — a faithfully
    /// ported quirk, not a bug. `PlainKeyScalarValueMap` has a scalar
    /// (`VIN`) value type but a plain `StringMapKeyType` key, so the value
    /// is never type-checked at all: even a value of the wrong JS type
    /// passes.
    #[test]
    fn a_scalar_valued_map_with_a_non_scalar_key_skips_value_type_checking() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(12345))]);
        validate_map(&mgr, "org.acme@1.0.0.PlainKeyScalarValueMap", &map)
            .expect("checkMapType only consults the key's scalar-ness, so an untyped value passes");
    }

    /// `checkMapType`'s `DateTime` primitive-kind arm (cargo-mutants
    /// found its `!parses_as_dayjs(value)` guard survived every mutation —
    /// `parses_as_dayjs` itself was unit-tested directly, but no fixture
    /// had a `DateTimeMapValueType` map to reach this guard through
    /// `check_map_type` itself).
    #[test]
    fn a_datetime_map_with_a_parseable_value_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("2024-05-01T00:00:00Z"))]);
        validate_map(&mgr, "org.acme@1.0.0.DateTimeMap", &map).unwrap();
    }

    #[test]
    fn a_datetime_map_with_an_unparseable_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(true))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.DateTimeMap", &map));
        assert!(
            err.to_string().contains("Expected Type of DateTime"),
            "{err}"
        );
    }

    /// `checkMapType`'s `Boolean` primitive-kind arm (same gap as the
    /// `DateTime` arm above — no `BooleanMapValueType` map fixture
    /// existed to reach it).
    #[test]
    fn a_boolean_map_with_a_boolean_value_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(true))]);
        validate_map(&mgr, "org.acme@1.0.0.BooleanMap", &map).unwrap();
    }

    #[test]
    fn a_boolean_map_with_a_non_boolean_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("nope"))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.BooleanMap", &map));
        assert!(
            err.to_string().contains("Expected Type of Boolean"),
            "{err}"
        );
    }

    // ---- Scalars ----

    #[test]
    fn a_scalar_field_matching_its_regex_passes() {
        let mgr = fixture();
        let garage = json!({ "$class": "org.acme@1.0.0.Garage", "vinField": "ABC12" });
        validate_instance(&mgr, &garage, &ValidateOptions::default()).unwrap();
    }

    #[test]
    fn a_scalar_field_violating_its_regex_is_rejected() {
        let mgr = fixture();
        let garage = json!({ "$class": "org.acme@1.0.0.Garage", "vinField": "not-valid!" });
        let err = err_of(validate_instance(
            &mgr,
            &garage,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string().contains("failed to match validation regex"),
            "{err}"
        );
    }

    // ---- Size and numeric domain validators ----

    #[test]
    fn an_array_field_within_its_size_bounds_passes() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "tags": ["a", "b"]
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    #[test]
    fn an_array_field_over_its_max_size_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "tags": ["a", "b", "c", "d"]
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("no more than 3"), "{err}");
    }

    #[test]
    fn a_numeric_field_outside_its_domain_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "rating": 9
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("upper bound"), "{err}");
    }

    /// [`check_enum`]'s `property.is_array() && !value.is_array()` guard
    /// (cargo-mutants found the `&&`->`||` and `delete !` mutants at this
    /// line survived): a valid *array* of enum values, which no existing
    /// test builds (every other enum test uses the non-array `color`).
    /// Under either mutant, `property.is_array()` (`true`) alone already
    /// makes the guard true, wrongly reporting a field type violation on
    /// this well-formed array.
    #[test]
    fn an_array_of_valid_enum_values_passes() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "colors": ["RED", "GREEN"]
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// [`check_primitive_item`]'s `sp.validator.is_some() ||
    /// sp.length_validator.is_some()` guard (cargo-mutants found the
    /// `||`->`&&` mutant survived): `code` carries a `validator` but no
    /// `length_validator`, so the real `||` runs `StringValidator` (and
    /// rejects a value its regex does not match) while the `&&` mutant
    /// would skip it (`length_validator` is `None`) and wrongly accept.
    #[test]
    fn a_string_field_s_own_validator_runs_without_a_length_validator_alongside_it() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "code": "not-uppercase"
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("code"), "{err}");
    }

    /// [`check_primitive_item`]'s `Property::Double(dp)` match arm
    /// (cargo-mutants found deleting it survived): no `Double` property
    /// existed anywhere in this fixture, so a deleted arm's fallthrough to
    /// `_ => unreachable!()` was never exercised.
    #[test]
    fn a_double_field_passes() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "weight": 12.5
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// [`property_has_default_value`] (cargo-mutants found the `-> false`
    /// mutant survived): every existing "missing required property" test
    /// omits a property with *no* default value, so the `true` branch
    /// (skip, rather than report missing) is never actually reached. `d`
    /// here has both `isOptional: false` and a `defaultValue`.
    #[test]
    fn a_missing_required_property_with_a_default_value_is_accepted() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme.defaults@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Defaulted",
                      "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "d",
                          "isArray": false, "isOptional": false, "defaultValue": "fallback" }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
        let value = json!({ "$class": "org.acme.defaults@1.0.0.Defaulted" });
        validate_instance(&mgr, &value, &ValidateOptions::default())
            .expect("a required property with a default value may be omitted");
    }

    /// [`fully_qualified_identifier`]'s `!id.is_empty()` match guard
    /// (cargo-mutants found the guard-true, guard-false and `delete !`
    /// mutants all survived): direct unit coverage of the pure function,
    /// distinguishing a present-but-empty id (falls back to the bare fqn,
    /// same as `None`) from a genuinely present one.
    #[test]
    fn fully_qualified_identifier_falls_back_to_the_bare_fqn_only_for_an_absent_or_empty_id() {
        assert_eq!(fully_qualified_identifier("ns.Foo", None), "ns.Foo");
        assert_eq!(fully_qualified_identifier("ns.Foo", Some("")), "ns.Foo");
        assert_eq!(
            fully_qualified_identifier("ns.Foo", Some("42")),
            "ns.Foo#42"
        );
    }

    /// [`identifiable_to_string`] (cargo-mutants found all three
    /// `-> None`/`Some(...)` mutants survived) and, incidentally,
    /// [`visit_class_declaration`]'s `!o.contains_key(RELATIONSHIP_TAG)`
    /// filter (the `delete !` mutant there): every existing enum/invalid-
    /// value test passes a plain string or number, for which
    /// `identifiable_to_string` already returns `None` (falls through to
    /// `js_to_string`) — never a `$class`-tagged value, so its `Some(...)`
    /// arm was never exercised. Assigning a `$$relationship`-tagged value to
    /// `pet` (a plain `Object`-typed, non-relationship property) hits
    /// exactly that arm: `visit_class_declaration` rejects it as "not a
    /// Resource" (a `Relationship` is `Identifiable`, never a `Resource`,
    /// module doc "Scope"), and the reported invalid value is
    /// `identifiable_to_string`'s `"Relationship {id=...}"` form.
    #[test]
    fn a_relationship_tagged_value_on_a_plain_object_property_reports_its_relationship_string_form()
    {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "pet": { "$$relationship": true, "$class": "org.acme@1.0.0.Dog" }
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string()
                .contains("Relationship {id=org.acme@1.0.0.Dog}"),
            "{err}"
        );
    }

    // ---- Wrong shape at the root (not a Resource / not a relationship) ----

    #[test]
    fn a_non_object_at_the_root_is_rejected() {
        let mgr = fixture();
        // No `$class` at all: `validate_instance` reports a harness-level
        // pre-port error, not a TS-reachable one (module doc: a real
        // `Resource` always has a `$class`).
        let err = err_of(validate_instance(
            &mgr,
            &json!(42),
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("$class"), "{err}");
    }

    #[test]
    fn convert_resources_to_relationships_permits_a_nested_resource_in_place_of_a_relationship() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicle": { "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1 }
        });
        let options = ValidateOptions {
            convert_resources_to_relationships: true,
            permit_resources_for_relationships: false,
        };
        validate_instance(&mgr, &owner, &options).unwrap();
    }

    /// The TS class and message of a failure, as the oracle records them.
    fn class_and_message(err: &Error) -> (&'static str, String) {
        let contract = err.contract().clone();
        (contract.kind.ts_class(), err.to_string())
    }

    // ---- A JS `undefined` is not `null` (fixture 642d743981a69328b04f1e33) ----

    /// `checkItem` reports an `undefined` array element with value and type
    /// both `undefined` (a `null` one would read `null`/`object`).
    #[test]
    fn an_undefined_array_element_is_reported_as_undefined() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "tags": ["a", js_undefined(), "b"]
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            class_and_message(&err),
            (
                "ValidationException",
                "Model violation in the \"org.acme@1.0.0.Vehicle#ABC12\" instance. The field \"tags\" has a value of \"undefined\" (type of value: \"undefined\"). Expected type of value: \"String[]\".".to_string()
            )
        );
    }

    /// An `undefined` field is `Util.isNull`, so an optional one is skipped.
    #[test]
    fn an_undefined_optional_field_is_skipped() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "tags": js_undefined()
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// `JSON.stringify([1, undefined])` is `[1,null]`.
    #[test]
    fn an_undefined_element_inside_a_reported_value_is_stringified_as_null() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12",
            "mileage": [1, js_undefined()]
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string()
                .contains("has a value of \"[1,null]\" (type of value: \"object\")"),
            "{err}"
        );
    }

    // ---- BC-06 (DV-008 was a V8 TypeError; fixture d444ebcf0cf5a3c23e5ee6dd) ----

    /// A string that reached a relationship array field is reported by its JS
    /// type (TS 5.0.0 called `obj.getFullyQualifiedType()` on it).
    #[test]
    fn a_non_array_non_identifiable_value_on_a_relationship_array_is_a_validation_error() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": "not-an-array"
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        let (class, message) = class_and_message(&err);
        assert_eq!(class, "ValidationException");
        assert!(
            message.contains("\"vehicles\" with type \"string\"")
                && message.contains("org.acme@1.0.0.Vehicle[]"),
            "{message}"
        );
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": 5
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        let (class, message) = class_and_message(&err);
        assert_eq!(class, "ValidationException");
        assert!(message.contains("with type \"number\""), "{message}");
    }

    /// A single `Relationship` on a relationship array field does have
    /// `getFullyQualifiedType()`, so TS reports the field assignment.
    #[test]
    fn a_single_relationship_on_a_relationship_array_is_an_invalid_field_assignment() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": { "$$relationship": true, "$class": "org.acme@1.0.0.Vehicle", "vin": "V1" }
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        let (class, message) = class_and_message(&err);
        assert_eq!(class, "ValidationException");
        assert!(
            message.contains("org.acme@1.0.0.Vehicle")
                && message.contains("org.acme@1.0.0.Vehicle[]"),
            "{message}"
        );
    }

    /// A `null` relationship array element is reported as `null` (TS 5.0.0
    /// called `value.toString()` on it).
    #[test]
    fn a_null_relationship_array_element_is_a_validation_error() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": [null]
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        let (class, message) = class_and_message(&err);
        assert_eq!(class, "ValidationException");
        assert!(
            message.contains("has a value of \"null\". Expected a \"Relationship\""),
            "{message}"
        );
    }

    /// `reportInvalidEnumValue`'s value goes through `String()`: a number is
    /// written as its digits, not dropped.
    #[test]
    fn a_numeric_enum_value_is_reported_by_its_string_form() {
        let mgr = fixture();
        let vehicle =
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "color": 1 });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string()
                .contains("Invalid enum value of \"1\" for the field \"Color\"."),
            "{err}"
        );
    }

    // ---- Collect-all diagnostics ----
    //
    // One test per `DiagnosticCode` (the issue's exit condition), plus a test
    // that collect-all really does gather more than one diagnostic in a
    // single pass, which is the point of the mode.

    /// The walk over `value`, collecting every violation, as diagnostics.
    fn collect_diagnostics(
        mgr: &ModelManager,
        _declared_fqn: &str,
        value: &Value,
        options: &ValidateOptions,
    ) -> ValidationReport {
        ValidationReport::new(
            collect_instance_violations(mgr, value, options, String::new(), true)
                .iter()
                .map(|(pointer, err)| super::super::diagnostic::walk_diagnostic(pointer, err))
                .collect(),
        )
    }

    fn diag_of(result: ValidationReport) -> Diagnostic {
        let mut diagnostics = result.into_diagnostics();
        assert_eq!(
            diagnostics.len(),
            1,
            "expected exactly one diagnostic, found {diagnostics:?}"
        );
        diagnostics.remove(0)
    }

    #[test]
    fn a_valid_instance_collects_no_diagnostics() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "a": "1", "b": "2", "c": "3" });
        let result = collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Leaf",
            &leaf,
            &ValidateOptions::default(),
        );
        assert!(result.is_valid());
        assert!(result.diagnostics().is_empty());
    }

    #[test]
    fn missing_required_property_is_diagnosed() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3" });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Leaf",
            &leaf,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::MissingRequiredProperty);
        assert_eq!(diag.pointer, "/a");
    }

    #[test]
    fn undeclared_field_is_diagnosed() {
        let mgr = fixture();
        let leaf = json!({
            "$class": "org.acme@1.0.0.Leaf", "a": "1", "b": "2", "c": "3", "zzz": "extra"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Leaf",
            &leaf,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::UndeclaredField);
        assert_eq!(diag.pointer, "/zzz");
    }

    #[test]
    fn type_violation_is_diagnosed() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": "not-a-number"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::TypeViolation);
        assert_eq!(diag.pointer, "/mileage");
    }

    #[test]
    fn invalid_enum_value_is_diagnosed() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "color": "PURPLE"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::InvalidEnumValue);
        assert_eq!(diag.pointer, "/color");
    }

    #[test]
    fn empty_identifier_is_diagnosed() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "", "mileage": 1 });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::EmptyIdentifier);
        assert_eq!(diag.pointer, "");
    }

    #[test]
    fn abstract_class_is_diagnosed() {
        let mgr = fixture();
        let animal = json!({ "$class": "org.acme@1.0.0.Animal", "name": "Rex" });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Animal",
            &animal,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::AbstractClass);
        assert_eq!(diag.pointer, "");
    }

    #[test]
    fn not_assignable_is_diagnosed() {
        let mgr = fixture();
        // `pet`'s declared type is `Animal`; a `Vehicle` is not assignable to it.
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "pet": { "$class": "org.acme@1.0.0.Vehicle", "vin": "XYZ99", "mileage": 2 }
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::NotAssignable);
        assert_eq!(diag.pointer, "/pet");
    }

    #[test]
    fn not_resource_is_diagnosed() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "pet": "just a string"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::NotResource);
        assert_eq!(diag.pointer, "/pet");
    }

    #[test]
    fn not_relationship_is_diagnosed() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1", "vehicle": "not a relationship"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Owner",
            &owner,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::NotRelationship);
        assert_eq!(diag.pointer, "/vehicle");
    }

    /// The plan's `NumberValidator::from_bounds` (a body replaced with JSON
    /// `null` must fail): [`validator_failure_is_diagnosed`] below only ever gives
    /// `rating` an out-of-range value, so a construction error from lost
    /// bounds (the constructor's own "no bounds" rejection) reports the same
    /// `ValidatorFailure` diagnostic the real out-of-range check does. An
    /// in-bounds value tells them apart.
    #[test]
    fn an_in_bounds_rating_collects_no_diagnostics() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "rating": 3
        });
        let result = collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        );
        assert!(result.is_valid());
        assert!(result.diagnostics().is_empty());
    }

    #[test]
    fn validator_failure_is_diagnosed() {
        let mgr = fixture();
        // `rating`'s `IntegerDomainValidator` is `0..=5`.
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "rating": 10
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::ValidatorFailure);
        assert_eq!(diag.pointer, "/rating");
    }

    #[test]
    fn type_not_found_is_diagnosed() {
        let mgr = fixture();
        let unknown = json!({ "$class": "org.acme@1.0.0.NoSuchType" });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.NoSuchType",
            &unknown,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::TypeNotFound);
        assert_eq!(diag.pointer, "");
    }

    /// The point of collect-all: several unrelated problems on one instance
    /// are all reported from a single call, not just the first one a
    /// first-error walk would stop at.
    #[test]
    fn collect_all_gathers_every_diagnostic_in_one_pass() {
        let mgr = fixture();
        let leaf = json!({
            // `a` is missing (required, from `Base`), and `zzz` is
            // undeclared: two unrelated problems, neither of which is the
            // other's cause.
            "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3", "zzz": "extra"
        });
        let result = collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Leaf",
            &leaf,
            &ValidateOptions::default(),
        );
        assert!(!result.is_valid());
        let codes: Vec<DiagnosticCode> = result.diagnostics().iter().map(|d| d.code).collect();
        assert!(
            codes.contains(&DiagnosticCode::MissingRequiredProperty),
            "{codes:?}"
        );
        assert!(
            codes.contains(&DiagnosticCode::UndeclaredField),
            "{codes:?}"
        );
        assert_eq!(result.diagnostics().len(), 2, "{:?}", result.diagnostics());

        // First-error, by contrast, only ever reports one.
        let err = err_of(validate_instance(&mgr, &leaf, &ValidateOptions::default()));
        let _ = err;
    }

    /// [`ModelManager::check_instance`] resolves the declared type from the
    /// value's own `$class`, and [`ModelManager::validate_instance`] reports
    /// the first error, as the free [`validate_instance`] does.
    #[test]
    fn model_manager_entry_points_agree_with_the_free_functions() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3" });
        let options = crate::instance::ValidationOptions::default();

        let result = mgr.check_instance(&leaf, &options);
        assert_eq!(
            diag_of(result).code,
            DiagnosticCode::MissingRequiredProperty
        );

        let err = err_of(mgr.validate_instance(&leaf, &options));
        assert!(err.to_string().contains("\"a\""), "{err}");
        let free = err_of(validate_instance(&mgr, &leaf, &ValidateOptions::default()));
        assert_eq!(err.kind(), free.kind());
        assert_eq!(err.code(), free.code());
    }

    /// The `_as` entry points check against the named type: an empty
    /// identifier is the `Factory` error `Serializer.fromJSON` raises.
    #[test]
    fn the_as_entry_points_validate_against_the_named_type() {
        let mgr = fixture();
        let fqn = "org.acme@1.0.0.Vehicle";
        let vehicle = json!({ "vin": "", "mileage": 1 });
        let options = crate::instance::ValidationOptions::default();

        let result = mgr.check_instance_as(fqn, &vehicle, &options);
        assert_eq!(diag_of(result).code, DiagnosticCode::EmptyIdentifier);

        let err = err_of(mgr.validate_instance_as(fqn, &vehicle, &options));
        assert_eq!(err.code(), "factory-newinstance-missingidentifier");
    }

    /// Review finding: collecting must not skip the `sizeValidator` check
    /// `check_array` runs for the first-error walk — otherwise the two modes
    /// disagree on whether an over-size array of class-typed elements is
    /// valid.
    #[test]
    fn collect_all_reports_a_class_typed_array_over_its_max_size() {
        let mgr = fixture();
        let garage = json!({
            "$class": "org.acme@1.0.0.Garage", "vinField": "ABC12",
            "items": [
                { "$class": "org.acme@1.0.0.Item", "name": "a" },
                { "$class": "org.acme@1.0.0.Item", "name": "b" },
                { "$class": "org.acme@1.0.0.Item", "name": "c" }
            ]
        });

        // First-error already catches this (it goes through `check_array`).
        let err = err_of(validate_instance(
            &mgr,
            &garage,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("items"), "{err}");

        // Collect-all must agree: this is not a valid instance.
        let result = collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Garage",
            &garage,
            &ValidateOptions::default(),
        );
        assert!(!result.is_valid(), "{result:?}");
        let codes: Vec<DiagnosticCode> = result.diagnostics().iter().map(|d| d.code).collect();
        assert!(
            codes.contains(&DiagnosticCode::ValidatorFailure),
            "{codes:?}"
        );
    }

    /// Review finding: the `_as` entry points must check the value's own
    /// `$class` against the named type, not silently validate whatever
    /// `value` claims to be (which is what `visit_class_declaration` does on
    /// its own, module doc).
    #[test]
    fn the_as_entry_points_reject_a_value_not_assignable_to_the_named_type() {
        let mgr = fixture();
        let dog_fqn = "org.acme@1.0.0.Dog";
        // A `Base` instance (unrelated to `Dog`/`Animal`), passed against
        // `Dog`'s own fqn.
        let base_instance = json!({ "$class": "org.acme@1.0.0.Base", "a": "x" });
        let options = crate::instance::ValidationOptions::default();

        let result = mgr.check_instance_as(dog_fqn, &base_instance, &options);
        assert!(!result.is_valid(), "{result:?}");
        assert_eq!(diag_of(result).code, DiagnosticCode::NotAssignable);

        let err = err_of(mgr.validate_instance_as(dog_fqn, &base_instance, &options));
        assert!(err.to_string().contains("not assignable"), "{err}");
    }

    // ---- One walk, stop or collect ----

    /// The first violation collected is the error the first-error walk
    /// returns (class, code and message), and every collected diagnostic's
    /// message is its error's own, from the catalogue: the same instance
    /// reads the same in both modes.
    #[test]
    fn the_first_violation_collected_is_the_error_thrown() {
        let mgr = fixture();
        let options = ValidateOptions::default();
        for value in [
            json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3", "zzz": "extra" }),
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": "far", "color": "PURPLE" }),
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": " ", "mileage": 1, "tags": ["a", 1, "b", 2] }),
            json!({
                "$class": "org.acme@1.0.0.Garage", "vinField": "bad!",
                "items": [ { "$class": "org.acme@1.0.0.Item" }, { "$class": "org.acme@1.0.0.Item", "name": 1 } ]
            }),
            json!({ "$class": "org.acme@1.0.0.Animal" }),
        ] {
            let thrown = validate_instance(&mgr, &value, &options).unwrap_err();
            let all = collect_instance_violations(&mgr, &value, &options, String::new(), true);
            let first = collect_instance_violations(&mgr, &value, &options, String::new(), false);
            assert_eq!(first.len(), 1, "{value}");
            assert_eq!(all[0], first[0], "{value}");
            let (_, collected) = &first[0];
            assert_eq!(collected.kind(), thrown.kind(), "{value}");
            assert_eq!(collected.code(), thrown.code(), "{value}");
            assert_eq!(collected.to_string(), thrown.to_string(), "{value}");
            assert_eq!(*collected, thrown, "{value}");
            let report = collect_diagnostics(&mgr, "", &value, &options);
            for (diagnostic, (_, err)) in report.diagnostics().iter().zip(&all) {
                assert_eq!(diagnostic.message, err.to_string());
            }
        }
    }

    /// Collecting goes on past each violation with the next key, property,
    /// array element and nested object, each at its own pointer, in walk
    /// order, with TS's wording.
    #[test]
    fn collecting_reports_each_violation_at_its_own_pointer() {
        let mgr = fixture();
        let garage = json!({
            "$class": "org.acme@1.0.0.Garage", "vinField": "bad!",
            "items": [
                { "$class": "org.acme@1.0.0.Item" },
                { "$class": "org.acme@1.0.0.Item", "name": 1, "a/b": true }
            ]
        });
        let found: Vec<(String, String)> = collect_instance_violations(
            &mgr,
            &garage,
            &ValidateOptions::default(),
            String::new(),
            true,
        )
        .into_iter()
        .map(|(pointer, err)| (pointer, err.code().to_string()))
        .collect();
        assert_eq!(
            found,
            [
                ("/vinField", "stringvalidator-validate-regexmismatch"),
                ("/items/0/name", "resourcevalidator-missingrequiredproperty"),
                ("/items/1/a~1b", "resourcevalidator-undeclaredfield"),
                ("/items/1/name", "resourcevalidator-fieldtypeviolation"),
            ]
            .map(|(p, c)| (p.to_string(), c.to_string()))
        );
        let report = collect_diagnostics(&mgr, "", &garage, &ValidateOptions::default());
        assert_eq!(
            report.diagnostics()[1].message,
            "The instance \"org.acme@1.0.0.Item\" is missing the required field \"name\"."
        );
    }

    /// A map's entries are collected one by one, at their keys (an entry
    /// whose key fails is not checked further).
    #[test]
    fn collecting_reports_each_map_entry_at_its_key() {
        let mgr = fixture();
        let map = js_map(vec![
            (json!("a"), json!(1)),
            (json!("b"), json!("ok")),
            (json!(7), json!(true)),
        ]);
        let id = mgr.declaration_id("org.acme@1.0.0.StringMap").unwrap();
        let map_plan = plan::map_plan(&mgr, id);
        let options = ValidateOptions::default();
        let sink = Sink::Collect {
            found: Vec::new(),
            all: true,
        };
        let mut params = Params::new(&mgr, &options, String::new(), sink);
        visit_map_declaration(&mut params, id, &map_plan, &map).unwrap();
        let pointers: Vec<String> = params.into_found().into_iter().map(|(p, _)| p).collect();
        assert_eq!(pointers, ["/a", "/7"]);
    }

    // ---- The enum blind spots (R2B-1, R2B-2) and an unknown
    //      nested `$class` (R2B-4) ----

    /// R2B-1: a map value whose `$class` names a scalar is kept by the
    /// populator as received (not a `Resource`), so TS's
    /// `visitClassDeclaration` reports it as not a resource, a
    /// `ValidationException`.
    #[test]
    fn a_map_value_whose_class_is_a_scalar_is_not_a_resource() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!({ "$class": "org.acme@1.0.0.VIN" }))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.ItemMap", &map));
        assert_eq!(err.kind(), ErrorKind::Validation);
        assert_eq!(err.code(), "resourcevalidator-notresourceorconcept");
    }

    /// R2B-1: a map value whose `$class` names an enum is a `Resource` of
    /// the enum (TS `EnumDeclaration.isClassDeclaration()` is true), whose
    /// walk reports the missing enum values as missing required properties.
    #[test]
    fn a_map_value_whose_class_is_an_enum_is_walked_as_a_resource() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!({ "$class": "org.acme@1.0.0.Color" }))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.ItemMap", &map));
        assert_eq!(err.kind(), ErrorKind::Validation);
        assert_eq!(err.code(), "resourcevalidator-missingrequiredproperty");
    }

    /// R2B-2 (a): a relationship to an enum reaches `checkRelationship`'s
    /// `getIdentifierFieldName()` test, the plain `Error` TS throws.
    #[test]
    fn a_relationship_to_an_enum_is_not_identifiable() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicle": { "$$relationship": true, "$class": "org.acme@1.0.0.Color" }
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert_eq!(err.code(), "resourcevalidator-checkrelationship-notidentifiable");
    }

    /// R2B-2 (b): an enum is assignable to its implicit `Concept` super
    /// type, as [`ModelManager::is_assignable_to`] answers.
    #[test]
    fn an_enum_is_assignable_to_concept_in_the_walk() {
        let mgr = fixture();
        let concept = "concerto@1.0.0.Concept";
        assert!(is_assignable(&mgr, "org.acme@1.0.0.Color", concept).unwrap());
        assert!(mgr.is_assignable_to("org.acme@1.0.0.Color", concept).unwrap());
        assert!(!is_assignable(&mgr, "org.acme@1.0.0.Color", "org.acme@1.0.0.Animal").unwrap());
    }

    /// R2B-4: a nested object whose own `$class` is not declared is TS
    /// `checkItem`'s field type violation (its `getType` failure is caught),
    /// not a `TypeNotFoundException`.
    #[test]
    fn a_nested_object_of_an_unknown_class_is_a_field_type_violation() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "pet": { "$class": "org.acme@1.0.0.Missing", "name": "x" }
        });
        let err = err_of(validate_instance(&mgr, &vehicle, &ValidateOptions::default()));
        assert_eq!(err.kind(), ErrorKind::Validation);
        assert_eq!(err.code(), "resourcevalidator-fieldtypeviolation");
    }

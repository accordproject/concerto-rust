use super::*;

#[test]
fn an_empty_result_is_valid() {
    let result = ValidationReport::new(Vec::new());
    assert!(result.is_valid());
    assert!(result.diagnostics().is_empty());
}

#[test]
fn a_result_with_an_error_diagnostic_is_not_valid() {
    let result = ValidationReport::new(vec![Diagnostic::error(
        "/name".to_string(),
        DiagnosticCode::TypeViolation,
        "bad".to_string(),
    )]);
    assert!(!result.is_valid());
    assert_eq!(result.diagnostics().len(), 1);
    assert_eq!(result.diagnostics()[0].code, DiagnosticCode::TypeViolation);
}

#[test]
fn every_diagnostic_code_has_a_stable_screaming_snake_case_spelling() {
    let codes = [
        DiagnosticCode::MissingRequiredProperty,
        DiagnosticCode::UndeclaredField,
        DiagnosticCode::TypeViolation,
        DiagnosticCode::InvalidEnumValue,
        DiagnosticCode::EmptyIdentifier,
        DiagnosticCode::AbstractClass,
        DiagnosticCode::NotAssignable,
        DiagnosticCode::NotResource,
        DiagnosticCode::NotRelationship,
        DiagnosticCode::ValidatorFailure,
        DiagnosticCode::TypeNotFound,
    ];
    for code in codes {
        assert_eq!(code.as_str(), code.to_string());
        assert_eq!(code.as_str(), code.as_str().to_uppercase());
    }
}

#[test]
fn a_populator_path_becomes_a_json_pointer() {
    assert_eq!(pointer_of_path("$"), "");
    assert_eq!(pointer_of_path("$.vin"), "/vin");
    assert_eq!(pointer_of_path("$.tags[0].name"), "/tags/0/name");
    assert_eq!(pointer_of_path("$.a/b.c~d"), "/a~1b/c~0d");
}

#[test]
fn a_report_converts_to_a_result_and_iterates() {
    let empty = ValidationReport::new(Vec::new());
    assert!(empty.into_result().is_ok());
    let report = ValidationReport::new(vec![Diagnostic::error(
        "/name".to_string(),
        DiagnosticCode::TypeViolation,
        "bad".to_string(),
    )]);
    assert_eq!((&report).into_iter().count(), 1);
    let report = report.into_result().unwrap_err();
    assert_eq!(report.into_iter().next().unwrap().pointer, "/name");
}

// ---- `diagnose` ----

use crate::ErrorKind;
use crate::json;

const MM: &str = "concerto.metamodel@1.0.0";

fn prop(class: &str, name: &str, extra: Value) -> Value {
    let mut node = json!({
        "$class": format!("{MM}.{class}"),
        "name": name,
        "isArray": false,
        "isOptional": false,
    });
    for (k, v) in extra.as_object().into_iter().flatten() {
        node[k] = v.clone();
    }
    node
}

fn type_ref(name: &str) -> Value {
    json!({ "type": { "$class": format!("{MM}.TypeIdentifier"), "name": name } })
}

/// `org.acme@1.0.0`: `Address { city }`, `Person` identified by
/// `email`, with an `address`, `tags: String[]`, an optional `age`, a
/// `colour` enum and a `friend` relationship; `Employee extends Person`;
/// an asset `Car`.
fn manager() -> ModelManager {
    let mut mm = ModelManager::new().unwrap();
    mm.load_model(
            &json!({
                "$class": format!("{MM}.Model"),
                "namespace": "org.acme@1.0.0",
                "declarations": [
                    { "$class": format!("{MM}.ConceptDeclaration"), "name": "Address", "isAbstract": false,
                      "properties": [prop("StringProperty", "city", json!({}))] },
                    { "$class": format!("{MM}.EnumDeclaration"), "name": "Colour",
                      "properties": [{ "$class": format!("{MM}.EnumProperty"), "name": "RED" }] },
                    { "$class": format!("{MM}.ParticipantDeclaration"), "name": "Person", "isAbstract": false,
                      "identified": { "$class": format!("{MM}.IdentifiedBy"), "name": "email" },
                      "properties": [
                        prop("StringProperty", "email", json!({})),
                        prop("ObjectProperty", "address", type_ref("Address")),
                        prop("StringProperty", "tags", json!({ "isArray": true, "isOptional": true })),
                        prop("IntegerProperty", "age", json!({ "isOptional": true })),
                        prop("ObjectProperty", "colour", { let mut t = type_ref("Colour"); t["isOptional"] = json!(true); t }),
                        prop("RelationshipProperty", "friend", { let mut t = type_ref("Person"); t["isOptional"] = json!(true); t }),
                      ] },
                    { "$class": format!("{MM}.ParticipantDeclaration"), "name": "Employee", "isAbstract": false,
                      "superType": { "$class": format!("{MM}.TypeIdentifier"), "name": "Person" },
                      "properties": [] },
                    { "$class": format!("{MM}.AssetDeclaration"), "name": "Car", "isAbstract": false,
                      "identified": { "$class": format!("{MM}.IdentifiedBy"), "name": "vin" },
                      "properties": [prop("StringProperty", "vin", json!({}))] },
                ]
            }),
            None,
        )
        .unwrap();
    mm
}

fn person(extra: Value) -> Value {
    let mut p = json!({
        "$class": "org.acme@1.0.0.Person",
        "email": "a@example.com",
        "address": { "$class": "org.acme@1.0.0.Address", "city": "Paris" }
    });
    for (k, v) in extra.as_object().into_iter().flatten() {
        p[k] = v.clone();
    }
    p
}

fn options() -> from_json::FromJsonOptions {
    from_json::FromJsonOptions::default()
}

#[test]
fn diagnose_a_valid_instance_reports_nothing() {
    let mm = manager();
    let d = diagnose(&mm, None, &person(json!({})), &options(), true);
    assert!(d.report.is_valid() && d.report.diagnostics().is_empty());
    assert!(d.error.is_none());
    let d = diagnose(
        &mm,
        Some("org.acme@1.0.0.Person"),
        &json!({ "$class": "org.acme@1.0.0.Employee", "email": "e@x", "address": { "city": "Rome" } }),
        &options(),
        true,
    );
    assert!(d.error.is_none(), "{:?}", d.error);
}

#[test]
fn diagnose_puts_the_thrown_error_first_and_locates_it() {
    let mm = manager();
    // `address.city` is missing and `colour` is not a `Colour`: the
    // first-error walk stops at one, the collect-all walk finds both.
    let instance =
        person(json!({ "address": { "$class": "org.acme@1.0.0.Address" }, "colour": "BLUE" }));
    let thrown = mm
        .validate_instance(&instance, &crate::instance::ValidationOptions::default())
        .unwrap_err();
    let all = diagnose(&mm, None, &instance, &options(), true);
    let first = diagnose(&mm, None, &instance, &options(), false);
    let error = all.error.clone().unwrap();
    assert_eq!(error, thrown);
    assert_eq!(first.report.diagnostics().len(), 1);
    assert_eq!(first.report.diagnostics()[0], all.report.diagnostics()[0]);
    let code = report_of_error(&thrown).diagnostics()[0].code;
    assert_eq!(all.report.diagnostics()[0].code, code);
    assert_eq!(all.report.diagnostics()[0].message, thrown.to_string());
    assert!(all.report.diagnostics().len() >= 2, "{:?}", all.report);
    // No two diagnostics share a location and a code.
    let d = all.report.diagnostics();
    for (i, a) in d.iter().enumerate() {
        assert!(
            !d[i + 1..]
                .iter()
                .any(|b| a.pointer == b.pointer && a.code == b.code)
        );
    }
    assert_eq!(
        diagnostics_of_error(&mm, None, &instance, &options(), &error),
        first.report.into_diagnostics()
    );
}

#[test]
fn diagnose_a_missing_nested_property_names_its_path_and_type() {
    let mm = manager();
    let instance = person(json!({ "address": { "$class": "org.acme@1.0.0.Address" } }));
    let d = diagnose(&mm, None, &instance, &options(), true);
    assert_eq!(d.error.unwrap().kind(), ErrorKind::Validation);
    let first = &d.report.diagnostics()[0];
    assert_eq!(first.code, DiagnosticCode::MissingRequiredProperty);
    assert_eq!(first.pointer, "/address/city");
    assert_eq!(first.expected.as_deref(), Some("String"));
    assert_eq!(first.severity, Severity::Error);
}

#[test]
fn diagnose_spells_the_expected_type_of_arrays_relationships_and_enums() {
    let mm = manager();
    let expected =
        |pointer: &str| expected_at(&mm, None, &person(json!({ "tags": ["a"] })), pointer);
    assert_eq!(expected("/tags").as_deref(), Some("String[]"));
    assert_eq!(expected("/tags/0").as_deref(), Some("String"));
    assert_eq!(
        expected("/friend").as_deref(),
        Some("--> org.acme@1.0.0.Person")
    );
    assert_eq!(
        expected("/colour").as_deref(),
        Some("org.acme@1.0.0.Colour")
    );
    assert_eq!(
        expected("/address").as_deref(),
        Some("org.acme@1.0.0.Address")
    );
    assert_eq!(expected("/address/city").as_deref(), Some("String"));
    assert_eq!(expected("/email/x"), None);
    assert_eq!(expected("/undeclared"), None);
    assert_eq!(expected(""), None);
    assert_eq!(
        expected_at(&mm, Some("org.acme@1.0.0.Person"), &json!({}), "").as_deref(),
        Some("org.acme@1.0.0.Person")
    );
}

#[test]
fn diagnose_a_wrong_type_takes_the_populator_path_and_type() {
    let mm = manager();
    let d = diagnose(
        &mm,
        None,
        &person(json!({ "age": "old" })),
        &options(),
        true,
    );
    let first = &d.report.diagnostics()[0];
    assert_eq!(first.code, DiagnosticCode::TypeViolation);
    assert_eq!(first.pointer, "/age");
    assert_eq!(first.expected.as_deref(), Some("Integer"));
}

#[test]
fn diagnose_checks_the_class_against_the_named_type() {
    let mm = manager();
    let car = json!({ "$class": "org.acme@1.0.0.Car", "vin": "1" });
    let d = diagnose(&mm, Some("org.acme@1.0.0.Person"), &car, &options(), true);
    assert_eq!(d.error.unwrap().kind(), ErrorKind::Validation);
    assert_eq!(d.report.diagnostics().len(), 1);
    let first = &d.report.diagnostics()[0];
    assert_eq!(first.code, DiagnosticCode::NotAssignable);
    assert_eq!(first.pointer, "");
    assert_eq!(first.expected.as_deref(), Some("org.acme@1.0.0.Person"));
    let unknown = json!({ "$class": "org.acme@1.0.0.Nope" });
    let d = diagnose(
        &mm,
        Some("org.acme@1.0.0.Person"),
        &unknown,
        &options(),
        true,
    );
    assert_eq!(d.error.unwrap().kind(), ErrorKind::TypeNotFound);
    assert_eq!(d.report.diagnostics()[0].code, DiagnosticCode::TypeNotFound);
    assert_eq!(d.report.diagnostics()[0].expected, None);
}

#[test]
fn diagnose_reports_each_1273_detail_with_its_expected_type() {
    let mm = manager();
    let strict = from_json::FromJsonOptions {
        reject_unknown_keys: true,
        reject_required_null: true,
        ..options()
    };
    let d = diagnose(
        &mm,
        None,
        &person(json!({ "address": { "city": null } })),
        &strict,
        false,
    );
    assert_eq!(d.error.as_ref().unwrap().details().len(), 1);
    let first = &d.report.diagnostics()[0];
    assert_eq!(first.code, DiagnosticCode::TypeViolation);
    assert_eq!(first.pointer, "/address/city");
    assert_eq!(first.expected.as_deref(), Some("String"));
    let d = diagnose(
        &mm,
        None,
        &person(json!({ "zip": 1, "zap": 2 })),
        &strict,
        false,
    );
    let codes: Vec<_> = d.report.diagnostics().iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        [
            DiagnosticCode::UndeclaredField,
            DiagnosticCode::UndeclaredField
        ]
    );
    assert_eq!(d.report.diagnostics()[0].expected, None);
}

#[test]
fn diagnose_an_instance_with_no_class() {
    let mm = manager();
    let d = diagnose(&mm, None, &json!({ "email": "a" }), &options(), true);
    assert_eq!(d.error.unwrap().kind(), ErrorKind::InvalidArgument);
    assert_eq!(d.report.diagnostics()[0].code, DiagnosticCode::NotResource);
    // Read as the named type instead.
    let d = diagnose(
        &mm,
        Some("org.acme@1.0.0.Address"),
        &json!({ "city": "Oslo" }),
        &options(),
        true,
    );
    assert!(d.error.is_none());
}
#[test]
fn diagnose_locates_an_error_that_names_no_path() {
    let mm = manager();
    let first = |instance: Value| {
        let d = diagnose(&mm, None, &instance, &options(), false);
        d.report
            .diagnostics()
            .iter()
            .map(|d| (d.code, d.pointer.clone()))
            .collect::<Vec<_>>()
    };
    // Undeclared keys, nested, one diagnostic each.
    assert_eq!(
        first(person(
            json!({ "address": { "$class": "org.acme@1.0.0.Address", "city": "P", "a/b": 1, "c": 2 } })
        )),
        [
            (DiagnosticCode::UndeclaredField, "/address/a~1b".to_string()),
            (DiagnosticCode::UndeclaredField, "/address/c".to_string()),
        ]
    );
    // A type that is not found, nested.
    assert_eq!(
        first(person(
            json!({ "address": { "$class": "org.acme@1.0.0.Nope" } })
        )),
        [(DiagnosticCode::TypeNotFound, "/address".to_string())]
    );
    // A value that is not a relationship.
    assert_eq!(
        first(person(json!({ "friend": 42 }))),
        [(DiagnosticCode::NotRelationship, "/friend".to_string())]
    );
    // An invalid enum value: the TS error names the enum, not the
    // property, so the walk's diagnostic of the same code is used.
    assert_eq!(
        first(person(json!({ "colour": "BLUE" }))),
        [(DiagnosticCode::InvalidEnumValue, "/colour".to_string())]
    );
    // An abstract type and a missing identifier, at the root.
    assert_eq!(
        first(json!({ "$class": "org.acme@1.0.0.Car", "vin": "" })),
        [(DiagnosticCode::EmptyIdentifier, String::new())]
    );
    let unknown = json!({ "$class": "org.acme@1.0.0.Nope" });
    assert_eq!(
        first(unknown),
        [(DiagnosticCode::TypeNotFound, String::new())]
    );
    assert_eq!(
        find_object(&json!([{ "a": 1 }, { "b": { "c": 1 } }]), "", &|m| m
            .contains_key("c")),
        Some("/1/b".to_string())
    );
}

#[test]
fn diagnose_read_takes_the_verdict_and_error_from_the_read() {
    let mm = manager();
    let bad =
        person(json!({ "address": { "$class": "org.acme@1.0.0.Address" }, "colour": "BLUE" }));
    let native = diagnose(&mm, None, &bad, &options(), true);
    let error = native.error.clone().unwrap();
    // The read raises the walk's own error: the walk's whole report is
    // kept, from the first reading that raises it.
    let readings = [person(json!({})), bad.clone()];
    let d = diagnose_read(
        &mm,
        None,
        &readings,
        &options(),
        true,
        || Err(error.clone()),
    );
    assert_eq!(d, native);
    // The read finds the document valid: no diagnostics, whatever the
    // walk would say.
    let d = diagnose_read(
        &mm,
        None,
        std::slice::from_ref(&bad),
        &options(),
        true,
        || Ok(()),
    );
    assert!(d.error.is_none() && d.report.is_valid());
    // A read error no reading raises: the error's own diagnostics,
    // located in the first reading.
    let other = person(json!({ "colour": "BLUE" }));
    let other_error = diagnose(&mm, None, &other, &options(), false)
        .error
        .unwrap();
    let d = diagnose_read(
        &mm,
        None,
        std::slice::from_ref(&bad),
        &options(),
        true,
        || Err(other_error.clone()),
    );
    assert_eq!(d.error.as_ref(), Some(&other_error));
    assert_eq!(
        d.report.into_diagnostics(),
        diagnostics_of_error(&mm, None, &bad, &options(), &other_error)
    );
    // With a named type, the class check comes first, before the read.
    let car = [json!({ "$class": "org.acme@1.0.0.Car", "vin": "1" })];
    let d = diagnose_read(
        &mm,
        Some("org.acme@1.0.0.Person"),
        &car,
        &options(),
        true,
        || panic!("read after a failed class check"),
    );
    assert_eq!(
        d.report.diagnostics()[0].code,
        DiagnosticCode::NotAssignable
    );
}

/// No readings is not a panic. The verdict and the
/// error are still the read's, and the error's diagnostics are located
/// at the root.
#[test]
fn diagnose_read_takes_no_readings() {
    let mm = manager();
    let d = diagnose_read(&mm, None, &[], &options(), true, || Ok(()));
    assert!(d.error.is_none() && d.report.is_valid());
    let error = diagnose(
        &mm,
        None,
        &person(json!({ "colour": "BLUE" })),
        &options(),
        false,
    )
    .error
    .unwrap();
    let d = diagnose_read(&mm, None, &[], &options(), true, || Err(error.clone()));
    assert_eq!(d.error.as_ref(), Some(&error));
    assert_eq!(
        d.report.into_diagnostics(),
        diagnostics_of_error(&mm, None, &Value::Null, &options(), &error)
    );
    // With a named type, the class check reads the missing document as
    // `null`, which has no `$class`, so the read gives the verdict.
    let d = diagnose_read(
        &mm,
        Some("org.acme@1.0.0.Person"),
        &[],
        &options(),
        true,
        || Ok(()),
    );
    assert!(d.error.is_none() && d.report.is_valid());
}

/// One walk, so every collect-all diagnostic carries the message of the
/// error the first-error walk would raise for it (the catalogue's TS
/// wording), at the pointer it was found at.
#[test]
fn diagnose_collects_with_the_catalogue_s_messages() {
    let mm = manager();
    let instance = person(json!({
        "address": { "$class": "org.acme@1.0.0.Address" },
        "colour": "BLUE"
    }));
    let d = diagnose(&mm, None, &instance, &options(), true);
    let got: Vec<_> = d
        .report
        .diagnostics()
        .iter()
        .map(|d| (d.code, d.pointer.as_str(), d.message.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                DiagnosticCode::MissingRequiredProperty,
                "/address/city",
                "The instance \"org.acme@1.0.0.Address\" is missing the required field \"city\"."
            ),
            // `parameters.rootResourceIdentifier` is the last resource
            // visited (`address`), as TS leaves it: the walk never
            // restores it on the way back up.
            (
                DiagnosticCode::InvalidEnumValue,
                "/colour",
                "Model violation in the \"org.acme@1.0.0.Address\" instance. Invalid enum value of \"BLUE\" for the field \"Colour\"."
            ),
        ]
    );
    assert_eq!(
        d.report.diagnostics()[0].message,
        d.error.unwrap().to_string()
    );
    // `check_instance` reads the same.
    let checked = mm.check_instance(&instance, &crate::instance::ValidationOptions::default());
    let messages: Vec<_> = checked
        .diagnostics()
        .iter()
        .map(|d| d.message.clone())
        .collect();
    assert_eq!(
        messages,
        got.iter()
            .map(|(_, _, m)| m.to_string())
            .collect::<Vec<_>>()
    );
}

use super::*;
use crate::json;

/// One resident manager, holding the one shared metamodel file under
/// its namespace, as `validateAst` registers it.
#[test]
fn the_resident_manager_holds_the_shared_metamodel_file() {
    let file = metamodel_model_file().unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &file,
        &metamodel_model_file().unwrap()
    ));
    assert_eq!(file.file_name(), Some(METAMODEL_NAMESPACE));
    with_resident_metamodel_manager(|mm| {
        let held = mm
            .shared_model_files()
            .find(|mf| mf.namespace() == METAMODEL_NAMESPACE)
            .expect("the resident manager holds the metamodel");
        assert!(std::sync::Arc::ptr_eq(held, &file));
        Ok(())
    })
    .unwrap();
}

/// The presets the `validateMetaModelInstance` binding takes read as
/// the options their callers used before: strict for `validateAst`, a
/// Serializer's defaults (the same as the manager's) for
/// `validateMetaModel`.
#[test]
fn metamodel_presets_read_as_their_callers_options() {
    let strict = MetaModelPreset::Strict.from_json_options();
    assert!(strict.reject_unknown_keys && strict.reject_required_null && strict.validate);
    assert_eq!(
        MetaModelPreset::Default.from_json_options(),
        FromJsonOptions::default()
    );
    assert_eq!(
        MetaModelPreset::Serializer.from_json_options(),
        FromJsonOptions::default()
    );
}

// ---- concerto-validate-rs's src/lib.rs tests, ported as tests of
//      this module ----

/// concerto-validate-rs `tests::test_valid_metamodel_validation`: the
/// vendored metamodel document validated against itself.
#[test]
fn valid_metamodel_validation() {
    let metamodel: Value =
        serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored AST is JSON");
    let result = validate_metamodel(&metamodel);
    assert!(
        result.is_ok(),
        "metamodel validation should succeed: {result:?}"
    );
}

/// concerto-validate-rs `tests::test_invalid_json`: `validate_metamodel`
/// takes a parsed [`Value`], so a document with no usable `$class` is the
/// nearest equivalent, and fails `from_json`'s "no `$class`" check.
#[test]
fn invalid_json_has_no_usable_class() {
    let result = validate_metamodel(&json!({}));
    assert!(
        result.is_err(),
        "a $class-less document should fail: {result:?}"
    );
}

/// concerto-validate-rs `tests::test_invalid_namespace`: a `namespace` of
/// the wrong JSON type fails validation.
#[test]
fn invalid_namespace_type() {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": 123
    });
    let result = validate_metamodel(&ast);
    assert!(
        result.is_err(),
        "a non-string namespace should fail: {result:?}"
    );
}

/// concerto-validate-rs `tests::test_missing_class_property`: a document
/// with no `$class` at all fails validation.
#[test]
fn missing_class_property() {
    let ast = json!({
        "namespace": "test.namespace",
        "imports": [],
        "declarations": []
    });
    let result = validate_metamodel(&ast);
    assert!(
        result.is_err(),
        "a $class-less document should fail: {result:?}"
    );
}

/// concerto-validate-rs `tests::test_simple_model_validation`: a small,
/// well-formed model passes validation.
#[test]
fn simple_model_validation() {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "test.namespace@1.0.0",
        "imports": [],
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "TestConcept",
                "isAbstract": false,
                "properties": [
                    {
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "testField",
                        "isArray": false,
                        "isOptional": false
                    }
                ]
            }
        ]
    });
    let result = validate_metamodel(&ast);
    assert!(
        result.is_ok(),
        "a simple valid model should pass validation: {result:?}"
    );
}

/// concerto-validate-rs `tests::test_extra_properties`: an undeclared
/// non-null property anywhere fails validation, under the default options
/// too (`validate_properties`, not the strict preset).
#[test]
fn extra_properties_rejected_under_the_strict_preset() {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "test.namespace@1.0.0",
        "imports": [],
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "TestConcept",
                "isAbstract": false,
                "isOptional": false,
                "properties": [
                    {
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "testField",
                        "isArray": false,
                        "isOptional": false,
                        "propertyType": "String"
                    }
                ]
            }
        ]
    });
    let result = validate_metamodel(&ast);
    assert!(
        result.is_err(),
        "extra properties should fail validation under the strict preset: {result:?}"
    );
}

/// The strict preset's own effect: an unknown property set to `null` is
/// ignored by default and rejected only with `reject_unknown_keys`. Fails
/// if `validate_metamodel` stops applying `STRICT_VALIDATE_OPTIONS`.
#[test]
fn null_extra_property_rejected_only_under_the_strict_preset() {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "test.namespace@1.0.0",
        "imports": [],
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "TestConcept",
                "isAbstract": false,
                "unknownKey": null,
                "properties": []
            }
        ]
    });

    let strict_result = validate_metamodel(&ast);
    assert!(
        strict_result.is_err(),
        "a null unknown property should be rejected by validate_metamodel, \
             which applies the strict preset: {strict_result:?}"
    );

    let mm = metamodel_model_manager().expect("metamodel model manager");
    let default_result = from_json(&mm, &ast, &FromJsonOptions::default(), &mut FixedEnv);
    assert!(
        default_result.is_ok(),
        "the same null unknown property should be ignored under the \
             (non-strict) default options: {default_result:?}"
    );
}

// ---- validateAst's own behaviour beyond validate_metamodel (not in
//      concerto-validate-rs, which has no version check at all) ----

#[test]
fn validate_ast_accepts_a_well_formed_model() {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": []
    });
    assert!(validate_ast(&ast).is_ok());
}

#[test]
fn validate_ast_rejects_an_unknown_metamodel_version() {
    let ast = json!({
        "$class": "concerto.metamodel@99.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "declarations": []
    });
    let err = validate_ast(&ast).expect_err("an unknown metamodel version should fail");
    let contract = err.contract().clone();
    assert_eq!(contract.kind, ErrorKind::Metamodel);
    assert_eq!(
        contract.message(),
        "Model file version 99.0.0 does not match metamodel version 1.0.0"
    );
}

#[test]
fn validate_ast_rejects_a_bad_metamodel_ast_with_an_undeclared_property() {
    // TS: modelmanager.js "#addModel > should throw for a bad metamodel
    // AST".
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "undeclared": []
    });
    assert!(validate_ast(&ast).is_err());
}

// ---- the resident metamodel manager ----

/// The structural check on a fresh metamodel manager.
fn validate_metamodel_on_a_fresh_manager(ast: &Value) -> Result<()> {
    let mm = metamodel_model_manager()?;
    let options = FromJsonOptions {
        reject_unknown_keys: true,
        reject_required_null: true,
        ..FromJsonOptions::default()
    };
    from_json(&mm, ast, &options, &mut FixedEnv)
        .map(|_resource| ())
        .map_err(|err| wrapped(&err))
}

fn outcome(result: Result<()>) -> Option<(ErrorKind, String)> {
    result.err().map(|err| (err.kind(), err.to_string()))
}

#[test]
fn validate_ast_on_the_resident_manager_matches_a_fresh_one_in_any_order() {
    let metamodel: Value = serde_json::from_str(METAMODEL_AST_JSON).unwrap();
    let cases = [
        metamodel.clone(),
        json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0", "undeclared": [] }),
        json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0", "imports": [], "declarations": [] }),
        json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": null, "declarations": [] }),
        json!({ "$class": "concerto.metamodel@1.0.0.Nope", "namespace": "org.acme@1.0.0" }),
        json!({ "$class": "concerto.metamodel@1.0.0.Model" }),
        json!({ "namespace": "org.acme@1.0.0" }),
        metamodel,
    ];
    // Twice over, so the second pass runs on a manager every case has
    // already been through.
    for _ in 0..2 {
        for ast in &cases {
            assert_eq!(
                outcome(validate_metamodel(ast)),
                outcome(validate_metamodel_on_a_fresh_manager(ast)),
                "{ast}"
            );
            let fresh =
                check_version(ast).and_then(|()| validate_metamodel_on_a_fresh_manager(ast));
            assert_eq!(outcome(validate_ast(ast)), outcome(fresh), "{ast}");
        }
    }
}

#[test]
fn validate_ast_on_the_resident_manager_is_per_thread() {
    let ok = json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0", "imports": [], "declarations": [] });
    let bad = json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0", "undeclared": [] });
    let expected = outcome(validate_metamodel_on_a_fresh_manager(&bad));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..3 {
                    assert!(validate_ast(&ok).is_ok());
                    assert_eq!(outcome(validate_ast(&bad)), expected);
                }
            });
        }
    });
}

// ---- ModelManager::validate_ast: `validateAst` on a caller's own
//      model manager, the `metamodelValidation` option ----

fn namespaces(mm: &ModelManager) -> Vec<String> {
    mm.model_files()
        .map(|mf| mf.namespace().to_string())
        .collect()
}

fn model_file(ast: &Value) -> ModelFile {
    ModelFile::from_json(ast, Some("test.cto".into())).expect("a well-formed model file")
}

#[test]
fn metamodel_validation_is_off_by_default_and_carried_by_scratch_copies() {
    let mut mm = ModelManager::new().unwrap();
    assert!(!mm.metamodel_validation());
    mm.set_metamodel_validation(true);
    assert!(mm.metamodel_validation());
    mm.load_model(
        &json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": []
        }),
        None,
    )
    .unwrap();
    assert!(
        mm.delete_model_file("org.acme@1.0.0")
            .unwrap()
            .metamodel_validation()
    );
}

#[test]
fn manager_validate_ast_accepts_a_well_formed_model_and_removes_the_metamodel() {
    let mut mm = ModelManager::new().unwrap();
    let before = namespaces(&mm);
    let mf = model_file(&json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": [{
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Person",
            "isAbstract": false,
            "properties": [{
                "$class": "concerto.metamodel@1.0.0.StringProperty",
                "name": "name",
                "isArray": false,
                "isOptional": false
            }]
        }]
    }));
    mm.validate_ast(&mf).unwrap();
    assert_eq!(namespaces(&mm), before);
    assert!(
        mm.declaration_id("concerto.metamodel@1.0.0.Model")
            .is_none()
    );
}

#[test]
fn manager_validate_ast_failure_leaves_the_metamodel_registered() {
    // TS: `validateAst`'s `deleteModelFile(MetaModelNamespace)` follows
    // the `try`/`catch` that re-throws, so a failed check never reaches it.
    let mut mm = ModelManager::new().unwrap();
    // The loader refuses an unknown key, so the model file holds a
    // malformation the loader does not check (a fraction in an
    // `Integer` field; `typed_ast`'s module doc, "Not checked").
    let position = json!({"$class": "concerto.metamodel@1.0.0.Position", "line": 1.5, "column": 1, "offset": 0});
    let mf = model_file(&json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": [{
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Thing",
            "isAbstract": false,
            "properties": [],
            "location": {"$class": "concerto.metamodel@1.0.0.Range", "start": position, "end": position}
        }]
    }));
    let err = mm
        .validate_ast(&mf)
        .expect_err("a fraction in an Integer field is invalid");
    let contract = err.contract().clone();
    assert_eq!(contract.kind, ErrorKind::Metamodel);
    let mut expected: Vec<String> = ModelManager::new().map(|fresh| namespaces(&fresh)).unwrap();
    expected.push(METAMODEL_NAMESPACE.to_string());
    assert_eq!(namespaces(&mm), expected);
    assert_eq!(
        mm.model_file(METAMODEL_NAMESPACE).unwrap().file_name(),
        Some(METAMODEL_NAMESPACE)
    );
    // A later check finds it already there and keeps it.
    let valid = model_file(&json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.other@1.0.0",
        "imports": [],
        "declarations": []
    }));
    mm.validate_ast(&valid).unwrap();
    assert_eq!(namespaces(&mm), expected);
}

// ---- ModelManager::validate_ast_value: the check over the AST alone,
//      on the resident metamodel manager where that is exact ----

/// An AST the `ModelFile` constructor rejects (no `namespace`,
/// `declarations` not an array) reaches the structural check, which
/// throws TS's `MetamodelException` and leaves the metamodel
/// registered, as TS 5.0.0's `validateAst` does.
#[test]
fn manager_validate_ast_value_reports_a_model_file_shape_error_as_a_metamodel_error() {
    for ast in [
        json!({ "$class": "concerto.metamodel@1.0.0.Model", "imports": [], "declarations": [] }),
        json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": "not an array"
        }),
    ] {
        assert!(ModelFile::from_json(&ast, None).is_err());
        let mut mm = ModelManager::new().unwrap();
        let err = mm.validate_ast_value(&ast).expect_err("the check fails");
        assert_eq!(kind_of(&err), Some(ErrorKind::Metamodel));
        assert!(mm.model_file(METAMODEL_NAMESPACE).is_some());
    }
}

/// A pass on the resident metamodel leaves the caller's manager exactly
/// as it was: no namespace, no mutation counted.
#[test]
fn manager_validate_ast_value_pass_leaves_the_manager_unchanged() {
    let mut mm = ModelManager::new().unwrap();
    let (before, state_version) = (namespaces(&mm), mm.state_version());
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": []
    });
    for _ in 0..2 {
        mm.validate_ast_value(&ast).unwrap();
        assert_eq!(namespaces(&mm), before);
        assert_eq!(mm.state_version(), state_version);
    }
}

/// A document typed by the caller's own model is resolved against the
/// caller's manager, as TS's `getSerializer().fromJSON` does: it fails
/// on the resident metamodel manager, which does not hold that type, so
/// the check runs on the caller's manager, where it passes, and the
/// metamodel is removed again.
#[test]
fn manager_validate_ast_value_resolves_the_callers_own_types() {
    let mut mm = ModelManager::new().unwrap();
    mm.load_model(
        &json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "properties": []
            }]
        }),
        None,
    )
    .unwrap();
    let before = namespaces(&mm);
    mm.validate_ast_value(&json!({ "$class": "org.acme@1.0.0.Person" }))
        .unwrap();
    assert_eq!(namespaces(&mm), before);
}

/// A manager without the system models (`ModelManager::default()`) does
/// not match the resident manager's, so its own check runs: the
/// metamodel's declarations cannot resolve their implicit `Concept`
/// super type there.
#[test]
fn manager_validate_ast_value_without_system_models_checks_the_manager_itself() {
    let ast = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": []
    });
    assert!(
        ModelManager::new()
            .unwrap()
            .validate_ast_value(&ast)
            .is_ok()
    );
    let mut bare = ModelManager::default();
    let err = bare
        .validate_ast_value(&ast)
        .expect_err("no system models to resolve against");
    assert_eq!(kind_of(&err), Some(ErrorKind::Metamodel));
    assert!(bare.model_file(METAMODEL_NAMESPACE).is_some());
}

#[test]
fn manager_validate_ast_version_mismatch_adds_nothing() {
    let mut mm = ModelManager::new().unwrap();
    let before = namespaces(&mm);
    let mf = model_file(&json!({
        "$class": "concerto.metamodel@99.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": []
    }));
    let err = mm
        .validate_ast(&mf)
        .expect_err("an unknown metamodel version");
    let contract = err.contract().clone();
    assert_eq!(
        contract.message(),
        "Model file version 99.0.0 does not match metamodel version 1.0.0"
    );
    assert_eq!(namespaces(&mm), before);
}

/// `validateAst` on an AST whose `$class` `getNamespace` cannot use:
/// the error, and nothing added to the manager. Expected values are the
/// frozen reference's (concerto-core 5.0.0, `new ModelManager({
/// metamodelValidation: true }).addModelFile(new ModelFile(mm, ast))`).
fn assert_bad_class(class: Option<Value>, kind: ErrorKind, message: &str) {
    let mut ast = json!({
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": []
    });
    if let Some(class) = class {
        ast["$class"] = class;
    }
    for err in [
        validate_ast(&ast).expect_err("the standalone check fails"),
        {
            let mut mm = ModelManager::new().unwrap();
            let before = namespaces(&mm);
            let err = mm
                .validate_ast(&model_file(&ast))
                .expect_err("the manager check fails");
            assert_eq!(namespaces(&mm), before, "nothing is added");
            err
        },
    ] {
        let contract = err.contract().clone();
        assert_eq!(contract.kind, kind);
        assert_eq!(contract.message(), message);
    }
}

#[test]
fn validate_ast_without_a_class_is_an_invalid_fqn() {
    assert_bad_class(None, ErrorKind::InvalidArgument, "FQN is invalid.");
    assert_bad_class(
        Some(json!(null)),
        ErrorKind::InvalidArgument,
        "FQN is invalid.",
    );
    assert_bad_class(
        Some(json!("")),
        ErrorKind::InvalidArgument,
        "FQN is invalid.",
    );
    assert_bad_class(
        Some(json!(0)),
        ErrorKind::InvalidArgument,
        "FQN is invalid.",
    );
    assert_bad_class(
        Some(json!(false)),
        ErrorKind::InvalidArgument,
        "FQN is invalid.",
    );
}

#[test]
fn validate_ast_with_a_non_string_class_is_a_type_error() {
    for class in [json!(5), json!(true), json!({})] {
        assert_bad_class(
            Some(class),
            ErrorKind::MalformedInput,
            "fqn.lastIndexOf is not a function",
        );
    }
    assert_bad_class(
        Some(json!(["."])),
        ErrorKind::MalformedInput,
        "fqn.substr is not a function",
    );
    assert_bad_class(
        Some(json!(["concerto.metamodel@1.0.0.Model"])),
        ErrorKind::InvalidArgument,
        "Namespace is null or undefined.",
    );
}

#[test]
fn validate_ast_with_an_unversioned_class_reports_version_null() {
    assert_bad_class(
        Some(json!("concerto.metamodel.Model")),
        ErrorKind::Metamodel,
        "Model file version null does not match metamodel version 1.0.0",
    );
}

// ---- `addMetamodel`, `validateMetaModel` and
//      `modelManagerFromMetaModel` ----

fn kind_of(err: &Error) -> Option<ErrorKind> {
    Some(err.contract().kind)
}

fn person_models() -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": [{
            "$class": "concerto.metamodel@1.0.0.Model",
            "decorators": [],
            "namespace": "test.person@1.0.0",
            "imports": [],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "name",
                    "isArray": false,
                    "isOptional": false
                }]
            }]
        }]
    })
}

#[test]
fn add_metamodel_registers_the_metamodel_under_its_namespace() {
    // TS: `new ModelManager({ addMetamodel: true })` adds
    // `this.metamodelModelFile` last, named after its namespace.
    for metamodel_validation in [false, true] {
        let mut mm = ModelManager::new().unwrap();
        mm.set_metamodel_validation(metamodel_validation);
        mm.add_metamodel().expect("the metamodel loads");
        assert_eq!(
            namespaces(&mm).last().map(String::as_str),
            Some(METAMODEL_NAMESPACE)
        );
        let file = mm.model_file(METAMODEL_NAMESPACE).unwrap();
        assert_eq!(file.file_name(), Some(METAMODEL_NAMESPACE));
        assert!(mm.get_declaration("concerto.metamodel@1.0.0.Model").is_ok());
    }
}

#[test]
fn add_metamodel_twice_is_the_already_exists_error() {
    let mut mm = ModelManager::new().unwrap();
    mm.add_metamodel().unwrap();
    let before = namespaces(&mm);
    assert!(mm.add_metamodel().is_err());
    assert_eq!(namespaces(&mm), before, "nothing is added");
}

#[test]
fn validate_meta_model_instance_accepts_a_metamodel_document() {
    validate_meta_model_instance(&person_models()).expect("a valid Models document");
    let model = person_models()["models"][0].clone();
    validate_meta_model_instance(&model).expect("a valid Model document");
}

#[test]
fn validate_meta_model_instance_does_not_wrap_the_serializer_error() {
    // TS `validateMetaModel` throws `serializer.fromJSON`'s own error;
    // only `validateAst` re-throws it as a `MetamodelException`.
    let mut bad = person_models();
    bad["models"][0]["namespace"] = json!(42);
    let err = validate_meta_model_instance(&bad).expect_err("a bad namespace fails");
    assert_ne!(kind_of(&err), Some(ErrorKind::Metamodel));
    assert!(validate_metamodel(&bad).is_err());
}

#[test]
fn model_manager_from_meta_model_loads_every_model() {
    for validate in [true, false] {
        let mm = model_manager_from_meta_model(&person_models(), validate).unwrap();
        assert_eq!(
            namespaces(&mm).last().map(String::as_str),
            Some("test.person@1.0.0")
        );
        let file = mm.model_file("test.person@1.0.0").unwrap();
        assert_eq!(file.file_name(), None);
        assert!(mm.get_declaration("test.person@1.0.0.Person").is_ok());
    }
}

#[test]
fn model_manager_from_meta_model_checks_the_shape_even_without_validate() {
    // Structurally invalid (an undeclared property), semantically fine.
    // `validate` runs `validateMetaModel` over the whole document first;
    // without it, BC-19 still rejects the model when its `ModelFile` is
    // built, with an `IllegalModelException`.
    let mut doc = person_models();
    doc["models"][0]["undeclared"] = json!(true);
    let err = model_manager_from_meta_model(&doc, true).expect_err("validateMetaModel");
    assert_ne!(kind_of(&err), Some(ErrorKind::IllegalModel));
    let err = model_manager_from_meta_model(&doc, false).expect_err("the load check");
    assert_eq!(kind_of(&err), Some(ErrorKind::IllegalModel));
    assert_eq!(
        err.contract().message(),
        "Model AST does not conform to the metamodel: Unexpected properties for type concerto.metamodel@1.0.0.Model: undeclared"
    );
}

// ---- `check_ast_shape` (BC-19, with BC-17 and BC-20) ----

fn person_model() -> Value {
    person_models()["models"][0].clone()
}

fn shape_code(ast: &Value) -> Option<&'static str> {
    check_ast_shape(ast).err().map(|err| {
        let contract = err.contract();
        assert_eq!(contract.kind, ErrorKind::IllegalModel, "{ast}");
        contract.code
    })
}

#[test]
fn check_ast_shape_accepts_well_formed_models() {
    assert_eq!(shape_code(&person_model()), None);
    let metamodel: Value = serde_json::from_str(METAMODEL_AST_JSON).unwrap();
    assert_eq!(shape_code(&metamodel), None);
    let mut with_super = person_model();
    with_super["declarations"][0]["superType"] =
        json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base"});
    with_super["declarations"][0]["decorators"] = json!([]);
    assert_eq!(shape_code(&with_super), None);
}

#[test]
fn check_ast_shape_rejects_a_non_array_decorators_value() {
    // BC-17: TS iterates a string by code unit and ignores a number.
    for decorators in [
        json!("💥emoji"),
        json!("x"),
        json!(""),
        json!(5),
        json!(true),
        json!({}),
    ] {
        let mut model = person_model();
        model["declarations"][0]["properties"][0]["decorators"] = decorators.clone();
        assert_eq!(
            shape_code(&model),
            Some("modelfile-load-decoratorsnotarray"),
            "{decorators}"
        );
    }
    let mut model = person_model();
    model["decorators"] = json!("x");
    let err = check_ast_shape(&model).unwrap_err();
    assert_eq!(
        err.contract().message(),
        "Invalid decorators. Expected array. Found \"x\""
    );
}

#[test]
fn check_ast_shape_tolerates_the_parsers_date_time_default() {
    // concerto-cto 5.0.0 writes `defaultValue` on a `DateTimeProperty`,
    // which the metamodel does not declare.
    let date_time = |default: Value| {
        let mut model = person_model();
        model["declarations"][0]["properties"][0] = json!({
            "$class": "concerto.metamodel@1.0.0.DateTimeProperty",
            "name": "born",
            "isArray": false,
            "isOptional": false,
            "defaultValue": default
        });
        model
    };
    let parsed = date_time(json!("2020-01-01T00:00:00Z"));
    assert_eq!(shape_code(&parsed), None);
    assert!(
        validate_ast(&parsed).is_err(),
        "validateAst itself rejects it"
    );
    assert_eq!(
        shape_code(&date_time(json!(5))),
        Some("modelfile-load-astshape")
    );
    // Only on a `DateTimeProperty`.
    let mut enum_value = person_model();
    enum_value["declarations"][0]["properties"][0]["$class"] =
        json!("concerto.metamodel@1.0.0.EnumProperty");
    enum_value["declarations"][0]["properties"][0]["defaultValue"] = json!("x");
    assert_eq!(shape_code(&enum_value), Some("modelfile-load-astshape"));
}

#[test]
fn check_ast_shape_requires_a_node_for_identified_and_the_validators() {
    // The metamodel check alone accepts a value with no own keys, or an
    // object without a `$class`, for these four keys.
    let identified = |value: Value| {
        let mut model = person_model();
        model["declarations"][0]["identified"] = value;
        model
    };
    let validator = |key: &str, value: Value| {
        let mut model = person_model();
        model["declarations"][0]["properties"][0][key] = value;
        model
    };
    let bad = [
        json!(0),
        json!(1),
        json!(true),
        json!(""),
        json!([]),
        json!({}),
    ];
    for value in bad.iter().cloned().chain([json!({"name": "name"})]) {
        assert_eq!(
            shape_code(&identified(value.clone())),
            Some("modelfile-load-nodenotobject"),
            "identified {value}"
        );
    }
    for key in ["sizeValidator", "lengthValidator", "validator"] {
        for value in bad.iter().cloned().chain([
            json!({"minLength": 1}),
            json!({"pattern": "a", "flags": ""}),
        ]) {
            assert_eq!(
                shape_code(&validator(key, value.clone())),
                Some("modelfile-load-nodenotobject"),
                "{key} {value}"
            );
        }
        assert_eq!(shape_code(&validator(key, Value::Null)), None, "{key} null");
    }
    assert_eq!(shape_code(&identified(Value::Null)), None);
    assert_eq!(
        shape_code(&identified(
            json!({"$class": "concerto.metamodel@1.0.0.Identified"})
        )),
        None
    );
    assert_eq!(
        shape_code(&validator(
            "validator",
            json!({"$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "a", "flags": ""})
        )),
        None
    );
    // A `$class` names the node's type, which the metamodel check checks.
    assert_eq!(
        shape_code(&validator(
            "validator",
            json!({"$class": "x", "pattern": "a", "flags": ""})
        )),
        Some("modelfile-load-astshape")
    );
}

#[test]
fn check_ast_shape_rejects_non_string_names() {
    // BC-20: TS coerces a name with `String()`.
    for name in [json!(1e308), json!(0), json!(false), json!(null), json!({})] {
        let mut model = person_model();
        model["declarations"][0]["name"] = name.clone();
        assert_eq!(
            shape_code(&model),
            Some("modelfile-load-namenotstring"),
            "{name}"
        );
    }
}

#[test]
fn check_ast_shape_rejects_an_empty_or_non_string_super_type_name() {
    // BC-20: `superType.name` of `""`, `0` or `false` gives TS's
    // "Could not find super type 0".
    for name in [json!(""), json!(0), json!(false), json!(null)] {
        let mut model = person_model();
        model["declarations"][0]["superType"] =
            json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": name});
        assert_eq!(
            shape_code(&model),
            Some("modelfile-load-supertypename"),
            "{name}"
        );
    }
    // `superType: {}` has no name: the metamodel check's error.
    let mut model = person_model();
    model["declarations"][0]["superType"] = json!({});
    assert_eq!(shape_code(&model), Some("modelfile-load-astshape"));
}

#[test]
fn check_ast_shape_rejects_what_the_metamodel_rejects() {
    // BC-19: an unknown property, a wrong-typed field and another
    // metamodel version (a malformed `identified` is step 1's,
    // `check_ast_shape_requires_a_node_for_identified_and_the_validators`).
    let mut unknown = person_model();
    unknown["undeclared"] = json!([]);
    let mut bounds = person_model();
    bounds["declarations"][0]["properties"][0] = json!({
        "$class": "concerto.metamodel@1.0.0.IntegerProperty",
        "name": "age",
        "isArray": false,
        "isOptional": false,
        "validator": {"$class": "concerto.metamodel@1.0.0.IntegerDomainValidator", "lower": "0"}
    });
    // DV-017's typeless relationship and DV-018's `null` decorator: the
    // metamodel check rejects both first, so those rows' own errors are
    // raised only with the check off.
    let mut relationship = person_model();
    relationship["declarations"][0]["properties"][0] = json!({
        "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
        "name": "home",
        "isArray": false,
        "isOptional": false
    });
    let mut null_decorator = person_model();
    null_decorator["declarations"][0]["decorators"] = json!([null]);
    for ast in [unknown, bounds, relationship, null_decorator] {
        assert_eq!(shape_code(&ast), Some("modelfile-load-astshape"), "{ast}");
    }
    // R2F-5: a metamodel version mismatch stays TS 5.0.0's
    // `MetamodelException`, not re-thrown.
    let mut version = person_model();
    version["$class"] = json!("concerto.metamodel@99.0.0.Model");
    let err = check_ast_shape(&version).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Metamodel);
    assert_eq!(err.code(), "basemodelmanager-validateast-versionmismatch");
    assert_eq!(
        err.contract().message(),
        "Model file version 99.0.0 does not match metamodel version 1.0.0"
    );
}

/// R2A-4: the default super type TS 5.0.0's `ModelFile.filter` copies
/// from a declaration's AST (`TypeIdentified`) is checked as a
/// `TypeIdentifier`, on the declaration kind it belongs to only.
#[test]
fn check_ast_shape_tolerates_a_filtered_default_super_type() {
    let default = |class: &str, name: &str| {
        json!({ "$class": class, "name": "A", "isAbstract": false, "properties": [],
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentified", "name": name } })
    };
    let mut model = person_model();
    model["declarations"] = json!([default(
        "concerto.metamodel@1.0.0.AssetDeclaration",
        "Asset"
    ),]);
    model["declarations"][0]["identified"] =
        json!({ "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" });
    model["declarations"][0]["properties"] = json!([{ "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "id", "isArray": false, "isOptional": false }]);
    assert_eq!(shape_code(&model), None);
    // Not on a concept, nor naming another type.
    let mut concept = person_model();
    concept["declarations"][0]["superType"] =
        json!({ "$class": "concerto.metamodel@1.0.0.TypeIdentified", "name": "Concept" });
    assert_eq!(shape_code(&concept), Some("modelfile-load-astshape"));
    let mut other = model.clone();
    other["declarations"][0]["superType"]["name"] = json!("Participant");
    assert_eq!(shape_code(&other), Some("modelfile-load-astshape"));
}

#[test]
fn check_ast_shape_reports_the_first_problem_in_document_order() {
    let mut model = person_model();
    model["declarations"][0]["name"] = json!(7);
    model["declarations"][0]["properties"][0]["decorators"] = json!("x");
    assert_eq!(shape_code(&model), Some("modelfile-load-namenotstring"));
    // Steps 1 (BC-17, BC-20) before step 2 (the metamodel check).
    let mut model = person_model();
    model["undeclared"] = json!(true);
    model["declarations"][0]["decorators"] = json!(1);
    assert_eq!(
        shape_code(&model),
        Some("modelfile-load-decoratorsnotarray")
    );
}

#[test]
fn model_manager_from_meta_model_keeps_the_error_for_a_non_object_model() {
    // `new ModelFile(mm, null)`: the constructor's own check, not BC-19.
    let doc = json!({"models": [null]});
    let err = model_manager_from_meta_model(&doc, false).expect_err("a null model");
    assert_ne!(kind_of(&err), Some(ErrorKind::IllegalModel));
}

#[test]
fn model_manager_from_meta_model_rejects_a_semantically_invalid_model() {
    // The type `Missing` is never declared: `addModelFile` validates.
    let mut doc = person_models();
    doc["models"][0]["declarations"][0]["properties"] = json!([{
        "$class": "concerto.metamodel@1.0.0.ObjectProperty",
        "name": "other",
        "type": {"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing"},
        "isArray": false,
        "isOptional": false
    }]);
    assert!(model_manager_from_meta_model(&doc, false).is_err());
}

#[test]
fn model_manager_from_meta_model_without_models_is_a_type_error() {
    for doc in [json!({}), json!({"models": null}), json!(null)] {
        let err = model_manager_from_meta_model(&doc, false).expect_err("no models array");
        assert_eq!(kind_of(&err), Some(ErrorKind::MalformedInput), "{doc}");
    }
    let err = model_manager_from_meta_model(&json!({"models": "x"}), false)
        .expect_err("models is not an array");
    assert_eq!(kind_of(&err), Some(ErrorKind::MalformedInput));
}

#[test]
fn model_manager_from_meta_model_rejects_a_duplicate_namespace() {
    let mut doc = person_models();
    let model = doc["models"][0].clone();
    doc["models"].as_array_mut().unwrap().push(model);
    assert!(model_manager_from_meta_model(&doc, false).is_err());
}

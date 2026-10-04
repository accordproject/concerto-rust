use super::*;
use serde_json::json;

/// `org.acme@1.0.0` with a single `Person { name: String }`.
fn sample_manager() -> ModelManager {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
        &json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false,
                  "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                  ] }
            ]
        }),
        None,
    )
    .unwrap();
    mgr
}

#[test]
fn falsy_or_equal_treats_absent_null_and_empty_string_as_true() {
    assert!(falsy_or_equal(None, &["x"]));
    assert!(falsy_or_equal(Some(&Value::Null), &["x"]));
    assert!(falsy_or_equal(Some(&json!("")), &["x"]));
}

#[test]
fn falsy_or_equal_matches_a_string_by_membership() {
    assert!(falsy_or_equal(Some(&json!("x")), &["x", "y"]));
    assert!(!falsy_or_equal(Some(&json!("z")), &["x", "y"]));
}

#[test]
fn falsy_or_equal_matches_an_array_by_intersection() {
    assert!(falsy_or_equal(Some(&json!(["z", "y"])), &["x", "y"]));
    assert!(!falsy_or_equal(Some(&json!(["z", "w"])), &["x", "y"]));
    // The intersection's own rules, without building it: an empty
    // array (truthy) intersects nothing, a non-string element is never
    // in the string array, and a repeated one counts once.
    assert!(!falsy_or_equal(Some(&json!([])), &["x"]));
    assert!(!falsy_or_equal(
        Some(&json!([1, null, true])),
        &["1", "null", "true"]
    ));
    assert!(falsy_or_equal(Some(&json!([1, "y", "y"])), &["y"]));
}

/// The compile-time class names are the namespace's.
#[test]
fn metamodel_class_names_are_qualified_by_the_metamodel_namespace() {
    use crate::instance::metamodel::METAMODEL_NAMESPACE;
    assert_eq!(
        MAP_DECLARATION_CLASS,
        model_util::qualify(METAMODEL_NAMESPACE, "MapDeclaration")
    );
    assert_eq!(
        IMPORT_TYPE_CLASS,
        model_util::qualify(METAMODEL_NAMESPACE, "ImportType")
    );
}

/// The validation manager shares the metamodel, the DCS model and the
/// caller's files, and parses none of them again.
#[test]
fn validate_shares_every_model_file() {
    let sample = sample_manager();
    let files: Vec<Arc<ModelFile>> = sample
        .shared_model_files()
        .filter(|mf| mf.namespace() == "org.acme@1.0.0")
        .cloned()
        .collect();
    let mgr = validate(&valid_command_set(), Some(&files)).unwrap();
    let shared = |namespace: &str| {
        mgr.shared_model_files()
            .find(|mf| mf.namespace() == namespace)
            .cloned()
            .unwrap()
    };
    assert!(Arc::ptr_eq(&shared("org.acme@1.0.0"), &files[0]));
    assert!(Arc::ptr_eq(
        &shared(crate::instance::metamodel::METAMODEL_NAMESPACE),
        &crate::instance::metamodel::metamodel_model_file().unwrap()
    ));
    let dcs = shared("org.accordproject.decoratorcommands@0.4.0");
    assert!(Arc::ptr_eq(
        &dcs,
        &dcs_model_file(VALIDATE_DCS_FILE_NAME).unwrap()
    ));
    assert_eq!(dcs.file_name(), Some(VALIDATE_DCS_FILE_NAME));
    // The files keep TS's order: system models, metamodel, the
    // caller's, then the DCS model.
    let order: Vec<&str> = mgr.model_files().map(ModelFile::namespace).collect();
    assert_eq!(
        order[order.len() - 3..],
        [
            crate::instance::metamodel::METAMODEL_NAMESPACE,
            "org.acme@1.0.0",
            "org.accordproject.decoratorcommands@0.4.0"
        ]
    );
    // `migrateAndValidate` names the DCS model file its own way.
    let migrate = dcs_model_file(MIGRATE_DCS_FILE_NAME).unwrap();
    assert_eq!(migrate.file_name(), Some(MIGRATE_DCS_FILE_NAME));
    assert!(!Arc::ptr_eq(&migrate, &dcs));
}

/// An invalid model file given to `validate` is still validated,
/// and fails as before.
#[test]
fn validate_still_validates_the_callers_model_files() {
    let mut broken = ModelManager::new().unwrap();
    broken
        .load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.broken@1.0.0",
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "A",
                    "isAbstract": false,
                    "superType": {
                        "$class": "concerto.metamodel@1.0.0.TypeIdentifier",
                        "name": "Missing"
                    },
                    "properties": []
                }]
            }),
            None,
        )
        .unwrap();
    let files: Vec<Arc<ModelFile>> = broken
        .shared_model_files()
        .filter(|mf| mf.namespace() == "org.broken@1.0.0")
        .cloned()
        .collect();
    let err = validate(&valid_command_set(), Some(&files)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::IllegalModel, "{err}");
}

/// An empty decorate shares the input's model files and options.
#[test]
fn decorate_with_no_command_sets_shares_the_model_files() {
    let sample = sample_manager();
    let same = decorate_models(&sample, &mut [], &mut DecorateOptions::default()).unwrap();
    let mine: Vec<&Arc<ModelFile>> = sample.shared_model_files().collect();
    let theirs: Vec<&Arc<ModelFile>> = same.shared_model_files().collect();
    assert_eq!(mine.len(), theirs.len());
    for (a, b) in mine.iter().zip(&theirs) {
        assert!(Arc::ptr_eq(a, b), "{}", a.namespace());
    }
}

#[test]
fn migrate_to_rewrites_only_the_decoratorcommands_class_version() {
    let mut value = json!({
        "$class": "org.accordproject.decoratorcommands@0.3.0.DecoratorCommandSet",
        "commands": [
            { "$class": "org.accordproject.decoratorcommands@0.3.0.Command", "type": "UPSERT" }
        ],
        "unrelated": { "$class": "concerto.metamodel@1.0.0.Decorator" }
    });
    migrate_to(&mut value).unwrap();
    assert_eq!(
        value["$class"],
        "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet"
    );
    assert_eq!(
        value["commands"][0]["$class"],
        "org.accordproject.decoratorcommands@0.4.0.Command"
    );
    assert_eq!(
        value["unrelated"]["$class"],
        "concerto.metamodel@1.0.0.Decorator"
    );
}

#[test]
fn can_migrate_only_within_the_same_major_and_to_a_strictly_higher_minor() {
    let older =
        json!({ "$class": "org.accordproject.decoratorcommands@0.3.0.DecoratorCommandSet" });
    assert!(can_migrate(&older, DCS_VERSION).unwrap());

    let same = json!({ "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet" });
    assert!(!can_migrate(&same, DCS_VERSION).unwrap());

    let other_major =
        json!({ "$class": "org.accordproject.decoratorcommands@1.0.0.DecoratorCommandSet" });
    assert!(!can_migrate(&other_major, DCS_VERSION).unwrap());
}

#[test]
fn can_migrate_takes_strict_semver_only() {
    // BC-41: no leading `v` and no surrounding whitespace, with the
    // error `parseNamespace` throws for any invalid version.
    for class in [
        "org.accordproject.decoratorcommands@v0.3.0.DecoratorCommandSet",
        "org.accordproject.decoratorcommands@ 0.3.0.DecoratorCommandSet",
    ] {
        let err = can_migrate(&json!({ "$class": class }), DCS_VERSION)
            .err()
            .unwrap_or_else(|| panic!("{class} was accepted"));
        assert!(err.to_string().to_lowercase().contains("invalid"), "{err}");
    }
    // BC-02: an unversioned `$class` namespace is
    // `parseNamespace`'s invalid namespace, a plain
    // `Error`.
    let err = can_migrate(
        &json!({ "$class": "org.accordproject.decoratorcommands.DecoratorCommandSet" }),
        DCS_VERSION,
    )
    .unwrap_err();
    assert_eq!(err.contract().kind, ErrorKind::InvalidArgument, "{err}");
    // Components above 2^53 are compared exactly: 2^53 + 1 and 2^53
    // are the same `f64`, but different majors.
    let big = json!({
        "$class": "org.accordproject.decoratorcommands@9007199254740993.0.0.DecoratorCommandSet"
    });
    assert!(!can_migrate(&big, "9007199254740992.1.0").unwrap());
    assert!(can_migrate(&big, "9007199254740993.1.0").unwrap());
}

#[test]
fn check_for_duplicate_decorators_rejects_a_repeated_name() {
    let ast = json!({ "decorators": [ {"name": "Foo"}, {"name": "Foo"} ] });
    let err = match check_for_duplicate_decorators(&ast) {
        Ok(()) => panic!("expected a duplicate-decorator error"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("Duplicate decorator Foo"));
}

#[test]
fn check_for_duplicate_decorators_accepts_distinct_names() {
    let ast = json!({ "decorators": [ {"name": "Foo"}, {"name": "Bar"} ] });
    assert!(check_for_duplicate_decorators(&ast).is_ok());
}

#[test]
fn apply_decorator_upsert_replaces_by_name_or_adds_a_new_one() {
    let mut decorated = json!({ "decorators": [ {"name": "Foo", "arguments": [{"value": 1}]} ] });
    apply_decorator(
        &mut decorated,
        "UPSERT",
        &json!({"name": "Foo", "arguments": [{"value": 2}]}),
    )
    .unwrap();
    assert_eq!(decorated["decorators"].as_array().unwrap().len(), 1);
    assert_eq!(decorated["decorators"][0]["arguments"][0]["value"], 2);

    apply_decorator(&mut decorated, "UPSERT", &json!({"name": "Bar"})).unwrap();
    assert_eq!(decorated["decorators"].as_array().unwrap().len(), 2);
}

#[test]
fn apply_decorator_append_adds_then_rejects_the_duplicate_it_created() {
    let mut decorated = json!({ "decorators": [ {"name": "Foo"} ] });
    let err = match apply_decorator(&mut decorated, "APPEND", &json!({"name": "Foo"})) {
        Ok(()) => panic!("expected a duplicate-decorator error"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("Duplicate decorator Foo"));
    // TS applies (pushes) the decorator, then checks: the duplicate is
    // still there to see, not rolled back.
    assert_eq!(decorated["decorators"].as_array().unwrap().len(), 2);
}

#[test]
fn apply_decorator_rejects_an_unknown_command_type() {
    let mut decorated = json!({});
    let err = match apply_decorator(&mut decorated, "REMOVE", &json!({"name": "Foo"})) {
        Ok(()) => panic!("expected an unknown-command-type error"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("Unknown command type REMOVE"));
}

#[test]
fn get_decorator_maps_indexes_by_the_first_target_field_in_priority_order() {
    let commands = vec![
        json!({"target": {"type": "T"}}),
        json!({"target": {"property": "p"}}),
        json!({"target": {"properties": ["a", "b"]}}),
        json!({"target": {"mapElement": "KEY"}}),
        json!({"target": {"declaration": "D"}}),
        json!({"target": {"namespace": "N"}}),
        // `type` wins over `declaration` when a command's target sets both.
        json!({"target": {"type": "T2", "declaration": "D2"}}),
    ];
    let maps = get_decorator_maps(&commands).unwrap();
    assert_eq!(maps.type_commands.get("T").unwrap().len(), 1);
    assert_eq!(maps.property_commands.get("p").unwrap().len(), 1);
    assert_eq!(maps.property_commands.get("a").unwrap().len(), 1);
    assert_eq!(maps.property_commands.get("b").unwrap().len(), 1);
    assert_eq!(maps.map_element_commands.get("KEY").unwrap().len(), 1);
    assert_eq!(maps.declaration_commands.get("D").unwrap().len(), 1);
    assert_eq!(maps.namespace_commands.get("N").unwrap().len(), 1);
    assert_eq!(maps.type_commands.get("T2").unwrap().len(), 1);
    assert!(!maps.declaration_commands.contains_key("D2"));
}

/// R2C-4: TS picks a command's map by truthiness
/// (`!!decoratorCommand?.target?.property`), so an empty `property` or
/// `type` falls through to the next target field.
#[test]
fn get_decorator_maps_skips_falsy_target_fields_as_js_does() {
    let commands = vec![
        json!({"target": {"declaration": "Person", "property": ""}}),
        json!({"target": {"declaration": "Person", "type": ""}}),
        json!({"target": {"namespace": "N", "properties": []}}),
        // A truthy non-string claims the command, but no name matches it.
        json!({"target": {"type": 5, "declaration": "D"}}),
    ];
    let maps = get_decorator_maps(&commands).unwrap();
    assert_eq!(maps.declaration_commands.get("Person").unwrap().len(), 2);
    assert!(maps.property_commands.is_empty());
    assert!(maps.type_commands.is_empty());
    assert!(maps.namespace_commands.is_empty());
    assert!(!maps.declaration_commands.contains_key("D"));
}

/// R2C-4 repro 1: `{ declaration: "Person", property: "" }` decorates
/// `Person` in TS 5.0.0 (the command is filed under the declaration, and
/// `executeCommand` sees no truthy `property`).
#[test]
fn an_empty_target_property_decorates_the_declaration() {
    let mgr = sample_manager();
    for empty in ["property", "type"] {
        let mut target = json!({ "namespace": "org.acme@1.0.0", "declaration": "Person" });
        target[empty] = json!("");
        let mut command_set = json!({
            "commands": [{
                "type": "UPSERT",
                "target": target,
                "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important" }
            }]
        });
        let decorated = decorate_models(
            &mgr,
            std::slice::from_mut(&mut command_set),
            &mut DecorateOptions::default(),
        )
        .unwrap();
        let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
        assert_eq!(
            ast["declarations"][0]["decorators"][0]["name"], "Important",
            "{empty}"
        );
    }
}

/// R2C-4 repro 2: a truthy non-array `properties` (validation off, the
/// default) is TS's `TypeError` from `target.properties.forEach`.
#[test]
fn a_non_array_target_properties_is_a_type_error() {
    let mgr = sample_manager();
    let mut command_set = json!({
        "commands": [{
            "type": "UPSERT",
            "target": { "declaration": "Person", "properties": "name" },
            "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important" }
        }]
    });
    let err = decorate_models(
        &mgr,
        std::slice::from_mut(&mut command_set),
        &mut DecorateOptions::default(),
    )
    .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::MalformedInput, "{err}");
}

#[test]
fn validate_command_rejects_a_namespace_that_does_not_exist() {
    let mgr = sample_manager();
    let command = json!({ "target": { "namespace": "does.not.exist@1.0.0" } });
    let err = match validate_command(&mgr, &command) {
        Ok(()) => panic!("expected a namespace-does-not-exist error"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn validate_command_rejects_an_unversioned_target_namespace() {
    // BC-02: `org.acme` does not match the loaded
    // `org.acme@1.0.0`; it is `parseNamespace`'s invalid namespace,
    // a plain `Error`.
    let mgr = sample_manager();
    for target in [
        json!({ "namespace": "org.acme" }),
        json!({ "namespace": "org.acme", "declaration": "Person", "property": "name" }),
    ] {
        let err = validate_command(&mgr, &json!({ "target": target })).unwrap_err();
        assert_eq!(err.contract().kind, ErrorKind::InvalidArgument, "{err}");
        assert!(err.to_string().contains("Invalid namespace"), "{err}");
    }
}

#[test]
fn decorate_models_rejects_an_unversioned_target_namespace_without_validation() {
    // BC-02: applying the commands rejects an
    // unversioned `target.namespace` too, with or without
    // `validateCommands`; a versioned one still applies.
    let mgr = sample_manager();
    let command_set = |namespace: &str| {
        json!({
            "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
            "name": "x",
            "version": "1.0.0",
            "commands": [{
                "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
                "type": "UPSERT",
                "target": {
                    "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget",
                    "namespace": namespace,
                    "declaration": "Person"
                },
                "decorator": {
                    "$class": "concerto.metamodel@1.0.0.Decorator",
                    "name": "Hello",
                    "arguments": []
                }
            }]
        })
    };
    for validate in [false, true] {
        let mut options = DecorateOptions {
            validate,
            validate_commands: validate,
            ..Default::default()
        };
        let mut sets = [command_set("org.acme")];
        let err = decorate_models(&mgr, &mut sets, &mut options).unwrap_err();
        assert_eq!(err.contract().kind, ErrorKind::InvalidArgument, "{err}");
        assert!(err.to_string().contains("Invalid namespace"), "{err}");

        let mut sets = [command_set("org.acme@1.0.0")];
        let decorated = decorate_models(&mgr, &mut sets, &mut options).unwrap();
        let person = &decorated.model_file("org.acme@1.0.0").unwrap().ast()["declarations"][0];
        assert_eq!(
            person["decorators"][0]["name"], "Hello",
            "validate={validate}"
        );
    }
}

#[test]
fn validate_command_accepts_a_real_namespace_declaration_and_property() {
    let mgr = sample_manager();
    let command = json!({
        "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "property": "name" }
    });
    assert!(validate_command(&mgr, &command).is_ok());
}

#[test]
fn validate_command_rejects_a_property_that_does_not_exist() {
    let mgr = sample_manager();
    let command = json!({
        "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "property": "nope" }
    });
    let err = match validate_command(&mgr, &command) {
        Ok(()) => panic!("expected a property-does-not-exist error"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn validate_command_rejects_a_declaration_that_does_not_exist() {
    // `test/decoratormanager.js` "#validateCommand should detect invalid
    // target declaration": a namespace that resolves but a declaration
    // that does not, with *no* `property`/`properties` — TS's
    // `resolveType('DecoratorCommand.target.declaration', fqn)` still
    // runs and throws (this is what the missing declaration-resolution
    // check let through silently before this fix).
    let mgr = sample_manager();
    let command = json!({ "target": { "namespace": "org.acme@1.0.0", "declaration": "Missing" } });
    let err = match validate_command(&mgr, &command) {
        Ok(()) => panic!("expected a declaration-does-not-exist error"),
        Err(e) => e,
    };
    // TS: `No type "org.acme@1.0.0.Missing" in namespace "org.acme@1.0.0"
    // for "DecoratorCommand.target.declaration".` (golden catalogue text,
    // `modelmanager-resolvetype-notypeinnsforcontext`).
    assert_eq!(
        err.to_string(),
        "No type \"org.acme@1.0.0.Missing\" in namespace \"org.acme@1.0.0\" for \"DecoratorCommand.target.declaration\"."
    );
}

#[test]
fn validate_command_rejects_an_unrecognised_target_type() {
    let mgr = sample_manager();
    let command = json!({ "target": { "type": "concerto.metamodel@1.0.0.Foo" } });
    let err = match validate_command(&mgr, &command) {
        Ok(()) => panic!("expected an unrecognised-type error"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("Foo"), "{err}");
}

#[test]
fn validate_command_rejects_properties_containing_a_property_that_does_not_exist() {
    let mgr = sample_manager();
    let command = json!({
        "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "properties": ["name", "nope"] }
    });
    let err = match validate_command(&mgr, &command) {
        Ok(()) => panic!("expected a property-does-not-exist error"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[test]
fn validate_command_rejects_both_property_and_properties() {
    let mgr = sample_manager();
    let command = json!({ "target": { "property": "a", "properties": ["b"] } });
    let err = match validate_command(&mgr, &command) {
        Ok(()) => panic!("expected a property/properties conflict error"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("both property and properties"));
}

#[test]
fn decorate_models_applies_a_declaration_level_upsert() {
    let mgr = sample_manager();
    let mut command_set = json!({
        "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
        "name": "test",
        "version": "0.4.0",
        "commands": [{
            "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
            "type": "UPSERT",
            "target": { "namespace": "org.acme@1.0.0", "declaration": "Person" },
            "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important" }
        }]
    });
    let decorated = decorate_models(
        &mgr,
        std::slice::from_mut(&mut command_set),
        &mut DecorateOptions::default(),
    )
    .unwrap();
    let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
    let person = &ast["declarations"][0];
    let names: Vec<&str> = person["decorators"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Important"]);
}

#[test]
fn decorate_models_applies_a_property_level_upsert() {
    let mgr = sample_manager();
    let mut command_set = json!({
        "commands": [{
            "type": "UPSERT",
            "target": { "namespace": "org.acme@1.0.0", "declaration": "Person", "property": "name" },
            "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Required" }
        }]
    });
    let decorated = decorate_models(
        &mgr,
        std::slice::from_mut(&mut command_set),
        &mut DecorateOptions::default(),
    )
    .unwrap();
    let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
    let name_prop = &ast["declarations"][0]["properties"][0];
    assert_eq!(name_prop["decorators"][0]["name"], "Required");
}

#[test]
fn decorate_models_applies_a_bare_namespace_command_to_the_model_itself() {
    let mgr = sample_manager();
    let mut command_set = json!({
        "commands": [{
            "type": "UPSERT",
            "target": {
                "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget",
                "namespace": "org.acme@1.0.0"
            },
            "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Stamped" }
        }]
    });
    let decorated = decorate_models(
        &mgr,
        std::slice::from_mut(&mut command_set),
        &mut DecorateOptions::default(),
    )
    .unwrap();
    let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
    assert_eq!(ast["decorators"][0]["name"], "Stamped");
    // A bare namespace target has no `declaration`, so it does not also
    // land on `Person` (`checkForNamespaceTargetAndApplyDecorator`
    // requires `target.declaration`).
    assert!(ast["declarations"][0].get("decorators").is_none());
}

#[test]
fn decorate_models_with_no_command_sets_leaves_the_model_untouched() {
    let mgr = sample_manager();
    let decorated = decorate_models(&mgr, &mut [], &mut DecorateOptions::default()).unwrap();
    let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
    assert!(ast["declarations"][0].get("decorators").is_none());
}

#[test]
fn decorate_models_upsert_replaces_an_existing_decorator_of_the_same_name() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
        &json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false,
                  "decorators": [ { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important", "arguments": [] } ],
                  "properties": [] }
            ]
        }),
        None,
    )
    .unwrap();
    let mut command_set = json!({
        "commands": [{
            "type": "UPSERT",
            "target": { "namespace": "org.acme@1.0.0", "declaration": "Person" },
            "decorator": { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Important",
                "arguments": [{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "yes"}] }
        }]
    });
    let decorated = decorate_models(
        &mgr,
        std::slice::from_mut(&mut command_set),
        &mut DecorateOptions::default(),
    )
    .unwrap();
    let ast = decorated.model_file("org.acme@1.0.0").unwrap().ast();
    let decorators = ast["declarations"][0]["decorators"].as_array().unwrap();
    assert_eq!(decorators.len(), 1);
    assert_eq!(decorators[0]["arguments"][0]["value"], "yes");
}

fn valid_command_set() -> Value {
    json!({
        "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
        "name": "web",
        "version": "1.0.0",
        "commands": [{
            "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
            "type": "UPSERT",
            "target": { "namespace": "org.acme@1.0.0", "declaration": "Person" },
            "decorator": { "name": "Important" }
        }]
    })
}

/// `validate_against` on the manager `validate` built accepts and
/// rejects what `validate` does, with the same error.
#[test]
fn validate_against_matches_validate_on_its_own_manager() {
    let sample = sample_manager();
    let files: Vec<Arc<ModelFile>> = sample
        .shared_model_files()
        .filter(|mf| mf.namespace() == "org.acme@1.0.0")
        .cloned()
        .collect();
    let mgr = validate(&valid_command_set(), Some(&files)).unwrap();
    assert!(validate_against(&mgr, &valid_command_set()).is_ok());

    let mut unknown_type = valid_command_set();
    unknown_type["commands"][0]["type"] = json!("DELETE");
    let mut no_class = valid_command_set();
    no_class.as_object_mut().unwrap().remove("$class");
    let mut unknown_class = valid_command_set();
    unknown_class["$class"] = json!("org.acme@1.0.0.Missing");
    for bad in [
        unknown_type,
        no_class,
        unknown_class,
        json!({ "$class": 1 }),
    ] {
        let expected = validate(&bad, Some(&files)).unwrap_err().to_string();
        let actual = validate_against(&mgr, &bad).unwrap_err().to_string();
        assert_eq!(actual, expected, "{bad}");
    }
}

#[test]
fn migrate_and_validate_with_should_validate_false_accepts_a_structurally_invalid_set_unchanged() {
    // Matches the reference: `shouldValidateCommands` alone, with
    // `shouldValidate` false, runs no check at all (TS nests the whole
    // block, including the per-command loop, inside `if (shouldValidate)`).
    let mgr = sample_manager();
    let mut sets = [json!({ "name": "x", "version": "1.0.0" })]; // no "commands" at all
    assert!(migrate_and_validate(&mgr, &mut sets, false, false, true).is_ok());
}

#[test]
fn migrate_and_validate_with_should_validate_true_rejects_a_missing_commands_array() {
    let mgr = sample_manager();
    let mut sets = [json!({
        "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
        "name": "x",
        "version": "1.0.0"
    })];
    let err = migrate_and_validate(&mgr, &mut sets, false, true, false).unwrap_err();
    assert!(err.to_string().contains("commands"), "{err}");
}

#[test]
fn migrate_and_validate_with_should_validate_true_rejects_a_command_missing_target() {
    let mgr = sample_manager();
    let mut command_set = valid_command_set();
    command_set["commands"][0]
        .as_object_mut()
        .unwrap()
        .remove("target");
    let mut sets = [command_set];
    let err = migrate_and_validate(&mgr, &mut sets, false, true, false).unwrap_err();
    assert!(err.to_string().contains("target"), "{err}");
}

#[test]
fn migrate_and_validate_runs_command_validation_only_when_both_flags_are_set() {
    let mgr = sample_manager();
    // A structurally valid command whose target references a namespace
    // that does not exist: only `validate_command` (semantic) catches
    // this, and only when both `should_validate` and
    // `should_validate_commands` are true.
    let mut command_set = valid_command_set();
    command_set["commands"][0]["target"] = json!({ "namespace": "does.not.exist@1.0.0" });
    let mut sets = [command_set.clone()];
    assert!(migrate_and_validate(&mgr, &mut sets, false, true, false).is_ok());

    let mut sets = [command_set];
    let err = migrate_and_validate(&mgr, &mut sets, false, true, true).unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[test]
fn decorate_models_validate_option_rejects_a_structurally_invalid_command_set() {
    let mgr = sample_manager();
    let mut command_set = valid_command_set();
    command_set.as_object_mut().unwrap().remove("commands");
    let mut options = DecorateOptions {
        validate: true,
        ..Default::default()
    };
    let err =
        decorate_models(&mgr, std::slice::from_mut(&mut command_set), &mut options).unwrap_err();
    assert!(err.to_string().contains("commands"), "{err}");
}

#[test]
fn decorate_models_default_options_skip_the_structural_check_and_fail_as_js_does() {
    // `validate` defaults to `false` (`DecorateOptions::default()`), so
    // the command set is not checked against `DCS_MODEL` first — but TS
    // then flattens `commandSet.commands` (`undefined` here) into one
    // `undefined` command and reads `command.decorator` through it: a
    // `TypeError`, not a silently skipped command set.
    let mgr = sample_manager();
    let mut command_set = valid_command_set();
    command_set.as_object_mut().unwrap().remove("commands");
    let err = decorate_models(
        &mgr,
        std::slice::from_mut(&mut command_set),
        &mut DecorateOptions::default(),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Cannot read properties of undefined (reading 'decorator')"
    );
}

/// R2C-1: `decorateModels(mm, [{ $class: '...CommandTarget' }], { validate:
/// true, validateCommands: true })`. Every `CommandTarget` field is
/// optional, so `serializer.fromJSON` accepts the set, and TS 5.0.0 then
/// throws a `TypeError` from `commandSet.commands.forEach` (`commands` is
/// `undefined`). This used to panic (a WASM trap).
#[test]
fn validate_commands_over_a_command_set_of_another_dcs_type_is_a_type_error() {
    let mgr = sample_manager();
    let mut sets = [json!({
        "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget"
    })];
    let mut options = DecorateOptions {
        validate: true,
        validate_commands: true,
        ..Default::default()
    };
    let err = decorate_models(&mgr, &mut sets, &mut options).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::MalformedInput, "{err}");
    assert_eq!(
        err.to_string(),
        "Cannot read properties of undefined (reading 'forEach')"
    );
}

/// R2C-1, the other shapes `commands` can take on a set that validates as
/// some type other than `DecoratorCommandSet`: `null` is a `TypeError` for
/// reading `forEach`, a non-array value one for calling it.
#[test]
fn validate_commands_reads_commands_for_each_as_js_does() {
    for (commands, expected) in [
        (
            Value::Null,
            "Cannot read properties of null (reading 'forEach')",
        ),
        (json!("x"), "commandSet.commands.forEach is not a function"),
        (json!({}), "commandSet.commands.forEach is not a function"),
    ] {
        let err = super::js_for_each(Some(&commands), "commandSet.commands.forEach").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MalformedInput, "{err}");
        assert_eq!(err.to_string(), expected);
    }
    let commands = json!([1, 2]);
    assert_eq!(super::js_for_each(Some(&commands), "e").unwrap().len(), 2);
}

/// R2D-2: the result manager's `decoratorValidation` comes from
/// `DecorateOptions::decorator_validation` when set (the binding passes the
/// target manager's), and the source's otherwise. The source keeps its
/// options and its validity proofs, for a decorated result and an empty
/// command set alike.
#[test]
fn the_result_decorator_validation_is_passed_in_and_the_source_is_untouched() {
    let mgr = sample_manager();
    let mut command_set = valid_command_set();
    command_set["commands"][0]["decorator"]["$class"] = json!("concerto.metamodel@1.0.0.Decorator");
    mgr.validate_models().unwrap();
    assert!(mgr.validity_proof("org.acme@1.0.0").is_some());
    let target = crate::introspect::DecoratorValidationOptions {
        missing_decorator: Some("warn".into()),
        invalid_decorator: None,
    };
    for sets in [vec![command_set.clone()], Vec::new()] {
        let mut sets = sets;
        let mut options = DecorateOptions {
            decorator_validation: Some(target.clone()),
            ..Default::default()
        };
        let decorated = decorate_models(&mgr, &mut sets, &mut options).unwrap();
        assert_eq!(decorated.decorator_validation(), &target);
        assert_eq!(
            mgr.decorator_validation(),
            &crate::introspect::DecoratorValidationOptions::default()
        );
        assert!(mgr.validity_proof("org.acme@1.0.0").is_some());
    }
    // Without one, the result takes the source's.
    let mut sets = [command_set];
    let decorated = decorate_models(&mgr, &mut sets, &mut DecorateOptions::default()).unwrap();
    assert_eq!(decorated.decorator_validation(), mgr.decorator_validation());
}

/// On a manager (system models included for
/// `ExtractAll`/`ExtractVocab`, not for `ExtractNonVocab`), `extract`
/// keeping its source gives the same result as without, and
/// `encode_extract_source` over the kept models rebuilds its command
/// sets and vocabularies.
#[test]
fn the_kept_source_rebuilds_the_extracted_command_sets() {
    let mut mgr = sample_manager();
    mgr.load_model(
        &json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.deco@1.0.0",
            "imports": [{ "$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.acme@1.0.0", "name": "Person" }],
            "decorators": [{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Term",
                "arguments": [{ "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "Deco" }] }],
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Staff", "isAbstract": false,
                  "decorators": [{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Ref",
                    "arguments": [{ "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
                      "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" }, "isArray": false }] }],
                  "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id", "isArray": false, "isOptional": false,
                      "decorators": [{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Term",
                        "arguments": [{ "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "Id" }] }] }
                  ] }
            ]
        }),
        None,
    )
    .unwrap();
    for action in [
        extractor::Action::ExtractAll,
        extractor::Action::ExtractVocab,
        extractor::Action::ExtractNonVocab,
    ] {
        let options = ExtractOptions::default();
        let direct = extract(&mgr, &options, action, false).unwrap();
        let mut kept = extract(&mgr, &options, action, true).unwrap();
        let source = kept.source_models.take().unwrap();
        assert_eq!(kept.decorator_command_set, direct.decorator_command_set);
        assert_eq!(kept.vocabularies, direct.vocabularies);
        let asts = |mm: &ModelManager| {
            mm.model_files()
                .map(|f| f.ast().clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(asts(&kept.model_manager), asts(&direct.model_manager));
        let system = source
            .iter()
            .any(|m| m.get("namespace").and_then(Value::as_str) == Some("concerto@1.0.0"));
        assert_eq!(system, action != extractor::Action::ExtractNonVocab);
        for locale in ["en", "fr"] {
            let options = ExtractOptions {
                remove_decorators_from_model: false,
                locale: locale.to_string(),
            };
            let fresh = extract(&mgr, &options, action, false).unwrap();
            let (sets, vocabularies) = encode_extract_source(&source, &options, action).unwrap();
            assert_eq!(sets, fresh.decorator_command_set, "{action:?} {locale}");
            assert_eq!(vocabularies, fresh.vocabularies, "{action:?} {locale}");
        }
    }
}

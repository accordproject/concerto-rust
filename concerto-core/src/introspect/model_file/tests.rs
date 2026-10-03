use super::*;

/// P5-93: the names a load reads from JSON text share the copy of the
/// text the file keeps (`concerto_metamodel::Name`), where each used to
/// be copied; an escaped one is a copy of its own.
#[test]
fn names_read_from_text_share_the_text_the_file_keeps() {
    let text = r#"{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.acme@1.0.0","declarations":[{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"Person","isAbstract":false,"properties":[{"$class":"concerto.metamodel@1.0.0.StringProperty","name":"first","isArray":false,"isOptional":false},{"$class":"concerto.metamodel@1.0.0.ObjectProperty","name":"l\u0061st","type":{"$class":"concerto.metamodel@1.0.0.TypeIdentifier","name":"Person"},"isArray":false,"isOptional":false}]}]}"#;
    let (file, _) = ModelFile::from_json_text_checked_with_imports(text, None, None)
        .unwrap()
        .unwrap();
    let kept = file.ast.text.as_deref().unwrap();
    let within = |name: &str| {
        let start = kept.as_ptr() as usize;
        let at = name.as_ptr() as usize;
        at >= start && at + name.len() <= start + kept.len()
    };
    let class = file.declarations()[0].as_class().unwrap();
    assert!(within(class.name()));
    let [first, last, ..] = class.own_properties() else {
        panic!("two properties");
    };
    assert!(within(first.name()));
    assert_eq!(last.name(), "last");
    assert!(!within(last.name()));
    assert!(within(last.type_name().unwrap()));
}

fn sample() -> ModelFile {
    ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.common@1.0.0", "name": "Address" }
            ],
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                  "name": "Person", "isAbstract": false, "properties": [] }
            ]
        }),
        Some("example.cto".into()),
    )
    .unwrap()
}

/// P5-77 (accordproject/concerto-rust#419): `compact_ast` keeps a
/// parsed AST as its compact JSON text, and the AST read back from it is
/// equal to the one it replaced (numbers included), so is the text a
/// second compaction returns, and a clone shares the text; a file read
/// from text keeps its own text.
#[test]
fn compact_ast_keeps_an_equal_ast_as_text() {
    let value = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.compact@1.0.0",
        "decorators": [{
            "$class": "concerto.metamodel@1.0.0.Decorator", "name": "N",
            "arguments": [
                { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 0.1 },
                { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": -0.0 },
                { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 1.0e300 },
                { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 9_007_199_254_740_993_u64 },
                { "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "\u{e9}\"\n" }
            ]
        }],
        "declarations": [
            { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
              "name": "Person", "isAbstract": false, "properties": [] }
        ]
    });
    let mut mf = ModelFile::from_json(&value, None).unwrap();
    assert!(!mf.built_by_typed_path());
    let text = mf.compact_ast().unwrap();
    assert_eq!(&*text, serde_json::to_string(&value).unwrap());
    assert!(mf.built_by_typed_path());
    let copy = mf.clone();
    assert_eq!(mf.ast(), &value);
    assert_eq!(copy.ast(), &value);
    assert_eq!(mf.compact_ast().unwrap(), text);

    let mut typed = ModelFile::from_json_text(&serde_json::to_string(&value).unwrap(), None, None)
        .unwrap()
        .unwrap();
    let own = typed.ast.text.clone();
    assert_eq!(&*typed.compact_ast().unwrap(), &*text);
    assert_eq!(typed.ast.text, own);
}

#[test]
fn parses_namespace_imports_and_declarations() {
    let mf = sample();
    assert_eq!(mf.namespace(), "org.example@1.0.0");
    assert_eq!(mf.version(), "1.0.0");
    assert_eq!(mf.declarations().len(), 1);
    // The declared import, then the built-in import of the system types.
    assert_eq!(mf.imports().len(), 2);
    assert_eq!(mf.imports()[1].namespace(), "concerto@1.0.0");
    assert!(mf.local_declaration("Person").is_some());
    assert!(!mf.is_system_namespace());
}

/// P5-93: `local_types` holds each name's hash. The last declaration of
/// a name wins (TS's `Map.set`), and a hash two names share is resolved
/// by name.
#[test]
fn local_types_find_the_last_declaration_of_a_name() {
    let concept = |name: &str| {
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": name, "isAbstract": false, "properties": [] })
    };
    let model = |declarations: Vec<serde_json::Value>| {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example@1.0.0",
            "declarations": declarations,
        })
    };
    // Scanned: a file of at most `LOCAL_SCAN_MAX` declarations.
    let mf =
        ModelFile::from_json(&model(vec![concept("A"), concept("B"), concept("A")]), None).unwrap();
    assert!(mf.local_types.is_empty());
    assert_eq!(mf.local_index("A"), Some(2));
    assert_eq!(mf.local_index("B"), Some(1));
    assert_eq!(mf.local_index("C"), None);
    assert!(mf.is_local_type("B") && !mf.is_local_type("C"));
    // Hashed.
    let mut declarations = vec![concept("A"), concept("B"), concept("A")];
    declarations.extend((0..LOCAL_SCAN_MAX).map(|i| concept(&format!("D{i}"))));
    let mut mf = ModelFile::from_json(&model(declarations), None).unwrap();
    assert!(!mf.local_types.is_empty());
    assert_eq!(mf.local_index("A"), Some(2));
    assert_eq!(mf.local_index("B"), Some(1));
    assert_eq!(mf.local_index("D7"), Some(10));
    assert_eq!(mf.local_index("C"), None);
    assert!(mf.is_local_type("B") && !mf.is_local_type("C"));
    // As if "A" and "C" shared a hash.
    mf.local_types.insert(name_hash("A"), SHARED_HASH);
    mf.local_types.insert(name_hash("C"), SHARED_HASH);
    assert_eq!(mf.local_index("A"), Some(2));
    assert_eq!(mf.local_index("C"), None);
    // As if "C" had "B"'s hash.
    mf.local_types.insert(name_hash("C"), 1);
    assert_eq!(mf.local_index("C"), None);
}

#[test]
fn resolves_local_primitive_and_import() {
    let mf = sample();
    assert_eq!(
        mf.resolve_local_type("Person").as_deref(),
        Some("org.example@1.0.0.Person")
    );
    assert_eq!(mf.resolve_local_type("String").as_deref(), Some("String"));
    assert_eq!(
        mf.resolve_local_type("Address").as_deref(),
        Some("org.common@1.0.0.Address")
    );
    assert_eq!(mf.resolve_local_type("Missing"), None);
}

/// TS: test/introspect/modelfile.js #constructor "should throw when null
/// ast provided" / "non object ast" / "invalid definitions" / "invalid
/// filename" — each a plain `Error`, checked in TS's order.
#[test]
fn constructor_arguments_are_checked_in_ts_order() {
    use serde_json::json;
    let message = |r: Result<()>| match r.unwrap_err().into_ported() {
        Some(c) => {
            assert_eq!(c.kind, ErrorKind::InvalidArgument);
            c.message()
        }
        other => panic!("expected a plain Error, got {other:?}"),
    };
    let ast = json!({ "namespace": "org.acme@1.0.0" });
    assert_eq!(
        message(ModelFile::check_constructor_arguments(
            Some(&json!(null)),
            None,
            None
        )),
        "ast not specified"
    );
    assert_eq!(
        message(ModelFile::check_constructor_arguments(
            None,
            Some(&json!({})),
            None
        )),
        "ast not specified"
    );
    assert_eq!(
        message(ModelFile::check_constructor_arguments(
            Some(&json!(true)),
            None,
            None
        )),
        "ModelFile expects a Concerto model AST as input."
    );
    assert_eq!(
        message(ModelFile::check_constructor_arguments(
            Some(&ast),
            Some(&json!({})),
            Some(&json!({}))
        )),
        "ModelFile expects an (optional) Concerto model definition as a string."
    );
    assert_eq!(
        message(ModelFile::check_constructor_arguments(
            Some(&ast),
            None,
            Some(&json!({}))
        )),
        "ModelFile expects an (optional) filename as a string."
    );
    // Falsy non-strings are ignored, as TS's `definitions && …` is.
    ModelFile::check_constructor_arguments(Some(&ast), Some(&json!(null)), Some(&json!("")))
        .unwrap();
    ModelFile::check_constructor_arguments(Some(&ast), Some(&json!("cto")), Some(&json!("a.cto")))
        .unwrap();
}

/// TS: `ClassDeclaration.process` rejects a system property name with
/// the declaration's own `ast.location` and the model file's name.
#[test]
fn a_system_property_name_is_rejected_with_the_declaration_location() {
    let location = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Range",
        "start": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 55, "line": 3, "column": 1 },
        "end": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 103, "line": 5, "column": 2 }
    });
    let err = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "C", "isAbstract": false, "location": location,
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "$class",
                      "isArray": false, "isOptional": false }
                ]
            }]
        }),
        Some("c.cto".into()),
    )
    .unwrap_err();
    let Some(err) = err.ported().cloned() else {
        panic!("expected a contract error, got {err:?}");
    };
    assert_eq!(err.location, Some(location));
    assert_eq!(
        err.final_message(),
        "Invalid field name '$class' File 'c.cto': line 3 column 1, to line 5 column 2. "
    );
}

/// TS's `ModelFile` constructor accepts two declarations of one name:
/// both stay in `getAllDeclarations()`, and the `localTypes` lookup keeps
/// the last (a `Map.set` per declaration). Rejecting the duplicate is
/// `ModelFile.validate()`'s job (P2-08 review: this test used to assert
/// that construction itself failed, which TS never does).
#[test]
#[allow(deprecated)]
fn duplicate_declaration_is_accepted_at_construction_and_the_last_wins() {
    let mf = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.dup@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "A", "isAbstract": false, "properties": [] },
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "A", "isAbstract": false, "properties": [] }
                ]
            }),
            None,
        )
        .expect("TS's constructor accepts a duplicate declaration name");
    assert_eq!(mf.declarations().len(), 2);
    assert_eq!(mf.local_index("A"), Some(1));
    assert!(mf.get_asset_declaration("A").is_some());
}

#[test]
fn missing_namespace_is_rejected() {
    let err = ModelFile::from_json(
        &serde_json::json!({ "$class": "concerto.metamodel@1.0.0.Model" }),
        None,
    );
    assert!(err.is_err());
}

#[test]
fn unversioned_namespace_is_rejected() {
    let err = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example",
            "declarations": []
        }),
        None,
    );
    assert!(err.is_err());
}

#[test]
fn non_array_declarations_or_imports_is_rejected() {
    let bad_decls = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.x@1.0.0",
            "declarations": { "not": "an array" }
        }),
        None,
    );
    assert!(bad_decls.is_err());

    let bad_imports = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.x@1.0.0",
            "imports": "nope"
        }),
        None,
    );
    assert!(bad_imports.is_err());
}

#[test]
fn keeps_the_ast_it_was_given_in_its_original_key_order() {
    let text = r#"{"namespace":"org.order@1.0.0","$class":"concerto.metamodel@1.0.0.Model","declarations":[{"properties":[],"name":"A","$class":"concerto.metamodel@1.0.0.ConceptDeclaration","isAbstract":false,"decorators":[]}]}"#;
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let mf = ModelFile::from_json(&value, None).unwrap();
    assert_eq!(mf.ast(), &value);
    assert_eq!(serde_json::to_string(mf.ast()).unwrap(), text);
}

// TS: test/introspect/modelfile.js `#isExternal`.
#[test]
fn is_external_reflects_an_at_prefixed_file_name() {
    let at_sign = ModelFile::from_json(&sample().ast().clone(), Some("@carlease".into())).unwrap();
    assert!(at_sign.is_external());
    let plain = ModelFile::from_json(&sample().ast().clone(), Some("carlease".into())).unwrap();
    assert!(!plain.is_external());
    assert!(!sample().is_external());
}

fn model_with_version(concerto_version: &str) -> serde_json::Value {
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.v@1.0.0",
        "concertoVersion": concerto_version,
        "declarations": []
    })
}

// TS: test/introspect/modelfile.js `#isCompatibleVersion`/`#getConcertoVersion`.
#[test]
fn a_concerto_version_satisfied_by_this_runtime_is_recorded_verbatim() {
    let mf = ModelFile::from_json(&model_with_version("^5.0.0"), None).unwrap();
    assert_eq!(mf.concerto_version(), Some("^5.0.0"));
}

#[test]
fn a_v3_concerto_version_is_accepted_for_backward_compatibility() {
    let mf = ModelFile::from_json(&model_with_version("^3.0.0"), None).unwrap();
    assert_eq!(mf.concerto_version(), Some("^3.0.0"));
}

#[test]
fn an_unsatisfiable_concerto_version_is_rejected() {
    let err = ModelFile::from_json(&model_with_version("^99.0.0"), None);
    let message = err.unwrap_err().to_string();
    assert!(message.contains("v3.0.0 or greater"));
    assert!(message.contains("^99.0.0"));
}

#[test]
fn no_concerto_version_at_all_leaves_it_none() {
    assert_eq!(sample().concerto_version(), None);
}

/// TS: test/introspect/modelfile.js #resolveImport "should throw if it
/// cannot resolve a type that is not imported": the message lists
/// `JSON.stringify(this.imports)` — the AST's own import nodes verbatim,
/// then the built-in system import.
#[test]
fn resolve_import_failure_lists_the_imports_as_ts_stringifies_them() {
    let mf = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "name": "Wow", "namespace": "org.doge@1.0.0" }
            ],
            "declarations": []
        }),
        None,
    )
    .unwrap();
    let Some(err) = mf.resolve_import("Coin").unwrap_err().into_ported() else {
        panic!("expected a contract error");
    };
    assert_eq!(
        err.final_message(),
        "Failed to find \"Coin\" in list of imports \"[[{\"$class\":\"concerto.metamodel@1.0.0.ImportType\",\"name\":\"Wow\",\"namespace\":\"org.doge@1.0.0\"},{\"$class\":\"concerto.metamodel@1.0.0.ImportTypes\",\"namespace\":\"concerto@1.0.0\",\"types\":[\"Concept\",\"Asset\",\"Transaction\",\"Participant\",\"Event\"]}]]\" for namespace \"org.acme@1.0.0\". "
    );
}

#[test]
#[allow(deprecated)]
fn resolves_and_reports_imported_types_by_their_visible_local_name() {
    let mf = sample();
    assert!(mf.is_imported_type("Address"));
    assert!(!mf.is_imported_type("Nonexistent"));
    assert_eq!(
        mf.resolve_import("Address").unwrap(),
        "org.common@1.0.0.Address"
    );
    assert_eq!(mf.get_imported_type("Address").unwrap(), "Address");
    assert!(mf.resolve_import("Nonexistent").is_err());
    assert!(mf.is_defined("Person"));
    assert!(mf.is_defined("String"));
    // TS `isDefined`: an imported-only name is not "defined" by this file.
    assert!(!mf.is_defined("Address"));
}

#[test]
#[allow(deprecated)]
fn get_imports_lists_declared_names_never_aliases() {
    let mf = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.alias@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportTypes",
                  "namespace": "org.common@1.0.0", "types": ["Address"],
                  "aliasedTypes": [
                    { "$class": "concerto.metamodel@1.0.0.AliasedType",
                      "name": "Address", "aliasedName": "Location" }
                  ] }
            ],
            "declarations": []
        }),
        None,
    )
    .unwrap();
    assert!(
        mf.get_imports()
            .contains(&"org.common@1.0.0.Address".to_string())
    );
    assert!(mf.is_imported_type("Location"));
    assert!(!mf.is_imported_type("Address"));
}

/// P5-28: `from_json_text_with_imports` loads the same file as
/// `from_json_text` and returns the AST's own `imports` node verbatim,
/// or `None` when the AST has none.
#[test]
fn from_json_text_with_imports_returns_the_imports_node() {
    let imports = serde_json::json!([
        { "$class": "concerto.metamodel@1.0.0.ImportType",
          "namespace": "org.common@1.0.0", "name": "Address", "uri": "u" }
    ]);
    let text = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.uri@1.0.0",
        "imports": imports,
        "declarations": []
    })
    .to_string();
    let (mf, node) = ModelFile::load_text_with_imports(&text, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(mf.namespace(), "org.uri@1.0.0");
    assert_eq!(node, Some(imports));
    let text = r#"{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.x@1.0.0"}"#;
    let (_, node) = ModelFile::load_text_with_imports(text, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(node, None);
    assert!(ModelFile::load_text_with_imports("{", None, None).is_err());
}

#[test]
#[allow(deprecated)]
fn get_import_uri_is_keyed_by_the_imports_first_fully_qualified_name() {
    let mf = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.uri@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.common@1.0.0", "name": "Address",
                  "uri": "https://example.org/common.cto" }
            ],
            "declarations": []
        }),
        None,
    )
    .unwrap();
    assert_eq!(
        mf.get_import_uri("org.common@1.0.0.Address"),
        Some("https://example.org/common.cto")
    );
    assert_eq!(mf.get_import_uri("org.common@1.0.0"), None);
    assert_eq!(
        mf.get_external_imports().get("org.common@1.0.0.Address"),
        Some(&"https://example.org/common.cto".to_string())
    );
}

#[test]
fn external_imports_preserves_import_order_for_several_uri_imports() {
    // Issue #263: `getExternalImports` must come back in import order
    // (TS builds `importUriMap` by assigning one key per import, in
    // file order), not the arbitrary order a `HashMap` would give.
    let n = 12;
    let imports: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.ImportType",
                "namespace": format!("org.n{i}@1.0.0"),
                "name": format!("T{i}"),
                "uri": format!("https://example.com/m{i}.cto"),
            })
        })
        .collect();
    let mf = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.order@1.0.0",
            "imports": imports,
            "declarations": []
        }),
        None,
    )
    .unwrap();

    let expected: Vec<String> = (0..n).map(|i| format!("org.n{i}@1.0.0.T{i}")).collect();
    let actual: Vec<String> = mf.external_imports().keys().cloned().collect();
    assert_eq!(actual, expected);
}

#[test]
fn external_imports_last_write_wins_in_place_for_a_duplicate_key() {
    // Matches TS's `importUriMap[key] = imp.uri`: assigning to an
    // existing plain-object key updates the value without moving it.
    let mf = ModelFile::from_json(
        &serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.dup@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.common@1.0.0", "name": "Address",
                  "uri": "https://example.org/first.cto" },
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.other@1.0.0", "name": "Thing",
                  "uri": "https://example.org/other.cto" },
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.common@1.0.0", "name": "Address",
                  "uri": "https://example.org/second.cto" }
            ],
            "declarations": []
        }),
        None,
    )
    .unwrap();

    let imports = mf.external_imports();
    assert_eq!(
        imports.keys().cloned().collect::<Vec<_>>(),
        vec![
            "org.common@1.0.0.Address".to_string(),
            "org.other@1.0.0.Thing".to_string(),
        ]
    );
    assert_eq!(
        imports.get("org.common@1.0.0.Address"),
        Some(&"https://example.org/second.cto".to_string())
    );
}

fn model_with_two_concepts(ns: &str) -> serde_json::Value {
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": ns,
        "declarations": [
            { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
              "name": "Keep", "isAbstract": false, "properties": [] },
            { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
              "name": "Drop", "isAbstract": false, "properties": [] }
        ]
    })
}

// TS: test/introspect/modelfile.js `#filter`.
#[test]
fn filter_keeps_only_matching_declarations() {
    let manager = crate::model_manager::ModelManager::new().unwrap();
    let mf = ModelFile::from_json(&model_with_two_concepts("org.f@1.0.0"), None).unwrap();
    let filtered = mf
        .filter(|d| d.name() == "Keep", &manager)
        .unwrap()
        .expect("Keep survives");
    assert_eq!(filtered.declarations().len(), 1);
    assert_eq!(filtered.declarations()[0].name(), "Keep");
}

#[test]
fn filter_returns_none_when_every_declaration_is_rejected() {
    let manager = crate::model_manager::ModelManager::new().unwrap();
    let mf = ModelFile::from_json(&model_with_two_concepts("org.f2@1.0.0"), None).unwrap();
    assert!(mf.filter(|_| false, &manager).unwrap().is_none());
}

#[test]
fn filter_drops_an_import_whose_only_type_is_filtered_out_of_its_source_file() {
    let mut manager = crate::model_manager::ModelManager::new().unwrap();
    manager
        .load_model(&model_with_two_concepts("org.src@1.0.0"), None)
        .unwrap();

    let importing = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.importing@1.0.0",
        "imports": [
            { "$class": "concerto.metamodel@1.0.0.ImportType",
              "namespace": "org.src@1.0.0", "name": "Drop" }
        ],
        "declarations": [
            { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
              "name": "User", "isAbstract": false, "properties": [] }
        ]
    });
    let mf = ModelFile::from_json(&importing, None).unwrap();

    // The predicate rejects `Drop` wherever it is asked about, including
    // in the source file `org.src@1.0.0` that the import is checked
    // against — so the import of `Drop` alone is dropped entirely.
    let filtered = mf
        .filter(|d| d.name() != "Drop", &manager)
        .unwrap()
        .expect("User survives");
    assert!(
        filtered
            .ast()
            .get("imports")
            .and_then(|v| v.as_array())
            .is_none_or(Vec::is_empty)
    );
}

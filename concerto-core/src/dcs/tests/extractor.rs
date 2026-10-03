use super::super::DECORATOR_STRING_TYPE;
use super::*;
use serde_json::json;

fn decorator(name: &str, value: &str) -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Decorator",
        "name": name,
        "arguments": [
            { "$class": DECORATOR_STRING_TYPE, "value": value }
        ]
    })
}

/// The command sets of `result`, parsed from their JSON text.
fn sets(result: &ExtractResult) -> Vec<Value> {
    serde_json::from_str(&result.decorator_command_set).expect("the command sets are JSON")
}

fn sample_models() -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": [{
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test@1.0.0",
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "decorators": [decorator("Term", "Person"), decorator("Custom", "hi")],
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "name",
                    "isArray": false,
                    "isOptional": false,
                    "decorators": [decorator("Term", "Name")]
                }]
            }]
        }]
    })
}

#[test]
fn extracts_a_vocabulary_and_a_non_vocabulary_command_set() {
    let extractor =
        DecoratorExtractor::new(true, "en", "0.4.0", sample_models(), Action::ExtractAll);
    let result = extractor.extract(false).expect("extraction succeeds");

    let sets = sets(&result);
    assert_eq!(sets.len(), 1);
    let dcs = &sets[0];
    assert_eq!(dcs["commands"].as_array().unwrap().len(), 1);
    assert_eq!(dcs["commands"][0]["decorator"]["name"], "Custom");

    assert_eq!(result.vocabularies.len(), 1);
    let vocab = &result.vocabularies[0];
    assert!(vocab.contains("locale: en\n"));
    assert!(vocab.contains("namespace: test@1.0.0\n"));
    assert!(vocab.contains("  - Person: Person\n"));
    assert!(vocab.contains("      - name: Name\n"));

    // removeDecoratorsFromModel + EXTRACT_ALL strips every decorator.
    let person = &result.model_manager.model_file("test@1.0.0").unwrap().ast()["declarations"][0];
    assert!(person.get("decorators").is_none());
}

#[test]
fn extract_vocab_only_leaves_non_vocab_decorators_in_place() {
    let extractor =
        DecoratorExtractor::new(true, "en", "0.4.0", sample_models(), Action::ExtractVocab);
    let result = extractor.extract(false).expect("extraction succeeds");
    assert_eq!(result.decorator_command_set, "[]");
    assert_eq!(result.vocabularies.len(), 1);

    let person = &result.model_manager.model_file("test@1.0.0").unwrap().ast()["declarations"][0];
    let names: Vec<&str> = person["decorators"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Custom"]);
}

#[test]
fn without_remove_the_result_models_are_the_source_models() {
    let source = sample_models();
    let extractor =
        DecoratorExtractor::new(false, "en", "0.4.0", source.clone(), Action::ExtractAll);
    let result = extractor.extract(false).expect("extraction succeeds");
    assert_eq!(sets(&result).len(), 1);
    assert_eq!(result.vocabularies.len(), 1);
    assert_eq!(
        result.model_manager.model_file("test@1.0.0").unwrap().ast(),
        &source["models"][0]
    );
}

#[test]
fn a_model_without_declarations_is_given_an_empty_array() {
    let models = json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": [{
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test@1.0.0",
            "decorators": [decorator("Term", "Test")]
        }]
    });
    let extractor = DecoratorExtractor::new(true, "en", "0.4.0", models, Action::ExtractAll);
    let result = extractor.extract(false).expect("extraction succeeds");
    let ast = result.model_manager.model_file("test@1.0.0").unwrap().ast();
    assert!(ast.get("decorators").is_none());
    assert_eq!(ast["declarations"], json!([]));
    assert_eq!(
        result.vocabularies,
        vec!["locale: en\nnamespace: test@1.0.0\nterm: Test\ndeclarations: []\n"]
    );
}

#[test]
fn map_keys_and_values_are_extracted_and_stripped() {
    let models = json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": [{
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test@1.0.0",
            "declarations": [{
                "$class": MAP_DECLARATION_CLASS,
                "name": "Dictionary",
                "key": {
                    "$class": "concerto.metamodel@1.0.0.StringMapKeyType",
                    "decorators": [decorator("Term", "Word"), decorator("Custom", "k")]
                },
                "value": {
                    "$class": "concerto.metamodel@1.0.0.StringMapValueType",
                    "decorators": [decorator("Custom", "v")]
                }
            }]
        }]
    });
    let extractor = DecoratorExtractor::new(true, "en", "0.4.0", models, Action::ExtractNonVocab);
    let result = extractor.extract(false).expect("extraction succeeds");
    let sets = sets(&result);
    let commands = sets[0]["commands"].as_array().unwrap();
    let targets: Vec<&Value> = commands
        .iter()
        .map(|c| &c["target"]["mapElement"])
        .collect();
    assert_eq!(targets, vec!["KEY", "VALUE"]);
    assert!(result.vocabularies.is_empty());

    // EXTRACT_NON_VOCAB with removal keeps only the vocabulary decorators.
    let map = &result.model_manager.model_file("test@1.0.0").unwrap().ast()["declarations"][0];
    let key_names: Vec<&str> = map["key"]["decorators"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(key_names, vec!["Term"]);
    assert_eq!(map["value"]["decorators"], json!([]));
}

/// Every action and both `removeDecoratorsFromModel` settings succeed
/// (or, with `expect_ok` false, fail), and the memo route agrees with a
/// fresh extraction ([`assert_memo_route_agrees`]). P5-103 (C-5) deleted
/// the `Value` route this used to hold the encoding to; the golden text
/// in [`the_encoding_matches_its_golden_text`] and the oracle corpus
/// cover the encoding directly.
fn assert_routes_agree(models: &Value, expect_ok: bool) {
    for action in [
        Action::ExtractAll,
        Action::ExtractVocab,
        Action::ExtractNonVocab,
    ] {
        for remove in [false, true] {
            let result = DecoratorExtractor::new(remove, "fr", "0.4.0", models.clone(), action)
                .extract(false);
            // A reserved vocabulary key fails every action that reads
            // the vocabulary decorators; `ExtractNonVocab` never does.
            let expect_ok = expect_ok || action == Action::ExtractNonVocab;
            match &result {
                Ok(_) => assert!(expect_ok, "{action:?} remove={remove}"),
                Err(e) => {
                    assert!(!expect_ok, "{action:?} remove={remove}: {e}");
                    assert!(e.to_string().contains("Invalid vocabulary key"), "{e}");
                }
            }
            if let Ok(result) = result {
                let parsed: Value = serde_json::from_str(&result.decorator_command_set)
                    .expect("the command sets are JSON");
                assert!(parsed.is_array(), "{action:?} remove={remove}");
            }
            assert_memo_route_agrees(models, action, remove);
        }
    }
}

/// P5-56 (T2, F-A2): `extract(true)` is `extract(false)` (same result,
/// same error) plus the source models, and `encode_source` over the kept
/// models gives the same command sets and vocabularies (or the same
/// error) as `extract` with any locale and either
/// `removeDecoratorsFromModel`, every time it is called.
fn assert_memo_route_agrees(models: &Value, action: Action, remove: bool) {
    let direct =
        DecoratorExtractor::new(remove, "fr", "0.4.0", models.clone(), action).extract(false);
    let keeping =
        DecoratorExtractor::new(remove, "fr", "0.4.0", models.clone(), action).extract(true);
    let (kept_result, source) = match (direct, keeping) {
        (Ok(d), Ok(mut k)) => {
            assert!(d.source_models.is_none());
            let source = k.source_models.take().expect("the source models are kept");
            (Some((d, k)), source)
        }
        (Err(d), Err(k)) => {
            assert_eq!(k, d, "{action:?} remove={remove}");
            (
                None,
                models["models"].as_array().cloned().unwrap_or_default(),
            )
        }
        (d, k) => panic!(
            "{action:?} remove={remove}: direct ok {}, keeping ok {}",
            d.is_ok(),
            k.is_ok()
        ),
    };
    if let Some((d, k)) = &kept_result {
        assert_eq!(k.decorator_command_set, d.decorator_command_set);
        assert_eq!(k.vocabularies, d.vocabularies);
        let asts = |mm: &ModelManager| {
            mm.model_files()
                .map(|f| f.ast().clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(asts(&k.model_manager), asts(&d.model_manager));
        assert_eq!(&source, models["models"].as_array().unwrap());
    }
    for (other_remove, locale) in [(remove, "fr"), (!remove, "de"), (remove, "fr")] {
        let encoded = DecoratorExtractor::new(other_remove, locale, "0.4.0", Value::Null, action)
            .encode_source(&source);
        let fresh = DecoratorExtractor::new(other_remove, locale, "0.4.0", models.clone(), action)
            .extract(false);
        match (encoded, fresh) {
            (Ok(e), Ok(f)) => {
                assert_eq!(e.0, f.decorator_command_set, "{action:?} {locale}");
                assert_eq!(e.1, f.vocabularies, "{action:?} {locale}");
            }
            // A result-model error comes first in `extract`; the
            // memo is only kept after a call that did not fail, so only
            // the transform's own errors can reach `encode_source`.
            (Err(e), Err(f)) => assert_eq!(e, f, "{action:?} {locale}"),
            (Ok(_), Err(_)) if kept_result.is_none() => {}
            (e, f) => panic!(
                "{action:?} {locale}: encode_source ok {}, extract ok {}",
                e.is_ok(),
                f.is_ok()
            ),
        }
    }
}

/// P5-103 (C-5): the encoding's output for a model that exercises every
/// argument kind, escapes, reserved-looking keys and map elements, held
/// to the text the `Value` route (deleted by P5-103) gave for it, so the
/// encoding keeps that route's bytes without the route itself.
#[test]
fn the_encoding_matches_its_golden_text() {
    assert_routes_agree(&sample_models(), true);

    let dec = |name: Value, args: Value| {
        let mut d = json!({ "$class": "concerto.metamodel@1.0.0.Decorator", "arguments": args });
        if !name.is_null() {
            d["name"] = name;
        }
        d
    };
    let s = |v: &str| json!([{ "$class": DECORATOR_STRING_TYPE, "value": v }]);
    let models = json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": [{
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.p557@1.2.3",
            "decorators": [
                dec(json!("Term_description"), s("ns \"quoted\"")),
                dec(json!("Term"), s("Namespace")),
                dec(json!("Term_term"), s("Replaced")),
                dec(json!("Term_"), s("empty key")),
                dec(json!("Meta"), json!([{ "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 1 }])),
            ],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "decorators": [
                    dec(json!("Term_plural"), s("People")),
                    dec(json!("Flag"), json!([{ "$class": "concerto.metamodel@1.0.0.DecoratorBoolean", "value": true }])),
                    dec(json!("Ref"), json!([{
                        "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
                        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person", "namespace": "org.p557@1.2.3" },
                        "isArray": true
                    }])),
                    json!({ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "NoArgs" }),
                    dec(json!("Odd"), json!([
                        { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 2.5e-7 },
                        { "$class": DECORATOR_STRING_TYPE, "value": "tab\t \"q\" \u{e9} \u{1F600} </" }
                    ])),
                ],
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "name",
                    "isArray": false,
                    "isOptional": false,
                    "decorators": [
                        dec(json!("Term_description"), s("The name")),
                        dec(json!("Term"), s("Name")),
                        dec(json!("Term_propertyVocabs"), s("kept")),
                        dec(json!("Custom"), json!([])),
                    ]
                }, {
                    "$class": "concerto.metamodel@1.0.0.IntegerProperty",
                    "name": "age",
                    "isArray": false,
                    "isOptional": false,
                    "decorators": [dec(json!("Term_unit"), json!([{ "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 10 }]))]
                }]
            }, {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Overwritten",
                "isAbstract": false,
                "decorators": [dec(json!("Term_propertyVocabs"), s("gone"))],
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "before",
                    "isArray": false,
                    "isOptional": false,
                    "decorators": [dec(json!("Term"), s("Before"))]
                }]
            }, {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Plain",
                "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "only",
                    "isArray": false,
                    "isOptional": false,
                    "decorators": [dec(json!("Term_x"), s("x"))]
                }]
            }, {
                "$class": MAP_DECLARATION_CLASS,
                "name": "Dictionary",
                "key": {
                    "$class": "concerto.metamodel@1.0.0.StringMapKeyType",
                    "decorators": [dec(json!("Term"), s("Word")), dec(json!("Custom"), s("k"))]
                },
                "value": {
                    "$class": "concerto.metamodel@1.0.0.StringMapValueType",
                    "decorators": [dec(json!("Term_meaning"), s("Meaning"))]
                }
            }]
        }, {
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.other@0.0.1-rc.1",
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Thing",
                "isAbstract": false,
                "decorators": [dec(json!("Custom"), s("thing"))],
                "properties": []
            }]
        }]
    });
    assert_routes_agree(&models, true);
    let result = DecoratorExtractor::new(false, "fr", "0.4.0", models, Action::ExtractAll)
        .extract(false)
        .unwrap();
    assert_eq!(
        result.decorator_command_set,
        include_str!("testdata/p557-extract-all-fr.json")
    );
    assert_eq!(
        result.vocabularies,
        [
            "locale: fr\nnamespace: org.p557@1.2.3\nterm: Replaced\ndescription: ns \"quoted\"\n: empty key\ndeclarations:\n  - Person: Person\n    plural: People\n    properties:\n      - name: Name\n        description: The name\n        propertyVocabs: kept\n      - age: age\n        unit: 10\n  - Overwritten: Overwritten\n    properties:\n      - before: Before\n  - Plain: Plain\n    properties:\n      - only: only\n        x: x\n  - Dictionary: Dictionary\n    properties:\n      - KEY: Word\n      - VALUE: VALUE\n        meaning: Meaning\n"
        ]
    );

    // Each reserved-key error.
    for (target, name) in [
        ("model", "Term_locale"),
        ("declaration", "Term_properties"),
        ("declaration", "Term_Person"),
        ("property", "Term_name"),
    ] {
        let mut bad = sample_models();
        let node = match target {
            "model" => &mut bad["models"][0],
            "declaration" => &mut bad["models"][0]["declarations"][0],
            _ => &mut bad["models"][0]["declarations"][0]["properties"][0],
        };
        node["decorators"] = json!([decorator("Custom", "first"), decorator(name, "oops")]);
        assert_routes_agree(&bad, false);
    }
}

/// P5-98 (C-11): a `Term_*` decorator's number argument is written in
/// the vocabulary YAML as JS `String(value)` writes it — TS 5.0.0's
/// `extractDecorators` gives `unit: 0.000001` and `max: 1e+21`, where
/// serde_json's own `Display` gave `1e-6` and `1e21`.
#[test]
fn a_vocabulary_number_argument_is_written_as_js_string_does() {
    let number =
        |v: Value| json!([{ "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": v }]);
    let dec = |name: &str, args: Value| json!({ "$class": "concerto.metamodel@1.0.0.Decorator", "name": name, "arguments": args });
    let models = json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": [{
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test@1.0.0",
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "P",
                "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.IntegerProperty",
                    "name": "age",
                    "isArray": false,
                    "isOptional": false,
                    "decorators": [
                        dec("Term_unit", number(json!(0.000001))),
                        dec("Term_max", number(json!(1e21))),
                    ]
                }]
            }]
        }]
    });
    let result = DecoratorExtractor::new(false, "en", "0.4.0", models.clone(), Action::ExtractAll)
        .extract(false)
        .unwrap();
    assert_eq!(
        result.vocabularies,
        [
            "locale: en\nnamespace: test@1.0.0\ndeclarations:\n  - P: P\n    properties:\n      - age: age\n        unit: 0.000001\n        max: 1e+21\n"
        ]
    );
    assert_routes_agree(&models, true);
}

#[test]
fn a_term_extension_key_matching_the_declaration_name_is_rejected() {
    let models = json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": [{
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test@1.0.0",
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "decorators": [decorator("Term_Person", "oops")],
                "properties": []
            }]
        }]
    });
    let extractor = DecoratorExtractor::new(false, "en", "0.4.0", models, Action::ExtractAll);
    let err = match extractor.extract(false) {
        Ok(_) => panic!("expected extraction to reject the reserved vocabulary key"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("Invalid vocabulary key"));
}

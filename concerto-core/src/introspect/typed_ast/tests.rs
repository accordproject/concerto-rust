use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ModelFile;
use crate::introspect::Decorated;

/// Everything a [`ModelFile`] holds, in a stable order (its own `Debug`
/// prints a `HashMap` whose order varies between instances).
fn key(file: &ModelFile) -> String {
    format!(
        "{:?}",
        (
            file.namespace(),
            file.version(),
            file.imports(),
            file.declarations(),
            file.file_name(),
            file.ast(),
            file.decorators(),
            file.concerto_version(),
            file.definitions(),
            file.is_external(),
        )
    )
}

/// What a load gives, comparable across the two inputs.
#[derive(Debug, PartialEq)]
enum Outcome {
    NotJson,
    Loaded(String),
    /// The error's kind and code (the message of a read error names
    /// a position only for text).
    Failed(String),
}

fn outcome(result: crate::Result<ModelFile>) -> Outcome {
    match result {
        Ok(file) => Outcome::Loaded(key(&file)),
        Err(e) if e.code() == "modelfile-load-unreadable" => {
            Outcome::Failed(format!("{:?} {}", e.kind(), e.code()))
        }
        Err(e) => Outcome::Failed(format!("{:?} {} {e}", e.kind(), e.code())),
    }
}

fn by_value(text: &str) -> Outcome {
    match serde_json::from_str::<Value>(text) {
        Err(_) => Outcome::NotJson,
        Ok(value) => outcome(ModelFile::from_owned_json_with_definitions(
            value,
            None,
            Some("m.cto".into()),
        )),
    }
}

fn by_text(text: &str) -> Outcome {
    match ModelFile::from_json_text(text, None, Some("m.cto".into())) {
        Err(_) => Outcome::NotJson,
        Ok(result) => outcome(result),
    }
}

/// Loads `text` from the text and from its `Value`; panics unless the
/// results are identical. Returns the result.
fn check(text: &str) -> Outcome {
    let typed = by_text(text);
    assert_eq!(
        typed,
        by_value(text),
        "text and Value loads differ for {text}"
    );
    typed
}

fn is_unreadable(outcome: &Outcome) -> bool {
    matches!(outcome, Outcome::Failed(f) if f.ends_with("modelfile-load-unreadable"))
}

fn model(declarations: Value) -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": declarations,
    })
}

fn concept(properties: Value) -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
        "name": "Thing",
        "isAbstract": false,
        "properties": properties,
    })
}

fn string_property(name: &str) -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": name,
        "isArray": false,
        "isOptional": false,
    })
}

#[test]
fn a_well_formed_model_loads() {
    let text = model(json!([
            concept(json!([string_property("a"), {
                "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
                "name": "r",
                "type": {"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Thing"},
                "decorators": [{"$class": "concerto.metamodel@1.0.0.Decorator", "name": "d", "arguments": [
                    {"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "x"}
                ]}],
            }])),
            {"$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "E", "properties": [
                {"$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "ONE"}
            ]},
        ]))
        .to_string();
    assert!(matches!(check(&text), Outcome::Loaded(_)));
}

#[test]
fn the_ast_is_parsed_from_the_kept_text_on_first_use() {
    let value = model(json!([concept(json!([string_property("a")]))]));
    let file = ModelFile::from_json_text(&value.to_string(), None, None)
        .unwrap()
        .unwrap();
    assert!(file.built_by_typed_path());
    assert_eq!(file.ast(), &value);
    let same = ModelFile::from_json(&value, None).unwrap();
    assert!(file.same_ast(&same) && same.same_ast(&file));
}

#[test]
fn text_a_value_parse_rejects_is_not_json_even_where_the_reader_skips_it() {
    // `serde_json` skips an unknown field without checking it the way a
    // `Value` parse does; `Strict` must not let these through.
    let concept = r#"{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"T","properties":[],"x":"#;
    for skipped in [
        "1e400",
        "-1e400",
        r#""\ud800""#,
        r#""\udc00x""#,
        &format!("{}{}", "[".repeat(200), "]".repeat(200)),
    ] {
        let text = format!(
            r#"{{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.acme@1.0.0","declarations":[{concept}{skipped}}}]}}"#
        );
        assert!(
            ModelFile::from_json_text(&text, None, None).is_err(),
            "{text}"
        );
        assert_eq!(check(&text), Outcome::NotJson);
    }
    // JSON that is not a model, and text that is JSON only up to the
    // point the reader stops at.
    assert_eq!(check("{\"namespace\": }"), Outcome::NotJson);
    assert_eq!(
        check(r#"{"namespace": 1, "declarations": [}"#),
        Outcome::NotJson
    );
    assert!(is_unreadable(&check("[]")));
}

#[test]
fn a_class_that_is_not_the_first_key_is_read_all_the_same() {
    let property = r#"{"isArray":false,"$class":"concerto.metamodel@1.0.0.StringProperty","name":"a","isOptional":false}"#;
    let first = model(json!([concept(json!([string_property("a")]))])).to_string();
    let later = format!(
        r#"{{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.acme@1.0.0","imports":[],"declarations":[{{"name":"Thing","isAbstract":false,"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","properties":[{property}]}}]}}"#
    );
    let (Outcome::Loaded(first), Outcome::Loaded(later)) = (check(&first), check(&later)) else {
        panic!("both load");
    };
    // The same model, but for the AST each keeps verbatim.
    let declarations = |key: &str| key[..key.find("Object {").unwrap_or(key.len())].to_string();
    assert_eq!(declarations(&first), declarations(&later));
    // A node with no string `$class` anywhere is unreadable.
    for declaration in [
        r#"{"name":"T","properties":[]}"#,
        r#"{"name":"T","$class":7,"properties":[]}"#,
    ] {
        let text = format!(
            r#"{{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.acme@1.0.0","declarations":[{declaration}]}}"#
        );
        assert!(matches!(check(&text), Outcome::Failed(_)), "{text}");
    }
}

#[test]
fn a_duplicate_key_in_the_text_is_unreadable() {
    let property = r#"{"$class":"concerto.metamodel@1.0.0.StringProperty","name":"a","isArray":false,"isOptional":false}"#;
    for declaration in [
        format!(
            r#"{{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"T","$class":"concerto.metamodel@1.0.0.AssetDeclaration","properties":[{property}]}}"#
        ),
        format!(
            r#"{{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"T","properties":[],"properties":[{property}]}}"#
        ),
        format!(
            r#"{{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"T","isAbstract":true,"isAbstract":false,"properties":[{property}]}}"#
        ),
    ] {
        let text = format!(
            r#"{{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.acme@1.0.0","declarations":[{declaration}]}}"#
        );
        assert!(is_unreadable(&by_text(&text)), "{text}");
    }
}

/// `identified` and the three validators are read as strictly as every
/// other field (the module doc, "Strictness"). Each value TS 5.0.0 read
/// with no type check is rejected by BC-19's shape check and, with the
/// check off, is unreadable, on both inputs.
#[test]
fn identified_and_the_validators_are_read_strictly() {
    let with = |key: &str, value: Value| {
        let mut node = json!({"$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "C",
                "isAbstract": false, "properties": [string_property("s")]});
        if key == "identified" {
            node[key] = value;
        } else {
            node["properties"][0][key] = value;
        }
        model(json!([node])).to_string()
    };
    let keyless = [
        json!(0),
        json!(false),
        json!(""),
        json!([]),
        json!({}),
        json!(1),
        json!(true),
    ];
    let mut texts = Vec::new();
    for value in keyless.iter().cloned().chain([
        json!({"$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": null}),
        json!({"$class": "foo.IdentifiedBy", "name": "s"}),
        json!({"name": "s"}),
    ]) {
        texts.push(with("identified", value));
    }
    for key in ["sizeValidator", "lengthValidator", "validator"] {
        for value in keyless.iter().cloned() {
            texts.push(with(key, value));
        }
    }
    texts.push(with("sizeValidator", json!({"minSize": 1})));
    texts.push(with(
        "lengthValidator",
        json!({"minLength": 2, "maxLength": 5}),
    ));
    texts.push(with("validator", json!({"pattern": "a", "flags": ""})));
    for text in &texts {
        let value: Value = serde_json::from_str(text).unwrap();
        assert!(crate::instance::check_ast_shape(&value).is_err(), "{text}");
        assert!(is_unreadable(&check(text)), "{text}");
    }
    // An empty `IdentifiedBy` name is no identity (TS reads it by
    // truthiness); `null` is no value.
    for value in [
        json!({"$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": ""}),
        Value::Null,
    ] {
        assert!(
            matches!(
                check(&with("identified", value.clone())),
                Outcome::Loaded(_)
            ),
            "{value}"
        );
    }
}

/// A key the generated struct for a node does not declare is
/// unreadable (the module doc, "Unknown keys"), on both inputs, except
/// the `defaultValue` the reference parser writes on a
/// `DateTimeProperty`.
#[test]
fn an_unknown_key_is_unreadable() {
    let type_identifier =
        json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Thing"});
    let position =
        json!({"$class": "concerto.metamodel@1.0.0.Position", "line": 1, "column": 1, "offset": 0});
    let well_formed = model(json!([
        {
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Thing",
            "isAbstract": false,
            "decorators": [{"$class": "concerto.metamodel@1.0.0.Decorator", "name": "d", "arguments": []}],
            "location": {"$class": "concerto.metamodel@1.0.0.Range", "start": position, "end": position},
            "properties": [
                string_property("a"),
                {"$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "o", "isArray": false,
                    "isOptional": true, "type": type_identifier},
            ],
        },
        {"$class": "concerto.metamodel@1.0.0.StringScalar", "name": "S",
            "validator": {"$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "a", "flags": ""}},
        {"$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "M",
            "key": {"$class": "concerto.metamodel@1.0.0.StringMapKeyType"},
            "value": {"$class": "concerto.metamodel@1.0.0.StringMapValueType"}},
    ]));
    assert!(matches!(
        check(&well_formed.to_string()),
        Outcome::Loaded(_)
    ));
    let paths: &[&[&str]] = &[
        &[],
        &["declarations", "0"],
        &["declarations", "0", "decorators", "0"],
        &["declarations", "0", "location"],
        &["declarations", "0", "location", "start"],
        &["declarations", "0", "properties", "0"],
        &["declarations", "0", "properties", "1", "type"],
        &["declarations", "1"],
        &["declarations", "1", "validator"],
        &["declarations", "2"],
    ];
    for path in paths {
        let mut ast = well_formed.clone();
        let mut node = &mut ast;
        for step in *path {
            node = match step.parse::<usize>() {
                Ok(i) => &mut node[i],
                Err(_) => &mut node[*step],
            };
        }
        node["undeclared"] = json!(1);
        let text = ast.to_string();
        assert!(crate::instance::check_ast_shape(&ast).is_err(), "{path:?}");
        assert!(is_unreadable(&check(&text)), "{path:?}");
    }
    // BC-19's one tolerance.
    let date_time = model(json!([concept(json!([{
        "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "d",
        "isArray": false, "isOptional": false, "defaultValue": "2020-01-01T00:00:00Z"
    }]))]));
    assert!(crate::instance::check_ast_shape(&date_time).is_ok());
    assert!(matches!(check(&date_time.to_string()), Outcome::Loaded(_)));
}

// -----------------------------------------------------------------------
// Differential test over every model AST available
// -----------------------------------------------------------------------

/// Every `concerto.metamodel@1.0.0.Model` object anywhere in `value`.
fn collect_models(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if map.get("$class").and_then(Value::as_str) == Some("concerto.metamodel@1.0.0.Model") {
                out.insert(value.to_string());
            }
            map.values().for_each(|v| collect_models(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_models(v, out)),
        _ => {}
    }
}

fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            json_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

/// Mutated copies of `ast`, one change each, on its first class-like
/// declaration and its first enum declaration, and each one's first two
/// properties: each change either leaves the model loadable or makes one
/// of the loader's checks fail.
fn mutations(ast: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(declarations) = ast.get("declarations").and_then(Value::as_array) else {
        return out;
    };
    let is_enum = |d: &Value| {
        d.get("$class")
            .and_then(Value::as_str)
            .is_some_and(|c| c.ends_with("EnumDeclaration"))
    };
    let indexes = [
        declarations
            .iter()
            .position(|d| d.get("properties").is_some() && !is_enum(d)),
        declarations.iter().position(is_enum),
    ];
    for index in indexes.into_iter().flatten() {
        mutate(ast, index, &mut out);
    }
    out
}

/// [`mutations`] for the declaration at `index`.
fn mutate(ast: &Value, index: usize, out: &mut Vec<Value>) {
    let mut edit = |path: &[&str], change: &dyn Fn(&mut serde_json::Map<String, Value>)| {
        let mut copy = ast.clone();
        let mut node = copy.get_mut("declarations").and_then(|d| d.get_mut(index));
        for step in path {
            node = node.and_then(|n| match step.parse::<usize>() {
                Ok(i) => n.get_mut(i),
                Err(_) => n.get_mut(*step),
            });
        }
        if let Some(Value::Object(map)) = node {
            change(map);
            out.push(copy);
        }
    };
    type Change = dyn Fn(&mut serde_json::Map<String, Value>);
    let changes: &[&Change] = &[
        &|m| {
            m.remove("name");
        },
        &|m| {
            m.insert("name".into(), json!("1bad"));
        },
        &|m| {
            m.insert("name".into(), json!("$timestamp"));
        },
        &|m| {
            m.insert("name".into(), Value::Null);
        },
        &|m| {
            m.insert("$class".into(), json!("concerto.metamodel@1.0.0.Nope"));
        },
        &|m| {
            m.insert("decorators".into(), json!([null]));
        },
        &|m| {
            m.insert("decorators".into(), json!("ab"));
        },
        &|m| {
            m.insert("decorators".into(), json!([{"$class": "concerto.metamodel@1.0.0.Decorator", "name": "d", "arguments": [{"$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 1.5}]}]));
        },
        &|m| {
            m.insert("extra".into(), json!({"a": [1, -0.0, "x", null]}));
        },
        &|m| {
            // `$class` moved to the end.
            if let Some(class) = m.shift_remove("$class") {
                m.insert("$class".into(), class);
            }
        },
        &|m| {
            m.remove("type");
        },
        &|m| {
            m.insert("type".into(), Value::Null);
        },
        &|m| {
            m.insert("isArray".into(), json!("yes"));
        },
        &|m| {
            m.insert(
                "superType".into(),
                json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Other"}),
            );
        },
        &|m| {
            m.insert("properties".into(), json!({}));
        },
        &|m| {
            m.remove("properties");
        },
        // Fields of the wrong type, which the shape check rejects.
        &|m| {
            m.insert("name".into(), json!(["C"]));
        },
        &|m| {
            m.insert("name".into(), json!(0));
        },
        &|m| {
            m.insert(
                "superType".into(),
                json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": null}),
            );
        },
        &|m| {
            m.insert(
                "superType".into(),
                json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier"}),
            );
        },
        &|m| {
            m.insert(
                "superType".into(),
                json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": false}),
            );
        },
        &|m| {
            m.insert("identified".into(), json!(0));
        },
        &|m| {
            m.insert(
                "identified".into(),
                json!({"$class": "concerto.metamodel@1.0.0.Identified"}),
            );
        },
        &|m| {
            m.insert(
                "identified".into(),
                json!({"$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": ""}),
            );
        },
        &|m| {
            m.insert(
                "identified".into(),
                json!({"$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "x"}),
            );
        },
        &|m| {
            m.insert(
                "identified".into(),
                json!({"$class": "IdentifiedBy", "name": "x"}),
            );
        },
        &|m| {
            m.insert(
                    "sizeValidator".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 2, "maxSize": 1}),
                );
        },
        &|m| {
            m.insert(
                "sizeValidator".into(),
                json!({"minSize": "1", "maxSize": null}),
            );
        },
        &|m| {
            m.insert(
                    "lengthValidator".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.StringLengthValidator", "minLength": 5, "maxLength": 2}),
                );
        },
        &|m| {
            m.insert(
                "lengthValidator".into(),
                json!({"minLength": [], "maxLength": "3"}),
            );
        },
        &|m| {
            m.insert("validator".into(), json!({"pattern": "^a", "flags": 7}));
        },
        &|m| {
            m.insert(
                    "validator".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.IntegerDomainValidator", "lower": 1, "upper": 0}),
                );
        },
        &|m| {
            m.insert("defaultValue".into(), json!(1.5));
        },
        &|m| {
            m.insert(
                "$class".into(),
                json!("concerto.metamodel@1.0.0.EnumProperty"),
            );
        },
        &|m| {
            m.insert(
                "$class".into(),
                json!("concerto.metamodel@1.0.0.StringProperty"),
            );
        },
        &|m| {
            m.insert(
                "$class".into(),
                json!("concerto.metamodel@1.0.0.ObjectProperty"),
            );
        },
        &|m| {
            m.insert(
                "type".into(),
                json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": ""}),
            );
        },
    ];
    for change in changes {
        edit(&[], change);
        edit(&["properties", "0"], change);
        edit(&["properties", "1"], change);
    }
}

/// Over every model AST of the benchmark sets and the oracle corpus
/// (with `CONCERTO_ORACLE_FIXTURES` set), and mutated copies of each:
/// the text and `Value` loads agree, and every AST that passes BC-19's
/// shape check is read (the drift guard: a metamodel change the typed
/// structs miss, or a shape the check accepts that the reader does not,
/// would otherwise fail every such model with `modelfile-load-unreadable`
/// on the JS API).
#[test]
fn every_shape_checked_model_is_read() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut roots = Vec::new();
    if let Ok(fixtures) = std::env::var("CONCERTO_ORACLE_FIXTURES") {
        let fixtures = PathBuf::from(fixtures);
        if let Some(oracle) = fixtures.parent() {
            roots.push(oracle.join("cto-cache"));
            if let Some(migration) = oracle.parent() {
                roots.push(migration.join("bench/fixtures/model-sets"));
            }
        }
        roots.push(fixtures);
    }
    roots.push(manifest.join("src"));
    roots.push(manifest.join("tests/typed_ast"));

    let mut files = Vec::new();
    roots.iter().for_each(|root| json_files(root, &mut files));
    let mut models = BTreeSet::new();
    for file in &files {
        if let Ok(text) = std::fs::read_to_string(file)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
        {
            collect_models(&value, &mut models);
        }
    }
    assert!(!models.is_empty());

    let (mut checked, mut loaded, mut mutants) = (0, 0, 0);
    let mut unread = Vec::new();
    let mut consider = |text: &str| {
        let outcome = check(text);
        let value: Value = serde_json::from_str(text).unwrap();
        if crate::instance::check_ast_shape(&value).is_ok() {
            checked += 1;
            loaded += usize::from(matches!(outcome, Outcome::Loaded(_)));
            if is_unreadable(&outcome) {
                unread.push(text.to_string());
            }
        }
    };
    for text in &models {
        consider(text);
        let ast: Value = serde_json::from_str(text).unwrap();
        for mutant in mutations(&ast) {
            mutants += 1;
            consider(&mutant.to_string());
        }
    }
    eprintln!(
        "typed AST read: {} models from {} files, {mutants} mutants; {checked} pass the shape check, {loaded} of them load",
        models.len(),
        files.len()
    );
    assert!(
        unread.is_empty(),
        "{} AST(s) pass the shape check but are unreadable, e.g. {}",
        unread.len(),
        unread[0]
    );
    assert!(loaded > 0);
}

/// A scalar or map declaration's variant struct is read from the
/// object's entries in place, with the same result as from a copy of
/// the object without `$class` (what the read built before).
#[test]
fn a_variant_is_read_as_from_a_copy_without_its_class() {
    use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

    use super::{strict_from_value, strict_variant_from_value};

    fn both<T: serde::de::DeserializeOwned + std::fmt::Debug>(value: &Value) {
        let mut copy = value.clone();
        if let Some(map) = copy.as_object_mut() {
            map.shift_remove("$class");
        }
        let in_place = strict_variant_from_value::<T>(value).map(|t| format!("{t:?}"));
        let from_copy = strict_from_value::<T>(&copy).map(|t| format!("{t:?}"));
        assert_eq!(in_place.is_ok(), from_copy.is_ok(), "{value}");
        if let (Ok(a), Ok(b)) = (in_place, from_copy) {
            assert_eq!(a, b, "{value}");
        }
    }
    let class = "concerto.metamodel@1.0.0.StringScalar";
    for value in [
        json!({"$class": class, "name": "S"}),
        json!({"name": "S", "$class": class, "defaultValue": "x",
                   "validator": {"$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "a", "flags": ""}}),
        json!({"$class": class, "name": "S", "extra": 1}),
        json!({"$class": class, "name": 1}),
        json!({"$class": class}),
        json!({"$class": class, "name": "S", "validator": null, "decorators": []}),
        json!({"name": "S"}),
        json!([class, "S"]),
        json!("S"),
    ] {
        both::<mm::StringScalar>(&value);
    }
    let class = "concerto.metamodel@1.0.0.MapDeclaration";
    for value in [
        json!({"$class": class, "name": "M",
                   "key": {"$class": "concerto.metamodel@1.0.0.StringMapKeyType"},
                   "value": {"$class": "concerto.metamodel@1.0.0.StringMapValueType"}}),
        json!({"$class": class, "name": "M",
                   "key": {"$class": "concerto.metamodel@1.0.0.StringMapKeyType", "x": 1},
                   "value": {"$class": "concerto.metamodel@1.0.0.StringMapValueType"}}),
        json!({"$class": class, "name": "M"}),
    ] {
        both::<mm::MapDeclaration>(&value);
    }
}

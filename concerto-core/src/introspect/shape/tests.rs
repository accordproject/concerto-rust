use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{RANGE, ast_conforms, node_conforms, optional_location};
use crate::error::Error;
use crate::instance::metamodel::{check_ast_shape, check_ast_shape_exact};
use crate::introspect::model_file::ModelFile;

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

fn string_property() -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "a",
        "isArray": false,
        "isOptional": false,
    })
}

/// An error's kind, code and message, comparable across two loads.
fn describe(err: &Error) -> String {
    format!("{:?} {} {err}", err.kind(), err.code())
}

/// What a load gives: the loaded file's namespace, or the error.
fn outcome(result: Result<Result<(ModelFile, Option<Value>), Error>, serde_json::Error>) -> String {
    match result {
        Err(e) => format!("not JSON: {e}"),
        Ok(Ok((file, imports))) => format!("loaded {} {imports:?}", file.namespace()),
        Ok(Err(e)) => describe(&e),
    }
}

/// The fold is exact for `ast`: when the typed read vouches for it the
/// full check accepts it; `check_ast_shape` gives the full check's
/// verdict and error; and the checked load of its text is the full
/// check's error, or else the unchecked load. Returns whether the read
/// vouched for it, and whether the full check accepts it.
fn assert_exact(ast: &Value) -> (bool, bool) {
    let fast = ast_conforms(ast);
    let exact = check_ast_shape_exact(ast);
    assert!(!fast || exact.is_ok(), "vouched for, but rejected: {ast}");
    assert_eq!(
        check_ast_shape(ast).map_err(|e| describe(&e)),
        exact.as_ref().map(|_| ()).map_err(describe),
        "{ast}"
    );
    let text = ast.to_string();
    let checked = outcome(ModelFile::load_text(&text, None, None, true));
    let expected = match &exact {
        Err(e) => describe(e),
        Ok(()) => outcome(ModelFile::load_text(&text, None, None, false)),
    };
    assert_eq!(checked, expected, "{text}");
    (fast, exact.is_ok())
}

/// The table of `check_ast_shape`'s rules (with BC-17, BC-19 and
/// BC-20) and where each now lives, with an AST each rule rejects:
/// every one is still rejected, with the same error, and not one of
/// them is vouched for by the typed read.
#[test]
fn every_shape_rule_has_a_new_home() {
    let with_property = |key: &str, value: Value| {
        let mut property = string_property();
        property[key] = value;
        model(json!([concept(json!([property]))]))
    };
    let with_declaration = |key: &str, value: Value| {
        let mut declaration = concept(json!([string_property()]));
        declaration[key] = value;
        model(json!([declaration]))
    };
    let type_identifier =
        |name: Value| json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": name});
    // (rule, where it lives now, an AST it rejects, the error code)
    let table: Vec<(&str, &str, Value, &str)> = vec![
        (
            "BC-17 decorators not an array",
            "typed decode (Option<Vec<Decorator>>)",
            with_declaration("decorators", json!("d")),
            "modelfile-load-decoratorsnotarray",
        ),
        (
            "BC-17 decorators not an array (property)",
            "typed decode (Option<Vec<Decorator>>)",
            with_property("decorators", json!({})),
            "modelfile-load-decoratorsnotarray",
        ),
        (
            "BC-20 super type name not a non-empty string",
            "shape::declaration_conforms",
            with_declaration("superType", type_identifier(json!(""))),
            "modelfile-load-supertypename",
        ),
        (
            "BC-20 super type name not a string",
            "typed decode (TypeIdentifier.name: String)",
            with_declaration("superType", type_identifier(json!(7))),
            "modelfile-load-supertypename",
        ),
        (
            "BC-20 name not a string",
            "typed decode (name: String)",
            with_property("name", json!(["a"])),
            "modelfile-load-namenotstring",
        ),
        (
            "BC-20 name not a string (Value node)",
            "shape::node_conforms (Ty::Name)",
            model(json!([{"$class": "concerto.metamodel@1.0.0.StringScalar", "name": 1}])),
            "modelfile-load-namenotstring",
        ),
        (
            "node rule: identified",
            "shape::identified_conforms",
            with_declaration("identified", json!({})),
            "modelfile-load-nodenotobject",
        ),
        (
            "node rule: sizeValidator",
            "typed decode (CollectionSizeValidator)",
            with_property("sizeValidator", json!(1)),
            "modelfile-load-nodenotobject",
        ),
        (
            "node rule: lengthValidator",
            "typed decode (StringLengthValidator)",
            with_property("lengthValidator", json!({"minLength": 1})),
            "modelfile-load-nodenotobject",
        ),
        (
            "node rule: validator",
            "typed decode (StringRegexValidator)",
            with_property("validator", json!([])),
            "modelfile-load-nodenotobject",
        ),
        (
            "version check",
            "shape::header_conforms (Model $class)",
            json!({"$class": "concerto.metamodel@2.0.0.Model", "namespace": "org.acme@1.0.0"}),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: unknown key",
            "typed decode (Strict) / shape::object_conforms",
            with_property("undeclared", json!(1)),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: unknown key in a decorator argument",
            "shape::node_conforms",
            with_declaration(
                "decorators",
                json!([{"$class": "concerto.metamodel@1.0.0.Decorator", "name": "d",
                    "arguments": [{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "v", "x": 1}]}]),
            ),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: unknown key in identified",
            "shape::identified_conforms",
            with_declaration(
                "identified",
                json!({"$class": "concerto.metamodel@1.0.0.Identified", "x": 1}),
            ),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: wrong $class on a TypeIdentifier",
            "shape::type_identifier",
            with_declaration(
                "superType",
                json!({"$class": "concerto.metamodel@1.0.0.AliasedType", "name": "X"}),
            ),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: wrong $class on a validator",
            "shape::property_conforms",
            with_property(
                "validator",
                json!({"$class": "concerto.metamodel@1.0.0.AliasedType", "pattern": "a", "flags": ""}),
            ),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: Integer field with a fraction",
            "shape::property_conforms (integers)",
            with_property(
                "lengthValidator",
                json!({"$class": "concerto.metamodel@1.0.0.StringLengthValidator", "minLength": 1.5}),
            ),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: name fails the identifier validator",
            "shape::is_name, then the full check",
            with_property("name", json!("1a")),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: EnumProperty in a concept",
            "shape::property_conforms",
            model(json!([concept(
                json!([{"$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "A"}])
            )])),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: required field null",
            "typed decode / shape::object_conforms (Need)",
            with_property("isArray", Value::Null),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: import unknown key",
            "shape::header_conforms (IMPORT)",
            json!({"$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0",
                    "imports": [{"$class": "concerto.metamodel@1.0.0.ImportAll", "namespace": "x@1.0.0", "x": 1}]}),
            "modelfile-load-astshape",
        ),
        (
            "metamodel: map key type of the wrong type",
            "shape::node_conforms (MAP_KEY_TYPE)",
            model(
                json!([{"$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "M",
                    "key": {"$class": "concerto.metamodel@1.0.0.StringMapValueType"},
                    "value": {"$class": "concerto.metamodel@1.0.0.StringMapValueType"}}]),
            ),
            "modelfile-load-astshape",
        ),
        (
            "tolerance: DateTimeProperty defaultValue not a string",
            "shape::property_conforms (date_time_default)",
            model(json!([concept(
                json!([{"$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "d",
                    "isArray": false, "isOptional": false, "defaultValue": 1}])
            )])),
            "modelfile-load-astshape",
        ),
    ];
    for (rule, home, ast, code) in table {
        let (fast, accepted) = assert_exact(&ast);
        assert!(!fast && !accepted, "{rule} ({home}): {ast}");
        let err = check_ast_shape(&ast).unwrap_err();
        assert_eq!(err.code(), code, "{rule} ({home})");
    }
    // BC-19's one tolerance is vouched for.
    let date_time = model(json!([concept(
        json!([{"$class": "concerto.metamodel@1.0.0.DateTimeProperty",
            "name": "d", "isArray": false, "isOptional": false, "defaultValue": "2020-01-01T00:00:00Z"}])
    )]));
    assert_eq!(assert_exact(&date_time), (true, true));
}

// -----------------------------------------------------------------------
// Differential test over every model AST available
// -----------------------------------------------------------------------

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

/// Every object node of `value`, by its path of keys and indexes.
fn object_paths(value: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match value {
        Value::Object(map) => {
            out.push(path.clone());
            for (key, child) in map {
                path.push(key.clone());
                object_paths(child, path, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                path.push(i.to_string());
                object_paths(child, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn at<'a>(value: &'a mut Value, path: &[String]) -> &'a mut Value {
    path.iter().fold(value, |node, step| match node {
        Value::Array(items) => &mut items[step.parse::<usize>().unwrap()],
        other => &mut other[step.as_str()],
    })
}

/// One-change copies of `ast` at the object node at `path`: each key
/// removed, set to `null` and to a value of every other JSON type; an
/// undeclared key added; and its `$class` removed or replaced.
fn mutants(ast: &Value, path: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    let node = at(&mut ast.clone(), path).clone();
    let Value::Object(map) = node else {
        return out;
    };
    let replacements = [
        Value::Null,
        json!(1.5),
        json!(2),
        json!(""),
        json!("x y"),
        json!("Name"),
        json!(true),
        json!([]),
        json!([null]),
        json!({}),
        json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "X"}),
    ];
    for key in map.keys() {
        let mut copy = ast.clone();
        at(&mut copy, path)
            .as_object_mut()
            .unwrap()
            .shift_remove(key);
        out.push(copy);
        for replacement in &replacements {
            let mut copy = ast.clone();
            at(&mut copy, path)[key.as_str()] = replacement.clone();
            out.push(copy);
        }
    }
    let mut copy = ast.clone();
    at(&mut copy, path)["undeclared"] = json!(null);
    out.push(copy);
    for class in [
        "concerto.metamodel@1.0.0.Identified",
        "concerto.metamodel@1.0.0.Range",
        "concerto.metamodel@1.0.0.StringScalar",
        "concerto.metamodel@1.0.0.Declaration",
        "concerto.metamodel@1.0.0.DateTimeMapKeyType",
        "concerto.metamodel@1.0.0.ImportTypes",
    ] {
        let mut copy = ast.clone();
        at(&mut copy, path)["$class"] = json!(class);
        out.push(copy);
    }
    out
}

/// Over every model AST of the benchmark sets and the oracle corpus
/// (with `CONCERTO_ORACLE_FIXTURES` set) and this crate's own, and
/// one-change mutants of every object node of a sample of them: the
/// fold is exact ([`assert_exact`]). Every unmutated model the full
/// check accepts is vouched for by the typed read (so the fold's fast
/// path is the one a well-formed model takes).
#[test]
fn the_fold_is_exact_over_every_model_and_its_mutants() {
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

    let (mut accepted, mut vouched, mut mutated, mut mutants_vouched) = (0, 0, 0, 0);
    let mut not_vouched = Vec::new();
    for (index, text) in models.iter().enumerate() {
        let ast: Value = serde_json::from_str(text).unwrap();
        let (fast, ok) = assert_exact(&ast);
        accepted += usize::from(ok);
        vouched += usize::from(fast);
        if ok && !fast {
            not_vouched.push(text.clone());
        }
        // Mutants of up to 40 object nodes of every 13th model.
        if index % 13 != 0 {
            continue;
        }
        let mut paths = Vec::new();
        object_paths(&ast, &mut Vec::new(), &mut paths);
        let stride = paths.len().div_ceil(40).max(1);
        for path in paths.iter().step_by(stride) {
            for mutant in mutants(&ast, path) {
                mutated += 1;
                mutants_vouched += usize::from(assert_exact(&mutant).0);
            }
        }
    }
    eprintln!(
        "BC-19 fold: {} models from {} files: {accepted} pass the full check, {vouched} vouched for by the typed read; {mutated} mutants, {mutants_vouched} vouched for",
        models.len(),
        files.len()
    );
    assert!(
        not_vouched.is_empty(),
        "{} model(s) pass the full check but are not vouched for, e.g. {}",
        not_vouched.len(),
        not_vouched.first().map(String::as_str).unwrap_or_default()
    );
}

/// A `location` read as a `Location` gets the verdict its
/// `Value` gets, from both reads.
#[test]
fn a_location_conforms_as_its_value_does() {
    use serde::de::DeserializeSeed;

    use crate::introspect::kept::LocationSeed;
    use crate::introspect::kept::tests::CASES;

    let mut accepted = 0;
    for text in CASES {
        let value: Value = serde_json::from_str(text).unwrap();
        let expected = value.is_null() || node_conforms(&value, RANGE);
        let mut d = serde_json::Deserializer::from_str(text);
        let from_text = LocationSeed.deserialize(&mut d).unwrap();
        let from_value = LocationSeed.deserialize(&value).unwrap();
        assert_eq!(optional_location(Some(&from_text)), expected, "{text}");
        assert_eq!(optional_location(Some(&from_value)), expected, "{text}");
        accepted += usize::from(expected);
    }
    // The usual `Range`, and `null`.
    assert_eq!(accepted, 2);
}

/// A `decorators` or `identified` value read as a [`Kept`] gets the
/// verdict its `Value` gets.
///
/// [`Kept`]: crate::introspect::kept::Kept
#[test]
fn kept_decorators_and_identified_conform_as_their_values_do() {
    use serde::de::DeserializeSeed;

    use super::{
        DECORATOR, IDENTIFIED, Nodes, decorators_conform, identified_conforms, value_conforms,
    };
    use crate::introspect::kept::KeptSeed;
    use crate::introspect::kept::tests::{DECORATOR_CASES, IDENTIFIED_CASES};

    let read = |text: &str| {
        let mut d = serde_json::Deserializer::from_str(text);
        KeptSeed.deserialize(&mut d).unwrap()
    };
    let mut accepted = 0;
    for text in DECORATOR_CASES {
        let value: Value = serde_json::from_str(text).unwrap();
        let expected = value.is_null() || value_conforms(&value, Nodes(DECORATOR));
        assert_eq!(decorators_conform(&read(text)), expected, "{text}");
        accepted += usize::from(expected);
    }
    // The first six lists, the three that differ from them only in
    // key order, a repeated key or an escaped key, and two more.
    assert_eq!(accepted, 11);
    let mut accepted = 0;
    for text in IDENTIFIED_CASES {
        let value: Value = serde_json::from_str(text).unwrap();
        let expected = value.is_null() || node_conforms(&value, IDENTIFIED);
        assert_eq!(identified_conforms(&read(text)), expected, "{text}");
        accepted += usize::from(expected);
    }
    // Both kinds, `null`, and an `IdentifiedBy` in another key order;
    // and two with a repeated key, one with escaped keys and one
    // with an escaped name.
    assert_eq!(accepted, 8);
}

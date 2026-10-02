//! P5-81 spike (accordproject/concerto-rust#425): the table decoder against
//! the derived decoder, on every model AST the spike collects (the oracle
//! corpus, the conformance models and concerto-core's test models; see
//! `spikes/p581-table-decoder/collect-models.mjs`), in the style of the
//! `kept` module's tests: the same input through both decoders, and the
//! decoded value and the accept/reject verdict compared.
//!
//! Each model, and a deterministic set of malformed variants of it (an
//! unknown key, a missing key, a wrong JSON type, `null`, `$class` moved
//! last, an unknown `$class`, the positional (array) form of a node, and a
//! duplicate key in the text), is loaded:
//! - from JSON text, with BC-19's folded shape check and without it
//!   (`ModelFile::load_text`), and from a parsed `Value`;
//! - as a whole `mm::Model`, strictly and leniently, from text and from a
//!   `Value` (the types the typed read does not reach, and lenient mode).
//!
//! An error is compared by its kind and code (P5-81: messages may differ).
//! Analysis only; runs only when `P581_MODELS` names the collected JSON
//! lines file (otherwise it reports the skip and passes).

use std::collections::BTreeMap;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::{Map, Value};

use crate::ModelFile;
use crate::introspect::Decorated;
use crate::introspect::typed_ast::{self, USE_DERIVED};

/// Everything a [`ModelFile`] holds, in a stable order (as the `typed_ast`
/// tests compare it).
fn key(file: &ModelFile) -> String {
    format!(
        "{:?}",
        (
            file.namespace(),
            file.version(),
            file.imports(),
            file.declarations(),
            file.file_name(),
            file.decorators(),
            file.concerto_version(),
            file.definitions(),
            file.is_external(),
        )
    )
}

#[derive(Debug, PartialEq)]
enum Outcome {
    NotJson,
    Loaded(String),
    Failed(String),
}

fn outcome(result: crate::Result<ModelFile>) -> Outcome {
    match result {
        Ok(file) => Outcome::Loaded(key(&file)),
        Err(e) => Outcome::Failed(format!("{:?} {}", e.kind(), e.code())),
    }
}

/// Every load's name and outcome.
type Loads = Vec<(&'static str, Outcome)>;

/// What every load of `text` gives, in one decoder.
fn loads(text: &str) -> Loads {
    let mut out = Vec::new();
    for (name, checked) in [("text", false), ("text-checked", true)] {
        let o = match ModelFile::load_text(text, None, Some("m.cto".into()), checked) {
            Err(_) => Outcome::NotJson,
            Ok(result) => outcome(result.map(|(file, _)| file)),
        };
        out.push((name, o));
    }
    let value = serde_json::from_str::<Value>(text);
    out.push((
        "value",
        match &value {
            Err(_) => Outcome::NotJson,
            Ok(value) => outcome(ModelFile::from_owned_json_with_definitions(
                value.clone(),
                None,
                Some("m.cto".into()),
            )),
        },
    ));
    let model = |r: serde_json::Result<mm::Model>| match r {
        Ok(m) => Outcome::Loaded(format!("{m:?}")),
        Err(e) => Outcome::Failed(format!("{:?}", e.classify())),
    };
    out.push(("mm-strict-text", {
        let mut d = serde_json::Deserializer::from_str(text);
        match typed_ast::decode_strict::<mm::Model, _>(&mut d) {
            Ok(m) => model(d.end().map(|()| m)),
            Err(e) => model(Err(e)),
        }
    }));
    out.push(("mm-lenient-text", model(typed_ast::lenient_from_str(text))));
    if let Ok(value) = &value {
        out.push((
            "mm-strict-value",
            model(typed_ast::decode_strict::<mm::Model, _>(value)),
        ));
        out.push((
            "mm-lenient-value",
            model(typed_ast::decode_lenient::<mm::Model, _>(value)),
        ));
    }
    out
}

/// `loads(text)` through the table decoder and through the derived one.
fn both(text: &str) -> (Loads, Loads) {
    let table = loads(text);
    USE_DERIVED.with(|d| d.set(true));
    let derived = loads(text);
    USE_DERIVED.with(|d| d.set(false));
    (table, derived)
}

#[derive(Clone, Debug)]
enum Seg {
    Key(String),
    Index(usize),
}

/// The paths of every object node in `value`, in document order.
fn object_paths(value: &Value, path: &mut Vec<Seg>, out: &mut Vec<Vec<Seg>>) {
    match value {
        Value::Object(map) => {
            out.push(path.clone());
            for (k, v) in map {
                path.push(Seg::Key(k.clone()));
                object_paths(v, path, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                object_paths(v, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn at<'a>(value: &'a mut Value, path: &[Seg]) -> &'a mut Value {
    path.iter().fold(value, |v, seg| match seg {
        Seg::Key(k) => &mut v[k.as_str()],
        Seg::Index(i) => &mut v[*i],
    })
}

/// A value of another JSON type.
fn wrong_type(value: &Value) -> Value {
    match value {
        Value::String(_) => Value::from(1),
        Value::Number(_) => Value::from("1"),
        Value::Bool(_) => Value::from("true"),
        Value::Array(_) => Value::Object(Map::new()),
        Value::Object(_) => Value::Array(Vec::new()),
        Value::Null => Value::Bool(false),
    }
}

/// The malformed variants of `ast` at the object node at `path`, as text.
fn mutations(ast: &Value, path: &[Seg]) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let node = {
        let mut copy = ast.clone();
        at(&mut copy, path).clone()
    };
    let Value::Object(map) = &node else {
        return out;
    };
    let other: Vec<&String> = map.keys().filter(|k| *k != "$class").collect();
    let mut edit = |name: &'static str, f: &dyn Fn(&mut Map<String, Value>)| {
        let mut copy = ast.clone();
        if let Value::Object(m) = at(&mut copy, path) {
            f(m);
        }
        out.push((name, copy.to_string()));
    };
    edit("unknown-key", &|m| {
        m.insert("zzUnknown".into(), Value::from(1));
    });
    if let Some(first) = other.first() {
        let first = (*first).clone();
        edit("missing-key", &|m| {
            m.shift_remove(&first);
        });
        edit("wrong-type", &|m| {
            let v = wrong_type(&m[&first]);
            m.insert(first.clone(), v);
        });
        edit("null", &|m| {
            m.insert(first.clone(), Value::Null);
        });
    }
    if let Some(last) = other.last() {
        let last = (*last).clone();
        edit("wrong-type-last", &|m| {
            let v = wrong_type(&m[&last]);
            m.insert(last.clone(), v);
        });
    }
    if map.contains_key("$class") {
        edit("class-last", &|m| {
            if let Some(c) = m.shift_remove("$class") {
                m.insert("$class".into(), c);
            }
        });
        edit("unknown-class", &|m| {
            if let Some(Value::String(c)) = m.get("$class") {
                let c = format!("{c}X");
                m.insert("$class".into(), Value::String(c));
            }
        });
        edit("duplicate-class-last", &|m| {
            // `$class` moved last, plus an unknown key first: buffered.
            if let Some(c) = m.shift_remove("$class") {
                let mut fresh = Map::new();
                fresh.insert("zzFirst".into(), Value::from(0));
                fresh.extend(std::mem::take(m));
                fresh.insert("$class".into(), c);
                *m = fresh;
            }
        });
    }
    // The positional form: the node as an array of its values.
    {
        let mut copy = ast.clone();
        let target = at(&mut copy, path);
        let values: Vec<Value> = map.values().cloned().collect();
        *target = Value::Array(values);
        out.push(("positional", copy.to_string()));
    }
    // A duplicate key in the text: the node's first key written twice.
    if let Some((k, v)) = map.iter().next() {
        let mut copy = ast.clone();
        let marker = "\u{1}p581dup\u{1}";
        if let Value::Object(m) = at(&mut copy, path) {
            let mut fresh = Map::new();
            fresh.insert(marker.to_string(), Value::Null);
            fresh.extend(m.clone());
            *m = fresh;
        }
        let text = copy.to_string();
        let marker_json = serde_json::to_string(marker).expect("a string");
        let dup = format!(
            "{}:{}",
            serde_json::to_string(k).expect("a string"),
            serde_json::to_string(v).expect("a value")
        );
        out.push((
            "duplicate-key",
            text.replacen(&format!("{marker_json}:null"), &dup, 1),
        ));
    }
    out
}

/// The first `n` characters of `s`.
fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[test]
fn table_decoder_matches_derived_decoder() {
    let Ok(path) = std::env::var("P581_MODELS") else {
        eprintln!("table_equiv: P581_MODELS is not set; skipped");
        return;
    };
    let nodes_per_model: usize = std::env::var("P581_MUTATION_NODES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(6);
    let text = std::fs::read_to_string(&path).expect("P581_MODELS is readable");
    let mut stats: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    let mut mismatches = Vec::new();
    let mut check = |source: &str, what: &str, text: &str| {
        let (table, derived) = both(text);
        let entry = stats.entry(format!("{source}/{what}")).or_default();
        entry[0] += 1;
        let accepted = table
            .iter()
            .filter(|(n, o)| *n == "text-checked" && matches!(o, Outcome::Loaded(_)))
            .count();
        entry[1] += accepted;
        entry[2] += 1 - accepted;
        if table != derived {
            let diff: Vec<String> = table
                .iter()
                .zip(&derived)
                .filter(|(a, b)| a != b)
                .map(|((n, a), (_, b))| {
                    let (a, b) = (format!("{a:?}"), format!("{b:?}"));
                    format!("{n}: table {} / derived {}", clip(&a, 200), clip(&b, 200))
                })
                .collect();
            mismatches.push(format!(
                "{source}/{what}: {}\n  {}",
                diff.join("\n  "),
                clip(text, 400)
            ));
        }
    };
    let mut models = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let entry: Value = serde_json::from_str(line).expect("a JSON line");
        let source = entry["source"].as_str().expect("a source");
        let model_text = entry["text"].as_str().expect("a text");
        models += 1;
        check(source, "as-given", model_text);
        let ast: Value = serde_json::from_str(model_text).expect("JSON");
        let mut paths = Vec::new();
        object_paths(&ast, &mut Vec::new(), &mut paths);
        let step = (paths.len() / nodes_per_model.max(1)).max(1);
        for p in paths.iter().step_by(step).take(nodes_per_model) {
            for (name, mutated) in mutations(&ast, p) {
                check(source, name, &mutated);
            }
        }
    }
    eprintln!("table_equiv: {models} models");
    eprintln!("table_equiv: case: inputs / accepted (checked text load) / rejected");
    for (k, [n, a, r]) in &stats {
        eprintln!("table_equiv: {k}: {n} / {a} / {r}");
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

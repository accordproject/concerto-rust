//! P5-42 (accordproject/concerto-rust#352): measure-only spike for the F-A
//! design report.
//!
//!   p542-native <models.json> [--iters N] [--warmup N]
//!
//! `<models.json>` is one of the P5-30 dumped inputs (the exact `models`
//! array the TS API hands `DcsManagerHandle`/`decoratorManagerExtract*`).
//! The input manager is built once, untimed, as the resident path keeps it
//! (P5-27). Then, per iteration, it times:
//!
//! - the stages of today's resident extract call, natively:
//!   `extract_total` (`dcs::extract_decorators`, all of it), and inside it
//!   `resolve` (`ModelManager::ast(resolve, system)`, the resolved `Value`
//!   tree the extractor walks) and `result_build` (`ModelManager::new` plus
//!   loading and validating the result models, as `DecoratorExtractor::extract`
//!   does); `new_mm` is `ModelManager::new` alone;
//! - what the binding does after it: `encode` (the P5-41 direct encode shape,
//!   serialised from borrowed ASTs), `stage_clone` (`stage_result`'s
//!   `ModelFile` clones) and `drop`;
//! - two prototype walks that replace the extractor's clone-and-walk
//!   (`process_models` plus the command-set building):
//!   `walk_borrowed` borrows each node's `decorators` array from the model
//!   files' own ASTs, and `walk_typed` reads the typed model (`ModelFile`,
//!   `Declaration`, `Property`, `Decorator`) and rebuilds each command from
//!   the typed decorator. Both build the same number of commands and
//!   vocabulary entries as the real extractor (checked), but neither is a
//!   parity-exact port: see README.md.
//!
//! Prints one JSON object: per stage, the median and min in microseconds.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, missing_docs)]

use std::hint::black_box;
use std::time::Instant;

use concerto_core::dcs;
use concerto_core::introspect::MapDeclaration;
use concerto_core::model_manager::AstOptions;
use concerto_core::{
    Declaration, Decorated, Decorator, DecoratorArgument, ModelFile, ModelManager, Property,
};
use serde_json::{Map, Value, json};

const EXCLUDE_NS: [&str; 3] = ["concerto@1.0.0", "concerto", "concerto.decorator@1.0.0"];
const MM: &str = "concerto.metamodel@1.0.0";
const DCS_VERSION: &str = "0.4.0";

fn source_manager(models: &[Value]) -> ModelManager {
    let mut mm = ModelManager::new().unwrap();
    for model in models {
        mm.add_model_with_definitions(model, None, None).unwrap();
    }
    mm
}

fn is_vocab(name: &str) -> bool {
    name == "Term" || name.starts_with("Term_")
}

fn target(ns: &str, decl: &str, prop: &str, map: &str) -> Value {
    let mut m = Map::new();
    m.insert(
        "$class".into(),
        Value::String(format!(
            "org.accordproject.decoratorcommands@{DCS_VERSION}.CommandTarget"
        )),
    );
    m.insert("namespace".into(), Value::String(ns.into()));
    if !decl.is_empty() {
        m.insert("declaration".into(), Value::String(decl.into()));
    }
    if !prop.is_empty() {
        m.insert("property".into(), Value::String(prop.into()));
    }
    if !map.is_empty() {
        m.insert("mapElement".into(), Value::String(map.into()));
    }
    Value::Object(m)
}

fn command(target: Value, decorator: Value) -> Value {
    json!({
        "$class": format!("org.accordproject.decoratorcommands@{DCS_VERSION}.Command"),
        "type": "UPSERT",
        "target": target,
        "decorator": decorator,
    })
}

/// Counts of what a walk produced: commands, vocabulary entries.
#[derive(Default, Debug, PartialEq, Clone, Copy)]
struct Counts {
    commands: usize,
    vocab: usize,
}

// ----- borrowed-AST walk -------------------------------------------------

struct Borrowed<'a> {
    mm: &'a ModelManager,
    commands: Vec<(&'a str, Value)>,
    vocab: Map<String, Value>,
}

impl<'a> Borrowed<'a> {
    fn node(&mut self, ns: &'a str, node: &'a Value, decl: &str, prop: &str, map: &str) {
        let Some(decorators) = node.get("decorators").and_then(Value::as_array) else {
            return;
        };
        let mut tgt: Option<Value> = None;
        for d in decorators {
            let name = d.get("name").and_then(Value::as_str).unwrap_or_default();
            if is_vocab(name) {
                let v = d
                    .get("arguments")
                    .and_then(|a| a.get(0))
                    .and_then(|a| a.get("value"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.vocab
                    .insert(format!("{ns}/{decl}/{prop}/{map}/{name}"), Value::from(v));
                continue;
            }
            let args: Vec<Value> = d
                .get("arguments")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .map(|arg| {
                            let class = arg.get("$class").and_then(Value::as_str).unwrap_or("");
                            if class.ends_with(".DecoratorTypeReference") {
                                // Resolve the type's namespace, as the
                                // resolved AST the real extractor walks has it.
                                let ty = arg.get("type");
                                let tname = ty
                                    .and_then(|t| t.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                let resolved = self.mm.resolve_type_name(ns, tname).ok();
                                json!({"$class": class, "type": {"$class": format!("{MM}.TypeIdentifier"), "name": tname, "namespace": resolved}, "isArray": arg.get("isArray")})
                            } else {
                                json!({"$class": class, "value": arg.get("value")})
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let t = tgt.get_or_insert_with(|| target(ns, decl, prop, map)).clone();
            self.commands.push((
                ns,
                command(t, json!({"$class": format!("{MM}.Decorator"), "name": name, "arguments": args})),
            ));
        }
    }

    fn walk(mut self) -> (Counts, Vec<Value>) {
        for mf in self.mm.model_files() {
            let ast = mf.ast();
            let ns = mf.namespace();
            self.node(ns, ast, "", "", "");
            for decl in ast
                .get("declarations")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                let dname = decl.get("name").and_then(Value::as_str).unwrap_or("");
                self.node(ns, decl, dname, "", "");
                if let Some(k) = decl.get("key") {
                    self.node(ns, k, dname, "", "KEY");
                }
                if let Some(v) = decl.get("value") {
                    self.node(ns, v, dname, "", "VALUE");
                }
                for p in decl
                    .get("properties")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                {
                    let pname = p.get("name").and_then(Value::as_str).unwrap_or("");
                    self.node(ns, p, dname, pname, "");
                }
            }
        }
        let counts = Counts {
            commands: self.commands.len(),
            vocab: self.vocab.len(),
        };
        (counts, command_sets(self.commands))
    }
}

/// One `DecoratorCommandSet` per namespace, in first-seen order.
fn command_sets(commands: Vec<(&str, Value)>) -> Vec<Value> {
    let mut sets: Vec<(&str, Vec<Value>)> = Vec::new();
    for (ns, c) in commands {
        match sets.iter_mut().find(|(n, _)| *n == ns) {
            Some((_, v)) => v.push(c),
            None => sets.push((ns, vec![c])),
        }
    }
    sets.into_iter()
        .map(|(ns, commands)| {
            let (name, version) = ns.split_once('@').unwrap_or((ns, ""));
            json!({
                "$class": format!("org.accordproject.decoratorcommands@{DCS_VERSION}.DecoratorCommandSet"),
                "name": name,
                "version": version,
                "commands": commands,
            })
        })
        .collect()
}

// ----- typed-model walk --------------------------------------------------

struct Typed<'a> {
    mm: &'a ModelManager,
    commands: Vec<(&'a str, Value)>,
    vocab: Map<String, Value>,
}

impl<'a> Typed<'a> {
    fn decorators(&mut self, ns: &'a str, decorators: &'a [Decorator], decl: &str, prop: &str, map: &str) {
        let mut tgt: Option<Value> = None;
        for d in decorators {
            let name = d.name();
            if is_vocab(name) {
                let v = match d.arguments().first() {
                    Some(DecoratorArgument::String(s)) => s.as_str(),
                    _ => "",
                };
                self.vocab
                    .insert(format!("{ns}/{decl}/{prop}/{map}/{name}"), Value::from(v));
                continue;
            }
            let args: Vec<Value> = d
                .arguments()
                .iter()
                .map(|arg| match arg {
                    DecoratorArgument::String(s) => {
                        json!({"$class": format!("{MM}.DecoratorString"), "value": s})
                    }
                    DecoratorArgument::Number(n) => {
                        json!({"$class": format!("{MM}.DecoratorNumber"), "value": n})
                    }
                    DecoratorArgument::Boolean(b) => {
                        json!({"$class": format!("{MM}.DecoratorBoolean"), "value": b})
                    }
                    DecoratorArgument::TypeReference(t) => {
                        let resolved = self.mm.resolve_type_name(ns, &t.name).ok();
                        json!({"$class": format!("{MM}.DecoratorTypeReference"), "type": {"$class": format!("{MM}.TypeIdentifier"), "name": t.name, "namespace": resolved}, "isArray": t.array})
                    }
                    _ => Value::Null,
                })
                .collect();
            let t = tgt.get_or_insert_with(|| target(ns, decl, prop, map)).clone();
            self.commands.push((
                ns,
                command(t, json!({"$class": format!("{MM}.Decorator"), "name": name, "arguments": args})),
            ));
        }
    }

    fn properties(&mut self, ns: &'a str, props: &'a [Property], decl: &str) {
        for p in props {
            self.decorators(ns, Decorated::decorators(p), decl, p.name(), "");
        }
    }

    fn map(&mut self, ns: &'a str, m: &'a MapDeclaration) {
        let name = m.name();
        self.decorators(ns, Decorated::decorators(m), name, "", "");
        self.decorators(ns, m.key_decorators(), name, "", "KEY");
        self.decorators(ns, m.value_decorators(), name, "", "VALUE");
    }

    fn walk(mut self) -> (Counts, Vec<Value>) {
        for mf in self.mm.model_files() {
            let mf: &'a ModelFile = mf;
            let ns = mf.namespace();
            self.decorators(ns, Decorated::decorators(mf), "", "", "");
            for decl in mf.declarations() {
                match decl {
                    Declaration::Class(c) => {
                        self.decorators(ns, c.decorators(), c.name(), "", "");
                        self.properties(ns, c.own_properties(), c.name());
                    }
                    Declaration::Enum(e) => {
                        self.decorators(ns, Decorated::decorators(decl), e.name(), "", "");
                        self.properties(ns, e.own_properties(), e.name());
                    }
                    Declaration::Scalar(s) => {
                        self.decorators(ns, s.decorators(), s.name(), "", "");
                    }
                    Declaration::Map(m) => self.map(ns, m),
                    _ => {}
                }
            }
        }
        let counts = Counts {
            commands: self.commands.len(),
            vocab: self.vocab.len(),
        };
        (counts, command_sets(self.commands))
    }
}


// ----- typed walk, encoded directly (no intermediate Value) --------------

/// One extracted non-vocabulary decorator, borrowed from the typed model.
struct Hit<'a> {
    ns: &'a str,
    decl: &'a str,
    prop: &'a str,
    map: &'static str,
    dec: &'a Decorator,
    /// The resolved namespace of each type-reference argument, in order.
    resolved: Vec<Option<String>>,
}

struct HitView<'a, 'b>(&'b Hit<'a>);

impl serde::Serialize for HitView<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        struct Target<'a, 'b>(&'b Hit<'a>);
        impl serde::Serialize for Target<'_, '_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let h = self.0;
                let mut m = s.serialize_map(None)?;
                m.serialize_entry("$class", "org.accordproject.decoratorcommands@0.4.0.CommandTarget")?;
                m.serialize_entry("namespace", h.ns)?;
                if !h.decl.is_empty() {
                    m.serialize_entry("declaration", h.decl)?;
                }
                if !h.prop.is_empty() {
                    m.serialize_entry("property", h.prop)?;
                }
                if !h.map.is_empty() {
                    m.serialize_entry("mapElement", h.map)?;
                }
                m.end()
            }
        }
        struct Args<'a, 'b>(&'b Hit<'a>);
        impl serde::Serialize for Args<'_, '_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeSeq;
                let mut seq = s.serialize_seq(None)?;
                let mut r = self.0.resolved.iter();
                for arg in self.0.dec.arguments() {
                    match arg {
                        DecoratorArgument::String(v) => seq.serialize_element(&json_arg("DecoratorString", v))?,
                        DecoratorArgument::Number(v) => seq.serialize_element(&json_arg("DecoratorNumber", v))?,
                        DecoratorArgument::Boolean(v) => seq.serialize_element(&json_arg("DecoratorBoolean", v))?,
                        DecoratorArgument::TypeReference(t) => {
                            let ns = r.next().cloned().flatten();
                            seq.serialize_element(&TypeRef(&t.name, ns.as_deref(), t.array))?
                        }
                        _ => seq.serialize_element(&())?,
                    }
                }
                seq.end()
            }
        }
        struct Arg<'v, V>(&'static str, &'v V);
        fn json_arg<'v, V>(c: &'static str, v: &'v V) -> Arg<'v, V> {
            Arg(c, v)
        }
        impl<V: serde::Serialize> serde::Serialize for Arg<'_, V> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("$class", &format_args!("concerto.metamodel@1.0.0.{}", self.0))?;
                m.serialize_entry("value", self.1)?;
                m.end()
            }
        }
        struct TypeRef<'a>(&'a str, Option<&'a str>, Option<bool>);
        impl serde::Serialize for TypeRef<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("$class", "concerto.metamodel@1.0.0.DecoratorTypeReference")?;
                m.serialize_entry("type", &serde_json::json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": self.0, "namespace": self.1}))?;
                m.serialize_entry("isArray", &self.2)?;
                m.end()
            }
        }
        struct Dec<'a, 'b>(&'b Hit<'a>);
        impl serde::Serialize for Dec<'_, '_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let mut m = s.serialize_map(Some(3))?;
                m.serialize_entry("$class", "concerto.metamodel@1.0.0.Decorator")?;
                m.serialize_entry("name", self.0.dec.name())?;
                m.serialize_entry("arguments", &Args(self.0))?;
                m.end()
            }
        }
        let mut m = s.serialize_map(Some(4))?;
        m.serialize_entry("$class", "org.accordproject.decoratorcommands@0.4.0.Command")?;
        m.serialize_entry("type", "UPSERT")?;
        m.serialize_entry("target", &Target(self.0))?;
        m.serialize_entry("decorator", &Dec(self.0))?;
        m.end()
    }
}

struct TypedDirect<'a> {
    mm: &'a ModelManager,
    hits: Vec<Hit<'a>>,
    vocab: Vec<(&'a str, &'a str, &'a str, &'static str, &'a Decorator)>,
}

impl<'a> TypedDirect<'a> {
    fn decorators(&mut self, ns: &'a str, decorators: &'a [Decorator], decl: &'a str, prop: &'a str, map: &'static str) {
        for d in decorators {
            if is_vocab(d.name()) {
                self.vocab.push((ns, decl, prop, map, d));
                continue;
            }
            let resolved = d
                .arguments()
                .iter()
                .filter_map(|a| match a {
                    DecoratorArgument::TypeReference(t) => Some(self.mm.resolve_type_name(ns, &t.name).ok()),
                    _ => None,
                })
                .collect();
            self.hits.push(Hit { ns, decl, prop, map, dec: d, resolved });
        }
    }

    fn walk(mut self) -> (Counts, String) {
        for mf in self.mm.model_files() {
            let mf: &'a ModelFile = mf;
            let ns = mf.namespace();
            self.decorators(ns, Decorated::decorators(mf), "", "", "");
            for decl in mf.declarations() {
                match decl {
                    Declaration::Class(c) => {
                        self.decorators(ns, c.decorators(), c.name(), "", "");
                        for p in c.own_properties() {
                            self.decorators(ns, Decorated::decorators(p), c.name(), p.name(), "");
                        }
                    }
                    Declaration::Enum(e) => {
                        self.decorators(ns, Decorated::decorators(decl), e.name(), "", "");
                        for p in e.own_properties() {
                            self.decorators(ns, Decorated::decorators(p), e.name(), p.name(), "");
                        }
                    }
                    Declaration::Scalar(s) => self.decorators(ns, s.decorators(), s.name(), "", ""),
                    Declaration::Map(m) => {
                        self.decorators(ns, Decorated::decorators(m), m.name(), "", "");
                        self.decorators(ns, m.key_decorators(), m.name(), "", "KEY");
                        self.decorators(ns, m.value_decorators(), m.name(), "", "VALUE");
                    }
                    _ => {}
                }
            }
        }
        let counts = Counts { commands: self.hits.len(), vocab: self.vocab.len() };
        let views: Vec<HitView> = self.hits.iter().map(HitView).collect();
        (counts, serde_json::to_string(&views).unwrap())
    }
}

// ----- the resident call's stages, as the binding runs them ------------

fn result_build(resolved: &Value) -> ModelManager {
    let mut mm = ModelManager::new().unwrap();
    for model in resolved["models"].as_array().unwrap() {
        let ns = model.get("namespace").and_then(Value::as_str).unwrap_or("");
        if EXCLUDE_NS.contains(&ns) {
            continue;
        }
        mm.add_model_with_definitions(model, None, None).unwrap();
    }
    mm.validate_models().unwrap();
    mm
}

/// The P5-41 direct encode shape: borrowed model ASTs, command sets and
/// vocabularies, straight to text.
fn encode(result: &dcs::extractor::ExtractResult) -> String {
    struct Asts<'a>(&'a ModelManager);
    impl serde::Serialize for Asts<'_> {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.collect_seq(self.0.model_files().map(ModelFile::ast))
        }
    }
    let mut out = String::new();
    out.push_str(&serde_json::to_string(&Asts(&result.model_manager)).unwrap());
    out.push_str(&serde_json::to_string(&result.decorator_command_set).unwrap());
    out.push_str(&serde_json::to_string(&result.vocabularies).unwrap());
    out
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = std::fs::read_to_string(&args[1]).expect("input file");
    let opt = |k: &str, d: &str| -> String {
        args.iter()
            .position(|a| a == k)
            .map(|i| args[i + 1].clone())
            .unwrap_or_else(|| d.to_string())
    };
    let iters: usize = opt("--iters", "30").parse().unwrap();
    let warmup: usize = opt("--warmup", "5").parse().unwrap();
    let models: Vec<Value> = serde_json::from_str(&input).unwrap();

    let t = Instant::now();
    let source = source_manager(&models);
    let cold_build_us = t.elapsed().as_secs_f64() * 1e6;
    let opts = dcs::ExtractOptions {
        remove_decorators_from_model: false,
        locale: "en".to_string(),
    };

    // Sanity: the prototypes find as many commands as the real extractor.
    let real = dcs::extract_decorators(&source, &opts).unwrap();
    let real_commands: usize = real
        .decorator_command_set
        .iter()
        .map(|s| s["commands"].as_array().map_or(0, Vec::len))
        .sum();
    let (b_counts, _) = Borrowed { mm: &source, commands: Vec::new(), vocab: Map::new() }.walk();
    let (t_counts, _) = Typed { mm: &source, commands: Vec::new(), vocab: Map::new() }.walk();
    let (d_counts, _) = TypedDirect { mm: &source, hits: Vec::new(), vocab: Vec::new() }.walk();
    drop(real);

    let names = [
        "extract_total",
        "resolve",
        "result_build",
        "new_mm",
        "encode",
        "stage_clone",
        "drop",
        "walk_borrowed",
        "walk_typed",
        "walk_typed_direct",
        "validate_only",
    ];
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); names.len()];
    for i in 0..warmup + iters {
        let mut row = [0.0f64; 11];
        let t = Instant::now();
        let result = dcs::extract_decorators(&source, &opts).unwrap();
        row[0] = t.elapsed().as_secs_f64() * 1e6;

        let t = Instant::now();
        let resolved = source
            .ast(AstOptions {
                resolve: true,
                include_system_models: true,
            })
            .unwrap();
        row[1] = t.elapsed().as_secs_f64() * 1e6;

        let t = Instant::now();
        let rebuilt = result_build(&resolved);
        row[2] = t.elapsed().as_secs_f64() * 1e6;
        drop(black_box(rebuilt));
        drop(black_box(resolved));

        let t = Instant::now();
        black_box(ModelManager::new().unwrap());
        row[3] = t.elapsed().as_secs_f64() * 1e6;

        let t = Instant::now();
        black_box(encode(&result));
        row[4] = t.elapsed().as_secs_f64() * 1e6;

        let t = Instant::now();
        let staged: Vec<ModelFile> = result
            .model_manager
            .model_files()
            .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
            .cloned()
            .collect();
        row[5] = t.elapsed().as_secs_f64() * 1e6;
        drop(black_box(staged));

        let t = Instant::now();
        drop(result);
        row[6] = t.elapsed().as_secs_f64() * 1e6;

        let t = Instant::now();
        black_box(Borrowed { mm: &source, commands: Vec::new(), vocab: Map::new() }.walk());
        row[7] = t.elapsed().as_secs_f64() * 1e6;

        let t = Instant::now();
        black_box(Typed { mm: &source, commands: Vec::new(), vocab: Map::new() }.walk());
        row[8] = t.elapsed().as_secs_f64() * 1e6;

        let t = Instant::now();
        black_box(TypedDirect { mm: &source, hits: Vec::new(), vocab: Vec::new() }.walk());
        row[9] = t.elapsed().as_secs_f64() * 1e6;

        let t = Instant::now();
        source.validate_models().unwrap();
        row[10] = t.elapsed().as_secs_f64() * 1e6;

        if i >= warmup {
            for (k, v) in row.iter().enumerate() {
                times[k].push(*v);
            }
        }
    }
    let mut out = Map::new();
    for (k, name) in names.iter().enumerate() {
        let min = times[k].iter().cloned().fold(f64::INFINITY, f64::min);
        let med = median(&mut times[k]);
        out.insert(
            (*name).to_string(),
            json!({"median_us": med.round(), "min_us": min.round()}),
        );
    }
    out.insert("cold_source_build_us".into(), json!(cold_build_us.round()));
    out.insert(
        "counts".into(),
        json!({
            "real_commands": real_commands,
            "borrowed": {"commands": b_counts.commands, "vocab": b_counts.vocab},
            "typed": {"commands": t_counts.commands, "vocab": t_counts.vocab},
            "typed_direct": {"commands": d_counts.commands, "vocab": d_counts.vocab},
        }),
    );
    println!("{}", Value::Object(out));
}

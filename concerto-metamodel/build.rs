//! Generates the metamodel types from the Concerto model ASTs.
//!
//! Each source model is downloaded from GitHub at a pinned tag and checked
//! against a pinned SHA-256 checksum. When there is no network, or the build is
//! offline (`CARGO_NET_OFFLINE=true` or `CONCERTO_METAMODEL_OFFLINE` set), the
//! vendored copy under `vendor/` is used instead; it is checked against the
//! same checksum, so both paths generate from identical input.
//!
//! The Rust types are generated here, from the model ASTs, into `OUT_DIR`:
//!
//! - a concept with no super type and no sub types becomes a struct that keeps
//!   its `$class`;
//! - an abstract concept becomes an enum tagged by `$class`, with one variant
//!   for every concrete type that extends it, directly or not;
//! - a concrete concept that has sub types and is used as a field type also
//!   becomes an enum, with a unit variant for itself;
//! - every other concrete concept becomes a struct used as a variant payload,
//!   whose `$class` is carried by the enum tag;
//! - a Concerto enum becomes a Rust enum of its values.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// A model AST the crate is generated from.
struct Source {
    /// The model's namespace, which is also its vendored file name.
    namespace: &'static str,
    /// Where to download the model from, at a pinned tag.
    url: &'static str,
    /// The SHA-256 of the file at `url`, as lowercase hex.
    sha256: &'static str,
}

const SOURCES: &[Source] = &[
    Source {
        namespace: "concerto@1.0.0",
        url: "https://raw.githubusercontent.com/accordproject/concerto/v5.0.0/packages/concerto-core/src/rootmodel.json",
        sha256: "66baa466d53e1df5f56cedc18611d798e919475ffc0aa98d988aaa2b885fbd97",
    },
    Source {
        namespace: "concerto.decorator@1.0.0",
        url: "https://raw.githubusercontent.com/accordproject/concerto/v5.0.0/packages/concerto-core/src/decoratormodel.json",
        sha256: "5c45cd6a56c048660a1969de9396d7afaca2e59d060e537ff28a892e34cc731a",
    },
    Source {
        namespace: "concerto.metamodel@1.0.0",
        url: "https://raw.githubusercontent.com/accordproject/concerto-metamodel/v3.17.0/lib/metamodel.json",
        sha256: "a1aaa07d5b27a2fe79b15300a4781deec7e87cd68979aa3ce8bd83a7c9085d8a",
    },
    Source {
        namespace: "org.accordproject.decoratorcommands@0.4.0",
        url: "https://raw.githubusercontent.com/accordproject/concerto-metamodel/v3.17.0/lib/dcsmodel.json",
        sha256: "7012b6dffb2bd169f57732bd07f52f61047be1a55a49a551802beb684d60bd88",
    },
];

/// The root namespace, whose transaction and event carry a timestamp.
const ROOT: &str = "concerto@1.0.0";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-env-changed=CARGO_NET_OFFLINE");
    println!("cargo:rerun-if-env-changed=CONCERTO_METAMODEL_OFFLINE");

    let root =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let offline = env::var("CARGO_NET_OFFLINE").is_ok_and(|v| v == "true")
        || env::var_os("CONCERTO_METAMODEL_OFFLINE").is_some();

    let models: Vec<Value> = SOURCES
        .iter()
        .map(|source| {
            let bytes = fetch(source, &root.join("vendor"), &out, offline);
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", source.namespace))
        })
        .collect();

    let types = TypeTable::new(&models);
    for model in &models {
        let namespace = str_field(model, "namespace");
        let code = types.generate(namespace);
        fs::write(out.join(format!("{}.rs", module_name(namespace))), code)
            .expect("failed to write generated source");
    }
}

/// The source's bytes, downloaded or else vendored, after checking the pinned
/// checksum. A download that does not match the checksum fails the build.
fn fetch(source: &Source, vendor: &Path, out: &Path, offline: bool) -> Vec<u8> {
    if !offline {
        match download(source.url, &out.join(format!("{}.json", source.namespace))) {
            Ok(bytes) => {
                check(source, &bytes, source.url);
                return bytes;
            }
            Err(reason) => println!(
                "cargo:warning=could not download {} ({reason}); using the vendored copy",
                source.url
            ),
        }
    }
    let path = vendor.join(format!("{}.json", source.namespace));
    let bytes =
        fs::read(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    check(source, &bytes, &path.display().to_string());
    bytes
}

fn download(url: &str, target: &Path) -> Result<Vec<u8>, String> {
    let status = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--max-time", "30", "--output"])
        .arg(target)
        .arg(url)
        .status()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !status.success() {
        return Err(format!("curl exited with {status}"));
    }
    fs::read(target).map_err(|e| e.to_string())
}

fn check(source: &Source, bytes: &[u8], origin: &str) {
    let digest = Sha256::digest(bytes);
    let actual: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert!(
        actual == source.sha256,
        "{origin} has SHA-256 {actual}, but {} is pinned to {}",
        source.namespace,
        source.sha256
    );
}

/// A concept or enum declaration, keyed in the [`TypeTable`] by its fully
/// qualified name.
struct Declared<'a> {
    namespace: &'a str,
    name: &'a str,
    ast: &'a Value,
    imports: &'a [Value],
}

impl<'a> Declared<'a> {
    fn is_enum(&self) -> bool {
        short_name(str_field(self.ast, "$class")) == "EnumDeclaration"
    }

    fn is_abstract(&self) -> bool {
        self.ast["isAbstract"].as_bool().unwrap_or(false)
    }

    fn properties(&self) -> &'a [Value] {
        self.ast["properties"].as_array().map_or(&[], Vec::as_slice)
    }

    /// Resolves a type name used in this declaration's model to its fully
    /// qualified name.
    fn resolve(&self, type_identifier: &Value) -> String {
        let name = str_field(type_identifier, "name");
        if let Some(namespace) = type_identifier["namespace"].as_str() {
            return format!("{namespace}.{name}");
        }
        for import in self.imports {
            let namespace = str_field(import, "namespace");
            let imported = match short_name(str_field(import, "$class")) {
                "ImportType" => str_field(import, "name") == name,
                "ImportTypes" => import["types"]
                    .as_array()
                    .is_some_and(|types| types.iter().any(|t| t.as_str() == Some(name))),
                other => panic!("{other} imports are not supported by the generator"),
            };
            if imported {
                return format!("{namespace}.{name}");
            }
        }
        format!("{}.{name}", self.namespace)
    }

    /// The fully qualified name of the super type, if there is one.
    fn super_type(&self) -> Option<String> {
        self.ast.get("superType").map(|t| self.resolve(t))
    }
}

struct TypeTable<'a> {
    declared: BTreeMap<String, Declared<'a>>,
    /// Declaration order, so generated items follow the models.
    order: Vec<String>,
    /// Types used as the type of a field.
    referenced: BTreeSet<String>,
}

impl<'a> TypeTable<'a> {
    fn new(models: &'a [Value]) -> Self {
        let mut declared = BTreeMap::new();
        let mut order = Vec::new();
        for model in models {
            let namespace = str_field(model, "namespace");
            let imports = model["imports"].as_array().map_or(&[][..], Vec::as_slice);
            for ast in model["declarations"].as_array().into_iter().flatten() {
                let name = str_field(ast, "name");
                let kind = short_name(str_field(ast, "$class"));
                assert!(
                    kind == "ConceptDeclaration" || kind == "EnumDeclaration",
                    "{namespace}.{name}: {kind} is not supported by the generator"
                );
                let fqn = format!("{namespace}.{name}");
                order.push(fqn.clone());
                declared.insert(
                    fqn,
                    Declared {
                        namespace,
                        name,
                        ast,
                        imports,
                    },
                );
            }
        }
        let mut table = Self {
            declared,
            order,
            referenced: BTreeSet::new(),
        };
        let referenced = table
            .declared
            .values()
            .filter(|d| !d.is_enum())
            .flat_map(|d| {
                d.properties()
                    .iter()
                    .filter_map(|p| p.get("type").map(|t| d.resolve(t)))
            })
            .collect();
        table.referenced = referenced;
        table
    }

    fn get(&self, fqn: &str) -> &Declared<'a> {
        self.declared
            .get(fqn)
            .unwrap_or_else(|| panic!("{fqn} is not declared in any source model"))
    }

    /// The chain of super types, nearest first.
    fn ancestors(&self, fqn: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = self.get(fqn).super_type();
        while let Some(parent) = current {
            current = self.get(&parent).super_type();
            chain.push(parent);
        }
        chain
    }

    fn has_subtypes(&self, fqn: &str) -> bool {
        self.declared
            .values()
            .any(|d| !d.is_enum() && d.super_type().as_deref() == Some(fqn))
    }

    /// The concrete types that extend `fqn`, directly or not, in declaration
    /// order.
    fn concrete_descendants(&self, fqn: &str) -> Vec<String> {
        self.order
            .iter()
            .filter(|candidate| {
                let d = self.get(candidate);
                !d.is_enum()
                    && !d.is_abstract()
                    && self.ancestors(candidate).iter().any(|a| a == fqn)
            })
            .cloned()
            .collect()
    }

    /// Whether the type is represented by a `$class`-tagged enum.
    fn is_tagged_enum(&self, fqn: &str) -> bool {
        let d = self.get(fqn);
        if d.is_enum() || self.concrete_descendants(fqn).is_empty() {
            return false;
        }
        d.is_abstract() || self.referenced.contains(fqn)
    }

    /// Whether the type only appears as the payload of a tagged enum variant,
    /// so its `$class` is carried by the enum's tag rather than by a field.
    fn is_variant(&self, fqn: &str) -> bool {
        self.get(fqn).super_type().is_some() || self.has_subtypes(fqn)
    }

    /// The fields of a concept, the root super type's first.
    fn fields(&self, fqn: &str) -> Vec<(&Declared<'a>, &'a Value)> {
        let mut chain = self.ancestors(fqn);
        chain.reverse();
        chain.push(fqn.to_string());
        chain
            .iter()
            .flat_map(|t| {
                let d = self.get(t);
                d.properties().iter().map(move |p| (d, p))
            })
            .collect()
    }

    fn generate(&self, namespace: &str) -> String {
        let mut code = String::new();
        for fqn in &self.order {
            let d = self.get(fqn);
            if d.namespace != namespace {
                continue;
            }
            if d.is_enum() {
                self.generate_enum(&mut code, d);
            } else if self.is_tagged_enum(fqn) {
                self.generate_tagged_enum(&mut code, fqn);
            } else {
                self.generate_struct(&mut code, fqn);
            }
        }
        code
    }

    fn generate_enum(&self, code: &mut String, d: &Declared) {
        let _ = writeln!(code, "/// `{}.{}`", d.namespace, d.name);
        code.push_str("#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]\n");
        let _ = writeln!(code, "pub enum {} {{", d.name);
        for value in d.properties() {
            let value = str_field(value, "name");
            let _ = writeln!(code, "    #[serde(rename = \"{value}\")]");
            let _ = writeln!(code, "    {},", pascal_case(value));
        }
        code.push_str("}\n\n");
    }

    fn generate_tagged_enum(&self, code: &mut String, fqn: &str) {
        let d = self.get(fqn);
        let _ = writeln!(
            code,
            "/// `{fqn}`, or any concrete type that extends it, selected by `$class`."
        );
        code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
        code.push_str("#[serde(tag = \"$class\")]\n");
        // Variants hold the AST nodes by value, as the structs do elsewhere.
        code.push_str("#[allow(clippy::large_enum_variant)]\n");
        let _ = writeln!(code, "pub enum {} {{", d.name);
        if !d.is_abstract() {
            assert!(
                self.fields(fqn).is_empty(),
                "{fqn}: a concrete type with fields and sub types cannot be a field type"
            );
            let _ = writeln!(code, "    #[serde(rename = \"{fqn}\")]");
            let _ = writeln!(code, "    {},", d.name);
        }
        for variant in self.concrete_descendants(fqn) {
            let v = self.get(&variant);
            let _ = writeln!(code, "    #[serde(rename = \"{variant}\")]");
            let _ = writeln!(
                code,
                "    {}({}),",
                v.name,
                self.rust_path(&variant, d.namespace)
            );
        }
        code.push_str("}\n\n");
    }

    fn generate_struct(&self, code: &mut String, fqn: &str) {
        let d = self.get(fqn);
        let _ = writeln!(code, "/// `{fqn}`");
        code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
        let _ = writeln!(code, "pub struct {} {{", d.name);
        if !self.is_variant(fqn) {
            code.push_str("    #[serde(rename = \"$class\")]\n    pub _class: String,\n");
        }
        if d.ast
            .get("identified")
            .is_some_and(|i| short_name(str_field(i, "$class")) == "Identified")
        {
            code.push_str("    #[serde(rename = \"$identifier\")]\n    pub _identifier: String,\n");
        }
        if d.namespace == ROOT && matches!(d.name, "Transaction" | "Event") {
            code.push_str(concat!(
                "    #[serde(\n",
                "        rename = \"$timestamp\",\n",
                "        serialize_with = \"crate::utils::serialize_datetime\",\n",
                "        deserialize_with = \"crate::utils::deserialize_datetime\"\n",
                "    )]\n",
                "    pub _timestamp: chrono::DateTime<chrono::Utc>,\n",
            ));
        }
        for (owner, property) in self.fields(fqn) {
            self.generate_field(code, fqn, owner, property);
        }
        code.push_str("}\n\n");
    }

    fn generate_field(&self, code: &mut String, fqn: &str, owner: &Declared, property: &Value) {
        let name = str_field(property, "name");
        let kind = short_name(str_field(property, "$class"));
        let mut rust = match kind {
            "StringProperty" => "String".to_string(),
            "BooleanProperty" => "bool".to_string(),
            "IntegerProperty" => "i32".to_string(),
            "LongProperty" => "i64".to_string(),
            "DoubleProperty" => "f64".to_string(),
            "ObjectProperty" => {
                let target = owner.resolve(&property["type"]);
                let t = self.get(&target);
                assert!(
                    t.is_enum() || self.is_tagged_enum(&target) || !self.is_variant(&target),
                    "{fqn}.{name}: {target} has no $class of its own to be a field type"
                );
                self.rust_path(&target, self.get(fqn).namespace)
            }
            other => panic!("{fqn}.{name}: {other} is not supported by the generator"),
        };
        if property["isArray"].as_bool().unwrap_or(false) {
            rust = format!("Vec<{rust}>");
        }
        let optional = property["isOptional"].as_bool().unwrap_or(false);
        let mut attributes = vec![format!("rename = \"{name}\"")];
        match property.get("defaultValue") {
            None => {}
            Some(Value::Bool(false)) if !optional => attributes.push("default".to_string()),
            Some(other) => panic!("{fqn}.{name}: default value {other} is not supported"),
        }
        if optional {
            rust = format!("Option<{rust}>");
            attributes.push("skip_serializing_if = \"Option::is_none\"".to_string());
        }
        let _ = writeln!(code, "    #[serde({})]", attributes.join(", "));
        let _ = writeln!(code, "    pub {}: {rust},", field_name(name));
    }

    /// The path to a generated type from the module of `namespace`.
    fn rust_path(&self, fqn: &str, namespace: &str) -> String {
        let d = self.get(fqn);
        if d.namespace == namespace {
            d.name.to_string()
        } else {
            format!("crate::{}::{}", module_name(d.namespace), d.name)
        }
    }
}

fn str_field<'v>(value: &'v Value, key: &str) -> &'v str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("expected a string `{key}` in {value}"))
}

fn short_name(class: &str) -> &str {
    class.rsplit('.').next().unwrap_or(class)
}

/// `concerto.metamodel@1.0.0` becomes `concerto_metamodel_1_0_0`.
fn module_name(namespace: &str) -> String {
    namespace.replace(['.', '@'], "_")
}

/// `resolvedName` becomes `resolved_name`; Rust keywords get a trailing `_`.
fn field_name(name: &str) -> String {
    let mut snake = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            snake.push('_');
            snake.push(c.to_ascii_lowercase());
        } else {
            snake.push(c);
        }
    }
    match snake.as_str() {
        "type" | "enum" | "struct" | "impl" | "mod" | "use" | "ref" | "self" | "super"
        | "crate" => {
            format!("{snake}_")
        }
        _ => snake,
    }
}

/// `KEY_VALUE` becomes `KeyValue`.
fn pascal_case(value: &str) -> String {
    value
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
            })
        })
        .collect()
}

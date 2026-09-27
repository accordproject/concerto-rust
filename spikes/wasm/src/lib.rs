//! Throwaway wasm-bindgen spike over `concerto-core` (concerto-rust#86).
//!
//! It exposes just enough of the `ModelManager` to answer the questions in the
//! spike: `add_model` / `validate_models`, a coarse snapshot of a whole
//! namespace (as a JSON string and as a JS object built by
//! serde-wasm-bindgen), fine-grained per-declaration getters keyed either by a
//! namespace string or by an integer handle, a few boundary probes, and two
//! ways of turning a `ConcertoError` into a JS `Error`.
//!
//! Nothing here is meant to survive into `concerto-wasm`; REPORT.md carries the
//! findings.

use std::cell::RefCell;

use concerto_core::{ConcertoError, Declaration, ModelManager, Property};
use js_sys::{Error as JsError, Function, Object};
use serde::Serialize;
use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

thread_local! {
    /// A JS function `(kind, message, props) => Error` registered by the
    /// loader. When set, errors are built by it, so they arrive in JS as
    /// instances of the loader's own `Error` subclasses.
    static ERROR_FACTORY: RefCell<Option<Function>> = const { RefCell::new(None) };
}

/// The structured form of a `ConcertoError`, as the plan's error contract
/// describes it: `{kind, message, fileName, location, ...}`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorPayload {
    kind: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    type_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}

fn payload(err: &ConcertoError) -> ErrorPayload {
    let message = err.to_string();
    let mut p = ErrorPayload {
        kind: "",
        message,
        type_name: None,
        namespace: None,
        file_name: None,
        location: None,
    };
    match err {
        ConcertoError::TypeNotFound { type_name } => {
            p.kind = "TypeNotFound";
            p.type_name = Some(type_name.clone());
        }
        ConcertoError::NamespaceNotFound { namespace } => {
            p.kind = "NamespaceNotFound";
            p.namespace = Some(namespace.clone());
        }
        ConcertoError::IllegalModel {
            file_name,
            location,
            ..
        } => {
            p.kind = "IllegalModel";
            p.file_name = file_name.clone();
            p.location = location.clone();
        }
        ConcertoError::ValidationFailed { .. } => {
            p.kind = "ValidationFailed";
        }
    }
    p
}

/// Converts a `ConcertoError` into the value thrown into JS.
///
/// With a factory registered, the factory builds the error (strategy B).
/// Otherwise a plain `Error` is built here, with `name` set to the kind and the
/// structured fields copied on as own properties (strategy A).
fn to_js(err: ConcertoError) -> JsValue {
    let p = payload(&err);
    let props = serde_wasm_bindgen::to_value(&p).unwrap_or(JsValue::NULL);
    let factory = ERROR_FACTORY.with(|f| f.borrow().clone());
    if let Some(factory) = factory {
        return factory
            .call3(
                &JsValue::NULL,
                &JsValue::from_str(p.kind),
                &JsValue::from_str(&p.message),
                &props,
            )
            .unwrap_or_else(|thrown| thrown);
    }
    let e = JsError::new(&p.message);
    e.set_name(p.kind);
    if let Ok(obj) = props.dyn_into::<Object>() {
        Object::assign(&e, &obj);
    }
    e.into()
}

/// Registers (or, with `undefined`, clears) the JS error factory.
#[wasm_bindgen(js_name = setErrorFactory)]
pub fn set_error_factory(factory: Option<Function>) {
    ERROR_FACTORY.with(|f| *f.borrow_mut() = factory);
}

/// Throws a sample of each `ConcertoError` variant, for the error-mapping
/// smoke test.
#[wasm_bindgen(js_name = throwSample)]
pub fn throw_sample(kind: &str) -> Result<(), JsValue> {
    let err = match kind {
        "TypeNotFound" => ConcertoError::TypeNotFound {
            type_name: "org.acme@1.0.0.Missing".into(),
        },
        "NamespaceNotFound" => ConcertoError::NamespaceNotFound {
            namespace: "org.missing@1.0.0".into(),
        },
        "IllegalModel" => ConcertoError::IllegalModel {
            message: "bad model".into(),
            file_name: Some("model.cto".into()),
            location: Some("line 3 column 5".into()),
        },
        _ => ConcertoError::ValidationFailed {
            message: "validation sample".into(),
        },
    };
    Err(to_js(err))
}

/// Panics inside Rust, to show what a panic looks like from JS (it is not a
/// `ConcertoError`: with `panic = "abort"` it traps).
#[wasm_bindgen(js_name = panicSample)]
pub fn panic_sample() {
    panic!("sample panic");
}

// ---------------------------------------------------------------------------
// Boundary probes
// ---------------------------------------------------------------------------

/// Does nothing: the floor cost of a JS → WASM call.
#[wasm_bindgen]
pub fn noop() {}

/// Adds two numbers: a call with scalar arguments and a scalar result.
#[wasm_bindgen]
pub fn add(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

/// Takes a string from JS: measures JS → WASM string marshalling.
#[wasm_bindgen(js_name = strLen)]
pub fn str_len(s: &str) -> usize {
    s.len()
}

/// Returns a string of `n` bytes: measures WASM → JS string marshalling.
#[wasm_bindgen(js_name = makeStr)]
pub fn make_str(n: usize) -> String {
    "x".repeat(n)
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PropertySnapshot<'a> {
    name: &'a str,
    type_name: Option<&'a str>,
    is_array: bool,
    is_optional: bool,
    is_relationship: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeclarationSnapshot<'a> {
    name: &'a str,
    kind: &'static str,
    is_abstract: bool,
    super_type: Option<&'a str>,
    properties: Vec<PropertySnapshot<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelFileSnapshot<'a> {
    namespace: &'a str,
    declarations: Vec<DeclarationSnapshot<'a>>,
}

fn prop_snapshot(p: &Property) -> PropertySnapshot<'_> {
    PropertySnapshot {
        name: p.name(),
        type_name: p.type_name(),
        is_array: p.is_array(),
        is_optional: p.is_optional(),
        is_relationship: p.is_relationship(),
    }
}

fn own_properties(d: &Declaration) -> &[Property] {
    d.as_class().map(|c| c.own_properties()).unwrap_or(&[])
}

fn decl_snapshot(d: &Declaration) -> DeclarationSnapshot<'_> {
    let class = d.as_class();
    DeclarationSnapshot {
        name: d.name(),
        kind: d.declaration_kind(),
        is_abstract: class.is_some_and(|c| c.is_abstract()),
        super_type: class.and_then(|c| c.super_type()).map(|t| t.name.as_str()),
        properties: own_properties(d).iter().map(prop_snapshot).collect(),
    }
}

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

/// A `ModelManager` with a namespace handle table beside it.
///
/// A namespace handle is an index into `namespaces`; a declaration is named by
/// `(namespace handle, declaration index)` and a property by adding a property
/// index. That stands in for the `DeclId` / `PropId` arena P1-04 will add.
#[wasm_bindgen]
pub struct Engine {
    manager: ModelManager,
    namespaces: Vec<String>,
}

impl Engine {
    fn file(&self, ns: &str) -> Result<&concerto_core::ModelFile, JsValue> {
        self.manager.model_file(ns).ok_or_else(|| {
            to_js(ConcertoError::NamespaceNotFound {
                namespace: ns.to_string(),
            })
        })
    }

    fn decl(&self, ns: &str, i: usize) -> Result<&Declaration, JsValue> {
        self.file(ns)?
            .declarations()
            .get(i)
            .ok_or_else(|| JsError::new("declaration index out of range").into())
    }

    fn prop(&self, ns: &str, i: usize, j: usize) -> Result<&Property, JsValue> {
        own_properties(self.decl(ns, i)?)
            .get(j)
            .ok_or_else(|| JsError::new("property index out of range").into())
    }

    fn ns_of(&self, h: u32) -> Result<&str, JsValue> {
        self.namespaces
            .get(h as usize)
            .map(String::as_str)
            .ok_or_else(|| JsError::new("bad namespace handle").into())
    }

    fn add(&mut self, value: &serde_json::Value, file_name: Option<String>) -> Result<(), JsValue> {
        self.manager.add_model(value, file_name).map_err(to_js)?;
        if let Some(ns) = value.get("namespace").and_then(|v| v.as_str()) {
            self.namespaces.push(ns.to_string());
        }
        Ok(())
    }
}

#[wasm_bindgen]
impl Engine {
    /// A fresh engine with the system model loaded.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<Engine, JsValue> {
        Ok(Engine {
            manager: ModelManager::new().map_err(to_js)?,
            namespaces: Vec::new(),
        })
    }

    /// Adds a model from its JSON AST, passed as a JSON string.
    #[wasm_bindgen(js_name = addModel)]
    pub fn add_model(&mut self, json: &str, file_name: Option<String>) -> Result<(), JsValue> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(|e| {
            to_js(ConcertoError::IllegalModel {
                message: e.to_string(),
                file_name: file_name.clone(),
                location: None,
            })
        })?;
        self.add(&value, file_name)
    }

    /// Adds a model from its JSON AST, passed as a JS object and converted
    /// with serde-wasm-bindgen.
    #[wasm_bindgen(js_name = addModelObject)]
    pub fn add_model_object(
        &mut self,
        ast: JsValue,
        file_name: Option<String>,
    ) -> Result<(), JsValue> {
        let value: serde_json::Value = serde_wasm_bindgen::from_value(ast)?;
        self.add(&value, file_name)
    }

    /// Runs semantic validation over every loaded model.
    #[wasm_bindgen(js_name = validateModels)]
    pub fn validate_models(&self) -> Result<(), JsValue> {
        self.manager.validate_models().map_err(to_js)
    }

    /// The integer handle of a loaded user namespace.
    #[wasm_bindgen(js_name = namespaceHandle)]
    pub fn namespace_handle(&self, ns: &str) -> Option<u32> {
        self.namespaces
            .iter()
            .position(|n| n == ns)
            .map(|i| i as u32)
    }

    // --- coarse ------------------------------------------------------------

    /// A snapshot of every user namespace, as one JSON string.
    #[wasm_bindgen(js_name = snapshotJson)]
    pub fn snapshot_json(&self) -> Result<String, JsValue> {
        let snap = self.snapshot()?;
        serde_json::to_string(&snap).map_err(|e| JsError::new(&e.to_string()).into())
    }

    /// The same snapshot, as a JS value built by serde-wasm-bindgen.
    #[wasm_bindgen(js_name = snapshotObject)]
    pub fn snapshot_object(&self) -> Result<JsValue, JsValue> {
        let snap = self.snapshot()?;
        let ser = serde_wasm_bindgen::Serializer::json_compatible();
        Ok(snap.serialize(&ser)?)
    }

    // --- fine-grained, keyed by namespace string -----------------------------

    #[wasm_bindgen(js_name = declCount)]
    pub fn decl_count(&self, ns: &str) -> Result<usize, JsValue> {
        Ok(self.file(ns)?.declarations().len())
    }

    #[wasm_bindgen(js_name = declName)]
    pub fn decl_name(&self, ns: &str, i: usize) -> Result<String, JsValue> {
        Ok(self.decl(ns, i)?.name().to_string())
    }

    #[wasm_bindgen(js_name = declKind)]
    pub fn decl_kind(&self, ns: &str, i: usize) -> Result<String, JsValue> {
        Ok(self.decl(ns, i)?.declaration_kind().to_string())
    }

    #[wasm_bindgen(js_name = propCount)]
    pub fn prop_count(&self, ns: &str, i: usize) -> Result<usize, JsValue> {
        Ok(own_properties(self.decl(ns, i)?).len())
    }

    #[wasm_bindgen(js_name = propName)]
    pub fn prop_name(&self, ns: &str, i: usize, j: usize) -> Result<String, JsValue> {
        Ok(self.prop(ns, i, j)?.name().to_string())
    }

    #[wasm_bindgen(js_name = propType)]
    pub fn prop_type(&self, ns: &str, i: usize, j: usize) -> Result<Option<String>, JsValue> {
        Ok(self.prop(ns, i, j)?.type_name().map(str::to_string))
    }

    #[wasm_bindgen(js_name = propIsOptional)]
    pub fn prop_is_optional(&self, ns: &str, i: usize, j: usize) -> Result<bool, JsValue> {
        Ok(self.prop(ns, i, j)?.is_optional())
    }

    // --- fine-grained, keyed by integer handle -------------------------------

    #[wasm_bindgen(js_name = hDeclCount)]
    pub fn h_decl_count(&self, h: u32) -> Result<usize, JsValue> {
        self.decl_count(self.ns_of(h)?)
    }

    #[wasm_bindgen(js_name = hDeclName)]
    pub fn h_decl_name(&self, h: u32, i: usize) -> Result<String, JsValue> {
        self.decl_name(self.ns_of(h)?, i)
    }

    #[wasm_bindgen(js_name = hDeclKind)]
    pub fn h_decl_kind(&self, h: u32, i: usize) -> Result<String, JsValue> {
        self.decl_kind(self.ns_of(h)?, i)
    }

    #[wasm_bindgen(js_name = hPropCount)]
    pub fn h_prop_count(&self, h: u32, i: usize) -> Result<usize, JsValue> {
        self.prop_count(self.ns_of(h)?, i)
    }

    #[wasm_bindgen(js_name = hPropName)]
    pub fn h_prop_name(&self, h: u32, i: usize, j: usize) -> Result<String, JsValue> {
        self.prop_name(self.ns_of(h)?, i, j)
    }

    #[wasm_bindgen(js_name = hPropType)]
    pub fn h_prop_type(&self, h: u32, i: usize, j: usize) -> Result<Option<String>, JsValue> {
        self.prop_type(self.ns_of(h)?, i, j)
    }

    #[wasm_bindgen(js_name = hPropIsOptional)]
    pub fn h_prop_is_optional(&self, h: u32, i: usize, j: usize) -> Result<bool, JsValue> {
        self.prop_is_optional(self.ns_of(h)?, i, j)
    }

    /// Panics while `&mut self` is borrowed, to show what happens to this
    /// object afterwards.
    #[wasm_bindgen(js_name = panicInMethod)]
    pub fn panic_in_method(&mut self) {
        self.namespaces.push("about to panic".into());
        panic!("sample panic inside a method");
    }

    /// Checks `Engine` is still usable after an error was thrown out of it.
    #[wasm_bindgen(js_name = namespaceCount)]
    pub fn namespace_count(&self) -> usize {
        self.namespaces.len()
    }
}

impl Engine {
    fn snapshot(&self) -> Result<Vec<ModelFileSnapshot<'_>>, JsValue> {
        self.namespaces
            .iter()
            .map(|ns| {
                let mf = self.file(ns)?;
                Ok(ModelFileSnapshot {
                    namespace: mf.namespace(),
                    declarations: mf.declarations().iter().map(decl_snapshot).collect(),
                })
            })
            .collect()
    }
}

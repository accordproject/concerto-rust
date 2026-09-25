//! The WASM binding of `concerto-core` for the concerto-core TS views
//! (PORTING.md section 4).
//!
//! It binds:
//! - the **handle API** (P4-01): one exported object per `ModelManager`,
//!   [`ModelManagerHandle`], over the P1-04 arena. Model files, declarations
//!   and properties cross the boundary as their dense `u32` handles
//!   (`ModelFileId`, `DeclId`, `PropId`), plain JS numbers; their state
//!   crosses as one JSON snapshot per element, which a view caches until
//!   `generation()` moves (spike REPORT §3, "Input to P1-04");
//! - the three P0-04b trial units, `ModelUtil`, `NumberValidator` and
//!   `ScalarDeclaration`, whose views still hand their JS objects back (the
//!   model graph they meet is TS until P4-06 … P4-08).
//!
//! Everything JS-shaped lives here, never in core (PORTING.md 4):
//! - **argument coercion** (3.5): each binding converts its JS arguments the
//!   way the TS member uses them, and says what it does not model;
//! - **the JS-callback [`ResolutionContext`]** (1.4): the model graph is still
//!   TS during the trial, so every collaborator call a ported member makes
//!   goes back to the JS objects it was given;
//! - **the error mapping** (2.3): an error leaves as the payload
//!   `{kind, code, params, message, location, errorType, modelFile}`, which
//!   the error factory the shim registers at load turns into the TS exception;
//! - **JS object construction**: `semver.parse` builds `versionParsed`, through
//!   a function the shim registers at load.
//!
//! Views are snapshot-based (spike REPORT §3): a call that builds an object
//! (`ScalarDeclaration.process`, the `NumberValidator` constructor) returns
//! its snapshot as JSON, the view caches it in the object's fields. The trial
//! units' later calls hand the snapshot back; a view over the arena holds a
//! handle instead.
//!
//! Strings cross the boundary as UTF-8, so a lone UTF-16 surrogate becomes
//! U+FFFD. No oracle fixture or unit test passes one.

use std::cell::RefCell;

use concerto_core::error::{ContractError, ErrorKind};
use concerto_core::instance::resource_id::ResourceId;
use concerto_core::introspect::FullyQualified;
use concerto_core::introspect::scalar::{ScalarDeclaration, ScalarValidator};
use concerto_core::introspect::validators::{NumberValidator, Validator};
use concerto_core::model_manager::{DeclId, ModelFileId, Node, PropId};
use concerto_core::model_manager::{ResolutionContext, ValidatedElement};
use concerto_core::model_util as mu;
use concerto_core::{ConcertoError, ModelManager, Named};
use js_sys::{Array, Function, JSON, Object, Reflect};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Host functions and errors
// ---------------------------------------------------------------------------

/// The JS functions the shim registers at load.
struct Host {
    /// `(payload) => Error`: builds the TS exception for an error payload.
    error_factory: Function,
    /// `semver.parse`.
    semver_parse: Function,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Registers the error factory and `semver.parse`. The shim calls it once,
/// right after loading the module.
#[wasm_bindgen(js_name = setHost)]
pub fn set_host(error_factory: Function, semver_parse: Function) {
    HOST.with(|h| {
        *h.borrow_mut() = Some(Host {
            error_factory,
            semver_parse,
        });
    });
}

/// What a binding can fail with: a JS exception raised by a callback (passed
/// through unchanged), or a core error to map.
enum Error {
    Js(JsValue),
    Contract(Box<ContractError>),
}

impl From<ContractError> for Error {
    fn from(err: ContractError) -> Self {
        Self::Contract(Box::new(err))
    }
}

impl From<ConcertoError> for Error {
    fn from(err: ConcertoError) -> Self {
        match err {
            ConcertoError::Contract(err) => Self::Contract(err),
            // The loader's errors that no unit has ported yet (the manager's
            // duplicate namespace, its circular-inheritance and handle
            // checks): they leave through the error factory like every other
            // core error, with the `pre-port` code and their message verbatim
            // (error/mod.rs, `ContractError::pre_port`), so the shim still
            // picks the TS class from `kind`.
            ConcertoError::IllegalModel {
                message, location, ..
            } => ContractError::pre_port(ErrorKind::IllegalModel, message, location).into(),
            ConcertoError::TypeNotFound { type_name } => {
                let message = format!("type not found: {type_name}");
                let mut err = ContractError::pre_port(ErrorKind::TypeNotFound, message, None);
                // `TypeNotFound` payloads carry `typeName` (table 2.3).
                err.params.push(("typeName", type_name));
                err.into()
            }
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

fn kind_name(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::IllegalModel => "IllegalModel",
        ErrorKind::TypeNotFound => "TypeNotFound",
        ErrorKind::Validator => "Validator",
        ErrorKind::Error => "Error",
        ErrorKind::JsTypeError => "JsTypeError",
    }
}

fn set(target: &Object, key: &str, value: &JsValue) {
    // Setting a data property on a fresh plain object cannot fail.
    let _ = Reflect::set(target, &JsValue::from_str(key), value);
}

/// The JS value of a JSON value (`JSON.parse` of its text).
fn to_js(value: &Value) -> JsValue {
    serde_json::to_string(value)
        .ok()
        .and_then(|text| JSON::parse(&text).ok())
        .unwrap_or(JsValue::NULL)
}

/// Turns an error into the JS value to throw. `model_file` is the JS model
/// file TS passes to an `IllegalModelException`, when the core error says TS
/// passes one.
fn throw(err: Error, model_file: Option<&JsValue>) -> JsValue {
    let err = match err {
        Error::Js(value) => return value,
        Error::Contract(err) => err,
    };
    let payload = Object::new();
    set(&payload, "kind", &JsValue::from_str(kind_name(err.kind)));
    set(&payload, "code", &JsValue::from_str(err.code));
    let params = Object::new();
    for (name, value) in &err.params {
        set(&params, name, &JsValue::from_str(value));
    }
    set(&payload, "params", &params);
    set(&payload, "message", &JsValue::from_str(&err.message()));
    let location = err.location.as_ref().map_or(JsValue::UNDEFINED, to_js);
    set(&payload, "location", &location);
    if let Some(report) = &err.validator {
        set(&payload, "errorType", &JsValue::from_str(report.error_type));
    }
    if err.model_file.is_some()
        && let Some(model_file) = model_file
    {
        set(&payload, "modelFile", model_file);
    }
    HOST.with(|h| match h.borrow().as_ref() {
        Some(host) => host
            .error_factory
            .call1(&JsValue::NULL, &payload)
            .unwrap_or_else(|thrown| thrown),
        None => js_sys::Error::new(&err.message()).into(),
    })
}

/// Runs a binding body and maps its error.
fn run<T>(body: impl FnOnce() -> Result<T>) -> std::result::Result<T, JsValue> {
    body().map_err(|e| throw(e, None))
}

// ---------------------------------------------------------------------------
// JS values
// ---------------------------------------------------------------------------

/// A V8 `TypeError`, built through the catalogue.
fn type_error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::JsTypeError, code, params).into()
}

/// A catalogue `Error`, built the same way `type_error` builds a `TypeError`.
fn plain_error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::Error, code, params).into()
}

/// JS `String(value)`.
fn js_string(value: &JsValue) -> Result<String> {
    if let Some(s) = value.as_string() {
        return Ok(s);
    }
    let string =
        Reflect::get(&js_sys::global(), &JsValue::from_str("String")).map_err(Error::Js)?;
    let string: Function = string.dyn_into().map_err(Error::Js)?;
    let text = string.call1(&JsValue::NULL, value).map_err(Error::Js)?;
    Ok(text.as_string().unwrap_or_default())
}

/// `value[name]`, with V8's error for a nullish `value`.
fn get(value: &JsValue, name: &str) -> Result<JsValue> {
    if value.is_undefined() || value.is_null() {
        let what = if value.is_undefined() {
            "undefined"
        } else {
            "null"
        };
        return Err(type_error(
            "engine-typeerror-readproperties",
            vec![("value", what.to_string()), ("property", name.to_string())],
        ));
    }
    if !value.is_object() && !value.is_function() {
        // A primitive receiver: none of the trial's collaborator calls reads
        // a property of one, so it reads as `undefined`.
        return Ok(JsValue::UNDEFINED);
    }
    Reflect::get(value, &JsValue::from_str(name)).map_err(Error::Js)
}

/// `value.name(...args)`; `expression` names the callee in a
/// "... is not a function" error.
fn call(value: &JsValue, name: &str, args: &[JsValue], expression: &str) -> Result<JsValue> {
    let method = get(value, name)?;
    let Some(method) = method.dyn_ref::<Function>() else {
        return Err(type_error(
            "engine-typeerror-notafunction",
            vec![("expression", expression.to_string())],
        ));
    };
    let list = Array::new();
    for arg in args {
        list.push(arg);
    }
    Reflect::apply(method, value, &list).map_err(Error::Js)
}

/// `value.name?.()`: `None` when the method is nullish.
fn call_optional(value: &JsValue, name: &str) -> Result<Option<JsValue>> {
    let method = get(value, name)?;
    if method.is_undefined() || method.is_null() {
        return Ok(None);
    }
    call(value, name, &[], name).map(Some)
}

/// A JS value as JSON, `None` for `undefined`. Values JSON cannot hold
/// (`NaN`, `Infinity`, functions) are not modelled: no model AST holds one.
fn to_json(value: &JsValue) -> Result<Option<Value>> {
    if value.is_undefined() {
        return Ok(None);
    }
    let text = JSON::stringify(value).map_err(Error::Js)?;
    Ok(text
        .as_string()
        .and_then(|text| serde_json::from_str(&text).ok()))
}

fn nullish(value: &JsValue) -> bool {
    value.is_undefined() || value.is_null()
}

/// `Option<bool>` as JS: `None` is `undefined`.
fn js_opt_bool(value: Option<bool>) -> JsValue {
    value.map_or(JsValue::UNDEFINED, JsValue::from_bool)
}

/// A string argument the TS member calls `method` on: a nullish value is
/// V8's property-read error, any other non-string a "not a function" error.
fn receiver(value: &JsValue, expression: &str, method: &str) -> Result<String> {
    if let Some(s) = value.as_string() {
        return Ok(s);
    }
    get(value, method)?;
    Err(type_error(
        "engine-typeerror-notafunction",
        vec![("expression", format!("{expression}.{method}"))],
    ))
}

// ---------------------------------------------------------------------------
// The JS-callback context (PORTING.md 1.4)
// ---------------------------------------------------------------------------

/// Answers collaborator calls by calling the JS objects the view passed.
struct JsContext;

impl ResolutionContext for JsContext {
    type Node = JsValue;
    type Error = Error;

    fn get_type(&self, model_file: &JsValue, type_name: Option<&str>) -> Result<Option<JsValue>> {
        let type_name = type_name.map_or(JsValue::NULL, JsValue::from_str);
        let found = call(model_file, "getType", &[type_name], "modelFile.getType")?;
        Ok((!nullish(&found)).then_some(found))
    }

    fn get_all_super_type_declarations(&self, declaration: &JsValue) -> Result<Vec<JsValue>> {
        let list = call(
            declaration,
            "getAllSuperTypeDeclarations",
            &[],
            "typeDeclaration.getAllSuperTypeDeclarations",
        )?;
        if !Array::is_array(&list) {
            return Err(type_error(
                "engine-typeerror-notafunction",
                vec![(
                    "expression",
                    "typeDeclaration.getAllSuperTypeDeclarations(...).some".to_string(),
                )],
            ));
        }
        Ok(Array::from(&list).iter().collect())
    }

    fn get_fully_qualified_name(&self, declaration: &JsValue) -> Result<String> {
        js_string(&call(
            declaration,
            "getFullyQualifiedName",
            &[],
            "type.getFullyQualifiedName",
        )?)
    }

    fn get_fully_qualified_type_name(&self, property: &JsValue) -> Result<String> {
        js_string(&call(
            property,
            "getFullyQualifiedTypeName",
            &[],
            "property.getFullyQualifiedTypeName",
        )?)
    }

    fn get_parent(&self, property: &JsValue) -> Result<JsValue> {
        call(property, "getParent", &[], "field.getParent")
    }

    fn get_model_file(&self, declaration: &JsValue) -> Result<JsValue> {
        call(declaration, "getModelFile", &[], "getModelFile")
    }

    fn get_type_name(&self, property: &JsValue) -> Result<Option<String>> {
        let type_name = call(property, "getType", &[], "field.getType")?;
        if nullish(&type_name) {
            return Ok(None);
        }
        js_string(&type_name).map(Some)
    }

    fn is_enum(&self, declaration: &JsValue) -> Result<bool> {
        Ok(call(declaration, "isEnum", &[], "typeDeclaration.isEnum")?.is_truthy())
    }

    fn is_map_declaration(&self, declaration: &JsValue) -> Result<Option<bool>> {
        Ok(call_optional(declaration, "isMapDeclaration")?.map(|v| v.is_truthy()))
    }

    fn is_scalar_declaration(&self, declaration: &JsValue) -> Result<Option<bool>> {
        Ok(call_optional(declaration, "isScalarDeclaration")?.map(|v| v.is_truthy()))
    }

    fn get_ast_class(&self, declaration: &JsValue) -> Result<Option<String>> {
        Ok(get(&get(declaration, "ast")?, "$class")?.as_string())
    }

    fn get_all_declarations(&self, model_file: &JsValue) -> Result<Vec<JsValue>> {
        let list = call(
            model_file,
            "getAllDeclarations",
            &[],
            "this.getModelFile().getAllDeclarations",
        )?;
        Ok(Array::from(&list).iter().collect())
    }
}

// ---------------------------------------------------------------------------
// ModelUtil (src/modelutil.ts)
// ---------------------------------------------------------------------------

/// TS: ModelUtil.getShortName
#[wasm_bindgen(js_name = modelUtilGetShortName)]
pub fn model_util_get_short_name(fqn: JsValue) -> std::result::Result<String, JsValue> {
    run(|| Ok(mu::get_short_name(&receiver(&fqn, "fqn", "lastIndexOf")?).to_string()))
}

/// TS: ModelUtil.getNamespace. `!fqn` covers every falsy value.
#[wasm_bindgen(js_name = modelUtilGetNamespace)]
pub fn model_util_get_namespace(fqn: JsValue) -> std::result::Result<String, JsValue> {
    run(|| {
        if !fqn.is_truthy() {
            return Ok(mu::get_namespace(None)?.to_string());
        }
        let fqn = receiver(&fqn, "fqn", "lastIndexOf")?;
        Ok(mu::get_namespace(Some(&fqn))?.to_string())
    })
}

/// TS: ModelUtil.parseNamespace. `versionParsed` is built by the registered
/// `semver.parse`, since the result must be a real `SemVer`.
#[wasm_bindgen(js_name = modelUtilParseNamespace)]
pub fn model_util_parse_namespace(
    ns: JsValue,
    options: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let disable = !nullish(&options) && get(&options, "disableVersionParsing")?.is_truthy();
        let parsed = if ns.is_truthy() {
            let ns = receiver(&ns, "ns", "split")?;
            mu::parse_namespace(Some(&ns), disable)?
        } else {
            mu::parse_namespace(None, disable)?
        };
        let out = Object::new();
        match parsed {
            mu::ParsedNamespace::NameOnly { name } => set(&out, "name", &JsValue::from_str(&name)),
            mu::ParsedNamespace::Full {
                name,
                escaped_namespace,
                version,
                version_parsed,
            } => {
                set(&out, "name", &JsValue::from_str(&name));
                set(
                    &out,
                    "escapedNamespace",
                    &JsValue::from_str(&escaped_namespace),
                );
                let version_js = version.as_deref().map_or(JsValue::NULL, JsValue::from_str);
                set(&out, "version", &version_js);
                let parsed_js = match version_parsed {
                    Some(semver) => HOST
                        .with(|h| {
                            h.borrow().as_ref().map(|host| {
                                host.semver_parse
                                    .call1(&JsValue::NULL, &JsValue::from_str(&semver.raw))
                            })
                        })
                        .unwrap_or(Ok(JsValue::NULL))
                        .map_err(Error::Js)?,
                    None => JsValue::NULL,
                };
                set(&out, "versionParsed", &parsed_js);
            }
        }
        Ok(out.into())
    })
}

/// TS: ModelUtil.importFullyQualifiedNames
#[wasm_bindgen(js_name = modelUtilImportFullyQualifiedNames)]
pub fn model_util_import_fully_qualified_names(
    imp: JsValue,
) -> std::result::Result<Array, JsValue> {
    run(|| {
        let imp = to_json(&imp)?;
        let names = mu::import_fully_qualified_names(imp.as_ref())?;
        Ok(names.iter().map(|n| JsValue::from_str(n)).collect())
    })
}

/// TS: ModelUtil.isPrimitiveType. `indexOf` is strict: a non-string is not a
/// primitive type name.
#[wasm_bindgen(js_name = modelUtilIsPrimitiveType)]
pub fn model_util_is_primitive_type(type_name: JsValue) -> bool {
    type_name
        .as_string()
        .is_some_and(|t| mu::is_primitive_type(&t))
}

/// TS: ModelUtil.isAssignableTo, over the JS model file and property. A
/// non-string `typeName` is converted with `String()`: not modelled beyond
/// that.
#[wasm_bindgen(js_name = modelUtilIsAssignableTo)]
pub fn model_util_is_assignable_to(
    model_file: JsValue,
    type_name: JsValue,
    property: JsValue,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        let type_name = js_string(&type_name)?;
        mu::is_assignable_to(&JsContext, &model_file, &type_name, &property)
    })
}

/// TS: ModelUtil.capitalizeFirstLetter
#[wasm_bindgen(js_name = modelUtilCapitalizeFirstLetter)]
pub fn model_util_capitalize_first_letter(string: JsValue) -> std::result::Result<String, JsValue> {
    run(|| {
        Ok(mu::capitalize_first_letter(&receiver(
            &string, "string", "charAt",
        )?))
    })
}

/// TS: ModelUtil.isEnum; `undefined` when the type is not found.
#[wasm_bindgen(js_name = modelUtilIsEnum)]
pub fn model_util_is_enum(field: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| Ok(js_opt_bool(mu::is_enum(&JsContext, &field)?)))
}

/// TS: ModelUtil.isMap; `undefined` when the type is not found.
#[wasm_bindgen(js_name = modelUtilIsMap)]
pub fn model_util_is_map(field: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| Ok(js_opt_bool(mu::is_map(&JsContext, &field)?)))
}

/// TS: ModelUtil.isScalar; `undefined` when the type is not found.
#[wasm_bindgen(js_name = modelUtilIsScalar)]
pub fn model_util_is_scalar(field: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| Ok(js_opt_bool(mu::is_scalar(&JsContext, &field)?)))
}

/// TS: ModelUtil.isValidIdentifier. `RegExp.prototype.test` converts its
/// argument with `String()`: `undefined` tests "undefined" (DV-002).
#[wasm_bindgen(js_name = modelUtilIsValidIdentifier)]
pub fn model_util_is_valid_identifier(name: JsValue) -> std::result::Result<bool, JsValue> {
    run(|| Ok(mu::is_valid_identifier(&js_string(&name)?)))
}

/// TS: ModelUtil.getFullyQualifiedName. A falsy namespace returns the `type`
/// argument itself, whatever it is.
#[wasm_bindgen(js_name = modelUtilGetFullyQualifiedName)]
pub fn model_util_get_fully_qualified_name(
    namespace: JsValue,
    type_name: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        if !namespace.is_truthy() {
            return Ok(type_name);
        }
        let joined = mu::get_fully_qualified_name(&js_string(&namespace)?, &js_string(&type_name)?);
        Ok(JsValue::from_str(&joined))
    })
}

/// TS: ModelUtil.removeNamespaceVersionFromFullyQualifiedName
#[wasm_bindgen(js_name = modelUtilRemoveNamespaceVersionFromFullyQualifiedName)]
pub fn model_util_remove_namespace_version_from_fully_qualified_name(
    fqn: JsValue,
) -> std::result::Result<String, JsValue> {
    run(|| {
        if !fqn.is_truthy() {
            return Ok(mu::remove_namespace_version_from_fully_qualified_name(
                None,
            )?);
        }
        let fqn = receiver(&fqn, "fqn", "lastIndexOf")?;
        Ok(mu::remove_namespace_version_from_fully_qualified_name(
            Some(&fqn),
        )?)
    })
}

/// TS: ModelUtil.isSystemProperty. `includes` is strict: a non-string is
/// never a system property.
#[wasm_bindgen(js_name = modelUtilIsSystemProperty)]
pub fn model_util_is_system_property(name: JsValue) -> bool {
    name.as_string().is_some_and(|n| mu::is_system_property(&n))
}

/// TS: ModelUtil.isPrivateSystemProperty (also used as an `Array.filter`
/// callback, so extra arguments are ignored).
#[wasm_bindgen(js_name = modelUtilIsPrivateSystemProperty)]
pub fn model_util_is_private_system_property(name: JsValue) -> bool {
    name.as_string()
        .is_some_and(|n| mu::is_private_system_property(&n))
}

/// The one key `isValidMapKey`/`isValidMapValue` read, `$class`, as JSON.
fn class_node(node: &JsValue) -> Result<Option<Value>> {
    if node.is_undefined() {
        return Ok(None);
    }
    if node.is_null() {
        return Ok(Some(Value::Null));
    }
    let class = get(node, "$class")?;
    Ok(Some(match class.as_string() {
        Some(class) => json!({ "$class": class }),
        None => json!({}),
    }))
}

/// TS: ModelUtil.isValidMapKey
#[wasm_bindgen(js_name = modelUtilIsValidMapKey)]
pub fn model_util_is_valid_map_key(key: JsValue) -> std::result::Result<bool, JsValue> {
    run(|| Ok(mu::is_valid_map_key(class_node(&key)?.as_ref())?))
}

/// TS: ModelUtil.isValidMapKeyScalar; `undefined` for a nullish declaration.
#[wasm_bindgen(js_name = modelUtilIsValidMapKeyScalar)]
pub fn model_util_is_valid_map_key_scalar(decl: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let decl = (!nullish(&decl)).then_some(decl);
        Ok(js_opt_bool(mu::is_valid_map_key_scalar(
            &JsContext,
            decl.as_ref(),
        )?))
    })
}

/// TS: ModelUtil.isValidMapValue
#[wasm_bindgen(js_name = modelUtilIsValidMapValue)]
pub fn model_util_is_valid_map_value(value: JsValue) -> std::result::Result<bool, JsValue> {
    run(|| Ok(mu::is_valid_map_value(class_node(&value)?.as_ref())?))
}

// ---------------------------------------------------------------------------
// ResourceId (src/model/resourceid.ts)
// ---------------------------------------------------------------------------

/// TS: ResourceId.fromURI. `legacyNamespace`/`legacyType` are the optional,
/// nullable legacy-format arguments; a nullish value is `None`, matching how
/// TS reads an omitted parameter.
#[wasm_bindgen(js_name = resourceIdFromURI)]
pub fn resource_id_from_uri(
    uri: JsValue,
    legacy_namespace: JsValue,
    legacy_type: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        // TS's private `parseUri` calls `uri.match(...)`, which throws a
        // `TypeError` for a non-string `uri`; `fromURI` catches that and
        // reports it as the same "Invalid URI" error a malformed string
        // produces, keyed on `String(uri)`. A non-string `uri` must not be
        // silently coerced into a valid id (PORTING.md 1.4 / P4-03 review).
        let uri = uri.as_string().ok_or_else(|| {
            js_string(&uri)
                .map(|rendered| {
                    plain_error("resourceid-fromuri-invaliduri", vec![("uri", rendered)])
                })
                .unwrap_or_else(|e| e)
        })?;
        let legacy_namespace = if nullish(&legacy_namespace) {
            None
        } else {
            Some(js_string(&legacy_namespace)?)
        };
        let legacy_type = if nullish(&legacy_type) {
            None
        } else {
            Some(js_string(&legacy_type)?)
        };
        let id = ResourceId::from_uri(&uri, legacy_namespace.as_deref(), legacy_type.as_deref())?;
        let out = Object::new();
        set(&out, "namespace", &JsValue::from_str(&id.namespace));
        set(&out, "type", &JsValue::from_str(&id.type_name));
        set(&out, "id", &JsValue::from_str(&id.id));
        Ok(out.into())
    })
}

/// TS: ResourceId.prototype.toURI. Takes the view's `namespace`/`type`/`id`
/// fields rather than a handle: `ResourceId` is a plain value object (the
/// ledger's HYBRID constructor row), so the view still holds its own state
/// and only the URI encoding runs in Rust.
#[wasm_bindgen(js_name = resourceIdToURI)]
pub fn resource_id_to_uri(
    namespace: JsValue,
    type_name: JsValue,
    id: JsValue,
) -> std::result::Result<String, JsValue> {
    run(|| {
        let namespace = js_string(&namespace)?;
        let type_name = js_string(&type_name)?;
        let id = js_string(&id)?;
        let resource = ResourceId::new(namespace, type_name, id)?;
        Ok(resource.to_uri())
    })
}

// ---------------------------------------------------------------------------
// NumberValidator (src/introspect/numbervalidator.ts)
// ---------------------------------------------------------------------------

/// The element a JS validator is attached to, read through the validator the
/// way `Validator` does: `this.field` and `this.getFieldOrScalarDeclaration()`.
/// This is the collaborator-call path of the `needs_fallback` constructor row
/// (the tests build it over `sinon.createStubInstance(Field)`).
struct JsElement<'a> {
    validator: &'a JsValue,
}

impl ValidatedElement for JsElement<'_> {
    fn default_value(&self) -> Result<Option<Value>> {
        // `this.field?.ast?.defaultValue`
        let field = get(self.validator, "field")?;
        if nullish(&field) {
            return Ok(None);
        }
        let ast = get(&field, "ast")?;
        if nullish(&ast) {
            return Ok(None);
        }
        to_json(&get(&ast, "defaultValue")?)
    }

    fn name(&self) -> Result<String> {
        // `this.field.getName()`
        let field = get(self.validator, "field")?;
        js_string(&call(&field, "getName", &[], "this.field.getName")?)
    }
}

impl FullyQualified for JsElement<'_> {
    type Error = Error;

    fn fully_qualified_name(&self) -> Result<String> {
        let element = call(
            self.validator,
            "getFieldOrScalarDeclaration",
            &[],
            "this.getFieldOrScalarDeclaration",
        )?;
        js_string(&call(
            &element,
            "getFullyQualifiedName",
            &[],
            "this.getFieldOrScalarDeclaration(...).getFullyQualifiedName",
        )?)
    }
}

/// The validator's snapshot, read back from the view's fields.
fn number_validator(view: &JsValue, lower: &str, upper: &str) -> Result<NumberValidator> {
    let read = |name: &str| -> Result<Value> {
        let value = if name.starts_with("get") {
            call(view, name, &[], name)?
        } else {
            get(view, name)?
        };
        Ok(to_json(&value)?.unwrap_or(Value::Null))
    };
    let snapshot = json!({ "lowerBound": read(lower)?, "upperBound": read(upper)? });
    serde_json::from_value(snapshot)
        .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
}

/// The `{lowerBound, upperBound}` snapshot as a JS object.
fn number_snapshot(validator: &NumberValidator) -> JsValue {
    serde_json::to_value(validator).map_or(JsValue::NULL, |v| to_js(&v))
}

/// TS: NumberValidator constructor, after `super(field, ast)`. `view` is the
/// object under construction; the result is its `{lowerBound, upperBound}`.
/// `ast.lower`/`ast.upper` are read only when they are own properties.
#[wasm_bindgen(js_name = numberValidatorNew)]
pub fn number_validator_new(view: JsValue, ast: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let own = |key: &str| -> Result<Option<Value>> {
            // `Object.prototype.hasOwnProperty.call(ast, key)`
            if !ast.is_object()
                || !Object::has_own(ast.unchecked_ref::<Object>(), &JsValue::from_str(key))
            {
                return Ok(None);
            }
            Ok(Some(to_json(&get(&ast, key)?)?.unwrap_or(Value::Null)))
        };
        let mut node = serde_json::Map::new();
        for key in ["lower", "upper"] {
            if let Some(value) = own(key)? {
                node.insert(key.to_string(), value);
            }
        }
        let validator =
            NumberValidator::new(&JsElement { validator: &view }, &Value::Object(node))?;
        Ok(number_snapshot(&validator))
    })
}

/// TS: NumberValidator.validate. `null` is always accepted; any other value
/// is compared as JS `<`/`>` would, through `Number(value)` (ResourceValidator
/// only passes finite numbers). A non-null identifier prints as `String(id)`.
#[wasm_bindgen(js_name = numberValidatorValidate)]
pub fn number_validator_validate(
    view: JsValue,
    identifier: JsValue,
    value: JsValue,
) -> std::result::Result<(), JsValue> {
    run(|| {
        let validator = number_validator(&view, "lowerBound", "upperBound")?;
        let identifier = if identifier.is_null() {
            None
        } else {
            Some(js_string(&identifier)?)
        };
        let value = if value.is_null() {
            None
        } else {
            // `+value`: JS ToNumber.
            Some(value.unchecked_into_f64())
        };
        validator.validate(
            &JsElement { validator: &view },
            identifier.as_deref(),
            value,
        )
    })
}

/// TS: NumberValidator.toString
#[wasm_bindgen(js_name = numberValidatorToString)]
pub fn number_validator_to_string(view: JsValue) -> std::result::Result<String, JsValue> {
    run(|| Ok(number_validator(&view, "lowerBound", "upperBound")?.to_string()))
}

/// TS: NumberValidator.compatibleWith. `number_validator_class` is the
/// `NumberValidator` class, for the `other instanceof NumberValidator` check;
/// the bounds are read through the getters, as TS does.
#[wasm_bindgen(js_name = numberValidatorCompatibleWith)]
pub fn number_validator_compatible_with(
    view: JsValue,
    other: JsValue,
    number_validator_class: Function,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        // `other instanceof NumberValidator`
        let prototype = get(&number_validator_class, "prototype")?;
        if !other.is_object() || !prototype.unchecked_ref::<Object>().is_prototype_of(&other) {
            return Ok(false);
        }
        let this = number_validator(&view, "getLowerBound", "getUpperBound")?;
        let other = number_validator(&other, "getLowerBound", "getUpperBound")?;
        Ok(this.compatible_with(Some(&Validator::Number(other))))
    })
}

// ---------------------------------------------------------------------------
// ScalarDeclaration (src/introspect/scalardeclaration.ts)
// ---------------------------------------------------------------------------

/// TS: ScalarDeclaration.process, after `super.process()`. Returns the
/// snapshot `{type, validator, defaultValue}`, where `validator` is `null`,
/// `{kind: "NumberValidator", lowerBound, upperBound}`, or
/// `{kind: "StringValidator"}` (the view builds the TS `StringValidator`
/// until P2-02 ports it).
#[wasm_bindgen(js_name = scalarDeclarationProcess)]
pub fn scalar_declaration_process(declaration: JsValue) -> std::result::Result<JsValue, JsValue> {
    let body = || -> Result<JsValue> {
        let ast = to_json(&get(&declaration, "ast")?)?.unwrap_or(Value::Null);
        let fqn = || {
            js_string(&call(
                &declaration,
                "getFullyQualifiedName",
                &[],
                "this.getFullyQualifiedName",
            )?)
        };
        let processed = ScalarDeclaration::process(&ast, None, &fqn)?;
        let validator = match &processed.validator {
            None => Value::Null,
            Some(ScalarValidator::Number(v)) => {
                let mut snapshot = serde_json::to_value(v).unwrap_or(Value::Null);
                if let Value::Object(map) = &mut snapshot {
                    map.insert("kind".to_string(), json!("NumberValidator"));
                }
                snapshot
            }
            Some(ScalarValidator::String { .. }) => json!({ "kind": "StringValidator" }),
        };
        Ok(to_js(&json!({
            "type": processed.scalar_type,
            "validator": validator,
            "defaultValue": processed.default_value,
        })))
    };
    body().map_err(|e| {
        let model_file = get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

/// TS: ScalarDeclaration.validate, after `super.validate()`.
#[wasm_bindgen(js_name = scalarDeclarationValidate)]
pub fn scalar_declaration_validate(declaration: JsValue) -> std::result::Result<(), JsValue> {
    run(|| ScalarDeclaration::validate(&JsContext, &declaration))
}

/// TS: ScalarDeclaration.toString
#[wasm_bindgen(js_name = scalarDeclarationToString)]
pub fn scalar_declaration_to_string(declaration: JsValue) -> std::result::Result<String, JsValue> {
    run(|| {
        let fqn = js_string(&call(
            &declaration,
            "getFullyQualifiedName",
            &[],
            "this.getFullyQualifiedName",
        )?)?;
        Ok(ScalarDeclaration::to_string(&fqn))
    })
}

// ---------------------------------------------------------------------------
// The handle API: one object per ModelManager (P4-01)
// ---------------------------------------------------------------------------

/// A handle that names nothing in this manager, as the arena reports one
/// (`model_manager.rs`, `unknown`): a `TypeNotFound` naming the node.
fn unknown(node: Node) -> Error {
    ConcertoError::TypeNotFound {
        type_name: format!("{node:?}"),
    }
    .into()
}

/// A snapshot as the JSON text that crosses the boundary: one string per
/// element, which the view parses once (spike REPORT §3: JSON text beats
/// serde-wasm-bindgen and per-field getters for trees).
fn snapshot(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
}

/// A `ModelManager`, exported to JS as one object (spike "Input to P1-04",
/// point 1). The model files, declarations and properties it holds are
/// addressed by the arena's dense `u32` handles, which cross the boundary as
/// plain numbers and keep naming the same element for the life of the
/// manager (PORTING.md 1.4). An element's state crosses as a JSON snapshot,
/// which a view caches until [`ModelManagerHandle::generation`] changes
/// (PORTING.md 1.5).
///
/// wasm-bindgen registers a `FinalizationRegistry`, so a view need not call
/// `free()`; after `free()`, every call throws.
#[wasm_bindgen]
pub struct ModelManagerHandle {
    manager: ModelManager,
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// A fresh manager with the `concerto@1.0.0` system model loaded.
    #[wasm_bindgen(constructor)]
    pub fn new() -> std::result::Result<ModelManagerHandle, JsValue> {
        run(|| {
            Ok(Self {
                manager: ModelManager::new()?,
            })
        })
    }

    /// Loads a model from its JSON AST, passed as JSON text (the view calls
    /// `JSON.stringify(ast)`: spike REPORT §3), and returns the handle of its
    /// model file. Malformed JSON throws a JS `SyntaxError`.
    #[wasm_bindgen(js_name = addModel)]
    pub fn add_model(
        &mut self,
        ast: &str,
        file_name: Option<String>,
    ) -> std::result::Result<u32, JsValue> {
        run(|| {
            let value: Value = serde_json::from_str(ast)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
            self.manager.add_model(&value, file_name)?;
            // `add_model` read the namespace from this AST, so it is there.
            let namespace = value
                .get("namespace")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.manager
                .model_file_id(namespace)
                .map(ModelFileId::index)
                .ok_or_else(|| {
                    ConcertoError::TypeNotFound {
                        type_name: namespace.to_string(),
                    }
                    .into()
                })
        })
    }

    /// Validates every loaded user model; throws the first problem found.
    #[wasm_bindgen(js_name = validateModels)]
    pub fn validate_models(&self) -> std::result::Result<(), JsValue> {
        run(|| Ok(self.manager.validate_models()?))
    }

    /// The mutation counter: a snapshot taken at one generation is current
    /// while the generation is unchanged. A JS number (exact up to 2^53).
    pub fn generation(&self) -> f64 {
        // Precision loss only past 2^53 mutations.
        #[allow(clippy::cast_precision_loss)]
        let generation = self.manager.generation() as f64;
        generation
    }

    /// The handle of the model file for a namespace; `undefined` if none.
    #[wasm_bindgen(js_name = modelFileId)]
    pub fn model_file_id(&self, namespace: &str) -> Option<u32> {
        self.manager
            .model_file_id(namespace)
            .map(ModelFileId::index)
    }

    /// The handles of every loaded model file, the system model included,
    /// in load order.
    #[wasm_bindgen(js_name = modelFileIds)]
    pub fn model_file_ids(&self) -> Vec<u32> {
        self.manager
            .model_files()
            .filter_map(|file| self.manager.model_file_id(file.namespace()))
            .map(ModelFileId::index)
            .collect()
    }

    /// The handle of a declaration, by its exact fully-qualified name;
    /// `undefined` if none.
    #[wasm_bindgen(js_name = declarationId)]
    pub fn declaration_id(&self, fqn: &str) -> Option<u32> {
        self.manager.declaration_id(fqn).map(DeclId::index)
    }

    /// The handles of a model file's declarations, in file order; empty for
    /// a handle that names nothing.
    #[wasm_bindgen(js_name = declarationIds)]
    pub fn declaration_ids(&self, model_file: u32) -> Vec<u32> {
        self.manager
            .declaration_ids(ModelFileId::from_index(model_file))
            .map(DeclId::index)
            .collect()
    }

    /// The handles of a class declaration's own properties, in declaration
    /// order; empty for any other declaration, or a handle that names nothing.
    #[wasm_bindgen(js_name = propertyIds)]
    pub fn property_ids(&self, declaration: u32) -> Vec<u32> {
        self.manager
            .property_ids(DeclId::from_index(declaration))
            .map(PropId::index)
            .collect()
    }

    /// The handle of a declaration's model file; `undefined` if the handle
    /// names nothing.
    #[wasm_bindgen(js_name = modelFileOf)]
    pub fn model_file_of(&self, declaration: u32) -> Option<u32> {
        self.manager
            .model_file_of(DeclId::from_index(declaration))
            .map(ModelFileId::index)
    }

    /// The handle of a property's declaration; `undefined` if the handle
    /// names nothing.
    #[wasm_bindgen(js_name = parentOf)]
    pub fn parent_of(&self, property: u32) -> Option<u32> {
        self.manager
            .parent_of(PropId::from_index(property))
            .map(DeclId::index)
    }

    /// A model file's snapshot, as JSON text:
    /// `{namespace, version, fileName, ast}`. `fileName` is `null` when the
    /// file has none; `ast` is the AST as it was loaded (OD-3).
    #[wasm_bindgen(js_name = modelFileSnapshot)]
    pub fn model_file_snapshot(&self, model_file: u32) -> std::result::Result<String, JsValue> {
        run(|| {
            let id = ModelFileId::from_index(model_file);
            let file = self
                .manager
                .file(id)
                .ok_or_else(|| unknown(Node::ModelFile(id)))?;
            snapshot(&json!({
                "namespace": file.namespace(),
                "version": file.version(),
                "fileName": file.file_name(),
                "ast": file.ast(),
            }))
        })
    }

    /// A declaration's snapshot, as JSON text:
    /// `{name, fullyQualifiedName, modelFile, ast}`, where `modelFile` is the
    /// handle of its model file and `ast` its node of the model file's AST.
    #[wasm_bindgen(js_name = declarationSnapshot)]
    pub fn declaration_snapshot(&self, declaration: u32) -> std::result::Result<String, JsValue> {
        run(|| {
            let id = DeclId::from_index(declaration);
            let (file_id, file, found) = self.declaration_parts(id)?;
            let ast = self.declaration_ast(id)?;
            snapshot(&json!({
                "name": found.name(),
                "fullyQualifiedName": mu::get_fully_qualified_name(file.namespace(), found.name()),
                "modelFile": file_id.index(),
                "ast": ast,
            }))
        })
    }

    /// A property's snapshot, as JSON text: `{name, declaration, ast}`, where
    /// `declaration` is the handle of the declaration it belongs to and `ast`
    /// its node of that declaration's AST.
    #[wasm_bindgen(js_name = propertySnapshot)]
    pub fn property_snapshot(&self, property: u32) -> std::result::Result<String, JsValue> {
        run(|| {
            let id = PropId::from_index(property);
            let missing = || unknown(Node::Property(id));
            let found = self.manager.property(id).ok_or_else(missing)?;
            let parent = self.manager.parent_of(id).ok_or_else(missing)?;
            let index = self
                .manager
                .property_ids(parent)
                .position(|p| p == id)
                .ok_or_else(missing)?;
            let ast = self
                .declaration_ast(parent)?
                .get("properties")
                .and_then(|properties| properties.get(index))
                .ok_or_else(missing)?;
            snapshot(&json!({
                "name": found.name(),
                "declaration": parent.index(),
                "ast": ast,
            }))
        })
    }
}

impl ModelManagerHandle {
    /// A declaration, with its model file and that file's handle.
    fn declaration_parts(
        &self,
        id: DeclId,
    ) -> Result<(
        ModelFileId,
        &concerto_core::ModelFile,
        &concerto_core::Declaration,
    )> {
        let missing = || unknown(Node::Declaration(id));
        let found = self.manager.declaration(id).ok_or_else(missing)?;
        let file_id = self.manager.model_file_of(id).ok_or_else(missing)?;
        let file = self.manager.file(file_id).ok_or_else(missing)?;
        Ok((file_id, file, found))
    }

    /// A declaration's node of its model file's AST.
    fn declaration_ast(&self, id: DeclId) -> Result<&Value> {
        let missing = || unknown(Node::Declaration(id));
        let (file_id, file, _) = self.declaration_parts(id)?;
        let index = self
            .manager
            .declaration_ids(file_id)
            .position(|d| d == id)
            .ok_or_else(missing)?;
        file.ast()
            .get("declarations")
            .and_then(|declarations| declarations.get(index))
            .ok_or_else(missing)
    }
}

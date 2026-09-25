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
//!   model graph they meet is TS until P4-06 … P4-08);
//! - `Decorator` and `Decorated` (P4-05): `Decorator.process` delegates to
//!   the P2-07 port ([`Decorator::from_ast`]) directly, needing no
//!   collaborator call; `Decorator.validate`'s argument and type-reference
//!   checks, and `Decorated.validate`'s duplicate-decorator check, are new
//!   code here rather than a binding of the existing (concrete-`ModelManager`)
//!   `Decorator::validate`, because the model graph these views meet is still
//!   TS (same reason as the trial units, above): they read [`JsContext`]
//!   collaborators the same way the trial units do, following the TS source
//!   directly rather than the native method's `ModelManager`-specific
//!   shortcuts. `Decorated.process`'s `DecoratorFactory` selection is not
//!   bound: that stays TS (decorator.rs module doc).
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
use std::collections::HashSet;

use concerto_core::error::{ContractError, ErrorKind};
use concerto_core::instance::dayjs::{Dayjs, UtcOffset};
use concerto_core::instance::resource_id::ResourceId;
use concerto_core::instance::{
    Instance, InstanceEnv, InstanceKind, JsValue as CoreValue, Serializer, SerializerOptions,
};
use concerto_core::instance::{generator, populator};
use concerto_core::introspect::FullyQualified;
use concerto_core::introspect::decorator::{Decorator, DecoratorArgument};
use concerto_core::introspect::field;
use concerto_core::introspect::property;
use concerto_core::introspect::scalar::{ScalarDeclaration, ScalarValidator};
use concerto_core::introspect::validators::{
    CollectionSizeValidator, NumberValidator, StringValidator, Validator,
};
use concerto_core::model_manager::{DeclId, ModelFileId, Node, PropId};
use concerto_core::model_manager::{ResolutionContext, ValidatedElement};
use concerto_core::model_util as mu;
use concerto_core::{ConcertoError, ModelManager, Named};
use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
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
        ErrorKind::Validation => "Validation",
        ErrorKind::Error => "Error",
        ErrorKind::JsTypeError => "JsTypeError",
        ErrorKind::JsRangeError => "JsRangeError",
        ErrorKind::Metamodel => "Metamodel",
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
// StringValidator (src/introspect/stringvalidator.ts) and
// CollectionSizeValidator (src/introspect/collectionsizevalidator.ts) (P4-04)
// ---------------------------------------------------------------------------
//
// Neither Rust type derives `Serialize`/`Deserialize` (`StringValidator` owns
// a compiled `regress::Regex`, which does not), so unlike `NumberValidator`
// there is no snapshot to deserialise a validator back from on every
// `validate`/`compatibleWith` call. Instead each call rebuilds the validator
// from the view's own cached AST (`view.validator`, the regex AST `super()`
// stored; length/size bounds read back from the snapshot the constructor
// cached), the same inputs the constructor itself validated, so the rebuild
// is deterministic and never observably re-runs a check that could now fail
// differently.

/// Tags a plain JS AST object (as the unit tests and the TS views hand it
/// across, with no `$class`) with the metamodel type it is, so it
/// deserialises into the typed AST the core validators take.
fn tag(mut json: Value, class: &str) -> Value {
    if let Value::Object(map) = &mut json {
        map.entry("$class".to_string())
            .or_insert_with(|| json!(class));
    }
    json
}

/// `{pattern, flags}`, or `None` for a nullish value. `flags` defaults to
/// `""`, matching `IStringRegexValidator.flags?: string` (the TS view's
/// callers, including the unit tests, often omit it for "no flags").
fn string_regex_ast(value: &JsValue) -> Result<Option<mm::StringRegexValidator>> {
    if nullish(value) {
        return Ok(None);
    }
    let mut json = tag(
        to_json(value)?.unwrap_or(Value::Null),
        "concerto.metamodel@1.0.0.StringRegexValidator",
    );
    if let Value::Object(map) = &mut json {
        map.entry("flags".to_string()).or_insert_with(|| json!(""));
    }
    serde_json::from_value(json)
        .map(Some)
        .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
}

/// `{minLength, maxLength}`, or `None` for a nullish value.
fn string_length_ast(value: &JsValue) -> Result<Option<mm::StringLengthValidator>> {
    if nullish(value) {
        return Ok(None);
    }
    let json = tag(
        to_json(value)?.unwrap_or(Value::Null),
        "concerto.metamodel@1.0.0.StringLengthValidator",
    );
    serde_json::from_value(json)
        .map(Some)
        .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
}

/// `{minSize, maxSize}`.
fn collection_size_ast(value: &JsValue) -> Result<mm::CollectionSizeValidator> {
    let json = tag(
        to_json(value)?.unwrap_or(Value::Null),
        "concerto.metamodel@1.0.0.CollectionSizeValidator",
    );
    serde_json::from_value(json).map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
}

/// TS: StringValidator constructor, after `super(field, validator)`. `view`
/// is the object under construction; the result is its `{minLength,
/// maxLength}` snapshot. The view builds its own cached `RegExp` from
/// `validator` afterwards: `StringValidator.getRegex` stays TS (public API
/// returns a live `RegExp`), and the pluggable `options.regExp` hook is never
/// reached here (the view only calls this binding when no hook is
/// configured, PORTING.md section 3).
#[wasm_bindgen(js_name = stringValidatorNew)]
pub fn string_validator_new(
    view: JsValue,
    validator: JsValue,
    length_validator: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let regex_ast = string_regex_ast(&validator)?;
        let length_ast = string_length_ast(&length_validator)?;
        let built = StringValidator::new(
            &JsElement { validator: &view },
            regex_ast.as_ref(),
            length_ast.as_ref(),
        )?;
        Ok(to_js(&json!({
            "minLength": built.min_length(),
            "maxLength": built.max_length(),
        })))
    })
}

/// Rebuilds the validator's snapshot from the view: the regex AST `super()`
/// cached at `view.validator`, and the length bounds the constructor cached
/// at `view.minLength`/`view.maxLength` (`None` for both, the only state a
/// successful construction can have left, means no length AST was ever
/// given).
fn string_validator(view: &JsValue) -> Result<StringValidator> {
    let regex_ast = string_regex_ast(&get(view, "validator")?)?;
    let min_length = get(view, "minLength")?;
    let max_length = get(view, "maxLength")?;
    let length_ast = if nullish(&min_length) && nullish(&max_length) {
        None
    } else {
        let json = tag(
            json!({
                "minLength": to_json(&min_length)?,
                "maxLength": to_json(&max_length)?,
            }),
            "concerto.metamodel@1.0.0.StringLengthValidator",
        );
        Some(
            serde_json::from_value::<mm::StringLengthValidator>(json)
                .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))?,
        )
    };
    StringValidator::new(
        &JsElement { validator: view },
        regex_ast.as_ref(),
        length_ast.as_ref(),
    )
}

/// TS: StringValidator.validate. `null` is always accepted.
#[wasm_bindgen(js_name = stringValidatorValidate)]
pub fn string_validator_validate(
    view: JsValue,
    identifier: JsValue,
    value: JsValue,
) -> std::result::Result<(), JsValue> {
    run(|| {
        let validator = string_validator(&view)?;
        let identifier = if identifier.is_null() {
            None
        } else {
            Some(js_string(&identifier)?)
        };
        let value = if value.is_null() {
            None
        } else {
            Some(js_string(&value)?)
        };
        validator.validate(
            &JsElement { validator: &view },
            identifier.as_deref(),
            value.as_deref(),
        )
    })
}

/// TS: StringValidator.compatibleWith. `string_validator_class` is the
/// `StringValidator` class, for the `other instanceof StringValidator` check.
#[wasm_bindgen(js_name = stringValidatorCompatibleWith)]
pub fn string_validator_compatible_with(
    view: JsValue,
    other: JsValue,
    string_validator_class: Function,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        // `other instanceof StringValidator`
        let prototype = get(&string_validator_class, "prototype")?;
        if !other.is_object() || !prototype.unchecked_ref::<Object>().is_prototype_of(&other) {
            return Ok(false);
        }
        let this = string_validator(&view)?;
        let other = string_validator(&other)?;
        Ok(this.compatible_with(Some(&Validator::String(other))))
    })
}

/// TS: CollectionSizeValidator constructor, after `super(field, validator)`.
/// `view` is the object under construction; the result is its `{minSize,
/// maxSize}` snapshot.
#[wasm_bindgen(js_name = collectionSizeValidatorNew)]
pub fn collection_size_validator_new(
    view: JsValue,
    ast: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let typed = collection_size_ast(&ast)?;
        let built = CollectionSizeValidator::new(&JsElement { validator: &view }, &typed)?;
        Ok(to_js(&json!({
            "minSize": built.min_size(),
            "maxSize": built.max_size(),
        })))
    })
}

/// Rebuilds the validator from `view.validator`, the AST `super()` cached.
fn collection_size_validator(view: &JsValue) -> Result<CollectionSizeValidator> {
    let ast = collection_size_ast(&get(view, "validator")?)?;
    CollectionSizeValidator::new(&JsElement { validator: view }, &ast)
}

/// TS: CollectionSizeValidator.validate. `value` is compared as JS `<`/`>`
/// would, through `Number(value)`.
#[wasm_bindgen(js_name = collectionSizeValidatorValidate)]
pub fn collection_size_validator_validate(
    view: JsValue,
    identifier: JsValue,
    value: JsValue,
) -> std::result::Result<(), JsValue> {
    run(|| {
        let validator = collection_size_validator(&view)?;
        let identifier = if identifier.is_null() {
            None
        } else {
            Some(js_string(&identifier)?)
        };
        // `+value`: JS ToNumber.
        let value = value.unchecked_into_f64();
        validator.validate(&JsElement { validator: &view }, identifier.as_deref(), value)
    })
}

/// TS: CollectionSizeValidator.compatibleWith.
/// `collection_size_validator_class` is the `CollectionSizeValidator` class,
/// for the `other instanceof CollectionSizeValidator` check.
#[wasm_bindgen(js_name = collectionSizeValidatorCompatibleWith)]
pub fn collection_size_validator_compatible_with(
    view: JsValue,
    other: JsValue,
    collection_size_validator_class: Function,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        // `other instanceof CollectionSizeValidator`
        let prototype = get(&collection_size_validator_class, "prototype")?;
        if !other.is_object() || !prototype.unchecked_ref::<Object>().is_prototype_of(&other) {
            return Ok(false);
        }
        let this = collection_size_validator(&view)?;
        let other = collection_size_validator(&other)?;
        Ok(this.compatible_with(Some(&Validator::CollectionSize(other))))
    })
}

// ---------------------------------------------------------------------------
// Property (src/introspect/property.ts) — P4-07
// ---------------------------------------------------------------------------

/// TS: Property.process, after `super.process()`. Returns the snapshot
/// `{name, type, array, optional}`; `type` is omitted (not merely `null`)
/// when the AST `$class` is `EnumProperty`, since that is the one case where
/// TS never assigns `this.type` (property.rs module doc on
/// [`property::ProcessedProperty`]). `this.sizeValidator` is not part of the
/// snapshot: the view still builds it directly by constructing a
/// `CollectionSizeValidator`, whose own binding already ports that TS
/// constructor.
#[wasm_bindgen(js_name = propertyProcess)]
pub fn property_process(view: JsValue) -> std::result::Result<JsValue, JsValue> {
    let body = || -> Result<JsValue> {
        let ast = to_json(&get(&view, "ast")?)?.unwrap_or(Value::Null);
        let processed = property::process::<Error>(&ast)?;
        let mut snapshot = serde_json::Map::new();
        snapshot.insert("name".to_string(), json!(processed.name));
        if processed.type_set {
            snapshot.insert("type".to_string(), json!(processed.property_type));
        }
        snapshot.insert("array".to_string(), json!(processed.array));
        snapshot.insert("optional".to_string(), json!(processed.optional));
        Ok(to_js(&Value::Object(snapshot)))
    };
    body().map_err(|e| {
        let model_file = call(&view, "getModelFile", &[], "this.getModelFile")
            .unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

/// TS: Property.validate, after `super.validate()` (`Decorated`'s, which the
/// view calls separately before this). `classDecl` is the argument TS's
/// `validate(classDecl)` takes. `ModelFile` and `ModelManager` are not yet
/// Rust-backed (P2-08), so type resolution and the "is this a map
/// declaration" check are reached by calling straight back into the same
/// collaborators TS itself calls (`modelFile.resolveType`, `modelFile.getType`),
/// through the small context interface PORTING.md section 3 describes for a
/// view without a real Rust-backed parent — only the branching around them
/// runs in Rust. `modelFile.resolveType`'s own thrown `TypeNotFoundException`
/// propagates unchanged (`Error::Js`, from the `?` on `call`), so its message
/// and class are never reimplemented here.
#[wasm_bindgen(js_name = propertyValidate)]
pub fn property_validate(
    property: JsValue,
    class_decl: JsValue,
) -> std::result::Result<(), JsValue> {
    let model_file = call(&class_decl, "getModelFile", &[], "classDecl.getModelFile")
        .unwrap_or(JsValue::UNDEFINED);
    let body = || -> Result<()> {
        let property_type = get(&property, "type")?;
        if !nullish(&property_type) {
            let fqn = js_string(&call(
                &property,
                "getFullyQualifiedName",
                &[],
                "this.getFullyQualifiedName",
            )?)?;
            let message = JsValue::from_str(&format!("property {fqn}"));
            call(
                &model_file,
                "resolveType",
                &[message, property_type.clone()],
                "modelFile.resolveType",
            )?;
        }

        let size_validator = get(&property, "sizeValidator")?;
        let array = get(&property, "array")?.is_truthy();
        if !nullish(&size_validator) && !array {
            let mut is_map_type = false;
            if !nullish(&property_type) {
                let is_primitive =
                    call(&property, "isPrimitive", &[], "this.isPrimitive")?.is_truthy();
                if !is_primitive {
                    if let Ok(resolved) = call(
                        &model_file,
                        "getType",
                        &[property_type.clone()],
                        "modelFile.getType",
                    ) {
                        if let Some(v) = call_optional(&resolved, "isMapDeclaration")? {
                            is_map_type = v.is_truthy();
                        }
                    }
                }
            }
            if !is_map_type {
                let fqn = js_string(&call(
                    &property,
                    "getFullyQualifiedName",
                    &[],
                    "this.getFullyQualifiedName",
                )?)?;
                let ast = get(&property, "ast")?;
                let location = to_json(&get(&ast, "location")?)?;
                let mut err = ContractError::new(
                    ErrorKind::IllegalModel,
                    "property-validate-sizevalidator",
                    vec![("fqn", fqn)],
                );
                err.location = location;
                err.model_file = Some(None);
                return Err(err.into());
            }
        }
        Ok(())
    };
    body().map_err(|e| throw(e, Some(&model_file)))
}

// ---------------------------------------------------------------------------
// Field (src/introspect/field.ts) — P4-07
// ---------------------------------------------------------------------------

/// TS: Field.process, after `super.process()` (`Property`'s, already run).
/// Returns the snapshot `{validator, defaultValue}`, where `validator` is
/// `null`, `{kind: "NumberValidator", lowerBound, upperBound}` or
/// `{kind: "StringValidator"}` — the identical selection
/// `scalarDeclarationProcess` returns for `ScalarDeclaration`, reusing the
/// same [`ScalarValidator`] shape (field.rs module doc).
#[wasm_bindgen(js_name = fieldProcess)]
pub fn field_process(view: JsValue) -> std::result::Result<JsValue, JsValue> {
    let body = || -> Result<JsValue> {
        let ast = to_json(&get(&view, "ast")?)?.unwrap_or(Value::Null);
        let property_type = get(&view, "type")?;
        let property_type = if nullish(&property_type) {
            None
        } else {
            Some(js_string(&property_type)?)
        };
        let fqn = || {
            js_string(&call(
                &view,
                "getFullyQualifiedName",
                &[],
                "this.getFullyQualifiedName",
            )?)
        };
        let processed = field::process(property_type.as_deref(), &ast, &fqn)?;
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
            "validator": validator,
            "defaultValue": processed.default_value,
        })))
    };
    body().map_err(|e| {
        let model_file = call(&view, "getModelFile", &[], "this.getModelFile")
            .unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

// ---------------------------------------------------------------------------
// RelationshipDeclaration (src/introspect/relationshipdeclaration.ts) — P4-07
// ---------------------------------------------------------------------------

/// TS: RelationshipDeclaration.validate, after `super.validate(classDecl)`
/// (`Property`'s, which the view calls separately before this — the same
/// layering `propertyValidate` itself uses for `Decorated`'s). `ModelFile`
/// and `ModelManager` are not yet Rust-backed (P2-08), so this calls back
/// into the same collaborators TS itself calls, in the same order and with
/// the same try/catch shape (only the "own model file" lookup is
/// unguarded, exactly as TS's is): the branching runs in Rust, the
/// resolution itself is the small context interface PORTING.md section 3
/// describes.
#[wasm_bindgen(js_name = relationshipDeclarationValidate)]
pub fn relationship_declaration_validate(
    view: JsValue,
    class_decl: JsValue,
) -> std::result::Result<(), JsValue> {
    let model_file = call(&class_decl, "getModelFile", &[], "classDecl.getModelFile")
        .unwrap_or(JsValue::UNDEFINED);
    let body = || -> Result<()> {
        let ast = get(&view, "ast")?;
        let location = to_json(&get(&ast, "location")?)?;
        let name = js_string(&call(&view, "getName", &[], "this.getName")?)?;
        // TS reads `this.getType()` throughout (a method call, not the raw
        // `type` field), so a test that stubs `getType()` alone still takes
        // effect here.
        let property_type = call(&view, "getType", &[], "this.getType")?;

        if nullish(&property_type) {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-notype",
                vec![],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }

        let type_str = js_string(&property_type)?;
        if mu::is_primitive_type(&type_str) {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-primitivetype",
                vec![("name", name), ("type", type_str)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }

        let parent = call(&view, "getParent", &[], "this.getParent")?;
        let namespace = js_string(&call(&parent, "getNamespace", &[], "parent.getNamespace")?)?;
        let fqtn = js_string(&call(
            &view,
            "getFullyQualifiedTypeName",
            &[],
            "this.getFullyQualifiedTypeName",
        )?)?;
        let type_namespace = mu::get_namespace(Some(&fqtn))?.to_string();
        let parent_model_file = call(&parent, "getModelFile", &[], "parent.getModelFile")?;

        let mut class_declaration: Option<JsValue> = None;
        if namespace == type_namespace {
            // TS does not guard this lookup: any error it raises propagates.
            let resolved = call(
                &parent_model_file,
                "getType",
                &[property_type.clone()],
                "modelFile.getType",
            )?;
            if !nullish(&resolved) {
                class_declaration = Some(resolved);
            }
        } else if let Ok(model_manager) = call(
            &parent_model_file,
            "getModelManager",
            &[],
            "modelFile.getModelManager",
        ) {
            if let Ok(resolved) = call(
                &model_manager,
                "getType",
                &[JsValue::from_str(&fqtn)],
                "modelManager.getType",
            ) {
                if !nullish(&resolved) {
                    class_declaration = Some(resolved);
                }
            }
        }

        let Some(class_declaration) = class_declaration else {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-missingtype",
                vec![("name", name), ("type", fqtn)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        };

        let is_identified = call(
            &class_declaration,
            "isIdentified",
            &[],
            "classDeclaration.isIdentified",
        )?
        .is_truthy();
        if !is_identified {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-notidentified",
                vec![("name", name), ("type", fqtn)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }
        Ok(())
    };
    body().map_err(|e| throw(e, Some(&model_file)))
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
// ClassDeclaration family (src/introspect/classdeclaration.ts,
// assetdeclaration.ts, conceptdeclaration.ts, participantdeclaration.ts,
// transactiondeclaration.ts, eventdeclaration.ts, enumdeclaration.ts) — P4-06
//
// The model graph these views meet is still TS (ModelFile/ModelManager are
// not Rust-backed until P4-08; PORTING.md 1.4), so every ported member that
// needs a collaborator (`getModelFile().getType(...)`, the model manager's
// `getType`) reaches it through the JS-callback context (PORTING.md 1.4,
// "until the arena owns the graph, every collaborator call goes through the
// JS-callback context"): the member's own algorithm is ported here, and each
// collaborator call it makes is a plain call back onto the JS object it was
// given, via the `get`/`call` helpers (not the generic `ResolutionContext`
// trait — the exact TS collaborator sequence, e.g. `_resolveSuperType`'s
// `isImportedType`/`resolveImport` branch, matters more here than a shape
// shared with the arena implementation). A member whose *only* pure content
// is a small decision at the end (`classDeclarationProcess`'s superType/
// idField choice, the kind-compatibility and identifier-redeclare checks) has
// that decision itself pulled into `concerto_core::ClassDeclaration` as a
// plain function, so the binding stays a collaborator-calling wrapper around
// real core logic rather than a reimplementation of it (the grain
// Declaration/Decorated, P4-05, used for `modelUtilIsValidIdentifier` and
// `decoratedFindDuplicateName`).
// ---------------------------------------------------------------------------

/// The metamodel `$class`'s short name: the text after the last `.`.
fn short_class(ast_class: &str) -> &str {
    ast_class.rsplit('.').next().unwrap_or(ast_class)
}

/// TS: `ClassDeclaration.process`, the superType/idField decision made
/// before the `ast.properties` loop (the loop itself builds `Field`/
/// `RelationshipDeclaration`/`EnumValueDeclaration` views; that construction
/// stays in TS, and since P4-07 those Property views delegate their own
/// `process`/`validate` to the `propertyProcess`/`propertyValidate`/
/// `fieldProcess`/`relationshipDeclarationValidate` bindings). Returns `{superType, idField,
/// addIdentifierField, addTimestampField}`:
/// - `superType`: `this.ast.superType.name` when the AST names one;
///   otherwise `null` only for the system model's own `Concept` declaration,
///   else the implicit `'Concept'` (TS: the `this.modelFile.isSystemModelFile()
///   && this.name === 'Concept'` exemption).
/// - `idField`/`addIdentifierField`: mirrors the `this.ast.identified` match;
///   `addIdentifierField` tells the view to still call its own
///   `addIdentifierField()` (it pushes a real `Field` view).
/// - `addTimestampField`: `this.fqn` is the system `Transaction` or `Event`.
#[wasm_bindgen(js_name = classDeclarationProcess)]
pub fn class_declaration_process(declaration: JsValue) -> std::result::Result<JsValue, JsValue> {
    let body = || -> Result<JsValue> {
        let ast = get(&declaration, "ast")?;

        let explicit_super_type = get(&ast, "superType")?;
        let explicit_super_type = if !nullish(&explicit_super_type) {
            Some(receiver(
                &get(&explicit_super_type, "name")?,
                "this.ast.superType.name",
                "toString",
            )?)
        } else {
            None
        };
        let is_system_model_file = if explicit_super_type.is_none() {
            let model_file = call(&declaration, "getModelFile", &[], "this.getModelFile")?;
            call(
                &model_file,
                "isSystemModelFile",
                &[],
                "this.modelFile.isSystemModelFile",
            )?
            .is_truthy()
        } else {
            // TS never evaluates `this.modelFile.isSystemModelFile()` on this
            // branch (short-circuited by `this.ast.superType`); no collaborator
            // call to make.
            false
        };
        let name = receiver(&get(&declaration, "name")?, "this.name", "toString")?;

        let identified = get(&ast, "identified")?;
        let (identified_class, identified_name) = if nullish(&identified) {
            (None, None)
        } else {
            let identified_class = receiver(
                &get(&identified, "$class")?,
                "this.ast.identified.$class",
                "toString",
            )?;
            let identified_name = if short_class(&identified_class) == "IdentifiedBy" {
                Some(receiver(
                    &get(&identified, "name")?,
                    "this.ast.identified.name",
                    "toString",
                )?)
            } else {
                None
            };
            (Some(identified_class), identified_name)
        };

        let fqn = receiver(&get(&declaration, "fqn")?, "this.fqn", "toString")?;

        let decision = concerto_core::ClassDeclaration::process_decision(
            explicit_super_type.as_deref(),
            is_system_model_file,
            &name,
            identified_class.as_deref(),
            identified_name.as_deref(),
            &fqn,
        );

        Ok(to_js(&json!({
            "superType": decision.super_type,
            "idField": decision.id_field,
            "addIdentifierField": decision.add_identifier_field,
            "addTimestampField": decision.add_timestamp_field,
        })))
    };
    body().map_err(|e| {
        let model_file = get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

/// TS: the kind-compatibility check in `ClassDeclaration._resolveSuperType`:
/// `classDecl.declarationKind() !== 'ConceptDeclaration' &&
/// this.declarationKind() !== classDecl.declarationKind()`, negated (`true`
/// when compatible). `child_kind`/`super_kind` are each side's
/// `declarationKind()` string; resolving `classDecl` itself stays TS (a
/// `getModelFile()`/model manager collaborator call).
#[wasm_bindgen(js_name = classDeclarationKindsCompatible)]
pub fn class_declaration_kinds_compatible(
    child_kind: JsValue,
    super_kind: JsValue,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        let child_kind = js_string(&child_kind)?;
        let super_kind = js_string(&super_kind)?;
        Ok(concerto_core::ClassDeclaration::kinds_compatible(
            &child_kind,
            &super_kind,
        ))
    })
}

/// TS: the super-type identifier redeclaration check in
/// `ClassDeclaration.validate` (the block guarded by `superType.isIdentified()`,
/// which the caller checks before calling this): `true` when the super type's
/// existing identifier cannot be redeclared. Resolving `superType` itself
/// (`getModelFile().getType(this.superType)`) stays TS.
#[wasm_bindgen(js_name = classDeclarationIdentifierRedeclareConflict)]
pub fn class_declaration_identifier_redeclare_conflict(
    child_is_system_identified: bool,
    super_is_system_identified: bool,
    super_is_explicitly_identified: bool,
) -> bool {
    concerto_core::ClassDeclaration::identifier_redeclare_conflict(
        child_is_system_identified,
        super_is_system_identified,
        super_is_explicitly_identified,
    )
}

/// TS: `ClassDeclaration.toString`. `super_type_name` is the raw (unqualified)
/// name `this.superType` holds (an explicit AST name or the implicit
/// `'Concept'`), never a resolved FQN; a `ClassDeclaration` receiver is never
/// an enum (`EnumDeclaration` overrides `toString`), matching
/// [`ClassDeclaration::to_string`]'s hardcoded `enum=false`.
#[wasm_bindgen(js_name = classDeclarationToString)]
pub fn class_declaration_to_string(
    fqn: JsValue,
    super_type_name: JsValue,
    is_abstract: bool,
) -> std::result::Result<String, JsValue> {
    run(|| {
        let fqn = js_string(&fqn)?;
        let super_type_name = if nullish(&super_type_name) {
            None
        } else {
            Some(js_string(&super_type_name)?)
        };
        Ok(concerto_core::ClassDeclaration::to_string(
            &fqn,
            super_type_name.as_deref(),
            is_abstract,
        ))
    })
}

/// TS: `EnumDeclaration.toString` (src/introspect/enumdeclaration.ts): the
/// override with no super type or abstract flag.
#[wasm_bindgen(js_name = enumDeclarationToString)]
pub fn enum_declaration_to_string(fqn: JsValue) -> std::result::Result<String, JsValue> {
    run(|| {
        let fqn = js_string(&fqn)?;
        Ok(concerto_core::introspect::declaration::EnumDeclaration::to_string(&fqn))
    })
}

/// TS: `ClassDeclaration.isAsset`/`isParticipant`/`isTransaction`/`isEvent`/
/// `isConcept`/`isEnum`/`isMapDeclaration`: each compares `this.type` (the
/// AST's own `$class`, already set by `process()`) against one metamodel
/// `$class`. `kind_type` is the receiver's `this.type`; `want` is the
/// metamodel short name to compare against (`"AssetDeclaration"`, …).
#[wasm_bindgen(js_name = classDeclarationIsKind)]
pub fn class_declaration_is_kind(
    kind_type: JsValue,
    want: JsValue,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        let kind_type = js_string(&kind_type)?;
        let want = js_string(&want)?;
        Ok(concerto_core::ClassDeclaration::is_kind(&kind_type, &want))
    })
}

/// `target[name] = value`, the write-back `_resolveSuperType` makes onto its
/// own `this.superTypeDeclaration` field (a real field, not a getter — TS
/// reads it directly as a cache in `getSuperTypeDeclaration`).
fn set_property(target: &JsValue, name: &str, value: &JsValue) -> Result<()> {
    Reflect::set(target, &JsValue::from_str(name), value).map_err(Error::Js)?;
    Ok(())
}

/// A real `IllegalModelException`, decorated with `model_file`/`location`
/// exactly as `new IllegalModelException(message, modelFile, location)`
/// would (`engine/errors.ts`'s `IllegalModel` factory applies the same
/// decoration to `message` unconditionally): `model_file: Some(None)` marks
/// the error as one TS passes a model file to, so [`throw`] attaches the
/// caller's own JS model file object to the payload. `code` stays
/// `"pre-port"` (`message` is TS's own un-templated string concatenation,
/// not a catalogue entry).
fn illegal_model_error(message: String, location: Option<Value>) -> Error {
    ContractError {
        kind: ErrorKind::IllegalModel,
        code: "pre-port",
        params: vec![("message", message)],
        location,
        model_file: Some(None),
        validator: None,
        details: Vec::new(),
    }
    .into()
}

/// `declaration.ast.location`, as JSON (`None` when nullish).
fn ast_location(declaration: &JsValue) -> Result<Option<Value>> {
    to_json(&get(&get(declaration, "ast")?, "location")?)
}

/// TS: the `classDecl = ...` resolution duplicated in
/// `ClassDeclaration._resolveSuperType`, `.getProperty` and `.getProperties`
/// (src/introspect/classdeclaration.ts): `this.getModelFile().isImportedType(name)`
/// ? `this.modelFile.getModelManager().getType(this.getModelFile().resolveImport(name))`
/// : `this.getModelFile().getType(name)`. `type_name` is the JS string being
/// resolved (`this.superType`, in every caller here); the result may be
/// nullish, exactly as `ModelFile.getType`/`ModelManager.getType` can answer.
fn resolve_named_type(declaration: &JsValue, type_name: &JsValue) -> Result<JsValue> {
    let model_file = call(declaration, "getModelFile", &[], "this.getModelFile")?;
    let is_imported = call(
        &model_file,
        "isImportedType",
        std::slice::from_ref(type_name),
        "this.getModelFile().isImportedType",
    )?
    .is_truthy();
    if is_imported {
        let fqn_super = call(
            &model_file,
            "resolveImport",
            std::slice::from_ref(type_name),
            "this.getModelFile().resolveImport",
        )?;
        let own_model_file = get(declaration, "modelFile")?;
        let manager = call(
            &own_model_file,
            "getModelManager",
            &[],
            "this.modelFile.getModelManager",
        )?;
        call(
            &manager,
            "getType",
            &[fqn_super],
            "this.modelFile.getModelManager().getType",
        )
    } else {
        call(
            &model_file,
            "getType",
            std::slice::from_ref(type_name),
            "this.getModelFile().getType",
        )
    }
}

/// TS: `ClassDeclaration._resolveSuperType`. Resolves `this.superType`
/// through [`resolve_named_type`], throws the same `IllegalModelException`
/// TS does when it cannot find the super type or the two kinds are
/// incompatible ([`concerto_core::ClassDeclaration::kinds_compatible`]), and
/// caches the result onto `this.superTypeDeclaration` before returning it —
/// the same field `getSuperTypeDeclaration` reads back as a cache.
#[wasm_bindgen(js_name = classDeclarationResolveSuperType)]
pub fn class_declaration_resolve_super_type(
    declaration: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    let body = || -> Result<JsValue> {
        let super_type = get(&declaration, "superType")?;
        if !super_type.is_truthy() {
            return Ok(JsValue::NULL);
        }
        set_property(&declaration, "superTypeDeclaration", &JsValue::NULL)?;

        let class_decl = resolve_named_type(&declaration, &super_type)?;
        if nullish(&class_decl) {
            let super_type_name = js_string(&super_type)?;
            return Err(illegal_model_error(
                format!("Could not find super type {super_type_name}"),
                ast_location(&declaration)?,
            ));
        }

        let child_kind = js_string(&call(
            &declaration,
            "declarationKind",
            &[],
            "this.declarationKind",
        )?)?;
        let super_kind = js_string(&call(
            &class_decl,
            "declarationKind",
            &[],
            "classDecl.declarationKind",
        )?)?;
        if !concerto_core::ClassDeclaration::kinds_compatible(&child_kind, &super_kind) {
            let child_name = js_string(&call(&declaration, "getName", &[], "this.getName")?)?;
            let super_name = js_string(&call(&class_decl, "getName", &[], "classDecl.getName")?)?;
            return Err(illegal_model_error(
                format!("{child_kind} ({child_name}) cannot extend {super_kind} ({super_name})"),
                ast_location(&declaration)?,
            ));
        }

        set_property(&declaration, "superTypeDeclaration", &class_decl)?;
        Ok(class_decl)
    };
    body().map_err(|e| {
        let model_file = get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

/// TS: `ClassDeclaration.getSuperTypeDeclaration`: the branch is pure field
/// reads; the fallback calls back `this._resolveSuperType()` (a collaborator
/// call — that method resolves and validates the super type).
#[wasm_bindgen(js_name = classDeclarationGetSuperTypeDeclaration)]
pub fn class_declaration_get_super_type_declaration(
    declaration: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        if !get(&declaration, "superType")?.is_truthy() {
            return Ok(JsValue::NULL);
        }
        let cached = get(&declaration, "superTypeDeclaration")?;
        if cached.is_truthy() {
            return Ok(cached);
        }
        call(
            &declaration,
            "_resolveSuperType",
            &[],
            "this._resolveSuperType",
        )
    })
}

/// TS: `ClassDeclaration.getSuperType`: `this.getSuperTypeDeclaration()`,
/// then `getFullyQualifiedName()` on the result if there is one.
#[wasm_bindgen(js_name = classDeclarationGetSuperType)]
pub fn class_declaration_get_super_type(
    declaration: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let super_type_decl = call(
            &declaration,
            "getSuperTypeDeclaration",
            &[],
            "this.getSuperTypeDeclaration",
        )?;
        if !super_type_decl.is_truthy() {
            return Ok(JsValue::NULL);
        }
        call(
            &super_type_decl,
            "getFullyQualifiedName",
            &[],
            "superTypeDeclaration.getFullyQualifiedName",
        )
    })
}

/// TS: `ClassDeclaration.getAllSuperTypeDeclarations`: repeats
/// `type = type.getSuperTypeDeclaration()` from `this`, collecting every
/// non-null result.
#[wasm_bindgen(js_name = classDeclarationGetAllSuperTypeDeclarations)]
pub fn class_declaration_get_all_super_type_declarations(
    declaration: JsValue,
) -> std::result::Result<Array, JsValue> {
    run(|| {
        let results = Array::new();
        let mut current = declaration;
        loop {
            let next = call(
                &current,
                "getSuperTypeDeclaration",
                &[],
                "type.getSuperTypeDeclaration",
            )?;
            if !next.is_truthy() {
                break;
            }
            results.push(&next);
            current = next;
        }
        Ok(results)
    })
}

/// TS: `ClassDeclaration.getIdentifierFieldName`: `this.idField` if set,
/// otherwise the super type's own answer, found through `getLocalType` (or,
/// failing that, the model manager). A `null` super type resolution reaches
/// the same unguarded `classDecl.getIdentifierFieldName()` call TS makes
/// (and the same host `TypeError` `call` raises for it).
#[wasm_bindgen(js_name = classDeclarationGetIdentifierFieldName)]
pub fn class_declaration_get_identifier_field_name(
    declaration: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let id_field = get(&declaration, "idField")?;
        if id_field.is_truthy() {
            return Ok(id_field);
        }
        let super_type = call(&declaration, "getSuperType", &[], "this.getSuperType")?;
        if !super_type.is_truthy() {
            return Ok(JsValue::NULL);
        }
        let model_file = call(&declaration, "getModelFile", &[], "this.getModelFile")?;
        let mut class_decl = call(
            &model_file,
            "getLocalType",
            std::slice::from_ref(&super_type),
            "this.getModelFile().getLocalType",
        )?;
        if !class_decl.is_truthy() {
            let own_model_file = get(&declaration, "modelFile")?;
            let manager = call(
                &own_model_file,
                "getModelManager",
                &[],
                "this.modelFile.getModelManager",
            )?;
            class_decl = call(
                &manager,
                "getType",
                &[super_type],
                "this.modelFile.getModelManager().getType",
            )?;
        }
        call(
            &class_decl,
            "getIdentifierFieldName",
            &[],
            "classDecl.getIdentifierFieldName",
        )
    })
}

/// TS: `ClassDeclaration.getProperty`: the receiver's own property if it has
/// one, otherwise the super type's answer (through [`resolve_named_type`]).
/// A `null` super type resolution reaches the same unguarded
/// `classDecl.getProperty(name)` call TS makes.
#[wasm_bindgen(js_name = classDeclarationGetProperty)]
pub fn class_declaration_get_property(
    declaration: JsValue,
    name: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let own = call(
            &declaration,
            "getOwnProperty",
            std::slice::from_ref(&name),
            "this.getOwnProperty",
        )?;
        if !nullish(&own) {
            return Ok(own);
        }
        let super_type = get(&declaration, "superType")?;
        if !super_type.is_truthy() {
            return Ok(JsValue::NULL);
        }
        let class_decl = resolve_named_type(&declaration, &super_type)?;
        call(&class_decl, "getProperty", &[name], "classDecl.getProperty")
    })
}

/// TS: `ClassDeclaration.getProperties`: the receiver's own properties, plus
/// (when it has a super type) the super type's own answer, found through
/// [`resolve_named_type`] — unlike `getProperty`, TS itself guards this
/// resolution with the same "Could not find super type" `IllegalModelException`
/// `_resolveSuperType` raises.
#[wasm_bindgen(js_name = classDeclarationGetProperties)]
pub fn class_declaration_get_properties(
    declaration: JsValue,
) -> std::result::Result<Array, JsValue> {
    let body = || -> Result<Array> {
        let own = call(&declaration, "getOwnProperties", &[], "this.getOwnProperties")?;
        let result = Array::new();
        for property in Array::from(&own).iter() {
            result.push(&property);
        }
        let super_type = get(&declaration, "superType")?;
        if !super_type.is_truthy() {
            return Ok(result);
        }
        let class_decl = resolve_named_type(&declaration, &super_type)?;
        if nullish(&class_decl) {
            let super_type_name = js_string(&super_type)?;
            return Err(illegal_model_error(
                format!("Could not find super type {super_type_name}"),
                ast_location(&declaration)?,
            ));
        }
        let inherited = call(
            &class_decl,
            "getProperties",
            &[],
            "classDecl.getProperties",
        )?;
        for property in Array::from(&inherited).iter() {
            result.push(&property);
        }
        Ok(result)
    };
    body().map_err(|e| {
        let model_file = get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

/// TS: `Introspector.getClassDeclarations`, inlined: every model file's
/// declarations, minus map and scalar declarations (which have no
/// superType-based subclass relationship, and whose `isMapDeclaration`/
/// `isScalarDeclaration` TS calls with `?.`, so a class-family declaration
/// without either method is just treated as neither).
fn get_class_declarations(model_manager: &JsValue) -> Result<Vec<JsValue>> {
    let model_files = call(
        model_manager,
        "getModelFiles",
        &[],
        "modelManager.getModelFiles",
    )?;
    let mut result = Vec::new();
    for model_file in Array::from(&model_files).iter() {
        let declarations = call(
            &model_file,
            "getAllDeclarations",
            &[],
            "modelFile.getAllDeclarations",
        )?;
        for declaration in Array::from(&declarations).iter() {
            let is_map = call_optional(&declaration, "isMapDeclaration")?
                .is_some_and(|v| v.is_truthy());
            let is_scalar = call_optional(&declaration, "isScalarDeclaration")?
                .is_some_and(|v| v.is_truthy());
            if !is_map && !is_scalar {
                result.push(declaration);
            }
        }
    }
    Ok(result)
}

/// Builds the same `subclassMap` TS does in `getAssignableClassDeclarations`
/// and `getDirectSubclasses`: every loaded class-like declaration, keyed by
/// its own super type's fully qualified name (in `getModelFiles`/
/// `getAllDeclarations` order, so each bucket's insertion order matches TS's
/// `Array.forEach` too).
fn build_subclass_map(
    model_manager: &JsValue,
) -> Result<std::collections::HashMap<String, Vec<JsValue>>> {
    let all = get_class_declarations(model_manager)?;
    let mut subclass_map: std::collections::HashMap<String, Vec<JsValue>> =
        std::collections::HashMap::new();
    for decl in &all {
        let super_type = call(decl, "getSuperType", &[], "declaration.getSuperType")?;
        if super_type.is_truthy() {
            let key = js_string(&super_type)?;
            subclass_map.entry(key).or_default().push(decl.clone());
        }
    }
    Ok(subclass_map)
}

/// TS: `ClassDeclaration.getAssignableClassDeclarations`: `this` plus every
/// direct and indirect subclass, deduplicated the way TS's
/// `Set<ClassDeclaration>` deduplicates — by declaration identity, which
/// (every FQN in a validated model manager names exactly one declaration
/// instance) is the same as deduplicating by fully qualified name here.
#[wasm_bindgen(js_name = classDeclarationGetAssignableClassDeclarations)]
pub fn class_declaration_get_assignable_class_declarations(
    declaration: JsValue,
) -> std::result::Result<Array, JsValue> {
    run(|| {
        let model_file = call(&declaration, "getModelFile", &[], "this.getModelFile")?;
        let model_manager = call(
            &model_file,
            "getModelManager",
            &[],
            "this.getModelFile().getModelManager",
        )?;
        let subclass_map = build_subclass_map(&model_manager)?;

        fn collect(
            declarations: &[JsValue],
            subclass_map: &std::collections::HashMap<String, Vec<JsValue>>,
            seen: &mut Vec<JsValue>,
            seen_keys: &mut HashSet<String>,
        ) -> Result<()> {
            for decl in declarations {
                let fqn = js_string(&call(
                    decl,
                    "getFullyQualifiedName",
                    &[],
                    "declaration.getFullyQualifiedName",
                )?)?;
                if seen_keys.insert(fqn.clone()) {
                    seen.push(decl.clone());
                }
                if let Some(children) = subclass_map.get(&fqn) {
                    collect(children, subclass_map, seen, seen_keys)?;
                }
            }
            Ok(())
        }

        let mut seen = Vec::new();
        let mut seen_keys = HashSet::new();
        collect(
            std::slice::from_ref(&declaration),
            &subclass_map,
            &mut seen,
            &mut seen_keys,
        )?;

        let result = Array::new();
        for d in seen {
            result.push(&d);
        }
        Ok(result)
    })
}

/// TS: `ClassDeclaration.getDirectSubclasses`: just the receiver's own
/// bucket in the same `subclassMap`, excluding the receiver itself.
#[wasm_bindgen(js_name = classDeclarationGetDirectSubclasses)]
pub fn class_declaration_get_direct_subclasses(
    declaration: JsValue,
) -> std::result::Result<Array, JsValue> {
    run(|| {
        let model_file = call(&declaration, "getModelFile", &[], "this.getModelFile")?;
        let model_manager = call(
            &model_file,
            "getModelManager",
            &[],
            "this.getModelFile().getModelManager",
        )?;
        let subclass_map = build_subclass_map(&model_manager)?;
        let fqn = js_string(&call(
            &declaration,
            "getFullyQualifiedName",
            &[],
            "this.getFullyQualifiedName",
        )?)?;
        let result = Array::new();
        if let Some(children) = subclass_map.get(&fqn) {
            for d in children {
                result.push(d);
            }
        }
        Ok(result)
    })
}

/// TS: `ClassDeclaration.getNestedProperty`: walks a dotted property path
/// one name at a time, resolving each step's class through
/// `getFullyQualifiedTypeName` and `modelManager.getType`, and stopping with
/// the same `IllegalModelException`/plain `Error` TS raises for a missing
/// property or a primitive/enum step that isn't the path's last element.
#[wasm_bindgen(js_name = classDeclarationGetNestedProperty)]
pub fn class_declaration_get_nested_property(
    declaration: JsValue,
    property_path: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    let body = || -> Result<JsValue> {
        let path = js_string(&property_path)?;
        let names: Vec<&str> = path.split('.').collect();
        let mut class_declaration = declaration.clone();
        let mut result = JsValue::UNDEFINED;

        for (n, name) in names.iter().enumerate() {
            let property = call(
                &class_declaration,
                "getProperty",
                &[JsValue::from_str(name)],
                "classDeclaration.getProperty",
            )?;
            if nullish(&property) {
                let fqn = js_string(&call(
                    &class_declaration,
                    "getFullyQualifiedName",
                    &[],
                    "classDeclaration.getFullyQualifiedName",
                )?)?;
                return Err(ContractError {
                    kind: ErrorKind::IllegalModel,
                    code: "classdeclaration-getnestedproperty-doesnotexist",
                    params: vec![("propertyName", (*name).to_string()), ("fqn", fqn)],
                    location: ast_location(&declaration)?,
                    model_file: Some(None),
                    validator: None,
                    details: Vec::new(),
                }
                .into());
            }
            result = property.clone();

            if n < names.len() - 1 {
                let is_primitive =
                    call(&property, "isPrimitive", &[], "result.isPrimitive")?.is_truthy();
                let is_enum =
                    call(&property, "isTypeEnum", &[], "result.isTypeEnum")?.is_truthy();
                if is_primitive || is_enum {
                    return Err(plain_error(
                        "classdeclaration-getnestedproperty-primitiveorenum",
                        vec![("propertyName", (*name).to_string()), ("propertyPath", path.clone())],
                    ));
                }
                let type_fqn = call(
                    &property,
                    "getFullyQualifiedTypeName",
                    &[],
                    "result.getFullyQualifiedTypeName",
                )?;
                let own_model_file = get(&declaration, "modelFile")?;
                let manager = call(
                    &own_model_file,
                    "getModelManager",
                    &[],
                    "this.modelFile.getModelManager",
                )?;
                class_declaration = call(
                    &manager,
                    "getType",
                    &[type_fqn],
                    "this.modelFile.getModelManager().getType",
                )?;
            }
        }

        Ok(result)
    };
    body().map_err(|e| {
        let model_file = get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

// ---------------------------------------------------------------------------
// MapDeclaration, MapKeyType, MapValueType (src/introspect/mapdeclaration.ts,
// mapkeytype.ts, mapvaluetype.ts) — P4-07
// ---------------------------------------------------------------------------

/// TS: MapDeclaration.process, after `super.process()`. Checks the AST's
/// `key`/`value` shape with the same checks `ModelUtil.isValidMapKey`/
/// `isValidMapValue` already run through Rust (P2-06), called here
/// natively ([`mu::is_valid_map_key`], [`mu::is_valid_map_value`]) rather
/// than through another JS round trip. The view still builds the
/// `MapKeyType`/`MapValueType` child views itself afterwards, the same way
/// `propertyProcess` still builds its own `CollectionSizeValidator`.
#[wasm_bindgen(js_name = mapDeclarationProcess)]
pub fn map_declaration_process(view: JsValue) -> std::result::Result<(), JsValue> {
    let body = || -> Result<()> {
        let ast = get(&view, "ast")?;
        let name = opt_get(&ast, "name")?;
        let name = if nullish(&name) {
            String::new()
        } else {
            js_string(&name)?
        };
        let key = to_json(&get(&ast, "key")?)?;
        let value = to_json(&get(&ast, "value")?)?;
        let location = to_json(&get(&ast, "location")?)?;

        if key.is_none() || value.is_none() {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "mapdeclaration-process-missingkeyvalue",
                vec![("name", name)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }
        if !mu::is_valid_map_key(key.as_ref())? {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "mapdeclaration-process-invalidkey",
                vec![("name", name)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }
        if !mu::is_valid_map_value(value.as_ref())? {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "mapdeclaration-process-invalidvalue",
                vec![("name", name)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }
        Ok(())
    };
    body().map_err(|e| {
        let model_file = get(&view, "modelFile").unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

/// TS: MapKeyType.processType. Pure AST logic (module doc); the `$class`
/// switch has no default arm, which is unreachable here since
/// `mapDeclarationProcess`'s `mu::is_valid_map_key` check already restricts
/// the AST to one of these three kinds before a `MapKeyType` is ever built —
/// the empty-string fallback below is never actually observed.
#[wasm_bindgen(js_name = mapKeyTypeProcess)]
pub fn map_key_type_process(view: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let ast = get(&view, "ast")?;
        let class = get(&ast, "$class")?;
        let class = if nullish(&class) {
            String::new()
        } else {
            js_string(&class)?
        };
        let type_name = match short_class(&class) {
            "DateTimeMapKeyType" => "DateTime".to_string(),
            "StringMapKeyType" => "String".to_string(),
            "ObjectMapKeyType" => {
                let ast_type = get(&ast, "type")?;
                js_string(&get(&ast_type, "name")?)?
            }
            _ => String::new(),
        };
        Ok(JsValue::from_str(&type_name))
    })
}

/// TS: MapKeyType.validate. `this.modelFile.getType(...)` is a live TS
/// collaborator call (`ModelFile` is not yet Rust-backed, P2-08); the
/// scalar-kind check itself is [`mu::is_valid_map_key_scalar`], already
/// shared with the native engine's own `validate_map_key`.
#[wasm_bindgen(js_name = mapKeyTypeValidate)]
pub fn map_key_type_validate(view: JsValue) -> std::result::Result<(), JsValue> {
    run(|| {
        let type_name = js_string(&get(&view, "type")?)?;
        if mu::is_primitive_type(&type_name) {
            return Ok(());
        }
        let model_file = get(&view, "modelFile")?;
        let ast = get(&view, "ast")?;
        let ast_type = get(&ast, "type")?;
        let type_name_ast = get(&ast_type, "name")?;
        let decl = call(
            &model_file,
            "getType",
            &[type_name_ast],
            "modelFile.getType",
        )?;
        let valid = mu::is_valid_map_key_scalar(&JsContext, Some(&decl))?;
        if valid != Some(true) {
            let parent = get(&view, "parent")?;
            let parent_name = js_string(&get(&parent, "name")?)?;
            return Err(ContractError::new(
                ErrorKind::IllegalModel,
                "mapkeytype-validate-invalidscalar",
                vec![("type", type_name), ("name", parent_name)],
            )
            .into());
        }
        Ok(())
    })
}

/// TS: MapValueType.processType. Pure AST logic (module doc), except the
/// `ObjectMapValueType`/`RelationshipMapValueType` arm's own shape checks,
/// which TS throws inline for.
#[wasm_bindgen(js_name = mapValueTypeProcess)]
pub fn map_value_type_process(view: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let ast = get(&view, "ast")?;
        let parent = get(&view, "parent")?;
        let parent_name = js_string(&get(&parent, "name")?)?;
        let class = get(&ast, "$class")?;
        let class = if nullish(&class) {
            String::new()
        } else {
            js_string(&class)?
        };
        let type_name = match short_class(&class) {
            "ObjectMapValueType" | "RelationshipMapValueType" => {
                let ast_type = get(&ast, "type")?;
                if nullish(&ast_type) {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "mapvaluetype-process-missingtype",
                        vec![("name", parent_name)],
                    )
                    .into());
                }
                let type_class = get(&ast_type, "$class")?;
                let type_name_field = get(&ast_type, "name")?;
                if nullish(&type_class) || nullish(&type_name_field) {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "mapvaluetype-process-malformedtype",
                        vec![("name", parent_name)],
                    )
                    .into());
                }
                if js_string(&type_class)? != "concerto.metamodel@1.0.0.TypeIdentifier" {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "mapvaluetype-process-invalidtypeclass",
                        vec![("name", parent_name)],
                    )
                    .into());
                }
                js_string(&type_name_field)?
            }
            "BooleanMapValueType" => "Boolean".to_string(),
            "DateTimeMapValueType" => "DateTime".to_string(),
            "StringMapValueType" => "String".to_string(),
            "IntegerMapValueType" => "Integer".to_string(),
            "LongMapValueType" => "Long".to_string(),
            "DoubleMapValueType" => "Double".to_string(),
            _ => String::new(),
        };
        Ok(JsValue::from_str(&type_name))
    })
}

/// TS: MapValueType.validate. `this.modelFile.getType(...)` is a live TS
/// collaborator call (`ModelFile` is not yet Rust-backed, P2-08); the
/// "is this a map declaration" check itself goes through [`JsContext`]'s
/// existing [`ResolutionContext::is_map_declaration`].
#[wasm_bindgen(js_name = mapValueTypeValidate)]
pub fn map_value_type_validate(view: JsValue) -> std::result::Result<(), JsValue> {
    run(|| {
        let type_name = js_string(&get(&view, "type")?)?;
        if mu::is_primitive_type(&type_name) {
            return Ok(());
        }
        let model_file = get(&view, "modelFile")?;
        let ast = get(&view, "ast")?;
        let ast_type = get(&ast, "type")?;
        let type_name_ast = get(&ast_type, "name")?;
        let decl = call(
            &model_file,
            "getType",
            &[type_name_ast],
            "modelFile.getType",
        )?;
        if JsContext.is_map_declaration(&decl)?.unwrap_or(false) {
            return Err(ContractError::new(
                ErrorKind::IllegalModel,
                "mapvaluetype-validate-mapnotsupported",
                vec![("type", type_name)],
            )
            .into());
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Decorator, Decorated (src/introspect/decorator.ts, decorated.ts) — P4-05
// ---------------------------------------------------------------------------

/// TS: Decorator.process. Builds `{name, arguments}` from the raw AST node,
/// through the P2-07 port ([`Decorator::from_ast`]); needs no collaborator
/// call.
#[wasm_bindgen(js_name = decoratorProcess)]
pub fn decorator_process(ast: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let ast_json = to_json(&ast)?.unwrap_or(Value::Null);
        let decorator = Decorator::from_ast(&ast_json);
        let arguments = Array::new();
        for arg in decorator.arguments() {
            arguments.push(&argument_to_js(arg));
        }
        let out = Object::new();
        set(&out, "name", &JsValue::from_str(decorator.name()));
        set(&out, "arguments", &arguments);
        Ok(out.into())
    })
}

/// One decoded [`DecoratorArgument`], as TS's `Decorator.process` would have
/// pushed it onto `this.arguments`. Built as a JS value directly, not
/// through [`to_js`]'s JSON round trip, which cannot represent `undefined`
/// (TS: `{ type: 'Identifier', name: ..., array: thing.isArray }` — the
/// object literal always creates the `array` *property*, even when
/// `thing.isArray` is `undefined`, which is a different, observable state
/// from the property being absent).
fn argument_to_js(arg: &DecoratorArgument) -> JsValue {
    match arg {
        DecoratorArgument::String(s) => JsValue::from_str(s),
        DecoratorArgument::Number(n) => JsValue::from_f64(*n),
        DecoratorArgument::Boolean(b) => JsValue::from_bool(*b),
        DecoratorArgument::TypeReference(t) => {
            let out = Object::new();
            set(&out, "type", &JsValue::from_str("Identifier"));
            set(&out, "name", &JsValue::from_str(&t.name));
            let array = t.array.map_or(JsValue::UNDEFINED, JsValue::from_bool);
            set(&out, "array", &array);
            out.into()
        }
    }
}

/// TS: the duplicate-decorator loop in `Decorated.validate`
/// (src/introspect/decorated.ts) — `names` is `this.decorators.map(d =>
/// d.getName())`. Returns the first name that repeats, in original order, or
/// `null`; the view throws the `IllegalModelException` itself (a plain
/// string message, no engine error payload needed).
#[wasm_bindgen(js_name = decoratedFindDuplicateName)]
pub fn decorated_find_duplicate_name(names: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let mut seen = HashSet::new();
        for name in Array::from(&names).iter() {
            let name = js_string(&name)?;
            if !seen.insert(name.clone()) {
                return Ok(JsValue::from_str(&name));
            }
        }
        Ok(JsValue::NULL)
    })
}

/// `value?.name`: `undefined` for a nullish `value`, as an optional-chain
/// property read is (unlike [`get`], which raises V8's error for one).
fn opt_get(value: &JsValue, name: &str) -> Result<JsValue> {
    if nullish(value) {
        return Ok(JsValue::UNDEFINED);
    }
    get(value, name)
}

/// JS `typeof value`, for the shapes a decorator argument or a decorator
/// validation option's value can be.
fn js_typeof(value: &JsValue) -> &'static str {
    if value.as_f64().is_some() {
        "number"
    } else if value.as_string().is_some() {
        "string"
    } else if value.as_bool().is_some() {
        "boolean"
    } else if value.is_undefined() {
        "undefined"
    } else {
        "object"
    }
}

/// `JSON.stringify(value)`.
fn json_stringify(value: &JsValue) -> Result<String> {
    JSON::stringify(value)
        .map(|s| s.as_string().unwrap_or_default())
        .map_err(Error::Js)
}

/// One property of a decorator's own type declaration, as
/// `Decorator.validate` reads it: `p.getName()`, `p.isOptional()`,
/// `p.getType()`. `node` is kept for the [`mu::is_assignable_to`] call, which
/// reads it as a [`ResolutionContext`] node.
struct PropertyView {
    node: JsValue,
    name: String,
    optional: bool,
    type_name: Option<String>,
}

/// TS: `p.getName()`, `p.isOptional()`, `p.getType()`, read together for one
/// element of `decoratorDecl.getProperties()`.
fn property_view(node: JsValue) -> Result<PropertyView> {
    let name = js_string(&call(&node, "getName", &[], "property.getName")?)?;
    let optional = call(&node, "isOptional", &[], "property.isOptional")?.is_truthy();
    let type_name = {
        let t = call(&node, "getType", &[], "property.getType")?;
        if nullish(&t) {
            None
        } else {
            Some(js_string(&t)?)
        }
    };
    Ok(PropertyView {
        node,
        name,
        optional,
        type_name,
    })
}

/// An option level (`'error'`, `'warn'`, ...) of a `DecoratorValidationOptions`
/// value, `None` when it is falsy: TS's `if (validationOptions.missingDecorator
/// || ...)` and `level === 'error'` checks, together.
fn level_option(options: &JsValue, key: &str) -> Result<Option<String>> {
    let value = get(options, key)?;
    if !value.is_truthy() {
        return Ok(None);
    }
    Ok(Some(js_string(&value)?))
}

/// `level === undefined` when TS's `validationOptions.<x>` is falsy, else the
/// level string, as a `JsValue` for a `handleError` call.
fn level_js(level: &Option<String>) -> JsValue {
    level
        .as_deref()
        .map_or(JsValue::UNDEFINED, JsValue::from_str)
}

/// TS: `this.handleError(level, err)`, called back on `view` so its own
/// method builds the exact `IllegalModelException` (message, model file,
/// location) and logs through `Logger.dispatch`, exactly as every other
/// call site of `handleError` does. `err` is a message string for one of
/// this function's own checks, or (from the outer catch, [`decorator_validate`])
/// whatever the try-equivalent threw — TS passes `handleError` either shape.
fn handle_error(view: &JsValue, level: &Option<String>, err: &JsValue) -> Result<()> {
    call(
        view,
        "handleError",
        &[level_js(level), err.clone()],
        "this.handleError",
    )?;
    Ok(())
}

/// TS: `this.handleError(validationOptions.invalidDecorator, err)` with a
/// message this function built itself.
fn report_invalid(view: &JsValue, invalid: &Option<String>, message: String) -> Result<()> {
    handle_error(view, invalid, &JsValue::from_str(&message))
}

/// `IllegalModelException`'s own message decoration
/// (`illegalmodelexception.ts`): `message + ' ' + messageSuffix`, where
/// `messageSuffix` is `File '<name>': ` when `modelFile` is truthy and its
/// `getName()` is truthy, followed by `line .. column .., to line .. column
/// ... ` when `location` is truthy, with the whole suffix's first character
/// upper-cased. Needed only for [`try_validate_decorator`]'s own resolution
/// failure ([`named_js_error`]): the real `IllegalModelException`
/// construction this replicates is `ModelFile.resolveType`'s own throw,
/// which is not a call this binding makes (P2-07 module doc,
/// `resolve_own_name`) — every other message here reaches the exception
/// through a real `view.handleError` call ([`handle_error`]), never through
/// this.
fn illegal_model_message(
    message: &str,
    model_file: &JsValue,
    location: &JsValue,
) -> Result<String> {
    let mut suffix = String::new();
    if model_file.is_truthy() {
        let file_name = call(model_file, "getName", &[], "modelFile.getName")?;
        if file_name.is_truthy() {
            suffix.push_str(&format!("File '{}': ", js_string(&file_name)?));
        }
    }
    if location.is_truthy() {
        let start = get(location, "start")?;
        let end = get(location, "end")?;
        let field = |node: &JsValue, name: &str| -> Result<String> { js_string(&get(node, name)?) };
        suffix.push_str(&format!(
            "line {} column {}, to line {} column {}. ",
            field(&start, "line")?,
            field(&start, "column")?,
            field(&end, "line")?,
            field(&end, "column")?,
        ));
    }
    let mut capitalized = String::with_capacity(suffix.len());
    let mut chars = suffix.chars();
    if let Some(first) = chars.next() {
        capitalized.extend(first.to_uppercase());
        capitalized.push_str(chars.as_str());
    }
    Ok(format!("{message} {capitalized}"))
}

/// An `Error` whose `name` is `IllegalModelException` and whose `message`
/// already carries [`illegal_model_message`]'s decoration, so that coercing
/// it (`String(err)`, `` `${err}` ``) reads the same way TS's caught
/// `IllegalModelException` would.
fn named_js_error(name: &str, text: &str) -> JsValue {
    let err = js_sys::Error::new(text);
    let _ = Reflect::set(&err, &JsValue::from_str("name"), &JsValue::from_str(name));
    err.into()
}

/// TS: `Decorator.validate`, driven through [`JsContext`] since the model
/// graph these views meet is still TS (module doc: "until P4-06 … P4-08").
/// `view` is the Decorator, already processed (`name`/`arguments` set);
/// `model_file` is `this.getParent().getModelFile()`; `context` is
/// `this.getParent().getFullyQualifiedName?.()` — nullish for a model file's
/// own decorator, exactly as TS's optional call leaves it.
///
/// Every exception this function and its helpers raise is built by calling
/// back into `view.handleError` (or, for the try block's own resolution
/// failure, a plain `Error` that coerces the same way TS's caught value
/// would): the `IllegalModelException` construction, its "File '...': "
/// decoration and the log call are never reimplemented here, so they cannot
/// drift from TS's. TS's outer `catch` re-reports *every* thrown value —
/// including a raw host `TypeError` from reading a collaborator that does
/// not behave like a real model element (e.g. a decorator named after a
/// primitive, so `mf.getType` resolves it to a type with no
/// `getProperties`) — through `missingDecorator`, so both of this binding's
/// [`Error`] variants are routed the same way: a [`Error::Contract`] (a host
/// `TypeError` this module's own `call`/`get` raised, or any other core
/// error [`try_validate_decorator`]'s collaborators produced) is first
/// turned into the JS exception it would coerce to ([`throw`], the same
/// mapping the whole binding uses to leave the module), so `handleError`
/// sees the same kind of value TS's `catch (err)` would have caught.
#[wasm_bindgen(js_name = decoratorValidate)]
pub fn decorator_validate(
    view: JsValue,
    model_file: JsValue,
    context: JsValue,
) -> std::result::Result<(), JsValue> {
    let body = || -> Result<()> {
        let mm = call(&model_file, "getModelManager", &[], "mf.getModelManager")?;
        let options = call(
            &mm,
            "getDecoratorValidation",
            &[],
            "mm.getDecoratorValidation",
        )?;
        let missing = level_option(&options, "missingDecorator")?;
        let invalid = level_option(&options, "invalidDecorator")?;
        if missing.is_none() && invalid.is_none() {
            return Ok(());
        }
        let context_name = if nullish(&context) {
            None
        } else {
            Some(js_string(&context)?)
        };
        match try_validate_decorator(&view, &model_file, context_name.as_deref(), &invalid) {
            Ok(()) => Ok(()),
            Err(Error::Js(caught)) => handle_error(&view, &missing, &caught),
            Err(err @ Error::Contract(_)) => {
                let caught = throw(err, Some(&model_file));
                handle_error(&view, &missing, &caught)
            }
        }
    };
    body().map_err(|e| throw(e, Some(&model_file)))
}

/// The body of TS `Decorator.validate`'s `try` block.
fn try_validate_decorator(
    view: &JsValue,
    model_file: &JsValue,
    context: Option<&str>,
    invalid: &Option<String>,
) -> Result<()> {
    let name = js_string(&get(view, "name")?)?;
    // TS: `mf.resolveType(decoratedName, this.getName(), this.ast.location);
    // const decoratorDecl = mf.getType(this.getName());` — `getType`
    // returning nothing is treated as `resolveType` failing to resolve the
    // name, the same simplification the native `Decorator::validate` already
    // makes (P2-07 module doc, `resolve_own_name`).
    let Some(decorator_decl) = JsContext.get_type(model_file, Some(&name))? else {
        let raw = format!(
            "Undeclared type \"{}\" in \"{}\".",
            name,
            context.unwrap_or("undefined"),
        );
        let location = opt_get(&get(view, "ast")?, "location")?;
        let message = illegal_model_message(&raw, model_file, &location)?;
        return Err(Error::Js(named_js_error("IllegalModelException", &message)));
    };

    let properties: Vec<PropertyView> = {
        let list = call(
            &decorator_decl,
            "getProperties",
            &[],
            "decoratorDecl.getProperties",
        )?;
        Array::from(&list)
            .iter()
            .map(property_view)
            .collect::<Result<Vec<_>>>()?
    };
    let (required, optional): (Vec<&PropertyView>, Vec<&PropertyView>) =
        properties.iter().partition(|p| !p.optional);
    let ordered: Vec<&PropertyView> = required
        .iter()
        .copied()
        .chain(optional.iter().copied())
        .collect();

    let arguments = Array::from(&get(view, "arguments")?);
    let arg_count = arguments.length() as usize;

    if arg_count < required.len() {
        let names = required
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        report_invalid(
            view,
            invalid,
            format!("Decorator {name} has too few arguments. Required properties are: [{names}]"),
        )?;
    }

    for n in 0..arg_count {
        let arg = arguments.get(n as u32);
        let Some(property) = ordered.get(n) else {
            let names = ordered
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            report_invalid(
                view,
                invalid,
                format!("Decorator {name} has too many arguments. Properties are: [{names}]"),
            )?;
            continue;
        };
        check_argument(view, &name, model_file, property, &arg, invalid)?;
    }
    Ok(())
}

/// TS: one iteration of the `switch (property.getType())` in
/// `Decorator.validate`.
fn check_argument(
    view: &JsValue,
    name: &str,
    model_file: &JsValue,
    property: &PropertyView,
    arg: &JsValue,
    invalid: &Option<String>,
) -> Result<()> {
    match property.type_name.as_deref() {
        Some("Integer") | Some("Double") | Some("Long") => {
            if arg.as_f64().is_none() {
                return report_invalid(
                    view,
                    invalid,
                    format!(
                        "Decorator {name} has invalid decorator argument. Expected number. Found {}, with value {}",
                        js_typeof(arg),
                        json_stringify(arg)?,
                    ),
                );
            }
        }
        Some("String") => {
            if arg.as_string().is_none() {
                return report_invalid(
                    view,
                    invalid,
                    format!(
                        "Decorator {name} has invalid decorator argument. Expected string. Found {}, with value {}",
                        js_typeof(arg),
                        json_stringify(arg)?,
                    ),
                );
            }
        }
        Some("Boolean") => {
            if arg.as_bool().is_none() {
                return report_invalid(
                    view,
                    invalid,
                    format!(
                        "Decorator {name} has invalid decorator argument. Expected boolean. Found {}, with value {}",
                        js_typeof(arg),
                        json_stringify(arg)?,
                    ),
                );
            }
        }
        _ => {
            return check_type_reference_argument(view, name, model_file, property, arg, invalid);
        }
    }
    Ok(())
}

/// TS: the `default:` arm — the argument must be a type reference,
/// resolvable, and assignable to the property's declared type.
fn check_type_reference_argument(
    view: &JsValue,
    name: &str,
    model_file: &JsValue,
    property: &PropertyView,
    arg: &JsValue,
    invalid: &Option<String>,
) -> Result<()> {
    // TS: `typeof arg !== 'object' || arg?.type !== 'Identifier'`.
    let is_type_reference = js_typeof(arg) == "object"
        && opt_get(arg, "type")?.as_string().as_deref() == Some("Identifier");
    if !is_type_reference {
        report_invalid(
            view,
            invalid,
            format!(
                "Decorator {name} has invalid decorator argument. Expected object. Found {}, with value {}",
                js_typeof(arg),
                json_stringify(arg)?,
            ),
        )?;
    }
    // TS: `handleError` above only throws when the decorator validation
    // option is `'error'` (a `?` propagation here, matching TS's `throw`),
    // so under `'warn'` control falls through to here with no
    // `return`/`else` guarding it in the TS `default:` arm, even though
    // `arg` may still not be a type reference. `typeReference.name` is a
    // direct (non-optional) property read of `arg`, which is exactly what
    // `get` already reproduces: V8's own `TypeError` for a nullish `arg`,
    // `undefined` for a non-object `arg`.
    let type_name = js_string(&get(arg, "name")?)?;
    // TS: `mf.getType(typeReference.name)` — non-throwing.
    let Some(type_decl) = JsContext.get_type(model_file, Some(&type_name))? else {
        return report_invalid(
            view,
            invalid,
            format!(
                "Decorator {name} references a type {type_name} which has not been defined/imported."
            ),
        );
    };
    let type_model_file = JsContext.get_model_file(&type_decl)?;
    let type_fqn = JsContext.get_fully_qualified_name(&type_decl)?;
    if !mu::is_assignable_to(&JsContext, &type_model_file, &type_fqn, &property.node)? {
        let property_fqn = JsContext.get_fully_qualified_type_name(&property.node)?;
        report_invalid(
            view,
            invalid,
            format!(
                "Decorator {name} references a type {type_name} which cannot be assigned to the declared type {property_fqn}"
            ),
        )?;
    }
    Ok(())
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

// ---------------------------------------------------------------------------
// Serializer fast path (P4-10, accordproject/concerto-rust#69)
// ---------------------------------------------------------------------------
//
// `Serializer.fromJSON`/`toJSON` cross the boundary in one call each
// (PORTING.md section 5 row 6, D7), rather than per field through the TS
// visitors (which keep their shells and stay the fallback path, plan §3).
// The view (`JSON.stringify`s its argument, reads a JSON string back, per
// `snapshot`'s doc above.
//
// Plain JSON crosses unchanged. Anything else is a one-key object tagged
// `@@oracle` ([`WIRE_TAG`]), so that a value JSON cannot hold (a non-finite
// number, `undefined`, a `Map`, a dayjs, an already-`Resource`/
// `ValidatedResource`/`Relationship` field) still round-trips. This mirrors
// `concerto-core/tests/oracle/instances.rs`'s own codec closely enough to
// reuse its design, but is self-contained here since that module is
// test-only; the TS-side codec (`src/engine/serializer-codec.ts`) writes and
// reads the exact same shapes.
//
// A `"typed"` value's `fields` holds every own property of the TS object,
// in order, `$`-prefixed handles included (`$namespace`, `$type`,
// `$identifierFieldName`, `$identifier`, `$timestamp`, and `$class` for a
// `Relationship`) except `$modelManager`/`$classDeclaration`/`$validator`
// (the view never sends those): decoding needs no separate model lookup, it
// is built directly into the `Instance`'s `props`. A dayjs crosses as
// `(epoch ms, utcOffset minutes)` (PORTING.md 3.3), never a date object;
// D7 keeps dayjs construction in TS, so the view rebuilds it from that pair.

/// The wire tag key, matching the oracle harness's own `M` constant.
const WIRE_TAG: &str = "@@oracle";

/// An engine-side error for a wire shape the codec does not recognise: not
/// a TS bug (the view controls what it sends), so it is reported the same
/// way as any other not-yet-ported call site (PORTING.md 7.2), rather than
/// through the message catalogue.
fn wire_error(reason: String) -> Error {
    ContractError::pre_port(ErrorKind::Error, reason, None).into()
}

/// A JS number that is not finite, or `-0`, in [`WIRE_TAG`]'s `"number"`
/// encoding.
fn decode_wire_number(text: &str) -> Result<f64> {
    match text {
        "NaN" => Ok(f64::NAN),
        "Infinity" => Ok(f64::INFINITY),
        "-Infinity" => Ok(f64::NEG_INFINITY),
        "-0" => Ok(-0.0),
        other => Err(wire_error(format!("an unrecognised wire number {other}"))),
    }
}

/// A `"typed"` wire value (module doc) as an [`Instance`]: `ctor` selects
/// the [`InstanceKind`], `fqn` is `class_fqn`, and every entry of `fields`
/// decodes straight into `props`, in order.
fn decode_wire_typed(map: &serde_json::Map<String, Value>) -> Result<Instance> {
    let kind = match map.get("ctor").and_then(Value::as_str) {
        Some("Resource") => InstanceKind::Resource,
        Some("ValidatedResource") => InstanceKind::ValidatedResource,
        Some("Relationship") => InstanceKind::Relationship,
        other => return Err(wire_error(format!("a typed wire value of class {other:?}"))),
    };
    let fqn = map
        .get("fqn")
        .and_then(Value::as_str)
        .ok_or_else(|| wire_error("a typed wire value without fqn".to_string()))?
        .to_string();
    let fields = map
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| wire_error("a typed wire value without fields".to_string()))?;
    let mut props = SerializerOptions::new();
    for (key, value) in fields {
        props.insert(key.clone(), decode_wire(value)?);
    }
    Ok(Instance {
        kind,
        class_fqn: fqn,
        props,
        validator_options: concerto_core::instance::ValidateOptions::default(),
    })
}

/// A wire value (module doc) as the [`CoreValue`] it decodes to: plain JSON
/// unchanged, and [`WIRE_TAG`]'s `undefined`, `number`, `dayjs`, `map` and
/// `typed` kinds.
fn decode_wire(value: &Value) -> Result<CoreValue> {
    match value {
        Value::Null => Ok(CoreValue::Null),
        Value::Bool(b) => Ok(CoreValue::Bool(*b)),
        Value::Number(n) => Ok(CoreValue::Number(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(s) => Ok(CoreValue::String(s.clone())),
        Value::Array(items) => items
            .iter()
            .map(decode_wire)
            .collect::<Result<Vec<_>>>()
            .map(CoreValue::Array),
        Value::Object(map) => match map.get(WIRE_TAG).and_then(Value::as_str) {
            None => {
                let mut out = SerializerOptions::new();
                for (key, item) in map {
                    out.insert(key.clone(), decode_wire(item)?);
                }
                Ok(CoreValue::Object(out))
            }
            Some("undefined") => Ok(CoreValue::Undefined),
            Some("number") => {
                let text = map
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| wire_error("a wire number without value".to_string()))?;
                decode_wire_number(text).map(CoreValue::Number)
            }
            Some("map") => {
                let entries = map
                    .get("entries")
                    .and_then(Value::as_array)
                    .ok_or_else(|| wire_error("a wire map without entries".to_string()))?;
                let mut decoded = Vec::with_capacity(entries.len());
                for entry in entries {
                    let pair = entry.as_array().ok_or_else(|| {
                        wire_error("a wire map entry that is not a pair".to_string())
                    })?;
                    let key = pair
                        .first()
                        .ok_or_else(|| wire_error("a wire map entry without a key".to_string()))?;
                    let value = pair.get(1).ok_or_else(|| {
                        wire_error("a wire map entry without a value".to_string())
                    })?;
                    decoded.push((decode_wire(key)?, decode_wire(value)?));
                }
                Ok(CoreValue::Map(decoded))
            }
            Some("dayjs") => {
                let valid = map.get("valid").and_then(Value::as_bool).unwrap_or(false);
                if !valid {
                    return Ok(CoreValue::DateTime(Dayjs::utc_invalid()));
                }
                let ms = map
                    .get("ms")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| wire_error("a valid wire dayjs without ms".to_string()))?;
                let offset = map.get("utcOffset").and_then(Value::as_f64).unwrap_or(0.0);
                let built = Dayjs::utc_from_number(ms);
                let built = if offset == 0.0 {
                    built
                } else {
                    built.utc_offset_set(&UtcOffset::Number(offset))
                };
                Ok(CoreValue::DateTime(built))
            }
            Some("typed") => decode_wire_typed(map).map(|i| CoreValue::Instance(Box::new(i))),
            Some(other) => Err(wire_error(format!(
                "a wire value of kind {other} has no engine counterpart"
            ))),
        },
    }
}

/// The options object a serializer call's `optionsText` decodes to
/// (`JSON.stringify`d by the view, `"null"` for no options).
fn decode_wire_options(text: &str) -> Result<Option<SerializerOptions>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
    match value {
        Value::Null => Ok(None),
        Value::Object(map) => {
            let mut options = SerializerOptions::new();
            for (key, item) in &map {
                options.insert(key.clone(), decode_wire(item)?);
            }
            Ok(Some(options))
        }
        _ => Err(wire_error(
            "serializer options that are not a plain object or null".to_string(),
        )),
    }
}

/// A JS number in [`WIRE_TAG`]'s `"number"` encoding: non-finite or `-0`
/// values only, since JSON already holds every other number.
fn encode_wire_number(n: f64) -> Value {
    if !n.is_finite() {
        let text = if n.is_nan() {
            "NaN"
        } else if n > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        };
        return json!({ WIRE_TAG: "number", "value": text });
    }
    if n == 0.0 && n.is_sign_negative() {
        return json!({ WIRE_TAG: "number", "value": "-0" });
    }
    serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
}

/// A dayjs as `(epoch ms, utcOffset minutes)` (PORTING.md 3.3): an invalid
/// one crosses as `{valid: false}`, carrying no time value.
fn encode_wire_dayjs(d: &Dayjs) -> Value {
    if !d.is_valid() {
        return json!({ WIRE_TAG: "dayjs", "valid": false });
    }
    json!({
        WIRE_TAG: "dayjs",
        "valid": true,
        "ms": d.epoch_ms(),
        "utcOffset": d.utc_offset(),
    })
}

/// An [`Instance`] in [`WIRE_TAG`]'s `"typed"` encoding (module doc): every
/// own property, `fields`, plus its class and TS constructor.
fn encode_wire_instance(i: &Instance) -> Value {
    let fields: serde_json::Map<String, Value> = i
        .props
        .iter()
        .map(|(k, v)| (k.clone(), encode_wire(v)))
        .collect();
    json!({
        WIRE_TAG: "typed",
        "ctor": i.kind.ctor(),
        "fqn": i.class_fqn,
        "fields": fields,
    })
}

/// A [`CoreValue`] as the wire value the view reads back (module doc).
fn encode_wire(v: &CoreValue) -> Value {
    match v {
        CoreValue::Undefined => json!({ WIRE_TAG: "undefined" }),
        CoreValue::Null => Value::Null,
        CoreValue::Bool(b) => Value::Bool(*b),
        CoreValue::Number(n) => encode_wire_number(*n),
        CoreValue::String(s) => Value::String(s.clone()),
        CoreValue::Array(items) => Value::Array(items.iter().map(encode_wire).collect()),
        CoreValue::Object(map) => Value::Object(
            map.iter()
                .map(|(k, x)| (k.clone(), encode_wire(x)))
                .collect(),
        ),
        CoreValue::Map(entries) => json!({
            WIRE_TAG: "map",
            "entries": entries
                .iter()
                .map(|(k, x)| Value::Array(vec![encode_wire(k), encode_wire(x)]))
                .collect::<Vec<_>>(),
        }),
        CoreValue::DateTime(d) => encode_wire_dayjs(d),
        CoreValue::Instance(i) => encode_wire_instance(i),
    }
}

/// Calls back the view's `env.newId()`/`env.nowMs()` (D7: the identifier
/// and the clock stay with the caller, `InstanceEnv`'s doc). Both trait
/// methods are infallible, so a callback that throws or returns the wrong
/// type is reported as best it can be (an empty id, or `0`) rather than
/// propagated: a real `Factory.newId`/clock never does either.
struct JsInstanceEnv {
    env: JsValue,
}

impl InstanceEnv for JsInstanceEnv {
    fn new_id(&mut self) -> String {
        call(&self.env, "newId", &[], "env.newId")
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default()
    }

    fn now_ms(&mut self) -> f64 {
        call(&self.env, "nowMs", &[], "env.nowMs")
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    }
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

    /// `Serializer.fromJSON`'s fast path (P4-10; module doc above
    /// "Serializer fast path"): decodes `json_text` (the JSON object to
    /// populate) and `options_text` (the serializer's merged options, or
    /// `"null"`), builds the resource in one call, and returns its wire
    /// encoding as JSON text for the view to materialise into a real
    /// `Resource`/`ValidatedResource`/`Relationship`. `env` is a plain JS
    /// object exposing `newId()`/`nowMs()` (D7).
    #[wasm_bindgen(js_name = serializerFromJson)]
    pub fn serializer_from_json(
        &self,
        json_text: &str,
        options_text: &str,
        env: JsValue,
    ) -> std::result::Result<String, JsValue> {
        run(|| {
            let json_value: Value = serde_json::from_str(json_text)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
            let object = decode_wire(&json_value)?;
            let options = decode_wire_options(options_text)?;
            let serializer = Serializer::new(true, true, options.as_ref())?;
            let mut js_env = JsInstanceEnv { env };
            let resource =
                serializer.from_json(&self.manager, &object, options.as_ref(), &mut js_env)?;
            snapshot(&encode_wire_instance(&resource))
        })
    }

    /// `Serializer.toJSON`'s fast path (P4-10; module doc above): the
    /// counterpart of [`Self::serializer_from_json`]. `wire_text` is the
    /// resource's `"typed"` wire encoding (module doc), `options_text` its
    /// merged options or `"null"`.
    #[wasm_bindgen(js_name = serializerToJson)]
    pub fn serializer_to_json(
        &self,
        wire_text: &str,
        options_text: &str,
    ) -> std::result::Result<String, JsValue> {
        run(|| {
            let wire_value: Value = serde_json::from_str(wire_text)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
            let resource = decode_wire(&wire_value)?;
            let options = decode_wire_options(options_text)?;
            let serializer = Serializer::new(true, true, options.as_ref())?;
            let result = serializer.to_json(&self.manager, &resource, options.as_ref())?;
            snapshot(&encode_wire(&result))
        })
    }
}

// ---------------------------------------------------------------------------
// JSONPopulator / JSONGenerator / ResourceValidator per-field delegation
// (task P4-10, accordproject/concerto-rust#69)
//
// The visitor shells (jsonpopulator.ts, jsongenerator.ts,
// resourcevalidator.ts) stay in TS -- white-box tests spy on their
// `visitX` methods -- but the leaf per-field check or coercion each calls
// (`convertToObject`, `convertToJSON`, `checkItem`'s primitive switch) is
// pure: it needs only the field's declared type name, the JSON value at
// that path, and the serializer's merged options, none of which needs a
// live `ModelManagerHandle`. So these are free functions, not methods on
// `ModelManagerHandle`, and reuse the same wire codec as the whole-document
// fast path above.

/// `JSONPopulator.convertToObject` (task P4-10): `json_text` is the wire
/// encoding (module doc on `decode_wire`) of the value at `path`,
/// `options_text` the serializer's merged options or `"null"`. Returns the
/// coerced value's wire encoding, or throws the same `ValidationException`
/// TS would for that path and type.
#[wasm_bindgen(js_name = populatorConvertPrimitive)]
pub fn populator_convert_primitive(
    type_name: &str,
    json_text: &str,
    options_text: &str,
    path: &str,
) -> std::result::Result<String, JsValue> {
    run(|| {
        let json_value: Value = serde_json::from_str(json_text)
            .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
        let value = decode_wire(&json_value)?;
        let options = decode_wire_options(options_text)?.unwrap_or_default();
        let popt = populator::populator_options(&options);
        let result = populator::convert_primitive(type_name, &value, &popt, path)?;
        snapshot(&encode_wire(&result))
    })
}

/// `JSONGenerator.convertToJSON` (task P4-10): the counterpart of
/// [`populator_convert_primitive`], for `Serializer.toJSON`'s visitor path.
#[wasm_bindgen(js_name = generatorConvertPrimitive)]
pub fn generator_convert_primitive(
    type_name: &str,
    json_text: &str,
    options_text: &str,
) -> std::result::Result<String, JsValue> {
    run(|| {
        let json_value: Value = serde_json::from_str(json_text)
            .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
        let value = decode_wire(&json_value)?;
        let options = decode_wire_options(options_text)?.unwrap_or_default();
        let gopt = generator::generator_options(&options);
        let result = generator::convert_primitive(type_name, &value, &gopt)?;
        snapshot(&encode_wire(&result))
    })
}

/// `ResourceValidator.checkItem`'s primitive-type switch (task P4-10,
/// resourcevalidator.ts:397): whether `json_text`'s value (already coerced
/// by the populator, as a real field value always is here) is valid for
/// the declared primitive `type_name`. A pure predicate -- the TS shell
/// still does its own `reportFieldTypeViolation` (needs `rootResourceIdentifier`
/// and the `Field`, neither of which crosses this call), and still makes
/// the `undefined`/`symbol` check the wire codec cannot cross.
#[wasm_bindgen(js_name = resourceValidatorPrimitiveValid)]
pub fn resource_validator_primitive_valid(
    type_name: &str,
    json_text: &str,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        let json_value: Value = serde_json::from_str(json_text)
            .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
        let value = decode_wire(&json_value)?;
        Ok(populator::primitive_field_valid(type_name, &value))
    })
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

#[cfg(test)]
mod tests {
    // Host-side tests of the pure wire codec (no `js_sys` call is reached on
    // these paths): `cargo test` from concerto-wasm/.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// `decode_wire`, which must succeed (`Error` has no `Debug`).
    fn decoded(value: &Value) -> CoreValue {
        match decode_wire(value) {
            Ok(v) => v,
            Err(_) => panic!("decode_wire failed on {value}"),
        }
    }

    /// The number `decode_wire` reads from `text`, the JSON a view's
    /// `JSON.stringify` wrote.
    fn wire_number(text: &str) -> f64 {
        let value: Value = serde_json::from_str(text).unwrap();
        match decoded(&value) {
            CoreValue::Number(n) => n,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    /// P4-10 review: without serde_json's `float_roundtrip` feature these
    /// two doubles came back 1 ULP off (…888 and …2917).
    #[test]
    fn decode_wire_keeps_doubles_exact() {
        for n in [989.9951327998887_f64, 477.95269883162916_f64] {
            let text = serde_json::to_string(&n).unwrap();
            assert_eq!(wire_number(&text).to_bits(), n.to_bits(), "{text}");
        }
        assert_eq!(
            wire_number("989.9951327998887").to_bits(),
            989.9951327998887_f64.to_bits()
        );
        assert_eq!(
            wire_number("477.95269883162916").to_bits(),
            477.95269883162916_f64.to_bits()
        );
    }

    /// A deterministic pseudo-random sample of finite doubles (xorshift64
    /// over raw bit patterns, so every exponent range is hit): each one
    /// written in shortest round-trip form (what JS `JSON.stringify` writes,
    /// and what `serde_json`/ryu writes too) decodes to the same bits, and
    /// encodes back to the same text.
    #[test]
    fn decode_wire_round_trips_a_random_sample_of_doubles() {
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut checked = 0;
        while checked < 200_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let n = f64::from_bits(state);
            if !n.is_finite() {
                continue;
            }
            let text = serde_json::to_string(&n).unwrap();
            let back = wire_number(&text);
            if n == 0.0 {
                // `-0` crosses as a tagged wire number, never plain JSON.
                assert_eq!(back, 0.0);
            } else {
                assert_eq!(back.to_bits(), n.to_bits(), "{text}");
                assert_eq!(
                    serde_json::to_string(&encode_wire(&CoreValue::Number(back))).unwrap(),
                    text
                );
            }
            checked += 1;
        }
    }

    /// The same doubles inside a document, as `serializerFromJson` and the
    /// per-field bindings decode it.
    #[test]
    fn decode_wire_keeps_nested_doubles_exact() {
        let value: Value = serde_json::from_str(
            r#"{"$class":"org.x@1.0.0.T","d":989.9951327998887,"ds":[477.95269883162916]}"#,
        )
        .unwrap();
        let CoreValue::Object(map) = decoded(&value) else {
            panic!("expected an object");
        };
        assert_eq!(map.get("d"), Some(&CoreValue::Number(989.9951327998887)));
        assert_eq!(
            map.get("ds"),
            Some(&CoreValue::Array(vec![CoreValue::Number(
                477.95269883162916
            )]))
        );
    }
}

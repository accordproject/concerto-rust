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
//!   bound: that stays TS (decorator.rs module doc);
//! - `ModelFile` (P4-08c): `getVersion`, `isSystemModelFile`, `getImports`,
//!   `isLocalType`, `filter` and `validate`, keyed by the same `ModelFileId`
//!   handle every other by-file lookup here already uses, plus a detached
//!   `process`/`fromAst` constructor for a file not yet registered in any
//!   manager (`modelFileFromAst`, near the bottom of the "ModelFile" section).
//!   Its declarations need no new handle: they already cross as the arena's
//!   own `DeclId` (`declarationIds`, `declarationSnapshot`).
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

use concerto_core::dcs;
use concerto_core::error::{ContractError, ErrorKind};
use concerto_core::instance::InstanceEnv;
use concerto_core::instance::dayjs::{Dayjs, UtcOffset};
use concerto_core::instance::resource_id::ResourceId;
use concerto_core::introspect::FullyQualified;
use concerto_core::introspect::decorator::{
    self, Decorator, DecoratorArgument, DecoratorValidationOptions,
};
use concerto_core::introspect::field;
use concerto_core::introspect::property;
use concerto_core::introspect::scalar::{ScalarDeclaration, ScalarValidator};
use concerto_core::introspect::validators;
use concerto_core::introspect::validators::{
    CollectionSizeValidator, NumberValidator, StringValidator, Validator,
};
use concerto_core::model_manager::{DeclId, ModelFileId, ModelFileSource, Node, PropId};
use concerto_core::model_manager::{ResolutionContext, ValidatedElement};
use concerto_core::model_util as mu;
use concerto_core::{Error as CoreError, ModelFile, ModelManager};
use concerto_core_js::{FromJsonOptions, Serializer, SerializerOptions, generator, populator};
use concerto_core_js::{Instance, InstanceKind, JsValue as CoreValue};
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

impl From<CoreError> for Error {
    fn from(err: CoreError) -> Self {
        // A check that no unit has ported yet (the manager's duplicate
        // namespace, its circular-inheritance and handle checks) carries the
        // `pre-port` code and its message verbatim (error/mod.rs,
        // `ContractError::pre_port`), so the shim still picks the TS class
        // from `kind`, like every other core error.
        Self::Contract(Box::new(err.into_contract()))
    }
}

type Result<T> = std::result::Result<T, Error>;

fn kind_name(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::IllegalModel => "IllegalModel",
        ErrorKind::TypeNotFound => "TypeNotFound",
        ErrorKind::Validator => "Validator",
        ErrorKind::Validation => "Validation",
        ErrorKind::InvalidArgument => "Error",
        ErrorKind::MalformedInput => "JsTypeError",
        ErrorKind::RecursionLimit => "JsRangeError",
        ErrorKind::Metamodel => "Metamodel",
        // `ErrorKind` is `#[non_exhaustive]`; a new kind is a plain `Error`
        // until the shim learns it.
        _ => "Error",
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
    // `matches!(err.model_file, Some(Some(_)))` is true exactly when
    // concerto-core has already decided this `IllegalModel` error carries a
    // *real* file name — either `attach_model_file`'s generic backstop
    // (validation.rs), which only ever fills in `contract.model_file` when it
    // was still bare `None` (its own gate), or a check that names a specific
    // file itself (`undeclared_type_error`). It is false for the two other
    // states the field can be in, both of which mean "no file, and none is
    // coming": bare `None`, left untouched by the one check that deliberately
    // skips `attach_model_file` (`ModelFile.validate`'s duplicate-class-name
    // scan, `check_unique_declaration_names` — TS itself never attaches a
    // file to that one), and `Some(None)`, a placeholder some checks set
    // themselves precisely to block `attach_model_file`'s backstop from
    // filling one in later (`validate_map_key`/`validate_map_value`'s own doc
    // comments). Note this is *not* the same test as `err.model_file.is_some()`,
    // which this same function's `modelFile`-attachment `if` below still uses
    // deliberately for its own, WASM-local purpose: a binding like
    // `property_validate` sets `Some(None)` itself, at this layer, as a signal
    // to *do* attach the JS `model_file` it always passes to `throw` — the
    // opposite meaning `Some(None)` carries inside concerto-core.
    //
    // A binding that has no JS `ModelFile` object to hand over (`model_file`
    // is `None` here, e.g. `modelFileValidateDetached`) still can't set
    // `modelFile` on the payload itself, but it can and must tell its JS
    // caller which of the two cases above this is, since only the caller
    // (`ModelFile.validate()`, modelfile.ts) knows which object `this` is to
    // attach — hence this flag travels regardless of whether `model_file` was
    // supplied.
    set(
        &payload,
        "needsModelFile",
        &JsValue::from_bool(matches!(err.model_file, Some(Some(_)))),
    );
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

/// [`throw`] for an error found in the model file registered under
/// `namespace`, whose JS `ModelFile` is `model_files[namespace]` (P5-11,
/// accordproject/concerto-rust#287): that file is attached exactly when
/// concerto-core names a file for the error (`needsModelFile`), as
/// `ModelFile.validate()` re-wraps the error of its own Rust call with
/// `this` (modelfile.ts).
fn throw_naming_file(err: Error, model_files: &JsValue, namespace: &str) -> JsValue {
    let names_file = matches!(&err, Error::Contract(c) if matches!(c.model_file, Some(Some(_))));
    let model_file = if names_file && model_files.is_object() {
        Reflect::get(model_files, &JsValue::from_str(namespace)).ok()
    } else {
        None
    };
    throw(err, model_file.as_ref().filter(|mf| !nullish(mf)))
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
    ContractError::new(ErrorKind::MalformedInput, code, params).into()
}

/// A catalogue `Error`, built the same way `type_error` builds a `TypeError`.
fn plain_error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::InvalidArgument, code, params).into()
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

/// `value.name?.()`: `None` when the method is nullish. `value` itself is
/// read unguarded, matching every TS call site this backs (including
/// `MapValueType.validate`'s deliberately-unguarded `decl.isMapDeclaration?.()`,
/// P4-08e/#189 DV note): a nullish `value` throws the same
/// "Cannot read properties of null/undefined" `TypeError` TS's property
/// read would.
fn call_optional(value: &JsValue, name: &str) -> Result<Option<JsValue>> {
    let method = get(value, name)?;
    if method.is_undefined() || method.is_null() {
        return Ok(None);
    }
    call(value, name, &[], name).map(Some)
}

/// A JS value as JSON, `None` for `undefined`. Values JSON cannot hold
/// (`NaN`, `Infinity`, functions) are not modelled: no model AST holds one.
///
/// A JS string may hold an unpaired UTF-16 surrogate (no valid Unicode
/// scalar exists for one alone); `JSON.stringify` still emits it as a
/// `\uD800`-range escape, which `serde_json` — building a real (UTF-8) Rust
/// `String` — rejects. This used to be swallowed by `.ok()`, turning the
/// *entire* value into `None` and silently discarding every other field
/// alongside it (accordproject/concerto-rust#73, P5-02 review: a property
/// AST's `name` field disappearing this way surfaced as a generic
/// `Error('No name for type null')` instead of `property::process`'s own,
/// correctly-classed `IllegalModelException` for an invalid name). Each
/// unpaired escape is replaced with U+FFFD instead, so parsing still
/// succeeds and every other field survives; the sanitized string content
/// then fails whatever check reads it on its own, correctly-classed terms
/// (e.g. `is_valid_identifier`), same as any other invalid string would.
fn to_json(value: &JsValue) -> Result<Option<Value>> {
    if value.is_undefined() {
        return Ok(None);
    }
    let text = JSON::stringify(value).map_err(Error::Js)?;
    let Some(text) = text.as_string() else {
        return Ok(None);
    };
    match serde_json::from_str(&text) {
        Ok(v) => Ok(Some(v)),
        Err(_) => {
            let sanitized = sanitize_lone_surrogate_escapes(&text);
            serde_json::from_str(&sanitized)
                .map(Some)
                .map_err(|e| Error::Js(js_sys::Error::new(&format!("to_json: {e}")).into()))
        }
    }
}

/// Replaces every `\uXXXX` escape inside a JSON string literal that is an
/// unpaired UTF-16 surrogate (high without an immediately following low, or
/// low without an immediately preceding high) with the `�` escape,
/// leaving every other character — including valid surrogate pairs and
/// every other escape — untouched. Only escapes inside string literals are
/// considered; the surrounding JSON structure (keys, punctuation) never
/// contains a `\u` sequence of its own in text `JSON.stringify` produces.
fn sanitize_lone_surrogate_escapes(text: &str) -> String {
    /// Reads a `\uXXXX` escape's 4 hex digits starting at `chars[at]`,
    /// returning the unit and its source characters, or `None` if `at` is
    /// out of range or the 4 characters there are not all hex digits.
    fn hex_unit(chars: &[char], at: usize) -> Option<(u32, &[char])> {
        let digits = chars.get(at..at + 4)?;
        let s: String = digits.iter().collect();
        Some((u32::from_str_radix(&s, 16).ok()?, digits))
    }

    /// Whether `chars[at..at + 2]` is a `\u` escape opener.
    fn is_u_escape(chars: &[char], at: usize) -> bool {
        chars.get(at..at + 2) == Some(['\\', 'u'].as_slice())
    }

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if !in_string {
            out.push(c);
            if c == '"' {
                in_string = true;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = false;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '\\'
            && is_u_escape(&chars, i)
            && let Some((unit, digits)) = hex_unit(&chars, i + 2)
        {
            if (0xD800..=0xDBFF).contains(&unit) {
                // High surrogate: valid only if immediately followed by a
                // low-surrogate escape.
                let low = is_u_escape(&chars, i + 6)
                    .then(|| hex_unit(&chars, i + 8))
                    .flatten();
                if let Some((l, _)) = low
                    && (0xDC00..=0xDFFF).contains(&l)
                {
                    out.push_str("\\u");
                    out.extend(digits);
                    i += 6;
                    continue;
                }
                out.push_str("\\uFFFD");
                i += 6;
                continue;
            }
            if (0xDC00..=0xDFFF).contains(&unit) {
                // A low surrogate reaching here was not just consumed as
                // the second half of a pair above, so it is unpaired on
                // its own.
                out.push_str("\\uFFFD");
                i += 6;
                continue;
            }
        }
        if let Some(&next) = (c == '\\').then(|| chars.get(i + 1)).flatten() {
            // Any other escape (`\\`, `\"`, `\n`, a non-surrogate `\uXXXX`,
            // …): copy the backslash and its one following character
            // through unchanged; the loop picks back up correctly whether
            // that was a simple escape or the first half of `\uXXXX`.
            out.push(c);
            out.push(next);
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
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
    run(|| Ok(mu::short_name(&receiver(&fqn, "fqn", "lastIndexOf")?).to_string()))
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

/// TS `ModelUtil.parseNamespace(ns, {disableVersionParsing})` of a JS value:
/// `!ns` fails for every falsy value; a truthy non-string has no `split`.
fn parse_namespace_js(ns: &JsValue, disable: bool) -> Result<mu::ParsedNamespace> {
    if ns.is_truthy() {
        let ns = receiver(ns, "ns", "split")?;
        Ok(mu::parse_namespace_with(Some(&ns), disable)?)
    } else {
        Ok(mu::parse_namespace_with(None, disable)?)
    }
}

/// Whether `ns` is a non-empty string with no `@`: a namespace with no
/// version, which `parseNamespace` rejects since BC-02 (R1, P5-50), and
/// which the model file header and `enforceImportVersioning` reject with
/// their own errors, as TS 5.0.0 did.
fn is_unversioned_namespace(ns: &JsValue) -> bool {
    ns.as_string()
        .is_some_and(|ns| !ns.is_empty() && !ns.contains('@'))
}

/// TS: ModelUtil.parseNamespace. When the engine gives a `versionParsed`
/// (a strict SemVer 2.0.0 version within node-semver's limits), the JS
/// value is built by the registered `semver.parse`, so that it is a real
/// node-semver `SemVer`; otherwise it is `null`, including for a version
/// beyond node-semver's limits (BC-41, P5-38), where `semver.parse` gives
/// `null` too.
#[wasm_bindgen(js_name = modelUtilParseNamespace)]
pub fn model_util_parse_namespace(
    ns: JsValue,
    options: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let disable = !nullish(&options) && get(&options, "disableVersionParsing")?.is_truthy();
        let parsed = parse_namespace_js(&ns, disable)?;
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

/// TS: ModelUtil.parseNamespace, with the version checked in Rust only
/// (P5-20, F4): no `semver.parse` callback, and the result comes back as
/// one string rather than an object built property by property across the
/// boundary. It throws what `modelUtilParseNamespace` throws. Otherwise the
/// first character says which result it is, and the rest holds its parts
/// separated by `@` (no part can contain one: the namespace has at most
/// one, and `name` and `version` are the text either side of it):
/// - `N<name>`: `{ name }` (`disableVersionParsing`);
/// - `U<name>@<escapedNamespace>`: no version, so `version` and
///   `versionParsed` are `null`;
/// - `V<name>@<escapedNamespace>@<version>`: the shim builds
///   `versionParsed` itself with `semver.parse`, in JS, where it costs far
///   less than a callback across the boundary. The Rust check is strict
///   SemVer 2.0.0 (BC-41), which `semver.parse` accepts too, except where
///   node-semver's own limits reject it (a component above
///   `Number.MAX_SAFE_INTEGER`, or more than 256 UTF-16 units): there
///   `semver.parse` returns `null`, as the engine's own `versionParsed` is
///   `None` (`model_util::semver_parse`).
#[wasm_bindgen(js_name = modelUtilParseNamespaceChecked)]
pub fn model_util_parse_namespace_checked(
    ns: JsValue,
    options: JsValue,
) -> std::result::Result<String, JsValue> {
    run(|| {
        let disable = !nullish(&options) && get(&options, "disableVersionParsing")?.is_truthy();
        Ok(match parse_namespace_js(&ns, disable)? {
            mu::ParsedNamespace::NameOnly { name } => format!("N{name}"),
            mu::ParsedNamespace::Full {
                name,
                escaped_namespace,
                version: None,
                ..
            } => format!("U{name}@{escaped_namespace}"),
            mu::ParsedNamespace::Full {
                name,
                escaped_namespace,
                version: Some(version),
                ..
            } => format!("V{name}@{escaped_namespace}@{version}"),
        })
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

/// TS: ModelUtil.isValidIdentifier. A non-string (`undefined`, `null`, a
/// number, ...) is not a valid identifier (BC-01, R1). TS 5.0.0 passed it to
/// `RegExp.prototype.test`, which converts it with `String()`, so
/// `undefined` and `null` answered `true` (DV-002).
#[wasm_bindgen(js_name = modelUtilIsValidIdentifier)]
pub fn model_util_is_valid_identifier(name: JsValue) -> std::result::Result<bool, JsValue> {
    run(|| {
        Ok(name
            .as_string()
            .is_some_and(|name| mu::is_valid_identifier(&name)))
    })
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
        let joined = mu::qualify(&js_string(&namespace)?, &js_string(&type_name)?);
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

/// `{pattern, flags}`, or `None` for a nullish value. Built through
/// `validators::regex_validator_from_ast`, which reads `pattern`/`flags`
/// completely untyped — a plain `ToString`-style coercion, matching `new
/// RegExp(validator.pattern, validator.flags)` — rather than `serde`'s
/// strict decode this used to run directly: TS's own call site for this
/// constructor is `Property.process`'s `new StringValidator(this,
/// this.ast.validator, this.ast.lengthValidator)` (property.ts/field.ts),
/// reading `this.ast.validator` with no type check at all, so a
/// fuzz-mutated, wrongly-typed `pattern`/`flags` (a bool, a number, an
/// array) must coerce here too, not fail the whole property's `process()`
/// (accordproject/concerto-rust#217: this binding is *TS's* call site for
/// the same constructor `validators::regex_validator_from_ast`'s own doc
/// comment already fixed the `Property::try_from` side of, so it needs the
/// identical fix).
fn string_regex_ast(value: &JsValue) -> Result<Option<mm::StringRegexValidator>> {
    if nullish(value) {
        return Ok(None);
    }
    let json = to_json(value)?.unwrap_or(Value::Null);
    Ok(validators::regex_validator_from_ast(Some(&json)))
}

/// `{minLength, maxLength}`, or `None` for a nullish value. Built through
/// `validators::length_validator_from_ast`, for the same reason and in the
/// same way as [`string_regex_ast`] — TS's own call site for
/// `new StringValidator(..., this.ast.lengthValidator)`
/// (accordproject/concerto-rust#217).
fn string_length_ast(value: &JsValue) -> Result<Option<mm::StringLengthValidator>> {
    if nullish(value) {
        return Ok(None);
    }
    let json = to_json(value)?.unwrap_or(Value::Null);
    Ok(validators::length_validator_from_ast(Some(&json)))
}

/// `{minSize, maxSize}`. Built through `validators::size_validator_from_ast`,
/// for the same reason as [`string_regex_ast`] — TS's own call site for
/// `new CollectionSizeValidator(this, this.ast.sizeValidator)`
/// (property.ts/field.ts), reading `minSize`/`maxSize` with no type check
/// (accordproject/concerto-rust#217). A nullish `value` (this binding's own
/// caller, like TS's constructor call site, only ever passes one when
/// `this.ast.sizeValidator` is itself present) falls back to the same
/// "$class only" node `size_validator_from_ast`'s own null-filter maps to
/// `None` for, so this preserves this function's pre-existing contract of
/// never itself returning `None`: an absent `minSize`/`maxSize` decodes as
/// `None` either way, so the unwrap below only ever supplies the
/// `$class`/bounds-absent shape.
fn collection_size_ast(value: &JsValue) -> Result<mm::CollectionSizeValidator> {
    let json = to_json(value)?.unwrap_or(Value::Null);
    Ok(
        validators::size_validator_from_ast(Some(&json)).unwrap_or(mm::CollectionSizeValidator {
            _class: String::new(),
            min_size: None,
            max_size: None,
        }),
    )
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
        // The raw `lengthValidator` AST itself, not just its typed
        // `{minLength, maxLength}` snapshot (`length_ast` above): a
        // fuzz-mutated `minLength`/`maxLength` needs JS's own untyped `>`
        // comparison, not one against a value already coerced to `f64`
        // (accordproject/concerto-rust#219). `nullish` mirrors
        // `string_length_ast`'s own guard: no AST, no raw value to compare.
        let raw_length_ast = if nullish(&length_validator) {
            None
        } else {
            to_json(&length_validator)?
        };
        let built = StringValidator::new(
            &JsElement { validator: &view },
            regex_ast.as_ref(),
            length_ast.as_ref(),
            raw_length_ast.as_ref(),
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
        // No raw AST to re-compare here: this rebuilds the validator from a
        // view whose construction already succeeded, so `minLength`/
        // `maxLength` are already the real, in-order numbers a prior,
        // successful `stringValidatorNew` call normalised.
        None,
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
        // The raw `sizeValidator` AST itself, not just its typed `{minSize,
        // maxSize}` snapshot (`typed` above): a fuzz-mutated `minSize`/
        // `maxSize` needs JS's own untyped `>` comparison, not one against a
        // value already coerced to `f64` (accordproject/concerto-rust#219).
        let raw_ast = to_json(&ast)?;
        let built = CollectionSizeValidator::new(
            &JsElement { validator: &view },
            &typed,
            raw_ast.as_ref(),
        )?;
        Ok(to_js(&json!({
            "minSize": built.min_size(),
            "maxSize": built.max_size(),
        })))
    })
}

/// Rebuilds the validator from `view.validator`, the AST `super()` cached.
fn collection_size_validator(view: &JsValue) -> Result<CollectionSizeValidator> {
    let ast = collection_size_ast(&get(view, "validator")?)?;
    // No raw AST to re-compare here: this rebuilds the validator from a view
    // whose construction already succeeded, so `minSize`/`maxSize` are
    // already the real, in-order numbers a prior, successful
    // `collectionSizeValidatorNew` call normalised.
    CollectionSizeValidator::new(&JsElement { validator: view }, &ast, None)
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
        validator.validate(
            &JsElement { validator: &view },
            identifier.as_deref(),
            value,
        )
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

/// The snapshot [`property_process`] returns for a processed property.
fn property_snapshot(processed: &property::ProcessedProperty) -> Value {
    let mut snapshot = serde_json::Map::new();
    snapshot.insert("name".to_string(), json!(processed.name));
    if processed.type_set {
        snapshot.insert("type".to_string(), json!(processed.property_type));
    }
    snapshot.insert("array".to_string(), json!(processed.array));
    snapshot.insert("optional".to_string(), json!(processed.optional));
    Value::Object(snapshot)
}

/// The snapshot [`field_process`] returns for a processed field.
fn field_snapshot(processed: &field::ProcessedField) -> Value {
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
    json!({
        "validator": validator,
        "defaultValue": processed.default_value,
    })
}

/// P5-06: the [`property_process`] and [`field_process`] snapshots of every
/// property of every declaration of a model file, computed in one call from
/// the model's JSON AST text (`JSON.stringify(ast)`), so that a `ModelFile`
/// view built in rust mode crosses the boundary once for all of its
/// properties instead of twice per property. Returns JSON text: an array
/// aligned with `ast.declarations`, holding, for a declaration with a
/// `properties` array, an array aligned with it of `{p, f}` entries (`p`
/// the `propertyProcess` snapshot, `f` the `fieldProcess` one for a
/// property whose `type` is what `p` sets) and otherwise `null`.
///
/// Never throws: any property whose own binding would throw (or that is not
/// a JSON object) gets `null` instead of an entry, as does a field whose
/// `fieldProcess` would, and `undefined` comes back for text that is not a
/// model AST, so the view falls back to the per-property bindings, which
/// raise every error exactly as before. A property whose `p` entry leaves
/// `type` unset (TS never assigns it) gets the `f` its fresh view would:
/// computed with no type.
#[wasm_bindgen(js_name = modelFilePropertySnapshots)]
pub fn model_file_property_snapshots(ast: &str) -> Option<String> {
    // Only the fields `property::process`/`field::process` read are parsed;
    // anything this light shape cannot read (a declaration or property that
    // is not an object, a `properties` that is not an array, a duplicate
    // key) fails the whole batch, and the view falls back to the
    // per-property bindings for every property.
    let model: LightModel = serde_json::from_str(ast).ok()?;
    let declarations = model.declarations?;
    // Written out directly rather than built as a `Value` first: the
    // entries are small and many, and building them was most of the cost.
    let mut out = String::with_capacity(ast.len() / 4);
    out.push('[');
    for (i, declaration) in declarations.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        match declaration.properties {
            None => out.push_str("null"),
            Some(properties) => {
                out.push('[');
                for (j, property) in properties.into_iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    write_property_entry(&mut out, &property.into_value())?;
                }
                out.push(']');
            }
        }
    }
    out.push(']');
    Some(out)
}

/// A model AST, as far as [`model_file_property_snapshots`] reads it.
#[derive(serde::Deserialize)]
struct LightModel {
    declarations: Option<Vec<LightDeclaration>>,
}

/// A declaration, as far as [`model_file_property_snapshots`] reads it.
#[derive(serde::Deserialize)]
struct LightDeclaration {
    properties: Option<Vec<LightProperty>>,
}

/// A property node, keeping only the keys `property::process` and
/// `field::process` read. A `null` value reads as absent, which both treat
/// the same way (`as_str`/`as_bool`/truthiness/`!Util.isNull`).
#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct LightProperty {
    #[serde(rename = "$class")]
    class: Option<Value>,
    name: Option<Value>,
    #[serde(rename = "type")]
    type_: Option<Value>,
    isArray: Option<Value>,
    isOptional: Option<Value>,
    validator: Option<Value>,
    lengthValidator: Option<Value>,
    defaultValue: Option<Value>,
    // P5-10b: read by `model_file_view_snapshot` only, never part of
    // `into_value`.
    decorators: Option<Value>,
    sizeValidator: Option<Value>,
}

impl LightProperty {
    /// The property as the JSON object the per-property bindings would
    /// have read those keys from.
    fn into_value(self) -> Value {
        let mut map = serde_json::Map::new();
        for (key, value) in [
            ("$class", self.class),
            ("name", self.name),
            ("type", self.type_),
            ("isArray", self.isArray),
            ("isOptional", self.isOptional),
            ("validator", self.validator),
            ("lengthValidator", self.lengthValidator),
            ("defaultValue", self.defaultValue),
        ] {
            if let Some(value) = value {
                map.insert(key.to_string(), value);
            }
        }
        Value::Object(map)
    }
}

/// Appends one `{p, f}` entry of [`model_file_property_snapshots`] (or
/// `null`) to `out` as JSON text: the same JSON `property_snapshot` and
/// `field_snapshot` serialise to.
fn write_property_entry(out: &mut String, ast: &Value) -> Option<()> {
    let Ok(processed) = property::process::<Error>(ast) else {
        out.push_str("null");
        return Some(());
    };
    out.push_str("{\"p\":{\"name\":");
    out.push_str(&serde_json::to_string(&processed.name).ok()?);
    if processed.type_set {
        out.push_str(",\"type\":");
        out.push_str(&serde_json::to_string(&processed.property_type).ok()?);
    }
    out.push_str(if processed.array {
        ",\"array\":true"
    } else {
        ",\"array\":false"
    });
    out.push_str(if processed.optional {
        ",\"optional\":true}"
    } else {
        ",\"optional\":false}"
    });
    let property_type = if processed.type_set {
        processed.property_type.as_deref()
    } else {
        None
    };
    // A field error names the view's fully-qualified name, which only the
    // view knows: never produced here (the entry falls back instead).
    let no_fqn = || -> Result<String> { Err(Error::Js(JsValue::UNDEFINED)) };
    out.push_str(",\"f\":");
    match field::process(property_type, ast, &no_fqn) {
        Ok(field) => out.push_str(&serde_json::to_string(&field_snapshot(&field)).ok()?),
        Err(_) => out.push_str("null"),
    }
    out.push('}');
    Some(())
}

/// P5-10a: the view snapshot of a whole model file, computed in one call
/// from its JSON AST text (`JSON.stringify(ast)`), so that building a
/// `ModelFile`'s declaration and property views crosses the boundary once
/// for the file. It extends [`model_file_property_snapshots`] with each
/// declaration's own construction-time decisions: `Declaration.process`'s
/// `isValidIdentifier`/`getFullyQualifiedName` calls and
/// `ClassDeclaration.process`'s [`class_declaration_process`] decision.
/// `namespace` is the file's `ModelFile.getNamespace()` (the AST's own
/// `namespace`).
///
/// Returns JSON text: an array aligned with `ast.declarations`, holding for
/// each declaration `{"d": d, "p": p}`:
/// - `d` is `{"name", "fqn", "cd", "defaulted"}` for a declaration whose
///   `name` is a string and a valid identifier, when `namespace` is
///   non-empty; otherwise `null`. `fqn` is what
///   `modelUtilGetFullyQualifiedName(namespace, name)` returns. `cd` is the
///   `classDeclarationProcess` snapshot, or `null` where this light reading
///   cannot decide it exactly as the binding would (a `superType` or
///   `identified` node that is not a plain object with a string `name`, or
///   a super-type-less `Concept`, whose decision depends on
///   `ModelFile.isSystemModelFile()`). `defaulted` is true when `cd` was
///   computed with the default super type `ModelFile.fromAst` gives an
///   asset, participant, transaction or event declaration that names none
///   (the view is then built from a copy of the declaration's AST node).
/// - `p` is the declaration's [`model_file_property_snapshots`] entry array,
///   or `null`.
///
/// Never throws: whatever the view would raise an error for gets `null`
/// here, and the view then calls the per-element binding exactly as before,
/// so every error comes from the same call as without the snapshot.
/// `undefined` comes back for text this light reading cannot read at all.
/// Additive.
#[wasm_bindgen(js_name = modelFileViewSnapshot)]
pub fn model_file_view_snapshot(ast: &str, namespace: Option<String>) -> Option<String> {
    let model: ViewModel = serde_json::from_str(ast).ok()?;
    let declarations = model.declarations?;
    let namespace = namespace.unwrap_or_default();
    let mut out = String::with_capacity(ast.len() / 3);
    out.push('[');
    for (i, declaration) in declarations.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"d\":");
        match declaration_view_entry(&declaration, &namespace) {
            Some(entry) => out.push_str(&entry.to_string()),
            None => out.push_str("null"),
        }
        // P5-10b: the declaration's own decorators, and its scalar or map
        // decisions, each only when it has one.
        write_optional(
            &mut out,
            "dec",
            decorators_view_snapshot(declaration.decorators.as_ref()),
        );
        write_optional(&mut out, "s", scalar_view_snapshot(&declaration));
        write_optional(&mut out, "m", map_view_snapshot(&declaration));
        out.push_str(",\"p\":");
        match declaration.properties {
            None => out.push_str("null"),
            Some(properties) => {
                out.push('[');
                for (j, property) in properties.into_iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    write_view_property_entry(&mut out, property)?;
                }
                out.push(']');
            }
        }
        out.push('}');
    }
    out.push(']');
    Some(out)
}

/// Appends `,"key":value` to `out` when `value` is `Some`.
fn write_optional(out: &mut String, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        out.push_str(",\"");
        out.push_str(key);
        out.push_str("\":");
        out.push_str(&value.to_string());
    }
}

/// P5-10b: one property's entry of [`model_file_view_snapshot`]: the
/// [`write_property_entry`] `{p, f}` entry (or `null`), extended with the
/// property's lazily built parts, each only when it can be decided here
/// exactly as its per-element binding decides it:
/// - `dec`: its decorators ([`decorators_view_snapshot`]);
/// - `sz`: its `collectionSizeValidatorNew` snapshot `{minSize, maxSize}`,
///   for a truthy `sizeValidator` whose constructor succeeds;
/// - `sv`: its `stringValidatorNew` snapshot `{minLength, maxLength}`, for a
///   field whose `f` entry has a `StringValidator` whose constructor
///   succeeds (the view only uses it when no custom `options.regExp` is
///   configured, as the binding is only called then).
fn write_view_property_entry(out: &mut String, mut property: LightProperty) -> Option<()> {
    let decorators = property.decorators.take();
    let size_validator = property.sizeValidator.take();
    let ast = property.into_value();
    let start = out.len();
    write_property_entry(out, &ast)?;
    if !out[start..].starts_with('{') {
        return Some(());
    }
    // The entry's closing brace, reopened for the extra keys.
    out.pop();
    write_optional(out, "dec", decorators_view_snapshot(decorators.as_ref()));
    let name = ast.get("name").and_then(Value::as_str);
    if let Some(name) = name {
        write_optional(
            out,
            "sz",
            size_validator_view_snapshot(name, size_validator.as_ref()),
        );
        // `Field.process`'s `StringValidator` arm (field::process): a
        // `String` property with a truthy `validator` or `lengthValidator`.
        let string_typed = property::process::<Error>(&ast)
            .is_ok_and(|p| p.type_set && p.property_type.as_deref() == Some("String"));
        if string_typed
            && (json_truthy(ast.get("validator")) || json_truthy(ast.get("lengthValidator")))
        {
            write_optional(out, "sv", string_validator_view_snapshot(name, &ast));
        }
    }
    out.push('}');
    Some(())
}

/// The element a validator built from JSON is attached to, for
/// [`model_file_view_snapshot`]: its `getName()` and `ast.defaultValue`. It
/// has no fully qualified name, so any error a constructor would report
/// fails, and the entry is left out (the view then calls the binding,
/// which raises it).
struct JsonElement<'a> {
    name: &'a str,
    default_value: Option<&'a Value>,
}

impl FullyQualified for JsonElement<'_> {
    type Error = Error;

    fn fully_qualified_name(&self) -> Result<String> {
        Err(Error::Js(JsValue::UNDEFINED))
    }
}

impl ValidatedElement for JsonElement<'_> {
    fn default_value(&self) -> Result<Option<Value>> {
        Ok(self.default_value.cloned())
    }

    fn name(&self) -> Result<String> {
        Ok(self.name.to_string())
    }
}

/// `collectionSizeValidatorNew`'s snapshot for a property's truthy
/// `sizeValidator` (TS: `this.ast.sizeValidator ? new
/// CollectionSizeValidator(this, this.ast.sizeValidator) : null`), or `None`
/// when there is none or its constructor would throw. The bounds are
/// compared as numbers: the file loaded, and BC-19's shape check rejects a
/// bound that is not one (P5-61; the binding keeps the raw comparison for a
/// validator built outside a model load).
fn size_validator_view_snapshot(name: &str, ast: Option<&Value>) -> Option<Value> {
    let ast = ast.filter(|v| json_truthy(Some(v)))?;
    let typed =
        validators::size_validator_from_ast(Some(ast)).unwrap_or(mm::CollectionSizeValidator {
            _class: String::new(),
            min_size: None,
            max_size: None,
        });
    let element = JsonElement {
        name,
        default_value: None,
    };
    let built = CollectionSizeValidator::new(&element, &typed, None).ok()?;
    Some(json!({ "minSize": built.min_size(), "maxSize": built.max_size() }))
}

/// `stringValidatorNew`'s snapshot for an element's `validator` and
/// `lengthValidator` (a field, or a String scalar), or `None` when its
/// constructor would throw. The length bounds are compared as numbers, as
/// in [`size_validator_view_snapshot`].
fn string_validator_view_snapshot(name: &str, ast: &Value) -> Option<Value> {
    let validator = ast.get("validator").filter(|v| !v.is_null());
    let length_validator = ast.get("lengthValidator").filter(|v| !v.is_null());
    let regex_ast = validators::regex_validator_from_ast(validator);
    let length_ast = validators::length_validator_from_ast(length_validator);
    let element = JsonElement {
        name,
        default_value: ast.get("defaultValue"),
    };
    let built =
        StringValidator::new(&element, regex_ast.as_ref(), length_ast.as_ref(), None).ok()?;
    Some(json!({ "minLength": built.min_length(), "maxLength": built.max_length() }))
}

/// P5-10b: the `decoratorProcess` results for an AST `decorators` value, as
/// `[{"n": name, "a": arguments}]` (`n` left out for a node with no `name`,
/// a type reference argument's `array` left out when its `isArray` is), or
/// `None` when there is nothing to build (no decorators) or it cannot be
/// decided here exactly as the binding decides it: a `decorators` that is
/// not an array, a node that is not an object or whose `name` is present
/// but not a string, or a number argument JSON cannot carry.
fn decorators_view_snapshot(decorators: Option<&Value>) -> Option<Value> {
    let Some(Value::Array(nodes)) = decorators else {
        return None;
    };
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        let Value::Object(map) = node else {
            return None;
        };
        if map.get("name").is_some_and(|n| !n.is_string()) {
            return None;
        }
        let decorator = Decorator::from_ast(node);
        let mut arguments = Vec::with_capacity(decorator.arguments().len());
        for argument in decorator.arguments() {
            arguments.push(match argument {
                DecoratorArgument::String(s) => json!(s),
                DecoratorArgument::Number(n) => {
                    Value::Number(serde_json::Number::from_f64(*n).filter(|_| n.is_finite())?)
                }
                DecoratorArgument::Boolean(b) => json!(b),
                DecoratorArgument::TypeReference(t) => match t.array {
                    Some(array) => json!({ "type": "Identifier", "name": t.name, "array": array }),
                    None => json!({ "type": "Identifier", "name": t.name }),
                },
                // `DecoratorArgument` is `#[non_exhaustive]`: a kind the
                // fast path does not know falls back to the view.
                _ => return None,
            });
        }
        let mut entry = serde_json::Map::new();
        if let Some(name) = decorator.js_name() {
            entry.insert("n".to_string(), json!(name));
        }
        entry.insert("a".to_string(), Value::Array(arguments));
        out.push(Value::Object(entry));
    }
    Some(Value::Array(out))
}

/// The metamodel classes `ModelFile.fromAst` builds a `ScalarDeclaration`
/// from.
const SCALAR_CLASSES: [&str; 6] = [
    "concerto.metamodel@1.0.0.BooleanScalar",
    "concerto.metamodel@1.0.0.IntegerScalar",
    "concerto.metamodel@1.0.0.LongScalar",
    "concerto.metamodel@1.0.0.DoubleScalar",
    "concerto.metamodel@1.0.0.StringScalar",
    "concerto.metamodel@1.0.0.DateTimeScalar",
];

/// P5-10b: a scalar declaration's `scalarDeclarationProcess` snapshot
/// `{type, validator, defaultValue}`, where a `StringValidator` also carries
/// its `stringValidatorNew` snapshot (`minLength`, `maxLength`), or `None`
/// when the declaration is not a scalar or processing it would throw.
fn scalar_view_snapshot(declaration: &ViewDeclaration) -> Option<Value> {
    let class = declaration.class.as_ref().and_then(Value::as_str)?;
    if !SCALAR_CLASSES.contains(&class) {
        return None;
    }
    let Some(Value::String(name)) = &declaration.name else {
        return None;
    };
    let mut ast = serde_json::Map::new();
    ast.insert("$class".to_string(), json!(class));
    ast.insert("name".to_string(), json!(name));
    for (key, value) in [
        ("validator", &declaration.validator),
        ("lengthValidator", &declaration.lengthValidator),
        ("defaultValue", &declaration.defaultValue),
    ] {
        if let Some(value) = value {
            ast.insert(key.to_string(), value.clone());
        }
    }
    let ast = Value::Object(ast);
    let no_fqn = || -> Result<String> { Err(Error::Js(JsValue::UNDEFINED)) };
    let processed = ScalarDeclaration::process(&ast, None, &no_fqn).ok()?;
    let validator = match &processed.validator {
        None => Value::Null,
        Some(ScalarValidator::Number(v)) => {
            let mut snapshot = serde_json::to_value(v).ok()?;
            let Value::Object(map) = &mut snapshot else {
                return None;
            };
            map.insert("kind".to_string(), json!("NumberValidator"));
            snapshot
        }
        Some(ScalarValidator::String { .. }) => {
            let mut snapshot = string_validator_view_snapshot(name, &ast)?;
            let Value::Object(map) = &mut snapshot else {
                return None;
            };
            map.insert("kind".to_string(), json!("StringValidator"));
            snapshot
        }
    };
    Some(json!({
        "type": processed.scalar_type,
        "validator": validator,
        "defaultValue": processed.default_value,
    }))
}

/// P5-10b: a map declaration's `mapDeclarationProcess` decision, with its
/// key and value types' `mapKeyTypeProcess`/`mapValueTypeProcess` types and
/// decorators, as `{"k": {"t", "dec"?}, "v": {"t", "dec"?}}`, or `None` when
/// the declaration is not a map or any of those would throw (or cannot be
/// decided here exactly as the bindings decide it).
fn map_view_snapshot(declaration: &ViewDeclaration) -> Option<Value> {
    if declaration.class.as_ref().and_then(Value::as_str)
        != Some("concerto.metamodel@1.0.0.MapDeclaration")
    {
        return None;
    }
    // `mapDeclarationProcess`: `this.ast.name` is interpolated into its
    // errors; a map with a string name only.
    if !matches!(declaration.name, Some(Value::String(_))) {
        return None;
    }
    let key = declaration.key.as_ref().filter(|v| json_truthy(Some(v)))?;
    let value = declaration
        .value
        .as_ref()
        .filter(|v| json_truthy(Some(v)))?;
    if !matches!(key, Value::Object(_)) || !matches!(value, Value::Object(_)) {
        return None;
    }
    if !mu::is_valid_map_key(Some(key)).ok()? || !mu::is_valid_map_value(Some(value)).ok()? {
        return None;
    }
    fn class(node: &Value) -> Option<&str> {
        node.get("$class").and_then(Value::as_str).map(short_class)
    }
    let key_type = match class(key)? {
        "DateTimeMapKeyType" => "DateTime".to_string(),
        "StringMapKeyType" => "String".to_string(),
        "ObjectMapKeyType" => key.get("type")?.get("name")?.as_str()?.to_string(),
        _ => return None,
    };
    let value_type = match class(value)? {
        "ObjectMapValueType" | "RelationshipMapValueType" => {
            let Some(Value::Object(ty)) = value.get("type") else {
                return None;
            };
            if ty.get("$class").and_then(Value::as_str)
                != Some("concerto.metamodel@1.0.0.TypeIdentifier")
            {
                return None;
            }
            ty.get("name")?.as_str()?.to_string()
        }
        "BooleanMapValueType" => "Boolean".to_string(),
        "DateTimeMapValueType" => "DateTime".to_string(),
        "StringMapValueType" => "String".to_string(),
        "IntegerMapValueType" => "Integer".to_string(),
        "LongMapValueType" => "Long".to_string(),
        "DoubleMapValueType" => "Double".to_string(),
        _ => return None,
    };
    let side = |node: &Value, type_name: String| -> Option<Value> {
        let mut entry = serde_json::Map::new();
        entry.insert("t".to_string(), json!(type_name));
        // A present `decorators` must be one the snapshot can decide.
        match node.get("decorators") {
            None | Some(Value::Null) => {}
            decorators => {
                entry.insert("dec".to_string(), decorators_view_snapshot(decorators)?);
            }
        }
        Some(Value::Object(entry))
    };
    Some(json!({ "k": side(key, key_type)?, "v": side(value, value_type)? }))
}

/// A model AST, as far as [`model_file_view_snapshot`] reads it.
#[derive(serde::Deserialize)]
struct ViewModel {
    declarations: Option<Vec<ViewDeclaration>>,
}

/// A declaration, as far as [`model_file_view_snapshot`] reads it. A `null`
/// value reads as absent; both are falsy to the TS tests this mirrors.
#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct ViewDeclaration {
    #[serde(rename = "$class")]
    class: Option<Value>,
    name: Option<Value>,
    superType: Option<Value>,
    identified: Option<Value>,
    properties: Option<Vec<LightProperty>>,
    // P5-10b: the declaration's decorators, a scalar's validators and
    // default value, and a map's key and value types.
    decorators: Option<Value>,
    validator: Option<Value>,
    lengthValidator: Option<Value>,
    defaultValue: Option<Value>,
    key: Option<Value>,
    value: Option<Value>,
}

/// JavaScript truthiness of a JSON value (`None` is `undefined`).
fn json_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// The metamodel declaration classes `ModelFile.fromAst` gives a default
/// super type when their AST names none, with that super type's name.
const DEFAULT_SUPER_TYPES: [(&str, &str); 4] = [
    ("concerto.metamodel@1.0.0.AssetDeclaration", "Asset"),
    (
        "concerto.metamodel@1.0.0.TransactionDeclaration",
        "Transaction",
    ),
    ("concerto.metamodel@1.0.0.EventDeclaration", "Event"),
    (
        "concerto.metamodel@1.0.0.ParticipantDeclaration",
        "Participant",
    ),
];

/// One declaration's `d` entry of [`model_file_view_snapshot`], or `None`.
fn declaration_view_entry(declaration: &ViewDeclaration, namespace: &str) -> Option<Value> {
    // `Declaration.process`: `isValidIdentifier(this.ast.name)` (an invalid
    // name throws there, so it gets no entry), then `this.fqn`, from a
    // truthy namespace (a falsy one returns the name itself: left to the
    // binding).
    let Some(Value::String(name)) = &declaration.name else {
        return None;
    };
    if namespace.is_empty() || !mu::is_valid_identifier(name) {
        return None;
    }
    let fqn = mu::qualify(namespace, name);

    // `ModelFile.fromAst`'s default super type for four declaration kinds.
    let class = declaration.class.as_ref().and_then(Value::as_str);
    let defaulted_to = if json_truthy(declaration.superType.as_ref()) {
        None
    } else {
        DEFAULT_SUPER_TYPES
            .iter()
            .find(|(c, _)| Some(*c) == class)
            .map(|(_, t)| *t)
    };
    let cd = class_declaration_view_decision(declaration, name, &fqn, defaulted_to);
    Some(json!({
        "name": name,
        "fqn": fqn,
        "cd": cd,
        "defaulted": defaulted_to.is_some(),
    }))
}

/// The [`class_declaration_process`] snapshot for a declaration read from
/// JSON, or `Value::Null` when it cannot be decided here exactly as the
/// binding decides it from the view.
fn class_declaration_view_decision(
    declaration: &ViewDeclaration,
    name: &str,
    fqn: &str,
    defaulted_to: Option<&str>,
) -> Value {
    // `this.ast.superType`: truthy, then its raw `.name`.
    let super_type: Option<String> = if let Some(t) = defaulted_to {
        Some(t.to_string())
    } else if json_truthy(declaration.superType.as_ref()) {
        match declaration.superType.as_ref() {
            Some(Value::Object(node)) => match node.get("name") {
                Some(Value::String(s)) => Some(s.clone()),
                _ => return Value::Null,
            },
            _ => return Value::Null,
        }
    } else {
        None
    };
    // A super-type-less `Concept`: the decision reads
    // `this.modelFile.isSystemModelFile()`, which only the view knows.
    if super_type.is_none() && name == "Concept" {
        return Value::Null;
    }
    // `this.ast.identified`: truthy, then its `$class` (strict equality) and,
    // for `IdentifiedBy`, its raw `.name`.
    let (identified_class, identified_name) = if json_truthy(declaration.identified.as_ref()) {
        match declaration.identified.as_ref() {
            Some(Value::Object(node)) => {
                let class = node
                    .get("$class")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if class == "concerto.metamodel@1.0.0.IdentifiedBy" {
                    match node.get("name") {
                        Some(Value::String(s)) => (Some(class), Some(s.clone())),
                        _ => return Value::Null,
                    }
                } else {
                    (Some(class), None)
                }
            }
            _ => return Value::Null,
        }
    } else {
        (None, None)
    };
    let decision = concerto_core::ClassDeclaration::process_decision(
        super_type.as_deref(),
        false,
        name,
        identified_class.as_deref(),
        identified_name.as_deref(),
        fqn,
    );
    json!({
        "superType": decision.super_type,
        "idField": decision.id_field,
        "addIdentifierField": decision.add_identifier_field,
        "addTimestampField": decision.add_timestamp_field,
    })
}

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
        Ok(to_js(&property_snapshot(&processed)))
    };
    body().map_err(|e| {
        let model_file =
            call(&view, "getModelFile", &[], "this.getModelFile").unwrap_or(JsValue::UNDEFINED);
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
        // TS: `if(this.type)` — a JS truthiness check (an empty-string type
        // is falsy and skips resolution), not a nullish check.
        if property_type.is_truthy() {
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
            // TS: `if(this.type && !this.isPrimitive())` — same truthiness
            // check as above.
            if property_type.is_truthy() {
                let is_primitive =
                    call(&property, "isPrimitive", &[], "this.isPrimitive")?.is_truthy();
                if !is_primitive
                    && let Ok(resolved) = call(
                        &model_file,
                        "getType",
                        std::slice::from_ref(&property_type),
                        "modelFile.getType",
                    )
                    && let Some(v) = call_optional(&resolved, "isMapDeclaration")?
                {
                    is_map_type = v.is_truthy();
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
        Ok(to_js(&field_snapshot(&processed)))
    };
    body().map_err(|e| {
        let model_file =
            call(&view, "getModelFile", &[], "this.getModelFile").unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

/// TS: `Field.getScalarField`, after the `this.scalarField` cache check
/// (still done by the view, since the cached instance stays a JS object) —
/// the P2-09 partial audit found this still TS although the ledger says
/// RUST (#154). `ModelFile` and `ModelManager` are not yet Rust-backed
/// (P2-08), so `isTypeScalar()`'s own collaborator calls
/// (`modelFile.resolveType`, `modelFile.getType`) are reached the same way
/// `propertyValidate` reaches them: only the branching around them, and the
/// scalar-to-property `$class` mapping (`field::scalar_to_field_ast`), run
/// in Rust. Returns the synthetic field's AST; the view still builds the
/// `Field` instance from it and sets `array` from `this.isArray()`, exactly
/// as the TS body's `new Field(this.getParent(), fieldAst)` and
/// `this.scalarField.array = this.isArray()` do.
#[wasm_bindgen(js_name = fieldGetScalarField)]
pub fn field_get_scalar_field(view: JsValue) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        // `isTypeScalar()`.
        let is_primitive = call(&view, "isPrimitive", &[], "this.isPrimitive")?.is_truthy();
        let resolved_type = if is_primitive {
            None
        } else {
            let parent = call(&view, "getParent", &[], "this.getParent")?;
            let model_file = call(&parent, "getModelFile", &[], "parent.getModelFile")?;
            let fqn = js_string(&call(
                &view,
                "getFullyQualifiedName",
                &[],
                "this.getFullyQualifiedName",
            )?)?;
            let property_type = call(&view, "getType", &[], "this.getType")?;
            call(
                &model_file,
                "resolveType",
                &[
                    JsValue::from_str(&format!("property {fqn}")),
                    property_type.clone(),
                ],
                "modelFile.resolveType",
            )?;
            Some(call(
                &model_file,
                "getType",
                &[property_type],
                "modelFile.getType",
            )?)
        };
        // `type.isScalarDeclaration?.()`: `undefined` (no such method) reads
        // as falsy, same as `call_optional`'s `None`; `resolved_type` is
        // `None` when `isPrimitive()` was true, which is also not scalar.
        let not_scalar = |view: &JsValue| -> Result<Error> {
            let name = js_string(&get(view, "name")?)?;
            Ok(plain_error(
                "field-getscalarfield-notscalar",
                vec![("name", name)],
            ))
        };
        let Some(resolved) = resolved_type else {
            return Err(not_scalar(&view)?);
        };
        let is_type_scalar =
            call_optional(&resolved, "isScalarDeclaration")?.is_some_and(|v| v.is_truthy());
        if !is_type_scalar {
            return Err(not_scalar(&view)?);
        }
        let scalar_ast = to_json(&get(&resolved, "ast")?)?.unwrap_or(Value::Null);
        let field_name = to_json(&get(&get(&view, "ast")?, "name")?)?.unwrap_or(Value::Null);
        let field_ast = field::scalar_to_field_ast::<Error>(&scalar_ast, field_name)?;
        Ok(to_js(&field_ast))
    })
}

/// TS: `Field.toString`. `name` and `array`/`optional` are read straight off
/// `this` (plain properties, as `propertyProcess`'s own snapshot sets them);
/// `getFullyQualifiedTypeName()` is called through the view since it is a
/// method, and, for a scalar field, already resolves to the scalar's own FQN
/// (P4-07's issue #195 supplement fixtures cover this — the type name is
/// never the underlying primitive).
#[wasm_bindgen(js_name = fieldToString)]
pub fn field_to_string(view: JsValue) -> std::result::Result<String, JsValue> {
    run(|| {
        let name = js_string(&get(&view, "name")?)?;
        let fully_qualified_type_name = js_string(&call(
            &view,
            "getFullyQualifiedTypeName",
            &[],
            "this.getFullyQualifiedTypeName",
        )?)?;
        let array = get(&view, "array")?.is_truthy();
        let optional = get(&view, "optional")?.is_truthy();
        Ok(field::to_string(
            &name,
            &fully_qualified_type_name,
            array,
            optional,
        ))
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

        // TS: `if(!this.getType())` — a JS truthiness check, so an
        // empty-string type (falsy) must hit this branch too, not just
        // null/undefined.
        if !property_type.is_truthy() {
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
                std::slice::from_ref(&property_type),
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
        ) && let Ok(resolved) = call(
            &model_manager,
            "getType",
            &[JsValue::from_str(&fqtn)],
            "modelManager.getType",
        ) && !nullish(&resolved)
        {
            class_declaration = Some(resolved);
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
/// - `superType`: `this.ast.superType.name` when the AST names one — including
///   the literal text `"undefined"` when a `superType` node is present but
///   carries no `name` at all (`this.ast.superType.name` reads as `undefined`
///   there, not `null`, and every downstream guard treats those two
///   differently: only an explicit `name: null` reads as "no super type",
///   review finding 2 on accordproject/concerto-rust#217); otherwise `null`
///   only for the system model's own `Concept` declaration, else the implicit
///   `'Concept'` (TS: the `this.modelFile.isSystemModelFile() && this.name
///   === 'Concept'` exemption).
/// - `idField`/`addIdentifierField`: mirrors the `this.ast.identified` match;
///   `addIdentifierField` tells the view to still call its own
///   `addIdentifierField()` (it pushes a real `Field` view).
/// - `addTimestampField`: `this.fqn` is the system `Transaction` or `Event`.
#[wasm_bindgen(js_name = classDeclarationProcess)]
pub fn class_declaration_process(declaration: JsValue) -> std::result::Result<JsValue, JsValue> {
    let body = || -> Result<JsValue> {
        let ast = get(&declaration, "ast")?;

        // TS: `if (this.ast.superType) { this.superType = this.ast.superType.name; }
        // else if (!(isSystemModelFile && name === 'Concept')) { this.superType = 'Concept'; }`
        // Neither branch ever calls `.toString()`: the outer test is plain JS
        // truthiness of the whole `superType` node (not merely non-nullish —
        // a fuzzed AST can put `false`/`0`/`""` there too, all falsy), and
        // once truthy, whatever `.name` holds (string, number, boolean,
        // `null`, absent, object, array) is stored on `this.superType`
        // as-is, UNSTRINGIFIED and uncoerced. That raw JS value's own type
        // and truthiness are themselves observable later: `_resolveSuperType`
        // (`classDeclarationResolveSuperType` above) keys off its truthiness,
        // `getProperty`/`getProperties` (below) off strict non-null, and
        // every "Could not find super type" message off its `ToString` —
        // three different tests a fuzzer can pull apart (`undefined` is
        // falsy but not `null`; `ToString(undefined)` is `"undefined"`, not
        // `""`). A single Rust `String` cannot answer the first two at once
        // (accordproject/concerto-rust#219, P5-05 stage-2 T2c), so the raw
        // `JsValue` is threaded straight through to the snapshot below
        // instead of being coerced or blanked here the way `receiver` would.
        let super_type_ast = get(&ast, "superType")?;
        let raw_super_type = if super_type_ast.is_truthy() {
            Some(get(&super_type_ast, "name")?)
        } else {
            None
        };
        // `process_decision` only needs to know whether the AST named a
        // super type at all (`None` applies its own implicit-`Concept`
        // default, or leaves it unset for the system model's own `Concept`);
        // once it has named one, the placeholder's content is never read —
        // `raw_super_type` is what actually reaches the snapshot.
        let explicit_super_type = raw_super_type.as_ref().map(|_| String::new());
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
        // TS: `name === 'Concept'` — a strict equality, not a `.toString()`
        // call, so a non-string `this.name` (reachable through a fuzzed
        // `ast.name`, missing, `null`, a bool, an array, ...) simply can
        // never equal the literal `'Concept'`, and TS accepts every one of
        // those without throwing (accordproject/concerto-rust#217); the
        // empty string can't equal `'Concept'` either, so it stands in
        // without a `receiver`/`js_string` call that could itself throw
        // (or, for a value like `["Concept"]` whose `toString()` happens to
        // read `"Concept"`, wrongly coerce a non-match into a match that
        // real `===` never would).
        let name = get(&declaration, "name")?.as_string().unwrap_or_default();

        // TS: `if (this.ast.identified) { ... }` — again plain truthiness of
        // the whole node, not merely non-nullish.
        let identified = get(&ast, "identified")?;
        let (identified_class, identified_name, raw_identified_name) = if identified.is_truthy() {
            // TS: `this.ast.identified.$class === '...IdentifiedBy'` (strict
            // equality) and `this.idField = this.ast.identified.name` (plain
            // assignment) — neither coerces. A non-string `$class` can never
            // match the literal comparison, so the empty string (never a
            // real `$class`) stands in for it without a receiver check.
            let class_value = get(&identified, "$class")?;
            let identified_class = class_value.as_string().unwrap_or_default();
            // `raw_identified_name` is `this.ast.identified.name`, UNSTRINGIFIED
            // and uncoerced, exactly as TS's plain assignment leaves it — a
            // fuzzed AST can put a number, boolean, `null`, or leave it
            // absent (`undefined`), and every one of those is falsy in TS,
            // so `idField`'s later truthiness guard (in
            // `ClassDeclaration.validate`, still TS) skips its
            // `getProperty(this.idField)` check entirely rather than
            // looking up a property literally named `"undefined"`/`"null"`/
            // `"false"` the way stringifying here would produce
            // (accordproject/concerto-rust#219 review: "Match TS name
            // handling: keep undefined, not the string \"undefined\"").
            // `identified_name` (a `&str`, for `process_decision` below) is
            // only ever consulted on this same branch, and only to decide
            // `process_decision`'s own placeholder `id_field` — which
            // `id_field_js` below always overrides with the raw value once
            // this branch is taken — so it need not itself be coerced.
            let raw_identified_name = if identified_class == "concerto.metamodel@1.0.0.IdentifiedBy"
            {
                Some(get(&identified, "name")?)
            } else {
                None
            };
            let identified_name = raw_identified_name.as_ref().map(|_| String::new());
            (Some(identified_class), identified_name, raw_identified_name)
        } else {
            (None, None, None)
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

        // `raw_super_type` (the AST's own `.name` value, untouched) when the
        // AST named a super type at all; otherwise `process_decision`'s own
        // string decision (the implicit `'Concept'`, or `null` for the
        // system model's own `Concept`).
        let super_type_js = match raw_super_type {
            Some(v) => v,
            None => decision
                .super_type
                .as_deref()
                .map_or(JsValue::NULL, JsValue::from_str),
        };
        // `raw_identified_name` (the AST's own `.identified.name` value,
        // untouched) when the AST named an explicit `IdentifiedBy`;
        // otherwise `process_decision`'s own string decision (`$identifier`
        // for the system-identified case, or `null` for no identity at
        // all).
        let id_field_js = match raw_identified_name {
            Some(v) => v,
            None => decision
                .id_field
                .as_deref()
                .map_or(JsValue::NULL, JsValue::from_str),
        };

        let result = Object::new();
        set(&result, "superType", &super_type_js);
        set(&result, "idField", &id_field_js);
        set(
            &result,
            "addIdentifierField",
            &JsValue::from_bool(decision.add_identifier_field),
        );
        set(
            &result,
            "addTimestampField",
            &JsValue::from_bool(decision.add_timestamp_field),
        );
        Ok(result.into())
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

/// BC-11 (R1): the `IllegalModelException` for a cyclic inheritance chain
/// met by a walk over the JS declaration views, the same error the engine's
/// own walk raises (concerto-core `ModelManager::class_info_of`). `cycle` is
/// the loop from the declaration met again round to the one whose super type
/// it is; `repeated` is that declaration. TS 5.0.0 overflowed V8's stack or
/// ran out of memory instead (DV-013).
fn circular_inheritance_error(cycle: &[JsValue], repeated: &JsValue) -> Result<Error> {
    let fqn = |declaration: &JsValue| -> Result<String> { js_string(&get(declaration, "fqn")?) };
    let mut names = cycle.iter().map(fqn).collect::<Result<Vec<_>>>()?;
    let name = fqn(repeated)?;
    names.push(name.clone());
    let mut err = ContractError::new(
        ErrorKind::IllegalModel,
        "classdeclaration-circularinheritance",
        vec![("type", name), ("cycle", names.join(" -> "))],
    );
    err.model_file = Some(None);
    Ok(err.into())
}

thread_local! {
    /// The declarations whose `getProperties`, `getProperty` or
    /// `getIdentifierFieldName` binding is running, with the binding's name,
    /// outermost first: each recurses into its super type through JS, so a
    /// declaration met again by the same binding is a cyclic inheritance
    /// chain (BC-11).
    static SUPER_WALKS: RefCell<Vec<(&'static str, JsValue)>> = const { RefCell::new(Vec::new()) };
}

/// A running step of a super type walk, removed from [`SUPER_WALKS`] when
/// dropped.
struct SuperWalk;

impl SuperWalk {
    /// Records `declaration` as walked by the binding `properties` names, or
    /// returns the BC-11 error when that binding is already walking it
    /// further out (a cyclic chain).
    fn enter(properties: &'static str, declaration: &JsValue) -> Result<Self> {
        let cycle = SUPER_WALKS.with(|walks| {
            let walks = walks.borrow();
            walks
                .iter()
                .position(|(kind, seen)| *kind == properties && Object::is(seen, declaration))
                .map(|start| {
                    walks
                        .iter()
                        .skip(start)
                        .filter(|(kind, _)| *kind == properties)
                        .map(|(_, seen)| seen.clone())
                        .collect::<Vec<_>>()
                })
        });
        if let Some(cycle) = cycle {
            return Err(circular_inheritance_error(&cycle, declaration)?);
        }
        SUPER_WALKS.with(|walks| walks.borrow_mut().push((properties, declaration.clone())));
        Ok(SuperWalk)
    }
}

impl Drop for SuperWalk {
    fn drop(&mut self) {
        SUPER_WALKS.with(|walks| {
            walks.borrow_mut().pop();
        });
    }
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

/// The system declaration kinds a user model may reuse the name of when
/// `dangerouslyAllowReservedSystemTypeNamesInUserModels` is set: TS
/// `Declaration.isReservedSystemTypeImport`'s `isConcept() || isAsset() ||
/// isTransaction() || isParticipant() || isEvent()` (declaration.ts), in
/// that order.
const RESERVED_SYSTEM_TYPE_KINDS: [&str; 5] = [
    "isConcept",
    "isAsset",
    "isTransaction",
    "isParticipant",
    "isEvent",
];

/// TS: `Declaration.validate`'s own check (P5-11,
/// accordproject/concerto-rust#287), after `super.validate()` (the view's
/// `Decorated.validate`): a declaration may not take the name of a type its
/// model file imports (#648), unless the model manager's
/// `dangerouslyAllowReservedSystemTypeNamesInUserModels` option is set and
/// `this.isReservedSystemTypeImport(modelFile, name)` says the name
/// resolves to a reserved system type. The same rule as concerto-core's
/// `check_import_clash` (validation.rs), run over the view's collaborators,
/// since a direct call may be on a view of a stubbed model file. Throws
/// the `IllegalModelException` TS throws, naming `this.modelFile` and at
/// `this.ast.location`; a collaborator's own error propagates unchanged.
#[wasm_bindgen(js_name = declarationValidate)]
pub fn declaration_validate(declaration: JsValue) -> std::result::Result<(), JsValue> {
    let body = || -> Result<()> {
        let model_file = call(&declaration, "getModelFile", &[], "this.getModelFile")?;
        let name = call(&declaration, "getName", &[], "this.getName")?;
        let imported = call(
            &model_file,
            "isImportedType",
            std::slice::from_ref(&name),
            "modelFile.isImportedType",
        )?;
        if !imported.is_truthy() {
            return Ok(());
        }
        // `Boolean(modelFile.getModelManager()?.options?.dangerously…)`.
        let manager = call(
            &model_file,
            "getModelManager",
            &[],
            "modelFile.getModelManager",
        )?;
        let allow = !nullish(&manager) && {
            let options = get(&manager, "options")?;
            !nullish(&options)
                && get(
                    &options,
                    "dangerouslyAllowReservedSystemTypeNamesInUserModels",
                )?
                .is_truthy()
        };
        if allow {
            let name = call(&declaration, "getName", &[], "this.getName")?;
            let reserved = call(
                &declaration,
                "isReservedSystemTypeImport",
                &[model_file, name],
                "this.isReservedSystemTypeImport",
            )?;
            if reserved.is_truthy() {
                return Ok(());
            }
        }
        let name = js_string(&call(&declaration, "getName", &[], "this.getName")?)?;
        Err(illegal_model_error(
            format!("Type '{name}' clashes with an imported type with the same name."),
            ast_location(&declaration)?,
        ))
    };
    body().map_err(|e| {
        let model_file = get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED);
        throw(e, Some(&model_file))
    })
}

/// TS: `Declaration.isReservedSystemTypeImport(modelFile, typeName)`
/// (P5-11, accordproject/concerto-rust#287): whether `typeName` resolves,
/// through `modelFile.getType`, to a declaration of a system model file
/// that is one of the reserved kinds ([`RESERVED_SYSTEM_TYPE_KINDS`]). The
/// same rule as concerto-core's `is_reserved_system_type_import`
/// (validation.rs), run over the view's collaborators; their own errors
/// propagate unchanged.
#[wasm_bindgen(js_name = declarationIsReservedSystemTypeImport)]
pub fn declaration_is_reserved_system_type_import(
    model_file: JsValue,
    type_name: JsValue,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        let imported = call(&model_file, "getType", &[type_name], "modelFile.getType")?;
        if !imported.is_truthy() || imported.is_string() {
            return Ok(false);
        }
        let imported_file = call(&imported, "getModelFile", &[], "importedType.getModelFile")?;
        if !imported_file.is_truthy()
            || !call(
                &imported_file,
                "isSystemModelFile",
                &[],
                "importedModelFile.isSystemModelFile",
            )?
            .is_truthy()
        {
            return Ok(false);
        }
        for kind in RESERVED_SYSTEM_TYPE_KINDS {
            if call(&imported, kind, &[], kind)?.is_truthy() {
                return Ok(true);
            }
        }
        Ok(false)
    })
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
/// non-null result. A declaration met again is a cyclic inheritance chain,
/// the BC-11 `IllegalModelException` (R1; TS 5.0.0 looped until it ran out
/// of memory, DV-013).
#[wasm_bindgen(js_name = classDeclarationGetAllSuperTypeDeclarations)]
pub fn class_declaration_get_all_super_type_declarations(
    declaration: JsValue,
) -> std::result::Result<Array, JsValue> {
    run(|| {
        let results = Array::new();
        let mut chain = vec![declaration.clone()];
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
            if let Some(start) = chain.iter().position(|seen| Object::is(seen, &next)) {
                return Err(circular_inheritance_error(
                    chain.get(start..).unwrap_or_default(),
                    &next,
                )?);
            }
            chain.push(next.clone());
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
/// (and the same host `TypeError` `call` raises for it). A declaration met
/// again is a cyclic inheritance chain (BC-11, [`SuperWalk`]).
#[wasm_bindgen(js_name = classDeclarationGetIdentifierFieldName)]
pub fn class_declaration_get_identifier_field_name(
    declaration: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let _walk = SuperWalk::enter("getIdentifierFieldName", &declaration)?;
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

/// P5-19 (accordproject/concerto-rust#317): TS `ClassDeclaration
/// .getIdentifierFieldName`, including its super type walk, in one binding.
///
/// Each level of the walk does what
/// [`class_declaration_get_identifier_field_name`] does, but runs the
/// `ClassDeclaration` methods the TS body reaches here instead of crossing
/// back into JS: `this.getSuperType()` (through `getSuperTypeDeclaration()`,
/// the field reads of [`class_declaration_get_super_type_declaration`], and
/// `getFullyQualifiedName()`, a read of `fqn`), `this.getModelFile()` (a
/// read of `modelFile`) and, the walk itself, `classDecl
/// .getIdentifierFieldName()`. `_resolveSuperType`, `getLocalType`,
/// `getModelManager` and `getType` are still called, as TS calls them, so
/// every error is raised by the same collaborator as before.
///
/// P5-36 (BC-50, accordproject/concerto-rust#346): the walk always inlines.
/// A `ClassDeclaration` method replaced at runtime (on the object or its
/// prototype) is not called; replacing these methods is not supported. A
/// `ScalarDeclaration` or `MapDeclaration` reached as a super type gives the
/// same `null` its own `getIdentifierFieldName` does (no truthy `idField`
/// or `superType`).
///
/// A super type seen earlier in the walk is a cyclic inheritance chain: the
/// BC-11 `IllegalModelException` (R1; TS 5.0.0 recursed until V8's stack
/// overflowed, DV-013).
///
/// Returns `[answer, cacheable, ...chain]`: `chain` is every declaration the
/// walk read, from `declaration` on, and `cacheable` is false when the walk
/// ended in a call (a nullish super type resolution), so its answer depends
/// on more than the chain's fields (engine/views.ts keeps the answer only
/// when it is true).
#[wasm_bindgen(js_name = classDeclarationGetIdentifierFieldNameWalk)]
pub fn class_declaration_get_identifier_field_name_walk(
    declaration: JsValue,
) -> std::result::Result<Array, JsValue> {
    run(|| {
        let mut cacheable = true;
        let mut chain: Vec<JsValue> = vec![declaration.clone()];
        let mut current = declaration;
        let answer = loop {
            let id_field = get(&current, "idField")?;
            if id_field.is_truthy() {
                break id_field;
            }

            // `const superType = this.getSuperType();`, through
            // `this.getSuperTypeDeclaration()`.
            let super_type_decl = if !get(&current, "superType")?.is_truthy() {
                JsValue::NULL
            } else {
                let cached = get(&current, "superTypeDeclaration")?;
                if cached.is_truthy() {
                    cached
                } else {
                    call(&current, "_resolveSuperType", &[], "this._resolveSuperType")?
                }
            };
            let super_type = if !super_type_decl.is_truthy() {
                JsValue::NULL
            } else {
                // `superTypeDeclaration.getFullyQualifiedName()`
                get(&super_type_decl, "fqn")?
            };
            if !super_type.is_truthy() {
                break JsValue::NULL;
            }

            // `this.getModelFile()`
            let model_file = get(&current, "modelFile")?;
            let mut class_decl = call(
                &model_file,
                "getLocalType",
                std::slice::from_ref(&super_type),
                "this.getModelFile().getLocalType",
            )?;
            if !class_decl.is_truthy() {
                let manager = call(
                    &model_file,
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

            // `return classDecl.getIdentifierFieldName();` -- a nullish
            // `classDecl` raises the same TypeError through `call`. A
            // declaration met again is a cyclic inheritance chain (BC-11).
            if let Some(start) = chain.iter().position(|d| Object::is(d, &class_decl)) {
                return Err(circular_inheritance_error(
                    chain.get(start..).unwrap_or_default(),
                    &class_decl,
                )?);
            }
            if !nullish(&class_decl) {
                chain.push(class_decl.clone());
                current = class_decl;
                continue;
            }
            cacheable = false;
            break call(
                &class_decl,
                "getIdentifierFieldName",
                &[],
                "classDecl.getIdentifierFieldName",
            )?;
        };

        let result = Array::new();
        result.push(&answer);
        result.push(&JsValue::from_bool(cacheable));
        for declaration in &chain {
            result.push(declaration);
        }
        Ok(result)
    })
}

/// TS: `ClassDeclaration.getProperty`: the receiver's own property if it has
/// one, otherwise the super type's answer (through [`resolve_named_type`]).
/// A `null` super type resolution reaches the same unguarded
/// `classDecl.getProperty(name)` call TS makes.
///
/// The guard is `this.superType !== null` — strict, not TS truthiness — so a
/// fuzzer-produced `this.superType` that is merely falsy (`undefined`, `0`,
/// `false`, `""`, from a `superType` AST node whose `name` was itself falsy;
/// [`class_declaration_process`]'s own module doc) still reaches the same
/// resolution TS does, rather than being treated as "no super type" the way
/// `getSuperType`/`_resolveSuperType`'s own, separate, truthiness guard
/// would (accordproject/concerto-rust#219, P5-05 stage-2 T2c).
#[wasm_bindgen(js_name = classDeclarationGetProperty)]
pub fn class_declaration_get_property(
    declaration: JsValue,
    name: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let _walk = SuperWalk::enter("getProperty", &declaration)?;
        let own = call(
            &declaration,
            "getOwnProperty",
            std::slice::from_ref(&name),
            "this.getOwnProperty",
        )?;
        if !nullish(&own) {
            return Ok(own);
        }
        // TS tests `this.superType !== null` (classdeclaration.ts), not
        // truthiness: an empty-string or `undefined` super type still goes
        // on to be resolved (#218).
        let super_type = get(&declaration, "superType")?;
        if super_type.is_null() {
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
///
/// Same `this.superType !== null` guard as `getProperty` above (not
/// truthiness): accordproject/concerto-rust#219.
#[wasm_bindgen(js_name = classDeclarationGetProperties)]
pub fn class_declaration_get_properties(
    declaration: JsValue,
) -> std::result::Result<Array, JsValue> {
    let body = || -> Result<Array> {
        let _walk = SuperWalk::enter("getProperties", &declaration)?;
        let own = call(
            &declaration,
            "getOwnProperties",
            &[],
            "this.getOwnProperties",
        )?;
        let result = Array::new();
        for property in Array::from(&own).iter() {
            result.push(&property);
        }
        // TS tests `this.superType !== null` (classdeclaration.ts), not
        // truthiness: an empty-string or `undefined` super type still goes
        // on to be resolved, fails to, and throws "Could not find super
        // type" (#218).
        let super_type = get(&declaration, "superType")?;
        if super_type.is_null() {
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
        let inherited = call(&class_decl, "getProperties", &[], "classDecl.getProperties")?;
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
            let is_map =
                call_optional(&declaration, "isMapDeclaration")?.is_some_and(|v| v.is_truthy());
            let is_scalar =
                call_optional(&declaration, "isScalarDeclaration")?.is_some_and(|v| v.is_truthy());
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
///
/// A declaration met again below itself is a cyclic inheritance chain: the
/// BC-11 `IllegalModelException` (R1; TS 5.0.0 recursed until V8's stack
/// overflowed, DV-013).
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

        /// `path` is the declarations from `this` down to `declarations`'
        /// super type, with their names.
        fn collect(
            declarations: &[JsValue],
            subclass_map: &std::collections::HashMap<String, Vec<JsValue>>,
            seen: &mut Vec<JsValue>,
            seen_keys: &mut HashSet<String>,
            path: &mut Vec<(String, JsValue)>,
        ) -> Result<()> {
            for decl in declarations {
                let fqn = js_string(&call(
                    decl,
                    "getFullyQualifiedName",
                    &[],
                    "declaration.getFullyQualifiedName",
                )?)?;
                if let Some(start) = path.iter().position(|(name, _)| *name == fqn) {
                    // `path` runs from super type to subclass; the chain
                    // runs the other way, from `decl` up to `decl` again.
                    let cycle = std::iter::once(decl.clone())
                        .chain(path.iter().skip(start + 1).rev().map(|(_, d)| d.clone()))
                        .collect::<Vec<_>>();
                    return Err(circular_inheritance_error(&cycle, decl)?);
                }
                if seen_keys.insert(fqn.clone()) {
                    seen.push(decl.clone());
                }
                if let Some(children) = subclass_map.get(&fqn) {
                    path.push((fqn, decl.clone()));
                    let walked = collect(children, subclass_map, seen, seen_keys, path);
                    path.pop();
                    walked?;
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
            &mut Vec::new(),
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
                let is_enum = call(&property, "isTypeEnum", &[], "result.isTypeEnum")?.is_truthy();
                if is_primitive || is_enum {
                    return Err(plain_error(
                        "classdeclaration-getnestedproperty-primitiveorenum",
                        vec![
                            ("propertyName", (*name).to_string()),
                            ("propertyPath", path.clone()),
                        ],
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
        // TS interpolates the raw `this.ast.name` into a template literal
        // in every one of this function's own messages
        // (`MapDeclaration must contain Key & Value properties
        // ${this.ast.name}`, mapdeclaration.ts), which applies JS `ToString`
        // to whatever value is there — including `undefined` (a missing
        // `name` key stringifies to the literal text `"undefined"`, not an
        // empty string) and `null` (`"null"`), not only a real string
        // (accordproject/concerto-rust#219, P5-05 stage-2 T2c): collapsing
        // both of those to `String::new()` reported `"MapDeclaration must
        // contain Key & Value properties  "` (an empty name) where TS
        // reports `"... properties undefined "`/`"... properties null "`.
        let name = js_string(&opt_get(&ast, "name")?)?;
        // TS: `if (!this.ast.key || !this.ast.value)` — plain JS truthiness
        // of the whole node, not merely "not `undefined`": a fuzz-mutated
        // `key`/`value` of `false`, `0`, `null` or `""` is exactly as falsy
        // as a missing one, and must fail this same check, not reach
        // `is_valid_map_key`/`is_valid_map_value`'s own, differently-worded
        // rejection instead (accordproject/concerto-rust#219, P5-05
        // stage-2 T2c: `key: 0` reached `to_json`'s `is_undefined`-only gate
        // here, which passed it through as `Some(0)`, giving "must contain
        // valid MapKeyType" instead of TS's "must contain Key & Value
        // properties" — the same theme `MapDeclaration::from_json`'s native
        // Rust construction path already fixed, here again for this WASM
        // binding's own, separate check).
        let key_raw = get(&ast, "key")?;
        let value_raw = get(&ast, "value")?;
        let key = to_json(&key_raw)?;
        let value = to_json(&value_raw)?;
        let location = to_json(&get(&ast, "location")?)?;

        if !key_raw.is_truthy() || !value_raw.is_truthy() {
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
        // `modelFile.getType` returns `null` when the type is not found (not
        // a thrown error), and TS's `isValidMapKeyScalar(decl)` optional-
        // chains off that (`decl?.isScalarDeclaration?.()`), so a nullish
        // `decl` here must become `None`, not `Some` of a JS null.
        let decl_opt = if nullish(&decl) { None } else { Some(&decl) };
        let valid = mu::is_valid_map_key_scalar(&JsContext, decl_opt)?;
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
                // TS: `!('type' in ast)`. `ast` is a genuine JS object here
                // (its own `$class` was just read as a real string above),
                // so the `in` operator itself cannot throw on this check; it
                // is exactly "does the key exist", true whenever `type` is
                // present at all — including an explicit `type: null`, which
                // is present, not missing. `get` returns real `undefined`
                // only for a key that is not there at all, so testing that
                // directly (not `nullish`, which also matches a present
                // `null`) is what keeps the two apart
                // (accordproject/concerto-rust#219 stage-2 T2c: `nullish`
                // here wrongly took the "missing type" branch for a present
                // `type: null`, which TS does not).
                let ast_type = get(&ast, "type")?;
                if ast_type.is_undefined() {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "mapvaluetype-process-missingtype",
                        vec![("name", parent_name)],
                    )
                    .into());
                }
                // TS: `!('$class' in ast.type) || !('name' in ast.type)`.
                // Unlike the check above, `ast.type` is NOT guaranteed to be
                // an object here — a fuzzed AST can set it to `null`, a
                // boolean, a number or a string — and the ECMAScript `in`
                // operator throws a `TypeError` when its right-hand side is
                // not an object (an array or a plain object does not throw;
                // it just falls through to the "malformed type" rejection
                // below like any other object missing both keys).
                if !ast_type.is_object() && !ast_type.is_function() {
                    return Err(type_error(
                        "engine-typeerror-inoperator",
                        vec![
                            ("key", "$class".to_string()),
                            ("value", js_string(&ast_type)?),
                        ],
                    ));
                }
                let type_class = get(&ast_type, "$class")?;
                let type_name_field = get(&ast_type, "name")?;
                // TS: `!('$class' in ast.type) || !('name' in ast.type)` —
                // a key-presence check, not a nullish one
                // (accordproject/concerto-rust#219 stage-2 T2c: this used
                // `nullish`, which wrongly took the "malformed type" branch
                // below for a present `type.$class: null`/`type.name: null`,
                // when TS's `in` sees the key, skips this branch, and goes
                // on to the `$class !== 'TypeIdentifier'` check instead —
                // the same "missing key" vs "present but null" distinction
                // this function's own `ast_type.is_undefined()` check above
                // already gets right for the outer `type` key). `get`
                // returns real `undefined` only for a key that is not there
                // at all, exactly like the outer check.
                if type_class.is_undefined() || type_name_field.is_undefined() {
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
///
/// DV-018 (maintainer-accepted, accordproject/concerto-rust#218): a `null`
/// or `undefined` node, where TS's `this.ast.name` (decorator.ts:139) throws
/// a `TypeError`, is an `IllegalModelException` instead
/// ([`decorator::not_an_object`], the error native Rust raises for the same
/// node). `view` is the `Decorator` being processed, optional so that an
/// older caller passing the AST alone still works: when given, its
/// `getParent().getModelFile()` is the model file the exception names, as
/// TS's `Decorator.handleError` passes it.
#[wasm_bindgen(js_name = decoratorProcess)]
pub fn decorator_process(ast: JsValue, view: JsValue) -> std::result::Result<JsValue, JsValue> {
    if ast.is_null() || ast.is_undefined() {
        let mut err = decorator::not_an_object(if ast.is_null() { "null" } else { "undefined" });
        err.model_file = Some(None);
        let model_file = if view.is_undefined() || view.is_null() {
            None
        } else {
            call(&view, "getParent", &[], "this.getParent")
                .and_then(|parent| {
                    call(
                        &parent,
                        "getModelFile",
                        &[],
                        "this.getParent().getModelFile",
                    )
                })
                .ok()
        };
        return Err(throw(err.into(), model_file.as_ref()));
    }
    run(|| {
        let ast_json = to_json(&ast)?.unwrap_or(Value::Null);
        let decorator = Decorator::from_ast(&ast_json);
        let arguments = Array::new();
        for arg in decorator.arguments() {
            arguments.push(&argument_to_js(arg));
        }
        let out = Object::new();
        // TS: `this.name = ast.name` (decorator.ts) — a plain, uncoerced
        // assignment, so a decorator node with no `name` key at all leaves
        // `this.name` genuinely `undefined`, not the empty string. That
        // distinction only shows up later, in `Decorated.validate`'s
        // duplicate-decorator scan (`decoratedFindDuplicateName`, TS:
        // `this.decorators.map(d => d.getName())`) — `js_name` (not
        // `name`) is what preserves it here (accordproject/concerto-rust#219,
        // review: a model-file's own `decorators: "__proto__"` — parsed one
        // UTF-16 code unit at a time into nameless decorators, DV-018 —
        // wrongly reported "Duplicate decorator " instead of TS's own
        // "Duplicate decorator undefined" until this used `name()`'s
        // always-a-string default instead).
        let name_js = decorator
            .js_name()
            .map_or(JsValue::UNDEFINED, JsValue::from_str);
        set(&out, "name", &name_js);
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
        // `DecoratorArgument` is `#[non_exhaustive]`; no other kind exists
        // today.
        _ => JsValue::UNDEFINED,
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

/// TS: `Decorator.validate`, driven through [`JsContext`] since the model
/// graph these views meet is still TS (module doc: "until P4-06 … P4-08").
/// `view` is the Decorator, already processed (`name`/`arguments` set);
/// `model_file` is `this.getParent().getModelFile()`; `context` is
/// `this.getParent().getFullyQualifiedName?.()` — nullish for a model file's
/// own decorator, exactly as TS's optional call leaves it.
///
/// Every exception this function and its helpers raise is built by calling
/// back into `view.handleError` (or, for the try block's own resolution
/// failure, the shim's own `IllegalModelException`): the
/// `IllegalModelException` construction, its "File '...': " decoration and
/// the log call are never reimplemented here, so they cannot drift from
/// TS's. `handleError` rethrows a caught `IllegalModelException` as it is
/// (BC-14, R1; TS 5.0.0 wrapped it again, DV-016). TS's outer `catch` re-reports *every* thrown value —
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
        // `ModelFile.resolveType`'s own `IllegalModelException(message, mf,
        // location)`, built by the shim, so that `handleError` rethrows it
        // as it is (BC-14, R1).
        let location = to_json(&opt_get(&get(view, "ast")?, "location")?)?;
        let err = illegal_model_error(raw, location);
        return Err(Error::Js(throw(err, Some(model_file))));
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
    CoreError::type_not_found(format!("{node:?}")).into()
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
    ContractError::pre_port(ErrorKind::InvalidArgument, reason, None).into()
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
    let mut props = SerializerOptions::default();
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
/// unchanged, and [`WIRE_TAG`]'s `undefined`, `number`, `bigint`, `dayjs`,
/// `map` and `typed` kinds.
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
                let mut out = SerializerOptions::default();
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
            Some("bigint") => {
                let text = map
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| wire_error("a wire bigint without value".to_string()))?;
                Ok(CoreValue::BigInt(text.to_string()))
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

/// P5-06c: a model file from its JSON AST text, through the typed AST read
/// ([`ModelFile::from_json_text`], the only model loader since P5-61): the
/// same file, and the same errors, as parsing the text into a `Value` and
/// loading that. Malformed JSON throws a JS `SyntaxError`, as it always has.
/// (`validateAst` reads the whole AST as a `Value` anyway.)
fn model_file_from_text(
    ast: &str,
    definitions: Option<String>,
    file_name: Option<String>,
) -> Result<ModelFile> {
    Ok(ModelFile::from_json_text(ast, definitions, file_name)
        .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))??)
}

/// The options object a serializer call's `optionsText` decodes to
/// (`JSON.stringify`d by the view, `"null"` for no options).
fn decode_wire_options(text: &str) -> Result<Option<SerializerOptions>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
    match value {
        Value::Null => Ok(None),
        Value::Object(map) => {
            let mut options = SerializerOptions::default();
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
        // The oracle harness's own `bigint` shape (`migration/oracle/lib/
        // codec.js`). The view's `decodeValue` has no `bigint` kind, so it
        // throws `EngineFastPathUnsupported` on this and the caller falls
        // back to the visitor path, as its `encodeValue` already does for
        // a `BigInt` it is handed (`unsupported-value:bigint`). No decoded
        // wire value is a `BigInt`, so this is not reached today.
        CoreValue::BigInt(s) => json!({ WIRE_TAG: "bigint", "value": s }),
    }
}

// P5-16 (accordproject/concerto-rust#310): `serializerFromJson` reads its
// document straight into a [`CoreValue`] ([`parse_wire`]) and writes its
// result straight to JSON text ([`WireOut`]), rather than through an
// intermediate `serde_json::Value` tree in each direction
// ([`decode_wire`]/[`encode_wire`] and [`snapshot`]): building, hashing and
// dropping those trees was about a third of the call. The values and the
// text are the same as the `Value` route's (the tests below check both).

/// Deserializes one wire value (module doc) directly into a [`CoreValue`],
/// as `decode_wire(&serde_json::from_str(text)?)` would. A wire shape the
/// codec does not recognise is not a JSON syntax error: its [`wire_error`]
/// is kept in `error` (the first one only) and the value read as
/// `undefined`, so parsing goes on and a syntax error anywhere in the text
/// still takes precedence, as it does when the whole text is parsed first.
struct WireSeed<'e> {
    error: &'e RefCell<Option<Error>>,
}

impl WireSeed<'_> {
    fn fail(&self, error: Error) -> CoreValue {
        let mut slot = self.error.borrow_mut();
        if slot.is_none() {
            *slot = Some(error);
        }
        CoreValue::Undefined
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for WireSeed<'_> {
    type Value = CoreValue;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<CoreValue, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for WireSeed<'_> {
    type Value = CoreValue;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, b: bool) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Bool(b))
    }

    fn visit_i64<E>(self, n: i64) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Number(n as f64))
    }

    fn visit_u64<E>(self, n: u64) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Number(n as f64))
    }

    fn visit_f64<E>(self, n: f64) -> std::result::Result<CoreValue, E> {
        // `serde_json::Value` holds a non-finite double as `null`.
        Ok(if n.is_finite() {
            CoreValue::Number(n)
        } else {
            CoreValue::Null
        })
    }

    fn visit_str<E>(self, s: &str) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::String(s.to_string()))
    }

    fn visit_string<E>(self, s: String) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::String(s))
    }

    fn visit_unit<E>(self) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Null)
    }

    fn visit_none<E>(self) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Null)
    }

    fn visit_some<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<CoreValue, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<CoreValue, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element_seed(WireSeed { error: self.error })? {
            items.push(item);
        }
        Ok(CoreValue::Array(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut access: A,
    ) -> std::result::Result<CoreValue, A::Error> {
        // serde_json gives no size hint; most documents' objects have a
        // handful of keys, so start with room for 8 rather than regrow.
        let mut map = SerializerOptions::with_capacity_and_hasher(
            access.size_hint().unwrap_or(8),
            Default::default(),
        );
        while let Some(key) = access.next_key::<String>()? {
            let value = access.next_value_seed(WireSeed { error: self.error })?;
            map.insert(key, value);
        }
        let kind = match map.get(WIRE_TAG) {
            Some(CoreValue::String(kind)) => kind.clone(),
            _ => return Ok(CoreValue::Object(map)),
        };
        Ok(match decode_wire_tagged(&kind, map) {
            Ok(value) => value,
            Err(error) => self.fail(error),
        })
    }
}

/// A [`WIRE_TAG`]ged object of kind `kind`, its entries already read into
/// `map`, as the [`CoreValue`] it decodes to: [`decode_wire`]'s tagged arms
/// over already-decoded entries.
fn decode_wire_tagged(kind: &str, mut map: SerializerOptions) -> Result<CoreValue> {
    let number = |map: &SerializerOptions, key: &str| match map.get(key) {
        Some(CoreValue::Number(n)) => Some(*n),
        _ => None,
    };
    match kind {
        "undefined" => Ok(CoreValue::Undefined),
        "number" => match map.get("value") {
            Some(CoreValue::String(text)) => decode_wire_number(text).map(CoreValue::Number),
            _ => Err(wire_error("a wire number without value".to_string())),
        },
        "bigint" => match map.swap_remove("value") {
            Some(CoreValue::String(text)) => Ok(CoreValue::BigInt(text)),
            _ => Err(wire_error("a wire bigint without value".to_string())),
        },
        "map" => {
            let Some(CoreValue::Array(entries)) = map.swap_remove("entries") else {
                return Err(wire_error("a wire map without entries".to_string()));
            };
            let mut decoded = Vec::with_capacity(entries.len());
            for entry in entries {
                let CoreValue::Array(pair) = entry else {
                    return Err(wire_error(
                        "a wire map entry that is not a pair".to_string(),
                    ));
                };
                let mut pair = pair.into_iter();
                let key = pair
                    .next()
                    .ok_or_else(|| wire_error("a wire map entry without a key".to_string()))?;
                let value = pair
                    .next()
                    .ok_or_else(|| wire_error("a wire map entry without a value".to_string()))?;
                decoded.push((key, value));
            }
            Ok(CoreValue::Map(decoded))
        }
        "dayjs" => {
            let valid = matches!(map.get("valid"), Some(CoreValue::Bool(true)));
            if !valid {
                return Ok(CoreValue::DateTime(Dayjs::utc_invalid()));
            }
            let ms = number(&map, "ms")
                .ok_or_else(|| wire_error("a valid wire dayjs without ms".to_string()))?;
            let offset = number(&map, "utcOffset").unwrap_or(0.0);
            let built = Dayjs::utc_from_number(ms);
            let built = if offset == 0.0 {
                built
            } else {
                built.utc_offset_set(&UtcOffset::Number(offset))
            };
            Ok(CoreValue::DateTime(built))
        }
        "typed" => {
            let kind = match map.get("ctor") {
                Some(CoreValue::String(ctor)) if ctor == "Resource" => InstanceKind::Resource,
                Some(CoreValue::String(ctor)) if ctor == "ValidatedResource" => {
                    InstanceKind::ValidatedResource
                }
                Some(CoreValue::String(ctor)) if ctor == "Relationship" => {
                    InstanceKind::Relationship
                }
                other => {
                    let other = match other {
                        Some(CoreValue::String(ctor)) => Some(ctor.as_str()),
                        _ => None,
                    };
                    return Err(wire_error(format!("a typed wire value of class {other:?}")));
                }
            };
            let Some(CoreValue::String(fqn)) = map.swap_remove("fqn") else {
                return Err(wire_error("a typed wire value without fqn".to_string()));
            };
            let Some(CoreValue::Object(props)) = map.swap_remove("fields") else {
                return Err(wire_error("a typed wire value without fields".to_string()));
            };
            Ok(CoreValue::Instance(Box::new(Instance {
                kind,
                class_fqn: fqn,
                props,
                validator_options: concerto_core::instance::ValidateOptions::default(),
            })))
        }
        other => Err(wire_error(format!(
            "a wire value of kind {other} has no engine counterpart"
        ))),
    }
}

/// `decode_wire(&serde_json::from_str(text)?)` in one pass (see
/// [`WireSeed`]): malformed JSON throws a JS `SyntaxError`, and an
/// unrecognised wire shape its [`wire_error`].
fn parse_wire(text: &str) -> Result<CoreValue> {
    use serde::de::DeserializeSeed;
    let error = RefCell::new(None);
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = WireSeed { error: &error }
        .deserialize(&mut deserializer)
        .and_then(|value| deserializer.end().map(|()| value))
        .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
    match error.into_inner() {
        Some(error) => Err(error),
        None => Ok(value),
    }
}

/// A [`CoreValue`] written as [`encode_wire`]'s JSON text, without building
/// the `serde_json::Value` first: `serde_json::to_string(&WireOut(v))` is
/// `serde_json::to_string(&encode_wire(v))`.
///
/// With `INTS` (the compact result, P5-16), a finite number with no
/// fractional part below 2^53 in magnitude is written as an integer (`42`,
/// not `42.0`): the same number to `JSON.parse`, which reads an integer
/// literal faster.
struct WireOut<'a, const INTS: bool = false>(&'a CoreValue);

/// An [`Instance`] written as [`encode_wire_instance`]'s JSON text.
struct WireInstanceOut<'a, const INTS: bool = false>(&'a Instance);

impl<const INTS: bool> serde::Serialize for WireOut<'_, INTS> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self.0 {
            CoreValue::Undefined => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry(WIRE_TAG, "undefined")?;
                map.end()
            }
            CoreValue::Null => s.serialize_unit(),
            CoreValue::Bool(b) => s.serialize_bool(*b),
            CoreValue::Number(n) => {
                let n = *n;
                let special = if n.is_nan() {
                    Some("NaN")
                } else if n.is_infinite() {
                    Some(if n > 0.0 { "Infinity" } else { "-Infinity" })
                } else if n == 0.0 && n.is_sign_negative() {
                    Some("-0")
                } else {
                    None
                };
                match special {
                    Some(text) => {
                        let mut map = s.serialize_map(Some(2))?;
                        map.serialize_entry(WIRE_TAG, "number")?;
                        map.serialize_entry("value", text)?;
                        map.end()
                    }
                    // `n` is finite and not `-0` here, so it is exactly
                    // representable as an `i64` when it is integral and
                    // below 2^53.
                    None if INTS && n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => {
                        s.serialize_i64(n as i64)
                    }
                    None => s.serialize_f64(n),
                }
            }
            CoreValue::String(text) => s.serialize_str(text),
            CoreValue::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&WireOut::<INTS>(item))?;
                }
                seq.end()
            }
            CoreValue::Object(entries) => {
                let mut map = s.serialize_map(Some(entries.len()))?;
                for (key, item) in entries {
                    map.serialize_entry(key, &WireOut::<INTS>(item))?;
                }
                map.end()
            }
            CoreValue::Map(entries) => {
                struct Pair<'a, const I: bool>(&'a CoreValue, &'a CoreValue);
                impl<const I: bool> serde::Serialize for Pair<'_, I> {
                    fn serialize<S: serde::Serializer>(
                        &self,
                        s: S,
                    ) -> std::result::Result<S::Ok, S::Error> {
                        let mut seq = s.serialize_seq(Some(2))?;
                        seq.serialize_element(&WireOut::<I>(self.0))?;
                        seq.serialize_element(&WireOut::<I>(self.1))?;
                        seq.end()
                    }
                }
                struct Entries<'a, const I: bool>(&'a [(CoreValue, CoreValue)]);
                impl<const I: bool> serde::Serialize for Entries<'_, I> {
                    fn serialize<S: serde::Serializer>(
                        &self,
                        s: S,
                    ) -> std::result::Result<S::Ok, S::Error> {
                        let mut seq = s.serialize_seq(Some(self.0.len()))?;
                        for (key, value) in self.0 {
                            seq.serialize_element(&Pair::<I>(key, value))?;
                        }
                        seq.end()
                    }
                }
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry(WIRE_TAG, "map")?;
                map.serialize_entry("entries", &Entries::<INTS>(entries))?;
                map.end()
            }
            CoreValue::DateTime(d) => {
                if !d.is_valid() {
                    let mut map = s.serialize_map(Some(2))?;
                    map.serialize_entry(WIRE_TAG, "dayjs")?;
                    map.serialize_entry("valid", &false)?;
                    return map.end();
                }
                let mut map = s.serialize_map(Some(4))?;
                map.serialize_entry(WIRE_TAG, "dayjs")?;
                map.serialize_entry("valid", &true)?;
                map.serialize_entry("ms", &d.epoch_ms())?;
                map.serialize_entry("utcOffset", &d.utc_offset())?;
                map.end()
            }
            CoreValue::Instance(i) => WireInstanceOut::<INTS>(i).serialize(s),
            CoreValue::BigInt(text) => {
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry(WIRE_TAG, "bigint")?;
                map.serialize_entry("value", text)?;
                map.end()
            }
        }
    }
}

impl<const INTS: bool> serde::Serialize for WireInstanceOut<'_, INTS> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        struct Fields<'a, const I: bool>(&'a SerializerOptions);
        impl<const I: bool> serde::Serialize for Fields<'_, I> {
            fn serialize<S: serde::Serializer>(
                &self,
                s: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                let mut map = s.serialize_map(Some(self.0.len()))?;
                for (key, value) in self.0 {
                    map.serialize_entry(key, &WireOut::<I>(value))?;
                }
                map.end()
            }
        }
        let mut map = s.serialize_map(Some(4))?;
        map.serialize_entry(WIRE_TAG, "typed")?;
        map.serialize_entry("ctor", self.0.kind.ctor())?;
        map.serialize_entry("fqn", &self.0.class_fqn)?;
        map.serialize_entry("fields", &Fields::<INTS>(&self.0.props))?;
        map.end()
    }
}

/// The own properties the view's `materializeTyped` reads by name rather
/// than copying, and which [`CompactInstanceOut`] therefore writes by
/// position instead of in its field object.
const COMPACT_HEADER_KEYS: [&str; 5] = [
    "$namespace",
    "$type",
    "$identifierFieldName",
    "$identifier",
    "$timestamp",
];

/// An [`Instance`] in the compact result shape of `serializerFromJsonCompact`
/// (P5-16): the JSON array
/// `[ctor, fqn, $namespace, $type, $identifierFieldName, $identifier,
/// $timestamp, fields]`, each value in its wire encoding (a missing one as
/// `undefined`), where `fields` is the `"typed"` shape's field object less
/// what the view's `materializeTyped` skips: those five keys, `$class`,
/// and the key named by `$identifierFieldName` when that is a string
/// (the constructor sets it from `$identifier`). Nested instances keep the
/// `"typed"` shape.
struct CompactInstanceOut<'a>(&'a Instance);

impl serde::Serialize for CompactInstanceOut<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        const UNDEFINED: CoreValue = CoreValue::Undefined;
        struct Rest<'a>(&'a SerializerOptions, Option<&'a str>);
        impl serde::Serialize for Rest<'_> {
            fn serialize<S: serde::Serializer>(
                &self,
                s: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                let mut map = s.serialize_map(None)?;
                for (key, value) in self.0 {
                    if COMPACT_HEADER_KEYS.contains(&key.as_str())
                        || key == "$class"
                        || Some(key.as_str()) == self.1
                    {
                        continue;
                    }
                    map.serialize_entry(key, &WireOut::<true>(value))?;
                }
                map.end()
            }
        }
        let props = &self.0.props;
        // The header values, found in one pass over the properties rather
        // than one hashed lookup each.
        let mut header: [&CoreValue; 5] = [&UNDEFINED; 5];
        for (key, value) in props {
            if let Some(slot) = COMPACT_HEADER_KEYS
                .iter()
                .position(|k| k == key)
                .and_then(|i| header.get_mut(i))
            {
                *slot = value;
            }
        }
        let [_, _, identifier_field_name, _, _] = header;
        let identifier_field = match identifier_field_name {
            CoreValue::String(name) => Some(name.as_str()),
            _ => None,
        };
        let mut seq = s.serialize_seq(Some(8))?;
        seq.serialize_element(self.0.kind.ctor())?;
        seq.serialize_element(&self.0.class_fqn)?;
        for value in header {
            seq.serialize_element(&WireOut::<true>(value))?;
        }
        seq.serialize_element(&Rest(props, identifier_field))?;
        seq.end()
    }
}

/// The serializer of the last `serializerFromJson` call and its merged
/// options as `from_json` reads them, keyed by that call's options text
/// (P5-16): a caller normally passes the same merged options on every call,
/// and both depend on nothing else, so a call with the same text reuses
/// them instead of decoding the options and building them again.
type SerializerCacheEntry = (String, Serializer, FromJsonOptions);

thread_local! {
    static FROM_JSON_SERIALIZER: RefCell<Option<SerializerCacheEntry>> =
        const { RefCell::new(None) };
}

impl ModelManagerHandle {
    /// The resource `serializerFromJson` and `serializerFromJsonCompact`
    /// build (module doc above "Serializer fast path"), in one pass each way
    /// ([`parse_wire`]) and with the serializer reused while the options
    /// text is unchanged ([`FROM_JSON_SERIALIZER`], P5-16).
    fn build_from_json(
        &self,
        json_text: &str,
        options_text: &str,
        env: JsValue,
    ) -> Result<Instance> {
        let object = parse_wire(json_text)?;
        let cached = FROM_JSON_SERIALIZER.with(|slot| {
            slot.borrow_mut()
                .take()
                .filter(|(text, _, _)| text == options_text)
        });
        let (text, serializer, prepared) = match cached {
            Some(entry) => entry,
            None => {
                let options = decode_wire_options(options_text)?;
                let serializer = Serializer::new(true, true, options.as_ref())?;
                // `from_json(options)` merges `options` over the
                // serializer's defaults, which were built from the same
                // `options` over the base defaults: merging them again
                // changes nothing, so the defaults are the merged options.
                let prepared = FromJsonOptions::new(&serializer.default_options);
                (options_text.to_string(), serializer, prepared)
            }
        };
        let mut js_env = JsInstanceEnv { env };
        let resource =
            serializer.from_json_prepared(&self.manager, &object, &prepared, &mut js_env);
        FROM_JSON_SERIALIZER.with(|slot| *slot.borrow_mut() = Some((text, serializer, prepared)));
        Ok(resource?)
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
    /// Bumped by every `&mut self` binding (and by [`Self::model_file_filter`]
    /// on its `target`), so a view can cache what it read from this handle
    /// for as long as the epoch is unchanged ([`Self::epoch`], P5-06). Unlike
    /// [`ModelManager::generation`], it never goes back: `updateModelFile`/
    /// `deleteModelFile` replace the whole manager, restarting its
    /// generation count.
    epoch: u64,
    /// Model files loaded by [`Self::stage_model_file`] and not yet
    /// committed or dropped (lazy views: P5-06a, P5-10a).
    staged: StagedModelFiles,
    /// The per-epoch extract result memo (P5-56, T2, F-A2,
    /// [`DcsExtractMemo`]): dropped whenever the epoch moves
    /// ([`Self::bump_epoch`]) and by [`Self::drop_dcs_memo`]. A `RefCell`,
    /// so the extract bindings keep taking `&self` and never move the epoch.
    dcs_memo: std::cell::RefCell<Option<DcsExtractMemo>>,
}

/// The staging slot of a [`ModelManagerHandle`] (lazy views: P5-06a, P5-10a):
/// model files loaded from their AST once, by
/// [`ModelManagerHandle::stage_model_file`], kept until the view registers
/// ([`ModelManagerHandle::commit_staged_model_file`]), validates
/// ([`ModelManagerHandle::model_file_validate_staged`]) or drops them.
/// Staging never changes the manager, so it never moves the epoch.
///
/// Bounded: past [`StagedModelFiles::CAPACITY`] entries the oldest one is
/// evicted. A view whose stage id was evicted gets `undefined` back and
/// falls back to sending the AST again, so eviction only costs time.
///
/// P5-77 (accordproject/concerto-rust#419): each file is kept shared
/// (`Arc`), so a DecoratorManager result staged from a manager that keeps
/// its files (the extract memo, P5-56) or is about to drop them
/// ([`stage_result`]) is staged without a deep copy, and registered as the
/// same shared file ([`ModelManager::add_shared_model_file`]).
#[derive(Default)]
struct StagedModelFiles {
    files: std::collections::BTreeMap<u32, std::sync::Arc<ModelFile>>,
    next: u32,
}

impl ModelManagerHandle {
    /// Moves the epoch on ([`Self::epoch`]): the manager has, or may have,
    /// changed. Also drops the extract result memo (P5-56), which is only
    /// ever valid for the epoch it was built at.
    fn bump_epoch(&mut self) {
        self.epoch += 1;
        *self.dcs_memo.get_mut() = None;
    }
}

impl StagedModelFiles {
    /// The most staged files kept at once.
    const CAPACITY: usize = 256;

    fn insert(&mut self, file: ModelFile) -> u32 {
        self.insert_shared(std::sync::Arc::new(file))
    }

    /// [`Self::insert`] for a model file that may also be held elsewhere
    /// (P5-77): the file is shared, not copied.
    fn insert_shared(&mut self, file: std::sync::Arc<ModelFile>) -> u32 {
        while self.files.len() >= Self::CAPACITY {
            self.files.pop_first();
        }
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        self.files.insert(id, file);
        id
    }
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// A fresh manager with the `concerto@1.0.0` system model loaded.
    #[wasm_bindgen(constructor)]
    pub fn new() -> std::result::Result<ModelManagerHandle, JsValue> {
        run(|| {
            Ok(Self {
                manager: ModelManager::new()?,
                epoch: 0,
                staged: StagedModelFiles::default(),
                dcs_memo: std::cell::RefCell::new(None),
            })
        })
    }

    /// TS `ModelManager`'s `dangerouslyAllowReservedSystemTypeNamesInUserModels`
    /// option (`ModelManager::set_dangerously_allow_reserved_system_type_names_in_user_models`).
    /// Additive: [`Self::new`] takes no options and leaves this `false`, so
    /// every existing caller is unaffected. A view must call this before
    /// [`Self::add_model_with_definitions`] validates a model that relies on
    /// it — otherwise that call's `ModelManager::validate_detached_model_file`
    /// check (P4-08a, accordproject/concerto-rust#173) runs with the option
    /// off, unlike native `add_model(s)`, and rejects a system type name the
    /// caller meant to allow.
    #[wasm_bindgen(js_name = setDangerouslyAllowReservedSystemTypeNamesInUserModels)]
    pub fn set_dangerously_allow_reserved_system_type_names_in_user_models(&mut self, allow: bool) {
        self.bump_epoch();
        self.manager
            .set_dangerously_allow_reserved_system_type_names_in_user_models(allow);
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
        self.bump_epoch();
        run(|| {
            let model_file = model_file_from_text(ast, None, file_name)?;
            let namespace = model_file.namespace().to_string();
            self.manager.add_model_file(model_file)?;
            self.manager
                .model_file_id(&namespace)
                .map(ModelFileId::index)
                .ok_or_else(|| CoreError::type_not_found(namespace.to_string()).into())
        })
    }

    /// Validates every loaded user model; throws the first problem found.
    #[wasm_bindgen(js_name = validateModels)]
    pub fn validate_models(&self) -> std::result::Result<(), JsValue> {
        run(|| Ok(self.manager.validate_models()?))
    }

    /// TS `this.options?.metamodelValidation` (P4-08b): whether a
    /// validating `addModelFile` checks the new file with
    /// [`Self::validate_ast`] first.
    #[wasm_bindgen(js_name = metamodelValidation)]
    pub fn metamodel_validation(&self) -> bool {
        self.manager.metamodel_validation()
    }

    /// Sets the constructor's `options.metamodelValidation` (P4-08b).
    #[wasm_bindgen(js_name = setMetamodelValidation)]
    pub fn set_metamodel_validation(&mut self, metamodel_validation: bool) {
        self.bump_epoch();
        self.manager.set_metamodel_validation(metamodel_validation);
    }

    /// TS `ModelManagerOptions.decoratorValidation`
    /// (`ModelManager::set_decorator_validation`). Additive, on the same
    /// pattern as [`Self::set_dangerously_allow_reserved_system_type_names_in_user_models`]:
    /// [`Self::new`] leaves this at its `Default` (both fields `None`, i.e.
    /// TS's `DEFAULT_DECORATOR_VALIDATION`, the check disabled), so every
    /// existing caller is unaffected until it calls this.
    ///
    /// `options` is a plain JS object shaped like the TS constructor option,
    /// `{missingDecorator?, invalidDecorator?}`; either or both keys may be
    /// omitted. As in TS, only a truthy (non-empty string) value enables the
    /// check for that field — `level_option` reproduces the same
    /// `validationOptions.missingDecorator || ...` truthiness TS uses when it
    /// reads these fields elsewhere.
    #[wasm_bindgen(js_name = setDecoratorValidation)]
    pub fn set_decorator_validation(
        &mut self,
        options: &JsValue,
    ) -> std::result::Result<(), JsValue> {
        self.bump_epoch();
        run(|| {
            let missing_decorator = level_option(options, "missingDecorator")?;
            let invalid_decorator = level_option(options, "invalidDecorator")?;
            self.manager
                .set_decorator_validation(DecoratorValidationOptions {
                    missing_decorator,
                    invalid_decorator,
                });
            Ok(())
        })
    }

    /// TS `BaseModelManager.validateAst(modelFile)` (P4-08b), for a model
    /// file given as its JSON AST text and file name: throws a
    /// `MetamodelException` when the AST does not conform to the metamodel.
    /// A failed check leaves the metamodel registered, as TS does, so it
    /// may bump [`Self::generation`]. Malformed JSON throws a JS
    /// `SyntaxError`.
    #[wasm_bindgen(js_name = validateAst)]
    pub fn validate_ast(
        &mut self,
        ast: &str,
        file_name: Option<String>,
    ) -> std::result::Result<(), JsValue> {
        self.bump_epoch();
        run(|| {
            let value: Value = serde_json::from_str(ast)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
            let model_file = ModelFile::from_owned_json_with_definitions(value, None, file_name)?;
            Ok(self.manager.validate_ast(&model_file)?)
        })
    }

    /// [`Self::validate_ast`] over the AST alone (P5-13,
    /// accordproject/concerto-rust#297): the JSON AST text is checked as it
    /// is ([`concerto_core::ModelManager::validate_ast_value`]), without
    /// first building a model file, which the check never reads and whose
    /// own constructor would reject some malformed ASTs with an
    /// `IllegalModelException` where TS's `validateAst` throws a
    /// `MetamodelException`. The TS caller already holds the `ModelFile`.
    /// Additive; malformed JSON throws a JS `SyntaxError`.
    #[wasm_bindgen(js_name = validateAstValue)]
    pub fn validate_ast_value(&mut self, ast: &str) -> std::result::Result<(), JsValue> {
        self.bump_epoch();
        run(|| {
            let value: Value = serde_json::from_str(ast)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
            Ok(self.manager.validate_ast_value(&value)?)
        })
    }

    /// P5-49 (BC-19 with BC-17 and BC-20, R1): the strict AST shape check
    /// the TS `ModelFile` constructor runs at model load
    /// ([`concerto_core::instance::check_ast_shape`]), over the JSON AST
    /// text. Throws an `IllegalModelException` for an AST that does not have
    /// the metamodel's shape. Reads nothing of this handle and changes
    /// nothing (not its epoch either). Additive; malformed JSON throws a JS
    /// `SyntaxError`.
    #[wasm_bindgen(js_name = checkAstShape)]
    pub fn check_ast_shape(&self, ast: &str) -> std::result::Result<(), JsValue> {
        run(|| {
            let value: Value = serde_json::from_str(ast)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
            Ok(concerto_core::instance::check_ast_shape(&value)?)
        })
    }

    /// The handle's own mutation counter (P5-06): bumped by every binding
    /// that can change this handle, and never reset, so anything a view
    /// read from the handle is still current while the epoch is unchanged.
    /// Additive; a JS number (exact up to 2^53).
    pub fn epoch(&self) -> f64 {
        // Precision loss only past 2^53 mutations.
        #[allow(clippy::cast_precision_loss)]
        let epoch = self.epoch as f64;
        epoch
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

    /// TS: `BaseModelManager.getModelFileByFileName(fileName)` —
    /// `this.getModelFiles().filter(mf => mf.getName() === fileName)[0]`.
    /// The namespace of the first loaded, non-system model file
    /// (registration order; the built-in decorator and root models are
    /// excluded, as `getModelFiles()`'s default argument excludes them)
    /// whose `getName()` equals `file_name`; `undefined` if none does,
    /// including when `file_name` names one of those system files
    /// (P2-11b-U4). The caller looks the namespace up in its own
    /// `this.modelFiles`, the way `getModelFile(namespace)` already does.
    /// An omitted or `undefined` `file_name` matches, as TS
    /// `getName() === undefined` does, the first such file loaded with no
    /// file name (`addCTOModel(text)` with no `fileName`), rather than
    /// failing the argument conversion (accordproject/concerto-rust#262).
    #[wasm_bindgen(js_name = modelManagerGetModelFileByFileName)]
    pub fn model_manager_get_model_file_by_file_name(
        &self,
        file_name: Option<String>,
    ) -> Option<String> {
        self.manager
            .model_file_by_optional_file_name(file_name.as_deref())
            .map(|mf| mf.namespace().to_string())
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
                "fullyQualifiedName": mu::qualify(file.namespace(), found.name()),
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
            let found = self.manager.property_by_id(id).ok_or_else(missing)?;
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
            let resource = self.build_from_json(json_text, options_text, env)?;
            serde_json::to_string(&WireInstanceOut::<false>(&resource))
                .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
        })
    }

    /// [`Self::serializer_from_json`] with the result in the compact shape
    /// (P5-16, accordproject/concerto-rust#310): the same resource, its
    /// top level written as [`CompactInstanceOut`] instead of a `"typed"`
    /// wire value, which the view parses and materialises in about half
    /// the time. Additive: [`Self::serializer_from_json`] is unchanged.
    #[wasm_bindgen(js_name = serializerFromJsonCompact)]
    pub fn serializer_from_json_compact(
        &self,
        json_text: &str,
        options_text: &str,
        env: JsValue,
    ) -> std::result::Result<String, JsValue> {
        run(|| {
            let resource = self.build_from_json(json_text, options_text, env)?;
            serde_json::to_string(&CompactInstanceOut(&resource))
                .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
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

    /// Loads a model from its JSON AST, as [`Self::add_model`] does, but
    /// also keeps `definitions` (the CTO source text, when the caller has
    /// it) exactly as [`ModelManager::add_model_with_definitions`] does —
    /// so a mirrored model file's `getModels()` content matches the TS
    /// `ModelFile.getDefinitions()` it was loaded from (P4-08). Additive:
    /// [`Self::add_model`] is unchanged and still passes `definitions: None`.
    ///
    /// `validate` mirrors TS `BaseModelManager.addModelFile`'s
    /// `!disableValidation`: when true, the new file is checked with
    /// [`ModelManager::validate_detached_model_file`] — against the manager
    /// as it stands, before the file is registered — exactly as the oracle
    /// harness's own `addModelFile`/`addModel` recipe step does
    /// (`tests/oracle/recipe.rs`), which is also how the reference decides
    /// these fixtures. Before this (P4-08a, accordproject/concerto-rust#173),
    /// this binding never validated at all — regardless of `validate` — so a
    /// model with, say, a missing identifier field, a declared type clashing
    /// with an import, or an undeclared referenced type was silently
    /// registered instead of rejected. The unconditional duplicate-namespace
    /// check (`ModelManager::add_model_with_definitions`'s own) still fires
    /// first regardless of `validate`, as TS's does.
    #[wasm_bindgen(js_name = addModelWithDefinitions)]
    pub fn add_model_with_definitions(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
        validate: bool,
    ) -> std::result::Result<u32, JsValue> {
        self.bump_epoch();
        run(|| {
            // P5-06c: the file is built once, through the typed AST path,
            // for both the check and the add; building it is the first
            // thing that can fail either way, so the errors are unchanged.
            let model_file = model_file_from_text(ast, definitions, file_name)?;
            let namespace = model_file.namespace().to_string();
            if validate && self.manager.model_file(&namespace).is_none() {
                // P5-48: validated and registered in one step, without a
                // scratch copy of the manager (the same checks and errors
                // as `validate_detached_model_file` then `add_model_file`).
                return self
                    .manager
                    .validate_and_add_model_file(model_file)
                    .map(ModelFileId::index)
                    .map_err(|(err, _)| err.into());
            }
            self.manager.add_model_file(model_file)?;
            self.manager
                .model_file_id(&namespace)
                .map(ModelFileId::index)
                .ok_or_else(|| CoreError::type_not_found(namespace).into())
        })
    }

    /// Lazy views (P5-06a spike, productionised in P5-10a): loads a model
    /// file from its JSON AST, passed as JSON text, **without registering
    /// it**, and keeps it in this handle's staging slot. Returns its stage
    /// id. This is the one time a lazily viewed `ModelFile`'s AST crosses
    /// into Rust: the same typed-path load ([`model_file_from_text`], P5-06c)
    /// [`Self::add_model_with_definitions`] and
    /// [`Self::model_file_validate_detached`] run, so it throws exactly
    /// what they would throw for this AST. Malformed JSON throws a JS
    /// `SyntaxError`. Does not change the manager or its epoch. Additive.
    #[wasm_bindgen(js_name = stageModelFile)]
    pub fn stage_model_file(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<u32, JsValue> {
        run(|| {
            let file = model_file_from_text(ast, definitions, file_name)?;
            Ok(self.staged.insert(file))
        })
    }

    /// P5-28 (accordproject/concerto-rust#333): [`Self::stage_model_file`]
    /// and the header [`model_file_from_ast_header`] would set, in one
    /// engine call, from one decode of the AST text. Stages the file exactly
    /// as [`Self::stage_model_file`] does (the same load, the same errors),
    /// and returns JSON text `{"id": <stage id>, "header": <header>}`, where
    /// `header` is [`staged_header`]'s reading of the loaded file's
    /// namespace and `imports` node, or `null` when it cannot vouch that
    /// [`model_file_from_ast_header`] would set exactly that (the caller then
    /// runs that binding, as before). Does not change the manager or its
    /// epoch. Additive.
    #[wasm_bindgen(js_name = stageModelFileWithHeader)]
    pub fn stage_model_file_with_header(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<String, JsValue> {
        run(|| {
            let (file, imports) =
                ModelFile::from_json_text_with_imports(ast, definitions, file_name)
                    .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))??;
            let header =
                staged_header_from_parts(file.namespace(), imports.as_ref()).unwrap_or(Value::Null);
            let id = self.staged.insert(file);
            snapshot(&json!({ "id": id, "header": header }))
        })
    }

    /// P5-69 (BC-19-b, R1; accordproject/concerto-rust#408):
    /// [`Self::stage_model_file_with_header`] with BC-19's AST shape check
    /// folded into the same load: one parse of the AST text and one strict
    /// decode ([`ModelFile::from_json_text_checked_with_imports`]), where the
    /// TS `ModelFile` constructor used to call [`Self::check_ast_shape`]
    /// first and then stage the same text. An AST the check rejects throws
    /// that check's `IllegalModelException` (one of its
    /// `modelfile-load-astshape`, `-decoratorsnotarray`, `-supertypename`,
    /// `-namenotstring` or `-nodenotobject` codes) before any part of the
    /// load runs; otherwise the file is staged, or the load's error thrown,
    /// exactly as [`Self::stage_model_file_with_header`] does. Returns the
    /// same JSON text. Does not change the manager or its epoch. Additive.
    #[wasm_bindgen(js_name = stageModelFileChecked)]
    pub fn stage_model_file_checked(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<String, JsValue> {
        run(|| {
            let (file, imports) =
                ModelFile::from_json_text_checked_with_imports(ast, definitions, file_name)
                    .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))??;
            let header =
                staged_header_from_parts(file.namespace(), imports.as_ref()).unwrap_or(Value::Null);
            let id = self.staged.insert(file);
            snapshot(&json!({ "id": id, "header": header }))
        })
    }

    /// P5-73 (accordproject/concerto-rust#414): a precomputed verdict for
    /// the two fixed system models, which the TS `BaseModelManager`
    /// constructor and `clearModelFiles` build a `ModelFile` for on every
    /// call, from the same constant ASTs. When `ast` is exactly the text of
    /// one of them ([`concerto_core::rootmodel::system_model_json_texts`]),
    /// returns the JSON text of the header [`Self::stage_model_file_checked`]
    /// returns for it (`null` when there is none), without loading or
    /// checking it again: that load, with BC-19's shape check, ran once on
    /// first use, and its verdict holds for the same text. Nothing is staged
    /// (the caller never commits a system model file, which this handle
    /// already holds). Any other text, including any other AST of a system
    /// namespace, returns `undefined`, and the caller loads and checks it as
    /// before, so no AST skips the check by this binding. Does not change
    /// the manager or its epoch. Additive.
    #[wasm_bindgen(js_name = systemModelFileHeader)]
    pub fn system_model_file_header(&self, ast: &str) -> Option<String> {
        system_model_header(ast)
    }

    /// P5-06a: registers a staged model file, as
    /// [`Self::add_model_with_definitions`] with `validate: false` would
    /// register the AST it was staged from (the same duplicate-namespace
    /// check, the same errors), without sending or parsing the AST again.
    /// The stage id is consumed. Returns the file's handle, or `undefined`
    /// if the stage id is unknown (evicted, or already consumed); the caller
    /// then falls back to [`Self::add_model_with_definitions`].
    #[wasm_bindgen(js_name = commitStagedModelFile)]
    pub fn commit_staged_model_file(
        &mut self,
        stage: u32,
    ) -> std::result::Result<Option<u32>, JsValue> {
        let Some(file) = self.staged.files.remove(&stage) else {
            return Ok(None);
        };
        self.bump_epoch();
        run(|| {
            let namespace = file.namespace().to_string();
            self.manager.add_shared_model_file(file)?;
            self.manager
                .model_file_id(&namespace)
                .map(|id| Some(ModelFileId::index(id)))
                .ok_or_else(|| CoreError::type_not_found(namespace).into())
        })
    }

    /// P5-34 (I-5, accordproject/concerto-rust#344): TS
    /// `BaseModelManager.addModelFile`'s validation and registration of a
    /// staged model file in one call: [`Self::model_file_validate_staged`]
    /// then [`Self::commit_staged_model_file`]. Returns the file's handle, or
    /// `undefined` if the stage id is unknown (evicted, or already consumed);
    /// the caller then validates and registers the file as before. A
    /// validation error is thrown as [`Self::model_file_validate_staged`]
    /// throws it and leaves the file staged and the manager unchanged; the
    /// stage is consumed only once validation passes, and a registration
    /// error is then thrown as [`Self::commit_staged_model_file`] throws it.
    /// Additive.
    #[wasm_bindgen(js_name = validateAndCommitStagedModelFile)]
    pub fn validate_and_commit_staged_model_file(
        &mut self,
        stage: u32,
    ) -> std::result::Result<Option<u32>, JsValue> {
        let Some(file) = self.staged.files.remove(&stage) else {
            return Ok(None);
        };
        // P5-48 (accordproject/concerto-rust#369): validated and registered
        // in one step (`ModelManager::validate_and_add_model_file`), without
        // a scratch copy of the manager and of the file. A validation error
        // hands the file back, and it stays staged under the same id, as
        // before; the epoch moves once validation has passed, as
        // `commit_staged_model_file` moves it.
        // P5-77: a file staged shared (a DecoratorManager result) is
        // copied here, as it was copied when it was staged before.
        match self
            .manager
            .validate_and_add_model_file(std::sync::Arc::unwrap_or_clone(file))
        {
            Ok(id) => {
                self.bump_epoch();
                Ok(Some(ModelFileId::index(id)))
            }
            Err((err, Some(file))) => {
                self.staged.files.insert(stage, std::sync::Arc::new(*file));
                run(|| Err(err.into()))
            }
            Err((err, None)) => {
                self.bump_epoch();
                run(|| Err(err.into()))
            }
        }
    }

    /// P5-06a: [`Self::model_file_validate_detached`] for a staged model
    /// file, without sending the AST again. Returns `true` once validated;
    /// `false` if the stage id is unknown, and the caller then falls back to
    /// [`Self::model_file_validate_detached`]. Throws the first problem
    /// found, as that binding does. The staged file stays staged.
    #[wasm_bindgen(js_name = modelFileValidateStaged)]
    pub fn model_file_validate_staged(&self, stage: u32) -> std::result::Result<bool, JsValue> {
        let Some(file) = self.staged.files.get(&stage) else {
            return Ok(false);
        };
        run(|| {
            self.manager.validate_detached_model_file(file)?;
            Ok(true)
        })
    }

    /// P5-06a: drops a staged model file that will never be registered
    /// here. An unknown stage id is ignored.
    #[wasm_bindgen(js_name = dropStagedModelFile)]
    pub fn drop_staged_model_file(&mut self, stage: u32) {
        self.staged.files.remove(&stage);
    }

    /// TS `BaseModelManager.resolveType(context, type)` (P4-08): delegates
    /// to [`ModelManager::resolve_type`], which the manager mirrors from
    /// every model the view has mirrored in with [`Self::add_model`]/
    /// [`Self::add_model_with_definitions`].
    #[wasm_bindgen(js_name = resolveType)]
    pub fn resolve_type(
        &self,
        context: &str,
        type_name: &str,
    ) -> std::result::Result<String, JsValue> {
        run(|| Ok(self.manager.resolve_type(context, type_name)?))
    }

    /// TS `BaseModelManager.derivesFrom(fqt1, fqt2)` (P4-08).
    #[wasm_bindgen(js_name = derivesFrom)]
    pub fn derives_from(&self, fqt1: &str, fqt2: &str) -> std::result::Result<bool, JsValue> {
        run(|| Ok(self.manager.derives_from(fqt1, fqt2)?))
    }

    /// TS `BaseModelManager.isAssignableTo(fqn, baseFqn)` (P4-08).
    #[wasm_bindgen(js_name = isAssignableTo)]
    pub fn is_assignable_to_type(&self, fqn: &str, base_fqn: &str) -> bool {
        self.manager.is_type_assignable_to(fqn, base_fqn)
    }

    /// TS `BaseModelManager.getNamespaces()` (P4-08): every registered
    /// model file's namespace, the system models included, in load order --
    /// matching `Object.keys(this.modelFiles)`, since TS inserts the
    /// decorator and root models into `this.modelFiles` in its constructor
    /// exactly as [`ModelManager::new`] mirrors them here.
    #[wasm_bindgen(js_name = getNamespaces)]
    pub fn get_namespaces(&self) -> Vec<String> {
        self.manager
            .model_files()
            .map(|file| file.namespace().to_string())
            .collect()
    }

    /// Mirrors TS `BaseModelManager.updateModelFile` (P4-08): rebuilds the
    /// model file for `ast`'s namespace from its JSON AST (as
    /// [`Self::add_model_with_definitions`] does), replacing whatever was
    /// registered there. `validate` is TS's `!disableValidation`. Returns
    /// the (possibly unchanged) handle of that namespace's model file.
    #[wasm_bindgen(js_name = updateModelFile)]
    pub fn update_model_file(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
        validate: bool,
    ) -> std::result::Result<u32, JsValue> {
        self.bump_epoch();
        run(|| {
            let model_file = model_file_from_text(ast, definitions, file_name)?;
            let namespace = model_file.namespace().to_string();
            self.manager = self.manager.update_model_file(model_file, validate)?;
            self.manager
                .model_file_id(&namespace)
                .map(ModelFileId::index)
                .ok_or_else(|| CoreError::type_not_found(namespace).into())
        })
    }

    /// Mirrors TS `BaseModelManager.deleteModelFile(namespace)` (P4-08).
    #[wasm_bindgen(js_name = deleteModelFile)]
    pub fn delete_model_file(&mut self, namespace: &str) -> std::result::Result<(), JsValue> {
        self.bump_epoch();
        run(|| {
            self.manager = self.manager.delete_model_file(namespace)?;
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // ModelFile (src/introspect/modelfile.ts) — P4-08c
    //
    // A model file is not its own handle type: it already has one, the same
    // `ModelFileId` P1-04's arena gives every loaded file (`modelFileId`,
    // `modelFileIds`, `modelFileSnapshot`, above), and its declarations
    // already cross as the arena's own `DeclId` handles (`declarationIds`,
    // `declarationSnapshot`) — there is nothing new to invent for either.
    // What was missing is the handful of `ModelFile` members
    // `modelFileSnapshot`'s plain `{namespace, version, fileName, ast}` does
    // not already answer: `getVersion` (`version` can be `null`, which the
    // snapshot's string cannot represent), `isSystemModelFile`, `getImports`
    // (the resolved fully-qualified names, built-in import included, not
    // the raw AST `imports` array `modelFileSnapshot` already exposes),
    // `isLocalType`, `filter` and `validate` — all bound below, keyed by the
    // same `u32` handle. `process`/`fromAst` are the constructor's own two
    // calls (modelfile.ts), inseparable in this port
    // (`ModelFile::from_json_with_definitions` runs both in one pass) and
    // reached only when a file is not yet registered in any manager
    // (`modelFileFromAst`, a free function below, since it needs none).
    // -----------------------------------------------------------------------

    /// TS: `ModelFile.getVersion`. `None` (JS `undefined`) for an unversioned
    /// namespace, which no registered file has: every namespace is required
    /// to carry a version (`parse_namespace_version`'s own check,
    /// model_file.rs; before BC-02, P5-50, a system model file's was exempt).
    #[wasm_bindgen(js_name = modelFileGetVersion)]
    pub fn model_file_get_version(
        &self,
        model_file: u32,
    ) -> std::result::Result<Option<String>, JsValue> {
        run(|| {
            let file = self.require_file(model_file)?;
            let version = file.version();
            Ok((!version.is_empty()).then(|| version.to_string()))
        })
    }

    /// TS: `ModelFile.isSystemModelFile`.
    #[wasm_bindgen(js_name = modelFileIsSystemModelFile)]
    pub fn model_file_is_system_model_file(
        &self,
        model_file: u32,
    ) -> std::result::Result<bool, JsValue> {
        run(|| Ok(self.require_file(model_file)?.is_system_namespace()))
    }

    /// TS: `ModelFile.getImports` — the fully-qualified names this file
    /// imports (the built-in system import included for a non-system file),
    /// as `ModelFile::get_imports` already resolves them.
    #[wasm_bindgen(js_name = modelFileGetImports)]
    pub fn model_file_get_imports(&self, model_file: u32) -> std::result::Result<Array, JsValue> {
        run(|| {
            Ok(self
                .require_file(model_file)?
                .imported_type_names()
                .iter()
                .map(|n| JsValue::from_str(n))
                .collect())
        })
    }

    /// TS: `ModelFile.getExternalImports` — `this.importUriMap` directly:
    /// a plain object keyed by each import's fully-qualified name, valued
    /// by its URI, in import order (issue #263: `external_imports` returns
    /// an `IndexMap`, so this iterates and inserts in that same order).
    #[wasm_bindgen(js_name = modelFileGetExternalImports)]
    pub fn model_file_get_external_imports(
        &self,
        model_file: u32,
    ) -> std::result::Result<Object, JsValue> {
        run(|| {
            let file = self.require_file(model_file)?;
            let out = Object::new();
            for (fqn, uri) in file.external_imports() {
                Reflect::set(&out, &JsValue::from_str(&fqn), &JsValue::from_str(&uri))
                    .map_err(Error::Js)?;
            }
            Ok(out)
        })
    }

    /// TS: `ModelFile.isLocalType`.
    #[wasm_bindgen(js_name = modelFileIsLocalType)]
    pub fn model_file_is_local_type(
        &self,
        model_file: u32,
        type_name: &str,
    ) -> std::result::Result<bool, JsValue> {
        run(|| Ok(self.require_file(model_file)?.is_local_type(type_name)))
    }

    /// TS: `ModelFile.getType(type)` (P5-11, accordproject/concerto-rust#287),
    /// answered by name ([`ModelManager::model_file_type_name`]): a
    /// primitive's own name, the fully-qualified name of the declaration the
    /// type resolves to, or `undefined` for TS `null`. The view maps a
    /// fully-qualified name (the only answer with a dot) to its own
    /// declaration view. Additive.
    #[wasm_bindgen(js_name = modelFileGetTypeName)]
    pub fn model_file_get_type_name(
        &self,
        model_file: u32,
        type_name: &str,
    ) -> std::result::Result<Option<String>, JsValue> {
        run(|| {
            self.require_file(model_file)?;
            Ok(self
                .manager
                .model_file_type_name(ModelFileId::from_index(model_file), type_name)?)
        })
    }

    /// TS: `ModelFile.getFullyQualifiedTypeName(type)` (P5-11,
    /// accordproject/concerto-rust#287): `ModelFile::fully_qualified_type_name`,
    /// `undefined` for TS `null`. Additive.
    #[wasm_bindgen(js_name = modelFileGetFullyQualifiedTypeName)]
    pub fn model_file_get_fully_qualified_type_name(
        &self,
        model_file: u32,
        type_name: &str,
    ) -> std::result::Result<Option<String>, JsValue> {
        run(|| {
            Ok(self
                .require_file(model_file)?
                .fully_qualified_type_name(type_name))
        })
    }

    /// TS: `ModelFile.resolveType(context, type, fileLocation)` (P5-11,
    /// accordproject/concerto-rust#287): [`ModelManager::model_file_resolve_type`].
    /// `file_location` is TS's optional `fileLocation`, and `view` the JS
    /// `ModelFile` (`this`), which the undeclared-type
    /// `IllegalModelException` names, as TS's does. Additive.
    #[wasm_bindgen(js_name = modelFileResolveType)]
    pub fn model_file_resolve_type(
        &self,
        model_file: u32,
        context: &str,
        type_name: &str,
        file_location: JsValue,
        view: JsValue,
    ) -> std::result::Result<(), JsValue> {
        let body = || -> Result<()> {
            self.require_file(model_file)?;
            let location = to_json(&file_location)?;
            Ok(self.manager.model_file_resolve_type(
                ModelFileId::from_index(model_file),
                context,
                type_name,
                location,
            )?)
        };
        body().map_err(|e| throw(e, Some(&view)))
    }

    /// TS: `BaseModelManager.getType(qualifiedName)` (P5-11,
    /// accordproject/concerto-rust#287), answered by name
    /// ([`ModelManager::type_declaration_name`]): the fully-qualified name
    /// of the declaration found, or the `TypeNotFoundException` TS throws.
    /// The view maps the name to its own declaration view. Additive.
    #[wasm_bindgen(js_name = getTypeName)]
    pub fn get_type_name(&self, qualified_name: &str) -> std::result::Result<String, JsValue> {
        run(|| Ok(self.manager.type_declaration_name(qualified_name)?))
    }

    /// TS: `BaseModelManager.validateModelFiles()` (P5-11,
    /// accordproject/concerto-rust#287): every model file validated in one
    /// call ([`ModelManager::validate_models_naming_file`]). `model_files`
    /// is the view's `this.modelFiles`: the first problem found is thrown
    /// naming the JS `ModelFile` it was found in, as that file's own
    /// `validate()` does. Additive.
    #[wasm_bindgen(js_name = validateModelFiles)]
    pub fn validate_model_files(&self, model_files: &JsValue) -> std::result::Result<(), JsValue> {
        self.manager
            .validate_models_naming_file()
            .map_err(|(namespace, err)| throw_naming_file(err.into(), model_files, &namespace))
    }

    /// TS: `BaseModelManager._throwAlreadyExists(modelFile)` (P5-11,
    /// accordproject/concerto-rust#287): throws the plain `Error` naming
    /// `namespace`, the incoming file's name (`file_name`) and the name of
    /// the model file already registered under `namespace`
    /// ([`ModelManager::check_namespace_available`]). Returns normally only
    /// when nothing is registered under `namespace`. Additive.
    #[wasm_bindgen(js_name = throwAlreadyExists)]
    pub fn throw_already_exists(
        &self,
        namespace: &str,
        file_name: Option<String>,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            Ok(self
                .manager
                .check_namespace_available(namespace, file_name.as_deref())?)
        })
    }

    /// TS: the apply, validate and rollback part of
    /// `BaseModelManager.updateExternalModels(options, fileDownloader)`
    /// (P5-11, accordproject/concerto-rust#287;
    /// [`ModelManager::update_external_models_naming_file`]); the download
    /// stays in JS. `sources` is JSON text: the downloaded files, in order,
    /// each `{ast, definitions, fileName}`. Each is added, or replaces the
    /// file under its namespace, without validation; then every model file
    /// is validated, and any failure leaves this handle as it was.
    /// `model_files` is the view's model files as they would be once
    /// applied (namespace to JS `ModelFile`): a validation failure is thrown
    /// naming the JS `ModelFile` it was found in. Additive.
    #[wasm_bindgen(js_name = updateExternalModels)]
    pub fn update_external_models(
        &mut self,
        sources: &str,
        model_files: &JsValue,
    ) -> std::result::Result<(), JsValue> {
        self.bump_epoch();
        let parsed = (|| -> Result<Vec<ModelFileSource>> {
            let value: Value = serde_json::from_str(sources)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
            let text = |source: &Value, key: &str| {
                source.get(key).and_then(Value::as_str).map(str::to_string)
            };
            Ok(value
                .as_array()
                .map(|list| {
                    list.iter()
                        .map(|source| ModelFileSource {
                            ast: source.get("ast").cloned().unwrap_or(Value::Null),
                            definitions: text(source, "definitions"),
                            file_name: text(source, "fileName"),
                        })
                        .collect()
                })
                .unwrap_or_default())
        })()
        .map_err(|e| throw(e, None))?;
        self.manager
            .update_external_models_naming_file(parsed)
            .map(|_| ())
            .map_err(|(namespace, err)| match namespace {
                Some(namespace) => throw_naming_file(err.into(), model_files, &namespace),
                None => throw(err.into(), None),
            })
    }

    /// TS: `ModelFile.validate()`, for a model file this manager already
    /// holds under its own namespace — the common case for a view whose
    /// `getModelManager()` is this handle (`ModelManager::validate_model_file`).
    /// Throws the first problem found.
    #[wasm_bindgen(js_name = modelFileValidate)]
    pub fn model_file_validate(&self, model_file: u32) -> std::result::Result<(), JsValue> {
        run(|| {
            let file = self.require_file(model_file)?;
            Ok(self.manager.validate_model_file(file)?)
        })
    }

    /// TS: `ModelFile.validate()` for a `ModelFile` that need not be the one
    /// this manager holds under its namespace — `new ModelFile(modelManager,
    /// ast, …)` followed directly by `validate()`, or the
    /// validate-before-register path a caller like `BaseModelManager.addModelFile`
    /// takes (`ModelManager::validate_detached_model_file`). `ast` is the
    /// model's JSON AST as JSON text (`JSON.stringify(ast)`, `addModel`'s own
    /// convention); `definitions`/`file_name` mirror the `ModelFile`
    /// constructor's own optional arguments.
    #[wasm_bindgen(js_name = modelFileValidateDetached)]
    pub fn model_file_validate_detached(
        &self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            let file = model_file_from_text(ast, definitions, file_name)?;
            Ok(self.manager.validate_detached_model_file(&file)?)
        })
    }

    /// TS: `ModelFile.filter(predicate, modelManager)`, for a model file this
    /// manager (the source) already holds; `target` is the `modelManager`
    /// argument — ordinarily a *different*, otherwise-empty manager, since
    /// `filter`'s own caller (`BaseModelManager.filter`) always builds a
    /// fresh one before filtering into it; TS never adds the result back to
    /// the file's own manager, and neither must this (a caller that means to
    /// keep it in `self` passes `self` as `target` too, but the common case
    /// is another handle). `predicate` is called with each candidate
    /// declaration's fully-qualified name — including a declaration of
    /// *another* file this one imports from, which `filter`'s own import
    /// pruning reaches (module doc on [`concerto_core::ModelFile::filter`])
    /// — the same `keep_fqn` convention `ModelManager::filter`
    /// (`BaseModelManager.filter`) already uses, so the view's own
    /// `Declaration -> bool` predicate is expected to look its argument back
    /// up by fully-qualified name (`declarationId`) the way that binding's
    /// caller must too. A predicate that throws propagates unchanged.
    ///
    /// The filtered model file, if any declaration survived, is added to
    /// `target` exactly as `addModel` would (so its declarations get the
    /// arena's ordinary handles there) and its handle in `target` is
    /// returned; `None` (JS `undefined`) if every declaration was filtered
    /// out, matching TS's `null`.
    #[wasm_bindgen(js_name = modelFileFilter)]
    pub fn model_file_filter(
        &self,
        model_file: u32,
        predicate: Function,
        target: &mut ModelManagerHandle,
    ) -> std::result::Result<Option<u32>, JsValue> {
        target.bump_epoch();
        run(|| {
            let file = self.require_file(model_file)?;
            // `ModelFile::filter`'s predicate carries no namespace of its
            // own (its doc comment): it is called both on `file`'s own
            // declarations *and*, while pruning `file`'s imports, on
            // declarations belonging to a *different* model file
            // (`source_manager.model_file(ns).get_local_type(...)`). Keying
            // the fully-qualified name off `file`'s namespace alone would
            // ask the JS predicate about the wrong FQN for every cross-file
            // (import) declaration, exactly the failure
            // `ModelManager::filter`'s own doc comment warns about. So the
            // real namespace for every declaration reachable from this
            // filter call is looked up by identity up front, across every
            // file `self.manager` holds.
            let fqn_by_decl: std::collections::HashMap<
                *const concerto_core::introspect::Declaration,
                String,
            > = self
                .manager
                .model_files()
                .flat_map(|mf| {
                    let namespace = mf.namespace();
                    mf.declarations().iter().map(move |decl| {
                        (
                            decl as *const concerto_core::introspect::Declaration,
                            mu::qualify(namespace, decl.name()),
                        )
                    })
                })
                .collect();
            let file_namespace = file.namespace().to_string();
            let js_err: RefCell<Option<Error>> = RefCell::new(None);
            let filtered = file.filter(
                |decl| {
                    if js_err.borrow().is_some() {
                        return false;
                    }
                    let fqn = fqn_by_decl
                        .get(&(decl as *const concerto_core::introspect::Declaration))
                        .cloned()
                        .unwrap_or_else(|| mu::qualify(&file_namespace, decl.name()));
                    match predicate.call1(&JsValue::NULL, &JsValue::from_str(&fqn)) {
                        Ok(v) => v.is_truthy(),
                        Err(e) => {
                            *js_err.borrow_mut() = Some(Error::Js(e));
                            false
                        }
                    }
                },
                &self.manager,
            )?;
            if let Some(err) = js_err.into_inner() {
                return Err(err);
            }
            let Some(filtered) = filtered else {
                return Ok(None);
            };
            let ast = filtered.ast().clone();
            let ns = filtered.namespace().to_string();
            let new_file_name = filtered.file_name().map(str::to_string);
            target
                .manager
                .add_model_with_definitions(&ast, None, new_file_name)?;
            target
                .manager
                .model_file_id(&ns)
                .map(ModelFileId::index)
                .map(Some)
                .ok_or_else(|| CoreError::type_not_found(ns.clone()).into())
        })
    }
}

impl ModelManagerHandle {
    /// A model file, by its handle; the same [`unknown`] `TypeNotFound` every
    /// other by-handle lookup here throws for one that names nothing.
    fn require_file(&self, model_file: u32) -> Result<&concerto_core::ModelFile> {
        let id = ModelFileId::from_index(model_file);
        self.manager
            .file(id)
            .ok_or_else(|| unknown(Node::ModelFile(id)))
    }
}

/// The metamodel namespace, TS `MetaModelNamespace` (concerto-metamodel).
const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// TS: `ModelFile.enforceImportVersioning(imp)` (P5-11,
/// accordproject/concerto-rust#287): `ModelUtil.parseNamespace(imp.namespace)`
/// must give a version, or the plain `Error` TS throws is raised; a
/// namespace `parseNamespace` rejects raises its own error first.
fn enforce_import_versioning(imp: &JsValue) -> Result<()> {
    let namespace = get(imp, "namespace")?;
    // BC-02 (R1, P5-50): `parseNamespace` rejects an unversioned namespace
    // itself now; an unversioned import keeps this function's own error.
    let versioned = !is_unversioned_namespace(&namespace)
        && matches!(
            parse_namespace_js(&namespace, false)?,
            mu::ParsedNamespace::Full { version: Some(ref v), .. } if !v.is_empty()
        );
    if versioned {
        return Ok(());
    }
    Err(ContractError::pre_port(
        ErrorKind::InvalidArgument,
        format!(
            "Cannot use an unversioned import {}.",
            js_string(&namespace)?
        ),
        None,
    )
    .into())
}

/// TS: `ModelFile.enforceImportVersioning(imp)` (P5-11,
/// accordproject/concerto-rust#287), [`enforce_import_versioning`]. Additive.
#[wasm_bindgen(js_name = modelFileEnforceImportVersioning)]
pub fn model_file_enforce_import_versioning(imp: JsValue) -> std::result::Result<(), JsValue> {
    run(|| enforce_import_versioning(&imp))
}

/// TS: `ModelFile.isCompatibleVersion()` (P5-11,
/// accordproject/concerto-rust#287), on the JS `ModelFile` `view`: when
/// `view.ast.concertoVersion` is truthy it must be a range this runtime
/// supports ([`concerto_core::introspect::model_file::compatible_concerto_version`]),
/// which is then stored as `view.concertoVersion`; otherwise the plain
/// `Error` TS throws is raised. A truthy non-string is never a range
/// node-semver can parse (`satisfies` and `minSatisfying` both give up on
/// it), so it is always that `Error`. Additive.
#[wasm_bindgen(js_name = modelFileIsCompatibleVersion)]
pub fn model_file_is_compatible_version(view: JsValue) -> std::result::Result<(), JsValue> {
    use concerto_core::introspect::model_file::{
        compatible_concerto_version, incompatible_concerto_version,
    };
    run(|| {
        let range = get(&get(&view, "ast")?, "concertoVersion")?;
        if !range.is_truthy() {
            return Ok(());
        }
        let Some(text) = range.as_string() else {
            return Err(incompatible_concerto_version(&js_string(&range)?).into());
        };
        let accepted = compatible_concerto_version(&text)?;
        set_property(&view, "concertoVersion", &JsValue::from_str(&accepted))
    })
}

/// TS: `ModelFile._fromAstHeader(ast)`, the part of `ModelFile.fromAst`
/// before the declarations (P5-11, accordproject/concerto-rust#287), on the
/// JS `ModelFile` `view`: parses and checks `ast.namespace` (every part a
/// valid identifier, and a version: since BC-02, R1, P5-50, for a system
/// file too, where TS 5.0.0 exempted `view.isSystemModelFile()`),
/// then sets `view.namespace`, `view.version` and `view.imports` (a copy
/// of `ast.imports`, plus the implicit import of the system types for a
/// non-system file), and fills `view.importShortNames` (local name, alias
/// included, to fully-qualified name) and `view.importUriMap` from the
/// imports, rejecting an unversioned import, a wildcard import and an
/// alias to a primitive type (the first through `view.enforceImportVersioning`,
/// as TS calls it). Each error has TS's class, and the JS errors
/// TS's own property reads and calls raise on a malformed AST keep theirs,
/// since this runs over the same JS values in the same order. Additive.
#[wasm_bindgen(js_name = modelFileFromAstHeader)]
pub fn model_file_from_ast_header(view: JsValue, ast: JsValue) -> std::result::Result<(), JsValue> {
    let body = || -> Result<()> {
        let namespace = get(&ast, "namespace")?;
        // BC-02 (R1, P5-50): an unversioned namespace keeps this header's
        // own checks and errors (the identifier check, then the plain
        // `Error` below), rather than `parseNamespace`'s.
        let (name, version) = if is_unversioned_namespace(&namespace) {
            (namespace.as_string().unwrap_or_default(), JsValue::NULL)
        } else {
            match parse_namespace_js(&namespace, false)? {
                mu::ParsedNamespace::Full { name, version, .. } => (
                    name,
                    version.map_or(JsValue::NULL, |v| JsValue::from_str(&v)),
                ),
                mu::ParsedNamespace::NameOnly { name } => (name, JsValue::UNDEFINED),
            }
        };
        for part in name.split('.') {
            if !mu::is_valid_identifier(part) {
                return Err(illegal_model_error(
                    format!("Invalid namespace part '{part}'"),
                    to_json(&get(&get(&view, "ast")?, "location")?)?,
                ));
            }
        }
        set_property(&view, "namespace", &namespace)?;
        set_property(&view, "version", &version)?;
        let is_system = || -> Result<bool> {
            Ok(call(&view, "isSystemModelFile", &[], "this.isSystemModelFile")?.is_truthy())
        };
        // BC-02 (R1, P5-50; DV-003 closed): every model file needs a
        // version; TS 5.0.0 exempted a system one (`isSystemModelFile()`, a
        // bare `concerto` namespace).
        if !version.is_truthy() {
            return Err(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                format!(
                    "Cannot create a ModelFile with an unversioned namespace: {}. All models \
                     must specify a version (e.g., @1.0.0).",
                    js_string(&namespace)?
                ),
                None,
            )
            .into());
        }

        // A copy, since the implicit import is added to it.
        let ast_imports = get(&ast, "imports")?;
        let imports = if ast_imports.is_truthy() {
            call(
                &ast_imports,
                "concat",
                &[Array::new().into()],
                "ast.imports.concat",
            )?
        } else {
            Array::new().into()
        };
        if !is_system()? {
            let implicit = to_js(&json!({
                "$class": format!("{METAMODEL_NAMESPACE}.ImportTypes"),
                "namespace": "concerto@1.0.0",
                "types": ["Concept", "Asset", "Transaction", "Participant", "Event"],
            }));
            call(&imports, "push", &[implicit], "imports.push")?;
        }
        set_property(&view, "imports", &imports)?;

        let short_names = get(&view, "importShortNames")?;
        let uri_map = get(&view, "importUriMap")?;
        let set_short_name = |key: &JsValue, value: &JsValue| -> Result<()> {
            call(
                &short_names,
                "set",
                &[key.clone(), value.clone()],
                "this.importShortNames.set",
            )
            .map(|_| ())
        };
        let import_types = format!("{METAMODEL_NAMESPACE}.ImportTypes");
        let import_all = format!("{METAMODEL_NAMESPACE}.ImportAll");
        for imp in each(&imports, "this.imports.forEach")? {
            // `this.enforceImportVersioning(imp)`, as TS calls it
            // ([`model_file_enforce_import_versioning`]).
            call(
                &view,
                "enforceImportVersioning",
                std::slice::from_ref(&imp),
                "this.enforceImportVersioning",
            )?;
            let class = get(&imp, "$class")?.as_string();
            if class.as_deref() == Some(import_all.as_str()) {
                return Err(ContractError::pre_port(
                    ErrorKind::InvalidArgument,
                    "Wildcard Imports are not permitted.".to_string(),
                    None,
                )
                .into());
            }
            if class.as_deref() == Some(import_types.as_str()) {
                let ns = js_string(&get(&imp, "namespace")?)?;
                let aliased = get(&imp, "aliasedTypes")?;
                let has_aliases = aliased.is_truthy() && js_length(&aliased)?.gt(&JsValue::from(0));
                let aliases = js_sys::Map::new();
                if has_aliases {
                    for entry in each(&aliased, "imp.aliasedTypes.forEach")? {
                        let alias_name = get(&entry, "name")?;
                        let aliased_name = get(&entry, "aliasedName")?;
                        if aliased_name
                            .as_string()
                            .is_some_and(|n| mu::is_primitive_type(&n))
                        {
                            return Err(ContractError::pre_port(
                                ErrorKind::InvalidArgument,
                                "Types cannot be aliased to primitive type".to_string(),
                                None,
                            )
                            .into());
                        }
                        aliases.set(&alias_name, &aliased_name);
                    }
                }
                for type_name in each(&get(&imp, "types")?, "imp.types.forEach")? {
                    let fqn = JsValue::from_str(&format!("{ns}.{}", js_string(&type_name)?));
                    let alias = aliases.get(&type_name);
                    let key = if has_aliases && !nullish(&alias) {
                        alias
                    } else {
                        type_name
                    };
                    set_short_name(&key, &fqn)?;
                }
            } else {
                let first = import_fully_qualified_name(&imp)?;
                set_short_name(&get(&imp, "name")?, &first)?;
            }
            let uri = get(&imp, "uri")?;
            if uri.is_truthy() {
                let first = import_fully_qualified_name(&imp)?;
                Reflect::set(&uri_map, &first, &uri).map_err(Error::Js)?;
            }
        }
        Ok(())
    };
    body().map_err(|e| throw(e, Some(&view)))
}

thread_local! {
    /// P5-73: for each fixed system model text
    /// ([`concerto_core::rootmodel::system_model_json_texts`]), the header
    /// text [`ModelManagerHandle::system_model_file_header`] returns, or
    /// `None` when its checked load failed (never expected; that text is then
    /// loaded and checked every time, as any other). Filled on first use.
    static SYSTEM_MODEL_HEADERS: std::cell::OnceCell<Vec<(&'static str, Option<String>)>> =
        const { std::cell::OnceCell::new() };
}

/// [`ModelManagerHandle::system_model_file_header`]: the header text of the
/// fixed system model whose AST is exactly `ast`, from one checked load of
/// that text ([`ModelFile::from_json_text_checked_with_imports`], what
/// `stageModelFileChecked` runs) on first use. `None` for any other text.
fn system_model_header(ast: &str) -> Option<String> {
    SYSTEM_MODEL_HEADERS.with(|cell| {
        cell.get_or_init(|| {
            concerto_core::rootmodel::system_model_json_texts()
                .into_iter()
                .map(|(file_name, text)| {
                    let header = match ModelFile::from_json_text_checked_with_imports(
                        text,
                        None,
                        Some(file_name.to_string()),
                    ) {
                        Ok(Ok((file, imports))) => serde_json::to_string(
                            &staged_header_from_parts(file.namespace(), imports.as_ref())
                                .unwrap_or(Value::Null),
                        )
                        .ok(),
                        _ => None,
                    };
                    (text, header)
                })
                .collect()
        })
        .iter()
        .find(|(text, _)| *text == ast)
        .and_then(|(_, header)| header.clone())
    })
}

/// P5-28 (accordproject/concerto-rust#333): what [`model_file_from_ast_header`]
/// sets on a JS `ModelFile` being constructed, read from a staged file's
/// namespace and its AST's `imports` node instead of from the JS values, so
/// [`ModelManagerHandle::stage_model_file_with_header`] can return it with
/// the stage. `{namespace, version, system, shortNames, uriMap}`: `version`
/// is the namespace's version or `null` (TS `this.version`), `system`
/// whether `isSystemModelFile()` holds during construction (TS: the
/// namespace is `concerto` or starts with `concerto@`, since the file is not
/// registered yet), `shortNames` the `importShortNames.set(key, fqn)` calls
/// in order, and `uriMap` the `importUriMap[key] = uri` assignments in
/// order. `this.imports` itself (a copy of `ast.imports` plus the implicit
/// import) is left to the caller, which keeps the AST's own import objects.
///
/// `None` whenever that binding would not simply set these values: any
/// error it would raise, and any AST shape outside the canonical one (a
/// non-string `$class`, namespace, name, type or alias, a non-array
/// `types` or `aliasedTypes`, a URI on an import with no first name, an
/// unrecognised import class). The caller then runs that binding over the
/// JS values, as before, so every error and every oddity keeps its path.
fn staged_header_from_parts(namespace: &str, imports: Option<&Value>) -> Option<Value> {
    let version = match mu::parse_namespace_with(Some(namespace), false).ok()? {
        mu::ParsedNamespace::Full { name, version, .. } => {
            if !name.split('.').all(mu::is_valid_identifier) {
                return None;
            }
            version
        }
        mu::ParsedNamespace::NameOnly { .. } => return None,
    };
    let system = namespace.starts_with("concerto@") || namespace == "concerto";
    if version.as_deref().is_none_or(str::is_empty) && !system {
        return None;
    }
    let ast_imports: &[Value] = match imports {
        None | Some(Value::Null) => &[],
        Some(Value::Array(items)) => items,
        Some(_) => return None,
    };
    let implicit = (!system).then(|| {
        json!({
            "$class": format!("{METAMODEL_NAMESPACE}.ImportTypes"),
            "namespace": "concerto@1.0.0",
            "types": ["Concept", "Asset", "Transaction", "Participant", "Event"],
        })
    });
    let import_types = format!("{METAMODEL_NAMESPACE}.ImportTypes");
    let import_type = format!("{METAMODEL_NAMESPACE}.ImportType");
    let mut short_names: Vec<Value> = Vec::new();
    let mut uri_map: Vec<Value> = Vec::new();
    for imp in ast_imports.iter().chain(implicit.as_ref()) {
        let imp = imp.as_object()?;
        let class = imp.get("$class")?.as_str()?;
        let ns = imp.get("namespace")?.as_str()?;
        // `this.enforceImportVersioning(imp)`.
        match mu::parse_namespace_with(Some(ns), false).ok()? {
            mu::ParsedNamespace::Full {
                version: Some(ref v),
                ..
            } if !v.is_empty() => {}
            _ => return None,
        }
        let first = if class == import_types {
            let mut aliases: Vec<(&str, &str)> = Vec::new();
            match imp.get("aliasedTypes") {
                None | Some(Value::Null) => {}
                Some(Value::Array(entries)) => {
                    for entry in entries {
                        let entry = entry.as_object()?;
                        let name = entry.get("name")?.as_str()?;
                        let aliased_name = entry.get("aliasedName")?.as_str()?;
                        if mu::is_primitive_type(aliased_name) {
                            return None;
                        }
                        // `Map.set`: a later entry for the same name wins.
                        match aliases.iter_mut().find(|(n, _)| *n == name) {
                            Some(slot) => slot.1 = aliased_name,
                            None => aliases.push((name, aliased_name)),
                        }
                    }
                }
                Some(_) => return None,
            }
            let types = imp.get("types")?.as_array()?;
            let mut first = None;
            for type_name in types {
                let type_name = type_name.as_str()?;
                let fqn = format!("{ns}.{type_name}");
                let key = aliases
                    .iter()
                    .find(|(n, _)| *n == type_name)
                    .map_or(type_name, |(_, alias)| alias);
                short_names.push(json!([key, fqn]));
                first.get_or_insert(fqn);
            }
            first
        } else if class == import_type {
            let name = imp.get("name")?.as_str()?;
            let fqn = format!("{ns}.{name}");
            short_names.push(json!([name, fqn]));
            Some(fqn)
        } else {
            return None;
        };
        match imp.get("uri") {
            None | Some(Value::Null) => {}
            Some(Value::String(uri)) if uri.is_empty() => {}
            Some(Value::String(uri)) => uri_map.push(json!([first?, uri])),
            Some(Value::Bool(false)) => {}
            Some(_) => return None,
        }
    }
    Some(json!({
        "namespace": namespace,
        "version": version,
        "system": system,
        "shortNames": short_names,
        "uriMap": uri_map,
    }))
}

/// `value.length`: a string primitive's own length (UTF-16 code units),
/// which [`get`] does not read.
fn js_length(value: &JsValue) -> Result<JsValue> {
    match value.as_string() {
        Some(text) => Ok(JsValue::from(text.encode_utf16().count() as f64)),
        None => get(value, "length"),
    }
}

/// `ModelUtil.importFullyQualifiedNames(imp)[0]`: `undefined` when there is
/// none.
fn import_fully_qualified_name(imp: &JsValue) -> Result<JsValue> {
    let names = mu::import_fully_qualified_names(to_json(imp)?.as_ref())?;
    Ok(names
        .first()
        .map_or(JsValue::UNDEFINED, |n| JsValue::from_str(n)))
}

/// The elements `value.forEach` visits, for a JS array; `expression` names
/// the callee in the `TypeError` any other value raises (a nullish one
/// fails reading `forEach` itself, as in TS).
fn each(value: &JsValue, expression: &str) -> Result<Vec<JsValue>> {
    if Array::is_array(value) {
        return Ok(Array::from(value).iter().collect());
    }
    get(value, "forEach")?;
    Err(type_error(
        "engine-typeerror-notafunction",
        vec![("expression", expression.to_string())],
    ))
}

/// TS: `new ModelFile(modelManager, ast, definitions, fileName)`, before it
/// is added to any manager — `process()` then `fromAst(this.ast)`, plus
/// `isCompatibleVersion()` and the `localTypes` build (the constructor's own
/// three steps, modelfile.ts). All of it runs in one pass here
/// (`ModelFile::from_json_with_definitions`); there is no Rust-side way to
/// call `process()` without immediately `fromAst()`-ing, so the two are one
/// binding. `ast`, `definitions` and `file_name` are the constructor's own
/// three arguments, as the JS values a view holds — nullish for an omitted
/// one — so this throws the same plain `Error`s the constructor's own
/// argument checks raise before ever reading the AST
/// (`ModelFile::check_constructor_arguments`), ahead of any error `fromAst`/
/// `isCompatibleVersion` themselves raise.
///
/// Returns the detached file's snapshot, as JSON text: `{namespace, version,
/// fileName, ast, isSystemModelFile, imports}` (`version` is `null` for an
/// unversioned namespace, as [`ModelManagerHandle::model_file_get_version`]
/// is). A file returned this way has no handle: its declarations are not yet
/// addressable until it is registered in a manager (`ModelManagerHandle::add_model`),
/// which every caller does before it needs one.
#[wasm_bindgen(js_name = modelFileFromAst)]
pub fn model_file_from_ast(
    ast: JsValue,
    definitions: JsValue,
    file_name: JsValue,
) -> std::result::Result<String, JsValue> {
    run(|| {
        let ast_json = to_json(&ast)?;
        let definitions_json = to_json(&definitions)?;
        let file_name_json = to_json(&file_name)?;
        ModelFile::check_constructor_arguments(
            ast_json.as_ref(),
            definitions_json.as_ref(),
            file_name_json.as_ref(),
        )?;
        let ast_value = ast_json.unwrap_or(Value::Null);
        let definitions = definitions.as_string();
        let file_name = file_name.as_string();
        let file = ModelFile::from_json_with_definitions(&ast_value, definitions, file_name)?;
        let version = (!file.version().is_empty()).then(|| file.version().to_string());
        snapshot(&json!({
            "namespace": file.namespace(),
            "version": version,
            "fileName": file.file_name(),
            "ast": file.ast(),
            "isSystemModelFile": file.is_system_namespace(),
            "imports": file.imported_type_names(),
        }))
    })
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

// ---------------------------------------------------------------------------
// DecoratorManager, DCS converter and extractor (src/decoratormanager.ts,
// src/decoratorextractor.ts) — P4-09
// ---------------------------------------------------------------------------
//
// Neither `DecoratorManager` nor `DecoratorExtractor` yet meets a
// Rust-backed `ModelManagerHandle` (P4-08 has not run): every binding below
// takes the plain model ASTs a view reads off its `ModelManager` with
// `getAst`/`getModelFiles`, builds its own throwaway native `ModelManager`
// (`model_manager_from_asts`) the way the already-reviewed P2-12 port's own
// callers do, and hands the result's models back as one more AST for the
// view's `new ModelManager().fromAst(...)` — the same shape `decorateModels`
// and `DecoratorExtractor.extract` already build in TS. `dcsconverter.ts`
// stays out of this: the seam ledger classifies every one of its members TS
// ("YAML (de)serialisation via the `yaml` npm lib ... no model semantics"),
// so `DecoratorManager.jsonToYaml`/`yamlToJson` only need `validate` below —
// the YAML conversion itself is unchanged TS on both sides of the view.

/// `new ModelManager()` (`src/modelmanager.ts`), then `models` (a JSON
/// array of model ASTs, none of them the system ones — a view reads them
/// off `ModelManager.getAst(resolve, false).models`, or a per-file
/// `getModelFiles(false).map(mf => mf.getAst())`) added the way
/// `fromAst`/`add_model` do (P4-09).
fn model_manager_from_asts(models: &[Value]) -> Result<ModelManager> {
    let mut mm = ModelManager::new()?;
    for model in models {
        mm.add_model_with_definitions(model, None, None)?;
    }
    Ok(mm)
}

/// [`model_manager_from_asts`], taking the `models` array itself (anything
/// but an array loads nothing, as `as_array().unwrap_or_default()` read it)
/// and moving each AST into its model file
/// ([`ModelManager::add_owned_model_with_definitions`]: same result, same
/// errors, in the same order) rather than copying the array and then every
/// AST in it (P5-40, F-B).
fn model_manager_from_owned_asts(models: Value) -> Result<ModelManager> {
    let mut mm = ModelManager::new()?;
    if let Value::Array(models) = models {
        for model in models {
            mm.add_owned_model_with_definitions(model, None, None)?;
        }
    }
    Ok(mm)
}

/// [`model_manager_from_asts`], plus the namespaces of the models it added
/// (as distinct from the system ones `ModelManager::new()` pre-loads) — for
/// [`decorator_manager_validate`], which must hand [`dcs::validate`] only
/// the caller's own model files: re-adding a system one to the fresh
/// validation manager `dcs::validate` builds internally is a duplicate
/// namespace.
fn model_manager_from_asts_with_user_ns(
    models: &[Value],
) -> Result<(ModelManager, HashSet<String>)> {
    let mut mm = ModelManager::new()?;
    let mut user_ns = HashSet::new();
    for model in models {
        mm.add_model_with_definitions(model, None, None)?;
        if let Some(ns) = model.get("namespace").and_then(Value::as_str) {
            user_ns.insert(ns.to_string());
        }
    }
    Ok((mm, user_ns))
}

/// A native `ModelManager`'s own models (the system ones included, in load
/// order) as `{ $class, models }` — the shape
/// `BaseModelManager.getAst`/`fromAst` (`src/basemodelmanager.ts`) use. The
/// view's own `fromAst` filters the system ones back out (`EXCLUDE_NS`)
/// exactly as it already does for the ts-mode `decorateModels`/`extract*`
/// bodies, so this need not filter them here.
fn model_manager_to_ast(mm: &ModelManager) -> Value {
    let models: Vec<Value> = mm.model_files().map(|mf| mf.ast().clone()).collect();
    json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": models,
    })
}

/// An `Option<bool>` the way [`dcs::DecorateOptions`]' `disable_*` fields
/// read a JS option: `Some(b)` only for a literal JS boolean, `None` for
/// anything else (absent, `null`, `undefined`, or a non-boolean value),
/// matching TS's `=== false`/truthy-assignment use of the same fields.
fn opt_bool(options: &Value, key: &str) -> Option<bool> {
    match options.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// [`dcs::DecorateOptions`] from `DecoratorManager.decorateModels`'s
/// `options` object.
fn decorate_options_from_js(options: &Value) -> dcs::DecorateOptions {
    dcs::DecorateOptions {
        migrate: options
            .get("migrate")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        validate: options
            .get("validate")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        validate_commands: options
            .get("validateCommands")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        default_namespace: options
            .get("defaultNamespace")
            .cloned()
            .filter(|v| !v.is_null()),
        skip_validation_and_resolution: options
            .get("skipValidationAndResolution")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        disable_metamodel_resolution: opt_bool(options, "disableMetamodelResolution"),
        disable_metamodel_validation: opt_bool(options, "disableMetamodelValidation"),
    }
}

/// [`dcs::ExtractOptions`] from `DecoratorManager.extractDecorators`'s (and
/// its `extractVocabularies`/`extractNonVocabDecorators` siblings') `options`
/// object; TS defaults `removeDecoratorsFromModel` to `false` and `locale`
/// to `'en'` the same way before either ever reads it.
fn extract_options_from_js(options: &Value) -> dcs::ExtractOptions {
    dcs::ExtractOptions {
        remove_decorators_from_model: options
            .get("removeDecoratorsFromModel")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        locale: options
            .get("locale")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "en".to_string()),
    }
}

/// `{ modelManager, decoratorCommandSet, vocabularies }`
/// (`ExtractDecoratorsResult`, `src/decoratormanager.ts`'s JSDoc typedef),
/// from a native [`dcs::extractor::EncodedExtractResult`].
///
/// The intermediate-`Value` encoding: [`extract_result_js`]'s fallback
/// (P5-41), with the command sets parsed back from their text (P5-57).
fn extract_result_to_js(result: &dcs::extractor::EncodedExtractResult) -> Value {
    let decorator_command_set: Value =
        serde_json::from_str(&result.decorator_command_set).unwrap_or(Value::Null);
    json!({
        "modelManager": model_manager_to_ast(&result.model_manager),
        "decoratorCommandSet": decorator_command_set,
        "vocabularies": result.vocabularies,
    })
}

/// P5-41 (F-C, accordproject/concerto-rust#351): the `{ "$class", "models" }`
/// AST [`model_manager_to_ast`] builds, serialised straight from the
/// manager's own model ASTs, without cloning them into a new `Value`.
struct ModelManagerAstView<'a>(&'a ModelManager);

impl serde::Serialize for ModelManagerAstView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("$class", "concerto.metamodel@1.0.0.Models")?;
        map.serialize_entry("models", &ModelAstsView(self.0))?;
        map.end()
    }
}

/// The `models` array of [`ModelManagerAstView`].
struct ModelAstsView<'a>(&'a ModelManager);

impl serde::Serialize for ModelAstsView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_seq(self.0.model_files().map(ModelFile::ast))
    }
}

/// The JSON text of [`extract_result_to_js`]'s object, serialised straight
/// from the borrowed [`dcs::extractor::EncodedExtractResult`] (same keys,
/// same order, same bytes), plus the resident path's `staged` and
/// `validated` keys, which [`DcsManagerHandle::extract`] appends after
/// them. P5-41 (F-C) serialises the model ASTs without cloning them into a
/// new `Value`; P5-57 (T3, accordproject/concerto-rust#378) splices in the
/// command sets, which the extractor has already encoded from the borrowed
/// AST nodes.
fn extract_result_text(
    result: &dcs::extractor::EncodedExtractResult,
    staged: Option<&[Value]>,
) -> serde_json::Result<String> {
    let mut out = Vec::new();
    out.extend_from_slice(b"{\"modelManager\":");
    serde_json::to_writer(&mut out, &ModelManagerAstView(&result.model_manager))?;
    out.extend_from_slice(b",\"decoratorCommandSet\":");
    out.extend_from_slice(result.decorator_command_set.as_bytes());
    out.extend_from_slice(b",\"vocabularies\":");
    serde_json::to_writer(&mut out, &result.vocabularies)?;
    if let Some(staged) = staged {
        out.extend_from_slice(b",\"staged\":");
        serde_json::to_writer(&mut out, staged)?;
        out.extend_from_slice(b",\"validated\":true");
    }
    out.push(b'}');
    // Every piece is serde_json output or a Rust `String`: valid UTF-8.
    String::from_utf8(out).map_err(serde::ser::Error::custom)
}

/// P5-41 (F-C): the JS value of an extract result, encoded directly
/// ([`extract_result_text`], then `JSON.parse`). Additive: the old
/// intermediate-`Value` path ([`extract_result_to_js`] then [`to_js`]) stays
/// as the fallback, should the direct encoding or its parse ever fail.
fn extract_result_js(
    result: &dcs::extractor::EncodedExtractResult,
    staged: Option<Vec<Value>>,
) -> JsValue {
    if let Some(js) = extract_result_text(result, staged.as_deref())
        .ok()
        .and_then(|text| JSON::parse(&text).ok())
    {
        return js;
    }
    let mut out = extract_result_to_js(result);
    if let (Some(staged), Some(map)) = (staged, out.as_object_mut()) {
        map.insert("staged".to_string(), Value::Array(staged));
        map.insert("validated".to_string(), Value::Bool(true));
    }
    to_js(&out)
}

/// TS: `DecoratorManager.falsyOrEqual`. `values` is always a plain string
/// array (every call site passes one).
#[wasm_bindgen(js_name = decoratorManagerFalsyOrEqual)]
pub fn decorator_manager_falsy_or_equal(
    test: JsValue,
    values: JsValue,
) -> std::result::Result<bool, JsValue> {
    run(|| {
        let test_json = to_json(&test)?;
        let values_json = to_json(&values)?.unwrap_or(Value::Array(Vec::new()));
        let values_vec: Vec<String> = values_json
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let values_refs: Vec<&str> = values_vec.iter().map(String::as_str).collect();
        Ok(dcs::falsy_or_equal(test_json.as_ref(), &values_refs))
    })
}

/// TS: `DecoratorManager.migrateTo` (the unused `version` parameter is
/// dropped, as [`dcs::migrate_to`]'s doc comment explains). Mutates a clone
/// of `decorator_command_set` and returns it; the view assigns the result
/// back onto its own variable exactly as the TS body's `return
/// decoratorCommandSet` does.
#[wasm_bindgen(js_name = decoratorManagerMigrateTo)]
pub fn decorator_manager_migrate_to(
    decorator_command_set: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let mut value = to_json(&decorator_command_set)?.unwrap_or(Value::Null);
        dcs::migrate_to(&mut value)?;
        Ok(to_js(&value))
    })
}

/// TS: `DecoratorManager.validate`'s structural check — the second half of
/// the TS body (`serializer.fromJSON(decoratorCommandSet)`); the view still
/// builds the returned `validationModelManager` itself (CTO parsing stays
/// TS). `model_files` is `null`/`undefined` for the no-model-files overload,
/// or an array of model ASTs (a view's own `modelFiles.map(mf =>
/// mf.getAst())`) for the other.
#[wasm_bindgen(js_name = decoratorManagerValidate)]
pub fn decorator_manager_validate(
    decorator_command_set: JsValue,
    model_files: JsValue,
) -> std::result::Result<(), JsValue> {
    run(|| {
        let command_set = to_json(&decorator_command_set)?.unwrap_or(Value::Null);
        match to_json(&model_files)? {
            None | Some(Value::Null) => {
                dcs::validate(&command_set, None)?;
            }
            Some(models) => {
                let models = models.as_array().cloned().unwrap_or_default();
                let (mm, user_ns) = model_manager_from_asts_with_user_ns(&models)?;
                let files: Vec<&ModelFile> = mm
                    .model_files()
                    .filter(|mf| user_ns.contains(mf.namespace()))
                    .collect();
                let refs: Option<&[&ModelFile]> =
                    if files.is_empty() { None } else { Some(&files) };
                dcs::validate(&command_set, refs)?;
            }
        }
        Ok(())
    })
}

/// TS: `DecoratorManager.executePropertyCommand`, which mutates `property`
/// in place and returns nothing; the view copies the mutated fields this
/// returns back onto its own `property` object.
#[wasm_bindgen(js_name = decoratorManagerExecutePropertyCommand)]
pub fn decorator_manager_execute_property_command(
    property: JsValue,
    command: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let mut prop = to_json(&property)?.unwrap_or(Value::Null);
        let cmd = to_json(&command)?.unwrap_or(Value::Null);
        dcs::execute_property_command(&mut prop, &cmd)?;
        Ok(to_js(&prop))
    })
}

/// TS: `DecoratorManager.decorateModels`, after the view's own early return
/// (an empty `decoratorCommandSet` returns `modelManager` itself, never
/// reaching this binding) and its `Array.isArray` normalisation. `models`
/// is `modelManager.getAst(!options.disableMetamodelResolution, false).models`,
/// read by the view's shim (concerto `src/engine/views.ts`
/// `decoratorManagerDecorateModels`): metamodel resolution is not ported
/// (`dcs::decorate_models`'s doc comment), so the TS ModelManager resolves
/// before the call, as the ts-mode body does, and the system namespaces are
/// left out, the native manager carrying its own. The result is the new
/// manager's own AST, for the shim's `new ModelManager({decoratorValidation})
/// .fromAst(decoratedAst, { disableValidation })`.
#[wasm_bindgen(js_name = decoratorManagerDecorateModels)]
pub fn decorator_manager_decorate_models(
    models: JsValue,
    decorator_command_sets: JsValue,
    options: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let models_json = to_json(&models)?.unwrap_or(Value::Array(Vec::new()));
        let models_vec = models_json.as_array().cloned().unwrap_or_default();
        let mm = model_manager_from_asts(&models_vec)?;

        let sets_json = to_json(&decorator_command_sets)?.unwrap_or(Value::Array(Vec::new()));
        let mut sets: Vec<Value> = sets_json.as_array().cloned().unwrap_or_default();

        let options_json = to_json(&options)?.unwrap_or_else(|| json!({}));
        let mut opts = decorate_options_from_js(&options_json);

        let decorated = dcs::decorate_models(&mm, &mut sets, &mut opts)?;
        Ok(to_js(&model_manager_to_ast(&decorated)))
    })
}

/// TS: `DecoratorManager.extractDecorators`. `models` is
/// `modelManager.getAst(true, false).models`: resolved on the TS side, as
/// the ts-mode body's own `getAst(true, true)` is, with the system
/// namespaces left out (see [`decorator_manager_decorate_models`]).
#[wasm_bindgen(js_name = decoratorManagerExtractDecorators)]
pub fn decorator_manager_extract_decorators(
    models: JsValue,
    options: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let models_json = to_json(&models)?.unwrap_or(Value::Array(Vec::new()));
        let mm = model_manager_from_owned_asts(models_json)?;
        let options_json = to_json(&options)?.unwrap_or_else(|| json!({}));
        let opts = extract_options_from_js(&options_json);
        let result = dcs::extract_encoded(&mm, &opts, dcs::extractor::Action::ExtractAll)?;
        Ok(extract_result_js(&result, None))
    })
}

/// TS: `DecoratorManager.extractVocabularies`. `models` is
/// `modelManager.getAst(true, false).models` (see
/// [`decorator_manager_extract_decorators`]).
#[wasm_bindgen(js_name = decoratorManagerExtractVocabularies)]
pub fn decorator_manager_extract_vocabularies(
    models: JsValue,
    options: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let models_json = to_json(&models)?.unwrap_or(Value::Array(Vec::new()));
        let mm = model_manager_from_owned_asts(models_json)?;
        let options_json = to_json(&options)?.unwrap_or_else(|| json!({}));
        let opts = extract_options_from_js(&options_json);
        let result = dcs::extract_encoded(&mm, &opts, dcs::extractor::Action::ExtractVocab)?;
        Ok(extract_result_js(&result, None))
    })
}

/// TS: `DecoratorManager.extractNonVocabDecorators`. `models` is
/// `modelManager.getAst(true, false).models`, resolved on the TS side and
/// without the system namespaces, matching the ts-mode body's own
/// `getAst(true)` call (the one-argument overload).
#[wasm_bindgen(js_name = decoratorManagerExtractNonVocabDecorators)]
pub fn decorator_manager_extract_non_vocab_decorators(
    models: JsValue,
    options: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    run(|| {
        let models_json = to_json(&models)?.unwrap_or(Value::Array(Vec::new()));
        let mm = model_manager_from_owned_asts(models_json)?;
        let options_json = to_json(&options)?.unwrap_or_else(|| json!({}));
        let opts = extract_options_from_js(&options_json);
        let result = dcs::extract_encoded(&mm, &opts, dcs::extractor::Action::ExtractNonVocab)?;
        Ok(extract_result_js(&result, None))
    })
}

// ---------------------------------------------------------------------------
// P5-27 (F6, accordproject/concerto-rust#332): a resident DCS manager with
// staged-handle results. Additive: the `decoratorManager*` bindings above
// are unchanged.
//
// The bindings above rebuild the input manager from the source models' AST
// on every call, and hand back the result models as AST only. The view then
// loads that AST into a new ModelManager (`fromAst`), which sends every
// model back into Rust (`stageModelFile`), reads each header across the
// boundary (`modelFileFromAstHeader`) and validates the whole set again
// (`validateModelFiles`), although Rust has just loaded and validated those
// same models.
//
// [`DcsManagerHandle`] keeps the input manager resident, so the view builds
// it once per source ModelManager and reuses it while that manager's epoch
// and model files are unchanged (engine/views.ts `dcsManagerFor`). Each of
// its operations stages the result's model files into the new
// ModelManager's own handle (`target`) and returns, with the result AST,
// each file's stage id and header ([`staged_header`]), and whether the
// result was validated. The view builds each ModelFile from its stage id and
// header without sending the AST again, and skips its own
// `validateModelFiles` when Rust has already validated the same files.
// ---------------------------------------------------------------------------

/// TS `EXCLUDE_NS` (src/basemodelmanager.ts): the system namespaces
/// `fromAst` skips, since the new manager already has them.
const DCS_EXCLUDE_NS: [&str; 3] = ["concerto@1.0.0", "concerto", "concerto.decorator@1.0.0"];

/// The system type names every non-system model file imports implicitly
/// (the `imports.push` in [`model_file_from_ast_header`]).
const IMPLICIT_SYSTEM_TYPES: [&str; 5] =
    ["Concept", "Asset", "Transaction", "Participant", "Event"];

/// What [`model_file_from_ast_header`] would set on a JS `ModelFile` built
/// from `ast`, when it would succeed: `[version, shortNames, uris]`, where
/// `shortNames` is the `importShortNames` entries and `uris` the
/// `importUriMap` entries, each flattened to `[key, value, key, value, …]`
/// in insertion order (the implicit system import included). The view sets
/// `namespace` to `ast.namespace` and `imports` to a copy of `ast.imports`
/// plus the implicit import, as the binding does, and applies these.
///
/// `None` whenever the header is not the plain case: a system namespace (no
/// implicit import), an unversioned or invalid namespace, an import the
/// binding would reject (unversioned, wildcard, aliased to a primitive
/// type), or any value that is not the JSON type the plain case reads. The
/// view then calls [`model_file_from_ast_header`] itself, which throws what
/// it throws. So a header returned here is always the one that binding
/// would set.
fn staged_header(ast: &Value) -> Option<Value> {
    let namespace = ast.get("namespace")?.as_str()?;
    if namespace.is_empty() || namespace == "concerto" || namespace.starts_with("concerto@") {
        return None;
    }
    let (name, version) = match mu::parse_namespace(namespace).ok()? {
        mu::ParsedNamespace::Full {
            name,
            version: Some(version),
            ..
        } if !version.is_empty() => (name, version),
        _ => return None,
    };
    if !name.split('.').all(mu::is_valid_identifier) {
        return None;
    }

    let implicit = json!({
        "$class": format!("{METAMODEL_NAMESPACE}.ImportTypes"),
        "namespace": "concerto@1.0.0",
        "types": IMPLICIT_SYSTEM_TYPES,
    });
    let own: &[Value] = match ast.get("imports") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(imports)) => imports,
        Some(_) => return None,
    };
    let import_types = format!("{METAMODEL_NAMESPACE}.ImportTypes");
    let import_type = format!("{METAMODEL_NAMESPACE}.ImportType");
    let mut short_names: Vec<Value> = Vec::new();
    let mut uris: Vec<Value> = Vec::new();
    for imp in own.iter().chain(std::iter::once(&implicit)) {
        // `enforceImportVersioning(imp)`: a versioned namespace.
        let imp_namespace = imp.get("namespace")?.as_str()?;
        match mu::parse_namespace(imp_namespace).ok()? {
            mu::ParsedNamespace::Full {
                version: Some(v), ..
            } if !v.is_empty() => {}
            _ => return None,
        }
        let class = imp.get("$class")?.as_str()?;
        if class == import_types {
            let aliases: Option<std::collections::HashMap<&str, &str>> =
                match imp.get("aliasedTypes") {
                    None | Some(Value::Null) => None,
                    Some(Value::Array(entries)) if entries.is_empty() => None,
                    Some(Value::Array(entries)) => {
                        let mut map = std::collections::HashMap::new();
                        for entry in entries {
                            let alias_name = entry.get("name")?.as_str()?;
                            let aliased_name = entry.get("aliasedName")?.as_str()?;
                            if mu::is_primitive_type(aliased_name) {
                                return None;
                            }
                            map.insert(alias_name, aliased_name);
                        }
                        Some(map)
                    }
                    Some(_) => return None,
                };
            for type_name in imp.get("types")?.as_array()? {
                let type_name = type_name.as_str()?;
                let key = aliases
                    .as_ref()
                    .and_then(|a| a.get(type_name).copied())
                    .unwrap_or(type_name);
                short_names.push(Value::from(key));
                short_names.push(Value::from(format!("{imp_namespace}.{type_name}")));
            }
        } else if class == import_type {
            let local = imp.get("name")?.as_str()?;
            short_names.push(Value::from(local));
            short_names.push(Value::from(format!("{imp_namespace}.{local}")));
        } else {
            return None;
        }
        match imp.get("uri") {
            None | Some(Value::Null) => {}
            Some(Value::String(uri)) if uri.is_empty() => {}
            Some(Value::String(uri)) => {
                let first = mu::import_fully_qualified_names(Some(imp))
                    .ok()?
                    .into_iter()
                    .next()?;
                uris.push(Value::from(first));
                uris.push(Value::from(uri.as_str()));
            }
            Some(_) => return None,
        }
    }
    Some(json!([version, short_names, uris]))
}

/// Stages every non-system model file of `result` into `target`'s staging
/// slot, while it has room (a file past [`StagedModelFiles::CAPACITY`] is
/// not staged, so no other stage is evicted), and returns one entry per
/// model file of `result`, in [`model_manager_to_ast`]'s order: `null` for a
/// file not staged, or `[stageId, header]` ([`staged_header`], `null` when
/// the view must read the header itself). Staging never changes `target`'s
/// manager or epoch.
///
/// P5-77 (accordproject/concerto-rust#419): each file is staged shared with
/// `result` ([`ModelManager::shared_model_files`]), not deep-copied; every
/// caller drops `result` (or keeps it unchanged, in the extract memo) once
/// the call returns.
fn stage_result(target: &mut ModelManagerHandle, result: &ModelManager) -> Vec<Value> {
    result
        .shared_model_files()
        .map(|mf| {
            if DCS_EXCLUDE_NS.contains(&mf.namespace())
                || target.staged.files.len() >= StagedModelFiles::CAPACITY
            {
                return Value::Null;
            }
            let header = staged_header(mf.ast()).unwrap_or(Value::Null);
            let id = target.staged.insert_shared(std::sync::Arc::clone(mf));
            json!([id, header])
        })
        .collect()
}

/// The input manager of the `DecoratorManager` operations, kept resident
/// across calls (P5-27, F6): the source models, as the view reads them off
/// `modelManager.getAst(resolve, false).models`, loaded once
/// ([`model_manager_from_owned_asts`], as each `decoratorManagerExtract*`
/// binding loads them on every call). The view keeps one per source
/// ModelManager and resolution flag, and builds a new one once that manager's epoch or model
/// files change. The operations never change it. Additive.
#[wasm_bindgen]
pub struct DcsManagerHandle {
    manager: ModelManager,
}

#[wasm_bindgen]
impl DcsManagerHandle {
    /// Loads `models` (a JSON array of model ASTs, none of them the system
    /// ones), throwing what [`decorator_manager_decorate_models`] throws
    /// while it loads them.
    #[wasm_bindgen(constructor)]
    pub fn new(models: JsValue) -> std::result::Result<DcsManagerHandle, JsValue> {
        run(|| {
            let models_json = to_json(&models)?.unwrap_or(Value::Array(Vec::new()));
            let manager = model_manager_from_owned_asts(models_json)?;
            Ok(Self { manager })
        })
    }

    /// [`decorator_manager_decorate_models`] on the resident manager, with
    /// the result staged into `target` (the new ModelManager's handle, as
    /// the view's `clearModelFiles` left it). Returns `{ast, staged,
    /// validated}`: `ast` is what that binding returns, `staged` is
    /// [`stage_result`]'s entries for `ast.models`, and `validated` is
    /// whether the result manager was validated (every model but the system
    /// ones).
    ///
    /// P5-54 (accordproject/concerto-rust#375): the result is validated
    /// with `target`'s `decoratorValidation`, as TS validates it in
    /// `new ModelManager({decoratorValidation: modelManager
    /// .getDecoratorValidation()}).fromAst(…)`: the view builds `target`
    /// with the source manager's option, and [`dcs::decorate_models`] gives
    /// its result the input manager's, so the resident manager takes
    /// `target`'s before it runs. A fresh resident manager has the default
    /// (disabled) option, so without this the view, which trusts
    /// `validated`, skipped the decorator checks.
    #[wasm_bindgen(js_name = decorateModels)]
    pub fn decorate_models(
        &mut self,
        target: &mut ModelManagerHandle,
        decorator_command_sets: JsValue,
        options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        self.manager
            .set_decorator_validation(target.manager.decorator_validation().clone());
        run(|| staged_decorate_models(&self.manager, target, &decorator_command_sets, &options))
    }

    /// [`decorator_manager_extract_decorators`] on the resident manager,
    /// with the result's model manager staged into `target`. Returns what
    /// that binding returns, plus `staged` (for `modelManager.models`) and
    /// `validated` (always `true`: the extractor validates its result).
    #[wasm_bindgen(js_name = extractDecorators)]
    pub fn extract_decorators(
        &self,
        target: &mut ModelManagerHandle,
        options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        self.extract(target, &options, dcs::extractor::Action::ExtractAll)
    }

    /// [`decorator_manager_extract_vocabularies`] on the resident manager
    /// (see [`Self::extract_decorators`]).
    #[wasm_bindgen(js_name = extractVocabularies)]
    pub fn extract_vocabularies(
        &self,
        target: &mut ModelManagerHandle,
        options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        self.extract(target, &options, dcs::extractor::Action::ExtractVocab)
    }

    /// [`decorator_manager_extract_non_vocab_decorators`] on the resident
    /// manager (see [`Self::extract_decorators`]).
    #[wasm_bindgen(js_name = extractNonVocabDecorators)]
    pub fn extract_non_vocab_decorators(
        &self,
        target: &mut ModelManagerHandle,
        options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        self.extract(target, &options, dcs::extractor::Action::ExtractNonVocab)
    }
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// TS: `DecoratorManager.validate`'s structural check
    /// (`serializer.fromJSON(decoratorCommandSet)`), against this handle's
    /// own resident manager (P5-27, F6). The view calls it on the
    /// `validationModelManager` it has just built and returns (the
    /// metamodel, the caller's model files and the DCS model), once that
    /// manager's rustHandle mirrors its model files; so, unlike
    /// [`decorator_manager_validate`], it neither sends the model files
    /// again nor rebuilds a manager from them, and [`dcs::validate_against`]
    /// throws what [`dcs::validate`] throws at the same step. Additive:
    /// `decoratorManagerValidate` is unchanged and remains the view's
    /// fallback. Never changes the manager.
    #[wasm_bindgen(js_name = dcsValidate)]
    pub fn dcs_validate(&self, decorator_command_set: JsValue) -> std::result::Result<(), JsValue> {
        run(|| {
            let command_set = to_json(&decorator_command_set)?.unwrap_or(Value::Null);
            dcs::validate_against(&self.manager, &command_set)?;
            Ok(())
        })
    }
}

impl DcsManagerHandle {
    /// One extract operation, staged into `target`.
    fn extract(
        &self,
        target: &mut ModelManagerHandle,
        options: &JsValue,
        action: dcs::extractor::Action,
    ) -> std::result::Result<JsValue, JsValue> {
        run(|| staged_extract(&self.manager, target, options, action))
    }
}

/// [`DcsManagerHandle::decorate_models`]'s body, on `manager` (whose
/// `decoratorValidation` the caller has already set to `target`'s): the
/// result staged into `target`, and `{ast, staged, validated}` returned.
fn staged_decorate_models(
    manager: &ModelManager,
    target: &mut ModelManagerHandle,
    decorator_command_sets: &JsValue,
    options: &JsValue,
) -> Result<JsValue> {
    let sets_json = to_json(decorator_command_sets)?.unwrap_or(Value::Array(Vec::new()));
    let mut sets: Vec<Value> = sets_json.as_array().cloned().unwrap_or_default();

    let options_json = to_json(options)?.unwrap_or_else(|| json!({}));
    let mut opts = decorate_options_from_js(&options_json);

    // `dcs::decorate_models` validates the result unless the command sets
    // are empty or `disable_metamodel_validation` (as the options stand
    // once `skip_validation_and_resolution` has set it) is `Some(true)`.
    let applied = !sets.is_empty();
    let decorated = dcs::decorate_models(manager, &mut sets, &mut opts)?;
    let validated = applied && opts.disable_metamodel_validation != Some(true);
    let staged = stage_result(target, &decorated);
    Ok(to_js(&json!({
        "ast": model_manager_to_ast(&decorated),
        "staged": staged,
        "validated": validated,
    })))
}

/// [`DcsManagerHandle::extract`]'s body, on `manager`: one extract
/// operation, its result's model files staged into `target`.
fn staged_extract(
    manager: &ModelManager,
    target: &mut ModelManagerHandle,
    options: &JsValue,
    action: dcs::extractor::Action,
) -> Result<JsValue> {
    let options_json = to_json(options)?.unwrap_or_else(|| json!({}));
    let opts = extract_options_from_js(&options_json);
    let result = dcs::extract_encoded(manager, &opts, action)?;
    // P5-77: staged shared, with the result's ASTs kept as text.
    Ok(compacted_extract_js(target, result, Vec::new()).0)
}

// ---------------------------------------------------------------------------
// P5-55 (T1, F-A1, accordproject/concerto-rust#376): the DecoratorManager
// operations on the source ModelManager's own rustHandle. Additive: the
// `decoratorManager*` bindings and [`DcsManagerHandle`] are unchanged and
// stay the view's fallbacks.
//
// The view's source ModelManager already mirrors its model files into its
// rustHandle (P4-08, P5-34), so the handle holds exactly the models a
// [`DcsManagerHandle`] would be built from: the same ASTs, loaded the same
// way, with the same system models. [`dcs::decorate_models`] and
// [`dcs::extract_encoded`] resolve those models themselves
// (`ModelManager::models_ast`), so running them on the handle's own manager
// skips the copy (`getAst`, then JsValue to `Value`, then the load) that a
// cold [`DcsManagerHandle`] costs. Each operation is
// [`DcsManagerHandle`]'s, staged into `target` the same way.
// ---------------------------------------------------------------------------

#[wasm_bindgen]
impl ModelManagerHandle {
    /// [`DcsManagerHandle::decorate_models`] on this handle's own manager.
    /// `target` is the new ModelManager's handle, never this one. The result
    /// is validated with `target`'s `decoratorValidation`, as
    /// [`DcsManagerHandle::decorate_models`] validates it (P5-54): this
    /// manager's own option is set to it for the call and restored after,
    /// so the call never changes this manager (nor its epoch).
    #[wasm_bindgen(js_name = dcsDecorateModels)]
    pub fn dcs_decorate_models(
        &mut self,
        target: &mut ModelManagerHandle,
        decorator_command_sets: JsValue,
        options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        let own = self.manager.decorator_validation().clone();
        self.manager
            .set_decorator_validation(target.manager.decorator_validation().clone());
        let result = run(|| {
            staged_decorate_models(&self.manager, target, &decorator_command_sets, &options)
        });
        self.manager.set_decorator_validation(own);
        result
    }

    /// [`DcsManagerHandle::extract_decorators`] on this handle's own
    /// manager, staged into `target` (the new ModelManager's handle, never
    /// this one). Never changes this manager.
    #[wasm_bindgen(js_name = dcsExtractDecorators)]
    pub fn dcs_extract_decorators(
        &self,
        target: &mut ModelManagerHandle,
        options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        run(|| self.memo_extract(target, &options, dcs::extractor::Action::ExtractAll))
    }

    /// [`DcsManagerHandle::extract_vocabularies`] on this handle's own
    /// manager (see [`Self::dcs_extract_decorators`]).
    #[wasm_bindgen(js_name = dcsExtractVocabularies)]
    pub fn dcs_extract_vocabularies(
        &self,
        target: &mut ModelManagerHandle,
        options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        run(|| self.memo_extract(target, &options, dcs::extractor::Action::ExtractVocab))
    }

    /// [`DcsManagerHandle::extract_non_vocab_decorators`] on this handle's
    /// own manager (see [`Self::dcs_extract_decorators`]).
    #[wasm_bindgen(js_name = dcsExtractNonVocabDecorators)]
    pub fn dcs_extract_non_vocab_decorators(
        &self,
        target: &mut ModelManagerHandle,
        options: JsValue,
    ) -> std::result::Result<JsValue, JsValue> {
        run(|| self.memo_extract(target, &options, dcs::extractor::Action::ExtractNonVocab))
    }
}

// ---------------------------------------------------------------------------
// P5-56 (T2, F-A2, accordproject/concerto-rust#377): a per-epoch extract
// result memo on the source handle (the P5-42 report's Design 2, on #352).
//
// With `removeDecoratorsFromModel` false, an extract's result models are the
// handle's own models, resolved, whatever the action and locale; only the
// command sets and vocabularies depend on those. So while the epoch is
// unchanged, a repeated `dcsExtract*` call reuses the result manager, its
// encoded AST and its staged headers, and rebuilds only the command sets
// and vocabularies from the kept source models
// ([`dcs::encode_extract_source`]): no resolve, no result-manager build,
// validation or drop, no AST encode.
//
// - Filled on the second call at the same epoch (and system flag), so a
//   one-shot caller never pays for it; the first call only notes its key.
// - Dropped when the epoch moves ([`ModelManagerHandle::bump_epoch`]), on
//   [`ModelManagerHandle::drop_dcs_memo`] and with the handle.
// - Errors are never memoised: a call that throws leaves no memo, and the
//   next call runs in full. A memo exists only after a call whose result
//   models loaded and validated, which is the only error that
//   `extract_encoded` reports ahead of the transform's, so a repeated call
//   throws what a full one throws.
// - Nothing shared is returned: the JS result is parsed from new text on
//   every call. P5-77: each staged model file is shared with the kept
//   result manager (a model file never changes once built), not cloned.
//
// P5-77 (accordproject/concerto-rust#419): with `removeDecoratorsFromModel`
// true, the result models are the handle's own models, resolved, with the
// decorators the action strips removed: they depend on the action, but not
// on the locale, and the command sets and vocabularies are read before any
// decorator is stripped ([`dcs::encode_extract_source`]). So the same memo
// serves that case too, keyed by the action as well; everything above
// holds for it unchanged.
// ---------------------------------------------------------------------------

/// A [`ModelManagerHandle`]'s extract memo (P5-56): its key, and the kept
/// result once the second call at that key has filled it.
struct DcsExtractMemo {
    /// `(epoch, system models walked, stripping action)`: `ExtractAll` and
    /// `ExtractVocab` walk the system models too, `ExtractNonVocab` does not
    /// ([`dcs::extract_encoded`]); the stripping action is the action when
    /// `removeDecoratorsFromModel` is true (P5-77), and `None` when it is
    /// false, since then every action gives the same result models.
    key: (u64, bool, Option<dcs::extractor::Action>),
    /// `None` after the first call at `key`, `Some` from the second on.
    kept: Option<DcsExtractKept>,
}

/// [`ModelManagerAstView`]'s text, from each model's own AST text
/// ([`ModelManager::compact_model_asts`]): the same compact envelope, byte
/// for byte (P5-77).
fn models_envelope_text(texts: &[std::sync::Arc<str>]) -> String {
    let mut out = String::with_capacity(64 + texts.iter().map(|t| t.len() + 1).sum::<usize>());
    out.push_str("{\"$class\":\"concerto.metamodel@1.0.0.Models\",\"models\":[");
    for (i, text) in texts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(text);
    }
    out.push_str("]}");
    out
}

/// P5-77: [`stage_result`] then [`extract_result_js`] for a result the
/// caller drops once the call returns, through [`DcsExtractKept`], so the
/// files staged into `target` keep their ASTs as text
/// ([`DcsExtractKept::new`]). The same JS value, the same stages.
fn compacted_extract_js(
    target: &mut ModelManagerHandle,
    result: dcs::extractor::EncodedExtractResult,
    source: Vec<Value>,
) -> (JsValue, DcsExtractKept) {
    let dcs::extractor::EncodedExtractResult {
        model_manager,
        decorator_command_set,
        vocabularies,
    } = result;
    let kept = DcsExtractKept::new(source, model_manager);
    let staged = kept.stage(target);
    let js = kept.result_js(&decorator_command_set, &vocabularies, staged);
    (js, kept)
}

/// What a repeated extract at the same key reuses.
struct DcsExtractKept {
    /// The resolved source models the extractor walks.
    source: Vec<Value>,
    /// The result manager, staged from on every call.
    result: ModelManager,
    /// The JSON text of the result manager's AST ([`ModelManagerAstView`]).
    ast_text: String,
    /// [`staged_header`] of each of `result`'s model files, in order.
    headers: Vec<Value>,
}

impl DcsExtractKept {
    /// P5-77 (accordproject/concerto-rust#419): the staged headers are read
    /// from the result's parsed ASTs first, then the ASTs are compacted
    /// ([`ModelManager::compact_model_asts`]): each result model file keeps
    /// its AST as the JSON text [`ModelManagerAstView`] writes for it, and
    /// [`Self::ast_text`] is spliced from those texts, byte for byte that
    /// view's text. So the files staged from it (shared, see
    /// [`Self::stage`]) hold text, not a parsed tree, for as long as the
    /// result ModelManager lives. Should the compaction fail, `ast_text` is
    /// left empty and [`Self::result_js`] takes its fallback, as before
    /// when the view's text failed.
    fn new(source: Vec<Value>, mut result: ModelManager) -> Self {
        let headers = result
            .model_files()
            .map(|mf| staged_header(mf.ast()).unwrap_or(Value::Null))
            .collect();
        let ast_text = result
            .compact_model_asts()
            .map(|texts| models_envelope_text(&texts))
            .unwrap_or_default();
        Self {
            source,
            result,
            ast_text,
            headers,
        }
    }

    /// [`stage_result`] from the kept result manager, with its kept headers.
    /// P5-77: each file is staged shared with the kept manager, which never
    /// changes, so a repeated extract copies no model file.
    fn stage(&self, target: &mut ModelManagerHandle) -> Vec<Value> {
        self.result
            .shared_model_files()
            .zip(&self.headers)
            .map(|(mf, header)| {
                if DCS_EXCLUDE_NS.contains(&mf.namespace())
                    || target.staged.files.len() >= StagedModelFiles::CAPACITY
                {
                    return Value::Null;
                }
                let id = target.staged.insert_shared(std::sync::Arc::clone(mf));
                json!([id, header])
            })
            .collect()
    }

    /// [`extract_result_text`] for the kept result and this call's command
    /// sets and vocabularies, byte for byte, with the AST spliced in from
    /// [`Self::ast_text`].
    fn result_text(
        &self,
        command_sets: &str,
        vocabularies: &[String],
        staged: &[Value],
    ) -> serde_json::Result<String> {
        if self.ast_text.is_empty() {
            return Err(serde::ser::Error::custom("no kept AST text"));
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"{\"modelManager\":");
        out.extend_from_slice(self.ast_text.as_bytes());
        out.extend_from_slice(b",\"decoratorCommandSet\":");
        out.extend_from_slice(command_sets.as_bytes());
        out.extend_from_slice(b",\"vocabularies\":");
        serde_json::to_writer(&mut out, vocabularies)?;
        out.extend_from_slice(b",\"staged\":");
        serde_json::to_writer(&mut out, staged)?;
        out.extend_from_slice(b",\"validated\":true}");
        String::from_utf8(out).map_err(serde::ser::Error::custom)
    }

    /// [`extract_result_js`] for the kept result: [`Self::result_text`],
    /// parsed, or the same intermediate-`Value` fallback.
    fn result_js(
        &self,
        command_sets: &str,
        vocabularies: &[String],
        staged: Vec<Value>,
    ) -> JsValue {
        let text = self.result_text(command_sets, vocabularies, &staged);
        if let Some(js) = text.ok().and_then(|text| JSON::parse(&text).ok()) {
            return js;
        }
        // The intermediate-`Value` fallback, as [`extract_result_js`]'s.
        let decorator_command_set: Value =
            serde_json::from_str(command_sets).unwrap_or(Value::Null);
        to_js(&json!({
            "modelManager": model_manager_to_ast(&self.result),
            "decoratorCommandSet": decorator_command_set,
            "vocabularies": vocabularies,
            "staged": staged,
            "validated": true,
        }))
    }
}

impl ModelManagerHandle {
    /// [`staged_extract`] on this handle's own manager, through the
    /// per-epoch memo (P5-56; P5-77 for `removeDecoratorsFromModel` true).
    fn memo_extract(
        &self,
        target: &mut ModelManagerHandle,
        options: &JsValue,
        action: dcs::extractor::Action,
    ) -> Result<JsValue> {
        let options_json = to_json(options)?.unwrap_or_else(|| json!({}));
        let opts = extract_options_from_js(&options_json);
        let key = (
            self.epoch,
            action != dcs::extractor::Action::ExtractNonVocab,
            opts.remove_decorators_from_model.then_some(action),
        );
        let mut memo = self.dcs_memo.borrow_mut();
        match memo.as_mut() {
            Some(DcsExtractMemo {
                key: memo_key,
                kept: Some(kept),
            }) if *memo_key == key => {
                let (command_sets, vocabularies) =
                    dcs::encode_extract_source(&kept.source, &opts, action)?;
                let staged = kept.stage(target);
                Ok(kept.result_js(&command_sets, &vocabularies, staged))
            }
            Some(DcsExtractMemo {
                key: memo_key,
                kept: kept @ None,
            }) if *memo_key == key => {
                let (result, source) =
                    dcs::extract_encoded_keeping_source(&self.manager, &opts, action)?;
                let (js, filled) = compacted_extract_js(target, result, source);
                *kept = Some(filled);
                Ok(js)
            }
            _ => {
                *memo = Some(DcsExtractMemo { key, kept: None });
                drop(memo);
                let result = dcs::extract_encoded(&self.manager, &opts, action)?;
                Ok(compacted_extract_js(target, result, Vec::new()).0)
            }
        }
    }
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// Drops this handle's extract result memo (P5-56), freeing the result
    /// models it keeps; the next repeated extract fills it again. Never
    /// changes the manager or its epoch. Additive.
    #[wasm_bindgen(js_name = dropDcsMemo)]
    pub fn drop_dcs_memo(&self) {
        *self.dcs_memo.borrow_mut() = None;
    }
}

// P5-12c (accordproject/concerto-rust#293): `ValidatedResource.validate()`,
// `setPropertyValue` and `addArrayValue` in one engine call each.
mod validate_resource;

#[cfg(test)]
mod tests {
    // Host-side tests of the pure wire codec (no `js_sys` call is reached on
    // these paths): `cargo test` from concerto-wasm/.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// P5-56 (T2, F-A2): a repeated extract through the memo writes, byte
    /// for byte, the text a full extract writes (result AST, command sets,
    /// vocabularies, staged ids and headers), for every action and locale,
    /// with `removeDecoratorsFromModel` false and (P5-77) true, and stages
    /// files whose ASTs equal the full extract's; moving the epoch drops the
    /// memo.
    #[test]
    fn the_extract_memo_writes_what_a_full_extract_writes() {
        let mm =
            |v: &str| json!([{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": v}]);
        let dec = |name: &str, args: Value| json!({"$class": "concerto.metamodel@1.0.0.Decorator", "name": name, "arguments": args});
        let mut handle = ModelManagerHandle::new().unwrap();
        handle
            .manager
            .add_model_ast(
                &json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.memo@1.0.0",
                    "decorators": [dec("Term", mm("Memo")), dec("Tag", mm("m"))],
                    "declarations": [{
                        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false,
                        "decorators": [dec("Term", mm("A person")), dec("Flag", json!([]))],
                        "properties": [{
                            "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false,
                            "decorators": [dec("Term_description", mm("The name")), dec("Tag", mm("n"))]
                        }]
                    }]
                }),
                None,
            )
            .unwrap();
        for (action, remove) in [
            dcs::extractor::Action::ExtractAll,
            dcs::extractor::Action::ExtractVocab,
            dcs::extractor::Action::ExtractNonVocab,
        ]
        .into_iter()
        .flat_map(|action| [(action, false), (action, true)])
        {
            let fill = dcs::ExtractOptions {
                remove_decorators_from_model: remove,
                ..dcs::ExtractOptions::default()
            };
            let (result, source) =
                dcs::extract_encoded_keeping_source(&handle.manager, &fill, action).unwrap();
            let kept = DcsExtractKept::new(source, result.model_manager);
            for locale in ["en", "fr"] {
                let opts = dcs::ExtractOptions {
                    remove_decorators_from_model: remove,
                    locale: locale.to_string(),
                };
                let full = dcs::extract_encoded(&handle.manager, &opts, action).unwrap();
                let mut t1 = ModelManagerHandle::new().unwrap();
                let mut t2 = ModelManagerHandle::new().unwrap();
                let staged = stage_result(&mut t1, &full.model_manager);
                let expected = extract_result_text(&full, Some(&staged)).unwrap();
                let (sets, vocabularies) =
                    dcs::encode_extract_source(&kept.source, &opts, action).unwrap();
                let staged = kept.stage(&mut t2);
                let got = kept.result_text(&sets, &vocabularies, &staged).unwrap();
                assert_eq!(got, expected, "{action:?} {remove} {locale}");
                assert_eq!(t2.staged.files.len(), t1.staged.files.len());
                assert!(!t2.staged.files.is_empty());
                for (a, b) in t1.staged.files.values().zip(t2.staged.files.values()) {
                    assert_eq!(a.namespace(), b.namespace());
                    assert_eq!(a.ast(), b.ast(), "{action:?} {remove} {locale}");
                }
            }
        }
        *handle.dcs_memo.get_mut() = Some(DcsExtractMemo {
            key: (handle.epoch, true, None),
            kept: None,
        });
        handle.bump_epoch();
        assert!(handle.dcs_memo.get_mut().is_none());
    }

    /// P5-41 (F-C) and P5-57 (T3): the direct encoding of an extract result
    /// (the model ASTs borrowed, the command sets encoded from the borrowed
    /// AST nodes) is, byte for byte, the JSON text of the old intermediate
    /// `Value` route (same keys, same order, same numbers), with and without
    /// the resident path's keys, for each extract action; so is the
    /// fallback's `Value`.
    #[test]
    fn extract_result_text_matches_the_value_route() {
        let dec = |name: &str, args: Value| json!({"$class": "concerto.metamodel@1.0.0.Decorator", "name": name, "arguments": args});
        let model = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.p541@1.0.0",
            "imports": [],
            "decorators": [dec("Term", json!([{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "A model"}]))],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "decorators": [
                    dec("Term", json!([{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "A person"}])),
                    dec("Weight", json!([{"$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 0.1}])),
                ],
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "name",
                    "isArray": false,
                    "isOptional": false,
                    "decorators": [
                        dec("Term", json!([{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "Name"}])),
                        dec("Flag", json!([])),
                    ],
                }],
            }],
        });
        for remove in [false, true] {
            let Ok(mm) = model_manager_from_asts(std::slice::from_ref(&model)) else {
                panic!("model_manager_from_asts failed");
            };
            let opts = dcs::ExtractOptions {
                remove_decorators_from_model: remove,
                locale: "en".to_string(),
            };
            for (action, value_route) in [
                (
                    dcs::extractor::Action::ExtractAll,
                    dcs::extract_decorators
                        as fn(
                            &ModelManager,
                            &dcs::ExtractOptions,
                        )
                            -> concerto_core::Result<dcs::extractor::ExtractResult>,
                ),
                (
                    dcs::extractor::Action::ExtractVocab,
                    dcs::extract_vocabularies,
                ),
                (
                    dcs::extractor::Action::ExtractNonVocab,
                    dcs::extract_non_vocab_decorators,
                ),
            ] {
                let value = value_route(&mm, &opts).unwrap();
                let result = dcs::extract_encoded(&mm, &opts, action).unwrap();
                let old = json!({
                    "modelManager": model_manager_to_ast(&value.model_manager),
                    "decoratorCommandSet": value.decorator_command_set,
                    "vocabularies": value.vocabularies,
                });
                assert_eq!(
                    extract_result_text(&result, None).unwrap(),
                    serde_json::to_string(&old).unwrap()
                );
                assert_eq!(extract_result_to_js(&result), old);
            }

            let value = dcs::extract_decorators(&mm, &opts).unwrap();
            assert!(!value.decorator_command_set.is_empty());
            assert!(!value.vocabularies.is_empty());
            let result =
                dcs::extract_encoded(&mm, &opts, dcs::extractor::Action::ExtractAll).unwrap();
            let old = json!({
                "modelManager": model_manager_to_ast(&value.model_manager),
                "decoratorCommandSet": value.decorator_command_set,
                "vocabularies": value.vocabularies,
            });

            let staged = vec![json!([0, null]), Value::Null];
            let mut old = old;
            let map = old.as_object_mut().unwrap();
            map.insert("staged".to_string(), Value::Array(staged.clone()));
            map.insert("validated".to_string(), Value::Bool(true));
            assert_eq!(
                extract_result_text(&result, Some(&staged)).unwrap(),
                serde_json::to_string(&old).unwrap()
            );
        }
    }

    /// P5-28: [`staged_header`] of a canonical file gives what
    /// `modelFileFromAstHeader` sets: the version, the short names in
    /// order (an alias in place of its type's name, the implicit system
    /// import last) and the URI map keyed by each import's first name.
    #[test]
    fn staged_header_reads_a_canonical_file() {
        let imports = json!([
            {"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a@1.0.0", "name": "A", "uri": "https://a"},
            {"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.b@2.0.0", "types": ["B", "C"],
             "aliasedTypes": [{"$class": "concerto.metamodel@1.0.0.AliasedType", "name": "C", "aliasedName": "D"}],
             "uri": "https://b"},
        ]);
        let header = staged_header_from_parts("org.x@1.0.0", Some(&imports)).unwrap();
        assert_eq!(
            header,
            json!({
                "namespace": "org.x@1.0.0",
                "version": "1.0.0",
                "system": false,
                "shortNames": [
                    ["A", "org.a@1.0.0.A"],
                    ["B", "org.b@2.0.0.B"],
                    ["D", "org.b@2.0.0.C"],
                    ["Concept", "concerto@1.0.0.Concept"],
                    ["Asset", "concerto@1.0.0.Asset"],
                    ["Transaction", "concerto@1.0.0.Transaction"],
                    ["Participant", "concerto@1.0.0.Participant"],
                    ["Event", "concerto@1.0.0.Event"],
                ],
                "uriMap": [["org.a@1.0.0.A", "https://a"], ["org.b@2.0.0.B", "https://b"]],
            })
        );
    }

    /// P5-28: a system file has no implicit import, and an unversioned
    /// system namespace gives a `null` version; no `imports` node is none.
    #[test]
    fn staged_header_reads_a_system_file() {
        let header = staged_header_from_parts("concerto", None).unwrap();
        assert_eq!(
            header,
            json!({"namespace": "concerto", "version": null, "system": true, "shortNames": [], "uriMap": []})
        );
        let header = staged_header_from_parts("concerto@1.0.0", Some(&Value::Null)).unwrap();
        assert_eq!(header["version"], json!("1.0.0"));
        assert_eq!(header["shortNames"], json!([]));
    }

    /// P5-28: anything `modelFileFromAstHeader` would throw for, or read
    /// from a shape other than the canonical one, gives no header, so the
    /// view calls that binding over the JS values as before.
    #[test]
    fn staged_header_declines_what_the_binding_would_not_simply_set() {
        let one = |imp: Value| staged_header_from_parts("org.x@1.0.0", Some(&json!([imp])));
        assert!(
            staged_header_from_parts("org.x", None).is_none(),
            "unversioned namespace"
        );
        assert!(
            staged_header_from_parts("org.1x@1.0.0", None).is_none(),
            "invalid namespace part"
        );
        assert!(
            staged_header_from_parts("org.x@1.0.0", Some(&json!({}))).is_none(),
            "non-array imports"
        );
        assert!(
            one(
                json!({"$class": "concerto.metamodel@1.0.0.ImportAll", "namespace": "org.a@1.0.0"})
            )
            .is_none()
        );
        assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a", "name": "A"})).is_none());
        assert!(
            one(json!({"$class": "ImportType", "namespace": "org.a@1.0.0", "name": "A"})).is_none()
        );
        assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.a@1.0.0", "types": ["A", 1]})).is_none());
        assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.a@1.0.0"})).is_none());
        assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.a@1.0.0", "types": [], "uri": "u"})).is_none());
        assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.a@1.0.0", "types": ["A"],
            "aliasedTypes": [{"name": "A", "aliasedName": "String"}]})).is_none());
        assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a@1.0.0", "name": "A", "uri": 1})).is_none());
        assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a@1.0.0", "name": "A", "uri": ""})).is_some());
    }

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

    /// A wire `bigint` round-trips through `decode_wire`/`encode_wire`
    /// (task P2-11b-U6): the digit string crosses unchanged in both
    /// directions.
    #[test]
    fn wire_bigint_round_trips() {
        let value: Value = serde_json::from_str(r#"{"@@oracle":"bigint","value":"10"}"#).unwrap();
        assert_eq!(decoded(&value), CoreValue::BigInt("10".to_string()));
        assert_eq!(encode_wire(&CoreValue::BigInt("10".to_string())), value);
    }

    /// P5-02 review (accordproject/concerto-rust#73): a lone (unpaired)
    /// UTF-16 surrogate escape is replaced with the `�` escape, which
    /// `serde_json` accepts, while every other field and a genuine
    /// surrogate *pair* survive untouched.
    #[test]
    fn sanitize_lone_surrogate_escapes_replaces_only_unpaired_ones() {
        let fffd_escape = "\\uFFFD"; // literal 6-char JSON escape, not U+FFFD itself
        // Lone high surrogate: replaced.
        assert_eq!(
            sanitize_lone_surrogate_escapes(r#"{"name":"\ud800"}"#),
            format!(r#"{{"name":"{fffd_escape}"}}"#)
        );
        // Lone low surrogate: replaced.
        assert_eq!(
            sanitize_lone_surrogate_escapes(r#"{"name":"\udc00"}"#),
            format!(r#"{{"name":"{fffd_escape}"}}"#)
        );
        // A valid surrogate pair (U+1F389, a real astral character): left
        // exactly as-is.
        assert_eq!(
            sanitize_lone_surrogate_escapes(r#"{"name":"🎉"}"#),
            r#"{"name":"🎉"}"#
        );
        // High surrogate followed by a non-surrogate escape: replaced, and
        // the following escape is unaffected.
        assert_eq!(
            sanitize_lone_surrogate_escapes(r#"{"name":"\ud800\n"}"#),
            format!(r#"{{"name":"{fffd_escape}\n"}}"#)
        );
        // No escapes at all: unchanged.
        assert_eq!(
            sanitize_lone_surrogate_escapes(r#"{"a":"b","n":1}"#),
            r#"{"a":"b","n":1}"#
        );
    }

    /// The sanitized text always reparses, and a lone surrogate's field
    /// becomes the literal U+FFFD character rather than vanishing.
    #[test]
    fn sanitize_lone_surrogate_escapes_output_is_valid_json() {
        let sanitized = sanitize_lone_surrogate_escapes(r#"{"name":"\ud800","ok":true}"#);
        let value: Value = serde_json::from_str(&sanitized).expect("sanitized text must parse");
        assert_eq!(
            value["name"],
            json!(char::from_u32(0xFFFD).unwrap().to_string())
        );
        assert_eq!(value["ok"], json!(true));
    }

    /// The wire documents `parse_wire`/`WireOut` are checked on: every
    /// kind, nested, plus plain JSON, a tag that is not a string, and a
    /// duplicate key.
    const WIRE_SAMPLES: &[&str] = &[
        r#"null"#,
        r#"true"#,
        r#"[1,-2,3.5,18446744073709551615,1e300,-0.0,0]"#,
        r#""a \"string\" with é and 🎉""#,
        r#"{"b":1,"a":[{"x":null}],"b":2}"#,
        r#"{"@@oracle":1,"x":"y"}"#,
        r#"{"@@oracle":"undefined"}"#,
        r#"[{"@@oracle":"number","value":"NaN"},{"@@oracle":"number","value":"-Infinity"},{"@@oracle":"number","value":"Infinity"},{"@@oracle":"number","value":"-0"}]"#,
        r#"{"@@oracle":"bigint","value":"123456789012345678901234567890"}"#,
        r#"{"@@oracle":"map","entries":[["k",{"@@oracle":"undefined"}],[{"@@oracle":"number","value":"NaN"},[1,2]]]}"#,
        r#"[{"@@oracle":"dayjs","valid":false},{"@@oracle":"dayjs","valid":true,"ms":1700000000123,"utcOffset":120},{"@@oracle":"dayjs","valid":true,"ms":0}]"#,
        r#"{"@@oracle":"typed","ctor":"ValidatedResource","fqn":"org.acme@1.0.0.Item","fields":{"$namespace":"org.acme@1.0.0","$type":"Item","$identifierFieldName":"id","$identifier":"i1","id":"i1","$timestamp":null,"n":{"@@oracle":"number","value":"NaN"},"child":{"@@oracle":"typed","ctor":"Relationship","fqn":"org.acme@1.0.0.Other","fields":{"$class":"org.acme@1.0.0.Other"}},"labels":["a","b"]}}"#,
        r#"{"@@oracle":"typed","ctor":"Resource","fqn":"org.acme@1.0.0.Item","fields":{}}"#,
    ];

    /// P5-16: `parse_wire` reads every sample as `decode_wire` does, and
    /// `WireOut` writes each decoded value as `encode_wire` plus `snapshot`
    /// do, byte for byte.
    #[test]
    fn parse_wire_and_wire_out_match_the_value_route() {
        for text in WIRE_SAMPLES {
            let value: Value = serde_json::from_str(text).unwrap();
            let expected = decoded(&value);
            let parsed = match parse_wire(text) {
                Ok(v) => v,
                Err(_) => panic!("parse_wire failed on {text}"),
            };
            let via_value = serde_json::to_string(&encode_wire(&expected)).unwrap();
            assert_eq!(
                serde_json::to_string(&encode_wire(&parsed)).unwrap(),
                via_value,
                "{text}"
            );
            assert_eq!(
                serde_json::to_string(&WireOut::<false>(&parsed)).unwrap(),
                via_value,
                "{text}"
            );
            if let CoreValue::Instance(instance) = &parsed {
                assert_eq!(
                    serde_json::to_string(&WireInstanceOut::<false>(instance)).unwrap(),
                    serde_json::to_string(&encode_wire_instance(instance)).unwrap(),
                    "{text}"
                );
            }
        }
    }

    /// P5-16: a wire shape the codec does not recognise fails `parse_wire`
    /// as it fails `decode_wire`, anywhere in the document.
    #[test]
    fn parse_wire_rejects_what_decode_wire_rejects() {
        for text in [
            r#"{"@@oracle":"bogus"}"#,
            r#"[1,{"@@oracle":"number","value":"1"}]"#,
            r#"{"@@oracle":"number"}"#,
            r#"{"@@oracle":"bigint","value":1}"#,
            r#"{"@@oracle":"map"}"#,
            r#"{"@@oracle":"map","entries":[1]}"#,
            r#"{"@@oracle":"map","entries":[[]]}"#,
            r#"{"@@oracle":"map","entries":[["k"]]}"#,
            r#"{"@@oracle":"dayjs","valid":true}"#,
            r#"{"@@oracle":"typed","ctor":"Other","fqn":"a.B","fields":{}}"#,
            r#"{"@@oracle":"typed","ctor":"Resource","fields":{}}"#,
            r#"{"@@oracle":"typed","ctor":"Resource","fqn":"a.B"}"#,
            r#"{"x":{"a":[{"@@oracle":"typed","ctor":"Resource","fqn":"a.B","fields":{"y":{"@@oracle":"nope"}}}]}}"#,
        ] {
            let value: Value = serde_json::from_str(text).unwrap();
            assert!(decode_wire(&value).is_err(), "decode_wire accepted {text}");
            assert!(parse_wire(text).is_err(), "parse_wire accepted {text}");
        }
    }

    /// P5-16: `CompactInstanceOut` writes the header values by position and
    /// the other fields in order, less the ones the view skips.
    #[test]
    fn compact_instance_out_moves_the_header_out_of_the_fields() {
        let text = r#"{"@@oracle":"typed","ctor":"ValidatedResource","fqn":"org.acme@1.0.0.Item","fields":{"$namespace":"org.acme@1.0.0","$type":"Item","$identifierFieldName":"id","$identifier":"i1","id":"i1","$timestamp":{"@@oracle":"dayjs","valid":true,"ms":5,"utcOffset":0},"n":{"@@oracle":"number","value":"NaN"},"$class":"x","child":{"@@oracle":"typed","ctor":"Relationship","fqn":"org.acme@1.0.0.Other","fields":{"$class":"org.acme@1.0.0.Other"}},"labels":["a","b"]}}"#;
        let CoreValue::Instance(instance) = parse_wire(text).ok().unwrap() else {
            panic!("not an instance");
        };
        assert_eq!(
            serde_json::to_string(&CompactInstanceOut(&instance)).unwrap(),
            r#"["ValidatedResource","org.acme@1.0.0.Item","org.acme@1.0.0","Item","id","i1",{"@@oracle":"dayjs","valid":true,"ms":5.0,"utcOffset":0.0},{"n":{"@@oracle":"number","value":"NaN"},"child":{"@@oracle":"typed","ctor":"Relationship","fqn":"org.acme@1.0.0.Other","fields":{"$class":"org.acme@1.0.0.Other"}},"labels":["a","b"]}]"#
        );
        // A missing header value is written as `undefined`, and with no
        // string `$identifierFieldName` no other key is dropped.
        let text = r#"{"@@oracle":"typed","ctor":"Resource","fqn":"a.B","fields":{"$identifierFieldName":1,"1":true}}"#;
        let CoreValue::Instance(instance) = parse_wire(text).ok().unwrap() else {
            panic!("not an instance");
        };
        let undefined = r#"{"@@oracle":"undefined"}"#;
        assert_eq!(
            serde_json::to_string(&CompactInstanceOut(&instance)).unwrap(),
            format!(
                r#"["Resource","a.B",{undefined},{undefined},1,{undefined},{undefined},{{"1":true}}]"#
            )
        );
    }

    /// P5-16: `WireOut::<true>` writes an integral number as an integer
    /// literal, which reads back as the same double, and every other number
    /// exactly as `WireOut::<false>` does.
    #[test]
    fn wire_out_ints_reads_back_the_same_numbers() {
        for n in [
            0.0,
            1.0,
            -1.0,
            42.0,
            1.5,
            -3.25,
            9_007_199_254_740_991.0,
            -9_007_199_254_740_991.0,
            9_007_199_254_740_992.0,
            1e21,
            1e-7,
            5e-324,
            f64::MAX,
        ] {
            let value = CoreValue::Number(n);
            let ints = serde_json::to_string(&WireOut::<true>(&value)).unwrap();
            let plain = serde_json::to_string(&WireOut::<false>(&value)).unwrap();
            let back: f64 = serde_json::from_str(&ints).unwrap();
            assert_eq!(back.to_bits(), n.to_bits(), "{ints}");
            if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
                assert!(!ints.contains('.') && !ints.contains('e'), "{ints}");
            } else {
                assert_eq!(ints, plain);
            }
        }
    }

    /// A model AST for [`staged_header`] with these imports.
    fn header_model(namespace: &str, imports: Value) -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": namespace,
            "imports": imports,
            "declarations": [],
        })
    }

    /// The implicit system import's `importShortNames` entries.
    fn implicit_short_names() -> Vec<Value> {
        IMPLICIT_SYSTEM_TYPES
            .iter()
            .flat_map(|t| [json!(t), json!(format!("concerto@1.0.0.{t}"))])
            .collect()
    }

    #[test]
    fn staged_header_has_the_version_and_the_implicit_import() {
        let header = staged_header(&header_model("org.acme@1.2.3", json!([]))).unwrap();
        assert_eq!(header, json!(["1.2.3", implicit_short_names(), []]));
        // An absent `imports` is read as none, as `ast.imports.concat` is skipped.
        let mut ast = header_model("org.acme@1.2.3", json!(null));
        ast.as_object_mut().unwrap().remove("imports");
        assert_eq!(staged_header(&ast).unwrap(), header);
    }

    #[test]
    fn staged_header_maps_imports_in_order_with_aliases_and_uris() {
        let imports = json!([
            {
                "$class": "concerto.metamodel@1.0.0.ImportTypes",
                "namespace": "org.other@1.0.0",
                "types": ["A", "B"],
                "aliasedTypes": [
                    {"$class": "concerto.metamodel@1.0.0.AliasedType", "name": "B", "aliasedName": "Bee"}
                ],
                "uri": "https://example.com/other.cto",
            },
            {
                "$class": "concerto.metamodel@1.0.0.ImportType",
                "namespace": "org.third@2.0.0",
                "name": "C",
                "uri": "https://example.com/third.cto",
            },
            {
                "$class": "concerto.metamodel@1.0.0.ImportTypes",
                "namespace": "org.fourth@1.0.0",
                "types": ["D"],
                "aliasedTypes": [],
                "uri": "",
            },
        ]);
        let header = staged_header(&header_model("org.acme@1.0.0", imports)).unwrap();
        let mut short_names = vec![
            json!("A"),
            json!("org.other@1.0.0.A"),
            json!("Bee"),
            json!("org.other@1.0.0.B"),
            json!("C"),
            json!("org.third@2.0.0.C"),
            json!("D"),
            json!("org.fourth@1.0.0.D"),
        ];
        short_names.extend(implicit_short_names());
        assert_eq!(
            header,
            json!([
                "1.0.0",
                short_names,
                [
                    "org.other@1.0.0.A",
                    "https://example.com/other.cto",
                    "org.third@2.0.0.C",
                    "https://example.com/third.cto",
                ],
            ])
        );
    }

    #[test]
    fn staged_header_leaves_every_other_case_to_the_binding() {
        let import = |value: Value| header_model("org.acme@1.0.0", json!([value]));
        let cases = [
            // System and unversioned or invalid namespaces.
            header_model("concerto@1.0.0", json!([])),
            header_model("concerto", json!([])),
            header_model("org.acme", json!([])),
            header_model("org.1acme@1.0.0", json!([])),
            header_model("org.acme@x", json!([])),
            json!({"$class": "concerto.metamodel@1.0.0.Model", "namespace": 1}),
            // Imports the binding rejects.
            import(json!({
                "$class": "concerto.metamodel@1.0.0.ImportType",
                "namespace": "org.other",
                "name": "A",
            })),
            import(json!({
                "$class": "concerto.metamodel@1.0.0.ImportAll",
                "namespace": "org.other@1.0.0",
            })),
            import(json!({
                "$class": "concerto.metamodel@1.0.0.ImportTypes",
                "namespace": "org.other@1.0.0",
                "types": ["A"],
                "aliasedTypes": [{"name": "A", "aliasedName": "String"}],
            })),
            // Values of another JSON type than the plain case reads.
            header_model("org.acme@1.0.0", json!({})),
            import(json!({
                "$class": "concerto.metamodel@1.0.0.ImportTypes",
                "namespace": "org.other@1.0.0",
                "types": [1],
            })),
            import(json!({
                "$class": "concerto.metamodel@1.0.0.ImportType",
                "namespace": "org.other@1.0.0",
                "name": "A",
                "uri": 1,
            })),
            import(json!({
                "$class": "concerto.metamodel@1.0.0.ImportTypes",
                "namespace": "org.other@1.0.0",
                "types": ["A"],
                "aliasedTypes": [{"name": "A", "aliasedName": null}],
            })),
        ];
        for ast in cases {
            assert_eq!(staged_header(&ast), None, "{ast}");
        }
    }

    /// P5-73 (accordproject/concerto-rust#414): the two fixed system model
    /// texts get the header a load of them gives, and any other text, even the same model with other whitespace, key
    /// order or a malformed node, gets none, so it is loaded and checked.
    #[test]
    fn system_model_header_is_only_for_the_exact_system_texts() {
        let texts = concerto_core::rootmodel::system_model_json_texts();
        for (file_name, text) in texts {
            let (file, imports) =
                ModelFile::from_json_text_with_imports(text, None, Some(file_name.into()))
                    .unwrap()
                    .unwrap();
            let expected = staged_header_from_parts(file.namespace(), imports.as_ref()).unwrap();
            let header = system_model_header(text).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&header).unwrap(), expected);

            let mut value: Value = serde_json::from_str(text).unwrap();
            let pretty = serde_json::to_string_pretty(&value).unwrap();
            assert_eq!(system_model_header(&pretty), None);
            value["decorators"] = json!("x");
            assert_eq!(system_model_header(&value.to_string()), None);
        }
        assert_eq!(system_model_header(""), None);
        assert_eq!(system_model_header(&format!("{} ", texts[1].1)), None);
    }
}

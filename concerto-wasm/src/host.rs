//! The host functions the shim registers, and the error mapping (PORTING.md 2.3).
//!
//! Split out of `lib.rs` (P5-104, review M7); the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// Host functions and errors
// ---------------------------------------------------------------------------

/// The JS functions the shim registers at load.
pub(crate) struct Host {
    /// `(payload) => Error`: builds the TS exception for an error payload.
    error_factory: Function,
}

/// Registers the error factory. The shim calls it once, right after loading
/// the module.
#[wasm_bindgen(js_name = setHost)]
pub fn set_host(error_factory: Function) {
    caches::HOST.with(|h| {
        *h.borrow_mut() = Some(Host { error_factory });
    });
}

/// What a binding can fail with: a JS exception raised by a callback (passed
/// through unchanged), or a core error to map.
///
/// The rule for errors raised here rather than by the error factory
/// (P5-104, D-8): malformed JSON text is the JS `SyntaxError` `JSON.parse`
/// throws ([`json_syntax`]); bytes not in the encoding a binding reads,
/// which the TS side never writes, are a bare JS `TypeError` ([`utf8_text`],
/// [`compact_layout_error`]); and a failure no input can cause is a plain
/// JS `Error` ([`internal`]). Everything else goes through [`throw`].
pub(crate) enum Error {
    Js(JsValue),
    Contract(Box<ContractError>),
    /// A core error about an instance (P5-89, accordproject/concerto#1325),
    /// with its diagnostics as the JSON array the payload carries as
    /// `details` ([`diagnostics_json`]).
    Instance(Box<ContractError>, Value),
    /// P5-101 (E-11, accordproject/concerto-rust#455): a value the
    /// serializer fast path's wire codec cannot carry ([`wire_error`]): the
    /// payload says so (`fastPathUnsupported`), so the TS side falls back
    /// to its visitor path on that flag rather than on the message text.
    Unsupported(Box<ContractError>),
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

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// What a binding returns to JS: its value, or the exception to throw
/// (P5-104, D-8).
pub(crate) type JsResult<T> = std::result::Result<T, JsValue>;

/// The JS `SyntaxError` that `JSON.parse` throws for malformed JSON text
/// (P5-104, D-8).
pub(crate) fn json_syntax(e: serde_json::Error) -> Error {
    Error::Js(js_sys::SyntaxError::new(&e.to_string()).into())
}

/// JSON text a binding was given, parsed; malformed text is the
/// [`json_syntax`] error (P5-104, D-8).
pub(crate) fn parse_json(text: &str) -> Result<Value> {
    serde_json::from_str(text).map_err(json_syntax)
}

/// A plain JS `Error` for a failure no input can cause, such as serializing
/// a value serde built (P5-104, D-8).
pub(crate) fn internal(e: impl std::fmt::Display) -> Error {
    Error::Js(js_sys::Error::new(&e.to_string()).into())
}

pub(crate) fn kind_name(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::IllegalModel => "IllegalModel",
        ErrorKind::TypeNotFound => "TypeNotFound",
        ErrorKind::Validation => "Validation",
        ErrorKind::InvalidArgument => "Error",
        ErrorKind::MalformedInput => "JsTypeError",
        ErrorKind::Metamodel => "Metamodel",
        // `ErrorKind` is `#[non_exhaustive]`; a new kind is a plain `Error`
        // until the shim learns it. So are `Validator`, which no check has
        // raised since BC-39, and `RecursionLimit`, raised by none since
        // BC-11 reports a circular super type chain as an IllegalModel
        // error (P5-103 removed their shim entries).
        _ => "Error",
    }
}

pub(crate) fn set(target: &Object, key: &str, value: &JsValue) {
    // Setting a data property on a fresh plain object cannot fail.
    let _ = Reflect::set(target, &JsValue::from_str(key), value);
}

/// The JS value of a JSON value (`JSON.parse` of its text).
pub(crate) fn to_js(value: &Value) -> JsValue {
    serde_json::to_string(value)
        .ok()
        .and_then(|text| JSON::parse(&text).ok())
        .unwrap_or(JsValue::NULL)
}

/// Turns an error into the JS value to throw. `model_file` is the JS model
/// file TS passes to an `IllegalModelException`, when the core error says TS
/// passes one.
pub(crate) fn throw(err: Error, model_file: Option<&JsValue>) -> JsValue {
    let unsupported = matches!(err, Error::Unsupported(_));
    let (err, details) = match err {
        Error::Js(value) => return value,
        Error::Contract(err) => (err, None),
        Error::Instance(err, details) => (err, Some(details)),
        Error::Unsupported(err) => (err, None),
    };
    let payload = Object::new();
    if unsupported {
        set(&payload, "fastPathUnsupported", &JsValue::TRUE);
    }
    if let Some(details) = &details {
        set(&payload, "details", &to_js(details));
    }
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
    caches::HOST.with(|h| match h.borrow().as_ref() {
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
/// `this` (modelfile.ts). With no `namespace`, it is [`throw`] naming no
/// file.
pub(crate) fn throw_naming_file(
    err: Error,
    model_files: &JsValue,
    namespace: Option<&str>,
) -> JsValue {
    let names_file = matches!(&err, Error::Contract(c) if matches!(c.model_file, Some(Some(_))));
    let model_file = match namespace {
        Some(namespace) if names_file && model_files.is_object() => {
            Reflect::get(model_files, &JsValue::from_str(namespace)).ok()
        }
        _ => None,
    };
    throw(err, model_file.as_ref().filter(|mf| !nullish(mf)))
}

/// P5-76: text a binding was given as UTF-8 bytes (a JS `TextEncoder`'s
/// output); a `TypeError` for bytes that are not UTF-8, which a
/// `TextEncoder` never writes.
pub(crate) fn utf8_text(bytes: &[u8]) -> JsResult<&str> {
    std::str::from_utf8(bytes)
        .map_err(|e| js_sys::TypeError::new(&format!("the text is not UTF-8: {e}")).into())
}

/// P5-92: the error for an AST in the compact binary layout whose bytes are
/// not in that layout (`stageModelFileBytes` with [`STAGE_COMPACT`]), which the TS side
/// never writes: a `TypeError`, as for bytes that are not UTF-8
/// ([`utf8_text`]).
pub(crate) fn compact_layout_error(e: serde_json::Error) -> Error {
    Error::Js(js_sys::TypeError::new(&e.to_string()).into())
}

/// Runs a binding body and maps its error: the one way a binding turns a
/// [`Result`] into a [`JsResult`] (P5-104, D-8), with [`run_naming`].
pub(crate) fn run<T>(body: impl FnOnce() -> Result<T>) -> JsResult<T> {
    body().map_err(|e| throw(e, None))
}

/// [`run`] for a binding whose error may name a JS model file:
/// `model_file` is read only when the body fails, and attached as
/// [`throw`] attaches it.
pub(crate) fn run_naming<T>(
    model_file: impl FnOnce() -> JsValue,
    body: impl FnOnce() -> Result<T>,
) -> JsResult<T> {
    body().map_err(|e| throw(e, Some(&model_file())))
}

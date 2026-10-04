//! Error types for `concerto-core` (PORTING.md section 2: the error contract).
//!
//! [`Error`] is the one error type of the crate: an opaque value with
//! accessors for its [`ErrorKind`], its stable catalogue [`code`](Error::code),
//! its message params, its [`Location`] in the model and its structured
//! [`Detail`]s (docs/public-api.md section 5.6). [`Result`] defaults to it.
//!
//! Behind it is the `{kind, code, params, location}` shape every error is
//! built from (section 2.1). `kind` selects the TS exception class the shim
//! throws (section 2.3); `code` is a key into the message catalogue, the
//! verbatim port of `messages/en.json` and the reference's inline templates
//! (section 2.2). A message with no catalogue entry uses the `"pre-port"`
//! entry, which claims no verbatim TS message.
//!
//! That shape (`ContractError`), the catalogue and the TS-specific parts of
//! the contract are the JS binding's: they are public only with the
//! `js-compat` feature.

mod catalogue;

use std::fmt::Write as _;

#[cfg(not(feature = "js-compat"))]
use catalogue::catalogue_entry;
/// The message catalogue and its lookup (PORTING.md section 2.2).
#[cfg(feature = "js-compat")]
pub use catalogue::{CATALOGUE, catalogue_entry};

/// Shorthand `Result` used all over `concerto-core`, with [`Error`] as its
/// default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The error type of `concerto-core`: a model that cannot be loaded, a type
/// that cannot be resolved, a model or an instance that fails validation.
///
/// Opaque: [`kind`](Error::kind) is the failure class (one TS exception
/// class each) and [`code`](Error::code) the stable catalogue key. The
/// message wording carries no stability promise (docs/public-api.md 2).
#[derive(Debug, Clone, PartialEq)]
pub struct Error(Box<ContractError>);

impl Error {
    /// The class of failure.
    pub fn kind(&self) -> ErrorKind {
        self.0.kind
    }

    /// The catalogue key of the message: a stable identifier, safe to match
    /// on. A message with no catalogue entry has the code `"pre-port"`.
    pub fn code(&self) -> &'static str {
        self.0.code
    }

    /// The params the message is rendered with, in template order. Each value
    /// is the string TS interpolates.
    pub fn params(&self) -> &[(&'static str, String)] {
        &self.0.params
    }

    /// Where in the model the problem is, when the error has a location that
    /// is a well-formed `concerto.metamodel@1.0.0.Range`.
    pub fn location(&self) -> Option<Location> {
        self.0.location.as_ref().and_then(Location::from_value)
    }

    /// The name of the model file the error is about, when it names one.
    pub fn file_name(&self) -> Option<&str> {
        self.0.model_file.as_ref().and_then(|name| name.as_deref())
    }

    /// The structured violations of a strict-option rejection
    /// (accordproject/concerto#1273); empty for every other error.
    pub fn details(&self) -> &[Detail] {
        &self.0.details
    }

    js_compat_pub! {
        /// The contract shape behind this error (PORTING.md section 2.1),
        /// which the JS binding hands to the TS error factory.
        pub fn contract(&self) -> &ContractError {
            &self.0
        }
    }

    /// [`Error::contract`], by value.
    #[cfg(feature = "js-compat")]
    pub fn into_contract(self) -> ContractError {
        *self.0
    }

    /// The contract shape behind this error, to amend in place.
    pub(crate) fn contract_mut(&mut self) -> &mut ContractError {
        &mut self.0
    }

    /// A catalogue error ([`ContractError::new`]) as the crate's error
    /// type: no location and no model file; [`Error::at`] adds the
    /// location.
    pub(crate) fn new(
        kind: ErrorKind,
        code: &'static str,
        params: Vec<(&'static str, String)>,
    ) -> Self {
        ContractError::new(kind, code, params).into()
    }

    /// This error at `location`: the AST node's `location`, verbatim, or
    /// `None` where TS passes none.
    pub(crate) fn at(mut self, location: Option<crate::json::Value>) -> Self {
        self.0.location = location;
        self
    }

    js_compat_pub! {
        /// A type that could not be resolved: `TypeNotFoundException`'s
        /// default message (`typenotfounderror-defaultmessage`), with
        /// `typeName` set (table 2.3).
        pub fn type_not_found(type_name: impl Into<String>) -> Self {
            ContractError::type_not_found(
                "typenotfounderror-defaultmessage",
                Vec::new(),
                type_name.into(),
                None,
            )
            .into()
        }
    }

    js_compat_pub! {
        /// A model that cannot be loaded, with no catalogue entry:
        /// `"pre-port"`, with `message` verbatim, naming the model file
        /// `file_name` when there is one ([`ContractError::model_file`]).
        pub fn illegal_model(
            message: impl Into<String>,
            file_name: Option<String>,
            location: Option<crate::json::Value>,
        ) -> Self {
            let mut contract =
                ContractError::pre_port(ErrorKind::IllegalModel, message.into(), location);
            contract.model_file = file_name.map(Some);
            contract.into()
        }
    }
}

/// The contract's message ([`ContractError::message`]); its wording carries
/// no stability promise (docs/public-api.md section 2).
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.message())
    }
}

impl std::error::Error for Error {}

impl From<ContractError> for Error {
    fn from(contract: ContractError) -> Self {
        Self(Box::new(contract))
    }
}

/// A position in a model's source: `concerto.metamodel@1.0.0.Position`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Position {
    /// The line, from 1.
    pub line: u64,
    /// The column, from 1.
    pub column: u64,
    /// The offset from the start of the source, from 0.
    pub offset: u64,
}

/// Where in a model's source an error is: `concerto.metamodel@1.0.0.Range`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Location {
    /// Where the node starts.
    pub start: Position,
    /// Where the node ends.
    pub end: Position,
    /// The source the positions refer to, when the AST names one.
    pub source: Option<String>,
}

impl Location {
    /// Reads a `Range` node. `None` when a position is missing or one of its
    /// numbers is not a non-negative integer.
    fn from_value(value: &crate::json::Value) -> Option<Self> {
        fn number(value: &crate::json::Value) -> Option<u64> {
            value.as_u64().or_else(|| {
                value
                    .as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0 && *f < 18_446_744_073_709_551_616.0)
                    .map(|f| f as u64)
            })
        }
        fn position(value: Option<&crate::json::Value>) -> Option<Position> {
            let value = value?;
            Some(Position {
                line: number(value.get("line")?)?,
                column: number(value.get("column")?)?,
                offset: number(value.get("offset")?)?,
            })
        }
        Some(Self {
            start: position(value.get("start"))?,
            end: position(value.get("end"))?,
            source: value
                .get("source")
                .and_then(|source| source.as_str())
                .map(str::to_string),
        })
    }
}

/// The kind of failure an error reports.
///
/// Each kind is one TS exception class (PORTING.md table 2.3), named in the
/// variant's doc comment; the TS class name is available to the JS binding
/// through `ErrorKind::ts_class` (`js-compat`). `ParseException` and
/// `SecurityException` have no Rust throw site and so no kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The model is not valid.
    ///
    /// TS: `IllegalModelException(message, modelFile, location)`.
    IllegalModel,
    /// A type the model refers to is not declared. `params` must include
    /// `typeName` (table 2.3): build these with `ContractError::type_not_found`
    /// or `Error::type_not_found`.
    ///
    /// TS: `TypeNotFoundException(typeName, message)`.
    TypeNotFound,
    /// A value fails a validator declared on a field or scalar (TS 5.0.0
    /// `Validator.reportError`). Nothing raises this kind (BC-39): a validator
    /// error at model load is [`ErrorKind::IllegalModel`], and on an instance
    /// [`ErrorKind::Validation`], each keeping the `errorType` in
    /// [`ContractError::validator`]. Kept for the enum's public shape.
    Validator,
    /// An instance does not conform to its model.
    ///
    /// TS: `ValidationException(message)` (table 2.3), thrown by
    /// `ResourceValidator` (`src/serializer/resourcevalidator.ts`, every
    /// `report*` method).
    Validation,
    /// An argument or input value is not acceptable to the operation.
    ///
    /// TS: a plain `Error(message)`.
    InvalidArgument,
    /// The input has the wrong shape for the operation reading it, such as a
    /// missing node or a value of the wrong kind where the TS code assumes
    /// one.
    ///
    /// TS: a `TypeError(message)` the V8 engine raises in the TS code.
    MalformedInput,
    /// A recursion point with no cycle check went too deep (PORTING.md 2.5).
    ///
    /// TS: a `RangeError(message)` the V8 engine raises (a stack overflow).
    /// Nothing raises this kind: a cyclic inheritance chain is
    /// [`ErrorKind::IllegalModel`] (BC-11). Kept for the enum's public shape.
    RecursionLimit,
    /// A document fails the metamodel check.
    ///
    /// TS: `MetamodelException(message)` (`src/metamodelexception.ts`),
    /// thrown by `BaseModelManager.validateAst`
    /// (`concerto_core::instance::metamodel`).
    Metamodel,
}

impl ErrorKind {
    /// The TS class name the shim throws for this kind, as the oracle
    /// records it in `error.class` (PORTING.md table 2.3). Only for the JS
    /// binding (`js-compat`).
    #[cfg(feature = "js-compat")]
    pub fn ts_class(self) -> &'static str {
        match self {
            ErrorKind::IllegalModel => "IllegalModelException",
            ErrorKind::TypeNotFound => "TypeNotFoundException",
            ErrorKind::Validator => "BaseException",
            ErrorKind::Validation => "ValidationException",
            ErrorKind::InvalidArgument => "Error",
            ErrorKind::MalformedInput => "TypeError",
            ErrorKind::RecursionLimit => "RangeError",
            ErrorKind::Metamodel => "MetamodelException",
        }
    }
}

js_compat_pub! {
    /// How a catalogue template is rendered (PORTING.md section 2.2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Renderer {
        /// `Globalize.formatMessage(key)`: the en.json text, used without params.
        Globalize,
        /// An inline template literal or string concatenation: each `{param}` is
        /// replaced once, and inserted values are never scanned again.
        Inline,
        /// Not a catalogue template: the single `message` param is used
        /// verbatim. Reserved for [`ContractError::pre_port`], whose one
        /// entry (`code = "pre-port"`) needs no golden test of its own name.
        Raw,
    }
}

js_compat_pub! {
    /// One message template.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CatalogueEntry {
        /// The catalogue key (`code`).
        pub code: &'static str,
        /// The template text, byte for byte.
        pub template: &'static str,
        /// Which renderer applies.
        pub renderer: Renderer,
        /// Every throw site that uses the template, in the frozen TS reference.
        pub sources: &'static [&'static str],
    }
}

/// Debug-asserts that a call site's `code` has a catalogue entry: nothing
/// else catches a mistyped code, which [`render`] would show as the
/// message itself.
#[inline]
fn debug_assert_catalogued(code: &str) {
    debug_assert!(
        catalogue_entry(code).is_some(),
        "error code {code:?} has no catalogue entry"
    );
}

/// Renders a template with its params.
fn render(code: &str, params: &[(&'static str, String)]) -> String {
    let Some(entry) = catalogue_entry(code) else {
        // Not reached in a debug build through `ContractError::new` or
        // `type_not_found`, which debug-assert the code; a release build, or
        // a hand-built `ContractError`, falls back to the code itself.
        return code.to_string();
    };
    match entry.renderer {
        // Globalize.messageFormatter (PORTING.md section 2.2): params are
        // substituted in insertion order over the whole message built so
        // far, each globally and with `String.prototype.replace`'s `$`
        // patterns in the value, so a value spelling a later param's
        // placeholder is substituted again, unlike `Inline`.
        Renderer::Globalize => {
            let mut message = entry.template.to_string();
            for (name, value) in params {
                message = globalize_replace_all(&message, name, value);
            }
            message
        }
        Renderer::Raw => params
            .iter()
            .find(|(name, _)| *name == "message")
            .map(|(_, value)| value.clone())
            .unwrap_or_default(),
        Renderer::Inline => {
            // One left-to-right pass: a `{name}` naming a param is replaced by
            // its value, and the value is not scanned again.
            let mut out = String::with_capacity(entry.template.len());
            let mut rest = entry.template;
            while let Some(open) = rest.find('{') {
                out.push_str(&rest[..open]);
                let after = &rest[open + 1..];
                let value = after.find('}').and_then(|close| {
                    let name = &after[..close];
                    params
                        .iter()
                        .find(|(param, _)| *param == name)
                        .map(|(_, value)| (value, close))
                });
                match value {
                    Some((value, close)) => {
                        out.push_str(value);
                        rest = &after[close + 1..];
                    }
                    None => {
                        out.push('{');
                        rest = after;
                    }
                }
            }
            out.push_str(rest);
            out
        }
    }
}

/// Replaces every `{name}` in `message` with `value`, as JS
/// `message.replace(new RegExp('\\{name\\}', 'g'), value)` does: `$`
/// patterns in `value` are resolved per occurrence against `message` as it
/// was before this call (section 2.2).
fn globalize_replace_all(message: &str, name: &str, value: &str) -> String {
    let pattern = format!("{{{name}}}");
    if !message.contains(pattern.as_str()) {
        return message.to_string();
    }
    let mut out = String::with_capacity(message.len());
    let mut last_end = 0;
    for (idx, _) in message.match_indices(pattern.as_str()) {
        out.push_str(&message[last_end..idx]);
        let before = &message[..idx];
        let after = &message[idx + pattern.len()..];
        out.push_str(&substitute_dollar_sequences(value, &pattern, before, after));
        last_end = idx + pattern.len();
    }
    out.push_str(&message[last_end..]);
    out
}

/// The substitution patterns `String.prototype.replace` recognises in a
/// plain-string replacement (no capture groups, since `{name}` has none):
/// `$$` is a literal `$`, `$&` is the matched substring, `` $` `` is the text
/// before the match and `$'` is the text after it. Any other `$x` is left
/// alone.
fn substitute_dollar_sequences(value: &str, matched: &str, before: &str, after: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.peek() {
                Some('$') => {
                    chars.next();
                    out.push('$');
                    continue;
                }
                Some('&') => {
                    chars.next();
                    out.push_str(matched);
                    continue;
                }
                Some('`') => {
                    chars.next();
                    out.push_str(before);
                    continue;
                }
                Some('\'') => {
                    chars.next();
                    out.push_str(after);
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

js_compat_pub! {
    /// What `Validator.reportError` adds to a validator message: the instance
    /// identifier, the fully qualified name of the field or scalar, and the
    /// concerto-util `errorType`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ValidatorReport {
        /// `String(id)`: `"null"` when TS passes `null`.
        pub id: String,
        /// `getFieldOrScalarDeclaration().getFullyQualifiedName()`.
        pub fqn: String,
        /// The concerto-util `ErrorCodes` value.
        pub error_type: &'static str,
    }
}

js_compat_pub! {
    /// An error in the shape of PORTING.md section 2.1.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ContractError {
        /// Selects the TS class.
        pub kind: ErrorKind,
        /// The catalogue key.
        pub code: &'static str,
        /// Each param is the JS `ToString` of what TS interpolates, in template
        /// order.
        pub params: Vec<(&'static str, String)>,
        /// The AST node's `location`, verbatim, or `None` where TS passes none.
        pub location: Option<crate::json::Value>,
        /// `IllegalModel` only: `Some` when TS passes a model file to the
        /// exception, holding that file's name (`modelFile.getName()`, `None`
        /// when it has none). The WASM shim passes the real JS model file instead.
        pub model_file: Option<Option<String>>,
        /// A validator error only (an `IllegalModel` or `Validation` error,
        /// BC-39): what `Validator.reportError` adds.
        pub validator: Option<ValidatorReport>,
        /// `ValidationException.details` (accordproject/concerto#1273): one
        /// entry per violation, for callers that enumerate them. Empty but
        /// for a [`ValidationOptions`](crate::instance::ValidationOptions)
        /// rejection.
        pub details: Vec<Detail>,
    }
}

/// The `code` of a [`Detail`] (accordproject/concerto#1273).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DetailCode {
    /// A key the declaration does not declare, rejected by
    /// `reject_unknown_keys`.
    UnknownProperty,
    /// A required property explicitly set to `null`, rejected by
    /// `reject_required_null`.
    TypeViolation,
}

impl DetailCode {
    /// The code's spelling (`UNKNOWN_PROPERTY`, `TYPE_VIOLATION`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownProperty => "UNKNOWN_PROPERTY",
            Self::TypeViolation => "TYPE_VIOLATION",
        }
    }
}

/// One structured violation in [`Error::details`]:
/// `{ path, code, expected, actual }`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Detail {
    /// The JSON path of the offending value (`$.declarations[0].name`).
    pub path: String,
    /// What kind of violation it is.
    pub code: DetailCode,
    /// The type the model expects there, when it expects one.
    pub expected: Option<String>,
    /// What the document holds there, when the violation is about it.
    pub actual: Option<String>,
}

impl ContractError {
    /// An error with no location and no model file.
    ///
    /// `code` must be a catalogue key ([`catalogue_entry`]); a debug build
    /// asserts it.
    pub fn new(kind: ErrorKind, code: &'static str, params: Vec<(&'static str, String)>) -> Self {
        debug_assert_catalogued(code);
        Self {
            kind,
            code,
            params,
            location: None,
            model_file: None,
            validator: None,
            details: Vec::new(),
        }
    }

    /// `ErrorKind::TypeNotFound`, from the catalogue: `params` gets the
    /// template's own params, plus `typeName` (table 2.3), the value
    /// `TypeNotFoundException`'s constructor stores separately from the
    /// rendered message.
    pub fn type_not_found(
        code: &'static str,
        mut params: Vec<(&'static str, String)>,
        type_name: String,
        location: Option<crate::json::Value>,
    ) -> Self {
        debug_assert_catalogued(code);
        params.push(("typeName", type_name));
        Self {
            kind: ErrorKind::TypeNotFound,
            code,
            params,
            location,
            model_file: None,
            validator: None,
            details: Vec::new(),
        }
    }

    /// A message with no catalogue entry (PORTING.md section 7.2): `message`
    /// is used verbatim, through [`Renderer::Raw`], claiming no verbatim TS
    /// template; the completeness test needs a golden test only for the
    /// shared `"pre-port"` entry.
    pub fn pre_port(
        kind: ErrorKind,
        message: String,
        location: Option<crate::json::Value>,
    ) -> Self {
        Self {
            kind,
            code: "pre-port",
            params: vec![("message", message)],
            location,
            model_file: None,
            validator: None,
            details: Vec::new(),
        }
    }

    /// The raw message: the rendered template, before the exception class
    /// decorates it. For `Validator` it is the whole `reportError` message,
    /// since `Validator.reportError` builds it before constructing the
    /// exception.
    pub fn message(&self) -> String {
        let message = render(self.code, &self.params);
        match &self.validator {
            Some(report) => render(
                "validator-reporterror",
                &[
                    ("id", report.id.clone()),
                    ("fqn", report.fqn.clone()),
                    ("msg", message),
                ],
            ),
            None => message,
        }
    }

    /// The message the TS exception ends up with, after its constructor has
    /// decorated it. Used by the native oracle harness only; the WASM shim
    /// hands [`ContractError::message`] to the real TS constructor.
    pub fn final_message(&self) -> String {
        let message = self.message();
        match self.kind {
            ErrorKind::IllegalModel => {
                // TS: IllegalModelException constructor
                // (src/introspect/illegalmodelexception.ts).
                let mut suffix = String::new();
                if let Some(Some(name)) = &self.model_file
                    && !name.is_empty()
                {
                    suffix = format!("File '{name}': ");
                }
                if let Some(location) = &self.location
                    && crate::ecma::is_truthy(location)
                {
                    let at = |pointer: &str| {
                        location
                            .pointer(pointer)
                            .map_or_else(|| "undefined".to_string(), crate::ecma::to_js_string)
                    };
                    // Writing to a `String` cannot fail.
                    let _ = write!(
                        suffix,
                        "line {} column {}, to line {} column {}. ",
                        at("/start/line"),
                        at("/start/column"),
                        at("/end/line"),
                        at("/end/column")
                    );
                }
                format!(
                    "{message} {}",
                    crate::model_util::capitalize_first_letter(&suffix)
                )
            }
            ErrorKind::TypeNotFound
            | ErrorKind::Validator
            | ErrorKind::Validation
            | ErrorKind::InvalidArgument
            | ErrorKind::MalformedInput
            | ErrorKind::RecursionLimit
            // TS: `MetamodelException` (src/metamodelexception.ts) is a bare
            // `BaseException(message)`: no suffix, no location.
            | ErrorKind::Metamodel => message,
        }
    }

    /// The `component` the oracle records for this error.
    #[cfg(feature = "js-compat")]
    pub fn component(&self) -> Option<&'static str> {
        match self.kind {
            ErrorKind::IllegalModel | ErrorKind::TypeNotFound => {
                Some("@accordproject/concerto-core")
            }
            // TS: `ValidationException` and `MetamodelException` extend
            // concerto-util's `BaseException` with no explicit `component`,
            // so its default (`@accordproject/concerto-util`) applies (table
            // 2.3).
            ErrorKind::Validator | ErrorKind::Validation | ErrorKind::Metamodel => {
                Some("@accordproject/concerto-util")
            }
            ErrorKind::InvalidArgument | ErrorKind::MalformedInput | ErrorKind::RecursionLimit => {
                None
            }
        }
    }
}

/// A typed AST `location` (`mm::Range`) re-serialised as the JSON value TS
/// holds for it, for a [`ContractError::location`] (PORTING.md 2.1). This is
/// not a verbatim copy of the AST's JSON, as `ScalarDeclaration::process`
/// makes: the typed `Range` is written back out, so any field it does not
/// model is dropped.
///
/// The metamodel's number fields are `f64`, so a `Range` serialised back
/// gives `3.0` where the AST said `3`; `JSON.stringify` writes `3`. Each
/// integral number that fits an `i64` or `u64` is written as a JSON integer
/// of its exact value (`-0` as `0`). Above 2^53 JS may print different
/// digits for the same value; a non-integral number, or one of 2^64 or
/// more, keeps its float form. No real source position comes near 2^53.
pub(crate) fn location_value(
    range: &concerto_metamodel::concerto_metamodel_1_0_0::Range,
) -> Option<crate::json::Value> {
    fn js_numbers(value: crate::json::Value) -> crate::json::Value {
        use crate::json::Value;
        // 2^63 and 2^64, both exact in f64.
        const TWO_63: f64 = 9_223_372_036_854_775_808.0;
        const TWO_64: f64 = 18_446_744_073_709_551_616.0;
        match value {
            Value::Number(n) if n.is_f64() => match n.as_f64() {
                Some(f) if f.fract() == 0.0 && (-TWO_63..TWO_63).contains(&f) => {
                    Value::from(f as i64)
                }
                Some(f) if f.fract() == 0.0 && (0.0..TWO_64).contains(&f) => Value::from(f as u64),
                _ => Value::Number(n),
            },
            Value::Array(items) => Value::Array(items.into_iter().map(js_numbers).collect()),
            Value::Object(map) => {
                Value::Object(map.into_iter().map(|(k, v)| (k, js_numbers(v))).collect())
            }
            other => other,
        }
    }
    crate::json::to_value(range).ok().map(js_numbers)
}

#[cfg(test)]
mod tests;

//! Error types for `concerto-core` (PORTING.md section 2: the error contract).
//!
//! [`ConcertoError`] covers the hard failures that stop a model from being
//! used: a type that cannot be resolved, or model JSON that does not satisfy
//! the metamodel. Each variant carries enough context to report what went
//! wrong and, where known, where.
//!
//! [`ContractError`] is the `{kind, code, params, location}` shape every
//! ported member builds its errors from (section 2.1). `kind` selects the TS
//! exception class the shim throws (section 2.3); `code` is a key into the
//! [`catalogue`] module, the verbatim port of `messages/en.json` and the
//! inline templates the reference throws (section 2.2), scoped to the keys
//! OD-5 lists. A call site that has not yet been faithfully ported from TS
//! builds its `ContractError` with [`ContractError::pre_port`] instead of a
//! catalogue code, so that it compiles against this contract without
//! claiming a verbatim TS message it does not have; the unit that later
//! ports that member (named in its doc comment) replaces the call with a
//! real catalogue entry and its golden test (section 7.2).

mod catalogue;

pub use catalogue::{CATALOGUE, catalogue_entry};

use thiserror::Error;

/// Shorthand `Result` used all over `concerto-core`.
pub type Result<T> = std::result::Result<T, ConcertoError>;

/// A hard failure raised while loading a model or resolving a type.
#[derive(Debug, Error)]
pub enum ConcertoError {
    /// A fully-qualified type could not be resolved in any loaded model.
    #[error("type not found: {type_name}")]
    TypeNotFound {
        /// The name that failed to resolve (qualified or short).
        type_name: String,
    },

    /// The model JSON is malformed or violates a metamodel rule.
    #[error("illegal model: {message}")]
    IllegalModel {
        /// A description of the problem.
        message: String,
        /// The originating file, if known.
        file_name: Option<String>,
        /// The AST node's `location` (`concerto.metamodel@1.0.0.Range`),
        /// copied verbatim, exactly as [`ContractError::location`] is
        /// (PORTING.md section 2.1). `None` where the check has no AST node
        /// in scope, or where TS itself passes none.
        location: Option<serde_json::Value>,
    },

    /// An error in the `{kind, code, params, location}` shape of PORTING.md
    /// section 2, raised by a ported member. The message is the TS message,
    /// byte for byte.
    #[error("{}", .0.message())]
    Contract(Box<ContractError>),
}

impl From<ContractError> for ConcertoError {
    fn from(err: ContractError) -> Self {
        Self::Contract(Box::new(err))
    }
}

/// Selects the TS class the shim throws (PORTING.md table 2.3). Only the
/// kinds a ported (or minimally adapted, section 7.2) unit raises exist so
/// far; `ParseException` and `SecurityException` have no Rust throw site
/// (2.3) and so no kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// `IllegalModelException(message, modelFile, location)`.
    IllegalModel,
    /// `TypeNotFoundException(typeName, message)`. `params` must include
    /// `typeName` (table 2.3); build these with [`ContractError::type_not_found`].
    TypeNotFound,
    /// concerto-util `BaseException(message, undefined, errorType)`, thrown by
    /// `Validator.reportError`.
    Validator,
    /// A plain JS `Error(message)`.
    Error,
    /// A JS `TypeError(message)` the V8 engine raises in the TS code.
    JsTypeError,
}

impl ErrorKind {
    /// The TS class name the shim throws for this kind, as the oracle records
    /// it in `error.class`.
    pub fn ts_class(self) -> &'static str {
        match self {
            Self::IllegalModel => "IllegalModelException",
            Self::TypeNotFound => "TypeNotFoundException",
            Self::Validator => "BaseException",
            Self::Error => "Error",
            Self::JsTypeError => "TypeError",
        }
    }
}

/// How a catalogue template is rendered (PORTING.md section 2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    /// `Globalize.formatMessage(key)`: the en.json text, used without params.
    Globalize,
    /// An inline template literal or string concatenation: each `{param}` is
    /// replaced once, and inserted values are never scanned again.
    Inline,
    /// Not a catalogue template at all: the single `message` param is used
    /// verbatim. Reserved for [`ContractError::pre_port`]; never cite this
    /// renderer as a faithful TS port (section 2.2), and its one entry
    /// (`code = "pre-port"`) is exempt from the OD-5 completeness test.
    Raw,
}

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

/// Renders a template with its params.
fn render(code: &str, params: &[(&'static str, String)]) -> String {
    let Some(entry) = catalogue_entry(code) else {
        // Unreachable for errors built through `ContractError::new`, which
        // only takes codes from the catalogue (see the completeness test).
        return code.to_string();
    };
    match entry.renderer {
        // Globalize.messageFormatter (globalize.ts), ported faithfully
        // (PORTING.md section 2.2): params are substituted in insertion
        // order, one param after another over the whole message built so
        // far, each substitution global (every `{name}` occurrence, not just
        // the first) and following `String.prototype.replace`'s special
        // patterns in the *value* (`$$`, `$&`, `` $` ``, `$'`). This means a
        // value inserted by an earlier param that happens to spell a later
        // param's placeholder is substituted again — unlike `Inline`, which
        // never rescans.
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

/// Replaces every `{name}` in `message` with `value`, JS
/// `message.replace(new RegExp('\\{name\\}', 'g'), value)` style: `$`/`\``
/// and the match position in `value` are resolved per occurrence, against
/// `message` as it was *before this call* (JS computes `` $` `` and `$'` from
/// the string the `.replace` call runs over, not from the output being
/// built), matching how `Globalize` calls `String.prototype.replace` once
/// per param (section 2.2).
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
    pub location: Option<serde_json::Value>,
    /// `IllegalModel` only: `Some` when TS passes a model file to the
    /// exception, holding that file's name (`modelFile.getName()`, `None`
    /// when it has none). The WASM shim passes the real JS model file instead.
    pub model_file: Option<Option<String>>,
    /// `Validator` only: what `Validator.reportError` adds.
    pub validator: Option<ValidatorReport>,
}

impl ContractError {
    /// An error with no location and no model file.
    pub fn new(kind: ErrorKind, code: &'static str, params: Vec<(&'static str, String)>) -> Self {
        Self {
            kind,
            code,
            params,
            location: None,
            model_file: None,
            validator: None,
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
        location: Option<serde_json::Value>,
    ) -> Self {
        params.push(("typeName", type_name));
        Self {
            kind: ErrorKind::TypeNotFound,
            code,
            params,
            location,
            model_file: None,
            validator: None,
        }
    }

    /// A call site that has not yet been faithfully ported from the TS
    /// reference (module doc; PORTING.md section 7.2). `message` is used
    /// verbatim, through [`Renderer::Raw`], never through the catalogue: it
    /// is not claimed to be a verbatim TS template, and the OD-5
    /// completeness test does not expect a golden test with its own name for
    /// every such call site, only for the one shared `"pre-port"` entry.
    /// The unit that later ports this member replaces the call with a real
    /// catalogue entry (and deletes this one).
    pub fn pre_port(kind: ErrorKind, message: String, location: Option<serde_json::Value>) -> Self {
        Self {
            kind,
            code: "pre-port",
            params: vec![("message", message)],
            location,
            model_file: None,
            validator: None,
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
    /// decorated it (OD-2). Used by the native oracle harness only; the WASM
    /// shim hands [`ContractError::message`] to the real TS constructor.
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
                    suffix.push_str(&format!(
                        "line {} column {}, to line {} column {}. ",
                        at("/start/line"),
                        at("/start/column"),
                        at("/end/line"),
                        at("/end/column")
                    ));
                }
                format!(
                    "{message} {}",
                    crate::model_util::capitalize_first_letter(&suffix)
                )
            }
            ErrorKind::TypeNotFound
            | ErrorKind::Validator
            | ErrorKind::Error
            | ErrorKind::JsTypeError => message,
        }
    }

    /// The `component` the oracle records for this error.
    pub fn component(&self) -> Option<&'static str> {
        match self.kind {
            ErrorKind::IllegalModel | ErrorKind::TypeNotFound => {
                Some("@accordproject/concerto-core")
            }
            ErrorKind::Validator => Some("@accordproject/concerto-util"),
            ErrorKind::Error | ErrorKind::JsTypeError => None,
        }
    }
}

/// A typed AST `location` (`mm::Range`) as the JSON value TS holds for it,
/// for a [`ContractError::location`] (PORTING.md 2.1).
///
/// OD-3 widened the metamodel's number fields to `f64`, so serialising a
/// `Range` straight back gives `3.0` where the AST said `3`. JS has a single
/// number type, so both are the same value and `JSON.stringify` writes `3`;
/// each integral number is written back as a JSON integer to match. Other
/// numbers are left as they are.
pub(crate) fn location_value(
    range: &concerto_metamodel::concerto_metamodel_1_0_0::Range,
) -> Option<serde_json::Value> {
    fn js_numbers(value: serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        // Integers up to 2^53 are exact in both f64 and i64.
        const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
        match value {
            Value::Number(n) => match n.as_f64() {
                // `-0` becomes `0`, as `JSON.stringify(-0)` writes it.
                Some(f)
                    if !n.is_i64() && !n.is_u64() && f.fract() == 0.0 && f.abs() <= MAX_SAFE =>
                {
                    Value::from(f as i64)
                }
                _ => Value::Number(n),
            },
            Value::Array(items) => Value::Array(items.into_iter().map(js_numbers).collect()),
            Value::Object(map) => {
                Value::Object(map.into_iter().map(|(k, v)| (k, js_numbers(v))).collect())
            }
            other => other,
        }
    }
    serde_json::to_value(range).ok().map(js_numbers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(code: &'static str, params: &[(&'static str, &str)]) -> ContractError {
        ContractError::new(
            ErrorKind::Error,
            code,
            params.iter().map(|(k, v)| (*k, (*v).to_string())).collect(),
        )
    }

    // Golden tests: one per trial catalogue entry (PORTING.md 6.3). Each
    // expected string is what the TS reference produces for the same params.

    #[test]
    fn golden_modelutil_getnamespace_nofnq() {
        assert_eq!(
            contract("modelutil-getnamespace-nofnq", &[]).message(),
            "FQN is invalid."
        );
    }

    #[test]
    fn golden_modelutil_parsenamespace_nullorundefined() {
        assert_eq!(
            contract("modelutil-parsenamespace-nullorundefined", &[]).message(),
            "Namespace is null or undefined."
        );
    }

    #[test]
    fn golden_modelutil_parsenamespace_invalidnamespace() {
        assert_eq!(
            contract(
                "modelutil-parsenamespace-invalidnamespace",
                &[("ns", "org.acme@1.0.0@2.3")]
            )
            .message(),
            "Invalid namespace org.acme@1.0.0@2.3"
        );
    }

    #[test]
    fn golden_modelutil_isassignableto_cannotfindtype() {
        assert_eq!(
            contract(
                "modelutil-isassignableto-cannotfindtype",
                &[("typeName", "org.doge.Doge")]
            )
            .message(),
            "Cannot find type org.doge.Doge"
        );
    }

    #[test]
    fn golden_metamodelutil_importfullyqualifiednames_unrecognizedimports() {
        assert_eq!(
            contract(
                "metamodelutil-importfullyqualifiednames-unrecognizedimports",
                &[("$class", "undefined")]
            )
            .message(),
            "Unrecognized imports undefined"
        );
    }

    #[test]
    fn golden_validator_reporterror() {
        let mut err = contract("numbervalidator-constructor-nobounds", &[]);
        err.kind = ErrorKind::Validator;
        err.validator = Some(ValidatorReport {
            id: "null".into(),
            fqn: "org.acme.myField".into(),
            error_type: "DefaultValidatorException",
        });
        assert_eq!(
            err.message(),
            "Validator error for field `null`. org.acme.myField: Invalid range, lower and-or upper bound must be specified."
        );
    }

    #[test]
    fn golden_numbervalidator_constructor_nobounds() {
        assert_eq!(
            contract("numbervalidator-constructor-nobounds", &[]).message(),
            "Invalid range, lower and-or upper bound must be specified."
        );
    }

    #[test]
    fn golden_numbervalidator_constructor_lowerhigherthanupper() {
        assert_eq!(
            contract("numbervalidator-constructor-lowerhigherthanupper", &[]).message(),
            "Lower bound must be less than or equal to upper bound."
        );
    }

    #[test]
    fn golden_numbervalidator_constructor_outsidelowerbound() {
        assert_eq!(
            contract(
                "numbervalidator-constructor-outsidelowerbound",
                &[("value", "5"), ("lowerBound", "10")]
            )
            .message(),
            "Value 5 is outside lower bound 10"
        );
    }

    #[test]
    fn golden_numbervalidator_constructor_outsideupperbound() {
        assert_eq!(
            contract(
                "numbervalidator-constructor-outsideupperbound",
                &[("value", "60"), ("upperBound", "50")]
            )
            .message(),
            "Value 60 is outside upper bound 50"
        );
    }

    #[test]
    fn golden_scalardeclaration_process_primitivename() {
        assert_eq!(
            contract(
                "scalardeclaration-process-primitivename",
                &[("scalarName", "String")]
            )
            .message(),
            "Invalid scalar name 'String'. Name conflicts with primitive type."
        );
    }

    #[test]
    fn golden_scalardeclaration_validate_duplicateclassname() {
        assert_eq!(
            contract(
                "scalardeclaration-validate-duplicateclassname",
                &[("name", "org.acme@1.0.0.A")]
            )
            .message(),
            "Duplicate class name org.acme@1.0.0.A"
        );
    }

    #[test]
    fn golden_engine_typeerror_readproperties() {
        assert_eq!(
            contract(
                "engine-typeerror-readproperties",
                &[("value", "undefined"), ("property", "$class")]
            )
            .message(),
            "Cannot read properties of undefined (reading '$class')"
        );
    }

    #[test]
    fn golden_engine_typeerror_notafunction() {
        assert_eq!(
            contract(
                "engine-typeerror-notafunction",
                &[("expression", "imp.types.forEach")]
            )
            .message(),
            "imp.types.forEach is not a function"
        );
    }

    /// Inline templates are template literals: an inserted value that looks
    /// like another param, or like a `replace` substitution pattern, stays as
    /// it is.
    #[test]
    fn inline_rendering_never_rescans_inserted_values() {
        let mut err = contract("numbervalidator-constructor-nobounds", &[]);
        err.kind = ErrorKind::Validator;
        err.validator = Some(ValidatorReport {
            id: "{fqn} $& $$".into(),
            fqn: "{msg}".into(),
            error_type: "DefaultValidatorException",
        });
        assert_eq!(
            err.message(),
            "Validator error for field `{fqn} $& $$`. {msg}: Invalid range, lower and-or upper bound must be specified."
        );
    }

    /// `Globalize` templates are the opposite of `Inline` on every edge case
    /// 6.3 asks for: a repeated `{param}` is replaced at each occurrence, a
    /// value containing another param's placeholder is substituted again
    /// once that later param is applied, and `$$`/`$&` in a value follow JS
    /// `String.prototype.replace` (section 2.2).
    #[test]
    fn globalize_rendering_rescans_inserted_values_and_applies_replace_patterns() {
        assert_eq!(
            contract(
                "factory-newinstance-missingidentifier",
                &[("type", "{namespace} $$ $&"), ("namespace", "org.acme")]
            )
            .message(),
            // The "type" pass runs first: "$$" -> "$", "$&" -> the matched
            // text "{type}" (not "{namespace}": $& is relative to *this*
            // pass's own match). That leaves a literal "{namespace}" in the
            // message, which the later "namespace" pass then substitutes too
            // (rescanning) — but the "{type}" that "$&" just inserted is not
            // reprocessed, because the "type" param has already run.
            "Missing identifier for Type \"org.acme $ {type}\" in namespace \"org.acme\"."
        );
    }

    // ---- the P1-05 additions (beyond the P0-04b trial payload above) ----

    #[test]
    fn golden_typenotfounderror_defaultmessage() {
        assert_eq!(
            contract(
                "typenotfounderror-defaultmessage",
                &[("typeName", "org.acme.Doge")]
            )
            .message(),
            "Type \"org.acme.Doge\" not found."
        );
    }

    #[test]
    fn golden_factory_newinstance_missingidentifier() {
        assert_eq!(
            contract(
                "factory-newinstance-missingidentifier",
                &[("type", "MyAsset"), ("namespace", "org.acme")]
            )
            .message(),
            "Missing identifier for Type \"MyAsset\" in namespace \"org.acme\"."
        );
    }

    #[test]
    fn golden_factory_newinstance_invalididentifier() {
        assert_eq!(
            contract(
                "factory-newinstance-invalididentifier",
                &[("type", "MyAsset"), ("namespace", "org.acme")]
            )
            .message(),
            "Invalid or missing identifier for Type \"MyAsset\" in namespace \"org.acme\"."
        );
    }

    #[test]
    fn golden_factory_newinstance_abstracttype() {
        assert_eq!(
            contract(
                "factory-newinstance-abstracttype",
                &[("type", "MyAsset"), ("namespace", "org.acme")]
            )
            .message(),
            "Cannot instantiate the abstract type \"MyAsset\" in the \"org.acme\" namespace."
        );
    }

    #[test]
    fn golden_factory_newinstance_typenotdeclaredinns() {
        assert_eq!(
            contract(
                "factory-newinstance-typenotdeclaredinns",
                &[("type", "MyAsset"), ("namespace", "org.acme")]
            )
            .message(),
            "Cannot instantiate Type \"MyAsset\" in namespace \"org.acme\"."
        );
    }

    #[test]
    fn golden_modelmanager_gettype_noregisteredns() {
        assert_eq!(
            contract(
                "modelmanager-gettype-noregisteredns",
                &[("type", "org.acme@1.0.0.Doge")]
            )
            .message(),
            "Namespace is not defined for type \"org.acme@1.0.0.Doge\"."
        );
    }

    // ---- the P1-05 exit-condition sweep (catalogue.rs module doc) ----

    #[test]
    fn golden_modelmanager_resolvetype_nonsfortype() {
        assert_eq!(
            contract(
                "modelmanager-resolvetype-nonsfortype",
                &[("type", "org.acme.Foo"), ("context", "resolving type")]
            )
            .message(),
            "No registered namespace for type \"org.acme.Foo\" in \"resolving type\"."
        );
    }

    #[test]
    fn golden_modelmanager_resolvetype_notypeinnsforcontext() {
        assert_eq!(
            contract(
                "modelmanager-resolvetype-notypeinnsforcontext",
                &[
                    ("context", "resolving type"),
                    ("type", "Foo"),
                    ("namespace", "org.acme@1.0.0")
                ]
            )
            .message(),
            "No type \"Foo\" in namespace \"org.acme@1.0.0\" for \"resolving type\"."
        );
    }

    #[test]
    fn golden_modelmanager_gettype_notypeinns() {
        assert_eq!(
            contract(
                "modelmanager-gettype-notypeinns",
                &[("type", "Foo"), ("namespace", "org.acme@1.0.0")]
            )
            .message(),
            "Type \"Foo\" is not defined in namespace \"org.acme@1.0.0\"."
        );
    }

    #[test]
    fn golden_modelmanager_gettype_duplicatensimport() {
        assert_eq!(
            contract(
                "modelmanager-gettype-duplicatensimport",
                &[
                    ("namespace", "org.acme"),
                    ("version1", "1.0.0"),
                    ("version2", "2.0.0")
                ]
            )
            .message(),
            "Importing types from different versions (\"1.0.0\", \"2.0.0\") of the same namespace \"org.acme\" is not permitted."
        );
    }

    #[test]
    fn golden_modelfile_resolvetype_undecltype() {
        assert_eq!(
            contract(
                "modelfile-resolvetype-undecltype",
                &[("type", "Foo"), ("context", "a field")]
            )
            .message(),
            "Undeclared type \"Foo\" in \"a field\"."
        );
    }

    #[test]
    fn golden_modelfile_resolveimport_failfindimp() {
        assert_eq!(
            contract(
                "modelfile-resolveimport-failfindimp",
                &[
                    ("type", "Foo"),
                    ("imports", "org.acme.Bar"),
                    ("namespace", "org.acme@1.0.0")
                ]
            )
            .message(),
            "Failed to find \"Foo\" in list of imports \"[org.acme.Bar]\" for namespace \"org.acme@1.0.0\"."
        );
    }

    #[test]
    fn golden_modelfile_constructor_unrecmodelelem() {
        assert_eq!(
            contract(
                "modelfile-constructor-unrecmodelelem",
                &[("type", "concerto.metamodel@1.0.0.Foo")]
            )
            .message(),
            "Unrecognised model element \"concerto.metamodel@1.0.0.Foo\"."
        );
    }

    #[test]
    fn golden_classdeclaration_validate_undefined_properties() {
        assert_eq!(
            contract(
                "classdeclaration-validate-undefined-properties",
                &[("class", "org.acme.Foo")]
            )
            .message(),
            "Properties of Class \"org.acme.Foo\" has to be defined."
        );
    }

    #[test]
    fn golden_classdeclaration_process_unrecmodelelem() {
        assert_eq!(
            contract(
                "classdeclaration-process-unrecmodelelem",
                &[("type", "concerto.metamodel@1.0.0.Bar")]
            )
            .message(),
            "Unrecognised model element \"concerto.metamodel@1.0.0.Bar\"."
        );
    }

    #[test]
    fn golden_classdeclaration_validate_selfextending() {
        assert_eq!(
            contract(
                "classdeclaration-validate-selfextending",
                &[("class", "Foo")]
            )
            .message(),
            "Class \"Foo\" cannot extend itself."
        );
    }

    #[test]
    fn golden_classdeclaration_validate_identifiernotproperty() {
        assert_eq!(
            contract(
                "classdeclaration-validate-identifiernotproperty",
                &[("class", "Foo"), ("idField", "bar")]
            )
            .message(),
            "Class \"Foo\" is identified by field \"bar\", but does not contain this property."
        );
    }

    #[test]
    fn golden_classdeclaration_validate_identifiernotstring() {
        assert_eq!(
            contract(
                "classdeclaration-validate-identifiernotstring",
                &[("class", "Foo"), ("idField", "bar")]
            )
            .message(),
            "Class \"Foo\" is identified by field \"bar\", but the type of the field is not \"String\"."
        );
    }

    #[test]
    fn golden_instancegenerator_newinstance_noconcreteclass() {
        assert_eq!(
            contract(
                "instancegenerator-newinstance-noconcreteclass",
                &[("type", "org.acme@1.0.0.Foo")]
            )
            .message(),
            "No concrete extending type for \"org.acme@1.0.0.Foo\"."
        );
    }

    #[test]
    fn golden_serializer_tojson_notcobject() {
        assert_eq!(
            contract("serializer-tojson-notcobject", &[]).message(),
            "\"Serializer.toJSON\" only accepts \"Concept\", \"Event\", \"Asset\", \"Participant\" or \"Transaction\"."
        );
    }

    #[test]
    fn golden_resourcevalidator_fieldtypeviolation() {
        assert_eq!(
            contract(
                "resourcevalidator-fieldtypeviolation",
                &[
                    ("resourceId", "org.acme.Foo#1"),
                    ("propertyName", "bar"),
                    ("fieldType", "String"),
                    ("value", "42"),
                    ("typeOfValue", "number")
                ]
            )
            .message(),
            "Model violation in the \"org.acme.Foo#1\" instance. The field \"bar\" has a value of \"42\" (type of value: \"number\"). Expected type of value: \"String\"."
        );
    }

    #[test]
    fn golden_resourcevalidator_notresourceorconcept() {
        assert_eq!(
            contract(
                "resourcevalidator-notresourceorconcept",
                &[
                    ("resourceId", "org.acme.Foo#1"),
                    ("classFQN", "org.acme.Bar"),
                    ("invalidValue", "42")
                ]
            )
            .message(),
            "Model violation in the \"org.acme.Foo#1\" instance. Class \"org.acme.Bar\" has the value of \"42\". Expected a \"Resource\" or a \"Concept\"."
        );
    }

    #[test]
    fn golden_resourcevalidator_notrelationship() {
        assert_eq!(
            contract(
                "resourcevalidator-notrelationship",
                &[
                    ("resourceId", "org.acme.Foo#1"),
                    ("classFQN", "org.acme.Bar"),
                    ("invalidValue", "42")
                ]
            )
            .message(),
            "Model violation in the \"org.acme.Foo#1\" instance. Class \"org.acme.Bar\" has a value of \"42\". Expected a \"Relationship\"."
        );
    }

    #[test]
    fn golden_resourcevalidator_missingrequiredproperty() {
        assert_eq!(
            contract(
                "resourcevalidator-missingrequiredproperty",
                &[("resourceId", "org.acme.Foo#1"), ("fieldName", "bar")]
            )
            .message(),
            "The instance \"org.acme.Foo#1\" is missing the required field \"bar\"."
        );
    }

    #[test]
    fn golden_resourcevalidator_emptyidentifier() {
        assert_eq!(
            contract(
                "resourcevalidator-emptyidentifier",
                &[("resourceId", "org.acme.Foo#1")]
            )
            .message(),
            "Instance \"org.acme.Foo#1\" has an empty identifier."
        );
    }

    #[test]
    fn golden_resourcevalidator_invalidenumvalue() {
        assert_eq!(
            contract(
                "resourcevalidator-invalidenumvalue",
                &[
                    ("resourceId", "org.acme.Foo#1"),
                    ("value", "BLUE"),
                    ("fieldName", "color")
                ]
            )
            .message(),
            "Model violation in the \"org.acme.Foo#1\" instance. Invalid enum value of \"BLUE\" for the field \"color\"."
        );
    }

    #[test]
    fn golden_resourcevalidator_abstractclass() {
        assert_eq!(
            contract(
                "resourcevalidator-abstractclass",
                &[("className", "org.acme.Foo")]
            )
            .message(),
            "The class \"org.acme.Foo\" is abstract and should not contain an instance."
        );
    }

    #[test]
    fn golden_resourcevalidator_undeclaredfield() {
        assert_eq!(
            contract(
                "resourcevalidator-undeclaredfield",
                &[
                    ("resourceId", "org.acme.Foo#1"),
                    ("propertyName", "bar"),
                    ("fullyQualifiedTypeName", "org.acme.Foo")
                ]
            )
            .message(),
            "Instance \"org.acme.Foo#1\" has a property named \"bar\", which is not declared in \"org.acme.Foo\"."
        );
    }

    #[test]
    fn golden_resourcevalidator_invalidfieldassignment() {
        assert_eq!(
            contract(
                "resourcevalidator-invalidfieldassignment",
                &[
                    ("resourceId", "org.acme.Foo#1"),
                    ("propertyName", "bar"),
                    ("objectType", "org.acme.Baz"),
                    ("fieldType", "org.acme.Bar")
                ]
            )
            .message(),
            "Instance \"org.acme.Foo#1\" has a property \"bar\" with type \"org.acme.Baz\" that is not derived from \"org.acme.Bar\"."
        );
    }

    /// The one non-catalogue renderer: [`ContractError::pre_port`] carries a
    /// hand-written message verbatim, for a call site not yet faithfully
    /// ported (module doc). This is not a golden test against the TS
    /// reference; it only pins the passthrough behaviour.
    #[test]
    fn golden_pre_port() {
        let err = ContractError::pre_port(
            ErrorKind::IllegalModel,
            "duplicate namespace: org.acme@1.0.0".into(),
            None,
        );
        assert_eq!(err.code, "pre-port");
        assert_eq!(err.message(), "duplicate namespace: org.acme@1.0.0");
    }

    /// [`ContractError::type_not_found`] appends `typeName` after the
    /// template's own params (table 2.3), without disturbing rendering.
    #[test]
    fn type_not_found_constructor_adds_type_name_param() {
        let err = ContractError::type_not_found(
            "modelmanager-gettype-noregisteredns",
            vec![("type", "org.acme@1.0.0.Doge".to_string())],
            "org.acme@1.0.0.Doge".to_string(),
            None,
        );
        assert_eq!(err.kind, ErrorKind::TypeNotFound);
        assert_eq!(
            err.params.last(),
            Some(&("typeName", "org.acme@1.0.0.Doge".to_string()))
        );
        assert_eq!(
            err.message(),
            "Namespace is not defined for type \"org.acme@1.0.0.Doge\"."
        );
    }

    /// OD-5: the Rust catalogue holds exactly the en.json keys used by a
    /// RUST or HYBRID member, plus `factory-newinstance-*` and
    /// `typenotfounderror-defaultmessage` (pre-approved ahead of their P3-01
    /// call site). Every key in that scope has an entry; see `catalogue.rs`
    /// for the completeness check the other way (every entry has a golden
    /// test).
    ///
    /// This list is derived from the ledger (`SEAM_LEDGER.tsv`, commit
    /// `c48423c`, OD-6) and the frozen TS reference, not from what the
    /// catalogue happens to hold today: `catalogue.rs`'s module doc records
    /// the reproducible grep, and the "RUST or HYBRID" scope is exactly
    /// [`Renderer::Globalize`] — a literal `en.json` key ported from
    /// `Globalize.messageFormatter`/`formatMessage` (2.2 step 1) — as
    /// opposed to [`Renderer::Inline`] (2.2 step 2: an inline template given
    /// an en.json-style name, not an actual en.json key, such as
    /// `modelutil-parsenamespace-invalidnamespace`). So this test also
    /// asserts the two lists coincide exactly, in both directions: every
    /// `Renderer::Globalize` entry is scoped by OD-5 (nothing unscoped
    /// sneaks in under this renderer) and every OD-5 key has an entry.
    #[test]
    fn od5_catalogue_scope_is_present() {
        const OD5_EN_JSON_KEYS: &[&str] = &[
            // Used today: ModelUtil.getNamespace (src/modelutil.ts, RUST).
            "modelutil-getnamespace-nofnq",
            // OD-5: pre-approved ahead of their call site.
            "typenotfounderror-defaultmessage",
            "factory-newinstance-missingidentifier",
            "factory-newinstance-invalididentifier",
            "factory-newinstance-abstracttype",
            "factory-newinstance-typenotdeclaredinns",
            // BaseModelManager.resolveType/getType (RUST) and ModelFile
            // (constructor, resolveType, resolveImport, validate; RUST).
            "modelmanager-gettype-noregisteredns",
            "modelmanager-resolvetype-nonsfortype",
            "modelmanager-resolvetype-notypeinnsforcontext",
            "modelmanager-gettype-notypeinns",
            "modelmanager-gettype-duplicatensimport",
            "modelfile-resolvetype-undecltype",
            "modelfile-resolveimport-failfindimp",
            "modelfile-constructor-unrecmodelelem",
            // ClassDeclaration.process/validate (RUST).
            "classdeclaration-validate-undefined-properties",
            "classdeclaration-process-unrecmodelelem",
            "classdeclaration-validate-selfextending",
            "classdeclaration-validate-identifiernotproperty",
            "classdeclaration-validate-identifiernotstring",
            // InstanceGenerator.findConcreteSubclass, reached from
            // newInstance (RUST).
            "instancegenerator-newinstance-noconcreteclass",
            // Serializer.toJSON (HYBRID).
            "serializer-tojson-notcobject",
            // ResourceValidator (HYBRID): every report* method.
            "resourcevalidator-fieldtypeviolation",
            "resourcevalidator-notresourceorconcept",
            "resourcevalidator-notrelationship",
            "resourcevalidator-missingrequiredproperty",
            "resourcevalidator-emptyidentifier",
            "resourcevalidator-invalidenumvalue",
            "resourcevalidator-abstractclass",
            "resourcevalidator-undeclaredfield",
            "resourcevalidator-invalidfieldassignment",
        ];
        for key in OD5_EN_JSON_KEYS {
            assert!(
                catalogue_entry(key).is_some(),
                "{key} missing from the catalogue"
            );
            assert_eq!(catalogue_entry(key).unwrap().renderer, Renderer::Globalize);
        }
        let globalize_entries: std::collections::HashSet<&str> = CATALOGUE
            .iter()
            .filter(|entry| entry.renderer == Renderer::Globalize)
            .map(|entry| entry.code)
            .collect();
        let od5_keys: std::collections::HashSet<&str> = OD5_EN_JSON_KEYS.iter().copied().collect();
        assert_eq!(
            globalize_entries, od5_keys,
            "every Renderer::Globalize entry must be exactly the OD-5 scope, no more and no less"
        );
    }

    #[test]
    fn illegal_model_decoration_matches_the_ts_constructor() {
        let mut err = contract(
            "scalardeclaration-process-primitivename",
            &[("scalarName", "String")],
        );
        err.kind = ErrorKind::IllegalModel;
        assert_eq!(
            err.final_message(),
            "Invalid scalar name 'String'. Name conflicts with primitive type. "
        );
        err.model_file = Some(Some("org.acme.cto".into()));
        err.location = Some(serde_json::json!({
            "start": {"line": 1, "column": 2}, "end": {"line": 3, "column": 4}
        }));
        assert_eq!(
            err.final_message(),
            "Invalid scalar name 'String'. Name conflicts with primitive type. File 'org.acme.cto': line 1 column 2, to line 3 column 4. "
        );
    }

    #[test]
    fn type_not_found_displays_name() {
        let err = ConcertoError::TypeNotFound {
            type_name: "org.acme@1.0.0.Foo".into(),
        };
        assert!(err.to_string().contains("org.acme@1.0.0.Foo"));
    }

    #[test]
    fn illegal_model_displays_message() {
        let err = ConcertoError::IllegalModel {
            message: "missing 'namespace'".into(),
            file_name: Some("model.json".into()),
            location: None,
        };
        assert!(err.to_string().contains("missing 'namespace'"));
    }
}

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
        /// The source location, if known.
        location: Option<String>,
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
    #[test]
    fn od5_catalogue_scope_is_present() {
        const OD5_EN_JSON_KEYS: &[&str] = &[
            // Used today: ModelUtil.getNamespace (src/modelutil.ts).
            "modelutil-getnamespace-nofnq",
            // Used today: ModelManager.getType's unregistered-namespace path
            // (src/basemodelmanager.ts), reused faithfully by
            // model_manager::ModelManager::resolve_type_name (section 7.2).
            "modelmanager-gettype-noregisteredns",
            // OD-5: pre-approved ahead of their call site.
            "typenotfounderror-defaultmessage",
            "factory-newinstance-missingidentifier",
            "factory-newinstance-invalididentifier",
            "factory-newinstance-abstracttype",
            "factory-newinstance-typenotdeclaredinns",
        ];
        for key in OD5_EN_JSON_KEYS {
            assert!(
                catalogue_entry(key).is_some(),
                "{key} missing from the catalogue"
            );
            assert_eq!(catalogue_entry(key).unwrap().renderer, Renderer::Globalize);
        }
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

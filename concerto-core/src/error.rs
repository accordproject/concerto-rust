//! Error types for `concerto-core`.
//!
//! [`ConcertoError`] covers the hard failures that stop a model from being
//! used: a type or namespace that cannot be resolved, or model JSON that does
//! not satisfy the metamodel. Each variant carries enough context to report
//! what went wrong and, where known, where.

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

    /// A namespace was referenced before any model declared it.
    #[error("namespace not found: {namespace}")]
    NamespaceNotFound {
        /// The namespace that could not be found.
        namespace: String,
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

    /// A loaded model is structurally sound but fails semantic validation:
    /// an unresolved super type, a property whose type is not declared, a
    /// duplicated field across an inheritance chain, and the like.
    #[error("validation failed: {message}")]
    ValidationFailed {
        /// A description of what did not validate.
        message: String,
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

// ---------------------------------------------------------------------------
// P0-04b trial: the error contract for the three trial units only.
//
// P1-05 owns the error module and the message catalogue. This section holds
// exactly what ModelUtil, NumberValidator and ScalarDeclaration need, in the
// shape PORTING.md section 2 fixes, so that P1-05 can absorb it into
// `error/` unchanged: the `ErrorKind` variants, the `ContractError` fields,
// the catalogue entries (with their sources and renderers) and their golden
// tests. Nothing else may be added here.
// ---------------------------------------------------------------------------

/// Selects the TS class the shim throws (PORTING.md table 2.3). Only the
/// kinds the trial units raise exist so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// `IllegalModelException(message, modelFile, location)`.
    IllegalModel,
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

/// The trial units' messages (P0-04b). P1-05 moves them into the catalogue.
pub const TRIAL_CATALOGUE: &[CatalogueEntry] = &[
    CatalogueEntry {
        code: "modelutil-getnamespace-nofnq",
        template: "FQN is invalid.",
        renderer: Renderer::Globalize,
        sources: &["src/modelutil.ts:93"],
    },
    CatalogueEntry {
        code: "modelutil-parsenamespace-nullorundefined",
        template: "Namespace is null or undefined.",
        renderer: Renderer::Inline,
        sources: &["src/modelutil.ts:124"],
    },
    CatalogueEntry {
        code: "modelutil-parsenamespace-invalidnamespace",
        template: "Invalid namespace {ns}",
        renderer: Renderer::Inline,
        sources: &["src/modelutil.ts:130", "src/modelutil.ts:136"],
    },
    CatalogueEntry {
        code: "modelutil-isassignableto-cannotfindtype",
        template: "Cannot find type {typeName}",
        renderer: Renderer::Inline,
        sources: &["src/modelutil.ts:196"],
    },
    CatalogueEntry {
        code: "metamodelutil-importfullyqualifiednames-unrecognizedimports",
        template: "Unrecognized imports {$class}",
        renderer: Renderer::Inline,
        sources: &["@accordproject/concerto-metamodel@3.17.0 lib/metamodelutil.js:257"],
    },
    CatalogueEntry {
        code: "validator-reporterror",
        template: "Validator error for field `{id}`. {fqn}: {msg}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/validator.ts:82"],
    },
    CatalogueEntry {
        code: "numbervalidator-constructor-nobounds",
        template: "Invalid range, lower and-or upper bound must be specified.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/numbervalidator.ts:65"],
    },
    CatalogueEntry {
        code: "numbervalidator-constructor-lowerhigherthanupper",
        template: "Lower bound must be less than or equal to upper bound.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/numbervalidator.ts:70"],
    },
    CatalogueEntry {
        code: "numbervalidator-constructor-outsidelowerbound",
        template: "Value {value} is outside lower bound {lowerBound}",
        renderer: Renderer::Inline,
        sources: &[
            "src/introspect/numbervalidator.ts:77",
            "src/introspect/numbervalidator.ts:111",
        ],
    },
    CatalogueEntry {
        code: "numbervalidator-constructor-outsideupperbound",
        template: "Value {value} is outside upper bound {upperBound}",
        renderer: Renderer::Inline,
        sources: &[
            "src/introspect/numbervalidator.ts:81",
            "src/introspect/numbervalidator.ts:115",
        ],
    },
    CatalogueEntry {
        code: "scalardeclaration-process-primitivename",
        template: "Invalid scalar name '{scalarName}'. Name conflicts with primitive type.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/scalardeclaration.ts:66"],
    },
    CatalogueEntry {
        code: "scalardeclaration-validate-duplicateclassname",
        template: "Duplicate class name {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/scalardeclaration.ts:138"],
    },
    CatalogueEntry {
        code: "engine-typeerror-readproperties",
        template: "Cannot read properties of {value} (reading '{property}')",
        renderer: Renderer::Inline,
        sources: &["V8 (property read on null or undefined)"],
    },
    CatalogueEntry {
        code: "engine-typeerror-notafunction",
        template: "{expression} is not a function",
        renderer: Renderer::Inline,
        sources: &["V8 (call of a non-function)"],
    },
];

/// Looks up a trial catalogue entry.
pub fn catalogue_entry(code: &str) -> Option<&'static CatalogueEntry> {
    TRIAL_CATALOGUE.iter().find(|entry| entry.code == code)
}

/// Renders a template with its params.
fn render(code: &str, params: &[(&'static str, String)]) -> String {
    let Some(entry) = catalogue_entry(code) else {
        // Unreachable for errors built through `ContractError::new`, which
        // only takes codes from the catalogue (see the completeness test).
        return code.to_string();
    };
    match entry.renderer {
        Renderer::Globalize => entry.template.to_string(),
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
            ErrorKind::Validator | ErrorKind::Error | ErrorKind::JsTypeError => message,
        }
    }

    /// The `component` the oracle records for this error.
    pub fn component(&self) -> Option<&'static str> {
        match self.kind {
            ErrorKind::IllegalModel => Some("@accordproject/concerto-core"),
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

    /// Every trial entry is unique, cites its source, and has a golden test
    /// above (checked by name).
    #[test]
    fn trial_catalogue_is_complete() {
        let source = include_str!("error.rs");
        for (i, entry) in TRIAL_CATALOGUE.iter().enumerate() {
            assert!(!entry.sources.is_empty(), "{} cites no source", entry.code);
            assert!(
                TRIAL_CATALOGUE[..i]
                    .iter()
                    .all(|e| e.code != entry.code && e.template != entry.template),
                "{} is duplicated",
                entry.code
            );
            let golden = format!("fn golden_{}()", entry.code.replace('-', "_"));
            assert!(
                source.contains(&golden),
                "{} has no golden test",
                entry.code
            );
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

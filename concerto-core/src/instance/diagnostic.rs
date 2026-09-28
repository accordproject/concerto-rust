//! Instance validation with diagnostics (docs/public-api.md section 5.7):
//! [`ValidationReport`] and the [`ModelManager`] entry points, over
//! [`ValidationOptions`]. Task P3-03 (accordproject/concerto-rust#58,
//! accordproject/concerto#1239) built the collect-all walk; P6-01
//! (accordproject/concerto-rust#83, step 5) gave it its stable names.
//!
//! There are two modes, picked by the method called:
//!
//! - **First error:** [`ModelManager::validate_instance`] returns
//!   `Result<()>`, with the error TS `Serializer.fromJSON` (with
//!   `validate: true`) would throw for the same document.
//! - **Collect-all (#1239):** [`ModelManager::check_instance`] walks the
//!   whole instance and returns a [`ValidationReport`] of every
//!   [`Diagnostic`] found, each with a JSON Pointer (RFC 6901) to the
//!   offending location, a stable [`DiagnosticCode`] and a [`Severity`].
//!
//! The input is plain JSON, as `Serializer.toJSON` writes it: a `DateTime` is
//! its ISO string, and a relationship is its URI. Both modes first read it
//! the way `Serializer.fromJSON` does (`super::from_json`), with the #1273
//! options ([`ValidationOptions::reject_unknown_keys`] and
//! [`ValidationOptions::reject_required_null`]) applied as the document is
//! read, then run the `ResourceValidator` walk. A document that cannot be
//! read (a malformed `DateTime`, an unknown `$class`, a #1273 rejection)
//! fails there: `check_instance` reports that failure as its diagnostics.
//! The `_as` forms check against a named type rather than the instance's
//! own `$class`.

use serde_json::Value;

use crate::error::{DetailCode, Error, Result};
use crate::model_manager::ModelManager;

use super::from_json::{self, FixedEnv};
use super::options::ValidationOptions;
use super::validate;

/// How serious a [`Diagnostic`] is.
///
/// Every check the collect-all walk runs today reports a violation that
/// makes the instance invalid, so only [`Severity::Error`] is produced so
/// far; the field exists (rather than every diagnostic being implicitly an
/// error) because #1239 asks for a `severity` on `Diagnostic` itself, for a
/// future check that is worth surfacing without failing validation on its
/// own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Severity {
    /// The instance is not valid as it stands.
    Error,
    /// Worth surfacing, but does not by itself make the instance invalid.
    Warning,
}

/// What kind of violation a [`Diagnostic`] reports. Each variant is a
/// distinct, stable code a caller can match on without parsing
/// [`Diagnostic::message`], the way #1273's [`DetailCode`](crate::error::DetailCode)
/// already does for the two `DeserializeOptions` rejections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticCode {
    /// A required property has no value, and no default to fall back to.
    MissingRequiredProperty,
    /// A key the declaration does not declare.
    UndeclaredField,
    /// A value's shape does not match its property's declared type.
    TypeViolation,
    /// An enum-typed value that is not one of the enum's own value names.
    InvalidEnumValue,
    /// An identified type's identifier is present but empty.
    EmptyIdentifier,
    /// The value's own `$class` is an abstract type.
    AbstractClass,
    /// A class- or relationship-typed value's own type is not assignable to
    /// the property's declared type.
    NotAssignable,
    /// A value that should be a `Resource`-shaped object is not one.
    NotResource,
    /// A relationship-typed value is neither a relationship nor (where the
    /// options allow it) a resource standing in for one.
    NotRelationship,
    /// A string/number/collection-size validator rejected the value.
    ValidatorFailure,
    /// A `$class` does not resolve to any type loaded into the model
    /// manager.
    TypeNotFound,
}

impl DiagnosticCode {
    /// The code's own stable spelling, `SCREAMING_SNAKE_CASE` like #1273's
    /// [`DetailCode::as_str`](crate::error::DetailCode::as_str).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingRequiredProperty => "MISSING_REQUIRED_PROPERTY",
            Self::UndeclaredField => "UNDECLARED_FIELD",
            Self::TypeViolation => "TYPE_VIOLATION",
            Self::InvalidEnumValue => "INVALID_ENUM_VALUE",
            Self::EmptyIdentifier => "EMPTY_IDENTIFIER",
            Self::AbstractClass => "ABSTRACT_CLASS",
            Self::NotAssignable => "NOT_ASSIGNABLE",
            Self::NotResource => "NOT_RESOURCE",
            Self::NotRelationship => "NOT_RELATIONSHIP",
            Self::ValidatorFailure => "VALIDATOR_FAILURE",
            Self::TypeNotFound => "TYPE_NOT_FOUND",
        }
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One violation found while validating an instance, as the collect-all walk
/// (`super::validate::collect_diagnostics`) reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// A JSON Pointer (RFC 6901) from the root of the validated value to the
    /// offending location: `""` for the root value itself, `"/vin"` for a
    /// top-level field, `"/tags/0"` for an array element, and so on. A `~`
    /// or `/` in a property name is escaped (`~0`/`~1`) as RFC 6901 requires.
    pub pointer: String,
    /// What kind of violation this is.
    pub code: DiagnosticCode,
    /// How serious it is.
    pub severity: Severity,
    /// A human-readable description, reusing the same message catalogue and
    /// rendering [`ModelManager::validate_instance`]'s first-error walk
    /// uses for the same underlying check, where the
    /// diagnostic is raised by that shared check (module doc); a violation
    /// only the collect-all walk itself detects (an unresolvable `$class`, a
    /// value that is not `Resource`-shaped) gets its own short description
    /// instead, since it has no ported TS message to reuse.
    pub message: String,
}

impl Diagnostic {
    /// Builds an [`Error`](Severity::Error)-severity diagnostic. Every check
    /// the collect-all walk runs today is one, so this is the collector's
    /// only constructor (module doc on [`Severity`]).
    pub(crate) fn error(pointer: String, code: DiagnosticCode, message: String) -> Self {
        Self {
            pointer,
            code,
            severity: Severity::Error,
            message,
        }
    }
}

/// Zero or more [`Diagnostic`]s: what [`ModelManager::check_instance`]
/// found. An empty report means the instance is valid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    diagnostics: Vec<Diagnostic>,
}

/// The name of [`ValidationReport`] before P6-01.
#[deprecated(since = "0.1.0", note = "renamed to `ValidationReport`")]
pub type ValidationResult = ValidationReport;

impl ValidationReport {
    pub(crate) fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self { diagnostics }
    }

    /// True when no [`Severity::Error`] diagnostic was found. (Every
    /// diagnostic the collect-all walk raises today is one, so this is
    /// currently equivalent to `diagnostics().is_empty()`; the distinction
    /// exists for when a `Warning`-severity check is added, module doc on
    /// [`Severity`].)
    pub fn is_valid(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Every diagnostic found, in the order the walk found them.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Consumes the report, returning its diagnostics.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    /// `Ok(())` when the report [is valid](Self::is_valid), and the report
    /// itself otherwise.
    pub fn into_result(self) -> std::result::Result<(), Self> {
        if self.is_valid() { Ok(()) } else { Err(self) }
    }
}

impl IntoIterator for ValidationReport {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.diagnostics.into_iter()
    }
}

impl<'a> IntoIterator for &'a ValidationReport {
    type Item = &'a Diagnostic;
    type IntoIter = std::slice::Iter<'a, Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.diagnostics.iter()
    }
}

impl ModelManager {
    /// Validates `instance`, a plain JSON document, against the models
    /// loaded here, and returns the first error, as TS
    /// `serializer.fromJSON(instance, {validate: true, ...options})` throws
    /// it. The type is the instance's own `$class`.
    ///
    /// # Errors
    ///
    /// The first violation, with the [`ErrorKind`](crate::ErrorKind) of the
    /// TS exception class: [`Validation`](crate::ErrorKind::Validation) for
    /// most, [`TypeNotFound`](crate::ErrorKind::TypeNotFound) for an unknown
    /// type, [`InvalidArgument`](crate::ErrorKind::InvalidArgument) for a
    /// missing `$class` or a bad identifier. A #1273 rejection lists its
    /// violations in [`Error::details`].
    pub fn validate_instance(&self, instance: &Value, options: &ValidationOptions) -> Result<()> {
        from_json::from_json(
            self,
            instance,
            &options.populate_options(true),
            &mut FixedEnv,
        )
        .map(|_| ())
    }

    /// [`validate_instance`](Self::validate_instance) against the type
    /// `fqn` rather than the instance's own `$class`. An instance with a
    /// `$class` must be of a type assignable to `fqn`, and is then checked
    /// as its own type; one with none is checked as `fqn`.
    ///
    /// # Errors
    ///
    /// As [`validate_instance`](Self::validate_instance), and a
    /// [`Validation`](crate::ErrorKind::Validation) error when the
    /// instance's type is not assignable to `fqn`.
    pub fn validate_instance_as(
        &self,
        fqn: &str,
        instance: &Value,
        options: &ValidationOptions,
    ) -> Result<()> {
        validate::check_assignable_to_declaration(self, fqn, instance)?;
        self.populate(Some(fqn), instance, options.populate_options(true))
            .map(|_| ())
    }

    /// Checks `instance`, a plain JSON document, against the models loaded
    /// here, and reports every violation found (accordproject/concerto#1239)
    /// instead of stopping at the first. The type is the instance's own
    /// `$class`. A document that cannot be read as an instance of its type
    /// (module doc) is reported by that failure alone.
    pub fn check_instance(
        &self,
        instance: &Value,
        options: &ValidationOptions,
    ) -> ValidationReport {
        match self.populate(None, instance, options.populate_options(false)) {
            Ok(populated) => {
                let fqn = populated
                    .get("$class")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                validate::collect_diagnostics(self, &fqn, &populated, &options.validate_options())
            }
            Err(err) => report_of_error(&err),
        }
    }

    /// [`check_instance`](Self::check_instance) against the type `fqn`
    /// rather than the instance's own `$class`, as
    /// [`validate_instance_as`](Self::validate_instance_as) reads it.
    pub fn check_instance_as(
        &self,
        fqn: &str,
        instance: &Value,
        options: &ValidationOptions,
    ) -> ValidationReport {
        if let Some(report) = validate::assignability_diagnostic(self, fqn, instance) {
            return report;
        }
        match self.populate(Some(fqn), instance, options.populate_options(false)) {
            Ok(populated) => {
                validate::collect_diagnostics(self, fqn, &populated, &options.validate_options())
            }
            Err(err) => report_of_error(&err),
        }
    }

    /// Reads `instance` as `Serializer.fromJSON` does: as its own `$class`
    /// when it has one, or else as `fqn`.
    fn populate(
        &self,
        fqn: Option<&str>,
        instance: &Value,
        options: from_json::FromJsonOptions,
    ) -> Result<Value> {
        let own_class = instance.get("$class").filter(|c| crate::ecma::is_truthy(c));
        match (own_class, fqn) {
            (None, Some(fqn)) => {
                from_json::from_json_as(self, instance, fqn, &options, &mut FixedEnv)
            }
            _ => from_json::from_json(self, instance, &options, &mut FixedEnv),
        }
    }
}

/// The diagnostics of a document that could not be read as an instance: one
/// per #1273 detail, or one for the error.
fn report_of_error(err: &Error) -> ValidationReport {
    if !err.details().is_empty() {
        return ValidationReport::new(
            err.details()
                .iter()
                .map(|detail| {
                    let code = match detail.code {
                        DetailCode::UnknownProperty => DiagnosticCode::UndeclaredField,
                        _ => DiagnosticCode::TypeViolation,
                    };
                    Diagnostic::error(pointer_of_path(&detail.path), code, err.to_string())
                })
                .collect(),
        );
    }
    let (code, message) = match err.code() {
        "serializer-fromjson-noclass"
        | "serializer-fromjson-mapnotsupported"
        | "serializer-fromjson-enumnotsupported" => (DiagnosticCode::NotResource, err.to_string()),
        "factory-newinstance-missingidentifier" => {
            (DiagnosticCode::EmptyIdentifier, err.to_string())
        }
        "factory-newinstance-abstracttype" => (DiagnosticCode::AbstractClass, err.to_string()),
        "factory-newresource-idregexmismatch" => {
            (DiagnosticCode::ValidatorFailure, err.to_string())
        }
        "jsonpopulator-validateproperties-unexpectedproperties" => {
            (DiagnosticCode::UndeclaredField, err.to_string())
        }
        "jsonpopulator-visitrelationshipdeclaration-notstringorobject"
        | "jsonpopulator-visitrelationshipdeclaration-notastring"
        | "jsonpopulator-visitrelationshipdeclaration-noclass" => {
            (DiagnosticCode::NotRelationship, err.to_string())
        }
        _ => validate::classify_error(err),
    };
    let pointer = err
        .params()
        .iter()
        .find(|(name, _)| *name == "path")
        .map(|(_, path)| pointer_of_path(path))
        .unwrap_or_default();
    ValidationReport::new(vec![Diagnostic::error(pointer, code, message)])
}

/// The JSON Pointer (RFC 6901) of a populator path (`$.tags[0].name`).
fn pointer_of_path(path: &str) -> String {
    let mut pointer = String::new();
    let mut rest = path.strip_prefix('$').unwrap_or(path);
    while !rest.is_empty() {
        let (segment, tail) = if let Some(after) = rest.strip_prefix('[') {
            match after.split_once(']') {
                Some((index, tail)) => (index, tail),
                None => (after, ""),
            }
        } else {
            let after = rest.strip_prefix('.').unwrap_or(rest);
            let end = after.find(['.', '[']).unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        pointer.push('/');
        pointer.push_str(&segment.replace('~', "~0").replace('/', "~1"));
        rest = tail;
    }
    pointer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_result_is_valid() {
        let result = ValidationReport::new(Vec::new());
        assert!(result.is_valid());
        assert!(result.diagnostics().is_empty());
    }

    #[test]
    fn a_result_with_an_error_diagnostic_is_not_valid() {
        let result = ValidationReport::new(vec![Diagnostic::error(
            "/name".to_string(),
            DiagnosticCode::TypeViolation,
            "bad".to_string(),
        )]);
        assert!(!result.is_valid());
        assert_eq!(result.diagnostics().len(), 1);
        assert_eq!(result.diagnostics()[0].code, DiagnosticCode::TypeViolation);
    }

    #[test]
    fn every_diagnostic_code_has_a_stable_screaming_snake_case_spelling() {
        let codes = [
            DiagnosticCode::MissingRequiredProperty,
            DiagnosticCode::UndeclaredField,
            DiagnosticCode::TypeViolation,
            DiagnosticCode::InvalidEnumValue,
            DiagnosticCode::EmptyIdentifier,
            DiagnosticCode::AbstractClass,
            DiagnosticCode::NotAssignable,
            DiagnosticCode::NotResource,
            DiagnosticCode::NotRelationship,
            DiagnosticCode::ValidatorFailure,
            DiagnosticCode::TypeNotFound,
        ];
        for code in codes {
            assert_eq!(code.as_str(), code.to_string());
            assert_eq!(code.as_str(), code.as_str().to_uppercase());
        }
    }

    #[test]
    fn a_populator_path_becomes_a_json_pointer() {
        assert_eq!(pointer_of_path("$"), "");
        assert_eq!(pointer_of_path("$.vin"), "/vin");
        assert_eq!(pointer_of_path("$.tags[0].name"), "/tags/0/name");
        assert_eq!(pointer_of_path("$.a/b.c~d"), "/a~1b/c~0d");
    }

    #[test]
    fn a_report_converts_to_a_result_and_iterates() {
        let empty = ValidationReport::new(Vec::new());
        assert!(empty.into_result().is_ok());
        let report = ValidationReport::new(vec![Diagnostic::error(
            "/name".to_string(),
            DiagnosticCode::TypeViolation,
            "bad".to_string(),
        )]);
        assert_eq!((&report).into_iter().count(), 1);
        let report = report.into_result().unwrap_err();
        assert_eq!(report.into_iter().next().unwrap().pointer, "/name");
    }
}

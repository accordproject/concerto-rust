//! `Diagnostic`/`ValidationResult` (task P3-03, accordproject/concerto-rust#58,
//! plan §4 Phase 3, accordproject/concerto#1239): the Rust-only foundation
//! this issue asks for, ahead of any TS-facing API decision (module doc on
//! [`super::validate`]).
//!
//! [`validate_instance`](super::validate::validate_instance) (task P3-01,
//! `ResourceValidator.validate`) is a faithful port of TS's own first-error
//! behaviour: the walk stops and returns as soon as one violation is found,
//! because that is what the ported TS member itself does. #1239 asks for a
//! second, Rust-only mode alongside it: **collect-all**, which walks the
//! whole instance and gathers every violation it finds in one pass, each one
//! a [`Diagnostic`] carrying a JSON Pointer (RFC 6901) to the offending
//! location, a stable [`DiagnosticCode`] and a [`Severity`]. The two modes
//! share the same underlying checks (this module's collector calls the
//! P3-01/P3-02 leaf checks directly wherever a TS-faithful single verdict is
//! enough, and only re-walks the recursive, class-shaped part of the tree
//! itself so it can keep going past one nested object's own first error);
//! the walk that gathers diagnostics is [`super::validate::collect_diagnostics`].
//!
//! [`ClassDeclaration::validate_instance`]/[`validate_instance_or_throw`] and
//! [`ModelManager::validate_instance`]/[`validate_instance_or_throw`] are the
//! entry points #1239 asks for: the plain name returns a [`ValidationResult`]
//! (collect-all, never fails), and the `_or_throw` name returns
//! `crate::error::Result<()>` (first-error, exactly
//! [`validate_instance`](super::validate::validate_instance)), so a caller
//! picks collect-all or first-error by the method it calls, the way
//! `validate_instance`/`validate_instance_or_throw` reads.
//!
//! [`ClassDeclaration::validate_instance`]: crate::introspect::declaration::ClassDeclaration::validate_instance
//! [`validate_instance_or_throw`]: crate::introspect::declaration::ClassDeclaration::validate_instance_or_throw

use serde_json::Value;

use crate::error::Result;
use crate::introspect::declaration::ClassDeclaration;
use crate::model_manager::ModelManager;

use super::validate::{self, ValidateOptions};

/// How serious a [`Diagnostic`] is.
///
/// Every check the collect-all walk runs today reports a violation that
/// makes the instance invalid, so only [`Severity::Error`] is produced so
/// far; the field exists (rather than every diagnostic being implicitly an
/// error) because #1239 asks for a `severity` on `Diagnostic` itself, for a
/// future check that is worth surfacing without failing validation on its
/// own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
/// ([`super::validate::collect_diagnostics`]) reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// rendering [`validate_instance`](super::validate::validate_instance)'s
    /// first-error walk uses for the same underlying check, where the
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

/// Zero or more [`Diagnostic`]s: the collect-all counterpart of
/// [`validate_instance`](super::validate::validate_instance)'s
/// `Result<()>`. An empty result means the instance is valid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationResult {
    diagnostics: Vec<Diagnostic>,
}

impl ValidationResult {
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

    /// Consumes the result, returning its diagnostics.
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}

impl ClassDeclaration {
    /// Validates `value` against this declaration, collecting every
    /// diagnostic found (task P3-03) instead of stopping at the first one.
    ///
    /// `fqn` is this declaration's own fully qualified name as registered in
    /// `mm` (the name passed to [`ModelManager::get_declaration`] to obtain
    /// `self`); the walk resolves everything else — properties, super
    /// types, nested declarations — through `mm`, the same way
    /// [`validate_instance`](super::validate::validate_instance) does.
    pub fn validate_instance(
        &self,
        mm: &ModelManager,
        fqn: &str,
        value: &Value,
        options: &ValidateOptions,
    ) -> ValidationResult {
        let _ = self;
        validate::collect_diagnostics(mm, fqn, value, options)
    }

    /// [`ClassDeclaration::validate_instance`], but first-error: returns as
    /// soon as one violation is found, raising it as a
    /// [`ConcertoError`](crate::error::ConcertoError) instead of collecting
    /// it — exactly [`validate_instance`](super::validate::validate_instance)
    /// (TS `Resource.validate`), called with `fqn` as the declared type.
    pub fn validate_instance_or_throw(
        &self,
        mm: &ModelManager,
        fqn: &str,
        value: &Value,
        options: &ValidateOptions,
    ) -> Result<()> {
        let _ = self;
        validate::validate_instance_against(mm, fqn, value, options)
    }
}

impl ModelManager {
    /// Validates `value` against the model loaded here, collecting every
    /// diagnostic found (task P3-03) instead of stopping at the first one.
    /// The declared type is `value`'s own `$class`, the way
    /// [`validate_instance`](super::validate::validate_instance) resolves it
    /// for a root call (TS `Resource.validate`, which always validates a
    /// resource against its own type).
    pub fn validate_instance(&self, value: &Value, options: &ValidateOptions) -> ValidationResult {
        validate::collect_diagnostics_from_value(self, value, options)
    }

    /// [`ModelManager::validate_instance`], but first-error: exactly
    /// [`validate_instance`](super::validate::validate_instance).
    pub fn validate_instance_or_throw(
        &self,
        value: &Value,
        options: &ValidateOptions,
    ) -> Result<()> {
        validate::validate_instance(self, value, options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_result_is_valid() {
        let result = ValidationResult::new(Vec::new());
        assert!(result.is_valid());
        assert!(result.diagnostics().is_empty());
    }

    #[test]
    fn a_result_with_an_error_diagnostic_is_not_valid() {
        let result = ValidationResult::new(vec![Diagnostic::error(
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
}

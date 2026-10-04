//! Instance validation with diagnostics (docs/public-api.md section 5.7,
//! accordproject/concerto#1239): [`ValidationReport`] and the
//! [`ModelManager`] entry points, over [`ValidationOptions`].
//!
//! There are two modes, picked by the method called:
//!
//! - **First error:** [`ModelManager::validate_instance`] returns
//!   `Result<()>`, with the error TS `Serializer.fromJSON` (with
//!   `validate: true`) would throw for the same document.
//! - **Collect-all:** [`ModelManager::check_instance`] walks the whole
//!   instance and returns a [`ValidationReport`] of every [`Diagnostic`]
//!   found, each with a JSON Pointer (RFC 6901) to the offending
//!   location, a stable [`DiagnosticCode`] and a [`Severity`].
//!
//! The input is plain JSON, as `Serializer.toJSON` writes it: a `DateTime` is
//! its ISO string, and a relationship is its URI. Both modes first read it
//! as `Serializer.fromJSON` does (`super::from_json`), with the
//! accordproject/concerto#1273 options
//! ([`ValidationOptions::reject_unknown_keys`],
//! [`ValidationOptions::reject_required_null`]) applied as it is read, then
//! run the `ResourceValidator` walk. A document that cannot be read fails
//! there, and `check_instance` reports that failure as its diagnostics.
//! The `_as` forms check against a named type rather than the instance's
//! own `$class`.

use crate::json::Value;

use crate::error::{DetailCode, Error, Result};
use crate::model_manager::ModelManager;

use super::from_json::{self, FixedEnv};
use super::options::ValidationOptions;
use super::validate;

/// How serious a [`Diagnostic`] is.
///
/// Every check the walk runs reports a violation that makes the instance
/// invalid, so only [`Severity::Error`] is produced; the field is there for
/// a check worth surfacing without failing validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Severity {
    /// The instance is not valid as it stands.
    Error,
    /// Worth surfacing, but does not by itself make the instance invalid.
    Warning,
}

/// What kind of violation a [`Diagnostic`] reports: a stable code a caller
/// can match on without parsing [`Diagnostic::message`], as
/// [`DetailCode`](crate::error::DetailCode) is.
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
    /// The code's stable spelling, `SCREAMING_SNAKE_CASE` like
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

/// One violation found while validating an instance, as the validation walk
/// reports it when it collects (`super::validate`, module doc "Stop or
/// collect").
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
    /// A human-readable description: the message of the error
    /// [`ModelManager::validate_instance`] raises for the same violation,
    /// from the same message catalogue (TS's wording). The first-error and
    /// collect-all modes run one walk, so a violation reads the same in
    /// both.
    pub message: String,
    /// What the model expects at [`pointer`](Self::pointer), when it expects
    /// a type there: the declared type as the model spells it (`String`,
    /// `String[]`, `org.acme@1.0.0.Address`, `--> org.acme@1.0.0.Person`),
    /// never quoting the instance. Only the JS binding's `validateInstance`
    /// fills it in; [`ModelManager::check_instance`] leaves it `None`.
    pub expected: Option<String>,
}

impl Diagnostic {
    /// Builds an [`Error`](Severity::Error)-severity diagnostic, the only
    /// severity the walk produces.
    pub(crate) fn error(pointer: String, code: DiagnosticCode, message: String) -> Self {
        Self {
            pointer,
            code,
            severity: Severity::Error,
            message,
            expected: None,
        }
    }
}

/// Zero or more [`Diagnostic`]s: what [`ModelManager::check_instance`]
/// found. An empty report means the instance is valid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    diagnostics: Vec<Diagnostic>,
}

impl ValidationReport {
    pub(crate) fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self { diagnostics }
    }

    /// True when no [`Severity::Error`] diagnostic was found.
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
    /// missing `$class` or a bad identifier. An
    /// accordproject/concerto#1273 rejection lists its violations in
    /// [`Error::details`].
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
    /// here, and reports every violation found instead of stopping at the
    /// first. The type is the instance's own `$class`. A document that cannot
    /// be read as an instance of its type is reported by that failure alone.
    pub fn check_instance(
        &self,
        instance: &Value,
        options: &ValidationOptions,
    ) -> ValidationReport {
        report(self, None, instance, &options.populate_options(false))
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
        report(self, Some(fqn), instance, &options.populate_options(false))
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

/// What validating a document found: the outcome of one read and one
/// walk.
enum Found {
    /// The document's own `$class` is not the named type, nor a subtype of
    /// it: the named type's check, which comes first.
    NamedType(Error),
    /// The document could not be read as an instance: the read's error.
    Unread(Error),
    /// The walk's violations (every one, or the first), each with the JSON
    /// Pointer of the value it was found at; none for a valid instance.
    Walked(Vec<(String, Error)>),
}

/// `instance` read as `Serializer.fromJSON` reads it (as the type `fqn`,
/// when one is given, which its own `$class` must then be or extend), then
/// walked collecting every violation (or, without `all`, the first). The
/// first violation is the error `Serializer.fromJSON` with `validate: true`
/// throws for the document.
fn find(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
    all: bool,
) -> Found {
    if let Some(fqn) = fqn
        && let Err(err) = validate::check_assignable_to_declaration(mm, fqn, instance)
    {
        return Found::NamedType(err);
    }
    match from_json::collect_violations(mm, instance, fqn, options, all) {
        Ok(found) => Found::Walked(found),
        Err(err) => Found::Unread(err),
    }
}

/// [`ModelManager::check_instance`] and its `_as` form: every violation
/// found. A document that cannot be read is reported by that failure alone.
fn report(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
) -> ValidationReport {
    match find(mm, fqn, instance, options, true) {
        Found::NamedType(err) => ValidationReport::new(vec![named_type_diagnostic(&err, &err)]),
        Found::Unread(err) => report_of_error(&err),
        Found::Walked(found) => ValidationReport::new(
            found
                .iter()
                .map(|(pointer, err)| walk_diagnostic(pointer, err))
                .collect(),
        ),
    }
}

/// The diagnostic of a violation the walk found at `pointer`: its code, and
/// the error's own message.
pub(super) fn walk_diagnostic(pointer: &str, err: &Error) -> Diagnostic {
    Diagnostic::error(pointer.to_string(), walk_code(err), err.to_string())
}

/// The [`DiagnosticCode`] of a violation the walk raised.
fn walk_code(err: &Error) -> DiagnosticCode {
    if err.kind() == crate::ErrorKind::TypeNotFound {
        return DiagnosticCode::TypeNotFound;
    }
    classify_error(err)
}

/// The diagnostic of the named type's check ([`Found::NamedType`] `err`),
/// at the root, with `message`'s message.
fn named_type_diagnostic(err: &Error, message: &Error) -> Diagnostic {
    let code = if err.kind() == crate::ErrorKind::TypeNotFound {
        DiagnosticCode::TypeNotFound
    } else {
        DiagnosticCode::NotAssignable
    };
    Diagnostic::error(String::new(), code, message.to_string())
}

/// Maps an [`Error`] to the [`DiagnosticCode`] it reports as; an error this
/// table does not know (a JS-engine-shaped one) is
/// [`DiagnosticCode::TypeViolation`], so a diagnostic is never dropped.
fn classify_error(err: &Error) -> DiagnosticCode {
    let ce = err.contract();
    if ce.validator.is_some() {
        return DiagnosticCode::ValidatorFailure;
    }
    match ce.code {
        "resourcevalidator-missingrequiredproperty" => DiagnosticCode::MissingRequiredProperty,
        "resourcevalidator-undeclaredfield" => DiagnosticCode::UndeclaredField,
        "resourcevalidator-emptyidentifier" => DiagnosticCode::EmptyIdentifier,
        "resourcevalidator-invalidenumvalue" => DiagnosticCode::InvalidEnumValue,
        "resourcevalidator-abstractclass" => DiagnosticCode::AbstractClass,
        "resourcevalidator-invalidfieldassignment" => DiagnosticCode::NotAssignable,
        "resourcevalidator-notresourceorconcept" => DiagnosticCode::NotResource,
        "resourcevalidator-notrelationship"
        | "resourcevalidator-checkrelationship-notidentifiable" => DiagnosticCode::NotRelationship,
        "typenotfounderror-defaultmessage" => DiagnosticCode::TypeNotFound,
        _ => DiagnosticCode::TypeViolation,
    }
}

/// What [`diagnose`] found: the report and, when the instance is not valid,
/// the error the first-error walk raised for it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Diagnosis {
    /// Every diagnostic, the first error's own first.
    pub report: ValidationReport,
    /// The error `Serializer.fromJSON` (with `validate: true` and the same
    /// options) throws for the instance, or `None` when it is valid.
    pub error: Option<Error>,
}

/// The JS binding's `validateInstance`: validates `instance` as
/// `Serializer.fromJSON` does with `options` and `validate: true`, as `fqn`
/// (its `$class` must then be `fqn` or a subtype) or its own `$class`. One
/// walk collects every violation (with `collect_all`) or the first, each at
/// its JSON Pointer; the first is the error `fromJSON` throws
/// ([`Diagnosis::error`]).
#[cfg_attr(
    not(feature = "js-compat"),
    expect(dead_code, reason = "js-compat seam only")
)]
pub fn diagnose(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
    collect_all: bool,
) -> Diagnosis {
    let (error, mut diagnostics) = match find(mm, fqn, instance, options, collect_all) {
        Found::NamedType(err) => {
            let diagnostic = named_type_diagnostic(&err, &err);
            (err, vec![diagnostic])
        }
        Found::Unread(err) => {
            let diagnostics = located(&err, instance);
            (err, diagnostics)
        }
        Found::Walked(found) => {
            let Some((_, first)) = found.first() else {
                return Diagnosis {
                    report: ValidationReport::default(),
                    error: None,
                };
            };
            let diagnostics = found
                .iter()
                .map(|(pointer, err)| walk_diagnostic(pointer, err))
                .collect();
            (first.clone(), diagnostics)
        }
    };
    fill_expected(mm, fqn, instance, &mut diagnostics);
    Diagnosis {
        report: ValidationReport::new(diagnostics),
        error: Some(error),
    }
}

/// The diagnostics the JS binding attaches as `details`
/// (accordproject/concerto#1325) to an error `Serializer.fromJSON` or
/// [`diagnose`] raised for `instance`: one per
/// accordproject/concerto#1273 detail, else one for the error, at the
/// pointer the walk found it at or where the error says, with its
/// [`expected`](Diagnostic::expected) type.
#[cfg_attr(
    not(feature = "js-compat"),
    expect(dead_code, reason = "js-compat seam only")
)]
pub fn diagnostics_of_error(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
    err: &Error,
) -> Vec<Diagnostic> {
    let mut diagnostics = match find(mm, fqn, instance, options, false) {
        Found::NamedType(found) => vec![named_type_diagnostic(&found, err)],
        Found::Walked(found) => match found.first() {
            Some((pointer, first)) if same_check(first, err) => {
                vec![walk_diagnostic(pointer, err)]
            }
            _ => located(err, instance),
        },
        Found::Unread(_) => located(err, instance),
    };
    fill_expected(mm, fqn, instance, &mut diagnostics);
    diagnostics
}

/// [`diagnose`] for a document that is not plain JSON (`undefined`, `-0`,
/// `NaN`, a `Map`, a dayjs), whose verdict `read` gives (the JS binding's
/// `Serializer.fromJSON`), so the error is always `read`'s. `readings` are
/// the document's tagged forms; the diagnostics are [`diagnose`]'s over the
/// first reading whose walk raises the same error, else the error's own
/// (located in the first reading, or at the root when there is none).
#[cfg_attr(
    not(feature = "js-compat"),
    expect(dead_code, reason = "js-compat seam only")
)]
pub fn diagnose_read(
    mm: &ModelManager,
    fqn: Option<&str>,
    readings: &[Value],
    options: &from_json::FromJsonOptions,
    collect_all: bool,
    read: impl FnOnce() -> Result<()>,
) -> Diagnosis {
    const NO_READING: &Value = &Value::Null;
    let primary = readings.first().unwrap_or(NO_READING);
    let checked = match fqn {
        Some(fqn) => {
            validate::check_assignable_to_declaration(mm, fqn, primary).and_then(|()| read())
        }
        None => read(),
    };
    let Err(error) = checked else {
        return Diagnosis {
            report: ValidationReport::default(),
            error: None,
        };
    };
    let report = readings
        .iter()
        .map(|reading| diagnose(mm, fqn, reading, options, collect_all))
        .find(|native| {
            native
                .error
                .as_ref()
                .is_some_and(|found| same_error(found, &error))
        })
        .map_or_else(
            || ValidationReport::new(diagnostics_of_error(mm, fqn, primary, options, &error)),
            |native| native.report,
        );
    Diagnosis {
        report,
        error: Some(error),
    }
}

/// Whether two errors are the same error: kind (the TS exception class),
/// catalogue code, parameters and details.
fn same_error(a: &Error, b: &Error) -> bool {
    same_check(a, b) && a.params() == b.params() && a.details() == b.details()
}

/// Whether two errors come from the same check: kind and catalogue code.
fn same_check(a: &Error, b: &Error) -> bool {
    a.kind() == b.kind() && a.code() == b.code()
}

/// The diagnostics of `err`, an error that stopped the read of `instance`
/// (or one no walk raises), located by what the error names: one per
/// detail at its path; or at the populator path it names; or at the object
/// or keys it is about ([`locate`]); or else at the root.
fn located(err: &Error, instance: &Value) -> Vec<Diagnostic> {
    let mut diagnostics = report_of_error(err).into_diagnostics();
    if !err.details().is_empty() {
        // One diagnostic per detail, in order (`report_of_error`).
        for (diagnostic, detail) in diagnostics.iter_mut().zip(err.details()) {
            diagnostic.expected.clone_from(&detail.expected);
        }
        return diagnostics;
    }
    let param = |wanted: &str| {
        err.params()
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, value)| value.as_str())
    };
    let [diagnostic] = diagnostics.as_mut_slice() else {
        return diagnostics;
    };
    if err.kind() == crate::ErrorKind::TypeNotFound {
        diagnostic.code = DiagnosticCode::TypeNotFound;
    }
    // A populator error names its path, and the type it expected there.
    if param("path").is_some() {
        diagnostic.expected = param("type").map(str::to_string);
        return diagnostics;
    }
    // An error that names the object (or the key) it is about.
    if let Some(pointers) = locate(err, instance) {
        let template = diagnostic.clone();
        return pointers
            .into_iter()
            .map(|pointer| Diagnostic {
                pointer,
                ..template.clone()
            })
            .collect();
    }
    diagnostics
}

/// Where in `instance` an error that names no path is, from what it names
/// instead: the type of the object it is about (an abstract type, a missing
/// identifier, a type that is not found), or the keys it rejects (an
/// unexpected property, the relationship a value is not one for). One
/// pointer per rejected key; `None` when the error names nothing to find.
fn locate(err: &Error, instance: &Value) -> Option<Vec<String>> {
    let param = |wanted: &str| {
        err.params()
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, value)| value.as_str())
    };
    match err.code() {
        "jsonpopulator-validateproperties-unexpectedproperties"
        | "jsonpopulator-getassignableproperties-reservedproperties"
        | "jsonpopulator-getassignableproperties-timestamp" => {
            let owner = param("fqn")?;
            let names: Vec<&str> = match param("properties") {
                Some(list) => list.split(", ").collect(),
                None => vec!["$timestamp"],
            };
            let first = *names.first()?;
            let object = find_object(instance, "", &|map| {
                map.contains_key(first) && class_of(map).is_none_or(|class| class == owner)
            })?;
            Some(
                names
                    .iter()
                    .map(|name| child_pointer(&object, name))
                    .collect(),
            )
        }
        "factory-newinstance-abstracttype"
        | "factory-newinstance-missingidentifier"
        | "factory-newinstance-invalididentifier" => {
            let fqn = format!("{}.{}", param("namespace")?, param("type")?);
            find_object(instance, "", &|map| class_of(map) == Some(fqn.as_str())).map(|p| vec![p])
        }
        "jsonpopulator-visitrelationshipdeclaration-notstringorobject"
        | "jsonpopulator-visitrelationshipdeclaration-notastring"
        | "jsonpopulator-visitrelationshipdeclaration-noclass" => {
            // `RelationshipDeclaration {name=friend, type=..., ...}`.
            let name = param("relationship")?
                .split_once("name=")?
                .1
                .split([',', '}'])
                .next()?;
            let object = find_object(instance, "", &|map| map.contains_key(name))?;
            Some(vec![child_pointer(&object, name)])
        }
        _ if err.kind() == crate::ErrorKind::TypeNotFound => {
            let type_name = param("typeName")?;
            let suffix = format!(".{type_name}");
            find_object(instance, "", &|map| {
                class_of(map).is_some_and(|class| class == type_name || class.ends_with(&suffix))
            })
            .map(|p| vec![p])
        }
        _ => None,
    }
}

/// The `$class` of an object, when it is a string.
fn class_of(map: &crate::json::Map<String, Value>) -> Option<&str> {
    map.get("$class").and_then(Value::as_str)
}

/// The pointer of the first object (depth first, the root first) in `value`
/// that `wanted` accepts.
fn find_object(
    value: &Value,
    pointer: &str,
    wanted: &dyn Fn(&crate::json::Map<String, Value>) -> bool,
) -> Option<String> {
    match value {
        Value::Object(map) => {
            if wanted(map) {
                return Some(pointer.to_string());
            }
            map.iter()
                .find_map(|(key, child)| find_object(child, &child_pointer(pointer, key), wanted))
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(i, child)| find_object(child, &format!("{pointer}/{i}"), wanted)),
        _ => None,
    }
}

/// A JSON Pointer one key deeper than `pointer`, escaped as RFC 6901 says.
fn child_pointer(pointer: &str, key: &str) -> String {
    format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// Fills in each diagnostic's [`expected`](Diagnostic::expected) type that
/// is still `None`, from the model ([`expected_at`]), for the codes that
/// are about a value's type.
fn fill_expected(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    diagnostics: &mut [Diagnostic],
) {
    for diagnostic in diagnostics {
        if diagnostic.expected.is_some()
            || matches!(
                diagnostic.code,
                DiagnosticCode::UndeclaredField
                    | DiagnosticCode::TypeNotFound
                    | DiagnosticCode::EmptyIdentifier
                    | DiagnosticCode::AbstractClass
            )
        {
            continue;
        }
        diagnostic.expected = expected_at(mm, fqn, instance, &diagnostic.pointer);
    }
}

/// The type the model declares at `pointer` in `instance`: `fqn` itself for
/// the root, and otherwise the declared type of the property (or array
/// element) the pointer names, found by walking the declarations down the
/// pointer, each nested object as its own `$class` when it has one. `None`
/// where the pointer leaves the declared properties, or goes through a map
/// or a relationship.
fn expected_at(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    pointer: &str,
) -> Option<String> {
    if pointer.is_empty() {
        return fqn.map(str::to_string);
    }
    let own_class = |value: &Value| {
        value
            .get("$class")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let mut class = own_class(instance).or_else(|| fqn.map(str::to_string))?;
    let mut value = Some(instance);
    let mut segments = pointer
        .strip_prefix('/')?
        .split('/')
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .peekable();
    while let Some(name) = segments.next() {
        let properties = mm.properties(&class).ok()?;
        let (owner, property) = properties.iter().find(|(_, p)| p.name() == name)?;
        let element_type = spell_type(mm, owner, property)?;
        let mut child = value.and_then(|v| v.get(name.as_str()));
        if property.is_array() {
            let Some(index) = segments.next() else {
                return Some(format!("{element_type}[]"));
            };
            child = child.and_then(|v| v.get(index.parse::<usize>().ok()?));
        }
        if segments.peek().is_none() {
            return Some(element_type);
        }
        if property.is_primitive() || property.is_relationship() {
            return None;
        }
        class = child
            .and_then(own_class)
            .unwrap_or_else(|| element_type.clone());
        value = child;
    }
    None
}

/// A property's declared type as the model spells it: the primitive, or the
/// fully qualified name of the type it names (`--> ` before a
/// relationship's). `None` for an enum value, which has no type.
fn spell_type(
    mm: &ModelManager,
    owner_fqn: &str,
    property: &crate::introspect::Property,
) -> Option<String> {
    let name = property.type_name()?;
    if property.is_primitive() {
        return Some(name.to_string());
    }
    let resolved = crate::model_util::get_namespace(Some(owner_fqn))
        .ok()
        .and_then(|ns| mm.resolve_type_name_at(ns, name, None).ok())
        .unwrap_or_else(|| name.to_string());
    Some(if property.is_relationship() {
        format!("--> {resolved}")
    } else {
        resolved
    })
}

/// The diagnostics of a document that could not be read as an instance:
/// one per detail, or one for the error.
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
        _ => (classify_error(err), err.to_string()),
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
mod tests;

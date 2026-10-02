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
    /// What the model expects at [`pointer`](Self::pointer), when it
    /// expects a type there (accordproject/concerto#1239, #1325): the
    /// declared type, as the model spells it (`String`, `String[]`,
    /// `org.acme@1.0.0.Address`, `--> org.acme@1.0.0.Person` for a
    /// relationship). Read from the model alone, it never quotes the
    /// instance. Only the JS binding's `validateInstance` (the js-compat
    /// `diagnose`, task P5-89) fills it in;
    /// [`ModelManager::check_instance`] leaves it `None`.
    pub expected: Option<String>,
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
        collect(self, None, instance, &options.populate_options(false))
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
        collect(self, Some(fqn), instance, &options.populate_options(false))
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

/// The collect-all walk: `instance` read as `Serializer.fromJSON` reads it
/// (`options`, never validating), then every violation found
/// ([`ModelManager::check_instance`] and its `_as` form). A document that
/// cannot be read is reported by that failure alone.
fn collect(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
) -> ValidationReport {
    walk(mm, fqn, instance, options).0
}

/// [`collect`], and whether the document could be read (and so was
/// walked): `false` when the report is that of the error that stopped the
/// read, or of the named type's own check.
fn walk(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
) -> (ValidationReport, bool) {
    if let Some(fqn) = fqn
        && let Some(report) = validate::assignability_diagnostic(mm, fqn, instance)
    {
        return (report, false);
    }
    let unvalidated = from_json::FromJsonOptions {
        validate: false,
        ..options.clone()
    };
    match mm.populate(fqn, instance, unvalidated) {
        Ok(populated) => {
            let target = match fqn {
                Some(fqn) => fqn.to_string(),
                None => populated
                    .get("$class")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            };
            let report = validate::collect_diagnostics(mm, &target, &populated, &options.validator);
            (report, true)
        }
        Err(err) => (report_of_error(&err), false),
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

/// The accordproject/concerto#1239 entry point of the JS binding
/// (concerto-wasm `validateInstance`): validates `instance`, a plain JSON
/// document, as `Serializer.fromJSON` does with `options` (and
/// `validate: true`), as the type `fqn` when one is given (its own `$class`
/// must then be `fqn` or a subtype of it) and as its own `$class` otherwise.
///
/// The verdict is the first-error walk's
/// ([`ModelManager::validate_instance`]), so the instance is valid exactly
/// when `Serializer.fromJSON` would not throw, and an invalid instance's
/// first diagnostic is the one for [`Diagnosis::error`], the error
/// `Serializer.fromJSON` throws ([`diagnostics_of_error`]). With
/// `collect_all`, every other violation the collect-all walk finds follows
/// it. A valid instance costs one walk; the collect-all walk only runs for
/// an invalid one. Every diagnostic gets its
/// [`expected`](Diagnostic::expected) type where the model gives one.
#[cfg_attr(not(feature = "js-compat"), allow(dead_code))]
pub fn diagnose(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
    collect_all: bool,
) -> Diagnosis {
    let checked = from_json::FromJsonOptions {
        validate: true,
        ..options.clone()
    };
    let first = match fqn {
        Some(fqn) => validate::check_assignable_to_declaration(mm, fqn, instance)
            .and_then(|()| mm.populate(Some(fqn), instance, checked)),
        None => mm.populate(None, instance, checked),
    };
    let Err(error) = first else {
        return Diagnosis {
            report: ValidationReport::default(),
            error: None,
        };
    };
    let mut collected = None;
    let mut diagnostics = first_diagnostics(mm, fqn, instance, options, &error, &mut collected);
    if collect_all {
        let (collected, walked) = collected.get_or_insert_with(|| walk(mm, fqn, instance, options));
        // A document that could not be read fails the read (or the named
        // type's check) with the first error itself: there is nothing more
        // to report.
        let more = if *walked {
            collected.diagnostics()
        } else {
            &[]
        };
        for diagnostic in more {
            if !diagnostics
                .iter()
                .any(|d| d.pointer == diagnostic.pointer && d.code == diagnostic.code)
            {
                diagnostics.push(diagnostic.clone());
            }
        }
    }
    fill_expected(mm, fqn, instance, &mut diagnostics);
    Diagnosis {
        report: ValidationReport::new(diagnostics),
        error: Some(error),
    }
}

/// The diagnostics of `err`, an error `Serializer.fromJSON` (or
/// [`diagnose`]'s first-error walk) raised for `instance` with `options`:
/// what the JS binding attaches to the exception as its `details`
/// (accordproject/concerto#1325). One per #1273 detail, or one for the
/// error, located by the collect-all walk when the error itself names no
/// path, and with its [`expected`](Diagnostic::expected) type.
#[cfg_attr(not(feature = "js-compat"), allow(dead_code))]
pub fn diagnostics_of_error(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
    err: &Error,
) -> Vec<Diagnostic> {
    let mut diagnostics = first_diagnostics(mm, fqn, instance, options, err, &mut None);
    fill_expected(mm, fqn, instance, &mut diagnostics);
    diagnostics
}

/// The diagnostics of the first error, `err`. A `ResourceValidator` error
/// names no path, so its location is that of the first diagnostic of the
/// same code (and, when the error names one, the same property) the
/// collect-all walk finds, which `collected` keeps for the caller ([`walk`]). The
/// message stays the error's own.
fn first_diagnostics(
    mm: &ModelManager,
    fqn: Option<&str>,
    instance: &Value,
    options: &from_json::FromJsonOptions,
    err: &Error,
    collected: &mut Option<(ValidationReport, bool)>,
) -> Vec<Diagnostic> {
    // The named type is checked first: an instance of another type fails
    // there, whatever else is wrong with it.
    if let Some(fqn) = fqn
        && let Some(report) = validate::assignability_diagnostic(mm, fqn, instance)
    {
        let mut diagnostics = report.into_diagnostics();
        for diagnostic in &mut diagnostics {
            diagnostic.message = err.to_string();
        }
        return diagnostics;
    }
    let mut diagnostics = report_of_error(err).into_diagnostics();
    if !err.details().is_empty() {
        // One diagnostic per #1273 detail, in order (`report_of_error`).
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
    // A `ResourceValidator` error: the collect-all walk's diagnostic of the
    // same code, for the property the error names when that is one.
    let property = param("fieldName").or_else(|| param("propertyName"));
    let (collected, _) = collected.get_or_insert_with(|| walk(mm, fqn, instance, options));
    let same_code = |d: &&Diagnostic| d.code == diagnostic.code;
    let found = collected
        .diagnostics()
        .iter()
        .filter(same_code)
        .find(|d| property.is_some_and(|name| last_segment(&d.pointer).as_deref() == Some(name)))
        .or_else(|| collected.diagnostics().iter().find(same_code));
    if let Some(found) = found {
        diagnostic.pointer.clone_from(&found.pointer);
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
fn class_of(map: &serde_json::Map<String, Value>) -> Option<&str> {
    map.get("$class").and_then(Value::as_str)
}

/// The pointer of the first object (depth first, the root first) in `value`
/// that `wanted` accepts.
fn find_object(
    value: &Value,
    pointer: &str,
    wanted: &dyn Fn(&serde_json::Map<String, Value>) -> bool,
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

/// The last reference token of a JSON Pointer, unescaped; `None` for the
/// root pointer.
fn last_segment(pointer: &str) -> Option<String> {
    let (_, last) = pointer.rsplit_once('/')?;
    Some(last.replace("~1", "/").replace("~0", "~"))
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

    // ---- `diagnose` (task P5-89, accordproject/concerto#1239) ----

    use crate::ErrorKind;
    use serde_json::json;

    const MM: &str = "concerto.metamodel@1.0.0";

    fn prop(class: &str, name: &str, extra: Value) -> Value {
        let mut node = json!({
            "$class": format!("{MM}.{class}"),
            "name": name,
            "isArray": false,
            "isOptional": false,
        });
        for (k, v) in extra.as_object().into_iter().flatten() {
            node[k] = v.clone();
        }
        node
    }

    fn type_ref(name: &str) -> Value {
        json!({ "type": { "$class": format!("{MM}.TypeIdentifier"), "name": name } })
    }

    /// `org.acme@1.0.0`: `Address { city }`, `Person` identified by
    /// `email`, with an `address`, `tags: String[]`, an optional `age`, a
    /// `colour` enum and a `friend` relationship; `Employee extends Person`;
    /// an asset `Car`.
    fn manager() -> ModelManager {
        let mut mm = ModelManager::new().unwrap();
        mm.load_model(
            &json!({
                "$class": format!("{MM}.Model"),
                "namespace": "org.acme@1.0.0",
                "declarations": [
                    { "$class": format!("{MM}.ConceptDeclaration"), "name": "Address", "isAbstract": false,
                      "properties": [prop("StringProperty", "city", json!({}))] },
                    { "$class": format!("{MM}.EnumDeclaration"), "name": "Colour",
                      "properties": [{ "$class": format!("{MM}.EnumProperty"), "name": "RED" }] },
                    { "$class": format!("{MM}.ParticipantDeclaration"), "name": "Person", "isAbstract": false,
                      "identified": { "$class": format!("{MM}.IdentifiedBy"), "name": "email" },
                      "properties": [
                        prop("StringProperty", "email", json!({})),
                        prop("ObjectProperty", "address", type_ref("Address")),
                        prop("StringProperty", "tags", json!({ "isArray": true, "isOptional": true })),
                        prop("IntegerProperty", "age", json!({ "isOptional": true })),
                        prop("ObjectProperty", "colour", { let mut t = type_ref("Colour"); t["isOptional"] = json!(true); t }),
                        prop("RelationshipProperty", "friend", { let mut t = type_ref("Person"); t["isOptional"] = json!(true); t }),
                      ] },
                    { "$class": format!("{MM}.ParticipantDeclaration"), "name": "Employee", "isAbstract": false,
                      "superType": { "$class": format!("{MM}.TypeIdentifier"), "name": "Person" },
                      "properties": [] },
                    { "$class": format!("{MM}.AssetDeclaration"), "name": "Car", "isAbstract": false,
                      "identified": { "$class": format!("{MM}.IdentifiedBy"), "name": "vin" },
                      "properties": [prop("StringProperty", "vin", json!({}))] },
                ]
            }),
            None,
        )
        .unwrap();
        mm
    }

    fn person(extra: Value) -> Value {
        let mut p = json!({
            "$class": "org.acme@1.0.0.Person",
            "email": "a@example.com",
            "address": { "$class": "org.acme@1.0.0.Address", "city": "Paris" }
        });
        for (k, v) in extra.as_object().into_iter().flatten() {
            p[k] = v.clone();
        }
        p
    }

    fn options() -> from_json::FromJsonOptions {
        from_json::FromJsonOptions::default()
    }

    #[test]
    fn diagnose_a_valid_instance_reports_nothing() {
        let mm = manager();
        let d = diagnose(&mm, None, &person(json!({})), &options(), true);
        assert!(d.report.is_valid() && d.report.diagnostics().is_empty());
        assert!(d.error.is_none());
        let d = diagnose(
            &mm,
            Some("org.acme@1.0.0.Person"),
            &json!({ "$class": "org.acme@1.0.0.Employee", "email": "e@x", "address": { "city": "Rome" } }),
            &options(),
            true,
        );
        assert!(d.error.is_none(), "{:?}", d.error);
    }

    #[test]
    fn diagnose_puts_the_thrown_error_first_and_locates_it() {
        let mm = manager();
        // `address.city` is missing and `colour` is not a `Colour`: the
        // first-error walk stops at one, the collect-all walk finds both.
        let instance =
            person(json!({ "address": { "$class": "org.acme@1.0.0.Address" }, "colour": "BLUE" }));
        let thrown = mm
            .validate_instance(&instance, &crate::instance::ValidationOptions::default())
            .unwrap_err();
        let all = diagnose(&mm, None, &instance, &options(), true);
        let first = diagnose(&mm, None, &instance, &options(), false);
        let error = all.error.clone().unwrap();
        assert_eq!(error, thrown);
        assert_eq!(first.report.diagnostics().len(), 1);
        assert_eq!(first.report.diagnostics()[0], all.report.diagnostics()[0]);
        let code = report_of_error(&thrown).diagnostics()[0].code;
        assert_eq!(all.report.diagnostics()[0].code, code);
        assert_eq!(all.report.diagnostics()[0].message, thrown.to_string());
        assert!(all.report.diagnostics().len() >= 2, "{:?}", all.report);
        // No two diagnostics share a location and a code.
        let d = all.report.diagnostics();
        for (i, a) in d.iter().enumerate() {
            assert!(
                !d[i + 1..]
                    .iter()
                    .any(|b| a.pointer == b.pointer && a.code == b.code)
            );
        }
        assert_eq!(
            diagnostics_of_error(&mm, None, &instance, &options(), &error),
            first.report.into_diagnostics()
        );
    }

    #[test]
    fn diagnose_a_missing_nested_property_names_its_path_and_type() {
        let mm = manager();
        let instance = person(json!({ "address": { "$class": "org.acme@1.0.0.Address" } }));
        let d = diagnose(&mm, None, &instance, &options(), true);
        assert_eq!(d.error.unwrap().kind(), ErrorKind::Validation);
        let first = &d.report.diagnostics()[0];
        assert_eq!(first.code, DiagnosticCode::MissingRequiredProperty);
        assert_eq!(first.pointer, "/address/city");
        assert_eq!(first.expected.as_deref(), Some("String"));
        assert_eq!(first.severity, Severity::Error);
    }

    #[test]
    fn diagnose_spells_the_expected_type_of_arrays_relationships_and_enums() {
        let mm = manager();
        let expected =
            |pointer: &str| expected_at(&mm, None, &person(json!({ "tags": ["a"] })), pointer);
        assert_eq!(expected("/tags").as_deref(), Some("String[]"));
        assert_eq!(expected("/tags/0").as_deref(), Some("String"));
        assert_eq!(
            expected("/friend").as_deref(),
            Some("--> org.acme@1.0.0.Person")
        );
        assert_eq!(
            expected("/colour").as_deref(),
            Some("org.acme@1.0.0.Colour")
        );
        assert_eq!(
            expected("/address").as_deref(),
            Some("org.acme@1.0.0.Address")
        );
        assert_eq!(expected("/address/city").as_deref(), Some("String"));
        assert_eq!(expected("/email/x"), None);
        assert_eq!(expected("/undeclared"), None);
        assert_eq!(expected(""), None);
        assert_eq!(
            expected_at(&mm, Some("org.acme@1.0.0.Person"), &json!({}), "").as_deref(),
            Some("org.acme@1.0.0.Person")
        );
        assert_eq!(last_segment("/a~1b/c~0d").as_deref(), Some("c~d"));
        assert_eq!(last_segment(""), None);
    }

    #[test]
    fn diagnose_a_wrong_type_takes_the_populator_path_and_type() {
        let mm = manager();
        let d = diagnose(
            &mm,
            None,
            &person(json!({ "age": "old" })),
            &options(),
            true,
        );
        let first = &d.report.diagnostics()[0];
        assert_eq!(first.code, DiagnosticCode::TypeViolation);
        assert_eq!(first.pointer, "/age");
        assert_eq!(first.expected.as_deref(), Some("Integer"));
    }

    #[test]
    fn diagnose_checks_the_class_against_the_named_type() {
        let mm = manager();
        let car = json!({ "$class": "org.acme@1.0.0.Car", "vin": "1" });
        let d = diagnose(&mm, Some("org.acme@1.0.0.Person"), &car, &options(), true);
        assert_eq!(d.error.unwrap().kind(), ErrorKind::Validation);
        assert_eq!(d.report.diagnostics().len(), 1);
        let first = &d.report.diagnostics()[0];
        assert_eq!(first.code, DiagnosticCode::NotAssignable);
        assert_eq!(first.pointer, "");
        assert_eq!(first.expected.as_deref(), Some("org.acme@1.0.0.Person"));
        let unknown = json!({ "$class": "org.acme@1.0.0.Nope" });
        let d = diagnose(
            &mm,
            Some("org.acme@1.0.0.Person"),
            &unknown,
            &options(),
            true,
        );
        assert_eq!(d.error.unwrap().kind(), ErrorKind::TypeNotFound);
        assert_eq!(d.report.diagnostics()[0].code, DiagnosticCode::TypeNotFound);
        assert_eq!(d.report.diagnostics()[0].expected, None);
    }

    #[test]
    fn diagnose_reports_each_1273_detail_with_its_expected_type() {
        let mm = manager();
        let strict = from_json::FromJsonOptions {
            reject_unknown_keys: true,
            reject_required_null: true,
            ..options()
        };
        let d = diagnose(
            &mm,
            None,
            &person(json!({ "address": { "city": null } })),
            &strict,
            false,
        );
        assert_eq!(d.error.as_ref().unwrap().details().len(), 1);
        let first = &d.report.diagnostics()[0];
        assert_eq!(first.code, DiagnosticCode::TypeViolation);
        assert_eq!(first.pointer, "/address/city");
        assert_eq!(first.expected.as_deref(), Some("String"));
        let d = diagnose(
            &mm,
            None,
            &person(json!({ "zip": 1, "zap": 2 })),
            &strict,
            false,
        );
        let codes: Vec<_> = d.report.diagnostics().iter().map(|d| d.code).collect();
        assert_eq!(
            codes,
            [
                DiagnosticCode::UndeclaredField,
                DiagnosticCode::UndeclaredField
            ]
        );
        assert_eq!(d.report.diagnostics()[0].expected, None);
    }

    #[test]
    fn diagnose_an_instance_with_no_class() {
        let mm = manager();
        let d = diagnose(&mm, None, &json!({ "email": "a" }), &options(), true);
        assert_eq!(d.error.unwrap().kind(), ErrorKind::InvalidArgument);
        assert_eq!(d.report.diagnostics()[0].code, DiagnosticCode::NotResource);
        // Read as the named type instead.
        let d = diagnose(
            &mm,
            Some("org.acme@1.0.0.Address"),
            &json!({ "city": "Oslo" }),
            &options(),
            true,
        );
        assert!(d.error.is_none());
    }
    #[test]
    fn diagnose_locates_an_error_that_names_no_path() {
        let mm = manager();
        let first = |instance: Value| {
            let d = diagnose(&mm, None, &instance, &options(), false);
            d.report
                .diagnostics()
                .iter()
                .map(|d| (d.code, d.pointer.clone()))
                .collect::<Vec<_>>()
        };
        // Undeclared keys, nested, one diagnostic each.
        assert_eq!(
            first(person(
                json!({ "address": { "$class": "org.acme@1.0.0.Address", "city": "P", "a/b": 1, "c": 2 } })
            )),
            [
                (DiagnosticCode::UndeclaredField, "/address/a~1b".to_string()),
                (DiagnosticCode::UndeclaredField, "/address/c".to_string()),
            ]
        );
        // A type that is not found, nested.
        assert_eq!(
            first(person(
                json!({ "address": { "$class": "org.acme@1.0.0.Nope" } })
            )),
            [(DiagnosticCode::TypeNotFound, "/address".to_string())]
        );
        // A value that is not a relationship.
        assert_eq!(
            first(person(json!({ "friend": 42 }))),
            [(DiagnosticCode::NotRelationship, "/friend".to_string())]
        );
        // An invalid enum value: the TS error names the enum, not the
        // property, so the walk's diagnostic of the same code is used.
        assert_eq!(
            first(person(json!({ "colour": "BLUE" }))),
            [(DiagnosticCode::InvalidEnumValue, "/colour".to_string())]
        );
        // An abstract type and a missing identifier, at the root.
        assert_eq!(
            first(json!({ "$class": "org.acme@1.0.0.Car", "vin": "" })),
            [(DiagnosticCode::EmptyIdentifier, String::new())]
        );
        let unknown = json!({ "$class": "org.acme@1.0.0.Nope" });
        assert_eq!(
            first(unknown),
            [(DiagnosticCode::TypeNotFound, String::new())]
        );
        assert_eq!(
            find_object(&json!([{ "a": 1 }, { "b": { "c": 1 } }]), "", &|m| m
                .contains_key("c")),
            Some("/1/b".to_string())
        );
    }
}

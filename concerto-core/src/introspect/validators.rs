//! Field and scalar validators: a port of `validator.ts`,
//! `numbervalidator.ts`, `stringvalidator.ts` and
//! `collectionsizevalidator.ts` from the TypeScript reference.
//!
//! P0-04b trial ported `NumberValidator`, with the part of
//! `Validator.reportError` it needs. P2-02 ports `StringValidator` (with the
//! `regress` crate for ECMAScript-compatible regex semantics, PORTING.md
//! section 3 and OD-4) and `CollectionSizeValidator`, plus every validator's
//! `compatibleWith`. Wiring `ScalarDeclaration` and `Property` to build these
//! instead of running their own ad hoc checks (`introspect::check_pattern`
//! and friends) is left to the tasks that own those types (P2-04, P2-05):
//! this module only has to exist and behave correctly for them to call into.

use std::fmt;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ecma;
use crate::error::{ContractError, ErrorKind, ValidatorReport};
use crate::model_manager::ValidatedElement;

/// concerto-util `ErrorCodes.DEFAULT_VALIDATOR_EXCEPTION`, the default
/// `errorType` of `Validator.reportError`.
const DEFAULT_VALIDATOR_EXCEPTION: &str = "DefaultValidatorException";

/// concerto-util `ErrorCodes.REGEX_VALIDATOR_EXCEPTION`: the `errorType`
/// `StringValidator` passes when the pattern fails to compile.
///
/// TS: StringValidator.constructor (src/introspect/stringvalidator.ts)
const REGEX_VALIDATOR_EXCEPTION: &str = "RegexValidatorException";

/// A validator attached to a field or a scalar declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum Validator {
    /// A numeric range.
    Number(NumberValidator),
    /// A string regex and/or length range.
    String(StringValidator),
    /// A collection (array or map) size range.
    CollectionSize(CollectionSizeValidator),
}

impl Validator {
    /// Dispatches to the variant's own `compatibleWith`.
    ///
    /// TS: Validator.compatibleWith (src/introspect/validator.ts), overridden
    /// per subclass.
    pub fn compatible_with(&self, other: Option<&Validator>) -> bool {
        match self {
            Self::Number(v) => v.compatible_with(other),
            Self::String(v) => v.compatible_with(other),
            Self::CollectionSize(v) => v.compatible_with(other),
        }
    }
}

/// Builds the error `Validator.reportError` throws: the message with the
/// instance identifier and the element's fully qualified name in front. The
/// name is read only here, as TS reads it only when it reports.
///
/// TS: Validator.reportError (src/introspect/validator.ts)
fn report_error<F: ValidatedElement>(
    field: &F,
    id: Option<&str>,
    error_type: &'static str,
    code: &'static str,
    params: Vec<(&'static str, String)>,
) -> F::Error {
    let fqn = match field.fully_qualified_name() {
        Ok(fqn) => fqn,
        Err(err) => return err,
    };
    let mut err = ContractError::new(ErrorKind::Validator, code, params);
    err.validator = Some(ValidatorReport {
        // `'…`' + id + '`…'`: a null id prints as "null".
        id: id.unwrap_or("null").to_string(),
        fqn,
        error_type,
    });
    err.into()
}

/// A validator that keeps non-null numbers between two bounds, inclusive.
///
/// A bound is the AST value as given (`None` is JS `null`). The metamodel
/// makes it a number, but TS keeps whatever the AST holds and compares it
/// with JS semantics, so the port does too.
///
/// It serialises to its snapshot, `{lowerBound, upperBound}`: the fields the
/// TS object holds, which the WASM view caches (PORTING.md 1.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NumberValidator {
    lower_bound: Option<Value>,
    upper_bound: Option<Value>,
}

/// `ast.<key>` when `ast` has it as an own property, else JS `null`. A
/// present `null` is also `null`.
fn bound(ast: &Value, key: &str) -> Option<Value> {
    match ast.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.clone()),
    }
}

impl NumberValidator {
    /// Builds the validator from its AST (`{lower, upper}`), checking the
    /// bounds and the element's default value.
    ///
    /// TS: NumberValidator.constructor (src/introspect/numbervalidator.ts)
    pub fn new<F: ValidatedElement>(field: &F, ast: &Value) -> Result<Self, F::Error> {
        // The hasOwnProperty guards: an absent bound stays null.
        let lower_bound = bound(ast, "lower");
        let upper_bound = bound(ast, "upper");

        match (&lower_bound, &upper_bound) {
            (None, None) => {
                return Err(report_error(
                    field,
                    None,
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "numbervalidator-constructor-nobounds",
                    Vec::new(),
                ));
            }
            (Some(lower), Some(upper)) if ecma::greater_than(lower, upper) => {
                return Err(report_error(
                    field,
                    None,
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "numbervalidator-constructor-lowerhigherthanupper",
                    Vec::new(),
                ));
            }
            _ => {}
        }

        if let Some(value) = field.default_value()? {
            if let Some(lower) = &lower_bound
                && ecma::less_than(&value, lower)
            {
                return Err(report_error(
                    field,
                    None,
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "numbervalidator-constructor-outsidelowerbound",
                    vec![
                        ("value", ecma::to_js_string(&value)),
                        ("lowerBound", ecma::to_js_string(lower)),
                    ],
                ));
            }
            if let Some(upper) = &upper_bound
                && ecma::greater_than(&value, upper)
            {
                return Err(report_error(
                    field,
                    None,
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "numbervalidator-constructor-outsideupperbound",
                    vec![
                        ("value", ecma::to_js_string(&value)),
                        ("upperBound", ecma::to_js_string(upper)),
                    ],
                ));
            }
        }

        Ok(Self {
            lower_bound,
            upper_bound,
        })
    }

    /// The lower bound, or `None` (JS `null`) when there is none.
    ///
    /// TS: NumberValidator.getLowerBound (src/introspect/numbervalidator.ts)
    pub fn lower_bound(&self) -> Option<&Value> {
        self.lower_bound.as_ref()
    }

    /// The upper bound, or `None` (JS `null`) when there is none.
    ///
    /// TS: NumberValidator.getUpperBound (src/introspect/numbervalidator.ts)
    pub fn upper_bound(&self) -> Option<&Value> {
        self.upper_bound.as_ref()
    }

    /// Checks an instance value. `None` is JS `null`, which is always
    /// accepted. `field` is the element the validator is attached to, read
    /// only to report an error.
    ///
    /// TS: NumberValidator.validate (src/introspect/numbervalidator.ts)
    pub fn validate<F: ValidatedElement>(
        &self,
        field: &F,
        identifier: Option<&str>,
        value: Option<f64>,
    ) -> Result<(), F::Error> {
        let Some(value) = value else {
            return Ok(());
        };
        if let Some(lower) = &self.lower_bound
            && ecma::number_less_than(value, lower)
        {
            return Err(report_error(
                field,
                identifier,
                DEFAULT_VALIDATOR_EXCEPTION,
                "numbervalidator-constructor-outsidelowerbound",
                vec![
                    ("value", ecma::number_to_string(value)),
                    ("lowerBound", ecma::to_js_string(lower)),
                ],
            ));
        }
        if let Some(upper) = &self.upper_bound
            && ecma::number_greater_than(value, upper)
        {
            return Err(report_error(
                field,
                identifier,
                DEFAULT_VALIDATOR_EXCEPTION,
                "numbervalidator-constructor-outsideupperbound",
                vec![
                    ("value", ecma::number_to_string(value)),
                    ("upperBound", ecma::to_js_string(upper)),
                ],
            ));
        }
        Ok(())
    }

    /// Whether every value this validator accepts is accepted by `other`.
    /// Anything but a number validator is incompatible.
    ///
    /// TS: NumberValidator.compatibleWith (src/introspect/numbervalidator.ts)
    pub fn compatible_with(&self, other: Option<&Validator>) -> bool {
        let Some(Validator::Number(other)) = other else {
            return false;
        };
        match (&self.lower_bound, &other.lower_bound) {
            (None, Some(_)) => return false,
            (Some(this), Some(other)) if ecma::less_than(this, other) => return false,
            _ => {}
        }
        match (&self.upper_bound, &other.upper_bound) {
            (None, Some(_)) => return false,
            (Some(this), Some(other)) if ecma::greater_than(this, other) => return false,
            _ => {}
        }
        true
    }
}

/// `NumberValidator lower: <lower> upper: <upper>`, with JS `ToString`
/// (`null` for a missing bound).
///
/// TS: NumberValidator.toString (src/introspect/numbervalidator.ts)
impl fmt::Display for NumberValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = |bound: &Option<Value>| {
            bound
                .as_ref()
                .map_or_else(|| "null".to_string(), ecma::to_js_string)
        };
        write!(
            f,
            "NumberValidator lower: {} upper: {}",
            text(&self.lower_bound),
            text(&self.upper_bound)
        )
    }
}

/// A validator that keeps a collection (array or map) within a size range,
/// inclusive.
///
/// TS: CollectionSizeValidator (src/introspect/collectionsizevalidator.ts)
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionSizeValidator {
    min_size: Option<f64>,
    max_size: Option<f64>,
}

impl CollectionSizeValidator {
    /// Builds the validator from its AST (`{minSize, maxSize}`).
    ///
    /// The metamodel's `min_size`/`max_size` already collapse an absent bound
    /// and an explicit `null` one into `None` alike (OD-3), which matches TS
    /// `validator.minSize ?? null` exactly: unlike `StringValidator`'s length
    /// bounds (below), `CollectionSizeValidator` reads its AST with `??`, not
    /// a strict `=== null` check, so there is no absent/null distinction to
    /// lose here.
    ///
    /// TS: CollectionSizeValidator.constructor
    /// (src/introspect/collectionsizevalidator.ts)
    pub fn new<F: ValidatedElement>(
        field: &F,
        validator: &mm::CollectionSizeValidator,
    ) -> Result<Self, F::Error> {
        let min_size = validator.min_size;
        let max_size = validator.max_size;

        if min_size.is_none() && max_size.is_none() {
            return Err(report_error(
                field,
                Some(&field.name()?),
                DEFAULT_VALIDATOR_EXCEPTION,
                "collectionsizevalidator-constructor-nosize",
                Vec::new(),
            ));
        } else if min_size.unwrap_or(0.0) < 0.0 || max_size.unwrap_or(0.0) < 0.0 {
            return Err(report_error(
                field,
                Some(&field.name()?),
                DEFAULT_VALIDATOR_EXCEPTION,
                "collectionsizevalidator-constructor-negativesize",
                Vec::new(),
            ));
        } else if let Some((min, max)) = min_size.zip(max_size)
            && min > max
        {
            // When either bound is absent, this is fine: no need to check
            // whether minSize > maxSize.
            return Err(report_error(
                field,
                Some(&field.name()?),
                DEFAULT_VALIDATOR_EXCEPTION,
                "collectionsizevalidator-constructor-mingreaterthanmax",
                Vec::new(),
            ));
        }

        Ok(Self { min_size, max_size })
    }

    /// The minimum size, or `None` (JS `null`) when there is none.
    ///
    /// TS: CollectionSizeValidator.getMinSize
    /// (src/introspect/collectionsizevalidator.ts)
    pub fn min_size(&self) -> Option<f64> {
        self.min_size
    }

    /// The maximum size, or `None` (JS `null`) when there is none.
    ///
    /// TS: CollectionSizeValidator.getMaxSize
    /// (src/introspect/collectionsizevalidator.ts)
    pub fn max_size(&self) -> Option<f64> {
        self.max_size
    }

    /// Checks an instance collection's size.
    ///
    /// TS: CollectionSizeValidator.validate
    /// (src/introspect/collectionsizevalidator.ts)
    pub fn validate<F: ValidatedElement>(
        &self,
        field: &F,
        identifier: Option<&str>,
        value: f64,
    ) -> Result<(), F::Error> {
        if let Some(min) = self.min_size
            && value < min
        {
            return Err(report_error(
                field,
                identifier,
                DEFAULT_VALIDATOR_EXCEPTION,
                "collectionsizevalidator-validate-belowminsize",
                vec![("minSize", ecma::number_to_string(min))],
            ));
        }
        if let Some(max) = self.max_size
            && value > max
        {
            return Err(report_error(
                field,
                identifier,
                DEFAULT_VALIDATOR_EXCEPTION,
                "collectionsizevalidator-validate-abovemaxsize",
                vec![("maxSize", ecma::number_to_string(max))],
            ));
        }
        Ok(())
    }

    /// Whether every collection size this validator accepts is accepted by
    /// `other`. Anything but a collection size validator is incompatible.
    ///
    /// TS: CollectionSizeValidator.compatibleWith
    /// (src/introspect/collectionsizevalidator.ts)
    pub fn compatible_with(&self, other: Option<&Validator>) -> bool {
        let Some(Validator::CollectionSize(other)) = other else {
            return false;
        };
        match (self.min_size, other.min_size) {
            (None, Some(_)) => return false,
            (Some(this), Some(other)) if this < other => return false,
            _ => {}
        }
        match (self.max_size, other.max_size) {
            (None, Some(_)) => return false,
            (Some(this), Some(other)) if this > other => return false,
            _ => {}
        }
        true
    }
}

/// A compiled string-regex validator: the pattern and flags as given, plus
/// the `regress` engine built from them (PORTING.md section 3, OD-4).
///
/// `regress::Regex` has no `PartialEq`, so equality and `Display` are defined
/// over the source pattern and flags only, which is everything TS's
/// `RegExp.toString()` (and so `compatibleWith`, which compares `pattern` and
/// `flags` directly) ever observes.
#[derive(Debug, Clone)]
struct CompiledRegex {
    pattern: String,
    flags: String,
    regex: regress::Regex,
}

impl PartialEq for CompiledRegex {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern && self.flags == other.flags
    }
}

/// `String(regex)`: `/pattern/flags`.
impl fmt::Display for CompiledRegex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}/{}", self.pattern, self.flags)
    }
}

impl CompiledRegex {
    /// `regex.lastIndex = 0; regex.test(value)` (then implicitly reset
    /// again): `regress` is stateless, so there is no `lastIndex` to leak
    /// between calls, and a `g` flag behaves exactly like a fresh, unanchored
    /// search every time, matching TS's reset-before-and-after discipline
    /// (`StringValidator.matchesRegex`).
    ///
    /// `regress` silently ignores `y` (sticky), which it does not implement,
    /// so sticky matching is done here: the leftmost match (if any) must
    /// start at offset 0, exactly what a sticky match at `lastIndex = 0`
    /// requires.
    fn matches(&self, value: &str) -> bool {
        match self.regex.find(value) {
            Some(found) => !self.flags.contains('y') || found.range.start == 0,
            None => false,
        }
    }
}

/// A validator that enforces a string's length and/or that it matches a
/// regular expression.
///
/// TS: StringValidator (src/introspect/stringvalidator.ts)
#[derive(Debug, Clone, PartialEq)]
pub struct StringValidator {
    // The metamodel's `min_length`/`max_length` collapse an absent bound and
    // an explicit `null` one into `None` alike (OD-3). TS reads
    // `lengthValidator?.minLength` (no `??`), so it can tell an explicit
    // `null` apart from an absent key for one thing only: whether *both*
    // bounds are exactly `null` (rather than both simply absent) trips the
    // "must be specified" error. No fixture or oracle case is known to reach
    // that corner (a `lengthValidator` object with neither key set at all),
    // so this port accepts the OD-3 collapse here rather than reading the raw
    // AST, and reports the same "must be specified" error either way.
    min_length: Option<f64>,
    max_length: Option<f64>,
    regex: Option<CompiledRegex>,
}

impl StringValidator {
    /// Builds the validator from its AST: an optional regex (`{pattern,
    /// flags}`) and an optional length range (`{minLength, maxLength}`), in
    /// that order (TS checks the length range, then the regex, then the
    /// element's default value).
    ///
    /// TS: StringValidator.constructor (src/introspect/stringvalidator.ts)
    pub fn new<F: ValidatedElement>(
        field: &F,
        validator: Option<&mm::StringRegexValidator>,
        length_validator: Option<&mm::StringLengthValidator>,
    ) -> Result<Self, F::Error> {
        let mut min_length = None;
        let mut max_length = None;

        if let Some(lv) = length_validator {
            min_length = lv.min_length;
            max_length = lv.max_length;

            if min_length.is_none() && max_length.is_none() {
                return Err(report_error(
                    field,
                    Some(&field.name()?),
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "stringvalidator-constructor-invalidlength",
                    Vec::new(),
                ));
            } else if min_length.unwrap_or(0.0) < 0.0 || max_length.unwrap_or(0.0) < 0.0 {
                return Err(report_error(
                    field,
                    Some(&field.name()?),
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "stringvalidator-constructor-negativelength",
                    Vec::new(),
                ));
            } else if let Some((min, max)) = min_length.zip(max_length)
                && min > max
            {
                // When either bound is absent, this is fine: no need to
                // check minLength > maxLength.
                return Err(report_error(
                    field,
                    Some(&field.name()?),
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "stringvalidator-constructor-mingreaterthanmax",
                    Vec::new(),
                ));
            }
        }

        let regex = match validator {
            None => None,
            Some(v) => match regress::Regex::with_flags(v.pattern.as_str(), v.flags.as_str()) {
                Ok(regex) => Some(CompiledRegex {
                    pattern: v.pattern.clone(),
                    flags: v.flags.clone(),
                    regex,
                }),
                Err(error) => {
                    // OD-4: V8's wording for the reasons `regress` can map;
                    // any other reason is an `engine` divergence.
                    let message = format!(
                        "Invalid regular expression: /{}/{}: {error}",
                        v.pattern, v.flags
                    );
                    return Err(report_error(
                        field,
                        Some(&field.name()?),
                        REGEX_VALIDATOR_EXCEPTION,
                        "stringvalidator-constructor-invalidregex",
                        vec![("message", message)],
                    ));
                }
            },
        };

        let built = Self {
            min_length,
            max_length,
            regex,
        };

        // `if(this.field?.ast?.defaultValue) { this.validate(field.getName(), this.field.ast.defaultValue); }`:
        // a plain JS truthy check, so a `null`, `false`, `0` or `""` default
        // skips the check, and only a string default reaches `.length`/regex
        // logic below (a non-string default is a model TS itself does not
        // guard against; this port skips the check for one rather than
        // guessing at JS's coercions).
        if let Some(value) = field.default_value()?
            && ecma::is_truthy(&value)
            && let Some(text) = value.as_str()
        {
            built.validate(field, Some(&field.name()?), Some(text))?;
        }

        Ok(built)
    }

    /// The minimum length, or `None` (JS `null`/`undefined`) when there is
    /// none.
    ///
    /// TS: StringValidator.getMinLength (src/introspect/stringvalidator.ts)
    pub fn min_length(&self) -> Option<f64> {
        self.min_length
    }

    /// The maximum length, or `None` (JS `null`/`undefined`) when there is
    /// none.
    ///
    /// TS: StringValidator.getMaxLength (src/introspect/stringvalidator.ts)
    pub fn max_length(&self) -> Option<f64> {
        self.max_length
    }

    /// The regex text, as `String(regex)` renders it (`/pattern/flags`), or
    /// `None` when no regex is declared.
    ///
    /// TS: StringValidator.getRegex (src/introspect/stringvalidator.ts)
    pub fn regex(&self) -> Option<String> {
        self.regex.as_ref().map(ToString::to_string)
    }

    /// Checks an instance value. `None` is JS `null`, which is always
    /// accepted. String length is measured in UTF-16 code units, as JS
    /// `String.prototype.length` counts them.
    ///
    /// TS: StringValidator.validate (src/introspect/stringvalidator.ts)
    pub fn validate<F: ValidatedElement>(
        &self,
        field: &F,
        identifier: Option<&str>,
        value: Option<&str>,
    ) -> Result<(), F::Error> {
        let Some(value) = value else {
            return Ok(());
        };
        let length = value.encode_utf16().count() as f64;
        if let Some(min) = self.min_length
            && length < min
        {
            return Err(report_error(
                field,
                identifier,
                DEFAULT_VALIDATOR_EXCEPTION,
                "stringvalidator-validate-belowminlength",
                vec![
                    ("value", value.to_string()),
                    ("minLength", ecma::number_to_string(min)),
                ],
            ));
        }
        if let Some(max) = self.max_length
            && length > max
        {
            return Err(report_error(
                field,
                identifier,
                DEFAULT_VALIDATOR_EXCEPTION,
                "stringvalidator-validate-abovemaxlength",
                vec![
                    ("value", value.to_string()),
                    ("maxLength", ecma::number_to_string(max)),
                ],
            ));
        }
        if let Some(regex) = &self.regex
            && !regex.matches(value)
        {
            return Err(report_error(
                field,
                identifier,
                DEFAULT_VALIDATOR_EXCEPTION,
                "stringvalidator-validate-regexmismatch",
                vec![
                    ("value", value.to_string()),
                    ("regex", regex.to_string()),
                ],
            ));
        }
        Ok(())
    }

    /// Whether every value this validator accepts is accepted by `other`:
    /// the same pattern and flags (or neither has one), and this validator's
    /// length range is no wider than `other`'s. Anything but a string
    /// validator is incompatible.
    ///
    /// TS: StringValidator.compatibleWith (src/introspect/stringvalidator.ts)
    pub fn compatible_with(&self, other: Option<&Validator>) -> bool {
        let Some(Validator::String(other)) = other else {
            return false;
        };
        // `this.validator?.pattern !== other.validator?.pattern` and the same
        // for `flags`: compared as the raw regex source, not the compiled
        // engine, so two validators with no regex at all (`undefined !==
        // undefined` is `false`) are equal on this count.
        fn pattern(v: &Option<CompiledRegex>) -> Option<&str> {
            v.as_ref().map(|r| r.pattern.as_str())
        }
        fn flags(v: &Option<CompiledRegex>) -> Option<&str> {
            v.as_ref().map(|r| r.flags.as_str())
        }
        if pattern(&self.regex) != pattern(&other.regex) || flags(&self.regex) != flags(&other.regex)
        {
            return false;
        }

        match (self.min_length, other.min_length) {
            (None, Some(_)) => return false,
            (Some(this), Some(other)) if this < other => return false,
            _ => {}
        }
        match (self.max_length, other.max_length) {
            (None, Some(_)) => return false,
            (Some(this), Some(other)) if this > other => return false,
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    //! Ported from `test/introspect/stringvalidator.js` and
    //! `test/introspect/collectionsizevalidator.js` (P2-02): the constructor,
    //! `validate` and `compatibleWith` cases, run here directly over the
    //! Rust types rather than through `sinon.createStubInstance(Field)`. Not
    //! ported: cases that assert on a live JS `RegExp` object (`getRegex()`
    //! returning something with a settable `lastIndex`) — `regress` is
    //! stateless, so [`CompiledRegex::matches`] has no `lastIndex` to leak in
    //! the first place, which is the property those cases check for.

    use super::*;
    use crate::error::{ConcertoError, Result};
    use crate::introspect::FullyQualified;

    /// A minimal `ValidatedElement`, standing in for `sinon.createStubInstance(Field)`.
    struct TestField {
        fqn: &'static str,
        name: &'static str,
        default_value: Option<Value>,
    }

    impl TestField {
        fn new(fqn: &'static str, name: &'static str) -> Self {
            Self {
                fqn,
                name,
                default_value: None,
            }
        }

        fn with_default(mut self, value: Value) -> Self {
            self.default_value = Some(value);
            self
        }
    }

    impl FullyQualified for TestField {
        type Error = ConcertoError;

        fn fully_qualified_name(&self) -> Result<String> {
            Ok(self.fqn.to_string())
        }
    }

    impl ValidatedElement for TestField {
        fn default_value(&self) -> Result<Option<Value>> {
            Ok(self.default_value.clone())
        }

        fn name(&self) -> Result<String> {
            Ok(self.name.to_string())
        }
    }

    fn field() -> TestField {
        TestField::new("org.acme.myField", "myField")
    }

    fn regex_ast(pattern: &str, flags: &str) -> mm::StringRegexValidator {
        serde_json::from_value(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
            "pattern": pattern,
            "flags": flags,
        }))
        .expect("valid StringRegexValidator AST")
    }

    fn length_ast(min: Option<f64>, max: Option<f64>) -> mm::StringLengthValidator {
        let mut ast = serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringLengthValidator" });
        if let Some(min) = min {
            ast["minLength"] = min.into();
        }
        if let Some(max) = max {
            ast["maxLength"] = max.into();
        }
        serde_json::from_value(ast).expect("valid StringLengthValidator AST")
    }

    fn size_ast(min: Option<f64>, max: Option<f64>) -> mm::CollectionSizeValidator {
        let mut ast = serde_json::json!({ "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator" });
        if let Some(min) = min {
            ast["minSize"] = min.into();
        }
        if let Some(max) = max {
            ast["maxSize"] = max.into();
        }
        serde_json::from_value(ast).expect("valid CollectionSizeValidator AST")
    }

    fn string_validator(
        pattern: Option<(&str, &str)>,
        length: Option<(Option<f64>, Option<f64>)>,
    ) -> Result<StringValidator> {
        let regex = pattern.map(|(p, f)| regex_ast(p, f));
        let length = length.map(|(min, max)| length_ast(min, max));
        StringValidator::new(&field(), regex.as_ref(), length.as_ref())
    }

    // ---- StringValidator: #constructor ----

    #[test]
    fn string_validator_rejects_an_invalid_regex() {
        let err = string_validator(Some(("^[A-z", "")), None).unwrap_err();
        assert!(err.to_string().contains("Validator error for field"));
    }

    #[test]
    fn string_validator_rejects_length_with_no_bounds() {
        let err = string_validator(Some(("^[A-z]", "")), Some((None, None))).unwrap_err();
        assert!(
            err.to_string()
                .contains("Invalid string length, minLength and-or maxLength must be specified")
        );
    }

    #[test]
    fn string_validator_rejects_min_length_above_max_length() {
        let err = string_validator(Some(("^[A-z]", "")), Some((Some(200.0), Some(100.0)))).unwrap_err();
        assert!(
            err.to_string()
                .contains("minLength must be less than or equal to maxLength")
        );
    }

    #[test]
    fn string_validator_rejects_negative_lengths() {
        for (min, max) in [(Some(-2.0), None), (None, Some(-100.0)), (Some(-1.0), Some(-100.0))] {
            let err = string_validator(None, Some((min, max))).unwrap_err();
            assert!(
                err.to_string()
                    .contains("minLength and-or maxLength must be positive integers"),
                "{min:?}/{max:?} should be rejected"
            );
        }
    }

    #[test]
    fn string_validator_rejects_a_default_value_shorter_than_min_length() {
        let f = field().with_default(serde_json::json!("abc"));
        let err = StringValidator::new(&f, None, Some(&length_ast(Some(5.0), Some(10.0)))).unwrap_err();
        assert!(
            err.to_string()
                .contains("The string length of 'abc' should be at least 5 characters.")
        );
    }

    #[test]
    fn string_validator_rejects_a_default_value_longer_than_max_length() {
        let f = field().with_default(serde_json::json!("abcdefgh"));
        let err = StringValidator::new(&f, None, Some(&length_ast(Some(2.0), Some(5.0)))).unwrap_err();
        assert!(
            err.to_string()
                .contains("The string length of 'abcdefgh' should not exceed 5 characters.")
        );
    }

    #[test]
    fn string_validator_accepts_a_default_value_matching_length_and_pattern() {
        let f = field().with_default(serde_json::json!("ABC"));
        assert!(
            StringValidator::new(
                &f,
                Some(&regex_ast("^[A-Z]{3,5}$", "")),
                Some(&length_ast(Some(3.0), Some(5.0)))
            )
            .is_ok()
        );
    }

    // ---- StringValidator: #validate ----

    #[test]
    fn string_validator_ignores_a_null_string() {
        let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "")), None).unwrap();
        assert!(v.validate(&field(), Some("id"), None).is_ok());
    }

    #[test]
    fn string_validator_validates_a_matching_string() {
        let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "")), None).unwrap();
        assert!(v.validate(&field(), Some("id"), Some("AB1234567")).is_ok());
    }

    #[test]
    fn string_validator_detects_a_mismatched_string() {
        let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "")), None).unwrap();
        let err = v.validate(&field(), Some("id"), Some("xyz")).unwrap_err();
        assert!(err.to_string().contains("Validator error for field `id`. org.acme.myField"));
    }

    #[test]
    fn string_validator_repeatedly_validates_a_matching_string_with_a_global_regex() {
        let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "g")), None).unwrap();
        for _ in 0..3 {
            assert!(v.validate(&field(), Some("id"), Some("AB1234567")).is_ok());
        }
    }

    #[test]
    fn string_validator_repeatedly_rejects_a_mismatched_string_with_a_global_regex() {
        let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "g")), None).unwrap();
        for _ in 0..2 {
            assert!(v.validate(&field(), Some("id"), Some("xyz")).is_err());
        }
    }

    #[test]
    fn string_validator_repeatedly_validates_a_matching_string_with_a_sticky_regex() {
        let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "y")), None).unwrap();
        for _ in 0..2 {
            assert!(v.validate(&field(), Some("id"), Some("AB1234567")).is_ok());
        }
    }

    #[test]
    fn string_validator_a_sticky_regex_only_matches_at_the_start() {
        // Not in the TS suite directly, but is exactly what `y` means: a
        // match that does not start at offset 0 must be rejected even though
        // the same pattern (without `y`) would find it further along.
        let v = string_validator(Some(("[0-9]+", "y")), None).unwrap();
        assert!(v.validate(&field(), Some("id"), Some("abc123")).is_err());
        assert!(v.validate(&field(), Some("id"), Some("123abc")).is_ok());
    }

    #[test]
    fn string_validator_validates_escaped_characters() {
        let v = string_validator(Some((r"^[\\]*\n$", "")), None).unwrap();
        assert!(v.validate(&field(), Some("id"), Some("\\\\\n")).is_ok());
        assert!(v.validate(&field(), Some("id"), Some("\\hi!\n")).is_err());
    }

    #[test]
    fn string_validator_validates_a_unicode_string() {
        let pattern = r"^(\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\\u[0-9A-Fa-f]{4})(?:\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\\u[0-9A-Fa-f]{4}|\p{Mn}|\p{Mc}|\p{Nd}|\p{Pc}|\u200C|\u200D)*$";
        let v = string_validator(Some((pattern, "u")), None).unwrap();
        assert!(v.validate(&field(), Some("id"), Some("AB1234567")).is_ok());
        assert!(v.validate(&field(), Some("id"), Some("1FOO")).is_err());
    }

    #[test]
    fn string_validator_length_only_bounds() {
        let min_only = string_validator(None, Some((Some(2.0), None))).unwrap();
        assert!(min_only.validate(&field(), Some("id"), Some("AB1234567455455455")).is_ok());
        let err = min_only.validate(&field(), Some("id"), Some("w")).unwrap_err();
        assert!(err.to_string().contains("The string length of 'w' should be at least 2 characters."));
        let err = min_only.validate(&field(), Some("id"), Some("")).unwrap_err();
        assert!(err.to_string().contains("The string length of '' should be at least 2 characters."));

        let max_only = string_validator(None, Some((None, Some(10.0)))).unwrap();
        assert!(max_only.validate(&field(), Some("id"), Some("ABCD123456")).is_ok());
        assert!(max_only.validate(&field(), Some("id"), Some("")).is_ok());
        let err = max_only.validate(&field(), Some("id"), Some("ABCD1234567")).unwrap_err();
        assert!(err.to_string().contains("should not exceed 10 characters."));
    }

    #[test]
    fn string_validator_length_takes_precedence_over_regex() {
        let v = string_validator(Some(("^[A-z]{1,100}$", "")), Some((Some(1.0), Some(10.0)))).unwrap();
        let err = v
            .validate(&field(), Some("id"), Some("AbCdefghijklmksadada"))
            .unwrap_err();
        assert!(err.to_string().contains("should not exceed 10 characters."));
    }

    // ---- StringValidator: #compatibleWith ----

    #[test]
    fn string_validator_is_incompatible_with_a_number_validator() {
        let other = NumberValidator::new(&field(), &serde_json::json!({"lower": -1, "upper": 1})).unwrap();
        let v = string_validator(Some(("foo", "")), Some((Some(1.0), Some(100.0)))).unwrap();
        assert!(!v.compatible_with(Some(&Validator::Number(other))));
    }

    #[test]
    fn string_validator_compatible_with_same_pattern_and_flags() {
        let other = string_validator(Some(("foo", "")), None).unwrap();
        let v = string_validator(Some(("foo", "")), None).unwrap();
        assert!(v.compatible_with(Some(&Validator::String(other))));
    }

    #[test]
    fn string_validator_incompatible_with_a_changed_pattern() {
        let other = string_validator(Some(("bar", "")), None).unwrap();
        let v = string_validator(Some(("foo", "")), None).unwrap();
        assert!(!v.compatible_with(Some(&Validator::String(other))));
    }

    #[test]
    fn string_validator_incompatible_with_changed_flags() {
        let other = string_validator(Some(("foo", "i")), None).unwrap();
        let v = string_validator(Some(("foo", "g")), None).unwrap();
        assert!(!v.compatible_with(Some(&Validator::String(other))));
    }

    #[test]
    fn string_validator_length_compatibility() {
        let wide = || string_validator(None, Some((Some(1.0), Some(100.0)))).unwrap();
        assert!(wide().compatible_with(Some(&Validator::String(wide()))));

        let narrow = string_validator(None, Some((None, Some(10.0)))).unwrap();
        assert!(!wide().compatible_with(Some(&Validator::String(narrow))));

        let this_tighter_min = string_validator(None, Some((Some(1.0), Some(100.0)))).unwrap();
        let other_tighter_min = string_validator(None, Some((Some(10.0), Some(100.0)))).unwrap();
        assert!(!this_tighter_min.compatible_with(Some(&Validator::String(other_tighter_min))));

        let this_wider_max = string_validator(None, Some((Some(1.0), Some(100.0)))).unwrap();
        let other_tighter_max = string_validator(None, Some((Some(1.0), Some(10.0)))).unwrap();
        assert!(!this_wider_max.compatible_with(Some(&Validator::String(other_tighter_max))));

        let this_no_max = string_validator(None, Some((Some(1.0), None))).unwrap();
        let other_has_max = string_validator(None, Some((Some(1.0), Some(10.0)))).unwrap();
        assert!(!this_no_max.compatible_with(Some(&Validator::String(other_has_max))));
    }

    // ---- CollectionSizeValidator: #constructor ----

    #[test]
    fn collection_size_validator_reads_both_bounds() {
        let v = CollectionSizeValidator::new(&field(), &size_ast(Some(1.0), Some(10.0))).unwrap();
        assert_eq!(v.min_size(), Some(1.0));
        assert_eq!(v.max_size(), Some(10.0));
    }

    #[test]
    fn collection_size_validator_min_only() {
        let v = CollectionSizeValidator::new(&field(), &size_ast(Some(3.0), None)).unwrap();
        assert_eq!(v.min_size(), Some(3.0));
        assert_eq!(v.max_size(), None);
    }

    #[test]
    fn collection_size_validator_rejects_no_bounds() {
        let err = CollectionSizeValidator::new(&field(), &size_ast(None, None)).unwrap_err();
        assert!(err.to_string().contains("minSize and/or maxSize must be specified"));
    }

    #[test]
    fn collection_size_validator_rejects_negative_bounds() {
        let err = CollectionSizeValidator::new(&field(), &size_ast(Some(-1.0), None)).unwrap_err();
        assert!(err.to_string().contains("positive integers"));
        let err = CollectionSizeValidator::new(&field(), &size_ast(None, Some(-2.0))).unwrap_err();
        assert!(err.to_string().contains("positive integers"));
    }

    #[test]
    fn collection_size_validator_rejects_min_above_max() {
        let err = CollectionSizeValidator::new(&field(), &size_ast(Some(5.0), Some(2.0))).unwrap_err();
        assert!(err.to_string().contains("minSize must be less than or equal to maxSize"));
    }

    #[test]
    fn collection_size_validator_allows_min_equal_max_and_zero() {
        let v = CollectionSizeValidator::new(&field(), &size_ast(Some(3.0), Some(3.0))).unwrap();
        assert_eq!(v.min_size(), Some(3.0));
        let v = CollectionSizeValidator::new(&field(), &size_ast(Some(0.0), Some(5.0))).unwrap();
        assert_eq!(v.min_size(), Some(0.0));
    }

    // ---- CollectionSizeValidator: #validate ----

    #[test]
    fn collection_size_validator_validate() {
        let v = CollectionSizeValidator::new(&field(), &size_ast(Some(2.0), Some(5.0))).unwrap();
        assert!(v.validate(&field(), Some("id"), 3.0).is_ok());
        let err = v.validate(&field(), Some("id"), 1.0).unwrap_err();
        assert!(err.to_string().contains("at least 2 elements"));
        let err = v.validate(&field(), Some("id"), 6.0).unwrap_err();
        assert!(err.to_string().contains("no more than 5 elements"));
    }

    // ---- CollectionSizeValidator: #compatibleWith ----

    #[test]
    fn collection_size_validator_compatible_with() {
        let v = |min: Option<f64>, max: Option<f64>| {
            CollectionSizeValidator::new(&field(), &size_ast(min, max)).unwrap()
        };

        assert!(!v(Some(1.0), None).compatible_with(None));
        assert!(v(Some(2.0), Some(5.0)).compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), Some(6.0))))));
        assert!(!v(Some(1.0), Some(5.0)).compatible_with(Some(&Validator::CollectionSize(v(Some(3.0), Some(5.0))))));
        assert!(!v(Some(1.0), Some(5.0)).compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), Some(3.0))))));
        assert!(!v(None, Some(10.0)).compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), Some(10.0))))));
        assert!(!v(Some(1.0), None).compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), Some(10.0))))));
        assert!(v(Some(1.0), Some(5.0)).compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), None)))));
        assert!(v(Some(1.0), Some(5.0)).compatible_with(Some(&Validator::CollectionSize(v(None, Some(5.0))))));
        assert!(v(Some(2.0), Some(8.0)).compatible_with(Some(&Validator::CollectionSize(v(Some(2.0), Some(8.0))))));
    }
}

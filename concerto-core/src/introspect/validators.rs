//! Field and scalar validators: a port of `validator.ts`,
//! `numbervalidator.ts`, `stringvalidator.ts` and
//! `collectionsizevalidator.ts` from the TypeScript reference.
//!
//! The port includes the part of `Validator.reportError` they need, and
//! every validator's `compatibleWith`. Regexes use the `regress` crate for
//! ECMAScript semantics (PORTING.md section 3). `ScalarDeclaration` and
//! `Property` (`check_bound_validators`) build these.

use crate::hash::SeededHashMap;
use std::cell::RefCell;
use std::fmt;

use crate::json::Value;
use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
#[cfg(feature = "js-compat")]
use concerto_metamodel::utils::class_name;
use serde::{Deserialize, Serialize};

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
#[non_exhaustive]
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

/// Builds the error `Validator.reportError` throws, the instance identifier
/// and the element's name (read only here, as in TS) in front. `kind` is
/// the class (BC-39): [`ErrorKind::IllegalModel`] for a check at model
/// load, [`ErrorKind::Validation`] for an instance value; both keep the
/// `errorType` (TS 5.0.0 threw a plain `BaseException`).
///
/// TS: Validator.reportError (src/introspect/validator.ts)
fn report_error<F: ValidatedElement>(
    field: &F,
    kind: ErrorKind,
    id: Option<&str>,
    error_type: &'static str,
    code: &'static str,
    params: Vec<(&'static str, String)>,
) -> F::Error {
    let fqn = match field.fully_qualified_name() {
        Ok(fqn) => fqn,
        Err(err) => return err,
    };
    let mut err = ContractError::new(kind, code, params);
    err.validator = Some(ValidatorReport {
        // `'…`' + id + '`…'`: a null id prints as "null".
        id: id.unwrap_or("null").to_string(),
        fqn,
        error_type,
    });
    err.into()
}

/// A validator sub-object's `key`, read as TS's `CollectionSizeValidator`/
/// `StringValidator` constructors read their bounds (an untyped property
/// read) and coerced with ECMAScript `ToNumber` ([`ecma::to_number`]).
/// `None` for a missing key or JSON `null`; any other value is `Some`, a
/// non-number becoming `NaN`, which never crosses a bound, as in TS.
#[cfg(feature = "js-compat")]
fn validator_number_field(ast: &Value, key: &str) -> Option<f64> {
    match ast.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => Some(ecma::to_number(value)),
    }
}

/// Whether `min > max` by JS's abstract relational comparison over the raw
/// AST values (`ecma::greater_than`), as TS compares them untouched: two
/// non-numeric strings compare lexicographically. `min`/`max` are the
/// coerced readings; without `raw` (the validator's AST sub-node) this is
/// the plain `f64` comparison.
fn bounds_out_of_order(
    raw: Option<&Value>,
    min_key: &str,
    max_key: &str,
    min: f64,
    max: f64,
) -> bool {
    match raw.and_then(|raw| raw.get(min_key).zip(raw.get(max_key))) {
        // A present, non-null pair on the raw AST: TS's own comparison.
        Some((min_raw, max_raw)) if !min_raw.is_null() && !max_raw.is_null() => {
            ecma::greater_than(min_raw, max_raw)
        }
        // No raw AST: the `f64` comparison.
        _ => min > max,
    }
}

/// A validator sub-object's string field (`pattern`/`flags`), read as
/// `new RegExp(validator.pattern, validator.flags)` reads it: a missing key
/// is the empty string, anything else goes through `ToString`
/// ([`ecma::to_js_string`]).
#[cfg(feature = "js-compat")]
fn validator_string_field(ast: &Value, key: &str) -> String {
    match ast.get(key) {
        None => String::new(),
        Some(value) => ecma::to_js_string(value),
    }
}

/// Builds a [`mm::CollectionSizeValidator`] from the raw `sizeValidator` AST
/// node with `validator_number_field`'s coercion instead of `serde`'s
/// strict decode, as TS's `new CollectionSizeValidator(this,
/// this.ast.sizeValidator)` reads the node with no type check. `None` for an
/// absent or `null` node. `pub` for concerto-wasm's
/// `collectionSizeValidatorNew`, TS's other call site for the constructor.
#[cfg(feature = "js-compat")]
pub fn size_validator_from_ast(raw: Option<&Value>) -> Option<mm::CollectionSizeValidator> {
    let raw = raw.filter(|value| !value.is_null())?;
    Some(mm::CollectionSizeValidator {
        _class: class_name(&validator_string_field(raw, "$class")),
        min_size: validator_number_field(raw, "minSize"),
        max_size: validator_number_field(raw, "maxSize"),
    })
}

/// [`size_validator_from_ast`], for a `StringProperty`/`StringScalar`'s own
/// `lengthValidator` (`mm::StringLengthValidator`, `{minLength, maxLength}`).
/// `pub` for the same reason as [`size_validator_from_ast`]: concerto-wasm's
/// `stringValidatorNew` binding is TS's own call site for
/// `new StringValidator(...)` and needs the same leniency.
#[cfg(feature = "js-compat")]
pub fn length_validator_from_ast(raw: Option<&Value>) -> Option<mm::StringLengthValidator> {
    let raw = raw.filter(|value| !value.is_null())?;
    Some(mm::StringLengthValidator {
        _class: class_name(&validator_string_field(raw, "$class")),
        min_length: length_bound_field(raw, "minLength"),
        max_length: length_bound_field(raw, "maxLength"),
    })
}

/// [`validator_number_field`], for `StringLengthValidator`'s
/// `minLength`/`maxLength`. BC-40: an absent key and an explicit `null` are
/// both "no bound", so a length validator with neither bound is rejected
/// (TS 5.0.0 rejected only two explicit `null`s).
#[cfg(feature = "js-compat")]
fn length_bound_field(ast: &Value, key: &str) -> Option<f64> {
    validator_number_field(ast, key)
}

/// [`size_validator_from_ast`], for a `StringProperty`/`StringScalar`'s own
/// `validator` (`{pattern, flags}`, coerced by `validator_string_field`).
/// `pub` for the same reason as [`size_validator_from_ast`].
#[cfg(feature = "js-compat")]
pub fn regex_validator_from_ast(raw: Option<&Value>) -> Option<mm::StringRegexValidator> {
    let raw = raw.filter(|value| !value.is_null())?;
    Some(mm::StringRegexValidator {
        _class: class_name(&validator_string_field(raw, "$class")),
        pattern: validator_string_field(raw, "pattern"),
        flags: validator_string_field(raw, "flags"),
    })
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
#[cfg(feature = "js-compat")]
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
    #[cfg(feature = "js-compat")]
    pub fn new<F: ValidatedElement>(field: &F, ast: &Value) -> Result<Self, F::Error> {
        // The hasOwnProperty guards: an absent bound stays null.
        Self::checked(field, bound(ast, "lower"), bound(ast, "upper"))
    }

    /// [`NumberValidator::new`] from typed bounds: a property's or a
    /// scalar's `{lower, upper}` as the strict read gives them, with the
    /// same checks and errors. `new` stays for the AST a caller hands in as
    /// JSON (the WASM binding's standalone validator).
    pub(crate) fn from_bounds<F: ValidatedElement>(
        field: &F,
        lower: Option<f64>,
        upper: Option<f64>,
    ) -> Result<Self, F::Error> {
        let bound = |b: Option<f64>| b.and_then(serde_json::Number::from_f64).map(Value::Number);
        Self::checked(field, bound(lower), bound(upper))
    }

    /// The constructor's checks, over its bounds.
    fn checked<F: ValidatedElement>(
        field: &F,
        lower_bound: Option<Value>,
        upper_bound: Option<Value>,
    ) -> Result<Self, F::Error> {
        match (&lower_bound, &upper_bound) {
            (None, None) => {
                return Err(report_error(
                    field,
                    ErrorKind::IllegalModel,
                    None,
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "numbervalidator-constructor-nobounds",
                    Vec::new(),
                ));
            }
            (Some(lower), Some(upper)) if ecma::greater_than(lower, upper) => {
                return Err(report_error(
                    field,
                    ErrorKind::IllegalModel,
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
                    ErrorKind::IllegalModel,
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
                    ErrorKind::IllegalModel,
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

    js_compat_pub! {
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
                    ErrorKind::Validation,
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
                    ErrorKind::Validation,
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
    js_compat_pub! {
        /// Builds the validator from its AST (`{minSize, maxSize}`).
        ///
        /// An absent bound and an explicit `null` are both `None`, as TS's
        /// `validator.minSize ?? null` reads them.
        ///
        /// TS: CollectionSizeValidator.constructor
        /// (src/introspect/collectionsizevalidator.ts)
        pub fn new<F: ValidatedElement>(
            field: &F,
            validator: &mm::CollectionSizeValidator,
            raw: Option<&Value>,
        ) -> Result<Self, F::Error> {
            let min_size = validator.min_size;
            let max_size = validator.max_size;

            if min_size.is_none() && max_size.is_none() {
                return Err(report_error(
                    field,
                    ErrorKind::IllegalModel,
                    Some(&field.name()?),
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "collectionsizevalidator-constructor-nosize",
                    Vec::new(),
                ));
            } else if min_size.unwrap_or(0.0) < 0.0 || max_size.unwrap_or(0.0) < 0.0 {
                return Err(report_error(
                    field,
                    ErrorKind::IllegalModel,
                    Some(&field.name()?),
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "collectionsizevalidator-constructor-negativesize",
                    Vec::new(),
                ));
            } else if let Some((min, max)) = min_size.zip(max_size)
                && bounds_out_of_order(raw, "minSize", "maxSize", min, max)
            {
                // When either bound is absent, this is fine: no need to check
                // whether minSize > maxSize.
                return Err(report_error(
                    field,
                    ErrorKind::IllegalModel,
                    Some(&field.name()?),
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "collectionsizevalidator-constructor-mingreaterthanmax",
                    Vec::new(),
                ));
            }

            Ok(Self { min_size, max_size })
        }
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

    js_compat_pub! {
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
                    ErrorKind::Validation,
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
                    ErrorKind::Validation,
                    identifier,
                    DEFAULT_VALIDATOR_EXCEPTION,
                    "collectionsizevalidator-validate-abovemaxsize",
                    vec![("maxSize", ecma::number_to_string(max))],
                ));
            }
            Ok(())
        }
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
/// the `regress` engine built from them (PORTING.md section 3).
///
/// Equality and `Display` cover the source pattern and flags only, all
/// that TS's `RegExp.toString()` and `compatibleWith` observe. The parts
/// are shared, so a rebuild reuses the cached compilation
/// ([`compile_regex`]) without copying either string.
#[derive(Debug, Clone)]
struct CompiledRegex {
    pattern: std::sync::Arc<str>,
    flags: std::sync::Arc<str>,
    regex: std::sync::Arc<regress::Regex>,
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

thread_local! {
    /// Compiled `StringValidator` regexes by flags, then pattern (nested so
    /// that a lookup borrows both strings). Only successful compilations are
    /// kept, so an error is always worded by a fresh compile.
    static REGEX_CACHE: RefCell<RegexCache> =
        RefCell::new(RegexCache::default());
}

/// [`REGEX_CACHE`]'s map: flags, then pattern, to the compiled regex.
type RegexCache = SeededHashMap<Box<str>, SeededHashMap<Box<str>, CompiledRegex>>;

/// The cache is cleared when it reaches this many entries, so a process
/// that sees an unbounded stream of distinct patterns stays bounded.
const REGEX_CACHE_LIMIT: usize = 1024;

/// `regress::Regex::with_flags(pattern, flags)`, memoised in [`REGEX_CACHE`].
fn compile_regex(pattern: &str, flags: &str) -> std::result::Result<CompiledRegex, regress::Error> {
    let cached = REGEX_CACHE.with(|cache| {
        cache
            .borrow()
            .get(flags)
            .and_then(|by_pattern| by_pattern.get(pattern))
            .cloned()
    });
    if let Some(regex) = cached {
        return Ok(regex);
    }
    let regex = CompiledRegex {
        pattern: pattern.into(),
        flags: flags.into(),
        regex: std::sync::Arc::new(regress::Regex::with_flags(pattern, flags)?),
    };
    REGEX_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache
            .values()
            .map(|by_pattern| by_pattern.len())
            .sum::<usize>()
            >= REGEX_CACHE_LIMIT
        {
            cache.clear();
        }
        cache
            .entry(flags.into())
            .or_default()
            .insert(pattern.into(), regex.clone());
    });
    Ok(regex)
}

impl CompiledRegex {
    /// `regex.lastIndex = 0; regex.test(value)`: `regress` is stateless, so
    /// a `g` flag behaves like a fresh search every time, as TS's reset
    /// gives. `regress` ignores `y` (sticky), so a sticky match is checked
    /// here: the leftmost match must start at offset 0.
    fn matches(&self, value: &str) -> bool {
        match self.regex.find(value) {
            Some(found) => !self.flags.contains('y') || found.range.start == 0,
            None => false,
        }
    }
}

/// Maps a `regress` compile-error reason to V8's own wording, where known:
/// both implement ECMAScript regex syntax but word the same syntax error
/// differently. Any reason not in this table keeps `regress`'s own text,
/// an `engine` divergence (PORTING.md 3.2).
fn v8_regex_reason(regress_reason: &str) -> &str {
    match regress_reason {
        // `(` with no closing `)`. Checked against the frozen TS 5.0.0
        // reference (`migration/oracle/reference`): `new RegExp('(')` throws
        // `SyntaxError: Invalid regular expression: /(/: Unterminated group`.
        "Unbalanced parenthesis" => "Unterminated group",
        other => other,
    }
}

/// Whether `flags` is a set of flags the JS `RegExp` constructor accepts:
/// each character one of `d`, `g`, `i`, `m`, `s`, `u`, `v`, `y`; no character
/// repeated; and `u`/`v` not combined (they select incompatible Unicode
/// modes).
///
/// PORTING.md section 3.2 ("Flags"): "With any other flag, the JS constructor
/// throws `Invalid flags supplied to RegExp constructor '<flags>'`."
fn valid_js_regex_flags(flags: &str) -> bool {
    const FLAGS: &str = "dgimsuvy";
    // `u` and `v`: bits 5 and 6 of `FLAGS`.
    const UNICODE_MODES: u8 = (1 << 5) | (1 << 6);
    let mut seen = 0u8;
    for c in flags.chars() {
        let Some(bit) = FLAGS.find(c) else {
            return false;
        };
        if seen & (1 << bit) != 0 {
            return false;
        }
        seen |= 1 << bit;
    }
    seen & UNICODE_MODES != UNICODE_MODES
}

/// A validator that enforces a string's length and/or that it matches a
/// regular expression.
///
/// TS: StringValidator (src/introspect/stringvalidator.ts)
#[derive(Debug, Clone, PartialEq)]
pub struct StringValidator {
    // An absent bound and an explicit `null` are both `None` (BC-40).
    min_length: Option<f64>,
    max_length: Option<f64>,
    regex: Option<CompiledRegex>,
}

impl StringValidator {
    js_compat_pub! {
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
            raw_length_validator: Option<&Value>,
        ) -> Result<Self, F::Error> {
            let mut min_length = None;
            let mut max_length = None;

            if let Some(lv) = length_validator {
                min_length = lv.min_length;
                max_length = lv.max_length;

                if min_length.is_none() && max_length.is_none() {
                    return Err(report_error(
                        field,
                        ErrorKind::IllegalModel,
                        Some(&field.name()?),
                        DEFAULT_VALIDATOR_EXCEPTION,
                        "stringvalidator-constructor-invalidlength",
                        Vec::new(),
                    ));
                } else if min_length.unwrap_or(0.0) < 0.0 || max_length.unwrap_or(0.0) < 0.0 {
                    return Err(report_error(
                        field,
                        ErrorKind::IllegalModel,
                        Some(&field.name()?),
                        DEFAULT_VALIDATOR_EXCEPTION,
                        "stringvalidator-constructor-negativelength",
                        Vec::new(),
                    ));
                } else if let Some((min, max)) = min_length.zip(max_length)
                    && bounds_out_of_order(raw_length_validator, "minLength", "maxLength", min, max)
                {
                    // When either bound is absent, this is fine: no need to
                    // check minLength > maxLength.
                    return Err(report_error(
                        field,
                        ErrorKind::IllegalModel,
                        Some(&field.name()?),
                        DEFAULT_VALIDATOR_EXCEPTION,
                        "stringvalidator-constructor-mingreaterthanmax",
                        Vec::new(),
                    ));
                }
            }

            let regex = match validator {
                None => None,
                Some(v) => {
                    // PORTING.md 3.2 ("Flags"): `new RegExp` rejects a
                    // duplicate, unrecognised or `u`-with-`v` flag before
                    // reading `pattern`; `regress` ignores unknown flags, so
                    // the check is made here.
                    if !valid_js_regex_flags(v.flags.as_str()) {
                        let message =
                            format!("Invalid flags supplied to RegExp constructor '{}'", v.flags);
                        return Err(report_error(
                            field,
                            ErrorKind::IllegalModel,
                            Some(&field.name()?),
                            REGEX_VALIDATOR_EXCEPTION,
                            "stringvalidator-constructor-invalidregex",
                            vec![("message", message)],
                        ));
                    }
                    match compile_regex(v.pattern.as_str(), v.flags.as_str()) {
                        Ok(regex) => Some(regex),
                        Err(error) => {
                            // V8's wording for the reasons `regress` can map;
                            // any other reason is an `engine` divergence.
                            let message = format!(
                                "Invalid regular expression: /{}/{}: {}",
                                v.pattern,
                                v.flags,
                                v8_regex_reason(&error.to_string())
                            );
                            return Err(report_error(
                                field,
                                ErrorKind::IllegalModel,
                                Some(&field.name()?),
                                REGEX_VALIDATOR_EXCEPTION,
                                "stringvalidator-constructor-invalidregex",
                                vec![("message", message)],
                            ));
                        }
                    }
                }
            };

            let built = Self {
                min_length,
                max_length,
                regex,
            };
            built.check_default(field)?;
            Ok(built)
        }
    }

    js_compat_pub! {
        /// This validator, as [`StringValidator::new`] builds it from the
        /// same validator nodes for another element `field`: the same
        /// bounds and regex, with `field`'s own default value checked
        /// against them. For a scalar's validator ([`super::scalar::ScalarValidator`])
        /// read for a property of that scalar type.
        pub fn for_field<F: ValidatedElement>(&self, field: &F) -> Result<Self, F::Error> {
            self.check_default(field)?;
            Ok(self.clone())
        }
    }

    /// The end of [`StringValidator::new`]: the element's own default value
    /// checked against this validator.
    fn check_default<F: ValidatedElement>(&self, field: &F) -> Result<(), F::Error> {
        // `if(this.field?.ast?.defaultValue) { this.validate(...) }`: a JS
        // truthy check, and only a string default is checked (TS does not
        // guard a non-string one). A default outside the validator is a
        // model error (BC-39).
        if let Some(value) = field.default_value()?
            && ecma::is_truthy(&value)
            && let Some(text) = value.as_str()
        {
            self.check(
                field,
                ErrorKind::IllegalModel,
                Some(&field.name()?),
                Some(text),
            )?;
        }
        Ok(())
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

    /// Whether a value matches the regex; `true` when there is none.
    ///
    /// TS: StringValidator.matchesRegex (src/introspect/stringvalidator.ts),
    /// ported as a helper for `Factory.newResource`'s identifier check
    /// (PORTING.md 7.2).
    pub fn matches_regex(&self, value: &str) -> bool {
        self.regex.as_ref().is_none_or(|regex| regex.matches(value))
    }

    js_compat_pub! {
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
            self.check(field, ErrorKind::Validation, identifier, value)
        }
    }

    /// [`StringValidator::validate`], reporting a failure as `kind`: an
    /// instance value is a [`ErrorKind::Validation`] error, and the
    /// constructor's default-value check an [`ErrorKind::IllegalModel`] one
    /// (BC-39).
    fn check<F: ValidatedElement>(
        &self,
        field: &F,
        kind: ErrorKind,
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
                kind,
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
                kind,
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
                kind,
                identifier,
                DEFAULT_VALIDATOR_EXCEPTION,
                "stringvalidator-validate-regexmismatch",
                vec![("value", value.to_string()), ("regex", regex.to_string())],
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
            v.as_ref().map(|r| &*r.pattern)
        }
        fn flags(v: &Option<CompiledRegex>) -> Option<&str> {
            v.as_ref().map(|r| &*r.flags)
        }
        if pattern(&self.regex) != pattern(&other.regex)
            || flags(&self.regex) != flags(&other.regex)
        {
            return false;
        }

        // TS: `isNull(thisMinLength)` (`NullUtil.isNull`, which is true for
        // both `undefined` and `null`). An absent or `null` bound is `None`
        // ([`length_bound_field`]); a `NaN` bound (a non-numeric AST value)
        // takes the same "no bound" branch.
        fn bound(bound: Option<f64>) -> Option<f64> {
            bound.filter(|b| !b.is_nan())
        }
        match (bound(self.min_length), bound(other.min_length)) {
            (None, Some(_)) => return false,
            (Some(this), Some(other)) if this < other => return false,
            _ => {}
        }
        match (bound(self.max_length), bound(other.max_length)) {
            (None, Some(_)) => return false,
            (Some(this), Some(other)) if this > other => return false,
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests;

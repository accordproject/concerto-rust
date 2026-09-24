//! Field and scalar validators: a port of `validator.ts` and
//! `numbervalidator.ts` from the TypeScript reference.
//!
//! P0-04b trial: only `NumberValidator` is ported, with the part of
//! `Validator.reportError` it needs. `StringValidator` and
//! `CollectionSizeValidator` follow in P2-02; until then the loader keeps its
//! own checks for them (`introspect::check_pattern` and friends).

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ecma;
use crate::error::{ContractError, ErrorKind, ValidatorReport};
use crate::model_manager::ValidatedElement;

/// concerto-util `ErrorCodes.DEFAULT_VALIDATOR_EXCEPTION`, the default
/// `errorType` of `Validator.reportError`.
const DEFAULT_VALIDATOR_EXCEPTION: &str = "DefaultValidatorException";

/// A validator attached to a field or a scalar declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum Validator {
    /// A numeric range.
    Number(NumberValidator),
}

/// Builds the error `Validator.reportError` throws: the message with the
/// instance identifier and the element's fully qualified name in front. The
/// name is read only here, as TS reads it only when it reports.
///
/// TS: Validator.reportError (src/introspect/validator.ts)
fn report_error<F: ValidatedElement>(
    field: &F,
    id: Option<&str>,
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
        error_type: DEFAULT_VALIDATOR_EXCEPTION,
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
                    "numbervalidator-constructor-nobounds",
                    Vec::new(),
                ));
            }
            (Some(lower), Some(upper)) if ecma::greater_than(lower, upper) => {
                return Err(report_error(
                    field,
                    None,
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

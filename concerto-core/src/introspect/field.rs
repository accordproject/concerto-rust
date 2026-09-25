//! `Field`'s own validator/default-value selection (src/introspect/field.ts).
//!
//! After `Property.process` sets `this.type`, `Field.process` picks the
//! validator (if any) that matches that type and reads `this.ast.defaultValue`
//! into `this.defaultValue`. The selection is identical in shape to
//! `ScalarDeclaration.process`'s own (`introspect::scalar`): a `NumberValidator`
//! for Integer/Long/Double when `ast.validator` is set, a `StringValidator`
//! for String when either `ast.validator` or `ast.lengthValidator` is set —
//! so this reuses the same [`ScalarValidator`] result shape rather than
//! duplicating it under a new name.

use serde_json::Value;

use crate::ecma;
use crate::error::ContractError;
use crate::introspect::FullyQualified;
use crate::introspect::scalar::ScalarValidator;
use crate::introspect::validators::NumberValidator;
use crate::model_manager::ValidatedElement;

/// What `Field.process` computes, after `Property.process` has set `type`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedField {
    /// `this.validator`, or `None` (JS `null`).
    pub validator: Option<ScalarValidator>,
    /// `this.defaultValue`, or `None` (JS `null`) when the AST has none or a
    /// nullish one.
    pub default_value: Option<Value>,
}

/// The field as the element its own `NumberValidator` is attached to: TS
/// passes `this`, so the validator reads `this.ast.defaultValue` and
/// `this.getFullyQualifiedName()`.
struct FieldElement<'a, E> {
    ast: &'a Value,
    fully_qualified_name: &'a dyn Fn() -> Result<String, E>,
}

impl<E: From<ContractError>> FullyQualified for FieldElement<'_, E> {
    type Error = E;

    fn fully_qualified_name(&self) -> Result<String, E> {
        (self.fully_qualified_name)()
    }
}

impl<E: From<ContractError>> ValidatedElement for FieldElement<'_, E> {
    fn default_value(&self) -> Result<Option<Value>, E> {
        Ok(self.ast.get("defaultValue").cloned())
    }

    fn name(&self) -> Result<String, E> {
        // `field.getName()`: a field's short name is its AST `name`.
        Ok(self
            .ast
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }
}

/// Computes the field's validator and default value from its AST, in the TS
/// order: the validator (whose constructor may fail), then the default
/// value.
///
/// `property_type` is `this.getType()`, already computed by
/// `Property.process`; `fully_qualified_name` is `this.getFullyQualifiedName()`,
/// called only if the validator reports an error.
///
/// TS: Field.process (src/introspect/field.ts)
pub fn process<E: From<ContractError>>(
    property_type: Option<&str>,
    ast: &Value,
    fully_qualified_name: &dyn Fn() -> Result<String, E>,
) -> Result<ProcessedField, E> {
    let truthy = |key: &str| ast.get(key).is_some_and(ecma::is_truthy);
    let validator = match property_type {
        Some("Integer" | "Double" | "Long") if truthy("validator") => {
            let element = FieldElement {
                ast,
                fully_qualified_name,
            };
            let validator_ast = ast.get("validator").unwrap_or(&Value::Null);
            Some(ScalarValidator::Number(NumberValidator::new(
                &element,
                validator_ast,
            )?))
        }
        Some("String") if truthy("validator") || truthy("lengthValidator") => {
            Some(ScalarValidator::String {
                validator: ast.get("validator").cloned(),
                length_validator: ast.get("lengthValidator").cloned(),
            })
        }
        _ => None,
    };

    // `!Util.isNull(this.ast.defaultValue)`.
    let default_value = match ast.get("defaultValue") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.clone()),
    };

    Ok(ProcessedField {
        validator,
        default_value,
    })
}

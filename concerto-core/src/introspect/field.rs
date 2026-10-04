//! `Field`'s own validator/default-value selection (src/introspect/field.ts).
//!
//! After `Property.process` sets `this.type`, `Field.process` picks the
//! validator (if any) that matches that type and reads `this.ast.defaultValue`
//! into `this.defaultValue`. The selection is identical in shape to
//! `ScalarDeclaration.process`'s own (`introspect::scalar`): a `NumberValidator`
//! for Integer/Long/Double when `ast.validator` is set, a `StringValidator`
//! for String when either `ast.validator` or `ast.lengthValidator` is set.
//! Unlike the scalar's, the field's `StringValidator` is not built here: the
//! view builds it ([`FieldValidator::String`]).

// What this module holds is the JS-compatibility seam's
// (`field::process` and the view snapshots it feeds); without the
// `js-compat` feature none of it is compiled.
#[cfg(feature = "js-compat")]
use serde_json::Value;

#[cfg(feature = "js-compat")]
use crate::ecma;
#[cfg(feature = "js-compat")]
use crate::error::{ContractError, ErrorKind};
#[cfg(feature = "js-compat")]
use crate::introspect::FullyQualified;
#[cfg(feature = "js-compat")]
use crate::introspect::validators::NumberValidator;
#[cfg(feature = "js-compat")]
use crate::model_manager::ValidatedElement;

/// The metamodel namespace (`MetaModelNamespace` in `concerto-metamodel`),
/// as `Field.getScalarField`'s own `$class` switch spells it.
#[cfg(feature = "js-compat")]
const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// The validator `Field.process` attaches.
#[cfg(feature = "js-compat")]
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValidator {
    /// `new NumberValidator(this, this.ast.validator)`, for Integer, Long
    /// and Double fields.
    Number(NumberValidator),
    /// `new StringValidator(this, this.ast.validator,
    /// this.ast.lengthValidator)`, for String fields: the arguments TS
    /// passes (`None` is `undefined`), from which the view builds the
    /// TS-facing `StringValidator`.
    String {
        /// `this.ast.validator`.
        validator: Option<Value>,
        /// `this.ast.lengthValidator`.
        length_validator: Option<Value>,
    },
}

/// What `Field.process` computes, after `Property.process` has set `type`.
#[cfg(feature = "js-compat")]
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedField {
    /// `this.validator`, or `None` (JS `null`).
    pub validator: Option<FieldValidator>,
    /// `this.defaultValue`, or `None` (JS `null`) when the AST has none or a
    /// nullish one.
    pub default_value: Option<Value>,
}

/// The field as the element its own `NumberValidator` is attached to: TS
/// passes `this`, so the validator reads `this.ast.defaultValue` and
/// `this.getFullyQualifiedName()`.
#[cfg(feature = "js-compat")]
struct FieldElement<'a, E> {
    ast: &'a Value,
    fully_qualified_name: &'a dyn Fn() -> Result<String, E>,
}

#[cfg(feature = "js-compat")]
impl<E: From<ContractError>> FullyQualified for FieldElement<'_, E> {
    type Error = E;

    fn fully_qualified_name(&self) -> Result<String, E> {
        (self.fully_qualified_name)()
    }
}

#[cfg(feature = "js-compat")]
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
#[cfg(feature = "js-compat")]
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
            Some(FieldValidator::Number(NumberValidator::new(
                &element,
                validator_ast,
            )?))
        }
        Some("String") if truthy("validator") || truthy("lengthValidator") => {
            Some(FieldValidator::String {
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

/// TS: `Field.toString` (src/introspect/field.ts). `name` is `this.name`,
/// `fully_qualified_type_name` is `this.getFullyQualifiedTypeName()`
/// (already resolved by the view — a scalar field's type name is its
/// scalar's own FQN, not the underlying primitive), and `array`/`optional`
/// are `this.array`/`this.optional`, read straight as booleans exactly as
/// TS's string concatenation coerces them.
#[cfg(feature = "js-compat")]
pub fn to_string(
    name: &str,
    fully_qualified_type_name: &str,
    array: bool,
    optional: bool,
) -> String {
    format!(
        "Field {{name={name}, type={fully_qualified_type_name}, array={array}, optional={optional}}}"
    )
}

/// TS: the `switch (type.ast.$class)` inside `Field.getScalarField`
/// (src/introspect/field.ts), after `isTypeScalar()` has already confirmed
/// `type` is a scalar declaration. Builds the synthetic field's AST from the
/// scalar's own AST (`JSON.parse(JSON.stringify(type.ast))`, here a clone),
/// with `$class` swapped for the matching `*Property` class and `name` set
/// to `field_name` (`this.ast.name`, the original field's own name). An
/// unrecognised `$class` — unreachable for a real scalar declaration, since
/// `ScalarDeclaration` only ever holds one of these six — errors exactly as
/// the TS `default` branch's `Unrecognized scalar type ${type.ast.$class}`
/// does, by way of the catalogue's `field-getscalarfield-unrecognizedtype`
/// entry.
///
/// `array` is not set here: the view sets it from `this.isArray()`, exactly
/// as the TS body's own `this.scalarField.array = this.isArray();` does,
/// after constructing the `Field` from this AST.
#[cfg(feature = "js-compat")]
pub fn scalar_to_field_ast<E: From<ContractError>>(
    scalar_ast: &Value,
    field_name: Value,
) -> Result<Value, E> {
    let class = scalar_ast.get("$class").and_then(Value::as_str);
    let property_class = match class {
        Some("concerto.metamodel@1.0.0.StringScalar") => {
            format!("{METAMODEL_NAMESPACE}.StringProperty")
        }
        Some("concerto.metamodel@1.0.0.BooleanScalar") => {
            format!("{METAMODEL_NAMESPACE}.BooleanProperty")
        }
        Some("concerto.metamodel@1.0.0.DateTimeScalar") => {
            format!("{METAMODEL_NAMESPACE}.DateTimeProperty")
        }
        Some("concerto.metamodel@1.0.0.DoubleScalar") => {
            format!("{METAMODEL_NAMESPACE}.DoubleProperty")
        }
        Some("concerto.metamodel@1.0.0.IntegerScalar") => {
            format!("{METAMODEL_NAMESPACE}.IntegerProperty")
        }
        Some("concerto.metamodel@1.0.0.LongScalar") => {
            format!("{METAMODEL_NAMESPACE}.LongProperty")
        }
        other => {
            let class_display = other
                .map(str::to_string)
                .unwrap_or_else(|| "undefined".to_string());
            return Err(ContractError::new(
                ErrorKind::InvalidArgument,
                "field-getscalarfield-unrecognizedtype",
                vec![("class", class_display)],
            )
            .into());
        }
    };
    let mut field_ast = scalar_ast.clone();
    field_ast["$class"] = Value::String(property_class);
    field_ast["name"] = field_name;
    Ok(field_ast)
}

#[cfg(test)]
mod tests;

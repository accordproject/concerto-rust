//! The members of `Typed`, `Identifiable`, `Resource` and
//! `ValidatedResource` that change an instance or check it against the
//! model (src/model/*.ts): `setPropertyValue`, `addArrayValue` and
//! `validate`, over an [`Instance`] (task P3-01b,
//! accordproject/concerto-rust#124).
//!
//! D7 keeps these objects in TS; the checks they run are the validator's
//! ([`super::validate`]), which is where their Rust behaviour lives. These
//! functions are the glue the Rust serializer needs to build and validate
//! an instance the way TS does.

use super::model;
use super::validate::{self, validate_instance_from};
use super::value::{Instance, InstanceKind, JsValue};
use crate::error::{ContractError, ErrorKind, Result};
use crate::model_manager::ModelManager;

/// `'The instance with id ' + this.getIdentifier() + ' trying to set field
/// ' + propName + ' which is not declared in the model.'`
fn undeclared(instance: &Instance, prop_name: &str) -> crate::ConcertoError {
    ContractError::new(
        ErrorKind::Error,
        "validatedresource-setpropertyvalue-undeclaredfield",
        vec![
            ("id", instance.get_identifier().to_js_string()),
            ("propName", prop_name.to_string()),
        ],
    )
    .into()
}

/// TS: `ValidatedResource.setPropertyValue`, or `Typed.setPropertyValue`
/// for a `Resource` or a `Relationship`.
pub fn set_property_value(
    mm: &ModelManager,
    instance: &mut Instance,
    prop_name: &str,
    value: JsValue,
) -> Result<()> {
    if instance.kind == InstanceKind::ValidatedResource {
        let class_declaration = model::get_type(mm, &instance.class_fqn)?;
        let Some((owner_fqn, field)) = class_declaration.property(prop_name)? else {
            return Err(undeclared(instance, prop_name));
        };
        validate::validate_property_value(
            mm,
            &owner_fqn,
            &field,
            &value.to_validator_value(),
            instance.fully_qualified_identifier(),
            &instance.validator_options,
        )?;
    }
    instance.set(prop_name, value);
    Ok(())
}

/// TS `Typed.addArrayValue`: `this[propName].push(value)`, or a new
/// one-element array when the property is falsy.
fn typed_add_array_value(instance: &mut Instance, prop_name: &str, value: JsValue) -> Result<()> {
    let current = instance.get(prop_name).clone();
    if current.is_truthy() {
        let JsValue::Array(mut items) = current else {
            return Err(model::not_a_function("this[propName].push"));
        };
        items.push(value);
        instance.set(prop_name, JsValue::Array(items));
    } else {
        instance.set(prop_name, JsValue::Array(vec![value]));
    }
    Ok(())
}

/// TS: `ValidatedResource.addArrayValue`, or `Typed.addArrayValue` for a
/// `Resource` or a `Relationship`.
pub fn add_array_value(
    mm: &ModelManager,
    instance: &mut Instance,
    prop_name: &str,
    value: JsValue,
) -> Result<()> {
    if instance.kind == InstanceKind::ValidatedResource {
        let class_declaration = model::get_type(mm, &instance.class_fqn)?;
        let Some((owner_fqn, field)) = class_declaration.property(prop_name)? else {
            return Err(undeclared(instance, prop_name));
        };
        if !field.is_array() {
            return Err(ContractError::new(
                ErrorKind::Error,
                "validatedresource-addarrayvalue-notanarray",
                vec![
                    ("id", instance.get_identifier().to_js_string()),
                    ("propName", prop_name.to_string()),
                ],
            )
            .into());
        }
        // `this[propName] ? this[propName].slice(0) : []`, then push.
        let current = instance.get(prop_name);
        let mut new_array = if current.is_truthy() {
            match current {
                JsValue::Array(items) => items.clone(),
                _ => return Err(model::not_a_function("this[propName].slice")),
            }
        } else {
            Vec::new()
        };
        new_array.push(value.clone());
        validate::validate_property_value(
            mm,
            &owner_fqn,
            &field,
            &JsValue::Array(new_array).to_validator_value(),
            instance.fully_qualified_identifier(),
            &instance.validator_options,
        )?;
    }
    typed_add_array_value(instance, prop_name, value)
}

/// TS: `ValidatedResource.validate`: the instance against its class
/// declaration, then what `ResourceValidator.visitClassDeclaration` writes
/// back ([`sync_identifiers`]).
pub fn validate(mm: &ModelManager, instance: &mut Instance) -> Result<()> {
    validate_instance_from(
        mm,
        &instance.to_validator_value(),
        &instance.validator_options,
        instance.fully_qualified_identifier(),
    )?;
    sync_identifiers(mm, instance)
}

/// `ResourceValidator.visitClassDeclaration` also writes to what it
/// validates: for an identified type whose identifying field is not
/// `$identifier`, `obj.$identifier = obj.getIdentifier()`. It does this for
/// every resource it visits (the root, and each nested resource a field or
/// a map value holds); this applies it after a successful validation.
pub fn sync_identifiers(mm: &ModelManager, instance: &mut Instance) -> Result<()> {
    if instance.kind == InstanceKind::Relationship {
        return Ok(());
    }
    if let Some(field) = mm.identifier_field_name(&instance.class_fqn)?
        && field != "$identifier"
    {
        let id = instance.get_identifier().clone();
        instance.set("$identifier", id);
    }
    for value in instance.props.values_mut() {
        sync_value(mm, value)?;
    }
    Ok(())
}

fn sync_value(mm: &ModelManager, value: &mut JsValue) -> Result<()> {
    match value {
        JsValue::Instance(i) => sync_identifiers(mm, i),
        JsValue::Array(items) => items.iter_mut().try_for_each(|v| sync_value(mm, v)),
        JsValue::Map(entries) => entries.iter_mut().try_for_each(|(_, v)| sync_value(mm, v)),
        _ => Ok(()),
    }
}

/// TS: `Resource.toJSON`: `this.getModelManager().getSerializer().toJSON(this)`,
/// with `serializer` the model manager's own; for a `Relationship`, which
/// is not a `Resource`, `Typed.toJSON`, which throws.
pub fn to_json(
    mm: &ModelManager,
    instance: &Instance,
    serializer: &super::Serializer,
) -> Result<JsValue> {
    if instance.kind == InstanceKind::Relationship {
        return Err(
            ContractError::new(ErrorKind::Error, "typed-tojson-useserializer", Vec::new()).into(),
        );
    }
    serializer.to_json(mm, &JsValue::Instance(Box::new(instance.clone())), None)
}

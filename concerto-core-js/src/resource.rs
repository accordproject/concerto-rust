//! The members of `Typed`, `Identifiable`, `Resource` and
//! `ValidatedResource` that change an instance or check it against the
//! model (src/model/*.ts): `setPropertyValue`, `addArrayValue` and
//! `validate`, over an [`Instance`] (task P3-01b,
//! accordproject/concerto-rust#124).
//!
//! D7 keeps these objects in TS; the checks they run are the validator's
//! ([`concerto_core::instance::validate`]), which is where their Rust behaviour lives. These
//! functions are the glue the Rust serializer needs to build and validate
//! an instance the way TS does.

use crate::value::{Instance, InstanceKind, JsValue};
use concerto_core::error::{ContractError, ErrorKind, Result};
use concerto_core::instance::model;
use concerto_core::instance::plan;
use concerto_core::instance::validate::{self, validate_instance_from};
use concerto_core::model_manager::ModelManager;

/// `'The instance with id ' + this.getIdentifier() + ' trying to set field
/// ' + propName + ' which is not declared in the model.'`
fn undeclared(instance: &Instance, prop_name: &str) -> concerto_core::Error {
    ContractError::new(
        ErrorKind::InvalidArgument,
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
        // Validation plan (P5-88): the plan's name index and field.
        if let Some(class_plan) = plan::class_plan(mm, class_declaration.id) {
            let Some(index) = class_plan.find(prop_name) else {
                return Err(undeclared(instance, prop_name));
            };
            validate::validate_property_value_planned(
                mm,
                &class_plan,
                index,
                &value.to_validator_value(),
                instance.fully_qualified_identifier(),
                &instance.validator_options,
            )?;
            instance.set(prop_name, value);
            return Ok(());
        }
        let Some((owner_fqn, field)) = class_declaration.property(prop_name)? else {
            return Err(undeclared(instance, prop_name));
        };
        validate::validate_property_value(
            mm,
            owner_fqn,
            field,
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
        // Validation plan (P5-88): the plan's name index and field.
        let class_plan = plan::class_plan(mm, class_declaration.id);
        let found = match &class_plan {
            Some(cp) => cp.find(prop_name).map(|i| {
                let (owner_fqn, field) = cp.property(mm, i);
                (owner_fqn, field, Some(i))
            }),
            None => class_declaration
                .property(prop_name)?
                .map(|(owner_fqn, field)| (owner_fqn, field, None)),
        };
        let Some((owner_fqn, field, index)) = found else {
            return Err(undeclared(instance, prop_name));
        };
        if !field.is_array() {
            return Err(ContractError::new(
                ErrorKind::InvalidArgument,
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
        match (&class_plan, index) {
            (Some(cp), Some(index)) => validate::validate_property_value_planned(
                mm,
                cp,
                index,
                &JsValue::Array(new_array).to_validator_value(),
                instance.fully_qualified_identifier(),
                &instance.validator_options,
            )?,
            _ => validate::validate_property_value(
                mm,
                owner_fqn,
                field,
                &JsValue::Array(new_array).to_validator_value(),
                instance.fully_qualified_identifier(),
                &instance.validator_options,
            )?,
        }
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
    // Validation plan (P5-88): the identifier field from the plan.
    let identifier = match plan::class_plan_by_name(mm, &instance.class_fqn) {
        Some(class_plan) => class_plan.identifier_field(mm),
        None => mm.identifier_field(&instance.class_fqn)?,
    };
    if let Some(field) = identifier
        && field != "$identifier"
    {
        // Already the same value (the usual case: the constructor set both):
        // the assignment would change nothing (P5-16).
        if instance.props.get("$identifier") != Some(instance.get_identifier()) {
            let id = instance.get_identifier().clone();
            instance.set("$identifier", id);
        }
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
    serializer: &super::serializer::Serializer,
) -> Result<JsValue> {
    if instance.kind == InstanceKind::Relationship {
        return Err(ContractError::new(
            ErrorKind::InvalidArgument,
            "typed-tojson-useserializer",
            Vec::new(),
        )
        .into());
    }
    serializer.to_json(mm, &JsValue::Instance(Box::new(instance.clone())), None)
}

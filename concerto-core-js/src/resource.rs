//! The members of `Typed`, `Identifiable`, `Resource` and
//! `ValidatedResource` that change an instance or check it against the
//! model (src/model/*.ts): `setPropertyValue`, `addArrayValue` and
//! `validate`, over an [`Instance`].
//!
//! D7 keeps these objects in TS; the checks they run are the validator's
//! ([`concerto_core::instance::validate`]), which is where their Rust behaviour lives. These
//! functions are the glue the Rust serializer needs to build and validate
//! an instance the way TS does.
//!
//! The validator reads the instance's own values in place
//! ([`concerto_core::instance::validate::ValidatorInput`]): no call
//! copies the value or the object graph it checks.

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
        // The validation plan: its name index and field; its chain's
        // error, when it does not resolve, is the one `getProperty`
        // raises.
        let class_plan = plan::class_plan(mm, class_declaration.id)?;
        let Some(index) = class_plan.find(prop_name) else {
            return Err(undeclared(instance, prop_name));
        };
        validate::validate_property_value(
            mm,
            &class_plan,
            index,
            &value,
            instance.fully_qualified_identifier(),
            &instance.validator_options,
        )?;
    }
    instance.set(prop_name, value);
    Ok(())
}

/// TS `Typed.addArrayValue`: `this[propName].push(value)`, in place (the
/// array is not copied), or a new one-element array when the property is
/// falsy.
fn typed_add_array_value(instance: &mut Instance, prop_name: &str, value: JsValue) -> Result<()> {
    match instance.props.get_mut(prop_name) {
        Some(JsValue::Array(items)) => items.push(value),
        Some(current) if current.is_truthy() => {
            return Err(model::not_a_function("this[propName].push"));
        }
        _ => instance.set(prop_name, JsValue::Array(vec![value])),
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
        // The validation plan: its name index and field.
        let class_plan = plan::class_plan(mm, class_declaration.id)?;
        let Some(index) = class_plan.find(prop_name) else {
            return Err(undeclared(instance, prop_name));
        };
        let (_, field) = class_plan.property(mm, index);
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
        // `this[propName] ? this[propName].slice(0) : []`, then push, then
        // validate that array; `Typed.addArrayValue` then pushes onto the
        // property itself. Here the value is pushed onto the property's own
        // array and validated there (popped again when it fails), so nothing
        // is copied.
        let current = instance.get(prop_name);
        if current.is_truthy() && !matches!(current, JsValue::Array(_)) {
            return Err(model::not_a_function("this[propName].slice"));
        }
        let root_resource_identifier = instance.fully_qualified_identifier();
        let options = instance.validator_options;
        if let Some(JsValue::Array(items)) = instance.props.get_mut(prop_name) {
            items.push(value);
            let outcome = validate::validate_property_value(
                mm,
                &class_plan,
                index,
                instance.get(prop_name),
                root_resource_identifier,
                &options,
            );
            if outcome.is_err()
                && let Some(JsValue::Array(items)) = instance.props.get_mut(prop_name)
            {
                items.pop();
            }
            return outcome;
        }
        let new_array = JsValue::Array(vec![value]);
        validate::validate_property_value(
            mm,
            &class_plan,
            index,
            &new_array,
            root_resource_identifier,
            &options,
        )?;
        instance.set(prop_name, new_array);
        return Ok(());
    }
    typed_add_array_value(instance, prop_name, value)
}

/// TS: `ValidatedResource.validate`: the instance against its class
/// declaration, then what `ResourceValidator.visitClassDeclaration` writes
/// back ([`sync_identifiers`]).
///
/// The instance is validated in place: it is moved into a [`JsValue`]
/// for the walk and back out, without being copied.
pub fn validate(mm: &ModelManager, instance: &mut Instance) -> Result<()> {
    let options = instance.validator_options;
    let root_resource_identifier = instance.fully_qualified_identifier();
    let root = JsValue::Instance(Box::new(std::mem::replace(
        instance,
        Instance::placeholder(),
    )));
    let outcome = validate_instance_from(mm, &root, &options, root_resource_identifier);
    let JsValue::Instance(validated) = root else {
        unreachable!("the root was built as an instance just above");
    };
    *instance = *validated;
    outcome?;
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
    // Validation plan: the identifier field from the plan.
    let identifier = plan::class_plan_by_name(mm, &instance.class_fqn)?.identifier_field(mm);
    if let Some(field) = identifier
        && field != "$identifier"
    {
        // Already the same value (the usual case: the constructor set both):
        // the assignment would change nothing.
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

/// Whether [`sync_identifiers`] would change `instance`: the same walk, in
/// the same order and with the same errors, read only. `Serializer.toJSON`
/// copies the resource to sync it only when it would change, which it
/// seldom does (the constructor sets both fields).
pub(crate) fn sync_needed(mm: &ModelManager, instance: &Instance) -> Result<bool> {
    if instance.kind == InstanceKind::Relationship {
        return Ok(false);
    }
    let identifier = plan::class_plan_by_name(mm, &instance.class_fqn)?.identifier_field(mm);
    if let Some(field) = identifier
        && field != "$identifier"
        && instance.props.get("$identifier") != Some(instance.get_identifier())
    {
        return Ok(true);
    }
    for value in instance.props.values() {
        if sync_value_needed(mm, value)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sync_value_needed(mm: &ModelManager, value: &JsValue) -> Result<bool> {
    match value {
        JsValue::Instance(i) => sync_needed(mm, i),
        JsValue::Array(items) => {
            for item in items {
                if sync_value_needed(mm, item)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        JsValue::Map(entries) => {
            for (_, item) in entries {
                if sync_value_needed(mm, item)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
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
/// with `serializer` the model manager's own; for a `Relationship`, which is not
/// a `Resource`, `Typed.toJSON`, which throws. `resource` holds the instance
/// (anything else is the serializer's own error), and is passed on as it is, not
/// copied.
pub fn to_json(
    mm: &ModelManager,
    resource: &JsValue,
    serializer: &super::serializer::Serializer,
) -> Result<JsValue> {
    if let JsValue::Instance(instance) = resource
        && instance.kind == InstanceKind::Relationship
    {
        return Err(ContractError::new(
            ErrorKind::InvalidArgument,
            "typed-tojson-useserializer",
            Vec::new(),
        )
        .into());
    }
    serializer.to_json(mm, resource, None)
}

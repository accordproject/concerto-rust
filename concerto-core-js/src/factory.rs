//! `Factory` (src/factory.ts): the model checks of `newResource`, which run
//! in Rust ([`check_new_resource`]), and the construction of
//! the instances the Rust serializer builds ([`new_resource`] and the
//! `newConcept`/`newRelationship`/`newTransaction`/`newEvent` wrappers).
//!
//! D7 keeps instance creation in TS: the WASM binding calls
//! [`check_new_resource`] and the TS `Factory` then builds its own object,
//! with `uuid` and dayjs. The rest of this module is the same construction
//! for the Rust-side serializer ([`super::populator`]), which needs an
//! instance to populate; the identifier and the clock come from the caller
//! ([`InstanceEnv`]), so that nothing here generates ids or reads time.
//!
//! `options.generate` (`InstanceGenerator` with a sample or empty value
//! generator) stays in TS; these
//! functions build the instance as TS does when `options.generate` is
//! falsy.

use crate::value::{Instance, InstanceKind, JsValue};
use concerto_core::error::{ContractError, ErrorKind, Result};
use concerto_core::instance::dayjs::Dayjs;
use concerto_core::instance::from_json::{self, FieldDefault, IdentifierArg};
use concerto_core::instance::model::{self, TypeRef};
use concerto_core::model_manager::ModelManager;
use concerto_core::{Error, model_util};

pub use concerto_core::instance::from_json::InstanceEnv;

/// What [`check_new_resource`] settles: everything `newResource` needs
/// from the model before it builds the object.
#[derive(Debug, Clone, PartialEq)]
pub struct NewResourceCheck {
    /// `classDecl.getFullyQualifiedName()`.
    pub class_fqn: String,
    /// `classDecl.getIdentifierFieldName()`.
    pub identifier_field_name: Option<String>,
    /// The identifier after `isSystemIdentified()`'s default.
    pub id: JsValue,
    /// `classDecl.isTransaction() || classDecl.isEvent()`: the instance
    /// gets `dayjs.utc()` as its `$timestamp`.
    pub timestamped: bool,
}

/// A `Factory.newResource` error: a plain `Error` from the catalogue.
fn error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::InvalidArgument, code, params).into()
}

/// The model checks of `Factory.newResource`, in TS order,
/// for an identifier that is any JS value: the checks are the native
/// route's ([`from_json::check_new_resource`]).
///
/// TS: Factory.newResource (src/factory.ts), up to the construction.
pub fn check_new_resource(
    mm: &ModelManager,
    ns: &str,
    type_name: &str,
    id: JsValue,
    new_id: &mut dyn FnMut() -> String,
) -> Result<NewResourceCheck> {
    let check = from_json::check_new_resource(mm, ns, type_name, identifier_arg(&id), new_id)?;
    Ok(NewResourceCheck {
        class_fqn: check.class_fqn,
        identifier_field_name: check.identifier_field_name,
        id: check.generated_id.map_or(id, JsValue::String),
        timestamped: check.timestamped,
    })
}

/// The identifier `Factory.newResource`'s checks read.
fn identifier_arg(id: &JsValue) -> IdentifierArg<'_> {
    match id {
        JsValue::Undefined | JsValue::Null => IdentifierArg::Nullish,
        JsValue::String(s) => IdentifierArg::String(s),
        other => IdentifierArg::Other {
            truthy: other.is_truthy(),
        },
    }
}

/// Builds a `Resource` or `ValidatedResource` the way `Factory.newResource`
/// does after its checks: the constructor, `assignFieldDefaults()` (each
/// default assigned through `setPropertyValue`, which validates it on a
/// `ValidatedResource`), then the identifying field.
///
/// TS: Factory.newResource (src/factory.ts), from `new ValidatedResource`.
pub fn build_resource(
    mm: &ModelManager,
    ns: &str,
    type_name: &str,
    check: NewResourceCheck,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
) -> Result<Instance> {
    build_resource_with(
        mm,
        ns,
        type_name,
        check,
        disable_validation,
        env,
        Defaults::Create,
    )
}

/// [`build_resource`], with the defaults applied as `mode` says.
fn build_resource_with(
    mm: &ModelManager,
    ns: &str,
    type_name: &str,
    check: NewResourceCheck,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
    mode: Defaults,
) -> Result<Instance> {
    let timestamp = if check.timestamped {
        JsValue::DateTime(Dayjs::utc_now(env.now_ms()))
    } else {
        JsValue::Null
    };
    let kind = if disable_validation {
        InstanceKind::Resource
    } else {
        InstanceKind::ValidatedResource
    };
    let identifier_field_name = from_json::identifiable_field_name(mm, ns, &check.class_fqn)?;
    let mut instance = Instance::new(
        kind,
        check.class_fqn.clone(),
        ns,
        type_name,
        identifier_field_name,
        check.id.clone(),
        timestamp,
    );
    let class_decl = model::get_type(mm, &instance.class_fqn)?;
    assign_field_defaults_of(&class_decl, &mut instance, mode)?;
    if let Some(id_field) = &check.identifier_field_name {
        instance.set(id_field, check.id);
    }
    Ok(instance)
}

/// TS: `Factory.newResource(ns, type, id, options)` with a falsy
/// `options.generate`.
pub fn new_resource(
    mm: &ModelManager,
    ns: &str,
    type_name: &str,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
) -> Result<Instance> {
    new_resource_with(
        mm,
        ns,
        type_name,
        id,
        disable_validation,
        env,
        Defaults::Create,
    )
}

/// [`new_resource`], with the defaults applied as `mode` says.
fn new_resource_with(
    mm: &ModelManager,
    ns: &str,
    type_name: &str,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
    mode: Defaults,
) -> Result<Instance> {
    let check = check_new_resource(mm, ns, type_name, id, &mut || env.new_id())?;
    build_resource_with(mm, ns, type_name, check, disable_validation, env, mode)
}

/// [`new_resource`] for a declaration already found: what
/// `newResource(decl.getNamespace(), decl.getName(), id)` does, whose own
/// type lookup finds `decl` again. The identifiable field name and the
/// field defaults are read off `decl` itself. Only population calls this,
/// so the defaults are applied as population applies them
/// ([`Defaults::Populate`]).
pub(crate) fn new_resource_of(
    decl: &TypeRef,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
) -> Result<Instance> {
    let (ns, type_name) = (decl.namespace(), decl.name());
    let check =
        from_json::check_new_resource_of(decl, ns, type_name, identifier_arg(&id), &mut || {
            env.new_id()
        })?;
    let id = check.generated_id.map_or(id, JsValue::String);
    let timestamp = if check.timestamped {
        JsValue::DateTime(Dayjs::utc_now(env.now_ms()))
    } else {
        JsValue::Null
    };
    let kind = if disable_validation {
        InstanceKind::Resource
    } else {
        InstanceKind::ValidatedResource
    };
    // `identifiable_field_name`: `getModelFile(ns).getType(fqn)` is `decl`.
    let identifier_field_name = decl
        .identifier_field_name()?
        .filter(|f| !f.is_empty())
        .map(str::to_string);
    let mut instance = Instance::new(
        kind,
        check.class_fqn,
        ns,
        type_name,
        identifier_field_name,
        id.clone(),
        timestamp,
    );
    assign_field_defaults_of(decl, &mut instance, Defaults::Populate)?;
    if let Some(id_field) = &check.identifier_field_name {
        instance.set(id_field, id);
    }
    Ok(instance)
}

/// TS: `Factory.newRelationship(ns, type, id)`.
pub fn new_relationship(
    mm: &ModelManager,
    ns: &str,
    type_name: &str,
    id: JsValue,
) -> Result<Instance> {
    let fqn = model_util::qualify(ns, type_name);
    let class_decl = model::get_type(mm, &fqn)?;
    if !class_decl.is_identified()? {
        return Err(error(
            "factory-newrelationship-notidentifiable",
            vec![("fqn", fqn)],
        ));
    }
    relationship(mm, &class_decl, ns, type_name, id)
}

/// TS: `new Relationship(modelManager, classDeclaration, ns, type, id)`.
pub(crate) fn relationship(
    mm: &ModelManager,
    class_decl: &TypeRef,
    ns: &str,
    type_name: &str,
    id: JsValue,
) -> Result<Instance> {
    let class_fqn = class_decl.fqn();
    let identifier_field_name = from_json::identifiable_field_name(mm, ns, class_fqn)?;
    Ok(Instance::new(
        InstanceKind::Relationship,
        class_fqn,
        ns,
        type_name,
        identifier_field_name,
        id,
        JsValue::Undefined,
    ))
}

/// TS: `Relationship.fromURI(modelManager, uri, defaultNamespace,
/// defaultType)` (src/model/relationship.ts).
pub fn relationship_from_uri(
    mm: &ModelManager,
    uri: &str,
    default_namespace: Option<&str>,
    default_type: Option<&str>,
) -> Result<Instance> {
    let resource_id = concerto_core::instance::resource_id::ResourceId::from_uri(
        uri,
        default_namespace,
        default_type,
    )?;
    let fqt = model_util::qualify(&resource_id.namespace, &resource_id.type_name);
    let class_decl = model::get_type(mm, &fqt)?;
    relationship(
        mm,
        &class_decl,
        &resource_id.namespace,
        &resource_id.type_name,
        JsValue::String(resource_id.id.clone()),
    )
}

/// `if (!ns) throw new Error('ns not specified'); else if (!type) throw
/// new Error('type not specified')`, shared by `newTransaction` and
/// `newEvent`.
fn check_ns_and_type(ns: &JsValue, type_name: &JsValue) -> Result<()> {
    if !ns.is_truthy() {
        return Err(error("factory-newtransaction-nsnotspecified", Vec::new()));
    }
    if !type_name.is_truthy() {
        return Err(error("factory-newtransaction-typenotspecified", Vec::new()));
    }
    Ok(())
}

/// TS: `Factory.newTransaction(ns, type, id, options)` with a falsy
/// `options.generate`. `ns` and `type_name` are checked for truthiness
/// first, then must be strings.
pub fn new_transaction(
    mm: &ModelManager,
    ns: &JsValue,
    type_name: &JsValue,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
) -> Result<Instance> {
    new_transaction_with(
        mm,
        ns,
        type_name,
        id,
        disable_validation,
        env,
        Defaults::Create,
    )
}

/// [`new_transaction`] for population ([`Defaults::Populate`]).
pub(crate) fn new_transaction_to_populate(
    mm: &ModelManager,
    ns: &JsValue,
    type_name: &JsValue,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
) -> Result<Instance> {
    new_transaction_with(
        mm,
        ns,
        type_name,
        id,
        disable_validation,
        env,
        Defaults::Populate,
    )
}

/// [`new_transaction`], with the defaults applied as `mode` says.
fn new_transaction_with(
    mm: &ModelManager,
    ns: &JsValue,
    type_name: &JsValue,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
    mode: Defaults,
) -> Result<Instance> {
    check_ns_and_type(ns, type_name)?;
    let (ns, type_name) = (ns.to_js_string(), type_name.to_js_string());
    let transaction = new_resource_with(mm, &ns, &type_name, id, disable_validation, env, mode)?;
    let decl = model::get_type(mm, &transaction.class_fqn)?;
    if !decl.is_transaction() {
        return Err(error(
            "factory-newtransaction-notatransaction",
            vec![("fqn", transaction.class_fqn.clone())],
        ));
    }
    Ok(transaction)
}

/// TS: `Factory.newEvent(ns, type, id, options)` with a falsy
/// `options.generate`.
pub fn new_event(
    mm: &ModelManager,
    ns: &JsValue,
    type_name: &JsValue,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
) -> Result<Instance> {
    new_event_with(
        mm,
        ns,
        type_name,
        id,
        disable_validation,
        env,
        Defaults::Create,
    )
}

/// [`new_event`] for population ([`Defaults::Populate`]).
pub(crate) fn new_event_to_populate(
    mm: &ModelManager,
    ns: &JsValue,
    type_name: &JsValue,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
) -> Result<Instance> {
    new_event_with(
        mm,
        ns,
        type_name,
        id,
        disable_validation,
        env,
        Defaults::Populate,
    )
}

/// [`new_event`], with the defaults applied as `mode` says.
fn new_event_with(
    mm: &ModelManager,
    ns: &JsValue,
    type_name: &JsValue,
    id: JsValue,
    disable_validation: bool,
    env: &mut dyn InstanceEnv,
    mode: Defaults,
) -> Result<Instance> {
    check_ns_and_type(ns, type_name)?;
    let (ns, type_name) = (ns.to_js_string(), type_name.to_js_string());
    let event = new_resource_with(mm, &ns, &type_name, id, disable_validation, env, mode)?;
    let decl = model::get_type(mm, &event.class_fqn)?;
    if !decl.is_event() {
        return Err(error(
            "factory-newevent-notanevent",
            vec![("fqn", event.class_fqn.clone())],
        ));
    }
    Ok(event)
}

/// TS: `Typed.assignFieldDefaults` (src/model/typed.ts): each field with a
/// non-null default gets it, converted by the field's type
/// ([`from_json::assign_field_defaults`]), through `this.setPropertyValue`
/// (which validates it on a `ValidatedResource`).
pub fn assign_field_defaults(mm: &ModelManager, instance: &mut Instance) -> Result<()> {
    let class_decl = model::get_type(mm, &instance.class_fqn)?;
    assign_field_defaults_of(&class_decl, instance, Defaults::Create)
}

/// Who applies the field defaults (BC-45): a `DateTime` default that
/// is not strict throws when it is applied.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Defaults {
    /// Instance creation (`Factory.newResource`): every default is applied,
    /// so a non-strict `DateTime` one throws at once, after the defaults
    /// before it were set.
    Create,
    /// Population (`JSONPopulator`): the document may still replace the
    /// default, so it is set as an invalid date, TS's `dayjs.utc(default)`,
    /// and [`crate::populator::JSONPopulator::visit_class_declaration`]
    /// throws only if it is still there once the fields are populated.
    Populate,
}

/// [`assign_field_defaults`] for the declaration of `instance`.
fn assign_field_defaults_of(
    class_decl: &TypeRef,
    instance: &mut Instance,
    mode: Defaults,
) -> Result<()> {
    let mm = class_decl.mm;
    from_json::assign_field_defaults_of(class_decl, &mut |name, value| {
        let value = match value {
            FieldDefault::Number(n) => JsValue::Number(n),
            FieldDefault::Bool(b) => JsValue::Bool(b),
            FieldDefault::DateTime(d) => JsValue::DateTime(d),
            FieldDefault::InvalidDateTime(err) if mode == Defaults::Create => return Err(err),
            FieldDefault::InvalidDateTime(_) => JsValue::DateTime(Dayjs::utc_invalid()),
            FieldDefault::Json(v) => JsValue::from_json(&v),
        };
        super::resource::set_property_value(mm, instance, name, value)
    })
}

/// BC-45: the error of the first field of `instance` (of `class_decl`) whose
/// non-strict `DateTime` default population left in place
/// ([`Defaults::Populate`]): an invalid date, which no populated value can
/// be.
pub(crate) fn check_populated_date_time_defaults(
    class_decl: &TypeRef,
    instance: &Instance,
) -> Result<()> {
    for (name, err) in from_json::invalid_date_time_defaults_of(class_decl) {
        if matches!(instance.props.get(&name), Some(JsValue::DateTime(d)) if !d.is_valid()) {
            return Err(err);
        }
    }
    Ok(())
}

/// Keys of `props`, for tests.
#[cfg(test)]
fn keys(props: &crate::value::JsObject) -> Vec<&str> {
    props.keys().map(String::as_str).collect()
}

#[cfg(test)]
mod tests;

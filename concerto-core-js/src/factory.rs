//! `Factory` (src/factory.ts): the model checks of `newResource`, which #32
//! point 4 moves to Rust ([`check_new_resource`]), and the construction of
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
//! generator) stays in TS (ledger: "Factory generate path (D7)"); these
//! functions build the instance as TS does when `options.generate` is falsy.

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

/// The model checks of `Factory.newResource`, in TS order (#32 point 4),
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
    assign_field_defaults(mm, &mut instance)?;
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
    let check = check_new_resource(mm, ns, type_name, id, &mut || env.new_id())?;
    build_resource(mm, ns, type_name, check, disable_validation, env)
}

/// [`new_resource`] for a declaration already found (P5-13): what
/// `newResource(decl.getNamespace(), decl.getName(), id)` does, whose own
/// type lookup finds `decl` again. The identifiable field name and the
/// field defaults are read off `decl` itself.
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
    assign_field_defaults_of(decl, &mut instance)?;
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
    check_ns_and_type(ns, type_name)?;
    let (ns, type_name) = (ns.to_js_string(), type_name.to_js_string());
    let transaction = new_resource(mm, &ns, &type_name, id, disable_validation, env)?;
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
    check_ns_and_type(ns, type_name)?;
    let (ns, type_name) = (ns.to_js_string(), type_name.to_js_string());
    let event = new_resource(mm, &ns, &type_name, id, disable_validation, env)?;
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
    assign_field_defaults_of(&class_decl, instance)
}

/// [`assign_field_defaults`] for the declaration of `instance`.
fn assign_field_defaults_of(class_decl: &TypeRef, instance: &mut Instance) -> Result<()> {
    let mm = class_decl.mm;
    from_json::assign_field_defaults_of(class_decl, &mut |name, value| {
        let value = match value {
            FieldDefault::Number(n) => JsValue::Number(n),
            FieldDefault::Bool(b) => JsValue::Bool(b),
            FieldDefault::DateTime(d) => JsValue::DateTime(d),
            FieldDefault::Json(v) => JsValue::from_json(&v),
        };
        super::resource::set_property_value(mm, instance, name, value)
    })
}

/// Keys of `props`, for tests.
#[cfg(test)]
fn keys(props: &indexmap::IndexMap<String, JsValue>) -> Vec<&str> {
    props.keys().map(String::as_str).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env;

    impl InstanceEnv for Env {
        fn new_id(&mut self) -> String {
            "00000000-0000-4000-8000-000000000000".into()
        }
        fn now_ms(&mut self) -> f64 {
            0.0
        }
    }

    fn manager(cto_ast: serde_json::Value) -> ModelManager {
        let mut mm = ModelManager::new().expect("a model manager");
        mm.add_model_with_definitions(&cto_ast, None, Some("test.cto".into()))
            .expect("the model loads");
        mm
    }

    fn model() -> ModelManager {
        manager(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                    "name": "Car",
                    "isAbstract": false,
                    "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "vin" },
                    "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "vin",
                          "isArray": false, "isOptional": false,
                          "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "^[A-Z]+$", "flags": "" } },
                        { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "wheels",
                          "isArray": false, "isOptional": false, "defaultValue": 4 }
                    ]
                },
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Shape",
                    "isAbstract": true,
                    "properties": []
                },
                {
                    "$class": "concerto.metamodel@1.0.0.TransactionDeclaration",
                    "name": "Tx",
                    "isAbstract": false,
                    "properties": []
                }
            ]
        }))
    }

    #[test]
    fn new_resource_builds_a_validated_resource_with_defaults() {
        let mm = model();
        let car = new_resource(
            &mm,
            "org.acme@1.0.0",
            "Car",
            JsValue::String("ABC".into()),
            false,
            &mut Env,
        )
        .expect("a car");
        assert_eq!(car.kind, InstanceKind::ValidatedResource);
        assert_eq!(
            keys(&car.props),
            [
                "$namespace",
                "$type",
                "$identifierFieldName",
                "$identifier",
                "vin",
                "$timestamp",
                "wheels"
            ]
        );
        assert_eq!(car.get("wheels"), &JsValue::Number(4.0));
        assert_eq!(car.get("$timestamp"), &JsValue::Null);
    }

    #[test]
    fn new_resource_checks_in_ts_order() {
        let mm = model();
        let message = |r: Result<Instance>| match r.map_err(Error::into_ported) {
            Err(Some(e)) => e.message(),
            other => panic!("expected an error, got {other:?}"),
        };
        assert_eq!(
            message(new_resource(
                &mm,
                "org.acme@1.0.0",
                "Shape",
                JsValue::Undefined,
                false,
                &mut Env
            )),
            "Cannot instantiate the abstract type \"Shape\" in the \"org.acme@1.0.0\" namespace."
        );
        assert_eq!(
            message(new_resource(
                &mm,
                "org.acme@1.0.0",
                "Car",
                JsValue::Number(1.0),
                false,
                &mut Env
            )),
            "Invalid or missing identifier for Type \"Car\" in namespace \"org.acme@1.0.0\"."
        );
        assert_eq!(
            message(new_resource(
                &mm,
                "org.acme@1.0.0",
                "Car",
                JsValue::String(" ".into()),
                false,
                &mut Env
            )),
            "Missing identifier for Type \"Car\" in namespace \"org.acme@1.0.0\"."
        );
        assert_eq!(
            message(new_resource(
                &mm,
                "org.acme@1.0.0",
                "Car",
                JsValue::String("abc".into()),
                false,
                &mut Env
            )),
            "Provided id does not match regex: /^[A-Z]+$/"
        );
        assert_eq!(
            message(new_resource(
                &mm,
                "org.acme@1.0.0",
                "Tx",
                JsValue::String("1".into()),
                false,
                &mut Env
            )),
            "Type is not identifiable org.acme@1.0.0.Tx"
        );
    }

    #[test]
    fn transactions_get_a_timestamp() {
        let mm = model();
        let tx = new_transaction(
            &mm,
            &JsValue::String("org.acme@1.0.0".into()),
            &JsValue::String("Tx".into()),
            JsValue::Undefined,
            false,
            &mut Env,
        )
        .expect("a transaction");
        assert!(matches!(tx.get("$timestamp"), JsValue::DateTime(_)));
        let message = match new_transaction(
            &mm,
            &JsValue::Undefined,
            &JsValue::String("Tx".into()),
            JsValue::Undefined,
            false,
            &mut Env,
        )
        .map_err(Error::into_ported)
        {
            Err(Some(e)) => e.message(),
            other => panic!("expected an error, got {other:?}"),
        };
        assert_eq!(message, "ns not specified");
    }
}

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

use serde_json::Value;

use super::dayjs::Dayjs;
use super::model::{self, FieldType, TypeRef};
use super::value::{Instance, InstanceKind, JsValue};
use crate::error::{ContractError, ErrorKind, Result};
use crate::introspect::scalar::ScalarValidator;
use crate::introspect::validators::StringValidator;
use crate::introspect::{FullyQualified, Property};
use crate::model_manager::{ModelManager, Node, ResolutionContext, ValidatedElement};
use crate::{ConcertoError, ecma, model_util};

/// What the TS `Factory` gets from its environment rather than from the
/// model (D7): a new identifier and the current time.
pub trait InstanceEnv {
    /// TS: `Factory.newId()`, `uuid.v4()`.
    fn new_id(&mut self) -> String;
    /// TS: the time `dayjs.utc()` reads, in ms since the epoch.
    fn now_ms(&mut self) -> f64;
}

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
fn error(code: &'static str, params: Vec<(&'static str, String)>) -> ConcertoError {
    ContractError::new(ErrorKind::Error, code, params).into()
}

/// The model checks of `Factory.newResource`, in TS order (#32 point 4):
/// the type lookup, the abstract type, the identifier's type, the empty
/// identifier and the identifier regex, plus the non-identifiable type
/// given an identifier. `new_id` is `Factory.newId`, called only for a
/// system-identified type given a nullish id.
///
/// TS: Factory.newResource (src/factory.ts), up to the construction.
pub fn check_new_resource(
    mm: &ModelManager,
    ns: &str,
    type_name: &str,
    id: JsValue,
    new_id: &mut dyn FnMut() -> String,
) -> Result<NewResourceCheck> {
    let qualified_name = model_util::get_fully_qualified_name(ns, type_name);
    let class_decl = model::get_type(mm, &qualified_name)?;

    if class_decl.is_abstract("classDecl.isAbstract")? {
        return Err(error(
            "factory-newinstance-abstracttype",
            vec![
                ("namespace", ns.to_string()),
                ("type", type_name.to_string()),
            ],
        ));
    }

    let id_field = class_decl.identifier_field_name()?;
    let mut id = id;
    if class_decl.is_system_identified()? && id.is_nullish() {
        id = JsValue::String(new_id());
    }
    if let Some(id_field) = &id_field {
        let JsValue::String(id_text) = &id else {
            return Err(error(
                "factory-newinstance-invalididentifier",
                vec![
                    ("namespace", ns.to_string()),
                    ("type", type_name.to_string()),
                ],
            ));
        };
        if ecma::js_trim(id_text).is_empty() {
            return Err(error(
                "factory-newinstance-missingidentifier",
                vec![
                    ("namespace", ns.to_string()),
                    ("type", type_name.to_string()),
                ],
            ));
        }
        // `if (id)`: a non-empty string here.
        if let Some(regex) = identifier_regex(&class_decl, id_field)?
            && !regex.matches_regex(id_text)
        {
            return Err(error(
                "factory-newresource-idregexmismatch",
                vec![("regex", regex.regex().unwrap_or_default())],
            ));
        }
    } else if id.is_truthy() {
        return Err(error(
            "factory-newresource-notidentifiable",
            vec![("fqn", class_decl.fqn())],
        ));
    }

    Ok(NewResourceCheck {
        class_fqn: class_decl.fqn(),
        identifier_field_name: id_field,
        id,
        timestamped: class_decl.is_transaction() || class_decl.is_event(),
    })
}

/// The element a string validator is attached to, for building one: its
/// name and fully-qualified name only (the default value was checked when
/// the model loaded).
struct IdElement {
    name: String,
    fqn: String,
}

impl FullyQualified for IdElement {
    type Error = ConcertoError;

    fn fully_qualified_name(&self) -> Result<String> {
        Ok(self.fqn.clone())
    }
}

impl ValidatedElement for IdElement {
    fn default_value(&self) -> Result<Option<Value>> {
        Ok(None)
    }

    fn name(&self) -> Result<String> {
        Ok(self.name.clone())
    }
}

/// `idFullField?.validator` when it has a `regex`: the identifying
/// property (unboxed with `getScalarField()` when its type is a scalar) and
/// its string validator.
fn identifier_regex(class_decl: &TypeRef, id_field: &str) -> Result<Option<StringValidator>> {
    let Some((owner_fqn, property)) = class_decl.property(id_field)? else {
        return Ok(None);
    };
    let element = IdElement {
        name: id_field.to_string(),
        fqn: format!("{owner_fqn}.{id_field}"),
    };
    let field = model::field(class_decl.mm, &owner_fqn, property)?;
    let validator = match (&field.field_type, &field.property) {
        (FieldType::Primitive("String"), Property::String(sp)) if sp.validator.is_some() => {
            StringValidator::new(
                &element,
                sp.validator.as_ref(),
                sp.length_validator.as_ref(),
            )?
        }
        (
            FieldType::Scalar {
                primitive: Some("String"),
                validator: Some(scalar_validator),
                ..
            },
            _,
        ) => {
            let ScalarValidator::String {
                validator: Some(regex),
                length_validator,
            } = scalar_validator.as_ref()
            else {
                return Ok(None);
            };
            let bad = |e: serde_json::Error| {
                ConcertoError::from(ContractError::pre_port(
                    ErrorKind::Error,
                    format!("invalid string validator: {e}"),
                    None,
                ))
            };
            let regex = serde_json::from_value(regex.clone()).map_err(bad)?;
            let length = length_validator
                .as_ref()
                .map(|v| serde_json::from_value(v.clone()).map_err(bad))
                .transpose()?;
            StringValidator::new(&element, Some(&regex), length.as_ref())?
        }
        _ => return Ok(None),
    };
    Ok(validator.regex().is_some().then_some(validator))
}

/// The `$identifierFieldName` the `Identifiable` constructor caches:
/// `modelManager.getModelFile(ns)?.getType(fqt)?.getIdentifierFieldName()
/// || '$identifier'`, with `fqt` the class declaration's name.
fn identifiable_field_name(mm: &ModelManager, ns: &str, class_fqn: &str) -> Result<Option<String>> {
    let Some(file) = mm.model_file_id(ns) else {
        return Ok(None);
    };
    let Some(Node::Declaration(id)) = mm.get_type(&Node::ModelFile(file), Some(class_fqn))? else {
        return Ok(None);
    };
    let decl = TypeRef {
        mm,
        id,
        decl: mm.declaration(id).expect("a live handle"),
    };
    Ok(decl.identifier_field_name()?.filter(|f| !f.is_empty()))
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
    let identifier_field_name = identifiable_field_name(mm, ns, &check.class_fqn)?;
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

/// TS: `Factory.newRelationship(ns, type, id)`.
pub fn new_relationship(
    mm: &ModelManager,
    ns: &str,
    type_name: &str,
    id: JsValue,
) -> Result<Instance> {
    let fqn = model_util::get_fully_qualified_name(ns, type_name);
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
    let identifier_field_name = identifiable_field_name(mm, ns, &class_fqn)?;
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
    let resource_id =
        super::resource_id::ResourceId::from_uri(uri, default_namespace, default_type)?;
    let fqt = model_util::get_fully_qualified_name(&resource_id.namespace, &resource_id.type_name);
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

/// The raw AST `defaultValue` of a property of `owner_fqn`
/// (`Field.getDefaultValue()`, `null` when nullish), read off the AST as TS
/// does: the typed `DateTimeProperty` carries none.
fn raw_default_value(mm: &ModelManager, owner_fqn: &str, name: &str) -> Option<Value> {
    let decl = mm.declaration_id(owner_fqn)?;
    let prop = mm.property_ids(decl).find(|id| {
        mm.property(*id)
            .is_some_and(|p| crate::Named::name(p) == name)
    })?;
    mm.property_default_value(prop).cloned()
}

/// TS: `Typed.assignFieldDefaults` (src/model/typed.ts): each field with a
/// non-null default gets it, converted by the field's type, through
/// `this.setPropertyValue` (which validates it on a `ValidatedResource`).
pub fn assign_field_defaults(mm: &ModelManager, instance: &mut Instance) -> Result<()> {
    let class_decl = model::get_type(mm, &instance.class_fqn)?;
    for (owner_fqn, property) in class_decl.properties("classDeclaration.getProperties")? {
        // `isField?.()`: relationships are not `Field`s.
        if property.is_relationship() || property.is_enum_value() {
            continue;
        }
        let name = crate::Named::name(&property).to_string();
        let field = model::field(mm, &owner_fqn, property)?;
        let (default_value, type_name) = match &field.field_type {
            FieldType::Scalar {
                default_value,
                primitive,
                ..
            } => (
                default_value.clone(),
                primitive.map(str::to_string).unwrap_or_default(),
            ),
            _ => (raw_default_value(mm, &owner_fqn, &name), field.type_name()),
        };
        let Some(default_value) = default_value.filter(|v| !v.is_null()) else {
            continue;
        };
        let js = JsValue::from_json(&default_value);
        let value = match type_name.as_str() {
            "Integer" | "Long" => JsValue::Number(ecma::parse_int(&js.to_js_string())),
            "Double" => JsValue::Number(ecma::parse_float(&js.to_js_string())),
            "Boolean" => JsValue::Bool(js == JsValue::Bool(true)),
            "DateTime" => JsValue::DateTime(match &js {
                JsValue::String(s) => Dayjs::utc_parse(s),
                JsValue::Number(n) => Dayjs::utc_from_number(*n),
                _ => Dayjs::utc_invalid(),
            }),
            // String, and "if we get this far the field should be an enum".
            _ => js,
        };
        super::resource::set_property_value(mm, instance, &name, value)?;
    }
    Ok(())
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
        mm.add_model(&cto_ast, Some("test.cto".into()))
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
        let message = |r: Result<Instance>| match r {
            Err(ConcertoError::Contract(e)) => e.message(),
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
        ) {
            Err(ConcertoError::Contract(e)) => e.message(),
            other => panic!("expected an error, got {other:?}"),
        };
        assert_eq!(message, "ns not specified");
    }
}

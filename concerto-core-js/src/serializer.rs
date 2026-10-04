//! `Serializer` (src/serializer.ts): the constructor's checks, and
//! `fromJSON`/`toJSON` as one whole-document call each (PORTING.md section
//! 5 row 6, option B), over [`crate::populator`], [`crate::generator`] and
//! [`concerto_core::instance::validate`].

use std::borrow::Cow;

use super::resource;
use crate::factory::{self, InstanceEnv};
use crate::generator::{Generator, generator_options};
use crate::populator::{Populator, from_json_options, get_property};
use crate::value::{Instance, JsValue};
use concerto_core::Error;
use concerto_core::error::{ContractError, ErrorKind, Result};
use concerto_core::instance::from_json::FromJsonOptions;
use concerto_core::instance::model;
use concerto_core::instance::validate::{ValidateOptions, validate_instance_from};
use concerto_core::introspect::Declaration;
use concerto_core::model_manager::ModelManager;

/// A serializer options object (`SerializerOptions`), its keys in
/// insertion order.
pub type SerializerOptions = crate::value::JsObject;

/// A `Serializer`: its default options. The factory and the model manager
/// it holds in TS are the caller's (every call takes the model manager).
#[derive(Debug, Clone, PartialEq)]
pub struct Serializer {
    /// `this.defaultOptions`.
    pub default_options: SerializerOptions,
}

fn plain_error(code: &'static str) -> Error {
    ContractError::new(ErrorKind::InvalidArgument, code, Vec::new()).into()
}

/// `Object.assign({}, a, b)`.
fn assign(a: &SerializerOptions, b: &SerializerOptions) -> SerializerOptions {
    let mut merged = a.clone();
    for (k, v) in b {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// `baseDefaultOptions`: `{ validate: true, utcOffset }`, where `utcOffset`
/// is `DateTimeUtil.setCurrentTime().utcOffset`, `dayjs().utcOffset()`,
/// which under `TZ=UTC` is `-0` (PORTING.md 3.3).
fn base_default_options() -> SerializerOptions {
    let mut options = SerializerOptions::default();
    options.insert("validate".to_string(), JsValue::Bool(true));
    options.insert("utcOffset".to_string(), JsValue::Number(-0.0));
    options
}

impl Serializer {
    /// TS: the `Serializer` constructor: the factory and the model manager
    /// must be truthy, and the default options are `Object.assign({},
    /// baseDefaultOptions, options || {})`.
    pub fn new(
        factory_is_truthy: bool,
        model_manager_is_truthy: bool,
        options: Option<&SerializerOptions>,
    ) -> Result<Self> {
        if !factory_is_truthy {
            return Err(plain_error("serializer-constructor-factorynull"));
        }
        if !model_manager_is_truthy {
            return Err(plain_error("serializer-constructor-modelmanagernull"));
        }
        let empty = SerializerOptions::default();
        Ok(Self {
            default_options: assign(&base_default_options(), options.unwrap_or(&empty)),
        })
    }

    /// `options ? Object.assign({}, this.defaultOptions, options) :
    /// this.defaultOptions`: borrowed in the second case, which is read
    /// only (the copy was a measurable part of a `fromJSON` call).
    fn options(&self, options: Option<&SerializerOptions>) -> Cow<'_, SerializerOptions> {
        match options {
            Some(options) => Cow::Owned(assign(&self.default_options, options)),
            None => Cow::Borrowed(&self.default_options),
        }
    }

    /// TS: Serializer.fromJSON.
    pub fn from_json(
        &self,
        mm: &ModelManager,
        json_object: &JsValue,
        options: Option<&SerializerOptions>,
        env: &mut dyn InstanceEnv,
    ) -> Result<Instance> {
        let options = self.options(options);
        self.from_json_prepared(mm, json_object, &from_json_options(&options), env)
    }

    /// [`Self::from_json`] with its merged options already read
    /// ([`from_json_options`]): a caller that makes many calls
    /// with the same options reads them once.
    pub fn from_json_prepared(
        &self,
        mm: &ModelManager,
        json_object: &JsValue,
        options: &FromJsonOptions,
        env: &mut dyn InstanceEnv,
    ) -> Result<Instance> {
        let class_name = get_property(json_object, "$class")?;
        if !class_name.is_truthy() {
            return Err(plain_error("serializer-fromjson-noclass"));
        }
        // DV-015: TS has no type check here and either crashes in
        // `ModelUtil.getShortName`/`getNamespace` or, for an array, resolves
        // it to a `TypeNotFoundException`; kept as an explicit rejection
        // (maintainer-accepted).
        let Some(class_name) = class_name.as_str() else {
            return Err(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                format!(
                    "a $class that is not a string: {}",
                    class_name.to_js_string()
                ),
                None,
            )
            .into());
        };
        let class_declaration = model::get_type(mm, class_name)?;
        let ns = class_declaration.namespace();
        let name = class_declaration.name();
        let id = match class_declaration.identifier_field_name()? {
            Some(field) => get_property(json_object, field)?,
            None => get_property(json_object, "null")?,
        };
        let resource = if class_declaration.is_transaction() {
            let ns_value = JsValue::String(ns.to_string());
            let name_value = JsValue::String(name.to_string());
            factory::new_transaction_to_populate(mm, &ns_value, &name_value, id, false, env)?
        } else if class_declaration.is_event() {
            let ns_value = JsValue::String(ns.to_string());
            let name_value = JsValue::String(name.to_string());
            factory::new_event_to_populate(mm, &ns_value, &name_value, id, false, env)?
        } else if class_declaration.is_map_declaration() {
            return Err(plain_error("serializer-fromjson-mapnotsupported"));
        } else if class_declaration.is_enum() {
            return Err(plain_error("serializer-fromjson-enumnotsupported"));
        } else {
            // A concept, or any other class declaration:
            // `this.factory.newResource(ns, name, id)`.
            factory::new_resource_of(&class_declaration, id, false, env)?
        };

        let mut populator = Populator::new(mm, env, options);
        let mut resource =
            populator.visit_class_declaration(&class_declaration, json_object, resource)?;

        if options.validate {
            resource::validate(mm, &mut resource)?;
        }
        Ok(resource)
    }

    /// TS: Serializer.toJSON.
    pub fn to_json(
        &self,
        mm: &ModelManager,
        resource: &JsValue,
        options: Option<&SerializerOptions>,
    ) -> Result<JsValue> {
        let JsValue::Instance(instance) = resource else {
            return Err(plain_error("serializer-tojson-notcobject"));
        };
        let class_declaration = model::get_type(mm, &instance.class_fqn)?;
        let options = self.options(options);
        // The resource is read in place. It is copied only when
        // validation's write-back (`sync_identifiers`) changes it, which
        // it seldom does, and never when `validate` is off.
        let mut synced: Option<JsValue> = None;
        if options.get("validate").is_some_and(JsValue::is_truthy) {
            // `classDeclaration.accept(validator, parameters)`:
            // `ResourceValidator.visit` sends a class declaration to
            // `visitClassDeclaration`, and does nothing for a scalar.
            match class_declaration.decl {
                Declaration::Class(_) => {
                    let truthy = |key: &str| options.get(key).is_some_and(JsValue::is_truthy);
                    let validate_options = ValidateOptions {
                        convert_resources_to_relationships: truthy(
                            "convertResourcesToRelationships",
                        ),
                        permit_resources_for_relationships: truthy(
                            "permitResourcesForRelationships",
                        ),
                    };
                    validate_instance_from(
                        mm,
                        resource,
                        &validate_options,
                        "undefined".to_string(),
                    )?;
                    if resource::sync_needed(mm, instance)? {
                        let mut instance = (**instance).clone();
                        resource::sync_identifiers(mm, &mut instance)?;
                        synced = Some(JsValue::Instance(Box::new(instance)));
                    }
                }
                Declaration::Scalar(_) => {}
                // `visitEnumDeclaration`/`visitMapDeclaration` over a
                // resource: no instance is built with such a type.
                _ => {
                    return Err(ContractError::pre_port(
                        ErrorKind::InvalidArgument,
                        format!(
                            "an instance of {}, which is not a class",
                            instance.class_fqn
                        ),
                        None,
                    )
                    .into());
                }
            }
        }
        let generator_options = generator_options(&options);
        let mut generator = Generator::new(mm, &generator_options);
        generator.accept_declaration(&class_declaration, synced.as_ref().unwrap_or(resource))
    }
}

#[cfg(test)]
mod tests;

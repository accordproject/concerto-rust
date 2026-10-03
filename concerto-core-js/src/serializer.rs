//! `Serializer` (src/serializer.ts): the constructor's checks, and
//! `fromJSON`/`toJSON` as one whole-document call each (PORTING.md section
//! 5 row 6, option B), over [`crate::populator`], [`crate::generator`] and
//! [`concerto_core::instance::validate`] (task P3-01b, accordproject/concerto-rust#124).

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
    /// only (P5-16: the copy was a measurable part of a `fromJSON` call).
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
    /// ([`from_json_options`]): a caller that makes many calls with the same
    /// options reads them once (P5-16, accordproject/concerto-rust#310).
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
        // (maintainer-accepted, accordproject/concerto-rust#156).
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
        // P5-102 (C-6): the resource is read in place. It is copied only
        // when validation's write-back (`sync_identifiers`) changes it,
        // which it seldom does, and never when `validate` is off.
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
mod tests {
    use serde_json::json;

    use super::*;
    use crate::deserialize::{STRICT_VALIDATE_OPTIONS, serializer_options};
    use crate::value::InstanceKind;
    use concerto_core::error::{Detail, DetailCode};
    use concerto_core::instance::ValidationOptions;
    use concerto_core::instance::dayjs::Dayjs;

    struct Env;

    impl InstanceEnv for Env {
        fn new_id(&mut self) -> String {
            "00000000-0000-4000-8000-000000000000".into()
        }
        fn now_ms(&mut self) -> f64 {
            0.0
        }
    }

    fn property(class: &str, name: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut p = json!({
            "$class": format!("concerto.metamodel@1.0.0.{class}"),
            "name": name,
            "isArray": false,
            "isOptional": false,
        });
        for (k, v) in extra.as_object().expect("an object") {
            p[k] = v.clone();
        }
        p
    }

    fn model() -> ModelManager {
        let type_ref = |name: &str| json!({ "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": name });
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.EnumDeclaration",
                    "name": "Color",
                    "properties": [
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" }
                    ]
                },
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Address",
                    "isAbstract": false,
                    "properties": [ property("StringProperty", "city", json!({})) ]
                },
                {
                    "$class": "concerto.metamodel@1.0.0.ParticipantDeclaration",
                    "name": "Person",
                    "isAbstract": false,
                    "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "email" },
                    "properties": [ property("StringProperty", "email", json!({})) ]
                },
                {
                    "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                    "name": "Car",
                    "isAbstract": false,
                    "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "vin" },
                    "properties": [
                        property("StringProperty", "vin", json!({})),
                        property("IntegerProperty", "wheels", json!({ "isOptional": true })),
                        property("DateTimeProperty", "built", json!({ "isOptional": true })),
                        property("ObjectProperty", "address", json!({ "isOptional": true, "type": type_ref("Address") })),
                        property("ObjectProperty", "color", json!({ "isOptional": true, "type": type_ref("Color") })),
                        property("RelationshipProperty", "owner", json!({ "isOptional": true, "type": type_ref("Person") })),
                        property("ObjectProperty", "drivers", json!({ "isOptional": true, "type": type_ref("PersonMap") })),
                    ]
                },
                {
                    "$class": "concerto.metamodel@1.0.0.MapDeclaration",
                    "name": "PersonMap",
                    "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                    "value": { "$class": "concerto.metamodel@1.0.0.RelationshipMapValueType", "type": type_ref("Person") }
                }
            ]
        });
        let mut mm = ModelManager::new().expect("a model manager");
        mm.add_model_with_definitions(&ast, None, Some("test.cto".into()))
            .expect("the model loads");
        mm
    }

    fn serializer() -> Serializer {
        Serializer::new(true, true, None).expect("a serializer")
    }

    fn message(result: Result<impl std::fmt::Debug>) -> String {
        match result.map_err(Error::into_ported) {
            Err(Some(e)) => e.message(),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    fn car_json() -> JsValue {
        JsValue::from_json(&json!({
            "$class": "org.acme@1.0.0.Car",
            "vin": "ABC",
            "wheels": 4,
            "built": "2021-01-01T10:00:00Z",
            "address": { "$class": "org.acme@1.0.0.Address", "city": "Paris" },
            "color": "RED",
            "owner": "resource:org.acme@1.0.0.Person#bob"
        }))
    }

    #[test]
    fn from_json_populates_and_to_json_writes_back() {
        let mm = model();
        let car = serializer()
            .from_json(&mm, &car_json(), None, &mut Env)
            .expect("a car");
        assert_eq!(car.kind, InstanceKind::ValidatedResource);
        assert_eq!(car.get("wheels"), &JsValue::Number(4.0));
        assert!(matches!(car.get("built"), JsValue::DateTime(d) if d.is_utc()));
        let JsValue::Instance(owner) = car.get("owner") else {
            panic!("a relationship");
        };
        assert_eq!(owner.kind, InstanceKind::Relationship);
        assert_eq!(owner.get_identifier(), &JsValue::String("bob".into()));

        let json = serializer()
            .to_json(&mm, &JsValue::Instance(Box::new(car)), None)
            .expect("the car's JSON");
        let JsValue::Object(map) = json else {
            panic!("an object");
        };
        assert_eq!(
            map.get("$class"),
            Some(&JsValue::String("org.acme@1.0.0.Car".into()))
        );
        assert_eq!(
            map.get("built"),
            Some(&JsValue::String("2021-01-01T10:00:00.000Z".into()))
        );
        assert_eq!(
            map.get("owner"),
            Some(&JsValue::String(
                "resource:org.acme@1.0.0.Person#bob".into()
            ))
        );
    }

    #[test]
    fn to_json_formats_date_times_in_the_utc_offset() {
        let mm = model();
        let car = serializer()
            .from_json(&mm, &car_json(), None, &mut Env)
            .expect("a car");
        let options: SerializerOptions = [("utcOffset".to_string(), JsValue::Number(60.0))]
            .into_iter()
            .collect();
        let JsValue::Object(map) = serializer()
            .to_json(&mm, &JsValue::Instance(Box::new(car)), Some(&options))
            .expect("the car's JSON")
        else {
            panic!("an object");
        };
        assert_eq!(
            map.get("built"),
            Some(&JsValue::String("2021-01-01T11:00:00.000+01:00".into()))
        );
    }

    /// P5-24 (BC-45, R1): population applies a non-strict `DateTime`
    /// default only when the document gives the field no value (absent or
    /// `null`), and then throws a `ValidationException`; a value of its own
    /// replaces the default, at the root, in a nested concept and in a
    /// transaction alike.
    #[test]
    fn from_json_applies_a_non_strict_date_time_default_only_when_it_stays() {
        let at = |default: &str| {
            json!({
                "$class": "concerto.metamodel@1.0.0.DateTimeProperty",
                "name": "at", "isArray": false, "isOptional": true, "defaultValue": default
            })
        };
        let mut mm = ModelManager::new().expect("a model manager");
        mm.add_model_with_definitions(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.dates@1.0.0",
                "imports": [],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Inner",
                      "isAbstract": false, "properties": [ at("2008-09-15T15:53:00") ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Outer",
                      "isAbstract": false, "properties": [
                          at("2008-09-15T15:53:00"),
                          property("ObjectProperty", "inner", json!({
                              "isOptional": true,
                              "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Inner" }
                          }))
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.TransactionDeclaration", "name": "Tx",
                      "isAbstract": false, "properties": [ at("2022-11-18") ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Good",
                      "isAbstract": false, "properties": [ at("2008-09-15T15:53:00Z") ] }
                ]
            }),
            None,
            Some("dates.cto".into()),
        )
        .expect("a lenient DateTime default does not fail model load");
        let from = |v: serde_json::Value| {
            serializer().from_json(&mm, &JsValue::from_json(&v), None, &mut Env)
        };
        let code = |r: Result<Instance>| {
            let err = r.unwrap_err();
            assert_eq!(err.kind(), ErrorKind::Validation, "{err}");
            err.code().to_string()
        };
        let given = "2020-01-01T00:00:00Z";
        for bad in [
            json!({ "$class": "org.dates@1.0.0.Outer" }),
            json!({ "$class": "org.dates@1.0.0.Outer", "at": null }),
            json!({ "$class": "org.dates@1.0.0.Outer", "at": given,
                    "inner": { "$class": "org.dates@1.0.0.Inner" } }),
            json!({ "$class": "org.dates@1.0.0.Tx" }),
        ] {
            assert_eq!(code(from(bad)), "typed-assignfielddefaults-datetime");
        }
        for ok in [
            json!({ "$class": "org.dates@1.0.0.Outer", "at": given }),
            json!({ "$class": "org.dates@1.0.0.Outer", "at": given,
                    "inner": { "$class": "org.dates@1.0.0.Inner", "at": given } }),
            json!({ "$class": "org.dates@1.0.0.Tx", "at": given }),
            json!({ "$class": "org.dates@1.0.0.Good" }),
        ] {
            let instance = from(ok).expect("no non-strict default stays");
            assert!(matches!(instance.get("at"), JsValue::DateTime(d) if d.is_valid()));
        }
    }

    #[test]
    fn from_json_reports_the_populator_checks() {
        let mm = model();
        let from = |v: serde_json::Value| {
            serializer().from_json(&mm, &JsValue::from_json(&v), None, &mut Env)
        };
        assert_eq!(
            message(from(json!({ "vin": "A" }))),
            "Invalid JSON data. Does not contain a $class type identifier."
        );
        assert_eq!(
            message(from(
                json!({ "$class": "org.acme@1.0.0.Car", "vin": "A", "extra": 1, "more": 2 })
            )),
            "Unexpected properties for type org.acme@1.0.0.Car: extra, more"
        );
        assert_eq!(
            message(from(
                json!({ "$class": "org.acme@1.0.0.Car", "vin": "A", "$namespace": "x" })
            )),
            "Unexpected reserved properties for type org.acme@1.0.0.Car: $namespace"
        );
        assert_eq!(
            message(from(
                json!({ "$class": "org.acme@1.0.0.Car", "vin": "A", "wheels": 1.5 })
            )),
            "Expected value at path `$.wheels` to be of type `Integer`"
        );
        assert_eq!(
            message(from(
                json!({ "$class": "org.acme@1.0.0.Car", "vin": "A", "owner": 1 })
            )),
            "Invalid JSON data. Found a value that is not a string or object: 1 for relationship RelationshipDeclaration {name=owner, type=org.acme@1.0.0.Person, array=false, optional=true}"
        );
        assert_eq!(
            message(from(json!({ "$class": "org.acme@1.0.0.Color" }))),
            "Attempting to create an ENUM declaration is not supported."
        );
        // BC-08 (R1; DV-010 was V8's circular-JSON TypeError): an object that
        // names an enum type and one of its values is an `Error` naming the
        // enum value.
        let unrecognised = from(json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A",
            "address": { "$class": "org.acme@1.0.0.Color", "RED": "x" }
        }));
        assert_eq!(
            message(unrecognised),
            "Unrecognised element \"org.acme@1.0.0.Color.RED\""
        );
    }

    #[test]
    fn strict_date_times_need_an_offset() {
        let mm = model();
        let options: SerializerOptions =
            [("strictQualifiedDateTimes".to_string(), JsValue::Bool(true))]
                .into_iter()
                .collect();
        let json = JsValue::from_json(
            &json!({ "$class": "org.acme@1.0.0.Car", "vin": "A", "built": "2021-01-01" }),
        );
        assert_eq!(
            message(serializer().from_json(&mm, &json, Some(&options), &mut Env)),
            "Expected value at path `$.built` to be of type `DateTime` with format YYYY-MM-DDTHH:mm:ss[Z]"
        );
        // P5-24 (BC-07, R1): without the flag, the same format rule and
        // the same `ValidationException`; a strict string is accepted.
        assert_eq!(
            message(serializer().from_json(&mm, &json, None, &mut Env)),
            "Expected value at path `$.built` to be of type `DateTime` with format YYYY-MM-DDTHH:mm:ss[Z]"
        );
        let strict = JsValue::from_json(
            &json!({ "$class": "org.acme@1.0.0.Car", "vin": "A", "built": "2021-01-01T00:00:00Z" }),
        );
        let car = serializer()
            .from_json(&mm, &strict, None, &mut Env)
            .expect("a car");
        assert_eq!(
            car.get("built"),
            &JsValue::DateTime(Dayjs::utc_parse("2021-01-01T00:00:00Z"))
        );
    }

    /// P5-51 (BC-10, R1; DV-012): the populator rejects `±Infinity` for
    /// an Integer or Long field, with validation off as well as on (before,
    /// `Math.trunc(Infinity) === Infinity` let it through, and only the
    /// validator caught it).
    #[test]
    fn an_infinite_integer_is_rejected_by_the_populator() {
        let mm = model();
        for n in [f64::INFINITY, f64::NEG_INFINITY] {
            let mut json = car_json();
            let JsValue::Object(map) = &mut json else {
                unreachable!()
            };
            map.insert("wheels".into(), JsValue::Number(n));
            let options: SerializerOptions = [("validate".to_string(), JsValue::Bool(false))]
                .into_iter()
                .collect();
            for options in [Some(&options), None] {
                let err = serializer()
                    .from_json(&mm, &json, options, &mut Env)
                    .expect_err("a non-finite Integer");
                assert_eq!(err.kind().ts_class(), "ValidationException", "{n}: {err}");
                assert_eq!(
                    err.to_string(),
                    "Expected value at path `$.wheels` to be of type `Integer`"
                );
            }
        }
    }

    #[test]
    fn to_json_options_for_relationships() {
        let mm = model();
        let mut car = serializer()
            .from_json(&mm, &car_json(), None, &mut Env)
            .expect("a car");
        let person = factory::new_resource(
            &mm,
            "org.acme@1.0.0",
            "Person",
            JsValue::String("bob".into()),
            false,
            &mut Env,
        )
        .expect("a person");
        car.set("owner", JsValue::Instance(Box::new(person)));
        let car = JsValue::Instance(Box::new(car));
        let with = |key: &str| -> SerializerOptions {
            [
                (key.to_string(), JsValue::Bool(true)),
                ("validate".to_string(), JsValue::Bool(false)),
            ]
            .into_iter()
            .collect()
        };
        let owner =
            |options: &SerializerOptions| match serializer().to_json(&mm, &car, Some(options)) {
                Ok(JsValue::Object(map)) => map.get("owner").cloned(),
                other => panic!("{other:?}"),
            };
        assert_eq!(
            owner(&with("convertResourcesToRelationships")),
            Some(JsValue::String("resource:org.acme@1.0.0.Person#bob".into()))
        );
        assert!(matches!(
            owner(&with("permitResourcesForRelationships")),
            Some(JsValue::Object(_))
        ));
        let plain: SerializerOptions = [("validate".to_string(), JsValue::Bool(false))]
            .into_iter()
            .collect();
        assert_eq!(
            message(serializer().to_json(&mm, &car, Some(&plain))),
            "Did not find a relationship for org.acme@1.0.0.Person found Resource {id=org.acme@1.0.0.Person#bob}"
        );
    }

    /// A car whose `drivers` map holds `drivers`.
    fn car_with_drivers(drivers: serde_json::Value) -> JsValue {
        JsValue::from_json(&json!({
            "$class": "org.acme@1.0.0.Car",
            "vin": "ABC",
            "drivers": drivers,
        }))
    }

    fn options(entries: &[(&str, bool)]) -> SerializerOptions {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), JsValue::Bool(*v)))
            .collect()
    }

    /// P5-58 (BC-05, R1; DV-007): a relationship-typed map value is a
    /// relationship, as a relationship property is: a URI (or a bare
    /// identifier) populates a `Relationship`, validates, and is written
    /// back as its URI.
    #[test]
    fn a_relationship_map_value_is_a_relationship() {
        let mm = model();
        let car = serializer()
            .from_json(
                &mm,
                &car_with_drivers(
                    json!({ "a": "resource:org.acme@1.0.0.Person#bob", "b": "carol" }),
                ),
                None,
                &mut Env,
            )
            .expect("a car");
        let JsValue::Map(entries) = car.get("drivers") else {
            panic!("a map");
        };
        for (_, value) in entries.iter().filter(|(k, _)| k.as_str() != Some("$class")) {
            let JsValue::Instance(driver) = value else {
                panic!("a relationship, got {value:?}");
            };
            assert_eq!(driver.kind, InstanceKind::Relationship);
            assert_eq!(driver.class_fqn, "org.acme@1.0.0.Person");
        }
        let json = serializer()
            .to_json(&mm, &JsValue::Instance(Box::new(car)), None)
            .expect("the car's JSON");
        let JsValue::Object(map) = json else {
            panic!("an object");
        };
        assert_eq!(
            map.get("drivers").cloned(),
            Some(JsValue::from_json(&json!({
                "a": "resource:org.acme@1.0.0.Person#bob",
                "b": "resource:org.acme@1.0.0.Person#carol"
            })))
        );
    }

    /// P5-58 (BC-05, R1; DV-007): an embedded resource in a relationship
    /// map is read only with `acceptResourcesForRelationships`, validated
    /// only with `permitResourcesForRelationships` or
    /// `convertResourcesToRelationships`, and written in full only with
    /// `permitResourcesForRelationships`: the options a relationship
    /// property takes, with the same outcomes.
    #[test]
    fn a_relationship_map_value_takes_an_embedded_resource_only_with_the_options() {
        let mm = model();
        let embedded =
            car_with_drivers(json!({ "a": { "$class": "org.acme@1.0.0.Person", "email": "bob" } }));
        // fromJSON: not without `acceptResourcesForRelationships`.
        let err = serializer()
            .from_json(&mm, &embedded, None, &mut Env)
            .expect_err("an embedded resource needs the option");
        assert_eq!(err.kind().ts_class(), "Error", "{err}");
        // With it, and validation off, the value is a resource.
        let accept = options(&[
            ("acceptResourcesForRelationships", true),
            ("validate", false),
        ]);
        let car = serializer()
            .from_json(&mm, &embedded, Some(&accept), &mut Env)
            .expect("a car with an embedded driver");
        let JsValue::Map(entries) = car.get("drivers") else {
            panic!("a map");
        };
        let Some((_, JsValue::Instance(driver))) =
            entries.iter().find(|(k, _)| k.as_str() == Some("a"))
        else {
            panic!("an instance");
        };
        assert_ne!(driver.kind, InstanceKind::Relationship);
        // With validation on, the resource's own validator (no options)
        // rejects it, as for a property.
        let accept_validate = options(&[("acceptResourcesForRelationships", true)]);
        let err = serializer()
            .from_json(&mm, &embedded, Some(&accept_validate), &mut Env)
            .expect_err("validation rejects the embedded resource");
        assert_eq!(err.kind().ts_class(), "ValidationException", "{err}");

        // toJSON of the car holding the resource.
        let car = JsValue::Instance(Box::new(car));
        let drivers =
            |options: &SerializerOptions| match serializer().to_json(&mm, &car, Some(options)) {
                Ok(JsValue::Object(map)) => map.get("drivers").cloned(),
                other => panic!("{other:?}"),
            };
        for validate in [true, false] {
            assert_eq!(
                drivers(&options(&[
                    ("convertResourcesToRelationships", true),
                    ("validate", validate)
                ])),
                Some(JsValue::from_json(
                    &json!({ "a": "resource:org.acme@1.0.0.Person#bob" })
                ))
            );
            let Some(JsValue::Object(written)) = drivers(&options(&[
                ("permitResourcesForRelationships", true),
                ("validate", validate),
            ])) else {
                panic!("the drivers map");
            };
            let Some(JsValue::Object(driver)) = written.get("a") else {
                panic!("the driver in full, got {written:?}");
            };
            assert_eq!(
                driver.get("$class"),
                Some(&JsValue::String("org.acme@1.0.0.Person".into()))
            );
            assert_eq!(driver.get("email"), Some(&JsValue::String("bob".into())));
        }
        let err = serializer()
            .to_json(&mm, &car, Some(&options(&[("validate", false)])))
            .expect_err("the generator needs an option");
        assert_eq!(err.kind().ts_class(), "Error", "{err}");
        let err = serializer()
            .to_json(&mm, &car, None)
            .expect_err("the validator needs an option");
        assert_eq!(err.kind().ts_class(), "ValidationException", "{err}");
    }

    #[test]
    fn set_property_value_validates_on_a_validated_resource() {
        let mm = model();
        let mut car = serializer()
            .from_json(&mm, &car_json(), None, &mut Env)
            .expect("a car");
        assert_eq!(
            message(resource::set_property_value(
                &mm,
                &mut car,
                "nope",
                JsValue::Null
            )),
            "The instance with id ABC trying to set field nope which is not declared in the model."
        );
        let wrong = message(resource::set_property_value(
            &mm,
            &mut car,
            "wheels",
            JsValue::String("x".into()),
        ));
        assert!(
            wrong.contains("The field \"wheels\" has a value of \"\"x\"\""),
            "{wrong}"
        );
        resource::set_property_value(&mm, &mut car, "wheels", JsValue::Number(3.0))
            .expect("a valid value");
        assert_eq!(car.get("wheels"), &JsValue::Number(3.0));
        assert_eq!(
            message(resource::add_array_value(
                &mm,
                &mut car,
                "wheels",
                JsValue::Number(1.0)
            )),
            "The instance with id ABC trying to add array item wheels which is not declared as an array in the model."
        );
    }

    // ---- P3-02: accordproject/concerto#1273's scenario table ----

    fn contract_error(result: Result<Instance>) -> concerto_core::error::ContractError {
        match result {
            Err(e) => e.into_contract(),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// `from_json` with `options`' flags, plus `validate`.
    fn deserialize(
        json: serde_json::Value,
        options: ValidationOptions,
        validate: bool,
    ) -> Result<Instance> {
        let mut options = serializer_options(options);
        options.insert("validate".to_string(), JsValue::Bool(validate));
        serializer().from_json(
            &model(),
            &JsValue::from_json(&json),
            Some(&options),
            &mut Env,
        )
    }

    const DEFAULT: ValidationOptions = {
        let mut options = ValidationOptions::STRICT;
        options.reject_unknown_keys = false;
        options.reject_required_null = false;
        options
    };
    const UNKNOWN_KEYS: ValidationOptions = {
        let mut options = DEFAULT;
        options.reject_unknown_keys = true;
        options
    };
    const REQUIRED_NULL: ValidationOptions = {
        let mut options = DEFAULT;
        options.reject_required_null = true;
        options
    };

    /// A detail as `(path, code, expected, actual)`: `Detail` is
    /// `#[non_exhaustive]`, so it is compared field by field.
    type DetailFields = (String, DetailCode, Option<String>, Option<String>);

    fn fields(details: &[Detail]) -> Vec<DetailFields> {
        details
            .iter()
            .map(|d| (d.path.clone(), d.code, d.expected.clone(), d.actual.clone()))
            .collect()
    }

    fn unknown_property(path: &str) -> DetailFields {
        (path.to_string(), DetailCode::UnknownProperty, None, None)
    }

    /// Row 1: an unknown field set to `null`.
    #[test]
    fn an_unknown_null_field_is_ignored_unless_unknown_keys_are_rejected() {
        let json = json!({ "$class": "org.acme@1.0.0.Car", "vin": "A", "extra": null });
        for options in [DEFAULT, REQUIRED_NULL] {
            let car = deserialize(json.clone(), options, true).expect("a car");
            assert_eq!(car.get("extra"), &JsValue::Undefined);
        }
        for options in [UNKNOWN_KEYS, STRICT_VALIDATE_OPTIONS] {
            let error = contract_error(deserialize(json.clone(), options, true));
            assert_eq!(error.kind, ErrorKind::Validation);
            assert_eq!(
                error.message(),
                "Unexpected properties for type org.acme@1.0.0.Car: extra"
            );
            assert_eq!(fields(&error.details), vec![unknown_property("$.extra")]);
        }
    }

    /// Row 2: an unknown field set to a value. The default keeps the legacy
    /// error (no details); the flag reports every unknown key, `null` ones
    /// included, as a detail of its own.
    #[test]
    fn an_unknown_field_with_a_value_is_always_an_error() {
        let json = json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A",
            "extra": "x", "other": null,
            "address": { "$class": "org.acme@1.0.0.Address", "city": "Paris", "zip": 1 }
        });
        for options in [DEFAULT, REQUIRED_NULL] {
            let error = contract_error(deserialize(json.clone(), options, true));
            assert_eq!(
                error.code,
                "jsonpopulator-validateproperties-unexpectedproperties"
            );
            assert_eq!(
                error.message(),
                "Unexpected properties for type org.acme@1.0.0.Car: extra"
            );
            assert!(error.details.is_empty());
        }
        for options in [UNKNOWN_KEYS, STRICT_VALIDATE_OPTIONS] {
            let error = contract_error(deserialize(json.clone(), options, true));
            assert_eq!(
                error.code,
                "jsonpopulator-rejectunknownkeys-unknownproperties"
            );
            assert_eq!(
                error.message(),
                "Unexpected properties for type org.acme@1.0.0.Car: extra, other"
            );
            assert_eq!(
                fields(&error.details),
                vec![unknown_property("$.extra"), unknown_property("$.other")]
            );
        }
        // A nested declaration reports its own path.
        let nested = json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A",
            "address": { "$class": "org.acme@1.0.0.Address", "city": "Paris", "zip": null }
        });
        let error = contract_error(deserialize(nested, UNKNOWN_KEYS, true));
        assert_eq!(
            fields(&error.details),
            vec![unknown_property("$.address.zip")]
        );
    }

    /// Row 3: a required field set to `null`.
    #[test]
    fn a_required_null_field_fails_at_once_only_when_rejected() {
        let json = json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A",
            "address": { "$class": "org.acme@1.0.0.Address", "city": null }
        });
        for options in [DEFAULT, UNKNOWN_KEYS] {
            // Dropped by the populator, then reported by the validator.
            let error = contract_error(deserialize(json.clone(), options, true));
            assert_ne!(error.code, "jsonpopulator-rejectrequirednull-requirednull");
            assert!(error.details.is_empty());
            // With `validate: false`, the document hydrates without it.
            let car = deserialize(json.clone(), options, false).expect("a car");
            let JsValue::Instance(address) = car.get("address") else {
                panic!("an address");
            };
            assert_eq!(address.get("city"), &JsValue::Undefined);
        }
        for options in [REQUIRED_NULL, STRICT_VALIDATE_OPTIONS] {
            for validate in [true, false] {
                let error = contract_error(deserialize(json.clone(), options, validate));
                assert_eq!(error.kind, ErrorKind::Validation);
                assert_eq!(
                    error.message(),
                    "Expected value at path `$.address.city` to be of type `String`, but got null"
                );
                assert_eq!(
                    fields(&error.details),
                    vec![(
                        "$.address.city".to_string(),
                        DetailCode::TypeViolation,
                        Some("String".to_string()),
                        Some("null".to_string()),
                    )]
                );
            }
        }
    }

    /// Row 4: an optional field set to `null` is skipped under every option.
    #[test]
    fn an_optional_null_field_is_always_skipped() {
        let json = json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A",
            "wheels": null, "address": null, "owner": null
        });
        for options in [
            DEFAULT,
            UNKNOWN_KEYS,
            REQUIRED_NULL,
            STRICT_VALIDATE_OPTIONS,
        ] {
            let car = deserialize(json.clone(), options, true).expect("a car");
            assert_eq!(car.get("wheels"), &JsValue::Undefined);
            assert_eq!(car.get("address"), &JsValue::Undefined);
        }
    }

    /// The preset is both flags, and the flags only change the outcome for
    /// the rows above: a valid document populates the same under each.
    #[test]
    fn the_strict_preset_accepts_a_valid_document() {
        for options in [
            DEFAULT,
            UNKNOWN_KEYS,
            REQUIRED_NULL,
            STRICT_VALIDATE_OPTIONS,
        ] {
            let mut serializer_options = serializer_options(options);
            serializer_options.insert("validate".to_string(), JsValue::Bool(true));
            let car = serializer()
                .from_json(&model(), &car_json(), Some(&serializer_options), &mut Env)
                .expect("a car");
            assert_eq!(car.get("wheels"), &JsValue::Number(4.0));
        }
    }

    /// DV-015: a non-string `$class` on the top-level document.
    /// `ModelUtil.getShortName`/`getNamespace` call `fqn.lastIndexOf` on it
    /// with no type check, and TS's outcome then depends on *which*
    /// non-string value it is: `true`, a number or a plain object have no
    /// `lastIndexOf` of their own, so V8 throws an uncaught `TypeError:
    /// fqn.lastIndexOf is not a function`; an array does have its own
    /// `Array.prototype.lastIndexOf` (which searches for an element equal to
    /// `'.'`, finds none, and returns `-1` rather than throwing), so TS
    /// doesn't crash at all — it stringifies the array and raises a normal
    /// `TypeNotFoundException: Namespace is not defined for type "…"`
    /// instead. Either way, the underlying bug is the same missing type
    /// check, and Rust raises the same explicit rejection for every shape;
    /// the maintainer accepted keeping Rust's clearer, explicit rejection
    /// over reproducing either TS outcome (accordproject/concerto-rust#156,
    /// whose decision and #160's follow-up both confirm this covers the
    /// array shape too, not only the `TypeError` crash).
    #[test]
    fn a_non_string_class_on_the_document_is_an_explicit_error() {
        // `null`, `false`, `0` and `""` are falsy in JS and take the earlier
        // "no $class" branch instead (`serializer-fromjson-noclass`), both
        // in TS (`if (!jsonObject.$class)`) and here — only a truthy,
        // non-string `$class` reaches this check. `json!([])` is the
        // non-crashing TypeNotFoundException shape above, not the TypeError
        // crash; it's included here because Rust's rejection, and DV-015's
        // scope, are the same for both.
        for class in [json!(true), json!(1), json!([]), json!({})] {
            let json = json!({ "$class": class, "vin": "A" });
            let error = message(serializer().from_json(
                &model(),
                &JsValue::from_json(&json),
                None,
                &mut Env,
            ));
            assert_eq!(
                error,
                format!(
                    "a $class that is not a string: {}",
                    JsValue::from_json(&class).to_js_string()
                )
            );
        }
    }

    /// The same DV-015 check, reached through `JSONPopulator.convertItem`
    /// (`populator.rs`) for a nested object field rather than the top-level
    /// document.
    #[test]
    fn a_non_string_class_on_a_nested_field_is_an_explicit_error() {
        let json = json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A",
            "address": { "$class": true, "city": "Paris" }
        });
        let error =
            message(serializer().from_json(&model(), &JsValue::from_json(&json), None, &mut Env));
        assert_eq!(error, "a $class that is not a string: true");
    }

    #[test]
    fn the_serializer_constructor_checks_its_arguments() {
        assert_eq!(
            message(Serializer::new(false, true, None)),
            "\"Factory\" cannot be \"null\"."
        );
        assert_eq!(
            message(Serializer::new(true, false, None)),
            "\"ModelManager\" cannot be \"null\"."
        );
        let s = serializer();
        assert_eq!(
            s.default_options.get("validate"),
            Some(&JsValue::Bool(true))
        );
    }

    /// P5-99 (B-4, B-5): a relationship map's values are populated in the
    /// document's order, one entry per key, each as a relationship with the
    /// map's target as its default type.
    #[test]
    fn from_json_populates_every_entry_of_a_relationship_map() {
        let mm = model();
        let drivers: serde_json::Map<String, serde_json::Value> = (0..50)
            .map(|i| (format!("d{i}"), json!(format!("p{i}"))))
            .collect();
        let json = JsValue::from_json(&json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A", "drivers": drivers
        }));
        let car = serializer()
            .from_json(&mm, &json, None, &mut Env)
            .expect("a car");
        let JsValue::Map(entries) = car.get("drivers") else {
            panic!("a map");
        };
        assert_eq!(entries.len(), 50);
        for (i, (key, value)) in entries.iter().enumerate() {
            assert_eq!(key, &JsValue::String(format!("d{i}")));
            let JsValue::Instance(driver) = value else {
                panic!("a relationship");
            };
            assert_eq!(driver.kind, InstanceKind::Relationship);
            assert_eq!(driver.get_identifier(), &JsValue::String(format!("p{i}")));
        }
    }

    /// P5-99 (B-3): `rejectUnknownKeys` and `rejectRequiredNull` read the
    /// declaration's properties from the validation plan.
    #[test]
    fn from_json_strict_options_reject_unknown_keys_and_required_nulls() {
        let mm = model();
        let options: SerializerOptions = [
            ("rejectUnknownKeys".to_string(), JsValue::Bool(true)),
            ("rejectRequiredNull".to_string(), JsValue::Bool(true)),
        ]
        .into_iter()
        .collect();
        let from = |v: serde_json::Value| {
            serializer().from_json(&mm, &JsValue::from_json(&v), Some(&options), &mut Env)
        };
        let err = from(json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A", "extra": null, "more": 1, "wheels": null
        }))
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Validation, "{err}");
        assert_eq!(
            err.code(),
            "jsonpopulator-rejectunknownkeys-unknownproperties"
        );
        let paths: Vec<&str> = err.details().iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, ["$.extra", "$.more"]);
        assert!(
            err.details()
                .iter()
                .all(|d| d.code == DetailCode::UnknownProperty)
        );

        let err = from(json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A", "wheels": null,
            "address": { "$class": "org.acme@1.0.0.Address", "city": null }
        }))
        .unwrap_err();
        assert_eq!(err.code(), "jsonpopulator-rejectrequirednull-requirednull");
        let [detail] = err.details() else {
            panic!("one detail: {:?}", err.details());
        };
        assert_eq!(detail.path, "$.address.city");
        assert_eq!(detail.code, DetailCode::TypeViolation);
        assert_eq!(detail.expected.as_deref(), Some("String"));
        assert_eq!(detail.actual.as_deref(), Some("null"));

        // An optional `null` is not rejected.
        from(json!({ "$class": "org.acme@1.0.0.Car", "vin": "A", "wheels": null }))
            .expect("an optional null is allowed");
    }

    // ---- P5-102 (accordproject/concerto-rust#456, C-6) ----

    /// An asset with array fields, for `addArrayValue`.
    fn bag_model() -> ModelManager {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.bag@1.0.0",
            "imports": [],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                "name": "Bag",
                "isAbstract": false,
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" },
                "properties": [
                    property("StringProperty", "id", json!({})),
                    property("StringProperty", "tags", json!({ "isArray": true, "isOptional": true })),
                    property("IntegerProperty", "counts", json!({ "isArray": true, "isOptional": true })),
                ]
            }]
        });
        let mut mm = ModelManager::new().expect("a model manager");
        mm.add_model_with_definitions(&ast, None, Some("bag.cto".into()))
            .expect("the model loads");
        mm
    }

    /// `addArrayValue` pushes onto the property's own array, validating it
    /// with the new item as TS does, and leaves a rejected item out.
    #[test]
    fn add_array_value_pushes_in_place_and_leaves_a_rejected_item_out() {
        let mm = bag_model();
        let mut bag = serializer()
            .from_json(
                &mm,
                &JsValue::from_json(
                    &json!({ "$class": "org.bag@1.0.0.Bag", "id": "b", "tags": ["a"] }),
                ),
                None,
                &mut Env,
            )
            .expect("a bag");
        assert_eq!(bag.kind, InstanceKind::ValidatedResource);
        let strings = |items: &[&str]| {
            JsValue::Array(items.iter().map(|s| JsValue::String((*s).into())).collect())
        };
        resource::add_array_value(&mm, &mut bag, "tags", JsValue::String("b".into()))
            .expect("a string");
        assert_eq!(bag.get("tags"), &strings(&["a", "b"]));
        let wrong = message(resource::add_array_value(
            &mm,
            &mut bag,
            "tags",
            JsValue::Number(1.0),
        ));
        assert!(wrong.contains("tags"), "{wrong}");
        assert_eq!(bag.get("tags"), &strings(&["a", "b"]));
        // A property with no array yet gets a new one, appended last.
        resource::add_array_value(&mm, &mut bag, "counts", JsValue::Number(1.0))
            .expect("an integer");
        assert_eq!(
            bag.get("counts"),
            &JsValue::Array(vec![JsValue::Number(1.0)])
        );
        assert_eq!(bag.props.keys().last().map(String::as_str), Some("counts"));
        // A rejected first item leaves the property as it was.
        bag.props.shift_remove("counts");
        message(resource::add_array_value(
            &mm,
            &mut bag,
            "counts",
            JsValue::String("x".into()),
        ));
        assert!(!bag.props.contains_key("counts"));
        // A truthy non-array is TS's `slice` TypeError, and changes nothing.
        bag.set("tags", JsValue::String("x".into()));
        assert!(
            resource::add_array_value(&mm, &mut bag, "tags", JsValue::String("y".into())).is_err()
        );
        assert_eq!(bag.get("tags"), &JsValue::String("x".into()));
    }

    /// The validator reads an instance in place exactly as it reads the
    /// instance's plain-JSON shape: the same verdict and the same error,
    /// for a valid car and for each way a field can be wrong.
    #[test]
    fn validation_in_place_matches_the_plain_json_shape() {
        let mm = model();
        let car = serializer()
            .from_json(&mm, &car_json(), None, &mut Env)
            .expect("a car");
        let person = |id: &str, kind: InstanceKind| {
            JsValue::Instance(Box::new(Instance::new(
                kind,
                "org.acme@1.0.0.Person",
                "org.acme@1.0.0",
                "Person",
                Some("email".into()),
                JsValue::String(id.into()),
                JsValue::Undefined,
            )))
        };
        let plain = |v: serde_json::Value| JsValue::from_json(&v);
        let variants: Vec<(&str, JsValue)> = vec![
            ("wheels", JsValue::String("x".into())),
            ("wheels", JsValue::Number(f64::NAN)),
            ("wheels", JsValue::Number(f64::INFINITY)),
            ("wheels", JsValue::Number(-0.0)),
            ("wheels", JsValue::BigInt("12".into())),
            ("wheels", JsValue::Array(vec![JsValue::Undefined])),
            ("wheels", plain(json!({ "$$undefined": true }))),
            ("wheels", plain(json!({ "$$number": "NaN" }))),
            ("built", JsValue::String("2021-01-01T10:00:00Z".into())),
            (
                "built",
                plain(json!({ "$$dayjs": "2021-01-01T10:00:00.000Z" })),
            ),
            ("address", person("bob", InstanceKind::Resource)),
            ("address", person("bob", InstanceKind::Relationship)),
            (
                "address",
                plain(json!({ "$class": "org.acme@1.0.0.Address", "city": 1 })),
            ),
            ("address", plain(json!({ "city": "Paris" }))),
            ("address", JsValue::Map(vec![])),
            ("color", JsValue::String("BLUE".into())),
            ("color", person("bob", InstanceKind::Relationship)),
            ("owner", person("bob", InstanceKind::Resource)),
            (
                "owner",
                plain(
                    json!({ "$$relationship": true, "$class": "org.acme@1.0.0.Person", "email": "x" }),
                ),
            ),
            (
                "owner",
                JsValue::String("resource:org.acme@1.0.0.Person#bob".into()),
            ),
            (
                "drivers",
                JsValue::Map(vec![(
                    JsValue::String("x".into()),
                    person("bob", InstanceKind::Relationship),
                )]),
            ),
            (
                "drivers",
                JsValue::Map(vec![(JsValue::Number(1.0), JsValue::String("x".into()))]),
            ),
            ("drivers", plain(json!({ "$$map": [["x", 1]] }))),
            ("drivers", plain(json!({ "x": 1 }))),
            ("vin", JsValue::String(" ".into())),
            ("extra", JsValue::Number(1.0)),
            ("$class", JsValue::String("org.acme@1.0.0.Address".into())),
        ];
        let mut cars = vec![car.clone()];
        for (name, value) in variants {
            let mut variant = car.clone();
            variant.set(name, value);
            cars.push(variant);
        }
        for options in [
            ValidateOptions::default(),
            ValidateOptions {
                convert_resources_to_relationships: true,
                permit_resources_for_relationships: false,
            },
        ] {
            for car in &cars {
                let in_place = validate_instance_from(
                    &mm,
                    &JsValue::Instance(Box::new(car.clone())),
                    &options,
                    car.fully_qualified_identifier(),
                );
                let plain_json = validate_instance_from(
                    &mm,
                    &car.to_validator_value(),
                    &options,
                    car.fully_qualified_identifier(),
                );
                assert_eq!(
                    format!("{in_place:?}"),
                    format!("{plain_json:?}"),
                    "{:?}",
                    car.props
                );
            }
        }
    }

    /// `toJSON` writes what it wrote when it synced a copy of the resource
    /// every time, and never changes the resource it is given.
    #[test]
    fn to_json_reads_the_resource_in_place() {
        let mm = model();
        let car = serializer()
            .from_json(&mm, &car_json(), None, &mut Env)
            .expect("a car");
        let mut stale = car.clone();
        stale.set("$identifier", JsValue::String("old".into()));
        let mut synced = stale.clone();
        resource::sync_identifiers(&mm, &mut synced).expect("synced");
        assert_ne!(synced, stale);
        assert!(resource::sync_needed(&mm, &stale).unwrap());
        assert!(!resource::sync_needed(&mm, &synced).unwrap());
        let stale = JsValue::Instance(Box::new(stale));
        let before = stale.clone();
        let written = serializer().to_json(&mm, &stale, None).expect("JSON");
        assert_eq!(stale, before);
        let expected = serializer()
            .to_json(&mm, &JsValue::Instance(Box::new(synced)), None)
            .expect("JSON");
        assert_eq!(written, expected);
    }

    /// A map value whose keys stringify alike: `Object.fromEntries` keeps
    /// the first key's place and the last value.
    #[test]
    fn to_json_writes_a_map_key_set_twice_once() {
        let mm = model();
        let mut car = serializer()
            .from_json(&mm, &car_json(), None, &mut Env)
            .expect("a car");
        let JsValue::Instance(owner) = car.get("owner").clone() else {
            panic!("a relationship");
        };
        let relationship = |id: &str| {
            let mut r = (*owner).clone();
            r.set_identifier(JsValue::String(id.into()));
            JsValue::Instance(Box::new(r))
        };
        car.set(
            "drivers",
            JsValue::Map(vec![
                (JsValue::Number(1.0), relationship("a")),
                (JsValue::String("x".into()), relationship("b")),
                (JsValue::String("1".into()), relationship("c")),
            ]),
        );
        let options: SerializerOptions = [("validate".to_string(), JsValue::Bool(false))]
            .into_iter()
            .collect();
        let JsValue::Object(json) = serializer()
            .to_json(&mm, &JsValue::Instance(Box::new(car)), Some(&options))
            .expect("JSON")
        else {
            panic!("an object");
        };
        let JsValue::Object(drivers) = json.get("drivers").expect("drivers") else {
            panic!("an object");
        };
        let entries: Vec<(&str, &JsValue)> = drivers.iter().map(|(k, v)| (k.as_str(), v)).collect();
        assert_eq!(
            entries,
            [
                (
                    "1",
                    &JsValue::String("resource:org.acme@1.0.0.Person#c".into())
                ),
                (
                    "x",
                    &JsValue::String("resource:org.acme@1.0.0.Person#b".into())
                ),
            ]
        );
    }
}

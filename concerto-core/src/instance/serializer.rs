//! `Serializer` (src/serializer.ts): the constructor's checks, and
//! `fromJSON`/`toJSON` as one whole-document call each (PORTING.md section
//! 5 row 6, option B), over [`super::populator`], [`super::generator`] and
//! [`super::validate`] (task P3-01b, accordproject/concerto-rust#124).

use indexmap::IndexMap;

use super::factory::{self, InstanceEnv};
use super::generator::{Generator, generator_options};
use super::model;
use super::populator::{Populator, get_property, populator_options};
use super::resource;
use super::validate::{ValidateOptions, validate_instance_from};
use super::value::{Instance, JsValue};
use crate::ConcertoError;
use crate::error::{ContractError, ErrorKind, Result};
use crate::introspect::Declaration;
use crate::model_manager::ModelManager;

/// A serializer options object (`SerializerOptions`), its keys in
/// insertion order.
pub type SerializerOptions = IndexMap<String, JsValue>;

/// A `Serializer`: its default options. The factory and the model manager
/// it holds in TS are the caller's (every call takes the model manager).
#[derive(Debug, Clone, PartialEq)]
pub struct Serializer {
    /// `this.defaultOptions`.
    pub default_options: SerializerOptions,
}

fn plain_error(code: &'static str) -> ConcertoError {
    ContractError::new(ErrorKind::Error, code, Vec::new()).into()
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
    let mut options = SerializerOptions::new();
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
        let empty = SerializerOptions::new();
        Ok(Self {
            default_options: assign(&base_default_options(), options.unwrap_or(&empty)),
        })
    }

    /// `options ? Object.assign({}, this.defaultOptions, options) :
    /// this.defaultOptions`.
    fn options(&self, options: Option<&SerializerOptions>) -> SerializerOptions {
        match options {
            Some(options) => assign(&self.default_options, options),
            None => self.default_options.clone(),
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

        let class_name = get_property(json_object, "$class")?;
        if !class_name.is_truthy() {
            return Err(plain_error("serializer-fromjson-noclass"));
        }
        let Some(class_name) = class_name.as_str() else {
            return Err(ContractError::pre_port(
                ErrorKind::Error,
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
        let name = class_declaration.name().to_string();
        let id = match class_declaration.identifier_field_name()? {
            Some(field) => get_property(json_object, &field)?,
            None => get_property(json_object, "null")?,
        };
        let ns_value = JsValue::String(ns.clone());
        let name_value = JsValue::String(name.clone());
        let resource = if class_declaration.is_transaction() {
            factory::new_transaction(mm, &ns_value, &name_value, id, false, env)?
        } else if class_declaration.is_event() {
            factory::new_event(mm, &ns_value, &name_value, id, false, env)?
        } else if class_declaration.is_concept() {
            factory::new_resource(mm, &ns, &name, id, false, env)?
        } else if class_declaration.is_map_declaration() {
            return Err(plain_error("serializer-fromjson-mapnotsupported"));
        } else if class_declaration.is_enum() {
            return Err(plain_error("serializer-fromjson-enumnotsupported"));
        } else {
            factory::new_resource(mm, &ns, &name, id, false, env)?
        };

        let populator_options = populator_options(&options);
        let mut populator = Populator::new(mm, env, &populator_options);
        let mut resource =
            populator.visit_class_declaration(&class_declaration, json_object, resource)?;

        if options.get("validate").is_some_and(JsValue::is_truthy) {
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
        let mut instance = (**instance).clone();
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
                        &instance.to_validator_value(),
                        &validate_options,
                        "undefined".to_string(),
                    )?;
                    resource::sync_identifiers(mm, &mut instance)?;
                }
                Declaration::Scalar(_) => {}
                // `visitEnumDeclaration`/`visitMapDeclaration` over a
                // resource: no instance is built with such a type.
                Declaration::Enum(_) | Declaration::Map(_) => {
                    return Err(ContractError::pre_port(
                        ErrorKind::Error,
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
        generator.accept_declaration(&class_declaration, &JsValue::Instance(Box::new(instance)))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::{DetailCode, ValidationDetail};
    use crate::instance::dayjs::Dayjs;
    use crate::instance::deserialize::{DeserializeOptions, STRICT_VALIDATE_OPTIONS};
    use crate::instance::value::InstanceKind;

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
                    ]
                }
            ]
        });
        let mut mm = ModelManager::new().expect("a model manager");
        mm.add_model(&ast, Some("test.cto".into()))
            .expect("the model loads");
        mm
    }

    fn serializer() -> Serializer {
        Serializer::new(true, true, None).expect("a serializer")
    }

    fn message(result: Result<impl std::fmt::Debug>) -> String {
        match result {
            Err(ConcertoError::Contract(e)) => e.message(),
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
        // DV-010: an object that names an enum type and one of its values.
        let circular = from(json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A",
            "address": { "$class": "org.acme@1.0.0.Color", "RED": "x" }
        }));
        assert!(message(circular).starts_with("Converting circular structure to JSON"));
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
        let lenient = serializer()
            .from_json(&mm, &json, None, &mut Env)
            .expect("a car");
        assert_eq!(
            lenient.get("built"),
            &JsValue::DateTime(Dayjs::utc_parse("2021-01-01"))
        );
    }

    /// DV-012: `Math.trunc(Infinity) === Infinity`, so the populator takes
    /// it as an Integer; the validator then rejects it.
    #[test]
    fn an_infinite_integer_passes_the_populator() {
        let mm = model();
        let mut json = car_json();
        let JsValue::Object(map) = &mut json else {
            unreachable!()
        };
        map.insert("wheels".into(), JsValue::Number(f64::INFINITY));
        let options: SerializerOptions = [("validate".to_string(), JsValue::Bool(false))]
            .into_iter()
            .collect();
        let car = serializer()
            .from_json(&mm, &json, Some(&options), &mut Env)
            .expect("a car");
        assert_eq!(car.get("wheels"), &JsValue::Number(f64::INFINITY));
        assert!(
            message(serializer().from_json(&mm, &json, None, &mut Env))
                .contains("has a value of \"Infinity\"")
        );
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

    fn contract_error(result: Result<Instance>) -> crate::error::ContractError {
        match result {
            Err(ConcertoError::Contract(e)) => *e,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// `from_json` with `options`' flags, plus `validate`.
    fn deserialize(
        json: serde_json::Value,
        options: DeserializeOptions,
        validate: bool,
    ) -> Result<Instance> {
        let mut options = options.serializer_options();
        options.insert("validate".to_string(), JsValue::Bool(validate));
        serializer().from_json(
            &model(),
            &JsValue::from_json(&json),
            Some(&options),
            &mut Env,
        )
    }

    const DEFAULT: DeserializeOptions = DeserializeOptions {
        reject_unknown_keys: false,
        reject_required_null: false,
    };
    const UNKNOWN_KEYS: DeserializeOptions = DeserializeOptions {
        reject_unknown_keys: true,
        reject_required_null: false,
    };
    const REQUIRED_NULL: DeserializeOptions = DeserializeOptions {
        reject_unknown_keys: false,
        reject_required_null: true,
    };

    fn unknown_property(path: &str) -> ValidationDetail {
        ValidationDetail {
            path: path.to_string(),
            code: DetailCode::UnknownProperty,
            expected: None,
            actual: None,
        }
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
            assert_eq!(error.details, vec![unknown_property("$.extra")]);
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
                error.details,
                vec![unknown_property("$.extra"), unknown_property("$.other")]
            );
        }
        // A nested declaration reports its own path.
        let nested = json!({
            "$class": "org.acme@1.0.0.Car", "vin": "A",
            "address": { "$class": "org.acme@1.0.0.Address", "city": "Paris", "zip": null }
        });
        let error = contract_error(deserialize(nested, UNKNOWN_KEYS, true));
        assert_eq!(error.details, vec![unknown_property("$.address.zip")]);
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
                    error.details,
                    vec![ValidationDetail {
                        path: "$.address.city".to_string(),
                        code: DetailCode::TypeViolation,
                        expected: Some("String".to_string()),
                        actual: Some("null".to_string()),
                    }]
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
            let mut serializer_options = options.serializer_options();
            serializer_options.insert("validate".to_string(), JsValue::Bool(true));
            let car = serializer()
                .from_json(&model(), &car_json(), Some(&serializer_options), &mut Env)
                .expect("a car");
            assert_eq!(car.get("wheels"), &JsValue::Number(4.0));
        }
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
}

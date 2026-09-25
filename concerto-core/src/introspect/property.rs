//! Properties, with their types kept intact.
//!
//! [`Property`] is a sum type whose variants are newtypes over the generated
//! metamodel structs: the eight field kinds of [`mm::Property`] and
//! [`mm::EnumProperty`]. Each node is deserialized into the concrete struct its
//! `$class` names, whether the `$class` is fully qualified or given as its bare
//! short name, so the validators and the referenced `type` are kept whole. A
//! class declaration keeps its properties as this type too, because it also
//! accepts an `EnumProperty`, which the generated `mm::Property` union does
//! not cover. The getters hang off the enum directly, and those it shares
//! with the declarations come from the traits in [`crate::introspect`].

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::Value;

use crate::derive::Named;
use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::decorator::{Decorated, Decorator, WithDecorators, parse_decorators};
use crate::introspect::{
    HasValidators, Named, Typed, check_domain, check_length, check_pattern, check_size,
    declared_class,
};
use crate::model_util::{get_short_name, is_system_property, is_valid_identifier};

/// What `Property.process` computes, after `super.process()` (which belongs
/// to `Decorated`).
///
/// TS: `Property.process` (src/introspect/property.ts). `property_type` is
/// `this.type`; `type_set` says whether TS assigns `this.type` at all —
/// the `EnumProperty` arm of the source switch falls through without an
/// assignment, so `this.type` is left `undefined` there, which the WASM view
/// tells apart from the explicit `null` an `ObjectProperty` with no `type`
/// AST node gets.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedProperty {
    /// `this.name`.
    pub name: String,
    /// `this.type`, when the switch sets it.
    pub property_type: Option<String>,
    /// Whether the switch sets `this.type` at all (`false` for `EnumProperty`).
    pub type_set: bool,
    /// `this.array`.
    pub array: bool,
    /// `this.optional`.
    pub optional: bool,
}

/// Computes `Property.process`'s fields directly from the AST, in the TS
/// order: the identifier check, the name, the `$class` switch for `type`,
/// then `array` and `optional`. `this.sizeValidator` is not computed here:
/// TS builds it by constructing a `CollectionSizeValidator`, which the WASM
/// view still does directly (its own binding already ports the TS
/// constructor).
///
/// TS: `Property.process` (src/introspect/property.ts)
pub fn process<E: From<ContractError>>(ast: &Value) -> std::result::Result<ProcessedProperty, E> {
    let name = ast
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !is_valid_identifier(&name) {
        return Err(ContractError::new(
            ErrorKind::IllegalModel,
            "property-process-invalidname",
            vec![("name", name)],
        )
        .into());
    }

    let class = ast
        .get("$class")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let short = get_short_name(class);
    let object_or_relationship_type = || {
        ast.get("type")
            .and_then(|t| t.get("name"))
            .and_then(Value::as_str)
            .map(String::from)
    };
    let (property_type, type_set): (Option<String>, bool) = match short {
        "BooleanProperty" => (Some("Boolean".to_string()), true),
        "DateTimeProperty" => (Some("DateTime".to_string()), true),
        "DoubleProperty" => (Some("Double".to_string()), true),
        "IntegerProperty" => (Some("Integer".to_string()), true),
        "LongProperty" => (Some("Long".to_string()), true),
        "StringProperty" => (Some("String".to_string()), true),
        "ObjectProperty" => (object_or_relationship_type(), true),
        "RelationshipProperty" => (object_or_relationship_type(), true),
        // `EnumProperty`, or anything else: the TS switch has no matching
        // `case`, so `this.type` is left unassigned.
        _ => (None, false),
    };

    let array = ast.get("isArray").and_then(Value::as_bool).unwrap_or(false);
    let optional = ast
        .get("isOptional")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(ProcessedProperty {
        name,
        property_type,
        type_set,
        array,
        optional,
    })
}

/// A single property of a concept-like or enum declaration. Each variant also
/// carries its processed decorators (module doc on
/// [`crate::introspect::decorator::WithDecorators`]).
#[derive(Debug, Clone, Named)]
pub enum Property {
    /// A `Boolean` primitive field.
    Boolean(WithDecorators<mm::BooleanProperty>),
    /// A `String` primitive field (may carry regex/length validators).
    String(WithDecorators<mm::StringProperty>),
    /// An `Integer` primitive field (may carry a domain validator).
    Integer(WithDecorators<mm::IntegerProperty>),
    /// A `Long` primitive field (may carry a domain validator).
    Long(WithDecorators<mm::LongProperty>),
    /// A `Double` primitive field (may carry a domain validator).
    Double(WithDecorators<mm::DoubleProperty>),
    /// A `DateTime` primitive field.
    DateTime(WithDecorators<mm::DateTimeProperty>),
    /// A field whose type is another declared concept/scalar.
    Object(WithDecorators<mm::ObjectProperty>),
    /// A relationship reference to an identifiable declaration.
    Relationship(WithDecorators<mm::RelationshipProperty>),
    /// A value member of an enum declaration.
    Enum(WithDecorators<mm::EnumProperty>),
}

/// Picks the same field out of whichever generated struct a [`Property`]
/// holds. The eight field kinds share the metamodel's property fields; an
/// enum value has fewer, so its arm is given separately.
macro_rules! property_field {
    ($property:expr, $p:ident => $field:expr, $value:pat => $enum_value:expr) => {
        match $property {
            Property::Boolean($p) => $field,
            Property::String($p) => $field,
            Property::Integer($p) => $field,
            Property::Long($p) => $field,
            Property::Double($p) => $field,
            Property::DateTime($p) => $field,
            Property::Object($p) => $field,
            Property::Relationship($p) => $field,
            Property::Enum($value) => $enum_value,
        }
    };
}

impl Property {
    /// Whether the property is an array (`[]`). Enum members are never arrays.
    pub fn is_array(&self) -> bool {
        property_field!(self, p => p.is_array, _ => false)
    }

    /// Whether the property is optional. Enum members are never optional.
    pub fn is_optional(&self) -> bool {
        property_field!(self, p => p.is_optional, _ => false)
    }

    /// `true` for the six primitive property kinds.
    pub fn is_primitive(&self) -> bool {
        matches!(
            self,
            Self::Boolean(_)
                | Self::String(_)
                | Self::Integer(_)
                | Self::Long(_)
                | Self::Double(_)
                | Self::DateTime(_)
        )
    }

    /// `true` if this is a relationship reference.
    pub fn is_relationship(&self) -> bool {
        matches!(self, Self::Relationship(_))
    }

    /// `true` if this is an enum value member.
    pub fn is_enum_value(&self) -> bool {
        matches!(self, Self::Enum(_))
    }

    /// The referenced type identifier, for object and relationship properties.
    pub fn type_identifier(&self) -> Option<&mm::TypeIdentifier> {
        match self {
            Self::Object(p) => Some(&p.type_),
            Self::Relationship(p) => Some(&p.type_),
            _ => None,
        }
    }

    /// The collection size validator, if one is declared on this property.
    pub fn size_validator(&self) -> Option<&mm::CollectionSizeValidator> {
        property_field!(self, p => p.size_validator.as_ref(), _ => None)
    }
}

impl Typed for Property {
    /// The name of the property's type. For primitives that's the primitive
    /// itself; for object/relationship properties it's the type they point at.
    /// Enum members don't have a type, so they get `None`.
    fn type_name(&self) -> Option<&str> {
        match self {
            Self::Boolean(_) => Some("Boolean"),
            Self::String(_) => Some("String"),
            Self::Integer(_) => Some("Integer"),
            Self::Long(_) => Some("Long"),
            Self::Double(_) => Some("Double"),
            Self::DateTime(_) => Some("DateTime"),
            Self::Object(p) => Some(&p.type_.name),
            Self::Relationship(p) => Some(&p.type_.name),
            Self::Enum(_) => None,
        }
    }
}

impl Decorated for Property {
    fn get_decorators(&self) -> &[Decorator] {
        property_field!(self, p => p.decorators(), p => p.decorators())
    }
}

impl TryFrom<&serde_json::Value> for Property {
    type Error = ConcertoError;

    fn try_from(value: &serde_json::Value) -> Result<Self> {
        let class = declared_class(value);
        if class.is_empty() {
            return Err(ConcertoError::IllegalModel {
                message: "property node is missing its $class".into(),
                file_name: None,
                location: None,
            });
        }
        // Concerto keeps a set of property names for itself, so a model may
        // not declare a field with one of them.
        if let Some(name) = value.get("name").and_then(|n| n.as_str())
            && is_system_property(name)
        {
            return Err(ConcertoError::IllegalModel {
                message: format!("Invalid field name '{name}'"),
                file_name: None,
                location: None,
            });
        }
        let kind = get_short_name(class);

        // Parse into whatever struct the `$class` says this is. If serde
        // chokes, the JSON is malformed for the kind it claims to be.
        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid {kind}: {e}"),
            file_name: None,
            location: None,
        };

        let decorators = parse_decorators(value);
        let property = match kind {
            "BooleanProperty" => Self::Boolean(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            "StringProperty" => Self::String(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            "IntegerProperty" => Self::Integer(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            "LongProperty" => Self::Long(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            "DoubleProperty" => Self::Double(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            "DateTimeProperty" => Self::DateTime(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            "ObjectProperty" => Self::Object(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            "RelationshipProperty" => Self::Relationship(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            "EnumProperty" => Self::Enum(WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(bad)?,
                decorators,
            )),
            other => {
                return Err(ConcertoError::IllegalModel {
                    message: format!("unknown property type: {other}"),
                    file_name: None,
                    location: None,
                });
            }
        };
        if !is_valid_identifier(property.name()) {
            return Err(ConcertoError::IllegalModel {
                message: format!("invalid identifier: {}", property.name()),
                file_name: None,
                location: None,
            });
        }
        property.check_validators()?;
        Ok(property)
    }
}

impl Property {
    /// Checks a collection size validator, which only an array may carry
    /// unless `allow_non_array` is set.
    fn check_size_validator(
        name: &str,
        is_array: bool,
        validator: &Option<mm::CollectionSizeValidator>,
        allow_non_array: bool,
    ) -> Result<()> {
        if let Some(v) = validator {
            if !is_array && !allow_non_array {
                return Err(ConcertoError::IllegalModel {
                    message: format!(
                        "size validator can only be applied to array or map properties: {name}"
                    ),
                    file_name: None,
                    location: None,
                });
            }
            check_size(name, v)?;
        }
        Ok(())
    }
}

impl HasValidators for Property {
    /// Checks the validators this property carries: a numeric range, a string
    /// length, a regular expression, and a collection size. These are part of
    /// the property's own declaration, so they are checked while loading
    /// rather than left to the validation pass.
    fn check_validators(&self) -> Result<()> {
        match self {
            Self::String(p) => {
                if let Some(validator) = &p.validator {
                    check_pattern(&p.name, validator)?;
                }
                if let Some(validator) = &p.length_validator {
                    check_length(&p.name, validator)?;
                }
                Self::check_size_validator(&p.name, p.is_array, &p.size_validator, false)
            }
            Self::Integer(p) => {
                if let Some(validator) = &p.validator {
                    check_domain(&p.name, validator.lower, validator.upper)?;
                }
                Self::check_size_validator(&p.name, p.is_array, &p.size_validator, false)
            }
            Self::Long(p) => {
                if let Some(validator) = &p.validator {
                    check_domain(&p.name, validator.lower, validator.upper)?;
                }
                Self::check_size_validator(&p.name, p.is_array, &p.size_validator, false)
            }
            Self::Double(p) => {
                if let Some(validator) = &p.validator {
                    check_domain(&p.name, validator.lower, validator.upper)?;
                }
                Self::check_size_validator(&p.name, p.is_array, &p.size_validator, false)
            }
            Self::Boolean(p) => {
                Self::check_size_validator(&p.name, p.is_array, &p.size_validator, false)
            }
            Self::DateTime(p) => {
                Self::check_size_validator(&p.name, p.is_array, &p.size_validator, false)
            }
            Self::Object(p) => {
                Self::check_size_validator(&p.name, p.is_array, &p.size_validator, true)
            }
            Self::Relationship(p) => {
                Self::check_size_validator(&p.name, p.is_array, &p.size_validator, false)
            }
            Self::Enum(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_derives_type_array_and_optional() {
        let processed = process::<ConcertoError>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "email",
            "isArray": true,
            "isOptional": true
        }))
        .expect("valid");
        assert_eq!(processed.name, "email");
        assert_eq!(processed.property_type.as_deref(), Some("String"));
        assert!(processed.type_set);
        assert!(processed.array);
        assert!(processed.optional);
    }

    #[test]
    fn process_object_property_type_is_the_referenced_name() {
        let processed = process::<ConcertoError>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty",
            "name": "address",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Address" }
        }))
        .expect("valid");
        assert_eq!(processed.property_type.as_deref(), Some("Address"));
    }

    #[test]
    fn process_enum_property_leaves_type_unset() {
        let processed = process::<ConcertoError>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.EnumProperty",
            "name": "RED"
        }))
        .expect("valid");
        assert!(!processed.type_set);
        assert_eq!(processed.property_type, None);
        assert!(!processed.array);
        assert!(!processed.optional);
    }

    #[test]
    fn process_rejects_an_invalid_identifier() {
        let err = process::<ConcertoError>(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "1bad",
            "isArray": false,
            "isOptional": false
        }))
        .unwrap_err();
        assert!(err.to_string().contains("Invalid property name '1bad'"));
    }

    fn prop(json: serde_json::Value) -> Property {
        Property::try_from(&json).expect("valid property")
    }

    #[test]
    fn parses_string_property_with_validators() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "email",
            "isArray": false,
            "isOptional": true,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
                "pattern": ".*@.*",
                "flags": ""
            }
        }));
        assert_eq!(p.name(), "email");
        assert!(p.is_optional());
        assert!(!p.is_array());
        assert!(p.is_primitive());
        assert_eq!(p.type_name(), Some("String"));
        match &p {
            Property::String(s) => assert!(s.validator.is_some()),
            _ => panic!("expected String"),
        }
    }

    #[test]
    fn parses_object_and_relationship_type_refs() {
        let o = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty",
            "name": "address",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Address" }
        }));
        assert!(!o.is_primitive());
        assert_eq!(o.type_name(), Some("Address"));
        assert_eq!(
            o.type_identifier().map(|t| t.name.as_str()),
            Some("Address")
        );

        let r = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
            "name": "owner",
            "isArray": true,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" }
        }));
        assert!(r.is_relationship());
        assert!(r.is_array());
        assert_eq!(r.type_name(), Some("Person"));
    }

    #[test]
    fn enum_member_has_no_type() {
        let e = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.EnumProperty",
            "name": "RED"
        }));
        assert!(e.is_enum_value());
        assert_eq!(e.type_name(), None);
        assert!(!e.is_array());
        assert!(!e.is_optional());
    }

    #[test]
    fn unknown_property_kind_errors() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryProperty",
            "name": "x"
        }));
        assert!(err.is_err());
    }

    #[test]
    fn missing_class_is_rejected() {
        let err = Property::try_from(&serde_json::json!({ "name": "x" }));
        assert!(err.unwrap_err().to_string().contains("$class"));
    }

    /// A `Double` property carrying the given range validator.
    fn ranged(lower: Option<f64>, upper: Option<f64>) -> serde_json::Value {
        let mut validator = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.DoubleDomainValidator"
        });
        if let Some(lower) = lower {
            validator["lower"] = lower.into();
        }
        if let Some(upper) = upper {
            validator["upper"] = upper.into();
        }
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.DoubleProperty",
            "name": "value", "isArray": false, "isOptional": false,
            "validator": validator
        })
    }

    /// A `String` property carrying the given length validator.
    fn sized(min: Option<i32>, max: Option<i32>) -> serde_json::Value {
        let mut validator = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringLengthValidator"
        });
        if let Some(min) = min {
            validator["minLength"] = min.into();
        }
        if let Some(max) = max {
            validator["maxLength"] = max.into();
        }
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "text", "isArray": false, "isOptional": false,
            "lengthValidator": validator
        })
    }

    #[test]
    fn a_property_name_must_be_an_identifier() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "1bad", "isArray": false, "isOptional": false
        }));
        assert!(err.unwrap_err().to_string().contains("invalid identifier"));
    }

    /// A `String` property carrying the given regex validator.
    fn matching(pattern: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "text", "isArray": false, "isOptional": false,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
                "pattern": pattern, "flags": ""
            }
        })
    }

    #[test]
    fn a_regex_validator_must_compile() {
        assert!(Property::try_from(&matching(r"^.+@.+\..+$")).is_ok());
        for pattern in ["*invalid", "[unclosed", "(unclosed"] {
            let err = Property::try_from(&matching(pattern));
            assert!(
                err.unwrap_err().to_string().contains("regular expression"),
                "{pattern} should be rejected"
            );
        }
    }

    #[test]
    fn range_lower_above_upper_is_rejected() {
        let err = Property::try_from(&ranged(Some(10.0), Some(5.0)));
        assert!(err.unwrap_err().to_string().contains("Lower bound"));
    }

    #[test]
    fn range_with_one_open_end_is_accepted() {
        assert!(Property::try_from(&ranged(Some(1.0), None)).is_ok());
        assert!(Property::try_from(&ranged(None, Some(1.0))).is_ok());
        assert!(Property::try_from(&ranged(Some(1.0), Some(10.0))).is_ok());
    }

    #[test]
    fn range_without_either_bound_is_rejected() {
        let err = Property::try_from(&ranged(None, None));
        assert!(err.unwrap_err().to_string().contains("Invalid range"));
    }

    /// OD-3: an Integer domain bound that overflows `i32` loads and
    /// validates, matching TS (which reads it as a plain JS number).
    ///
    /// Checked against the frozen TS 5.0.0 reference (`migration/oracle/reference`
    /// in the `/home/user/concerto` workspace): `ModelManager.fromAst` loading
    /// the same `IntegerDomainValidator` AST returns a `NumberValidator` whose
    /// `upperBound` is `2147483648`, matching `upper` here.
    #[test]
    fn integer_domain_bound_above_i32_max_loads() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.IntegerProperty",
            "name": "value", "isArray": false, "isOptional": false,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
                "lower": 0,
                "upper": (i32::MAX as i64) + 1
            }
        }));
        match &p {
            Property::Integer(i) => {
                assert_eq!(
                    i.validator.as_ref().unwrap().upper,
                    Some((i32::MAX as f64) + 1.0)
                );
            }
            _ => panic!("expected Integer"),
        }
    }

    /// OD-3: a Long domain bound above `i64::MAX` loads, as JS rounds it to
    /// the nearest f64 and TS accepts it.
    ///
    /// Checked against the frozen TS 5.0.0 reference (`migration/oracle/reference`
    /// in the `/home/user/concerto` workspace): `ModelManager.fromAst` loading
    /// the same `LongDomainValidator` AST returns a `NumberValidator` whose
    /// `upperBound` is `10000000000000000000` (`1e19`), matching `upper` here.
    #[test]
    fn long_domain_bound_above_i64_max_loads() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.LongProperty",
            "name": "value", "isArray": false, "isOptional": false,
            "validator": {
                "$class": "concerto.metamodel@1.0.0.LongDomainValidator",
                "lower": 0,
                "upper": 1e19
            }
        }));
        match &p {
            Property::Long(l) => {
                assert_eq!(l.validator.as_ref().unwrap().upper, Some(1e19));
            }
            _ => panic!("expected Long"),
        }
    }

    #[test]
    fn negative_string_length_is_rejected() {
        let err = Property::try_from(&sized(Some(-1), Some(5)));
        assert!(err.unwrap_err().to_string().contains("positive integers"));
    }

    #[test]
    fn string_length_min_above_max_is_rejected() {
        let err = Property::try_from(&sized(Some(10), Some(5)));
        assert!(err.unwrap_err().to_string().contains("minLength"));
    }

    #[test]
    fn string_length_within_bounds_is_accepted() {
        assert!(Property::try_from(&sized(Some(1), Some(5))).is_ok());
        assert!(Property::try_from(&sized(None, Some(5))).is_ok());
    }

    /// A `String[]` property with a collection size validator.
    fn collection_sized(is_array: bool, min: Option<i32>, max: Option<i32>) -> serde_json::Value {
        let mut validator = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator"
        });
        if let Some(min) = min {
            validator["minSize"] = min.into();
        }
        if let Some(max) = max {
            validator["maxSize"] = max.into();
        }
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "tags", "isArray": is_array, "isOptional": false,
            "sizeValidator": validator
        })
    }

    #[test]
    fn size_validator_on_array_is_accepted() {
        assert!(Property::try_from(&collection_sized(true, Some(1), Some(10))).is_ok());
        assert!(Property::try_from(&collection_sized(true, Some(2), None)).is_ok());
        assert!(Property::try_from(&collection_sized(true, None, Some(5))).is_ok());
    }

    #[test]
    fn size_validator_on_non_array_is_rejected() {
        let err = Property::try_from(&collection_sized(false, Some(1), Some(5)));
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("size validator can only be applied to array or map")
        );
    }

    #[test]
    fn size_validator_min_above_max_is_rejected() {
        let err = Property::try_from(&collection_sized(true, Some(10), Some(2)));
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("minSize must be less than or equal to maxSize")
        );
    }

    #[test]
    fn size_validator_negative_bounds_rejected() {
        let err = Property::try_from(&collection_sized(true, Some(-1), Some(5)));
        assert!(err.unwrap_err().to_string().contains("positive integers"));
    }

    #[test]
    fn size_validator_on_object_property_without_array_is_allowed() {
        let json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty",
            "name": "contacts",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "PhoneBook" },
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": 1,
                "maxSize": 5
            }
        });
        assert!(Property::try_from(&json).is_ok());
    }

    #[test]
    fn size_validator_on_relationship_array_is_accepted() {
        let json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
            "name": "advisors",
            "isArray": true,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": 1,
                "maxSize": 3
            }
        });
        let p = Property::try_from(&json).unwrap();
        assert!(p.size_validator().is_some());
        assert_eq!(p.size_validator().unwrap().min_size, Some(1.0));
        assert_eq!(p.size_validator().unwrap().max_size, Some(3.0));
    }

    #[test]
    fn size_validator_on_non_array_relationship_is_rejected() {
        let json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
            "name": "owner",
            "isArray": false,
            "isOptional": false,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": 1
            }
        });
        let err = Property::try_from(&json);
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("size validator can only be applied to array or map")
        );
    }

    #[test]
    fn unknown_property_kind_is_reported_by_name() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryProperty",
            "name": "x"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: unknown property type: MysteryProperty"
        );
    }

    #[test]
    fn missing_class_is_reported_verbatim() {
        let err = Property::try_from(&serde_json::json!({ "name": "x" }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: property node is missing its $class"
        );
    }

    #[test]
    fn a_property_class_may_be_given_as_the_short_name() {
        let p = prop(serde_json::json!({
            "$class": "StringProperty",
            "name": "email",
            "isArray": false,
            "isOptional": false
        }));
        assert_eq!(p.name(), "email");
        assert!(p.is_primitive());
    }

    #[test]
    fn a_reserved_name_is_rejected_before_the_kind_is_checked() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryProperty",
            "name": "$identifier"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: Invalid field name '$identifier'"
        );
    }

    /// P2-04 (plan §1.2's "enum ... reserved values" gap; issue #48): an
    /// enum value may not take a reserved (system) property name either,
    /// the same check every other property kind gets above.
    ///
    /// Checked against the frozen TS 5.0.0 reference
    /// (`migration/oracle/reference`): `ModelManager.addCTOModel` on
    ///
    /// ```cto
    /// namespace org.acme.enumreserved@1.0.0
    /// enum Status {
    ///   o $identifier
    /// }
    /// ```
    ///
    /// raises `IllegalModelException: Invalid field name '$identifier'`,
    /// matching this test verbatim.
    #[test]
    fn a_reserved_name_is_rejected_on_an_enum_value() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.EnumProperty",
            "name": "$identifier"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: Invalid field name '$identifier'"
        );
    }

    #[test]
    fn a_malformed_property_is_reported_under_its_own_kind() {
        let err = Property::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "s",
            "isArray": "yes"
        }));
        assert!(
            err.unwrap_err()
                .to_string()
                .starts_with("illegal model: invalid StringProperty: ")
        );
    }

    /// Ported from `test/introspect/property.js` #getSizeValidator "should
    /// reject size on a non-array Integer property" — the same
    /// `check_size_validator` path `size_validator_on_non_array_is_rejected`
    /// exercises for a `String` property, checked here for `Integer` too.
    #[test]
    fn size_validator_on_non_array_integer_property_is_rejected() {
        let json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.IntegerProperty",
            "name": "count", "isArray": false, "isOptional": false,
            "sizeValidator": {
                "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                "minSize": 1, "maxSize": 5
            }
        });
        let err = Property::try_from(&json);
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("size validator can only be applied to array or map")
        );
    }

    /// Ported from `test/introspect/property.js` #getSizeValidator "should
    /// return null when no size validator".
    #[test]
    fn size_validator_is_none_when_absent() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "tags", "isArray": true, "isOptional": false
        }));
        assert!(p.size_validator().is_none());
    }

    /// Ported from `test/introspect/field.js` #constructor "should not have a
    /// default value by default" and "should save the incoming default
    /// value". TS builds a `Field` over a stubbed `ClassDeclaration` parent
    /// for these two, but `process()` never calls it (`this.ast.defaultValue`
    /// only), so the stub is inert scaffolding, not white-box coupling
    /// (module doc on [`crate::model_manager::ModelManager::property_default_value`],
    /// which is the same raw-AST read for a `PropId` already in the arena);
    /// `Property::try_from` alone is the faithful port here, no `ModelManager`
    /// or parent needed.
    #[test]
    fn a_default_value_is_read_from_the_ast_when_present() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "field", "isArray": false, "isOptional": false,
            "defaultValue": "wowSuchDefault"
        }));
        match &p {
            Property::String(s) => {
                assert_eq!(s.default_value.as_deref(), Some("wowSuchDefault"));
            }
            _ => panic!("expected String"),
        }

        let without = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "field", "isArray": false, "isOptional": false
        }));
        match &without {
            Property::String(s) => assert_eq!(s.default_value, None),
            _ => panic!("expected String"),
        }
    }

    /// Ported from `test/introspect/field.js` #getDefaultValue "should return
    /// the default value for falsy defaults": a JSON `false` default is not
    /// itself nullish, so it is kept (`Util.isNull` in TS, `!v.is_null()` in
    /// [`crate::model_manager::ModelManager::property_default_value`]),
    /// unlike a JSON `null`.
    #[test]
    fn a_falsy_boolean_default_value_is_not_treated_as_absent() {
        let p = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.BooleanProperty",
            "name": "field", "isArray": false, "isOptional": false,
            "defaultValue": false
        }));
        match &p {
            Property::Boolean(b) => assert_eq!(b.default_value, Some(false)),
            _ => panic!("expected Boolean"),
        }
    }

    /// Ported from `test/introspect/field.js` #constructor "should not be
    /// optional by default" and "should detect if field is optional".
    #[test]
    fn optional_defaults_to_false_and_follows_the_ast() {
        let not_optional = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "field", "isArray": false
        }));
        assert!(!not_optional.is_optional());

        let optional = prop(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "field", "isArray": false, "isOptional": true
        }));
        assert!(optional.is_optional());
    }
}

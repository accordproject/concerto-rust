//! Properties, with their types kept intact.
//!
//! [`Property`] is a sum type whose variants are newtypes over the generated
//! metamodel structs: the eight field kinds of [`mm::Property`] and
//! [`mm::EnumProperty`]. Each node is deserialized into the concrete struct its
//! `$class` names, whether the `$class` is fully qualified or given as its bare
//! short name, so the validators and the referenced `type` are kept whole. A
//! class declaration keeps its properties as this type too, because it also
//! accepts an `EnumProperty`, which the generated `mm::Property` union does
//! not cover. The getters hang off the enum directly. No trait hierarchy to
//! chase.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, Result};
use crate::introspect::{check_domain, check_length, check_pattern, check_size, declared_class};
use crate::model_util::{is_system_property, is_valid_identifier, short_name};

/// A single property of a concept-like or enum declaration.
#[derive(Debug, Clone)]
pub enum Property {
    /// A `Boolean` primitive field.
    Boolean(mm::BooleanProperty),
    /// A `String` primitive field (may carry regex/length validators).
    String(mm::StringProperty),
    /// An `Integer` primitive field (may carry a domain validator).
    Integer(mm::IntegerProperty),
    /// A `Long` primitive field (may carry a domain validator).
    Long(mm::LongProperty),
    /// A `Double` primitive field (may carry a domain validator).
    Double(mm::DoubleProperty),
    /// A `DateTime` primitive field.
    DateTime(mm::DateTimeProperty),
    /// A field whose type is another declared concept/scalar.
    Object(mm::ObjectProperty),
    /// A relationship reference to an identifiable declaration.
    Relationship(mm::RelationshipProperty),
    /// A value member of an enum declaration.
    Enum(mm::EnumProperty),
}

impl Property {
    /// The property's name.
    pub fn name(&self) -> &str {
        match self {
            Self::Boolean(p) => &p.name,
            Self::String(p) => &p.name,
            Self::Integer(p) => &p.name,
            Self::Long(p) => &p.name,
            Self::Double(p) => &p.name,
            Self::DateTime(p) => &p.name,
            Self::Object(p) => &p.name,
            Self::Relationship(p) => &p.name,
            Self::Enum(p) => &p.name,
        }
    }

    /// Whether the property is an array (`[]`). Enum members are never arrays.
    pub fn is_array(&self) -> bool {
        match self {
            Self::Boolean(p) => p.is_array,
            Self::String(p) => p.is_array,
            Self::Integer(p) => p.is_array,
            Self::Long(p) => p.is_array,
            Self::Double(p) => p.is_array,
            Self::DateTime(p) => p.is_array,
            Self::Object(p) => p.is_array,
            Self::Relationship(p) => p.is_array,
            Self::Enum(_) => false,
        }
    }

    /// Whether the property is optional. Enum members are never optional.
    pub fn is_optional(&self) -> bool {
        match self {
            Self::Boolean(p) => p.is_optional,
            Self::String(p) => p.is_optional,
            Self::Integer(p) => p.is_optional,
            Self::Long(p) => p.is_optional,
            Self::Double(p) => p.is_optional,
            Self::DateTime(p) => p.is_optional,
            Self::Object(p) => p.is_optional,
            Self::Relationship(p) => p.is_optional,
            Self::Enum(_) => false,
        }
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

    /// The name of the property's type. For primitives that's the primitive
    /// itself; for object/relationship properties it's the type they point at.
    /// Enum members don't have a type, so they get `None`.
    pub fn type_name(&self) -> Option<&str> {
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

    /// The collection size validator, if one is declared on this property.
    pub fn size_validator(&self) -> Option<&mm::CollectionSizeValidator> {
        match self {
            Self::Boolean(p) => p.size_validator.as_ref(),
            Self::String(p) => p.size_validator.as_ref(),
            Self::Integer(p) => p.size_validator.as_ref(),
            Self::Long(p) => p.size_validator.as_ref(),
            Self::Double(p) => p.size_validator.as_ref(),
            Self::DateTime(p) => p.size_validator.as_ref(),
            Self::Object(p) => p.size_validator.as_ref(),
            Self::Relationship(p) => p.size_validator.as_ref(),
            Self::Enum(_) => None,
        }
    }

    /// The decorators attached to this property.
    pub fn decorators(&self) -> &[mm::Decorator] {
        match self {
            Self::Boolean(p) => p.decorators.as_deref().unwrap_or(&[]),
            Self::String(p) => p.decorators.as_deref().unwrap_or(&[]),
            Self::Integer(p) => p.decorators.as_deref().unwrap_or(&[]),
            Self::Long(p) => p.decorators.as_deref().unwrap_or(&[]),
            Self::Double(p) => p.decorators.as_deref().unwrap_or(&[]),
            Self::DateTime(p) => p.decorators.as_deref().unwrap_or(&[]),
            Self::Object(p) => p.decorators.as_deref().unwrap_or(&[]),
            Self::Relationship(p) => p.decorators.as_deref().unwrap_or(&[]),
            Self::Enum(p) => p.decorators.as_deref().unwrap_or(&[]),
        }
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
        let kind = short_name(class);

        // Parse into whatever struct the `$class` says this is. If serde
        // chokes, the JSON is malformed for the kind it claims to be.
        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid {kind}: {e}"),
            file_name: None,
            location: None,
        };

        let property = match kind {
            "BooleanProperty" => Self::Boolean(serde_json::from_value(value.clone()).map_err(bad)?),
            "StringProperty" => Self::String(serde_json::from_value(value.clone()).map_err(bad)?),
            "IntegerProperty" => Self::Integer(serde_json::from_value(value.clone()).map_err(bad)?),
            "LongProperty" => Self::Long(serde_json::from_value(value.clone()).map_err(bad)?),
            "DoubleProperty" => Self::Double(serde_json::from_value(value.clone()).map_err(bad)?),
            "DateTimeProperty" => {
                Self::DateTime(serde_json::from_value(value.clone()).map_err(bad)?)
            }
            "ObjectProperty" => Self::Object(serde_json::from_value(value.clone()).map_err(bad)?),
            "RelationshipProperty" => {
                Self::Relationship(serde_json::from_value(value.clone()).map_err(bad)?)
            }
            "EnumProperty" => Self::Enum(serde_json::from_value(value.clone()).map_err(bad)?),
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
    /// Checks the validators this property carries: a numeric range, a string
    /// length, a regular expression, and a collection size. These are part of
    /// the property's own declaration, so they are checked while loading
    /// rather than left to the validation pass.
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
        assert_eq!(p.size_validator().unwrap().min_size, Some(1));
        assert_eq!(p.size_validator().unwrap().max_size, Some(3));
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
}

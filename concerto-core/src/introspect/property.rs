//! Properties, with their types kept intact.
//!
//! Deserializing a property through the generated metamodel loses the parts we
//! care about: the validators, and the referenced `type` on object and
//! relationship properties all get dropped into a bare [`mm::Property`]. So we
//! read each property a second time, straight from its raw JSON, into this
//! [`Property`] enum and let the `$class` decide the variant. The getters hang
//! off the enum directly. No trait hierarchy to chase.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, Result};
use crate::introspect::declared_class;
use crate::model_util::short_name;

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
        let kind = short_name(class);

        // Parse into whatever struct the `$class` says this is. If serde
        // chokes, the JSON is malformed for the kind it claims to be.
        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid {kind}: {e}"),
            file_name: None,
            location: None,
        };

        Ok(match kind {
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
        })
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
}

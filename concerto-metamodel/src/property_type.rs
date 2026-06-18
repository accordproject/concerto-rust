//! Property type validation and discrimination.
//!
//! This module provides utilities to identify and validate different property types
//! from the Concerto metamodel.

/// Property type discriminator for routing to specific validators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropertyType {
    /// String primitive property type
    String,
    /// Integer primitive property type
    Integer,
    /// Double primitive property type
    Double,
    /// Long primitive property type
    Long,
    /// Boolean primitive property type
    Boolean,
    /// DateTime primitive property type
    DateTime,
    /// Object property type (references another type)
    Object,
    /// Relationship property type (references an identifiable type)
    Relationship,
    /// Enum property type
    Enum,
}

impl PropertyType {
    /// Identify property type from the $class discriminator string.
    ///
    /// # Arguments
    /// * `discriminator` - The `$class` field value from a property
    ///
    /// # Returns
    /// `Some(PropertyType)` if the discriminator is recognized, `None` otherwise
    pub fn from_discriminator(discriminator: &str) -> Option<Self> {
        match discriminator {
            "concerto.metamodel@1.0.0.StringProperty" => Some(PropertyType::String),
            "concerto.metamodel@1.0.0.IntegerProperty" => Some(PropertyType::Integer),
            "concerto.metamodel@1.0.0.DoubleProperty" => Some(PropertyType::Double),
            "concerto.metamodel@1.0.0.LongProperty" => Some(PropertyType::Long),
            "concerto.metamodel@1.0.0.BooleanProperty" => Some(PropertyType::Boolean),
            "concerto.metamodel@1.0.0.DateTimeProperty" => Some(PropertyType::DateTime),
            "concerto.metamodel@1.0.0.ObjectProperty" => Some(PropertyType::Object),
            "concerto.metamodel@1.0.0.RelationshipProperty" => Some(PropertyType::Relationship),
            "concerto.metamodel@1.0.0.EnumProperty" => Some(PropertyType::Enum),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_property_discriminator() {
        let discriminator = "concerto.metamodel@1.0.0.StringProperty";
        assert_eq!(
            PropertyType::from_discriminator(discriminator),
            Some(PropertyType::String)
        );
    }

    #[test]
    fn test_integer_property_discriminator() {
        let discriminator = "concerto.metamodel@1.0.0.IntegerProperty";
        assert_eq!(
            PropertyType::from_discriminator(discriminator),
            Some(PropertyType::Integer)
        );
    }

    #[test]
    fn test_unknown_discriminator() {
        let discriminator = "unknown.property.type";
        assert_eq!(PropertyType::from_discriminator(discriminator), None);
    }

    #[test]
    fn test_all_property_types() {
        let types = vec![
            ("concerto.metamodel@1.0.0.StringProperty", PropertyType::String),
            ("concerto.metamodel@1.0.0.IntegerProperty", PropertyType::Integer),
            ("concerto.metamodel@1.0.0.DoubleProperty", PropertyType::Double),
            ("concerto.metamodel@1.0.0.LongProperty", PropertyType::Long),
            ("concerto.metamodel@1.0.0.BooleanProperty", PropertyType::Boolean),
            ("concerto.metamodel@1.0.0.DateTimeProperty", PropertyType::DateTime),
            ("concerto.metamodel@1.0.0.ObjectProperty", PropertyType::Object),
            ("concerto.metamodel@1.0.0.RelationshipProperty", PropertyType::Relationship),
            ("concerto.metamodel@1.0.0.EnumProperty", PropertyType::Enum),
        ];

        for (discriminator, expected) in types {
            assert_eq!(
                PropertyType::from_discriminator(discriminator),
                Some(expected),
                "Failed for discriminator: {}",
                discriminator
            );
        }
    }
}

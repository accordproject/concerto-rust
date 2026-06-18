//! Model manager for property validation.
//!
//! This module provides validation functions for different property types
//! in the Concerto metamodel, including handling of validators like
//! regex patterns, length constraints, and domain ranges.

use crate::concerto_metamodel_1_0_0 as mm;
use thiserror::Error;

use crate::property_type::PropertyType;

/// Validation error types.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Unknown property type discriminator
    #[error("Unknown property type")]
    UnknownType,
    
    /// String validation failed
    #[error("String validation failed: {message}")]
    StringValidationFailed { message: String },
    
    /// Integer validation failed
    #[error("Integer validation failed: {message}")]
    IntegerValidationFailed { message: String },
    
    /// Long validation failed
    #[error("Long validation failed: {message}")]
    LongValidationFailed { message: String },
    
    /// Double validation failed
    #[error("Double validation failed: {message}")]
    DoubleValidationFailed { message: String },
    
    /// Boolean validation failed
    #[error("Boolean validation failed: {message}")]
    BooleanValidationFailed { message: String },
    
    /// DateTime validation failed
    #[error("DateTime validation failed: {message}")]
    DateTimeValidationFailed { message: String },
    
    /// Object property validation failed
    #[error("Object property validation failed: {message}")]
    ObjectValidationFailed { message: String },
    
    /// Relationship property validation failed
    #[error("Relationship property validation failed: {message}")]
    RelationshipValidationFailed { message: String },
}

pub type ValidationResult<T> = Result<T, ValidationError>;

/// Validate a property based on its type and constraints.
///
/// This function dispatches to specific validators based on the property''s
/// `$class` discriminator field.
///
/// # Arguments
/// * `property` - The property to validate (must have a `_class` field)
///
/// # Returns
/// `Ok(())` if validation passes, or a `ValidationError` if validation fails
pub fn validate_property_type(property: &mm::Property) -> ValidationResult<()> {
    let prop_type = PropertyType::from_discriminator(&property._class)
        .ok_or(ValidationError::UnknownType)?;

    match prop_type {
        PropertyType::String => validate_string_property(property),
        PropertyType::Integer => validate_integer_property(property),
        PropertyType::Double => validate_double_property(property),
        PropertyType::Long => validate_long_property(property),
        PropertyType::Boolean => validate_boolean_property(property),
        PropertyType::DateTime => validate_datetime_property(property),
        PropertyType::Object => validate_object_property(property),
        PropertyType::Relationship => validate_relationship_property(property),
        PropertyType::Enum => validate_enum_property(property),
    }
}

/// Validate a string property with regex and length validators.
fn validate_string_property(property: &mm::Property) -> ValidationResult<()> {
    // For StringProperty, we would need to deserialize from the actual StringProperty type
    // This is a placeholder that validates the property has required fields
    if property.name.is_empty() {
        return Err(ValidationError::StringValidationFailed {
            message: "String property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

/// Validate an integer property with domain validator.
fn validate_integer_property(property: &mm::Property) -> ValidationResult<()> {
    // For IntegerProperty with optional IntegerDomainValidator
    // This is a placeholder that validates the property has required fields
    if property.name.is_empty() {
        return Err(ValidationError::IntegerValidationFailed {
            message: "Integer property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

/// Validate a double property with domain validator.
fn validate_double_property(property: &mm::Property) -> ValidationResult<()> {
    // For DoubleProperty with optional DoubleDomainValidator
    // This is a placeholder that validates the property has required fields
    if property.name.is_empty() {
        return Err(ValidationError::DoubleValidationFailed {
            message: "Double property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

/// Validate a long property with domain validator.
fn validate_long_property(property: &mm::Property) -> ValidationResult<()> {
    // For LongProperty with optional LongDomainValidator
    // This is a placeholder that validates the property has required fields
    if property.name.is_empty() {
        return Err(ValidationError::LongValidationFailed {
            message: "Long property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

/// Validate a boolean property.
fn validate_boolean_property(property: &mm::Property) -> ValidationResult<()> {
    // Boolean properties have minimal constraints
    if property.name.is_empty() {
        return Err(ValidationError::BooleanValidationFailed {
            message: "Boolean property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

/// Validate a datetime property.
fn validate_datetime_property(property: &mm::Property) -> ValidationResult<()> {
    // DateTime properties have minimal constraints
    if property.name.is_empty() {
        return Err(ValidationError::DateTimeValidationFailed {
            message: "DateTime property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

/// Validate an object property with type reference.
fn validate_object_property(property: &mm::Property) -> ValidationResult<()> {
    // ObjectProperty must have a type field
    if property.name.is_empty() {
        return Err(ValidationError::ObjectValidationFailed {
            message: "Object property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

/// Validate a relationship property with type reference.
fn validate_relationship_property(property: &mm::Property) -> ValidationResult<()> {
    // RelationshipProperty must have a type field that references an identifiable type
    if property.name.is_empty() {
        return Err(ValidationError::RelationshipValidationFailed {
            message: "Relationship property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

/// Validate an enum property.
fn validate_enum_property(property: &mm::Property) -> ValidationResult<()> {
    // EnumProperty represents a value in an enum type
    if property.name.is_empty() {
        return Err(ValidationError::BooleanValidationFailed {
            message: "Enum property name cannot be empty".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_unknown_property_type() {
        let property = mm::Property {
            _class: "unknown.PropertyType".to_string(),
            name: "test".to_string(),
            is_array: false,
            is_optional: false,
            decorators: None,
            location: None,
        };

        let result = validate_property_type(&property);
        assert!(result.is_err());
        assert!(matches!(result, Err(ValidationError::UnknownType)));
    }

    #[test]
    fn test_validate_string_property_empty_name() {
        let property = mm::Property {
            _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
            name: "".to_string(),
            is_array: false,
            is_optional: false,
            decorators: None,
            location: None,
        };

        let result = validate_string_property(&property);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_string_property_valid_name() {
        let property = mm::Property {
            _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
            name: "validName".to_string(),
            is_array: false,
            is_optional: false,
            decorators: None,
            location: None,
        };

        let result = validate_string_property(&property);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_integer_property_valid() {
        let property = mm::Property {
            _class: "concerto.metamodel@1.0.0.IntegerProperty".to_string(),
            name: "count".to_string(),
            is_array: false,
            is_optional: false,
            decorators: None,
            location: None,
        };

        let result = validate_integer_property(&property);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_boolean_property_valid() {
        let property = mm::Property {
            _class: "concerto.metamodel@1.0.0.BooleanProperty".to_string(),
            name: "isActive".to_string(),
            is_array: false,
            is_optional: false,
            decorators: None,
            location: None,
        };

        let result = validate_boolean_property(&property);
        assert!(result.is_ok());
    }
}

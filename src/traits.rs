use crate::{ConcertoError, ModelManager, metamodel_validation::is_valid_identifier};
use crate::metamodel::concerto_metamodel_1_0_0::*;
use crate::validation::Validate;

/// Base trait for declaration types
pub trait DeclarationBase {
    /// Get the name of the declaration
    fn get_name(&self) -> &str;

    /// Get the decorators if any
    fn get_decorators(&self) -> Option<&Vec<Decorator>>;

    /// Get the location if any
    fn get_location(&self) -> Option<&Range>;
}

// Implement for all declaration types
impl DeclarationBase for Declaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }

}

impl DeclarationBase for AssetDeclaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl DeclarationBase for ConceptDeclaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl DeclarationBase for EnumDeclaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl DeclarationBase for ParticipantDeclaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl DeclarationBase for TransactionDeclaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl DeclarationBase for EventDeclaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl DeclarationBase for MapDeclaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

/// Trait for concept-based declarations (Asset, Participant, Transaction, Event all inherit from Concept)
pub trait ConceptDeclarationBase: DeclarationBase {
    /// Get whether the concept is abstract
    fn is_abstract(&self) -> bool;

    /// Get the super type if any
    fn get_super_type(&self) -> Option<&TypeIdentifier>;

    /// Get the properties
    fn get_properties(&self) -> &Vec<Property>;

    /// Get the identified information if any
    fn get_identified(&self) -> Option<&Identified>;
}

impl ConceptDeclarationBase for ConceptDeclaration {
    fn is_abstract(&self) -> bool {
        self.is_abstract
    }

    fn get_super_type(&self) -> Option<&TypeIdentifier> {
        self.super_type.as_ref()
    }

    fn get_properties(&self) -> &Vec<Property> {
        &self.properties
    }

    fn get_identified(&self) -> Option<&Identified> {
        self.identified.as_ref()
    }
}

impl ConceptDeclarationBase for AssetDeclaration {
    fn is_abstract(&self) -> bool {
        self.is_abstract
    }

    fn get_super_type(&self) -> Option<&TypeIdentifier> {
        self.super_type.as_ref()
    }

    fn get_properties(&self) -> &Vec<Property> {
        &self.properties
    }

    fn get_identified(&self) -> Option<&Identified> {
        self.identified.as_ref()
    }
}

impl ConceptDeclarationBase for ParticipantDeclaration {
    fn is_abstract(&self) -> bool {
        self.is_abstract
    }

    fn get_super_type(&self) -> Option<&TypeIdentifier> {
        self.super_type.as_ref()
    }

    fn get_properties(&self) -> &Vec<Property> {
        &self.properties
    }

    fn get_identified(&self) -> Option<&Identified> {
        self.identified.as_ref()
    }
}

impl ConceptDeclarationBase for TransactionDeclaration {
    fn is_abstract(&self) -> bool {
        self.is_abstract
    }

    fn get_super_type(&self) -> Option<&TypeIdentifier> {
        self.super_type.as_ref()
    }

    fn get_properties(&self) -> &Vec<Property> {
        &self.properties
    }

    fn get_identified(&self) -> Option<&Identified> {
        self.identified.as_ref()
    }
}

impl ConceptDeclarationBase for EventDeclaration {
    fn is_abstract(&self) -> bool {
        self.is_abstract
    }

    fn get_super_type(&self) -> Option<&TypeIdentifier> {
        self.super_type.as_ref()
    }

    fn get_properties(&self) -> &Vec<Property> {
        &self.properties
    }

    fn get_identified(&self) -> Option<&Identified> {
        self.identified.as_ref()
    }
}

/// Base trait for property types
pub trait PropertyBase {
    /// Get the name of the property
    fn get_name(&self) -> &str;

    /// Check if property is an array
    fn is_array(&self) -> bool;

    /// Check if property is optional
    fn is_optional(&self) -> bool;

    /// Get the decorators if any
    fn get_decorators(&self) -> Option<&Vec<Decorator>>;

    /// Get the location if any
    fn get_location(&self) -> Option<&Range>;
}

// Implement for all property types
impl PropertyBase for RelationshipProperty {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_array(&self) -> bool {
        self.is_array
    }

    fn is_optional(&self) -> bool {
        self.is_optional
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl PropertyBase for ObjectProperty {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_array(&self) -> bool {
        self.is_array
    }

    fn is_optional(&self) -> bool {
        self.is_optional
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl PropertyBase for BooleanProperty {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_array(&self) -> bool {
        self.is_array
    }

    fn is_optional(&self) -> bool {
        self.is_optional
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl PropertyBase for DateTimeProperty {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_array(&self) -> bool {
        self.is_array
    }

    fn is_optional(&self) -> bool {
        self.is_optional
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl PropertyBase for StringProperty {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_array(&self) -> bool {
        self.is_array
    }

    fn is_optional(&self) -> bool {
        self.is_optional
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl PropertyBase for DoubleProperty {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_array(&self) -> bool {
        self.is_array
    }

    fn is_optional(&self) -> bool {
        self.is_optional
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl PropertyBase for IntegerProperty {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_array(&self) -> bool {
        self.is_array
    }

    fn is_optional(&self) -> bool {
        self.is_optional
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl PropertyBase for LongProperty {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn is_array(&self) -> bool {
        self.is_array
    }

    fn is_optional(&self) -> bool {
        self.is_optional
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

/// Trait for property types that have a type reference
pub trait TypeReferenceProperty: PropertyBase {
    /// Get the type reference
    fn get_type_reference(&self) -> &TypeIdentifier;
}

impl TypeReferenceProperty for ObjectProperty {
    fn get_type_reference(&self) -> &TypeIdentifier {
        &self.type_
    }
}

impl TypeReferenceProperty for RelationshipProperty {
    fn get_type_reference(&self) -> &TypeIdentifier {
        &self.type_
    }
}

/// Trait for property validation
pub trait PropertyValidator {
    /// Validate the property against a model manager
    fn validate(&self, model_manager: &ModelManager) -> Result<(), ConcertoError>;
}

impl PropertyValidator for RelationshipProperty {
    fn validate(&self, model_manager: &ModelManager) -> Result<(), ConcertoError> {
         model_manager.validate_type_exists(&self.type_)
    }
}

impl PropertyValidator for ObjectProperty {
    fn validate(&self, model_manager: &ModelManager) -> Result<(), ConcertoError> {
        model_manager.validate_type_exists(&self.type_)
    }
}

// Implementation for base Property
impl PropertyValidator for Property {
    fn validate(&self, _model_manager: &ModelManager) -> Result<(), ConcertoError> {
        // Validate property name
        if !is_valid_identifier(&self.name) {
            return Err(ConcertoError::ValidationError(format!(
                "'{}' is not a valid property name. Identifiers must start with a letter and can contain only letters, numbers, or underscores",
                self.name
            )));
        }

        // Validate decorators if present
        if let Some(decs) = &self.decorators {
            for decorator in decs {
                decorator.validate()?;
            }
        }

        Ok(())
    }
}

// Default implementation for primitive types that don't need validation
macro_rules! impl_property_validator_noop {
    ($t:ty) => {
        impl PropertyValidator for $t {
            fn validate(&self, _model_manager: &ModelManager) -> Result<(), ConcertoError> {
                // No validation needed for primitive types
                Ok(())
            }
        }
    };
}

impl_property_validator_noop!(StringProperty);
impl_property_validator_noop!(BooleanProperty);
impl_property_validator_noop!(DateTimeProperty);
impl_property_validator_noop!(DoubleProperty);
impl_property_validator_noop!(IntegerProperty);
impl_property_validator_noop!(LongProperty);

/// Trait for declaration validation
pub trait DeclarationValidator {
    /// Validates the declaration structure and rules
    fn validate_declaration(&self) -> Result<(), ConcertoError>;

    /// Validates that a declaration has a valid name according to Concerto rules
    fn validate_name(&self, name: &str) -> Result<(), ConcertoError>;

    /// Validates decorators if present
    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError>;
}

/// Common validator implementation that can be reused across declaration types
pub struct CommonDeclarationValidator;

impl CommonDeclarationValidator {
    /// Helper function to validate a Concerto identifier
    pub fn validate_identifier(name: &str) -> Result<(), ConcertoError> {
        // This should use the same validation logic as in metamodel_validation.rs
        use crate::metamodel_validation::is_valid_identifier;

        if !is_valid_identifier(name) {
            return Err(ConcertoError::ValidationError(
                format!("'{}' is not a valid identifier name. Identifiers must start with a letter and can contain only letters, numbers, or underscores", name)
            ));
        }
        Ok(())
    }

    /// Helper function to validate decorators
    pub fn validate_decorators(decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        if let Some(decs) = decorators {
            for decorator in decs {
                // Use the Validate trait for each decorator
                use crate::validation::Validate;
                decorator.validate()?;
            }
        }
        Ok(())
    }

    /// Helper function to validate properties and check for duplicates
    pub fn validate_properties(properties: &[Property]) -> Result<(), ConcertoError> {
        use std::collections::HashSet;
        let mut property_names = HashSet::new();

        for property in properties {
            // Use the Validate trait for each property
            // This does the basic property validation without model context
            // For model context validation, we'd use PropertyValidator
            Validate::validate(property)?;

            // Check for duplicate property names
            if !property_names.insert(&property.name) {
                return Err(ConcertoError::ValidationError(
                    format!("Duplicate property: {}", property.name)
                ));
            }
        }

        Ok(())
    }
}

// Implement the DeclarationValidator for ConceptDeclaration and its child types
impl DeclarationValidator for ConceptDeclaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;

        // Validate properties if present
        CommonDeclarationValidator::validate_properties(&self.properties)?;

        // Validate decorators
        self.validate_decorators(&self.decorators)?;

        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

// We can implement for the other declaration types similarly
impl DeclarationValidator for AssetDeclaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        CommonDeclarationValidator::validate_properties(&self.properties)?;
        self.validate_decorators(&self.decorators)?;
        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

impl DeclarationValidator for ParticipantDeclaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        CommonDeclarationValidator::validate_properties(&self.properties)?;
        self.validate_decorators(&self.decorators)?;
        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

impl DeclarationValidator for TransactionDeclaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        CommonDeclarationValidator::validate_properties(&self.properties)?;
        self.validate_decorators(&self.decorators)?;
        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

impl DeclarationValidator for EventDeclaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        CommonDeclarationValidator::validate_properties(&self.properties)?;
        self.validate_decorators(&self.decorators)?;
        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

impl DeclarationValidator for EnumDeclaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;

        // Validate enum properties (values)
        use std::collections::HashSet;
        let mut enum_values = HashSet::new();
        for property in &self.properties {
            // Validate each enum property
            if !CommonDeclarationValidator::validate_identifier(&property.name).is_ok() {
                return Err(ConcertoError::ValidationError(
                    format!("'{}' is not a valid enum property name", property.name)
                ));
            }

            // Check for duplicate enum values
            if !enum_values.insert(&property.name) {
                return Err(ConcertoError::ValidationError(
                    format!("Duplicate enum value: {}", property.name)
                ));
            }

            // Validate decorators on enum properties
            CommonDeclarationValidator::validate_decorators(&property.decorators)?;
        }

        // Validate decorators
        self.validate_decorators(&self.decorators)?;

        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

impl DeclarationValidator for MapDeclaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;

        // Validate key type - must be StringMapKeyType or DateTimeMapKeyType
        // We need to check the _class field of the key type
        if !self.key._class.contains("StringMapKeyType") && !self.key._class.contains("DateTimeMapKeyType") {
            return Err(ConcertoError::ValidationError(
                "Invalid map key type. Map keys must be String or DateTime".to_string()
            ));
        }

        // Value type is always present by the struct definition

        // Validate decorators
        self.validate_decorators(&self.decorators)?;

        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

/// Trait for scalar declarations
pub trait ScalarDeclarationBase: DeclarationBase {
    /// Get the default value type if any
    fn get_default_value(&self) -> Option<&str>;
}

// Implementation for ScalarDeclaration
impl DeclarationBase for ScalarDeclaration {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

/// Trait for scalar types with validators
pub trait ValidatedScalar {
    /// Check if the scalar has a validator
    fn has_validator(&self) -> bool;
}

// Add implementation for ScalarDeclaration
impl DeclarationValidator for ScalarDeclaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        self.validate_decorators(&self.decorators)?;

        // Basic validation for scalar declaration
        // More specific validation would happen in specialized scalar implementations

        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

// Implementations for specialized scalar types
impl DeclarationBase for StringScalar {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl ScalarDeclarationBase for StringScalar {
    fn get_default_value(&self) -> Option<&str> {
        self.default_value.as_deref()
    }
}

impl ValidatedScalar for StringScalar {
    fn has_validator(&self) -> bool {
        self.validator.is_some()
    }
}

impl DeclarationBase for IntegerScalar {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl ValidatedScalar for IntegerScalar {
    fn has_validator(&self) -> bool {
        self.validator.is_some()
    }
}

impl DeclarationBase for DoubleScalar {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl ValidatedScalar for DoubleScalar {
    fn has_validator(&self) -> bool {
        self.validator.is_some()
    }
}

impl DeclarationBase for BooleanScalar {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

impl DeclarationBase for DateTimeScalar {
    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_decorators(&self) -> Option<&Vec<Decorator>> {
        self.decorators.as_ref()
    }

    fn get_location(&self) -> Option<&Range> {
        self.location.as_ref()
    }
}

// Implement DeclarationValidator for the specific scalar types
impl DeclarationValidator for StringScalar {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        self.validate_decorators(&self.decorators)?;

        // Validate string regex validator if present
        if let Some(validator) = &self.validator {
            // StringRegexValidator has a direct pattern field (not Option)
            // Try to compile the regex to validate it
            match regex::Regex::new(&validator.pattern) {
                Ok(_) => {}, // Regex is valid
                Err(_) => {
                    return Err(ConcertoError::ValidationError(
                        format!("Invalid regex pattern in string validator: {}", validator.pattern)
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

// Implement for numeric scalar types with validators
macro_rules! impl_numeric_scalar_validator {
    ($scalar_type:ty) => {
        impl DeclarationValidator for $scalar_type {
            fn validate_declaration(&self) -> Result<(), ConcertoError> {
                self.validate_name(&self.name)?;
                self.validate_decorators(&self.decorators)?;

                // Validate numeric domain validator if present
                if let Some(validator) = &self.validator {
                    // For numeric domain validators with lower/upper bounds
                    // Both lower and upper are actual values, not Option types
                    if validator.lower > validator.upper {
                        return Err(ConcertoError::ValidationError(
                            format!("Invalid range in validator: lower bound ({:?}) must be less than or equal to upper bound ({:?})",
                                validator.lower, validator.upper)
                        ));
                    }
                }

                Ok(())
            }

            fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
                CommonDeclarationValidator::validate_identifier(name)
            }

            fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
                CommonDeclarationValidator::validate_decorators(decorators)
            }
        }
    };
}

impl_numeric_scalar_validator!(IntegerScalar);
impl_numeric_scalar_validator!(DoubleScalar);
impl_numeric_scalar_validator!(LongScalar);

// Simple validation for other scalar types
impl DeclarationValidator for BooleanScalar {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        self.validate_decorators(&self.decorators)
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

impl DeclarationValidator for DateTimeScalar {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        self.validate_decorators(&self.decorators)
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        CommonDeclarationValidator::validate_decorators(decorators)
    }
}

// Add Validate implementations for all scalar types
impl Validate for StringScalar {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use the DeclarationValidator trait we implemented
        self.validate_declaration()
    }
}

impl Validate for IntegerScalar {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use the DeclarationValidator trait we implemented
        self.validate_declaration()
    }
}

impl Validate for DoubleScalar {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use the DeclarationValidator trait we implemented
        self.validate_declaration()
    }
}

impl Validate for LongScalar {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use the DeclarationValidator trait we implemented
        self.validate_declaration()
    }
}

impl Validate for BooleanScalar {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use the DeclarationValidator trait we implemented
        self.validate_declaration()
    }
}

impl Validate for DateTimeScalar {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use the DeclarationValidator trait we implemented
        self.validate_declaration()
    }
}

use crate::error::ConcertoError;
use crate::validation::Validate;
use crate::metamodel::concerto_metamodel_1_0_0::*;

use regex::Regex;

/// Checks if a string is a valid identifier name in Concerto
/// Identifiers must start with a letter and contain only letters, numbers, or underscores
pub fn is_valid_identifier(name: &str) -> bool {
    lazy_static::lazy_static! {
        // TODO use the full regex from the Concerto spec
        static ref IDENTIFIER_REGEX: Regex = Regex::new(r"^[a-zA-Z][a-zA-Z0-9_]*$").unwrap();
    }

    IDENTIFIER_REGEX.is_match(name)
}

impl Validate for Model {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Validate namespace
        if self.namespace.is_empty() {
            return Err(ConcertoError::ValidationError("Namespace cannot be empty".to_string()));
        }

        // Validate declarations if any
        if let Some(declarations) = &self.declarations {
            // Check for duplicate declaration names
            let mut declaration_names = std::collections::HashSet::new();
            for decl in declarations {
                // Check if this declaration name has already been seen
                if !declaration_names.insert(&decl.name) {
                    return Err(ConcertoError::ValidationError(
                        format!("Duplicate declaration name: {}", decl.name)
                    ));
                }

                // Validate the declaration itself
                decl.validate()?;
            }
        }

        // Validate decorators if any
        if let Some(decorators) = &self.decorators {
            for decorator in decorators {
                decorator.validate()?;
            }
        }

        Ok(())
    }
}

impl Validate for Declaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Basic validation of a declaration using the common validator
        self.validate_name(&self.name)?;
        self.validate_decorators(&self.decorators)?;
        Ok(())
    }
}

// Implement DeclarationValidator for the Declaration struct
impl DeclarationValidator for Declaration {
    fn validate_declaration(&self) -> Result<(), ConcertoError> {
        self.validate_name(&self.name)?;
        self.validate_decorators(&self.decorators)?;
        Ok(())
    }

    fn validate_name(&self, name: &str) -> Result<(), ConcertoError> {
        crate::traits::CommonDeclarationValidator::validate_identifier(name)
    }

    fn validate_decorators(&self, decorators: &Option<Vec<Decorator>>) -> Result<(), ConcertoError> {
        crate::traits::CommonDeclarationValidator::validate_decorators(decorators)
    }
}

impl Validate for Decorator {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Validate the name of the decorator
        if self.name.is_empty() {
            return Err(ConcertoError::ValidationError("Decorator name cannot be empty".to_string()));
        }

        // Arguments validation would go here if needed

        Ok(())
    }
}

// Use DeclarationValidator trait for implementing Validate
use crate::traits::DeclarationValidator;

impl Validate for AssetDeclaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        // We can use our new trait for validation
        self.validate_declaration()
    }
}

impl Validate for ConceptDeclaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Call the common declaration validation
        self.validate_declaration()?;

        // Check for duplicate property names
        let mut property_names = std::collections::HashSet::new();
        for property in &self.properties {
            // Check if this property name has already been seen
            if !property_names.insert(&property.name) {
                return Err(ConcertoError::ValidationError(
                    format!("Duplicate property name: {} in concept {}", property.name, self.name)
                ));
            }

            // Validate the property itself
            property.validate()?;
        }

        Ok(())
    }
}

impl Validate for ParticipantDeclaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        self.validate_declaration()
    }
}

impl Validate for TransactionDeclaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        self.validate_declaration()
    }
}

impl Validate for EventDeclaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        self.validate_declaration()
    }
}

impl Validate for EnumDeclaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use our new trait for validation
        self.validate_declaration()
    }
}

impl Validate for EnumProperty {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Validate enum property name
        if !is_valid_identifier(&self.name) {
            return Err(ConcertoError::ValidationError(format!("'{}' is not a valid enum property name. Identifiers must start with a letter and can contain only letters, numbers, or underscores", self.name)));
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

impl Validate for MapKeyType {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Validate decorators if present
        if let Some(decorators) = &self.decorators {
            for decorator in decorators {
                decorator.validate()?;
            }
        }

        // Basic validation - other checks happen in MapDeclaration::validate_declaration
        Ok(())
    }
}

// Add validation for MapValueType
impl Validate for MapValueType {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Validate decorators if present
        if let Some(decorators) = &self.decorators {
            for decorator in decorators {
                decorator.validate()?;
            }
        }

        // Each value type might have additional specific validation requirements
        // but for now this is sufficient

        Ok(())
    }
}

// Fix the MapDeclaration validate method to use our new trait
impl Validate for MapDeclaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use our new trait for validation
        self.validate_declaration()
    }
}

impl Validate for ScalarDeclaration {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Use the DeclarationValidator trait we implemented
        self.validate_declaration()
    }
}

impl Validate for Property {
    fn validate(&self) -> Result<(), ConcertoError> {
        // For the standalone Validate trait, we'll do basic validation
        // The PropertyValidator trait is for validation within a model context

        // Validate property name
        if !is_valid_identifier(&self.name) {
            return Err(ConcertoError::ValidationError(format!("'{}' is not a valid property name. Identifiers must start with a letter and can contain only letters, numbers, or underscores", self.name)));
        }

        // TODO check for other reserved names

        // Check for system-reserved property names
        if self.name.starts_with('$') {
            return Err(ConcertoError::ValidationError(format!("Invalid field name '{}'. Property names starting with $ are reserved for system use", self.name)));
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

// Import validation
impl Validate for Import {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Validate namespace
        if self.namespace.is_empty() {
            return Err(ConcertoError::ValidationError("Import namespace cannot be empty".to_string()));
        }

        Ok(())
    }
}

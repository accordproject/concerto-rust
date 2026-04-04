use std::collections::HashMap;

use crate::error::ConcertoError;
use crate::model_file::ModelFile;
use crate::validation::Validate;
use crate::metamodel::concerto_metamodel_1_0_0::*;
use crate::traits::*;

/// Manages models and provides validation
/// Maps from JavaScript ModelManager class but using metamodel types
#[derive(Debug, Default)]
pub struct ModelManager {
    /// The model files managed by this instance
    pub models: HashMap<String, ModelFile>,

    /// Whether strict validation is enabled
    pub strict: bool,
}

impl ModelManager {
    /// Creates a new model manager
    pub fn new(strict: bool) -> Self {
        ModelManager {
            models: HashMap::new(),
            strict,
        }
    }

    /// Adds a model file to the model manager
    pub fn add_model_file(&mut self, model_file: ModelFile) -> Result<(), ConcertoError> {
        // Basic validation of the model file (without cross-model validation)
        model_file.validate()?;

        // Add to our collection
        self.models.insert(model_file.model.namespace.clone(), model_file);

        Ok(())
    }

    /// Gets a model file by namespace
    pub fn get_model_file(&self, namespace: &str) -> Option<&ModelFile> {
        self.models.get(namespace)
    }

    /// Gets all model files
    pub fn get_model_files(&self) -> Vec<&ModelFile> {
        self.models.values().collect()
    }

    /// Validates all models
    pub fn validate_models(&self) -> Result<(), ConcertoError> {
        // First, validate each model file individually
        for model_file in self.models.values() {
            model_file.validate()?;
        }

        // Then perform cross-model validation
        self.validate_references()?;

        // Validate no circular inheritance
        self.validate_no_circular_inheritance()?;

        Ok(())
    }

    /// Validates that there is no circular inheritance in the model
    fn validate_no_circular_inheritance(&self) -> Result<(), ConcertoError> {
        for model_file in self.models.values() {
            if let Some(declarations) = &model_file.model.declarations {
                // Build a map of class name -> supertype name for concept-like declarations
                let mut inheritance_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                for decl in declarations {
                    let super_type = match decl {
                        Declaration::Concept(d) => d.super_type.as_ref(),
                        Declaration::Asset(d) => d.super_type.as_ref(),
                        Declaration::Participant(d) => d.super_type.as_ref(),
                        Declaration::Transaction(d) => d.super_type.as_ref(),
                        Declaration::Event(d) => d.super_type.as_ref(),
                        _ => None,
                    };
                    if let Some(st) = super_type {
                        inheritance_map.insert(decl.name().to_string(), st.name.clone());
                    }
                }

                // For each class, walk the inheritance chain looking for cycles
                for class_name in inheritance_map.keys() {
                    let mut visited = std::collections::HashSet::new();
                    visited.insert(class_name.clone());
                    let mut current = class_name.clone();
                    while let Some(next_super) = inheritance_map.get(&current) {
                        if !visited.insert(next_super.clone()) {
                            return Err(ConcertoError::ValidationError(
                                format!("Circular inheritance detected involving class {}", class_name)
                            ));
                        }
                        current = next_super.clone();
                    }
                }
            }
        }
        Ok(())
    }



    /// Validates all references between models
    fn validate_references(&self) -> Result<(), ConcertoError> {
        // Iterate through all model files
        for model_file in self.models.values() {
            // Get declarations for this model file
            if let Some(declarations) = &model_file.model.declarations {
                // Check each declaration - using traits to handle different types
                for declaration in declarations {
                    // Check for concept declarations that might have super types
                    if let Some(concept) = self.as_concept_declaration(declaration) {
                        // Validate super type if present
                        if let Some(super_type) = concept.get_super_type() {
                            self.validate_type_exists(super_type)?;
                        }

                        // Validate property types
                        for property in concept.get_properties() {
                            self.validate_property(property)?;
                        }
                    }

                    // Check for map declarations
                    if let Some(map_decl) = self.as_map_declaration(declaration) {
                        self.validate_map_value_type(&map_decl.value)?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Returns the inner concept-like declaration as a trait object, if applicable
    fn as_concept_declaration<'a>(&self, decl: &'a Declaration) -> Option<&'a dyn ConceptDeclarationBase> {
        match decl {
            Declaration::Concept(d) => Some(d as &dyn ConceptDeclarationBase),
            Declaration::Asset(d) => Some(d as &dyn ConceptDeclarationBase),
            Declaration::Participant(d) => Some(d as &dyn ConceptDeclarationBase),
            Declaration::Transaction(d) => Some(d as &dyn ConceptDeclarationBase),
            Declaration::Event(d) => Some(d as &dyn ConceptDeclarationBase),
            _ => None,
        }
    }

    /// Returns the inner MapDeclaration if the declaration is a map
    fn as_map_declaration<'a>(&self, decl: &'a Declaration) -> Option<&'a MapDeclaration> {
        match decl {
            Declaration::Map(d) => Some(d),
            _ => None,
        }
    }

    /// Validates a property
    fn validate_property(&self, property: &Property) -> Result<(), ConcertoError> {
        // Use our PropertyValidator trait
        use crate::traits::PropertyValidator;
        PropertyValidator::validate(property, self)
    }

    /// Validates a map value type
    fn validate_map_value_type(&self, value_type: &MapValueType) -> Result<(), ConcertoError> {
        // Implementation would depend on the MapValueType structure
        Ok(())
    }

    /// Validates that a referenced type exists in the model
    pub fn validate_type_exists(&self, type_id: &crate::metamodel::concerto_metamodel_1_0_0::TypeIdentifier) -> Result<(), ConcertoError> {
        let namespace = match &type_id.namespace {
            Some(ns) => ns,
            None => {
                return Err(ConcertoError::ValidationError(
                    format!("Type {} is missing namespace", type_id.name)
                ));
            }
        };

        // Find the model file for this namespace
        let model_file = match self.get_model_file(namespace) {
            Some(mf) => mf,
            None => {
                return Err(ConcertoError::ValidationError(
                    format!("Could not find namespace {}", namespace)
                ));
            }
        };

        // Check if type exists in this namespace using the DeclarationBase trait
        if let Some(declarations) = &model_file.model.declarations {
            for decl in declarations {
                // Use the DeclarationBase trait to get name regardless of declaration type
                if decl.name() == type_id.name {
                    return Ok(());
                }
            }
        }

        Err(ConcertoError::ValidationError(
            format!("Could not find type {}.{}", namespace, type_id.name)
        ))
    }

    /// Validates a property type
    fn validate_property_type(&self, property: &Property) -> Result<(), ConcertoError> {
        // Using the PropertyValidator trait to validate based on property type
        // In a real implementation, you'd check the _class field and cast to the appropriate type
        // For demonstration purposes, we'll just return Ok

        // Example of how it might work:
        // if property._class == "concerto.metamodel@1.0.0.RelationshipProperty" {
        //     let relationship = property as &RelationshipProperty;
        //     if let Some(type_id) = &relationship.type_reference {
        //         self.validate_type_exists(type_id)?;
        //     } else {
        //         return Err(ConcertoError::ValidationError(
        //             "Relationship type is missing type reference".to_string()
        //         ));
        //     }
        // }

        Ok(())
    }
}

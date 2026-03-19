use std::path::Path;
use std::fs;

use crate::error::ConcertoError;
use crate::metamodel::concerto_metamodel_1_0_0::{Model, Import, Declaration};
use crate::validation::Validate;

/// Represents a Concerto model file
/// Using the metamodel structures
#[derive(Debug, Clone)]
pub struct ModelFile {
    /// The metamodel representation
    pub model: Model,
    pub content: String,
    pub file_name: String,
}

impl ModelFile {
    /// Creates a new model file

    pub fn new(model: Model, content: String, file_name: String) -> Self {
        ModelFile { model, content, file_name }
    }

    /// Creates a new model file from a namespace and optional version.
    /// Convenience constructor for tests and simple use cases.
    pub fn from_namespace(namespace: String, version: Option<String>) -> Self {
        let model = Model {
            _class: "concerto.metamodel@1.0.0.Model".to_string(),
            namespace,
            source_uri: None,
            concerto_version: version,
            imports: None,
            declarations: None,
            decorators: None,
        };
        ModelFile {
            model,
            content: String::new(),
            file_name: String::new(),
        }
    }
    pub fn get_name(&self) -> String {
        self.model.namespace.clone()
    }

    

    /// Loads a model file from a string
    pub fn from_string(content: String) -> Result<Self, ConcertoError> {
        // Note: This is a placeholder - in a real implementation,
        // you would parse the string into a Model structure.
        // For now, we'll just return an error as this functionality would be
        // provided by the JavaScript parser
        Err(ConcertoError::ParseError(
            "Parsing from string is not implemented in Rust yet. Use the JavaScript parser.".to_string()
        ))
    }

    /// Loads a model file from a file path
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConcertoError> {
        let content = fs::read_to_string(&path)
            .map_err(|e| ConcertoError::ParseError(format!("Failed to read file: {}", e)))?;

        Self::from_string(content)
    }

    /// Gets the namespace for this model file
    pub fn get_namespace(&self) -> &str {
        &self.model.namespace
    }

    /// Gets the version for this model file
    pub fn get_version(&self) -> Option<&String> {
        self.model.concerto_version.as_ref()
    }

    /// Gets the imports for this model file
    pub fn get_imports(&self) -> Vec<&Import> {
        match &self.model.imports {
            Some(imports) => imports.iter().collect(),
            None => Vec::new(),
        }
    }

    /// Gets the declarations for this model file
    pub fn get_declarations(&self) -> Vec<&Declaration> {
        match &self.model.declarations {
            Some(declarations) => declarations.iter().collect(),
            None => Vec::new(),
        }
    }

    /// Adds an import to this model file
    pub fn add_import(&mut self, import: Import) {
        if self.model.imports.is_none() {
            self.model.imports = Some(Vec::new());
        }

        if let Some(imports) = &mut self.model.imports {
            imports.push(import);
        }
    }

    /// Adds a declaration to this model file
    pub fn add_declaration(&mut self, declaration: Declaration) {
        if self.model.declarations.is_none() {
            self.model.declarations = Some(Vec::new());
        }

        if let Some(declarations) = &mut self.model.declarations {
            declarations.push(declaration);
        }
    }

}

impl Validate for ModelFile {
    fn validate(&self) -> Result<(), ConcertoError> {
        // Validate the model structure first
        self.model.validate()?;

        // Validate properties within concept-like declarations
        if let Some(declarations) = &self.model.declarations {
            for declaration in declarations {
                // Check concept-like declarations that have properties
                if declaration._class.contains("ConceptDeclaration")
                    || declaration._class.contains("AssetDeclaration")
                    || declaration._class.contains("ParticipantDeclaration")
                    || declaration._class.contains("TransactionDeclaration")
                    || declaration._class.contains("EventDeclaration")
                {
                    let properties = declaration.get_properties();
                    for prop in &properties {
                        prop.validate()?;
                    }
                }
            }
        }

        Ok(())
    }
}

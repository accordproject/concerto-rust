use crate::error::{ConcertoError, Result};
use crate::introspect::declarations::{ClassDeclaration, Declaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::validation;
use crate::model_util;

use std::collections::HashMap;

// ===================================================================
// ModelManagerOptions
// ===================================================================

/// Configuration for [`ModelManager`].
#[derive(Debug, Clone)]
pub struct ModelManagerOptions {
    /// Require versioned namespaces.
    pub strict: bool,
    /// Enable the Map type feature.
    pub enable_map_type: bool,
}

impl Default for ModelManagerOptions {
    fn default() -> Self {
        Self {
            strict: false,
            enable_map_type: true,
        }
    }
}

// ===================================================================
// ModelManager
// ===================================================================

/// Manages a set of Concerto model files, providing type resolution
/// and validation across namespaces.
#[derive(Debug)]
pub struct ModelManager {
    model_files: HashMap<String, ModelFile>,
    options: ModelManagerOptions,
}

impl ModelManager {
    /// Creates a new [`ModelManager`] with the built-in root model loaded.
    pub fn new(options: ModelManagerOptions) -> Result<Self> {
        let mut mgr = Self {
            model_files: HashMap::new(),
            options,
        };
        mgr.add_root_model()?;
        Ok(mgr)
    }

    fn add_root_model(&mut self) -> Result<()> {
        let json = crate::rootmodel::root_model_ast();
        let mf = ModelFile::from_json(&json, Some("rootmodel".into()))?;
        self.model_files.insert(mf.namespace().to_string(), mf);
        Ok(())
    }

    /// Add a model from its JSON AST representation.
    pub fn add_model(
        &mut self,
        json: &serde_json::Value,
        file_name: Option<String>,
        disable_validation: bool,
    ) -> Result<()> {
        let mf = ModelFile::from_json(json, file_name)?;
        let ns = mf.namespace().to_string();

        if self.model_files.contains_key(&ns) {
            return Err(ConcertoError::IllegalModel {
                message: format!("Duplicate namespace: {ns}"),
                file_name: mf.file_name().map(String::from),
                location: None,
            });
        }

        self.model_files.insert(ns, mf);

        if !disable_validation {
            self.validate_model_files()?;
        }

        Ok(())
    }

    /// Add multiple models at once. On error, all additions are rolled back.
    pub fn add_models(
        &mut self,
        models: &[serde_json::Value],
        disable_validation: bool,
    ) -> Result<()> {
        let mut added_namespaces = Vec::new();

        for json in models {
            let mf = ModelFile::from_json(json, None)?;
            let ns = mf.namespace().to_string();

            if self.model_files.contains_key(&ns) {
                // Rollback
                for ns in &added_namespaces {
                    self.model_files.remove(ns);
                }
                return Err(ConcertoError::IllegalModel {
                    message: format!("Duplicate namespace: {ns}"),
                    file_name: mf.file_name().map(String::from),
                    location: None,
                });
            }

            self.model_files.insert(ns.clone(), mf);
            added_namespaces.push(ns);
        }

        if !disable_validation {
            if let Err(e) = self.validate_model_files() {
                // Rollback
                for ns in &added_namespaces {
                    self.model_files.remove(ns);
                }
                return Err(e);
            }
        }

        Ok(())
    }

    /// Validate all loaded model files.
    pub fn validate_model_files(&self) -> Result<()> {
        validation::validate_model_files(&self.model_files)
    }

    /// Resolve a fully-qualified type name to its declaration.
    pub fn get_type(&self, fqn: &str) -> Result<&Declaration> {
        let ns = model_util::get_namespace(fqn);
        let short = model_util::get_short_name(fqn);

        let mf = self.model_files.get(ns).ok_or_else(|| {
            ConcertoError::TypeNotFound {
                type_name: fqn.to_string(),
            }
        })?;

        mf.get_local_type(short).ok_or_else(|| {
            ConcertoError::TypeNotFound {
                type_name: fqn.to_string(),
            }
        })
    }

    /// Look up a model file by namespace.
    pub fn get_model_file(&self, namespace: &str) -> Option<&ModelFile> {
        self.model_files.get(namespace)
    }

    /// Returns all loaded model files (excluding system namespaces).
    pub fn get_model_files(&self) -> Vec<&ModelFile> {
        self.model_files
            .values()
            .filter(|mf| !mf.is_system_model_file())
            .collect()
    }

    /// Returns all loaded model files including system namespaces.
    pub fn get_all_model_files(&self) -> Vec<&ModelFile> {
        self.model_files.values().collect()
    }

    pub fn options(&self) -> &ModelManagerOptions {
        &self.options
    }

    /// Returns all class declarations across all model files.
    pub fn class_declarations(&self) -> Vec<&ClassDeclaration> {
        self.model_files
            .values()
            .flat_map(|mf| mf.class_declarations())
            .collect()
    }

    /// Look up a class declaration by FQN.
    pub fn get_class_declaration(&self, fqn: &str) -> Result<&ClassDeclaration> {
        let decl = self.get_type(fqn)?;
        decl.as_class().ok_or_else(|| ConcertoError::TypeNotFound {
            type_name: fqn.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_manager_creates_with_root_model() {
        let mgr = ModelManager::new(ModelManagerOptions::default()).unwrap();
        assert!(mgr.get_model_file("concerto@1.0.0").is_some());
        assert!(mgr.get_type("concerto@1.0.0.Concept").is_ok());
        assert!(mgr.get_type("concerto@1.0.0.Asset").is_ok());
    }

    #[test]
    fn test_model_manager_add_model() {
        let mut mgr = ModelManager::new(ModelManagerOptions::default()).unwrap();

        let model_json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example@1.0.0",
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Person",
                    "isAbstract": false,
                    "properties": [
                        {
                            "$class": "concerto.metamodel@1.0.0.StringProperty",
                            "name": "name",
                            "isArray": false,
                            "isOptional": false
                        }
                    ]
                }
            ]
        });

        mgr.add_model(&model_json, None, false).unwrap();
        assert!(mgr.get_type("org.example@1.0.0.Person").is_ok());
    }

    #[test]
    fn test_duplicate_namespace_rejected() {
        let mut mgr = ModelManager::new(ModelManagerOptions::default()).unwrap();

        let model_json = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example@1.0.0",
            "declarations": []
        });

        mgr.add_model(&model_json, None, true).unwrap();
        let result = mgr.add_model(&model_json, None, true);
        assert!(result.is_err());
    }

    #[test]
    fn test_batch_add_with_rollback() {
        let mut mgr = ModelManager::new(ModelManagerOptions::default()).unwrap();

        let models = vec![
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.a@1.0.0",
                "declarations": []
            }),
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.a@1.0.0",
                "declarations": []
            }),
        ];

        let result = mgr.add_models(&models, true);
        assert!(result.is_err());
        // First namespace should have been rolled back
        assert!(mgr.get_model_file("org.a@1.0.0").is_none());
    }
}

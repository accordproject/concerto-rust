use std::collections::HashMap;

use crate::error::{ConcertoError, Result};
use crate::model_util;

use super::declarations::*;
use super::imports::ImportDecl;
use super::traits::Named;

/// A parsed model file with resolved declarations and imports.
#[derive(Debug, Clone)]
pub struct ModelFile {
    namespace: String,
    version: Option<String>,
    declarations: Vec<Declaration>,
    imports: Vec<ImportDecl>,
    /// Maps short (local) type name → index into `declarations`.
    local_types: HashMap<String, usize>,
    file_name: Option<String>,
    is_external: bool,
}

impl ModelFile {
    /// Build a [`ModelFile`] by deserializing the raw JSON AST of a
    /// `concerto.metamodel@1.0.0.Model`.
    ///
    /// This delegates to `Declaration::try_from` which inspects the `$class`
    /// discriminator on each declaration and property to construct the correct
    /// sum-type variant.
    pub fn from_json(json: &serde_json::Value, file_name: Option<String>) -> Result<Self> {
        let namespace = json
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConcertoError::IllegalModel {
                message: "Model missing 'namespace'".into(),
                file_name: file_name.clone(),
                location: None,
            })?
            .to_string();

        let parsed_ns = model_util::parse_namespace(&namespace)?;

        let is_external = json
            .get("sourceUri")
            .and_then(|v| v.as_str())
            .is_some();

        // --- imports -------------------------------------------------------
        let imports = match json.get("imports") {
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(|v| ImportDecl::try_from(v.clone()))
                .collect::<Result<Vec<_>>>()?,
            _ => vec![],
        };

        // --- declarations --------------------------------------------------
        let raw_decls = match json.get("declarations") {
            Some(serde_json::Value::Array(arr)) => arr,
            _ => return Ok(Self::new_empty(namespace, parsed_ns.version, file_name, is_external, imports)),
        };

        let mut declarations = Vec::with_capacity(raw_decls.len());
        let mut local_types = HashMap::new();

        for (idx, raw) in raw_decls.iter().enumerate() {
            let decl = Declaration::try_from(raw.clone()).map_err(|e| match e {
                ConcertoError::IllegalModel { message, .. } => ConcertoError::IllegalModel {
                    message,
                    file_name: file_name.clone(),
                    location: None,
                },
                other => other,
            })?;
            local_types.insert(decl.name().to_string(), idx);
            declarations.push(decl);
        }

        Ok(Self {
            namespace,
            version: parsed_ns.version,
            declarations,
            imports,
            local_types,
            file_name,
            is_external,
        })
    }

    fn new_empty(
        namespace: String,
        version: Option<String>,
        file_name: Option<String>,
        is_external: bool,
        imports: Vec<ImportDecl>,
    ) -> Self {
        Self {
            namespace,
            version,
            declarations: vec![],
            imports,
            local_types: HashMap::new(),
            file_name,
            is_external,
        }
    }

    // ---------------------------------------------------------------
    // Accessors
    // ---------------------------------------------------------------

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    pub fn is_external(&self) -> bool {
        self.is_external
    }

    pub fn all_declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    pub fn imports(&self) -> &[ImportDecl] {
        &self.imports
    }

    pub fn get_local_type(&self, name: &str) -> Option<&Declaration> {
        self.local_types.get(name).map(|&idx| &self.declarations[idx])
    }

    /// Resolve a short type name to its fully-qualified name, checking
    /// local types first, then imports.
    pub fn fully_qualified_type_name(&self, short_name: &str) -> Result<String> {
        // 1. Primitive?
        if model_util::is_primitive_type(short_name) {
            return Ok(short_name.to_string());
        }

        // 2. Local type?
        if self.local_types.contains_key(short_name) {
            return Ok(model_util::fully_qualified_name(&self.namespace, short_name));
        }

        // 3. Imported type?
        for imp in &self.imports {
            match imp {
                ImportDecl::Type(t) if t.name == short_name => {
                    return Ok(model_util::fully_qualified_name(&t.namespace, &t.name));
                }
                ImportDecl::Types(t) => {
                    if t.types.iter().any(|n| n == short_name) {
                        return Ok(model_util::fully_qualified_name(&t.namespace, short_name));
                    }
                    if let Some(aliased) = &t.aliased_types {
                        if let Some(at) = aliased.iter().find(|a| a.name == short_name) {
                            return Ok(model_util::fully_qualified_name(
                                &t.namespace,
                                &at.aliased_name,
                            ));
                        }
                    }
                }
                ImportDecl::All(_) => {
                    // Cannot resolve without the model manager — keep searching
                }
                _ => {}
            }
        }

        Err(ConcertoError::TypeNotFound {
            type_name: short_name.to_string(),
        })
    }

    /// Returns `true` when this is the `concerto@1.0.0` system model.
    pub fn is_system_model_file(&self) -> bool {
        self.namespace.starts_with("concerto@") || self.namespace == "concerto"
    }

    // ---------------------------------------------------------------
    // Filtered declaration accessors
    // ---------------------------------------------------------------

    pub fn class_declarations(&self) -> Vec<&ClassDeclaration> {
        self.declarations
            .iter()
            .filter_map(|d| d.as_class())
            .collect()
    }

    pub fn concept_declarations(&self) -> Vec<&ConceptDeclaration> {
        self.class_declarations()
            .into_iter()
            .filter_map(|c| match c {
                ClassDeclaration::Concept(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    pub fn asset_declarations(&self) -> Vec<&AssetDeclaration> {
        self.class_declarations()
            .into_iter()
            .filter_map(|c| match c {
                ClassDeclaration::Asset(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    pub fn participant_declarations(&self) -> Vec<&ParticipantDeclaration> {
        self.class_declarations()
            .into_iter()
            .filter_map(|c| match c {
                ClassDeclaration::Participant(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    pub fn transaction_declarations(&self) -> Vec<&TransactionDeclaration> {
        self.class_declarations()
            .into_iter()
            .filter_map(|c| match c {
                ClassDeclaration::Transaction(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    pub fn event_declarations(&self) -> Vec<&EventDeclaration> {
        self.class_declarations()
            .into_iter()
            .filter_map(|c| match c {
                ClassDeclaration::Event(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    pub fn enum_declarations(&self) -> Vec<&EnumDeclaration> {
        self.class_declarations()
            .into_iter()
            .filter_map(|c| match c {
                ClassDeclaration::Enum(d) => Some(d),
                _ => None,
            })
            .collect()
    }

    pub fn scalar_declarations(&self) -> Vec<&ScalarDeclaration> {
        self.declarations
            .iter()
            .filter_map(|d| d.as_scalar())
            .collect()
    }

    pub fn map_declarations(&self) -> Vec<&MapDeclaration> {
        self.declarations
            .iter()
            .filter_map(|d| d.as_map())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_root_model() {
        let json = crate::rootmodel::root_model_ast();
        let mf = ModelFile::from_json(&json, Some("rootmodel".into())).unwrap();

        assert_eq!(mf.namespace(), "concerto@1.0.0");
        assert_eq!(mf.version(), Some("1.0.0"));
        assert!(mf.is_system_model_file());

        // Root model has 5 declarations: Concept, Asset, Participant, Transaction, Event
        assert_eq!(mf.all_declarations().len(), 5);

        // All are concept declarations in the root model
        assert_eq!(mf.concept_declarations().len(), 5);

        // Check specific types exist
        assert!(mf.get_local_type("Concept").is_some());
        assert!(mf.get_local_type("Asset").is_some());
        assert!(mf.get_local_type("Participant").is_some());
        assert!(mf.get_local_type("Transaction").is_some());
        assert!(mf.get_local_type("Event").is_some());
    }

    #[test]
    fn test_fully_qualified_type_name() {
        let json = crate::rootmodel::root_model_ast();
        let mf = ModelFile::from_json(&json, None).unwrap();

        assert_eq!(
            mf.fully_qualified_type_name("Concept").unwrap(),
            "concerto@1.0.0.Concept"
        );
        assert_eq!(
            mf.fully_qualified_type_name("String").unwrap(),
            "String"
        );
        assert!(mf.fully_qualified_type_name("DoesNotExist").is_err());
    }
}

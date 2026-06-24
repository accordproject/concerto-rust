//! One parsed Concerto model file.
//!
//! A [`ModelFile`] owns the declarations and imports of a single namespace and
//! indexes its declarations by short name. It resolves a short name to a
//! fully-qualified one from what it declares or imports: the primitives, its
//! own declarations, and its named imports. (Wildcard imports are rejected
//! while parsing, per strict mode in Concerto v4.)

use std::collections::HashMap;

use crate::error::{ConcertoError, Result};
use crate::introspect::declaration::Declaration;
use crate::introspect::import::Import;
use crate::model_util::{is_primitive_type, parse_namespace, qualify};

/// A parsed model file for one namespace.
#[derive(Debug, Clone)]
pub struct ModelFile {
    namespace: String,
    version: Option<String>,
    imports: Vec<Import>,
    declarations: Vec<Declaration>,
    local_types: HashMap<String, usize>,
    file_name: Option<String>,
}

impl ModelFile {
    /// Builds a model file from the JSON AST of a `concerto.metamodel@….Model`.
    pub fn from_json(value: &serde_json::Value, file_name: Option<String>) -> Result<Self> {
        let namespace = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConcertoError::IllegalModel {
                message: "model missing 'namespace'".into(),
                file_name: file_name.clone(),
                location: None,
            })?
            .to_string();

        let version = parse_namespace(&namespace)?.version;

        let imports = match value.get("imports") {
            None => Vec::new(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(Import::try_from)
                .collect::<Result<Vec<_>>>()?,
            Some(_) => {
                return Err(ConcertoError::IllegalModel {
                    message: "model 'imports' must be an array".into(),
                    file_name: file_name.clone(),
                    location: None,
                });
            }
        };

        let mut declarations = Vec::new();
        let mut local_types = HashMap::new();
        match value.get("declarations") {
            None => {}
            Some(serde_json::Value::Array(arr)) => {
                for raw in arr {
                    let decl = Declaration::try_from(raw).map_err(|e| annotate(e, &file_name))?;
                    if local_types
                        .insert(decl.name().to_string(), declarations.len())
                        .is_some()
                    {
                        return Err(ConcertoError::IllegalModel {
                            message: format!(
                                "duplicate declaration '{}' in {namespace}",
                                decl.name()
                            ),
                            file_name: file_name.clone(),
                            location: None,
                        });
                    }
                    declarations.push(decl);
                }
            }
            Some(_) => {
                return Err(ConcertoError::IllegalModel {
                    message: "model 'declarations' must be an array".into(),
                    file_name: file_name.clone(),
                    location: None,
                });
            }
        }

        Ok(Self {
            namespace,
            version,
            imports,
            declarations,
            local_types,
            file_name,
        })
    }

    /// The full namespace, including any version, e.g. `org.example@1.0.0`.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The version part of the namespace, if it has one.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// The originating file name, if one was supplied.
    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    /// Every declaration, in the order they appear in the file.
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    /// The imports.
    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    /// Finds a declaration by its short name.
    pub fn local_declaration(&self, short: &str) -> Option<&Declaration> {
        self.local_types.get(short).map(|&i| &self.declarations[i])
    }

    /// True if this is the built-in `concerto` system namespace.
    pub fn is_system(&self) -> bool {
        self.namespace == "concerto" || self.namespace.starts_with("concerto@")
    }

    /// Resolves a short name from what this file declares or imports: the
    /// primitives, its own declarations, and its named imports. Returns `None`
    /// if the name is none of those.
    pub fn resolve_local(&self, short: &str) -> Option<String> {
        if is_primitive_type(short) {
            return Some(short.to_string());
        }
        if self.local_types.contains_key(short) {
            return Some(qualify(&self.namespace, short));
        }
        self.imports.iter().find_map(|imp| imp.resolve(short))
    }
}

/// Stamps this file's name onto an `IllegalModel` error that came up while
/// parsing one of its declarations, so the message points somewhere useful.
fn annotate(err: ConcertoError, file_name: &Option<String>) -> ConcertoError {
    match err {
        ConcertoError::IllegalModel {
            message, location, ..
        } => ConcertoError::IllegalModel {
            message,
            file_name: file_name.clone(),
            location,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ModelFile {
        ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.common@1.0.0", "name": "Address" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                      "name": "Person", "isAbstract": false, "properties": [] }
                ]
            }),
            Some("example.cto".into()),
        )
        .unwrap()
    }

    #[test]
    fn parses_namespace_imports_and_declarations() {
        let mf = sample();
        assert_eq!(mf.namespace(), "org.example@1.0.0");
        assert_eq!(mf.version(), Some("1.0.0"));
        assert_eq!(mf.declarations().len(), 1);
        assert_eq!(mf.imports().len(), 1);
        assert!(mf.local_declaration("Person").is_some());
        assert!(!mf.is_system());
    }

    #[test]
    fn resolves_local_primitive_and_import() {
        let mf = sample();
        assert_eq!(
            mf.resolve_local("Person").as_deref(),
            Some("org.example@1.0.0.Person")
        );
        assert_eq!(mf.resolve_local("String").as_deref(), Some("String"));
        assert_eq!(
            mf.resolve_local("Address").as_deref(),
            Some("org.common@1.0.0.Address")
        );
        assert_eq!(mf.resolve_local("Missing"), None);
    }

    #[test]
    fn duplicate_declaration_is_rejected() {
        let err = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.dup@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "A", "isAbstract": false, "properties": [] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "A", "isAbstract": false, "properties": [] }
                ]
            }),
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn missing_namespace_is_rejected() {
        let err = ModelFile::from_json(
            &serde_json::json!({ "$class": "concerto.metamodel@1.0.0.Model" }),
            None,
        );
        assert!(err.is_err());
    }

    #[test]
    fn non_array_declarations_or_imports_is_rejected() {
        let bad_decls = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.x@1.0.0",
                "declarations": { "not": "an array" }
            }),
            None,
        );
        assert!(bad_decls.is_err());

        let bad_imports = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.x@1.0.0",
                "imports": "nope"
            }),
            None,
        );
        assert!(bad_imports.is_err());
    }
}

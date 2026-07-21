//! A model's imports, given proper types.
//!
//! Deserializing through the generated metamodel flattens every import into the
//! base `Import` struct and discards the type names it pulls in. Each import is
//! therefore re-read from its raw JSON into this [`Import`] enum, keyed on the
//! `$class`, so the introspect layer can resolve a short name back to the
//! namespace that declares it.

use crate::error::{ConcertoError, Result};
use crate::introspect::declared_class;
use crate::model_util::{qualify, short_name};

/// A single import statement in a model file. Wildcard imports (`import ns.*`)
/// are rejected while parsing, mirroring strict mode in Concerto v4.
#[derive(Debug, Clone)]
pub enum Import {
    /// `import ns.Name`: a single named type.
    Type {
        /// The namespace the type is imported from.
        namespace: String,
        /// The name of the imported type.
        name: String,
    },
    /// `import ns.{A, B}`: several named types, optionally aliased.
    Types {
        /// The namespace the types are imported from.
        namespace: String,
        /// The names of the imported types.
        names: Vec<String>,
        /// `(local_alias, original_name)` pairs for aliased imports.
        aliases: Vec<(String, String)>,
    },
}

impl Import {
    /// The namespace this import refers to.
    pub fn namespace(&self) -> &str {
        match self {
            Self::Type { namespace, .. } | Self::Types { namespace, .. } => namespace,
        }
    }

    /// Resolves a short name to its fully-qualified name, but only when this
    /// import names it explicitly.
    pub fn resolve(&self, short: &str) -> Option<String> {
        match self {
            Self::Type { namespace, name } if name == short => Some(qualify(namespace, name)),
            Self::Type { .. } => None,
            Self::Types {
                namespace,
                names,
                aliases,
            } => {
                if let Some((_, original)) = aliases.iter().find(|(alias, _)| alias == short) {
                    return Some(qualify(namespace, original));
                }
                if names.iter().any(|n| n == short) {
                    return Some(qualify(namespace, short));
                }
                None
            }
        }
    }
}

impl TryFrom<&serde_json::Value> for Import {
    type Error = ConcertoError;

    fn try_from(value: &serde_json::Value) -> Result<Self> {
        let class = declared_class(value);
        if class.is_empty() {
            return Err(ConcertoError::IllegalModel {
                message: "import node is missing its $class".into(),
                file_name: None,
                location: None,
            });
        }
        let kind = short_name(class);

        let namespace = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConcertoError::IllegalModel {
                message: format!("import ({kind}) missing 'namespace'"),
                file_name: None,
                location: None,
            })?
            .to_string();

        Ok(match kind {
            // Concerto v4 disallows wildcard imports; reject them up front.
            "ImportAll" => {
                return Err(ConcertoError::IllegalModel {
                    message: format!("wildcard imports are not allowed: import {namespace}.*"),
                    file_name: None,
                    location: None,
                });
            }
            "ImportType" => {
                let name = value
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ConcertoError::IllegalModel {
                        message: "ImportType missing 'name'".into(),
                        file_name: None,
                        location: None,
                    })?
                    .to_string();
                Self::Type { namespace, name }
            }
            "ImportTypes" => {
                let names = value
                    .get("types")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let aliases = value
                    .get("aliasedTypes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|a| {
                                let alias = a.get("aliasedName").and_then(|v| v.as_str())?;
                                let original = a.get("name").and_then(|v| v.as_str())?;
                                Some((alias.to_string(), original.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Self::Types {
                    namespace,
                    names,
                    aliases,
                }
            }
            other => {
                return Err(ConcertoError::IllegalModel {
                    message: format!("unknown import type: {other}"),
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

    #[test]
    fn resolves_named_import() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": "org.acme@1.0.0",
            "name": "Person"
        }))
        .unwrap();
        assert_eq!(imp.namespace(), "org.acme@1.0.0");
        assert_eq!(
            imp.resolve("Person").as_deref(),
            Some("org.acme@1.0.0.Person")
        );
        assert_eq!(imp.resolve("Other"), None);
    }

    #[test]
    fn resolves_multi_import_with_alias() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.acme@1.0.0",
            "types": ["A", "B"],
            "aliasedTypes": [
                { "$class": "concerto.metamodel@1.0.0.AliasedType", "name": "B", "aliasedName": "Bee" }
            ]
        }))
        .unwrap();
        assert_eq!(imp.resolve("A").as_deref(), Some("org.acme@1.0.0.A"));
        assert_eq!(imp.resolve("Bee").as_deref(), Some("org.acme@1.0.0.B"));
        assert_eq!(imp.resolve("C"), None);
    }

    #[test]
    fn wildcard_import_is_rejected() {
        let err = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportAll",
            "namespace": "org.acme@1.0.0"
        }));
        assert!(err.unwrap_err().to_string().contains("wildcard"));
    }

    #[test]
    fn missing_class_is_rejected() {
        let err = Import::try_from(&serde_json::json!({ "namespace": "org.acme@1.0.0" }));
        assert!(err.unwrap_err().to_string().contains("$class"));
    }
}

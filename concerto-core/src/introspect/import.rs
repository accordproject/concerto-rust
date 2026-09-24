//! A model's imports, given proper types.
//!
//! [`Import`] is a newtype over the metamodel's own `mm::ImportType` and
//! `mm::ImportTypes` structs, selected from the node's `$class`. A wildcard
//! import (`import ns.*`, `mm::ImportAll`) is rejected while parsing,
//! mirroring strict mode in Concerto v4, so the introspect layer never has to
//! consider it.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, Result};
use crate::introspect::{declared_class, with_qualified_class};
use crate::model_util::{qualify, short_name};

/// A single import statement in a model file, wrapping the matching
/// generated struct.
#[derive(Debug, Clone)]
pub enum Import {
    /// `import ns.Name`: a single named type.
    Type(mm::ImportType),
    /// `import ns.{A, B}`: several named types, optionally aliased.
    Types(mm::ImportTypes),
}

impl Import {
    /// The namespace this import refers to.
    pub fn namespace(&self) -> &str {
        match self {
            Self::Type(t) => &t.namespace,
            Self::Types(t) => &t.namespace,
        }
    }

    /// The names this import pulls in, as they are declared in the source
    /// namespace. An alias renames a type locally but does not change the name
    /// it is declared under, so these are the names to look for over there.
    pub fn imported_names(&self) -> &[String] {
        match self {
            Self::Type(t) => std::slice::from_ref(&t.name),
            Self::Types(t) => &t.types,
        }
    }

    /// The names this import makes visible in the importing file. An aliased
    /// type is visible under its alias rather than its declared name, so these
    /// are the names a local declaration could collide with.
    pub fn local_names(&self) -> Vec<&str> {
        match self {
            Self::Type(t) => vec![t.name.as_str()],
            Self::Types(t) => t
                .types
                .iter()
                .map(|name| {
                    t.aliased_types
                        .as_deref()
                        .unwrap_or(&[])
                        .iter()
                        .find(|aliased| &aliased.name == name)
                        .map_or(name.as_str(), |aliased| aliased.aliased_name.as_str())
                })
                .collect(),
        }
    }

    /// Resolves a short name to its fully-qualified name, but only when this
    /// import names it explicitly.
    pub fn resolve(&self, short: &str) -> Option<String> {
        match self {
            Self::Type(t) if t.name == short => Some(qualify(&t.namespace, &t.name)),
            Self::Type(_) => None,
            Self::Types(t) => {
                if let Some(aliased) = t
                    .aliased_types
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .find(|aliased| aliased.aliased_name == short)
                {
                    return Some(qualify(&t.namespace, &aliased.name));
                }
                if t.types.iter().any(|n| n == short) {
                    return Some(qualify(&t.namespace, short));
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
            })?;

        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid import: {e}"),
            file_name: None,
            location: None,
        };

        match kind {
            // Concerto v4 disallows wildcard imports; reject them up front,
            // before trying to parse the rest of the node.
            "ImportAll" => Err(ConcertoError::IllegalModel {
                message: format!("wildcard imports are not allowed: import {namespace}.*"),
                file_name: None,
                location: None,
            }),
            "ImportType" => {
                if value.get("name").and_then(|v| v.as_str()).is_none() {
                    return Err(ConcertoError::IllegalModel {
                        message: "ImportType missing 'name'".into(),
                        file_name: None,
                        location: None,
                    });
                }
                // `kind` may be the bare `$class` short name; qualify it so
                // the generated, `$class`-tagged `mm::Import` recognises it
                // the same way it recognises the fully-qualified form.
                let qualified = with_qualified_class(value, kind);
                let t: mm::ImportType = serde_json::from_value(qualified).map_err(bad)?;
                Ok(Self::Type(t))
            }
            "ImportTypes" => {
                let mut node = with_qualified_class(value, kind);
                if let Some(obj) = node.as_object_mut() {
                    // A model with no named types (`import ns.{}`) omits
                    // 'types' entirely rather than writing an empty array.
                    obj.entry("types")
                        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                }
                let t: mm::ImportTypes = serde_json::from_value(node).map_err(bad)?;
                Ok(Self::Types(t))
            }
            other => Err(ConcertoError::IllegalModel {
                message: format!("unknown import type: {other}"),
                file_name: None,
                location: None,
            }),
        }
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
    fn local_names_use_the_alias_where_one_is_given() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.acme@1.0.0",
            "types": ["A", "B"],
            "aliasedTypes": [
                { "$class": "concerto.metamodel@1.0.0.AliasedType", "name": "B", "aliasedName": "Bee" }
            ]
        }))
        .unwrap();
        assert_eq!(imp.local_names(), ["A", "Bee"]);

        let single = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": "org.acme@1.0.0",
            "name": "Person"
        }))
        .unwrap();
        assert_eq!(single.local_names(), ["Person"]);
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
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("import node is missing its $class")
        );
    }

    #[test]
    fn an_import_class_may_be_given_as_the_short_name() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "ImportType",
            "namespace": "org.acme@1.0.0",
            "name": "Person"
        }))
        .unwrap();
        assert_eq!(imp.namespace(), "org.acme@1.0.0");
        assert_eq!(
            imp.resolve("Person").as_deref(),
            Some("org.acme@1.0.0.Person")
        );
    }

    #[test]
    fn import_types_with_no_types_array_is_empty() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.acme@1.0.0"
        }))
        .unwrap();
        assert!(imp.imported_names().is_empty());
        assert!(imp.local_names().is_empty());
    }

    #[test]
    fn unknown_import_kind_errors() {
        let err = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryImport",
            "namespace": "org.acme@1.0.0"
        }));
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("unknown import type: MysteryImport")
        );
    }
}

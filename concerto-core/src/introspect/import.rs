//! A model's imports, given proper types.
//!
//! [`Import`] is a sum type whose variants are newtypes over the generated
//! `mm::ImportType` and `mm::ImportTypes` structs, selected from the node's
//! `$class`. Each is filled from exactly the fields the import is read for:
//! the namespace, the imported name or names, and the aliases. A `types` or
//! `aliasedTypes` entry of the wrong shape is skipped rather than rejected, as
//! it always has been, so these structs are built from the node's values
//! instead of by deserializing the whole node. A wildcard import
//! (`import ns.*`) is rejected while parsing, mirroring strict mode in
//! Concerto v4.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, Result};
use crate::introspect::{declared_class, qualified_class};
use crate::model_util::{get_fully_qualified_name, get_short_name};

/// A single import statement in a model file. Wildcard imports (`import ns.*`)
/// are rejected while parsing, mirroring strict mode in Concerto v4.
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
                    aliases(t)
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
            Self::Type(t) if t.name == short => {
                Some(get_fully_qualified_name(&t.namespace, &t.name))
            }
            Self::Type(_) => None,
            Self::Types(t) => {
                if let Some(aliased) = aliases(t)
                    .iter()
                    .find(|aliased| aliased.aliased_name == short)
                {
                    return Some(get_fully_qualified_name(&t.namespace, &aliased.name));
                }
                if t.types.iter().any(|n| n == short) {
                    return Some(get_fully_qualified_name(&t.namespace, short));
                }
                None
            }
        }
    }
}

/// The aliases of a multi-type import, or none.
fn aliases(import: &mm::ImportTypes) -> &[mm::AliasedType] {
    import.aliased_types.as_deref().unwrap_or(&[])
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
        let kind = get_short_name(class);

        let namespace = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConcertoError::IllegalModel {
                message: format!("import ({kind}) missing 'namespace'"),
                file_name: None,
                location: None,
            })?
            .to_string();
        let uri = value
            .get("uri")
            .and_then(|v| v.as_str())
            .map(str::to_string);

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
                Self::Type(mm::ImportType {
                    namespace,
                    uri,
                    name,
                })
            }
            "ImportTypes" => {
                // Entries of the wrong shape are skipped, not rejected.
                let types = value
                    .get("types")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let aliased_types = value
                    .get("aliasedTypes")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(aliased_type).collect())
                    .unwrap_or_default();
                Self::Types(mm::ImportTypes {
                    namespace,
                    uri,
                    types,
                    aliased_types: Some(aliased_types),
                })
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

/// Reads one `aliasedTypes` entry, or `None` if it lacks a string `name` or
/// `aliasedName`. An entry with no `$class` of its own is still an alias, and
/// is given the metamodel's.
fn aliased_type(entry: &serde_json::Value) -> Option<mm::AliasedType> {
    let aliased_name = entry.get("aliasedName").and_then(|v| v.as_str())?;
    let name = entry.get("name").and_then(|v| v.as_str())?;
    let class = match declared_class(entry) {
        "" => qualified_class("AliasedType"),
        class => class.to_string(),
    };
    Some(mm::AliasedType {
        _class: class,
        name: name.to_string(),
        aliased_name: aliased_name.to_string(),
    })
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
        assert!(err.unwrap_err().to_string().contains("$class"));
    }

    #[test]
    fn missing_class_is_reported_verbatim() {
        let err = Import::try_from(&serde_json::json!({ "namespace": "org.acme@1.0.0" }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: import node is missing its $class"
        );
    }

    #[test]
    fn unknown_import_kind_errors() {
        let err = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryImport",
            "namespace": "org.acme@1.0.0"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: unknown import type: MysteryImport"
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
    fn an_alias_with_no_class_is_still_an_alias() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.acme@1.0.0",
            "types": ["A", "B"],
            "aliasedTypes": [{ "name": "B", "aliasedName": "Bee" }]
        }))
        .unwrap();
        assert_eq!(imp.resolve("Bee").as_deref(), Some("org.acme@1.0.0.B"));
        assert_eq!(imp.local_names(), ["A", "Bee"]);
    }

    #[test]
    fn non_string_types_and_malformed_aliases_are_skipped() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.acme@1.0.0",
            "types": ["A", 3, { "name": "C" }],
            "aliasedTypes": [{ "name": "A" }, 7, { "name": "A", "aliasedName": "Ay" }]
        }))
        .unwrap();
        assert_eq!(imp.imported_names(), ["A"]);
        assert_eq!(imp.resolve("Ay").as_deref(), Some("org.acme@1.0.0.A"));
    }

    #[test]
    fn types_or_aliases_that_are_not_arrays_are_empty() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.acme@1.0.0",
            "types": "A",
            "aliasedTypes": {}
        }))
        .unwrap();
        assert!(imp.imported_names().is_empty());
        assert_eq!(imp.resolve("A"), None);
    }

    #[test]
    fn a_non_string_uri_is_ignored() {
        let imp = Import::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": "org.acme@1.0.0",
            "name": "Person",
            "uri": 5
        }))
        .unwrap();
        assert_eq!(
            imp.resolve("Person").as_deref(),
            Some("org.acme@1.0.0.Person")
        );
    }
}

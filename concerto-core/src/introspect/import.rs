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
use concerto_metamodel::utils::class_name;

use crate::error::{Error, Result};
use crate::introspect::declared_class;
use crate::model_util::{qualify, short_name};

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

    /// The URI this import was given (`import ns.Name from 'uri'`), if any.
    pub fn uri(&self) -> Option<&str> {
        match self {
            Self::Type(t) => t.uri.as_deref(),
            Self::Types(t) => t.uri.as_deref(),
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
    ///
    /// TS: `ModelFile.fromAst`'s `importShortNames` map (modelfile.ts) sets
    /// one local name per imported type: the alias when the type has one, the
    /// declared name otherwise (`this.importShortNames.set(alias ?? type,
    /// ...)`). An aliased type's *declared* name is never also registered, so
    /// it does not resolve under it — P2-08 review carry-over (a) from
    /// P2-04's review (#48): this used to check `t.types` unconditionally
    /// after the alias check, so an aliased import's original name still
    /// resolved.
    pub fn resolve(&self, short: &str) -> Option<String> {
        match self {
            Self::Type(t) if t.name == short => Some(qualify(&t.namespace, &t.name)),
            Self::Type(_) => None,
            Self::Types(t) => {
                let aliased = aliases(t);
                t.types.iter().find_map(|name| {
                    let local_name = aliased
                        .iter()
                        .find(|a| &a.name == name)
                        .map_or(name.as_str(), |a| a.aliased_name.as_str());
                    (local_name == short).then(|| qualify(&t.namespace, name))
                })
            }
        }
    }
}

/// The aliases of a multi-type import, or none.
fn aliases(import: &mm::ImportTypes) -> &[mm::AliasedType] {
    import.aliased_types.as_deref().unwrap_or(&[])
}

impl Import {
    /// The `aliasedTypes` this import declares, or an empty slice for a
    /// single-type import (which has no `aliasedTypes` field at all) or a
    /// multi-type import with none.
    pub fn aliased_types(&self) -> &[mm::AliasedType] {
        match self {
            Self::Type(_) => &[],
            Self::Types(t) => aliases(t),
        }
    }
}

impl TryFrom<&serde_json::Value> for Import {
    type Error = Error;

    fn try_from(value: &serde_json::Value) -> Result<Self> {
        let class = declared_class(value);
        if class.is_empty() {
            return Err(Error::illegal_model(
                "import node is missing its $class",
                None,
                None,
            ));
        }
        let kind = short_name(class);

        let namespace = value
            .get("namespace")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::illegal_model(format!("import ({kind}) missing 'namespace'"), None, None)
            })?
            .to_string();
        let uri = value
            .get("uri")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        Ok(match kind {
            // Concerto v4 disallows wildcard imports; reject them up front.
            //
            // TS: `ModelFile.fromAst`'s `ImportAll` arm (modelfile.ts) throws
            // a plain `Error('Wildcard Imports are not permitted.')` — not an
            // `IllegalModelException` (no model file, no location, no
            // "clashes"/"unrecognized" catalogue wording), and the message
            // does not name the namespace.
            "ImportAll" => {
                return Err(crate::error::ContractError::pre_port(
                    crate::error::ErrorKind::InvalidArgument,
                    "Wildcard Imports are not permitted.".to_string(),
                    None,
                )
                .into());
            }
            "ImportType" => {
                let name = value
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| Error::illegal_model("ImportType missing 'name'", None, None))?
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
                return Err(Error::illegal_model(
                    format!("unknown import type: {other}"),
                    None,
                    None,
                ));
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
        "" => "concerto.metamodel@1.0.0.AliasedType".into(),
        class => class_name(class),
    };
    Some(mm::AliasedType {
        _class: class,
        name: name.to_string(),
        aliased_name: aliased_name.to_string(),
    })
}

#[cfg(test)]
mod tests;

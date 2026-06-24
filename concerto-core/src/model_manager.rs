//! Loads model files and resolves types across namespaces.
//!
//! The [`ModelManager`] is the only stateful object in the core. It owns the
//! loaded [`ModelFile`]s and provides the operations that need to see more than
//! one namespace at once: resolving a type (local or imported),
//! collecting every property along an inheritance chain, and checking whether
//! one type is assignable to another. Keeping that state here lets the
//! validation layer remain a function over already-resolved model state.

use std::collections::{HashMap, HashSet};

use crate::error::{ConcertoError, Result};
use crate::introspect::declaration::{ClassDeclaration, Declaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::model_util::{namespace_of, parse_namespace, qualify, short_name};
use crate::rootmodel::root_model_ast;

/// Owns a set of model files and resolves types across them.
#[derive(Debug, Default)]
pub struct ModelManager {
    model_files: HashMap<String, ModelFile>,
}

impl ModelManager {
    /// A fresh manager with the `concerto@1.0.0` system model already loaded.
    pub fn new() -> Result<Self> {
        let mut mgr = Self::default();
        let root = ModelFile::from_json(&root_model_ast(), Some("concerto@1.0.0".into()))?;
        mgr.model_files.insert(root.namespace().to_string(), root);
        Ok(mgr)
    }

    /// Loads a model from its JSON AST. Loading two models with the same
    /// namespace is an error.
    pub fn add_model(
        &mut self,
        value: &serde_json::Value,
        file_name: Option<String>,
    ) -> Result<()> {
        let mf = ModelFile::from_json(value, file_name)?;
        let ns = mf.namespace().to_string();
        if self.model_files.contains_key(&ns) {
            return Err(ConcertoError::IllegalModel {
                message: format!("duplicate namespace: {ns}"),
                file_name: mf.file_name().map(str::to_string),
                location: None,
            });
        }
        self.model_files.insert(ns, mf);
        Ok(())
    }

    /// The loaded model file for a namespace, if there is one.
    pub fn model_file(&self, namespace: &str) -> Option<&ModelFile> {
        self.model_files.get(namespace)
    }

    /// Looks up a declaration by its fully-qualified name.
    ///
    /// Matching ignores the namespace version, so `org.acme@1.0.0.Foo` and
    /// `org.acme.Foo` resolve to the same declaration.
    pub fn get_declaration(&self, fqn: &str) -> Result<&Declaration> {
        let ns = namespace_of(fqn);
        let short = short_name(fqn);

        if let Some(mf) = self.model_files.get(ns)
            && let Some(decl) = mf.local_declaration(short)
        {
            return Ok(decl);
        }

        let target_ns = bare_namespace(ns);
        self.model_files
            .values()
            .filter(|mf| bare_namespace(mf.namespace()) == target_ns)
            .find_map(|mf| mf.local_declaration(short))
            .ok_or_else(|| ConcertoError::TypeNotFound {
                type_name: fqn.to_string(),
            })
    }

    /// Resolves a short name, as written inside `in_namespace`, to its
    /// fully-qualified name, using the primitives, local declarations and named
    /// imports the model file can see.
    pub fn resolve_type_name(&self, in_namespace: &str, short: &str) -> Result<String> {
        let mf =
            self.model_files
                .get(in_namespace)
                .ok_or_else(|| ConcertoError::NamespaceNotFound {
                    namespace: in_namespace.to_string(),
                })?;

        mf.resolve_local(short)
            .ok_or_else(|| ConcertoError::TypeNotFound {
                type_name: qualify(in_namespace, short),
            })
    }

    /// Every property of a type, gathered by walking from the type up through
    /// all of its super types. Returns an error if the name is not a
    /// concept-like type, a super type cannot be resolved, or the inheritance
    /// chain is circular.
    pub fn get_all_properties(&self, fqn: &str) -> Result<Vec<&Property>> {
        Ok(self
            .super_chain(fqn)?
            .into_iter()
            .flat_map(|(_, class)| class.own_properties())
            .collect())
    }

    /// Returns `true` if a value of `sub_fqn` is also a valid `super_fqn`: the
    /// two are the same type, or `sub_fqn` transitively extends `super_fqn`.
    pub fn is_assignable_to(&self, sub_fqn: &str, super_fqn: &str) -> Result<bool> {
        let target = bare_fqn(super_fqn);
        if bare_fqn(sub_fqn) == target {
            return Ok(true);
        }
        match self.get_declaration(sub_fqn)?.as_class() {
            None => Ok(false),
            Some(_) => Ok(self
                .super_chain(sub_fqn)?
                .iter()
                .any(|(fqn, _)| bare_fqn(fqn) == target)),
        }
    }

    /// Walks a class's inheritance chain, handing back each
    /// `(full-name, declaration)` pair from the type up to its root.
    fn super_chain(&self, fqn: &str) -> Result<Vec<(String, &ClassDeclaration)>> {
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut current = fqn.to_string();

        loop {
            if !visited.insert(bare_fqn(&current)) {
                return Err(ConcertoError::IllegalModel {
                    message: format!("circular inheritance detected at {current}"),
                    file_name: None,
                    location: None,
                });
            }

            let class = self.get_declaration(&current)?.as_class().ok_or_else(|| {
                ConcertoError::IllegalModel {
                    message: format!("{current} is not a concept-like declaration"),
                    file_name: None,
                    location: None,
                }
            })?;

            let next = self.super_type_fqn(class, namespace_of(&current))?;
            chain.push((current, class));
            match next {
                Some(parent) => current = parent,
                None => break,
            }
        }

        Ok(chain)
    }

    /// Works out the full name of a class's direct super type, resolved in the
    /// namespace where the class is declared.
    fn super_type_fqn(
        &self,
        class: &ClassDeclaration,
        in_namespace: &str,
    ) -> Result<Option<String>> {
        let Some(ti) = class.super_type() else {
            return Ok(None);
        };
        if let Some(ns) = &ti.namespace {
            return Ok(Some(qualify(ns, &ti.name)));
        }
        if let Some(resolved) = &ti.resolved_name {
            return Ok(Some(resolved.clone()));
        }
        Ok(Some(self.resolve_type_name(in_namespace, &ti.name)?))
    }
}

/// Drops the `@version` off a namespace.
fn bare_namespace(namespace: &str) -> String {
    parse_namespace(namespace)
        .map(|p| p.name)
        .unwrap_or_else(|_| namespace.to_string())
}

/// Strips the version off a name's namespace, so two names compare equal
/// whether or not they were written with one.
fn bare_fqn(fqn: &str) -> String {
    let ns = namespace_of(fqn);
    if ns.is_empty() {
        return fqn.to_string();
    }
    qualify(&bare_namespace(ns), short_name(fqn))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `org.example@1.0.0` with Person ← Employee ← Manager and an enum.
    fn manager() -> ModelManager {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Employee", "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "salary", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Manager", "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Employee" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "title", "isArray": false, "isOptional": true }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "Color",
                      "properties": [ { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" } ] }
                ]
            }),
            None,
        )
        .unwrap();
        mgr
    }

    #[test]
    fn preloads_system_model() {
        let mgr = ModelManager::new().unwrap();
        assert!(mgr.get_declaration("concerto@1.0.0.Concept").is_ok());
        assert!(mgr.get_declaration("concerto@1.0.0.Asset").is_ok());
    }

    #[test]
    fn duplicate_namespace_rejected() {
        let mut mgr = ModelManager::new().unwrap();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.x@1.0.0", "declarations": []
        });
        mgr.add_model(&model, None).unwrap();
        assert!(mgr.add_model(&model, None).is_err());
    }

    #[test]
    fn resolves_local_and_version_insensitive() {
        let mgr = manager();
        assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
        // version-insensitive lookup
        assert!(mgr.get_declaration("org.example.Manager").is_ok());
        assert!(mgr.get_declaration("org.example@1.0.0.Nope").is_err());
    }

    #[test]
    fn collects_inherited_properties_in_order() {
        let mgr = manager();
        let props = mgr.get_all_properties("org.example@1.0.0.Manager").unwrap();
        let names: Vec<&str> = props.iter().map(|p| p.name()).collect();
        // Manager's own first, then Employee, then Person up the chain.
        assert_eq!(names, ["title", "salary", "name"]);
    }

    #[test]
    fn assignability_follows_inheritance() {
        let mgr = manager();
        assert!(
            mgr.is_assignable_to("org.example@1.0.0.Manager", "org.example@1.0.0.Person")
                .unwrap()
        );
        assert!(
            mgr.is_assignable_to("org.example@1.0.0.Manager", "org.example@1.0.0.Manager")
                .unwrap()
        );
        assert!(
            !mgr.is_assignable_to("org.example@1.0.0.Person", "org.example@1.0.0.Manager")
                .unwrap()
        );
    }

    #[test]
    fn unresolved_super_type_is_hard_error() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.broken@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Orphan", "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Ghost" },
                      "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
        assert!(mgr.get_all_properties("org.broken@1.0.0.Orphan").is_err());
    }

    #[test]
    fn get_all_properties_on_enum_errors() {
        let mgr = manager();
        assert!(mgr.get_all_properties("org.example@1.0.0.Color").is_err());
    }
}

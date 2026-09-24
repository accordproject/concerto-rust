//! Loads model files and resolves types across namespaces.
//!
//! The [`ModelManager`] is the only stateful object in the core. It owns the
//! loaded [`ModelFile`]s and provides the operations that need to see more than
//! one namespace at once: resolving a type (local or imported),
//! collecting every property along an inheritance chain, and checking whether
//! one type is assignable to another. Keeping that state here lets the
//! validation layer remain a function over already-resolved model state.

use std::collections::{HashMap, HashSet};

use crate::error::{ConcertoError, ContractError, Result};
use crate::introspect::FullyQualified;
// `name()` moved from the property type to the `Named` trait; the test module
// below reaches it through `use super::*`.
#[cfg(test)]
use crate::introspect::Named;
use crate::introspect::declaration::{ClassDeclaration, Declaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::model_util::{get_fully_qualified_name, get_namespace, get_short_name};
use crate::rootmodel::root_model_ast;

/// The namespace part of a fully-qualified name, `""` when there is none.
fn namespace_of(fqn: &str) -> &str {
    // An empty name has no namespace; the loader looks it up and fails.
    get_namespace(Some(fqn)).unwrap_or_default()
}

/// The collaborator calls a ported member makes (PORTING.md 1.4).
///
/// In TS, some members call other model objects: a model file, the model
/// manager, a parent declaration. The Rust port makes each such call through
/// this trait, so that core never knows whether it is talking to the arena or
/// to JS objects. Each method mirrors the TS method it replaces, with the same
/// name in snake case and the same failure.
///
/// P0-04b trial: this holds only the calls the three trial units make. P1-04
/// owns the trait and its arena implementation (with `DeclId`/`PropId`
/// handles); until then the implementations are the WASM binding's
/// JS-callback context and the native oracle harness's.
pub trait ResolutionContext {
    /// A handle to a model element: a model file, a declaration or a property.
    type Node;
    /// What a collaborator call can raise. The JS-callback context carries the
    /// JS exception through unchanged.
    type Error: From<ContractError>;

    /// TS: ModelFile.getType (src/introspect/modelfile.ts). `None` is a
    /// nullish result.
    fn get_type(
        &self,
        model_file: &Self::Node,
        type_name: &str,
    ) -> std::result::Result<Option<Self::Node>, Self::Error>;

    /// TS: ClassDeclaration.getAllSuperTypeDeclarations (src/introspect/classdeclaration.ts)
    fn get_all_super_type_declarations(
        &self,
        declaration: &Self::Node,
    ) -> std::result::Result<Vec<Self::Node>, Self::Error>;

    /// TS: Declaration.getFullyQualifiedName (src/introspect/declaration.ts)
    fn get_fully_qualified_name(
        &self,
        declaration: &Self::Node,
    ) -> std::result::Result<String, Self::Error>;

    /// TS: Property.getFullyQualifiedTypeName (src/introspect/property.ts)
    fn get_fully_qualified_type_name(
        &self,
        property: &Self::Node,
    ) -> std::result::Result<String, Self::Error>;

    /// TS: Property.getParent (src/introspect/property.ts)
    fn get_parent(&self, property: &Self::Node) -> std::result::Result<Self::Node, Self::Error>;

    /// TS: Declaration.getModelFile (src/introspect/declaration.ts)
    fn get_model_file(
        &self,
        declaration: &Self::Node,
    ) -> std::result::Result<Self::Node, Self::Error>;

    /// TS: Property.getType (src/introspect/property.ts)
    fn get_type_name(&self, property: &Self::Node) -> std::result::Result<String, Self::Error>;

    /// TS: Declaration.isEnum (src/introspect/declaration.ts)
    fn is_enum(&self, declaration: &Self::Node) -> std::result::Result<bool, Self::Error>;

    /// TS: `declaration.isMapDeclaration?.()`; `None` when the method is
    /// missing.
    fn is_map_declaration(
        &self,
        declaration: &Self::Node,
    ) -> std::result::Result<Option<bool>, Self::Error>;

    /// TS: `declaration.isScalarDeclaration?.()`; `None` when the method is
    /// missing.
    fn is_scalar_declaration(
        &self,
        declaration: &Self::Node,
    ) -> std::result::Result<Option<bool>, Self::Error>;

    /// TS: `declaration.ast.$class`; `None` when it is not a string.
    fn get_ast_class(
        &self,
        declaration: &Self::Node,
    ) -> std::result::Result<Option<String>, Self::Error>;

    /// TS: ModelFile.getAllDeclarations (src/introspect/modelfile.ts)
    fn get_all_declarations(
        &self,
        model_file: &Self::Node,
    ) -> std::result::Result<Vec<Self::Node>, Self::Error>;
}

/// The field or scalar declaration a validator is attached to, as a validator
/// reads it (TS: `Validator.field`, typed `Property | ScalarDeclaration`).
///
/// P0-04b trial: a validator is built both by the Rust loader (for a scalar
/// declaration it is constructing) and, through the WASM binding, over a JS
/// field that may be a sinon stub (the `NumberValidator` constructor is a
/// `needs_fallback` row). P1-04 decides whether this folds into
/// [`ResolutionContext`].
///
/// Its [`FullyQualified`] name is TS
/// `this.getFieldOrScalarDeclaration().getFullyQualifiedName()`, read only
/// when an error is reported; its `Error` is what reading the element can
/// raise.
pub trait ValidatedElement: FullyQualified {
    /// TS: `this.field?.ast?.defaultValue`; `None` is `undefined`.
    fn default_value(&self) -> std::result::Result<Option<serde_json::Value>, Self::Error>;
}

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
    // TODO: The corresponding method in TS implementation accepts a CTO string and parses it.
    // since we don't have a parser in this implementation, this shoul dbe `add_model_file`,
    // or `add_model_ast` together with `add_model_file` that accepts `ModelFile` instance.
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

    /// Every loaded model file, including the built-in system model. The order
    /// is unspecified.
    pub fn model_files(&self) -> impl Iterator<Item = &ModelFile> {
        self.model_files.values()
    }

    /// Looks up a declaration by its fully-qualified name.
    ///
    /// Namespace versions are mandatory in Concerto v4, so the lookup is
    /// exact: the name must be written with the versioned namespace it was
    /// declared in.
    pub fn get_declaration(&self, fqn: &str) -> Result<&Declaration> {
        self.model_files
            .get(namespace_of(fqn))
            .and_then(|mf| mf.local_declaration(get_short_name(fqn)))
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

        mf.resolve_local_type(short)
            .ok_or_else(|| ConcertoError::TypeNotFound {
                type_name: get_fully_qualified_name(in_namespace, short),
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
        if sub_fqn == super_fqn {
            return Ok(true);
        }
        match self.get_declaration(sub_fqn)?.as_class() {
            None => Ok(false),
            Some(_) => Ok(self
                .super_chain(sub_fqn)?
                .iter()
                .any(|(fqn, _)| fqn == super_fqn)),
        }
    }

    /// Walks a class's inheritance chain, handing back each
    /// `(full-name, declaration)` pair from the type up to its root.
    fn super_chain(&self, fqn: &str) -> Result<Vec<(String, &ClassDeclaration)>> {
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut current = fqn.to_string();

        loop {
            if !visited.insert(current.clone()) {
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
            return Ok(Some(get_fully_qualified_name(ns, &ti.name)));
        }
        if let Some(resolved) = &ti.resolved_name {
            return Ok(Some(resolved.clone()));
        }
        Ok(Some(self.resolve_type_name(in_namespace, &ti.name)?))
    }
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
    fn resolves_by_exact_fqn_only() {
        let mgr = manager();
        assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
        // versions are mandatory, so an unversioned lookup does not resolve
        assert!(mgr.get_declaration("org.example.Manager").is_err());
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

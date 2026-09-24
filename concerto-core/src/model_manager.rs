//! Loads model files and resolves types across namespaces.
//!
//! The [`ModelManager`] is the only stateful object in the core. It owns the
//! loaded [`ModelFile`]s and provides the operations that need to see more than
//! one namespace at once: resolving a type (local or imported),
//! collecting every property along an inheritance chain, and checking whether
//! one type is assignable to another. Keeping that state here lets the
//! validation layer remain a function over already-resolved model state.
//!
//! # The graph and its handles
//!
//! The manager owns the model graph (plan §3, "Rust owns the graph"). Its
//! model files, declarations and properties sit in an append-only arena and
//! are addressed by dense `u32` handles: [`ModelFileId`], [`DeclId`] and
//! [`PropId`]. A handle keeps naming the same element for the life of the
//! manager, across every later call and mutation: loading a model only
//! appends, and a handle is never reused. Looking an element up by its handle
//! is an index into the arena, never a string hash. [`ModelManager::generation`]
//! counts the mutations, so that a binding caching a snapshot of an element
//! knows when to drop it (spike input on #41; PORTING.md 1.5).
//!
//! Any future removal (P1-06's rollback, `deleteModelFile`,
//! `clearModelFiles`) must leave a tombstone rather than shift the arena, so
//! that the handles of the elements that stay remain valid.
//!
//! A ported member reaches its collaborators through the
//! [`ResolutionContext`] trait. The manager implements it over the arena, with
//! [`Node`] as its handle; `concerto-wasm` implements it over JS objects, for
//! views a white-box test builds over stubbed collaborators (PORTING.md 1.4).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use serde_json::Value;

use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::declaration::{ClassDeclaration, Declaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::introspect::{FullyQualified, Named, Typed};
use crate::model_util::{
    PRIMITIVE_TYPES, get_fully_qualified_name, get_namespace, get_short_name, is_primitive_type,
};
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
/// There are two implementations:
///
/// - [`ModelManager`], over its arena, with [`Node`] as the handle. This is
///   the real one: the Rust engine owns the graph.
/// - The JS-callback context in `concerto-wasm`, with a `JsValue` as the
///   handle, for the collaborator fallback: a view that a white-box test builds
///   over a stubbed collaborator, with no Rust-backed parent, resolves its
///   collaborator calls by calling that collaborator back.
///
/// The methods are only those a port needs; a port that needs another adds
/// it, naming the TS call it replaces.
pub trait ResolutionContext {
    /// A handle to a model element: a model file, a declaration or a property.
    type Node;
    /// What a collaborator call can raise. The JS-callback context carries the
    /// JS exception through unchanged.
    type Error: From<ContractError>;

    /// TS: ModelFile.getType (src/introspect/modelfile.ts). `type_name` is
    /// `None` when TS passes `null` or `undefined`; the result is `None` when
    /// TS returns a nullish value.
    fn get_type(
        &self,
        model_file: &Self::Node,
        type_name: Option<&str>,
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

    /// TS: Property.getType (src/introspect/property.ts). `None` is a nullish
    /// type, as an enum value has.
    fn get_type_name(
        &self,
        property: &Self::Node,
    ) -> std::result::Result<Option<String>, Self::Error>;

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
/// This stays its own trait rather than [`ResolutionContext`] methods on a
/// node (OD-12, settled in P1-04): TS builds a validator while it is
/// constructing the element the validator is attached to
/// (`ScalarDeclaration.process` runs in the constructor), before that element
/// is in the arena and has a handle. The validator reads only that one
/// element (PORTING.md 1.1, rule 9).
///
/// Its [`FullyQualified`] name is TS
/// `this.getFieldOrScalarDeclaration().getFullyQualifiedName()`, read only
/// when an error is reported; its `Error` is what reading the element can
/// raise.
pub trait ValidatedElement: FullyQualified {
    /// TS: `this.field?.ast?.defaultValue`; `None` is `undefined`.
    fn default_value(&self) -> std::result::Result<Option<serde_json::Value>, Self::Error>;
}

/// Declares a handle type: a dense `u32` index into one of the arena's
/// tables.
macro_rules! handle {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u32);

        impl $name {
            /// The handle with this raw index, as a binding gets it back from
            /// JS. An index the manager never handed out names nothing: every
            /// lookup of it answers `None` or an error.
            pub const fn from_index(index: u32) -> Self {
                Self(index)
            }

            /// The raw index, as a binding passes it to JS (a plain number).
            pub const fn index(self) -> u32 {
                self.0
            }

            /// The position in the arena table this handle indexes.
            fn slot(self) -> usize {
                self.0 as usize
            }
        }
    };
}

handle! {
    /// A handle to a model file loaded into a [`ModelManager`].
    ModelFileId
}

handle! {
    /// A handle to a declaration of a model file loaded into a
    /// [`ModelManager`].
    DeclId
}

handle! {
    /// A handle to a property of a class declaration loaded into a
    /// [`ModelManager`]. The values of an enum declaration have no handle
    /// yet: they are not [`Property`] values until P2-04 ports them.
    PropId
}

/// A node of the model graph, as the manager's [`ResolutionContext`] hands
/// it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    /// A loaded model file.
    ModelFile(ModelFileId),
    /// A declaration.
    Declaration(DeclId),
    /// A property of a class declaration.
    Property(PropId),
    /// A primitive type name. `ModelFile.getType` answers a primitive type
    /// with its name, a JS string, rather than a declaration.
    Primitive(&'static str),
}

/// A loaded model file, and the handles of its declarations.
#[derive(Debug)]
struct FileSlot {
    model_file: ModelFile,
    declarations: Range<u32>,
}

/// Where a declaration is: its model file, its position in
/// [`ModelFile::declarations`], and the handles of its properties.
#[derive(Debug)]
struct DeclSlot {
    model_file: ModelFileId,
    index: usize,
    properties: Range<u32>,
}

/// Where a property is: its declaration, and its position in
/// [`ClassDeclaration::own_properties`].
#[derive(Debug)]
struct PropSlot {
    declaration: DeclId,
    index: usize,
}

/// Owns a set of model files and resolves types across them.
#[derive(Debug, Default)]
pub struct ModelManager {
    files: Vec<FileSlot>,
    namespaces: HashMap<String, ModelFileId>,
    declarations: Vec<DeclSlot>,
    properties: Vec<PropSlot>,
    generation: u64,
}

/// The next handle of an arena table holding `len` entries.
///
/// A full arena is a Rust-only failure (PORTING.md 2.3): TS keeps its model
/// graph in unbounded JS arrays and objects, so no TS class, message or
/// fixture corresponds to it. It keeps the nearest existing variant,
/// `ConcertoError::IllegalModel` (the model cannot be loaded), with no
/// catalogue entry, rather than a new `ErrorKind`, which 2.3 forbids when no
/// TS class matches. Four billion elements is not a model anyone loads, but
/// the boundary path must not panic.
fn next_index(len: usize) -> Result<u32> {
    u32::try_from(len).map_err(|_| ConcertoError::IllegalModel {
        message: "the model manager cannot address any more elements".into(),
        file_name: None,
        location: None,
    })
}

/// V8's `TypeError` for calling a method the receiver does not have.
/// `expression` is the one the JS-callback context names for the same call.
fn not_a_function(expression: &str) -> ConcertoError {
    ContractError::new(
        ErrorKind::JsTypeError,
        "engine-typeerror-notafunction",
        vec![("expression", expression.to_string())],
    )
    .into()
}

/// A handle this manager never handed out.
///
/// A stale or foreign handle is a Rust-only failure (PORTING.md 2.3): TS
/// passes object references, which cannot dangle or belong to another
/// manager, so no TS class, message or fixture corresponds to it, and it
/// can only arise from a bug in a caller holding handles (the binding, a
/// harness). It keeps the nearest existing variant,
/// `ConcertoError::TypeNotFound` (the handle names no element), with no
/// catalogue entry, rather than a new `ErrorKind`, which 2.3 forbids when no
/// TS class matches.
fn unknown(node: Node) -> ConcertoError {
    ConcertoError::TypeNotFound {
        type_name: format!("{node:?}"),
    }
}

/// The fully qualified name a short name is imported as, if it is imported.
/// Later imports overwrite earlier ones, as `Map.set` does.
///
/// TS: ModelFile.isImportedType, ModelFile.resolveImport
/// (src/introspect/modelfile.ts), over the `importShortNames` map that
/// ModelFile.fromAst builds
fn imported_type(model_file: &ModelFile, type_name: &str) -> Option<String> {
    model_file.imports().iter().rev().find_map(|import| {
        import
            .local_names()
            .into_iter()
            .zip(import.imported_names())
            .rfind(|(local, _)| *local == type_name)
            .map(|(_, imported)| get_fully_qualified_name(import.namespace(), imported))
    })
}

impl ModelManager {
    /// A fresh manager with the `concerto@1.0.0` system model already loaded.
    pub fn new() -> Result<Self> {
        let mut mgr = Self::default();
        let root = ModelFile::from_json(&root_model_ast(), Some("concerto@1.0.0".into()))?;
        mgr.insert(root)?;
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
        if self.namespaces.contains_key(mf.namespace()) {
            return Err(ConcertoError::IllegalModel {
                message: format!("duplicate namespace: {}", mf.namespace()),
                file_name: mf.file_name().map(str::to_string),
                location: None,
            });
        }
        self.insert(mf)?;
        Ok(())
    }

    /// Appends a model file, its declarations and their properties to the
    /// arena, and counts the mutation. Nothing is changed if a handle cannot
    /// be allocated.
    fn insert(&mut self, model_file: ModelFile) -> Result<ModelFileId> {
        let file_id = ModelFileId(next_index(self.files.len())?);
        let mut declarations = Vec::new();
        let mut properties = Vec::new();
        for (index, declaration) in model_file.declarations().iter().enumerate() {
            let decl_id = DeclId(next_index(self.declarations.len() + declarations.len())?);
            let first = next_index(self.properties.len() + properties.len())?;
            let own = declaration
                .as_class()
                .map_or(&[][..], ClassDeclaration::own_properties);
            properties.extend((0..own.len()).map(|index| PropSlot {
                declaration: decl_id,
                index,
            }));
            let end = next_index(self.properties.len() + properties.len())?;
            declarations.push(DeclSlot {
                model_file: file_id,
                index,
                properties: first..end,
            });
        }
        let first = next_index(self.declarations.len())?;
        let end = next_index(self.declarations.len() + declarations.len())?;

        self.namespaces
            .insert(model_file.namespace().to_string(), file_id);
        self.files.push(FileSlot {
            model_file,
            declarations: first..end,
        });
        self.declarations.extend(declarations);
        self.properties.extend(properties);
        self.generation += 1;
        Ok(file_id)
    }

    /// A counter that every mutation of the manager increases. A snapshot of
    /// an element taken at one generation is current while the generation is
    /// unchanged.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The loaded model file for a namespace, if there is one.
    pub fn model_file(&self, namespace: &str) -> Option<&ModelFile> {
        self.model_file_id(namespace).and_then(|id| self.file(id))
    }

    /// Every loaded model file, including the built-in system model, in the
    /// order they were loaded.
    pub fn model_files(&self) -> impl Iterator<Item = &ModelFile> {
        self.files.iter().map(|slot| &slot.model_file)
    }

    /// The handle of the loaded model file for a namespace, if there is one.
    pub fn model_file_id(&self, namespace: &str) -> Option<ModelFileId> {
        self.namespaces.get(namespace).copied()
    }

    /// The handle of a declaration, by its fully-qualified name. The lookup is
    /// exact, as [`ModelManager::get_declaration`]'s is.
    pub fn declaration_id(&self, fqn: &str) -> Option<DeclId> {
        self.local_type(self.model_file_id(namespace_of(fqn))?, get_short_name(fqn))
    }

    /// The model file a handle names.
    pub fn file(&self, id: ModelFileId) -> Option<&ModelFile> {
        self.files.get(id.slot()).map(|slot| &slot.model_file)
    }

    /// The declaration a handle names.
    pub fn declaration(&self, id: DeclId) -> Option<&Declaration> {
        let slot = self.declarations.get(id.slot())?;
        self.file(slot.model_file)?.declarations().get(slot.index)
    }

    /// The property a handle names.
    pub fn property(&self, id: PropId) -> Option<&Property> {
        let slot = self.properties.get(id.slot())?;
        self.declaration(slot.declaration)?
            .as_class()?
            .own_properties()
            .get(slot.index)
    }

    /// The handles of a model file's declarations, in the order they appear
    /// in the file. None for a handle the manager never handed out.
    pub fn declaration_ids(&self, file: ModelFileId) -> impl Iterator<Item = DeclId> + use<> {
        self.files
            .get(file.slot())
            .map_or(0..0, |slot| slot.declarations.clone())
            .map(DeclId)
    }

    /// The handles of a class declaration's own properties, in the order they
    /// are declared. None for any other declaration, or for a handle the
    /// manager never handed out.
    pub fn property_ids(&self, declaration: DeclId) -> impl Iterator<Item = PropId> + use<> {
        self.declarations
            .get(declaration.slot())
            .map_or(0..0, |slot| slot.properties.clone())
            .map(PropId)
    }

    /// The model file a declaration belongs to.
    ///
    /// TS: Declaration.getModelFile (src/introspect/declaration.ts)
    pub fn model_file_of(&self, declaration: DeclId) -> Option<ModelFileId> {
        self.declarations
            .get(declaration.slot())
            .map(|slot| slot.model_file)
    }

    /// The declaration a property belongs to.
    ///
    /// TS: Property.getParent (src/introspect/property.ts)
    pub fn parent_of(&self, property: PropId) -> Option<DeclId> {
        self.properties
            .get(property.slot())
            .map(|slot| slot.declaration)
    }

    /// Looks up a declaration by its fully-qualified name.
    ///
    /// Namespace versions are mandatory in Concerto v4, so the lookup is
    /// exact: the name must be written with the versioned namespace it was
    /// declared in.
    pub fn get_declaration(&self, fqn: &str) -> Result<&Declaration> {
        self.declaration_id(fqn)
            .and_then(|id| self.declaration(id))
            .ok_or_else(|| ConcertoError::TypeNotFound {
                type_name: fqn.to_string(),
            })
    }

    /// Resolves a short name, as written inside `in_namespace`, to its
    /// fully-qualified name, using the primitives, local declarations and named
    /// imports the model file can see.
    ///
    /// `location` is the AST node's `location`, copied verbatim into the
    /// error this raises when the namespace is not registered (PORTING.md
    /// section 2.1); pass `None` where the caller has no AST node in scope.
    pub fn resolve_type_name(
        &self,
        in_namespace: &str,
        short: &str,
        location: Option<serde_json::Value>,
    ) -> Result<String> {
        let mf = self.model_file(in_namespace).ok_or_else(|| {
            // TS: BaseModelManager.getType's unregistered-namespace path
            // (src/basemodelmanager.ts), reused for the equivalent check
            // here (error/catalogue.rs doc comment on the entry).
            let fqn = get_fully_qualified_name(in_namespace, short);
            ContractError::type_not_found(
                "modelmanager-gettype-noregisteredns",
                vec![("type", fqn.clone())],
                fqn,
                location,
            )
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
        // TS: ClassDeclaration._resolveSuperType passes `this.ast.location`
        // to every error it raises (src/introspect/classdeclaration.ts); the
        // class whose super type is being resolved is the AST node in scope
        // here, so its `location` is passed on, re-serialised from the typed
        // `mm::Range` by `location_value` (PORTING.md 2.1).
        let location = class.location().and_then(crate::error::location_value);
        Ok(Some(self.resolve_type_name(
            in_namespace,
            &ti.name,
            location,
        )?))
    }

    /// The handle of the declaration a model file's `getLocalType(type)`
    /// finds: `type` is prefixed with the file's namespace unless it already
    /// starts with it.
    ///
    /// TS: ModelFile.getLocalType (src/introspect/modelfile.ts)
    fn local_type(&self, file: ModelFileId, type_name: &str) -> Option<DeclId> {
        let slot = self.files.get(file.slot())?;
        let namespace = slot.model_file.namespace();
        let short = match type_name.strip_prefix(namespace) {
            // Keyed by `<namespace>.<name>`, so what follows the namespace
            // must be a dot and a name.
            Some(rest) => rest.strip_prefix('.')?,
            None => type_name,
        };
        let index = u32::try_from(slot.model_file.local_index(short)?).ok()?;
        Some(DeclId(slot.declarations.start.checked_add(index)?))
    }

    /// The fully qualified name of a declaration: its model file's namespace,
    /// then its name.
    ///
    /// TS: Declaration.getFullyQualifiedName (src/introspect/declaration.ts)
    fn declaration_fqn(&self, id: DeclId) -> Result<String> {
        let (Some(file), Some(declaration)) = (
            self.model_file_of(id).and_then(|file| self.file(file)),
            self.declaration(id),
        ) else {
            return Err(unknown(Node::Declaration(id)));
        };
        Ok(get_fully_qualified_name(
            file.namespace(),
            declaration.name(),
        ))
    }

    /// The AST node an element was built from, as TS keeps it in `ast`;
    /// `None` for a primitive type name, whose `ast` is `undefined`.
    fn ast(&self, node: Node) -> Result<Option<&Value>> {
        let found = match node {
            Node::ModelFile(id) => self.file(id).map(ModelFile::ast),
            Node::Declaration(id) => self.declaration_ast(id),
            Node::Property(id) => self.properties.get(id.slot()).and_then(|slot| {
                self.declaration_ast(slot.declaration)?
                    .get("properties")?
                    .get(slot.index)
            }),
            Node::Primitive(_) => return Ok(None),
        };
        found.map(Some).ok_or_else(|| unknown(node))
    }

    /// The AST node of a declaration, within its model file's AST.
    fn declaration_ast(&self, id: DeclId) -> Option<&Value> {
        let slot = self.declarations.get(id.slot())?;
        self.file(slot.model_file)?
            .ast()
            .get("declarations")?
            .get(slot.index)
    }
}

/// The manager answers collaborator calls from its own graph. A node of a
/// kind whose TS object has no such method answers V8's "is not a function"
/// `TypeError`, as the JS-callback context does for the same call; that is
/// what TS raises for a primitive type name that `ModelFile.getType`
/// returned. A handle this manager never handed out is an error.
///
/// The answers come from the loader's model state. Where that state is not
/// yet at parity with TS, so are the answers: the implicit `Concept` super
/// type (P2-03) is not in [`ResolutionContext::get_all_super_type_declarations`],
/// and super types resolve as the loader resolves them (P2-03, P2-08).
impl ResolutionContext for ModelManager {
    type Node = Node;
    type Error = ConcertoError;

    fn get_type(&self, model_file: &Node, type_name: Option<&str>) -> Result<Option<Node>> {
        let Node::ModelFile(file) = *model_file else {
            return Err(not_a_function("modelFile.getType"));
        };
        let mf = self.file(file).ok_or_else(|| unknown(*model_file))?;
        // A nullish type is not a primitive, not imported, and fails the
        // `type &&` of `isLocalType`.
        let Some(type_name) = type_name else {
            return Ok(None);
        };
        if let Some(&primitive) = PRIMITIVE_TYPES.iter().find(|&&p| p == type_name) {
            return Ok(Some(Node::Primitive(primitive)));
        }
        if let Some(fqn) = imported_type(mf, type_name) {
            // `getModelManager().getModelFile(getNamespace(fqn))`, then that
            // file's `getLocalType(fqn)`.
            return Ok(self
                .model_file_id(namespace_of(&fqn))
                .and_then(|other| self.local_type(other, &fqn))
                .map(Node::Declaration));
        }
        Ok(self.local_type(file, type_name).map(Node::Declaration))
    }

    fn get_all_super_type_declarations(&self, declaration: &Node) -> Result<Vec<Node>> {
        let not_a_function = || not_a_function("typeDeclaration.getAllSuperTypeDeclarations");
        let Node::Declaration(id) = *declaration else {
            return Err(not_a_function());
        };
        match self.declaration(id).ok_or_else(|| unknown(*declaration))? {
            Declaration::Class(_) => self
                .super_chain(&self.declaration_fqn(id)?)?
                .into_iter()
                // The chain starts with the type itself.
                .skip(1)
                .map(|(fqn, _)| {
                    self.declaration_id(&fqn)
                        .map(Node::Declaration)
                        .ok_or(ConcertoError::TypeNotFound { type_name: fqn })
                })
                .collect(),
            // An enum is a ClassDeclaration in TS whose only super type is the
            // implicit `Concept`, which the loader does not add yet (P2-03).
            Declaration::Enum(_) => Ok(Vec::new()),
            Declaration::Scalar(_) | Declaration::Map(_) => Err(not_a_function()),
        }
    }

    fn get_fully_qualified_name(&self, declaration: &Node) -> Result<String> {
        match *declaration {
            Node::Declaration(id) => self.declaration_fqn(id),
            // TS: Property.getFullyQualifiedName (src/introspect/property.ts)
            Node::Property(id) => {
                let (Some(parent), Some(property)) = (self.parent_of(id), self.property(id)) else {
                    return Err(unknown(*declaration));
                };
                Ok(format!(
                    "{}.{}",
                    self.declaration_fqn(parent)?,
                    property.name()
                ))
            }
            Node::ModelFile(_) | Node::Primitive(_) => {
                Err(not_a_function("type.getFullyQualifiedName"))
            }
        }
    }

    fn get_fully_qualified_type_name(&self, property: &Node) -> Result<String> {
        let Node::Property(id) = *property else {
            return Err(not_a_function("property.getFullyQualifiedTypeName"));
        };
        let (Some(field), Some(file)) = (
            self.property(id),
            self.parent_of(id)
                .and_then(|parent| self.model_file_of(parent)),
        ) else {
            return Err(unknown(*property));
        };
        let type_name = field.type_name();
        if let Some(type_name) = type_name
            && is_primitive_type(type_name)
        {
            return Ok(type_name.to_string());
        }
        let mf = self.file(file).ok_or_else(|| unknown(*property))?;
        // TS: ModelFile.getFullyQualifiedTypeName (src/introspect/modelfile.ts)
        let resolved = match type_name {
            None => None,
            Some(type_name) => match imported_type(mf, type_name) {
                Some(fqn) => Some(fqn),
                None => self
                    .local_type(file, type_name)
                    .map(|local| self.declaration_fqn(local))
                    .transpose()?,
            },
        };
        // TODO(#48): TS: Property.getFullyQualifiedTypeName
        // (src/introspect/property.ts:218) throws `ErrorKind::Error` with the
        // inline template `'Failed to find fully qualified type name for
        // property ' + this.name + ' with type ' + this.type` here
        // (ModelFile.getFullyQualifiedTypeName itself returns null and never
        // throws). P2-04 ports Property.getFullyQualifiedTypeName and adds
        // that template and its golden test to the catalogue (PORTING.md
        // 6.3). Until then the already-ported ModelUtil.isAssignableTo
        // (model_util.rs) reaches this natively as `TypeNotFound`, where TS
        // throws `Error`.
        resolved.ok_or_else(|| ConcertoError::TypeNotFound {
            type_name: type_name.unwrap_or("null").to_string(),
        })
    }

    fn get_parent(&self, property: &Node) -> Result<Node> {
        let Node::Property(id) = *property else {
            return Err(not_a_function("field.getParent"));
        };
        self.parent_of(id)
            .map(Node::Declaration)
            .ok_or_else(|| unknown(*property))
    }

    fn get_model_file(&self, declaration: &Node) -> Result<Node> {
        let Node::Declaration(id) = *declaration else {
            return Err(not_a_function("getModelFile"));
        };
        self.model_file_of(id)
            .map(Node::ModelFile)
            .ok_or_else(|| unknown(*declaration))
    }

    fn get_type_name(&self, property: &Node) -> Result<Option<String>> {
        let Node::Property(id) = *property else {
            return Err(not_a_function("field.getType"));
        };
        let field = self.property(id).ok_or_else(|| unknown(*property))?;
        Ok(field.type_name().map(str::to_string))
    }

    fn is_enum(&self, declaration: &Node) -> Result<bool> {
        let Node::Declaration(id) = *declaration else {
            return Err(not_a_function("typeDeclaration.isEnum"));
        };
        let found = self.declaration(id).ok_or_else(|| unknown(*declaration))?;
        Ok(found.is_enum_declaration())
    }

    fn is_map_declaration(&self, declaration: &Node) -> Result<Option<bool>> {
        match *declaration {
            Node::Declaration(id) => Ok(Some(
                self.declaration(id)
                    .ok_or_else(|| unknown(*declaration))?
                    .is_map_declaration(),
            )),
            // No such method on a model file, a property or a string.
            Node::ModelFile(_) | Node::Property(_) | Node::Primitive(_) => Ok(None),
        }
    }

    fn is_scalar_declaration(&self, declaration: &Node) -> Result<Option<bool>> {
        match *declaration {
            Node::Declaration(id) => Ok(Some(
                self.declaration(id)
                    .ok_or_else(|| unknown(*declaration))?
                    .is_scalar_declaration(),
            )),
            // No such method on a model file, a property or a string.
            Node::ModelFile(_) | Node::Property(_) | Node::Primitive(_) => Ok(None),
        }
    }

    fn get_ast_class(&self, declaration: &Node) -> Result<Option<String>> {
        let Some(ast) = self.ast(*declaration)? else {
            return Err(ContractError::new(
                ErrorKind::JsTypeError,
                "engine-typeerror-readproperties",
                vec![
                    ("value", "undefined".to_string()),
                    ("property", "$class".to_string()),
                ],
            )
            .into());
        };
        Ok(ast
            .get("$class")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    fn get_all_declarations(&self, model_file: &Node) -> Result<Vec<Node>> {
        let Node::ModelFile(id) = *model_file else {
            return Err(not_a_function("this.getModelFile().getAllDeclarations"));
        };
        if self.file(id).is_none() {
            return Err(unknown(*model_file));
        }
        Ok(self.declaration_ids(id).map(Node::Declaration).collect())
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

    /// [`manager`] plus `org.other@1.0.0`, which imports from it (and from a
    /// namespace that is not loaded) and declares a concept and a scalar.
    fn manager_with_imports() -> ModelManager {
        let mut mgr = manager();
        mgr.add_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.other@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportTypes",
                      "namespace": "org.example@1.0.0", "types": ["Person", "Manager", "Color"] },
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.missing@1.0.0", "name": "Ghost" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Team", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "lead", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" } },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "colour", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Color" } },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "label", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Email" }
                ]
            }),
            None,
        )
        .unwrap();
        mgr
    }

    fn decl(mgr: &ModelManager, fqn: &str) -> Node {
        Node::Declaration(mgr.declaration_id(fqn).unwrap())
    }

    fn file_node(mgr: &ModelManager, namespace: &str) -> Node {
        Node::ModelFile(mgr.model_file_id(namespace).unwrap())
    }

    /// The property of a class declaration, by name.
    fn prop(mgr: &ModelManager, fqn: &str, name: &str) -> Node {
        let id = mgr
            .property_ids(mgr.declaration_id(fqn).unwrap())
            .find(|&id| mgr.property(id).unwrap().name() == name)
            .unwrap();
        Node::Property(id)
    }

    #[test]
    fn handles_survive_later_loads() {
        let mut mgr = manager();
        let person = mgr.declaration_id("org.example@1.0.0.Person").unwrap();
        let file = mgr.model_file_id("org.example@1.0.0").unwrap();
        let generation = mgr.generation();

        mgr.add_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.later@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();

        assert!(mgr.generation() > generation);
        assert_eq!(mgr.declaration_id("org.example@1.0.0.Person"), Some(person));
        assert_eq!(mgr.model_file_id("org.example@1.0.0"), Some(file));
        assert_eq!(mgr.model_file_of(person), Some(file));
        assert_ne!(mgr.declaration_id("org.later@1.0.0.Person"), Some(person));
        // A handle round-trips through its raw index, as a binding passes it.
        assert_eq!(DeclId::from_index(person.index()), person);
    }

    #[test]
    fn a_failed_load_changes_nothing() {
        let mut mgr = manager();
        let generation = mgr.generation();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example@1.0.0", "declarations": []
        });
        assert!(mgr.add_model(&model, None).is_err());
        assert_eq!(mgr.generation(), generation);
        assert_eq!(mgr.model_files().count(), 2);
    }

    #[test]
    fn walks_the_graph_by_handle() {
        let mgr = manager();
        let file = mgr.model_file_id("org.example@1.0.0").unwrap();
        let names: Vec<&str> = mgr
            .declaration_ids(file)
            .map(|id| mgr.declaration(id).unwrap().name())
            .collect();
        assert_eq!(names, ["Person", "Employee", "Manager", "Color"]);

        let employee = mgr.declaration_id("org.example@1.0.0.Employee").unwrap();
        let props: Vec<PropId> = mgr.property_ids(employee).collect();
        assert_eq!(props.len(), 1);
        assert_eq!(mgr.property(props[0]).unwrap().name(), "salary");
        assert_eq!(mgr.parent_of(props[0]), Some(employee));

        // Enum values are not properties yet (P2-04).
        let color = mgr.declaration_id("org.example@1.0.0.Color").unwrap();
        assert_eq!(mgr.property_ids(color).count(), 0);
        // Model files are listed in load order, the system model first.
        let namespaces: Vec<&str> = mgr.model_files().map(ModelFile::namespace).collect();
        assert_eq!(namespaces, ["concerto@1.0.0", "org.example@1.0.0"]);
    }

    #[test]
    fn unknown_handles_name_nothing() {
        let mgr = manager();
        let stale = DeclId::from_index(u32::MAX);
        assert!(mgr.declaration(stale).is_none());
        assert!(mgr.model_file_of(stale).is_none());
        assert_eq!(mgr.property_ids(stale).count(), 0);
        assert!(mgr.property(PropId::from_index(u32::MAX)).is_none());
        assert_eq!(
            mgr.declaration_ids(ModelFileId::from_index(u32::MAX))
                .count(),
            0
        );
        assert!(mgr.get_model_file(&Node::Declaration(stale)).is_err());
        assert!(mgr.is_enum(&Node::Declaration(stale)).is_err());
    }

    #[test]
    fn get_type_follows_model_file_get_type() {
        let mgr = manager_with_imports();
        let example = file_node(&mgr, "org.example@1.0.0");
        let other = file_node(&mgr, "org.other@1.0.0");
        let person = decl(&mgr, "org.example@1.0.0.Person");

        assert_eq!(
            mgr.get_type(&example, Some("Person")).unwrap(),
            Some(person)
        );
        // `getLocalType` takes a name that already starts with the namespace.
        assert_eq!(
            mgr.get_type(&example, Some("org.example@1.0.0.Person"))
                .unwrap(),
            Some(person)
        );
        assert_eq!(
            mgr.get_type(&example, Some("String")).unwrap(),
            Some(Node::Primitive("String"))
        );
        assert_eq!(mgr.get_type(&example, Some("Nope")).unwrap(), None);
        assert_eq!(mgr.get_type(&example, None).unwrap(), None);
        // Imported, from a loaded namespace and from one that is not.
        assert_eq!(mgr.get_type(&other, Some("Person")).unwrap(), Some(person));
        assert_eq!(mgr.get_type(&other, Some("Ghost")).unwrap(), None);
        // The built-in import of the system types.
        assert_eq!(
            mgr.get_type(&other, Some("Concept")).unwrap(),
            Some(decl(&mgr, "concerto@1.0.0.Concept"))
        );
        // Employee is declared over there but not imported.
        assert_eq!(mgr.get_type(&other, Some("Employee")).unwrap(), None);

        let err = mgr.get_type(&person, Some("Person")).unwrap_err();
        assert_eq!(err.to_string(), "modelFile.getType is not a function");
    }

    #[test]
    fn answers_the_collaborator_getters() {
        let mgr = manager_with_imports();
        let employee = decl(&mgr, "org.example@1.0.0.Employee");
        let salary = prop(&mgr, "org.example@1.0.0.Employee", "salary");
        let lead = prop(&mgr, "org.other@1.0.0.Team", "lead");

        assert_eq!(mgr.get_parent(&salary).unwrap(), employee);
        assert_eq!(
            mgr.get_model_file(&employee).unwrap(),
            file_node(&mgr, "org.example@1.0.0")
        );
        assert_eq!(
            mgr.get_fully_qualified_name(&employee).unwrap(),
            "org.example@1.0.0.Employee"
        );
        assert_eq!(
            mgr.get_fully_qualified_name(&salary).unwrap(),
            "org.example@1.0.0.Employee.salary"
        );
        assert_eq!(
            mgr.get_type_name(&salary).unwrap().as_deref(),
            Some("Double")
        );
        assert_eq!(mgr.get_type_name(&lead).unwrap().as_deref(), Some("Person"));
        assert_eq!(
            mgr.get_fully_qualified_type_name(&salary).unwrap(),
            "Double"
        );
        assert_eq!(
            mgr.get_fully_qualified_type_name(&lead).unwrap(),
            "org.example@1.0.0.Person"
        );
        assert_eq!(
            mgr.get_ast_class(&employee).unwrap().as_deref(),
            Some("concerto.metamodel@1.0.0.ConceptDeclaration")
        );
        assert_eq!(
            mgr.get_ast_class(&salary).unwrap().as_deref(),
            Some("concerto.metamodel@1.0.0.DoubleProperty")
        );
        let supers: Vec<String> = mgr
            .get_all_super_type_declarations(&decl(&mgr, "org.example@1.0.0.Manager"))
            .unwrap()
            .iter()
            .map(|node| mgr.get_fully_qualified_name(node).unwrap())
            .collect();
        assert_eq!(
            supers,
            ["org.example@1.0.0.Employee", "org.example@1.0.0.Person"]
        );
        let declarations = mgr
            .get_all_declarations(&file_node(&mgr, "org.other@1.0.0"))
            .unwrap();
        assert_eq!(
            declarations,
            [
                decl(&mgr, "org.other@1.0.0.Team"),
                decl(&mgr, "org.other@1.0.0.Email")
            ]
        );
    }

    #[test]
    fn a_primitive_type_answers_as_a_js_string_does() {
        let mgr = manager();
        let string = Node::Primitive("String");
        assert_eq!(
            mgr.is_enum(&string).unwrap_err().to_string(),
            "typeDeclaration.isEnum is not a function"
        );
        assert_eq!(mgr.is_map_declaration(&string).unwrap(), None);
        assert_eq!(mgr.is_scalar_declaration(&string).unwrap(), None);
        assert_eq!(
            mgr.get_ast_class(&string).unwrap_err().to_string(),
            "Cannot read properties of undefined (reading '$class')"
        );
    }

    #[test]
    fn ported_members_run_on_the_arena() {
        use crate::introspect::ScalarDeclaration;
        use crate::model_util;

        let mgr = manager_with_imports();
        let other = file_node(&mgr, "org.other@1.0.0");
        let lead = prop(&mgr, "org.other@1.0.0.Team", "lead");
        let colour = prop(&mgr, "org.other@1.0.0.Team", "colour");
        let label = prop(&mgr, "org.other@1.0.0.Team", "label");

        assert!(model_util::is_assignable_to(&mgr, &other, "Manager", &lead).unwrap());
        assert!(!model_util::is_assignable_to(&mgr, &other, "Color", &lead).unwrap());
        // Imported from a namespace that is not loaded: `getType` finds nothing.
        let err = model_util::is_assignable_to(&mgr, &other, "Ghost", &lead).unwrap_err();
        assert!(err.to_string().contains("Ghost"), "{err}");

        assert_eq!(model_util::is_enum(&mgr, &colour).unwrap(), Some(true));
        assert_eq!(model_util::is_enum(&mgr, &lead).unwrap(), Some(false));
        assert_eq!(model_util::is_map(&mgr, &colour).unwrap(), Some(false));
        assert_eq!(model_util::is_scalar(&mgr, &lead).unwrap(), Some(false));
        // `modelFile.getType('String')` is the string itself.
        assert!(model_util::is_enum(&mgr, &label).is_err());
        assert_eq!(model_util::is_scalar(&mgr, &label).unwrap(), None);

        let email = decl(&mgr, "org.other@1.0.0.Email");
        assert_eq!(
            model_util::is_valid_map_key_scalar(&mgr, Some(&email)).unwrap(),
            Some(true)
        );
        assert!(ScalarDeclaration::validate(&mgr, &email).is_ok());
    }

    /// PORTING.md 2.1: `location` is copied verbatim from the AST node the
    /// caller passes, never recomputed and never hard-coded to `None`.
    #[test]
    fn resolve_type_name_carries_the_given_location_verbatim() {
        let mgr = ModelManager::new().unwrap();
        let location = serde_json::json!({
            "start": {"line": 3, "column": 1, "offset": 20},
            "end": {"line": 3, "column": 9, "offset": 28}
        });
        let err = mgr
            .resolve_type_name("org.does.not.exist@1.0.0", "Foo", Some(location.clone()))
            .unwrap_err();
        match err {
            ConcertoError::Contract(contract) => assert_eq!(contract.location, Some(location)),
            other => panic!("expected a Contract error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_type_name_with_no_location_carries_none() {
        let mgr = ModelManager::new().unwrap();
        let err = mgr
            .resolve_type_name("org.does.not.exist@1.0.0", "Foo", None)
            .unwrap_err();
        match err {
            ConcertoError::Contract(contract) => assert_eq!(contract.location, None),
            other => panic!("expected a Contract error, got {other:?}"),
        }
    }
}

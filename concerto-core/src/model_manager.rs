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
//! [`PropId`]. Loading a model only appends, so a handle keeps naming the
//! same element across every later load: a handle is never reused. Replacing
//! or removing a model file (`update_model_file`, `delete_model_file`,
//! `update_model_ast`, `remove_model`) builds a new arena, and every handle
//! handed out before it is invalid after it. Looking an element up by its
//! handle is an index into the arena, never a string hash.
//! `ModelManager::generation` counts the mutations and never goes back to an
//! earlier value for a different state (A-3), so that a binding caching a
//! snapshot of an element knows when to drop it (spike input on #41;
//! PORTING.md 1.5).
//!
//! [`ModelManager::add_models`] (P1-06) undoes a failed batch by truncating
//! each vector back to its length before the call: safe without a tombstone,
//! because a batch only ever appends and rolls back its own tail, so no
//! handle from before the call is touched. Any future removal that is not a
//! batch's own rollback (`deleteModelFile`, `clearModelFiles`) is a different
//! shape — it must leave a tombstone rather than shift the arena, so that the
//! handles of the elements that stay remain valid.
//!
//! A ported member reaches its collaborators through the
//! `ResolutionContext` trait. The manager implements it over the arena, with
//! `Node` as its handle; `concerto-wasm` implements it over JS objects, for
//! views a white-box test builds over stubbed collaborators (PORTING.md 1.4).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

use rustc_hash::FxHashMap;

use serde_json::Value;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::declaration::{ClassDeclaration, ClassKind, Declaration, EnumDeclaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::introspect::{DeclarationKind, FullyQualified};
use crate::model_util::{
    self, PRIMITIVE_TYPES, get_namespace, is_primitive_type, namespace_of, qualify, short_name,
};
use crate::rootmodel::{decorator_model_ast, root_model_ast};

/// The namespaces TS `BaseModelManager.getModelFiles()` leaves out unless it
/// is asked to include them: the system model, its unversioned name, and the
/// decorator model. The match is on the exact namespace string.
///
/// TS: `EXCLUDE_NS` (src/basemodelmanager.ts).
///
/// `pub(crate)` so [`crate::dcs::decorate_models`] can reuse it for its own
/// `getAst`-shaped model collection (P2-12) rather than duplicating the
/// namespace list.
pub(crate) const EXCLUDE_NS: [&str; 3] = ["concerto@1.0.0", "concerto", "concerto.decorator@1.0.0"];

js_compat_pub! {
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
}

js_compat_pub! {
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

        /// TS: `field.getName()`. `StringValidator` and `CollectionSizeValidator`
        /// (unlike `NumberValidator`) pass this as the identifier of every error
        /// their constructor reports, and `StringValidator` passes it again as
        /// the identifier for the `defaultValue` check it runs at load time
        /// (P2-02).
        fn name(&self) -> std::result::Result<String, Self::Error>;
    }
}

/// Declares a handle type: a dense `u32` index into one of the arena's
/// tables.
macro_rules! handle {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u32);

        impl $name {
            js_compat_pub! {
                /// The handle with this raw index, as a binding gets it back from
                /// JS. An index the manager never handed out names nothing: every
                /// lookup of it answers `None` or an error.
                pub const fn from_index(index: u32) -> Self {
                    Self(index)
                }
            }

            js_compat_pub! {
                /// The raw index, as a binding passes it to JS (a plain number).
                pub const fn index(self) -> u32 {
                    self.0
                }
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
    /// A handle to a property of a class declaration, or a value of an enum
    /// declaration (P2-04), loaded into a [`ModelManager`]. Both are
    /// [`Property`] values, addressed the same way, through the unified
    /// `ClassLike::own_properties`.
    PropId
}

js_compat_pub! {
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
}

/// A loaded model file, and the handles of its declarations.
///
/// The model file is shared (`Arc`, P5-18): a model file never changes once
/// registered, so a scratch copy of the arena
/// ([`ModelManager::with_model_file_registered`]) shares every file it
/// keeps instead of deep-cloning it.
#[derive(Debug)]
struct FileSlot {
    model_file: Arc<ModelFile>,
    declarations: Range<u32>,
    /// P5-97 (accordproject/concerto-rust#448): set once the file has
    /// passed [`ModelManager::validate_model_file`] in this manager (in
    /// [`ModelManager::validate_models`], or when
    /// [`ModelManager::validate_and_add_model_file`] registered it), so a
    /// later [`ModelManager::validate_models`] need not validate it again.
    /// A file's validity reads only the file, the files of the namespaces
    /// it reaches through its imports, and the manager's options: adding a
    /// file never changes it (a namespace is never registered twice), so
    /// the mark holds until the options change (each setter clears every
    /// mark), a failed batch is rolled back (its marks are restored), or
    /// the manager is rebuilt (update, delete: a new manager starts with
    /// none).
    validated: std::sync::atomic::AtomicBool,
    /// P5-97: what lets this manager take the file as validated without
    /// validating it, when the file is shared from a manager that had
    /// validated it ([`ModelManager::validity_proof`]): the namespaces the
    /// file reaches there, with their files, and that manager's options.
    proof: Option<Arc<ValidityProof>>,
}

impl Clone for FileSlot {
    fn clone(&self) -> Self {
        Self {
            model_file: Arc::clone(&self.model_file),
            declarations: self.declarations.clone(),
            validated: std::sync::atomic::AtomicBool::new(
                self.validated.load(std::sync::atomic::Ordering::Relaxed),
            ),
            proof: self.proof.clone(),
        }
    }
}

js_compat_pub! {
    /// P5-97 (accordproject/concerto-rust#448): why a model file shared from
    /// one manager into another ([`ModelManager::add_shared_model_file_with_proof`])
    /// is valid there too, without validating it again: it passed
    /// validation in the source manager, whose options were these, and
    /// these are every namespace it reaches through its imports (its own,
    /// the system models and the transitive import closure), each with the
    /// very file (`Arc`) the source held. A file's validity reads nothing
    /// else, so it holds in any manager with the same options whose files
    /// under those namespaces are those same files. The target checks that
    /// when it validates ([`ModelManager::validate_models`]) and validates
    /// the file as usual when it does not hold.
    #[derive(Debug)]
    pub struct ValidityProof {
        options: ManagerOptions,
        closure: Box<[(Box<str>, Arc<ModelFile>)]>,
    }
}

/// Where a declaration is: its model file, its position in
/// [`ModelFile::declarations`], the handles of its properties, and its
/// fully-qualified name (TS keeps it on the declaration, P5-13), built once
/// when the file is registered. The name is shared (`Arc<str>`, P5-18), so
/// copying the arena copies no string.
#[derive(Debug, Clone)]
struct DeclSlot {
    model_file: ModelFileId,
    index: usize,
    properties: Range<u32>,
    fqn: Arc<str>,
}

/// The inheritance facts of one class-like or enum declaration, from its
/// declaration handle up to its root (P5-13): what `super_chain`,
/// `getProperties`, `getProperty` and `getIdentifierFieldName` walk on every
/// call. It depends only on the registered files, so it is cached per
/// declaration until they change ([`ModelManager::invalidate_caches`]).
#[derive(Debug)]
struct ClassInfo {
    /// The declaration itself, then each super type up to the root.
    chain: Box<[DeclId]>,
    /// Every property along the chain, in `getProperties()` order.
    properties: Box<[PropId]>,
}

js_compat_pub! {
    /// A borrowed view of every property of a class-like or enum
    /// declaration, own and inherited (TS `getProperties()`), each with the
    /// fully-qualified name of the declaration that declares it: the
    /// allocation-free form of [`ModelManager::properties`] (P5-13).
    #[derive(Clone)]
    pub struct ClassProperties<'a> {
        mm: &'a ModelManager,
        info: Arc<ClassInfo>,
    }
}

impl<'a> ClassProperties<'a> {
    /// Each property with its declaring type's fully-qualified name, in
    /// `getProperties()` order.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a Property)> + '_ {
        let mm = self.mm;
        self.info.properties.iter().map(move |id| {
            mm.property_with_owner(*id)
                .expect("a cached property handle is live")
        })
    }

    /// The first property named `name` (TS `getProperty(name)`).
    pub fn find(&self, name: &str) -> Option<(&'a str, &'a Property)> {
        self.iter().find(|(_, p)| p.name() == name)
    }

    /// Whether a property named `name` exists.
    pub fn contains(&self, name: &str) -> bool {
        self.find(name).is_some()
    }
}

/// The arena lengths and generation before
/// [`ModelManager::append_for_validation`] appended a file (P5-48).
pub(crate) struct AppendMark {
    files: usize,
    declarations: usize,
    properties: usize,
    generation: u64,
}

/// Where a property is: its declaration, and its position in
/// [`ClassDeclaration::own_properties`].
#[derive(Debug, Clone)]
struct PropSlot {
    declaration: DeclId,
    index: usize,
}

js_compat_pub! {
    /// TS `ModelFileSource` (basemodelmanager.ts): a model file as a
    /// `FileLoader` returns it, before it becomes a [`ModelFile`] —
    /// [`ModelManager::update_external_models`]' input.
    #[derive(Debug, Clone)]
    pub struct ModelFileSource {
        /// The model's metamodel AST.
        pub ast: Value,
        /// Its CTO source text, when it has one.
        pub definitions: Option<String>,
        /// Its file name (a downloaded file's starts with `@`).
        pub file_name: Option<String>,
    }
}

/// The options of a [`ModelManager`] (TS `ModelManagerOptions`), held as
/// one value (A-5, accordproject/concerto-rust#448): the builder fills it,
/// and every manager derived from another (a fork, a filter's result, a
/// scratch copy, a rebuild after a removal) copies it whole.
#[derive(Debug, Clone, Default, PartialEq)]
struct ManagerOptions {
    /// TS `ModelManagerOptions.decoratorValidation`, `DEFAULT_DECORATOR_VALIDATION`
    /// by default (both fields `None`): see
    /// [`crate::introspect::decorator::DecoratorValidationOptions`].
    decorator_validation: crate::introspect::decorator::DecoratorValidationOptions,
    /// TS `ModelManagerOptions.dangerouslyAllowReservedSystemTypeNamesInUserModels`
    /// (`basemodelmanager.ts`), read back as `modelFile.getModelManager()
    /// ?.options?.dangerouslyAllowReservedSystemTypeNamesInUserModels`
    /// (`Declaration.validate`, declaration.ts). `false` (JS `undefined`,
    /// falsy) by default: a transitional escape hatch that lets a user model
    /// redeclare a name it also imports from the system namespace, so long as
    /// the imported name resolves to one of the five reserved system
    /// declarations (`Concept`, `Asset`, `Participant`, `Transaction`,
    /// `Event`).
    allow_reserved_system_type_names: bool,
    /// TS `ModelManagerOptions.metamodelValidation` (`basemodelmanager.ts`):
    /// when truthy, `addModelFile` checks a new model file's AST against the
    /// metamodel ([`ModelManager::validate_ast`]) before its semantic
    /// validation. `false` (JS `undefined`) by default. Task P4-08b.
    metamodel_validation: bool,
}

/// Owns a set of model files and resolves types across them.
#[derive(Debug, Default)]
pub struct ModelManager {
    files: Vec<FileSlot>,
    namespaces: FxHashMap<String, ModelFileId>,
    declarations: Vec<DeclSlot>,
    properties: Vec<PropSlot>,
    generation: u64,
    /// The manager's options (A-5): one value, so every manager derived
    /// from this one (a fork, a filter, a scratch copy) starts from
    /// `self.options.clone()` and never drops one.
    options: ManagerOptions,
    /// Every per-declaration answer computed so far, by declaration handle
    /// ([`DeclCache`]): inheritance chains (P5-06, P5-13), validation plans
    /// (P5-88) and field defaults (P5-13), under one lock (F-10, B-7,
    /// accordproject/concerto-rust#448). An answer depends only on the
    /// registered files: an append keeps every answer it cannot change
    /// ([`ModelManager::keep_caches_for_append`], P5-97), and any other
    /// change drops them all ([`ModelManager::invalidate_caches`]).
    decl_cache: DeclCache,
    /// `generation + 1` when [`ModelManager::has_system_files_of`] last
    /// found this manager's system model files to be the resident
    /// metamodel manager's own (P5-13); 0 before any such check.
    system_files_checked: std::sync::atomic::AtomicU64,
}

/// Options for [`ModelManager::ast`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AstOptions {
    /// Resolve every type name to its declaring namespace
    /// ([`ModelManager::resolve_meta_model`]).
    pub resolve: bool,
    /// Include the system models (`concerto@1.0.0` and
    /// `concerto.decorator@1.0.0`).
    pub include_system_models: bool,
}

/// Builds a [`ModelManager`] with options ([`ModelManager::builder`]).
///
/// TS: the `ModelManagerOptions` of the `ModelManager` constructor.
#[derive(Debug, Clone, Default)]
pub struct ModelManagerBuilder {
    options: ManagerOptions,
}

impl ModelManagerBuilder {
    /// How unknown decorators and their arguments are reported (TS
    /// `decoratorValidation`). By default they are not checked.
    pub fn decorator_validation(
        mut self,
        options: crate::introspect::decorator::DecoratorValidationOptions,
    ) -> Self {
        self.options.decorator_validation = options;
        self
    }

    /// Whether a model is checked against the metamodel before its semantic
    /// validation (TS `metamodelValidation`). Off by default.
    pub fn metamodel_validation(mut self, on: bool) -> Self {
        self.options.metamodel_validation = on;
        self
    }

    /// Whether a user model may declare a name it also imports from the
    /// system namespace, when that name is one of the five reserved system
    /// declarations (TS `dangerouslyAllowReservedSystemTypeNamesInUserModels`,
    /// a transitional escape hatch). Off by default.
    pub fn allow_reserved_system_type_names(mut self, on: bool) -> Self {
        self.options.allow_reserved_system_type_names = on;
        self
    }

    /// A manager with these options and the system models loaded, as
    /// [`ModelManager::new`] gives.
    pub fn build(self) -> Result<ModelManager> {
        let mut manager = ModelManager::new()?;
        manager.options = self.options;
        Ok(manager)
    }
}

/// The next handle of an arena table holding `len` entries.
///
/// A full arena is a Rust-only failure (PORTING.md 2.3): TS keeps its model
/// graph in unbounded JS arrays and objects, so no TS class, message or
/// fixture corresponds to it. It keeps the nearest existing kind, a pre-port
/// `IllegalModel` (`Error::illegal_model`: the model cannot be loaded), with no
/// catalogue entry, rather than a new `ErrorKind`, which 2.3 forbids when no
/// TS class matches. Four billion elements is not a model anyone loads, but
/// the boundary path must not panic.
fn next_index(len: usize) -> Result<u32> {
    u32::try_from(len).map_err(|_| {
        Error::illegal_model(
            "the model manager cannot address any more elements",
            None,
            None,
        )
    })
}

/// V8's `TypeError` for calling a method the receiver does not have.
/// `expression` is the one the JS-callback context names for the same call.
fn not_a_function(expression: &str) -> Error {
    ContractError::new(
        ErrorKind::MalformedInput,
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
/// harness). It keeps the nearest existing kind, a pre-port
/// `TypeNotFound` (`Error::type_not_found`: the handle names no element), with no
/// catalogue entry, rather than a new `ErrorKind`, which 2.3 forbids when no
/// TS class matches.
fn unknown(node: Node) -> Error {
    Error::type_not_found(format!("{node:?}"))
}

/// Locks `mutex`, recovering it when a thread panicked while holding it
/// (A-7, accordproject/concerto-rust#448): every cache under a lock only
/// ever holds whole answers, so a poisoned one is still consistent.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One declaration's cached answers (F-10, B-7,
/// accordproject/concerto-rust#448), each computed on first use.
#[derive(Debug, Clone, Default)]
struct DeclFacts {
    /// Its inheritance chain and properties; only a successful answer.
    class: Option<Arc<ClassInfo>>,
    /// Its validation plan ([`crate::instance::plan`]); `Some(None)`
    /// records a declaration with no plan.
    plan: Option<Option<Arc<crate::instance::plan::ClassPlan>>>,
    /// Its converted field defaults
    /// ([`crate::instance::from_json::assign_field_defaults_of`]); only a
    /// successful answer.
    field_defaults: Option<Arc<crate::instance::from_json::FieldDefaults>>,
}

/// The per-declaration cache of a [`ModelManager`]: one slot of
/// [`DeclFacts`] per declaration handle, under one lock (a `Mutex` rather
/// than a `RefCell`, so the manager stays `Sync`).
#[derive(Debug, Default)]
struct DeclCache(Mutex<Vec<DeclFacts>>);

impl DeclCache {
    /// The answer `field` selects for declaration `id`, or `compute`'s,
    /// which is kept when it succeeds. The lock is not held while
    /// computing, so `compute` may read this cache again.
    fn get_or_try_insert_with<T: Clone, E>(
        &self,
        id: DeclId,
        field: fn(&mut DeclFacts) -> &mut Option<T>,
        compute: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        if let Some(answer) = lock(&self.0)
            .get_mut(id.slot())
            .and_then(|facts| field(facts).clone())
        {
            return Ok(answer);
        }
        let answer = compute()?;
        let mut cache = lock(&self.0);
        if cache.len() <= id.slot() {
            cache.resize_with(id.slot() + 1, DeclFacts::default);
        }
        *field(&mut cache[id.slot()]) = Some(answer.clone());
        Ok(answer)
    }

    /// A copy of every answer, for a fork ([`ModelManager::fork`]).
    fn snapshot(&self) -> Self {
        Self(Mutex::new(lock(&self.0).clone()))
    }

    /// Every answer, for the caller to edit in place.
    fn facts_mut(&mut self) -> &mut Vec<DeclFacts> {
        self.0
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads every answer under the lock.
    #[cfg(any(test, feature = "validation-plan-testing"))]
    fn read<R>(&self, read: impl FnOnce(&[DeclFacts]) -> R) -> R {
        read(&lock(&self.0))
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
            .map(|(_, imported)| qualify(import.namespace(), imported))
    })
}

/// The class-like facts a `ClassDeclaration` or an `EnumDeclaration` carries,
/// unified for the members TS defines once on `ClassDeclaration` and
/// `EnumDeclaration` inherits unchanged (enumdeclaration.ts overrides only
/// `toString` and `declarationKind`, PORTING.md 1.1 rule 2). The manager's
/// inheritance-walking members (`super_chain` and everything built on it)
/// read a declaration through this instead of `Declaration::as_class`, so
/// that an enum's implicit `Concept` super type, own properties and identity
/// are seen the same way a concept-like declaration's are.
#[derive(Clone, Copy)]
enum ClassLike<'a> {
    Class(&'a ClassDeclaration),
    Enum(&'a EnumDeclaration),
}

impl<'a> ClassLike<'a> {
    fn from_declaration(declaration: &'a Declaration) -> Option<Self> {
        match declaration {
            Declaration::Class(class) => Some(Self::Class(class)),
            Declaration::Enum(e) => Some(Self::Enum(e)),
            Declaration::Scalar(_) | Declaration::Map(_) => None,
        }
    }

    fn own_properties(&self) -> &'a [Property] {
        match self {
            Self::Class(class) => class.own_properties(),
            Self::Enum(e) => e.own_properties(),
        }
    }

    fn own_identifier_field_name(&self) -> Option<&'a str> {
        match self {
            Self::Class(class) => class.own_identifier_field_name(),
            Self::Enum(e) => e.own_identifier_field_name(),
        }
    }

    /// The direct super type this declaration's own AST names, or the
    /// implicit `Concept` — `None` only for the system model's own `Concept`
    /// declaration.
    fn super_type(&self) -> Option<mm::TypeIdentifier> {
        match self {
            Self::Class(class) => class.super_type().cloned(),
            Self::Enum(e) => Some(e.implicit_super_type()),
        }
    }

    fn location(&self) -> Option<&'a mm::Range> {
        match self {
            Self::Class(class) => class.location(),
            Self::Enum(e) => e.location(),
        }
    }
}

/// TS: `ClassDeclaration._resolveSuperType`/`getProperty`/… all reached
/// through a receiver whose prototype chain includes `ClassDeclaration`;
/// reaching one of these members on a scalar or map declaration is not a TS
/// shape at all (neither extends `ClassDeclaration`), so no fixture or TS
/// class corresponds to it (PORTING.md 2.3), the same as [`unknown`] and
/// [`not_a_function`] above.
fn not_a_class_like(fqn: &str) -> Error {
    Error::illegal_model(
        format!("{fqn} is not a concept-like or enum declaration"),
        None,
        None,
    )
}

/// TS `BaseModelManager._throwAlreadyExists(modelFile)` (basemodelmanager.ts):
/// a plain `Error`, thrown for a model file whose namespace is already
/// registered — never the `duplicate namespace: …` `IllegalModelException`
/// this port raised before (P2-08b review). `existing` is the model file
/// already holding `namespace`; `new_file_name` is the incoming file's own
/// name, when TS's caller has one to pass (`addModelFile`'s `modelFile`
/// always does; `add_models`/`insert_models`' own per-model `file_name`
/// argument might not).
fn already_exists(namespace: &str, new_file_name: Option<&str>, existing: &ModelFile) -> Error {
    fn named(name: Option<&str>) -> Option<&str> {
        name.filter(|n| !n.is_empty())
    }
    let prefix = named(new_file_name)
        .map(|n| format!(" specified in file {n}"))
        .unwrap_or_default();
    let postfix = named(existing.file_name())
        .map(|n| format!(" in file {n}"))
        .unwrap_or_default();
    ContractError::new(
        ErrorKind::InvalidArgument,
        "basemodelmanager-throwalreadyexists",
        vec![
            ("namespace", namespace.to_string()),
            ("prefix", prefix),
            ("postfix", postfix),
        ],
    )
    .into()
}

thread_local! {
    /// The decorator and root system model files, loaded from their vendored
    /// ASTs once per thread and shared by every new manager (P5-06; P5-48,
    /// accordproject/concerto-rust#369: shared, not deep-cloned, as P5-18's
    /// scratch managers share their files): a model file is a pure function
    /// of its AST and file name, and a manager never changes a registered
    /// file, so a shared file is indistinguishable from a fresh load, without
    /// re-serialising and re-reading both ASTs on every
    /// [`ModelManager::new`].
    static SYSTEM_MODEL_FILES: std::cell::RefCell<Option<(Arc<ModelFile>, Arc<ModelFile>)>> =
        const { std::cell::RefCell::new(None) };
}

/// The decorator and root system model files, as [`ModelManager::new`]
/// loads them (see [`SYSTEM_MODEL_FILES`]). A load error is returned, and
/// not cached, exactly as an uncached load would return it.
fn system_model_files() -> Result<(Arc<ModelFile>, Arc<ModelFile>)> {
    if let Some(files) = SYSTEM_MODEL_FILES.with(|cache| cache.borrow().clone()) {
        return Ok(files);
    }
    let decorator = Arc::new(ModelFile::from_json(
        &decorator_model_ast(),
        Some("concerto_decorator_1.0.0.cto".into()),
    )?);
    let root = Arc::new(ModelFile::from_json(
        &root_model_ast(),
        Some("concerto_1.0.0.cto".into()),
    )?);
    SYSTEM_MODEL_FILES.with(|cache| {
        *cache.borrow_mut() = Some((Arc::clone(&decorator), Arc::clone(&root)));
    });
    Ok((decorator, root))
}

impl ModelManager {
    /// A fresh manager with both system models already loaded: the decorator
    /// model, then the root model.
    ///
    /// TS: `BaseModelManager`'s constructor calls `this.addDecoratorModel()`
    /// then `this.addRootModel()` (src/basemodelmanager.ts), each of which
    /// builds a `ModelFile` from the vendored AST and adds it with
    /// `addModelFile(m, cto, fileName, true)` - validation disabled. The
    /// arena's `Self::insert` never validates on load (that is a separate,
    /// opt-in pass, [`crate::validation`]), so it already behaves as TS's
    /// `disableValidation = true` does for both models.
    pub fn new() -> Result<Self> {
        let mut mgr = Self::default();
        // TS: `decoratorModelFile`/`rootModelFile`
        // (src/decoratormodelhelper.ts, src/rootmodelhelper.ts) — the
        // vendored `.cto` file names `addDecoratorModel`/`addRootModel` pass
        // to `addModelFile`, which `Declaration.getModelFile().getName()`
        // (and the outer `ModelFile.getName()`) then returns verbatim; not
        // the namespace, which happens to differ only for these two files
        // because every other file name in this port comes from the caller.
        let (decorator, root) = system_model_files()?;
        mgr.insert_shared(decorator)?;
        mgr.insert_shared(root)?;
        Ok(mgr)
    }

    /// Loads a model from its JSON AST. Loading two models with the same
    /// namespace is an error.
    ///
    /// Deprecated: TS `addModel` takes CTO text, so the name is kept free for
    /// the CTO follow-up (docs/public-api.md section 5.2).
    #[deprecated(since = "0.1.0", note = "use `add_model_ast`")]
    pub fn add_model(
        &mut self,
        value: &serde_json::Value,
        file_name: Option<String>,
    ) -> Result<()> {
        self.load_model(value, file_name)
    }

    /// [`ModelManager::add_model`]'s load, for the crate's own callers.
    pub(crate) fn load_model(
        &mut self,
        value: &serde_json::Value,
        file_name: Option<String>,
    ) -> Result<()> {
        self.add_model_with_definitions(value, None, file_name)
    }

    js_compat_pub! {
        /// [`ModelManager::add_model`], keeping `definitions` — the CTO source
        /// text, when the caller has it — as TS's `ModelFile.getDefinitions()`
        /// does. `add_model` is this with `definitions: None`, which is every
        /// call site but the oracle harness's own `addCTOModel`/`addModel`
        /// replay (P2-08b, `ModelManager::get_models`'s only consumer so far):
        /// TS's own `ctoProcessFile` always sets `definitions` to the input CTO
        /// text (coerced to a string when it was not already one), so a model
        /// loaded any other way in this port — a hand-built AST, `fromAst`, the
        /// decorator and root models — has none either, matching TS's
        /// `AstModelManager`/`astProcessFile`, whose `definitions` stays
        /// `undefined`.
        pub fn add_model_with_definitions(
            &mut self,
            value: &serde_json::Value,
            definitions: Option<String>,
            file_name: Option<String>,
        ) -> Result<()> {
            let mf = ModelFile::from_json_with_definitions(value, definitions, file_name)?;
            self.add_loaded_model_file(mf).map(drop)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::add_model_with_definitions`], taking ownership of the
        /// AST so it is kept without being copied
        /// ([`ModelFile::from_owned_json_with_definitions`], P5-06). Same
        /// result, same errors, in the same order.
        pub fn add_owned_model_with_definitions(
            &mut self,
            value: serde_json::Value,
            definitions: Option<String>,
            file_name: Option<String>,
        ) -> Result<()> {
            let mf = ModelFile::from_owned_json_with_definitions(value, definitions, file_name)?;
            self.add_loaded_model_file(mf).map(drop)
        }
    }

    js_compat_pub! {
        /// Adds an already-built model file, such as one
        /// [`ModelFile::from_json_text`] read (P5-06c): the duplicate-namespace
        /// check and registration [`ModelManager::add_model_with_definitions`]
        /// runs once it has built the file itself.
        pub fn add_model_file(&mut self, mf: ModelFile) -> Result<()> {
            self.add_loaded_model_file(mf).map(drop)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::add_model_file`] for a model file that may also
        /// be registered in another manager (P5-77,
        /// accordproject/concerto-rust#419): the same duplicate-namespace
        /// check and the same errors, but the file is shared, not copied,
        /// as [`ModelManager::shared_model_files`] hands it out.
        pub fn add_shared_model_file(&mut self, mf: Arc<ModelFile>) -> Result<()> {
            if let Some(existing) = self
                .namespaces
                .get(mf.namespace())
                .and_then(|id| self.file(*id))
            {
                return Err(already_exists(mf.namespace(), mf.file_name(), existing));
            }
            self.insert_shared(mf).map(drop)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::model_files`], as the shared handles this manager
        /// keeps them in (P5-77): a model file never changes once
        /// registered, so another manager can register the same file
        /// ([`ModelManager::add_shared_model_file`]) without copying it.
        pub fn shared_model_files(&self) -> impl Iterator<Item = &Arc<ModelFile>> {
            self.files.iter().map(|slot| &slot.model_file)
        }
    }

    js_compat_pub! {
        /// P5-97 (accordproject/concerto-rust#448): a new manager over the
        /// same models: the same options, the same model files in the same
        /// order (shared, `Arc`, never copied), and the same handles, so
        /// every [`ModelFileId`], [`DeclId`] and [`PropId`] of this manager
        /// names the same element in the fork. Nothing is validated again:
        /// each file keeps whether it was validated here.
        ///
        /// The fork starts with this manager's warmed caches (inheritance
        /// chains, instance facts, validation plans). Those answers are about
        /// this manager's declarations, which the fork holds unchanged, and a
        /// file the fork adds later cannot change them
        /// (`ModelManager::keep_caches_for_append`: the files here cannot
        /// import a namespace they did not already resolve). So a server can
        /// keep one base manager of its common models, warm it once, and fork
        /// it per request: each request adds its own models to its fork,
        /// isolated from every other fork and from the base.
        ///
        /// The two managers are independent from then on: a later change to
        /// either one never reaches the other.
        pub fn fork(&self) -> Self {
            Self {
                files: self.files.clone(),
                namespaces: self.namespaces.clone(),
                declarations: self.declarations.clone(),
                properties: self.properties.clone(),
                generation: self.generation,
                options: self.options.clone(),
                decl_cache: self.decl_cache.snapshot(),
                system_files_checked: std::sync::atomic::AtomicU64::new(
                    self.system_files_checked
                        .load(std::sync::atomic::Ordering::Relaxed),
                ),
            }
        }
    }

    js_compat_pub! {
        /// P5-97 (accordproject/concerto-rust#448): why the model file
        /// registered under `namespace` is valid in any manager that holds
        /// it with the same options and the same files under the namespaces
        /// it reaches ([`ValidityProof`]), or `None` when it has not been
        /// validated here (or reaches a namespace this manager does not
        /// hold). [`ModelManager::add_shared_model_file_with_proof`] takes
        /// it with the shared file.
        pub fn validity_proof(&self, namespace: &str) -> Option<Arc<ValidityProof>> {
            let id = *self.namespaces.get(namespace)?;
            if !self.known_valid(id) {
                return None;
            }
            // The namespaces the file reaches: its own, the system models
            // (every file's implicit import) and the transitive closure of
            // its imports, each with the file held under it.
            let mut closure: Vec<(Box<str>, Arc<ModelFile>)> = Vec::new();
            let mut seen: HashSet<&str> = HashSet::new();
            let mut pending: Vec<&str> = vec![namespace];
            pending.extend(EXCLUDE_NS.iter().copied().filter(|ns| self.namespaces.contains_key(*ns)));
            while let Some(ns) = pending.pop() {
                if !seen.insert(ns) {
                    continue;
                }
                let slot = &self.files[self.namespaces.get(ns)?.slot()];
                for import in slot.model_file.imports() {
                    pending.push(import.namespace());
                }
                closure.push((Box::from(ns), Arc::clone(&slot.model_file)));
            }
            Some(Arc::new(ValidityProof {
                options: self.options.clone(),
                closure: closure.into_boxed_slice(),
            }))
        }
    }

    js_compat_pub! {
        /// [`ModelManager::add_shared_model_file`], with the
        /// [`ValidityProof`] the source manager gave for the file, if any:
        /// [`ModelManager::validate_models`] then takes the file as valid
        /// without validating it when the proof holds here (P5-97).
        pub fn add_shared_model_file_with_proof(
            &mut self,
            mf: Arc<ModelFile>,
            proof: Option<Arc<ValidityProof>>,
        ) -> Result<ModelFileId> {
            if let Some(existing) = self
                .namespaces
                .get(mf.namespace())
                .and_then(|id| self.file(*id))
            {
                return Err(already_exists(mf.namespace(), mf.file_name(), existing));
            }
            let id = self.insert_shared(mf)?;
            self.files[id.slot()].proof = proof;
            Ok(id)
        }
    }

    /// P5-97: whether the file `id` may be taken as valid without
    /// validating it: it passed validation in this manager, or it carries a
    /// [`ValidityProof`] that holds here (then it is marked validated).
    pub(crate) fn known_valid(&self, id: ModelFileId) -> bool {
        use std::sync::atomic::Ordering;
        let Some(slot) = self.files.get(id.slot()) else {
            return false;
        };
        if slot.validated.load(Ordering::Relaxed) {
            return true;
        }
        let Some(proof) = &slot.proof else {
            return false;
        };
        let holds = self.proof_holds(proof);
        if holds {
            slot.validated.store(true, Ordering::Relaxed);
        }
        holds
    }

    /// P5-97: whether `proof` holds in this manager: the same options, and
    /// the very same file under each namespace it names.
    fn proof_holds(&self, proof: &ValidityProof) -> bool {
        proof.options == self.options
            && proof.closure.iter().all(|(ns, file)| {
                self.namespaces
                    .get(&**ns)
                    .and_then(|id| self.files.get(id.slot()))
                    .is_some_and(|slot| Arc::ptr_eq(&slot.model_file, file))
            })
    }

    /// P5-97: records that the file `id` passed validation in this manager.
    pub(crate) fn mark_validated(&self, id: ModelFileId) {
        if let Some(slot) = self.files.get(id.slot()) {
            slot.validated
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// P5-97: every file's validated mark, for a batch to restore when it
    /// rolls back ([`ModelManager::restore_validated`]).
    fn validated_marks(&self) -> Vec<bool> {
        self.files
            .iter()
            .map(|slot| slot.validated.load(std::sync::atomic::Ordering::Relaxed))
            .collect()
    }

    /// P5-97: puts back the marks [`ModelManager::validated_marks`] took
    /// (a file validated while a rolled-back batch was registered may have
    /// reached one of its files).
    fn restore_validated(&mut self, marks: &[bool]) {
        for (slot, mark) in self.files.iter_mut().zip(marks) {
            *slot.validated.get_mut() = *mark;
        }
    }

    /// P5-97: forgets every validated mark (an option changed).
    fn clear_validated(&mut self) {
        for slot in &mut self.files {
            *slot.validated.get_mut() = false;
        }
    }

    js_compat_pub! {
        /// P5-77: each model file's AST as compact JSON text, in
        /// [`ModelManager::model_files`] order ([`ModelFile::compact_ast`]).
        /// A file only this manager holds keeps its AST as that text from
        /// then on, so a manager that is kept but whose ASTs are rarely read
        /// again holds the text instead of the parsed tree; a file shared
        /// with another manager is serialised and left as it is. Every
        /// [`ModelFile::ast`] stays equal to what it was, so nothing a
        /// caller can read changes, and the caches stay valid.
        pub fn compact_model_asts(&mut self) -> serde_json::Result<Vec<Arc<str>>> {
            self.files
                .iter_mut()
                .map(|slot| match Arc::get_mut(&mut slot.model_file) {
                    Some(model_file) => model_file.compact_ast(),
                    None => Ok(Arc::from(serde_json::to_string(slot.model_file.ast())?)),
                })
                .collect()
        }
    }

    /// The duplicate-namespace check and registration
    /// [`ModelManager::add_model_with_definitions`] runs once the file is
    /// loaded.
    fn add_loaded_model_file(&mut self, mf: ModelFile) -> Result<ModelFileId> {
        if let Some(existing) = self
            .namespaces
            .get(mf.namespace())
            .and_then(|id| self.file(*id))
        {
            return Err(already_exists(mf.namespace(), mf.file_name(), existing));
        }
        self.insert(mf)
    }

    /// P5-48 (accordproject/concerto-rust#369): registers `model_file` in
    /// place for [`ModelManager::validate_and_add_model_file`], when this
    /// manager does not hold its namespace and every file it holds is
    /// registered under its own namespace — exactly the case in which
    /// [`ModelManager::with_model_file_registered`]'s scratch copy is this
    /// manager's arena with the file appended. Returns the file's handle and
    /// what [`ModelManager::undo_append`] needs to take it out again, or
    /// `None` (nothing changed) when that is not the case or no handle can
    /// be allocated; the file is then still the caller's.
    pub(crate) fn append_for_validation(
        &mut self,
        model_file: &Arc<ModelFile>,
    ) -> Option<(ModelFileId, AppendMark)> {
        let namespace = model_file.namespace();
        if self.namespaces.contains_key(namespace) || self.namespaces.len() != self.files.len() {
            return None;
        }
        let mark = AppendMark {
            files: self.files.len(),
            declarations: self.declarations.len(),
            properties: self.properties.len(),
            generation: self.generation,
        };
        let id = self.insert_shared(Arc::clone(model_file)).ok()?;
        Some((id, mark))
    }

    /// Takes out the file [`ModelManager::append_for_validation`] appended,
    /// leaving the arena, the namespaces and the generation as they were
    /// before it (as [`ModelManager::load_models`] rolls a batch back). The
    /// caches are dropped: they may hold the appended file's handles.
    pub(crate) fn undo_append(&mut self, mark: AppendMark) {
        if let Some(slot) = self.files.get(mark.files) {
            let namespace = slot.model_file.namespace().to_string();
            self.namespaces.remove(&namespace);
        }
        self.files.truncate(mark.files);
        self.declarations.truncate(mark.declarations);
        self.properties.truncate(mark.properties);
        self.generation = mark.generation;
        self.invalidate_caches();
    }

    /// Loads a batch of models irrespective of import order between them,
    /// then validates the whole manager once; on any failure the batch has
    /// no effect ([`ModelManager::add_model_asts`]).
    #[deprecated(since = "0.1.0", note = "use `add_model_asts`")]
    pub fn add_models<'a>(
        &mut self,
        models: impl IntoIterator<Item = (&'a serde_json::Value, Option<String>)>,
    ) -> Result<Vec<ModelFileId>> {
        self.load_models(models)
    }

    /// Loads a batch of models irrespective of import order between them
    /// (#26, P1-06). Every model in `models` is added first, then the whole
    /// manager — the new models together with whatever was already loaded —
    /// is validated once with [`ModelManager::validate_models`]. If loading
    /// or that validation fails, the batch has no effect at all: every model
    /// this call added is discarded and the error is returned, exactly as if
    /// `add_models` had never been called.
    ///
    /// This is the batch counterpart of [`ModelManager::add_model`], which
    /// stays order-sensitive only in the sense that it does not validate at
    /// all (validation is a separate, explicit step); `add_models` is what
    /// lets a caller load a set of mutually-dependent models without sorting
    /// them into dependency order first, matching the TS reference's
    /// `addModelFiles` (`basemodelmanager.ts`).
    ///
    /// TS: `BaseModelManager.addModelFiles`.
    pub(crate) fn load_models<'a>(
        &mut self,
        models: impl IntoIterator<Item = (&'a serde_json::Value, Option<String>)>,
    ) -> Result<Vec<ModelFileId>> {
        // A snapshot of every piece of state a load mutates, so a failure
        // partway through — a duplicate namespace within the batch, a
        // structural error in one of the models, or a semantic validation
        // failure over the whole set — can be undone exactly. Because the
        // arena is append-only and this call is the only writer while it
        // runs, everything it adds sits in a contiguous tail of each vector;
        // rolling back is truncating each one back to its snapshot length; no
        // handle handed out before this call is touched, since none of them
        // name a slot at or past that length.
        let files_len = self.files.len();
        let declarations_len = self.declarations.len();
        let properties_len = self.properties.len();
        let namespaces_snapshot = self.namespaces.clone();
        let generation = self.generation;
        let validated = self.validated_marks();

        let mut result = Ok(Vec::new());
        for (value, file_name) in models {
            let outcome = ModelFile::from_json(value, file_name).and_then(|mf| {
                if let Some(existing) = self
                    .namespaces
                    .get(mf.namespace())
                    .and_then(|id| self.file(*id))
                {
                    return Err(already_exists(mf.namespace(), mf.file_name(), existing));
                }
                self.insert(mf)
            });
            match outcome {
                Ok(id) => {
                    if let Ok(ids) = &mut result {
                        ids.push(id);
                    }
                }
                Err(err) => {
                    result = Err(err);
                    break;
                }
            }
        }
        // Validate the whole manager, new models and pre-existing ones
        // together, only once every model in the batch loaded cleanly.
        if result.is_ok()
            && let Err(err) = self.validate_models()
        {
            result = Err(err);
        }

        if let Err(err) = result {
            self.files.truncate(files_len);
            self.declarations.truncate(declarations_len);
            self.properties.truncate(properties_len);
            self.invalidate_caches();
            self.namespaces = namespaces_snapshot;
            self.generation = generation;
            self.restore_validated(&validated);
            return Err(err);
        }
        result
    }

    /// A scratch copy of this manager — same options, same model files in the
    /// same order — in which `model_file` is registered under its namespace,
    /// in place of whatever file this manager itself holds there (appended
    /// last when it holds none). Used to validate a model file that this
    /// manager never registered ([`ModelManager::validate_detached_model_file`]):
    /// every check `validate_model_file` runs resolves a namespace *through
    /// the manager*, so the file under validation must be the one registered
    /// under its own namespace for its local types to resolve to itself, as
    /// TS's `this.isLocalType`/`this.getLocalType` always do (P2-08).
    ///
    /// P5-18 (accordproject/concerto-rust#316): the copy shares this
    /// manager's model files (`Arc`) rather than deep-cloning them, and
    /// `model_file` is registered as given, not copied (A-4). When this manager holds nothing
    /// under `model_file`'s namespace (`addModelFile`'s validate-before-
    /// register, the common case), the copy is this manager's arena as it
    /// stands, with `model_file` appended: re-registering the same files in
    /// the same order would rebuild exactly those tables, since every handle
    /// is its slot's position. Otherwise `model_file` takes the old file's
    /// place in the order, so the tables after it are rebuilt, as before.
    /// Either way the copy starts with empty caches and ends at the same
    /// generation as a copy built file by file.
    pub(crate) fn with_model_file_registered(&self, model_file: Arc<ModelFile>) -> Result<Self> {
        let namespace = model_file.namespace().to_string();
        let namespace = namespace.as_str();
        let appended =
            !self.namespaces.contains_key(namespace) && self.namespaces.len() == self.files.len();
        let mut scratch = Self {
            options: self.options.clone(),
            ..Self::default()
        };
        if appended {
            scratch.files = self.files.clone();
            scratch.namespaces = self.namespaces.clone();
            scratch.declarations = self.declarations.clone();
            scratch.properties = self.properties.clone();
            // One `insert` per file, as a copy built file by file counts.
            scratch.generation = u64::try_from(self.files.len()).unwrap_or(u64::MAX);
            scratch.insert_shared(model_file)?;
            return Ok(scratch);
        }
        let mut placed = false;
        for existing in &self.files {
            if existing.model_file.namespace() == namespace {
                scratch.insert_shared(Arc::clone(&model_file))?;
                placed = true;
            } else {
                scratch.insert_shared(Arc::clone(&existing.model_file))?;
            }
        }
        if !placed {
            scratch.insert_shared(model_file)?;
        }
        Ok(scratch)
    }

    /// Appends a model file, its declarations and their properties to the
    /// arena, and counts the mutation. Nothing is changed if a handle cannot
    /// be allocated.
    fn insert(&mut self, model_file: ModelFile) -> Result<ModelFileId> {
        self.insert_shared(Arc::new(model_file))
    }

    /// [`ModelManager::insert`] for a model file that may already be
    /// registered in another manager: the file itself is shared, not
    /// copied (P5-18).
    fn insert_shared(&mut self, model_file: Arc<ModelFile>) -> Result<ModelFileId> {
        // P5-97: an append keeps every cached answer that cannot change
        // (`keep_caches_for_append`).
        self.keep_caches_for_append();
        let file_id = ModelFileId(next_index(self.files.len())?);
        let mut declarations = Vec::new();
        let mut properties = Vec::new();
        for (index, declaration) in model_file.declarations().iter().enumerate() {
            let decl_id = DeclId(next_index(self.declarations.len() + declarations.len())?);
            let first = next_index(self.properties.len() + properties.len())?;
            // P2-04: an enum's values get arena-addressed `PropId`s the same
            // way a class declaration's own fields do, through the unified
            // `ClassLike::own_properties` (both `ClassDeclaration` and
            // `EnumDeclaration` are class-like, module doc on `ClassLike`);
            // a scalar or map declaration is neither, so it has no
            // properties at all.
            let own: &[Property] = ClassLike::from_declaration(declaration)
                .map_or(&[][..], |class| class.own_properties());
            properties.extend((0..own.len()).map(|index| PropSlot {
                declaration: decl_id,
                index,
            }));
            let end = next_index(self.properties.len() + properties.len())?;
            declarations.push(DeclSlot {
                model_file: file_id,
                index,
                properties: first..end,
                fqn: Arc::from(qualify(model_file.namespace(), declaration.name())),
            });
        }
        let first = next_index(self.declarations.len())?;
        let end = next_index(self.declarations.len() + declarations.len())?;

        self.namespaces
            .insert(model_file.namespace().to_string(), file_id);
        self.files.push(FileSlot {
            model_file,
            declarations: first..end,
            validated: std::sync::atomic::AtomicBool::new(false),
            proof: None,
        });
        self.declarations.extend(declarations);
        self.properties.extend(properties);
        self.generation += 1;
        Ok(file_id)
    }

    /// A builder for a manager with options: decorator validation,
    /// metamodel validation, and reserved system type names.
    pub fn builder() -> ModelManagerBuilder {
        ModelManagerBuilder::default()
    }

    /// Loads a model from its JSON AST (a `concerto.metamodel@1.0.0.Model`
    /// document), named `file_name`, and returns its handle. The model is
    /// checked for structure only: run [`ModelManager::validate_models`] once
    /// every model is loaded. Loading two models with the same namespace is
    /// an error.
    ///
    /// TS: `ModelManager.addModel` with an AST (`AstModelManager`), without
    /// the validation it runs.
    pub fn add_model_ast(
        &mut self,
        ast: &serde_json::Value,
        file_name: Option<&str>,
    ) -> Result<ModelFileId> {
        let mf = ModelFile::from_json_with_definitions(ast, None, file_name.map(str::to_string))?;
        self.add_loaded_model_file(mf)
    }

    /// [`ModelManager::add_model_ast`] for an AST given as JSON text: the
    /// same result, read without building a `serde_json::Value` first
    /// (P5-06d, accordproject/concerto-rust#239). Text that is not JSON is
    /// an `IllegalModel` error.
    pub fn add_model_ast_text(
        &mut self,
        json: &str,
        file_name: Option<&str>,
    ) -> Result<ModelFileId> {
        let mf = ModelFile::from_json_text(json, None, file_name.map(str::to_string)).map_err(
            |err| {
                Error::illegal_model(
                    format!("invalid JSON: {err}"),
                    file_name.map(str::to_string),
                    None,
                )
            },
        )??;
        self.add_loaded_model_file(mf)
    }

    /// Loads a batch of models irrespective of import order between them,
    /// then validates the whole manager once ([`ModelManager::validate_models`]).
    /// If loading or validation fails, the batch has no effect: every model
    /// this call added is discarded and the error is returned. The handles
    /// come back in the order of `models`.
    ///
    /// TS: `BaseModelManager.addModelFiles`.
    pub fn add_model_asts<'a>(
        &mut self,
        models: impl IntoIterator<Item = (&'a serde_json::Value, Option<&'a str>)>,
    ) -> Result<Vec<ModelFileId>> {
        self.load_models(
            models
                .into_iter()
                .map(|(ast, file_name)| (ast, file_name.map(str::to_string))),
        )
    }

    /// Replaces the loaded model with the same namespace as `ast`, in place,
    /// and returns the new model's handle. As with
    /// [`ModelManager::add_model_ast`], only the structure is checked. Every
    /// handle this manager handed out before the call is invalid after it.
    /// It is an error when no model with that namespace is loaded; the
    /// manager is then unchanged.
    ///
    /// TS: `BaseModelManager.updateModelFile`, without the validation it runs.
    pub fn update_model_ast(
        &mut self,
        ast: &serde_json::Value,
        file_name: Option<&str>,
    ) -> Result<ModelFileId> {
        let mf = ModelFile::from_json(ast, file_name.map(str::to_string))?;
        let namespace = mf.namespace().to_string();
        let updated = self.update_model_file(mf, false)?;
        self.adopt(updated);
        self.model_file_id(&namespace)
            .ok_or_else(|| Error::type_not_found(namespace))
    }

    /// Removes the loaded model for `namespace`, in place. Every handle this
    /// manager handed out before the call is invalid after it. It is an
    /// error when no model with that namespace is loaded; the manager is
    /// then unchanged.
    ///
    /// TS: `BaseModelManager.deleteModelFile`.
    pub fn remove_model(&mut self, namespace: &str) -> Result<()> {
        let deleted = self.delete_model_file(namespace)?;
        self.adopt(deleted);
        Ok(())
    }

    /// TS: `BaseModelManager.getDecoratorValidation`.
    pub fn decorator_validation(
        &self,
    ) -> &crate::introspect::decorator::DecoratorValidationOptions {
        &self.options.decorator_validation
    }

    js_compat_pub! {
        /// Sets the decorator validation options, matching the TS constructor's
        /// `options.decoratorValidation` (there is no separate TS setter; the
        /// port exposes one so a manager already built can still opt in, as this
        /// crate's own tests do).
        pub fn set_decorator_validation(
            &mut self,
            options: crate::introspect::decorator::DecoratorValidationOptions,
        ) {
            // P5-97: validity depends on the options.
            self.clear_validated();
            self.options.decorator_validation = options;
        }
    }

    js_compat_pub! {
        /// TS: `modelFile.getModelManager()?.options?.dangerouslyAllowReservedSystemTypeNamesInUserModels`,
        /// coerced with `Boolean(...)` (`Declaration.validate`, declaration.ts).
        pub fn dangerously_allow_reserved_system_type_names_in_user_models(&self) -> bool {
            self.options.allow_reserved_system_type_names
        }
    }

    js_compat_pub! {
        /// Sets the escape hatch above, matching the TS constructor's
        /// `options.dangerouslyAllowReservedSystemTypeNamesInUserModels` (there is
        /// no separate TS setter; the port exposes one the same way
        /// [`Self::set_decorator_validation`] does).
        pub fn set_dangerously_allow_reserved_system_type_names_in_user_models(&mut self, allow: bool) {
            self.clear_validated();
            self.options.allow_reserved_system_type_names = allow;
        }
    }

    /// TS: `this.options?.metamodelValidation`, as `addModelFile` reads it
    /// (JS truthiness). See [`Self::validate_ast`].
    pub fn metamodel_validation(&self) -> bool {
        self.options.metamodel_validation
    }

    js_compat_pub! {
        /// Sets the option above, matching the TS constructor's
        /// `options.metamodelValidation` (there is no separate TS setter; the
        /// port exposes one the same way [`Self::set_decorator_validation`]
        /// does). This port's `add_model` never validates (validation is an
        /// explicit step), so a caller replaying TS's validating `addModelFile`
        /// runs [`Self::validate_ast`] when this is set, then the new file's
        /// semantic validation (`Self::validate_detached_model_file`).
        pub fn set_metamodel_validation(&mut self, metamodel_validation: bool) {
            self.clear_validated();
            self.options.metamodel_validation = metamodel_validation;
        }
    }

    /// TS `BaseModelManager.validateAst(modelFile)` (`src/basemodelmanager.ts`,
    /// task P4-08b): checks `model_file`'s AST against the metamodel,
    /// resolved through *this* manager.
    ///
    /// 1. The version check (`crate::instance::metamodel::check_version`):
    ///    a `MetamodelException` when the AST's `$class` names another
    ///    metamodel version (or none: "version null"), and `getNamespace`'s
    ///    own `Error`/`TypeError` when `$class` is missing or not a string.
    ///    Nothing is added.
    /// 2. Unless this manager already holds `concerto.metamodel@1.0.0`, the
    ///    cached metamodel file is registered without validation
    ///    (`this.addModelFile(this.metamodelModelFile, undefined,
    ///    MetaModelNamespace, true)`), so its types resolve.
    /// 3. `this.getSerializer().fromJSON(modelFile.getAst())`
    ///    (`crate::instance::metamodel::deserialize_ast`); any failure is a
    ///    `MetamodelException` with the underlying message.
    /// 4. On success, the metamodel file added in step 2 is removed again
    ///    (`this.deleteModelFile(MetaModelNamespace)`).
    ///
    /// **A failure in step 3 leaves the metamodel registered**, exactly as TS
    /// does: its `deleteModelFile` runs after the `try`/`catch` that
    /// re-throws, so it is never reached when the check fails, and the
    /// manager keeps `concerto.metamodel@1.0.0` (visible to
    /// `getModelFiles`, `getAst` and every later `validateAst`, which then
    /// finds it already there).
    ///
    /// Only the file's AST is read. When this manager does not hold the
    /// metamodel, steps 2 to 4 first run against a resident manager that
    /// holds it (P5-13, `validate_ast_value`), with the same outcome and the
    /// same end state.
    pub fn validate_ast(&mut self, model_file: &ModelFile) -> Result<()> {
        self.validate_ast_value(model_file.ast())
    }

    js_compat_pub! {
        /// [`ModelManager::validate_ast`] over the AST itself (P5-13,
        /// accordproject/concerto-rust#297): the check reads nothing of the
        /// model file but its AST, so a caller that holds only the AST (the
        /// concerto-wasm `validateAstValue` binding, whose TS caller already
        /// holds the `ModelFile`) need not build one first. Building one
        /// would also reject some ASTs with the `ModelFile` constructor's own
        /// `IllegalModelException` (a missing `namespace`, `declarations`
        /// that is not an array) before the check could throw TS's
        /// `MetamodelException`.
        ///
        /// When this manager does not hold the metamodel, the structural
        /// check first runs against a resident, per-thread manager that holds
        /// it permanently, with its caches warm, so a document that passes
        /// costs no metamodel registration, removal or cache clearing here;
        /// `self` is then left exactly as TS's add-then-delete leaves it.
        /// That answer is used only when it is a pass and this manager's
        /// system model files are the resident's own: every type such a check
        /// resolves is then in a namespace whose file is the same in both
        /// managers, so the check in `self` passes too. Anything else — a
        /// failure, or a manager whose system files were replaced or
        /// removed — runs the check in `self`, as below, so the error, and
        /// the metamodel left registered by a failure, are exactly TS's.
        pub fn validate_ast_value(&mut self, ast: &Value) -> Result<()> {
            use crate::instance::metamodel::{
                METAMODEL_NAMESPACE, check_version, deserialize_ast, metamodel_model_file,
            };
            check_version(ast)?;
            let already_has_metamodel = self.model_file(METAMODEL_NAMESPACE).is_some();
            if !already_has_metamodel && self.passes_on_resident_metamodel(ast) {
                return Ok(());
            }
            let files_len = self.files.len();
            let declarations_len = self.declarations.len();
            let properties_len = self.properties.len();
            if !already_has_metamodel {
                self.insert_shared(metamodel_model_file()?)?;
            }
            deserialize_ast(self, ast)?;
            if !already_has_metamodel {
                // `deleteModelFile(MetaModelNamespace)`: the metamodel is the
                // arena's tail (nothing else was added since step 2), so
                // removing it is truncating each table back, as `add_models`'s
                // rollback does; the removal is a mutation of its own.
                self.files.truncate(files_len);
                self.declarations.truncate(declarations_len);
                self.properties.truncate(properties_len);
                self.invalidate_caches();
                self.namespaces.remove(METAMODEL_NAMESPACE);
                self.generation += 1;
            }
            Ok(())
        }
    }

    /// Whether `ast` passes the structural check on the resident metamodel
    /// manager ([`ModelManager::validate_ast_value`]), when this manager's
    /// system model files match that manager's. `false` also when the
    /// resident manager cannot be built, so the caller's own path reports
    /// that error.
    fn passes_on_resident_metamodel(&self, ast: &Value) -> bool {
        use crate::instance::metamodel::{deserialize_ast, metamodel_model_file};
        thread_local! {
            static RESIDENT: std::cell::RefCell<Option<ModelManager>> =
                const { std::cell::RefCell::new(None) };
        }
        RESIDENT.with(|cell| {
            let Ok(mut cell) = cell.try_borrow_mut() else {
                return false;
            };
            if cell.is_none() {
                let Ok(mut resident) = ModelManager::new() else {
                    return false;
                };
                let Ok(metamodel) = metamodel_model_file() else {
                    return false;
                };
                if resident.insert_shared(metamodel).is_err() {
                    return false;
                }
                *cell = Some(resident);
            }
            let Some(resident) = cell.as_ref() else {
                return false;
            };
            self.has_system_files_of(resident) && deserialize_ast(resident, ast).is_ok()
        })
    }

    /// Whether this manager holds the same decorator and root model files
    /// as `other` (by AST, which is all a model file's lookups are built
    /// from). Answered once per [`ModelManager::generation`].
    fn has_system_files_of(&self, other: &ModelManager) -> bool {
        use std::sync::atomic::Ordering;
        // `generation + 1`, so that the default 0 means "not checked".
        let checked = self.generation.wrapping_add(1);
        if self.system_files_checked.load(Ordering::Relaxed) == checked {
            return true;
        }
        let same = EXCLUDE_NS
            .iter()
            .all(|ns| match (self.model_file(ns), other.model_file(ns)) {
                (Some(mine), Some(theirs)) => mine.ast() == theirs.ast(),
                (None, None) => true,
                _ => false,
            });
        if same {
            self.system_files_checked.store(checked, Ordering::Relaxed);
        }
        same
    }

    /// TS `new ModelManager({ addMetamodel: true })` (`src/basemodelmanager.ts`
    /// constructor; accordproject/concerto-rust#265): registers the cached
    /// metamodel file, `concerto.metamodel@1.0.0` under its own namespace as
    /// file name (`this.addModelFile(this.metamodelModelFile)`), through
    /// `addModelFile`'s validating path. There is no TS option on this
    /// port's [`ModelManager::new`]; a caller replaying the option calls this
    /// right after construction and after setting the other options, as the
    /// TS constructor adds the file last.
    ///
    /// `addModelFile` order: a namespace already registered is the
    /// already-exists error (TS `_throwAlreadyExists`) without any
    /// validation; otherwise, when [`Self::metamodel_validation`] is set, the
    /// metamodel file itself is checked with [`Self::validate_ast`], then its
    /// semantic validation runs (`Self::validate_detached_model_file`), and
    /// only then is it registered.
    pub fn add_metamodel(&mut self) -> Result<()> {
        let model_file = crate::instance::metamodel::metamodel_model_file()?;
        if self.model_file(model_file.namespace()).is_none() {
            if self.options.metamodel_validation {
                self.validate_ast(&model_file)?;
            }
            self.validate_detached_model_file(&model_file)?;
        }
        self.add_shared_model_file(model_file)
    }

    js_compat_pub! {
        /// A counter that every mutation of the manager increases. A snapshot of
        /// an element taken at one generation is current while the generation is
        /// unchanged. It never repeats an earlier value for a different state:
        /// a manager rebuilt from this one and adopted in its place
        /// ([`ModelManager::adopt`]: an update, a removal, external models)
        /// continues the count (A-3, accordproject/concerto-rust#448), and a
        /// failed batch that is rolled back restores the count it started from
        /// together with the very state it had then.
        pub fn generation(&self) -> u64 {
            self.generation
        }
    }

    js_compat_pub! {
        /// Replaces this manager with `next`, one built from it
        /// ([`ModelManager::update_model_file`],
        /// [`ModelManager::delete_model_file`]), as one mutation: `next`
        /// continues this manager's [`ModelManager::generation`] (A-3,
        /// accordproject/concerto-rust#448), so a snapshot taken before is
        /// never taken as current after. A rebuilt manager's own count
        /// restarts with its arena, and adopting it as it is could repeat an
        /// earlier generation.
        pub fn adopt(&mut self, mut next: Self) {
            next.generation = self.generation.wrapping_add(1);
            next.system_files_checked = std::sync::atomic::AtomicU64::new(0);
            *self = next;
        }
    }

    /// The loaded model file for a namespace, if there is one.
    pub fn model_file(&self, namespace: &str) -> Option<&ModelFile> {
        self.model_file_id(namespace).and_then(|id| self.file(id))
    }

    /// Every loaded model file, including the built-in decorator and root
    /// models, in the order they were loaded.
    pub fn model_files(&self) -> impl Iterator<Item = &ModelFile> {
        self.files.iter().map(|slot| &*slot.model_file)
    }

    /// TS: `BaseModelManager.getModelFileByFileName(fileName)` —
    /// `this.getModelFiles().filter(mf => mf.getName() === fileName)[0]`.
    /// `getModelFiles()` called with no argument excludes the built-in
    /// decorator and root models (`EXCLUDE_NS`), so this searches the
    /// same filtered set, not [`model_files`]: the first loaded,
    /// non-system model file (registration order) whose `getName()`
    /// equals `file_name`, or `None` (JS `undefined`) if none does —
    /// including when `file_name` names one of the system files
    /// (`concerto_1.0.0.cto`, `concerto_decorator_1.0.0.cto`), which TS
    /// never returns from this default-argument call.
    ///
    /// [`model_files`]: Self::model_files
    pub fn model_file_by_file_name(&self, file_name: &str) -> Option<&ModelFile> {
        self.model_file_by_optional_file_name(Some(file_name))
    }

    js_compat_pub! {
        /// [`model_file_by_file_name`] for a `fileName` that may be JS
        /// `undefined`. TS compares with `mf.getName() === fileName`, so an
        /// omitted or `undefined` argument matches the first non-system model
        /// file that was loaded without a file name (for example
        /// `addCTOModel(text)` or `addModel(ast)` with no `fileName`), whose
        /// `getName()` is `undefined`. `None` here finds that file, the one
        /// whose [`ModelFile::file_name`] is `None`.
        ///
        /// [`model_file_by_file_name`]: Self::model_file_by_file_name
        pub fn model_file_by_optional_file_name(
            &self,
            file_name: Option<&str>,
        ) -> Option<&ModelFile> {
            self.model_files()
                .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
                .find(|mf| mf.file_name() == file_name)
        }
    }

    /// The handle of the loaded model file for a namespace, if there is one.
    pub fn model_file_id(&self, namespace: &str) -> Option<ModelFileId> {
        self.namespaces.get(namespace).copied()
    }

    /// The handle of a declaration, by its fully-qualified name. The lookup is
    /// exact, as [`ModelManager::get_declaration`]'s is.
    pub fn declaration_id(&self, fqn: &str) -> Option<DeclId> {
        self.local_type(self.model_file_id(namespace_of(fqn))?, short_name(fqn))
    }

    /// The model file a handle names.
    pub fn file(&self, id: ModelFileId) -> Option<&ModelFile> {
        self.files.get(id.slot()).map(|slot| &*slot.model_file)
    }

    /// The declaration a handle names.
    pub fn declaration(&self, id: DeclId) -> Option<&Declaration> {
        let slot = self.declarations.get(id.slot())?;
        self.file(slot.model_file)?.declarations().get(slot.index)
    }

    js_compat_pub! {
        /// The property a handle names. Resolves through `ClassLike` so that
        /// an enum's own values (P2-04), addressed the same as a class
        /// declaration's fields (`insert`'s doc comment), resolve here too.
        pub fn property_by_id(&self, id: PropId) -> Option<&Property> {
            let slot = self.properties.get(id.slot())?;
            ClassLike::from_declaration(self.declaration(slot.declaration)?)?
                .own_properties()
                .get(slot.index)
        }
    }

    js_compat_pub! {
        /// The handles of a model file's declarations, in the order they appear
        /// in the file. None for a handle the manager never handed out.
        pub fn declaration_ids(&self, file: ModelFileId) -> impl Iterator<Item = DeclId> + use<> {
            self.files
                .get(file.slot())
                .map_or(0..0, |slot| slot.declarations.clone())
                .map(DeclId)
        }
    }

    js_compat_pub! {
        /// Every class-like or enum declaration across every loaded model file
        /// whose namespace is not in `EXCLUDE_NS`, in file order and then
        /// declaration order — a map or scalar declaration is left out, as is
        /// the decorator and root models' own declarations (P2-08 review: this
        /// previously iterated every loaded file, `EXCLUDE_NS` included, which
        /// put system declarations like `Concept` into the result).
        ///
        /// TS: `Introspector.getClassDeclarations` (src/introspect/introspector.ts):
        /// `modelFile.getAllDeclarations().filter(d =>
        /// !d.isMapDeclaration?.() && !d.isScalarDeclaration?.())`, concatenated
        /// over `modelManager.getModelFiles()` — which, called with no argument,
        /// already leaves the system and decorator models out by their
        /// namespace string (`EXCLUDE_NS`), not by `ModelFile.isSystemModelFile`
        /// (`getModelFiles`, src/basemodelmanager.ts). Delegates to
        /// `Self::all_class_like`, which [`Self::get_assignable_class_declarations`]
        /// and [`Self::get_direct_subclasses`] already search this same way.
        pub fn class_declarations(&self) -> impl Iterator<Item = DeclId> + '_ {
            self.all_class_like().map(|(_, id)| id)
        }
    }

    js_compat_pub! {
        /// The handles of a class declaration's own properties, in the order they
        /// are declared. None for any other declaration, or for a handle the
        /// manager never handed out.
        pub fn property_ids(&self, declaration: DeclId) -> impl Iterator<Item = PropId> + use<> {
            self.declarations
                .get(declaration.slot())
                .map_or(0..0, |slot| slot.properties.clone())
                .map(PropId)
        }
    }

    js_compat_pub! {
        /// The model file a declaration belongs to.
        ///
        /// TS: Declaration.getModelFile (src/introspect/declaration.ts)
        pub fn model_file_of(&self, declaration: DeclId) -> Option<ModelFileId> {
            self.declarations
                .get(declaration.slot())
                .map(|slot| slot.model_file)
        }
    }

    js_compat_pub! {
        /// The declaration a property belongs to.
        ///
        /// TS: Property.getParent (src/introspect/property.ts)
        pub fn parent_of(&self, property: PropId) -> Option<DeclId> {
            self.properties
                .get(property.slot())
                .map(|slot| slot.declaration)
        }
    }

    js_compat_pub! {
        /// A property's own `defaultValue`, read straight off its raw AST (P2-04).
        ///
        /// TS: `Field.getDefaultValue` (src/introspect/field.ts) reads
        /// `this.ast.defaultValue` regardless of the property's kind, with
        /// `Util.isNull` treating a JSON `null` the same as an absent key. The
        /// generated `mm::DateTimeProperty` carries no `defaultValue` field at
        /// all (the official metamodel does not declare one there, unlike the
        /// other five field kinds), so a typed per-variant read would silently
        /// lose a `DateTime` field's default; reading the raw AST instead, the
        /// same as TS, keeps it. `None` for a handle the manager never handed
        /// out, matching every other `PropId` lookup here.
        pub fn property_default_value(&self, id: PropId) -> Option<&Value> {
            let slot = self.properties.get(id.slot())?;
            self.declaration_ast(slot.declaration)?
                .get("properties")?
                .get(slot.index)?
                .get("defaultValue")
                .filter(|v| !v.is_null())
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getType(qualifiedName)` (basemodelmanager.ts):
        /// the model file registered under the name's namespace, then that
        /// file's own `getType(qualifiedName)`, each failure its own
        /// `TypeNotFoundException` — `Namespace is not defined for type "<fqn>".`
        /// when no file holds the namespace, `Type "<short>" is not defined in
        /// namespace "<ns>".` when the file has no such type (P2-08 review).
        pub fn get_type_declaration(&self, qualified_name: &str) -> Result<DeclId> {
            let namespace = get_namespace(Some(qualified_name))?;
            let Some(file) = self.model_file_id(namespace) else {
                return Err(ContractError::type_not_found(
                    "modelmanager-gettype-noregisteredns",
                    vec![("type", qualified_name.to_string())],
                    qualified_name.to_string(),
                    None,
                )
                .into());
            };
            match ResolutionContext::get_type(self, &Node::ModelFile(file), Some(qualified_name))? {
                Some(Node::Declaration(id)) => Ok(id),
                _ => Err(ContractError::type_not_found(
                    "modelmanager-gettype-notypeinns",
                    vec![
                        ("type", short_name(qualified_name).to_string()),
                        ("namespace", namespace.to_string()),
                    ],
                    qualified_name.to_string(),
                    None,
                )
                .into()),
            }
        }
    }

    /// Every declaration of every loaded model file, the system models
    /// included, with its fully-qualified name: files in load order, then
    /// declarations in AST order.
    pub fn declarations(&self) -> impl Iterator<Item = (String, &Declaration)> {
        self.model_files().flat_map(|mf| {
            mf.declarations()
                .iter()
                .map(move |declaration| (qualify(mf.namespace(), declaration.name()), declaration))
        })
    }

    /// The class declarations of one kind, with their fully-qualified names,
    /// across the user models (the system models left out), in load order.
    /// The kind is matched exactly, not through inheritance.
    ///
    /// TS: `BaseModelManager.getAssetDeclarations`, `getConceptDeclarations`
    /// and the other `get<Kind>Declarations`.
    pub fn class_declarations_of_kind(
        &self,
        kind: ClassKind,
    ) -> impl Iterator<Item = (String, &ClassDeclaration)> {
        self.user_model_files().flat_map(move |mf| {
            mf.declarations().iter().filter_map(move |declaration| {
                let class = declaration.as_class()?;
                (class.kind() == kind).then(|| (qualify(mf.namespace(), class.name()), class))
            })
        })
    }

    /// The enum declarations of the user models (the system models left
    /// out), with their fully-qualified names, in load order.
    ///
    /// TS: `BaseModelManager.getEnumDeclarations`.
    pub fn enum_declarations(&self) -> impl Iterator<Item = (String, &EnumDeclaration)> {
        self.user_model_files().flat_map(|mf| {
            mf.declarations()
                .iter()
                .filter_map(move |declaration| match declaration {
                    Declaration::Enum(e) => Some((qualify(mf.namespace(), e.name()), e)),
                    Declaration::Class(_) | Declaration::Scalar(_) | Declaration::Map(_) => None,
                })
        })
    }

    /// The loaded model files outside `EXCLUDE_NS`, in load order: what TS's
    /// `getModelFiles()` returns.
    fn user_model_files(&self) -> impl Iterator<Item = &ModelFile> {
        self.model_files()
            .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
    }

    /// The direct super type of the concept-like or enum type `fqn`, with its
    /// fully-qualified name, or `None` when it has none (only the system
    /// `Concept`).
    ///
    /// TS: `ClassDeclaration.getSuperTypeDeclaration`.
    pub fn super_type(&self, fqn: &str) -> Result<Option<(String, &Declaration)>> {
        let Some(super_fqn) = self.super_type_name(fqn)? else {
            return Ok(None);
        };
        let declaration = self.get_declaration(&super_fqn)?;
        Ok(Some((super_fqn, declaration)))
    }

    /// Every super type of `fqn`, from its direct super type up to the root,
    /// with their fully-qualified names. A cyclic chain is an
    /// `IllegalModel` error (BC-11).
    ///
    /// TS: `ClassDeclaration.getAllSuperTypeDeclarations`.
    pub fn super_types(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>> {
        self.with_declarations(self.super_type_names(fqn)?)
    }

    /// The declarations that directly extend `fqn`, with their
    /// fully-qualified names, in load order.
    ///
    /// TS: `ClassDeclaration.getDirectSubclasses`.
    pub fn subclasses(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>> {
        self.with_declarations(self.direct_subclass_names(fqn)?)
    }

    /// `fqn` itself and every declaration that transitively extends it, with
    /// their fully-qualified names: what a value of type `fqn` can be.
    ///
    /// TS: `ClassDeclaration.getAssignableClassDeclarations`.
    pub fn assignable_types(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>> {
        self.with_declarations(self.assignable_type_names(fqn)?)
    }

    /// Each name, with its declaration.
    fn with_declarations(&self, names: Vec<String>) -> Result<Vec<(String, &Declaration)>> {
        names
            .into_iter()
            .map(|name| {
                let declaration = self.get_declaration(&name)?;
                Ok((name, declaration))
            })
            .collect()
    }

    /// The properties declared directly on the concept-like or enum type
    /// `fqn` (for an enum, its values), not those it inherits.
    ///
    /// TS: `ClassDeclaration.getOwnProperties`.
    pub fn own_properties(&self, fqn: &str) -> Result<&[Property]> {
        let class = ClassLike::from_declaration(self.get_declaration(fqn)?)
            .ok_or_else(|| not_a_class_like(fqn))?;
        Ok(class.own_properties())
    }

    /// Every property of `fqn`, own and inherited, with the fully-qualified
    /// name of the declaration that declares each: the type's own first,
    /// then each super type's up to the root. It is an error when `fqn` is
    /// not a concept-like or enum type, a super type cannot be resolved, or
    /// the chain is cyclic (`IllegalModel`, BC-11).
    ///
    /// TS: `ClassDeclaration.getProperties`, with `Property.getParent()`.
    pub fn properties(&self, fqn: &str) -> Result<Vec<(String, &Property)>> {
        Ok(self
            .class_properties(fqn)?
            .iter()
            .map(|(owner, property)| (owner.to_string(), property))
            .collect())
    }

    /// [`ModelManager::properties`], borrowed from the model (P5-13): the
    /// same properties, owners, order and errors, with nothing copied.
    pub(crate) fn class_properties(&self, fqn: &str) -> Result<ClassProperties<'_>> {
        Ok(ClassProperties {
            mm: self,
            info: self.class_info(fqn)?,
        })
    }

    js_compat_pub! {
        /// [`ModelManager::properties`], borrowed from the model, for a
        /// declaration handle (P5-13).
        pub fn class_properties_of(&self, id: DeclId) -> Result<ClassProperties<'_>> {
            Ok(ClassProperties {
                mm: self,
                info: self.class_info_of(id)?,
            })
        }
    }

    /// A property and the fully-qualified name of its declaration.
    fn property_with_owner(&self, id: PropId) -> Option<(&str, &Property)> {
        let slot = self.properties.get(id.slot())?;
        let owner = self.declarations.get(slot.declaration.slot())?;
        Some((&*owner.fqn, self.property_by_id(id)?))
    }

    /// The property called `name`, own or inherited, with the
    /// fully-qualified name of the declaration that declares it, or `None`.
    ///
    /// TS: `ClassDeclaration.getProperty`.
    pub fn property(&self, fqn: &str, name: &str) -> Result<Option<(String, &Property)>> {
        Ok(self
            .class_properties(fqn)?
            .find(name)
            .map(|(owner, property)| (owner.to_string(), property)))
    }

    /// The property at a dotted `path` (`a.b.c`), following the declared type
    /// of each property but the last, with the fully-qualified name of the
    /// declaration that declares it.
    ///
    /// TS: `ClassDeclaration.getNestedProperty`.
    pub fn property_path(&self, fqn: &str, path: &str) -> Result<(String, &Property)> {
        let (owner, found) = self.nested_property(fqn, path)?;
        let property = self
            .own_properties(&owner)?
            .iter()
            .find(|p| p.name() == found.name())
            .ok_or_else(|| Error::type_not_found(qualify(&owner, found.name())))?;
        Ok((owner, property))
    }

    /// The name of the field that identifies instances of `fqn`: its own
    /// (an `identified by` field, or `$identifier` for `identified`), or its
    /// nearest super type's. `None` when nothing in the chain declares one.
    ///
    /// TS: `ClassDeclaration.getIdentifierFieldName`.
    pub fn identifier_field(&self, fqn: &str) -> Result<Option<&str>> {
        let info = self.class_info(fqn)?;
        Ok(self.chain_identifier_field(&info))
    }

    js_compat_pub! {
        /// [`ModelManager::identifier_field`] for a declaration handle (P5-13).
        pub fn identifier_field_of(&self, id: DeclId) -> Result<Option<&str>> {
            let info = self.class_info_of(id)?;
            Ok(self.chain_identifier_field(&info))
        }
    }

    /// The nearest identifying field along a cached chain.
    fn chain_identifier_field(&self, info: &ClassInfo) -> Option<&str> {
        info.chain.iter().find_map(|id| {
            self.declaration(*id)
                .and_then(ClassLike::from_declaration)
                .and_then(|class| class.own_identifier_field_name())
        })
    }

    /// Every loaded model's AST, in load order, in the metamodel's `Models`
    /// envelope. [`AstOptions`] chooses whether the system models are
    /// included and whether type names are resolved to their namespaces.
    ///
    /// TS: `BaseModelManager.getAst(resolve, includeConcertoNamespaces)`.
    pub fn ast(&self, options: AstOptions) -> Result<Value> {
        self.models_ast(options.resolve, options.include_system_models)
    }

    /// Looks up a declaration by its fully-qualified name.
    ///
    /// Namespace versions are mandatory in Concerto v4, so the lookup is
    /// exact: the name must be written with the versioned namespace it was
    /// declared in.
    pub fn get_declaration(&self, fqn: &str) -> Result<&Declaration> {
        self.declaration_id(fqn)
            .and_then(|id| self.declaration(id))
            .ok_or_else(|| Error::type_not_found(fqn.to_string()))
    }

    /// TS: ModelFile.getFullyQualifiedTypeName (src/introspect/modelfile.ts):
    /// a primitive's own name, an imported name's fully-qualified name, a
    /// local declaration's fully-qualified name, or `None` (JS `null`), for
    /// `type_name` as written in the model file of `namespace`. `None` too
    /// when no model file has that namespace.
    pub fn model_file_fully_qualified_type_name(
        &self,
        namespace: &str,
        type_name: &str,
    ) -> Option<String> {
        let mf = self.model_file(namespace)?;
        if crate::model_util::is_primitive_type(type_name) {
            return Some(type_name.to_string());
        }
        if let Some(fqn) = imported_type(mf, type_name) {
            return Some(fqn);
        }
        let file = self.model_file_id(namespace)?;
        let id = self.local_type(file, type_name)?;
        self.declaration_fqn(id).ok()
    }

    /// Resolves a short name, as written inside `in_namespace`, to its
    /// fully-qualified name, using the primitives, local declarations and named
    /// imports the model file can see.
    pub fn resolve_type_name(&self, in_namespace: &str, short: &str) -> Result<String> {
        self.resolve_type_name_at(in_namespace, short, None)
    }

    js_compat_pub! {
        /// [`ModelManager::resolve_type_name`], with the location of the AST node
        /// the name was read from.
        ///
        /// `location` is the AST node's `location`, copied verbatim into the
        /// error this raises when the namespace is not registered (PORTING.md
        /// section 2.1); pass `None` where the caller has no AST node in scope.
        pub fn resolve_type_name_at(
            &self,
            in_namespace: &str,
            short: &str,
            location: Option<serde_json::Value>,
        ) -> Result<String> {
            self.resolve_type_name_lazy(in_namespace, short, || location)
        }
    }

    /// [`ModelManager::resolve_type_name_at`], building the location only
    /// when the error that carries it is raised (P5-48).
    pub(crate) fn resolve_type_name_lazy(
        &self,
        in_namespace: &str,
        short: &str,
        location: impl FnOnce() -> Option<serde_json::Value>,
    ) -> Result<String> {
        let mf = self.model_file(in_namespace).ok_or_else(|| {
            // TS: BaseModelManager.getType's unregistered-namespace path
            // (src/basemodelmanager.ts), reused for the equivalent check
            // here (error/catalogue.rs doc comment on the entry).
            let fqn = qualify(in_namespace, short);
            ContractError::type_not_found(
                "modelmanager-gettype-noregisteredns",
                vec![("type", fqn.clone())],
                fqn,
                location(),
            )
        })?;

        mf.resolve_local_type(short)
            .ok_or_else(|| Error::type_not_found(qualify(in_namespace, short)))
    }

    /// The name of the field that gives `fqn` its identity: its own, if it
    /// declares one (explicit `identified by field`, giving that field's
    /// name, or system `identified`, giving `$identifier`), otherwise its
    /// nearest super type's, walking up the chain. `None` if nothing from
    /// `fqn` up to the root declares an identity.
    ///
    /// TS: ClassDeclaration.getIdentifierFieldName
    /// (src/introspect/classdeclaration.ts) — including its two callers that
    /// are themselves inherited, `isIdentified` (`!!getIdentifierFieldName()`)
    /// and `isSystemIdentified` (`getIdentifierFieldName() === '$identifier'`),
    /// which have no separate Rust method: a caller after either compares
    /// this result directly, the same way TS's own body does. Contrast
    /// [`ClassDeclaration::identifier_field_name`] and
    /// [`ClassDeclaration::is_identified`], which read only `fqn`'s own AST,
    /// the same as TS's own (non-inherited) `idField`.
    #[deprecated(since = "0.1.0", note = "use `identifier_field`")]
    pub fn identifier_field_name(&self, fqn: &str) -> Result<Option<String>> {
        Ok(self.identifier_field(fqn)?.map(str::to_string))
    }

    /// [`ModelManager::identifier_field_name`], as a boolean.
    ///
    /// TS: `ClassDeclaration.isIdentified` (src/introspect/classdeclaration.ts):
    /// `!!this.getIdentifierFieldName()`, inherited unchanged by `EnumDeclaration`.
    pub fn is_identified(&self, fqn: &str) -> Result<bool> {
        Ok(self.identifier_field(fqn)?.is_some())
    }

    /// [`ModelManager::identifier_field_name`], `true` only for the system
    /// `$identifier`.
    ///
    /// TS: `ClassDeclaration.isSystemIdentified`: `this.getIdentifierFieldName()
    /// === '$identifier'`, inherited unchanged by `EnumDeclaration`.
    pub fn is_system_identified(&self, fqn: &str) -> Result<bool> {
        Ok(self.identifier_field(fqn)? == Some("$identifier"))
    }

    /// Every property of a type, own and inherited: the type's own first,
    /// then each super type's up to the root ([`ModelManager::properties`]
    /// gives each with the name of the declaration that declares it). It is
    /// an error when `fqn` is not a concept-like or enum type, a super type
    /// cannot be resolved, or the chain is cyclic (`RecursionLimit`).
    ///
    /// TS: `ClassDeclaration.getProperties`.
    pub fn get_all_properties(&self, fqn: &str) -> Result<Vec<&Property>> {
        Ok(self
            .class_properties(fqn)?
            .iter()
            .map(|(_, property)| property)
            .collect())
    }

    /// The property with a given name, own or inherited, or `None` if it does
    /// not exist, alongside its declaring type's fully-qualified name (see
    /// [`ModelManager::get_all_properties`]).
    ///
    /// TS: `ClassDeclaration.getProperty`, inherited unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `property`")]
    pub fn get_property(&self, fqn: &str, name: &str) -> Result<Option<(String, Property)>> {
        Ok(self
            .property(fqn, name)?
            .map(|(owner, property)| (owner, property.clone())))
    }

    /// The properties declared directly on `fqn`, not those it inherits.
    ///
    /// TS: `ClassDeclaration.getOwnProperties`, inherited unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `own_properties`")]
    pub fn get_own_properties(&self, fqn: &str) -> Result<Vec<Property>> {
        Ok(self.own_properties(fqn)?.to_vec())
    }

    /// A nested property, following a dotted path (`a.b.c`) through the
    /// declared types of each element but the last.
    ///
    /// TS: `ClassDeclaration.getNestedProperty` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `property_path`")]
    pub fn get_nested_property(
        &self,
        fqn: &str,
        property_path: &str,
    ) -> Result<(String, Property)> {
        self.nested_property(fqn, property_path)
    }

    /// A nested property, following a dotted path (`a.b.c`) through the
    /// declared types of each element but the last.
    ///
    /// TS: `ClassDeclaration.getNestedProperty` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    fn nested_property(&self, fqn: &str, property_path: &str) -> Result<(String, Property)> {
        let names: Vec<&str> = property_path.split('.').collect();
        let mut search_root = fqn.to_string();
        let mut result = None;
        for (n, name) in names.iter().enumerate() {
            let Some((declaring_fqn, property)) = self
                .property(&search_root, name)?
                .map(|(owner, property)| (owner, property.clone()))
            else {
                return Err(ContractError::new(
                    ErrorKind::IllegalModel,
                    "classdeclaration-getnestedproperty-doesnotexist",
                    vec![
                        ("propertyName", (*name).to_string()),
                        ("fqn", search_root.clone()),
                    ],
                )
                .into());
            };
            let is_last = n == names.len() - 1;
            if !is_last {
                // TS: `Property.isTypeEnum` (src/introspect/property.ts):
                // `this.isPrimitive() ? false : this.getParent().getModelFile()
                // .getType(this.getType()).isEnum()`. Reached here only for an
                // object/relationship field (the walk's own `get_property`
                // already ruled out a missing property, and an intermediate
                // step is never itself an enum *value* — the field whose
                // declared type is an enum trips this same check one level
                // higher, before the walk ever reaches the value), which is
                // always a `ClassDeclaration`'s own field (an enum value is
                // never itself an intermediate step of a nested path), so
                // its `PropId` is always in the arena (`find_property_id`).
                let is_enum = !property.is_primitive() && {
                    let prop_id = self
                        .find_property_id(&declaring_fqn, name)?
                        .expect("get_property just found this property");
                    model_util::is_enum(self, &Node::Property(prop_id))?.unwrap_or(false)
                };
                if property.is_primitive() || is_enum {
                    return Err(ContractError::new(
                        ErrorKind::InvalidArgument,
                        "classdeclaration-getnestedproperty-primitiveorenum",
                        vec![
                            ("propertyName", (*name).to_string()),
                            ("propertyPath", property_path.to_string()),
                        ],
                    )
                    .into());
                }
                let prop_id = self
                    .find_property_id(&declaring_fqn, name)?
                    .expect("get_property just found this property");
                search_root = self.get_fully_qualified_type_name(&Node::Property(prop_id))?;
            }
            result = Some((declaring_fqn, property));
        }
        Ok(result.expect("propertyPath.split('.') always yields at least one name"))
    }

    /// The [`PropId`] of the property named `name`, declared directly on
    /// `declaring_fqn` (not inherited — the id-level counterpart of
    /// [`ModelManager::get_own_properties`]) — for
    /// [`ModelManager::get_nested_property`]'s recursive step, which needs a
    /// [`Node::Property`] handle to reach the already-ported
    /// `model_util::is_enum` and [`ResolutionContext::get_fully_qualified_type_name`].
    /// Only ever called for a `ClassDeclaration`'s own field (see the
    /// caller); works the same for an enum's own values (P2-04), though the
    /// caller never reaches one.
    fn find_property_id(&self, declaring_fqn: &str, name: &str) -> Result<Option<PropId>> {
        let Some(owner) = self.declaration_id(declaring_fqn) else {
            return Ok(None);
        };
        Ok(self
            .property_ids(owner)
            .find(|id| self.property_by_id(*id).is_some_and(|p| p.name() == name)))
    }

    /// The FQN of `fqn`'s direct super type, or `None` when it has none (only
    /// the system model's own `Concept`).
    ///
    /// TS: `ClassDeclaration.getSuperType` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `super_type`")]
    pub fn get_super_type(&self, fqn: &str) -> Result<Option<String>> {
        self.super_type_name(fqn)
    }

    /// The FQN of `fqn`'s direct super type, or `None` when it has none (only
    /// the system model's own `Concept`).
    ///
    /// TS: `ClassDeclaration.getSuperType` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    fn super_type_name(&self, fqn: &str) -> Result<Option<String>> {
        let class = ClassLike::from_declaration(self.get_declaration(fqn)?)
            .ok_or_else(|| not_a_class_like(fqn))?;
        self.super_type_fqn(&class, namespace_of(fqn))
    }

    js_compat_pub! {
        /// The [`DeclId`] of `fqn`'s direct super type, or `None` when it has
        /// none.
        ///
        /// TS: `ClassDeclaration.getSuperTypeDeclaration`, inherited unchanged by
        /// `EnumDeclaration`.
        pub fn get_super_type_declaration(&self, fqn: &str) -> Result<Option<DeclId>> {
            let Some(super_fqn) = self.super_type_name(fqn)? else {
                return Ok(None);
            };
            Ok(self.declaration_id(&super_fqn))
        }
    }

    /// Every super type of `fqn`, from its direct super type up to the root,
    /// as fully-qualified names.
    ///
    /// TS: `ClassDeclaration.getAllSuperTypeDeclarations`, inherited unchanged
    /// by `EnumDeclaration`. On a cyclic inheritance chain this walks
    /// `super_chain`, so it returns the same `IllegalModelException` naming
    /// the cycle as `getProperties`/`getProperty`/`getIdentifierFieldName`
    /// (BC-11, R1; TS 5.0.0 loops until it runs out of memory, DV-013).
    #[deprecated(since = "0.1.0", note = "use `super_types`")]
    pub fn get_all_super_type_names(&self, fqn: &str) -> Result<Vec<String>> {
        self.super_type_names(fqn)
    }

    /// Every super type of `fqn`, from its direct super type up to the root,
    /// as fully-qualified names.
    ///
    /// TS: `ClassDeclaration.getAllSuperTypeDeclarations`, inherited unchanged
    /// by `EnumDeclaration`. On a cyclic inheritance chain this walks
    /// `super_chain`, so it returns the same `IllegalModelException` naming
    /// the cycle as `getProperties`/`getProperty`/`getIdentifierFieldName`
    /// (BC-11, R1; TS 5.0.0 loops until it runs out of memory, DV-013).
    fn super_type_names(&self, fqn: &str) -> Result<Vec<String>> {
        Ok(self
            .super_chain(fqn)?
            .into_iter()
            .skip(1)
            .map(|(fqn, _)| fqn)
            .collect())
    }

    /// Every class-like or enum declaration loaded, across every model file
    /// whose namespace is not in [`EXCLUDE_NS`], in registration order — the
    /// population `getAssignableClassDeclarations` and `getDirectSubclasses`
    /// search (TS: `new Introspector(modelManager).getClassDeclarations()`,
    /// src/introspect/introspector.ts, which reads
    /// `modelManager.getModelFiles()` with no argument, so the system and
    /// decorator models are left out by their namespace string, not by
    /// `ModelFile.isSystemModelFile`; then every declaration that is not a
    /// map or a scalar, which leaves the class-like kinds and
    /// `EnumDeclaration`).
    fn all_class_like(&self) -> impl Iterator<Item = (String, DeclId)> + '_ {
        self.model_files()
            .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
            .flat_map(move |mf| {
                let file = self.model_file_id(mf.namespace()).expect("just iterated");
                self.declaration_ids(file).filter_map(move |id| {
                    let declaration = self.declaration(id)?;
                    ClassLike::from_declaration(declaration)?;
                    Some((format!("{}.{}", mf.namespace(), declaration.name()), id))
                })
            })
    }

    /// `fqn` itself, plus every declaration that (transitively) extends it.
    ///
    /// TS: `ClassDeclaration.getAssignableClassDeclarations`, inherited
    /// unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `assignable_types`")]
    pub fn get_assignable_class_declarations(&self, fqn: &str) -> Result<Vec<String>> {
        self.assignable_type_names(fqn)
    }

    /// `fqn` itself, plus every declaration that (transitively) extends it.
    ///
    /// TS: `ClassDeclaration.getAssignableClassDeclarations`, inherited
    /// unchanged by `EnumDeclaration`.
    fn assignable_type_names(&self, fqn: &str) -> Result<Vec<String>> {
        // Builds the same `subclassMap` TS does: every loaded class-like
        // declaration's direct super type FQN to the declarations that name
        // it, in the order they were first seen walking the population.
        let mut subclasses: HashMap<String, Vec<String>> = HashMap::new();
        for (child_fqn, id) in self.all_class_like() {
            let class =
                ClassLike::from_declaration(self.declaration(id).expect("all_class_like found it"))
                    .expect("all_class_like already filtered to class-like");
            if let Some(super_fqn) = self.super_type_fqn(&class, namespace_of(&child_fqn))? {
                subclasses.entry(super_fqn).or_default().push(child_fqn);
            }
        }
        // TS's `collectSubclasses` is a pre-order walk from `[this]` that adds
        // each declaration to a `Set` (so a later revisit is a no-op) before
        // recursing into its own direct subclasses.
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        let mut stack = vec![fqn.to_string()];
        while let Some(current) = stack.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let mut children = subclasses.remove(&current).unwrap_or_default();
            results.push(current);
            children.reverse();
            stack.extend(children);
        }
        Ok(results)
    }

    /// Just the declarations that directly extend `fqn`, excluding `fqn`
    /// itself.
    ///
    /// TS: `ClassDeclaration.getDirectSubclasses`, inherited unchanged by
    /// `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `subclasses`")]
    pub fn get_direct_subclasses(&self, fqn: &str) -> Result<Vec<String>> {
        self.direct_subclass_names(fqn)
    }

    /// Just the declarations that directly extend `fqn`, excluding `fqn`
    /// itself.
    ///
    /// TS: `ClassDeclaration.getDirectSubclasses`, inherited unchanged by
    /// `EnumDeclaration`.
    fn direct_subclass_names(&self, fqn: &str) -> Result<Vec<String>> {
        let mut results = Vec::new();
        for (child_fqn, id) in self.all_class_like() {
            let class =
                ClassLike::from_declaration(self.declaration(id).expect("all_class_like found it"))
                    .expect("all_class_like already filtered to class-like");
            if self
                .super_type_fqn(&class, namespace_of(&child_fqn))?
                .as_deref()
                == Some(fqn)
            {
                results.push(child_fqn);
            }
        }
        Ok(results)
    }

    /// Returns `true` if a value of `sub_fqn` is also a valid `super_fqn`: the
    /// two are the same type, or `sub_fqn` transitively extends `super_fqn`.
    ///
    /// On a cyclic inheritance chain this walks `super_chain`, so it returns
    /// the same `IllegalModelException` naming the cycle as
    /// `getProperties`/`getProperty`/`getIdentifierFieldName` (BC-11, R1;
    /// TS 5.0.0 returns `true` when the target is in the cycle and otherwise
    /// loops until it runs out of memory, DV-013).
    pub fn is_assignable_to(&self, sub_fqn: &str, super_fqn: &str) -> Result<bool> {
        if sub_fqn == super_fqn {
            return Ok(true);
        }
        match self.get_declaration(sub_fqn)?.as_class() {
            None => Ok(false),
            Some(_) => {
                let info = self.class_info(sub_fqn)?;
                Ok(info
                    .chain
                    .iter()
                    .any(|id| self.decl_fqn(*id).is_ok_and(|fqn| fqn == super_fqn)))
            }
        }
    }

    /// Walks a class's inheritance chain, handing back each
    /// `(full-name, declaration)` pair from the type up to its root.
    ///
    /// TS walks this chain by recursion (`ClassDeclaration.getProperties`,
    /// `getProperty`, `getIdentifierFieldName`), with no cycle check, so a
    /// cyclic chain overflows V8's stack. This walk is a loop with a
    /// visited set (PORTING.md 2.5 rule 1) and, when it meets a declaration
    /// again, returns the `RangeError` V8 raises (rule 2), after the same
    /// earlier checks: a missing or non-class super type still fails first.
    fn super_chain(&self, fqn: &str) -> Result<Vec<(String, ClassLike<'_>)>> {
        let info = self.class_info(fqn)?;
        info.chain
            .iter()
            .map(|id| {
                let class = self
                    .declaration(*id)
                    .and_then(ClassLike::from_declaration)
                    .ok_or_else(|| unknown(Node::Declaration(*id)))?;
                Ok((self.decl_fqn(*id)?.to_string(), class))
            })
            .collect()
    }

    /// The handle `getType(fqn)` finds (TS `BaseModelManager.getType`): the
    /// exact-name lookup when it succeeds, which is always
    /// [`ModelManager::get_type_declaration`]'s answer too (a
    /// fully-qualified name is never a primitive or an import's local
    /// name), otherwise `get_type_declaration` itself, for its errors.
    fn type_declaration_impl(&self, fqn: &str) -> Result<DeclId> {
        match self.declaration_id(fqn) {
            Some(id) => Ok(id),
            None => self.get_type_declaration(fqn),
        }
    }

    /// The cached [`ClassInfo`] of the declaration `fqn` names, resolved
    /// the way `getType` does ([`ModelManager::type_declaration`]).
    fn class_info(&self, fqn: &str) -> Result<Arc<ClassInfo>> {
        self.class_info_of(self.type_declaration_impl(fqn)?)
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getType(qualifiedName)`'s handle, found by
        /// the exact-name lookup first (P5-13): the same handle and errors
        /// as [`ModelManager::get_type_declaration`].
        pub fn type_declaration(&self, fqn: &str) -> Result<DeclId> {
            self.type_declaration_impl(fqn)
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager._throwAlreadyExists(modelFile)` (P5-11,
        /// accordproject/concerto-rust#287): the plain `Error` for a model
        /// file named `new_file_name` declaring `namespace`, which the model
        /// file registered under it already declares; `Ok` when nothing is
        /// registered under `namespace`.
        pub fn check_namespace_available(
            &self,
            namespace: &str,
            new_file_name: Option<&str>,
        ) -> Result<()> {
            match self.model_file(namespace) {
                Some(existing) => Err(already_exists(namespace, new_file_name, existing)),
                None => Ok(()),
            }
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getType(qualifiedName)`, answered by name
        /// (P5-11, accordproject/concerto-rust#287): the fully-qualified name
        /// of the declaration [`ModelManager::type_declaration`] finds, with
        /// its `TypeNotFoundException`s. The view maps the name to its own
        /// declaration view.
        pub fn type_declaration_name(&self, fqn: &str) -> Result<String> {
            let id = self.type_declaration_impl(fqn)?;
            self.declaration_fqn(id)
        }
    }

    js_compat_pub! {
        /// TS `ModelFile.getType(type)` of the model file `file`, answered by
        /// name (P5-11, accordproject/concerto-rust#287): a primitive type's
        /// own name, the fully-qualified name of the declaration the type
        /// resolves to (a local declaration, or an import's target in the
        /// model file registered under its namespace), or `None` (TS `null`)
        /// when it resolves to neither. A primitive name never contains a
        /// dot and a fully-qualified name always does, so the view tells the
        /// two apart without another call.
        pub fn model_file_type_name(
            &self,
            file: ModelFileId,
            type_name: &str,
        ) -> Result<Option<String>> {
            match ResolutionContext::get_type(self, &Node::ModelFile(file), Some(type_name))? {
                Some(Node::Primitive(primitive)) => Ok(Some(primitive.to_string())),
                Some(Node::Declaration(id)) => self.declaration_fqn(id).map(Some),
                Some(_) | None => Ok(None),
            }
        }
    }

    js_compat_pub! {
        /// TS `ModelFile.resolveType(context, type, fileLocation)` of the
        /// model file `file` (P5-11, accordproject/concerto-rust#287): a
        /// primitive passes; a name the file imports must resolve in the
        /// model file of the import's namespace
        /// ([`ModelManager::resolve_type`], TS
        /// `this.getModelManager().resolveType(context, this.resolveImport(type))`);
        /// any other name must be declared locally, or the
        /// `IllegalModelException` `modelfile-resolvetype-undecltype` naming
        /// this file is raised, at `location` (TS `fileLocation`).
        pub fn model_file_resolve_type(
            &self,
            file: ModelFileId,
            context: &str,
            type_name: &str,
            location: Option<serde_json::Value>,
        ) -> Result<()> {
            if is_primitive_type(type_name) {
                return Ok(());
            }
            let mf = self.file(file).ok_or_else(|| unknown(Node::ModelFile(file)))?;
            if let Some(fqn) = imported_type(mf, type_name) {
                return self.resolve_type(context, &fqn).map(|_| ());
            }
            if mf.is_local_type(type_name) {
                return Ok(());
            }
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "modelfile-resolvetype-undecltype",
                vec![
                    ("type", type_name.to_string()),
                    ("context", context.to_string()),
                ],
            );
            err.model_file = Some(mf.file_name().map(str::to_string));
            err.location = location;
            Err(err.into())
        }
    }

    /// BC-11 (R1): the `IllegalModelException` for a cyclic inheritance
    /// chain. `cycle` is the loop from `repeated` round to the declaration
    /// whose super type is `repeated` again; the error carries `repeated`'s
    /// model file (TS 5.0.0 overflowed V8's stack instead, DV-013).
    fn circular_inheritance(&self, cycle: &[DeclId], repeated: DeclId) -> Error {
        let name = |id: DeclId| self.decl_fqn(id).unwrap_or_default().to_string();
        let path = cycle
            .iter()
            .chain(std::iter::once(&repeated))
            .map(|id| name(*id))
            .collect::<Vec<_>>()
            .join(" -> ");
        let mut err = ContractError::new(
            ErrorKind::IllegalModel,
            "classdeclaration-circularinheritance",
            vec![("type", name(repeated)), ("cycle", path)],
        );
        err.model_file = Some(
            self.model_file_of(repeated)
                .and_then(|file| self.file(file))
                .and_then(ModelFile::file_name)
                .map(str::to_string),
        );
        err.into()
    }

    /// The cached [`ClassInfo`] of a declaration, computed on first use.
    ///
    /// TS walks the chain by recursion (`ClassDeclaration.getProperties`,
    /// `getProperty`, `getIdentifierFieldName`), with no cycle check, so in
    /// TS 5.0.0 a cyclic chain overflowed V8's stack (DV-013). This walk is
    /// a loop with a visited set (PORTING.md 2.5 rule 1) and, when it meets
    /// a declaration again, returns an `IllegalModelException` naming the
    /// cycle (BC-11, R1), after the same earlier checks: a missing or
    /// non-class super type still fails first.
    fn class_info_of(&self, id: DeclId) -> Result<Arc<ClassInfo>> {
        self.decl_cache.get_or_try_insert_with(
            id,
            |facts| &mut facts.class,
            || self.compute_class_info(id),
        )
    }

    /// [`Self::class_info_of`]'s walk, uncached.
    fn compute_class_info(&self, id: DeclId) -> Result<Arc<ClassInfo>> {
        let mut chain = Vec::new();
        let mut properties = Vec::new();
        let mut current = id;
        loop {
            if let Some(start) = chain.iter().position(|seen| *seen == current) {
                return Err(self.circular_inheritance(&chain[start..], current));
            }
            let current_fqn = self.decl_fqn(current)?;
            let declaration = self
                .declaration(current)
                .ok_or_else(|| unknown(Node::Declaration(current)))?;
            let class = ClassLike::from_declaration(declaration)
                .ok_or_else(|| not_a_class_like(current_fqn))?;

            let next = self.super_type_fqn(&class, namespace_of(current_fqn))?;
            chain.push(current);
            properties.extend(self.property_ids(current));
            match next {
                // TS resolves each step of the chain the same way `getType`
                // does (`this.modelManager.getType(this.superType)`,
                // `ClassDeclaration.getSuperTypeDeclaration`), so an
                // unregistered super-type namespace raises `getType`'s own
                // `TypeNotFoundException` ("Namespace is not defined for type
                // ...").
                Some(parent) => current = self.type_declaration_impl(&parent)?,
                None => break,
            }
        }
        Ok(Arc::new(ClassInfo {
            chain: chain.into_boxed_slice(),
            properties: properties.into_boxed_slice(),
        }))
    }

    /// Declaration `id`'s converted field defaults
    /// ([`crate::instance::from_json::assign_field_defaults_of`], P5-13):
    /// `compute`'s answer, computed on first use and then cached until the
    /// registered files change, as the inheritance facts are. An error is
    /// returned, and not cached.
    pub(crate) fn cached_field_defaults(
        &self,
        id: DeclId,
        compute: impl FnOnce() -> Result<crate::instance::from_json::FieldDefaults>,
    ) -> Result<Arc<crate::instance::from_json::FieldDefaults>> {
        self.decl_cache.get_or_try_insert_with(
            id,
            |facts| &mut facts.field_defaults,
            || compute().map(Arc::new),
        )
    }

    /// P5-97 (accordproject/concerto-rust#448): the caches an append
    /// ([`ModelManager::insert_shared`]) leaves valid. Appending a file
    /// adds a namespace no file held (a namespace is never registered
    /// twice) and changes no registered file or handle, so every name that
    /// resolved before resolves to the same declaration after: only a
    /// resolution that failed can change. The caches keep only answers
    /// built from successful resolutions, so they stay:
    ///
    /// - an inheritance chain ([`ClassInfo`]) is cached only once every
    ///   super type resolved;
    /// - an instance fact is cached only when its computation succeeded
    ///   (field defaults: when every field type resolved);
    /// - a validation plan is kept only when nothing in it was left
    ///   unresolved or unplanned ([`crate::instance::plan::ClassPlan::is_settled`]);
    ///   any other plan, and a declaration recorded as having none, is built
    ///   again on next use.
    ///
    /// So adding a request's user files to a manager forked from a base
    /// ([`ModelManager::fork`]) keeps every warmed answer about the base's
    /// declarations, and never changes one. A removal, a rollback and a
    /// rebuild still drop everything ([`ModelManager::invalidate_caches`]).
    fn keep_caches_for_append(&mut self) {
        for facts in self.decl_cache.facts_mut() {
            if !matches!(&facts.plan, Some(Some(plan)) if plan.is_settled()) {
                facts.plan = None;
            }
        }
    }

    /// Drops every answer cached from the registered files (P5-06); called
    /// by every change to them but an append
    /// ([`ModelManager::keep_caches_for_append`]).
    fn invalidate_caches(&mut self) {
        self.decl_cache.facts_mut().clear();
    }

    /// Declaration `id`'s validation plan ([`crate::instance::plan`]), built
    /// by `build` on first use and cached until the registered files
    /// change. The lock is not held while building.
    pub(crate) fn cached_plan(
        &self,
        id: DeclId,
        build: impl FnOnce() -> Option<crate::instance::plan::ClassPlan>,
    ) -> Option<Arc<crate::instance::plan::ClassPlan>> {
        let built: std::result::Result<_, std::convert::Infallible> = self
            .decl_cache
            .get_or_try_insert_with(id, |facts| &mut facts.plan, || Ok(build().map(Arc::new)));
        match built {
            Ok(plan) => plan,
        }
    }

    /// The number of validation plans cached, and of their properties: a
    /// test- and dev-only measure (`validation-plan-testing`).
    #[cfg(any(test, feature = "validation-plan-testing"))]
    pub(crate) fn plan_cache_stats(&self) -> (usize, usize) {
        self.decl_cache.read(|facts| {
            facts
                .iter()
                .filter_map(|f| f.plan.as_ref())
                .flatten()
                .fold((0, 0), |(n, p), plan| (n + 1, p + plan.props.len()))
        })
    }

    /// P5-97: the number of cached inheritance chains, instance facts and
    /// validation plans (a declaration recorded as having none included): a
    /// test-only measure.
    #[cfg(test)]
    pub(crate) fn cache_counts(&self) -> (usize, usize, usize) {
        self.decl_cache.read(|facts| {
            (
                facts.iter().filter(|f| f.class.is_some()).count(),
                facts.iter().filter(|f| f.field_defaults.is_some()).count(),
                facts.iter().filter(|f| f.plan.is_some()).count(),
            )
        })
    }

    /// For the validation plan: a class-like declaration's chain and its
    /// properties (own, then inherited), from the inheritance cache.
    pub(crate) fn class_chain_and_properties(
        &self,
        id: DeclId,
    ) -> Result<(Vec<DeclId>, Vec<PropId>)> {
        let info = self.class_info_of(id)?;
        Ok((info.chain.to_vec(), info.properties.to_vec()))
    }

    /// For the validation plan: the identifier field a class-like
    /// declaration itself declares, if any.
    pub(crate) fn own_identifier_field_name_of(&self, id: DeclId) -> Option<&str> {
        self.declaration(id)
            .and_then(ClassLike::from_declaration)
            .and_then(|class| class.own_identifier_field_name())
    }

    /// For the validation plan: the declaration that declares a property.
    pub(crate) fn property_owner_of(&self, id: PropId) -> Option<DeclId> {
        self.properties.get(id.slot()).map(|slot| slot.declaration)
    }

    /// For the validation plan: [`Self::property_with_owner`].
    pub(crate) fn property_with_owner_of(&self, id: PropId) -> Option<(&str, &Property)> {
        self.property_with_owner(id)
    }

    /// Works out the full name of a class's direct super type, resolved in the
    /// namespace where the class is declared.
    ///
    /// TS: `this.superType = this.ast.superType.name` (`ClassDeclaration.process`)
    /// keeps only the AST `TypeIdentifier`'s `name`, discarding `namespace`
    /// and `resolvedName` — an aliased import's `TypeIdentifier` carries the
    /// *target* declaration's namespace in `namespace` (with the alias, not
    /// the target's own name, in `name`), which `resolveImport`'s alias
    /// lookup needs the whole import list to untangle correctly
    /// (`this.getModelFile().isImportedType(this.superType)` /
    /// `resolveImport`, `_resolveSuperType`); qualifying `name` directly with
    /// `namespace` would build the alias's name in the target namespace,
    /// which does not exist there. So this always resolves through
    /// [`ModelManager::resolve_type_name`] (`ModelFile.getType`'s own path,
    /// PORTING.md 6.2), over `ti.name` alone, the same as the implicit
    /// `Concept`/`Asset`/… super type already does.
    fn super_type_fqn(&self, class: &ClassLike<'_>, in_namespace: &str) -> Result<Option<String>> {
        let Some(ti) = class.super_type() else {
            return Ok(None);
        };
        // TS: ClassDeclaration._resolveSuperType passes `this.ast.location`
        // to every error it raises (src/introspect/classdeclaration.ts); the
        // class whose super type is being resolved is the AST node in scope
        // here, so its `location` is passed on, re-serialised from the typed
        // `mm::Range` by `location_value` (PORTING.md 2.1).
        // P5-48: the location is re-serialised only on an error path, not
        // on every (successful) step of a super-type walk.
        let location = || class.location().and_then(crate::error::location_value);
        match self.resolve_type_name_lazy(in_namespace, &ti.name, location) {
            Ok(fqn) => Ok(Some(fqn)),
            // TS: `_resolveSuperType`'s own hardcoded `IllegalModelException`
            // (src/introspect/classdeclaration.ts) — `resolve_type_name`'s
            // own failure is `TypeNotFound` (`ModelManager.getType`'s shape,
            // a different TS throw site), so it is remapped here, the same
            // way `validation.rs`'s `check_super_type` already raises this
            // exact message (`failed`) for the same TS call.
            Err(err) if err.is_unported_type_not_found() => Err(ContractError::pre_port(
                ErrorKind::IllegalModel,
                format!("Could not find super type {}", ti.name),
                location(),
            )
            .into()),
            Err(other) => Err(other),
        }
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
        self.decl_fqn(id).map(str::to_string)
    }

    js_compat_pub! {
        /// A declaration's fully-qualified name, borrowed from the arena,
        /// where [`ModelManager::insert`] built it once (P5-13).
        pub fn decl_fqn(&self, id: DeclId) -> Result<&str> {
            self.declarations
                .get(id.slot())
                .map(|slot| &*slot.fqn)
                .ok_or_else(|| unknown(Node::Declaration(id)))
        }
    }

    /// The AST node an element was built from, as TS keeps it in `ast`;
    /// `None` for a primitive type name, whose `ast` is `undefined`.
    fn node_ast(&self, node: Node) -> Result<Option<&Value>> {
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

    /// TS `BaseModelManager.resolveType(context, type)` (basemodelmanager.ts,
    /// `private`): a primitive type name passes through unchanged; otherwise
    /// `type` must name a registered namespace, and within it a type local
    /// to that namespace's own file (an imported name is rejected, even when
    /// it resolves). `context` is free text for the error message only (TS
    /// passes call-site descriptions such as a property's fully qualified
    /// name).
    pub fn resolve_type(&self, context: &str, type_name: &str) -> Result<String> {
        if is_primitive_type(type_name) {
            return Ok(type_name.to_string());
        }
        let namespace = get_namespace(Some(type_name))?;
        let Some(model_file) = self.model_file(namespace) else {
            return Err(ContractError::new(
                ErrorKind::IllegalModel,
                "modelmanager-resolvetype-nonsfortype",
                vec![
                    ("type", type_name.to_string()),
                    ("context", context.to_string()),
                ],
            )
            .into());
        };
        if model_file.is_local_type(type_name) {
            return Ok(type_name.to_string());
        }
        Err(ContractError::new(
            ErrorKind::IllegalModel,
            "modelmanager-resolvetype-notypeinnsforcontext",
            vec![
                ("context", context.to_string()),
                ("type", type_name.to_string()),
                ("namespace", model_file.namespace().to_string()),
            ],
        )
        .into())
    }

    /// TS `BaseModelManager.derivesFrom(fqt1, fqt2)` (basemodelmanager.ts):
    /// `fqt1` must resolve (`this.getType(fqt1)`, propagated verbatim —
    /// `ModelManager::get_type_declaration` is the same lookup `getType`
    /// dispatches to); then true when `fqt1` and `fqt2` are the same type, or
    /// `fqt1` transitively extends it. [`ModelManager::is_assignable_to`]
    /// already walks exactly this chain (including the implicit `Concept`
    /// super type, P2-03), so this reuses it once `fqt1`'s own resolution is
    /// confirmed with `getType`'s error surface — `is_assignable_to`'s own
    /// lookup raises a different one (the pre-port `TypeNotFound` of `Error::type_not_found`, not
    /// the catalogued `IllegalModelException` `getType` raises).
    pub fn derives_from(&self, fqt1: &str, fqt2: &str) -> Result<bool> {
        self.get_type_declaration(fqt1)?;
        self.is_assignable_to(fqt1, fqt2)
    }

    js_compat_pub! {
        /// TS `BaseModelManager.isAssignableTo(fqn, baseFqn)`
        /// (basemodelmanager.ts). This is a different method from
        /// [`ModelManager::is_assignable_to`] — TS itself gives `ModelManager`
        /// two unrelated `isAssignableTo`s, `ModelUtil`'s own static
        /// (`crate::model_util::is_assignable_to`) and this one: `fqn` must
        /// resolve to a *concrete* (non-abstract) type before
        /// [`ModelManager::derives_from`] is even asked — an abstract `fqn` is
        /// `false` even against itself — and a lookup failure is caught, not
        /// propagated.
        pub fn is_type_assignable_to(&self, fqn: &str, base_fqn: &str) -> bool {
            let Ok(id) = self.get_type_declaration(fqn) else {
                return false;
            };
            if self
                .declaration(id)
                .and_then(Declaration::as_class)
                .is_some_and(|class| class.is_abstract())
            {
                return false;
            }
            self.derives_from(fqn, base_fqn).unwrap_or(false)
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getAssignableConcreteTypes(baseFqn)`
        /// (basemodelmanager.ts): every concrete (non-abstract) declaration
        /// assignable to `baseFqn`, `baseFqn` itself included when it is
        /// concrete; empty when `baseFqn` is not in the model (TS catches
        /// `getType`'s error and returns `[]`).
        pub fn get_assignable_concrete_types(&self, base_fqn: &str) -> Vec<DeclId> {
            let Ok(names) = self.assignable_type_names(base_fqn) else {
                return Vec::new();
            };
            names
                .into_iter()
                .filter_map(|fqn| self.declaration_id(&fqn))
                .filter(|id| {
                    !self
                        .declaration(*id)
                        .and_then(Declaration::as_class)
                        .is_some_and(|class| class.is_abstract())
                })
                .collect()
        }
    }

    /// TS `getFileNameFromIdentifier` (basemodelmanager.ts, module-private):
    /// the last non-empty `/`- or `\`-delimited segment of `file_identifier`
    /// once its trailing separators are stripped; `file_identifier` itself
    /// when that leaves nothing.
    fn file_name_from_identifier(file_identifier: &str) -> String {
        let trimmed = file_identifier.trim_end_matches(['/', '\\']);
        trimmed
            .rsplit(['/', '\\'])
            .find(|segment| !segment.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| file_identifier.to_string())
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getModels(options)` (basemodelmanager.ts): every
        /// registered model file but the root and decorator models
        /// (`this.getModelFiles()`'s default excludes `EXCLUDE_NS`), as a
        /// `(name, content)` pair — `content` is `None` exactly where TS's
        /// `file.definitions` is `undefined`. `include_external_models` is TS's
        /// `options.includeExternalModels` (`true` by default there; the oracle
        /// harness always passes it explicitly).
        pub fn get_models(&self, include_external_models: bool) -> Vec<(String, Option<String>)> {
            self.model_files()
                .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
                .filter(|mf| include_external_models || !mf.is_external())
                .map(|mf| {
                    let name = match mf.file_name() {
                        None | Some("UNKNOWN") | Some("") => format!("{}.cto", mf.namespace()),
                        Some(identifier) => Self::file_name_from_identifier(identifier),
                    };
                    (name, mf.definitions().map(str::to_string))
                })
                .collect()
        }
    }

    /// TS `BaseModelManager.getAst(resolve, includeConcertoNamespaces)`
    /// (basemodelmanager.ts): every registered model file's own AST
    /// ([`ModelFile::ast`]), in [`ModelManager::model_files`] order, wrapped
    /// in the metamodel's `Models` envelope; a system namespace
    /// (`EXCLUDE_NS`) is left out unless `include_concerto_namespaces`.
    /// `resolve` runs each model through [`ModelManager::resolve_meta_model`]
    /// first — the only way this can fail, the same as TS's uncaught throw
    /// from `resolveMetaModel`.
    #[deprecated(since = "0.1.0", note = "use `ast`")]
    pub fn get_ast(&self, resolve: bool, include_concerto_namespaces: bool) -> Result<Value> {
        self.models_ast(resolve, include_concerto_namespaces)
    }

    /// TS `BaseModelManager.getAst(resolve, includeConcertoNamespaces)`
    /// (basemodelmanager.ts): every registered model file's own AST
    /// ([`ModelFile::ast`]), in [`ModelManager::model_files`] order, wrapped
    /// in the metamodel's `Models` envelope; a system namespace
    /// (`EXCLUDE_NS`) is left out unless `include_concerto_namespaces`.
    /// `resolve` runs each model through [`ModelManager::resolve_meta_model`]
    /// first — the only way this can fail, the same as TS's uncaught throw
    /// from `resolveMetaModel`.
    pub(crate) fn models_ast(
        &self,
        resolve: bool,
        include_concerto_namespaces: bool,
    ) -> Result<Value> {
        // TS re-reads `getAst(false, true)` inside every `resolveMetaModel`
        // call, but nothing registers or removes a model file in between, so
        // one borrowed snapshot of the registered models serves every file
        // (P5-17 F1: the per-file snapshot was an O(N^2) deep clone).
        let prior_models = if resolve {
            Some(self.prior_models())
        } else {
            None
        };
        let mut models = Vec::new();
        for mf in self.model_files() {
            if !include_concerto_namespaces && EXCLUDE_NS.contains(&mf.namespace()) {
                continue;
            }
            models.push(match &prior_models {
                Some(prior_models) => metamodel_util::resolve_local_names(prior_models, mf.ast())?,
                None => mf.ast().clone(),
            });
        }
        Ok(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": models,
        }))
    }

    /// TS `BaseModelManager.resolveMetaModel(metaModel)` (basemodelmanager.ts):
    /// `meta_model` (typically one of this manager's own model files' AST,
    /// but any well-formed metamodel `Model` node) with every type name it
    /// holds resolved to its declaring namespace, against this manager's own
    /// currently-registered models (`this.getAst(false, true)`) — a port of
    /// `@accordproject/concerto-metamodel`'s `MetaModelUtil.resolveLocalNames`
    /// (`metamodel_util`, below this `impl` block), the only consumer this
    /// manager has for it.
    pub fn resolve_meta_model(&self, meta_model: &Value) -> Result<Value> {
        metamodel_util::resolve_local_names(&self.prior_models(), meta_model)
    }

    /// The models `resolve_meta_model` resolves against — TS's
    /// `this.getAst(false, true).models`, every registered model file's own
    /// AST (system namespaces included) in [`ModelManager::model_files`]
    /// order — borrowed and indexed by each AST's own `namespace`, instead
    /// of deep-cloned into a `Models` envelope. The first model with a given
    /// namespace wins, the same as TS `findNamespace`'s `Array.find`.
    fn prior_models(&self) -> metamodel_util::PriorModels<'_> {
        let mut prior_models = metamodel_util::PriorModels::new();
        for mf in self.model_files() {
            let ast = mf.ast();
            if let Some(namespace) = ast.get("namespace").and_then(Value::as_str) {
                prior_models.entry(namespace).or_insert(ast);
            }
        }
        prior_models
    }

    /// TS `BaseModelManager.get<Kind>Declarations()` (basemodelmanager.ts,
    /// six near-identical methods, each `this.getModelFiles().reduce((prev,
    /// cur) => prev.concat(cur.get<Kind>Declarations()), [])`): every
    /// non-system, non-decorator model file's own declarations whose
    /// constructor name is `ctor` — matched exactly on `$class`, never by
    /// inheritance, the same way every other `{ctor, fqn}` summary in this
    /// port already does (P2-08 review) — concatenated in registration
    /// order.
    fn declarations_by_ctor(&self, ctor: &str) -> Vec<DeclId> {
        self.model_files()
            .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
            .flat_map(|mf| {
                let file = self.model_file_id(mf.namespace());
                file.into_iter().flat_map(|f| self.declaration_ids(f))
            })
            .filter(|id| {
                self.declaration(*id).is_some_and(|d| match d {
                    Declaration::Class(class) => class.declaration_kind() == ctor,
                    Declaration::Enum(_) => ctor == "EnumDeclaration",
                    Declaration::Scalar(_) => ctor == "ScalarDeclaration",
                    Declaration::Map(_) => ctor == "MapDeclaration",
                })
            })
            .collect()
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getAssetDeclarations()`.
        pub fn get_asset_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("AssetDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getTransactionDeclarations()`.
        pub fn get_transaction_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("TransactionDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getEventDeclarations()`.
        pub fn get_event_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("EventDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getParticipantDeclarations()`.
        pub fn get_participant_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("ParticipantDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getConceptDeclarations()`.
        pub fn get_concept_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("ConceptDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getEnumDeclarations()`.
        pub fn get_enum_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("EnumDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.filter(predicate, options)` (basemodelmanager.ts):
        /// a scratch manager holding every registered model file's declarations
        /// for which `keep_fqn` is true, filtering each file's own imports the
        /// same way ([`crate::introspect::model_file::ModelFile::filter`]'s
        /// module doc); a file with nothing left is dropped. `predicate` is a
        /// `Declaration -> bool` in TS, keyed here by fully-qualified name
        /// instead, since that is all the oracle's own `predicate` encoding
        /// carries (`tests/oracle/ops.rs`). The root model is skipped exactly as
        /// TS's `modelFile.isSystemModelFile()` check does — the decorator model
        /// is *not* skipped, because TS does not skip it either, so it is
        /// filtered like any other file (which in practice always empties it,
        /// since `keep_fqn` never names one of its own declarations in the
        /// corpus). The result always starts from a fresh `BaseModelManager`
        /// (TS: `new BaseModelManager({...this.options}, this.processFile)`),
        /// never the receiver's own kind. `disable_validation` is TS's
        /// `options?.disableValidation`; unless set, the filtered files are
        /// validated once, together (TS: `modelManager.addModelFiles(...)`).
        pub fn filter_by_fqn(
            &self,
            keep_fqn: impl Fn(&str) -> bool,
            disable_validation: bool,
        ) -> Result<Self> {
            self.filter_declarations(|fqn, _| keep_fqn(fqn), disable_validation)
        }
    }

    /// A new manager with only the declarations `keep` accepts, given each
    /// declaration's fully-qualified name and the declaration itself. A model
    /// file left with no declaration is dropped, and each file's imports are
    /// filtered the same way. The system root model is kept whole. The new
    /// manager has this one's options, and its files are validated together.
    ///
    /// TS: `BaseModelManager.filter(predicate)`.
    pub fn filter(&self, keep: impl Fn(&str, &Declaration) -> bool) -> Result<Self> {
        self.filter_declarations(keep, false)
    }

    /// [`ModelManager::filter_by_fqn`] over a predicate on the declaration too.
    fn filter_declarations(
        &self,
        keep: impl Fn(&str, &Declaration) -> bool,
        disable_validation: bool,
    ) -> Result<Self> {
        // A-5: every option, `metamodel_validation` included, as TS's
        // `new BaseModelManager({...this.options})` does.
        let mut result = Self::new()?;
        result.options = self.options.clone();

        // `ModelFile::filter`'s predicate carries no namespace of its own
        // (its doc): it is called both on the file being filtered *and*,
        // for that file's own imports, on a *different* file's declarations
        // (`source_manager.model_file(ns).get_local_type(...)`). A `decl ->
        // bool` predicate built from one file's namespace alone would ask
        // `keep_fqn` about the wrong fully-qualified name for every
        // cross-file (import) check, silently dropping every import that
        // should stay. So the fully-qualified check runs once, up front,
        // over every file's own real namespace, and this instead keeps by
        // *identity*: every declaration the predicate is ever handed here is
        // a reference into `self`'s own arena (`self` is `source_manager`
        // below), so a declaration kept by an earlier file is still
        // recognised when it is reached again through another file's
        // imports.
        let keep = &keep;
        let kept: std::collections::HashSet<*const Declaration> = self
            .model_files()
            .flat_map(|mf| {
                let namespace = mf.namespace();
                mf.declarations().iter().filter_map(move |decl| {
                    keep(&qualify(namespace, decl.name()), decl)
                        .then_some(decl as *const Declaration)
                })
            })
            .collect();

        let mut filtered_files = Vec::new();
        for model_file in self.shared_model_files() {
            if model_file.is_system_namespace() {
                continue;
            }
            // P5-97: a file the filter keeps exactly as it is is shared, not
            // rebuilt, and is not validated again when this manager had
            // validated it and every file it reaches is shared too
            // (`ValidityProof`).
            match model_file
                .filter_outcome(|decl| kept.contains(&(decl as *const Declaration)), self)?
            {
                crate::introspect::model_file::FilterOutcome::Empty => {}
                crate::introspect::model_file::FilterOutcome::Unchanged => {
                    filtered_files.push((
                        Arc::clone(model_file),
                        self.validity_proof(model_file.namespace()),
                    ));
                }
                crate::introspect::model_file::FilterOutcome::Filtered(f) => {
                    filtered_files.push((Arc::new(*f), None));
                }
            }
        }
        result.insert_models(filtered_files, !disable_validation)?;
        Ok(result)
    }

    js_compat_pub! {
        /// TS `updateModelFile(modelFile, fileName, disableValidation)`'s
        /// registration step, for an already-parsed `model_file` (the oracle
        /// harness's own CTO -> AST step handles the string overload; CTO
        /// parsing stays out of Rust's scope, plan §1.1). TS requires the
        /// namespace to already be registered, a plain `Error`
        /// (`basemodelmanager-updatemodelfile-notfound`); everything past that
        /// matches `ModelManager::with_model_file_registered`, which already
        /// builds exactly the scratch copy TS's own registration
        /// (`this.modelFiles[ns] = modelFile`) produces. Never mutates `self`:
        /// on any error the caller simply does not adopt the result, the same
        /// way TS's own catch leaves `this` unchanged.
        pub fn update_model_file(&self, model_file: ModelFile, validate: bool) -> Result<Self> {
            let namespace = model_file.namespace().to_string();
            if self.model_file(&namespace).is_none() {
                return Err(ContractError::new(
                    ErrorKind::InvalidArgument,
                    "basemodelmanager-updatemodelfile-notfound",
                    vec![("namespace", namespace)],
                )
                .into());
            }
            let updated = self.with_model_file_registered(Arc::new(model_file))?;
            if validate {
                let mf = updated
                    .model_file(&namespace)
                    .expect("with_model_file_registered registers the file under its namespace");
                updated.validate_model_file(mf)?;
            }
            Ok(updated)
        }
    }

    js_compat_pub! {
        /// TS `deleteModelFile(namespace)` (basemodelmanager.ts): the manager
        /// with every registered model file but `namespace`'s, or a plain
        /// `Error` (`basemodelmanager-deletemodelfile-notfound`) when it holds
        /// none. `deleteModelFile` has no arena-level tombstone yet (module doc,
        /// "any future removal ... is a different shape"), so this rebuilds a
        /// fresh manager from the survivors, the same way
        /// `ModelManager::with_model_file_registered`'s own scratch copy does.
        pub fn delete_model_file(&self, namespace: &str) -> Result<Self> {
            if self.model_file(namespace).is_none() {
                return Err(ContractError::new(
                    ErrorKind::InvalidArgument,
                    "basemodelmanager-deletemodelfile-notfound",
                    Vec::new(),
                )
                .into());
            }
            let mut scratch = Self {
                options: self.options.clone(),
                ..Self::default()
            };
            // A-4: the survivors are shared, not deep-cloned.
            for existing in self.shared_model_files() {
                if existing.namespace() != namespace {
                    scratch.insert_shared(Arc::clone(existing))?;
                }
            }
            Ok(scratch)
        }
    }

    js_compat_pub! {
        /// The Rust half of TS `BaseModelManager.updateExternalModels(options,
        /// fileDownloader)` (basemodelmanager.ts; ledger: HYBRID, the download
        /// stays in JS). `external_models` is what
        /// `downloader.downloadExternalDependencies(...)` resolved to, in order:
        /// each is built as `new ModelFile(this, ast, definitions, fileName)`,
        /// then registered without validation — `updateModelFile(mf, name,
        /// true)` when its namespace is already registered (by `self` or an
        /// earlier download in the same batch), `addModelFile(mf, null, name,
        /// true)` otherwise — and finally every registered model file is
        /// validated (`validateModelFiles`). The model files are returned in the
        /// same order, as TS's `externalModelFiles`.
        ///
        /// Any error leaves `self` exactly as it was, as TS's `catch` restores
        /// `this.modelFiles` before rethrowing.
        pub fn update_external_models(
            &mut self,
            external_models: impl IntoIterator<Item = ModelFileSource>,
        ) -> Result<Vec<Arc<ModelFile>>> {
            self.update_external_models_naming_file(external_models)
                .map_err(|(_, err)| err)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::update_external_models`], with the namespace of
        /// the model file whose validation failed, when that is the failure
        /// (P5-11, accordproject/concerto-rust#287): TS's final
        /// `validateModelFiles()` throws that file's own `validate()` error,
        /// which names the file.
        pub fn update_external_models_naming_file(
            &mut self,
            external_models: impl IntoIterator<Item = ModelFileSource>,
        ) -> std::result::Result<Vec<Arc<ModelFile>>, (Option<String>, Error)> {
            // A-4: each downloaded file is built once and shared (`Arc`)
            // between the scratch manager and the list returned, and the
            // scratch is rebuilt only for a file that replaces one; a new
            // namespace is appended to the scratch this call already owns,
            // which is the same arena `with_model_file_registered` would
            // build, without copying it once per file.
            let mut updated: Option<Self> = None;
            let mut registered = Vec::new();
            for source in external_models {
                let mf = Arc::new(
                    ModelFile::from_json_with_definitions(
                        &source.ast,
                        source.definitions,
                        source.file_name,
                    )
                    .map_err(|err| (None, err))?,
                );
                let replaces = updated
                    .as_ref()
                    .unwrap_or(self)
                    .model_file(mf.namespace())
                    .is_some();
                match updated.as_mut() {
                    Some(scratch)
                        if !replaces && scratch.namespaces.len() == scratch.files.len() =>
                    {
                        // `addModelFile`'s already-exists check cannot fire here.
                        scratch
                            .insert_shared(Arc::clone(&mf))
                            .map_err(|err| (None, err))?;
                    }
                    _ => {
                        // `addModelFile`'s already-exists check cannot fire
                        // here; `updateModelFile` without validation is the
                        // same scratch registration.
                        let next = updated
                            .as_ref()
                            .unwrap_or(self)
                            .with_model_file_registered(Arc::clone(&mf))
                            .map_err(|err| (None, err))?;
                        updated = Some(next);
                    }
                }
                registered.push(mf);
            }
            updated
                .as_ref()
                .unwrap_or(self)
                .validate_models_naming_file()
                .map_err(|(namespace, err)| (Some(namespace), err))?;
            if let Some(updated) = updated {
                self.adopt(updated);
            }
            Ok(registered)
        }
    }

    /// The rollback core of [`ModelManager::add_models`] and
    /// [`ModelManager::filter`]: inserts every already-built `files` in
    /// order, then, unless `validate` is false, runs
    /// [`ModelManager::validate_models`] once over the whole manager; either
    /// failure undoes every insert this call made, exactly as `add_models`
    /// does for its own AST-building version of the same loop.
    fn insert_models(
        &mut self,
        files: Vec<(Arc<ModelFile>, Option<Arc<ValidityProof>>)>,
        validate: bool,
    ) -> Result<()> {
        let files_len = self.files.len();
        let declarations_len = self.declarations.len();
        let properties_len = self.properties.len();
        let namespaces_snapshot = self.namespaces.clone();
        let generation = self.generation;
        let validated = self.validated_marks();

        let mut result: Result<()> = Ok(());
        for (mf, proof) in files {
            if let Err(err) = self.add_shared_model_file_with_proof(mf, proof) {
                result = Err(err);
                break;
            }
        }
        if result.is_ok()
            && validate
            && let Err(err) = self.validate_models()
        {
            result = Err(err);
        }

        if let Err(err) = result {
            self.files.truncate(files_len);
            self.declarations.truncate(declarations_len);
            self.properties.truncate(properties_len);
            self.invalidate_caches();
            self.namespaces = namespaces_snapshot;
            self.generation = generation;
            self.restore_validated(&validated);
            return Err(err);
        }
        Ok(())
    }
}

/// The manager answers collaborator calls from its own graph. A node of a
/// kind whose TS object has no such method answers V8's "is not a function"
/// `TypeError`, as the JS-callback context does for the same call; that is
/// what TS raises for a primitive type name that `ModelFile.getType`
/// returned. A handle this manager never handed out is an error.
///
/// The answers come from the loader's model state. Where that state is not
/// yet at parity with TS, so are the answers: super types resolve as the
/// loader resolves them (P2-08). The implicit `Concept` super type (P2-03) is
/// in every class-like or enum declaration's `super_chain` (`ClassLike`), so
/// it is in [`ResolutionContext::get_all_super_type_declarations`] too, for
/// both [`Declaration::Class`] and [`Declaration::Enum`] — TS's
/// `EnumDeclaration extends ClassDeclaration` gives an enum the same implicit
/// `Concept` super type (P2-03).
impl ResolutionContext for ModelManager {
    type Node = Node;
    type Error = Error;

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
            Declaration::Class(_) | Declaration::Enum(_) => self
                .super_chain(&self.declaration_fqn(id)?)?
                .into_iter()
                // The chain starts with the type itself.
                .skip(1)
                .map(|(fqn, _)| {
                    self.declaration_id(&fqn)
                        .map(Node::Declaration)
                        .ok_or(Error::type_not_found(fqn))
                })
                .collect(),
            Declaration::Scalar(_) | Declaration::Map(_) => Err(not_a_function()),
        }
    }

    fn get_fully_qualified_name(&self, declaration: &Node) -> Result<String> {
        match *declaration {
            Node::Declaration(id) => self.declaration_fqn(id),
            // TS: Property.getFullyQualifiedName (src/introspect/property.ts)
            Node::Property(id) => {
                let (Some(parent), Some(property)) = (self.parent_of(id), self.property_by_id(id))
                else {
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
            self.property_by_id(id),
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
        // TS: Property.getFullyQualifiedTypeName (src/introspect/property.ts:218)
        // throws a plain `Error` (`ErrorKind::InvalidArgument`, not
        // `IllegalModelException`) with its own inline template
        // (`property-getfullyqualifiedtypename-notfound`) when
        // `ModelFile.getFullyQualifiedTypeName` returns `null` — which it
        // does, rather than throwing, so this is the one throw site for
        // both. `this.type` is JS `null` for an enum value (P2-04) and
        // renders as the literal string `null`, matching `+ this.type`'s
        // own string coercion.
        resolved.ok_or_else(|| {
            let field = self.property_by_id(id).expect("checked above");
            ContractError::new(
                ErrorKind::InvalidArgument,
                "property-getfullyqualifiedtypename-notfound",
                vec![
                    ("name", field.name().to_string()),
                    ("type", type_name.unwrap_or("null").to_string()),
                ],
            )
            .into()
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
        let field = self.property_by_id(id).ok_or_else(|| unknown(*property))?;
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
        let Some(ast) = self.node_ast(*declaration)? else {
            return Err(ContractError::new(
                ErrorKind::MalformedInput,
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

/// A port of `@accordproject/concerto-metamodel@3.17.0`'s
/// `lib/metamodelutil.js` `resolveLocalNames` and its private helpers — the
/// only part of that package [`ModelManager::resolve_meta_model`] needs.
/// Every function here is a line-for-line port: it walks the same plain
/// metamodel-AST [`Value`] shape [`ModelFile::ast`] already stores (no typed
/// `mm::*` struct, the same divergence `crate::dcs` documents for the same
/// AST), and raises the same plain JS `Error`/`TypeError` TS does, through
/// the catalogue (`metamodelutil-*`, `engine-typeerror-readproperties`).
mod metamodel_util {
    use std::collections::HashMap;

    use serde_json::Value;

    use crate::error::{ContractError, Error, ErrorKind, Result};

    /// The metamodel's own namespace, short for the five reserved
    /// declarations `createNameTable` seeds the table with.
    const CONCERTO_NS: &str = "concerto@1.0.0";

    /// The metamodel's namespace prefix, stripped from a node's `$class` to
    /// get the short name the `switch` in `resolveTypeNames` matches on.
    const MM_NS: &str = "concerto.metamodel@1.0.0.";

    /// One `createNameTable` entry: the namespace and (possibly aliased)
    /// local name a bare name resolves to.
    struct ResolvedName {
        namespace: String,
        name: String,
        resolved_name: Option<String>,
    }

    /// The registered models (`getAst(false, true).models`), borrowed and
    /// keyed by namespace (first model wins, as TS `findNamespace`'s
    /// `Array.find` does); see `ModelManager::prior_models`.
    pub(super) type PriorModels<'a> = HashMap<&'a str, &'a Value>;

    /// TS `findNamespace`: the model in `prior_models` whose namespace is
    /// `namespace`, if one is registered.
    fn find_namespace<'a>(prior_models: &PriorModels<'a>, namespace: &str) -> Option<&'a Value> {
        prior_models.get(namespace).copied()
    }

    /// TS `findDeclaration`: `model`'s own declaration named `name`, if any.
    fn find_declaration<'a>(model: &'a Value, name: &str) -> Option<&'a Value> {
        model
            .get("declarations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|decl| decl.get("name").and_then(Value::as_str) == Some(name))
    }

    /// TS: `Declaration ${imp.name} in namespace ${namespace} not found`.
    fn declaration_not_found(name: &str, namespace: &str) -> Error {
        ContractError::new(
            ErrorKind::InvalidArgument,
            "metamodelutil-createnametable-declarationnotfound",
            vec![
                ("name", name.to_string()),
                ("namespace", namespace.to_string()),
            ],
        )
        .into()
    }

    /// TS: `Name ${name} not found`.
    fn name_not_found(name: &str) -> Error {
        ContractError::new(
            ErrorKind::InvalidArgument,
            "metamodelutil-resolvename-notfound",
            vec![("name", name.to_string())],
        )
        .into()
    }

    /// TS: `Unrecognized $class ${String(metaModel.$class)}`.
    fn unrecognized_class(rendered: String) -> Error {
        ContractError::new(
            ErrorKind::InvalidArgument,
            "metamodelutil-resolvetypenames-unrecognizedclass",
            vec![("class", rendered)],
        )
        .into()
    }

    /// A JS `TypeError` for reading `.declarations` of `undefined`: TS's
    /// `findNamespace` returns `undefined` for an import whose namespace is
    /// not (yet) registered, and every `createNameTable` branch reads
    /// straight off that result without an existence check.
    fn undefined_declarations() -> Error {
        ContractError::new(
            ErrorKind::MalformedInput,
            "engine-typeerror-readproperties",
            vec![
                ("value", "undefined".to_string()),
                ("property", "declarations".to_string()),
            ],
        )
        .into()
    }

    /// TS `createNameTable`: a bare-name -> (namespace, name[, resolvedName])
    /// table for `meta_model`, seeded with the five reserved
    /// `concerto@1.0.0` declarations, then every name `meta_model` imports —
    /// in import order, a later import overriding an earlier one — and
    /// finally every name `meta_model` declares itself (overriding its own
    /// imports), the same override order as TS's two `forEach` loops.
    fn create_name_table(
        prior_models: &PriorModels<'_>,
        meta_model: &Value,
    ) -> Result<HashMap<String, ResolvedName>> {
        let mut table: HashMap<String, ResolvedName> =
            ["Concept", "Asset", "Participant", "Transaction", "Event"]
                .into_iter()
                .map(|name| {
                    (
                        name.to_string(),
                        ResolvedName {
                            namespace: CONCERTO_NS.to_string(),
                            name: name.to_string(),
                            resolved_name: None,
                        },
                    )
                })
                .collect();

        for imp in meta_model
            .get("imports")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let namespace = imp
                .get("namespace")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let model_file = find_namespace(prior_models, namespace);
            let class = imp
                .get("$class")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match class.strip_prefix(MM_NS) {
                Some("ImportType") => {
                    let model_file = model_file.ok_or_else(undefined_declarations)?;
                    let name = imp.get("name").and_then(Value::as_str).unwrap_or_default();
                    if find_declaration(model_file, name).is_none() {
                        return Err(declaration_not_found(name, namespace));
                    }
                    table.insert(
                        name.to_string(),
                        ResolvedName {
                            namespace: namespace.to_string(),
                            name: name.to_string(),
                            resolved_name: None,
                        },
                    );
                }
                Some("ImportTypes") => {
                    // TS only reads `modelFile.declarations` inside
                    // `imp.types.forEach`, so an import of no types from an
                    // unregistered namespace does not throw.
                    let aliases: HashMap<&str, &str> = imp
                        .get("aliasedTypes")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|a| {
                            Some((
                                a.get("name").and_then(Value::as_str)?,
                                a.get("aliasedName").and_then(Value::as_str)?,
                            ))
                        })
                        .collect();
                    for ty in imp
                        .get("types")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let Some(ty) = ty.as_str() else { continue };
                        let model_file = model_file.ok_or_else(undefined_declarations)?;
                        if find_declaration(model_file, ty).is_none() {
                            return Err(declaration_not_found(ty, namespace));
                        }
                        let local_name = aliases.get(ty).copied().unwrap_or(ty);
                        let entry = if local_name != ty {
                            ResolvedName {
                                namespace: namespace.to_string(),
                                name: local_name.to_string(),
                                resolved_name: Some(ty.to_string()),
                            }
                        } else {
                            ResolvedName {
                                namespace: namespace.to_string(),
                                name: ty.to_string(),
                                resolved_name: None,
                            }
                        };
                        table.insert(local_name.to_string(), entry);
                    }
                }
                _ => {
                    // TS's `else` branch: `ImportAll` (and anything else),
                    // every one of the target model's own declarations.
                    let model_file = model_file.ok_or_else(undefined_declarations)?;
                    for decl in model_file
                        .get("declarations")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(name) = decl.get("name").and_then(Value::as_str) {
                            table.insert(
                                name.to_string(),
                                ResolvedName {
                                    namespace: namespace.to_string(),
                                    name: name.to_string(),
                                    resolved_name: None,
                                },
                            );
                        }
                    }
                }
            }
        }

        let own_namespace = meta_model
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for decl in meta_model
            .get("declarations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = decl.get("name").and_then(Value::as_str) {
                table.insert(
                    name.to_string(),
                    ResolvedName {
                        namespace: own_namespace.to_string(),
                        name: name.to_string(),
                        resolved_name: None,
                    },
                );
            }
        }

        Ok(table)
    }

    /// Sets a `TypeIdentifier`-shaped `node`'s `namespace` (and `name`, and
    /// `resolvedName` when the table entry carries one) from `table[name]`,
    /// the shared tail of TS's `superType` case and its `.type` group
    /// (`metaModel.superType.namespace = resolveName(name, table);
    /// metaModel.superType.name = table[name].name; if (table[name]?.resolvedName)
    /// …`): `table[name].name` always equals `name` itself (every
    /// `createNameTable` branch keys an entry under its own `name`), so
    /// re-reading the table after the (no-op) name reassignment, as TS does,
    /// is the same as reading it once.
    fn set_resolved_type_identifier(
        node: &mut Value,
        name: &str,
        table: &HashMap<String, ResolvedName>,
    ) -> Result<()> {
        let entry = table.get(name).ok_or_else(|| name_not_found(name))?;
        let Some(map) = node.as_object_mut() else {
            return Ok(());
        };
        map.insert("namespace".into(), Value::String(entry.namespace.clone()));
        map.insert("name".into(), Value::String(entry.name.clone()));
        if let Some(resolved) = &entry.resolved_name {
            map.insert("resolvedName".into(), Value::String(resolved.clone()));
        }
        Ok(())
    }

    /// TS `resolveTypeNames`: mutates `node` (and everything it holds) in
    /// place, adding the fully-qualified `namespace` (and `resolvedName`,
    /// where the name table has one) next to every type name `node` or one
    /// of its descendants carries — a super type, an object/relationship
    /// property's or map key/value's `type`, a decorator type reference
    /// argument, and a scalar declaration's own name.
    fn resolve_type_names(node: &mut Value, table: &HashMap<String, ResolvedName>) -> Result<()> {
        // Any element can carry a decorator (including a primitive field),
        // so resolve those first, exactly as TS does before its `switch`.
        if let Some(decorators) = node.get_mut("decorators").and_then(Value::as_array_mut) {
            for decorator in decorators.iter_mut() {
                resolve_type_names(decorator, table)?;
            }
        }

        let class_value = node.get("$class");
        let class_str = class_value
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        // TS: `if (!metaModel.$class) throw ...` — only a missing, `null`,
        // non-string or empty `$class` is falsy; anything else truthy that
        // matches no `case` below falls through `default` as a no-op.
        let Some(class_str) = class_str else {
            let rendered = match class_value {
                None => "undefined".to_string(),
                Some(Value::Null) => "null".to_string(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
            };
            return Err(unrecognized_class(rendered));
        };
        let Some(short) = class_str.strip_prefix(MM_NS) else {
            return Ok(());
        };

        match short {
            "Model" => {
                if let Some(decls) = node.get_mut("declarations").and_then(Value::as_array_mut) {
                    for decl in decls.iter_mut() {
                        resolve_type_names(decl, table)?;
                    }
                }
            }
            "EnumDeclaration"
            | "AssetDeclaration"
            | "ConceptDeclaration"
            | "EventDeclaration"
            | "TransactionDeclaration"
            | "ParticipantDeclaration" => {
                if let Some(super_type) = node.get_mut("superType") {
                    let name = super_type
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(name) = name {
                        set_resolved_type_identifier(super_type, &name, table)?;
                    }
                }
                if let Some(props) = node.get_mut("properties").and_then(Value::as_array_mut) {
                    for property in props.iter_mut() {
                        resolve_type_names(property, table)?;
                    }
                }
            }
            "MapDeclaration" => {
                if let Some(key) = node.get_mut("key") {
                    resolve_type_names(key, table)?;
                }
                if let Some(value) = node.get_mut("value") {
                    resolve_type_names(value, table)?;
                }
            }
            "Decorator" => {
                if let Some(args) = node.get_mut("arguments").and_then(Value::as_array_mut) {
                    for argument in args.iter_mut() {
                        resolve_type_names(argument, table)?;
                    }
                }
            }
            "ObjectProperty"
            | "RelationshipProperty"
            | "DecoratorTypeReference"
            | "ObjectMapKeyType"
            | "ObjectMapValueType"
            | "RelationshipMapValueType" => {
                if let Some(type_node) = node.get_mut("type") {
                    let name = type_node
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if let Some(name) = name {
                        set_resolved_type_identifier(type_node, &name, table)?;
                    }
                }
            }
            "StringScalar" | "BooleanScalar" | "DateTimeScalar" | "DoubleScalar" | "LongScalar"
            | "IntegerScalar" => {
                let name = node.get("name").and_then(Value::as_str).map(str::to_string);
                if let Some(name) = name {
                    let namespace = table
                        .get(&name)
                        .map(|entry| entry.namespace.clone())
                        .ok_or_else(|| name_not_found(&name))?;
                    if let Some(map) = node.as_object_mut() {
                        map.insert("namespace".into(), Value::String(namespace));
                        map.insert("name".into(), Value::String(name));
                    }
                }
            }
            // Every other `$class` (primitive properties and map key/value
            // types, decorator literals, …) needs no name resolution: TS's
            // `default` case is a no-op once `metaModel.$class` is truthy,
            // which every well-formed node here already established.
            _ => {}
        }
        Ok(())
    }

    /// TS `resolveLocalNames`: `meta_model` with every type name it holds
    /// resolved to its declaring namespace, against `prior_models`
    /// (`ModelManager.getAst(false, true)`'s models, see [`PriorModels`]).
    pub(super) fn resolve_local_names(
        prior_models: &PriorModels<'_>,
        meta_model: &Value,
    ) -> Result<Value> {
        let table = create_name_table(prior_models, meta_model)?;
        let mut result = meta_model.clone();
        resolve_type_names(&mut result, &table)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `org.example@1.0.0` with Person ← Employee ← Manager and an enum.
    fn manager() -> ModelManager {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
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

    /// P5-77 (accordproject/concerto-rust#419): a file registered shared
    /// in a second manager is the same file, with the same duplicate
    /// namespace error as `add_model_file`; `compact_model_asts` returns
    /// each file's AST text, compacts a file only this manager holds and
    /// leaves a shared one as it is, and every AST reads back equal.
    #[test]
    fn shared_model_files_and_compact_model_asts() {
        let mut source = manager();
        let before: Vec<Value> = source.model_files().map(|mf| mf.ast().clone()).collect();
        let mut other = ModelManager::new().unwrap();
        let shared = source
            .shared_model_files()
            .find(|mf| mf.namespace() == "org.example@1.0.0")
            .cloned()
            .unwrap();
        other.add_shared_model_file(Arc::clone(&shared)).unwrap();
        let held = other
            .shared_model_files()
            .find(|mf| mf.namespace() == "org.example@1.0.0")
            .unwrap();
        assert!(Arc::ptr_eq(held, &shared));
        let dup = other
            .add_shared_model_file(Arc::clone(&shared))
            .unwrap_err();
        let dup_owned = other.add_model_file((*shared).clone()).unwrap_err();
        assert_eq!(dup.to_string(), dup_owned.to_string());
        drop(shared);

        let texts = source.compact_model_asts().unwrap();
        assert_eq!(texts.len(), before.len());
        for ((text, mf), ast) in texts.iter().zip(source.model_files()).zip(&before) {
            assert_eq!(&**text, serde_json::to_string(ast).unwrap());
            assert_eq!(mf.ast(), ast);
        }
        let mut alone = manager();
        let texts = alone.compact_model_asts().unwrap();
        for (text, ast) in texts.iter().zip(&before) {
            assert_eq!(&**text, serde_json::to_string(ast).unwrap());
        }
        assert_eq!(
            alone
                .model_files()
                .map(|mf| mf.ast().clone())
                .collect::<Vec<_>>(),
            before
        );
    }

    /// TS: `Field.getDefaultValue` (src/introspect/field.ts) reads
    /// `this.ast.defaultValue` straight off the raw AST, for every field kind
    /// — including `DateTimeProperty`, which the official metamodel does not
    /// declare a `defaultValue` field on at all (module doc on
    /// [`ModelManager::property_default_value`]), so a typed per-variant read
    /// would silently drop one. Covers a present string default, an absent
    /// one, one explicitly `null` in the AST (also `None`, the same as
    /// absent: `Field.getDefaultValue`'s own doc says falsy-but-not-`false`
    /// values are still returned, but TS's `null` and `undefined` are
    /// indistinguishable through a plain property read, and `filter(!is_null)`
    /// is this port's chosen way to collapse the two), and a `DateTime`
    /// field's, which is the case this method exists for.
    #[test]
    fn property_default_value_reads_the_raw_ast_including_datetime() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Order", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "status",
                          "isArray": false, "isOptional": true, "defaultValue": "OPEN" },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "note",
                          "isArray": false, "isOptional": true },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "nulled",
                          "isArray": false, "isOptional": true, "defaultValue": null },
                        { "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "placedAt",
                          "isArray": false, "isOptional": true, "defaultValue": "2020-01-01T00:00:00.000Z" }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();

        let prop = |name: &str| {
            mgr.find_property_id("org.example@1.0.0.Order", name)
                .unwrap()
                .unwrap_or_else(|| panic!("{name} not found"))
        };

        assert_eq!(
            mgr.property_default_value(prop("status")),
            Some(&serde_json::json!("OPEN"))
        );
        assert_eq!(mgr.property_default_value(prop("note")), None);
        assert_eq!(mgr.property_default_value(prop("nulled")), None);
        assert_eq!(
            mgr.property_default_value(prop("placedAt")),
            Some(&serde_json::json!("2020-01-01T00:00:00.000Z"))
        );
        assert!(
            mgr.property_default_value(PropId::from_index(u32::MAX))
                .is_none()
        );
    }

    /// TS: `getDirectSubclasses` builds its population from
    /// `Introspector.getClassDeclarations()`, which reads
    /// `modelManager.getModelFiles()` with no argument and so leaves out
    /// every namespace in `EXCLUDE_NS` (src/basemodelmanager.ts). A fresh
    /// manager has only those, so nothing directly extends the system root.
    #[test]
    #[allow(deprecated)]
    fn direct_subclasses_of_a_fresh_manager_leave_out_the_system_models() {
        let mgr = ModelManager::new().unwrap();
        assert!(
            mgr.get_direct_subclasses("concerto@1.0.0.Concept")
                .unwrap()
                .is_empty()
        );
        assert!(
            mgr.get_direct_subclasses("concerto@1.0.0.Asset")
                .unwrap()
                .is_empty()
        );
    }

    /// Only the user declarations extend the system root, in registration
    /// order: `Person` implicitly, and the enum `Color` implicitly too.
    /// `Asset`, `Participant`, `Transaction`, `Event` and the decorator
    /// model's own declarations are not in the population.
    #[test]
    #[allow(deprecated)]
    fn direct_subclasses_are_only_user_declarations() {
        let mgr = manager();
        assert_eq!(
            mgr.get_direct_subclasses("concerto@1.0.0.Concept").unwrap(),
            ["org.example@1.0.0.Person", "org.example@1.0.0.Color"]
        );
        assert_eq!(
            mgr.get_direct_subclasses("org.example@1.0.0.Person")
                .unwrap(),
            ["org.example@1.0.0.Employee"]
        );
    }

    /// TS `collectSubclasses([this])` always adds the receiver itself, so a
    /// fresh manager's `Concept` is assignable only from itself.
    #[test]
    #[allow(deprecated)]
    fn assignable_class_declarations_of_a_fresh_manager_leave_out_the_system_models() {
        let mgr = ModelManager::new().unwrap();
        assert_eq!(
            mgr.get_assignable_class_declarations("concerto@1.0.0.Concept")
                .unwrap(),
            ["concerto@1.0.0.Concept"]
        );
    }

    #[test]
    #[allow(deprecated)]
    fn assignable_class_declarations_are_the_receiver_and_user_declarations() {
        let mgr = manager();
        assert_eq!(
            mgr.get_assignable_class_declarations("concerto@1.0.0.Concept")
                .unwrap(),
            [
                "concerto@1.0.0.Concept",
                "org.example@1.0.0.Person",
                "org.example@1.0.0.Employee",
                "org.example@1.0.0.Manager",
                "org.example@1.0.0.Color",
            ]
        );
    }

    /// A user asset that implicitly extends the system `Asset` is found; the
    /// system root declarations themselves are not.
    #[test]
    #[allow(deprecated)]
    fn a_user_asset_is_the_only_direct_subclass_of_asset() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Car", "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.Identified" },
                      "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            mgr.get_direct_subclasses("concerto@1.0.0.Asset").unwrap(),
            ["org.acme@1.0.0.Car"]
        );
        assert_eq!(
            mgr.get_assignable_class_declarations("concerto@1.0.0.Asset")
                .unwrap(),
            ["concerto@1.0.0.Asset", "org.acme@1.0.0.Car"]
        );
        assert!(
            mgr.get_direct_subclasses("concerto@1.0.0.Concept")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn preloads_system_model() {
        let mgr = ModelManager::new().unwrap();
        assert!(mgr.get_declaration("concerto@1.0.0.Concept").is_ok());
        assert!(mgr.get_declaration("concerto@1.0.0.Asset").is_ok());
    }

    /// TS: `ModelFile.fromAst` (src/introspect/modelfile.ts) defaults a
    /// `superType`-less `AssetDeclaration` to `Asset` itself, not the generic
    /// `Concept` `ClassDeclaration.process`'s own fallback gives a
    /// `ConceptDeclaration` — so an asset with no explicit `extends` still
    /// inherits `Asset`'s own `$identifier` even though it names its own
    /// explicit identifier too (TS allows the redeclaration-looking overlap
    /// here specifically because the two are never simultaneously in
    /// `getProperties()`'s own duplicate-name check only when both are
    /// literally named `$identifier`, which an explicit `identified by`
    /// field never is).
    #[test]
    #[allow(deprecated)]
    fn an_asset_with_no_extends_implicitly_extends_asset_itself() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme.defaults@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "DefaultAsset", "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "assetId" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "assetId", "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "value", "isArray": false, "isOptional": false }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            mgr.get_super_type("org.acme.defaults@1.0.0.DefaultAsset")
                .unwrap()
                .as_deref(),
            Some("concerto@1.0.0.Asset")
        );
        let props = mgr
            .properties("org.acme.defaults@1.0.0.DefaultAsset")
            .unwrap();
        let names: Vec<(&str, &str)> = props
            .iter()
            .map(|(owner, p)| (owner.as_str(), p.name()))
            .collect();
        assert_eq!(
            names,
            [
                ("org.acme.defaults@1.0.0.DefaultAsset", "assetId"),
                ("org.acme.defaults@1.0.0.DefaultAsset", "value"),
                ("concerto@1.0.0.Asset", "$identifier"),
            ]
        );
    }

    /// P1-07b: a fresh manager preloads `concerto.decorator@1.0.0` as well as
    /// `concerto@1.0.0`, decorator model first, matching TS's
    /// `addDecoratorModel(); addRootModel();`.
    #[test]
    fn preloads_decorator_model_before_root_model() {
        let mgr = ModelManager::new().unwrap();
        assert!(mgr.model_file("concerto.decorator@1.0.0").is_some());
        assert!(mgr.model_file("concerto@1.0.0").is_some());
        let namespaces: Vec<&str> = mgr.model_files().map(ModelFile::namespace).collect();
        assert_eq!(namespaces, ["concerto.decorator@1.0.0", "concerto@1.0.0"]);
    }

    /// P1-07b exit condition: `concerto.decorator@1.0.0.Decorator` and
    /// `DotNetNamespace` resolve on a fresh manager.
    #[test]
    fn decorator_and_dot_net_namespace_resolve() {
        let mgr = ModelManager::new().unwrap();
        let decorator = mgr
            .get_declaration("concerto.decorator@1.0.0.Decorator")
            .unwrap();
        assert_eq!(decorator.name(), "Decorator");
        let dot_net_namespace = mgr
            .get_declaration("concerto.decorator@1.0.0.DotNetNamespace")
            .unwrap();
        assert_eq!(dot_net_namespace.name(), "DotNetNamespace");
    }

    /// P1-07b exit condition: a user model that imports
    /// `concerto.decorator@1.0.0.Decorator` and extends it loads and
    /// validates against the preloaded decorator model.
    #[test]
    fn user_model_extending_decorator_loads_and_validates() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "concerto.decorator@1.0.0", "name": "Decorator" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "CustomDecorator",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Decorator" },
                      "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();

        assert!(
            mgr.get_declaration("org.acme@1.0.0.CustomDecorator")
                .is_ok()
        );
        assert!(
            mgr.is_assignable_to(
                "org.acme@1.0.0.CustomDecorator",
                "concerto.decorator@1.0.0.Decorator"
            )
            .unwrap()
        );
        assert!(mgr.validate_models().is_ok());
    }

    #[test]
    fn duplicate_namespace_rejected() {
        let mut mgr = ModelManager::new().unwrap();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.x@1.0.0", "declarations": []
        });
        mgr.load_model(&model, None).unwrap();
        assert!(mgr.load_model(&model, None).is_err());
    }

    /// TS `_throwAlreadyExists`: a plain `Error`, never the `IllegalModel`
    /// this port raised before (P2-08b review) — with both files' names in
    /// the message when both have one.
    #[test]
    fn duplicate_namespace_names_both_files() {
        let mut mgr = ModelManager::new().unwrap();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.x@1.0.0", "declarations": []
        });
        mgr.load_model(&model, Some("old.cto".into())).unwrap();
        let err = mgr.load_model(&model, Some("new.cto".into())).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Namespace org.x@1.0.0 specified in file new.cto is already declared in file old.cto"
        );
    }

    /// Neither file has a name: both optional clauses drop out.
    #[test]
    fn duplicate_namespace_without_file_names() {
        let mut mgr = ModelManager::new().unwrap();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.x@1.0.0", "declarations": []
        });
        mgr.load_model(&model, None).unwrap();
        let err = mgr.load_model(&model, None).unwrap_err();
        assert_eq!(err.to_string(), "Namespace org.x@1.0.0 is already declared");
    }

    /// [`ModelManager::add_models`] hits the same duplicate-namespace check.
    #[test]
    fn add_models_rejects_a_duplicate_namespace_with_the_ts_message() {
        let mut mgr = ModelManager::new().unwrap();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.x@1.0.0", "declarations": []
        });
        mgr.load_model(&model, Some("old.cto".into())).unwrap();
        let err = mgr
            .load_models([(&model, Some("new.cto".to_string()))])
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Namespace org.x@1.0.0 specified in file new.cto is already declared in file old.cto"
        );
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
        let props = mgr.properties("org.example@1.0.0.Manager").unwrap();
        let names: Vec<&str> = props.iter().map(|(_, p)| p.name()).collect();
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
        mgr.load_model(
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
        assert!(mgr.properties("org.broken@1.0.0.Orphan").is_err());
    }

    /// TS: `EnumDeclaration extends ClassDeclaration` inherits
    /// `getProperties` unchanged, so an enum's values come back the same way
    /// a class's fields do (P2-03 closes the implicit-`Concept` gap this
    /// relies on; `Concept` itself has no properties, so an enum's `Color`
    /// has none to inherit).
    #[test]
    fn get_all_properties_on_enum_gives_its_values() {
        let mgr = manager();
        let properties = mgr.properties("org.example@1.0.0.Color").unwrap();
        let names: Vec<&str> = properties.iter().map(|(_, p)| p.name()).collect();
        assert_eq!(names, ["RED"]);
        assert_eq!(properties[0].0, "org.example@1.0.0.Color");
        assert!(properties[0].1.is_enum_value());
    }

    /// [`manager`] plus `org.other@1.0.0`, which imports from it (and from a
    /// namespace that is not loaded) and declares a concept and a scalar.
    fn manager_with_imports() -> ModelManager {
        let mut mgr = manager();
        mgr.load_model(
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

    #[test]
    fn get_ast_unresolved_leaves_type_names_bare() {
        let mgr = manager_with_imports();
        let ast = mgr.models_ast(false, false).unwrap();
        let models = ast.get("models").and_then(Value::as_array).unwrap();
        assert_eq!(models.len(), 2, "the system namespaces are excluded");
        let other = models
            .iter()
            .find(|m| m.get("namespace").and_then(Value::as_str) == Some("org.other@1.0.0"))
            .unwrap();
        let lead_type = other.pointer("/declarations/0/properties/0/type").unwrap();
        assert_eq!(
            lead_type.get("name").and_then(Value::as_str),
            Some("Person")
        );
        assert!(lead_type.get("namespace").is_none());
    }

    #[test]
    fn get_ast_resolved_adds_the_declaring_namespace() {
        let mgr = manager_with_clean_import();
        let ast = mgr.models_ast(true, false).unwrap();
        let models = ast.get("models").and_then(Value::as_array).unwrap();
        let clean = models
            .iter()
            .find(|m| m.get("namespace").and_then(Value::as_str) == Some("org.clean@1.0.0"))
            .unwrap();
        // `Team.lead: Person` — imported (unaliased) from `org.example@1.0.0`.
        let lead_type = clean.pointer("/declarations/0/properties/0/type").unwrap();
        assert_eq!(
            lead_type.get("namespace").and_then(Value::as_str),
            Some("org.example@1.0.0")
        );
        assert_eq!(
            lead_type.get("name").and_then(Value::as_str),
            Some("Person")
        );
        assert!(lead_type.get("resolvedName").is_none());
        // `Team.label: String` — a primitive, untouched.
        let label = clean.pointer("/declarations/0/properties/1").unwrap();
        assert_eq!(label.get("namespace"), None);
    }

    /// `Email` (`org.clean@1.0.0`) is a `StringScalar`, whose own case
    /// resolves `namespace`/`name` on the node itself, not under `.type`.
    #[test]
    fn get_ast_resolved_resolves_a_scalar_declarations_own_name() {
        let mgr = manager_with_clean_import();
        let resolved = mgr
            .resolve_meta_model(mgr.model_file("org.clean@1.0.0").unwrap().ast())
            .unwrap();
        let email = resolved
            .get("declarations")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .find(|d| d.get("name").and_then(Value::as_str) == Some("Email"))
            .unwrap();
        assert_eq!(
            email.get("namespace").and_then(Value::as_str),
            Some("org.clean@1.0.0")
        );
    }

    /// `Manager` is imported from `org.example@1.0.0` via `ImportTypes`;
    /// resolving a super type that names it adds that namespace.
    #[test]
    fn resolve_meta_model_resolves_an_imported_super_type() {
        let mgr = manager_with_imports();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.super@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.example@1.0.0", "name": "Manager" }
            ],
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Lead", "isAbstract": false,
                  "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Manager" },
                  "properties": [] }
            ]
        });
        let resolved = mgr.resolve_meta_model(&model).unwrap();
        let super_type = resolved.pointer("/declarations/0/superType").unwrap();
        assert_eq!(
            super_type.get("namespace").and_then(Value::as_str),
            Some("org.example@1.0.0")
        );
    }

    /// TS `resolveName`: a plain `Error`, "Name {name} not found", for a
    /// type that resolves to no import and no local declaration.
    #[test]
    fn resolve_meta_model_rejects_an_unresolvable_name() {
        let mgr = manager_with_imports();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.broken@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Orphan", "isAbstract": false,
                  "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Ghost" },
                  "properties": [] }
            ]
        });
        let err = mgr.resolve_meta_model(&model).unwrap_err();
        assert_eq!(err.to_string(), "Name Ghost not found");
    }

    /// TS `createNameTable`'s `ImportType` branch: a plain `Error`, when the
    /// imported declaration itself is not in the target namespace.
    #[test]
    fn resolve_meta_model_rejects_an_import_of_an_undeclared_type() {
        let mgr = manager_with_imports();
        let model = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.broken@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.example@1.0.0", "name": "Nope" }
            ],
            "declarations": []
        });
        let err = mgr.resolve_meta_model(&model).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Declaration Nope in namespace org.example@1.0.0 not found"
        );
    }

    /// TS's `createNameTable` only reads the target model inside
    /// `imp.types.forEach`, so an `ImportTypes` of no types from a namespace
    /// that is not registered resolves; one naming a type is a `TypeError`.
    #[test]
    fn resolve_meta_model_accepts_an_empty_import_types_from_an_unknown_namespace() {
        let mgr = manager();
        let model = |types: serde_json::Value| {
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.lonely@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportTypes",
                      "namespace": "org.missing@1.0.0", "types": types }
                ],
                "declarations": []
            })
        };
        assert_eq!(
            mgr.resolve_meta_model(&model(serde_json::json!([])))
                .unwrap(),
            model(serde_json::json!([]))
        );
        let err = mgr
            .resolve_meta_model(&model(serde_json::json!(["Thing"])))
            .unwrap_err();
        assert!(matches!(
            err.ported(),
            Some(c) if c.kind == ErrorKind::MalformedInput
        ));
    }

    /// [`manager`] plus `org.clean@1.0.0`, which imports `Person` from it
    /// and declares a concept and a scalar — [`manager_with_imports`]
    /// without its deliberately unresolvable import, for tests that resolve
    /// a whole model's metamodel ([`ModelManager::resolve_meta_model`]
    /// walks every import, not just the ones a lookup happens to reach).
    fn manager_with_clean_import() -> ModelManager {
        let mut mgr = manager();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.clean@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.example@1.0.0", "name": "Person" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Team", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "lead", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" } },
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
            .find(|&id| mgr.property_by_id(id).unwrap().name() == name)
            .unwrap();
        Node::Property(id)
    }

    #[test]
    fn handles_survive_later_loads() {
        let mut mgr = manager();
        let person = mgr.declaration_id("org.example@1.0.0.Person").unwrap();
        let file = mgr.model_file_id("org.example@1.0.0").unwrap();
        let generation = mgr.generation();

        mgr.load_model(
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
        assert!(mgr.load_model(&model, None).is_err());
        assert_eq!(mgr.generation(), generation);
        // The two system models (P1-07b) plus `org.example@1.0.0` from `manager()`.
        assert_eq!(mgr.model_files().count(), 3);
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
        assert_eq!(mgr.property_by_id(props[0]).unwrap().name(), "salary");
        assert_eq!(mgr.parent_of(props[0]), Some(employee));

        // An enum's own values get `PropId`s too (P2-04), addressed the same
        // way a class declaration's fields are.
        let color = mgr.declaration_id("org.example@1.0.0.Color").unwrap();
        let color_props: Vec<PropId> = mgr.property_ids(color).collect();
        assert_eq!(color_props.len(), 1);
        assert_eq!(mgr.property_by_id(color_props[0]).unwrap().name(), "RED");
        assert!(mgr.property_by_id(color_props[0]).unwrap().is_enum_value());
        assert_eq!(mgr.parent_of(color_props[0]), Some(color));
        // Model files are listed in load order: the decorator model, then the
        // root model (P1-07b, matching TS's `addDecoratorModel(); addRootModel();`).
        let namespaces: Vec<&str> = mgr.model_files().map(ModelFile::namespace).collect();
        assert_eq!(
            namespaces,
            [
                "concerto.decorator@1.0.0",
                "concerto@1.0.0",
                "org.example@1.0.0"
            ]
        );
    }

    /// TS: `Introspector.getClassDeclarations` (test/introspect/introspector.js).
    #[test]
    fn class_declarations_span_every_loaded_model_file_and_include_enums() {
        let mgr = manager();
        let names: Vec<&str> = mgr
            .class_declarations()
            .map(|id| mgr.declaration(id).unwrap().name())
            .collect();
        // Every user declaration, including the enum `Color` — TS's
        // `!isMapDeclaration?.() && !isScalarDeclaration?.()` leaves an enum
        // in, only a map or scalar out.
        for name in ["Person", "Employee", "Manager", "Color"] {
            assert!(names.contains(&name), "{name} missing from {names:?}");
        }
        // `Introspector.getClassDeclarations` reads
        // `modelManager.getModelFiles()` with no argument, which leaves out
        // the built-in decorator and root models by namespace (`EXCLUDE_NS`,
        // src/basemodelmanager.ts) — so their own class-like declarations,
        // such as the root model's `Concept`, are not in the result.
        assert!(!names.contains(&"Concept"));
    }

    /// A map or scalar declaration is left out of `class_declarations`, the
    /// same way `Introspector.getClassDeclarations` leaves them out of TS's
    /// `instanceof ClassDeclaration` filter.
    #[test]
    fn class_declarations_exclude_maps_and_scalars() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.mapscalar@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Postcode" },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "Lookup",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" } },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person",
                      "isAbstract": false, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
        let names: Vec<&str> = mgr
            .class_declarations()
            .map(|id| mgr.declaration(id).unwrap().name())
            .filter(|n| ["Postcode", "Lookup", "Person"].contains(n))
            .collect();
        assert_eq!(names, ["Person"]);
    }

    /// `child@1.0.0.Child { o Integer age }`, imported into `parent@1.0.0` as
    /// `Kid` (`import child@1.0.0.{Child as Kid}`); `parent@1.0.0`'s own
    /// `Child` concept has a `kid` field of that aliased type. The TS
    /// original (`test/introspect/property.js` "Property - Test for
    /// property types using Import Aliasing", `test/data/aliasing/*.cto`;
    /// P2-04, issue #48) builds this over `ModelManager.resolveMetaModel`,
    /// which this port does not have yet (P2-08); this is the same shape
    /// built directly from AST, the only difference this suite's own three
    /// assertions can see (none of them reads a decorator or a resolved
    /// type reference, the only things `resolveMetaModel` would add).
    fn aliasing_manager() -> ModelManager {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "child@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Child", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "age", "isArray": false, "isOptional": false }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "parent@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "child@1.0.0",
                      "types": ["Child"],
                      "aliasedTypes": [
                        { "$class": "concerto.metamodel@1.0.0.AliasedType", "name": "Child", "aliasedName": "Kid" }
                      ] }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Child", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "kid", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Kid" } }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
        mgr
    }

    /// TS: `property.getType().should.equal('Kid')` — the alias, not the
    /// target's own name, since `Property.process` keeps only `this.ast.type.name`
    /// (property.ts).
    #[test]
    fn an_aliased_import_s_property_keeps_the_local_alias_as_its_type() {
        let mgr = aliasing_manager();
        let child = mgr.declaration_id("parent@1.0.0.Child").unwrap();
        let kid = mgr
            .property_ids(child)
            .find(|id| mgr.property_by_id(*id).unwrap().name() == "kid")
            .unwrap();
        assert_eq!(mgr.property_by_id(kid).unwrap().type_name(), Some("Kid"));
    }

    /// TS: `property.getFullyQualifiedTypeName().should.equal('child@1.0.0.Child')`
    /// — resolved through the import alias to the type it actually names.
    #[test]
    fn an_aliased_import_s_property_resolves_its_fully_qualified_type_name() {
        let mgr = aliasing_manager();
        let child = mgr.declaration_id("parent@1.0.0.Child").unwrap();
        let kid = mgr
            .property_ids(child)
            .find(|id| mgr.property_by_id(*id).unwrap().name() == "kid")
            .unwrap();
        assert_eq!(
            mgr.get_fully_qualified_type_name(&Node::Property(kid))
                .unwrap(),
            "child@1.0.0.Child"
        );
    }

    /// TS: `property.getFullyQualifiedName().should.equal('parent@1.0.0.Child.kid')`.
    #[test]
    fn an_aliased_import_s_property_has_its_own_fully_qualified_name() {
        let mgr = aliasing_manager();
        let child = mgr.declaration_id("parent@1.0.0.Child").unwrap();
        let kid = mgr
            .property_ids(child)
            .find(|id| mgr.property_by_id(*id).unwrap().name() == "kid")
            .unwrap();
        assert_eq!(
            mgr.get_fully_qualified_name(&Node::Property(kid)).unwrap(),
            "parent@1.0.0.Child.kid"
        );
    }

    #[test]
    fn unknown_handles_name_nothing() {
        let mgr = manager();
        let stale = DeclId::from_index(u32::MAX);
        assert!(mgr.declaration(stale).is_none());
        assert!(mgr.model_file_of(stale).is_none());
        assert_eq!(mgr.property_ids(stale).count(), 0);
        assert!(mgr.property_by_id(PropId::from_index(u32::MAX)).is_none());
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
            [
                "org.example@1.0.0.Employee",
                "org.example@1.0.0.Person",
                // `Person` has no `superType` of its own, so it implicitly
                // extends `Concept` (P2-03, `ClassDeclaration` doc comment).
                "concerto@1.0.0.Concept"
            ]
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
            .resolve_type_name_at("org.does.not.exist@1.0.0", "Foo", Some(location.clone()))
            .unwrap_err();
        match err.into_ported() {
            Some(contract) => assert_eq!(contract.location, Some(location)),
            other => panic!("expected a Contract error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_type_name_with_no_location_carries_none() {
        let mgr = ModelManager::new().unwrap();
        let err = mgr
            .resolve_type_name_at("org.does.not.exist@1.0.0", "Foo", None)
            .unwrap_err();
        match err.into_ported() {
            Some(contract) => assert_eq!(contract.location, None),
            other => panic!("expected a Contract error, got {other:?}"),
        }
    }

    /// `org.base@1.0.0.Base`, and `org.dependent@1.0.0.Sub`, which extends it.
    /// Loading `Sub` before `Base` with a single [`ModelManager::add_model`]
    /// succeeds too (loading never validates on its own), but validating the
    /// pair only succeeds once both are loaded, whatever order they loaded
    /// in; [`ModelManager::add_models`] (#26, P1-06) is what does both steps
    /// as one all-or-nothing unit.
    fn base_model() -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.base@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Base", "isAbstract": false, "properties": [] }
            ]
        })
    }

    fn dependent_model() -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.dependent@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.base@1.0.0", "name": "Base" }
            ],
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Sub", "isAbstract": false,
                  "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" },
                  "properties": [] }
            ]
        })
    }

    #[test]
    fn add_models_relaxes_import_order() {
        let base = base_model();
        let dependent = dependent_model();

        // The dependency-last order would defeat a single `add_model` per
        // file followed by eager per-file validation; `add_models` adds both
        // first and validates once, so the order they are listed in does not
        // matter.
        let mut mgr = ModelManager::new().unwrap();
        let ids = mgr
            .load_models([(&dependent, None), (&base, None)])
            .unwrap();
        assert_eq!(ids.len(), 2);
        assert!(mgr.validate_models().is_ok());
        assert!(
            mgr.is_assignable_to("org.dependent@1.0.0.Sub", "org.base@1.0.0.Base")
                .unwrap()
        );

        // The reverse order validates just as cleanly.
        let mut mgr2 = ModelManager::new().unwrap();
        mgr2.load_models([(&base, None), (&dependent, None)])
            .unwrap();
        assert!(mgr2.validate_models().is_ok());
    }

    #[test]
    fn add_models_rolls_back_the_whole_batch_on_validation_failure() {
        let mut mgr = manager();
        let generation = mgr.generation();
        let namespaces_before: Vec<String> = mgr
            .model_files()
            .map(|mf| mf.namespace().to_string())
            .collect();

        // `Sub` extends a `Base` that is never part of this batch, so the
        // batch-wide `validate_models` fails; the dependent model on its own
        // is otherwise well formed, so only the missing super type is at
        // fault.
        let dependent = dependent_model();
        let err = mgr.load_models([(&dependent, None)]).unwrap_err();
        assert!(err.to_string().contains("Base"), "{err}");

        // Nothing from the failed batch survives: not the new namespace, not
        // the generation counter, not the arena length.
        assert_eq!(mgr.generation(), generation);
        assert_eq!(mgr.model_file_id("org.dependent@1.0.0"), None);
        let namespaces_after: Vec<String> = mgr
            .model_files()
            .map(|mf| mf.namespace().to_string())
            .collect();
        assert_eq!(namespaces_after, namespaces_before);
    }

    #[test]
    fn add_models_rolls_back_on_duplicate_namespace_within_the_batch() {
        let mut mgr = ModelManager::new().unwrap();
        let generation = mgr.generation();
        let base = base_model();

        assert!(mgr.load_models([(&base, None), (&base, None)]).is_err());

        assert_eq!(mgr.generation(), generation);
        assert_eq!(mgr.model_file_id("org.base@1.0.0"), None);
    }

    #[test]
    fn add_models_rolls_back_on_duplicate_against_an_existing_model() {
        let mut mgr = manager();
        let generation = mgr.generation();
        let count_before = mgr.model_files().count();

        let clash = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example@1.0.0", "declarations": []
        });
        let other = base_model();
        // The clash is listed second, so the first model in the batch does
        // get inserted before the failure — and must be undone too.
        assert!(mgr.load_models([(&other, None), (&clash, None)]).is_err());

        assert_eq!(mgr.generation(), generation);
        assert_eq!(mgr.model_files().count(), count_before);
        assert_eq!(mgr.model_file_id("org.base@1.0.0"), None);
    }

    #[test]
    fn add_models_leaves_pre_existing_models_validating_on_success() {
        let mut mgr = manager();
        let base = base_model();
        let dependent = dependent_model();
        mgr.load_models([(&dependent, None), (&base, None)])
            .unwrap();
        // The pre-existing models (from `manager()`) are still there and
        // still validate, alongside the two the batch added.
        assert!(mgr.get_declaration("org.example@1.0.0.Manager").is_ok());
        assert!(mgr.validate_models().is_ok());
    }

    // P2-08b: BaseModelManager.resolveType, derivesFrom, isAssignableTo,
    // getAssignableConcreteTypes, getModels, the get<Kind>Declarations
    // family, filter, updateModelFile and deleteModelFile.

    #[test]
    fn resolve_type_passes_primitives_through() {
        let mgr = manager();
        assert_eq!(
            mgr.resolve_type("ctx", "String").unwrap(),
            "String".to_string()
        );
    }

    #[test]
    fn resolve_type_resolves_a_local_type() {
        let mgr = manager();
        assert_eq!(
            mgr.resolve_type("ctx", "org.example@1.0.0.Employee")
                .unwrap(),
            "org.example@1.0.0.Employee"
        );
    }

    #[test]
    fn resolve_type_rejects_an_unregistered_namespace() {
        let mgr = manager();
        let err = mgr
            .resolve_type("org.example@1.0.0.Person", "org.nope@1.0.0.Foo")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "No registered namespace for type \"org.nope@1.0.0.Foo\" in \"org.example@1.0.0.Person\"."
        );
    }

    #[test]
    fn resolve_type_rejects_an_imported_name() {
        let mgr = manager_with_imports();
        // `Person` is imported into `org.other@1.0.0`, not declared there.
        let err = mgr
            .resolve_type("ctx", "org.other@1.0.0.Person")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "No type \"org.other@1.0.0.Person\" in namespace \"org.other@1.0.0\" for \"ctx\"."
        );
    }

    #[test]
    fn derives_from_is_true_for_the_same_type() {
        let mgr = manager();
        assert!(
            mgr.derives_from("org.example@1.0.0.Employee", "org.example@1.0.0.Employee")
                .unwrap()
        );
    }

    #[test]
    fn derives_from_walks_the_super_chain() {
        let mgr = manager();
        assert!(
            mgr.derives_from("org.example@1.0.0.Manager", "org.example@1.0.0.Person")
                .unwrap()
        );
        assert!(
            mgr.derives_from("org.example@1.0.0.Manager", "concerto@1.0.0.Concept")
                .unwrap()
        );
    }

    #[test]
    fn derives_from_is_false_for_the_wrong_direction() {
        let mgr = manager();
        assert!(
            !mgr.derives_from("org.example@1.0.0.Person", "org.example@1.0.0.Manager")
                .unwrap()
        );
    }

    #[test]
    fn derives_from_propagates_gettype_s_error() {
        let mgr = manager();
        assert!(
            mgr.derives_from("org.example@1.0.0.Nope", "org.example@1.0.0.Person")
                .is_err()
        );
    }

    #[test]
    fn base_manager_is_assignable_to_matches_ts_including_the_abstract_check() {
        let mut mgr = manager();
        // Person has no explicit `isAbstract`; make an abstract type to
        // exercise the "false even against itself" branch TS's own test
        // covers (`isAssignableTo should return false when fqn is abstract`).
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.abs@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Base", "isAbstract": true, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();

        assert!(!mgr.is_type_assignable_to("org.abs@1.0.0.Base", "org.abs@1.0.0.Base"));
        assert!(
            mgr.is_type_assignable_to("org.example@1.0.0.Employee", "org.example@1.0.0.Employee")
        );
        assert!(
            mgr.is_type_assignable_to("org.example@1.0.0.Employee", "org.example@1.0.0.Person")
        );
        assert!(
            !mgr.is_type_assignable_to("org.example@1.0.0.Person", "org.example@1.0.0.Employee")
        );
        assert!(!mgr.is_type_assignable_to("org.example@1.0.0.Nope", "org.example@1.0.0.Person"));
    }

    #[test]
    fn get_assignable_concrete_types_leaves_out_the_abstract_base_and_absent_types() {
        let mut mgr = manager();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.abs2@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Base", "isAbstract": true, "properties": [] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Child", "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" }, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();

        let names: Vec<String> = mgr
            .get_assignable_concrete_types("org.abs2@1.0.0.Base")
            .into_iter()
            .filter_map(|id| mgr.declaration(id).map(|d| d.name().to_string()))
            .collect();
        assert_eq!(names, vec!["Child".to_string()]);
        assert!(
            mgr.get_assignable_concrete_types("org.abs2@1.0.0.Nope")
                .is_empty()
        );
    }

    #[test]
    fn get_models_excludes_system_and_decorator_models() {
        let mgr = manager();
        let models = mgr.get_models(true);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].0, "org.example@1.0.0.cto");
    }

    #[test]
    fn get_models_names_a_file_from_its_file_name() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.named@1.0.0", "declarations": []
            }),
            Some("https://example.org/models/".to_string()),
        )
        .unwrap();
        let models = mgr.get_models(true);
        assert_eq!(models, vec![("models".to_string(), None)]);
    }

    #[test]
    fn model_file_by_file_name_finds_the_matching_file() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model_with_definitions(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.named@1.0.0", "declarations": []
            }),
            None,
            Some("models/org.named.cto".to_string()),
        )
        .unwrap();
        let found = mgr
            .model_file_by_file_name("models/org.named.cto")
            .expect("a file was registered under this name");
        assert_eq!(found.namespace(), "org.named@1.0.0");
    }

    #[test]
    fn model_file_by_file_name_is_none_when_nothing_matches() {
        let mgr = manager();
        assert!(mgr.model_file_by_file_name("no-such-file.cto").is_none());
    }

    /// TS `getModelFileByFileName` calls `getModelFiles()` with no
    /// argument, which excludes the built-in decorator and root models
    /// (`EXCLUDE_NS`). So even though those files are registered under
    /// exactly these names (`ModelManager::new`), looking either of them
    /// up by file name must answer `None` (JS `undefined`), the same as
    /// TS, not the system `ModelFile`.
    #[test]
    fn model_file_by_file_name_excludes_the_system_model_files() {
        let mgr = manager();
        assert!(mgr.model_file_by_file_name("concerto_1.0.0.cto").is_none());
        assert!(
            mgr.model_file_by_file_name("concerto_decorator_1.0.0.cto")
                .is_none()
        );
    }

    /// TS `getModelFileByFileName(undefined)` returns the first loaded
    /// model file whose `getName()` is `undefined`, i.e. one added with no
    /// file name; a named file never matches (accordproject/concerto-rust#262).
    #[test]
    fn model_file_by_optional_file_name_none_finds_the_unnamed_file() {
        let mut mgr = ModelManager::new().unwrap();
        assert!(mgr.model_file_by_optional_file_name(None).is_none());
        for (ns, name) in [
            ("org.named@1.0.0", Some("named.cto".to_string())),
            ("org.unnamed@1.0.0", None),
            ("org.unnamed2@1.0.0", None),
        ] {
            mgr.add_model_with_definitions(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": ns, "declarations": []
                }),
                None,
                name,
            )
            .unwrap();
        }
        assert_eq!(
            mgr.model_file_by_optional_file_name(None)
                .map(ModelFile::namespace),
            Some("org.unnamed@1.0.0")
        );
        assert_eq!(
            mgr.model_file_by_optional_file_name(Some("named.cto"))
                .map(ModelFile::namespace),
            Some("org.named@1.0.0")
        );
    }

    #[test]
    fn get_concept_declarations_excludes_the_system_concepts() {
        let mgr = manager();
        let names: Vec<String> = mgr
            .get_concept_declarations()
            .into_iter()
            .filter_map(|id| mgr.declaration(id).map(|d| d.name().to_string()))
            .collect();
        assert_eq!(
            names,
            vec![
                "Person".to_string(),
                "Employee".to_string(),
                "Manager".to_string()
            ]
        );
    }

    #[test]
    fn filter_keeps_only_matching_declarations_and_the_system_models() {
        let mgr = manager();
        let kept = mgr
            .filter_by_fqn(|fqn| fqn == "org.example@1.0.0.Person", false)
            .unwrap();
        assert!(kept.model_file("org.example@1.0.0").is_some());
        assert!(kept.get_declaration("org.example@1.0.0.Person").is_ok());
        assert!(kept.get_declaration("org.example@1.0.0.Employee").is_err());
        assert!(kept.model_file("concerto@1.0.0").is_some());
        assert!(kept.model_file("concerto.decorator@1.0.0").is_some());
    }

    #[test]
    fn filter_drops_a_file_left_with_no_declarations() {
        let mgr = manager();
        let kept = mgr.filter_by_fqn(|_| false, true).unwrap();
        assert!(kept.model_file("org.example@1.0.0").is_none());
    }

    #[test]
    fn update_model_file_replaces_the_registered_file() {
        let mgr = manager();
        let replacement = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Only", "isAbstract": false, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
        let updated = mgr.update_model_file(replacement, true).unwrap();
        assert!(updated.get_declaration("org.example@1.0.0.Only").is_ok());
        assert!(updated.get_declaration("org.example@1.0.0.Person").is_err());
        // `mgr` itself is untouched.
        assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
    }

    /// P5-18 (accordproject/concerto-rust#316): the scratch copy that
    /// validates a file whose namespace is not registered is this manager's
    /// arena with the file appended. It must be exactly the manager a
    /// file-by-file rebuild (the pre-P5-18 copy) produces: the same files in
    /// the same order, the same handles and names, the same generation, and
    /// empty caches. The files it keeps are shared, not deep-cloned.
    #[test]
    fn with_model_file_registered_appends_exactly_as_a_rebuild_would() {
        let mgr = manager();
        // Warm the source's caches: the copy must not inherit them.
        assert!(mgr.properties("org.example@1.0.0.Person").is_ok());
        let fresh = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.new@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "A", "isAbstract": false, "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "s", "isArray": false, "isOptional": false }
                    ] },
                    { "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "E", "properties": [
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "ONE" }
                    ] }
                ]
            }),
            Some("new.cto".into()),
        )
        .unwrap();
        let scratch = mgr
            .with_model_file_registered(Arc::new(fresh.clone()))
            .unwrap();

        let mut rebuilt = ModelManager::default();
        for existing in mgr.model_files() {
            rebuilt.insert(existing.clone()).unwrap();
        }
        rebuilt.insert(fresh.clone()).unwrap();

        let files = |m: &ModelManager| {
            m.files
                .iter()
                .map(|f| (f.model_file.namespace().to_string(), f.declarations.clone()))
                .collect::<Vec<_>>()
        };
        let decls = |m: &ModelManager| {
            m.declarations
                .iter()
                .map(|d| {
                    (
                        d.model_file,
                        d.index,
                        d.properties.clone(),
                        d.fqn.to_string(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let props = |m: &ModelManager| {
            m.properties
                .iter()
                .map(|p| (p.declaration, p.index))
                .collect::<Vec<_>>()
        };
        let namespaces = |m: &ModelManager| {
            let mut v: Vec<_> = m.namespaces.iter().map(|(k, v)| (k.clone(), *v)).collect();
            v.sort();
            v
        };
        assert_eq!(files(&scratch), files(&rebuilt));
        assert_eq!(decls(&scratch), decls(&rebuilt));
        assert_eq!(props(&scratch), props(&rebuilt));
        assert_eq!(namespaces(&scratch), namespaces(&rebuilt));
        assert_eq!(scratch.generation, rebuilt.generation);
        assert_eq!(scratch.cache_counts(), (0, 0, 0));
        for (mine, theirs) in mgr.files.iter().zip(&scratch.files) {
            assert!(Arc::ptr_eq(&mine.model_file, &theirs.model_file));
        }
        assert!(
            scratch
                .model_file("org.new@1.0.0")
                .unwrap()
                .same_ast(&fresh)
        );
        // The source is untouched.
        assert!(mgr.model_file("org.new@1.0.0").is_none());
    }

    /// P5-18: a file whose namespace *is* registered still takes the old
    /// file's place in the order, as the pre-P5-18 copy did.
    #[test]
    fn with_model_file_registered_replaces_in_place() {
        let mgr = manager();
        let replacement = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "declarations": []
            }),
            None,
        )
        .unwrap();
        let scratch = mgr
            .with_model_file_registered(Arc::new(replacement.clone()))
            .unwrap();
        let order = |m: &ModelManager| {
            m.model_files()
                .map(|f| f.namespace().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&scratch), order(&mgr));
        assert!(
            scratch
                .model_file("org.example@1.0.0")
                .unwrap()
                .same_ast(&replacement)
        );
        assert_eq!(scratch.generation, u64::try_from(mgr.files.len()).unwrap());
    }

    #[test]
    fn update_model_file_rejects_an_unregistered_namespace() {
        let mgr = manager();
        let fresh = ModelFile::from_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.new@1.0.0", "declarations": []
            }),
            None,
        )
        .unwrap();
        let err = mgr.update_model_file(fresh, true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Model file for namespace org.new@1.0.0 not found"
        );
    }

    #[test]
    fn delete_model_file_removes_the_namespace() {
        let mgr = manager();
        let deleted = mgr.delete_model_file("org.example@1.0.0").unwrap();
        assert!(deleted.model_file("org.example@1.0.0").is_none());
        assert!(mgr.model_file("org.example@1.0.0").is_some());
    }

    #[test]
    fn delete_model_file_rejects_an_absent_namespace() {
        let mgr = manager();
        let err = mgr.delete_model_file("org.nope@1.0.0").unwrap_err();
        assert_eq!(err.to_string(), "Model file does not exist");
    }

    /// A downloaded `org.ext@1.0.0` declaring `E` (and `extra` when given),
    /// as `updateExternalModels`' downloader returns it.
    fn external(declarations: serde_json::Value) -> ModelFileSource {
        ModelFileSource {
            ast: serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.ext@1.0.0",
                "declarations": declarations
            }),
            definitions: Some("namespace org.ext@1.0.0".into()),
            file_name: Some("@example.com.ext.cto".into()),
        }
    }

    fn concept(name: &str) -> serde_json::Value {
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": name, "isAbstract": false, "properties": [] })
    }

    #[test]
    fn update_external_models_adds_then_updates_a_namespace() {
        let mut mgr = manager();
        let added = mgr
            .update_external_models([external(serde_json::json!([concept("E")]))])
            .unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].file_name(), Some("@example.com.ext.cto"));
        assert!(mgr.get_declaration("org.ext@1.0.0.E").is_ok());
        assert!(mgr.model_file("org.ext@1.0.0").unwrap().is_external());

        // The same namespace again replaces it, in place.
        mgr.update_external_models([external(serde_json::json!([concept("F")]))])
            .unwrap();
        assert!(mgr.get_declaration("org.ext@1.0.0.F").is_ok());
        assert!(mgr.get_declaration("org.ext@1.0.0.E").is_err());
        assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
    }

    #[test]
    fn update_external_models_with_nothing_downloaded_still_validates() {
        let mut mgr = manager();
        assert!(mgr.update_external_models([]).unwrap().is_empty());
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.bad@1.0.0",
                "declarations": [{ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "B", "isAbstract": false,
                    "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing" },
                    "properties": [] }]
            }),
            None,
        )
        .unwrap();
        assert!(mgr.update_external_models([]).is_err());
    }

    #[test]
    fn update_external_models_rolls_back_when_validation_fails() {
        let mut mgr = manager();
        let broken = serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "E", "isAbstract": false,
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing" },
            "properties": [] }]);
        assert!(
            mgr.update_external_models([
                external(serde_json::json!([concept("E")])),
                external(broken)
            ])
            .is_err()
        );
        assert!(mgr.model_file("org.ext@1.0.0").is_none());
        assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
    }

    /// BC-11 (R1): a cyclic chain is an `IllegalModelException` naming the
    /// cycle, from every entry point (TS 5.0.0 overflowed V8's stack or ran
    /// out of memory, DV-013).
    #[test]
    fn circular_inheritance_is_an_illegal_model_error() {
        let concept = |name: &str, sup: &str| {
            serde_json::json!({ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": name, "isAbstract": false, "properties": [],
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": sup } })
        };
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.cycle@1.0.0",
                "declarations": [concept("A", "C"), concept("B", "A"), concept("C", "B")]
            }),
            None,
        )
        .unwrap();
        #[allow(deprecated)]
        let errors = [
            mgr.properties("org.cycle@1.0.0.A").unwrap_err(),
            mgr.validate_models().unwrap_err(),
            mgr.super_types("org.cycle@1.0.0.B").unwrap_err(),
            mgr.get_all_super_type_names("org.cycle@1.0.0.B")
                .unwrap_err(),
            mgr.is_assignable_to("org.cycle@1.0.0.A", "org.cycle@1.0.0.B")
                .unwrap_err(),
            mgr.is_assignable_to("org.cycle@1.0.0.A", "org.cycle@1.0.0.Other")
                .unwrap_err(),
            mgr.derives_from("org.cycle@1.0.0.C", "org.cycle@1.0.0.A")
                .unwrap_err(),
        ];
        for err in errors {
            let Some(c) = err.ported().cloned() else {
                panic!("expected a contract error, got {err:?}");
            };
            assert_eq!(c.kind, ErrorKind::IllegalModel);
            assert_eq!(c.code, "classdeclaration-circularinheritance");
            assert!(
                c.message()
                    .starts_with("The super type chain of \"org.cycle@1.0.0.")
                    && c.message().contains(" is circular: "),
                "{}",
                c.message()
            );
            assert_eq!(c.location, None);
        }
        let err = mgr.properties("org.cycle@1.0.0.A").unwrap_err();
        assert_eq!(
            err.ported().unwrap().message(),
            "The super type chain of \"org.cycle@1.0.0.A\" is circular: org.cycle@1.0.0.A -> org.cycle@1.0.0.C -> org.cycle@1.0.0.B -> org.cycle@1.0.0.A."
        );
    }

    #[test]
    #[allow(deprecated)]
    fn a_super_type_imported_from_an_unregistered_namespace_is_not_defined() {
        // TS `ClassDeclaration._resolveSuperType`, for an *imported* super
        // type, resolves it through
        // `this.modelFile.getModelManager().getType(fqnSuper)`
        // (`BaseModelManager.getType`), whose own unregistered-namespace
        // check raises "Namespace is not defined for type ...". `super_chain`
        // used to re-look-up each step's declaration with
        // `ModelManager::get_declaration` — an internal, un-catalogued,
        // exact-FQN lookup meant for stale-handle detection (its own doc
        // comment) — which raised a bare `TypeNotFoundException` instead.
        // Walking through `get_type_declaration` (P2-08d,
        // accordproject/concerto-rust#151, oracle fixture
        // `unit/ClassDeclaration.getIdentifierFieldName/557a5087518a8343fade9b97`)
        // reuses the same catalogue entry `BaseModelManager.getType` does.
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme.l2@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.acme.l1@1.0.0", "name": "Base" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                      "name": "Vehicle", "isAbstract": false, "properties": [],
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" } }
                ]
            }),
            None,
        )
        .unwrap();
        // `org.acme.l1@1.0.0` (the import's target) is never added.
        for err in [
            mgr.identifier_field_name("org.acme.l2@1.0.0.Vehicle")
                .unwrap_err(),
            mgr.properties("org.acme.l2@1.0.0.Vehicle").unwrap_err(),
        ] {
            let Some(c) = err.ported().cloned() else {
                panic!("expected a contract error, got {err:?}");
            };
            assert_eq!(c.kind, ErrorKind::TypeNotFound);
            assert_eq!(
                c.message(),
                "Namespace is not defined for type \"org.acme.l1@1.0.0.Base\"."
            );
        }
    }

    /// Loads `org.other@1.0.0` (a concept `Shape`) and `org.main@1.0.0`
    /// (a concept `Local`), which imports `Shape` under the alias `Figure`.
    fn aliased_manager() -> ModelManager {
        let mut mgr = ModelManager::new().unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.other@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Shape",
                      "isAbstract": false, "properties": [] }
                ]
            }),
            Some("other.cto".into()),
        )
        .unwrap();
        mgr.load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.main@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.other@1.0.0",
                      "types": ["Shape"],
                      "aliasedTypes": [ { "$class": "concerto.metamodel@1.0.0.AliasedType",
                                          "name": "Shape", "aliasedName": "Figure" } ] }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Local",
                      "isAbstract": false, "properties": [] }
                ]
            }),
            Some("main.cto".into()),
        )
        .unwrap();
        mgr
    }

    /// TS `ModelFile.getType` by name (P5-11, accordproject/concerto-rust#287):
    /// a primitive's own name, a local type's and an aliased import's
    /// fully-qualified name, and `None` for an unknown or unaliased name.
    #[test]
    fn model_file_type_name_answers_like_model_file_get_type() {
        let mgr = aliased_manager();
        let main = mgr.model_file_id("org.main@1.0.0").unwrap();
        let name = |t: &str| mgr.model_file_type_name(main, t).unwrap();
        assert_eq!(name("String").as_deref(), Some("String"));
        assert_eq!(name("Local").as_deref(), Some("org.main@1.0.0.Local"));
        assert_eq!(
            name("org.main@1.0.0.Local").as_deref(),
            Some("org.main@1.0.0.Local")
        );
        assert_eq!(name("Figure").as_deref(), Some("org.other@1.0.0.Shape"));
        assert_eq!(name("Shape"), None);
        assert_eq!(name("Missing"), None);
    }

    /// TS `BaseModelManager.getType` by name (P5-11): the declaration's
    /// fully-qualified name, or the `TypeNotFoundException` for an unknown
    /// namespace or type.
    #[test]
    fn type_declaration_name_answers_like_get_type() {
        let mgr = aliased_manager();
        assert_eq!(
            mgr.type_declaration_name("org.other@1.0.0.Shape").unwrap(),
            "org.other@1.0.0.Shape"
        );
        for missing in [
            "org.nowhere@1.0.0.Shape",
            "org.other@1.0.0.Missing",
            "String",
        ] {
            let err = mgr.type_declaration_name(missing).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::TypeNotFound, "{missing}");
        }
    }

    /// TS `ModelFile.resolveType` (P5-11): a primitive, a local type and an
    /// import resolving in its own namespace pass; any other name is the
    /// undeclared-type `IllegalModelException`, naming the file and carrying
    /// the caller's location.
    #[test]
    fn model_file_resolve_type_rejects_an_undeclared_type() {
        let mgr = aliased_manager();
        let main = mgr.model_file_id("org.main@1.0.0").unwrap();
        for ok in ["Integer", "Local", "Figure"] {
            mgr.model_file_resolve_type(main, "ctx", ok, None).unwrap();
        }
        let location = serde_json::json!({ "start": { "line": 1 } });
        let err = mgr
            .model_file_resolve_type(main, "ctx", "Missing", Some(location.clone()))
            .unwrap_err();
        let contract = err.contract();
        assert_eq!(contract.kind, ErrorKind::IllegalModel);
        assert_eq!(contract.code, "modelfile-resolvetype-undecltype");
        assert_eq!(contract.model_file, Some(Some("main.cto".to_string())));
        assert_eq!(contract.location, Some(location));
    }

    /// TS `_throwAlreadyExists` (P5-11): the plain `Error` naming both files
    /// for a registered namespace; nothing for one that is not registered.
    #[test]
    fn check_namespace_available_names_both_files() {
        let mgr = aliased_manager();
        let err = mgr
            .check_namespace_available("org.other@1.0.0", Some("new.cto"))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        assert_eq!(
            err.to_string(),
            "Namespace org.other@1.0.0 specified in file new.cto is already declared in file other.cto"
        );
        mgr.check_namespace_available("org.free@1.0.0", None)
            .unwrap();
    }

    /// `update_external_models_naming_file` (P5-11) names the namespace of
    /// the file whose validation failed, and leaves the manager unchanged.
    #[test]
    fn update_external_models_names_the_failing_file_and_rolls_back() {
        let mut mgr = aliased_manager();
        let before: Vec<String> = mgr
            .model_files()
            .map(|f| f.namespace().to_string())
            .collect();
        let broken = ModelFileSource {
            ast: serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.broken@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Bad",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Nowhere" },
                      "properties": [] }
                ]
            }),
            definitions: None,
            file_name: Some("@broken.cto".into()),
        };
        let (namespace, err) = mgr
            .update_external_models_naming_file([broken])
            .unwrap_err();
        assert_eq!(namespace.as_deref(), Some("org.broken@1.0.0"));
        assert_eq!(err.kind(), ErrorKind::IllegalModel);
        let after: Vec<String> = mgr
            .model_files()
            .map(|f| f.namespace().to_string())
            .collect();
        assert_eq!(before, after);
    }

    /// P5-97 (accordproject/concerto-rust#448): a model AST with one
    /// namespace and the given declarations and imports.
    fn model(namespace: &str, imports: Value, declarations: Value) -> Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": namespace,
            "imports": imports,
            "declarations": declarations,
        })
    }

    /// P5-97: a concept `name` extending `super_type` (if any), with one
    /// string property `field`.
    fn p597_concept(name: &str, super_type: Option<&str>, field: &str) -> Value {
        let mut decl = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": name, "isAbstract": false,
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": field, "isArray": false, "isOptional": false }
            ]
        });
        if let Some(super_type) = super_type {
            decl["superType"] = serde_json::json!({ "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": super_type });
        }
        decl
    }

    /// P5-97: a user model in `namespace` importing `Person` from the
    /// `manager()` base and declaring `User extends Person`.
    fn user_model(namespace: &str) -> Value {
        model(
            namespace,
            serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.ImportType",
                                 "namespace": "org.example@1.0.0", "name": "Person" }]),
            serde_json::json!([p597_concept("User", Some("Person"), "login")]),
        )
    }

    /// The answers a server reads about one type.
    fn answers(mgr: &ModelManager, fqn: &str) -> (Vec<String>, Vec<String>, Option<String>, bool) {
        (
            mgr.super_types(fqn)
                .unwrap()
                .into_iter()
                .map(|(n, _)| n)
                .collect(),
            mgr.properties(fqn)
                .unwrap()
                .into_iter()
                .map(|(owner, p)| format!("{owner}.{}", p.name()))
                .collect(),
            mgr.super_type(fqn).unwrap().map(|(n, _)| n),
            mgr.is_assignable_to(fqn, "org.example@1.0.0.Person")
                .unwrap(),
        )
    }

    /// P5-97: a fork holds the same files (shared) under the same handles,
    /// with the same options and validated marks, and starts with the
    /// base's warmed caches; adding user models to the fork keeps every
    /// cached base answer and changes none, and neither manager sees the
    /// other's later changes.
    #[test]
    fn fork_shares_files_inherits_caches_and_is_isolated() {
        let mut base = manager();
        base.set_decorator_validation(crate::introspect::decorator::DecoratorValidationOptions {
            missing_decorator: Some("warn".into()),
            invalid_decorator: None,
        });
        base.validate_models().unwrap();
        let fqns = [
            "org.example@1.0.0.Person",
            "org.example@1.0.0.Employee",
            "org.example@1.0.0.Manager",
        ];
        let before: Vec<_> = fqns.iter().map(|f| answers(&base, f)).collect();
        let person = base.declaration_id("org.example@1.0.0.Manager").unwrap();
        let plan = crate::instance::plan::class_plan(&base, person).unwrap();
        assert!(plan.is_settled());
        let warmed = base.cache_counts();
        assert!(warmed.0 >= 3 && warmed.2 >= 1, "{warmed:?}");

        let mut fork = base.fork();
        assert_eq!(fork.cache_counts(), warmed);
        assert_eq!(fork.generation(), base.generation());
        assert_eq!(fork.decorator_validation(), base.decorator_validation());
        for (a, b) in base.shared_model_files().zip(fork.shared_model_files()) {
            assert!(Arc::ptr_eq(a, b));
        }
        for fqn in fqns {
            assert_eq!(base.declaration_id(fqn), fork.declaration_id(fqn));
        }
        let ns = base.model_file_id("org.example@1.0.0").unwrap();
        assert!(fork.known_valid(ns));

        // A request's user models: the base's cached answers stay, unchanged.
        fork.add_model_ast(&user_model("org.user@1.0.0"), Some("user.cto"))
            .unwrap();
        fork.validate_models().unwrap();
        let after_add = fork.cache_counts();
        assert!(
            after_add.0 >= warmed.0 && after_add.2 >= warmed.2,
            "{after_add:?} {warmed:?}"
        );
        let fork_plan = crate::instance::plan::class_plan(&fork, person).unwrap();
        assert!(
            Arc::ptr_eq(&plan, &fork_plan),
            "the base's plan is reused, not rebuilt"
        );
        let after: Vec<_> = fqns.iter().map(|f| answers(&fork, f)).collect();
        assert_eq!(before, after);
        assert!(
            fork.is_assignable_to("org.user@1.0.0.User", "org.example@1.0.0.Person")
                .unwrap()
        );

        // Isolation: the base never sees the fork's models, and a fork
        // never sees the base's or another fork's later ones.
        assert!(base.model_file("org.user@1.0.0").is_none());
        assert!(base.get_type_declaration("org.user@1.0.0.User").is_err());
        let mut other = base.fork();
        other
            .add_model_ast(&user_model("org.user@1.0.0"), Some("other.cto"))
            .unwrap();
        assert_eq!(
            fork.model_file("org.user@1.0.0").unwrap().file_name(),
            Some("user.cto")
        );
        assert_eq!(
            other.model_file("org.user@1.0.0").unwrap().file_name(),
            Some("other.cto")
        );
        base.add_model_ast(&user_model("org.later@1.0.0"), None)
            .unwrap();
        assert!(fork.model_file("org.later@1.0.0").is_none());
        assert!(other.model_file("org.later@1.0.0").is_none());
        let deleted = base.delete_model_file("org.example@1.0.0").unwrap();
        assert!(deleted.model_file("org.example@1.0.0").is_none());
        assert_eq!(
            after,
            fqns.iter().map(|f| answers(&fork, f)).collect::<Vec<_>>()
        );
    }

    /// P5-97: an append keeps a cached answer only when it cannot change:
    /// a plan with an unresolved field type is built again once the type's
    /// namespace is added, and then resolves.
    #[test]
    fn an_append_rebuilds_an_unsettled_plan() {
        let mut mgr = ModelManager::new().unwrap();
        let mut holder = p597_concept("Holder", None, "name");
        holder["properties"].as_array_mut().unwrap().push(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "later", "isArray": false, "isOptional": true,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Later" }
        }));
        mgr.add_model_ast(
            &model(
                "org.holder@1.0.0",
                serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.ImportType",
                                     "namespace": "org.later@1.0.0", "name": "Later" }]),
                serde_json::json!([holder]),
            ),
            None,
        )
        .unwrap();
        let id = mgr.declaration_id("org.holder@1.0.0.Holder").unwrap();
        let plan = crate::instance::plan::class_plan(&mgr, id).unwrap();
        assert!(!plan.is_settled());
        mgr.add_model_ast(
            &model(
                "org.later@1.0.0",
                serde_json::json!([]),
                serde_json::json!([p597_concept("Later", None, "x")]),
            ),
            None,
        )
        .unwrap();
        let rebuilt = crate::instance::plan::class_plan(&mgr, id).unwrap();
        assert!(!Arc::ptr_eq(&plan, &rebuilt));
        assert!(rebuilt.is_settled());
    }

    /// P5-97: `filter` shares a file it keeps unchanged, and does not
    /// validate it again only when the source had validated it and every
    /// file it reaches is shared too; the result is the one the rebuilding
    /// filter gave, and a source never validated still fails the same way.
    #[test]
    fn filter_shares_unchanged_files_and_keeps_validation() {
        let mut base = manager();
        base.add_model_ast(&user_model("org.user@1.0.0"), Some("user.cto"))
            .unwrap();
        base.validate_models().unwrap();
        let keep_all = |fqn: &str| !fqn.starts_with("concerto.decorator@");
        let result = base.filter_by_fqn(keep_all, false).unwrap();
        for ns in ["org.example@1.0.0", "org.user@1.0.0"] {
            let a = base
                .shared_model_files()
                .find(|f| f.namespace() == ns)
                .unwrap();
            let b = result
                .shared_model_files()
                .find(|f| f.namespace() == ns)
                .unwrap();
            assert!(Arc::ptr_eq(a, b), "{ns} is shared");
            let id = result.model_file_id(ns).unwrap();
            assert!(result.known_valid(id), "{ns} is known valid");
        }
        // Dropping `Manager` changes org.example: it is rebuilt, and the
        // user file, still shared, is validated again (its proof no longer
        // holds), exactly as before.
        let partial = base
            .filter_by_fqn(|fqn| keep_all(fqn) && !fqn.ends_with(".Manager"), false)
            .unwrap();
        let example = partial
            .shared_model_files()
            .find(|f| f.namespace() == "org.example@1.0.0")
            .unwrap();
        assert!(!base.shared_model_files().any(|f| Arc::ptr_eq(f, example)));
        let user = partial.model_file_id("org.user@1.0.0").unwrap();
        let proof = partial.files[user.slot()].proof.clone().unwrap();
        assert!(
            !partial.proof_holds(&proof),
            "its import was rebuilt: validated again"
        );
        assert!(partial.known_valid(user), "and it passed");
        let names = |m: &ModelManager| -> Vec<String> {
            m.super_types("org.user@1.0.0.User")
                .unwrap()
                .into_iter()
                .map(|(n, _)| n)
                .collect()
        };
        assert_eq!(names(&partial), names(&base));

        // A source that never validated an invalid file: the filter
        // validates it and throws, as the rebuilding filter did.
        let mut unvalidated = manager();
        unvalidated
            .add_model_ast(
                &model(
                    "org.bad@1.0.0",
                    serde_json::json!([]),
                    serde_json::json!([p597_concept("Bad", Some("Nowhere"), "x")]),
                ),
                None,
            )
            .unwrap();
        let err = unvalidated.filter_by_fqn(keep_all, false).unwrap_err();
        let expected = unvalidated.validate_models().unwrap_err();
        assert_eq!(err.to_string(), expected.to_string());
        assert!(unvalidated.filter_by_fqn(keep_all, true).is_ok());
    }

    /// P5-97: a proof holds only under the source's options, and the
    /// validated marks go when an option changes or a batch rolls back.
    #[test]
    fn validated_marks_follow_options_and_rollbacks() {
        let mut base = manager();
        base.validate_models().unwrap();
        let id = base.model_file_id("org.example@1.0.0").unwrap();
        assert!(base.known_valid(id));
        let proof = base.validity_proof("org.example@1.0.0").unwrap();
        let shared = base
            .shared_model_files()
            .find(|f| f.namespace() == "org.example@1.0.0")
            .cloned()
            .unwrap();

        let mut target = ModelManager::new().unwrap();
        target.set_dangerously_allow_reserved_system_type_names_in_user_models(true);
        let tid = target
            .add_shared_model_file_with_proof(Arc::clone(&shared), Some(Arc::clone(&proof)))
            .unwrap();
        assert!(
            !target.known_valid(tid),
            "other options: the proof does not hold"
        );
        let mut same = ModelManager::new().unwrap();
        let sid = same
            .add_shared_model_file_with_proof(shared, Some(proof))
            .unwrap();
        assert!(same.known_valid(sid));

        base.set_metamodel_validation(true);
        assert!(!base.known_valid(id));
        base.validate_models().unwrap();
        assert!(base.known_valid(id));

        // A batch whose validation fails restores the marks.
        let bad = model(
            "org.bad@1.0.0",
            serde_json::json!([]),
            serde_json::json!([p597_concept("Bad", Some("Nowhere"), "x")]),
        );
        let user = user_model("org.user@1.0.0");
        assert!(base.load_models([(&user, None), (&bad, None)]).is_err());
        assert!(base.known_valid(id));
        assert!(base.model_file("org.user@1.0.0").is_none());
    }

    /// A-3 (accordproject/concerto-rust#448): an update or a removal adopts
    /// a rebuilt manager, and the generation keeps rising: it never repeats
    /// one an earlier state had.
    #[test]
    fn generation_never_repeats_across_updates_and_removals() {
        let mut mgr = manager();
        mgr.add_model_ast(&user_model("org.user@1.0.0"), Some("user.cto"))
            .unwrap();
        let mut seen = vec![mgr.generation()];
        mgr.update_model_ast(&user_model("org.user@1.0.0"), Some("user2.cto"))
            .unwrap();
        seen.push(mgr.generation());
        mgr.remove_model("org.user@1.0.0").unwrap();
        seen.push(mgr.generation());
        mgr.add_model_ast(&user_model("org.other@1.0.0"), None)
            .unwrap();
        seen.push(mgr.generation());
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
        // A rebuilt manager on its own restarts its count; adopting it does not.
        let rebuilt = mgr.delete_model_file("org.other@1.0.0").unwrap();
        assert!(rebuilt.generation() < mgr.generation());
        let before = mgr.generation();
        mgr.adopt(rebuilt);
        assert_eq!(mgr.generation(), before + 1);
    }

    /// A-4: a removal, an update and the metamodel share the files they
    /// keep (`Arc`), never deep-copy them.
    #[test]
    fn rebuilds_share_the_files_they_keep() {
        let mut mgr = manager();
        mgr.add_model_ast(&user_model("org.user@1.0.0"), None)
            .unwrap();
        let file = |m: &ModelManager, ns: &str| {
            m.shared_model_files()
                .find(|f| f.namespace() == ns)
                .cloned()
                .unwrap()
        };
        let example = file(&mgr, "org.example@1.0.0");
        let deleted = mgr.delete_model_file("org.user@1.0.0").unwrap();
        assert!(Arc::ptr_eq(&example, &file(&deleted, "org.example@1.0.0")));
        for (a, b) in mgr
            .shared_model_files()
            .take(2)
            .zip(deleted.shared_model_files())
        {
            assert!(Arc::ptr_eq(a, b), "the system files are shared");
        }
        let replacement = ModelFile::from_json(&user_model("org.user@1.0.0"), None).unwrap();
        let updated = mgr.update_model_file(replacement, false).unwrap();
        assert!(Arc::ptr_eq(&example, &file(&updated, "org.example@1.0.0")));
        let a = crate::instance::metamodel::metamodel_model_file().unwrap();
        let b = crate::instance::metamodel::metamodel_model_file().unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    /// A-5: a filter's result keeps every option of the manager it filters,
    /// `metamodel_validation` included, as TS's
    /// `new BaseModelManager({...this.options})` does.
    #[test]
    fn filter_keeps_every_option() {
        let mut mgr = manager();
        mgr.set_metamodel_validation(true);
        mgr.set_dangerously_allow_reserved_system_type_names_in_user_models(true);
        mgr.set_decorator_validation(crate::introspect::decorator::DecoratorValidationOptions {
            missing_decorator: Some("warn".into()),
            invalid_decorator: None,
        });
        // (Keeping the decorator model's declarations too would register it
        // twice, as TS's `filter(() => true)` does.)
        let filtered = mgr
            .filter(|fqn, _| fqn.starts_with("org.example@"))
            .unwrap();
        assert_eq!(filtered.options, mgr.options);
        assert_eq!(mgr.fork().options, mgr.options);
        let deleted = mgr.delete_model_file("org.example@1.0.0").unwrap();
        assert_eq!(deleted.options, mgr.options);
    }

    /// A-4: external models are each registered once, shared with the list
    /// returned, and a new namespace is appended in place.
    #[test]
    fn external_models_are_shared_with_the_list_returned() {
        let mut mgr = manager();
        let external = |ns: &str, name: &str| ModelFileSource {
            ast: model(
                ns,
                serde_json::json!([]),
                serde_json::json!([p597_concept(name, None, "x")]),
            ),
            definitions: None,
            file_name: Some(format!("@{ns}.cto")),
        };
        let added = mgr
            .update_external_models([
                external("org.ext.a@1.0.0", "A"),
                external("org.ext.b@1.0.0", "B"),
                external("org.ext.a@1.0.0", "A2"),
            ])
            .unwrap();
        assert_eq!(added.len(), 3);
        let held = |ns: &str| {
            mgr.shared_model_files()
                .find(|f| f.namespace() == ns)
                .cloned()
                .unwrap()
        };
        assert!(Arc::ptr_eq(&added[1], &held("org.ext.b@1.0.0")));
        assert!(Arc::ptr_eq(&added[2], &held("org.ext.a@1.0.0")));
        assert!(mgr.get_declaration("org.ext.a@1.0.0.A2").is_ok());
        assert!(mgr.get_declaration("org.ext.a@1.0.0.A").is_err());
    }
}

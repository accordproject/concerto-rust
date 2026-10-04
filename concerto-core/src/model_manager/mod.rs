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
//! The manager owns the model graph. Its model files, declarations and
//! properties sit in an append-only arena, addressed by dense `u32` handles
//! ([`ModelFileId`], [`DeclId`], [`PropId`]); looking an element up is an
//! index, never a string hash. Loading a model only appends, so a handle
//! keeps naming the same element across every later load. A failed batch is
//! undone by truncating each table back to its length before the call, which
//! touches no earlier handle. Replacing or removing a model file
//! (`update_model_file`, `delete_model_file`, `update_model_ast`,
//! `remove_model`) rebuilds the arena from the surviving files, and every
//! handle handed out before it is invalid after it.
//! The manager counts its mutations internally (its state version), which
//! keys its own once-per-state checks; it is not exported, and a binding
//! that caches snapshots keeps its own counter (concerto-wasm's epoch).
//! For the same manager, outside a rolled-back batch, the count never
//! returns to an earlier value for a different state; a rolled back batch
//! restores it, so the next mutation can reuse a value.
//!
//! A ported member reaches its collaborators through the
//! `ResolutionContext` trait, which the manager implements over the arena.

use std::cell::RefCell;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use crate::hash::{FastSeededHashMap, SeededHashSet};
use rustc_hash::FxHashSet;

use crate::json::Value;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::FullyQualified;
use crate::introspect::declaration::{ClassDeclaration, ClassKind, Declaration, EnumDeclaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::model_util::{
    self, PRIMITIVE_TYPES, get_namespace, is_primitive_type, namespace_of, qualify, short_name,
};
use crate::rootmodel::{decorator_model_ast, root_model_ast};

mod caches;
mod deprecated;
mod handles;
mod hierarchy;
mod lookups;
mod resolution_context;
mod ts_compat;

use caches::DeclCache;
use hierarchy::{ClassInfo, ClassLike};

pub use handles::{DeclId, ModelFileId, PropId};
js_compat_pub! {
    pub use handles::Node;
}
js_compat_pub! {
    pub use caches::ValidityProof;
}
js_compat_pub! {
    pub use hierarchy::ClassProperties;
}
js_compat_pub! {
    pub use resolution_context::{ResolutionContext, ValidatedElement};
}
#[cfg(feature = "js-compat")]
pub use ts_compat::ModelFileSource;

/// The namespaces TS `BaseModelManager.getModelFiles()` leaves out unless it
/// is asked to include them: the system model, its unversioned name, and the
/// decorator model. The match is on the exact namespace string.
///
/// TS: `EXCLUDE_NS` (src/basemodelmanager.ts).
///
pub(crate) const EXCLUDE_NS: [&str; 3] = ["concerto@1.0.0", "concerto", "concerto.decorator@1.0.0"];

/// A loaded model file, and the handles of its declarations.
///
/// The model file is shared (`Arc`): a model file never changes once
/// registered, so a scratch copy of the arena
/// ([`ModelManager::with_model_file_registered`]) shares every file it
/// keeps instead of deep-cloning it.
#[derive(Debug)]
struct FileSlot {
    model_file: Arc<ModelFile>,
    declarations: Range<u32>,
    /// Set once the file has passed [`ModelManager::validate_model_file`] in
    /// this manager, so a later [`ModelManager::validate_models`] skips it.
    /// Validity reads only the file, the files its imports reach, and the
    /// options, so the mark holds until the options change (each setter
    /// clears every mark), a failed batch is rolled back, or the manager is
    /// rebuilt.
    validated: std::sync::atomic::AtomicBool,
    /// What lets this manager take the file as validated without
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

/// Where a declaration is: its model file, its position in
/// [`ModelFile::declarations`], the handles of its properties, and its
/// fully-qualified name (TS keeps it on the declaration), built once when
/// the file is registered. The name is shared (`Arc<str>`), so copying the
/// arena copies no string.
#[derive(Debug, Clone)]
struct DeclSlot {
    model_file: ModelFileId,
    index: usize,
    properties: Range<u32>,
    fqn: Arc<str>,
}

/// The arena lengths and state version at one point, which
/// [`ModelManager::rollback`] returns the arena to: before
/// [`ModelManager::append_for_validation`] appended a file, or before
/// a batch.
pub(crate) struct AppendMark {
    files: usize,
    declarations: usize,
    properties: usize,
    state_version: u64,
}

/// Where a property is: its declaration, and its position in
/// [`ClassDeclaration::own_properties`].
#[derive(Debug, Clone)]
struct PropSlot {
    declaration: DeclId,
    index: usize,
}

/// The options of a [`ModelManager`] (TS `ModelManagerOptions`), held as
/// one value: the builder fills it, and every manager derived from another
/// (a fork, a filter's result, a scratch copy, a rebuild after a removal)
/// copies it whole.
#[derive(Debug, Clone, Default, PartialEq)]
struct ManagerOptions {
    /// TS `ModelManagerOptions.decoratorValidation`, `DEFAULT_DECORATOR_VALIDATION`
    /// by default (both fields `None`): see
    /// [`crate::introspect::decorator::DecoratorValidationOptions`].
    decorator_validation: crate::introspect::decorator::DecoratorValidationOptions,
    /// TS `ModelManagerOptions.dangerouslyAllowReservedSystemTypeNamesInUserModels`
    /// (`Declaration.validate`). `false` by default: a transitional escape
    /// hatch that lets a user model redeclare a name it imports from the
    /// system namespace when it is one of the five reserved system
    /// declarations.
    allow_reserved_system_type_names: bool,
    /// TS `ModelManagerOptions.metamodelValidation`: when truthy,
    /// `addModelFile` checks a new model file's AST against the metamodel
    /// ([`ModelManager::validate_ast`]) before its semantic validation.
    /// `false` by default.
    metamodel_validation: bool,
    /// TS `ModelManagerOptions.addMetamodel`: set by
    /// [`ModelManager::add_metamodel`], so that [`ModelManager::filter`]'s
    /// new manager holds the metamodel from the start, as TS's constructor
    /// does, and keeps it whole (BC-53).
    add_metamodel: bool,
}

/// Owns a set of model files and resolves types across them.
#[derive(Debug, Default)]
pub struct ModelManager {
    files: Vec<FileSlot>,
    /// Keyed by namespaces from user models, so hashed with the per-process
    /// seeded foldhash ([`FastSeededState`](crate::hash::FastSeededState);
    /// PORTING.md 3.7).
    namespaces: FastSeededHashMap<String, ModelFileId>,
    declarations: Vec<DeclSlot>,
    properties: Vec<PropSlot>,
    state_version: u64,
    /// The manager's options: one value, so every manager derived from
    /// this one (a fork, a filter, a scratch copy) starts from
    /// `self.options.clone()` and never drops one.
    options: ManagerOptions,
    /// Every per-declaration answer computed so far, by declaration handle
    /// ([`DeclCache`]): inheritance chains, validation plans and field
    /// defaults, under one lock. An answer depends only on the registered
    /// files: an append keeps every answer it cannot change
    /// ([`ModelManager::keep_caches_for_append`]), and any other change
    /// drops them all ([`ModelManager::invalidate_caches`]).
    decl_cache: DeclCache,
    /// `state_version + 1` when [`ModelManager::has_system_files_of`] last
    /// found this manager's system model files to be the resident
    /// metamodel manager's own; 0 before any such check.
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

/// The next handle of an arena table holding `len` entries. A full arena
/// (no TS counterpart; no model reaches four billion elements) is a
/// pre-port `IllegalModel` error rather than a panic.
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

/// A handle this manager never handed out: a caller's bug with no TS
/// counterpart (TS passes object references), so a pre-port `TypeNotFound`
/// with no catalogue entry.
fn unknown(node: Node) -> Error {
    Error::type_not_found(format!("{node:?}"))
}

/// TS `BaseModelManager._throwAlreadyExists(modelFile)`: a plain `Error` for
/// a model file whose namespace is already registered. `existing` holds
/// `namespace`; `new_file_name` is the incoming file's name, when the caller
/// has one.
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
    /// ASTs once per thread and shared by every new manager: a model file is
    /// a pure function of its AST and file name and is never changed once
    /// registered, so a shared file is indistinguishable from a fresh load.
    static SYSTEM_MODEL_FILES: RefCell<Option<(Arc<ModelFile>, Arc<ModelFile>)>> =
        const { RefCell::new(None) };
}

/// The decorator and root system model files, as [`ModelManager::new`]
/// loads them (see [`SYSTEM_MODEL_FILES`]). A load error is returned, not
/// cached.
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
    /// TS: `BaseModelManager`'s constructor runs `addDecoratorModel()` then
    /// `addRootModel()`, each adding a vendored AST with validation disabled;
    /// a load here never validates.
    pub fn new() -> Result<Self> {
        let mut mgr = Self::default();
        // TS: the vendored `.cto` file names `addDecoratorModel`/
        // `addRootModel` pass to `addModelFile`, which `getName()` returns.
        let (decorator, root) = system_model_files()?;
        mgr.insert_shared(decorator)?;
        mgr.insert_shared(root)?;
        Ok(mgr)
    }

    /// [`ModelManager::add_model`]'s load, for the crate's own callers.
    pub(crate) fn load_model(
        &mut self,
        value: &crate::json::Value,
        file_name: Option<String>,
    ) -> Result<()> {
        self.add_model_with_definitions(value, None, file_name)
    }

    js_compat_pub! {
        /// [`ModelManager::add_model`], keeping `definitions`, the CTO source
        /// text, as TS's `ModelFile.getDefinitions()` does. A model loaded
        /// from an AST has none, as in TS's `astProcessFile`.
        pub fn add_model_with_definitions(
            &mut self,
            value: &crate::json::Value,
            definitions: Option<String>,
            file_name: Option<String>,
        ) -> Result<()> {
            let mf = ModelFile::from_json_with_definitions(value, definitions, file_name)?;
            self.add_loaded_model_file(mf).map(drop)
        }
    }

    /// [`ModelManager::add_model_with_definitions`], taking ownership of the
    /// AST so it is kept without being copied. Same result and errors.
    #[cfg(feature = "js-compat")]
    pub fn add_owned_model_with_definitions(
        &mut self,
        value: crate::json::Value,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> Result<()> {
        let mf = ModelFile::from_owned_json_with_definitions(value, definitions, file_name)?;
        self.add_loaded_model_file(mf).map(drop)
    }

    js_compat_pub! {
        /// Adds an already-built model file, such as one
        /// [`ModelFile::from_json_text`] read: the duplicate-namespace check
        /// and registration [`ModelManager::add_model_with_definitions`] runs
        /// once it has built the file itself.
        #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
        pub fn add_model_file(&mut self, mf: ModelFile) -> Result<()> {
            self.add_loaded_model_file(mf).map(drop)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::add_model_file`] for a model file that may also
        /// be registered in another manager: the same duplicate-namespace
        /// check and the same errors, but the file is shared, not copied,
        /// as [`ModelManager::shared_model_files`] hands it out.
        pub fn add_shared_model_file(&mut self, mf: Arc<ModelFile>) -> Result<()> {
            self.register_new(mf).map(drop)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::model_files`], as the shared handles this manager
        /// keeps them in: a model file never changes once registered, so
        /// another manager can register the same file
        /// ([`ModelManager::add_shared_model_file`]) without copying it.
        pub fn shared_model_files(&self) -> impl Iterator<Item = &Arc<ModelFile>> {
            self.files.iter().map(|slot| &slot.model_file)
        }
    }

    js_compat_pub! {
        /// The model file a handle names, as the shared handle this manager
        /// keeps it in ([`ModelManager::shared_model_files`]).
        #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
        pub fn shared_file(&self, id: ModelFileId) -> Option<&Arc<ModelFile>> {
            self.files.get(id.slot()).map(|slot| &slot.model_file)
        }
    }

    /// A new manager over the same models: the same options, the same model
    /// files in the same order (shared, never copied), and the same handles.
    /// Nothing is validated again, and the fork starts with this manager's
    /// warmed caches, so a server can warm one base manager and fork it per
    /// request. The two managers are independent from then on.
    #[cfg(feature = "js-compat")]
    pub fn fork(&self) -> Self {
        Self {
            files: self.files.clone(),
            namespaces: self.namespaces.clone(),
            declarations: self.declarations.clone(),
            properties: self.properties.clone(),
            state_version: self.state_version,
            options: self.options.clone(),
            decl_cache: self.decl_cache.snapshot(),
            system_files_checked: std::sync::atomic::AtomicU64::new(
                self.system_files_checked
                    .load(std::sync::atomic::Ordering::Relaxed),
            ),
        }
    }

    js_compat_pub! {
        /// [`ModelManager::add_shared_model_file`], with the
        /// [`ValidityProof`] the source manager gave for the file, if any:
        /// [`ModelManager::validate_models`] then takes the file as valid
        /// without validating it when the proof holds here.
        pub fn add_shared_model_file_with_proof(
            &mut self,
            mf: Arc<ModelFile>,
            proof: Option<Arc<ValidityProof>>,
        ) -> Result<ModelFileId> {
            let id = self.register_new(mf)?;
            self.files[id.slot()].proof = proof;
            Ok(id)
        }
    }

    /// Each model file's AST as compact JSON text, in
    /// [`ModelManager::model_files`] order ([`ModelFile::compact_ast`]).
    /// A file only this manager holds keeps its AST as that text from then
    /// on (a shared file is serialised and left as it is). Every
    /// [`ModelFile::ast`] stays equal, so nothing a caller reads changes and
    /// the caches stay valid.
    #[cfg(feature = "js-compat")]
    pub fn compact_model_asts(&mut self) -> serde_json::Result<Vec<Arc<str>>> {
        self.files
            .iter_mut()
            .map(|slot| match Arc::get_mut(&mut slot.model_file) {
                Some(model_file) => model_file.compact_ast(),
                None => Ok(Arc::from(serde_json::to_string(slot.model_file.ast())?)),
            })
            .collect()
    }

    /// The duplicate-namespace check and registration
    /// [`ModelManager::add_model_with_definitions`] runs once the file is
    /// loaded.
    fn add_loaded_model_file(&mut self, mf: ModelFile) -> Result<ModelFileId> {
        self.register_new(Arc::new(mf))
    }

    /// Registers `model_file` under a namespace no file holds yet: TS
    /// `addModelFile`'s already-exists check
    /// ([`ModelManager::check_namespace_available`]), then
    /// [`ModelManager::insert_shared`].
    fn register_new(&mut self, model_file: Arc<ModelFile>) -> Result<ModelFileId> {
        self.check_namespace_available(model_file.namespace(), model_file.file_name())?;
        self.insert_shared(model_file)
    }

    /// The arena lengths and state version now, for
    /// [`ModelManager::rollback`] to return to.
    fn mark(&self) -> AppendMark {
        AppendMark {
            files: self.files.len(),
            declarations: self.declarations.len(),
            properties: self.properties.len(),
            state_version: self.state_version,
        }
    }

    /// Takes the files appended since `mark` out of the arena: truncates each
    /// table, removes their namespaces and drops the caches, which may hold
    /// their handles. The appended files are the arena's tail, so no handle
    /// from before `mark` is touched. The state version is the caller's to
    /// set.
    fn truncate_to(&mut self, mark: &AppendMark) {
        for slot in self.files.get(mark.files..).unwrap_or_default() {
            let namespace = slot.model_file.namespace();
            if self
                .namespaces
                .get(namespace)
                .is_some_and(|id| id.slot() >= mark.files)
            {
                self.namespaces.remove(namespace);
            }
        }
        self.files.truncate(mark.files);
        self.declarations.truncate(mark.declarations);
        self.properties.truncate(mark.properties);
        self.invalidate_caches();
    }

    /// Undoes every append since `mark`: the arena, the namespaces and the
    /// state version are as they were then ([`ModelManager::truncate_to`]).
    fn rollback(&mut self, mark: AppendMark) {
        self.truncate_to(&mark);
        self.state_version = mark.state_version;
    }

    /// Registers `model_file` in place for
    /// [`ModelManager::validate_and_add_model_file`], when its namespace is free
    /// and every file is under its own namespace. Returns the handle and what
    /// [`ModelManager::undo_append`] needs, or `None`, changing nothing.
    #[cfg(feature = "js-compat")]
    pub(crate) fn append_for_validation(
        &mut self,
        model_file: &Arc<ModelFile>,
    ) -> Option<(ModelFileId, AppendMark)> {
        let namespace = model_file.namespace();
        if self.namespaces.contains_key(namespace) || self.namespaces.len() != self.files.len() {
            return None;
        }
        let mark = self.mark();
        let id = self.insert_shared(Arc::clone(model_file)).ok()?;
        Some((id, mark))
    }

    /// Takes out the file [`ModelManager::append_for_validation`] appended,
    /// leaving the arena, the namespaces and the state version as they were
    /// before it (as [`ModelManager::load_models`] rolls a batch back). The
    /// caches are dropped: they may hold the appended file's handles.
    #[cfg(feature = "js-compat")]
    pub(crate) fn undo_append(&mut self, mark: AppendMark) {
        self.rollback(mark);
    }

    /// Loads a batch of models irrespective of import order between them:
    /// every model is added, then the whole manager is validated once with
    /// [`ModelManager::validate_models`]. If loading or validation fails, the
    /// batch has no effect and the error is returned.
    ///
    /// TS: `BaseModelManager.addModelFiles`.
    pub(crate) fn load_models<'a>(
        &mut self,
        models: impl IntoIterator<Item = (&'a crate::json::Value, Option<String>)>,
    ) -> Result<Vec<ModelFileId>> {
        self.register_batch(
            models.into_iter().map(|(value, file_name)| {
                ModelFile::from_json(value, file_name).map(|mf| (Arc::new(mf), None))
            }),
            true,
        )
    }

    /// A scratch copy of this manager, sharing its files, with `model_file`
    /// registered under its namespace (replacing or appended), so a detached
    /// file's local types resolve to itself
    /// ([`ModelManager::validate_detached_model_file`]). It starts with empty
    /// caches, at the state version a file-by-file copy would have.
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
            scratch.state_version = u64::try_from(self.files.len()).unwrap_or(u64::MAX);
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

    /// [`ModelManager::insert_shared`] for an unshared model file: a test
    /// helper (the crate's own loads all go through
    /// [`ModelManager::register_new`]).
    #[cfg(test)]
    fn insert(&mut self, model_file: ModelFile) -> Result<ModelFileId> {
        self.insert_shared(Arc::new(model_file))
    }

    /// Appends a model file, its declarations and their properties to the
    /// arena, and counts the mutation. Nothing is changed if a handle cannot
    /// be allocated. The file may already be registered in another manager:
    /// it is shared, not copied.
    fn insert_shared(&mut self, model_file: Arc<ModelFile>) -> Result<ModelFileId> {
        // An append keeps every cached answer that cannot change
        // (`keep_caches_for_append`).
        self.keep_caches_for_append();
        let file_id = ModelFileId(next_index(self.files.len())?);
        let mut declarations = Vec::new();
        let mut properties = Vec::new();
        for (index, declaration) in model_file.declarations().iter().enumerate() {
            let decl_id = DeclId(next_index(self.declarations.len() + declarations.len())?);
            let first = next_index(self.properties.len() + properties.len())?;
            // A class-like declaration's own properties (an enum's values
            // included) get `PropId`s; a scalar or map has none.
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
        self.state_version += 1;
        Ok(file_id)
    }

    /// A builder for a manager with options: decorator validation,
    /// metamodel validation, and reserved system type names.
    pub fn builder() -> ModelManagerBuilder {
        ModelManagerBuilder::default()
    }

    /// Loads a model from its JSON AST, named `file_name`, and returns its
    /// handle. Only the structure is checked: run
    /// [`ModelManager::validate_models`] once every model is loaded. A second
    /// model with the same namespace is an error.
    ///
    /// TS: `ModelManager.addModel` with an AST, without its validation.
    pub fn add_model_ast(
        &mut self,
        ast: &crate::json::Value,
        file_name: Option<&str>,
    ) -> Result<ModelFileId> {
        let mf = ModelFile::from_json_with_definitions(ast, None, file_name.map(str::to_string))?;
        self.add_loaded_model_file(mf)
    }

    /// [`ModelManager::add_model_ast`] for an AST given as JSON text: the
    /// same result, read without building a `crate::json::Value` first.
    /// Text that is not JSON is an `IllegalModel` error.
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
        models: impl IntoIterator<Item = (&'a crate::json::Value, Option<&'a str>)>,
    ) -> Result<Vec<ModelFileId>> {
        self.load_models(
            models
                .into_iter()
                .map(|(ast, file_name)| (ast, file_name.map(str::to_string))),
        )
    }

    /// Replaces the loaded model with `ast`'s namespace and returns the new
    /// handle; only the structure is checked. Every earlier handle is invalid
    /// after the call. An error, changing nothing, when no such model is loaded.
    ///
    /// TS: `BaseModelManager.updateModelFile`, without its validation.
    pub fn update_model_ast(
        &mut self,
        ast: &crate::json::Value,
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

    /// Sets the decorator validation options, TS's constructor
    /// `options.decoratorValidation`, on a manager already built.
    #[cfg(feature = "js-compat")]
    pub fn set_decorator_validation(
        &mut self,
        options: crate::introspect::decorator::DecoratorValidationOptions,
    ) {
        // Validity depends on the options.
        self.clear_validated();
        self.options.decorator_validation = options;
    }

    js_compat_pub! {
        /// TS: `modelFile.getModelManager()?.options?.dangerouslyAllowReservedSystemTypeNamesInUserModels`,
        /// coerced with `Boolean(...)` (`Declaration.validate`, declaration.ts).
        pub fn dangerously_allow_reserved_system_type_names_in_user_models(&self) -> bool {
            self.options.allow_reserved_system_type_names
        }
    }

    /// Sets the escape hatch above, TS's constructor
    /// `options.dangerouslyAllowReservedSystemTypeNamesInUserModels`.
    #[cfg(feature = "js-compat")]
    pub fn set_dangerously_allow_reserved_system_type_names_in_user_models(&mut self, allow: bool) {
        self.clear_validated();
        self.options.allow_reserved_system_type_names = allow;
    }

    /// TS: `this.options?.metamodelValidation`, as `addModelFile` reads it
    /// (JS truthiness). See [`Self::validate_ast`].
    pub fn metamodel_validation(&self) -> bool {
        self.options.metamodel_validation
    }

    /// Sets the option above, TS's constructor `options.metamodelValidation`.
    /// `add_model` never validates, so a caller replaying TS's validating
    /// `addModelFile` runs [`Self::validate_ast`] when this is set, then the
    /// file's semantic validation.
    #[cfg(feature = "js-compat")]
    pub fn set_metamodel_validation(&mut self, metamodel_validation: bool) {
        self.clear_validated();
        self.options.metamodel_validation = metamodel_validation;
    }

    /// TS `BaseModelManager.validateAst(modelFile)`
    /// (`src/basemodelmanager.ts`): checks `model_file`'s AST against the
    /// metamodel, resolved through *this* manager.
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
    /// **A failure in step 3 leaves the metamodel registered**, as in TS,
    /// whose `deleteModelFile` follows the `try`/`catch` that re-throws.
    /// When this manager does not hold the metamodel, steps 2 to 4 first run
    /// against the resident metamodel manager, with the same outcome and end
    /// state.
    pub fn validate_ast(&mut self, model_file: &ModelFile) -> Result<()> {
        self.validate_ast_value(model_file.ast())
    }

    js_compat_pub! {
        /// [`ModelManager::validate_ast`] over the AST itself (`validateAstValue`),
        /// since building a `ModelFile` first could throw the constructor's error
        /// instead of TS's `MetamodelException`. The resident metamodel manager's
        /// passing answer is used only when this manager's system files are its
        /// own; anything else runs in `self`, so errors and end state are TS's.
        pub fn validate_ast_value(&mut self, ast: &Value) -> Result<()> {
            use crate::instance::metamodel::{
                METAMODEL_NAMESPACE, check_version, deserialize_ast, metamodel_model_file,
            };
            check_version(ast)?;
            let already_has_metamodel = self.model_file(METAMODEL_NAMESPACE).is_some();
            if !already_has_metamodel && self.passes_on_resident_metamodel(ast) {
                return Ok(());
            }
            let mark = self.mark();
            if !already_has_metamodel {
                self.insert_shared(metamodel_model_file()?)?;
            }
            deserialize_ast(self, ast)?;
            if !already_has_metamodel {
                // `deleteModelFile(MetaModelNamespace)`: the metamodel is the
                // arena's tail, so it is truncated, a mutation of its own.
                self.truncate_to(&mark);
                self.state_version += 1;
            }
            Ok(())
        }
    }

    /// Whether `ast` passes the structural check on the resident metamodel
    /// manager, when this manager's system model files match its. `false`
    /// also when the resident manager cannot be built, so the caller's own
    /// path reports that error.
    fn passes_on_resident_metamodel(&self, ast: &Value) -> bool {
        use crate::instance::metamodel::{deserialize_ast, with_resident_metamodel_manager};
        with_resident_metamodel_manager(|resident| {
            Ok(self.has_system_files_of(resident) && deserialize_ast(resident, ast).is_ok())
        })
        .unwrap_or(false)
    }

    /// Whether this manager holds the same decorator and root model files
    /// as `other`: the same shared file (the usual case, as both managers
    /// hold `system_model_files`' own), or else the same AST, which is all
    /// a model file's lookups are built from. Answered once per state
    /// version.
    fn has_system_files_of(&self, other: &ModelManager) -> bool {
        use std::sync::atomic::Ordering;
        // `state_version + 1`, so that the default 0 means "not checked".
        let checked = self.state_version.wrapping_add(1);
        if self.system_files_checked.load(Ordering::Relaxed) == checked {
            return true;
        }
        let same = EXCLUDE_NS.iter().all(|ns| {
            match (self.shared_model_file(ns), other.shared_model_file(ns)) {
                (Some(mine), Some(theirs)) => {
                    Arc::ptr_eq(mine, theirs) || mine.ast() == theirs.ast()
                }
                (None, None) => true,
                _ => false,
            }
        });
        if same {
            self.system_files_checked.store(checked, Ordering::Relaxed);
        }
        same
    }

    /// The shared handle of the model file registered under `namespace`.
    fn shared_model_file(&self, namespace: &str) -> Option<&Arc<ModelFile>> {
        let id = self.namespaces.get(namespace)?;
        self.files.get(id.slot()).map(|slot| &slot.model_file)
    }

    /// TS `new ModelManager({ addMetamodel: true })`: registers the cached
    /// metamodel file through `addModelFile`'s validating path, called after
    /// the other options, as TS adds it last. A taken namespace is the
    /// already-exists error; otherwise, with [`Self::metamodel_validation`],
    /// [`Self::validate_ast`] runs, then semantic validation.
    pub fn add_metamodel(&mut self) -> Result<()> {
        let model_file = crate::instance::metamodel::metamodel_model_file()?;
        if self.model_file(model_file.namespace()).is_none() {
            if self.options.metamodel_validation {
                self.validate_ast(&model_file)?;
            }
            self.validate_detached_model_file(&model_file)?;
        }
        self.add_shared_model_file(model_file)?;
        self.options.add_metamodel = true;
        Ok(())
    }

    /// The version of the manager's state, increased by every mutation,
    /// for the tests: for the same manager, outside a rolled-back batch, it
    /// never repeats an earlier value for a different state, and an adopted
    /// rebuild ([`ModelManager::adopt`]) continues the count. A rolled back
    /// batch restores the count with the state, so the next mutation after
    /// it can reuse a value an undone one had.
    #[cfg(test)]
    pub(crate) fn state_version(&self) -> u64 {
        self.state_version
    }

    js_compat_pub! {
        /// Replaces this manager with `next`, one built from it
        /// ([`ModelManager::update_model_file`],
        /// [`ModelManager::delete_model_file`]), as one mutation: `next`
        /// continues this manager's state version, so a check answered
        /// before is never taken as current after.
        pub fn adopt(&mut self, mut next: Self) {
            next.state_version = self.state_version.wrapping_add(1);
            next.system_files_checked = std::sync::atomic::AtomicU64::new(0);
            *self = next;
        }
    }

    /// A new manager with only the declarations `keep` accepts (by fqn and
    /// declaration), dropping emptied files and filtering imports alike. The
    /// decorator and root models are kept whole (BC-53), and so is the
    /// metamodel of a manager given [`ModelManager::add_metamodel`], which
    /// the new manager adds first, as TS's `new BaseModelManager({
    /// ...this.options })` does with `addMetamodel`. Same options; the
    /// files are validated together.
    ///
    /// TS: `BaseModelManager.filter(predicate)`.
    pub fn filter(&self, keep: impl Fn(&str, &Declaration) -> bool) -> Result<Self> {
        self.filter_declarations(keep, false)
    }

    js_compat_pub! {
        /// TS `ModelFile.filter(predicate, modelManager)` for the file `id`
        /// names here ([`ModelFile::filter_outcome`]), with `keep` handed
        /// each candidate's fully-qualified name, borrowed from the arena:
        /// the file's own declarations, then those its imports name in
        /// their source files here.
        #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
        pub fn filter_model_file(
            &self,
            id: ModelFileId,
            keep: impl Fn(&str) -> bool,
        ) -> Result<crate::introspect::model_file::FilterOutcome> {
            let file = self.file(id).ok_or_else(|| unknown(Node::ModelFile(id)))?;
            file.filter_outcome_at(
                |namespace, index, decl| match self.decl_id_in(namespace, index) {
                    Some(id) => keep(&self.declarations[id.slot()].fqn),
                    None => keep(&qualify(namespace, decl.name())),
                },
                self,
            )
        }
    }

    /// The handle of the declaration at `index` in the file registered
    /// under `namespace`, if there is one.
    fn decl_id_in(&self, namespace: &str, index: usize) -> Option<DeclId> {
        let slot = self.files.get(self.model_file_id(namespace)?.slot())?;
        self.decl_id_at(slot, index)
            .filter(|id| slot.declarations.contains(&id.index()))
    }

    /// [`ModelManager::filter_by_fqn`] over a predicate on the declaration too.
    fn filter_declarations(
        &self,
        keep: impl Fn(&str, &Declaration) -> bool,
        disable_validation: bool,
    ) -> Result<Self> {
        // `ModelFile::filter`'s predicate is called on the file's own
        // declarations and, for its imports, on other files' declarations,
        // so a predicate built from one namespace would ask `keep_fqn` about
        // the wrong name. The fully-qualified check therefore runs once, up
        // front, over every file, and the result is kept by `DeclId`, so a
        // declaration reached again through another file's imports is still
        // recognised.
        //
        // BC-53: a declaration of a file `result` already holds from
        // `Self::new()` (or `add_metamodel`) is kept without asking `keep`,
        // so the file it names stays whole in `result`.
        let mut result = self.empty_like()?;
        if self.options.add_metamodel {
            result.add_metamodel()?;
        }
        let keep = &keep;
        let result_ref = &result;
        let mut kept = vec![false; self.declarations.len()];
        for slot in &self.files {
            let mf = &slot.model_file;
            let held = mf.is_system_namespace() || result_ref.model_file(mf.namespace()).is_some();
            for (id, fqn, decl) in self.declarations_in(std::iter::once(slot)) {
                kept[id.slot()] = held || keep(fqn, decl);
            }
        }
        let is_kept = |namespace: &str, index: usize, _: &Declaration| {
            self.decl_id_in(namespace, index)
                .and_then(|id| kept.get(id.slot()).copied())
                .unwrap_or(false)
        };

        let mut filtered_files = Vec::new();
        for model_file in self.shared_model_files() {
            // BC-53: skip every file `result` already holds from
            // `Self::new()`, the decorator model as well as the root model.
            if model_file.is_system_namespace()
                || result.model_file(model_file.namespace()).is_some()
            {
                continue;
            }
            // A file kept exactly as it is is shared, not rebuilt, and is not
            // validated again where its `ValidityProof` holds.
            match model_file.filter_outcome_at(is_kept, self)? {
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

    /// A new manager with this one's options and the system models, holding
    /// `files` (shared, each with the [`ValidityProof`] it came with, if
    /// any), validated together unless `validate` is false. The shape
    /// `filter`'s result and the empty-input result of
    /// `crate::dcs::decorate_models` share.
    #[cfg(feature = "js-compat")]
    pub(crate) fn new_like_with(
        &self,
        files: Vec<(Arc<ModelFile>, Option<Arc<ValidityProof>>)>,
        validate: bool,
    ) -> Result<Self> {
        let mut result = self.empty_like()?;
        result.insert_models(files, validate)?;
        Ok(result)
    }

    /// A new manager with this one's options and only the system models
    /// (`Self::new()`), and every option, as TS's `new
    /// BaseModelManager({...this.options})` does.
    fn empty_like(&self) -> Result<Self> {
        let mut result = Self::new()?;
        result.options = self.options.clone();
        Ok(result)
    }

    /// This manager's own model files (those of `EXCLUDE_NS` left out, as
    /// `getModelFiles()` leaves them out), shared, each with its
    /// [`ValidityProof`] here: what another manager registers to hold the
    /// same models without copying or, where the proof holds there,
    /// validating them again ([`ModelManager::insert_models`]).
    #[cfg(feature = "js-compat")]
    pub(crate) fn user_files_with_proofs(
        &self,
    ) -> Vec<(Arc<ModelFile>, Option<Arc<ValidityProof>>)> {
        self.user_file_slots()
            .map(|slot| &slot.model_file)
            .map(|mf| (Arc::clone(mf), self.validity_proof(mf.namespace())))
            .collect()
    }

    js_compat_pub! {
        /// TS `updateModelFile(modelFile, fileName, disableValidation)`'s
        /// registration step, for an already-parsed `model_file`: a plain
        /// `Error` (`basemodelmanager-updatemodelfile-notfound`) when the
        /// namespace is not registered, else
        /// `ModelManager::with_model_file_registered`'s scratch copy. Never
        /// mutates `self`: the caller adopts the result.
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
        /// TS `deleteModelFile(namespace)`: a new manager with every
        /// registered model file but `namespace`'s, rebuilt from the
        /// survivors (shared), or a plain `Error`
        /// (`basemodelmanager-deletemodelfile-notfound`) when it holds none.
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
            for existing in self.shared_model_files() {
                if existing.namespace() != namespace {
                    scratch.insert_shared(Arc::clone(existing))?;
                }
            }
            Ok(scratch)
        }
    }

    /// The rollback core of [`ModelManager::add_models`] and
    /// [`ModelManager::filter`]: inserts every built file in order, then,
    /// unless `validate` is false, validates the whole manager once; either
    /// failure undoes every insert.
    pub(crate) fn insert_models(
        &mut self,
        files: Vec<(Arc<ModelFile>, Option<Arc<ValidityProof>>)>,
        validate: bool,
    ) -> Result<()> {
        self.register_batch(files.into_iter().map(Ok), validate)
            .map(drop)
    }

    /// The batch core of [`ModelManager::load_models`] and
    /// [`ModelManager::insert_models`]: registers each file (with its
    /// [`ValidityProof`]), stopping at the first failure, then validates once
    /// unless `validate` is false. Any failure undoes the whole batch
    /// ([`ModelManager::rollback`]). As the arena's only writer meanwhile, its
    /// files sit in a contiguous tail of each table.
    fn register_batch(
        &mut self,
        files: impl IntoIterator<Item = Result<(Arc<ModelFile>, Option<Arc<ValidityProof>>)>>,
        validate: bool,
    ) -> Result<Vec<ModelFileId>> {
        let mark = self.mark();
        let validated = self.validated_marks();
        let result = files
            .into_iter()
            .map(|file| {
                let (mf, proof) = file?;
                self.add_shared_model_file_with_proof(mf, proof)
            })
            .collect::<Result<Vec<_>>>()
            .and_then(|ids| {
                // Validate the whole manager, new models and pre-existing
                // ones together, only once every model in the batch loaded
                // cleanly.
                if validate {
                    self.validate_models()?;
                }
                Ok(ids)
            });
        if result.is_err() {
            self.rollback(mark);
            self.restore_validated(&validated);
        }
        result
    }
}

/// `@accordproject/concerto-metamodel@3.17.0`'s `resolveLocalNames` and its
/// helpers (lib/metamodelutil.js), for [`ModelManager::resolve_meta_model`],
/// over the plain AST [`Value`] [`ModelFile::ast`] stores, with the same
/// plain JS `Error`/`TypeError`s (`metamodelutil-*`).
mod metamodel_util;

#[cfg(test)]
mod tests;

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
//! `ModelManager::state_version` counts the mutations and never goes back to an
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

use std::collections::HashSet;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use rustc_hash::FxHashMap;

use serde_json::Value;

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
js_compat_pub! {
    #[allow(unused_imports)]
    pub use ts_compat::ModelFileSource;
}

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

/// The arena lengths and state version at one point, which
/// [`ModelManager::rollback`] returns the arena to: before
/// [`ModelManager::append_for_validation`] appended a file (P5-48), or
/// before a batch (A-6).
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
    state_version: u64,
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
    /// `state_version + 1` when [`ModelManager::has_system_files_of`] last
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
    /// arena's `Self::insert_shared` never validates on load (that is a separate,
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
            self.register_new(mf).map(drop)
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
                state_version: self.state_version,
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
        /// [`ModelManager::add_shared_model_file`], with the
        /// [`ValidityProof`] the source manager gave for the file, if any:
        /// [`ModelManager::validate_models`] then takes the file as valid
        /// without validating it when the proof holds here (P5-97).
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
        self.register_new(Arc::new(mf))
    }

    /// Registers `model_file` under a namespace no file holds yet: TS
    /// `addModelFile`'s already-exists check
    /// ([`ModelManager::check_namespace_available`]), then
    /// [`ModelManager::insert_shared`] (A-6, accordproject/concerto-rust#458).
    fn register_new(&mut self, model_file: Arc<ModelFile>) -> Result<ModelFileId> {
        self.check_namespace_available(model_file.namespace(), model_file.file_name())?;
        self.insert_shared(model_file)
    }

    /// The arena lengths and state version now, for
    /// [`ModelManager::rollback`] to return to (A-6).
    fn mark(&self) -> AppendMark {
        AppendMark {
            files: self.files.len(),
            declarations: self.declarations.len(),
            properties: self.properties.len(),
            state_version: self.state_version,
        }
    }

    /// Takes the files appended since `mark` out of the arena: truncates
    /// each table back to its length then, removes the namespaces those
    /// files were registered under, and drops the caches, which may hold
    /// their handles. Safe without a tombstone (module doc): the appended
    /// files are the arena's tail, so no handle from before `mark` is
    /// touched. The state version is the caller's to set.
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
        let mark = self.mark();
        let id = self.insert_shared(Arc::clone(model_file)).ok()?;
        Some((id, mark))
    }

    /// Takes out the file [`ModelManager::append_for_validation`] appended,
    /// leaving the arena, the namespaces and the state version as they were
    /// before it (as [`ModelManager::load_models`] rolls a batch back). The
    /// caches are dropped: they may hold the appended file's handles.
    pub(crate) fn undo_append(&mut self, mark: AppendMark) {
        self.rollback(mark);
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
        self.register_batch(
            models.into_iter().map(|(value, file_name)| {
                ModelFile::from_json(value, file_name).map(|mf| (Arc::new(mf), None))
            }),
            true,
        )
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
    /// state version as a copy built file by file.
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
    /// it is shared, not copied (P5-18).
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
        self.state_version += 1;
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
            let mark = self.mark();
            if !already_has_metamodel {
                self.insert_shared(metamodel_model_file()?)?;
            }
            deserialize_ast(self, ast)?;
            if !already_has_metamodel {
                // `deleteModelFile(MetaModelNamespace)`: the metamodel is the
                // arena's tail (nothing else was added since step 2), so
                // removing it is truncating each table back, as `add_models`'s
                // rollback does; the removal is a mutation of its own.
                self.truncate_to(&mark);
                self.state_version += 1;
            }
            Ok(())
        }
    }

    /// Whether `ast` passes the structural check on the resident metamodel
    /// manager ([`ModelManager::validate_ast_value`]), when this manager's
    /// system model files match that manager's. `false` also when the
    /// resident manager cannot be built, so the caller's own path reports
    /// that error. The resident manager is the crate's one
    /// (`instance::metamodel::with_resident_metamodel_manager`, P5-102:
    /// before, this kept a second one of its own).
    fn passes_on_resident_metamodel(&self, ast: &Value) -> bool {
        use crate::instance::metamodel::{deserialize_ast, with_resident_metamodel_manager};
        with_resident_metamodel_manager(|resident| {
            Ok(self.has_system_files_of(resident) && deserialize_ast(resident, ast).is_ok())
        })
        .unwrap_or(false)
    }

    /// Whether this manager holds the same decorator and root model files
    /// as `other`: the same shared file (P5-102, A-12: the usual case, as
    /// both managers hold `system_model_files`' own), or else the same AST,
    /// which is all a model file's lookups are built from. Answered once
    /// per [`ModelManager::state_version`].
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
        /// The version of the manager's state, which every mutation of the
        /// manager increases (P5-100, F-3: named `generation` before). A
        /// snapshot of an element taken at one version is current while the
        /// version is unchanged. It never repeats an earlier value for a different state:
        /// a manager rebuilt from this one and adopted in its place
        /// ([`ModelManager::adopt`]: an update, a removal, external models)
        /// continues the count (A-3, accordproject/concerto-rust#448), and a
        /// failed batch that is rolled back restores the count it started from
        /// together with the very state it had then.
        pub fn state_version(&self) -> u64 {
            self.state_version
        }
    }

    js_compat_pub! {
        /// Replaces this manager with `next`, one built from it
        /// ([`ModelManager::update_model_file`],
        /// [`ModelManager::delete_model_file`]), as one mutation: `next`
        /// continues this manager's [`ModelManager::state_version`] (A-3,
        /// accordproject/concerto-rust#448), so a snapshot taken before is
        /// never taken as current after. A rebuilt manager's own count
        /// restarts with its arena, and adopting it as it is could repeat an
        /// earlier state version.
        pub fn adopt(&mut self, mut next: Self) {
            next.state_version = self.state_version.wrapping_add(1);
            next.system_files_checked = std::sync::atomic::AtomicU64::new(0);
            *self = next;
        }
    }

    /// A new manager with only the declarations `keep` accepts, given each
    /// declaration's fully-qualified name and the declaration itself. A model
    /// file left with no declaration is dropped, and each file's imports are
    /// filtered the same way. The built-in decorator and root models are kept
    /// whole (BC-53). The new manager has this one's options, and its files
    /// are validated together.
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
        //
        // BC-53: every declaration of a file `result` already holds from
        // `Self::new()` counts as kept without asking `keep`, so an import
        // of one (a user type extending `Decorator`) is never pruned while
        // the file it names stays whole in `result`.
        let mut result = self.empty_like()?;
        let keep = &keep;
        let result_ref = &result;
        let kept: rustc_hash::FxHashSet<*const Declaration> = self
            .files
            .iter()
            .flat_map(|slot| {
                let mf = &slot.model_file;
                let held =
                    mf.is_system_namespace() || result_ref.model_file(mf.namespace()).is_some();
                self.declarations_in(std::iter::once(slot))
                    .filter_map(move |(_, fqn, decl)| {
                        (held || keep(fqn, decl)).then_some(decl as *const Declaration)
                    })
            })
            .collect();

        let mut filtered_files = Vec::new();
        for model_file in self.shared_model_files() {
            // BC-53: skip every file `result` already holds from
            // `Self::new()`, the decorator model as well as the root model.
            if model_file.is_system_namespace()
                || result.model_file(model_file.namespace()).is_some()
            {
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

    /// P5-102 (accordproject/concerto-rust#456, C-2): a new manager with
    /// this one's options and the system models, holding `files` (shared,
    /// each with the [`ValidityProof`] it came with, if any), validated
    /// together unless `validate` is false. The shape `filter`'s result and
    /// the empty-input result of `crate::dcs::decorate_models` share.
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
    /// (`Self::new()`). A-5: every option, `metamodel_validation` included,
    /// as TS's `new BaseModelManager({...this.options})` does.
    fn empty_like(&self) -> Result<Self> {
        let mut result = Self::new()?;
        result.options = self.options.clone();
        Ok(result)
    }

    /// P5-102 (C-2): this manager's own model files (those of `EXCLUDE_NS`
    /// left out, as `getModelFiles()` leaves them out), shared, each with its [`ValidityProof`] here: what another
    /// manager registers to hold the same models without copying or, where
    /// the proof holds there, validating them again
    /// ([`ModelManager::insert_models`]).
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

    /// The rollback core of [`ModelManager::add_models`] and
    /// [`ModelManager::filter`]: inserts every already-built `files` in
    /// order, then, unless `validate` is false, runs
    /// [`ModelManager::validate_models`] once over the whole manager; either
    /// failure undoes every insert this call made, exactly as `add_models`
    /// does for its own AST-building version of the same loop.
    pub(crate) fn insert_models(
        &mut self,
        files: Vec<(Arc<ModelFile>, Option<Arc<ValidityProof>>)>,
        validate: bool,
    ) -> Result<()> {
        self.register_batch(files.into_iter().map(Ok), validate)
            .map(drop)
    }

    /// The batch core of [`ModelManager::load_models`] and
    /// [`ModelManager::insert_models`] (A-6): registers each file in turn
    /// (with its [`ValidityProof`], if any), stopping at the first that
    /// fails to build or whose namespace is taken, then, unless `validate`
    /// is false, runs [`ModelManager::validate_models`] once over the whole
    /// manager. Any failure undoes the whole batch
    /// ([`ModelManager::rollback`], and the validated marks are restored),
    /// exactly as if it had never been called. Because the arena is
    /// append-only and this call is the only writer while it runs,
    /// everything it adds sits in a contiguous tail of each table.
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

/// A port of `@accordproject/concerto-metamodel@3.17.0`'s
/// `lib/metamodelutil.js` `resolveLocalNames` and its private helpers — the
/// only part of that package [`ModelManager::resolve_meta_model`] needs.
/// Every function here is a line-for-line port: it walks the same plain
/// metamodel-AST [`Value`] shape [`ModelFile::ast`] already stores (no typed
/// `mm::*` struct, the same divergence `crate::dcs` documents for the same
/// AST), and raises the same plain JS `Error`/`TypeError` TS does, through
/// the catalogue (`metamodelutil-*`, `engine-typeerror-readproperties`).
mod metamodel_util;

#[cfg(test)]
mod tests;

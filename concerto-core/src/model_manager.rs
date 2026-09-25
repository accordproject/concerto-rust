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
//! [`ModelManager::add_models`] (P1-06) undoes a failed batch by truncating
//! each vector back to its length before the call: safe without a tombstone,
//! because a batch only ever appends and rolls back its own tail, so no
//! handle from before the call is touched. Any future removal that is not a
//! batch's own rollback (`deleteModelFile`, `clearModelFiles`) is a different
//! shape — it must leave a tombstone rather than shift the arena, so that the
//! handles of the elements that stay remain valid.
//!
//! A ported member reaches its collaborators through the
//! [`ResolutionContext`] trait. The manager implements it over the arena, with
//! [`Node`] as its handle; `concerto-wasm` implements it over JS objects, for
//! views a white-box test builds over stubbed collaborators (PORTING.md 1.4).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use serde_json::Value;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::declaration::{ClassDeclaration, Declaration, EnumDeclaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::introspect::{FullyQualified, Named, Typed};
use crate::model_util::{
    self, PRIMITIVE_TYPES, get_fully_qualified_name, get_namespace, get_short_name,
    is_primitive_type,
};
use crate::rootmodel::{decorator_model_ast, root_model_ast};

/// The namespaces TS `BaseModelManager.getModelFiles()` leaves out unless it
/// is asked to include them: the system model, its unversioned name, and the
/// decorator model. The match is on the exact namespace string.
///
/// TS: `EXCLUDE_NS` (src/basemodelmanager.ts).
const EXCLUDE_NS: [&str; 3] = ["concerto@1.0.0", "concerto", "concerto.decorator@1.0.0"];

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

    /// TS: `field.getName()`. `StringValidator` and `CollectionSizeValidator`
    /// (unlike `NumberValidator`) pass this as the identifier of every error
    /// their constructor reports, and `StringValidator` passes it again as
    /// the identifier for the `defaultValue` check it runs at load time
    /// (P2-02).
    fn name(&self) -> std::result::Result<String, Self::Error>;
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
    /// A handle to a property of a class declaration, or a value of an enum
    /// declaration (P2-04), loaded into a [`ModelManager`]. Both are
    /// [`Property`] values, addressed the same way, through the unified
    /// [`ClassLike::own_properties`].
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
    dangerously_allow_reserved_system_type_names_in_user_models: bool,
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
fn not_a_class_like(fqn: &str) -> ConcertoError {
    ConcertoError::IllegalModel {
        message: format!("{fqn} is not a concept-like or enum declaration"),
        file_name: None,
        location: None,
    }
}

impl ModelManager {
    /// A fresh manager with both system models already loaded: the decorator
    /// model, then the root model.
    ///
    /// TS: `BaseModelManager`'s constructor calls `this.addDecoratorModel()`
    /// then `this.addRootModel()` (src/basemodelmanager.ts), each of which
    /// builds a `ModelFile` from the vendored AST and adds it with
    /// `addModelFile(m, cto, fileName, true)` - validation disabled. The
    /// arena's [`Self::insert`] never validates on load (that is a separate,
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
        let decorator = ModelFile::from_json(
            &decorator_model_ast(),
            Some("concerto_decorator_1.0.0.cto".into()),
        )?;
        mgr.insert(decorator)?;
        let root = ModelFile::from_json(&root_model_ast(), Some("concerto_1.0.0.cto".into()))?;
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
    pub fn add_models<'a>(
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

        let mut result = Ok(Vec::new());
        for (value, file_name) in models {
            let outcome = ModelFile::from_json(value, file_name).and_then(|mf| {
                if self.namespaces.contains_key(mf.namespace()) {
                    return Err(ConcertoError::IllegalModel {
                        message: format!("duplicate namespace: {}", mf.namespace()),
                        file_name: mf.file_name().map(str::to_string),
                        location: None,
                    });
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
            self.namespaces = namespaces_snapshot;
            self.generation = generation;
            return Err(err);
        }
        result
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
    /// TS: `BaseModelManager.getDecoratorValidation`.
    pub fn decorator_validation(
        &self,
    ) -> &crate::introspect::decorator::DecoratorValidationOptions {
        &self.decorator_validation
    }

    /// Sets the decorator validation options, matching the TS constructor's
    /// `options.decoratorValidation` (there is no separate TS setter; the
    /// port exposes one so a manager already built can still opt in, as this
    /// crate's own tests do).
    pub fn set_decorator_validation(
        &mut self,
        options: crate::introspect::decorator::DecoratorValidationOptions,
    ) {
        self.decorator_validation = options;
    }

    /// TS: `modelFile.getModelManager()?.options?.dangerouslyAllowReservedSystemTypeNamesInUserModels`,
    /// coerced with `Boolean(...)` (`Declaration.validate`, declaration.ts).
    pub fn dangerously_allow_reserved_system_type_names_in_user_models(&self) -> bool {
        self.dangerously_allow_reserved_system_type_names_in_user_models
    }

    /// Sets the escape hatch above, matching the TS constructor's
    /// `options.dangerouslyAllowReservedSystemTypeNamesInUserModels` (there is
    /// no separate TS setter; the port exposes one the same way
    /// [`Self::set_decorator_validation`] does).
    pub fn set_dangerously_allow_reserved_system_type_names_in_user_models(&mut self, allow: bool) {
        self.dangerously_allow_reserved_system_type_names_in_user_models = allow;
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The loaded model file for a namespace, if there is one.
    pub fn model_file(&self, namespace: &str) -> Option<&ModelFile> {
        self.model_file_id(namespace).and_then(|id| self.file(id))
    }

    /// Every loaded model file, including the built-in decorator and root
    /// models, in the order they were loaded.
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

    /// The property a handle names. Resolves through [`ClassLike`] so that
    /// an enum's own values (P2-04), addressed the same as a class
    /// declaration's fields (`insert`'s doc comment), resolve here too.
    pub fn property(&self, id: PropId) -> Option<&Property> {
        let slot = self.properties.get(id.slot())?;
        ClassLike::from_declaration(self.declaration(slot.declaration)?)?
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

    /// Every concept-like declaration across every loaded model file (the
    /// decorator and root models included, as TS's own `getModelFiles()`
    /// does), in file order and then declaration order — a map or scalar
    /// declaration is left out.
    ///
    /// TS: `Introspector.getClassDeclarations` (src/introspect/introspector.ts):
    /// `modelFile.getAllDeclarations().filter(d =>
    /// !d.isMapDeclaration?.() && !d.isScalarDeclaration?.())`, concatenated
    /// over every loaded model file.
    pub fn class_declarations(&self) -> impl Iterator<Item = DeclId> + '_ {
        self.files
            .iter()
            .flat_map(|file| file.declarations.clone().map(DeclId))
            .filter(move |id| {
                self.declaration(*id)
                    .is_some_and(|d| !d.is_map_declaration() && !d.is_scalar_declaration())
            })
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
    pub fn identifier_field_name(&self, fqn: &str) -> Result<Option<String>> {
        Ok(self
            .super_chain(fqn)?
            .into_iter()
            .find_map(|(_, class)| class.own_identifier_field_name())
            .map(str::to_string))
    }

    /// [`ModelManager::identifier_field_name`], as a boolean.
    ///
    /// TS: `ClassDeclaration.isIdentified` (src/introspect/classdeclaration.ts):
    /// `!!this.getIdentifierFieldName()`, inherited unchanged by `EnumDeclaration`.
    pub fn is_identified(&self, fqn: &str) -> Result<bool> {
        Ok(self.identifier_field_name(fqn)?.is_some())
    }

    /// [`ModelManager::identifier_field_name`], `true` only for the system
    /// `$identifier`.
    ///
    /// TS: `ClassDeclaration.isSystemIdentified`: `this.getIdentifierFieldName()
    /// === '$identifier'`, inherited unchanged by `EnumDeclaration`.
    pub fn is_system_identified(&self, fqn: &str) -> Result<bool> {
        Ok(self.identifier_field_name(fqn)?.as_deref() == Some("$identifier"))
    }

    /// Every property of a type, gathered by walking from the type up through
    /// all of its super types, each alongside the fully-qualified name of the
    /// declaration that actually declares it (TS: `Property.getParent()
    /// .getFullyQualifiedName()`) — an inherited property's is its super
    /// type's, not `fqn`'s own. Returns an error if the name is not a
    /// concept-like or enum type, a super type cannot be resolved, or the
    /// inheritance chain is circular.
    ///
    /// TS: `ClassDeclaration.getProperties` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    pub fn get_all_properties(&self, fqn: &str) -> Result<Vec<(String, Property)>> {
        Ok(self
            .super_chain(fqn)?
            .into_iter()
            .flat_map(|(owner_fqn, class)| {
                class
                    .own_properties()
                    .iter()
                    .cloned()
                    .map(move |p| (owner_fqn.clone(), p))
                    .collect::<Vec<_>>()
            })
            .collect())
    }

    /// The property with a given name, own or inherited, or `None` if it does
    /// not exist, alongside its declaring type's fully-qualified name (see
    /// [`ModelManager::get_all_properties`]).
    ///
    /// TS: `ClassDeclaration.getProperty`, inherited unchanged by `EnumDeclaration`.
    pub fn get_property(&self, fqn: &str, name: &str) -> Result<Option<(String, Property)>> {
        Ok(self
            .get_all_properties(fqn)?
            .into_iter()
            .find(|(_, p)| p.name() == name))
    }

    /// The properties declared directly on `fqn`, not those it inherits.
    ///
    /// TS: `ClassDeclaration.getOwnProperties`, inherited unchanged by `EnumDeclaration`.
    pub fn get_own_properties(&self, fqn: &str) -> Result<Vec<Property>> {
        let class = ClassLike::from_declaration(self.get_declaration(fqn)?)
            .ok_or_else(|| not_a_class_like(fqn))?;
        Ok(class.own_properties().to_vec())
    }

    /// A nested property, following a dotted path (`a.b.c`) through the
    /// declared types of each element but the last.
    ///
    /// TS: `ClassDeclaration.getNestedProperty` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    pub fn get_nested_property(
        &self,
        fqn: &str,
        property_path: &str,
    ) -> Result<(String, Property)> {
        let names: Vec<&str> = property_path.split('.').collect();
        let mut search_root = fqn.to_string();
        let mut result = None;
        for (n, name) in names.iter().enumerate() {
            let Some((declaring_fqn, property)) = self.get_property(&search_root, name)? else {
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
                        ErrorKind::Error,
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
            .find(|id| self.property(*id).is_some_and(|p| p.name() == name)))
    }

    /// The FQN of `fqn`'s direct super type, or `None` when it has none (only
    /// the system model's own `Concept`).
    ///
    /// TS: `ClassDeclaration.getSuperType` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    pub fn get_super_type(&self, fqn: &str) -> Result<Option<String>> {
        let class = ClassLike::from_declaration(self.get_declaration(fqn)?)
            .ok_or_else(|| not_a_class_like(fqn))?;
        self.super_type_fqn(&class, namespace_of(fqn))
    }

    /// The [`DeclId`] of `fqn`'s direct super type, or `None` when it has
    /// none.
    ///
    /// TS: `ClassDeclaration.getSuperTypeDeclaration`, inherited unchanged by
    /// `EnumDeclaration`.
    pub fn get_super_type_declaration(&self, fqn: &str) -> Result<Option<DeclId>> {
        let Some(super_fqn) = self.get_super_type(fqn)? else {
            return Ok(None);
        };
        Ok(self.declaration_id(&super_fqn))
    }

    /// Every super type of `fqn`, from its direct super type up to the root,
    /// as fully-qualified names.
    ///
    /// TS: `ClassDeclaration.getAllSuperTypeDeclarations`, inherited unchanged
    /// by `EnumDeclaration`.
    pub fn get_all_super_type_names(&self, fqn: &str) -> Result<Vec<String>> {
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
    pub fn get_assignable_class_declarations(&self, fqn: &str) -> Result<Vec<String>> {
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
    pub fn get_direct_subclasses(&self, fqn: &str) -> Result<Vec<String>> {
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
    fn super_chain(&self, fqn: &str) -> Result<Vec<(String, ClassLike<'_>)>> {
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

            let class = ClassLike::from_declaration(self.get_declaration(&current)?)
                .ok_or_else(|| not_a_class_like(&current))?;

            let next = self.super_type_fqn(&class, namespace_of(&current))?;
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
        let location = class.location().and_then(crate::error::location_value);
        match self.resolve_type_name(in_namespace, &ti.name, location.clone()) {
            Ok(fqn) => Ok(Some(fqn)),
            // TS: `_resolveSuperType`'s own hardcoded `IllegalModelException`
            // (src/introspect/classdeclaration.ts) — `resolve_type_name`'s
            // own failure is `TypeNotFound` (`ModelManager.getType`'s shape,
            // a different TS throw site), so it is remapped here, the same
            // way `validation.rs`'s `check_super_type` already raises this
            // exact message (`failed`) for the same TS call.
            Err(ConcertoError::TypeNotFound { .. }) => Err(ContractError::pre_port(
                ErrorKind::IllegalModel,
                format!("Could not find super type {}", ti.name),
                location,
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
/// yet at parity with TS, so are the answers: super types resolve as the
/// loader resolves them (P2-08). The implicit `Concept` super type (P2-03) is
/// in every class-like or enum declaration's `super_chain` (`ClassLike`), so
/// it is in [`ResolutionContext::get_all_super_type_declarations`] too, for
/// both [`Declaration::Class`] and [`Declaration::Enum`] — TS's
/// `EnumDeclaration extends ClassDeclaration` gives an enum the same implicit
/// `Concept` super type (P2-03).
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
            Declaration::Class(_) | Declaration::Enum(_) => self
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
        // TS: Property.getFullyQualifiedTypeName (src/introspect/property.ts:218)
        // throws a plain `Error` (`ErrorKind::Error`, not
        // `IllegalModelException`) with its own inline template
        // (`property-getfullyqualifiedtypename-notfound`) when
        // `ModelFile.getFullyQualifiedTypeName` returns `null` — which it
        // does, rather than throwing, so this is the one throw site for
        // both. `this.type` is JS `null` for an enum value (P2-04) and
        // renders as the literal string `null`, matching `+ this.type`'s
        // own string coercion.
        resolved.ok_or_else(|| {
            let field = self.property(id).expect("checked above");
            ContractError::new(
                ErrorKind::Error,
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
        mgr.add_model(
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
    fn assignable_class_declarations_of_a_fresh_manager_leave_out_the_system_models() {
        let mgr = ModelManager::new().unwrap();
        assert_eq!(
            mgr.get_assignable_class_declarations("concerto@1.0.0.Concept")
                .unwrap(),
            ["concerto@1.0.0.Concept"]
        );
    }

    #[test]
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
    fn a_user_asset_is_the_only_direct_subclass_of_asset() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
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
    fn an_asset_with_no_extends_implicitly_extends_asset_itself() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
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
            .get_all_properties("org.acme.defaults@1.0.0.DefaultAsset")
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
        mgr.add_model(
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

    /// TS: `EnumDeclaration extends ClassDeclaration` inherits
    /// `getProperties` unchanged, so an enum's values come back the same way
    /// a class's fields do (P2-03 closes the implicit-`Concept` gap this
    /// relies on; `Concept` itself has no properties, so an enum's `Color`
    /// has none to inherit).
    #[test]
    fn get_all_properties_on_enum_gives_its_values() {
        let mgr = manager();
        let properties = mgr.get_all_properties("org.example@1.0.0.Color").unwrap();
        let names: Vec<&str> = properties.iter().map(|(_, p)| p.name()).collect();
        assert_eq!(names, ["RED"]);
        assert_eq!(properties[0].0, "org.example@1.0.0.Color");
        assert!(properties[0].1.is_enum_value());
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
        assert_eq!(mgr.property(props[0]).unwrap().name(), "salary");
        assert_eq!(mgr.parent_of(props[0]), Some(employee));

        // An enum's own values get `PropId`s too (P2-04), addressed the same
        // way a class declaration's fields are.
        let color = mgr.declaration_id("org.example@1.0.0.Color").unwrap();
        let color_props: Vec<PropId> = mgr.property_ids(color).collect();
        assert_eq!(color_props.len(), 1);
        assert_eq!(mgr.property(color_props[0]).unwrap().name(), "RED");
        assert!(mgr.property(color_props[0]).unwrap().is_enum_value());
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
        // The built-in system and decorator models load first, so their own
        // class-like declarations (e.g. `Concept`) are included too — TS's
        // `Introspector.getClassDeclarations` iterates every loaded model
        // file, system ones included.
        assert!(names.contains(&"Concept"));
    }

    /// A map or scalar declaration is left out of `class_declarations`, the
    /// same way `Introspector.getClassDeclarations` leaves them out of TS's
    /// `instanceof ClassDeclaration` filter.
    #[test]
    fn class_declarations_exclude_maps_and_scalars() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
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
        mgr.add_model(
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
        mgr.add_model(
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
            .find(|id| mgr.property(*id).unwrap().name() == "kid")
            .unwrap();
        assert_eq!(mgr.property(kid).unwrap().type_name(), Some("Kid"));
    }

    /// TS: `property.getFullyQualifiedTypeName().should.equal('child@1.0.0.Child')`
    /// — resolved through the import alias to the type it actually names.
    #[test]
    fn an_aliased_import_s_property_resolves_its_fully_qualified_type_name() {
        let mgr = aliasing_manager();
        let child = mgr.declaration_id("parent@1.0.0.Child").unwrap();
        let kid = mgr
            .property_ids(child)
            .find(|id| mgr.property(*id).unwrap().name() == "kid")
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
            .find(|id| mgr.property(*id).unwrap().name() == "kid")
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
        let ids = mgr.add_models([(&dependent, None), (&base, None)]).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(mgr.validate_models().is_ok());
        assert!(
            mgr.is_assignable_to("org.dependent@1.0.0.Sub", "org.base@1.0.0.Base")
                .unwrap()
        );

        // The reverse order validates just as cleanly.
        let mut mgr2 = ModelManager::new().unwrap();
        mgr2.add_models([(&base, None), (&dependent, None)])
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
        let err = mgr.add_models([(&dependent, None)]).unwrap_err();
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

        assert!(mgr.add_models([(&base, None), (&base, None)]).is_err());

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
        assert!(mgr.add_models([(&other, None), (&clash, None)]).is_err());

        assert_eq!(mgr.generation(), generation);
        assert_eq!(mgr.model_files().count(), count_before);
        assert_eq!(mgr.model_file_id("org.base@1.0.0"), None);
    }

    #[test]
    fn add_models_leaves_pre_existing_models_validating_on_success() {
        let mut mgr = manager();
        let base = base_model();
        let dependent = dependent_model();
        mgr.add_models([(&dependent, None), (&base, None)]).unwrap();
        // The pre-existing models (from `manager()`) are still there and
        // still validate, alongside the two the batch added.
        assert!(mgr.get_declaration("org.example@1.0.0.Manager").is_ok());
        assert!(mgr.validate_models().is_ok());
    }
}

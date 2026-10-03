//! Semantic validation of loaded models.
//!
//! Loading a model checks that it is structurally well formed: the JSON parses
//! into the metamodel, each node carries what its `$class` requires, and a
//! declaration's own validators make sense. This module runs the checks that
//! are left once the models are loaded, mostly those that need more than one
//! declaration in view: resolving a super type, ensuring a relationship points
//! at an identifiable type, and catching a field that is declared twice along
//! an inheritance chain. These are the checks the Concerto specification calls
//! semantic validation, and they run over an already loaded [`ModelManager`].
//!
//! Validation stops at the first problem. TS raises every one of these as
//! `IllegalModelException` (`ClassDeclaration.validate` and its callees;
//! PORTING.md section 2.3), so every error here carries
//! a contract error with `ErrorKind::IllegalModel`, from the error
//! catalogue (`Error::new`). A model whose inheritance is circular surfaces
//! the `RangeError` TS's recursion overflows with (`ErrorKind::RecursionLimit`,
//! PORTING.md section 2.5, DV-013), raised by the model manager's
//! super-type walk. A model that validates cleanly returns `Ok(())`.

use rustc_hash::FxHashSet;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::declaration::{ClassDeclaration, Declaration, MapDeclaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::introspect::{DeclarationKind, Decorated, Typed, Validate};
use crate::model_manager::ModelManager;
use crate::model_util::{self, get_namespace, is_primitive_type, qualify, short_name};

/// A class's own AST `location`, for an error's `location` ([`Error::at`])
/// (PORTING.md 2.1). `ClassDeclaration` keeps its `location` as a typed
/// `mm::Range`, so this re-serialises it through
/// [`crate::error::location_value`]; it is not a verbatim copy of the AST's
/// JSON the way `ScalarDeclaration::process` reads `ast.location`.
fn class_location(class: &ClassDeclaration) -> Option<serde_json::Value> {
    class.location().and_then(crate::error::location_value)
}

/// A property's own AST `location` (TS: `this.ast.location` inside
/// `Property.validate`/`Decorated.validate`, property.ts/decorated.ts —
/// `this` there is the property, not its owning class). P2-08 review
/// carry-over (c) from P2-04's review (#48): earlier code used the owning
/// class's location for these, before `Property` carried its own.
fn property_location(property: &Property) -> Option<serde_json::Value> {
    property.location().and_then(crate::error::location_value)
}

/// A typed AST `location`, re-serialised for an error only once that error
/// is raised (P5-48, accordproject/concerto-rust#369): the checks below take
/// the typed `mm::Range` and build its JSON value on their error path alone,
/// not on every call, which is what a check that passes (the common case)
/// used to pay for.
fn lazy_location(range: Option<&mm::Range>) -> Option<serde_json::Value> {
    range.and_then(crate::error::location_value)
}

impl ModelManager {
    /// Validates every loaded model except the root model (`concerto@1.0.0`),
    /// so user models and the built-in decorator model are checked. Returns
    /// `Ok(())` if every model is semantically valid, otherwise the first
    /// problem found.
    ///
    /// TS: `validateModelFiles` (basemodelmanager.ts) — `for (ns in
    /// this.modelFiles)`, the order the files were added in. A subclass's
    /// pass also checks the properties it inherits (`validate_property`), so
    /// when two files are both invalid the order decides which error comes
    /// first; P2-08 review: this used to sort by namespace instead.
    pub fn validate_models(&self) -> Result<()> {
        self.validate_models_naming_file().map_err(|(_, err)| err)
    }

    js_compat_pub! {
        /// [`ModelManager::validate_models`], with the namespace of the model
        /// file the first problem was found in (P5-11,
        /// accordproject/concerto-rust#287): TS `validateModelFiles` throws
        /// that file's own `validate()` error, which names the file, so a
        /// binding needs to know which one failed.
        pub fn validate_models_naming_file(&self) -> std::result::Result<(), (String, Error)> {
            // P5-97 (accordproject/concerto-rust#448): a file already known
            // to be valid here (validated in this manager, or shared with a
            // proof that holds) is not validated again: it would pass, so
            // the first error found is the same.
            let model_files = self
                .model_files()
                .enumerate()
                .filter(|(_, model_file)| !model_file.is_system_namespace());

            for (index, model_file) in model_files {
                let id = crate::model_manager::ModelFileId::from_index(
                    u32::try_from(index).unwrap_or(u32::MAX),
                );
                if self.known_valid(id) {
                    continue;
                }
                self.validate_model_file(model_file)
                    .map_err(|err| (model_file.namespace().to_string(), err))?;
                self.mark_validated(id);
            }
            Ok(())
        }
    }

    /// TS `ModelFile.validate()` (modelfile.ts), checked against `self` as
    /// the file's owning model manager — the same `this.getModelManager()`
    /// import resolution reaches. [`ModelManager::validate_models`] runs
    /// this over every loaded file; the oracle's `ModelFile.validate` op
    /// runs it directly on one (P2-08).
    ///
    /// **`model_file` must be the file `self` itself has registered under
    /// its namespace**, the same object [`ModelManager::model_file`] would
    /// return: `check_imports` and, through [`ModelManager::resolve_type_name`],
    /// every super-type and property-type lookup this pass runs, all resolve
    /// a namespace *through `self`* (`self.model_file(namespace)`), not
    /// through `model_file` directly. TS's `this.getModelManager()` always
    /// finds `this` this way, because `this` is a live reference the caller
    /// already holds; a `model_file` this port reconstructs from an AST
    /// rather than fetches from `self` is a different value with the same
    /// content, and `self` has no way to recognise it as "the same file" for
    /// these lookups if it is not registered — every check that needs to
    /// resolve *this file's own* namespace back to itself then wrongly
    /// reports it as undeclared (a caller that cannot guarantee registration
    /// must check first, as `ops.rs`'s `registered_file` does for the oracle
    /// dispatch).
    ///
    /// In order: (1) `super.validate()` — the file's own decorators
    /// (`Decorated.validate`); (2) the `getImports()` loop; (3) the
    /// duplicate-class-name scan (`check_unique_declaration_names`):
    /// `ModelFile::from_json` accepts a second declaration of one name, as
    /// TS's constructor does, so this is where it is rejected; (4) each
    /// declaration, in file order — including, first thing, the
    /// import-clash check every declaration kind reaches through its own
    /// `super.validate()` chain (`check_import_clash`'s doc comment).
    ///
    /// Every error but step (3)'s names `model_file` as TS's does
    /// (`attach_model_file`); step (3)'s `IllegalModelException` is
    /// constructed with no model file at all in TS, so it has no `File
    /// '<name>'` suffix.
    pub fn validate_model_file(&self, model_file: &ModelFile) -> Result<()> {
        self.validate_model_file_with_import_scope(model_file, self, None)
    }

    /// [`ModelManager::validate_model_file`], checking `model_file`'s own
    /// `getImports()` loop against `import_scope` rather than against
    /// `self`. The two differ only for
    /// [`ModelManager::validate_detached_model_file`]'s scratch branch
    /// (P2-08d, accordproject/concerto-rust#151): every check here but `check_imports` needs
    /// `model_file` registered in `self` to resolve its own local types
    /// (TS bypasses the manager for those, `this.getModelFile().getType`,
    /// so a scratch registration is a Rust-only accommodation, that
    /// function's doc comment); `check_imports`'s `this.getModelManager()
    /// .getModelFile(importNamespace)` is the one lookup in
    /// `ModelFile.validate()` TS actually routes through the manager, and
    /// it must see the manager exactly as it stood when TS calls
    /// `.validate()` — which, for every caller of `validate_detached_model_file`,
    /// never yet holds `model_file`'s own namespace.
    ///
    /// `hidden` names a namespace `import_scope` is taken not to hold
    /// (P5-48: [`ModelManager::validate_and_add_model_file`] validates a file
    /// it has already registered, and its own namespace must still look
    /// unregistered to `check_imports`).
    fn validate_model_file_with_import_scope(
        &self,
        model_file: &ModelFile,
        import_scope: &ModelManager,
        hidden: Option<&str>,
    ) -> Result<()> {
        let attach = |e| attach_model_file(e, model_file);
        validate_decorators(self, model_file.namespace(), model_file, None).map_err(attach)?;
        check_unique_decorators(model_file, None).map_err(attach)?;
        check_imports(import_scope, hidden, model_file).map_err(attach)?;
        check_unique_declaration_names(model_file)?;
        for declaration in model_file.declarations() {
            declaration
                .validate(self, model_file.namespace())
                .map_err(attach)?;
        }
        Ok(())
    }

    js_compat_pub! {
        /// TS `modelFile.validate()` for a `ModelFile` whose `getModelManager()`
        /// is `self` but which `self` may never have registered — `new
        /// ModelFile(modelManager, ast)` followed directly by `validate()`, or
        /// `addModelFile`'s validate-before-register. When `self` already holds
        /// exactly this file (same AST, same file name) under its namespace,
        /// this is [`ModelManager::validate_model_file`] on that file. Otherwise
        /// it validates against a scratch copy of `self` with `model_file`
        /// registered in place of whatever `self` holds under its namespace
        /// (`ModelManager::with_model_file_registered`), so that the file's own
        /// local types resolve to itself, as TS's `this.getLocalType` does,
        /// while every import still resolves through the same files `self`
        /// holds. `self` itself is never changed (P2-08).
        ///
        /// The scratch branch's `check_imports` step is the one exception
        /// (P2-08d, accordproject/concerto-rust#151): it runs against `self`, not the scratch, because
        /// `model_file` is never genuinely registered under its own namespace
        /// at the point TS calls `.validate()` here — a self-import (an
        /// `import` statement naming `model_file`'s own namespace) must fail
        /// "namespace is not defined" the same way any other not-yet-loaded
        /// namespace does, not resolve to `model_file` itself.
        ///
        /// One divergence remains, and no oracle fixture reaches it: a file
        /// *another* file's declarations reach back into during this pass (an
        /// imported super type whose own super type lives in `model_file`'s
        /// namespace) sees `model_file` here, where TS would see the file `self`
        /// actually holds under that namespace.
        pub fn validate_detached_model_file(&self, model_file: &ModelFile) -> Result<()> {
            let registered = self.model_file(model_file.namespace());
            if let Some(registered) = registered
                && registered.same_ast(model_file)
                && registered.file_name() == model_file.file_name()
            {
                return self.validate_model_file(registered);
            }
            let scratch = self.with_model_file_registered(std::sync::Arc::new(model_file.clone()))?;
            let registered = scratch
                .model_file(model_file.namespace())
                .expect("with_model_file_registered registers the file under its namespace");
            // P2-08d (#151): `import_scope: self`, not `scratch` — see the doc comment
            // above and on `validate_model_file_with_import_scope`.
            scratch.validate_model_file_with_import_scope(registered, self, None)
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.addModelFile`'s validate-then-register for a
        /// model file this manager does not hold yet: the same checks, the
        /// same first error and the same result as
        /// [`ModelManager::validate_detached_model_file`] followed, once it
        /// passes, by [`ModelManager::add_model_file`], returning the new
        /// file's handle.
        ///
        /// P5-48 (accordproject/concerto-rust#369): when this manager does
        /// not hold the file's namespace (the common case), the file is
        /// registered first and validated in place, and taken out again if
        /// validation fails, rather than validated in a scratch copy of the
        /// manager holding a deep copy of the file
        /// ([`ModelManager::with_model_file_registered`]) and then
        /// registered. The manager validated is the one the scratch copy
        /// would be (the same files, in the same order, the same options),
        /// and `check_imports` still sees the manager without the file's
        /// namespace (P2-08d). Any other case takes the two-step path.
        ///
        /// On a validation error the manager is as it was (its caches aside)
        /// and the file is handed back (boxed) with the error; an error from the
        /// registration itself (a namespace already registered, only on the
        /// two-step path) consumes it, as [`ModelManager::add_model_file`]
        /// does.
        pub fn validate_and_add_model_file(
            &mut self,
            model_file: ModelFile,
        ) -> std::result::Result<crate::model_manager::ModelFileId, (Error, Option<Box<ModelFile>>)> {
            self.validate_and_add_shared_model_file(std::sync::Arc::new(model_file))
                .map_err(|(err, handed_back)| {
                    let handed_back = handed_back.map(|shared| {
                        Box::new(std::sync::Arc::try_unwrap(shared).unwrap_or_else(|shared| (*shared).clone()))
                    });
                    (err, handed_back)
                })
        }
    }

    js_compat_pub! {
        /// [`ModelManager::validate_and_add_model_file`] for a model file that
        /// may also be held elsewhere (P5-101, D-9,
        /// accordproject/concerto-rust#455): the same checks, the same first
        /// error and the same result, but the file is registered shared, as
        /// [`ModelManager::add_shared_model_file`] registers it, not copied.
        /// On a validation error the shared file is handed back with the
        /// error; an error from the registration itself consumes it.
        pub fn validate_and_add_shared_model_file(
            &mut self,
            shared: std::sync::Arc<ModelFile>,
        ) -> std::result::Result<
            crate::model_manager::ModelFileId,
            (Error, Option<std::sync::Arc<ModelFile>>),
        > {
            if let Some((id, mark)) = self.append_for_validation(&shared) {
                let namespace = shared.namespace();
                return match self.validate_model_file_with_import_scope(&shared, self, Some(namespace)) {
                    Ok(()) => {
                        // P5-97: it passed with its own namespace hidden from
                        // its imports, so it passes `validate_models` too.
                        self.mark_validated(id);
                        Ok(id)
                    }
                    Err(err) => {
                        self.undo_append(mark);
                        Err((err, Some(shared)))
                    }
                };
            }
            if let Err(err) = self.validate_detached_model_file(&shared) {
                return Err((err, Some(shared)));
            }
            self.add_shared_model_file_with_proof(shared, None)
                .map_err(|err| (err, None))
        }
    }

    js_compat_pub! {
        /// TS `declaration.validate()` called directly on one declaration of a
        /// `ModelFile` built with `new ModelFile(modelManager, ast)` and never
        /// registered — `MapDeclaration.validate`'s oracle fixtures do exactly
        /// this (the fixture's `declref` targets an `mfnew` model file, P2-06b).
        /// Reuses [`ModelManager::validate_detached_model_file`]'s
        /// scratch-registration resolution, but for the one declaration at
        /// `index` in [`ModelFile::declarations`] rather than the whole file, so
        /// this is never charged for a sibling declaration's own errors, or for
        /// `ModelFile.validate`'s own import and duplicate-name checks — neither
        /// of which the recorded op ever runs.
        ///
        /// A pre-port `IllegalModel` error (`Error::illegal_model`, no TS
        /// class corresponds to it) if `model_file` has no declaration at
        /// `index`: a harness-only bound, the same convention `model_manager.rs`'s
        /// `next_index` documents.
        pub fn validate_detached_declaration(
            &self,
            model_file: &ModelFile,
            index: usize,
        ) -> Result<()> {
            let (scratch, namespace) = self.detached_scratch(model_file)?;
            let declaration = scratch
                .model_file(&namespace)
                .expect("with_model_file_registered registers the file under its namespace")
                .declarations()
                .get(index)
                .cloned()
                .ok_or_else(|| no_such_detached_declaration(model_file, index))?;
            declaration.validate(&scratch, &namespace)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::validate_detached_declaration`], but for just the key
        /// half of the map declaration at `index` (TS `MapKeyType.validate`,
        /// called directly rather than through `MapDeclaration.validate`).
        pub fn validate_detached_map_key(&self, model_file: &ModelFile, index: usize) -> Result<()> {
            let (scratch, map) = self.detached_map(model_file, index)?;
            validate_map_key(&scratch, model_file.namespace(), &map)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::validate_detached_map_key`], for the value half (TS
        /// `MapValueType.validate`).
        pub fn validate_detached_map_value(&self, model_file: &ModelFile, index: usize) -> Result<()> {
            let (scratch, map) = self.detached_map(model_file, index)?;
            validate_map_value(&scratch, model_file.namespace(), &map)
        }
    }

    /// The scratch-registered copy of `self`
    /// [`ModelManager::validate_detached_model_file`] builds (with
    /// `model_file` registered under its own namespace, in place of
    /// whatever `self` holds there), plus that namespace as an owned
    /// `String` — every caller here goes on to borrow `model_file`'s
    /// declaration back out of the *scratch* copy, not `self`.
    fn detached_scratch(&self, model_file: &ModelFile) -> Result<(Self, String)> {
        let scratch = self.with_model_file_registered(std::sync::Arc::new(model_file.clone()))?;
        Ok((scratch, model_file.namespace().to_string()))
    }

    /// [`ModelManager::detached_scratch`], plus the `MapDeclaration` at
    /// `index`, cloned out so it can be validated against the scratch copy
    /// without borrowing the copy at the same time.
    fn detached_map(&self, model_file: &ModelFile, index: usize) -> Result<(Self, MapDeclaration)> {
        let (scratch, namespace) = self.detached_scratch(model_file)?;
        let declaration = scratch
            .model_file(&namespace)
            .expect("with_model_file_registered registers the file under its namespace")
            .declarations()
            .get(index)
            .cloned()
            .ok_or_else(|| no_such_detached_declaration(model_file, index))?;
        match declaration {
            Declaration::Map(map) => Ok((scratch, map)),
            _ => Err(no_such_detached_declaration(model_file, index)),
        }
    }
}

/// A harness-only bound: `index` names no declaration of `model_file` (for
/// [`ModelManager::detached_map`], not one that loaded as a
/// `MapDeclaration`). No TS class corresponds to this, the same convention
/// `model_manager.rs`'s `next_index` documents.
fn no_such_detached_declaration(model_file: &ModelFile, index: usize) -> Error {
    Error::illegal_model(
        format!("no MapDeclaration at index {index}"),
        model_file.file_name().map(str::to_string),
        None,
    )
}

/// TS: `ModelFile.validate()`'s "Check if names of the declarations are
/// unique" loop (modelfile.ts): the first declaration whose fully-qualified
/// name repeats an earlier one's throws an `IllegalModelException` whose
/// message is `Duplicate class name <fqn>` — built with no model file and no
/// location, so neither is set here (and [`ModelManager::validate_model_file`]
/// does not attach one).
fn check_unique_declaration_names(model_file: &ModelFile) -> Result<()> {
    let mut seen = FxHashSet::default();
    for declaration in model_file.declarations() {
        if !seen.insert(declaration.name()) {
            return Err(Error::new(
                ErrorKind::IllegalModel,
                "modelfile-validate-duplicateclassname",
                vec![("fqn", qualify(model_file.namespace(), declaration.name()))],
            )
            .at(None));
        }
    }
    Ok(())
}

/// Fills in the current model file's name on an `IllegalModel` contract error
/// that does not carry one yet.
///
/// TS: every `IllegalModelException` raised while validating a model file's
/// own imports and declarations is constructed with `this.modelFile` (or,
/// for the file's own checks, `this` itself) — always the file being
/// validated here, never a different one (`ModelFile.validate`,
/// `Decorated.validate`, `ClassDeclaration.validate`/`_resolveSuperType`,
/// `Property.validate`/`RelationshipDeclaration.validate`, all in
/// src/introspect). The constructor decorates the message with `File
/// '<name>': ` whenever that file has one (illegalmodelexception.ts) — part
/// of `ModelFile.getName()`'s contract (P2-08). [`undeclared_type_error`]
/// already attaches its own file name; this backstops every other check in
/// this module, which builds a bare [`Error`] through
/// [`Error::new`] with no file in scope. A `model_file`
/// already set (as `undeclared_type_error` sets its own) is left alone, and
/// only `IllegalModel`-kind contract errors are touched.
pub(crate) fn attach_model_file(mut err: Error, model_file: &ModelFile) -> Error {
    let contract = err.contract();
    if contract.model_file.is_none() && contract.kind == ErrorKind::IllegalModel {
        err.contract_mut().model_file = Some(model_file.file_name().map(str::to_string));
    }
    err
}

/// A declaration may not take the name of a type its file imports (including
/// implicitly, from its own namespace, which is caught the same way) —
/// unless the model manager's `dangerouslyAllowReservedSystemTypeNamesInUserModels`
/// escape hatch is set and the imported name resolves to one of the five
/// reserved system declarations.
///
/// TS: `Declaration.validate` (declaration.ts, "#648"), reached through every
/// subtype's own `super.validate()` chain — `ClassDeclaration.validate` (so
/// every concept-like declaration and, unchanged, `EnumDeclaration`),
/// `ScalarDeclaration.validate` and `MapDeclaration.validate` all call it
/// after their own decorator checks and before anything else (P2-08; the
/// `MapDeclaration` call site was added by #152, closing finding F2).
fn check_import_clash(
    manager: &ModelManager,
    namespace: &str,
    name: &str,
    location: Option<&mm::Range>,
) -> Result<()> {
    let Some(model_file) = manager.model_file(namespace) else {
        return Ok(());
    };
    if !model_file.is_imported_type(name) {
        return Ok(());
    }
    if manager.dangerously_allow_reserved_system_type_names_in_user_models()
        && is_reserved_system_type_import(manager, model_file, name)
    {
        return Ok(());
    }
    Err(Error::new(
        ErrorKind::IllegalModel,
        "declaration-validate-importclash",
        vec![("name", name.to_string())],
    )
    .at(lazy_location(location)))
}

/// TS: `Declaration.isReservedSystemTypeImport` (declaration.ts) — `name`,
/// already known to be an imported type of `model_file`, resolves to a
/// concept-like declaration (concept, asset, participant, transaction or
/// event — never an enum, scalar or map) of a system model file.
fn is_reserved_system_type_import(
    manager: &ModelManager,
    model_file: &ModelFile,
    name: &str,
) -> bool {
    let Ok(fqn) = model_file.resolve_import(name) else {
        return false;
    };
    let Ok(declaration) = manager.get_declaration(&fqn) else {
        return false;
    };
    if !declaration.is_class_declaration() {
        return false;
    }
    let Ok(owner_namespace) = get_namespace(Some(&fqn)) else {
        return false;
    };
    manager
        .model_file(owner_namespace)
        .is_some_and(ModelFile::is_system_namespace)
}

impl Validate for Declaration {
    /// Class-like and map declarations have their own checks in this pass; an
    /// enum's are just its decorators (P2-07) and those of its values, since
    /// nothing else about it needs another declaration in view. A scalar's
    /// structural checks (its validator, its default value) are fully run
    /// while loading, but its decorators are not (TS `Decorated.validate`
    /// still runs from `ScalarDeclaration.validate`, scalardeclaration.ts),
    /// so this pass checks those the same way it does for an enum.
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()> {
        match self {
            Declaration::Class(class) => class.validate(manager, namespace),
            Declaration::Map(map) => map.validate(manager, namespace),
            Declaration::Enum(enm) => {
                let fqn = qualify(namespace, enm.name());
                // TS: `EnumDeclaration` inherits `ClassDeclaration.validate`
                // unchanged, whose own `super.validate()` reaches
                // `Declaration.validate`'s decorator and import-clash checks
                // before anything class-specific (P2-08). Within the
                // decorator checks, `Decorated.validate` runs each
                // decorator's own `.validate()` before the duplicate-name
                // scan (F4, #152).
                validate_decorators(manager, namespace, enm, Some(&fqn))?;
                check_unique_decorators(enm, None)?;
                check_import_clash(manager, namespace, enm.name(), enm.location())?;
                // TS: `ClassDeclaration.validate`'s duplicate-field-name
                // check, inherited unchanged by `EnumDeclaration` — run in
                // the same position relative to the decorator checks above
                // and the per-value checks below as TS's single `validate()`
                // body runs it relative to its own two neighbours (P2-04,
                // closing the "enum duplicate values" gap of plan §1.2).
                check_unique_field_names(manager, enm.name(), None, &fqn)?;
                for value in enm.values() {
                    // P5-48: the value's name is built only when a decorator
                    // check will read it.
                    let value_fqn = decorator_context(manager, value)
                        .then(|| format!("{fqn}.{}", value.name()));
                    validate_decorators(manager, namespace, value, value_fqn.as_deref())?;
                    check_unique_decorators(value, None)?;
                }
                Ok(())
            }
            Declaration::Scalar(scalar) => {
                let fqn = qualify(namespace, scalar.name());
                // TS: `ScalarDeclaration.validate`'s `super.validate()` goes
                // straight to `Declaration.validate` (it extends
                // `Declaration`, not `ClassDeclaration`): decorators, then
                // the import-clash check (P2-08). Its own further check
                // (a duplicate-FQN scan over `getModelFile()
                // .getAllDeclarations()`) is unreachable on this pass:
                // `ModelFile.validate()` runs the same scan over the same
                // declarations before validating any of them
                // (`check_unique_declaration_names`), so a duplicate never
                // gets this far. Within the decorator checks, `Decorated
                // .validate` runs each decorator's own `.validate()` before
                // the duplicate-name scan (F4, #152).
                validate_decorators(manager, namespace, scalar, Some(&fqn))?;
                check_unique_decorators(scalar, None)?;
                check_import_clash(manager, namespace, scalar.name(), None)
            }
        }
    }
}

impl Validate for ClassDeclaration {
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()> {
        let fqn = qualify(namespace, self.name());
        // TS: `ClassDeclaration.validate`'s `super.validate()`
        // (classdeclaration.ts) reaches `Declaration.validate`'s
        // `super.validate()` first — `Decorated.validate`'s decorator checks
        // (each decorator's own `.validate()`, then the duplicate-name scan,
        // F4, #152) — and only then `Declaration.validate`'s own
        // import-clash check ([`check_import_clash`]'s doc comment), before
        // this method's own super-type block (P2-08, reordered #152: this
        // used to run the decorator checks last).
        validate_decorators(manager, namespace, self, Some(&fqn))?;
        check_unique_decorators(self, self.location())?;
        check_import_clash(manager, namespace, self.name(), self.location())?;
        check_super_type(manager, namespace, self)?;
        // TS: the `if (this.idField)` identity block — `check_identifier`'s
        // not-a-property/not-a-string/optional checks, then
        // `check_identity_matches_super`'s super-type redeclare check — runs
        // before the "we also have to check fields defined in super
        // classes" duplicate-name loop (classdeclaration.ts `validate`).
        // Reordered (P2-08c review): a system-identified subclass of an
        // explicitly-identified super type used to reach the duplicate-name
        // loop first, misreporting the redeclare as two same-named
        // `$identifier` fields (its own, and the implicit `Asset`/etc. root
        // super type's own system identifier) instead of naming the conflict.
        check_identifier(manager, namespace, self)?;
        check_identity_matches_super(manager, namespace, self)?;
        check_unique_field_names(manager, self.name(), self.location(), &fqn)?;
        // TS: `for (field of this.getProperties())` — every property, own
        // and then inherited (`getProperties` walks up the super-type
        // chain), each validated in this class's own pass (P2-08 review:
        // this used to loop over `own_properties()` only, so a file
        // validated on its own never checked what it inherits).
        // P5-48: the borrowed property list (P5-13's `class_properties`),
        // not a copied one with an owned owner name per property.
        for (owner_fqn, property) in manager.class_properties(&fqn)?.iter() {
            validate_property(manager, namespace, owner_fqn, property)?;
        }
        Ok(())
    }
}

/// TS: one iteration of `ClassDeclaration.validate`'s property loop
/// (classdeclaration.ts) for `class` in `namespace`, over a property declared
/// by `owner_fqn` (`class` itself, or one of its super types).
///
/// TS picks the declaration `field.validate(classDecl)` runs against: `this`
/// (`class`) when the field is primitive or declared in `class`'s own
/// namespace; otherwise the declaration of the field's *type*
/// (`modelManager.getType(field.getFullyQualifiedTypeName())`, the type name
/// resolved in the declaring file). `classDecl.getModelFile()` is where
/// `Property.validate` resolves the type name and which file its errors
/// name. `Decorated.validate` (the property's own decorators) and
/// `RelationshipDeclaration.validate`'s lookups use the property's own
/// parent instead — the declaring file.
fn validate_property(
    manager: &ModelManager,
    namespace: &str,
    owner_fqn: &str,
    property: &Property,
) -> Result<()> {
    let owner_ns = get_namespace(Some(owner_fqn))?;
    // P5-48: the property's own name is built only when a decorator check
    // will read it.
    let property_fqn =
        decorator_context(manager, property).then(|| format!("{owner_fqn}.{}", property.name()));
    // `field.getModelFile()`: the declaring file, for an inherited
    // property's own `Decorated.validate` errors.
    let owner_file = manager.model_file(owner_ns);
    let in_owner_file = |e: Error| match owner_file {
        Some(file) if owner_ns != namespace => attach_model_file(e, file),
        _ => e,
    };

    // TS: `Property.validate` runs `super.validate()` — `Decorated.validate`:
    // each decorator's own `.validate()` when enabled, then the
    // duplicate-decorator scan (F4, #152) — before its own `resolveType`
    // call (property.ts). `check_property_type` below is that
    // `resolveType`/relationship logic, so the decorator checks run first
    // here too (P2-08 review carry-over (b) from P2-04's review, #48).
    validate_decorators(manager, owner_ns, property, property_fqn.as_deref())
        .map_err(in_owner_file)?;
    check_unique_decorators(property, property.location()).map_err(in_owner_file)?;

    let type_name = property.type_identifier().map(|t| t.name.as_str());
    let is_primitive = type_name.is_none_or(is_primitive_type);
    if is_primitive || owner_ns == namespace {
        return check_property_type(manager, namespace, owner_ns, owner_fqn, property);
    }

    // `field.getFullyQualifiedTypeName()`: resolved in the declaring file,
    // a plain `Error` when it does not resolve there; then
    // `modelManager.getType(typeFqn)`.
    let type_name = type_name.unwrap_or_default();
    let Some(type_fqn) = resolve(manager, owner_ns, type_name) else {
        return Err(Error::new(
            ErrorKind::InvalidArgument,
            "property-getfullyqualifiedtypename-notfound",
            vec![
                ("name", property.name().to_string()),
                ("type", type_name.to_string()),
            ],
        ));
    };
    // TS `modelManager.getType(typeFqn)`, with its own two errors.
    manager.get_type_declaration(&type_fqn)?;
    let context_ns = get_namespace(Some(&type_fqn))?;
    check_property_type(manager, context_ns, owner_ns, owner_fqn, property).map_err(|e| {
        match manager.model_file(context_ns) {
            Some(file) if context_ns != namespace => attach_model_file(e, file),
            _ => e,
        }
    })
}

/// Whether [`validate_decorators`] reads its `context` for `element`: only
/// when decorator validation is enabled and `element` has a decorator
/// (P5-48, so that a caller builds the context string only then).
fn decorator_context(manager: &ModelManager, element: &impl Decorated) -> bool {
    manager.decorator_validation().is_enabled() && !element.decorators().is_empty()
}

/// Runs [`crate::introspect::decorator::Decorator::validate`] over every
/// decorator an element carries, when the model manager's
/// `decoratorValidation` option enables it (TS `Decorator.validate` is a
/// no-op otherwise, and so is this: P2-07).
fn validate_decorators(
    manager: &ModelManager,
    namespace: &str,
    element: &impl Decorated,
    context: Option<&str>,
) -> Result<()> {
    if !manager.decorator_validation().is_enabled() {
        return Ok(());
    }
    for decorator in element.decorators() {
        decorator.validate(manager, namespace, context)?;
    }
    Ok(())
}

/// An element may not carry the same decorator twice.
fn check_unique_decorators(element: &impl Decorated, location: Option<&mm::Range>) -> Result<()> {
    let mut seen = FxHashSet::default();
    for decorator in element.decorators() {
        // TS keys its `Set` on `getName()` and interpolates it into the
        // message as is, so a decorator with no `name` at all is its own
        // entry and reads `undefined` (accordproject/concerto-rust#218).
        let name = decorator.js_name();
        if !seen.insert(name) {
            return Err(Error::new(
                ErrorKind::IllegalModel,
                "decorated-validate-duplicatedecorator",
                vec![("name", name.unwrap_or("undefined").to_string())],
            )
            .at(lazy_location(location)));
        }
    }
    Ok(())
}

/// A class's own super type, short-name kinds exempt from the self-extend
/// check.
///
/// TS: `ClassDeclaration.validate`'s super-type block
/// (src/introspect/classdeclaration.ts): `['Asset', 'Concept', 'Event',
/// 'Participant', 'Transaction']`.
const SELF_EXTENDING_EXEMPT: [&str; 5] =
    ["Asset", "Concept", "Event", "Participant", "Transaction"];

/// The super type, if any (explicit or implicit `Concept`, `ClassDeclaration`
/// doc comment), must not be the class's own name (unless it is one of the
/// five built-in kinds), must resolve to a declared type, and — unless that
/// type is a concept — must be the same kind as the class itself: an asset
/// cannot extend a participant, for example.
///
/// TS: `ClassDeclaration.validate`'s super-type block, then
/// `_resolveSuperType` (src/introspect/classdeclaration.ts).
fn check_super_type(
    manager: &ModelManager,
    namespace: &str,
    class: &ClassDeclaration,
) -> Result<()> {
    let Some(super_type) = class.super_type() else {
        return Ok(());
    };

    if super_type.name == class.name() && !SELF_EXTENDING_EXEMPT.contains(&super_type.name.as_str())
    {
        return Err(Error::new(
            ErrorKind::IllegalModel,
            "classdeclaration-validate-selfextending",
            vec![("class", class.name().to_string())],
        )
        .at(class_location(class)));
    }

    let super_declaration = resolve(manager, namespace, &super_type.name)
        .and_then(|fqn| manager.get_declaration(&fqn).ok());
    let Some(super_declaration) = super_declaration else {
        // TS: `_resolveSuperType`'s hardcoded string, not a catalogue
        // template (Globalize is never called on this path).
        return Err(Error::new(
            ErrorKind::IllegalModel,
            "classdeclaration-resolvesupertype-notfound",
            vec![("superType", super_type.name.to_string())],
        )
        .at(class_location(class)));
    };

    // A super type that is not a concept must be the exact same kind as the
    // subtype. This also covers a super type that resolves to a non-class
    // declaration (an enum, scalar or map): TS never checks `classDecl` is a
    // `ClassDeclaration` before comparing `declarationKind()`, so extending
    // one of those fails here, with the same message, rather than with
    // "could not find".
    if super_declaration.declaration_kind() != "ConceptDeclaration"
        && class.declaration_kind() != super_declaration.declaration_kind()
    {
        return Err(Error::new(
            ErrorKind::IllegalModel,
            "classdeclaration-resolvesupertype-kindmismatch",
            vec![
                ("kind", class.declaration_kind().to_string()),
                ("name", class.name().to_string()),
                (
                    "superKind",
                    super_declaration.declaration_kind().to_string(),
                ),
                ("superName", super_declaration.name().to_string()),
            ],
        )
        .at(class_location(class)));
    }

    Ok(())
}

/// No field name may appear twice once inherited fields are included, so a
/// subtype cannot silently redeclare a field from a super type.
///
/// TS: `ClassDeclaration.validate`'s `uniquePropertyNames` loop
/// (classdeclaration.ts), inherited unchanged by `EnumDeclaration` — an
/// enum's values are properties too (`getProperties()`), so two values of
/// the same name in one enum are rejected exactly the way two same-named
/// fields on a class are, with the same message and catalogue code
/// (`declaration_name`/`location` let both callers share this one check;
/// see [`ClassDeclaration::validate`] and the `Declaration::Enum` arm of
/// [`Validate for Declaration`]).
fn check_unique_field_names(
    manager: &ModelManager,
    declaration_name: &str,
    location: Option<&mm::Range>,
    fqn: &str,
) -> Result<()> {
    // P5-48: the borrowed property list and borrowed names, in an FxHash
    // set (only ever probed, never iterated).
    let properties = manager.class_properties(fqn)?;
    let mut seen = rustc_hash::FxHashSet::default();
    for (_, property) in properties.iter() {
        if !seen.insert(property.name()) {
            return Err(Error::new(
                ErrorKind::IllegalModel,
                "classdeclaration-validate-duplicatefieldname",
                vec![
                    ("class", declaration_name.to_string()),
                    ("fieldName", property.name().to_string()),
                ],
            )
            .at(lazy_location(location)));
        }
    }
    Ok(())
}

/// A field-provided identifier (`identified by field`) must name a required
/// field typed as `String` or a String-based scalar. The field itself may
/// come from a super type (TS: `this.getProperty(this.idField)`
/// — src/introspect/classdeclaration.ts — inherited, unlike
/// [`ClassDeclaration::identifier_field_name`] itself, which only ever names
/// one of this class's own fields).
fn check_identifier(
    manager: &ModelManager,
    namespace: &str,
    class: &ClassDeclaration,
) -> Result<()> {
    let Some(field_name) = class.identifier_field_name() else {
        return Ok(());
    };
    let fqn = qualify(namespace, class.name());
    let (owner, field) = manager
        .class_properties(&fqn)?
        .find(field_name)
        .ok_or_else(|| {
            Error::new(
                ErrorKind::IllegalModel,
                "classdeclaration-validate-identifiernotproperty",
                vec![
                    ("class", class.name().to_string()),
                    ("idField", field_name.to_string()),
                ],
            )
            .at(class_location(class))
        })?;

    // TS checks the type first, then optionality (classdeclaration.ts
    // `validate`), so an optional non-String identifier reports the type.
    // TS: `idField.getParent().getModelFile().getType(idField.getType())`
    // resolves the field's type in the file that *declares* the field, which
    // for an inherited identifier is the super type's, not this class's.
    let owner_namespace = get_namespace(Some(owner))?;
    if !is_string_typed(manager, owner_namespace, field) {
        return Err(Error::new(
            ErrorKind::IllegalModel,
            "classdeclaration-validate-identifiernotstring",
            vec![
                ("class", class.name().to_string()),
                ("idField", field_name.to_string()),
            ],
        )
        .at(class_location(class)));
    }
    // TS: hardcoded, not a catalogue template (Globalize is never called on
    // this path).
    if field.is_optional() {
        return Err(Error::new(
            ErrorKind::IllegalModel,
            "classdeclaration-validate-identifieroptional",
            vec![],
        )
        .at(class_location(class)));
    }
    Ok(())
}

/// Whether a field is a `String`, or an object field whose type resolves to a
/// scalar declared over `String`.
fn is_string_typed(manager: &ModelManager, namespace: &str, field: &Property) -> bool {
    if matches!(field, Property::String(_)) {
        return true;
    }
    let Some(type_identifier) = field.type_identifier() else {
        return false;
    };
    resolve(manager, namespace, &type_identifier.name)
        .and_then(|fqn| manager.get_declaration(&fqn).ok())
        .is_some_and(|declaration| declaration.type_name() == Some("String"))
}

/// Object and relationship properties must point at a declared type; a
/// relationship additionally must target an identifiable class, never a
/// primitive.
/// `namespace` is the namespace of TS's `classDecl.getModelFile()` — where
/// the type name is resolved ([`validate_property`]); `owner_ns` and
/// `owner` name the property's declaring class, for its fully-qualified
/// name and for `RelationshipDeclaration.validate`'s own target lookup;
/// `class` is the class whose pass this is, for the last-resort fallback's
/// location. `owner_fqn` is `owner_ns` and `owner` qualified (the arena's
/// cached FQN, P5-48: not rebuilt per property).
#[allow(clippy::too_many_arguments)]
fn check_property_type(
    manager: &ModelManager,
    namespace: &str,
    owner_ns: &str,
    owner_fqn: &str,
    property: &Property,
) -> Result<()> {
    let Some(type_identifier) = property.type_identifier() else {
        // A primitive field: TS's `resolveType` of a primitive always
        // succeeds, so the size-validator check is all that is left.
        return check_size_validator_target(owner_fqn, property, false);
    };

    if type_identifier.name.is_empty() {
        // TS: `this.type` is falsy for an empty-string type name exactly as
        // it is for `null`/`undefined` (`ObjectProperty`'s `this.ast.type ?
        // this.ast.type.name : null`, `RelationshipProperty`'s unconditional
        // `this.ast.type.name`), so `Property.validate`'s `if(this.type)`
        // guard skips `resolveType` entirely — no "undeclared type" is ever
        // raised for it. `RelationshipDeclaration.validate` then makes its
        // own `if(!this.getType())` check straight after `super.validate`
        // (relationshipdeclaration.ts): a relationship with no type is
        // rejected there; any other property kind is silently accepted, the
        // same as a genuinely absent `type` node.
        if property.is_relationship() {
            return Err(Error::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-notype",
                vec![],
            )
            .at(property_location(property)));
        }
        return check_size_validator_target(owner_fqn, property, false);
    }

    if is_primitive_type(&type_identifier.name) {
        // TS: `resolveType` succeeds for a primitive, then `Property.validate`
        // runs its size-validator check (a primitive is never a map), all
        // before `RelationshipDeclaration.validate`'s own checks.
        check_size_validator_target(owner_fqn, property, false)?;
    }

    if property.is_relationship() && is_primitive_type(&type_identifier.name) {
        // TS: RelationshipDeclaration.validate's own hardcoded message
        // (src/introspect/relationshipdeclaration.ts): `'Relationship ' +
        // this.getName() + ' cannot be to the primitive type ' +
        // this.getType()` — no owner clause.
        return Err(Error::new(
            ErrorKind::IllegalModel,
            "relationshipdeclaration-validate-primitivetype",
            vec![
                ("name", property.name().to_string()),
                ("type", type_identifier.name.to_string()),
            ],
        )
        .at(property_location(property)));
    }

    let target_fqn = resolve(manager, namespace, &type_identifier.name);

    let Some(target_fqn) = target_fqn else {
        // TS: `Property.validate` runs `classDecl.getModelFile().resolveType(
        // 'property ' + this.getFullyQualifiedName(), this.type)` before any
        // relationship-specific check (property.ts) — `RelationshipDeclaration
        // .validate` calls it through `super.validate(classDecl)` first thing.
        // `resolveType` (modelfile.ts) is the same import/local lookup
        // `resolve` above does; when the type name does not resolve through
        // it at all, this is the error every property kind raises — a
        // relationship never reaches its own "points to a missing type" check
        // below, because `resolveType` throws first.
        return Err(undeclared_type_error(
            manager,
            namespace,
            &type_identifier.name,
            format!("property {owner_fqn}.{}", property.name()),
        ));
    };

    let target = manager.get_declaration(&target_fqn).ok();
    if !is_primitive_type(&type_identifier.name) {
        // TS: `Property.validate`'s size-validator check runs right after
        // `resolveType` succeeds and before any relationship-specific check;
        // a type that `getType` cannot find (swallowed by its try/catch)
        // counts as not a map.
        check_size_validator_target(
            owner_fqn,
            property,
            target.is_some_and(Declaration::is_map_declaration),
        )?;
    }
    // `RelationshipDeclaration.validate` looks its target up from the
    // property's own parent — the declaring file — not from `classDecl`.
    // The two agree unless an inherited property is validated in the
    // context of its type's declaration ([`validate_property`]).
    let (target_fqn, target) = if owner_ns == namespace {
        (target_fqn, target)
    } else {
        match resolve(manager, owner_ns, &type_identifier.name) {
            Some(fqn) => {
                let target = manager.get_declaration(&fqn).ok();
                (fqn, target)
            }
            None => (target_fqn, target),
        }
    };
    let Some(target) = target else {
        if property.is_relationship() {
            // TS: `'Relationship ' + this.getName() + ' points to a missing
            // type ' + this.getFullyQualifiedTypeName()`
            // (relationshipdeclaration.ts), reached only once `resolveType`
            // above has already succeeded (the type resolves through
            // import/local lookup, so `target_fqn` is always known here) and
            // the declaration lookup `RelationshipDeclaration.validate` does
            // on top of that — `getModelFile().getType(...)` in the same
            // namespace, `getModelManager().getType(...)` otherwise, swallowed
            // into `null` on error — still comes back empty.
            //
            // No unit test reaches this branch: through the full
            // `validate_models` pipeline, `target_fqn` (via [`resolve`],
            // ultimately `ModelFile::resolve_local_type`) and
            // `manager.get_declaration` always agree. A local name in
            // `resolve_local_type`'s `local_types` map is built straight from
            // the file's own declarations, the same list `get_declaration`
            // reads; an imported name is checked against the *target*
            // namespace's declarations by [`check_imported_types_exist`],
            // which every model file's own imports are run through before any
            // of its declarations validate. So an import naming a
            // non-existent type is rejected earlier, with a different
            // message, before a relationship of that file ever reaches this
            // check. This mirrors TS: real divergence between `resolveType`
            // and the later `getType` lookup needs the try/catch around
            // `getModelManager().getType(...)` to swallow a *different* kind
            // of failure than "undeclared", which the current port does not
            // yet model.
            return Err(Error::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-missingtype",
                vec![("name", property.name().to_string()), ("type", target_fqn)],
            )
            .at(property_location(property)));
        }
        // Not yet observed for a non-relationship property: TS's own
        // `Property.validate` has no declaration lookup beyond `resolveType`
        // above, which just succeeded, so this is unreached in practice. Kept
        // as a defensive fallback rather than an `unreachable!`, since
        // `resolve` and `get_declaration` are still two separate Rust
        // lookups that could in principle disagree; it raises the error
        // `resolveType` itself raises for a type it cannot find (P5-98,
        // B-10: no longer a `pre-port` message of its own).
        return Err(undeclared_type_error(
            manager,
            namespace,
            &type_identifier.name,
            format!("property {owner_fqn}.{}", property.name()),
        ));
    };

    if property.is_relationship() {
        // TS: `classDeclaration.isIdentified()` (RelationshipDeclaration.validate,
        // src/introspect/relationshipdeclaration.ts) — inherited, so a
        // target that has no identity of its own but extends one that does
        // (every `Asset`/`Participant`, for one) still counts.
        let identifiable =
            target.is_class_declaration() && manager.identifier_field(&target_fqn)?.is_some();
        if !identifiable {
            // TS: `'Relationship ' + this.getName() + ' must be to a class
            // that has an identifier, but this is to ' +
            // this.getFullyQualifiedTypeName()` — no owner clause, and with
            // the target's own fully-qualified name appended.
            return Err(Error::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-notidentified",
                vec![("name", property.name().to_string()), ("type", target_fqn)],
            )
            .at(property_location(property)));
        }
    }

    Ok(())
}

/// TS: `Property.validate`'s size-validator check (property.ts): a
/// `sizeValidator` on a property that is not an array is allowed only when
/// the property's type resolves to a map declaration (`is_map_type`). Its
/// own hardcoded message names the property by
/// `getFullyQualifiedName()` — the owning declaration's fully-qualified name
/// plus the property's — with `this.ast.location`. TS's `Property`
/// constructor does not check this (P2-08: the check moved here from
/// `Property::check_validators`, so a `ModelFile` with such a property still
/// constructs).
fn check_size_validator_target(
    owner_fqn: &str,
    property: &Property,
    is_map_type: bool,
) -> Result<()> {
    if property.size_validator().is_some() && !property.is_array() && !is_map_type {
        return Err(Error::new(
            ErrorKind::IllegalModel,
            "property-validate-sizevalidator",
            vec![("fqn", format!("{owner_fqn}.{}", property.name()))],
        )
        .at(property_location(property)));
    }
    Ok(())
}

/// Resolves a `TypeIdentifier`'s `name` to a fully-qualified name, through
/// the imports and local declarations of `namespace`.
fn resolve(manager: &ModelManager, namespace: &str, name: &str) -> Option<String> {
    // TS: every `TypeIdentifier` consumer in the reference — `this.superType
    // = this.ast.superType.name` (`ClassDeclaration.process`), `this.type =
    // this.ast.type.name` (`Property.process`, `MapKeyType.process`,
    // `MapValueType.process`) — keeps only `name`, discarding `namespace`
    // (and `resolvedName`) outright, then resolves that bare name through
    // the declaring namespace's own imports (`isImportedType`/
    // `resolveImport`) or local declarations, never by qualifying `name`
    // with `ti.namespace` directly. That distinction matters for an aliased
    // import's `TypeIdentifier`: its `namespace` is the *target*'s (the
    // import's own `namespace`), while `name` is the local alias, not the
    // target's own declared name — qualifying `name` with that `namespace`
    // directly would build `{target namespace}.{alias}`, a name nothing
    // declares; the alias only resolves correctly by going through the
    // import list ([`ModelManager::resolve_type_name`], PORTING.md 6.2),
    // which every caller here now always does.
    //
    // The error is discarded (`.ok()`) by every caller: they use `None` to
    // mean "does not resolve" and build their own error, so the location `resolve_type_name` would attach to its own
    // error never surfaces. Passing `None` here is exact, not a shortcut.
    manager.resolve_type_name_at(namespace, name, None).ok()
}

/// TS `ModelFile.validate`'s single loop over `this.getImports()`
/// (modelfile.ts), for every fully-qualified name this file imports (so an
/// `import ns.{A, B}` is walked once per name, not once per import
/// statement): the source namespace must be loaded; no earlier import in
/// this file may have named a different version of the same bare namespace
/// (the global `concerto` namespace is exempt); and the imported short name
/// must actually be declared there. `None` for every error's `location`:
/// `ModelFile` carries no `location` field in this port (7.2), and TS itself
/// passes none on this path (`this` alone, no `fileLocation` argument).
///
/// TS computes `modelFile` (the lookup below) and then *unconditionally*
/// destructures `ModelUtil.parseNamespace(importNamespace)` — before it ever
/// checks `!modelFile` and throws `modelmanager-gettype-noregisteredns`. So a
/// import namespace that is both unregistered and fails `parseNamespace`
/// (for example a mutated import `name` that makes the synthesised
/// `importNamespace` carry a second, semver-invalid `@version` segment)
/// surfaces `parseNamespace`'s own plain `Error`, never the "no registered
/// ns" `IllegalModelException` — reproduced here by calling
/// [`parse_namespace`] first and propagating its error with `?`, faithfully
/// including that ordering (accordproject/concerto-rust#241, the `../`
/// namespace-import mismatch off #219).
fn check_imports(
    manager: &ModelManager,
    hidden: Option<&str>,
    model_file: &ModelFile,
) -> Result<()> {
    // P5-48 (accordproject/concerto-rust#369): the walk over
    // `imported_type_names()`, reading each import's namespace and name in
    // place; a fully-qualified name is built only for an error, or for a
    // name the plain split would not give back (an empty part, or a dot in
    // the imported name).
    type Borrowed<'a> = std::borrow::Cow<'a, str>;
    let mut seen_versions: rustc_hash::FxHashMap<Borrowed<'_>, Option<Borrowed<'_>>> =
        rustc_hash::FxHashMap::default();
    for imp in model_file.imports() {
        for imported in imp.imported_names() {
            let owned: String;
            let in_place =
                !imp.namespace().is_empty() && !imported.is_empty() && !imported.contains('.');
            let (import_namespace, import_short_name) = if in_place {
                (imp.namespace(), imported.as_str())
            } else {
                owned = qualify(imp.namespace(), imported);
                (get_namespace(Some(&owned))?, short_name(&owned))
            };
            let import_fqn = || qualify(imp.namespace(), imported);

            let found = if hidden == Some(import_namespace) {
                None
            } else {
                manager.model_file(import_namespace)
            };
            // Borrowed from the import itself, or, on the rare path that built
            // the name, copied (the set outlives it).
            let (name, version): (Borrowed<'_>, Option<Borrowed<'_>>) = if in_place {
                let (name, version) = model_util::split_namespace(imp.namespace())?;
                (Borrowed::Borrowed(name), version.map(Borrowed::Borrowed))
            } else {
                let (name, version) = model_util::split_namespace(import_namespace)?;
                (
                    Borrowed::Owned(name.to_string()),
                    version.map(|v| Borrowed::Owned(v.to_string())),
                )
            };

            let Some(source_file) = found else {
                return Err(Error::new(
                    ErrorKind::IllegalModel,
                    "modelmanager-gettype-noregisteredns",
                    vec![("type", import_fqn())],
                )
                .at(None));
            };

            let is_global_model = name == "concerto";
            if let Some(existing) = seen_versions.get(&name)
                && *existing != version
                && !is_global_model
            {
                return Err(Error::new(
                    ErrorKind::IllegalModel,
                    "modelmanager-gettype-duplicatensimport",
                    vec![
                        ("namespace", import_namespace.to_string()),
                        (
                            "version1",
                            existing.as_deref().unwrap_or_default().to_string(),
                        ),
                        (
                            "version2",
                            version.as_deref().unwrap_or_default().to_string(),
                        ),
                    ],
                )
                .at(None));
            }
            seen_versions.insert(name, version);

            if !source_file.is_local_type(import_short_name) {
                return Err(Error::new(
                    ErrorKind::IllegalModel,
                    "modelmanager-gettype-notypeinns",
                    vec![
                        ("type", import_short_name.to_string()),
                        ("namespace", import_namespace.to_string()),
                    ],
                )
                .at(None));
            }
        }
    }
    Ok(())
}

/// A type that carries the system identifier may not extend one that is
/// identified by a field of its own, because the two identities would disagree.
fn check_identity_matches_super(
    manager: &ModelManager,
    namespace: &str,
    class: &ClassDeclaration,
) -> Result<()> {
    // TS: `if (this.idField)` — own identity only; a class with no identity
    // of its own has nothing to conflict with its super type here (a
    // subtype that merely inherits identity is not this check's concern).
    if !class.is_identified() {
        return Ok(());
    }
    let Some(super_type) = class.super_type() else {
        return Ok(());
    };
    let Some(super_fqn) = resolve(manager, namespace, &super_type.name) else {
        // An unresolved super type is reported by `check_super_type`.
        return Ok(());
    };
    let Ok(super_declaration) = manager.get_declaration(&super_fqn) else {
        return Ok(());
    };
    let Some(super_class) = super_declaration.as_class() else {
        return Ok(());
    };
    // TS: `superType.isIdentified()` — inherited, so a direct super type
    // with no identity of its own but an identified ancestor still gates
    // this check.
    let Some(super_id_field) = manager.identifier_field(&super_fqn)? else {
        return Ok(());
    };
    // TS: within `if (this.idField)`, `this.isSystemIdentified()` reduces to
    // whether this class's own `idField` is `$identifier`, since own
    // identity always wins over inherited in `getIdentifierFieldName`.
    let this_system_identified = class.identifier_field_name().is_none();
    // TS: `this.isSystemIdentified()` ? `!superType.isSystemIdentified()` (both
    // inherited) : `superType.isExplicitlyIdentified()` (the direct super
    // type's own field, not inherited further).
    let redeclares = if this_system_identified {
        super_id_field != "$identifier"
    } else {
        super_class.identifier_field_name().is_some()
    };
    if redeclares {
        return Err(Error::new(
            ErrorKind::IllegalModel,
            "classdeclaration-validate-redeclaredidentifier",
            vec![
                ("superType", super_fqn.to_string()),
                ("idField", super_id_field.to_string()),
            ],
        )
        .at(class_location(class)));
    }
    Ok(())
}

impl Validate for MapDeclaration {
    /// Checks a map against the key and value types the specification
    /// permits, and, like every other declaration (P2-07), that it carries no
    /// duplicate decorator and that its decorators pass `decoratorValidation`
    /// when enabled.
    ///
    /// TS: `MapDeclaration.validate` (src/introspect/mapdeclaration.ts) is
    /// `super.validate(); this.key.validate(); this.value.validate()`, where
    /// `super.validate()` is `Declaration.validate` — `Decorated.validate`'s
    /// decorator checks (each decorator's own `.validate()`, then the
    /// duplicate-name scan, F4), then the import-clash check — run before
    /// the key and value checks (F2, F3, #152: this used to run the key and
    /// value checks first, and never ran the import-clash check at all). The
    /// oracle op `MapDeclaration.validate` exercises this whole sequence,
    /// while `MapKeyType.validate` and `MapValueType.validate` exercise
    /// `validate_map_key` and `validate_map_value` in isolation (see
    /// `tests/oracle/ops.rs`).
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()> {
        let fqn = qualify(namespace, self.name());
        validate_decorators(manager, namespace, self, Some(&fqn))?;
        check_unique_decorators(self, None)?;
        check_import_clash(manager, namespace, self.name(), None)?;
        validate_map_key(manager, namespace, self)?;
        validate_map_value(manager, namespace, self)
    }
}

js_compat_pub! {
    /// `MapKeyType.validate` (src/introspect/mapkeytype.ts). The key-kind
    /// membership check TS makes at `MapDeclaration` construction time
    /// (`ModelUtil.isValidMapKey`) is the typed read's: a loaded
    /// `MapDeclaration`'s key is always one of the metamodel's key kinds
    /// (P5-61).
    ///
    /// Every error passes `location: None`: `MapDeclaration` does not keep
    /// its key's location, so there is no AST node to copy from.
    pub fn validate_map_key(
        manager: &ModelManager,
        namespace: &str,
        map: &MapDeclaration,
    ) -> Result<()> {
        // An object key names a scalar, which has to be over a String or DateTime.
        //
        // TS: `MapKeyType.validate` (src/introspect/mapkeytype.ts) — this is a
        // different check, with a different message, than the kind-membership
        // one above (which ports `MapDeclaration`'s own construction-time
        // `ModelUtil.isValidMapKey`, this function's own doc comment): this one
        // runs once the key's kind is already known to be `ObjectMapKeyType`,
        // over the scalar it names.
        if let Some(key) = map.key_type() {
            let scalar = resolve(manager, namespace, &key.name)
                .and_then(|fqn| manager.get_declaration(&fqn).ok())
                .and_then(Typed::type_name);
            if !matches!(scalar, Some("String") | Some("DateTime")) {
                // TS: `MapKeyType.validate` throws `new
                // IllegalModelException(message)` with no `modelFile` argument
                // (mapkeytype.ts) — unlike a construction-time check, this
                // message never gets a `File '<name>': ` suffix. A default
                // `model_file: None` would let
                // [`ModelManager::validate_model_file`]'s generic
                // [`attach_model_file`] stamp one on anyway, so it is marked
                // `Some(None)` ("no file, and already decided") here instead.
                let mut err = ContractError::new(
                    ErrorKind::IllegalModel,
                    "mapkeytype-validate-invalidscalar",
                    vec![
                        ("type", key.name.to_string()),
                        ("name", map.name().to_string()),
                    ],
                );
                err.model_file = Some(None);
                return Err(err.into());
            }
        }
        Ok(())
    }
}

js_compat_pub! {
    /// `MapValueType.validate` (src/introspect/mapvaluetype.ts). The
    /// value-kind membership check and `MapValueType.processType`'s "must
    /// contain property 'type'" checks are the typed read's: a loaded
    /// `MapDeclaration`'s value is always one of the metamodel's value
    /// kinds, and an object or relationship value always has a `type` with
    /// a string `name` (P5-61).
    pub fn validate_map_value(
        manager: &ModelManager,
        namespace: &str,
        map: &MapDeclaration,
    ) -> Result<()> {
        // TS: `MapValueType.processType` (src/introspect/mapvaluetype.ts): an
        // object or relationship value's `type.$class` must be
        // `TypeIdentifier`. The typed read keeps the `$class` string as
        // given, so this is checked here.
        if let Some(t) = map.value_type()
            && t._class != crate::introspect::qualified_class("TypeIdentifier")
        {
            // TS names the value type `ObjectMapValueType` here for a
            // relationship value too (the template's own text).
            return Err(Error::new(ErrorKind::IllegalModel, "mapvaluetype-process-invalidtypeclass", vec![("name", map.name().to_string())]).at(None));
        }

        // TS: `MapValueType.validate` allows any declaration as a map value except
        // another MapDeclaration ("All declarations, with the exception of
        // MapDeclarations, are valid Values."); it does not itself check that the
        // referenced type is declared.
        if let Some(value) = map.value_type() {
            let declared = resolve(manager, namespace, &value.name)
                .and_then(|fqn| manager.get_declaration(&fqn).ok());
            let Some(declared) = declared else {
                // BC-12 (R1): an undeclared value type is an
                // `IllegalModelException` naming it. TS 5.0.0 read
                // `this.modelFile.getType(...)`'s `null` straight into
                // `decl.isMapDeclaration?.()` (the `?.` guards only the call,
                // not the property read), so V8 threw `TypeError: Cannot read
                // properties of null (reading 'isMapDeclaration')` (DV-014).
                return Err(undeclared_type_error(
                    manager,
                    namespace,
                    &value.name,
                    format!("the value of map {}", qualify(namespace, map.name())),
                ));
            };
            if declared.is_map_declaration() {
                return Err(Error::new(ErrorKind::IllegalModel, "mapvaluetype-validate-mapnotsupported", vec![("type", value.name.to_string())]).at(None));
            }
        }
        Ok(())
    }
}

/// Builds a semantic-validation error through a message-catalogue entry
/// (PORTING.md 2.1, 2.2): `code` is a catalogue key, either a template TS
/// builds with `Globalize(...).messageFormatter(code)(params)` or one of
/// TS's own hardcoded strings ported as a [`crate::error::Renderer::Inline`]
/// entry (P5-98, B-10: these used to be `pre-port` messages).
///
/// `modelfile-resolvetype-undecltype`: TS's `ModelFile.resolveType` (module
/// doc on [`resolve`]) — a type name that resolves through neither the
/// primitive list, an import, nor a local declaration. `context` is TS's own
/// `context` argument verbatim (e.g. `'property ' + this.getFullyQualifiedName()`,
/// `Property.validate`, property.ts); no `location`, since `resolveType`'s own
/// callers on this path never pass its optional `fileLocation` third argument.
///
/// TS passes `this` (the `ModelFile`) as the exception's `modelFile` argument
/// (`IllegalModelException` constructor), which the message ends with
/// `modelFile.getName()` for, when the file was given a name — `namespace`'s
/// own [`ModelFile::file_name`], read fresh through `manager` rather than
/// threaded in by every caller, since a property's declaring class always
/// carries its namespace already.
fn undeclared_type_error(
    manager: &ModelManager,
    namespace: &str,
    type_name: &str,
    context: String,
) -> Error {
    let mut err = ContractError::new(
        ErrorKind::IllegalModel,
        "modelfile-resolvetype-undecltype",
        vec![("type", type_name.to_string()), ("context", context)],
    );
    err.model_file = Some(
        manager
            .model_file(namespace)
            .and_then(ModelFile::file_name)
            .map(str::to_string),
    );
    err.into()
}

#[cfg(test)]
mod tests;

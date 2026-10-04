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
//! `IllegalModelException` (`ClassDeclaration.validate` and its callees), so
//! every error here is an `ErrorKind::IllegalModel` contract error from the
//! error catalogue; a circular inheritance chain is one too (BC-11). A model
//! that validates cleanly returns `Ok(())`.

use crate::hash::{SeededHashMap, SeededHashSet};

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::declaration::{ClassDeclaration, Declaration, MapDeclaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::introspect::{DeclarationKind, Decorated, Typed, Validate};
use crate::model_manager::ModelManager;
use crate::model_util::{self, get_namespace, is_primitive_type, qualify, short_name};

/// A class's own AST `location`, re-serialised from its typed `mm::Range`,
/// for an error's `location` ([`Error::at`]).
fn class_location(class: &ClassDeclaration) -> Option<serde_json::Value> {
    class.location().and_then(crate::error::location_value)
}

/// A property's own AST `location` (TS: `this.ast.location` inside
/// `Property.validate`/`Decorated.validate`, where `this` is the property).
fn property_location(property: &Property) -> Option<serde_json::Value> {
    property.location().and_then(crate::error::location_value)
}

/// A typed AST `location`, re-serialised only when an error is raised.
fn lazy_location(range: Option<&mm::Range>) -> Option<serde_json::Value> {
    range.and_then(crate::error::location_value)
}

impl ModelManager {
    /// Validates every loaded model except the root model (`concerto@1.0.0`),
    /// returning the first problem found.
    ///
    /// TS: `validateModelFiles` (basemodelmanager.ts), in the order the files
    /// were added: a subclass also checks the properties it inherits, so the
    /// order decides which error comes first.
    pub fn validate_models(&self) -> Result<()> {
        self.validate_models_naming_file().map_err(|(_, err)| err)
    }

    js_compat_pub! {
        /// [`ModelManager::validate_models`], with the namespace of the model
        /// file the first problem was found in: TS `validateModelFiles`
        /// throws that file's own `validate()` error, which names the file,
        /// so a binding needs to know which one failed.
        pub fn validate_models_naming_file(&self) -> std::result::Result<(), (String, Error)> {
            // A file already known to be valid here is not validated again:
            // it would pass, so the first error found is the same.
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

    /// TS `ModelFile.validate()` (modelfile.ts), with `self` as the owning manager.
    ///
    /// **`model_file` must be the file `self` has registered under its
    /// namespace**, or its own types report as undeclared; otherwise use
    /// [`ModelManager::validate_detached_model_file`]. Checks run in TS order;
    /// the duplicate-class-name error carries no model file, as in TS.
    pub fn validate_model_file(&self, model_file: &ModelFile) -> Result<()> {
        self.validate_model_file_with_import_scope(model_file, self, None)
    }

    /// [`ModelManager::validate_model_file`], with `model_file`'s
    /// `getImports()` loop checked against `import_scope` rather than `self`:
    /// for [`ModelManager::validate_detached_model_file`]'s scratch branch,
    /// whose `check_imports` must see the manager as it stood before the
    /// file's namespace was registered. `hidden` names a namespace
    /// `import_scope` is taken not to hold.
    fn validate_model_file_with_import_scope(
        &self,
        model_file: &ModelFile,
        import_scope: &ModelManager,
        hidden: Option<&str>,
    ) -> Result<()> {
        let attach = |e| attach_model_file(e, model_file);
        check_decorators(self, model_file.namespace(), model_file, None, None).map_err(attach)?;
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
        /// TS `modelFile.validate()` for a file whose manager is `self` but which
        /// `self` may not have registered: validates against a scratch copy of
        /// `self` with `model_file` in place, leaving `self` unchanged. One
        /// divergence no oracle fixture reaches: a file an imported declaration
        /// reaches back into, in `model_file`'s namespace, is `model_file` here,
        /// where TS sees the file `self` holds.
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
            // `import_scope: self`, not `scratch` (doc comment above).
            scratch.validate_model_file_with_import_scope(registered, self, None)
        }
    }

    /// TS `BaseModelManager.addModelFile`'s validate-then-register, with the
    /// same checks, first error and result as validating then
    /// [`ModelManager::add_model_file`]; returns the new file's handle. On a
    /// validation error the manager is unchanged (caches aside) and the file is
    /// handed back with the error; a registration error consumes it.
    #[cfg(feature = "js-compat")]
    pub fn validate_and_add_model_file(
        &mut self,
        model_file: ModelFile,
    ) -> std::result::Result<crate::model_manager::ModelFileId, (Error, Option<Box<ModelFile>>)>
    {
        self.validate_and_add_shared_model_file(std::sync::Arc::new(model_file))
            .map_err(|(err, handed_back)| {
                let handed_back = handed_back.map(|shared| {
                    Box::new(
                        std::sync::Arc::try_unwrap(shared)
                            .unwrap_or_else(|shared| (*shared).clone()),
                    )
                });
                (err, handed_back)
            })
    }

    /// [`ModelManager::validate_and_add_model_file`] for a shared model file,
    /// registered as [`ModelManager::add_shared_model_file`] registers it.
    /// On a validation error the shared file is handed back with the error;
    /// a registration error consumes it.
    #[cfg(feature = "js-compat")]
    pub fn validate_and_add_shared_model_file(
        &mut self,
        shared: std::sync::Arc<ModelFile>,
    ) -> std::result::Result<
        crate::model_manager::ModelFileId,
        (Error, Option<std::sync::Arc<ModelFile>>),
    > {
        if let Some((id, mark)) = self.append_for_validation(&shared) {
            let namespace = shared.namespace();
            return match self.validate_model_file_with_import_scope(&shared, self, Some(namespace))
            {
                Ok(()) => {
                    // It passed with its own namespace hidden from its
                    // imports, so it passes `validate_models` too.
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

    /// TS `declaration.validate()` on one declaration of an unregistered
    /// `ModelFile` (as `MapDeclaration.validate`'s oracle fixtures call it):
    /// [`ModelManager::validate_detached_model_file`]'s scratch resolution,
    /// for the declaration at `index` only. A pre-port `IllegalModel` error
    /// if `model_file` has no declaration at `index` (a harness-only bound).
    #[cfg(feature = "js-compat")]
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

    /// [`ModelManager::validate_detached_declaration`], but for just the key
    /// half of the map declaration at `index` (TS `MapKeyType.validate`,
    /// called directly rather than through `MapDeclaration.validate`).
    #[cfg(feature = "js-compat")]
    pub fn validate_detached_map_key(&self, model_file: &ModelFile, index: usize) -> Result<()> {
        let (scratch, map) = self.detached_map(model_file, index)?;
        validate_map_key(&scratch, model_file.namespace(), &map)
    }

    /// [`ModelManager::validate_detached_map_key`], for the value half (TS
    /// `MapValueType.validate`).
    #[cfg(feature = "js-compat")]
    pub fn validate_detached_map_value(&self, model_file: &ModelFile, index: usize) -> Result<()> {
        let (scratch, map) = self.detached_map(model_file, index)?;
        validate_map_value(&scratch, model_file.namespace(), &map)
    }

    /// The scratch copy of `self` [`ModelManager::validate_detached_model_file`]
    /// builds, and `model_file`'s namespace, which callers borrow the
    /// declaration back out of the scratch copy by.
    #[cfg(feature = "js-compat")]
    fn detached_scratch(&self, model_file: &ModelFile) -> Result<(Self, String)> {
        let scratch = self.with_model_file_registered(std::sync::Arc::new(model_file.clone()))?;
        Ok((scratch, model_file.namespace().to_string()))
    }

    /// [`ModelManager::detached_scratch`], plus the `MapDeclaration` at
    /// `index`, cloned out so it can be validated against the scratch copy.
    #[cfg(feature = "js-compat")]
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

/// A harness-only bound: `index` names no `MapDeclaration` of `model_file`.
/// No TS class corresponds to it.
#[cfg(feature = "js-compat")]
fn no_such_detached_declaration(model_file: &ModelFile, index: usize) -> Error {
    Error::illegal_model(
        format!("no MapDeclaration at index {index}"),
        model_file.file_name().map(str::to_string),
        None,
    )
}

/// TS: `ModelFile.validate()`'s unique-names loop: the first declaration whose
/// fully-qualified name repeats an earlier one's throws `Duplicate class name
/// <fqn>`, with no model file and no location.
fn check_unique_declaration_names(model_file: &ModelFile) -> Result<()> {
    let mut seen = SeededHashSet::default();
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

/// Fills in the model file's name on an `IllegalModel` contract error that
/// does not carry one yet.
///
/// TS constructs every `IllegalModelException` raised while validating a
/// file's imports and declarations with that file, and the constructor
/// appends `File '<name>': ` when it has a name. A `model_file` already set
/// (as [`undeclared_type_error`] sets it) is left alone.
pub(crate) fn attach_model_file(mut err: Error, model_file: &ModelFile) -> Error {
    let contract = err.contract();
    if contract.model_file.is_none() && contract.kind == ErrorKind::IllegalModel {
        err.contract_mut().model_file = Some(model_file.file_name().map(str::to_string));
    }
    err
}

/// A declaration may not take the name of a type its file imports (its own
/// namespace included), unless the manager's
/// `dangerouslyAllowReservedSystemTypeNamesInUserModels` option is set and
/// the name resolves to one of the five reserved system declarations.
///
/// TS: `Declaration.validate` (declaration.ts), reached through every
/// declaration kind's `super.validate()` chain after its decorator checks.
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

/// TS: `Declaration.isReservedSystemTypeImport`: an imported `name` resolves
/// to a concept-like declaration of a system model file.
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
    /// Class-like and map declarations have their own checks; an enum's and
    /// a scalar's are their decorators (and an enum's values'), since loading
    /// runs every other scalar check.
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()> {
        match self {
            Declaration::Class(class) => class.validate(manager, namespace),
            Declaration::Map(map) => map.validate(manager, namespace),
            Declaration::Enum(enm) => {
                let fqn = qualify(namespace, enm.name());
                // TS: `EnumDeclaration` inherits `ClassDeclaration.validate`:
                // decorator and import-clash checks first.
                check_decorators(manager, namespace, enm, Some(&fqn), None)?;
                check_import_clash(manager, namespace, enm.name(), enm.location())?;
                // TS: `ClassDeclaration.validate`'s duplicate-field-name
                // check, between the decorator and per-value checks.
                check_unique_field_names(manager, enm.name(), None, &fqn)?;
                for value in enm.values() {
                    // The value's name is built only when a decorator check
                    // will read it.
                    let value_fqn = decorator_context(manager, value)
                        .then(|| format!("{fqn}.{}", value.name()));
                    check_decorators(manager, namespace, value, value_fqn.as_deref(), None)?;
                }
                Ok(())
            }
            Declaration::Scalar(scalar) => {
                let fqn = qualify(namespace, scalar.name());
                // TS: `ScalarDeclaration.validate` reaches
                // `Declaration.validate` (decorators, then the import-clash
                // check). Its duplicate-FQN scan is unreachable here:
                // `check_unique_declaration_names` has already run it.
                check_decorators(manager, namespace, scalar, Some(&fqn), None)?;
                check_import_clash(manager, namespace, scalar.name(), None)
            }
        }
    }
}

impl Validate for ClassDeclaration {
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()> {
        let fqn = qualify(namespace, self.name());
        // TS: `ClassDeclaration.validate`'s `super.validate()` runs the
        // decorator checks, then the import-clash check, before the
        // super-type block.
        check_decorators(manager, namespace, self, Some(&fqn), self.location())?;
        check_import_clash(manager, namespace, self.name(), self.location())?;
        check_super_type(manager, namespace, self)?;
        // TS: the `if (this.idField)` identity block runs before the
        // duplicate-name loop, so a redeclared identity is reported as such.
        check_identifier(manager, namespace, self)?;
        check_identity_matches_super(manager, namespace, self)?;
        check_unique_field_names(manager, self.name(), self.location(), &fqn)?;
        // TS: `for (field of this.getProperties())`: every property, own and
        // inherited, validated in this class's pass.
        for (owner_fqn, property) in manager.class_properties(&fqn)?.iter() {
            validate_property(manager, namespace, owner_fqn, property)?;
        }
        Ok(())
    }
}

/// TS: one iteration of `ClassDeclaration.validate`'s property loop for
/// `class`, over a property declared by `owner_fqn`. The field validates
/// against `class` when primitive or in `class`'s namespace, else against
/// its type's declaration, whose file resolves the name and is named in the
/// errors.
fn validate_property(
    manager: &ModelManager,
    namespace: &str,
    owner_fqn: &str,
    property: &Property,
) -> Result<()> {
    let owner_ns = get_namespace(Some(owner_fqn))?;
    // The property's own name is built only when a decorator check will
    // read it.
    let property_fqn =
        decorator_context(manager, property).then(|| format!("{owner_fqn}.{}", property.name()));
    // `field.getModelFile()`: the declaring file, for an inherited
    // property's own `Decorated.validate` errors.
    let owner_file = manager.model_file(owner_ns);
    let in_owner_file = |e: Error| match owner_file {
        Some(file) if owner_ns != namespace => attach_model_file(e, file),
        _ => e,
    };

    // TS: `Property.validate` runs `Decorated.validate` before its own
    // `resolveType` call (`check_property_type`).
    check_decorators(
        manager,
        owner_ns,
        property,
        property_fqn.as_deref(),
        property.location(),
    )
    .map_err(in_owner_file)?;

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
/// when decorator validation is enabled and `element` has a decorator (so
/// that a caller builds the context string only then).
fn decorator_context(manager: &ModelManager, element: &impl Decorated) -> bool {
    manager.decorator_validation().is_enabled() && !element.decorators().is_empty()
}

/// TS `Decorated.validate`: each decorator's own `.validate()`
/// ([`validate_decorators`]), then the duplicate-name scan
/// ([`check_unique_decorators`]), in that order. `context` is the element's
/// FQN for the decorator checks; `location` is where a duplicate is
/// reported.
fn check_decorators(
    manager: &ModelManager,
    namespace: &str,
    element: &impl Decorated,
    context: Option<&str>,
    location: Option<&mm::Range>,
) -> Result<()> {
    validate_decorators(manager, namespace, element, context)?;
    check_unique_decorators(element, location)
}

/// Runs [`crate::introspect::decorator::Decorator::validate`] over every
/// decorator an element carries, when `decoratorValidation` enables it.
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
    let mut seen = SeededHashSet::default();
    for decorator in element.decorators() {
        // TS keys its `Set` on `getName()` and interpolates it into the
        // message as is, so a decorator with no `name` at all is its own
        // entry and reads `undefined`.
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

/// The super type, if any (explicit or the implicit `Concept`), must not be
/// the class's own name (unless one of the five built-in kinds), must
/// resolve, and, unless it is a concept, must be the class's own kind.
///
/// TS: `ClassDeclaration.validate`'s super-type block, then
/// `_resolveSuperType`.
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

    // A super type that is not a concept must be the same kind. TS never
    // checks it is a `ClassDeclaration`, so an enum, scalar or map super
    // type fails here too, with the same message.
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

/// No field name may appear twice once inherited fields are included.
///
/// TS: `ClassDeclaration.validate`'s `uniquePropertyNames` loop, inherited by
/// `EnumDeclaration`, whose values are properties too.
fn check_unique_field_names(
    manager: &ModelManager,
    declaration_name: &str,
    location: Option<&mm::Range>,
    fqn: &str,
) -> Result<()> {
    // Seeded: the names come from user models (PORTING.md 3.7).
    let properties = manager.class_properties(fqn)?;
    let mut seen = SeededHashSet::default();
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
/// field typed as `String` or a String-based scalar, possibly inherited (TS:
/// `this.getProperty(this.idField)`).
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

    // TS checks the type first, then optionality. The field's type resolves
    // in the file that declares it.
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
/// relationship must target an identifiable class, never a primitive.
/// `namespace` is `classDecl.getModelFile()`'s, where the type name resolves
/// ([`validate_property`]); `owner_ns`/`owner`/`owner_fqn` name the declaring
/// class; `class` is the class whose pass this is.
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
        // TS: an empty type name is falsy, so `Property.validate` skips
        // `resolveType`; `RelationshipDeclaration.validate` then rejects a
        // relationship with no type, and any other property kind passes.
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
        // TS: `RelationshipDeclaration.validate`'s own message, with no owner
        // clause.
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
        // TS: `Property.validate` runs `resolveType` before any
        // relationship check, so an unresolvable type is this error for
        // every property kind.
        return Err(undeclared_type_error(
            manager,
            namespace,
            &type_identifier.name,
            format!("property {owner_fqn}.{}", property.name()),
        ));
    };

    let target = manager.get_declaration(&target_fqn).ok();
    if !is_primitive_type(&type_identifier.name) {
        // TS: the size-validator check runs after `resolveType`, before any
        // relationship check; a type `getType` cannot find is not a map.
        check_size_validator_target(
            owner_fqn,
            property,
            target.is_some_and(Declaration::is_map_declaration),
        )?;
    }
    // `RelationshipDeclaration.validate` looks its target up from the
    // declaring file, not from `classDecl`.
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
            // TS: "points to a missing type", when the declaration lookup
            // after a successful `resolveType` comes back empty. Unreached
            // through `validate_models`: an imported name is checked against
            // its namespace by `check_imports` first, and a local name comes
            // from the same declarations `get_declaration` reads.
            return Err(Error::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-missingtype",
                vec![("name", property.name().to_string()), ("type", target_fqn)],
            )
            .at(property_location(property)));
        }
        // Unreached in practice (`resolveType` just succeeded); a defensive
        // fallback in case `resolve` and `get_declaration` disagree, with the
        // error `resolveType` raises.
        return Err(undeclared_type_error(
            manager,
            namespace,
            &type_identifier.name,
            format!("property {owner_fqn}.{}", property.name()),
        ));
    };

    if property.is_relationship() {
        // TS: `classDeclaration.isIdentified()`, inherited.
        let identifiable =
            target.is_class_declaration() && manager.identifier_field(&target_fqn)?.is_some();
        if !identifiable {
            // TS: no owner clause, the target's fully-qualified name
            // appended.
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

/// TS: `Property.validate`'s size-validator check: a `sizeValidator` on a
/// non-array property is allowed only when its type is a map declaration.
/// The message names the property's fully-qualified name, with its
/// location.
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
    // TS keeps only a `TypeIdentifier`'s `name` and resolves it through the
    // declaring namespace's imports or local declarations, never qualifying
    // it with `ti.namespace` (which, for an aliased import, is the target's
    // while `name` is the alias). Callers build their own error from `None`.
    manager.resolve_type_name_at(namespace, name, None).ok()
}

/// TS `ModelFile.validate`'s loop over `getImports()`: the namespace must be
/// loaded, no earlier import may name another version of the same bare
/// namespace (`concerto` exempt), and the name must be declared there. As in
/// TS, [`model_util::parse_namespace`] runs before the loaded check, and no
/// `location` is set.
fn check_imports(
    manager: &ModelManager,
    hidden: Option<&str>,
    model_file: &ModelFile,
) -> Result<()> {
    // Each import's namespace and name are read in place; a fully-qualified
    // name is built only for an error, or where the plain split would not
    // give it back.
    type Borrowed<'a> = std::borrow::Cow<'a, str>;
    let mut seen_versions: SeededHashMap<Borrowed<'_>, Option<Borrowed<'_>>> =
        SeededHashMap::default();
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
            // Borrowed from the import, or copied on the rare path that built
            // the name.
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
    // TS: `if (this.idField)`: own identity only.
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
    // TS: `superType.isIdentified()`, inherited.
    let Some(super_id_field) = manager.identifier_field(&super_fqn)? else {
        return Ok(());
    };
    // TS: within `if (this.idField)`, `this.isSystemIdentified()` is whether
    // the own `idField` is `$identifier`.
    let this_system_identified = class.identifier_field_name().is_none();
    // TS: then `!superType.isSystemIdentified()` (inherited), or
    // `superType.isExplicitlyIdentified()` (the direct super type's own).
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
    /// Checks a map's key and value types, its decorators and its name
    /// against imports.
    ///
    /// TS: `MapDeclaration.validate` is `super.validate()` (decorators, then
    /// the import-clash check), then `this.key.validate()` and
    /// `this.value.validate()`.
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()> {
        let fqn = qualify(namespace, self.name());
        check_decorators(manager, namespace, self, Some(&fqn), None)?;
        check_import_clash(manager, namespace, self.name(), None)?;
        validate_map_key(manager, namespace, self)?;
        validate_map_value(manager, namespace, self)
    }
}

js_compat_pub! {
    /// `MapKeyType.validate` (src/introspect/mapkeytype.ts). The key-kind
    /// check TS makes at construction is the typed read's. Every error has
    /// no location: `MapDeclaration` does not keep its key's.
    pub fn validate_map_key(
        manager: &ModelManager,
        namespace: &str,
        map: &MapDeclaration,
    ) -> Result<()> {
        // An object key names a scalar over a String or DateTime.
        if let Some(key) = map.key_type() {
            let scalar = resolve(manager, namespace, &key.name)
                .and_then(|fqn| manager.get_declaration(&fqn).ok())
                .and_then(Typed::type_name);
            if !matches!(scalar, Some("String") | Some("DateTime")) {
                // TS throws with no `modelFile`, so no `File '<name>': `
                // suffix: `Some(None)` stops `attach_model_file` adding one.
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
    /// value-kind and "must contain property 'type'" checks are the typed
    /// read's.
    pub fn validate_map_value(
        manager: &ModelManager,
        namespace: &str,
        map: &MapDeclaration,
    ) -> Result<()> {
        // TS: `MapValueType.processType`: an object or relationship value's
        // `type.$class` must be `TypeIdentifier`, which the typed read keeps
        // as given.
        if let Some(t) = map.value_type()
            && t._class != crate::introspect::qualified_class("TypeIdentifier")
        {
            // TS names the value type `ObjectMapValueType` here for a
            // relationship value too (the template's own text).
            return Err(Error::new(ErrorKind::IllegalModel, "mapvaluetype-process-invalidtypeclass", vec![("name", map.name().to_string())]).at(None));
        }

        // TS: any declaration but a MapDeclaration is a valid value.
        if let Some(value) = map.value_type() {
            let declared = resolve(manager, namespace, &value.name)
                .and_then(|fqn| manager.get_declaration(&fqn).ok());
            let Some(declared) = declared else {
                // BC-12: an undeclared value type is an
                // `IllegalModelException` naming it, where TS 5.0.0 threw a
                // V8 `TypeError` (DV-014).
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

/// `modelfile-resolvetype-undecltype`: TS's `ModelFile.resolveType` error for
/// a type name that resolves through neither the primitives, an import, nor
/// a local declaration. `context` is TS's `context` argument verbatim; no
/// location. The error names `namespace`'s model file, as TS passes `this`.
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

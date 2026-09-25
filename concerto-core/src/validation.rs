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
//! `ConcertoError::Contract` with `ErrorKind::IllegalModel` — built through
//! [`ContractError::pre_port`] (`failed`, below) until a P2 task ports the
//! check's exact TS wording. A model that cannot be walked at all, such as
//! one whose inheritance is circular, surfaces the
//! [`ConcertoError::IllegalModel`] raised while resolving it. Note that TS
//! itself has no cycle check on this path and instead recurses until V8
//! overflows the stack (`RangeError`, PORTING.md section 2.5): this
//! pre-port cycle check is not yet a faithful port, and section 2.5 assigns
//! fixing it to the task that ports the recursion point it stands in for.
//! A model that validates cleanly returns `Ok(())`.

use std::collections::{HashMap, HashSet};

use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::declaration::{ClassDeclaration, Declaration, MapDeclaration};
use crate::introspect::model_file::ModelFile;
use crate::introspect::model_file::split_versioned_namespace;
use crate::introspect::property::Property;
use crate::introspect::{DeclarationKind, Decorated, Named, Typed, Validate};
use crate::model_manager::ModelManager;
use crate::model_util::{
    get_fully_qualified_name, get_namespace, get_short_name, is_primitive_type,
};

/// A class's own AST `location`, for [`failed`]'s `location` parameter
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

impl ModelManager {
    /// Validates every loaded model except the root model (`concerto@1.0.0`),
    /// so user models and the built-in decorator model are checked. Returns `Ok(())` if every model is semantically valid, otherwise
    /// the first problem found. Namespaces are visited in order so that the
    /// same set of models always reports the same problem.
    pub fn validate_models(&self) -> Result<()> {
        let mut model_files: Vec<_> = self
            .model_files()
            .filter(|model_file| !model_file.is_system_namespace())
            .collect();
        model_files.sort_by_key(|model_file| model_file.namespace());

        for model_file in model_files {
            self.validate_model_file(model_file)?;
        }
        Ok(())
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
    /// duplicate-class-name scan ([`check_unique_declaration_names`]):
    /// `ModelFile::from_json` accepts a second declaration of one name, as
    /// TS's constructor does, so this is where it is rejected; (4) each
    /// declaration, in file order — including, first thing, the
    /// import-clash check every declaration kind reaches through its own
    /// `super.validate()` chain ([`check_import_clash`]'s doc comment).
    ///
    /// Every error but step (3)'s names `model_file` as TS's does
    /// ([`attach_model_file`]); step (3)'s `IllegalModelException` is
    /// constructed with no model file at all in TS, so it has no `File
    /// '<name>'` suffix.
    pub fn validate_model_file(&self, model_file: &ModelFile) -> Result<()> {
        let attach = |e| attach_model_file(e, model_file);
        check_unique_decorators(model_file, None).map_err(attach)?;
        validate_decorators(self, model_file.namespace(), model_file, None).map_err(attach)?;
        check_imports(self, model_file).map_err(attach)?;
        check_unique_declaration_names(model_file)?;
        for declaration in model_file.declarations() {
            declaration
                .validate(self, model_file.namespace())
                .map_err(attach)?;
        }
        Ok(())
    }

    /// TS `modelFile.validate()` for a `ModelFile` whose `getModelManager()`
    /// is `self` but which `self` may never have registered — `new
    /// ModelFile(modelManager, ast)` followed directly by `validate()`, or
    /// `addModelFile`'s validate-before-register. When `self` already holds
    /// exactly this file (same AST, same file name) under its namespace,
    /// this is [`ModelManager::validate_model_file`] on that file. Otherwise
    /// it validates against a scratch copy of `self` with `model_file`
    /// registered in place of whatever `self` holds under its namespace
    /// ([`ModelManager::with_model_file_registered`]), so that the file's own
    /// local types resolve to itself, as TS's `this.getLocalType` does,
    /// while every import still resolves through the same files `self`
    /// holds. `self` itself is never changed (P2-08).
    ///
    /// One divergence remains, and no oracle fixture reaches it: a file
    /// *another* file's declarations reach back into during this pass (an
    /// imported super type whose own super type lives in `model_file`'s
    /// namespace) sees `model_file` here, where TS would see the file `self`
    /// actually holds under that namespace.
    pub fn validate_detached_model_file(&self, model_file: &ModelFile) -> Result<()> {
        let registered = self.model_file(model_file.namespace());
        if let Some(registered) = registered
            && registered.ast() == model_file.ast()
            && registered.file_name() == model_file.file_name()
        {
            return self.validate_model_file(registered);
        }
        let scratch = self.with_model_file_registered(model_file)?;
        let registered = scratch
            .model_file(model_file.namespace())
            .expect("with_model_file_registered registers the file under its namespace");
        scratch.validate_model_file(registered)
    }
}

/// TS: `ModelFile.validate()`'s "Check if names of the declarations are
/// unique" loop (modelfile.ts): the first declaration whose fully-qualified
/// name repeats an earlier one's throws an `IllegalModelException` whose
/// message is `Duplicate class name <fqn>` — built with no model file and no
/// location, so neither is set here (and [`ModelManager::validate_model_file`]
/// does not attach one).
fn check_unique_declaration_names(model_file: &ModelFile) -> Result<()> {
    let mut seen = HashSet::new();
    for declaration in model_file.declarations() {
        if !seen.insert(declaration.name()) {
            return Err(failed(
                format!(
                    "Duplicate class name {}",
                    get_fully_qualified_name(model_file.namespace(), declaration.name())
                ),
                None,
            ));
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
/// this module, which builds a bare [`ConcertoError`] through
/// [`failed`]/[`catalogue_error`] with no file in scope. A `model_file`
/// already set (as `undeclared_type_error` sets its own) is left alone, and
/// only `IllegalModel`-kind contract errors are touched.
fn attach_model_file(err: ConcertoError, model_file: &ModelFile) -> ConcertoError {
    match err {
        ConcertoError::Contract(mut contract)
            if contract.model_file.is_none() && contract.kind == ErrorKind::IllegalModel =>
        {
            contract.model_file = Some(model_file.file_name().map(str::to_string));
            ConcertoError::Contract(contract)
        }
        other => other,
    }
}

/// A declaration may not take the name of a type its file imports (including
/// implicitly, from its own namespace, which is caught the same way) —
/// unless the model manager's `dangerouslyAllowReservedSystemTypeNamesInUserModels`
/// escape hatch is set and the imported name resolves to one of the five
/// reserved system declarations.
///
/// TS: `Declaration.validate` (declaration.ts, "#648"), reached through every
/// subtype's own `super.validate()` chain — `ClassDeclaration.validate` (so
/// every concept-like declaration and, unchanged, `EnumDeclaration`) and
/// `ScalarDeclaration.validate` both call it first thing, before their own
/// checks (P2-08). `MapDeclaration` reaches it the same way in TS, but is
/// P2-06's territory; not called from here.
fn check_import_clash(
    manager: &ModelManager,
    namespace: &str,
    name: &str,
    location: Option<serde_json::Value>,
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
    Err(failed(
        format!("Type '{name}' clashes with an imported type with the same name."),
        location,
    ))
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
                let fqn = get_fully_qualified_name(namespace, enm.name());
                // TS: `EnumDeclaration` inherits `ClassDeclaration.validate`
                // unchanged, whose own `super.validate()` reaches
                // `Declaration.validate`'s decorator and import-clash checks
                // before anything class-specific (P2-08).
                check_unique_decorators(enm, None)?;
                validate_decorators(manager, namespace, enm, Some(&fqn))?;
                check_import_clash(manager, namespace, enm.name(), None)?;
                // TS: `ClassDeclaration.validate`'s duplicate-field-name
                // check, inherited unchanged by `EnumDeclaration` — run in
                // the same position relative to the decorator checks above
                // and the per-value checks below as TS's single `validate()`
                // body runs it relative to its own two neighbours (P2-04,
                // closing the "enum duplicate values" gap of plan §1.2).
                check_unique_field_names(manager, enm.name(), None, &fqn)?;
                for value in enm.values() {
                    check_unique_decorators(value, None)?;
                    validate_decorators(
                        manager,
                        namespace,
                        value,
                        Some(&format!("{fqn}.{}", value.name())),
                    )?;
                }
                Ok(())
            }
            Declaration::Scalar(scalar) => {
                let fqn = get_fully_qualified_name(namespace, scalar.name());
                // TS: `ScalarDeclaration.validate`'s `super.validate()` goes
                // straight to `Declaration.validate` (it extends
                // `Declaration`, not `ClassDeclaration`): decorators, then
                // the import-clash check (P2-08). Its own further check
                // (a duplicate-FQN scan over `getModelFile()
                // .getAllDeclarations()`) is unreachable on this pass:
                // `ModelFile.validate()` runs the same scan over the same
                // declarations before validating any of them
                // (`check_unique_declaration_names`), so a duplicate never
                // gets this far.
                check_unique_decorators(scalar, None)?;
                validate_decorators(manager, namespace, scalar, Some(&fqn))?;
                check_import_clash(manager, namespace, scalar.name(), None)
            }
        }
    }
}

impl Validate for ClassDeclaration {
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()> {
        // TS: `ClassDeclaration.validate`'s `super.validate()`
        // (classdeclaration.ts) reaches `Declaration.validate`'s import-clash
        // check ([`check_import_clash`]'s doc comment) before this method's
        // own super-type block (P2-08).
        check_import_clash(manager, namespace, self.name(), class_location(self))?;
        check_super_type(manager, namespace, self)?;
        let fqn = get_fully_qualified_name(namespace, self.name());
        check_unique_field_names(manager, self.name(), class_location(self), &fqn)?;
        check_identifier(manager, namespace, self)?;
        check_identity_matches_super(manager, namespace, self)?;
        check_unique_decorators(self, class_location(self))?;
        validate_decorators(manager, namespace, self, Some(&fqn))?;
        for property in self.own_properties() {
            // TS: `Property.validate` runs `super.validate()` — the
            // `Decorated` duplicate-decorator check and, when enabled,
            // `Decorator.validate` — before its own `resolveType` call
            // (property.ts). `check_property_type` below is that
            // `resolveType`/relationship logic, so the decorator checks run
            // first here too (P2-08 review carry-over (b) from P2-04's
            // review, #48).
            check_unique_decorators(property, property_location(property))?;
            validate_decorators(
                manager,
                namespace,
                property,
                Some(&format!("{fqn}.{}", property.name())),
            )?;
            check_property_type(manager, namespace, self, property)?;
        }
        Ok(())
    }
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
    for decorator in element.get_decorators() {
        decorator.validate(manager, namespace, context)?;
    }
    Ok(())
}

/// An element may not carry the same decorator twice.
fn check_unique_decorators(
    element: &impl Decorated,
    location: Option<serde_json::Value>,
) -> Result<()> {
    let mut seen = HashSet::new();
    for decorator in element.get_decorators() {
        if !seen.insert(decorator.name()) {
            return Err(failed(
                format!("Duplicate decorator {}", decorator.name()),
                location,
            ));
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
        return Err(catalogue_error(
            "classdeclaration-validate-selfextending",
            vec![("class", class.name().to_string())],
            class_location(class),
        ));
    }

    let super_declaration = resolve(manager, namespace, &super_type.name)
        .and_then(|fqn| manager.get_declaration(&fqn).ok());
    let Some(super_declaration) = super_declaration else {
        // TS: `_resolveSuperType`'s hardcoded string, not a catalogue
        // template (Globalize is never called on this path).
        return Err(failed(
            format!("Could not find super type {}", super_type.name),
            class_location(class),
        ));
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
        return Err(failed(
            format!(
                "{} ({}) cannot extend {} ({})",
                class.declaration_kind(),
                class.name(),
                super_declaration.declaration_kind(),
                super_declaration.name()
            ),
            class_location(class),
        ));
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
    location: Option<serde_json::Value>,
    fqn: &str,
) -> Result<()> {
    let mut seen = HashSet::new();
    for (_, property) in manager.get_all_properties(fqn)? {
        if !seen.insert(property.name().to_string()) {
            return Err(catalogue_error(
                "classdeclaration-validate-duplicatefieldname",
                vec![
                    ("class", declaration_name.to_string()),
                    ("fieldName", property.name().to_string()),
                ],
                location,
            ));
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
    let fqn = get_fully_qualified_name(namespace, class.name());
    let (owner, field) = manager
        .get_all_properties(&fqn)?
        .into_iter()
        .find(|(_, property)| property.name() == field_name)
        .ok_or_else(|| {
            catalogue_error(
                "classdeclaration-validate-identifiernotproperty",
                vec![
                    ("class", class.name().to_string()),
                    ("idField", field_name.to_string()),
                ],
                class_location(class),
            )
        })?;

    // TS checks the type first, then optionality (classdeclaration.ts
    // `validate`), so an optional non-String identifier reports the type.
    // TS: `idField.getParent().getModelFile().getType(idField.getType())`
    // resolves the field's type in the file that *declares* the field, which
    // for an inherited identifier is the super type's, not this class's.
    let owner_namespace = get_namespace(Some(&owner))?;
    if !is_string_typed(manager, owner_namespace, &field) {
        return Err(catalogue_error(
            "classdeclaration-validate-identifiernotstring",
            vec![
                ("class", class.name().to_string()),
                ("idField", field_name.to_string()),
            ],
            class_location(class),
        ));
    }
    // TS: hardcoded, not a catalogue template (Globalize is never called on
    // this path).
    if field.is_optional() {
        return Err(failed(
            "Identifying fields cannot be optional.".to_string(),
            class_location(class),
        ));
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
fn check_property_type(
    manager: &ModelManager,
    namespace: &str,
    class: &ClassDeclaration,
    property: &Property,
) -> Result<()> {
    let owner = class.name();
    let Some(type_identifier) = property.type_identifier() else {
        // A primitive field: TS's `resolveType` of a primitive always
        // succeeds, so the size-validator check is all that is left.
        return check_size_validator_target(namespace, class, property, false);
    };

    if is_primitive_type(&type_identifier.name) {
        // TS: `resolveType` succeeds for a primitive, then `Property.validate`
        // runs its size-validator check (a primitive is never a map), all
        // before `RelationshipDeclaration.validate`'s own checks.
        check_size_validator_target(namespace, class, property, false)?;
    }

    if property.is_relationship() && is_primitive_type(&type_identifier.name) {
        // TS: RelationshipDeclaration.validate's own hardcoded message
        // (src/introspect/relationshipdeclaration.ts): `'Relationship ' +
        // this.getName() + ' cannot be to the primitive type ' +
        // this.getType()` — no owner clause.
        return Err(failed(
            format!(
                "Relationship {} cannot be to the primitive type {}",
                property.name(),
                type_identifier.name
            ),
            property_location(property),
        ));
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
            format!(
                "property {}.{}",
                get_fully_qualified_name(namespace, owner),
                property.name()
            ),
        ));
    };

    let target = manager.get_declaration(&target_fqn).ok();
    if !is_primitive_type(&type_identifier.name) {
        // TS: `Property.validate`'s size-validator check runs right after
        // `resolveType` succeeds and before any relationship-specific check;
        // a type that `getType` cannot find (swallowed by its try/catch)
        // counts as not a map.
        check_size_validator_target(
            namespace,
            class,
            property,
            target.is_some_and(Declaration::is_map_declaration),
        )?;
    }
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
            return Err(failed(
                format!(
                    "Relationship {} points to a missing type {}",
                    property.name(),
                    target_fqn
                ),
                property_location(property),
            ));
        }
        // Not yet observed for a non-relationship property: TS's own
        // `Property.validate` has no declaration lookup beyond `resolveType`
        // above, which just succeeded, so this is unreached in practice. Kept
        // as a defensive pre-port fallback rather than an `unreachable!`,
        // since `resolve` and `get_declaration` are still two separate Rust
        // lookups that could in principle disagree.
        return Err(failed(
            format!(
                "Undeclared type {} referenced by {}.{}",
                type_identifier.name,
                owner,
                property.name()
            ),
            class_location(class),
        ));
    };

    if property.is_relationship() {
        // TS: `classDeclaration.isIdentified()` (RelationshipDeclaration.validate,
        // src/introspect/relationshipdeclaration.ts) — inherited, so a
        // target that has no identity of its own but extends one that does
        // (every `Asset`/`Participant`, for one) still counts.
        let identifiable =
            target.is_class_declaration() && manager.identifier_field_name(&target_fqn)?.is_some();
        if !identifiable {
            // TS: `'Relationship ' + this.getName() + ' must be to a class
            // that has an identifier, but this is to ' +
            // this.getFullyQualifiedTypeName()` — no owner clause, and with
            // the target's own fully-qualified name appended.
            return Err(failed(
                format!(
                    "Relationship {} must be to a class that has an identifier, but this is to {}",
                    property.name(),
                    target_fqn
                ),
                property_location(property),
            ));
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
    namespace: &str,
    class: &ClassDeclaration,
    property: &Property,
    is_map_type: bool,
) -> Result<()> {
    if property.size_validator().is_some() && !property.is_array() && !is_map_type {
        return Err(failed(
            format!(
                "size validator can only be applied to array or map properties: {}.{}",
                get_fully_qualified_name(namespace, class.name()),
                property.name()
            ),
            property_location(property),
        ));
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
    // mean "does not resolve" and build their own message (`failed`,
    // below), so the location `resolve_type_name` would attach to its own
    // error never surfaces. Passing `None` here is exact, not a shortcut.
    manager.resolve_type_name(namespace, name, None).ok()
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
fn check_imports(manager: &ModelManager, model_file: &ModelFile) -> Result<()> {
    let mut seen_versions: HashMap<String, Option<String>> = HashMap::new();
    for import_fqn in model_file.get_imports() {
        let import_namespace = get_namespace(Some(&import_fqn))?;
        let import_short_name = get_short_name(&import_fqn);

        if manager.model_file(import_namespace).is_none() {
            return Err(catalogue_error(
                "modelmanager-gettype-noregisteredns",
                vec![("type", import_fqn.clone())],
                None,
            ));
        }

        let (name, import_version) = split_versioned_namespace(import_namespace)?;
        let is_global_model = name == "concerto";
        if let Some(existing) = seen_versions.get(&name)
            && *existing != Some(import_version.clone())
            && !is_global_model
        {
            return Err(catalogue_error(
                "modelmanager-gettype-duplicatensimport",
                vec![
                    ("namespace", import_namespace.to_string()),
                    ("version1", existing.clone().unwrap_or_default()),
                    ("version2", import_version.clone()),
                ],
                None,
            ));
        }
        seen_versions.insert(name, Some(import_version));

        let source_file = manager
            .model_file(import_namespace)
            .expect("checked registered above");
        if !source_file.is_local_type(import_short_name) {
            return Err(catalogue_error(
                "modelmanager-gettype-notypeinns",
                vec![
                    ("type", import_short_name.to_string()),
                    ("namespace", import_namespace.to_string()),
                ],
                None,
            ));
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
    let Some(super_id_field) = manager.identifier_field_name(&super_fqn)? else {
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
        return Err(failed(
            format!(
                "Super class {super_fqn} has an explicit identifier {super_id_field} that cannot be redeclared."
            ),
            class_location(class),
        ));
    }
    Ok(())
}

/// The key kinds the specification allows: a `String` or `DateTime`, or an
/// object key naming a scalar over one of those.
const MAP_KEY_KINDS: &[&str] = &["StringMapKeyType", "DateTimeMapKeyType", "ObjectMapKeyType"];

/// The value kinds the specification allows: any primitive, or an object value
/// naming a scalar or a concept. A relationship is not among them.
const MAP_VALUE_KINDS: &[&str] = &[
    "BooleanMapValueType",
    "DateTimeMapValueType",
    "DoubleMapValueType",
    "IntegerMapValueType",
    "LongMapValueType",
    "StringMapValueType",
    "ObjectMapValueType",
];

impl Validate for MapDeclaration {
    /// Checks a map against the key and value types the specification
    /// permits, and, like every other declaration (P2-07), that it carries no
    /// duplicate decorator and that its decorators pass `decoratorValidation`
    /// when enabled.
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()> {
        check_map_types(manager, namespace, self)?;
        let fqn = get_fully_qualified_name(namespace, self.name());
        check_unique_decorators(self, None)?;
        validate_decorators(manager, namespace, self, Some(&fqn))
    }
}

/// Every error in `check_map_types` passes `location: None`: `MapDeclaration`'s
/// own doc comment records that its `location` (and its key's and value's) is
/// deliberately not read, so there is no AST node to copy from, not a gap
/// left for later.
fn check_map_types(manager: &ModelManager, namespace: &str, map: &MapDeclaration) -> Result<()> {
    if !MAP_KEY_KINDS.contains(&map.key_kind()) {
        return Err(failed(
            format!(
                "The key of map {} must be a String or DateTime, or a scalar over one of them",
                map.name()
            ),
            None,
        ));
    }
    if !MAP_VALUE_KINDS.contains(&map.value_kind()) {
        return Err(failed(
            format!(
                "The value of map {} may not be a {}",
                map.name(),
                map.value_kind()
            ),
            None,
        ));
    }

    // An object key names a scalar, which has to be over a String or DateTime.
    if let Some(key) = map.key_type() {
        let scalar = resolve(manager, namespace, &key.name)
            .and_then(|fqn| manager.get_declaration(&fqn).ok())
            .and_then(Typed::type_name);
        if !matches!(scalar, Some("String") | Some("DateTime")) {
            return Err(failed(
                format!(
                    "The key of map {} must be a String or DateTime, or a scalar over one of them",
                    map.name()
                ),
                None,
            ));
        }
    }

    // An object value names a concept or a scalar, and it has to be declared.
    if let Some(value) = map.value_type() {
        let declared = resolve(manager, namespace, &value.name)
            .and_then(|fqn| manager.get_declaration(&fqn).ok());
        let Some(declared) = declared else {
            return Err(failed(
                format!(
                    "Undeclared type {} referenced by the value of map {}",
                    value.name,
                    map.name()
                ),
                None,
            ));
        };
        if !declared.is_class_declaration() && !declared.is_scalar_declaration() {
            return Err(failed(
                format!(
                    "The value of map {} must be a concept or a scalar, and {} is neither",
                    map.name(),
                    value.name
                ),
                None,
            ));
        }
    }
    Ok(())
}

/// Builds a semantic-validation error from a hand-written message.
///
/// TS: `ClassDeclaration.validate` and its callees throw
/// `IllegalModelException` for every one of these checks (section 2.3), so
/// `kind` is `IllegalModel`. The message text itself is not yet a faithful
/// port of the TS wording (that is P2-01/P2-03/P2-08's job, one class at a
/// time, PORTING.md section 7.2), so it is built with
/// [`ContractError::pre_port`] rather than a catalogue code.
///
/// `location` is the AST node's `location`, copied verbatim, exactly as
/// every real TS throw on this path passes `this.ast.location` (PORTING.md
/// 2.1); callers pass their class's own [`class_location`], or `None` where
/// no class-like declaration is in scope (2.2's rule that `location` is
/// `None` exactly where TS passes none does not yet apply to every check
/// here, since the check itself is still pre-port; `None` is a placeholder
/// there too, not a claim that TS passes none).
fn failed(message: String, location: Option<serde_json::Value>) -> ConcertoError {
    ContractError::pre_port(ErrorKind::IllegalModel, message, location).into()
}

/// [`failed`], but through a real message-catalogue entry (PORTING.md 2.1,
/// 2.2) instead of the `pre-port` stand-in: `code` is a catalogue key whose
/// template TS builds with `Globalize(...).messageFormatter(code)(params)`,
/// so this is used only for a check whose TS raises through Globalize, never
/// for one of TS's own hardcoded strings (those stay on [`failed`]).
fn catalogue_error(
    code: &'static str,
    params: Vec<(&'static str, String)>,
    location: Option<serde_json::Value>,
) -> ConcertoError {
    let mut err = ContractError::new(ErrorKind::IllegalModel, code, params);
    err.location = location;
    err.into()
}

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
) -> ConcertoError {
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
mod tests {
    use crate::error::ConcertoError;
    use crate::introspect::Named;
    use crate::model_manager::ModelManager;

    /// Loads `org.example@1.0.0` with the given declarations and validates it.
    fn validate(declarations: serde_json::Value) -> crate::error::Result<()> {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.example@1.0.0",
                    "declarations": declarations
                }),
                None,
            )
            .unwrap();
        manager.validate_models()
    }

    fn concept(body: serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "isAbstract": false,
            "properties": []
        });
        v.as_object_mut()
            .unwrap()
            .extend(body.as_object().unwrap().clone());
        v
    }

    #[test]
    fn super_type_that_exists_passes() {
        let err = validate(serde_json::json!([
            concept(serde_json::json!({ "name": "Person" })),
            concept(serde_json::json!({
                "name": "Employee",
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" }
            }))
        ]));
        assert!(err.is_ok());
    }

    #[test]
    fn super_type_that_is_missing_fails() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Employee",
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Ghost" }
        }))]));
        assert!(err.unwrap_err().to_string().contains("super type"));
    }

    /// PORTING.md 2.1: `failed`'s `location` is the failing class's own AST
    /// `location`, copied verbatim, not hard-coded to `None` (P1-05 exit
    /// condition).
    #[test]
    fn super_type_that_is_missing_carries_the_class_ast_location() {
        let location = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Range",
            "start": {"$class": "concerto.metamodel@1.0.0.Position", "line": 3, "column": 1, "offset": 20},
            "end": {"$class": "concerto.metamodel@1.0.0.Position", "line": 3, "column": 9, "offset": 28}
        });
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Employee",
            "location": location,
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Ghost" }
        }))]))
        .unwrap_err();
        match err {
            ConcertoError::Contract(contract) => assert_eq!(contract.location, Some(location)),
            other => panic!("expected a Contract error, got {other:?}"),
        }
    }

    #[test]
    fn field_redeclared_from_super_type_fails() {
        let err = validate(serde_json::json!([
            concept(serde_json::json!({
                "name": "Person",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id", "isArray": false, "isOptional": false }
                ]
            })),
            concept(serde_json::json!({
                "name": "Employee",
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id", "isArray": false, "isOptional": false }
                ]
            }))
        ]));
        assert!(err.unwrap_err().to_string().contains("more than one field"));
    }

    #[test]
    fn unique_field_names_across_inheritance_pass() {
        let err = validate(serde_json::json!([
            concept(serde_json::json!({
                "name": "Person",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                ]
            })),
            concept(serde_json::json!({
                "name": "Employee",
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "salary", "isArray": false, "isOptional": false }
                ]
            }))
        ]));
        assert!(err.is_ok());
    }

    #[test]
    fn relationship_to_primitive_fails() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Order",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "total",
                  "isArray": false, "isOptional": false,
                  "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Double" } }
            ]
        }))]));
        // TS (relationshipdeclaration.ts): `'Relationship ' + this.getName()
        // + ' cannot be to the primitive type ' + this.getType()` — no
        // owner clause.
        assert_eq!(
            err.unwrap_err().to_string(),
            "Relationship total cannot be to the primitive type Double"
        );
    }

    #[test]
    fn relationship_to_unidentified_class_fails() {
        let err = validate(serde_json::json!([
            concept(serde_json::json!({ "name": "Address" })),
            concept(serde_json::json!({
                "name": "Order",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "shipTo",
                      "isArray": false, "isOptional": false,
                      "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Address" } }
                ]
            }))
        ]));
        // TS: `'Relationship ' + this.getName() + ' must be to a class that
        // has an identifier, but this is to ' +
        // this.getFullyQualifiedTypeName()` — no owner clause, with the
        // target's own fully-qualified name appended.
        assert_eq!(
            err.unwrap_err().to_string(),
            "Relationship shipTo must be to a class that has an identifier, \
             but this is to org.example@1.0.0.Address"
        );
    }

    #[test]
    fn relationship_to_identified_class_passes() {
        // A concept (not an asset/participant/transaction/event): the
        // implicit super type here is `Concept` itself (no properties of its
        // own to collide with), unlike the four identified kinds, which
        // implicitly extend their own system kind (P2-03) and so would
        // already carry a `$identifier` of their own — this test is about
        // the relationship check, not that.
        let err = validate(serde_json::json!([
            concept(serde_json::json!({
                "name": "Vehicle",
                "identified": { "$class": "concerto.metamodel@1.0.0.Identified" },
            })),
            concept(serde_json::json!({
                "name": "Order",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "car",
                      "isArray": false, "isOptional": false,
                      "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Vehicle" } }
                ]
            }))
        ]));
        assert!(err.is_ok());
    }

    /// TS: `RelationshipDeclaration.validate` calls `super.validate(classDecl)`
    /// first (relationshipdeclaration.ts), which is `Property.validate`'s own
    /// `classDecl.getModelFile().resolveType('property ' +
    /// this.getFullyQualifiedName(), this.type)` (property.ts) — the
    /// structural check every property kind shares. `Ghost` resolves through
    /// neither an import nor a local declaration, so `resolveType` throws the
    /// catalogue's `modelfile-resolvetype-undecltype` message
    /// (modelfile.ts) before `RelationshipDeclaration`'s own "points to a
    /// missing type" check (which needs a type that *did* resolve) ever
    /// runs. Evidence: conformance fixture
    /// `concepts/models/RELATIONSHIP_002/relationship_002_type_not_exist.cto`
    /// (oracle id `0708cadc678cefd55e7c5e12`, `ModelManager.addCTOModel`) —
    /// this was a P2-04 review blocker: the branch below used to fire the
    /// relationship-specific message for this unresolvable case too.
    #[test]
    fn relationship_to_unresolvable_type_fails_with_the_undeclared_type_message() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Order",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "ship",
                  "isArray": false, "isOptional": false,
                  "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Ghost" } }
            ]
        }))]));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Undeclared type \"Ghost\" in \"property org.example@1.0.0.Order.ship\"."
        );
    }

    #[test]
    fn object_property_of_undeclared_type_fails() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Order",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "line",
                  "isArray": false, "isOptional": false,
                  "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "LineItem" } }
            ]
        }))]));
        // Same shared `Property.validate` → `resolveType` check as the
        // relationship case above (property.ts, modelfile.ts): a
        // non-relationship property gets the identical catalogue message.
        assert_eq!(
            err.unwrap_err().to_string(),
            "Undeclared type \"LineItem\" in \"property org.example@1.0.0.Order.line\"."
        );
    }

    #[test]
    fn object_property_of_declared_type_passes() {
        let err = validate(serde_json::json!([
            concept(serde_json::json!({ "name": "LineItem" })),
            concept(serde_json::json!({
                "name": "Order",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "line",
                      "isArray": false, "isOptional": false,
                      "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "LineItem" } }
                ]
            }))
        ]));
        assert!(err.is_ok());
    }

    #[test]
    fn duplicate_decorator_is_rejected() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Product",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "productId",
                  "isArray": false, "isOptional": false,
                  "decorators": [
                    { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "custom", "arguments": [] },
                    { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "custom", "arguments": [] }
                  ] }
            ]
        }))]));
        assert!(err.unwrap_err().to_string().contains("Duplicate decorator"));
    }

    #[test]
    fn distinct_decorators_are_accepted() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Product",
            "decorators": [
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "one", "arguments": [] },
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "two", "arguments": [] }
            ],
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "productId",
                  "isArray": false, "isOptional": false,
                  "decorators": [
                    { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "custom", "arguments": [] }
                  ] }
            ]
        }))]));
        assert!(err.is_ok());
    }

    /// P2-07: duplicate decorators are now caught on an enum declaration too,
    /// not only on class-like declarations and their properties.
    #[test]
    fn duplicate_decorator_on_an_enum_declaration_is_rejected() {
        let err = validate(serde_json::json!([{
            "$class": "concerto.metamodel@1.0.0.EnumDeclaration",
            "name": "Colour",
            "decorators": [
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] },
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] }
            ],
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" }
            ]
        }]));
        assert!(err.unwrap_err().to_string().contains("Duplicate decorator"));
    }

    /// P2-07: a scalar's own decorators are checked too, matching TS
    /// `ScalarDeclaration.validate` running `Decorated.validate` via
    /// `super.validate()`.
    #[test]
    fn duplicate_decorator_on_a_scalar_declaration_is_rejected() {
        let err = validate(serde_json::json!([{
            "$class": "concerto.metamodel@1.0.0.StringScalar",
            "name": "Email",
            "decorators": [
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] },
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] }
            ]
        }]));
        assert!(err.unwrap_err().to_string().contains("Duplicate decorator"));
    }

    /// P2-07: a map's own decorators are checked too, matching TS
    /// `MapDeclaration.validate` running `Decorated.validate` via
    /// `super.validate()`.
    #[test]
    fn duplicate_decorator_on_a_map_declaration_is_rejected() {
        let err = validate(serde_json::json!([{
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "Lookup",
            "decorators": [
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] },
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] }
            ],
            "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
            "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" }
        }]));
        assert!(err.unwrap_err().to_string().contains("Duplicate decorator"));
    }

    /// P2-07: an enum value's own decorators are checked, matching the doc
    /// comment on `impl Validate for Declaration` that an enum's checks
    /// include "those of its values".
    #[test]
    fn duplicate_decorator_on_an_enum_value_is_rejected() {
        let err = validate(serde_json::json!([{
            "$class": "concerto.metamodel@1.0.0.EnumDeclaration",
            "name": "Colour",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED",
                  "decorators": [
                    { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "hex", "arguments": [] },
                    { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "hex", "arguments": [] }
                  ] }
            ]
        }]));
        assert!(err.unwrap_err().to_string().contains("Duplicate decorator"));
    }

    /// P2-07: a model file's own decorators (on its `namespace`) are checked
    /// too, matching TS `ModelFile.validate` running `Decorated.validate` via
    /// `super.validate()` (modelfile.ts).
    #[test]
    fn duplicate_decorator_on_a_namespace_is_rejected() {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.example@1.0.0",
                    "decorators": [
                        { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] },
                        { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] }
                    ],
                    "declarations": []
                }),
                None,
            )
            .unwrap();
        assert!(
            manager
                .validate_models()
                .unwrap_err()
                .to_string()
                .contains("Duplicate decorator")
        );
    }

    #[test]
    fn duplicate_decorator_on_a_declaration_is_rejected() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Product",
            "decorators": [
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] },
                { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "tag", "arguments": [] }
            ],
            "properties": []
        }))]));
        assert!(err.unwrap_err().to_string().contains("Duplicate decorator"));
    }

    /// Loads `org.example@1.0.0` with the given imports and declarations.
    fn validate_with_imports(
        imports: serde_json::Value,
        declarations: serde_json::Value,
    ) -> crate::error::Result<()> {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.common@1.0.0",
                    "declarations": [concept(serde_json::json!({ "name": "Address" }))]
                }),
                None,
            )
            .unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.example@1.0.0",
                    "imports": imports,
                    "declarations": declarations
                }),
                None,
            )
            .unwrap();
        manager.validate_models()
    }

    /// `org.a@1.0.0` declares `scalar SSN extends String` and `abstract
    /// concept Person { o SSN id }`; `org.b@1.0.0` imports only `Person` and
    /// declares `concept Emp identified by id extends Person {}`, plus
    /// `extra` (e.g. a shadowing `SSN` of its own). Validates both.
    fn validate_inherited_scalar_identifier(extra: serde_json::Value) -> crate::error::Result<()> {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.a@1.0.0",
                    "declarations": [
                        { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "SSN" },
                        concept(serde_json::json!({
                            "name": "Person",
                            "isAbstract": true,
                            "properties": [
                                { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "id",
                                  "isArray": false, "isOptional": false,
                                  "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "SSN" } }
                            ]
                        }))
                    ]
                }),
                None,
            )
            .unwrap();
        let mut declarations = vec![concept(serde_json::json!({
            "name": "Emp",
            "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" },
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" }
        }))];
        declarations.extend(extra.as_array().unwrap().iter().cloned());
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.b@1.0.0",
                    "imports": [
                        { "$class": "concerto.metamodel@1.0.0.ImportType",
                          "namespace": "org.a@1.0.0", "name": "Person" }
                    ],
                    "declarations": declarations
                }),
                None,
            )
            .unwrap();
        manager.validate_models()
    }

    /// TS resolves an inherited identifier's type in the file that declares
    /// the field (`idField.getParent().getModelFile()`,
    /// classdeclaration.ts), so a String scalar the subclass's own file never
    /// imports still counts.
    #[test]
    fn inherited_identifier_scalar_resolves_in_the_declaring_namespace() {
        let result = validate_inherited_scalar_identifier(serde_json::json!([]));
        assert!(result.is_ok(), "{result:?}");
    }

    /// A same-named scalar over `Integer` in the subclass's own namespace is
    /// not the one the inherited field names, so it does not make the
    /// identifier non-String.
    #[test]
    fn inherited_identifier_scalar_ignores_a_shadowing_scalar_in_the_subclass_namespace() {
        let result = validate_inherited_scalar_identifier(serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.IntegerScalar", "name": "SSN" }
        ]));
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn declaration_clashing_with_an_imported_name_is_rejected() {
        let err = validate_with_imports(
            serde_json::json!([
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.common@1.0.0", "name": "Address" }
            ]),
            serde_json::json!([concept(serde_json::json!({ "name": "Address" }))]),
        );
        // TS: test/introspect/modelfile.js, the exact
        // `IllegalModelException` message `Declaration.validate` throws.
        assert_eq!(
            err.unwrap_err().to_string(),
            "Type 'Address' clashes with an imported type with the same name."
        );
    }

    /// TS: test/introspect/modelfile.js "should recognise a user-space type
    /// with the same name as a Prototype" — every non-system model file
    /// implicitly imports `Concept`/`Asset`/`Transaction`/`Participant`/
    /// `Event` from the system namespace (`ModelFile::from_json`'s built-in
    /// import), so a plain user concept named `Transaction`, with no import
    /// of its own, still clashes.
    #[test]
    fn a_user_declaration_clashes_with_the_implicitly_imported_system_types() {
        let err = validate(serde_json::json!([concept(
            serde_json::json!({ "name": "Transaction" })
        )]));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Type 'Transaction' clashes with an imported type with the same name."
        );
    }

    /// TS: test/introspect/modelfile.js "should allow a system type name
    /// when dangerouslyAllowReservedSystemTypeNamesInUserModels is enabled".
    #[test]
    fn dangerously_allow_reserved_system_type_names_permits_a_system_name_clash() {
        let mut manager = ModelManager::new().unwrap();
        manager.set_dangerously_allow_reserved_system_type_names_in_user_models(true);
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "A@1.0.0",
                    "declarations": [{
                        "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                        "name": "Asset",
                        "isAbstract": false,
                        "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "assetId" },
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "assetId",
                              "isArray": false, "isOptional": false }
                        ]
                    }]
                }),
                None,
            )
            .unwrap();
        assert!(manager.validate_models().is_ok());
    }

    /// TS: test/introspect/modelfile.js "should still fail non-system
    /// clashes when dangerouslyAllowReservedSystemTypeNamesInUserModels is
    /// enabled" — the escape hatch only bypasses a clash with a *system*
    /// declaration; an ordinary imported user type still clashes.
    #[test]
    fn dangerously_allow_reserved_system_type_names_does_not_permit_other_clashes() {
        let mut manager = ModelManager::new().unwrap();
        manager.set_dangerously_allow_reserved_system_type_names_in_user_models(true);
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "A@1.0.0",
                    "declarations": [concept(serde_json::json!({
                        "name": "B",
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name",
                              "isArray": false, "isOptional": false }
                        ]
                    }))]
                }),
                None,
            )
            .unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "B@1.0.0",
                    "imports": [
                        { "$class": "concerto.metamodel@1.0.0.ImportType",
                          "namespace": "A@1.0.0", "name": "B" }
                    ],
                    "declarations": [concept(serde_json::json!({ "name": "B" }))]
                }),
                None,
            )
            .unwrap();
        assert_eq!(
            manager.validate_models().unwrap_err().to_string(),
            "Type 'B' clashes with an imported type with the same name."
        );
    }

    #[test]
    fn declaration_beside_a_distinct_import_is_accepted() {
        let err = validate_with_imports(
            serde_json::json!([
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.common@1.0.0", "name": "Address" }
            ]),
            serde_json::json!([concept(serde_json::json!({ "name": "Person" }))]),
        );
        assert!(err.is_ok());
    }

    #[test]
    fn importing_from_the_files_own_namespace_is_rejected() {
        // A self-import makes the local declaration clash with itself.
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.example@1.0.0",
                    "imports": [
                        { "$class": "concerto.metamodel@1.0.0.ImportType",
                          "namespace": "org.example@1.0.0", "name": "LocalType" }
                    ],
                    "declarations": [concept(serde_json::json!({ "name": "LocalType" }))]
                }),
                None,
            )
            .unwrap();
        assert!(
            manager
                .validate_models()
                .unwrap_err()
                .to_string()
                .contains("clashes")
        );
    }

    #[test]
    fn an_aliased_import_clashes_under_its_alias() {
        // `import org.common.{Address as Location}` occupies Location, not Address.
        let imports = serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.ImportTypes",
              "namespace": "org.common@1.0.0", "types": ["Address"],
              "aliasedTypes": [
                { "$class": "concerto.metamodel@1.0.0.AliasedType",
                  "name": "Address", "aliasedName": "Location" }
              ] }
        ]);
        let clash = validate_with_imports(
            imports.clone(),
            serde_json::json!([concept(serde_json::json!({ "name": "Location" }))]),
        );
        assert!(clash.unwrap_err().to_string().contains("clashes"));

        let free = validate_with_imports(
            imports,
            serde_json::json!([concept(serde_json::json!({ "name": "Address" }))]),
        );
        assert!(free.is_ok());
    }

    /// Loads `org.a@1.0.0` and `org.a@2.0.0`, then `org.t@1.0.0` with the
    /// given imports, and validates.
    fn validate_importing(imports: serde_json::Value) -> crate::error::Result<()> {
        let mut manager = ModelManager::new().unwrap();
        for (namespace, declared) in [("org.a@1.0.0", "X"), ("org.a@2.0.0", "Y")] {
            manager
                .add_model(
                    &serde_json::json!({
                        "$class": "concerto.metamodel@1.0.0.Model",
                        "namespace": namespace,
                        "declarations": [concept(serde_json::json!({ "name": declared }))]
                    }),
                    None,
                )
                .unwrap();
        }
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.t@1.0.0",
                    "imports": imports,
                    "declarations": [concept(serde_json::json!({ "name": "Local" }))]
                }),
                None,
            )
            .unwrap();
        manager.validate_models()
    }

    fn import_of(namespace: &str, name: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": namespace, "name": name
        })
    }

    #[test]
    fn importing_a_type_that_does_not_exist_is_rejected() {
        let err = validate_importing(serde_json::json!([import_of("org.a@1.0.0", "Ghost")]));
        assert!(err.unwrap_err().to_string().contains("not defined"));
    }

    #[test]
    fn importing_a_declared_type_is_accepted() {
        assert!(validate_importing(serde_json::json!([import_of("org.a@1.0.0", "X")])).is_ok());
    }

    #[test]
    fn importing_two_versions_of_one_namespace_is_rejected() {
        let err = validate_importing(serde_json::json!([
            import_of("org.a@1.0.0", "X"),
            import_of("org.a@2.0.0", "Y")
        ]));
        assert!(err.unwrap_err().to_string().contains("different versions"));
    }

    #[test]
    fn a_system_identifier_may_not_extend_an_explicit_one() {
        let err = validate(serde_json::json!([
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Explicit",
                "isAbstract": false,
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "code" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "code",
                      "isArray": false, "isOptional": false }
                ]
            },
            {
                "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Systemic",
                "isAbstract": false,
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Explicit" },
                "identified": { "$class": "concerto.metamodel@1.0.0.Identified" },
                "properties": []
            }
        ]));
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("cannot be redeclared")
        );
    }

    /// TS: introspect/classdeclaration.js, "#validate should throw when an
    /// super type identifier is redeclared"
    /// (test/data/parser/classdeclaration.identifierextendsfromsupertype.cto)
    /// and introspect/identifieddeclaration.js, "#identified should not
    /// allow overriding explicit identifier with an explicit identifier":
    /// two classes each explicitly `identified by` their own field is the
    /// "explicit-over-explicit identity" gap this task closes.
    #[test]
    fn an_explicit_identifier_may_not_extend_an_explicit_one() {
        let err = validate(serde_json::json!([
            {
                "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "p1",
                "isAbstract": true,
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "a1" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "a1",
                      "isArray": false, "isOptional": false }
                ]
            },
            {
                "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "p2",
                "isAbstract": false,
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "p1" },
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "a1" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "a2",
                      "isArray": false, "isOptional": false }
                ]
            }
        ]));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Super class org.example@1.0.0.p1 has an explicit identifier a1 that cannot be redeclared."
        );
    }

    /// TS: introspect/identifieddeclaration.js, "#identified should not
    /// allow overriding system identifier": both `FancyOrder` and the
    /// `Asset` it implicitly extends declare a bare `identified` (system),
    /// so each contributes its own synthesised `$identifier` field
    /// (P2-03, `ClassDeclaration::from_json`) and the two collide as a
    /// duplicate field name — not the identity-redeclare check, which
    /// allows a system identifier over a system identifier.
    #[test]
    fn a_system_identifier_over_a_system_identifier_collides_as_a_duplicate_field() {
        let err = validate(serde_json::json!([{
            "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "FancyOrder",
            "isAbstract": false,
            // The metamodel AST an `asset` with no explicit `extends`
            // actually carries: the CTO parser (concerto-cto, out of scope
            // here) fills in `superType: Asset` itself, so
            // `ClassDeclaration.process`'s implicit-`Concept` fallback never
            // fires for it.
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Asset", "namespace": "concerto@1.0.0" },
            "identified": { "$class": "concerto.metamodel@1.0.0.Identified" },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "sku",
                  "isArray": false, "isOptional": false }
            ]
        }]));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Class \"FancyOrder\" has more than one field named \"$identifier\"."
        );
    }

    /// P2-04 (plan §1.2's "enum duplicate ... values" gap; issue #48):
    /// `EnumDeclaration` inherits `ClassDeclaration.validate` unchanged, so
    /// two values of the same name in one enum are rejected exactly like two
    /// same-named fields on a class — same catalogue code, same message.
    /// Checked against the frozen TS 5.0.0 reference
    /// (`migration/oracle/reference`): `ModelManager.addCTOModel` on
    ///
    /// ```cto
    /// namespace org.acme.enumdup@1.0.0
    /// enum Status {
    ///   o ACTIVE
    ///   o ACTIVE
    /// }
    /// ```
    ///
    /// raises `IllegalModelException: Class "Status" has more than one field
    /// named "ACTIVE".`, matching this test verbatim.
    #[test]
    fn duplicate_enum_value_name_is_rejected() {
        let err = validate(serde_json::json!([{
            "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "Status",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "ACTIVE" },
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "ACTIVE" }
            ]
        }]));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Class \"Status\" has more than one field named \"ACTIVE\"."
        );
    }

    /// The non-duplicate case: distinct enum value names load and validate
    /// cleanly, the same as the reference.
    #[test]
    fn distinct_enum_value_names_pass() {
        let ok = validate(serde_json::json!([{
            "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "Status",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "ACTIVE" },
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "INACTIVE" }
            ]
        }]));
        assert!(ok.is_ok());
    }

    /// TS: introspect/classdeclaration.js "#validation validation of super
    /// types" (test/data/parser/validation.cto): a `participant` cannot
    /// extend an `asset`, even though neither names a super type explicitly
    /// incompatible on its face — the kind-compatibility gap this task
    /// closes.
    #[test]
    fn a_participant_cannot_extend_an_asset() {
        let err = validate(serde_json::json!([
            {
                "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "A",
                "isAbstract": false,
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id",
                      "isArray": false, "isOptional": false }
                ]
            },
            {
                "$class": "concerto.metamodel@1.0.0.ParticipantDeclaration", "name": "B",
                "isAbstract": false,
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "A" },
                "properties": []
            }
        ]));
        assert_eq!(
            err.unwrap_err().to_string(),
            "ParticipantDeclaration (B) cannot extend AssetDeclaration (A)"
        );
    }

    /// A class may extend another of the same kind freely (the
    /// kind-compatibility check only fires against a *different*, non-concept
    /// kind).
    #[test]
    fn a_participant_may_extend_a_participant() {
        let err = validate(serde_json::json!([
            {
                "$class": "concerto.metamodel@1.0.0.ParticipantDeclaration", "name": "A",
                "isAbstract": true,
                "properties": []
            },
            {
                "$class": "concerto.metamodel@1.0.0.ParticipantDeclaration", "name": "B",
                "isAbstract": false,
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "A" },
                "properties": []
            }
        ]));
        assert!(err.is_ok());
    }

    /// TS: `ClassDeclaration.process`'s implicit `Concept` super type
    /// (src/introspect/classdeclaration.ts): a class whose AST carries no
    /// `superType` at all still passes `check_super_type`, because it
    /// implicitly extends `Concept`.
    #[test]
    fn a_class_with_no_super_type_implicitly_extends_concept() {
        assert!(
            validate(serde_json::json!([concept(
                serde_json::json!({ "name": "Standalone" })
            )]))
            .is_ok()
        );
    }

    /// TS: `ClassDeclaration.validate`'s self-extend check
    /// (src/introspect/classdeclaration.ts).
    #[test]
    fn a_class_extending_itself_is_rejected() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Loop",
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Loop" }
        }))]));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Class \"Loop\" cannot extend itself."
        );
    }

    /// TS: introspect/identifieddeclaration.js, "#identified should create a
    /// system identifier" / "should allow declaring explicit identifier" /
    /// "should allow abstract assets without an identifier": `getProperties()`
    /// (here, [`ModelManager::get_all_properties`]) includes the `$identifier`
    /// field the system `Asset` declaration carries, whether or not the
    /// subtype declares its own identity, and
    /// [`ModelManager::identifier_field_name`] inherits it.
    #[test]
    fn an_asset_inherits_the_system_identifier_field() {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.example@1.0.0",
                    "declarations": [{
                        "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Order",
                        "isAbstract": false,
                        // As the CTO parser fills in for an `asset` with no
                        // explicit `extends` (out of scope here).
                        "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Asset", "namespace": "concerto@1.0.0" },
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "price",
                              "isArray": false, "isOptional": false }
                        ]
                    }]
                }),
                None,
            )
            .unwrap();
        manager.validate_models().unwrap();

        let properties = manager
            .get_all_properties("org.example@1.0.0.Order")
            .unwrap();
        let names: Vec<&str> = properties.iter().map(|(_, p)| p.name()).collect();
        assert_eq!(names, ["price", "$identifier"]);
        assert_eq!(
            manager
                .identifier_field_name("org.example@1.0.0.Order")
                .unwrap()
                .as_deref(),
            Some("$identifier")
        );
    }

    /// TS: the same file, "should allow declaring explicit identifier": an
    /// explicitly identified subtype still inherits the ambient
    /// `$identifier` field from `Asset` ("this allows addition of an
    /// `$identifier` field from a supertype even if this type is explicitly
    /// identified", `ClassDeclaration.getProperties`), even though its own
    /// identity is `sku`.
    #[test]
    fn an_explicitly_identified_asset_still_inherits_the_system_identifier_field() {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.example@1.0.0",
                    "declarations": [{
                        "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Order",
                        "isAbstract": false,
                        // As the CTO parser fills in for an `asset` with no
                        // explicit `extends` (out of scope here).
                        "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Asset", "namespace": "concerto@1.0.0" },
                        "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "sku" },
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "sku",
                              "isArray": false, "isOptional": false },
                            { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "price",
                              "isArray": false, "isOptional": false }
                        ]
                    }]
                }),
                None,
            )
            .unwrap();
        manager.validate_models().unwrap();

        let properties = manager
            .get_all_properties("org.example@1.0.0.Order")
            .unwrap();
        let names: Vec<&str> = properties.iter().map(|(_, p)| p.name()).collect();
        assert_eq!(names, ["sku", "price", "$identifier"]);
        assert_eq!(
            manager
                .identifier_field_name("org.example@1.0.0.Order")
                .unwrap()
                .as_deref(),
            Some("sku")
        );
    }

    /// TS: `ClassDeclaration.addTimestampField`, added only to the system
    /// model's own `Transaction`/`Event` declarations and inherited from
    /// there (src/introspect/classdeclaration.ts).
    #[test]
    fn a_transaction_inherits_the_system_timestamp_field() {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.example@1.0.0",
                    "declarations": [{
                        "$class": "concerto.metamodel@1.0.0.TransactionDeclaration", "name": "Payment",
                        "isAbstract": false,
                        // As the CTO parser fills in for a `transaction`
                        // with no explicit `extends` (out of scope here).
                        "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Transaction", "namespace": "concerto@1.0.0" },
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "amount",
                              "isArray": false, "isOptional": false }
                        ]
                    }]
                }),
                None,
            )
            .unwrap();
        manager.validate_models().unwrap();

        let properties = manager
            .get_all_properties("org.example@1.0.0.Payment")
            .unwrap();
        let names: Vec<&str> = properties.iter().map(|(_, p)| p.name()).collect();
        assert_eq!(names, ["amount", "$timestamp"]);
    }

    /// A relationship to a class with no identity of its own, but that
    /// extends one that has (every `Asset`/`Participant`), is valid: the
    /// inherited-identifier-lookup gap this task closes.
    ///
    /// TS: RelationshipDeclaration.validate calls the target's inherited
    /// `isIdentified()` (src/introspect/relationshipdeclaration.ts).
    #[test]
    fn a_relationship_to_a_class_identified_only_through_its_super_type_passes() {
        let err = validate(serde_json::json!([
            {
                "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Vehicle",
                "isAbstract": false,
                // As the CTO parser fills in for an `asset` with no explicit
                // `extends` (out of scope here) — the source of Vehicle's
                // inherited identity this test exercises.
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Asset", "namespace": "concerto@1.0.0" },
                "properties": []
            },
            concept(serde_json::json!({
                "name": "Fleet",
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "vehicle",
                    "isArray": false, "isOptional": false,
                    "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Vehicle" }
                }]
            }))
        ]));
        assert!(err.is_ok());
    }

    /// A map with the given key and value nodes, beside a String scalar and a
    /// concept it can point at.
    fn map_with(key: serde_json::Value, value: serde_json::Value) -> serde_json::Value {
        serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Code" },
            { "$class": "concerto.metamodel@1.0.0.DateTimeScalar", "name": "When" },
            concept(serde_json::json!({ "name": "Item" })),
            { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "Lookup",
              "key": key, "value": value }
        ])
    }

    fn object_type(name: &str, class: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": format!("concerto.metamodel@1.0.0.{class}"),
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": name }
        })
    }

    #[test]
    fn a_map_key_must_be_string_or_datetime() {
        let string_value =
            serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapValueType" });
        // A concept is not a legal key.
        let err = validate(map_with(
            object_type("Item", "ObjectMapKeyType"),
            string_value.clone(),
        ));
        assert!(err.unwrap_err().to_string().contains("String or DateTime"));

        // A scalar over String or over DateTime is.
        for scalar in ["Code", "When"] {
            assert!(
                validate(map_with(
                    object_type(scalar, "ObjectMapKeyType"),
                    string_value.clone()
                ))
                .is_ok(),
                "a scalar over {scalar} should be a legal key"
            );
        }

        // As is a plain String key.
        assert!(
            validate(map_with(
                serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" }),
                string_value
            ))
            .is_ok()
        );
    }

    #[test]
    fn a_map_key_kind_outside_the_allowed_set_is_rejected() {
        // Only String, DateTime and object keys exist; anything else is not a
        // key the specification allows.
        let err = validate(map_with(
            serde_json::json!({ "$class": "concerto.metamodel@1.0.0.IntegerMapKeyType" }),
            serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapValueType" }),
        ));
        assert!(err.unwrap_err().to_string().contains("String or DateTime"));
    }

    #[test]
    fn a_map_value_must_name_a_declared_type() {
        let key = serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" });
        let err = validate(map_with(
            key.clone(),
            object_type("Missing", "ObjectMapValueType"),
        ));
        assert!(err.unwrap_err().to_string().contains("Undeclared type"));

        assert!(validate(map_with(key, object_type("Item", "ObjectMapValueType"))).is_ok());
    }

    #[test]
    fn a_map_value_may_not_be_a_relationship() {
        let err = validate(map_with(
            serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" }),
            object_type("Item", "RelationshipMapValueType"),
        ));
        assert!(err.unwrap_err().to_string().contains("may not be a"));
    }

    #[test]
    fn a_map_value_may_not_be_an_enum() {
        let key = serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" });
        let mut declarations = map_with(key, object_type("Colour", "ObjectMapValueType"));
        declarations
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "Colour",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" }
                ]
            }));
        let err = validate(declarations);
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("must be a concept or a scalar")
        );
    }

    /// P2-04 (issue #48): `Property.validate`'s size-validator check
    /// (property.ts) allows a non-array size validator only when the
    /// property's own type is a map declaration — checked here only once
    /// the target type is known, which is why it is a `validate_models`
    /// check (`check_property_type`) rather than a load-time one
    /// (`Property::check_validators`, which only knows the property's own
    /// AST, not what its type resolves to).
    ///
    /// Ported from `test/introspect/property.js` #getSizeValidator "should
    /// reject size on a non-array, non-map object property".
    #[test]
    fn size_validator_on_a_non_array_object_property_of_a_non_map_type_is_rejected() {
        let err = validate(serde_json::json!([
            concept(serde_json::json!({ "name": "B" })),
            concept(serde_json::json!({
                "name": "A",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "thing",
                      "isArray": false, "isOptional": false,
                      "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "B" },
                      "sizeValidator": { "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 1, "maxSize": 5 } }
                ]
            }))
        ]));
        assert_eq!(
            err.unwrap_err().to_string(),
            "size validator can only be applied to array or map properties: org.example@1.0.0.A.thing"
        );
    }

    /// Ported from `test/introspect/property.js` #getSizeValidator "should
    /// allow size on a map-typed property".
    #[test]
    fn size_validator_on_a_non_array_object_property_of_a_map_type_is_allowed() {
        let key = serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" });
        let value = serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapValueType" });
        let err = validate(serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "M", "key": key, "value": value },
            concept(serde_json::json!({
                "name": "A",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "data",
                      "isArray": false, "isOptional": false,
                      "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "M" },
                      "sizeValidator": { "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 1, "maxSize": 5 } }
                ]
            }))
        ]));
        assert!(err.is_ok());
    }

    /// Ported from `test/introspect/property.js` #getSizeValidator "should
    /// allow size on a map-typed property imported from another namespace":
    /// the map declaration and the property pointing at it are in different
    /// namespaces, so resolving the property's type needs the import list,
    /// not just the local declarations `validate`'s own helpers build.
    #[test]
    fn size_validator_on_a_map_type_imported_from_another_namespace_is_allowed() {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "maps@1.0.0",
                    "declarations": [
                        { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "PhoneBook",
                          "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                          "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" } }
                    ]
                }),
                None,
            )
            .unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "t@1.0.0",
                    "imports": [
                        { "$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "maps@1.0.0", "name": "PhoneBook" }
                    ],
                    "declarations": [concept(serde_json::json!({
                        "name": "A",
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "contacts",
                              "isArray": false, "isOptional": false,
                              "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "PhoneBook" },
                              "sizeValidator": { "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 1, "maxSize": 10 } }
                        ]
                    }))]
                }),
                None,
            )
            .unwrap();
        assert!(manager.validate_models().is_ok());
    }

    #[test]
    fn test_fresh_model_manager_is_valid() {
        // A fresh manager has only the two system models loaded (P1-07b: the
        // decorator model, then the root model); it must validate. Only the
        // root model is skipped by `is_system_namespace`, so this also covers
        // the decorator model's own declarations validating cleanly. (TS
        // `validateModelFiles` validates every model file, the root included.)
        let manager = ModelManager::new().unwrap();
        assert!(manager.validate_models().is_ok());
    }

    #[test]
    fn identifier_of_non_string_type_fails() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Entity",
            "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "id", "isArray": false, "isOptional": false }
            ]
        }))]));
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("the type of the field is not \"String\"")
        );
    }

    #[test]
    fn identifier_of_string_type_passes() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Entity",
            "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id", "isArray": false, "isOptional": false }
            ]
        }))]));
        assert!(err.is_ok());
    }

    #[test]
    fn identifier_of_string_scalar_type_passes() {
        let err = validate(serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "CustomString" },
            concept(serde_json::json!({
                "name": "Book",
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "isbn" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "isbn", "isArray": false, "isOptional": false,
                      "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "CustomString" } }
                ]
            }))
        ]));
        assert!(err.is_ok());
    }

    #[test]
    fn optional_identifier_fails() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Product",
            "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "productId" },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "productId", "isArray": false, "isOptional": true }
            ]
        }))]));
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("Identifying fields cannot be optional")
        );
    }

    /// TS `ClassDeclaration.validate` checks `identifiernotstring` before
    /// "Identifying fields cannot be optional.", so a field that is both
    /// optional and not a String reports the type.
    #[test]
    fn optional_non_string_identifier_reports_the_type_first() {
        let err = validate(serde_json::json!([concept(serde_json::json!({
            "name": "Product",
            "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "productId" },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "productId", "isArray": false, "isOptional": true }
            ]
        }))]));
        let message = err.unwrap_err().to_string();
        assert!(
            message.contains("the type of the field is not \"String\""),
            "{message}"
        );
        assert!(!message.contains("cannot be optional"), "{message}");
    }

    #[test]
    fn reserved_field_name_is_rejected_at_load() {
        // A `$`-prefixed field name is rejected while loading, before validation.
        let mut manager = ModelManager::new().unwrap();
        let result = manager.add_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "declarations": [concept(serde_json::json!({
                    "name": "Thing",
                    "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "$class", "isArray": false, "isOptional": false }
                    ]
                }))]
            }),
            None,
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid field name")
        );
    }

    /// A model with two declarations named `A` (a concept, then an asset).
    fn duplicate_model(namespace: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": namespace,
            "declarations": [
                concept(serde_json::json!({ "name": "A" })),
                { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "A",
                  "isAbstract": false, "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "id",
                      "isArray": false, "isOptional": false }
                  ],
                  "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "id" } }
            ]
        })
    }

    /// Asserts `err` is TS `ModelFile.validate()`'s duplicate-name
    /// `IllegalModelException`: its exact message, and neither a model file
    /// nor a location (TS passes neither), even when the file has a name.
    fn assert_duplicate_class_name(err: ConcertoError, fqn: &str) {
        let ConcertoError::Contract(contract) = &err else {
            panic!("expected a contract error, got {err:?}");
        };
        assert_eq!(contract.kind, crate::error::ErrorKind::IllegalModel);
        assert_eq!(contract.model_file, None);
        assert_eq!(contract.location, None);
        // The trailing space is TS `IllegalModelException`'s own, from an
        // empty file/location suffix.
        assert_eq!(
            contract.final_message(),
            format!("Duplicate class name {fqn} ")
        );
    }

    /// TS: `ModelFile.validate()`'s duplicate-name scan (P2-08). Loading
    /// (`add_model`, which never validates — TS `addModelFile(…, true)`)
    /// accepts the duplicate; the later `validateModelFiles` rejects it.
    #[test]
    fn duplicate_declaration_is_accepted_on_load_and_rejected_by_validation() {
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(&duplicate_model("org.dup@1.0.0"), Some("dup.cto".into()))
            .expect("loading without validation accepts a duplicate name");
        let file = manager.model_file("org.dup@1.0.0").unwrap();
        assert_eq!(file.declarations().len(), 2);
        assert_duplicate_class_name(
            manager.validate_model_file(file).unwrap_err(),
            "org.dup@1.0.0.A",
        );
        assert_duplicate_class_name(manager.validate_models().unwrap_err(), "org.dup@1.0.0.A");
    }

    /// TS `addModelFiles` (validation on) rejects the batch at its
    /// `validateModelFiles` step, and leaves the manager as it was.
    #[test]
    fn add_models_rejects_a_duplicate_declaration_at_validation() {
        let mut manager = ModelManager::new().unwrap();
        let model = duplicate_model("org.dup@1.0.0");
        let err = manager
            .add_models([(&model, Some("dup.cto".to_string()))])
            .unwrap_err();
        assert_duplicate_class_name(err, "org.dup@1.0.0.A");
        assert!(manager.model_file("org.dup@1.0.0").is_none());
    }

    /// The duplicate-name scan runs after the import checks and before any
    /// declaration is validated, as in TS: an undeclared import wins over a
    /// duplicate, and a duplicate wins over a declaration's own problem.
    #[test]
    fn duplicate_declaration_scan_runs_between_imports_and_declarations() {
        let mut manager = ModelManager::new().unwrap();
        let mut model = duplicate_model("org.dup@1.0.0");
        model["declarations"][0]["superType"] = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing"
        });
        manager.add_model(&model, None).unwrap();
        assert_duplicate_class_name(manager.validate_models().unwrap_err(), "org.dup@1.0.0.A");

        let mut manager = ModelManager::new().unwrap();
        let mut model = duplicate_model("org.dup@1.0.0");
        model["imports"] = serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.ImportType",
              "namespace": "org.missing@1.0.0", "name": "X" }
        ]);
        manager.add_model(&model, None).unwrap();
        let message = manager.validate_models().unwrap_err().to_string();
        assert!(!message.contains("Duplicate class name"), "{message}");
    }

    /// TS `new ModelFile(mm, ast).validate()`: a file its manager never
    /// registered validates against that manager (imports resolve through
    /// it, local types through the file itself), and the manager is left
    /// unchanged.
    #[test]
    fn a_detached_model_file_validates_against_its_manager() {
        use crate::introspect::model_file::ModelFile;
        let mut manager = ModelManager::new().unwrap();
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.common@1.0.0",
                    "declarations": [concept(serde_json::json!({ "name": "Address" }))]
                }),
                None,
            )
            .unwrap();
        let generation = manager.generation();
        let importing = |declarations: serde_json::Value| {
            ModelFile::from_json(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.b@1.0.0",
                    "imports": [
                        { "$class": "concerto.metamodel@1.0.0.ImportType",
                          "namespace": "org.common@1.0.0", "name": "Address" }
                    ],
                    "declarations": declarations
                }),
                None,
            )
            .unwrap()
        };
        let valid = importing(serde_json::json!([
            concept(serde_json::json!({ "name": "Base" })),
            concept(serde_json::json!({
                "name": "Person",
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "home",
                      "isArray": false, "isOptional": false,
                      "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Address" } }
                ]
            }))
        ]));
        manager.validate_detached_model_file(&valid).unwrap();

        let duplicate = importing(serde_json::json!([
            concept(serde_json::json!({ "name": "Person" })),
            concept(serde_json::json!({ "name": "Person" }))
        ]));
        assert_duplicate_class_name(
            manager
                .validate_detached_model_file(&duplicate)
                .unwrap_err(),
            "org.b@1.0.0.Person",
        );
        assert!(manager.model_file("org.b@1.0.0").is_none());
        assert_eq!(manager.generation(), generation);
    }

    /// TS `Property.validate`'s size-validator check for a primitive field
    /// and a relationship: construction accepts both (P2-08), validation
    /// rejects them with TS's exact message — the property's fully-qualified
    /// name — and before a relationship's own primitive-type check.
    ///
    /// Ported from `test/introspect/property.js` #getSizeValidator "should
    /// reject size on a non-array String property" / "... Integer property".
    #[test]
    fn size_validator_on_a_non_array_primitive_or_relationship_is_rejected_by_validation() {
        let sized = |class: &str, name: &str, extra: serde_json::Value| {
            let mut p = serde_json::json!({
                "$class": format!("concerto.metamodel@1.0.0.{class}"),
                "name": name, "isArray": false, "isOptional": false,
                "sizeValidator": { "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator",
                                   "minSize": 1, "maxSize": 5 }
            });
            p.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            p
        };
        let cases = [
            sized("StringProperty", "name", serde_json::json!({})),
            sized("IntegerProperty", "count", serde_json::json!({})),
            sized(
                "RelationshipProperty",
                "owner",
                serde_json::json!({ "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "String" } }),
            ),
        ];
        for property in cases {
            let name = property["name"].as_str().unwrap().to_string();
            let err = validate(serde_json::json!([concept(serde_json::json!({
                "name": "A",
                "properties": [property]
            }))]))
            .unwrap_err();
            assert_eq!(
                err.to_string(),
                format!(
                    "size validator can only be applied to array or map properties: org.example@1.0.0.A.{name}"
                )
            );
        }
    }
}

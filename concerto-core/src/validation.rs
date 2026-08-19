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
//! Validation stops at the first problem. A rule that a model breaks is
//! reported as [`ConcertoError::ValidationFailed`]; a model that cannot be
//! walked at all, such as one whose inheritance is circular, surfaces the
//! [`ConcertoError::IllegalModel`] raised while resolving it. A model that
//! validates cleanly returns `Ok(())`.

use std::collections::{HashMap, HashSet};

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, Result};
use crate::introspect::declaration::{
    ClassDeclaration, Declaration, MapDeclaration, ScalarDeclaration,
};
use crate::introspect::import::Import;
use crate::introspect::model_file::ModelFile;
use crate::introspect::property::Property;
use crate::model_manager::ModelManager;
use crate::model_util::{is_primitive_type, parse_namespace, qualify};

impl ModelManager {
    /// Validates every loaded user model, leaving the built-in system model
    /// aside. Returns `Ok(())` if every model is semantically valid, otherwise
    /// the first problem found. Namespaces are visited in order so that the
    /// same set of models always reports the same problem.
    pub fn validate_models(&self) -> Result<()> {
        let mut model_files: Vec<_> = self
            .model_files()
            .filter(|model_file| !model_file.is_system_namespace())
            .collect();
        model_files.sort_by_key(|model_file| model_file.namespace());

        for model_file in model_files {
            check_import_clashes(model_file)?;
            check_import_namespaces(model_file)?;
            check_imported_types_exist(self, model_file)?;
            for declaration in model_file.declarations() {
                validate_declaration(self, model_file.namespace(), declaration)?;
            }
        }
        Ok(())
    }
}

/// A declaration may not take the name of a type the file imports. Importing
/// from the file's own namespace is caught the same way, because such an import
/// names a type the file declares.
fn check_import_clashes(model_file: &ModelFile) -> Result<()> {
    let imported: HashSet<&str> = model_file
        .imports()
        .iter()
        .flat_map(Import::local_names)
        .collect();
    for declaration in model_file.declarations() {
        if imported.contains(declaration.name()) {
            return Err(failed(format!(
                "Type {} clashes with an imported type with the same name",
                declaration.name()
            )));
        }
    }
    Ok(())
}

/// Validates one declaration. Class-like declarations are the only ones with
/// checks in this pass; enum and scalar declarations are checked while loading.
fn validate_declaration(
    manager: &ModelManager,
    namespace: &str,
    declaration: &Declaration,
) -> Result<()> {
    match declaration {
        Declaration::Class(class) => validate_class(manager, namespace, class),
        Declaration::Map(map) => check_map_types(manager, namespace, map),
        Declaration::Enum(_) | Declaration::Scalar(_) => Ok(()),
    }
}

fn validate_class(manager: &ModelManager, namespace: &str, class: &ClassDeclaration) -> Result<()> {
    check_super_type(manager, namespace, class)?;
    check_unique_field_names(manager, class, &qualify(namespace, class.name()))?;
    check_identifier(manager, namespace, class)?;
    check_identity_matches_super(manager, namespace, class)?;
    check_unique_decorators(class.decorators())?;
    for property in class.own_properties() {
        check_property_type(manager, namespace, class.name(), property)?;
        check_unique_decorators(property.decorators())?;
    }
    Ok(())
}

/// An element may not carry the same decorator twice.
fn check_unique_decorators(decorators: &[mm::Decorator]) -> Result<()> {
    let mut seen = HashSet::new();
    for decorator in decorators {
        if !seen.insert(decorator.name.as_str()) {
            return Err(failed(format!("Duplicate decorator {}", decorator.name)));
        }
    }
    Ok(())
}

/// The super type, if any, must resolve to a declared class.
fn check_super_type(
    manager: &ModelManager,
    namespace: &str,
    class: &ClassDeclaration,
) -> Result<()> {
    let Some(super_type) = class.super_type() else {
        return Ok(());
    };
    let resolves_to_class = resolve(
        manager,
        namespace,
        &super_type.name,
        super_type.namespace.as_deref(),
    )
    .and_then(|fqn| manager.get_declaration(&fqn).ok())
    .is_some_and(|decl| decl.is_class_declaration());
    if resolves_to_class {
        Ok(())
    } else {
        Err(failed(format!(
            "Could not find super type {} for {}",
            super_type.name,
            class.name()
        )))
    }
}

/// No field name may appear twice once inherited fields are included, so a
/// subtype cannot silently redeclare a field from a super type.
fn check_unique_field_names(
    manager: &ModelManager,
    class: &ClassDeclaration,
    fqn: &str,
) -> Result<()> {
    let mut seen = HashSet::new();
    for property in manager.get_all_properties(fqn)? {
        if !seen.insert(property.name()) {
            return Err(failed(format!(
                "{} has more than one field named {}",
                class.name(),
                property.name()
            )));
        }
    }
    Ok(())
}

/// A field-provided identifier (`identified by field`) must name a required
/// field typed as `String` or a String-based scalar.
fn check_identifier(
    manager: &ModelManager,
    namespace: &str,
    class: &ClassDeclaration,
) -> Result<()> {
    let Some(field_name) = class.identifier_field_name() else {
        return Ok(());
    };
    let field = class
        .own_properties()
        .iter()
        .find(|property| property.name() == field_name)
        .ok_or_else(|| {
            failed(format!(
                "Class {} is identified by {field_name}, which it does not declare",
                class.name()
            ))
        })?;

    if field.is_optional() {
        return Err(failed(format!(
            "Identifying fields cannot be optional: {field_name}"
        )));
    }
    if !is_string_typed(manager, namespace, field) {
        return Err(failed(format!(
            "Class {} identifier {field_name} must be a String or a String-based scalar",
            class.name()
        )));
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
    resolve(
        manager,
        namespace,
        &type_identifier.name,
        type_identifier.namespace.as_deref(),
    )
    .and_then(|fqn| manager.get_declaration(&fqn).ok())
    .and_then(Declaration::as_scalar)
    .is_some_and(|scalar| scalar.scalar_type() == "String")
}

/// Object and relationship properties must point at a declared type; a
/// relationship additionally must target an identifiable class, never a
/// primitive.
fn check_property_type(
    manager: &ModelManager,
    namespace: &str,
    owner: &str,
    property: &Property,
) -> Result<()> {
    let Some(type_identifier) = property.type_identifier() else {
        return Ok(());
    };

    if property.is_relationship() && is_primitive_type(&type_identifier.name) {
        return Err(failed(format!(
            "Relationship {} on {} cannot be to the primitive type {}",
            property.name(),
            owner,
            type_identifier.name
        )));
    }

    let target = resolve(
        manager,
        namespace,
        &type_identifier.name,
        type_identifier.namespace.as_deref(),
    )
    .and_then(|fqn| manager.get_declaration(&fqn).ok());

    let Some(target) = target else {
        return Err(failed(format!(
            "Undeclared type {} referenced by {}.{}",
            type_identifier.name,
            owner,
            property.name()
        )));
    };

    if property.is_relationship() {
        let identifiable = target
            .as_class()
            .is_some_and(ClassDeclaration::is_identified);
        if !identifiable {
            return Err(failed(format!(
                "Relationship {} on {} must be to a class that has an identifier",
                property.name(),
                owner
            )));
        }
    }

    Ok(())
}

/// Resolves a referenced type to a fully-qualified name. A reference that
/// carries its own namespace is qualified directly; otherwise it is resolved
/// through the imports and local declarations of `namespace`.
fn resolve(
    manager: &ModelManager,
    namespace: &str,
    name: &str,
    reference_namespace: Option<&str>,
) -> Option<String> {
    match reference_namespace {
        Some(ns) => Some(qualify(ns, name)),
        None => manager.resolve_type_name(namespace, name).ok(),
    }
}

/// A file may not import two versions of one namespace, since a short name
/// could then mean either of them.
fn check_import_namespaces(model_file: &ModelFile) -> Result<()> {
    let mut versions: HashMap<String, String> = HashMap::new();
    for import in model_file.imports() {
        let namespace = parse_namespace(import.namespace())?;
        match versions.get(&namespace.name) {
            Some(seen) if *seen != namespace.version => {
                return Err(failed(format!(
                    "Importing types from different versions ({seen} and {}) of the same namespace {} is not permitted",
                    namespace.version, namespace.name
                )));
            }
            _ => {
                versions.insert(namespace.name, namespace.version);
            }
        }
    }
    Ok(())
}

/// Every imported type must exist in the namespace it is imported from.
fn check_imported_types_exist(manager: &ModelManager, model_file: &ModelFile) -> Result<()> {
    for import in model_file.imports() {
        for name in import.imported_names() {
            let fqn = qualify(import.namespace(), name);
            if manager.get_declaration(&fqn).is_err() {
                return Err(failed(format!(
                    "Type {name} is not defined in namespace {}",
                    import.namespace()
                )));
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
    if !class.is_identified() || class.identifier_field_name().is_some() {
        return Ok(());
    }
    let Some(super_type) = class.super_type() else {
        return Ok(());
    };
    let super_class = resolve(
        manager,
        namespace,
        &super_type.name,
        super_type.namespace.as_deref(),
    )
    .and_then(|fqn| manager.get_declaration(&fqn).ok())
    .and_then(Declaration::as_class);
    let Some(super_class) = super_class else {
        return Ok(());
    };
    if let Some(field) = super_class.identifier_field_name() {
        return Err(failed(format!(
            "Super type {} has an explicit identifier {field} that {} cannot redeclare",
            super_type.name,
            class.name()
        )));
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

/// Checks a map against the key and value types the specification permits.
fn check_map_types(manager: &ModelManager, namespace: &str, map: &MapDeclaration) -> Result<()> {
    if !MAP_KEY_KINDS.contains(&map.key_kind()) {
        return Err(failed(format!(
            "The key of map {} must be a String or DateTime, or a scalar over one of them",
            map.name()
        )));
    }
    if !MAP_VALUE_KINDS.contains(&map.value_kind()) {
        return Err(failed(format!(
            "The value of map {} may not be a {}",
            map.name(),
            map.value_kind()
        )));
    }

    // An object key names a scalar, which has to be over a String or DateTime.
    if let Some(key) = map.key_type() {
        let scalar = resolve(manager, namespace, &key.name, key.namespace.as_deref())
            .and_then(|fqn| manager.get_declaration(&fqn).ok())
            .and_then(Declaration::as_scalar)
            .map(ScalarDeclaration::scalar_type);
        if !matches!(scalar, Some("String") | Some("DateTime")) {
            return Err(failed(format!(
                "The key of map {} must be a String or DateTime, or a scalar over one of them",
                map.name()
            )));
        }
    }

    // An object value names a concept or a scalar, and it has to be declared.
    if let Some(value) = map.value_type() {
        let declared = resolve(manager, namespace, &value.name, value.namespace.as_deref())
            .and_then(|fqn| manager.get_declaration(&fqn).ok());
        let Some(declared) = declared else {
            return Err(failed(format!(
                "Undeclared type {} referenced by the value of map {}",
                value.name,
                map.name()
            )));
        };
        if !declared.is_class_declaration() && !declared.is_scalar_declaration() {
            return Err(failed(format!(
                "The value of map {} must be a concept or a scalar, and {} is neither",
                map.name(),
                value.name
            )));
        }
    }
    Ok(())
}

/// Builds a [`ConcertoError::ValidationFailed`] with the given message.
fn failed(message: String) -> ConcertoError {
    ConcertoError::ValidationFailed { message }
}

#[cfg(test)]
mod tests {
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
        assert!(err.unwrap_err().to_string().contains("primitive type"));
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
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("must be to a class that has an identifier")
        );
    }

    #[test]
    fn relationship_to_identified_class_passes() {
        let err = validate(serde_json::json!([
            {
                "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                "name": "Vehicle", "isAbstract": false,
                "identified": { "$class": "concerto.metamodel@1.0.0.Identified" },
                "properties": []
            },
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
        assert!(err.unwrap_err().to_string().contains("Undeclared type"));
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

    #[test]
    fn declaration_clashing_with_an_imported_name_is_rejected() {
        let err = validate_with_imports(
            serde_json::json!([
                { "$class": "concerto.metamodel@1.0.0.ImportType",
                  "namespace": "org.common@1.0.0", "name": "Address" }
            ]),
            serde_json::json!([concept(serde_json::json!({ "name": "Address" }))]),
        );
        assert!(err.unwrap_err().to_string().contains("clashes"));
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
        assert!(err.unwrap_err().to_string().contains("cannot redeclare"));
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

    #[test]
    fn test_fresh_model_manager_is_valid() {
        // A fresh manager has only the system model loaded; it must validate.
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
        assert!(err.unwrap_err().to_string().contains("identifier"));
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
}

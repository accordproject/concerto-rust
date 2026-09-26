//! The model queries the instance layer makes, answered from the
//! [`ModelManager`] arena with TS's own semantics for each declaration kind
//! (the `Declaration` defaults, and the `ClassDeclaration` members that
//! `EnumDeclaration` inherits, including `isClassDeclaration() === true`).
//!
//! These are the collaborator calls of `Factory`, `JSONPopulator` and
//! `JSONGenerator` (`modelManager.getType(...)`, then `classDecl.isX()`),
//! kept in one place so that each port reads the same way as its TS.

use std::sync::Arc;

use super::value::JsValue;
use crate::error::{ContractError, ErrorKind, Result};
use crate::introspect::scalar::ScalarValidator;
use crate::introspect::validators::StringValidator;
use crate::introspect::{ClassKind, Declaration, Named, Property, Typed};
use crate::model_manager::{DeclId, ModelManager};
use crate::model_util;

/// A declaration found by [`get_type`]: what TS holds after
/// `modelManager.getType(name)`.
#[derive(Clone, Copy)]
pub(crate) struct TypeRef<'a> {
    pub mm: &'a ModelManager,
    pub id: DeclId,
    pub decl: &'a Declaration,
}

/// TS: `modelManager.getType(qualifiedName)` (`BaseModelManager.getType`).
pub(crate) fn get_type<'a>(mm: &'a ModelManager, qualified_name: &str) -> Result<TypeRef<'a>> {
    let id = mm.get_type_declaration(qualified_name)?;
    let decl = mm
        .declaration(id)
        .expect("get_type_declaration returns a live handle");
    Ok(TypeRef { mm, id, decl })
}

/// V8's `TypeError: <expression> is not a function`.
pub(crate) fn not_a_function(expression: &str) -> crate::ConcertoError {
    ContractError::new(
        ErrorKind::JsTypeError,
        "engine-typeerror-notafunction",
        vec![("expression", expression.to_string())],
    )
    .into()
}

/// `'Unrecognised ' + JSON.stringify(thing)` for an introspection object:
/// `JSON.stringify` throws V8's circular-structure `TypeError` first. DV-010
pub(crate) fn unrecognised() -> crate::ConcertoError {
    ContractError::new(
        ErrorKind::JsTypeError,
        "engine-typeerror-circularjson",
        Vec::new(),
    )
    .into()
}

impl<'a> TypeRef<'a> {
    /// TS `getFullyQualifiedName()`.
    pub fn fqn(&self) -> String {
        model_util::get_fully_qualified_name(&self.namespace(), self.name())
    }

    /// TS `getNamespace()`: the model file's namespace.
    pub fn namespace(&self) -> String {
        self.mm
            .model_file_of(self.id)
            .and_then(|f| self.mm.file(f))
            .map(|f| f.namespace().to_string())
            .unwrap_or_default()
    }

    /// TS `getName()`.
    pub fn name(&self) -> &'a str {
        self.decl.name()
    }

    fn class_kind(&self) -> Option<ClassKind> {
        self.decl.as_class().map(|c| c.kind())
    }

    /// TS `isClassDeclaration?.()`: true for a `ClassDeclaration`, and for
    /// an `EnumDeclaration`, which extends it.
    pub fn is_class_declaration(&self) -> bool {
        matches!(self.decl, Declaration::Class(_) | Declaration::Enum(_))
    }

    /// TS `isMapDeclaration?.()`.
    pub fn is_map_declaration(&self) -> bool {
        matches!(self.decl, Declaration::Map(_))
    }

    /// TS `isEnum()`.
    pub fn is_enum(&self) -> bool {
        matches!(self.decl, Declaration::Enum(_))
    }

    /// TS `isTransaction?.()`.
    pub fn is_transaction(&self) -> bool {
        self.class_kind() == Some(ClassKind::Transaction)
    }

    /// TS `isEvent?.()`.
    pub fn is_event(&self) -> bool {
        self.class_kind() == Some(ClassKind::Event)
    }

    /// TS `isConcept?.()`.
    pub fn is_concept(&self) -> bool {
        self.class_kind() == Some(ClassKind::Concept)
    }

    /// TS `isAbstract()`: the AST flag for a class declaration, `false` for
    /// an enum, `true` for a scalar (`ScalarDeclaration.isAbstract`), and
    /// V8's `TypeError` for a map, which has no such method.
    pub fn is_abstract(&self, expression: &str) -> Result<bool> {
        match self.decl {
            Declaration::Class(c) => Ok(c.is_abstract()),
            Declaration::Enum(e) => Ok(e.is_abstract()),
            Declaration::Scalar(_) => Ok(true),
            Declaration::Map(_) => Err(not_a_function(expression)),
        }
    }

    /// TS `getIdentifierFieldName()`: the inherited identifying field of a
    /// class or enum declaration, and `null` (`Declaration`'s and
    /// `ScalarDeclaration`'s default) for a map or a scalar.
    pub fn identifier_field_name(&self) -> Result<Option<String>> {
        match self.decl {
            Declaration::Class(_) | Declaration::Enum(_) => {
                // P5-06b: the cached table's answer, when there is one.
                match class_info_of(self.mm, self.id) {
                    Some(info) => Ok(info.identifier_field_name.clone()),
                    None => self.mm.identifier_field_name(&self.fqn()),
                }
            }
            Declaration::Scalar(_) | Declaration::Map(_) => Ok(None),
        }
    }

    /// TS `isIdentified()`.
    pub fn is_identified(&self) -> Result<bool> {
        Ok(self.identifier_field_name()?.is_some())
    }

    /// TS `isSystemIdentified()`.
    pub fn is_system_identified(&self) -> Result<bool> {
        Ok(self.identifier_field_name()?.as_deref() == Some("$identifier"))
    }

    /// TS `getProperties()`: each property with the fully-qualified name of
    /// the declaration that declares it. A map or a scalar has none of its
    /// own (V8's `TypeError`, which no instance path reaches with one).
    pub fn properties(&self, expression: &str) -> Result<Vec<(String, Property)>> {
        match self.decl {
            Declaration::Class(_) | Declaration::Enum(_) => self.mm.get_all_properties(&self.fqn()),
            _ => Err(not_a_function(expression)),
        }
    }

    /// TS `getProperty(name)`.
    pub fn property(&self, name: &str) -> Result<Option<(String, Property)>> {
        // `properties(...)` then `find`, without cloning every other
        // property first (P5-06): `get_property` resolves the same super
        // chain and returns the same first match.
        match self.decl {
            Declaration::Class(_) | Declaration::Enum(_) => self.mm.get_property(&self.fqn(), name),
            _ => Err(not_a_function("classDeclaration.getProperty")),
        }
    }
}

/// What a field's declared type resolves to, in the model file of the
/// declaration that declares it (`Field.isPrimitive`, `isTypeEnum`,
/// `isTypeScalar`, `ModelUtil.isMap`, and `RelationshipDeclaration`).
#[derive(Debug, Clone)]
pub(crate) enum FieldType {
    /// A primitive field (`isPrimitive()`): its type name.
    Primitive(&'static str),
    /// A field whose type is a scalar (`isTypeScalar()`): the primitive it
    /// aliases, its default value and its validator (what
    /// `getScalarField()` copies from the scalar's AST).
    Scalar {
        primitive: Option<&'static str>,
        default_value: Option<serde_json::Value>,
        validator: Option<Box<ScalarValidator>>,
    },
    /// A field whose type is an enum (`isTypeEnum()`).
    Enum(String),
    /// A field whose type is a map (`ModelUtil.isMap(field)`).
    Map(String),
    /// A field whose type is a concept-like declaration.
    Class(String),
    /// A relationship: the fully-qualified name of its target.
    Relationship(String),
    /// An enum value member (an `EnumDeclaration`'s own property), which
    /// has no type.
    EnumValue,
}

/// A property of an instance's declaration, with what its type resolves
/// to.
#[derive(Debug, Clone)]
pub(crate) struct Field {
    /// The fully-qualified name of the declaration that declares it
    /// (`getParent().getFullyQualifiedName()`).
    pub owner_fqn: String,
    pub property: Property,
    pub field_type: FieldType,
}

impl Field {
    /// TS `getName()`.
    pub fn name(&self) -> &str {
        self.property.name()
    }

    /// TS `isArray()`.
    pub fn is_array(&self) -> bool {
        self.property.is_array()
    }

    /// TS `getType()`: the declared type name as written (a primitive's own
    /// name), or, for the unboxed `getScalarField()`, the scalar's primitive.
    pub fn type_name(&self) -> String {
        match &self.field_type {
            FieldType::Scalar { primitive, .. } => {
                primitive.map(str::to_string).unwrap_or_default()
            }
            _ => self.property.type_name().unwrap_or_default().to_string(),
        }
    }

    /// [`Self::type_name`], borrowed.
    pub fn type_name_str(&self) -> &str {
        match &self.field_type {
            FieldType::Scalar { primitive, .. } => primitive.unwrap_or_default(),
            _ => self.property.type_name().unwrap_or_default(),
        }
    }

    /// TS `getFullyQualifiedTypeName()`.
    pub fn fully_qualified_type_name(&self) -> String {
        match &self.field_type {
            FieldType::Primitive(p) => (*p).to_string(),
            FieldType::Scalar { primitive, .. } => {
                primitive.map(str::to_string).unwrap_or_default()
            }
            FieldType::Enum(f)
            | FieldType::Map(f)
            | FieldType::Class(f)
            | FieldType::Relationship(f) => f.clone(),
            FieldType::EnumValue => String::new(),
        }
    }

    /// TS `isPrimitive()`, of the field after `getScalarField()` unboxing
    /// (a scalar field unboxes to a primitive one).
    pub fn is_primitive(&self) -> bool {
        matches!(
            self.field_type,
            FieldType::Primitive(_) | FieldType::Scalar { .. }
        )
    }

    /// TS `RelationshipDeclaration.toString()`.
    pub fn relationship_to_string(&self) -> String {
        format!(
            "RelationshipDeclaration {{name={}, type={}, array={}, optional={}}}",
            self.name(),
            self.fully_qualified_type_name(),
            self.property.is_array(),
            self.property.is_optional(),
        )
    }
}

/// Resolves a property's declared type in its owner's model file.
pub(crate) fn field(mm: &ModelManager, owner_fqn: &str, property: Property) -> Result<Field> {
    let field_type = match &property {
        Property::Relationship(rp) => {
            let namespace = model_util::get_namespace(Some(owner_fqn))?;
            FieldType::Relationship(mm.resolve_type_name(namespace, &rp.type_.name, None)?)
        }
        Property::Object(op) => {
            let namespace = model_util::get_namespace(Some(owner_fqn))?;
            let fqn = mm.resolve_type_name(namespace, &op.type_.name, None)?;
            match mm.get_declaration(&fqn)? {
                Declaration::Enum(_) => FieldType::Enum(fqn),
                Declaration::Scalar(s) => FieldType::Scalar {
                    primitive: s.scalar_type(),
                    default_value: s.default_value().cloned(),
                    validator: s.validator().cloned().map(Box::new),
                },
                Declaration::Map(_) => FieldType::Map(fqn),
                Declaration::Class(_) => FieldType::Class(fqn),
            }
        }
        Property::Enum(_) => FieldType::EnumValue,
        Property::Boolean(_) => FieldType::Primitive("Boolean"),
        Property::String(_) => FieldType::Primitive("String"),
        Property::Integer(_) => FieldType::Primitive("Integer"),
        Property::Long(_) => FieldType::Primitive("Long"),
        Property::Double(_) => FieldType::Primitive("Double"),
        Property::DateTime(_) => FieldType::Primitive("DateTime"),
    };
    Ok(Field {
        owner_fqn: owner_fqn.to_string(),
        property,
        field_type,
    })
}

/// What the instance layer asks of a class-like declaration on every
/// instance, worked out once per model state (P5-06b): its inherited
/// identifying field, its properties (own and inherited, as
/// `getProperties()` lists them), each one's resolved type, the defaults
/// `assignFieldDefaults` assigns, and the identifier's `regex` validator.
/// [`ModelManager`] caches it by name and drops it on every change to the
/// registered files, as it does its super-type chains.
///
/// Only answers that succeed are kept: a table is built only when the
/// declaration's chain resolves, and a property whose type does not resolve
/// (or an identifier validator that fails to build) is left out, so that
/// its caller takes the uncached path and raises the same error, at the
/// same point, as before.
#[derive(Debug)]
pub(crate) struct ClassInfo {
    /// TS `getFullyQualifiedName()`.
    pub fqn: String,
    /// TS `getIdentifierFieldName()`.
    pub identifier_field_name: Option<String>,
    /// TS `getProperties()`: each with its declaring type's name.
    pub properties: Vec<(String, Property)>,
    /// [`field`] of each of [`Self::properties`], index for index, where it
    /// resolves.
    pub fields: Vec<Option<Field>>,
    /// `assignFieldDefaults`' `(name, value)` pairs, in order; `None` when a
    /// field it reaches does not resolve.
    pub defaults: Option<Vec<(String, JsValue)>>,
    /// `Factory.newResource`'s identifier `regex` validator: `Some(None)`
    /// when there is none, `None` when it could not be worked out here.
    pub id_regex: Option<Option<StringValidator>>,
}

impl ClassInfo {
    /// TS `getProperty(name)`'s index into [`Self::properties`].
    pub fn property_index(&self, name: &str) -> Option<usize> {
        self.properties.iter().position(|(_, p)| p.name() == name)
    }

    /// The resolved [`Field`] of the property `name`, when it is declared
    /// and resolves.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields[self.property_index(name)?].as_ref()
    }
}

/// The [`ClassInfo`] of the class-like declaration named `fqn` exactly
/// (`<namespace>.<name>`), or `None` when any of it cannot be worked out
/// (the caller then takes its own uncached path).
pub(crate) fn class_info(mm: &ModelManager, fqn: &str) -> Option<Arc<ClassInfo>> {
    class_info_of(mm, mm.declaration_id(fqn)?)
}

/// [`class_info`], for the declaration `id`.
pub(crate) fn class_info_of(mm: &ModelManager, id: DeclId) -> Option<Arc<ClassInfo>> {
    mm.cached_class_info(id, || {
        let decl = mm.declaration(id)?;
        if !matches!(decl, Declaration::Class(_) | Declaration::Enum(_)) {
            return None;
        }
        let fqn = TypeRef { mm, id, decl }.fqn();
        let class_decl = get_type(mm, &fqn).ok()?;
        if class_decl.id != id {
            return None;
        }
        let fqn = fqn.as_str();
        let identifier_field_name = mm.identifier_field_name(fqn).ok()?;
        let properties = mm.get_all_properties(fqn).ok()?;
        let fields: Vec<Option<Field>> = properties
            .iter()
            .map(|(owner_fqn, property)| field(mm, owner_fqn, property.clone()).ok())
            .collect();
        let mut defaults = Some(Vec::new());
        for ((owner_fqn, property), resolved) in properties.iter().zip(&fields) {
            if property.is_relationship() || property.is_enum_value() {
                continue;
            }
            let Some(resolved) = resolved else {
                defaults = None;
                break;
            };
            if let (Some(defaults), Some(value)) = (
                defaults.as_mut(),
                super::factory::field_default(mm, owner_fqn, property.name(), resolved),
            ) {
                defaults.push((property.name().to_string(), value));
            }
        }
        let id_regex = match &identifier_field_name {
            Some(id_field) => super::factory::identifier_regex(&class_decl, id_field).ok(),
            None => Some(None),
        };
        Some(ClassInfo {
            fqn: fqn.to_string(),
            identifier_field_name,
            properties,
            fields,
            defaults,
            id_regex,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(namespace: &str, declarations: serde_json::Value) -> serde_json::Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": namespace,
            "imports": [],
            "declarations": declarations,
        })
    }

    /// P5-06b: a table is built only once the whole super chain resolves,
    /// is dropped when the registered files change, and agrees with the
    /// uncached lookups it stands for.
    #[test]
    fn class_info_follows_the_registered_files() {
        let mut mm = ModelManager::new().expect("a model manager");
        let base = model(
            "org.base@1.0.0",
            json!([{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Base",
                "isAbstract": false,
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "key" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "key",
                      "isArray": false, "isOptional": false }
                ]
            }]),
        );
        let mut child = model(
            "org.child@1.0.0",
            json!([{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Child",
                "isAbstract": false,
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "size",
                      "isArray": false, "isOptional": false, "defaultValue": 3 }
                ]
            }]),
        );
        child["imports"] = json!([{
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": "org.base@1.0.0",
            "name": "Base"
        }]);
        mm.add_model(&base, Some("base.cto".into()))
            .expect("base loads");
        mm.add_model(&child, Some("child.cto".into()))
            .expect("child loads");

        let info = class_info(&mm, "org.child@1.0.0.Child").expect("a table");
        assert_eq!(info.fqn, "org.child@1.0.0.Child");
        assert_eq!(
            info.identifier_field_name,
            mm.identifier_field_name("org.child@1.0.0.Child").unwrap()
        );
        let uncached = mm.get_all_properties("org.child@1.0.0.Child").unwrap();
        assert_eq!(
            info.properties
                .iter()
                .map(|(o, p)| (o.clone(), p.name().to_string()))
                .collect::<Vec<_>>(),
            uncached
                .iter()
                .map(|(o, p)| (o.clone(), p.name().to_string()))
                .collect::<Vec<_>>(),
        );
        assert!(info.fields.iter().all(Option::is_some));
        assert_eq!(
            info.defaults,
            Some(vec![("size".to_string(), JsValue::Number(3.0))])
        );
        assert_eq!(info.id_regex.as_ref().map(Option::is_some), Some(false));
        // The same table while nothing changes.
        let again = class_info(&mm, "org.child@1.0.0.Child").expect("a table");
        assert!(Arc::ptr_eq(&info, &again));
        // A change to the registered files drops it.
        let other = model("org.other@1.0.0", json!([]));
        mm.add_model(&other, Some("other.cto".into()))
            .expect("other loads");
        let rebuilt = class_info(&mm, "org.child@1.0.0.Child").expect("a table");
        assert!(!Arc::ptr_eq(&info, &rebuilt));
        // Not a class-like declaration, or not a name at all: no table.
        assert!(class_info(&mm, "org.child@1.0.0.Missing").is_none());
        assert!(class_info(&mm, "String").is_none());
    }
}

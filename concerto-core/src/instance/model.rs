//! The model queries the instance layer makes, answered from the
//! [`ModelManager`] arena with TS's own semantics for each declaration kind
//! (the `Declaration` defaults, and the `ClassDeclaration` members that
//! `EnumDeclaration` inherits, including `isClassDeclaration() === true`).
//!
//! These are the collaborator calls of `Factory`, `JSONPopulator` and
//! `JSONGenerator` (`modelManager.getType(...)`, then `classDecl.isX()`),
//! kept in one place so that each port reads the same way as its TS.

use crate::error::{ContractError, ErrorKind, Result};
use crate::introspect::scalar::ScalarValidator;
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
                self.mm.identifier_field_name(&self.fqn())
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
        Ok(self
            .properties("classDeclaration.getProperty")?
            .into_iter()
            .find(|(_, p)| p.name() == name))
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

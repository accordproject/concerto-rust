//! The model queries the instance layer makes, answered from the
//! [`ModelManager`] arena with TS's own semantics for each declaration kind
//! (the `Declaration` defaults, and the `ClassDeclaration` members that
//! `EnumDeclaration` inherits, including `isClassDeclaration() === true`).
//!
//! These are the collaborator calls of `Factory`, `JSONPopulator` and
//! `JSONGenerator` (`modelManager.getType(...)`, then `classDecl.isX()`),
//! kept in one place so that each port reads the same way as its TS.

use serde_json::Value;

use crate::Error;
use crate::error::{ContractError, ErrorKind, Result};
use crate::introspect::FullyQualified;
use crate::introspect::scalar::ScalarValidator;
use crate::introspect::validators::StringValidator;
use crate::introspect::{ClassKind, Declaration, Property};
use crate::model_manager::{ClassProperties, DeclId, ModelManager, ValidatedElement};
use crate::model_util;

/// A declaration found by [`get_type`]: what TS holds after
/// `modelManager.getType(name)`.
#[derive(Clone, Copy)]
pub struct TypeRef<'a> {
    /// The model manager the declaration is in.
    pub mm: &'a ModelManager,
    /// The declaration's handle.
    pub id: DeclId,
    /// The declaration.
    pub decl: &'a Declaration,
}

/// TS: `modelManager.getType(qualifiedName)` (`BaseModelManager.getType`).
pub fn get_type<'a>(mm: &'a ModelManager, qualified_name: &str) -> Result<TypeRef<'a>> {
    let id = mm.type_declaration(qualified_name)?;
    let decl = mm
        .declaration(id)
        .expect("get_type_declaration returns a live handle");
    Ok(TypeRef { mm, id, decl })
}

/// V8's `TypeError: <expression> is not a function`.
pub fn not_a_function(expression: &str) -> crate::Error {
    ContractError::new(
        ErrorKind::MalformedInput,
        "engine-typeerror-notafunction",
        vec![("expression", expression.to_string())],
    )
    .into()
}

/// `'Unrecognised ' + JSON.stringify(thing)` for an introspection object:
/// `JSON.stringify` throws V8's circular-structure `TypeError` first. DV-010
pub fn unrecognised() -> crate::Error {
    ContractError::new(
        ErrorKind::MalformedInput,
        "engine-typeerror-circularjson",
        Vec::new(),
    )
    .into()
}

impl<'a> TypeRef<'a> {
    /// TS `getFullyQualifiedName()`, which the manager keeps for every
    /// declaration (P5-13).
    pub fn fqn(&self) -> &'a str {
        self.mm.decl_fqn(self.id).unwrap_or_default()
    }

    /// TS `getNamespace()`: the model file's namespace.
    pub fn namespace(&self) -> &'a str {
        self.mm
            .model_file_of(self.id)
            .and_then(|f| self.mm.file(f))
            .map(|f| f.namespace())
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
    pub fn identifier_field_name(&self) -> Result<Option<&'a str>> {
        match self.decl {
            Declaration::Class(_) | Declaration::Enum(_) => self.mm.identifier_field_of(self.id),
            Declaration::Scalar(_) | Declaration::Map(_) => Ok(None),
        }
    }

    /// TS `isIdentified()`.
    pub fn is_identified(&self) -> Result<bool> {
        Ok(self.identifier_field_name()?.is_some())
    }

    /// TS `isSystemIdentified()`.
    pub fn is_system_identified(&self) -> Result<bool> {
        Ok(self.identifier_field_name()? == Some("$identifier"))
    }

    /// TS `getProperties()`: each property with the fully-qualified name of
    /// the declaration that declares it, borrowed from the model (P5-13). A
    /// map or a scalar has none of its own (V8's `TypeError`, which no
    /// instance path reaches with one).
    pub fn properties(&self, expression: &str) -> Result<ClassProperties<'a>> {
        match self.decl {
            Declaration::Class(_) | Declaration::Enum(_) => self.mm.class_properties_of(self.id),
            _ => Err(not_a_function(expression)),
        }
    }

    /// TS `getProperty(name)`.
    pub fn property(&self, name: &str) -> Result<Option<(&'a str, &'a Property)>> {
        match self.decl {
            Declaration::Class(_) | Declaration::Enum(_) => {
                Ok(self.mm.class_properties_of(self.id)?.find(name))
            }
            _ => Err(not_a_function("classDeclaration.getProperty")),
        }
    }
}

/// What a field's declared type resolves to, in the model file of the
/// declaration that declares it (`Field.isPrimitive`, `isTypeEnum`,
/// `isTypeScalar`, `ModelUtil.isMap`, and `RelationshipDeclaration`),
/// borrowed from the model (P5-13).
#[derive(Debug, Clone)]
pub enum FieldType<'a> {
    /// A primitive field (`isPrimitive()`): its type name.
    Primitive(&'static str),
    /// A field whose type is a scalar (`isTypeScalar()`): the primitive it
    /// aliases, its default value and its validator (what
    /// `getScalarField()` copies from the scalar's AST).
    Scalar {
        /// The primitive type the scalar aliases.
        primitive: Option<&'static str>,
        /// The scalar's default value.
        default_value: Option<&'a serde_json::Value>,
        /// The scalar's validator.
        validator: Option<&'a ScalarValidator>,
    },
    /// A field whose type is an enum (`isTypeEnum()`).
    Enum(&'a str),
    /// A field whose type is a map (`ModelUtil.isMap(field)`).
    Map(&'a str),
    /// A field whose type is a concept-like declaration.
    Class(&'a str),
    /// A relationship: the fully-qualified name of its target.
    Relationship(String),
    /// An enum value member (an `EnumDeclaration`'s own property), which
    /// has no type.
    EnumValue,
}

/// A property of an instance's declaration, with what its type resolves
/// to, borrowed from the model (P5-13).
#[derive(Debug, Clone)]
pub struct Field<'a> {
    /// The fully-qualified name of the declaration that declares it
    /// (`getParent().getFullyQualifiedName()`).
    pub owner_fqn: &'a str,
    /// The property.
    pub property: &'a Property,
    /// What its type resolves to.
    pub field_type: FieldType<'a>,
}

impl Field<'_> {
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
    pub fn type_name(&self) -> &str {
        match &self.field_type {
            FieldType::Scalar { primitive, .. } => primitive.unwrap_or_default(),
            _ => self.property.type_name().unwrap_or_default(),
        }
    }

    /// TS `getFullyQualifiedTypeName()`.
    pub fn fully_qualified_type_name(&self) -> &str {
        match &self.field_type {
            FieldType::Primitive(p) => p,
            FieldType::Scalar { primitive, .. } => primitive.unwrap_or_default(),
            FieldType::Enum(f) | FieldType::Map(f) | FieldType::Class(f) => f,
            FieldType::Relationship(f) => f,
            FieldType::EnumValue => "",
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

    /// The relationship slot of a relationship property (`--> T field`),
    /// or `None` for any other field.
    pub fn relationship_slot(&self) -> Option<RelationshipSlot<'_>> {
        let FieldType::Relationship(target_fqn) = &self.field_type else {
            return None;
        };
        Some(RelationshipSlot {
            owner_fqn: self.owner_fqn,
            name: self.name(),
            target_fqn,
            is_array: self.property.is_array(),
            is_optional: self.property.is_optional(),
            map_value: false,
        })
    }
}

/// Where a relationship is held (P5-58, BC-05, R1): a relationship property
/// (`--> T field`), or the value of a map declared with a relationship
/// value type (`map M { o String --> T }`). The populator's and the
/// generator's relationship code takes one of these, so a map value is
/// read and written by the same code as a relationship property, under the
/// same `acceptResourcesForRelationships`, `convertResourcesToRelationships`
/// and `permitResourcesForRelationships` options.
#[derive(Debug, Clone, Copy)]
pub struct RelationshipSlot<'a> {
    /// The fully-qualified name of the class that declares the property,
    /// or of the map.
    pub owner_fqn: &'a str,
    /// The property's name, or the map's name for a map value.
    pub name: &'a str,
    /// The fully-qualified name of the declared target type.
    pub target_fqn: &'a str,
    /// TS `isArray()`: `false` for a map value.
    pub is_array: bool,
    /// TS `isOptional()`: `false` for a map value.
    pub is_optional: bool,
    /// Whether this is a map's value rather than a property.
    pub map_value: bool,
}

impl RelationshipSlot<'_> {
    /// TS `RelationshipDeclaration.toString()` for a property; for a map
    /// value, the same shape naming the map.
    pub fn relationship_to_string(&self) -> String {
        if self.map_value {
            format!(
                "RelationshipMapValueType {{map={}, type={}}}",
                self.owner_fqn, self.target_fqn,
            )
        } else {
            format!(
                "RelationshipDeclaration {{name={}, type={}, array={}, optional={}}}",
                self.name, self.target_fqn, self.is_array, self.is_optional,
            )
        }
    }
}

/// Whether a map's value type is a relationship (`RelationshipMapValueType`,
/// `map M { o String --> T }`).
pub fn is_relationship_map(map_declaration: &TypeRef) -> bool {
    matches!(map_declaration.decl, Declaration::Map(map) if map.value_kind() == "RelationshipMapValueType")
}

/// The fully-qualified target type of a map whose value type is a
/// relationship (`RelationshipMapValueType`, P5-58, BC-05, R1), resolved in
/// the map's own model file as a relationship property's type is; `None`
/// for any other map. Pair it with the map's [`TypeRef`] to build its
/// [`RelationshipSlot`] with [`map_relationship_slot`].
pub fn map_relationship_target(map_declaration: &TypeRef) -> Result<Option<String>> {
    let Declaration::Map(map) = map_declaration.decl else {
        return Ok(None);
    };
    if !is_relationship_map(map_declaration) {
        return Ok(None);
    }
    let Some(type_id) = map.value_type() else {
        return Ok(None);
    };
    let fqn = map_declaration
        .mm
        .resolve_type_name_at(map_declaration.namespace(), &type_id.name, None)?;
    Ok(Some(fqn))
}

/// The [`RelationshipSlot`] of a map's relationship-typed value, whose
/// target [`map_relationship_target`] resolved.
pub fn map_relationship_slot<'a>(
    map_declaration: &TypeRef<'a>,
    target_fqn: &'a str,
) -> RelationshipSlot<'a> {
    RelationshipSlot {
        owner_fqn: map_declaration.fqn(),
        name: map_declaration.name(),
        target_fqn,
        is_array: false,
        is_optional: false,
        map_value: true,
    }
}

/// Resolves a property's declared type in its owner's model file.
pub fn field<'a>(
    mm: &'a ModelManager,
    owner_fqn: &'a str,
    property: &'a Property,
) -> Result<Field<'a>> {
    let field_type = match property {
        Property::Relationship(rp) => {
            let namespace = model_util::get_namespace(Some(owner_fqn))?;
            FieldType::Relationship(mm.resolve_type_name_at(namespace, &rp.type_.name, None)?)
        }
        Property::Object(op) => {
            let namespace = model_util::get_namespace(Some(owner_fqn))?;
            let fqn = mm.resolve_type_name_at(namespace, &op.type_.name, None)?;
            // `get_declaration(&fqn)`, keeping the handle for its name.
            let id = mm
                .declaration_id(&fqn)
                .ok_or_else(|| Error::type_not_found(fqn.clone()))?;
            let target = mm.decl_fqn(id)?;
            match mm
                .declaration(id)
                .expect("declaration_id returns a live handle")
            {
                Declaration::Enum(_) => FieldType::Enum(target),
                Declaration::Scalar(s) => FieldType::Scalar {
                    primitive: s.processed_type(),
                    default_value: s.default_value(),
                    validator: s.validator(),
                },
                Declaration::Map(_) => FieldType::Map(target),
                Declaration::Class(_) => FieldType::Class(target),
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
        owner_fqn,
        property,
        field_type,
    })
}

/// The element a string validator is attached to, for building one: its
/// name and fully-qualified name only (the default value was checked when
/// the model loaded).
struct IdElement {
    name: String,
    fqn: String,
}

impl FullyQualified for IdElement {
    type Error = Error;

    fn fully_qualified_name(&self) -> Result<String> {
        Ok(self.fqn.clone())
    }
}

impl ValidatedElement for IdElement {
    fn default_value(&self) -> Result<Option<Value>> {
        Ok(None)
    }

    fn name(&self) -> Result<String> {
        Ok(self.name.clone())
    }
}

/// `idFullField?.validator` when it has a `regex`: the identifying
/// property (unboxed with `getScalarField()` when its type is a scalar) and
/// its string validator.
pub fn identifier_regex(
    class_decl: &TypeRef,
    id_field: &str,
) -> Result<Option<StringValidator>> {
    let Some((owner_fqn, property)) = class_decl.property(id_field)? else {
        return Ok(None);
    };
    let element = IdElement {
        name: id_field.to_string(),
        fqn: format!("{owner_fqn}.{id_field}"),
    };
    let field = field(class_decl.mm, owner_fqn, property)?;
    let validator = match (&field.field_type, field.property) {
        (FieldType::Primitive("String"), Property::String(sp)) if sp.validator.is_some() => {
            StringValidator::new(
                &element,
                sp.validator.as_ref(),
                sp.length_validator.as_ref(),
                None,
            )?
        }
        (
            FieldType::Scalar {
                primitive: Some("String"),
                validator: Some(scalar_validator),
                ..
            },
            _,
        ) => {
            let ScalarValidator::String {
                validator: Some(regex),
                length_validator,
            } = scalar_validator
            else {
                return Ok(None);
            };
            let bad = |e: serde_json::Error| {
                Error::from(ContractError::pre_port(
                    ErrorKind::InvalidArgument,
                    format!("invalid string validator: {e}"),
                    None,
                ))
            };
            let regex = serde_json::from_value(regex.clone()).map_err(bad)?;
            let length = length_validator
                .as_ref()
                .map(|v| serde_json::from_value(v.clone()).map_err(bad))
                .transpose()?;
            StringValidator::new(&element, Some(&regex), length.as_ref(), None)?
        }
        _ => return Ok(None),
    };
    Ok(validator.regex().is_some().then_some(validator))
}

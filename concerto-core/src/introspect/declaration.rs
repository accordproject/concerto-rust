//! Internal representations of the Concerto declarations.
//!
//! Concerto's JavaScript runtime models declarations as an inheritance
//! hierarchy: concept, asset, participant, transaction and event all extend a
//! common class declaration. Inheritance like that isn't idiomatic in Rust, so
//! the five class-like declarations are represented by a single
//! [`ClassDeclaration`] over the matching generated `mm::*Declaration` struct,
//! tagged with a [`ClassKind`], while enums, scalars and maps are the other
//! variants of the [`Declaration`] sum type. Each variant is selected by
//! matching on the node's `$class`.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde::de::Error as _;

use crate::derive::{DeclarationKind, Named};
use crate::error::{ConcertoError, Result};
use crate::introspect::property::Property;
use crate::introspect::scalar::{self, ScalarDeclaration};
use crate::introspect::{
    DeclarationKind, Decorated, HasValidators, Named, Typed, declared_class, qualified_class,
};
use crate::model_util::{
    MAP_KEY_KINDS, MAP_VALUE_KINDS, get_fully_qualified_name, get_short_name, is_valid_identifier,
};

/// Which class-like declaration a [`ClassDeclaration`] represents. Its
/// [`DeclarationKind`] is the metamodel `$class` short name for the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, DeclarationKind)]
pub enum ClassKind {
    /// A `concept`.
    #[concerto(kind = "ConceptDeclaration")]
    Concept,
    /// An `asset` (system-identifiable).
    #[concerto(kind = "AssetDeclaration")]
    Asset,
    /// A `participant` (system-identifiable).
    #[concerto(kind = "ParticipantDeclaration")]
    Participant,
    /// A `transaction`.
    #[concerto(kind = "TransactionDeclaration")]
    Transaction,
    /// An `event`.
    #[concerto(kind = "EventDeclaration")]
    Event,
}

impl ClassKind {
    fn from_short(short: &str) -> Option<Self> {
        Some(match short {
            "ConceptDeclaration" => Self::Concept,
            "AssetDeclaration" => Self::Asset,
            "ParticipantDeclaration" => Self::Participant,
            "TransactionDeclaration" => Self::Transaction,
            "EventDeclaration" => Self::Event,
            _ => return None,
        })
    }
}

/// The generated struct behind a [`ClassDeclaration`], one variant per kind.
#[derive(Debug, Clone)]
enum ClassNode {
    Concept(mm::ConceptDeclaration),
    Asset(mm::AssetDeclaration),
    Participant(mm::ParticipantDeclaration),
    Transaction(mm::TransactionDeclaration),
    Event(mm::EventDeclaration),
}

/// Picks the same field out of whichever generated struct a [`ClassNode`]
/// holds; the five share the metamodel's class declaration fields.
macro_rules! class_field {
    ($node:expr, $decl:ident => $field:expr) => {
        match $node {
            ClassNode::Concept($decl) => $field,
            ClassNode::Asset($decl) => $field,
            ClassNode::Participant($decl) => $field,
            ClassNode::Transaction($decl) => $field,
            ClassNode::Event($decl) => $field,
        }
    };
}

/// A concept-like declaration: concept, asset, participant, transaction or
/// event, distinguished by [`ClassDeclaration::kind`].
///
/// It wraps the generated `mm::*Declaration` struct for its kind. The one
/// field held apart is the property list: a class declaration's `properties`
/// may hold an `EnumProperty`, which the generated `mm::Property` union does
/// not cover, so each property is kept as a [`Property`] (itself a newtype
/// over its generated struct) and the generated struct's own `properties` is
/// left empty.
#[derive(Debug, Clone)]
pub struct ClassDeclaration {
    node: ClassNode,
    properties: Vec<Property>,
}

impl ClassDeclaration {
    /// The kind of class-like declaration this is.
    pub fn kind(&self) -> ClassKind {
        match self.node {
            ClassNode::Concept(_) => ClassKind::Concept,
            ClassNode::Asset(_) => ClassKind::Asset,
            ClassNode::Participant(_) => ClassKind::Participant,
            ClassNode::Transaction(_) => ClassKind::Transaction,
            ClassNode::Event(_) => ClassKind::Event,
        }
    }

    /// Abstract types can't be instantiated on their own.
    pub fn is_abstract(&self) -> bool {
        class_field!(&self.node, d => d.is_abstract)
    }

    /// The super type this declaration extends, if it extends one.
    pub fn super_type(&self) -> Option<&mm::TypeIdentifier> {
        class_field!(&self.node, d => d.super_type.as_ref())
    }

    /// The properties declared directly on this type. Inherited properties are
    /// not included; those are gathered separately by walking the supertype
    /// chain.
    pub fn own_properties(&self) -> &[Property] {
        &self.properties
    }

    /// The source location, if the AST carried one.
    pub fn location(&self) -> Option<&mm::Range> {
        class_field!(&self.node, d => d.location.as_ref())
    }

    /// True if the type has an identity, whether system-assigned or explicit.
    pub fn is_identified(&self) -> bool {
        self.identified().is_some()
    }

    /// The name of the field that provides identity, for a type that is
    /// identified by one of its own fields (`identified by field`). A
    /// system-identified type (`identified`) or a type with no identity both
    /// return `None`.
    pub fn identifier_field_name(&self) -> Option<&str> {
        match self.identified() {
            Some(mm::Identified::IdentifiedBy(by)) => Some(&by.name),
            _ => None,
        }
    }

    fn identified(&self) -> Option<&mm::Identified> {
        class_field!(&self.node, d => d.identified.as_ref())
    }

    /// Reads the declaration fields into the generated struct for `kind`, then
    /// each property into a [`Property`]. The properties are read from the
    /// node itself, so the generated struct is given an empty list.
    fn from_json(kind: ClassKind, value: &serde_json::Value) -> Result<Self> {
        let mut fields = value.clone();
        if let Some(object) = fields.as_object_mut() {
            object.insert("properties".into(), serde_json::Value::Array(Vec::new()));
        }
        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid {}: {e}", kind.declaration_kind()),
            file_name: None,
            location: None,
        };
        let node = match kind {
            ClassKind::Concept => ClassNode::Concept(serde_json::from_value(fields).map_err(bad)?),
            ClassKind::Asset => ClassNode::Asset(serde_json::from_value(fields).map_err(bad)?),
            ClassKind::Participant => {
                ClassNode::Participant(serde_json::from_value(fields).map_err(bad)?)
            }
            ClassKind::Transaction => {
                ClassNode::Transaction(serde_json::from_value(fields).map_err(bad)?)
            }
            ClassKind::Event => ClassNode::Event(serde_json::from_value(fields).map_err(bad)?),
        };
        Ok(Self {
            node,
            properties: parse_properties(value)?,
        })
    }
}

impl Named for ClassDeclaration {
    /// The declaration's short name (without namespace).
    fn name(&self) -> &str {
        class_field!(&self.node, d => &d.name)
    }
}

impl Decorated for ClassDeclaration {
    fn decorators(&self) -> &[mm::Decorator] {
        class_field!(&self.node, d => d.decorators.as_deref().unwrap_or(&[]))
    }
}

impl DeclarationKind for ClassDeclaration {
    fn declaration_kind(&self) -> &'static str {
        self.kind().declaration_kind()
    }
}

/// Loads a scalar declaration: the generated node for its `$class` (the
/// loader's structural check), then the ported `ScalarDeclaration.process`.
/// The name is checked first, as `Declaration.process` runs before it in TS.
fn load_scalar(
    short: &str,
    value: &serde_json::Value,
    namespace: &str,
    file_name: Option<&str>,
) -> Result<ScalarDeclaration> {
    let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
        message: format!("invalid {short}: {e}"),
        file_name: None,
        location: None,
    };
    let v = value.clone();
    let node = match short {
        "BooleanScalar" => {
            mm::ScalarDeclaration::BooleanScalar(serde_json::from_value(v).map_err(bad)?)
        }
        "IntegerScalar" => {
            mm::ScalarDeclaration::IntegerScalar(serde_json::from_value(v).map_err(bad)?)
        }
        "LongScalar" => mm::ScalarDeclaration::LongScalar(serde_json::from_value(v).map_err(bad)?),
        "DoubleScalar" => {
            mm::ScalarDeclaration::DoubleScalar(serde_json::from_value(v).map_err(bad)?)
        }
        "StringScalar" => {
            mm::ScalarDeclaration::StringScalar(serde_json::from_value(v).map_err(bad)?)
        }
        "DateTimeScalar" => {
            mm::ScalarDeclaration::DateTimeScalar(serde_json::from_value(v).map_err(bad)?)
        }
        other => {
            return Err(ConcertoError::IllegalModel {
                message: format!("unknown scalar type: {other}"),
                file_name: None,
                location: None,
            });
        }
    };
    let name = scalar::node_name(&node);
    check_identifier(name)?;
    let fqn = get_fully_qualified_name(namespace, name);
    let processed =
        ScalarDeclaration::process(value, file_name, &|| Ok::<_, ConcertoError>(fqn.clone()))?;
    let scalar = ScalarDeclaration::new(node, processed);
    scalar.check_validators()?;
    Ok(scalar)
}

/// A top-level declaration within a model file.
#[derive(Debug, Clone, Named, DeclarationKind)]
#[concerto(delegate)]
#[allow(clippy::large_enum_variant)]
pub enum Declaration {
    /// A concept-like declaration (see [`ClassDeclaration`]).
    Class(ClassDeclaration),
    /// An enumeration.
    Enum(EnumDeclaration),
    /// A scalar alias over a primitive.
    Scalar(ScalarDeclaration),
    /// A map type.
    Map(MapDeclaration),
}

/// An enumeration declaration: a newtype over the generated
/// [`mm::EnumDeclaration`].
#[derive(Debug, Clone, Named, DeclarationKind)]
#[concerto(kind = "EnumDeclaration")]
pub struct EnumDeclaration(mm::EnumDeclaration);

/// A map declaration.
///
/// A map is read for its name, and for the kind (the `$class` short name) and
/// the referenced `type`, if any, of its key and of its value. The map's own
/// decorators and location, and those of its key and value, are not read.
///
/// [`MapDeclaration::Typed`] is a newtype over the generated
/// [`mm::MapDeclaration`]. It is used whenever that struct can hold all four
/// of those facts; a malformed `decorators` or `location` is left out of it,
/// because it is not read.
///
/// [`MapDeclaration::Untyped`] keeps the four facts as read from the node. It
/// is used only when the key or the value (or both) is something
/// `mm::MapKeyType` or `mm::MapValueType` cannot represent:
///
/// - it is missing, or its kind is not one the metamodel declares (for
///   example `IntegerMapKeyType`);
/// - its kind carries a `type`, but the `type` is missing or is not a
///   `TypeIdentifier`;
/// - its kind carries no `type`, but the node has a well-formed `type` anyway.
///
/// Either way, a key or value kind the specification does not allow reaches
/// semantic validation, which reports it, and a referenced type is checked
/// there whichever variant holds it.
#[derive(Debug, Clone, Named, DeclarationKind)]
#[concerto(kind = "MapDeclaration")]
#[allow(clippy::large_enum_variant)]
pub enum MapDeclaration {
    /// The key and value are both representable by the generated union types.
    Typed(mm::MapDeclaration),
    /// The key or value is not representable by the generated union types.
    Untyped {
        /// The map's short name.
        name: String,
        /// The `$class` short name of the key node, or `""` if there is none.
        key_kind: String,
        /// The type the key node refers to, if it has a well-formed `type`.
        key_type: Option<mm::TypeIdentifier>,
        /// The `$class` short name of the value node, or `""` if there is none.
        value_kind: String,
        /// The type the value node refers to, if it has a well-formed `type`.
        value_type: Option<mm::TypeIdentifier>,
    },
}

impl MapDeclaration {
    /// The metamodel `$class` short name of the key node, such as
    /// `StringMapKeyType`.
    pub fn key_kind(&self) -> &str {
        match self {
            Self::Typed(m) => match &m.key {
                mm::MapKeyType::StringMapKeyType(_) => "StringMapKeyType",
                mm::MapKeyType::DateTimeMapKeyType(_) => "DateTimeMapKeyType",
                mm::MapKeyType::ObjectMapKeyType(_) => "ObjectMapKeyType",
            },
            Self::Untyped { key_kind, .. } => key_kind,
        }
    }

    /// The metamodel `$class` short name of the value node, such as
    /// `ObjectMapValueType`.
    pub fn value_kind(&self) -> &str {
        match self {
            Self::Typed(m) => match &m.value {
                mm::MapValueType::BooleanMapValueType(_) => "BooleanMapValueType",
                mm::MapValueType::DateTimeMapValueType(_) => "DateTimeMapValueType",
                mm::MapValueType::StringMapValueType(_) => "StringMapValueType",
                mm::MapValueType::IntegerMapValueType(_) => "IntegerMapValueType",
                mm::MapValueType::LongMapValueType(_) => "LongMapValueType",
                mm::MapValueType::DoubleMapValueType(_) => "DoubleMapValueType",
                mm::MapValueType::ObjectMapValueType(_) => "ObjectMapValueType",
                mm::MapValueType::RelationshipMapValueType(_) => "RelationshipMapValueType",
            },
            Self::Untyped { value_kind, .. } => value_kind,
        }
    }

    /// The type the key refers to, for a key that is not a primitive.
    pub fn key_type(&self) -> Option<&mm::TypeIdentifier> {
        match self {
            Self::Typed(m) => match &m.key {
                mm::MapKeyType::ObjectMapKeyType(k) => Some(&k.type_),
                _ => None,
            },
            Self::Untyped { key_type, .. } => key_type.as_ref(),
        }
    }

    /// The type the value refers to, for a value that is not a primitive.
    pub fn value_type(&self) -> Option<&mm::TypeIdentifier> {
        match self {
            Self::Typed(m) => match &m.value {
                mm::MapValueType::ObjectMapValueType(v) => Some(&v.type_),
                mm::MapValueType::RelationshipMapValueType(v) => Some(&v.type_),
                _ => None,
            },
            Self::Untyped { value_type, .. } => value_type.as_ref(),
        }
    }

    /// `MapKeyType.getType` (src/introspect/mapkeytype.ts): the primitive
    /// name for a `String`/`DateTime` key, or the raw (unresolved) referenced
    /// type name for an object key, exactly as `processType` sets `this.type`
    /// from `this.ast.type.name` without consulting the model manager.
    pub fn key_type_name(&self) -> &str {
        match self.key_kind() {
            "DateTimeMapKeyType" => "DateTime",
            "StringMapKeyType" => "String",
            _ => self.key_type().map_or("", |t| t.name.as_str()),
        }
    }

    /// `MapValueType.getType` (src/introspect/mapvaluetype.ts): the primitive
    /// name for a primitive value, or the raw (unresolved) referenced type
    /// name for an object or relationship value.
    pub fn value_type_name(&self) -> &str {
        match self.value_kind() {
            "BooleanMapValueType" => "Boolean",
            "DateTimeMapValueType" => "DateTime",
            "StringMapValueType" => "String",
            "IntegerMapValueType" => "Integer",
            "LongMapValueType" => "Long",
            "DoubleMapValueType" => "Double",
            _ => self.value_type().map_or("", |t| t.name.as_str()),
        }
    }

    /// `MapDeclaration.toString` (src/introspect/mapdeclaration.ts):
    /// `MapDeclaration {id=<fully qualified name>}`.
    pub fn to_string(fully_qualified_name: &str) -> String {
        format!("MapDeclaration {{id={fully_qualified_name}}}")
    }

    fn from_json(value: &serde_json::Value) -> Result<Self> {
        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid MapDeclaration: {e}"),
            file_name: None,
            location: None,
        };
        let name: String = match value.get("name") {
            Some(name) => serde_json::from_value(name.clone()).map_err(bad)?,
            None => return Err(bad(serde_json::Error::missing_field("name"))),
        };

        let key_kind = node_kind(value.get("key"));
        let key_type = type_reference(value.get("key"));
        let value_kind = node_kind(value.get("value"));
        let value_type = type_reference(value.get("value"));

        if let Some(typed) = typed_map(value, &key_kind, &value_kind).map(Self::Typed)
            && typed.key_type().is_some() == key_type.is_some()
            && typed.value_type().is_some() == value_type.is_some()
        {
            return Ok(typed);
        }
        Ok(Self::Untyped {
            name,
            key_kind,
            key_type,
            value_kind,
            value_type,
        })
    }
}

/// Deserializes a map node into the generated [`mm::MapDeclaration`], if its
/// key and value kinds are ones the generated unions declare. The key and
/// value `$class` are qualified from their short names, and a `decorators` or
/// `location` that does not deserialize is dropped, since neither is read.
fn typed_map(
    value: &serde_json::Value,
    key_kind: &str,
    value_kind: &str,
) -> Option<mm::MapDeclaration> {
    if !MAP_KEY_KINDS.contains(&key_kind) || !MAP_VALUE_KINDS.contains(&value_kind) {
        return None;
    }
    let mut node = value.clone();
    let map = node.as_object_mut()?;
    drop_unreadable_annotations(map);
    for (field, kind) in [("key", key_kind), ("value", value_kind)] {
        let part = map.get_mut(field)?.as_object_mut()?;
        part.insert(
            "$class".into(),
            serde_json::Value::String(qualified_class(kind)),
        );
        drop_unreadable_annotations(part);
    }
    serde_json::from_value(node).ok()
}

/// Removes a `decorators` or `location` entry that does not deserialize into
/// its generated type.
fn drop_unreadable_annotations(node: &mut serde_json::Map<String, serde_json::Value>) {
    if node
        .get("decorators")
        .is_some_and(|d| serde_json::from_value::<Option<Vec<mm::Decorator>>>(d.clone()).is_err())
    {
        node.remove("decorators");
    }
    if node
        .get("location")
        .is_some_and(|l| serde_json::from_value::<Option<mm::Range>>(l.clone()).is_err())
    {
        node.remove("location");
    }
}

/// The `$class` short name of a map key or value node.
fn node_kind(node: Option<&serde_json::Value>) -> String {
    node.map(|n| get_short_name(declared_class(n)).to_string())
        .unwrap_or_default()
}

/// The type a map key or value node points at. Primitive keys and values carry
/// no reference, so they give `None`.
fn type_reference(node: Option<&serde_json::Value>) -> Option<mm::TypeIdentifier> {
    serde_json::from_value(node?.get("type")?.clone()).ok()
}

impl Typed for Declaration {
    /// The primitive type of a scalar declaration; every other declaration has
    /// none.
    fn type_name(&self) -> Option<&str> {
        match self {
            Self::Scalar(s) => s.type_name(),
            Self::Class(_) | Self::Enum(_) | Self::Map(_) => None,
        }
    }
}

impl Declaration {
    /// Borrow this as a [`ClassDeclaration`], if it is one.
    pub fn as_class(&self) -> Option<&ClassDeclaration> {
        match self {
            Self::Class(c) => Some(c),
            _ => None,
        }
    }

    /// Borrow this as a [`ScalarDeclaration`], if it is one.
    pub fn as_scalar(&self) -> Option<&ScalarDeclaration> {
        match self {
            Self::Scalar(s) => Some(s),
            _ => None,
        }
    }

    /// Borrow this as a [`MapDeclaration`], if it is one.
    pub fn as_map(&self) -> Option<&MapDeclaration> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// `true` if this is a concept-like (class) declaration.
    pub fn is_class_declaration(&self) -> bool {
        matches!(self, Self::Class(_))
    }

    /// `true` if this is an enum declaration.
    pub fn is_enum_declaration(&self) -> bool {
        matches!(self, Self::Enum(_))
    }

    /// `true` if this is a scalar declaration.
    pub fn is_scalar_declaration(&self) -> bool {
        matches!(self, Self::Scalar(_))
    }

    /// `true` if this is a map declaration.
    pub fn is_map_declaration(&self) -> bool {
        matches!(self, Self::Map(_))
    }
}

fn parse_properties(value: &serde_json::Value) -> Result<Vec<Property>> {
    match value.get("properties") {
        None => Ok(Vec::new()),
        Some(serde_json::Value::Array(arr)) => arr.iter().map(Property::try_from).collect(),
        Some(_) => Err(ConcertoError::IllegalModel {
            message: "'properties' must be an array".into(),
            file_name: None,
            location: None,
        }),
    }
}

impl TryFrom<&serde_json::Value> for Declaration {
    type Error = ConcertoError;

    /// Loads a declaration outside any namespace or file.
    fn try_from(value: &serde_json::Value) -> Result<Self> {
        Self::from_model_json(value, "", None)
    }
}

impl Declaration {
    /// Loads a declaration of the model file for `namespace`, named
    /// `file_name`: both are what the TS declaration reads from its model file
    /// when it reports an error.
    pub(crate) fn from_model_json(
        value: &serde_json::Value,
        namespace: &str,
        file_name: Option<&str>,
    ) -> Result<Self> {
        let class = declared_class(value);
        if class.is_empty() {
            return Err(ConcertoError::IllegalModel {
                message: "declaration node is missing its $class".into(),
                file_name: None,
                location: None,
            });
        }
        let kind = get_short_name(class);

        if let Some(class_kind) = ClassKind::from_short(kind) {
            let class = Self::Class(ClassDeclaration::from_json(class_kind, value)?);
            return check_name(class);
        }

        let declaration = match kind {
            "EnumDeclaration" => Self::Enum(EnumDeclaration(
                serde_json::from_value(value.clone()).map_err(|e| ConcertoError::IllegalModel {
                    message: format!("invalid EnumDeclaration: {e}"),
                    file_name: None,
                    location: None,
                })?,
            )),
            "MapDeclaration" => Self::Map(MapDeclaration::from_json(value)?),
            s if s.ends_with("Scalar") => {
                Self::Scalar(load_scalar(s, value, namespace, file_name)?)
            }
            other => {
                return Err(ConcertoError::IllegalModel {
                    message: format!("unknown declaration type: {other}"),
                    file_name: None,
                    location: None,
                });
            }
        };
        check_name(declaration)
    }
}

/// Every declaration name has to be a legal identifier.
fn check_name(declaration: Declaration) -> Result<Declaration> {
    check_identifier(declaration.name())?;
    Ok(declaration)
}

fn check_identifier(name: &str) -> Result<()> {
    if is_valid_identifier(name) {
        Ok(())
    } else {
        Err(ConcertoError::IllegalModel {
            message: format!("invalid identifier: {name}"),
            file_name: None,
            location: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(json: serde_json::Value) -> Declaration {
        Declaration::try_from(&json).expect("valid declaration")
    }

    #[test]
    fn parses_concept_with_typed_properties() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Person",
            "isAbstract": false,
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Thing" },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "firstName", "isArray": false, "isOptional": false },
                { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "age", "isArray": false, "isOptional": true }
            ]
        }));

        let c = d.as_class().expect("class");
        assert_eq!(c.kind(), ClassKind::Concept);
        assert_eq!(c.name(), "Person");
        assert!(!c.is_abstract());
        assert_eq!(c.super_type().map(|t| t.name.as_str()), Some("Thing"));
        assert_eq!(c.own_properties().len(), 2);
        assert_eq!(c.own_properties()[0].type_name(), Some("String"));
        assert!(c.own_properties()[1].is_optional());

        assert!(d.is_class_declaration());
        assert!(!d.is_enum_declaration());
    }

    #[test]
    fn asset_kind_is_tagged() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
            "name": "Vehicle",
            "isAbstract": false,
            "properties": []
        }));
        assert_eq!(d.declaration_kind(), "AssetDeclaration");
        assert_eq!(d.as_class().unwrap().kind(), ClassKind::Asset);
    }

    #[test]
    fn parses_enum_and_scalar_and_map() {
        let e = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.EnumDeclaration",
            "name": "Color",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" },
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "GREEN" }
            ]
        }));
        assert!(e.is_enum_declaration());
        assert!(!e.is_class_declaration());
        assert_eq!(e.name(), "Color");

        let s = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringScalar",
            "name": "Email",
            "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": ".*", "flags": "" }
        }));
        assert_eq!(s.as_scalar().unwrap().scalar_type(), Some("String"));
        assert_eq!(s.name(), "Email");
        assert!(s.is_scalar_declaration());

        let m = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "Dictionary",
            "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
            "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" }
        }));
        assert!(m.is_map_declaration());
        assert_eq!(m.name(), "Dictionary");
    }

    #[test]
    fn unknown_declaration_kind_errors() {
        assert!(
            Declaration::try_from(&serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.WidgetDeclaration",
                "name": "X"
            }))
            .is_err()
        );
    }

    #[test]
    fn missing_class_is_rejected() {
        let err = Declaration::try_from(&serde_json::json!({ "name": "X" }));
        assert!(err.unwrap_err().to_string().contains("$class"));
    }

    #[test]
    fn non_array_properties_is_rejected() {
        assert!(
            Declaration::try_from(&serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Bad",
                "properties": { "not": "an array" }
            }))
            .is_err()
        );
    }

    #[test]
    fn a_declaration_name_must_be_an_identifier() {
        for kind in ["ConceptDeclaration", "EnumDeclaration"] {
            let err = Declaration::try_from(&serde_json::json!({
                "$class": format!("concerto.metamodel@1.0.0.{kind}"),
                "name": "1Bad", "isAbstract": false, "properties": []
            }));
            assert!(
                err.unwrap_err().to_string().contains("invalid identifier"),
                "{kind} with a bad name should be rejected"
            );
        }
    }

    #[test]
    fn scalar_reports_its_concrete_kind() {
        let s = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Email"
        }));
        assert_eq!(s.declaration_kind(), "StringScalar");
    }

    #[test]
    fn scalar_with_reversed_range_is_rejected() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.IntegerScalar",
            "name": "Score",
            "validator": {
                "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
                "lower": 10, "upper": 5
            }
        }));
        assert!(err.unwrap_err().to_string().contains("Lower bound"));
    }

    #[test]
    fn scalar_with_valid_range_is_accepted() {
        assert!(
            Declaration::try_from(&serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.IntegerScalar",
                "name": "Score",
                "validator": {
                    "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
                    "lower": 0, "upper": 10
                }
            }))
            .is_ok()
        );
    }

    #[test]
    fn non_array_properties_is_reported_verbatim() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Bad",
            "properties": { "not": "an array" }
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: 'properties' must be an array"
        );
    }

    #[test]
    fn a_class_declaration_with_no_properties_field_loads_with_none() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Empty"
        }));
        assert!(d.as_class().unwrap().own_properties().is_empty());
    }

    #[test]
    fn a_class_declaration_property_class_may_be_given_as_the_short_name() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Person",
            "properties": [
                { "$class": "StringProperty", "name": "firstName", "isArray": false, "isOptional": false }
            ]
        }));
        let c = d.as_class().unwrap();
        assert_eq!(c.own_properties().len(), 1);
        assert_eq!(c.own_properties()[0].type_name(), Some("String"));
    }

    #[test]
    fn an_enum_property_in_a_class_declaration_is_kept() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "C",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "s" }
            ]
        }));
        let c = d.as_class().unwrap();
        assert_eq!(c.own_properties().len(), 1);
        assert!(c.own_properties()[0].is_enum_value());
    }

    #[test]
    fn a_malformed_class_header_is_reported_before_its_properties() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "C",
            "decorators": "x",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.MysteryProperty", "name": "s" }
            ]
        }));
        assert!(
            err.unwrap_err()
                .to_string()
                .starts_with("illegal model: invalid ConceptDeclaration: ")
        );
    }

    /// The pre-port loader accepts a short scalar `$class` (TS
    /// `ModelFile.fromAst` rejects it; P2-08 ports that), but the ported
    /// `ScalarDeclaration.process` compares the fully-qualified `$class`, as
    /// TS does, so the scalar has no type (`getType()` is `null`).
    #[test]
    fn a_scalar_class_may_be_given_as_the_short_name() {
        let s = decl(serde_json::json!({
            "$class": "StringScalar",
            "name": "Email"
        }));
        assert_eq!(s.declaration_kind(), "StringScalar");
        assert_eq!(s.as_scalar().unwrap().scalar_type(), None);
    }

    #[test]
    fn unknown_scalar_kind_errors() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryScalar",
            "name": "X"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: unknown scalar type: MysteryScalar"
        );
    }

    /// A map declaration with the given key, and an object value naming `Nope`.
    fn map_to_nope(key: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        let mut map = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "M",
            "key": key,
            "value": {
                "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Nope" }
            }
        });
        for (field, value) in extra.as_object().unwrap() {
            map[field] = value.clone();
        }
        map
    }

    fn string_key() -> serde_json::Value {
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" })
    }

    #[test]
    fn a_well_formed_map_is_typed() {
        let d = decl(map_to_nope(string_key(), serde_json::json!({})));
        let map = d.as_map().unwrap();
        assert!(matches!(map, MapDeclaration::Typed(_)));
        assert_eq!(map.key_kind(), "StringMapKeyType");
        assert_eq!(map.value_kind(), "ObjectMapValueType");
        assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
    }

    #[test]
    fn a_map_with_malformed_decorators_or_location_is_typed_and_keeps_its_value_type() {
        for extra in [
            serde_json::json!({ "decorators": "x" }),
            serde_json::json!({ "location": 1 }),
        ] {
            let d = decl(map_to_nope(string_key(), extra.clone()));
            let map = d.as_map().unwrap();
            assert!(matches!(map, MapDeclaration::Typed(_)), "{extra}");
            assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
        }
    }

    #[test]
    fn a_map_key_class_may_be_given_as_the_short_name() {
        let d = decl(map_to_nope(
            serde_json::json!({ "$class": "StringMapKeyType" }),
            serde_json::json!({}),
        ));
        let map = d.as_map().unwrap();
        assert!(matches!(map, MapDeclaration::Typed(_)));
        assert_eq!(map.key_kind(), "StringMapKeyType");
        assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
    }

    #[test]
    fn a_map_with_an_unrecognised_key_kind_still_loads() {
        let m = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "Lookup",
            "key": { "$class": "concerto.metamodel@1.0.0.IntegerMapKeyType" },
            "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" }
        }));
        let map = m.as_map().expect("map declaration");
        assert!(matches!(map, MapDeclaration::Untyped { .. }));
        assert_eq!(map.name(), "Lookup");
        assert_eq!(map.key_kind(), "IntegerMapKeyType");
        assert_eq!(map.value_kind(), "StringMapValueType");
        assert!(map.key_type().is_none());
    }

    #[test]
    fn a_map_with_an_unrecognised_key_kind_keeps_its_value_type() {
        let d = decl(map_to_nope(
            serde_json::json!({ "$class": "concerto.metamodel@1.0.0.IntegerMapKeyType" }),
            serde_json::json!({}),
        ));
        let map = d.as_map().unwrap();
        assert!(matches!(map, MapDeclaration::Untyped { .. }));
        assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
    }

    #[test]
    fn a_map_with_no_key_loads_with_an_empty_key_kind() {
        let mut node = map_to_nope(string_key(), serde_json::json!({}));
        node.as_object_mut().unwrap().remove("key");
        let d = decl(node);
        let map = d.as_map().unwrap();
        assert_eq!(map.key_kind(), "");
        assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
    }

    #[test]
    fn an_object_map_value_with_no_type_loads_with_none() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "M",
            "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
            "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType" }
        }));
        let map = d.as_map().unwrap();
        assert_eq!(map.value_kind(), "ObjectMapValueType");
        assert!(map.value_type().is_none());
    }

    #[test]
    fn a_type_on_a_primitive_map_key_is_still_kept() {
        let d = decl(map_to_nope(
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.StringMapKeyType",
                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "K" }
            }),
            serde_json::json!({}),
        ));
        let map = d.as_map().unwrap();
        assert_eq!(map.key_kind(), "StringMapKeyType");
        assert_eq!(map.key_type().map(|t| t.name.as_str()), Some("K"));
    }

    #[test]
    fn a_map_with_no_name_is_rejected_with_the_serde_message() {
        let mut node = map_to_nope(string_key(), serde_json::json!({}));
        node.as_object_mut().unwrap().remove("name");
        let err = Declaration::try_from(&node);
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: invalid MapDeclaration: missing field `name`"
        );

        let err =
            Declaration::try_from(&map_to_nope(string_key(), serde_json::json!({ "name": 5 })));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: invalid MapDeclaration: invalid type: integer `5`, expected a string"
        );
    }

    /// A `MapDeclaration` with the given key and value nodes.
    fn map_with(key: serde_json::Value, value: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "MapPermutation1",
            "key": key,
            "value": value,
        })
    }

    fn kind(short: &str) -> serde_json::Value {
        serde_json::json!({ "$class": format!("concerto.metamodel@1.0.0.{short}") })
    }

    fn object_kind(short: &str, type_name: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": format!("concerto.metamodel@1.0.0.{short}"),
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": type_name },
        })
    }

    // TS: MapDeclaration test/introspect/mapdeclaration.js `#getKey` "should
    // return the correct Type when called".
    #[test]
    fn key_type_name_is_string_for_a_string_key() {
        let d = decl(map_with(
            kind("StringMapKeyType"),
            kind("StringMapValueType"),
        ));
        assert_eq!(d.as_map().unwrap().key_type_name(), "String");
    }

    #[test]
    fn key_type_name_is_datetime_for_a_datetime_key() {
        let d = decl(map_with(
            kind("DateTimeMapKeyType"),
            kind("StringMapValueType"),
        ));
        assert_eq!(d.as_map().unwrap().key_type_name(), "DateTime");
    }

    // TS: "should return the correct Type when called - Scalar String/DateTime":
    // an object key's type is the raw referenced name, unresolved.
    #[test]
    fn key_type_name_is_the_raw_referenced_name_for_an_object_key() {
        let d = decl(map_with(
            object_kind("ObjectMapKeyType", "GUID"),
            kind("StringMapValueType"),
        ));
        assert_eq!(d.as_map().unwrap().key_type_name(), "GUID");
    }

    // TS: MapDeclaration test/introspect/mapdeclaration.js `#getValue` "should
    // return the correct Type when called", one case per primitive value kind.
    #[test]
    fn value_type_name_covers_every_primitive_kind() {
        let cases = [
            ("BooleanMapValueType", "Boolean"),
            ("DateTimeMapValueType", "DateTime"),
            ("StringMapValueType", "String"),
            ("IntegerMapValueType", "Integer"),
            ("LongMapValueType", "Long"),
            ("DoubleMapValueType", "Double"),
        ];
        for (mm_kind, expected) in cases {
            let d = decl(map_with(kind("StringMapKeyType"), kind(mm_kind)));
            assert_eq!(
                d.as_map().unwrap().value_type_name(),
                expected,
                "{mm_kind} should report {expected}"
            );
        }
    }

    // TS: "should return the correct values when called - Scalar
    // String/DateTime", and the relationship value case: an object or
    // relationship value's type is the raw referenced name, unresolved.
    #[test]
    fn value_type_name_is_the_raw_referenced_name_for_an_object_or_relationship_value() {
        let d = decl(map_with(
            kind("StringMapKeyType"),
            object_kind("ObjectMapValueType", "GUID"),
        ));
        assert_eq!(d.as_map().unwrap().value_type_name(), "GUID");

        let d = decl(map_with(
            kind("StringMapKeyType"),
            object_kind("RelationshipMapValueType", "Person"),
        ));
        assert_eq!(d.as_map().unwrap().value_type_name(), "Person");
    }

    // TS: `#toString` "should give the correct value for Map Declaration".
    #[test]
    fn to_string_matches_ts() {
        assert_eq!(
            MapDeclaration::to_string("com.acme@1.0.0.Dictionary"),
            "MapDeclaration {id=com.acme@1.0.0.Dictionary}"
        );
    }

    // TS: `#Introspect` "should return the correct value on introspection".
    #[test]
    fn declaration_kind_and_is_map_declaration_agree_with_ts() {
        let d = decl(map_with(
            kind("StringMapKeyType"),
            kind("StringMapValueType"),
        ));
        assert_eq!(d.declaration_kind(), "MapDeclaration");
        assert!(d.is_map_declaration());
        assert!(!d.is_class_declaration());
        assert!(!d.is_enum_declaration());
        assert!(!d.is_scalar_declaration());
    }
}

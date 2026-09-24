//! Internal representations of the Concerto declarations.
//!
//! Concerto's JavaScript runtime models declarations as an inheritance
//! hierarchy: concept, asset, participant, transaction and event all extend a
//! common class declaration. Inheritance like that isn't idiomatic in Rust, so
//! the five class-like declarations are represented by a single
//! [`ClassDeclaration`] newtype-enum over the matching `mm::*Declaration`
//! struct, tagged with a [`ClassKind`], while enums, scalars and maps are the
//! other variants of the [`Declaration`] sum type. Each variant is selected by
//! matching on the node's `$class`.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, Result};
use crate::introspect::property::Property;
use crate::introspect::{check_domain, check_length, check_pattern, declared_class};
use crate::model_util::{is_valid_identifier, short_name};

/// Which class-like declaration a [`ClassDeclaration`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKind {
    /// A `concept`.
    Concept,
    /// An `asset` (system-identifiable).
    Asset,
    /// A `participant` (system-identifiable).
    Participant,
    /// A `transaction`.
    Transaction,
    /// An `event`.
    Event,
}

impl ClassKind {
    /// The metamodel `$class` short name for this kind.
    pub fn declaration_kind(self) -> &'static str {
        match self {
            Self::Concept => "ConceptDeclaration",
            Self::Asset => "AssetDeclaration",
            Self::Participant => "ParticipantDeclaration",
            Self::Transaction => "TransactionDeclaration",
            Self::Event => "EventDeclaration",
        }
    }

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

/// A concept-like declaration: concept, asset, participant, transaction or
/// event, distinguished by [`ClassDeclaration::kind`]. Each variant is a
/// newtype over the matching generated `mm::*Declaration` struct; the five
/// share an identical field shape (the metamodel's own inheritance), so the
/// accessors below just pick the right field out of whichever variant this is.
#[derive(Debug, Clone)]
pub enum ClassDeclaration {
    /// `concerto.metamodel@1.0.0.ConceptDeclaration`.
    Concept(mm::ConceptDeclaration),
    /// `concerto.metamodel@1.0.0.AssetDeclaration`.
    Asset(mm::AssetDeclaration),
    /// `concerto.metamodel@1.0.0.ParticipantDeclaration`.
    Participant(mm::ParticipantDeclaration),
    /// `concerto.metamodel@1.0.0.TransactionDeclaration`.
    Transaction(mm::TransactionDeclaration),
    /// `concerto.metamodel@1.0.0.EventDeclaration`.
    Event(mm::EventDeclaration),
}

impl ClassDeclaration {
    /// The kind of class-like declaration this is.
    pub fn kind(&self) -> ClassKind {
        match self {
            Self::Concept(_) => ClassKind::Concept,
            Self::Asset(_) => ClassKind::Asset,
            Self::Participant(_) => ClassKind::Participant,
            Self::Transaction(_) => ClassKind::Transaction,
            Self::Event(_) => ClassKind::Event,
        }
    }

    /// The declaration's short name (without namespace).
    pub fn name(&self) -> &str {
        match self {
            Self::Concept(d) => &d.name,
            Self::Asset(d) => &d.name,
            Self::Participant(d) => &d.name,
            Self::Transaction(d) => &d.name,
            Self::Event(d) => &d.name,
        }
    }

    /// Abstract types can't be instantiated on their own.
    pub fn is_abstract(&self) -> bool {
        match self {
            Self::Concept(d) => d.is_abstract,
            Self::Asset(d) => d.is_abstract,
            Self::Participant(d) => d.is_abstract,
            Self::Transaction(d) => d.is_abstract,
            Self::Event(d) => d.is_abstract,
        }
    }

    /// The super type this declaration extends, if it extends one.
    pub fn super_type(&self) -> Option<&mm::TypeIdentifier> {
        match self {
            Self::Concept(d) => d.super_type.as_ref(),
            Self::Asset(d) => d.super_type.as_ref(),
            Self::Participant(d) => d.super_type.as_ref(),
            Self::Transaction(d) => d.super_type.as_ref(),
            Self::Event(d) => d.super_type.as_ref(),
        }
    }

    /// The properties declared directly on this type, converted to the
    /// introspect [`Property`] wrapper. Inherited properties are not
    /// included; those are gathered separately by walking the supertype
    /// chain.
    pub fn own_properties(&self) -> Vec<Property> {
        let properties: &[mm::Property] = match self {
            Self::Concept(d) => &d.properties,
            Self::Asset(d) => &d.properties,
            Self::Participant(d) => &d.properties,
            Self::Transaction(d) => &d.properties,
            Self::Event(d) => &d.properties,
        };
        properties.iter().cloned().map(Property::from).collect()
    }

    /// The decorators attached to this declaration.
    pub fn decorators(&self) -> &[mm::Decorator] {
        match self {
            Self::Concept(d) => d.decorators.as_deref().unwrap_or(&[]),
            Self::Asset(d) => d.decorators.as_deref().unwrap_or(&[]),
            Self::Participant(d) => d.decorators.as_deref().unwrap_or(&[]),
            Self::Transaction(d) => d.decorators.as_deref().unwrap_or(&[]),
            Self::Event(d) => d.decorators.as_deref().unwrap_or(&[]),
        }
    }

    /// The source location, if the AST carried one.
    pub fn location(&self) -> Option<&mm::Range> {
        match self {
            Self::Concept(d) => d.location.as_ref(),
            Self::Asset(d) => d.location.as_ref(),
            Self::Participant(d) => d.location.as_ref(),
            Self::Transaction(d) => d.location.as_ref(),
            Self::Event(d) => d.location.as_ref(),
        }
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
            Some(mm::Identified::IdentifiedBy(id)) => Some(id.name.as_str()),
            _ => None,
        }
    }

    fn identified(&self) -> Option<&mm::Identified> {
        match self {
            Self::Concept(d) => d.identified.as_ref(),
            Self::Asset(d) => d.identified.as_ref(),
            Self::Participant(d) => d.identified.as_ref(),
            Self::Transaction(d) => d.identified.as_ref(),
            Self::Event(d) => d.identified.as_ref(),
        }
    }

    /// Deserializes the whole node into the concrete `mm::*Declaration`
    /// struct for `kind`, then runs the same checks [`Property::try_from`]
    /// would on each of its properties. The generated struct only parses
    /// properties structurally (they are typed `Vec<mm::Property>`), so the
    /// business-rule checks - a name that is not reserved, a legal
    /// identifier, a validator that makes sense - are run here explicitly
    /// rather than lost.
    fn from_json(kind: ClassKind, value: &serde_json::Value) -> Result<Self> {
        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid {}: {e}", kind.declaration_kind()),
            file_name: None,
            location: None,
        };
        let declaration = match kind {
            ClassKind::Concept => {
                Self::Concept(serde_json::from_value(value.clone()).map_err(bad)?)
            }
            ClassKind::Asset => Self::Asset(serde_json::from_value(value.clone()).map_err(bad)?),
            ClassKind::Participant => {
                Self::Participant(serde_json::from_value(value.clone()).map_err(bad)?)
            }
            ClassKind::Transaction => {
                Self::Transaction(serde_json::from_value(value.clone()).map_err(bad)?)
            }
            ClassKind::Event => Self::Event(serde_json::from_value(value.clone()).map_err(bad)?),
        };
        for property in declaration.own_properties() {
            property.validate()?;
        }
        Ok(declaration)
    }
}

/// A scalar: a named alias for a primitive, sometimes with a validator
/// attached. A newtype over the metamodel's own `$class`-tagged
/// [`mm::ScalarDeclaration`]; [`ScalarDeclaration::scalar_type`] tells you
/// which primitive it wraps.
#[derive(Debug, Clone)]
pub struct ScalarDeclaration(mm::ScalarDeclaration);

impl ScalarDeclaration {
    /// The scalar's short name.
    pub fn name(&self) -> &str {
        match &self.0 {
            mm::ScalarDeclaration::BooleanScalar(s) => &s.name,
            mm::ScalarDeclaration::IntegerScalar(s) => &s.name,
            mm::ScalarDeclaration::LongScalar(s) => &s.name,
            mm::ScalarDeclaration::DoubleScalar(s) => &s.name,
            mm::ScalarDeclaration::StringScalar(s) => &s.name,
            mm::ScalarDeclaration::DateTimeScalar(s) => &s.name,
        }
    }

    /// The primitive type this scalar aliases.
    pub fn scalar_type(&self) -> &'static str {
        match &self.0 {
            mm::ScalarDeclaration::BooleanScalar(_) => "Boolean",
            mm::ScalarDeclaration::IntegerScalar(_) => "Integer",
            mm::ScalarDeclaration::LongScalar(_) => "Long",
            mm::ScalarDeclaration::DoubleScalar(_) => "Double",
            mm::ScalarDeclaration::StringScalar(_) => "String",
            mm::ScalarDeclaration::DateTimeScalar(_) => "DateTime",
        }
    }

    /// The metamodel `$class` short name for this scalar, e.g. `StringScalar`.
    pub fn declaration_kind(&self) -> &'static str {
        match &self.0 {
            mm::ScalarDeclaration::BooleanScalar(_) => "BooleanScalar",
            mm::ScalarDeclaration::IntegerScalar(_) => "IntegerScalar",
            mm::ScalarDeclaration::LongScalar(_) => "LongScalar",
            mm::ScalarDeclaration::DoubleScalar(_) => "DoubleScalar",
            mm::ScalarDeclaration::StringScalar(_) => "StringScalar",
            mm::ScalarDeclaration::DateTimeScalar(_) => "DateTimeScalar",
        }
    }

    fn from_json(short: &str, value: &serde_json::Value) -> Result<Self> {
        let scalar: mm::ScalarDeclaration =
            serde_json::from_value(value.clone()).map_err(|e| ConcertoError::IllegalModel {
                message: format!("invalid {short}: {e}"),
                file_name: None,
                location: None,
            })?;
        let wrapped = Self(scalar);
        wrapped.check_validator()?;
        Ok(wrapped)
    }

    /// Checks the range, length or regular expression validator this scalar
    /// carries, on the same terms as the equivalent property.
    fn check_validator(&self) -> Result<()> {
        match &self.0 {
            mm::ScalarDeclaration::StringScalar(s) => {
                if let Some(validator) = &s.validator {
                    check_pattern(&s.name, validator)?;
                }
                match &s.length_validator {
                    Some(validator) => check_length(&s.name, validator),
                    None => Ok(()),
                }
            }
            mm::ScalarDeclaration::IntegerScalar(s) => match &s.validator {
                Some(validator) => check_domain(&s.name, validator.lower, validator.upper),
                None => Ok(()),
            },
            mm::ScalarDeclaration::LongScalar(s) => match &s.validator {
                Some(validator) => check_domain(&s.name, validator.lower, validator.upper),
                None => Ok(()),
            },
            mm::ScalarDeclaration::DoubleScalar(s) => match &s.validator {
                Some(validator) => check_domain(&s.name, validator.lower, validator.upper),
                None => Ok(()),
            },
            // Boolean and DateTime scalars declare no validator in the
            // metamodel, so there is nothing to check. Listing them keeps this
            // exhaustive: a new scalar kind will not compile until it is
            // handled here.
            mm::ScalarDeclaration::BooleanScalar(_) | mm::ScalarDeclaration::DateTimeScalar(_) => {
                Ok(())
            }
        }
    }
}

/// A top-level declaration within a model file.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Declaration {
    /// A concept-like declaration (see [`ClassDeclaration`]).
    Class(ClassDeclaration),
    /// An enumeration.
    Enum(mm::EnumDeclaration),
    /// A scalar alias over a primitive.
    Scalar(ScalarDeclaration),
    /// A map type.
    Map(MapDeclaration),
}

/// A map declaration: a newtype over [`mm::MapDeclaration`]'s shared fields,
/// keeping the kind and the referenced type of its key and value.
///
/// The key and value nodes are *also* parsed against the metamodel's own
/// [`mm::MapKeyType`] and [`mm::MapValueType`], but leniently: a kind the
/// metamodel does not declare (e.g. an `IntegerMapKeyType`) is kept as its
/// bare `$class` short name with no resolved type, rather than failing to
/// load the model. This lets a key or value kind the specification does not
/// allow reach semantic validation, which reports it with a proper message,
/// instead of an opaque deserialization error.
#[derive(Debug, Clone)]
pub struct MapDeclaration {
    name: String,
    key_kind: String,
    key: Option<mm::MapKeyType>,
    value_kind: String,
    value: Option<mm::MapValueType>,
}

impl MapDeclaration {
    /// The map's short name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The metamodel `$class` short name of the key node, such as
    /// `StringMapKeyType`.
    pub fn key_kind(&self) -> &str {
        &self.key_kind
    }

    /// The metamodel `$class` short name of the value node, such as
    /// `ObjectMapValueType`.
    pub fn value_kind(&self) -> &str {
        &self.value_kind
    }

    /// The type the key refers to, for a key that is not a primitive.
    pub fn key_type(&self) -> Option<&mm::TypeIdentifier> {
        match &self.key {
            Some(mm::MapKeyType::ObjectMapKeyType(k)) => Some(&k.type_),
            _ => None,
        }
    }

    /// The type the value refers to, for a value that is not a primitive.
    pub fn value_type(&self) -> Option<&mm::TypeIdentifier> {
        match &self.value {
            Some(mm::MapValueType::ObjectMapValueType(v)) => Some(&v.type_),
            Some(mm::MapValueType::RelationshipMapValueType(v)) => Some(&v.type_),
            _ => None,
        }
    }

    fn from_json(value: &serde_json::Value) -> Result<Self> {
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConcertoError::IllegalModel {
                message: "invalid MapDeclaration: missing 'name'".into(),
                file_name: None,
                location: None,
            })?
            .to_string();

        let key_node = value.get("key");
        let value_node = value.get("value");
        let key = key_node.and_then(|n| serde_json::from_value::<mm::MapKeyType>(n.clone()).ok());
        let value =
            value_node.and_then(|n| serde_json::from_value::<mm::MapValueType>(n.clone()).ok());

        Ok(Self {
            name,
            key_kind: key
                .as_ref()
                .map(map_key_kind)
                .unwrap_or_else(|| node_kind(key_node)),
            key,
            value_kind: value
                .as_ref()
                .map(map_value_kind)
                .unwrap_or_else(|| node_kind(value_node)),
            value,
        })
    }
}

/// The metamodel `$class` short name of a known map key kind.
fn map_key_kind(key: &mm::MapKeyType) -> String {
    match key {
        mm::MapKeyType::StringMapKeyType(_) => "StringMapKeyType",
        mm::MapKeyType::DateTimeMapKeyType(_) => "DateTimeMapKeyType",
        mm::MapKeyType::ObjectMapKeyType(_) => "ObjectMapKeyType",
    }
    .to_string()
}

/// The metamodel `$class` short name of a known map value kind.
fn map_value_kind(value: &mm::MapValueType) -> String {
    match value {
        mm::MapValueType::BooleanMapValueType(_) => "BooleanMapValueType",
        mm::MapValueType::DateTimeMapValueType(_) => "DateTimeMapValueType",
        mm::MapValueType::StringMapValueType(_) => "StringMapValueType",
        mm::MapValueType::IntegerMapValueType(_) => "IntegerMapValueType",
        mm::MapValueType::LongMapValueType(_) => "LongMapValueType",
        mm::MapValueType::DoubleMapValueType(_) => "DoubleMapValueType",
        mm::MapValueType::ObjectMapValueType(_) => "ObjectMapValueType",
        mm::MapValueType::RelationshipMapValueType(_) => "RelationshipMapValueType",
    }
    .to_string()
}

/// The bare `$class` short name of a map key or value node the metamodel does
/// not declare (or that is missing/malformed), so validation still has
/// something to report.
fn node_kind(node: Option<&serde_json::Value>) -> String {
    node.map(|n| short_name(declared_class(n)).to_string())
        .unwrap_or_default()
}

impl Declaration {
    /// The declaration's short name.
    pub fn name(&self) -> &str {
        match self {
            Self::Class(c) => c.name(),
            Self::Enum(e) => &e.name,
            Self::Scalar(s) => s.name(),
            Self::Map(m) => m.name(),
        }
    }

    /// The metamodel `$class` short name for this declaration.
    pub fn declaration_kind(&self) -> &'static str {
        match self {
            Self::Class(c) => c.kind().declaration_kind(),
            Self::Enum(_) => "EnumDeclaration",
            Self::Scalar(s) => s.declaration_kind(),
            Self::Map(_) => "MapDeclaration",
        }
    }

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

impl TryFrom<&serde_json::Value> for Declaration {
    type Error = ConcertoError;

    fn try_from(value: &serde_json::Value) -> Result<Self> {
        let class = declared_class(value);
        if class.is_empty() {
            return Err(ConcertoError::IllegalModel {
                message: "declaration node is missing its $class".into(),
                file_name: None,
                location: None,
            });
        }
        let kind = short_name(class);

        if let Some(class_kind) = ClassKind::from_short(kind) {
            let class = Self::Class(ClassDeclaration::from_json(class_kind, value)?);
            return check_name(class);
        }

        let declaration = match kind {
            "EnumDeclaration" => {
                Self::Enum(serde_json::from_value(value.clone()).map_err(|e| {
                    ConcertoError::IllegalModel {
                        message: format!("invalid EnumDeclaration: {e}"),
                        file_name: None,
                        location: None,
                    }
                })?)
            }
            "MapDeclaration" => Self::Map(MapDeclaration::from_json(value)?),
            s if s.ends_with("Scalar") => Self::Scalar(ScalarDeclaration::from_json(s, value)?),
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
    if is_valid_identifier(declaration.name()) {
        Ok(declaration)
    } else {
        Err(ConcertoError::IllegalModel {
            message: format!("invalid identifier: {}", declaration.name()),
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
        assert_eq!(s.as_scalar().unwrap().scalar_type(), "String");
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
}

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, Result};
use crate::model_util;

use super::properties::PropertyDecl;
use super::traits::{Decorated, HasProperties, Identifiable, Named};

use concerto_macros::{Decorated, HasProperties, Identifiable, Named};

// ===================================================================
// Flattened class-declaration structs
// ===================================================================

/// A concept declaration with fields extracted from the metamodel AST.
#[derive(Debug, Clone, Decorated, Named, HasProperties, Identifiable)]
pub struct ConceptDeclaration {
    pub(crate) name: String,
    pub(crate) decorators: Option<Vec<mm::Decorator>>,
    pub(crate) location: Option<mm::Range>,
    pub(crate) is_abstract: bool,
    pub(crate) identified: Option<mm::Identified>,
    pub(crate) super_type: Option<mm::TypeIdentifier>,
    pub(crate) properties: Vec<PropertyDecl>,
}

/// An asset declaration with fields extracted from the metamodel AST.
#[derive(Debug, Clone, Decorated, Named, HasProperties, Identifiable)]
pub struct AssetDeclaration {
    pub(crate) name: String,
    pub(crate) decorators: Option<Vec<mm::Decorator>>,
    pub(crate) location: Option<mm::Range>,
    pub(crate) is_abstract: bool,
    pub(crate) identified: Option<mm::Identified>,
    pub(crate) super_type: Option<mm::TypeIdentifier>,
    pub(crate) properties: Vec<PropertyDecl>,
}

/// A participant declaration with fields extracted from the metamodel AST.
#[derive(Debug, Clone, Decorated, Named, HasProperties, Identifiable)]
pub struct ParticipantDeclaration {
    pub(crate) name: String,
    pub(crate) decorators: Option<Vec<mm::Decorator>>,
    pub(crate) location: Option<mm::Range>,
    pub(crate) is_abstract: bool,
    pub(crate) identified: Option<mm::Identified>,
    pub(crate) super_type: Option<mm::TypeIdentifier>,
    pub(crate) properties: Vec<PropertyDecl>,
}

/// A transaction declaration with fields extracted from the metamodel AST.
#[derive(Debug, Clone, Decorated, Named, HasProperties, Identifiable)]
pub struct TransactionDeclaration {
    pub(crate) name: String,
    pub(crate) decorators: Option<Vec<mm::Decorator>>,
    pub(crate) location: Option<mm::Range>,
    pub(crate) is_abstract: bool,
    pub(crate) identified: Option<mm::Identified>,
    pub(crate) super_type: Option<mm::TypeIdentifier>,
    pub(crate) properties: Vec<PropertyDecl>,
}

/// An event declaration with fields extracted from the metamodel AST.
#[derive(Debug, Clone, Decorated, Named, HasProperties, Identifiable)]
pub struct EventDeclaration {
    pub(crate) name: String,
    pub(crate) decorators: Option<Vec<mm::Decorator>>,
    pub(crate) location: Option<mm::Range>,
    pub(crate) is_abstract: bool,
    pub(crate) identified: Option<mm::Identified>,
    pub(crate) super_type: Option<mm::TypeIdentifier>,
    pub(crate) properties: Vec<PropertyDecl>,
}

/// An enum declaration with fields extracted from the metamodel AST.
#[derive(Debug, Clone, Decorated, Named)]
pub struct EnumDeclaration {
    pub(crate) name: String,
    pub(crate) decorators: Option<Vec<mm::Decorator>>,
    pub(crate) location: Option<mm::Range>,
    pub(crate) properties: Vec<PropertyDecl>,
}

impl HasProperties for EnumDeclaration {
    fn own_properties(&self) -> &[PropertyDecl] {
        &self.properties
    }
    fn super_type(&self) -> Option<&mm::TypeIdentifier> {
        None
    }
    fn is_abstract(&self) -> bool {
        false
    }
}

impl Identifiable for EnumDeclaration {
    fn is_identified(&self) -> bool {
        false
    }
    fn is_system_identified(&self) -> bool {
        false
    }
    fn is_explicitly_identified(&self) -> bool {
        false
    }
    fn identifier_field_name(&self) -> Option<&str> {
        None
    }
}

/// Wraps `mm::MapDeclaration`.
#[derive(Debug, Clone, Decorated, Named)]
pub struct MapDeclaration(pub(crate) mm::MapDeclaration);

// ===================================================================
// Constructors from metamodel types
// ===================================================================

macro_rules! impl_class_decl_new {
    ($rust_ty:ident, $mm_ty:ty) => {
        impl $rust_ty {
            pub(crate) fn new(mm: $mm_ty, properties: Vec<PropertyDecl>) -> Self {
                Self {
                    name: mm.name,
                    decorators: mm.decorators,
                    location: mm.location,
                    is_abstract: mm.is_abstract,
                    identified: mm.identified,
                    super_type: mm.super_type,
                    properties,
                }
            }
        }
    };
}

impl_class_decl_new!(ConceptDeclaration, mm::ConceptDeclaration);
impl_class_decl_new!(AssetDeclaration, mm::AssetDeclaration);
impl_class_decl_new!(ParticipantDeclaration, mm::ParticipantDeclaration);
impl_class_decl_new!(TransactionDeclaration, mm::TransactionDeclaration);
impl_class_decl_new!(EventDeclaration, mm::EventDeclaration);

impl EnumDeclaration {
    pub(crate) fn new(mm: mm::EnumDeclaration, properties: Vec<PropertyDecl>) -> Self {
        Self {
            name: mm.name,
            decorators: mm.decorators,
            location: mm.location,
            properties,
        }
    }
}

// ===================================================================
// ScalarDeclKind — sum over scalar sub-types
// ===================================================================

#[derive(Debug, Clone)]
pub enum ScalarDeclKind {
    Boolean(mm::BooleanScalar),
    Integer(mm::IntegerScalar),
    Long(mm::LongScalar),
    Double(mm::DoubleScalar),
    String(mm::StringScalar),
    DateTime(mm::DateTimeScalar),
}

/// Wraps one of the six metamodel scalar types.
#[derive(Debug, Clone)]
pub struct ScalarDeclaration(pub(crate) ScalarDeclKind);

// ===================================================================
// ClassDeclaration — sum type replacing JS inheritance subtree
// ===================================================================

#[derive(Debug, Clone)]
pub enum ClassDeclaration {
    Concept(ConceptDeclaration),
    Asset(AssetDeclaration),
    Participant(ParticipantDeclaration),
    Transaction(TransactionDeclaration),
    Event(EventDeclaration),
    Enum(EnumDeclaration),
}

// ===================================================================
// Declaration — top-level sum type
// ===================================================================

#[derive(Debug, Clone)]
pub enum Declaration {
    Class(ClassDeclaration),
    Scalar(ScalarDeclaration),
    Map(MapDeclaration),
}

// ===================================================================
// TryFrom<serde_json::Value> — construct from AST JSON
// ===================================================================

fn parse_properties_from_json(value: &serde_json::Value) -> Result<Vec<PropertyDecl>> {
    match value.get("properties") {
        Some(serde_json::Value::Array(arr)) => {
            arr.iter().map(|v| PropertyDecl::try_from(v.clone())).collect()
        }
        _ => Ok(vec![]),
    }
}

macro_rules! impl_try_from_for_class_decl {
    ($rust_ty:ident, $mm_ty:ty, $variant:ident, $label:expr) => {
        impl TryFrom<serde_json::Value> for $rust_ty {
            type Error = ConcertoError;
            fn try_from(value: serde_json::Value) -> Result<Self> {
                let inner: $mm_ty =
                    serde_json::from_value(value.clone()).map_err(|e| ConcertoError::IllegalModel {
                        message: format!(concat!("Invalid ", $label, ": {}"), e),
                        file_name: None,
                        location: None,
                    })?;
                let properties = parse_properties_from_json(&value)?;
                Ok(Self::new(inner, properties))
            }
        }
    };
}

impl_try_from_for_class_decl!(ConceptDeclaration, mm::ConceptDeclaration, Concept, "ConceptDeclaration");
impl_try_from_for_class_decl!(AssetDeclaration, mm::AssetDeclaration, Asset, "AssetDeclaration");
impl_try_from_for_class_decl!(ParticipantDeclaration, mm::ParticipantDeclaration, Participant, "ParticipantDeclaration");
impl_try_from_for_class_decl!(TransactionDeclaration, mm::TransactionDeclaration, Transaction, "TransactionDeclaration");
impl_try_from_for_class_decl!(EventDeclaration, mm::EventDeclaration, Event, "EventDeclaration");
impl_try_from_for_class_decl!(EnumDeclaration, mm::EnumDeclaration, Enum, "EnumDeclaration");

impl TryFrom<serde_json::Value> for MapDeclaration {
    type Error = ConcertoError;
    fn try_from(value: serde_json::Value) -> Result<Self> {
        let inner: mm::MapDeclaration =
            serde_json::from_value(value).map_err(|e| ConcertoError::IllegalModel {
                message: format!("Invalid MapDeclaration: {e}"),
                file_name: None,
                location: None,
            })?;
        Ok(MapDeclaration(inner))
    }
}

impl TryFrom<serde_json::Value> for ScalarDeclaration {
    type Error = ConcertoError;
    fn try_from(value: serde_json::Value) -> Result<Self> {
        let class = value
            .get("$class")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = model_util::get_short_name(&class).to_string();

        let mk_err = |e: serde_json::Error, k: &str| ConcertoError::IllegalModel {
            message: format!("Invalid {k}: {e}"),
            file_name: None,
            location: None,
        };

        let scalar_kind = match kind.as_str() {
            "BooleanScalar" => ScalarDeclKind::Boolean(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?),
            "IntegerScalar" => ScalarDeclKind::Integer(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?),
            "LongScalar" => ScalarDeclKind::Long(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?),
            "DoubleScalar" => ScalarDeclKind::Double(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?),
            "StringScalar" => ScalarDeclKind::String(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?),
            "DateTimeScalar" => ScalarDeclKind::DateTime(serde_json::from_value(value).map_err(|e| mk_err(e, &kind))?),
            _ => {
                return Err(ConcertoError::IllegalModel {
                    message: format!("Unknown scalar type: {class}"),
                    file_name: None,
                    location: None,
                });
            }
        };

        Ok(ScalarDeclaration(scalar_kind))
    }
}

impl TryFrom<serde_json::Value> for Declaration {
    type Error = ConcertoError;
    fn try_from(value: serde_json::Value) -> Result<Self> {
        let class = value
            .get("$class")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = model_util::get_short_name(&class).to_string();

        match kind.as_str() {
            "ConceptDeclaration" => Ok(Declaration::Class(ClassDeclaration::Concept(
                ConceptDeclaration::try_from(value)?,
            ))),
            "AssetDeclaration" => Ok(Declaration::Class(ClassDeclaration::Asset(
                AssetDeclaration::try_from(value)?,
            ))),
            "ParticipantDeclaration" => Ok(Declaration::Class(ClassDeclaration::Participant(
                ParticipantDeclaration::try_from(value)?,
            ))),
            "TransactionDeclaration" => Ok(Declaration::Class(ClassDeclaration::Transaction(
                TransactionDeclaration::try_from(value)?,
            ))),
            "EventDeclaration" => Ok(Declaration::Class(ClassDeclaration::Event(
                EventDeclaration::try_from(value)?,
            ))),
            "EnumDeclaration" => Ok(Declaration::Class(ClassDeclaration::Enum(
                EnumDeclaration::try_from(value)?,
            ))),
            "MapDeclaration" => Ok(Declaration::Map(MapDeclaration::try_from(value)?)),
            s if s.ends_with("Scalar") => {
                Ok(Declaration::Scalar(ScalarDeclaration::try_from(value)?))
            }
            _ => Err(ConcertoError::IllegalModel {
                message: format!("Unknown declaration type: {class}"),
                file_name: None,
                location: None,
            }),
        }
    }
}

// ===================================================================
// Decorated / Named for ScalarDeclaration
// ===================================================================

impl Decorated for ScalarDeclaration {
    fn decorators(&self) -> &[mm::Decorator] {
        match &self.0 {
            ScalarDeclKind::Boolean(s) => s.decorators.as_deref().unwrap_or(&[]),
            ScalarDeclKind::Integer(s) => s.decorators.as_deref().unwrap_or(&[]),
            ScalarDeclKind::Long(s) => s.decorators.as_deref().unwrap_or(&[]),
            ScalarDeclKind::Double(s) => s.decorators.as_deref().unwrap_or(&[]),
            ScalarDeclKind::String(s) => s.decorators.as_deref().unwrap_or(&[]),
            ScalarDeclKind::DateTime(s) => s.decorators.as_deref().unwrap_or(&[]),
        }
    }
}

impl Named for ScalarDeclaration {
    fn name(&self) -> &str {
        match &self.0 {
            ScalarDeclKind::Boolean(s) => &s.name,
            ScalarDeclKind::Integer(s) => &s.name,
            ScalarDeclKind::Long(s) => &s.name,
            ScalarDeclKind::Double(s) => &s.name,
            ScalarDeclKind::String(s) => &s.name,
            ScalarDeclKind::DateTime(s) => &s.name,
        }
    }
    fn location(&self) -> Option<&mm::Range> {
        match &self.0 {
            ScalarDeclKind::Boolean(s) => s.location.as_ref(),
            ScalarDeclKind::Integer(s) => s.location.as_ref(),
            ScalarDeclKind::Long(s) => s.location.as_ref(),
            ScalarDeclKind::Double(s) => s.location.as_ref(),
            ScalarDeclKind::String(s) => s.location.as_ref(),
            ScalarDeclKind::DateTime(s) => s.location.as_ref(),
        }
    }
}

impl ScalarDeclaration {
    /// Returns the primitive type that this scalar wraps.
    pub fn scalar_type(&self) -> &str {
        match &self.0 {
            ScalarDeclKind::Boolean(_) => "Boolean",
            ScalarDeclKind::Integer(_) => "Integer",
            ScalarDeclKind::Long(_) => "Long",
            ScalarDeclKind::Double(_) => "Double",
            ScalarDeclKind::String(_) => "String",
            ScalarDeclKind::DateTime(_) => "DateTime",
        }
    }

    pub fn kind(&self) -> &ScalarDeclKind {
        &self.0
    }
}

// ===================================================================
// Dispatch on ClassDeclaration sum type
// ===================================================================

impl Decorated for ClassDeclaration {
    fn decorators(&self) -> &[mm::Decorator] {
        match self {
            Self::Concept(d) => d.decorators(),
            Self::Asset(d) => d.decorators(),
            Self::Participant(d) => d.decorators(),
            Self::Transaction(d) => d.decorators(),
            Self::Event(d) => d.decorators(),
            Self::Enum(d) => d.decorators(),
        }
    }
}

impl Named for ClassDeclaration {
    fn name(&self) -> &str {
        match self {
            Self::Concept(d) => d.name(),
            Self::Asset(d) => d.name(),
            Self::Participant(d) => d.name(),
            Self::Transaction(d) => d.name(),
            Self::Event(d) => d.name(),
            Self::Enum(d) => d.name(),
        }
    }
    fn location(&self) -> Option<&mm::Range> {
        match self {
            Self::Concept(d) => d.location(),
            Self::Asset(d) => d.location(),
            Self::Participant(d) => d.location(),
            Self::Transaction(d) => d.location(),
            Self::Event(d) => d.location(),
            Self::Enum(d) => d.location(),
        }
    }
}

impl HasProperties for ClassDeclaration {
    fn own_properties(&self) -> &[PropertyDecl] {
        match self {
            Self::Concept(d) => d.own_properties(),
            Self::Asset(d) => d.own_properties(),
            Self::Participant(d) => d.own_properties(),
            Self::Transaction(d) => d.own_properties(),
            Self::Event(d) => d.own_properties(),
            Self::Enum(d) => d.own_properties(),
        }
    }
    fn super_type(&self) -> Option<&mm::TypeIdentifier> {
        match self {
            Self::Concept(d) => d.super_type(),
            Self::Asset(d) => d.super_type(),
            Self::Participant(d) => d.super_type(),
            Self::Transaction(d) => d.super_type(),
            Self::Event(d) => d.super_type(),
            Self::Enum(d) => d.super_type(),
        }
    }
    fn is_abstract(&self) -> bool {
        match self {
            Self::Concept(d) => d.is_abstract(),
            Self::Asset(d) => d.is_abstract(),
            Self::Participant(d) => d.is_abstract(),
            Self::Transaction(d) => d.is_abstract(),
            Self::Event(d) => d.is_abstract(),
            Self::Enum(d) => d.is_abstract(),
        }
    }
}

impl Identifiable for ClassDeclaration {
    fn is_identified(&self) -> bool {
        match self {
            Self::Concept(d) => d.is_identified(),
            Self::Asset(d) => d.is_identified(),
            Self::Participant(d) => d.is_identified(),
            Self::Transaction(d) => d.is_identified(),
            Self::Event(d) => d.is_identified(),
            Self::Enum(d) => d.is_identified(),
        }
    }
    fn is_system_identified(&self) -> bool {
        match self {
            Self::Concept(d) => d.is_system_identified(),
            Self::Asset(d) => d.is_system_identified(),
            Self::Participant(d) => d.is_system_identified(),
            Self::Transaction(d) => d.is_system_identified(),
            Self::Event(d) => d.is_system_identified(),
            Self::Enum(d) => d.is_system_identified(),
        }
    }
    fn is_explicitly_identified(&self) -> bool {
        match self {
            Self::Concept(d) => d.is_explicitly_identified(),
            Self::Asset(d) => d.is_explicitly_identified(),
            Self::Participant(d) => d.is_explicitly_identified(),
            Self::Transaction(d) => d.is_explicitly_identified(),
            Self::Event(d) => d.is_explicitly_identified(),
            Self::Enum(d) => d.is_explicitly_identified(),
        }
    }
    fn identifier_field_name(&self) -> Option<&str> {
        match self {
            Self::Concept(d) => d.identifier_field_name(),
            Self::Asset(d) => d.identifier_field_name(),
            Self::Participant(d) => d.identifier_field_name(),
            Self::Transaction(d) => d.identifier_field_name(),
            Self::Event(d) => d.identifier_field_name(),
            Self::Enum(d) => d.identifier_field_name(),
        }
    }
}

impl ClassDeclaration {
    /// Returns the declaration kind string, matching JS `declarationKind()`.
    pub fn declaration_kind(&self) -> &'static str {
        match self {
            Self::Concept(_) => "ConceptDeclaration",
            Self::Asset(_) => "AssetDeclaration",
            Self::Participant(_) => "ParticipantDeclaration",
            Self::Transaction(_) => "TransactionDeclaration",
            Self::Event(_) => "EventDeclaration",
            Self::Enum(_) => "EnumDeclaration",
        }
    }

    pub fn is_concept(&self) -> bool {
        matches!(self, Self::Concept(_))
    }
    pub fn is_asset(&self) -> bool {
        matches!(self, Self::Asset(_))
    }
    pub fn is_participant(&self) -> bool {
        matches!(self, Self::Participant(_))
    }
    pub fn is_transaction(&self) -> bool {
        matches!(self, Self::Transaction(_))
    }
    pub fn is_event(&self) -> bool {
        matches!(self, Self::Event(_))
    }
    pub fn is_enum(&self) -> bool {
        matches!(self, Self::Enum(_))
    }
}

// ===================================================================
// Dispatch on Declaration sum type
// ===================================================================

impl Decorated for Declaration {
    fn decorators(&self) -> &[mm::Decorator] {
        match self {
            Self::Class(d) => d.decorators(),
            Self::Scalar(d) => d.decorators(),
            Self::Map(d) => d.decorators(),
        }
    }
}

impl Named for Declaration {
    fn name(&self) -> &str {
        match self {
            Self::Class(d) => d.name(),
            Self::Scalar(d) => d.name(),
            Self::Map(d) => d.name(),
        }
    }
    fn location(&self) -> Option<&mm::Range> {
        match self {
            Self::Class(d) => d.location(),
            Self::Scalar(d) => d.location(),
            Self::Map(d) => d.location(),
        }
    }
}

impl Declaration {
    pub fn is_class(&self) -> bool {
        matches!(self, Self::Class(_))
    }
    pub fn is_scalar(&self) -> bool {
        matches!(self, Self::Scalar(_))
    }
    pub fn is_map(&self) -> bool {
        matches!(self, Self::Map(_))
    }

    pub fn as_class(&self) -> Option<&ClassDeclaration> {
        match self {
            Self::Class(c) => Some(c),
            _ => None,
        }
    }
    pub fn as_scalar(&self) -> Option<&ScalarDeclaration> {
        match self {
            Self::Scalar(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_map(&self) -> Option<&MapDeclaration> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }
}

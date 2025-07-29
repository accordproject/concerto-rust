use serde::{ Deserialize, Serialize };
use chrono::{ DateTime, TimeZone, Utc };

use crate::metamodel::concerto_1_0_0::*;
use crate::metamodel::utils::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "line",
   )]
   pub line: i32,

   #[serde(
      rename = "column",
   )]
   pub column: i32,

   #[serde(
      rename = "offset",
   )]
   pub offset: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "start",
   )]
   pub start: Position,

   #[serde(
      rename = "end",
   )]
   pub end: Position,

   #[serde(
      rename = "source",
      skip_serializing_if = "Option::is_none",
   )]
   pub source: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TypeIdentifier {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "namespace",
      skip_serializing_if = "Option::is_none",
   )]
   pub namespace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecoratorLiteral {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecoratorString {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "value",
   )]
   pub value: String,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecoratorNumber {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "value",
   )]
   pub value: f64,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecoratorBoolean {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "value",
   )]
   pub value: bool,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecoratorTypeReference {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "type",
   )]
   pub type_: TypeIdentifier,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decorator {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "arguments",
      skip_serializing_if = "Option::is_none",
   )]
   pub arguments: Option<Vec<DecoratorLiteral>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Identified {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IdentifiedBy {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Declaration {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MapKeyType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MapDeclaration {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "key",
   )]
   pub key: MapKeyType,

   #[serde(
      rename = "value",
   )]
   pub value: MapValueType,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StringMapKeyType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DateTimeMapKeyType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ObjectMapKeyType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "type",
   )]
   pub type_: TypeIdentifier,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BooleanMapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DateTimeMapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StringMapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IntegerMapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LongMapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DoubleMapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ObjectMapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "type",
   )]
   pub type_: TypeIdentifier,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RelationshipMapValueType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "type",
   )]
   pub type_: TypeIdentifier,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EnumDeclaration {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "properties",
   )]
   pub properties: Vec<EnumProperty>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EnumProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConceptDeclaration {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "isAbstract",
   )]
   pub is_abstract: bool,

   #[serde(
      rename = "identified",
      skip_serializing_if = "Option::is_none",
   )]
   pub identified: Option<Identified>,

   #[serde(
      rename = "superType",
      skip_serializing_if = "Option::is_none",
   )]
   pub super_type: Option<TypeIdentifier>,

   #[serde(
      rename = "properties",
   )]
   pub properties: Vec<Property>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AssetDeclaration{
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "isAbstract",
   )]
   pub is_abstract: bool,

   #[serde(
      rename = "identified",
      skip_serializing_if = "Option::is_none",
   )]
   pub identified: Option<Identified>,

   #[serde(
      rename = "superType",
      skip_serializing_if = "Option::is_none",
   )]
   pub super_type: Option<TypeIdentifier>,

   #[serde(
      rename = "properties",
   )]
   pub properties: Vec<Property>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ParticipantDeclaration {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "isAbstract",
   )]
   pub is_abstract: bool,

   #[serde(
      rename = "identified",
      skip_serializing_if = "Option::is_none",
   )]
   pub identified: Option<Identified>,

   #[serde(
      rename = "superType",
      skip_serializing_if = "Option::is_none",
   )]
   pub super_type: Option<TypeIdentifier>,

   #[serde(
      rename = "properties",
   )]
   pub properties: Vec<Property>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionDeclaration {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "isAbstract",
   )]
   pub is_abstract: bool,

   #[serde(
      rename = "identified",
      skip_serializing_if = "Option::is_none",
   )]
   pub identified: Option<Identified>,

   #[serde(
      rename = "superType",
      skip_serializing_if = "Option::is_none",
   )]
   pub super_type: Option<TypeIdentifier>,

   #[serde(
      rename = "properties",
   )]
   pub properties: Vec<Property>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventDeclaration {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "isAbstract",
   )]
   pub is_abstract: bool,

   #[serde(
      rename = "identified",
      skip_serializing_if = "Option::is_none",
   )]
   pub identified: Option<Identified>,

   #[serde(
      rename = "superType",
      skip_serializing_if = "Option::is_none",
   )]
   pub super_type: Option<TypeIdentifier>,

   #[serde(
      rename = "properties",
   )]
   pub properties: Vec<Property>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Property {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RelationshipProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "type",
   )]
   pub type_: TypeIdentifier,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ObjectProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<String>,

   #[serde(
      rename = "type",
   )]
   pub type_: TypeIdentifier,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BooleanProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<bool>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DateTimeProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StringProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<String>,

   #[serde(
      rename = "validator",
      skip_serializing_if = "Option::is_none",
   )]
   pub validator: Option<StringRegexValidator>,

   #[serde(
      rename = "lengthValidator",
      skip_serializing_if = "Option::is_none",
   )]
   pub length_validator: Option<StringLengthValidator>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StringRegexValidator {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "pattern",
   )]
   pub pattern: String,

   #[serde(
      rename = "flags",
   )]
   pub flags: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StringLengthValidator {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "minLength",
      skip_serializing_if = "Option::is_none",
   )]
   pub min_length: Option<i32>,

   #[serde(
      rename = "maxLength",
      skip_serializing_if = "Option::is_none",
   )]
   pub max_length: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DoubleProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<f64>,

   #[serde(
      rename = "validator",
      skip_serializing_if = "Option::is_none",
   )]
   pub validator: Option<DoubleDomainValidator>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DoubleDomainValidator {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "lower",
      skip_serializing_if = "Option::is_none",
   )]
   pub lower: Option<f64>,

   #[serde(
      rename = "upper",
      skip_serializing_if = "Option::is_none",
   )]
   pub upper: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IntegerProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<i32>,

   #[serde(
      rename = "validator",
      skip_serializing_if = "Option::is_none",
   )]
   pub validator: Option<IntegerDomainValidator>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IntegerDomainValidator {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "lower",
      skip_serializing_if = "Option::is_none",
   )]
   pub lower: Option<i32>,

   #[serde(
      rename = "upper",
      skip_serializing_if = "Option::is_none",
   )]
   pub upper: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LongProperty {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<i64>,

   #[serde(
      rename = "validator",
      skip_serializing_if = "Option::is_none",
   )]
   pub validator: Option<LongDomainValidator>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: bool,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: bool,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LongDomainValidator {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "lower",
      skip_serializing_if = "Option::is_none",
   )]
   pub lower: Option<i64>,

   #[serde(
      rename = "upper",
      skip_serializing_if = "Option::is_none",
   )]
   pub upper: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AliasedType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "aliasedName",
   )]
   pub aliased_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Import {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "namespace",
   )]
   pub namespace: String,

   #[serde(
      rename = "uri",
      skip_serializing_if = "Option::is_none",
   )]
   pub uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImportAll {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "namespace",
   )]
   pub namespace: String,

   #[serde(
      rename = "uri",
      skip_serializing_if = "Option::is_none",
   )]
   pub uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImportType {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "namespace",
   )]
   pub namespace: String,

   #[serde(
      rename = "uri",
      skip_serializing_if = "Option::is_none",
   )]
   pub uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImportTypes {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "types",
   )]
   pub types: Vec<String>,

   #[serde(
      rename = "aliasedTypes",
      skip_serializing_if = "Option::is_none",
   )]
   pub aliased_types: Option<Vec<AliasedType>>,

   #[serde(
      rename = "namespace",
   )]
   pub namespace: String,

   #[serde(
      rename = "uri",
      skip_serializing_if = "Option::is_none",
   )]
   pub uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "namespace",
   )]
   pub namespace: String,

   #[serde(
      rename = "sourceUri",
      skip_serializing_if = "Option::is_none",
   )]
   pub source_uri: Option<String>,

   #[serde(
      rename = "concertoVersion",
      skip_serializing_if = "Option::is_none",
   )]
   pub concerto_version: Option<String>,

   #[serde(
      rename = "imports",
      skip_serializing_if = "Option::is_none",
   )]
   pub imports: Option<Vec<Import>>,

   #[serde(
      rename = "declarations",
      skip_serializing_if = "Option::is_none",
   )]
   pub declarations: Option<Vec<Declaration>>,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Models {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "models",
   )]
   pub models: Vec<Model>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScalarDeclaration {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BooleanScalar {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<bool>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IntegerScalar {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<i32>,

   #[serde(
      rename = "validator",
      skip_serializing_if = "Option::is_none",
   )]
   pub validator: Option<IntegerDomainValidator>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LongScalar {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<i64>,

   #[serde(
      rename = "validator",
      skip_serializing_if = "Option::is_none",
   )]
   pub validator: Option<LongDomainValidator>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DoubleScalar {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<f64>,

   #[serde(
      rename = "validator",
      skip_serializing_if = "Option::is_none",
   )]
   pub validator: Option<DoubleDomainValidator>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StringScalar {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<String>,

   #[serde(
      rename = "validator",
      skip_serializing_if = "Option::is_none",
   )]
   pub validator: Option<StringRegexValidator>,

   #[serde(
      rename = "lengthValidator",
      skip_serializing_if = "Option::is_none",
   )]
   pub length_validator: Option<StringLengthValidator>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DateTimeScalar {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "defaultValue",
      skip_serializing_if = "Option::is_none",
   )]
   pub default_value: Option<String>,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "decorators",
      skip_serializing_if = "Option::is_none",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename = "location",
      skip_serializing_if = "Option::is_none",
   )]
   pub location: Option<Range>,
}


use serde::{ Deserialize, Serialize };
use chrono::{ DateTime, TimeZone, Utc };
use serde_json::Value;

use crate::metamodel::utils::*;
pub use super::concerto_metamodel_1_0_0::*;

impl Clone for Identified {
   fn clone(&self) -> Self {
      Self {
         _class: self._class.clone(),
      }
   }
}

impl Clone for StringLengthValidator {
   fn clone(&self) -> Self {
       Self {
         _class: self._class.clone(),
         min_length: self.min_length.clone(),
         max_length: self.max_length.clone(),
       }
   }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Type {
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
pub struct Validator {
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

#[derive(Clone, Debug)]
pub enum DeclarationUnion {
   Declaration(super::concerto_metamodel_1_0_0::Declaration),
   MapDeclaration(super::concerto_metamodel_1_0_0::MapDeclaration),
   EnumDeclaration(super::concerto_metamodel_1_0_0::EnumDeclaration),
   ConceptDeclaration(super::concerto_metamodel_1_0_0::ConceptDeclaration),
}

#[derive(Clone, Debug)]
pub enum ImportUnion {
   Import(super::concerto_metamodel_1_0_0::Import),
   ImportAll(super::concerto_metamodel_1_0_0::ImportAll),
   ImportType(super::concerto_metamodel_1_0_0::ImportType),
   ImportTypes(super::concerto_metamodel_1_0_0::ImportTypes),
}

// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub struct Declaration {
//    #[serde(
//       rename = "$class",
//    )]
//    pub _class: String,

//    #[serde(
//       rename = "name",
//    )]
//    pub name: String,

//    #[serde(
//       rename = "isAbstract",
//    )]
//    pub is_abstract: Option<bool>,

//    #[serde(
//       rename = "properties",
//    )]
//    pub properties: Option<Vec<Properties>>,

//    #[serde(
//       rename = "decorators",
//       skip_serializing_if = "Option::is_none",
//    )]
//    pub decorators: Option<Vec<Decorator>>,

//    #[serde(
//       rename = "location",
//       skip_serializing_if = "Option::is_none",
//    )]
//    pub location: Option<Range>,

//    #[serde(
//       rename = "superType",
//       skip_serializing_if = "Option::is_none"
//    )]
//    pub super_type: Option<SuperType>,

//    #[serde(
//       rename = "identified",
//       skip_serializing_if = "Option::is_none"
//    )]
//    pub identified: Option<Identified>,

//    #[serde(
//       rename = "value",
//       skip_serializing_if = "Option::is_none"
//    )]
//    pub value: Option<Values>,
// }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Values{
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename="type",
   )]
   pub r#type: Option<Type>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperType{
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Properties {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "name",
   )]
   pub name: String,

   #[serde(
      rename = "type",
   )]
   pub r#type: Option<Type>,

   #[serde(
      rename = "isOptional",
   )]
   pub is_optional: Option<bool>,

   #[serde(
      rename = "isArray",
   )]
   pub is_array: Option<bool>,

   #[serde(
      rename="decorators",
   )]
   pub decorators: Option<Vec<Decorator>>,

   #[serde(
      rename="validator",
   )]
   pub validator: Option<Validator>,

   #[serde(
      rename="lengthValidator",
   )]
   pub length_validator: Option<StringLengthValidator>
}



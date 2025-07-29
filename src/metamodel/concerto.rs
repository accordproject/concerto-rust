use serde::{ Deserialize, Serialize };
use chrono::{ DateTime, TimeZone, Utc };

use crate::metamodel::concerto_decorator_1_0_0::*;
use crate::metamodel::utils::*;

#[derive(Debug, Serialize, Deserialize)]
pub struct Concept {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Asset {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "$identifier",
   )]
   pub _identifier: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Participant {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "$identifier",
   )]
   pub _identifier: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Transaction {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Event {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,
}


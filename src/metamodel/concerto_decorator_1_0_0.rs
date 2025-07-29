use serde::{ Deserialize, Serialize };
use chrono::{ DateTime, TimeZone, Utc };

use crate::metamodel::concerto_1_0_0::*;
use crate::metamodel::utils::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decorator {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DotNetNamespace {
   #[serde(
      rename = "$class",
   )]
   pub _class: String,

   #[serde(
      rename = "namespace",
   )]
   pub namespace: String,
}


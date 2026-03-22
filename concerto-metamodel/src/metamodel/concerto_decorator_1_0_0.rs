use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::concerto_1_0_0::*;
use super::utils::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decorator {
    #[serde(rename = "$class")]
    pub _class: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DotNetNamespace {
    #[serde(rename = "$class")]
    pub _class: String,

    #[serde(rename = "namespace")]
    pub namespace: String,
}

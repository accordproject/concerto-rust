use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::concerto_decorator_1_0_0::*;
use super::utils::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Concept {
    #[serde(rename = "$class")]
    pub _class: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    #[serde(rename = "$class")]
    pub _class: String,

    #[serde(rename = "$identifier")]
    pub _identifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Participant {
    #[serde(rename = "$class")]
    pub _class: String,

    #[serde(rename = "$identifier")]
    pub _identifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    #[serde(rename = "$class")]
    pub _class: String,

    #[serde(
        rename = "$timestamp",
        serialize_with = "serialize_datetime",
        deserialize_with = "deserialize_datetime"
    )]
    pub _timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "$class")]
    pub _class: String,

    #[serde(
        rename = "$timestamp",
        serialize_with = "serialize_datetime",
        deserialize_with = "deserialize_datetime"
    )]
    pub _timestamp: DateTime<Utc>,
}

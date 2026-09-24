//! Serde helpers for the `DateTime` fields of the generated types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serializer};

/// Serializes a timestamp in the ISO 8601 / RFC 3339 form.
pub fn serialize_datetime<S>(datetime: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&datetime.format("%+").to_string())
}

/// Deserializes a timestamp written as `YYYY-MM-DDTHH:MM:SS.sss` plus a zone.
pub fn deserialize_datetime<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    let datetime_str = String::deserialize(deserializer)?;
    DateTime::parse_from_str(&datetime_str, "%Y-%m-%dT%H:%M:%S%.3f%Z")
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(serde::de::Error::custom)
}

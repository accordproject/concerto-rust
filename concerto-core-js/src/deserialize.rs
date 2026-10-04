//! The `DeserializeOptions` flags (accordproject/concerto#1273): the
//! populate-time flags shared by `Serializer.fromJSON` and
//! `validateMetaModel`.
//!
//! Both flags default to off, which keeps today's `fromJSON` behaviour:
//!
//! | Scenario | Default | `reject_unknown_keys` | `reject_required_null` |
//! | --- | --- | --- | --- |
//! | Unknown field = `null` | ignored | error (`UNKNOWN_PROPERTY`) | — |
//! | Unknown field = non-null | error (legacy) | error (`UNKNOWN_PROPERTY`) | — |
//! | Required field = `null` | `ResourceValidator` | — | error with path and type (`TYPE_VIOLATION`) |
//! | Optional field = `null` | skipped | skipped | skipped |
//!
//! A rejection is a `ValidationException` whose
//! [`details`](concerto_core::Error::details) lists each violation.
//! On the serializer's option bag the flags are the keys
//! `rejectUnknownKeys` and `rejectRequiredNull`; they apply while the
//! document is populated, so they hold with `validate: false` too.

use concerto_core::instance::ValidationOptions;

use crate::serializer::SerializerOptions;
use crate::value::JsValue;

/// The serializer option key for [`ValidationOptions::reject_unknown_keys`].
pub(crate) const REJECT_UNKNOWN_KEYS: &str = "rejectUnknownKeys";
/// The serializer option key for [`ValidationOptions::reject_required_null`].
pub(crate) const REJECT_REQUIRED_NULL: &str = "rejectRequiredNull";

/// The `STRICT_VALIDATE_OPTIONS` preset, both flags on: core's
/// [`ValidationOptions::STRICT`].
pub const STRICT_VALIDATE_OPTIONS: ValidationOptions = ValidationOptions::STRICT;

/// The two flags of `options` as serializer options, to pass to
/// `Serializer::from_json` (or merge into a larger option bag).
pub fn serializer_options(options: ValidationOptions) -> SerializerOptions {
    [
        (
            REJECT_UNKNOWN_KEYS.to_string(),
            JsValue::Bool(options.reject_unknown_keys),
        ),
        (
            REJECT_REQUIRED_NULL.to_string(),
            JsValue::Bool(options.reject_required_null),
        ),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests;

//! `DeserializeOptions` (accordproject/concerto#1273, task P3-02,
//! accordproject/concerto-rust#57): the populate-time flags shared by
//! `Serializer.fromJSON` and, from P3-04, `validateMetaModel`.
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
//! [`details`](crate::error::ContractError::details) lists each violation.
//! On the serializer's option bag the flags are the keys
//! `rejectUnknownKeys` and `rejectRequiredNull`; they apply while the
//! document is populated, so they hold with `validate: false` too.

use super::serializer::SerializerOptions;
use super::value::JsValue;

/// The serializer option key for [`DeserializeOptions::reject_unknown_keys`].
const REJECT_UNKNOWN_KEYS: &str = "rejectUnknownKeys";
/// The serializer option key for [`DeserializeOptions::reject_required_null`].
const REJECT_REQUIRED_NULL: &str = "rejectRequiredNull";

/// #1273's `DeserializeOptions`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeserializeOptions {
    /// `rejectUnknownKeys`: a key the declaration does not declare is an
    /// error whatever its value, `null` included (Zod `.strict()`).
    pub reject_unknown_keys: bool,
    /// `rejectRequiredNull`: a required property explicitly set to `null` is
    /// an error at once, naming its path and type, instead of being dropped
    /// and reported later by `ResourceValidator` as missing.
    pub reject_required_null: bool,
}

/// #1273's `STRICT_VALIDATE_OPTIONS` preset: both flags on.
pub const STRICT_VALIDATE_OPTIONS: DeserializeOptions = DeserializeOptions {
    reject_unknown_keys: true,
    reject_required_null: true,
};

impl DeserializeOptions {
    /// The flags as serializer options, to pass to `Serializer::from_json`
    /// (or merge into a larger option bag).
    pub fn serializer_options(self) -> SerializerOptions {
        [
            (
                REJECT_UNKNOWN_KEYS.to_string(),
                JsValue::Bool(self.reject_unknown_keys),
            ),
            (
                REJECT_REQUIRED_NULL.to_string(),
                JsValue::Bool(self.reject_required_null),
            ),
        ]
        .into_iter()
        .collect()
    }

    /// The flags a serializer option bag sets (each one truthy).
    pub(crate) fn from_serializer_options(options: &SerializerOptions) -> Self {
        let truthy = |key: &str| options.get(key).is_some_and(JsValue::is_truthy);
        Self {
            reject_unknown_keys: truthy(REJECT_UNKNOWN_KEYS),
            reject_required_null: truthy(REJECT_REQUIRED_NULL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_off() {
        assert_eq!(
            DeserializeOptions::default(),
            DeserializeOptions {
                reject_unknown_keys: false,
                reject_required_null: false,
            }
        );
        assert_eq!(
            DeserializeOptions::from_serializer_options(&SerializerOptions::new()),
            DeserializeOptions::default()
        );
    }

    #[test]
    fn strict_preset_sets_both_flags() {
        assert_eq!(
            STRICT_VALIDATE_OPTIONS,
            DeserializeOptions {
                reject_unknown_keys: true,
                reject_required_null: true,
            }
        );
    }

    #[test]
    fn serializer_options_round_trip() {
        for options in [
            DeserializeOptions::default(),
            STRICT_VALIDATE_OPTIONS,
            DeserializeOptions {
                reject_unknown_keys: true,
                reject_required_null: false,
            },
            DeserializeOptions {
                reject_unknown_keys: false,
                reject_required_null: true,
            },
        ] {
            assert_eq!(
                DeserializeOptions::from_serializer_options(&options.serializer_options()),
                options
            );
        }
    }
}

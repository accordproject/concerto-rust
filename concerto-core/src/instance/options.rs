//! [`ValidationOptions`]: the options of instance validation
//! (docs/public-api.md section 5.7), which merge the `ResourceValidator`
//! options and accordproject/concerto#1273's `DeserializeOptions`.

use super::from_json::FromJsonOptions;
use super::validate::ValidateOptions;

/// How [`ModelManager::validate_instance`](crate::ModelManager::validate_instance)
/// and its siblings check an instance.
///
/// Every option is off by default. [`ValidationOptions::STRICT`] turns on
/// the two accordproject/concerto#1273 checks. The struct is `#[non_exhaustive]`, so a caller
/// starts from a preset and sets fields:
///
/// ```
/// use concerto_core::instance::ValidationOptions;
///
/// let mut options = ValidationOptions::STRICT;
/// options.permit_resources_for_relationships = true;
/// assert!(options.reject_unknown_keys);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidationOptions {
    /// TS `convertResourcesToRelationships` (`ResourceValidator`): a
    /// relationship property may hold a resource instead of a relationship
    /// URI.
    pub convert_resources_to_relationships: bool,
    /// TS `permitResourcesForRelationships` (`ResourceValidator`): a
    /// relationship property may hold a resource instead of a relationship
    /// URI.
    pub permit_resources_for_relationships: bool,
    /// accordproject/concerto#1273 `rejectUnknownKeys`: a key that the
    /// declaration does not declare is an error whatever its value, `null`
    /// included. The error lists each key as a
    /// [`DetailCode::UnknownProperty`](crate::error::DetailCode::UnknownProperty)
    /// detail.
    pub reject_unknown_keys: bool,
    /// accordproject/concerto#1273 `rejectRequiredNull`: a required property
    /// set to `null` is an error at once, with its path and type as a
    /// [`DetailCode::TypeViolation`](crate::error::DetailCode::TypeViolation)
    /// detail, rather than a missing property found later.
    pub reject_required_null: bool,
}

impl ValidationOptions {
    /// accordproject/concerto#1273's `STRICT_VALIDATE_OPTIONS`:
    /// [`reject_unknown_keys`](Self::reject_unknown_keys) and
    /// [`reject_required_null`](Self::reject_required_null).
    pub const STRICT: Self = Self {
        convert_resources_to_relationships: false,
        permit_resources_for_relationships: false,
        reject_unknown_keys: true,
        reject_required_null: true,
    };

    /// The options of the `Serializer.fromJSON` walk the checks run as. A
    /// relationship property can hold a resource only when the populator
    /// accepts one (`acceptResourcesForRelationships`), so either
    /// relationship option turns that on as well.
    pub(crate) fn populate_options(self, validate: bool) -> FromJsonOptions {
        FromJsonOptions {
            validate,
            accept_resources_for_relationships: self.convert_resources_to_relationships
                || self.permit_resources_for_relationships,
            reject_unknown_keys: self.reject_unknown_keys,
            reject_required_null: self.reject_required_null,
            validator: self.validate_options(),
            ..FromJsonOptions::default()
        }
    }

    /// The `ResourceValidator` options.
    pub(crate) fn validate_options(self) -> ValidateOptions {
        ValidateOptions {
            convert_resources_to_relationships: self.convert_resources_to_relationships,
            permit_resources_for_relationships: self.permit_resources_for_relationships,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_off_and_strict_sets_the_1273_flags() {
        let default = ValidationOptions::default();
        assert!(!default.reject_unknown_keys && !default.reject_required_null);
        assert!(!default.convert_resources_to_relationships);
        assert!(!default.permit_resources_for_relationships);
        let strict = ValidationOptions::STRICT;
        assert!(strict.reject_unknown_keys && strict.reject_required_null);
        assert!(!strict.permit_resources_for_relationships);
    }

    #[test]
    fn a_relationship_option_lets_the_populator_accept_resources() {
        assert!(
            !ValidationOptions::default()
                .populate_options(true)
                .accept_resources_for_relationships
        );
        let options = ValidationOptions {
            permit_resources_for_relationships: true,
            ..ValidationOptions::default()
        };
        let from_json = options.populate_options(false);
        assert!(from_json.accept_resources_for_relationships);
        assert!(from_json.validator.permit_resources_for_relationships);
        assert!(!from_json.validate);
    }
}

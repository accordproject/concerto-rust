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

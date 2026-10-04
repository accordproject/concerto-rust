use super::*;
use crate::populator::from_json_options;

/// The two flags as `fromJSON` reads them from a serializer option bag.
fn read(options: &SerializerOptions) -> (bool, bool) {
    let read = from_json_options(options);
    (read.reject_unknown_keys, read.reject_required_null)
}

#[test]
fn defaults_are_off() {
    let default = ValidationOptions::default();
    assert!(!default.reject_unknown_keys && !default.reject_required_null);
    assert_eq!(read(&SerializerOptions::default()), (false, false));
}

#[test]
fn strict_preset_sets_both_flags() {
    assert_eq!(
        read(&serializer_options(STRICT_VALIDATE_OPTIONS)),
        (true, true)
    );
}

#[test]
fn serializer_options_round_trip() {
    for (unknown, required) in [(false, false), (true, true), (true, false), (false, true)] {
        let mut options = ValidationOptions::default();
        options.reject_unknown_keys = unknown;
        options.reject_required_null = required;
        assert_eq!(read(&serializer_options(options)), (unknown, required));
    }
}

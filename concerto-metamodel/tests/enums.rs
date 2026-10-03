//! Concerto enums keep the shape the published crate had before
//! concerto-codegen generated it: `Copy`, `PartialEq` and `Eq`, PascalCase
//! variants, serialised as the enum value (accordproject/concerto-rust#461).

use concerto_metamodel::org_accordproject_decoratorcommands_0_4_0::{CommandType, MapElement};

#[test]
fn concerto_enums_keep_their_api() {
    let command = CommandType::Upsert;
    let copy = command;
    assert_eq!(command, copy);
    assert_ne!(CommandType::Append, CommandType::Upsert);

    for (value, json) in [
        (MapElement::Key, "\"KEY\""),
        (MapElement::Value, "\"VALUE\""),
        (MapElement::KeyValue, "\"KEY_VALUE\""),
    ] {
        assert_eq!(serde_json::to_string(&value).unwrap(), json);
        assert_eq!(serde_json::from_str::<MapElement>(json).unwrap(), value);
    }
    assert_eq!(
        serde_json::from_str::<CommandType>("\"APPEND\"").unwrap(),
        CommandType::Append
    );
    assert!(serde_json::from_str::<CommandType>("\"Append\"").is_err());
}

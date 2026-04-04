//! Tests for map validation

use concerto_core::metamodel::concerto_metamodel_1_0_0::{
    MapDeclaration, Declaration, MapKeyType, MapValueType
};
use concerto_core::validation::Validate;

#[test]
fn test_map_with_valid_string_key() {
    let map_decl = MapDeclaration {
        _class: "concerto.metamodel@1.0.0.MapDeclaration".to_string(),
        name: "StringKeyMap".to_string(),
        key: MapKeyType {
            _class: "concerto.metamodel@1.0.0.StringMapKeyType".to_string(),
            decorators: None,
            location: None,
        },
        value: MapValueType {
            _class: "concerto.metamodel@1.0.0.IntegerMapValueType".to_string(),
            decorators: None,
            location: None,
        },
        decorators: None,
        location: None,
    };

    let declaration = Declaration::Map(map_decl);
    let result = declaration.validate();
    assert!(result.is_ok());
}

#[test]
fn test_map_with_datetime_key() {
    let map_decl = MapDeclaration {
        _class: "concerto.metamodel@1.0.0.MapDeclaration".to_string(),
        name: "DateTimeKeyMap".to_string(),
        key: MapKeyType {
            _class: "concerto.metamodel@1.0.0.DateTimeMapKeyType".to_string(),
            decorators: None,
            location: None,
        },
        value: MapValueType {
            _class: "concerto.metamodel@1.0.0.StringMapValueType".to_string(),
            decorators: None,
            location: None,
        },
        decorators: None,
        location: None,
    };

    let declaration = Declaration::Map(map_decl);
    let result = declaration.validate();
    assert!(result.is_ok());
}

#[test]
fn test_map_with_empty_name() {
    let map_decl = MapDeclaration {
        _class: "concerto.metamodel@1.0.0.MapDeclaration".to_string(),
        name: "".to_string(),
        key: MapKeyType {
            _class: "concerto.metamodel@1.0.0.StringMapKeyType".to_string(),
            decorators: None,
            location: None,
        },
        value: MapValueType {
            _class: "concerto.metamodel@1.0.0.StringMapValueType".to_string(),
            decorators: None,
            location: None,
        },
        decorators: None,
        location: None,
    };

    let declaration = Declaration::Map(map_decl);
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("name"));
}

#[test]
fn test_map_with_invalid_key_type() {
    // Object key types are not allowed — only String and DateTime
    let map_decl = MapDeclaration {
        _class: "concerto.metamodel@1.0.0.MapDeclaration".to_string(),
        name: "InvalidKeyMap".to_string(),
        key: MapKeyType {
            _class: "concerto.metamodel@1.0.0.ObjectMapKeyType".to_string(),
            decorators: None,
            location: None,
        },
        value: MapValueType {
            _class: "concerto.metamodel@1.0.0.StringMapValueType".to_string(),
            decorators: None,
            location: None,
        },
        decorators: None,
        location: None,
    };

    let declaration = Declaration::Map(map_decl);
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("map key"));
}

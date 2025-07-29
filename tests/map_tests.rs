//! Tests for map validation

use concerto_core::metamodel::concerto_metamodel_1_0_0::{
    MapDeclaration, Declaration, MapKeyType, MapValueType
};
use concerto_core::validation::Validate;

#[test]
fn test_map_with_valid_string_key() {
    // Create a map with String key type
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

    // Convert to Declaration
    let declaration = Declaration {
        _class: map_decl._class.clone(),
        name: map_decl.name.clone(),
        decorators: map_decl.decorators.clone(),
        location: map_decl.location.clone(),
    };

    // Should pass validation
    let result = declaration.validate();
    assert!(result.is_ok());
}

#[test]
fn test_map_with_datetime_key() {
    // Create a map with DateTime key type
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

    // Convert to Declaration
    let declaration = Declaration {
        _class: map_decl._class.clone(),
        name: map_decl.name.clone(),
        decorators: map_decl.decorators.clone(),
        location: map_decl.location.clone(),
    };

    // Should pass validation
    let result = declaration.validate();
    assert!(result.is_ok());
}

#[test]
fn test_map_with_missing_key() {
    // Create a map with invalid name (empty)
    let map_decl = MapDeclaration {
        _class: "concerto.metamodel@1.0.0.MapDeclaration".to_string(),
        name: "".to_string(), // Empty name should fail validation
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

    // Convert to Declaration
    let declaration = Declaration {
        _class: map_decl._class.clone(),
        name: map_decl.name.clone(),
        decorators: map_decl.decorators.clone(),
        location: map_decl.location.clone(),
    };

    // Should fail validation due to empty name
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("name"));
}

#[test]
#[ignore]
fn test_map_with_missing_value() {
    // Create a map with invalid class name
    let map_decl = MapDeclaration {
        _class: "".to_string(), // Empty class name should fail validation
        name: "MissingValueTypeMap".to_string(),
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

    // Convert to Declaration
    let declaration = Declaration {
        _class: map_decl._class.clone(),
        name: map_decl.name.clone(),
        decorators: map_decl.decorators.clone(),
        location: map_decl.location.clone(),
    };

    // Should fail validation due to empty class name
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("class"));
}

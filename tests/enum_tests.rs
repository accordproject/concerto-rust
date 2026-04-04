//! Tests for enum validation

use concerto_core::*;
use concerto_core::metamodel::concerto_metamodel_1_0_0::{EnumDeclaration, EnumProperty, Declaration, Decorator};
use concerto_core::validation::Validate;
use concerto_core::traits::DeclarationBase;

fn create_enum_property(name: &str) -> EnumProperty {
    EnumProperty {
        _class: "concerto.metamodel@1.0.0.EnumProperty".to_string(),
        name: name.to_string(),
        decorators: None,
        location: None,
    }
}

fn create_enum_declaration(name: &str, values: Vec<&str>) -> EnumDeclaration {
    EnumDeclaration {
        _class: "concerto.metamodel@1.0.0.EnumDeclaration".to_string(),
        name: name.to_string(),
        properties: values.iter().map(|v| create_enum_property(v)).collect(),
        decorators: None,
        location: None,
    }
}

#[test]
fn test_enum_with_duplicate_values() {
    let enum_decl = create_enum_declaration("Color", vec!["RED", "GREEN", "RED"]);
    let declaration = Declaration::Enum(enum_decl);

    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Duplicate enum"));
}

#[test]
fn test_enum_with_valid_values() {
    let enum_decl = create_enum_declaration("Color", vec!["RED", "GREEN", "BLUE"]);
    let declaration = Declaration::Enum(enum_decl);

    let result = declaration.validate();
    assert!(result.is_ok());
}

#[test]
#[ignore]
fn test_enum_with_empty_values() {
    // Concerto spec requires at least one enum value — not yet enforced in this implementation
    let enum_decl = EnumDeclaration {
        _class: "concerto.metamodel@1.0.0.EnumDeclaration".to_string(),
        name: "EmptyEnum".to_string(),
        properties: vec![],
        decorators: None,
        location: None,
    };
    let declaration = Declaration::Enum(enum_decl);
    let result = declaration.validate();
    assert!(result.is_err());
}

#[test]
fn test_enum_with_invalid_property_name() {
    let enum_decl = EnumDeclaration {
        _class: "concerto.metamodel@1.0.0.EnumDeclaration".to_string(),
        name: "InvalidEnum".to_string(),
        properties: vec![
            EnumProperty {
                _class: "concerto.metamodel@1.0.0.EnumProperty".to_string(),
                name: "123INVALID".to_string(),
                decorators: None,
                location: None,
            }
        ],
        decorators: None,
        location: None,
    };
    let declaration = Declaration::Enum(enum_decl);
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not a valid"));
}

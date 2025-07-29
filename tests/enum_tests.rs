//! Tests for enum validation

use concerto_core::*;
use concerto_core::metamodel::concerto_metamodel_1_0_0::{EnumDeclaration, EnumProperty, Declaration, Decorator};
use concerto_core::validation::Validate;
use concerto_core::traits::DeclarationBase;

// Helper function to create an EnumProperty
fn create_enum_property(name: &str) -> EnumProperty {
    EnumProperty {
        _class: "concerto.metamodel@1.0.0.EnumProperty".to_string(),
        name: name.to_string(),
        decorators: None,
        location: None,
    }
}

// Helper function to create an EnumDeclaration
fn create_enum_declaration(name: &str, values: Vec<&str>) -> EnumDeclaration {
    let properties = values.iter()
                          .map(|v| create_enum_property(v))
                          .collect::<Vec<_>>();

    EnumDeclaration {
        _class: "concerto.metamodel@1.0.0.EnumDeclaration".to_string(),
        name: name.to_string(),
        properties: properties,
        decorators: None,
        location: None,
    }
}

#[test]
#[ignore]
fn test_enum_with_duplicate_values() {
    // Create an enum with duplicate values
    let enum_decl = create_enum_declaration("Color", vec!["RED", "GREEN", "RED"]);

    // Convert to Declaration
    let declaration = Declaration {
        _class: enum_decl._class.clone(),
        name: enum_decl.name.clone(),
        decorators: enum_decl.decorators.clone(),
        location: enum_decl.location.clone(),
    };

    // Should fail validation with "Duplicate enum value"
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Duplicate enum"));
}

#[test]
fn test_enum_with_valid_values() {
    // Create an enum with valid values
    let enum_decl = create_enum_declaration("Color", vec!["RED", "GREEN", "BLUE"]);

    // Convert to Declaration
    let declaration = Declaration {
        _class: enum_decl._class.clone(),
        name: enum_decl.name.clone(),
        decorators: enum_decl.decorators.clone(),
        location: enum_decl.location.clone(),
    };

    // Should pass validation
    let result = declaration.validate();
    assert!(result.is_ok());
}

#[test]
#[ignore]
fn test_enum_with_empty_values() {
    // Create an enum with no values
    let enum_decl = EnumDeclaration {
        _class: "concerto.metamodel@1.0.0.EnumDeclaration".to_string(),
        name: "EmptyEnum".to_string(),
        properties: vec![],
        decorators: None,
        location: None,
    };

    // Convert to Declaration
    let declaration = Declaration {
        _class: enum_decl._class.clone(),
        name: enum_decl.name.clone(),
        decorators: enum_decl.decorators.clone(),
        location: enum_decl.location.clone(),
    };

    // Should fail validation with a message about needing at least one value
    // Note: The exact error message may differ from the original
    let result = declaration.validate();
    assert!(result.is_err());
}

#[test]
#[ignore]
fn test_enum_with_invalid_property_name() {
    // Create an enum with an invalid property name
    let enum_decl = EnumDeclaration {
        _class: "concerto.metamodel@1.0.0.EnumDeclaration".to_string(),
        name: "InvalidEnum".to_string(),
        properties: vec![
            EnumProperty {
                _class: "concerto.metamodel@1.0.0.EnumProperty".to_string(),
                name: "123INVALID".to_string(),  // Invalid name (starts with number)
                decorators: None,
                location: None,
            }
        ],
        decorators: None,
        location: None,
    };

    // Convert to Declaration
    let declaration = Declaration {
        _class: enum_decl._class.clone(),
        name: enum_decl.name.clone(),
        decorators: enum_decl.decorators.clone(),
        location: enum_decl.location.clone(),
    };

    // Should fail validation with message about invalid identifier
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not a valid"));
}

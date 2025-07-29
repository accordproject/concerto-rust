//! Basic tests for declaration validation

use concerto_core::metamodel::concerto_metamodel_1_0_0::{ConceptDeclaration, Property, Declaration};
use concerto_core::validation::Validate;

#[test]
fn test_declaration_validation() {
    // Create a valid declaration
    let concept_decl = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        super_type: None,
        properties: vec![
            Property {
                _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
                name: "firstName".to_string(),
                is_array: false,
                is_optional: false,
                decorators: None,
                location: None,
            },
            Property {
                _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
                name: "lastName".to_string(),
                is_array: false,
                is_optional: false,
                decorators: None,
                location: None,
            },
        ],
        decorators: None,
        is_abstract: false,
        identified: None,
        location: None,
    };

    // Convert ConceptDeclaration to Declaration
    let declaration = Declaration {
        _class: concept_decl._class.clone(),
        name: concept_decl.name.clone(),
        decorators: concept_decl.decorators.clone(),
        location: concept_decl.location.clone(),
    };

    // Validate the declaration
    let result = declaration.validate();
    assert!(result.is_ok());

    // Test invalid declaration (empty name)
    let invalid_concept_decl = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "".to_string(),
        super_type: None,
        properties: vec![],
        decorators: None,
        is_abstract: false,
        identified: None,
        location: None,
    };

    // Convert to Declaration
    let invalid_declaration = Declaration {
        _class: invalid_concept_decl._class.clone(),
        name: invalid_concept_decl.name.clone(),
        decorators: invalid_concept_decl.decorators.clone(),
        location: invalid_concept_decl.location.clone(),
    };

    let result = invalid_declaration.validate();
    assert!(result.is_err());
}

#[test]
fn test_property_validation() {
    // Create a valid property
    let property = Property {
        _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
        name: "firstName".to_string(),
        is_array: false,
        is_optional: false,
        decorators: None,
        location: None,
    };

    // Validate the property
    let result = property.validate();
    assert!(result.is_ok());

    // Test invalid property (empty name)
    let invalid_property = Property {
        _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
        name: "".to_string(),
        is_array: false,
        is_optional: false,
        decorators: None,
        location: None,
    };

    let result = invalid_property.validate();
    assert!(result.is_err());
}

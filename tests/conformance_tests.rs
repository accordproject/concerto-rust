//! Integration tests for validation rules

use concerto_core::model_file::ModelFile;
use concerto_core::metamodel::concerto_metamodel_1_0_0::{
    ConceptDeclaration, Declaration, Property, TypeIdentifier
};
use concerto_core::validation::Validate;
use concerto_core::model_manager::ModelManager;

#[test]
fn test_conformance_system_property_name() {
    let mut model_file = ModelFile::from_namespace("org.example".to_string(), Some("1.0.0".to_string()));

    let concept_decl = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        super_type: None,
        properties: vec![
            Property {
                _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
                name: "$class".to_string(), // system-reserved
                is_array: false,
                is_optional: false,
                decorators: None,
                location: None,
            }
        ],
        decorators: None,
        is_abstract: false,
        identified: None,
        location: None,
    };

    model_file.add_declaration(Declaration::Concept(concept_decl));

    let result = model_file.validate();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("$class") || err.contains("reserved") || err.contains("not a valid"),
        "unexpected error: {}", err);
}

#[test]
fn test_conformance_duplicate_declaration() {
    let mut model_file = ModelFile::from_namespace("org.example".to_string(), Some("1.0.0".to_string()));

    let concept_decl1 = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        super_type: None,
        properties: vec![],
        decorators: None,
        is_abstract: false,
        identified: None,
        location: None,
    };

    let concept_decl2 = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        super_type: None,
        properties: vec![],
        decorators: None,
        is_abstract: false,
        identified: None,
        location: None,
    };

    model_file.add_declaration(Declaration::Concept(concept_decl1));
    model_file.add_declaration(Declaration::Concept(concept_decl2));

    let result = model_file.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Duplicate"));
}

#[test]
fn test_conformance_circular_inheritance() {
    let mut model_manager = ModelManager::new(true);
    let mut model_file = ModelFile::from_namespace("org.example".to_string(), Some("1.0.0".to_string()));

    // Person extends Employee
    let concept_decl1 = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        super_type: Some(TypeIdentifier {
            _class: "concerto.metamodel@1.0.0.TypeIdentifier".to_string(),
            name: "Employee".to_string(),
            namespace: Some("org.example".to_string()),
        }),
        properties: vec![],
        decorators: None,
        is_abstract: false,
        identified: None,
        location: None,
    };

    // Employee extends Person (circular)
    let concept_decl2 = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Employee".to_string(),
        super_type: Some(TypeIdentifier {
            _class: "concerto.metamodel@1.0.0.TypeIdentifier".to_string(),
            name: "Person".to_string(),
            namespace: Some("org.example".to_string()),
        }),
        properties: vec![],
        decorators: None,
        is_abstract: false,
        identified: None,
        location: None,
    };

    model_file.add_declaration(Declaration::Concept(concept_decl1));
    model_file.add_declaration(Declaration::Concept(concept_decl2));

    let result = model_manager.add_model_file(model_file);
    assert!(result.is_ok());

    let result = model_manager.validate_models();
    assert!(result.is_err());
    let error_message = result.unwrap_err().to_string();
    assert!(error_message.contains("circular") || error_message.contains("Circular"));
}

#[test]
fn test_conformance_property_duplicate_names() {
    let mut model_file = ModelFile::from_namespace("org.example".to_string(), Some("1.0.0".to_string()));

    let concept_decl = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        super_type: None,
        properties: vec![
            Property {
                _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
                name: "name".to_string(),
                is_array: false,
                is_optional: false,
                decorators: None,
                location: None,
            },
            Property {
                _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
                name: "name".to_string(), // duplicate
                is_array: false,
                is_optional: false,
                decorators: None,
                location: None,
            }
        ],
        decorators: None,
        is_abstract: false,
        identified: None,
        location: None,
    };

    // Validate the ConceptDeclaration directly — it catches duplicates
    let concept_result = concept_decl.validate();
    assert!(concept_result.is_err());
    assert!(concept_result.unwrap_err().to_string().contains("Duplicate property"));

    // Validate via ModelFile — same result since Declaration::Concept dispatches to concept_decl.validate()
    model_file.add_declaration(Declaration::Concept(concept_decl));
    let file_result = model_file.validate();
    assert!(file_result.is_err());
}

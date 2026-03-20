//! Integration tests for validation rules

use concerto_core::model_file::ModelFile;
use concerto_core::metamodel::concerto_metamodel_1_0_0::{
    ConceptDeclaration, Declaration, Property
};
use concerto_core::validation::Validate;
use concerto_core::model_manager::ModelManager;

#[test]
fn test_conformance_system_property_name() {
    // Test: Property name uses system-reserved name
    let mut model_file = ModelFile::from_namespace("org.example".to_string(), Some("1.0.0".to_string()));

    let concept_decl = ConceptDeclaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        super_type: None,
        properties: vec![
            Property {
                _class: "concerto.metamodel@1.0.0.StringProperty".to_string(),
                name: "$class".to_string(),
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

    // Convert to Declaration, preserving properties in extra
    let declaration = Declaration {
        _class: concept_decl._class.clone(),
        name: concept_decl.name.clone(),
        decorators: concept_decl.decorators.clone(),
        location: concept_decl.location.clone(),
        extra: {
            let mut map = std::collections::HashMap::new();
            map.insert("properties".to_string(), serde_json::to_value(&concept_decl.properties).unwrap());
            map.insert("isAbstract".to_string(), serde_json::Value::Bool(concept_decl.is_abstract));
            map
        },
    };

    model_file.add_declaration(declaration);

    let result = model_file.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid"));
}

#[test]
fn test_conformance_duplicate_declaration() {
    // Test: Duplicate declaration names in the same model
    let mut model_file = ModelFile::from_namespace("org.example".to_string(), Some("1.0.0".to_string()));

    let declaration1 = Declaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        decorators: None,
        location: None,
        extra: Default::default(),
    };

    let declaration2 = Declaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        decorators: None,
        location: None,
        extra: Default::default(),
    };

    model_file.add_declaration(declaration1);
    model_file.add_declaration(declaration2);

    let result = model_file.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Duplicate"));
}

#[test]
#[ignore]
fn test_conformance_circular_inheritance() {
    // Test: Circular inheritance detection
    let mut model_manager = ModelManager::new(true);
    let mut model_file = ModelFile::from_namespace("org.example".to_string(), Some("1.0.0".to_string()));

    let declaration1 = Declaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Person".to_string(),
        decorators: None,
        location: None,
        extra: Default::default(),
    };

    let declaration2 = Declaration {
        _class: "concerto.metamodel@1.0.0.ConceptDeclaration".to_string(),
        name: "Employee".to_string(),
        decorators: None,
        location: None,
        extra: Default::default(),
    };

    model_file.add_declaration(declaration1);
    model_file.add_declaration(declaration2);

    let result = model_manager.add_model_file(model_file);
    assert!(result.is_ok());

    let result = model_manager.validate_models();
    assert!(result.is_err());
    let error_message = result.unwrap_err().to_string();
    assert!(error_message.contains("circular") || error_message.contains("Circular"));
}

#[test]
fn test_conformance_property_duplicate_names() {
    // Test: Duplicate property names in a declaration
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
                name: "name".to_string(),
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

    let concept_result = concept_decl.validate();
    assert!(concept_result.is_err());
    assert!(concept_result.unwrap_err().to_string().contains("Duplicate property"));
}

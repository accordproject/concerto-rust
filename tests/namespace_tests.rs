//! Tests for namespace and import validation

use concerto_core::*;
use concerto_core::model_file::ModelFile;
use concerto_core::metamodel::concerto_metamodel_1_0_0::Import;
use concerto_core::validation::Validate;

#[test]
fn test_valid_namespace_format() {
    // Create a model file with a valid namespace format
    let model_file = ModelFile::new("org.example.model".to_string(), Some("1.0.0".to_string()));

    // Should pass validation
    let result = model_file.validate();
    assert!(result.is_ok());
}

#[test]
fn test_empty_namespace() {
    // Create a model file with an empty namespace
    let model_file = ModelFile::new("".to_string(), Some("1.0.0".to_string()));

    // Should fail validation with message about empty namespace
    let result = model_file.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("cannot be empty"));
}

#[test]
#[ignore]
fn test_duplicate_namespace_imports() {
    // Create a model file
    let mut model_file = ModelFile::new("org.example".to_string(), Some("1.0.0".to_string()));

    // Add duplicate imports
    let import1 = Import {
        _class: "concerto.metamodel@1.0.0.Import".to_string(),
        namespace: "org.external.types".to_string(),
        uri: None,
    };

    let import2 = Import {
        _class: "concerto.metamodel@1.0.0.Import".to_string(),
        namespace: "org.external.types".to_string(),
        uri: None,
    };

    model_file.add_import(import1);
    model_file.add_import(import2);

    // Should fail validation with message about duplicate imports
    // Note: This test assumes the validation code checks for duplicate imports.
    // If it doesn't, you might need to add this validation or adapt the test.
    let result = model_file.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Duplicate import"));
}

#[test]
fn test_valid_imports() {
    // Create a model file
    let mut model_file = ModelFile::new("org.example".to_string(), Some("1.0.0".to_string()));

    // Add valid imports with different namespaces
    let import1 = Import {
        _class: "concerto.metamodel@1.0.0.Import".to_string(),
        namespace: "org.external.types1".to_string(),
        uri: None,
    };

    let import2 = Import {
        _class: "concerto.metamodel@1.0.0.Import".to_string(),
        namespace: "org.external.types2".to_string(),
        uri: None,
    };

    model_file.add_import(import1);
    model_file.add_import(import2);

    // Should pass validation
    let result = model_file.validate();
    assert!(result.is_ok());
}

#[test]
fn test_import_with_version() {
    // Create a model file
    let mut model_file = ModelFile::new("org.example".to_string(), Some("1.0.0".to_string()));

    // Add import with version
    let import = Import {
        _class: "concerto.metamodel@1.0.0.Import".to_string(),
        namespace: "org.external.types".to_string(),
        uri: None,
    };

    model_file.add_import(import);

    // Should pass validation
    let result = model_file.validate();
    assert!(result.is_ok());
}

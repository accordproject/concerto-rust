# Concerto Rust

A prototype Rust implementation of the Concerto modeling language, focusing on structural and semantic metamodel validation.

## Overview

This project is a partial Rust implementation of the [Concerto](https://github.com/accordproject/concerto) modeling language, which was originally developed in JavaScript. This Rust version focuses specifically on the structural and semantic validation aspects of the metamodel.

## Features

The implementation includes:

- Model file parsing and validation
- Declaration types (Asset, Concept, Enum, Scalar, Map)
- Property type validation
- Import and namespace validation
- Semantic validation based on the Concerto Conformance rules

## Project Structure

The project is organized as follows:

- `src/`
  - `lib.rs`: Main entry point exporting public modules
  - `declaration.rs`: Core declarations for data modeling
  - `model_file.rs`: Represents model files with namespace and imports management
  - `model_manager.rs`: Manages model collections and cross-model validations
  - `error.rs`: Error types for the library
  - `validation.rs`: Validation traits and implementations
  - `introspect/mod.rs`: Introspection capabilities
  - `util.rs`: Utility functions

## Testing

The test suite includes:

- `declaration_tests.rs`: Tests for declaration validation
- `conformance_tests.rs`: Tests for conformance with the specification
- `enum_tests.rs`: Tests for enum declarations and validation
- `scalar_tests.rs`: Tests for scalar declarations and validation
- `map_tests.rs`: Tests for map declarations and validation
- `namespace_tests.rs`: Tests for namespace validation

## Usage

```rust
use concerto_core::{
    ModelFile,
    Declaration,
    ModelManager,
    validation::Validate,
};

// Create a model file
let model_file = ModelFile {
    namespace: "org.example".to_string(),
    imports: vec![],
    declarations: vec![
        // Add your declarations here
    ],
};

// Validate the model file
match model_file.validate() {
    Ok(_) => println!("Model file is valid"),
    Err(e) => println!("Validation error: {}", e),
};

// Create a model manager
let mut model_manager = ModelManager::new();
model_manager.add_model_file(model_file).unwrap();

// Validate the entire model
match model_manager.validate() {
    Ok(_) => println!("Model is valid"),
    Err(e) => println!("Validation error: {}", e),
};
```

## Status

This is a partial implementation focusing on the validation aspects of Concerto. It includes:

- Basic model file structure and validation
- Declaration validation
- Cross-model validation
- Import and namespace validation
- Support for Enums, Scalars, and Maps

## License

This project is licensed under the Apache 2.0 License.

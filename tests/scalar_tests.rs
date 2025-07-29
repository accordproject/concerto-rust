//! Tests for scalar validation

use concerto_core::metamodel::concerto_metamodel_1_0_0::{
    Declaration, IntegerScalar, StringScalar,
    IntegerDomainValidator, StringRegexValidator, StringLengthValidator
};
use concerto_core::validation::Validate;

#[test]
fn test_scalar_with_valid_number_bounds() {
    // Create a scalar with valid number bounds
    let scalar_decl = IntegerScalar {
        _class: "concerto.metamodel@1.0.0.IntegerScalar".to_string(),
        name: "Percentage".to_string(),
        validator: Some(IntegerDomainValidator {
            _class: "concerto.metamodel@1.0.0.IntegerDomainValidator".to_string(),
            lower: Some(0),
            upper: Some(100),
        }),
        decorators: None,
        location: None,
        default_value: None,
    };

    // Should pass validation
    let result = scalar_decl.validate();
    assert!(result.is_ok());
}

#[test]
#[ignore]
fn test_scalar_with_invalid_number_bounds() {
    // Create a scalar with lower > upper
    let scalar_decl = IntegerScalar {
        _class: "concerto.metamodel@1.0.0.IntegerScalar".to_string(),
        name: "InvalidRange".to_string(),
        validator: Some(IntegerDomainValidator {
            _class: "concerto.metamodel@1.0.0.IntegerDomainValidator".to_string(),
            lower: Some(100),
            upper: Some(0),
        }),
        decorators: None,
        location: None,
        default_value: None,
    };

    // Create a declaration
    let declaration = Declaration {
        _class: scalar_decl._class.clone(),
        name: scalar_decl.name.clone(),
        decorators: scalar_decl.decorators.clone(),
        location: scalar_decl.location.clone(),
    };

    // Should fail validation with message about bounds
    // Note: The exact validation message may differ based on implementation
    let result = declaration.validate();
    assert!(result.is_err());
    // The error message should contain something about bounds
    assert!(result.unwrap_err().to_string().contains("bound"));
}

#[test]
fn test_scalar_with_string_validator() {
    // Create a scalar with string validator
    let scalar_decl = StringScalar {
        _class: "concerto.metamodel@1.0.0.StringScalar".to_string(),
        name: "Email".to_string(),
        validator: Some(StringRegexValidator {
            _class: "concerto.metamodel@1.0.0.StringRegexValidator".to_string(),
            pattern: "^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\\.[a-zA-Z]{2,}$".to_string(),
            flags: "".to_string(),
        }),
        decorators: None,
        location: None,
        default_value: None,
        length_validator: None,
    };

    // Create a declaration
    let declaration = Declaration {
        _class: scalar_decl._class.clone(),
        name: scalar_decl.name.clone(),
        decorators: scalar_decl.decorators.clone(),
        location: scalar_decl.location.clone(),
    };

    // Should pass validation
    let result = declaration.validate();
    assert!(result.is_ok());
}

#[test]
fn test_scalar_with_invalid_name() {
    // Create a scalar with invalid name
    let scalar_decl = StringScalar {
        _class: "concerto.metamodel@1.0.0.StringScalar".to_string(),
        name: "123Invalid".to_string(),
        validator: None,
        decorators: None,
        location: None,
        default_value: None,
        length_validator: None,
    };

    // Create a declaration
    let declaration = Declaration {
        _class: scalar_decl._class.clone(),
        name: scalar_decl.name.clone(),
        decorators: scalar_decl.decorators.clone(),
        location: scalar_decl.location.clone(),
    };

    // Should fail validation with message about invalid identifier
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not a valid identifier"));
}

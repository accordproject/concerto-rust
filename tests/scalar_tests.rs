//! Tests for scalar validation

use concerto_core::metamodel::concerto_metamodel_1_0_0::{
    Declaration, IntegerScalar, StringScalar,
    IntegerDomainValidator, StringRegexValidator, StringLengthValidator
};
use concerto_core::validation::Validate;

#[test]
fn test_scalar_with_valid_number_bounds() {
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

    let result = scalar_decl.validate();
    assert!(result.is_ok());
}

#[test]
fn test_scalar_with_invalid_number_bounds() {
    // lower > upper should fail
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

    let declaration = Declaration::IntegerScalar(scalar_decl);
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("bound"));
}

#[test]
fn test_scalar_with_string_validator() {
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

    let declaration = Declaration::StringScalar(scalar_decl);
    let result = declaration.validate();
    assert!(result.is_ok());
}

#[test]
fn test_scalar_with_invalid_name() {
    let scalar_decl = StringScalar {
        _class: "concerto.metamodel@1.0.0.StringScalar".to_string(),
        name: "123Invalid".to_string(),
        validator: None,
        decorators: None,
        location: None,
        default_value: None,
        length_validator: None,
    };

    let declaration = Declaration::StringScalar(scalar_decl);
    let result = declaration.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not a valid identifier"));
}

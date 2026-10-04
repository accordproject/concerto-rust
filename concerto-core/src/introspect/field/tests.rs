//! `field.rs` tests: `to_string` and `scalar_to_field_ast`.

mod to_string_tests {
    use super::super::*;

    #[test]
    fn matches_ts_format() {
        assert_eq!(
            to_string("name", "String", false, false),
            "Field {name=name, type=String, array=false, optional=false}"
        );
        assert_eq!(
            to_string("tags", "String", true, true),
            "Field {name=tags, type=String, array=true, optional=true}"
        );
        assert_eq!(
            to_string("code", "supp.core@1.0.0.Code", false, true),
            "Field {name=code, type=supp.core@1.0.0.Code, array=false, optional=true}"
        );
    }
}

mod scalar_to_field_ast_tests {
    use super::super::*;
    use crate::json;

    #[derive(Debug)]
    struct TestError(ContractError);

    impl From<ContractError> for TestError {
        fn from(err: ContractError) -> Self {
            Self(err)
        }
    }

    #[test]
    fn maps_every_scalar_class_to_its_property_class() {
        let cases = [
            ("StringScalar", "StringProperty"),
            ("BooleanScalar", "BooleanProperty"),
            ("DateTimeScalar", "DateTimeProperty"),
            ("DoubleScalar", "DoubleProperty"),
            ("IntegerScalar", "IntegerProperty"),
            ("LongScalar", "LongProperty"),
        ];
        for (scalar_class, property_class) in cases {
            let scalar_ast = json!({
                "$class": format!("{METAMODEL_NAMESPACE}.{scalar_class}"),
                "name": "S",
            });
            let field_ast: Value =
                scalar_to_field_ast::<TestError>(&scalar_ast, json!("myField")).unwrap();
            assert_eq!(
                field_ast["$class"],
                json!(format!("{METAMODEL_NAMESPACE}.{property_class}"))
            );
            assert_eq!(field_ast["name"], json!("myField"));
        }
    }

    #[test]
    fn errors_on_an_unrecognized_scalar_class() {
        let scalar_ast = json!({ "$class": format!("{METAMODEL_NAMESPACE}.MapScalar") });
        let err = scalar_to_field_ast::<TestError>(&scalar_ast, json!("myField")).unwrap_err();
        assert_eq!(err.0.code, "field-getscalarfield-unrecognizedtype");
    }
}

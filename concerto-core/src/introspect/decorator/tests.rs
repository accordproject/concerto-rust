use super::*;

fn decorator(json: Value) -> Decorator {
    Decorator::from_ast(&json)
}

fn ast(name: &str, arguments: Value) -> Value {
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Decorator",
        "name": name,
        "arguments": arguments
    })
}

fn string_arg(value: &str) -> Value {
    serde_json::json!({ "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": value })
}

fn number_arg(value: f64) -> Value {
    serde_json::json!({ "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": value })
}

fn boolean_arg(value: bool) -> Value {
    serde_json::json!({ "$class": "concerto.metamodel@1.0.0.DecoratorBoolean", "value": value })
}

fn type_ref_arg(name: &str, array: bool) -> Value {
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": name },
        "isArray": array
    })
}

// TS: Decorator #constructor "should store values".
#[test]
fn stores_name_and_string_arguments() {
    let d = decorator(ast(
        "Test",
        serde_json::json!([string_arg("one"), string_arg("two"), string_arg("three")]),
    ));
    assert_eq!(d.name(), "Test");
    assert_eq!(
        d.arguments(),
        &[
            DecoratorArgument::String("one".into()),
            DecoratorArgument::String("two".into()),
            DecoratorArgument::String("three".into()),
        ]
    );
}

#[test]
fn no_arguments_field_gives_an_empty_list() {
    let d = Decorator::from_ast(&serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Decorator",
        "name": "noargs"
    }));
    assert!(d.arguments().is_empty());
}

// TS: Decorators #grammar covers every literal kind and a type reference,
// array and non-array, plus a boolean literal where a type reference is
// written in the CTO source (`@returns(true)`).
#[test]
fn reads_every_argument_kind() {
    let d = decorator(ast(
        "all",
        serde_json::json!([
            string_arg("foo"),
            number_arg(1.0),
            number_arg(-1.0),
            number_arg(10.2),
            number_arg(-10.2),
            boolean_arg(false),
            boolean_arg(true),
        ]),
    ));
    assert_eq!(
        d.arguments(),
        &[
            DecoratorArgument::String("foo".into()),
            DecoratorArgument::Number(1.0),
            DecoratorArgument::Number(-1.0),
            DecoratorArgument::Number(10.2),
            DecoratorArgument::Number(-10.2),
            DecoratorArgument::Boolean(false),
            DecoratorArgument::Boolean(true),
        ]
    );

    let non_array = decorator(ast(
        "returns",
        serde_json::json!([type_ref_arg("MyConcept", false)]),
    ));
    assert_eq!(
        non_array.arguments(),
        &[DecoratorArgument::TypeReference(TypeReferenceArgument {
            name: "MyConcept".into(),
            array: Some(false)
        })]
    );

    let array = decorator(ast(
        "returns",
        serde_json::json!([type_ref_arg("MyConcept", true)]),
    ));
    assert_eq!(
        array.arguments(),
        &[DecoratorArgument::TypeReference(TypeReferenceArgument {
            name: "MyConcept".into(),
            array: Some(true)
        })]
    );

    let boolean_where_identifier_expected =
        decorator(ast("returns", serde_json::json!([boolean_arg(true)])));
    assert_eq!(
        boolean_where_identifier_expected.arguments(),
        &[DecoratorArgument::Boolean(true)]
    );
}

#[test]
fn a_short_class_is_accepted_for_the_type_reference() {
    let d = decorator(ast(
        "returns",
        serde_json::json!([{
            "$class": "DecoratorTypeReference",
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "String" },
            "isArray": false
        }]),
    ));
    assert_eq!(
        d.arguments(),
        &[DecoratorArgument::TypeReference(TypeReferenceArgument {
            name: "String".into(),
            array: Some(false)
        })]
    );
}

fn manager_with(cto_declarations: Value) -> ModelManager {
    let mut manager = ModelManager::new().expect("system models load");
    manager
        .load_model(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "declarations": cto_declarations
            }),
            None,
        )
        .expect("model loads");
    manager
}

fn decorated_concept(name: &str, decorators: Value) -> Value {
    serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
        "name": name,
        "isAbstract": false,
        "decorators": decorators,
        "properties": []
    })
}

/// Disabled by default (`DEFAULT_DECORATOR_VALIDATION`): even a decorator
/// whose name resolves nowhere at all is accepted without a manager
/// opting in, matching every "should validate" test in `decorators.js`
/// that never sets `decoratorValidation`.
#[test]
fn validate_is_a_no_op_when_disabled() {
    let manager = manager_with(serde_json::json!([decorated_concept(
        "Car",
        serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "category", "arguments": [] }])
    )]));
    let decl = manager.get_declaration("org.acme@1.0.0.Car").unwrap();
    let decorator = decl.decorator("category").unwrap();
    assert!(
        decorator
            .validate(&manager, "org.acme@1.0.0", Some("org.acme@1.0.0.Car"))
            .is_ok()
    );
}

/// TS `decorators.js` "#validate should fail to validate type refs that
/// are not defined locally": `missingDecorator: 'error'`, a decorator
/// whose own name ("category") is undeclared anywhere. The undeclared-type
/// `IllegalModelException` is thrown as it is, with no embedded
/// `IllegalModelException: ` fragment (BC-14, `Decorator::rethrow`).
#[test]
fn missing_decorator_error_reports_the_undeclared_type_wrapped_once() {
    let mut manager = manager_with(serde_json::json!([decorated_concept(
        "Car",
        serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "category", "arguments": [] }])
    )]));
    manager.set_decorator_validation(DecoratorValidationOptions {
        missing_decorator: Some("error".into()),
        invalid_decorator: None,
    });
    let decl = manager.get_declaration("org.acme@1.0.0.Car").unwrap();
    let decorator = decl.decorator("category").unwrap();
    let err = decorator
        .validate(&manager, "org.acme@1.0.0", Some("org.acme@1.0.0.Car"))
        .unwrap_err();
    let message = err.to_string();
    assert!(
        message.starts_with("Undeclared type \"category\""),
        "{message}"
    );
    assert!(!message.contains("IllegalModelException"), "{message}");
}

/// The same failure with `missingDecorator` left off: the error is
/// logged, not thrown (module doc on `Decorator::handle`).
#[test]
fn missing_decorator_off_is_silent() {
    let mut with_invalid_only = manager_with(serde_json::json!([decorated_concept(
        "Car",
        serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "category", "arguments": [] }])
    )]));
    with_invalid_only.set_decorator_validation(DecoratorValidationOptions {
        missing_decorator: None,
        invalid_decorator: Some("error".into()),
    });
    let decl = with_invalid_only
        .get_declaration("org.acme@1.0.0.Car")
        .unwrap();
    let decorator = decl.decorator("category").unwrap();
    assert!(
        decorator
            .validate(
                &with_invalid_only,
                "org.acme@1.0.0",
                Some("org.acme@1.0.0.Car")
            )
            .is_ok()
    );
}

/// A decorator whose name resolves to a real declaration: too few
/// arguments is reported through `invalidDecorator`. `missingDecorator`
/// must *also* be `'error'` for the throw to reach the caller: TS wraps
/// the whole check in one `try`/`catch`, so an `invalidDecorator` throw
/// is itself caught and re-reported through `missingDecorator` (module
/// doc on `Decorator::rethrow`) — with `missingDecorator` off, the same
/// failure is only logged (`missing_decorator_off_is_silent` covers
/// exactly that half of this behaviour, for the resolve-own-name case).
#[test]
fn too_few_arguments_is_reported_through_invalid_decorator() {
    let mut manager = manager_with(serde_json::json!([
        decorated_concept("Marker", serde_json::json!([])),
        {
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Category",
            "isAbstract": false,
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "value", "isArray": false, "isOptional": false }
            ]
        },
        decorated_concept(
            "Car",
            serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Category", "arguments": [] }])
        ),
    ]));
    manager.set_decorator_validation(DecoratorValidationOptions {
        missing_decorator: Some("error".into()),
        invalid_decorator: Some("error".into()),
    });
    let decl = manager.get_declaration("org.acme@1.0.0.Car").unwrap();
    let decorator = decl.decorator("Category").unwrap();
    let err = decorator
        .validate(&manager, "org.acme@1.0.0", Some("org.acme@1.0.0.Car"))
        .unwrap_err();
    assert!(err.to_string().contains("too few arguments"), "{err}");
}
